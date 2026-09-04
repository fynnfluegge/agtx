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

/// A running child server, plus what a person needs to reach it.
pub struct ServeSession {
    child: Child,
    pub url: String,
    pub port: u16,
}

impl ServeSession {
    /// Start `agtx serve` bound to every interface, seeded with `code`.
    ///
    /// Off-loopback on purpose: a server the phone cannot reach is the one
    /// thing this overlay exists to avoid. That also means the child requires
    /// pairing, which the code is for.
    pub fn start(port: u16) -> Result<Self> {
        let exe = std::env::current_exe().context("locating the agtx binary")?;
        let code = uuid::Uuid::new_v4().simple().to_string();
        let host = local_ipv4().unwrap_or_else(|| "127.0.0.1".to_string());
        let url = format!("http://{host}:{port}/#pair={code}");

        let child = Command::new(exe)
            .args(["serve", "--host", "0.0.0.0", "--port"])
            .arg(port.to_string())
            .args(["--pair-code", &code])
            // The TUI owns the screen; anything the child prints would land on
            // top of the board. Its errors are surfaced by `check` instead.
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .context("starting agtx serve")?;

        Ok(Self { child, url, port })
    }

    /// Whether the child is still running.
    ///
    /// `Some(status)` means it exited — most often because the port was already
    /// taken, or this build has no `serve` feature. Either way the overlay has
    /// to stop claiming to be serving.
    pub fn check(&mut self) -> Option<std::process::ExitStatus> {
        self.child.try_wait().ok().flatten()
    }

    pub fn stop(&mut self) {
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

/// The `W` overlay's state.
pub struct MobilePopup {
    /// `None` when nothing is being served.
    pub session: Option<ServeSession>,
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
            session: None,
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

    /// Start serving, or stop if already running.
    pub fn toggle(&mut self, port: u16) {
        if self.session.is_some() {
            self.session = None; // dropping stops the child
            self.message = Some("Stopped serving.".to_string());
            return;
        }
        match ServeSession::start(port) {
            Ok(session) => {
                self.session = Some(session);
                self.message = None;
            }
            Err(e) => self.message = Some(format!("Could not start: {e}")),
        }
    }

    /// Notice a child that exited on its own — a taken port, or a build without
    /// the `serve` feature. Returns whether anything changed.
    pub fn poll_session(&mut self) -> bool {
        let exited = self.session.as_mut().and_then(|s| s.check()).is_some();
        if exited {
            self.session = None;
            self.message = Some(
                "The server stopped on its own. The port may be in use, or this build lacks \
                 the `serve` feature."
                    .to_string(),
            );
        }
        exited
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

    /// The QR module grid for the current pairing URL, if one is being served.
    ///
    /// The **grid**, not the ANSI rendering: ratatui does not interpret escape
    /// sequences, so `qr::render`'s output would be drawn as literal garbage.
    /// The overlay turns these modules into styled spans instead.
    ///
    /// Only available in a build with the `serve` feature — which is also the
    /// only build whose child could have started, so the overlay never has a
    /// URL it cannot draw.
    #[cfg(feature = "serve")]
    pub fn qr_grid(&self) -> Option<(usize, Vec<bool>)> {
        crate::web::qr::grid(&self.session.as_ref()?.url)
    }

    #[cfg(not(feature = "serve"))]
    pub fn qr_grid(&self) -> Option<(usize, Vec<bool>)> {
        None
    }
}

impl Default for MobilePopup {
    fn default() -> Self {
        Self::new()
    }
}
