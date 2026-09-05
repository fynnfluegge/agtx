//! Running `agtx serve` as a child of the TUI, and the overlay that drives it.
//!
//! **Why the TUI mints the pairing code.** The obvious alternative is to let
//! the server mint one and parse it back out of the child's stdout, which makes
//! the banner's wording load-bearing and breaks silently when it changes.
//! Passing `--pair-code` in the other direction means the TUI already knows the
//! URL before the child has started, so it can draw the QR immediately and
//! never has to read the child at all.
//!
//! **Why the child is killed on drop.** agtx has never held a long-running
//! child of its own — tmux windows outlive it deliberately, and agents live in
//! those windows. A server that outlived the TUI would hold its port with
//! nothing on screen owning it, which surfaces a week later as "why does serve
//! say address in use".

use std::net::IpAddr;
use std::process::{Child, Command, Stdio};

use anyhow::{Context, Result};

/// Default port for the TUI-launched server.
///
/// The same one `agtx serve` uses on its own, so a device paired through the
/// overlay keeps working against a hand-started server and the other way round.
pub const DEFAULT_PORT: u16 = 8787;

/// How far the served board should reach.
///
/// Only these two. `--tunnel public` exists on the CLI and is deliberately not
/// offered behind a keypress: it publishes an endpoint that can type into a
/// running agent, and that should cost more than tapping `t` twice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// Bound to every interface. Works on this wifi; a private address is
    /// unroutable from mobile data.
    Lan,
    /// `tailscale serve` into a loopback listener. Anywhere, but only devices
    /// signed into the same tailnet.
    Tailnet,
}

impl Reach {
    pub fn label(&self) -> &'static str {
        match self {
            Reach::Lan => "local network",
            Reach::Tailnet => "tailnet",
        }
    }

    /// What a person gets, in the words the overlay offers it in.
    ///
    /// Kept short enough to fit beside the key and the label without the box
    /// having to grow: the overlay draws unwrapped, so a long phrase here is
    /// silently cut rather than reflowed.
    pub fn describe(&self) -> &'static str {
        match self {
            Reach::Lan => "this wifi only, not mobile data",
            Reach::Tailnet => "anywhere, your devices only",
        }
    }

    /// Why this option cannot be used right now, if it cannot.
    ///
    /// Checked before starting rather than after: a child that exits because
    /// Tailscale is missing surfaces as "the server stopped on its own", which
    /// says nothing about what to install.
    #[cfg(feature = "serve")]
    pub fn unavailable(&self) -> Option<&'static str> {
        match self {
            Reach::Lan => None,
            Reach::Tailnet if !crate::web::tunnel::installed("tailscale") => {
                Some("install Tailscale and sign this machine in")
            }
            Reach::Tailnet if crate::web::tunnel::tailnet_hostname().is_none() => {
                Some("Tailscale is installed but not signed in")
            }
            Reach::Tailnet => None,
        }
    }

    /// A build without the feature can never serve, so both options say so —
    /// and say what to do about it. "This build has no web server" is true and
    /// useless: the reader is left to guess whether that is a bug, a missing
    /// dependency, or something they can fix.
    #[cfg(not(feature = "serve"))]
    pub fn unavailable(&self) -> Option<&'static str> {
        Some("rebuild with `cargo build --release --features serve`")
    }
}

/// A running child server, plus what a person needs to reach it.
pub struct ServeSession {
    child: Child,
    pub url: String,
    pub port: u16,
    pub reach: Reach,
}

impl ServeSession {
    /// Start `agtx serve` for `reach`, seeded with a fresh pairing code.
    ///
    /// The two modes differ in more than a flag. LAN binds every interface and
    /// the URL is this machine's private address. A tunnel leaves the listener
    /// on loopback — Tailscale proxies into it — and the URL is the **tailnet
    /// hostname**, which agtx does not choose and therefore has to ask for.
    /// Both require pairing: `ServeOptions::is_loopback` counts a tunnel as
    /// exposure even though the bind address says otherwise.
    pub fn start(port: u16, reach: Reach) -> Result<Self> {
        let exe = std::env::current_exe().context("locating the agtx binary")?;
        let code = uuid::Uuid::new_v4().simple().to_string();

        let mut args: Vec<String> = vec!["serve".into(), "--port".into(), port.to_string()];
        let url = match reach {
            Reach::Lan => {
                args.push("--host".into());
                args.push("0.0.0.0".into());
                let host = local_ipv4().unwrap_or_else(|| "127.0.0.1".to_string());
                format!("http://{host}:{port}/#pair={code}")
            }
            Reach::Tailnet => {
                args.push("--tunnel".into());
                args.push("private".into());
                let host = tailnet_host().context(
                    "could not read this machine's tailnet name from `tailscale status`",
                )?;
                // https, because that is what `tailscale serve --https=443`
                // publishes — and the port is the tunnel's, not the listener's.
                format!("https://{host}/#pair={code}")
            }
        };
        args.push("--pair-code".into());
        args.push(code);

        let child = Command::new(exe)
            .args(&args)
            // The TUI owns the screen, so the child's own output must not land
            // on top of the board — but its *errors* are the only place some
            // things are explained. A failed tunnel prints the one-time link
            // that enables Tailscale Serve, and discarding that leaves the
            // overlay able to say only "it stopped", which helps nobody.
            //
            // The overlay only starts LAN or private Tailscale serving, whose
            // stderr is limited to startup errors; the banner and QR use stdout.
            // Before adding public tunnels here, drain stderr continuously:
            // cloudflared forwards ongoing logs through agtx serve's stderr,
            // and waiting until exit would fill this pipe and block the tunnel.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("starting agtx serve")?;

        Ok(Self {
            child,
            url,
            port,
            reach,
        })
    }

    /// Whether the child is still running.
    ///
    /// `Some(status)` means it exited — most often because the port was already
    /// taken, or this build has no `serve` feature. Either way the overlay has
    /// to stop claiming to be serving.
    /// Whether this URL only resolves inside the local network.
    ///
    /// True for the private ranges — 10/8, 172.16/12, 192.168/16 and the
    /// link-local 169.254/16 — plus loopback. A phone on mobile data has no
    /// route to any of them, so a QR carrying one is a code that scans
    /// perfectly and then times out.
    pub fn is_lan_only(&self) -> bool {
        Self::lan_only_url(&self.url)
    }

    /// Split out so it can be tested against the addresses that matter without
    /// starting a server for each one.
    pub fn lan_only_url(url: &str) -> bool {
        let host = url
            .trim_start_matches("http://")
            .trim_start_matches("https://")
            .split(['/', ':'])
            .next()
            .unwrap_or_default();
        match host.parse::<std::net::Ipv4Addr>() {
            Ok(ip) => ip.is_private() || ip.is_loopback() || ip.is_link_local(),
            // A hostname rather than an address means a tunnel put it there.
            Err(_) => false,
        }
    }

    /// A session wrapping an arbitrary long-lived child, for tests.
    ///
    /// The ownership rule this exists to pin — that closing the overlay must
    /// not stop the server — is only observable with a session present, and the
    /// test that claimed to check it had none. It could never have failed.
    #[cfg(feature = "test-mocks")]
    pub fn for_test(child: Child, url: &str) -> Self {
        Self {
            child,
            url: url.to_string(),
            port: DEFAULT_PORT,
            reach: Reach::Lan,
        }
    }

    pub fn check(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    /// Whatever the child wrote to stderr before dying.
    ///
    /// Only meaningful once it has exited — reading earlier would block on a
    /// pipe nobody is finished writing to.
    pub fn take_stderr(&mut self) -> String {
        use std::io::Read;
        let Some(mut err) = self.child.stderr.take() else {
            return String::new();
        };
        let mut buf = String::new();
        let _ = err.read_to_string(&mut buf);
        buf.trim().to_string()
    }

    pub fn stop(&mut self) {
        // `Child::kill` sends SIGKILL, which cannot be caught — and the child
        // has cleanup that matters: a tunnel configures the Tailscale daemon,
        // so the exposure outlives the process unless it is torn down. Ask
        // politely first, and only insist if it does not go.
        //
        // Shelling out to `kill` rather than adding a libc dependency for one
        // signal, which is the same trade agtx already makes for tmux and git.
        let _ = Command::new("kill")
            .args(["-TERM", &self.child.id().to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        // Up to two seconds, polled: a tunnel teardown is a round trip to the
        // local daemon, not an instant thing.
        for _ in 0..20 {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }

        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ServeSession {
    fn drop(&mut self) {
        self.stop();
    }
}

/// This machine's first non-loopback IPv4.
///
/// Read from the OS rather than parsed out of `ifconfig`: a UDP socket
/// "connected" to an off-machine address makes the kernel pick the outbound
/// interface without sending anything.
pub fn local_ipv4() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?; // TEST-NET-1: routed nowhere
    let ip = sock.local_addr().ok()?.ip();
    match ip {
        IpAddr::V4(v4) if !v4.is_loopback() => Some(v4.to_string()),
        _ => None,
    }
}

#[cfg(feature = "serve")]
fn tailnet_host() -> Option<String> {
    crate::web::tunnel::tailnet_hostname()
}

#[cfg(not(feature = "serve"))]
fn tailnet_host() -> Option<String> {
    None
}

/// The `W` overlay's state.
pub struct MobilePopup {
    /// Devices as of the last refresh, so the list does not hit the database
    /// on every frame.
    pub devices: Vec<crate::db::MobileDevice>,
    pub selected: usize,
    /// A one-line result of the last action, shown under the list.
    pub message: Option<String>,
}

impl MobilePopup {
    pub fn new() -> Self {
        let mut popup = Self {
            devices: Vec::new(),
            selected: 0,
            message: None,
        };
        popup.reload_devices();
        popup
    }

    pub fn reload_devices(&mut self) {
        self.devices = crate::db::Database::open_global()
            .and_then(|db| db.list_mobile_devices())
            .unwrap_or_default();
        self.selected = self.selected.min(self.devices.len().saturating_sub(1));
    }

    /// Stop serving if running; otherwise start with the given reach.
    ///
    /// Each key does a whole thing — `s` serves to this wifi, `t` serves via
    /// the tailnet — rather than one key setting a mode another key acts on.
    /// A hidden mode means neither label can say what pressing it will
    /// actually do, and the answer to "why can my phone not see this" was one
    /// keypress of invisible state away.
    pub fn serve(&mut self, session: &mut Option<ServeSession>, port: u16, reach: Reach) {
        if session.is_some() {
            *session = None; // dropping stops the child
            self.message = Some("Stopped serving.".to_string());
            return;
        }
        if let Some(why) = reach.unavailable() {
            self.message = Some(format!("Cannot serve to the {}: {why}.", reach.label()));
            return;
        }
        match ServeSession::start(port, reach) {
            Ok(started) => {
                *session = Some(started);
                self.message = None;
            }
            Err(e) => self.message = Some(format!("Could not start: {e}")),
        }
    }

    /// Notice a child that exited on its own — a taken port, or a build without
    /// the `serve` feature. Returns whether anything changed.
    pub fn poll_session(&mut self, session: &mut Option<ServeSession>) -> bool {
        let exited = session.as_mut().and_then(|s| s.check()).is_some();
        if !exited {
            return false;
        }

        // Relay what the child said. It knows things the TUI does not — that
        // the port is taken, or that Tailscale Serve is disabled for this
        // tailnet and the one-time link that enables it.
        let detail = session
            .as_mut()
            .map(|s| s.take_stderr())
            .unwrap_or_default();
        *session = None;

        self.message = Some(if detail.is_empty() {
            "The server stopped. The port may be in use, or this build lacks the `serve` feature."
                .to_string()
        } else {
            // Last line first: anyhow prints `Error: <context>` then the cause,
            // and the cause is the part that says what to do.
            let line = detail.lines().rfind(|l| !l.trim().is_empty());
            format!("Server stopped: {}", line.unwrap_or(&detail))
        });
        true
    }

    pub fn revoke_selected(&mut self) {
        let Some(device) = self.devices.get(self.selected).cloned() else {
            return;
        };
        let revoked = crate::db::Database::open_global()
            .and_then(|db| db.revoke_mobile_device(&device.id))
            .unwrap_or(false);
        self.message = Some(if revoked {
            format!(
                "Revoked “{}”. It must scan a new code to return.",
                device.label
            )
        } else {
            format!("Could not revoke “{}”.", device.label)
        });
        self.reload_devices();
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.devices.is_empty() {
            return;
        }
        let last = self.devices.len() - 1;
        self.selected = match delta {
            d if d < 0 => self.selected.saturating_sub(d.unsigned_abs()),
            d => (self.selected + d as usize).min(last),
        };
    }
}

/// The QR module grid for a pairing URL.
///
/// The **grid**, not the ANSI rendering: ratatui does not interpret escape
/// sequences, so `qr::render`'s output would be drawn as literal garbage. The
/// overlay turns these modules into styled spans instead.
#[cfg(feature = "serve")]
pub fn qr_grid(url: &str) -> Option<(usize, Vec<bool>)> {
    crate::web::qr::grid(url)
}

#[cfg(not(feature = "serve"))]
pub fn qr_grid(_url: &str) -> Option<(usize, Vec<bool>)> {
    None
}

impl Default for MobilePopup {
    fn default() -> Self {
        Self::new()
    }
}
