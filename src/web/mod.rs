//! `agtx serve` — the board over HTTP.
//!
//! The server never links `App`. It reads SQLite, git and tmux, exactly as the
//! MCP server does, so it can run beside a TUI or without one. What it cannot
//! do is *execute* a transition: that machinery lives on `App` (see the mobile
//! plan's "one hard constraint"), which is why the heartbeat exists — a client
//! must be able to tell a queued tap from a completed one.

pub mod assets;
pub mod auth;
pub mod qr;
pub mod routes;
pub mod state;
pub mod tunnel;
pub mod writes;
pub mod ws;

use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};

pub use state::{ServeMode, ServerState};

/// How `agtx serve` was invoked.
pub struct ServeOptions {
    pub project_path: Option<PathBuf>,
    pub host: IpAddr,
    pub port: u16,
    /// How far to expose the board, if at all.
    pub tunnel: Option<tunnel::TunnelScope>,
    /// A pairing code supplied by the caller rather than minted here.
    ///
    /// Set when the TUI launches the server: it needs the URL — and the QR —
    /// before the child exists.
    pub pair_code: Option<String>,
}

impl Default for ServeOptions {
    fn default() -> Self {
        Self {
            project_path: None,
            // Loopback, always, until a caller says otherwise. Anyone who
            // reaches this server can read every task, diff and agent pane on
            // the machine.
            host: IpAddr::from([127, 0, 0, 1]),
            port: 8787,
            tunnel: None,
            pair_code: None,
        }
    }
}

impl ServeOptions {
    /// Whether anything beyond this machine can reach the board.
    ///
    /// A tunnel counts even though the listener stays on loopback: the provider
    /// proxies into it, so the exposure is real and pairing must be required.
    /// Reading only the bind address here is how a tunnelled server ends up
    /// unauthenticated.
    pub fn is_loopback(&self) -> bool {
        self.host.is_loopback() && self.tunnel.is_none()
    }
}

pub async fn serve(opts: ServeOptions) -> Result<()> {
    // Off-loopback needs a credential, full stop. This exposes task
    // descriptions, whole diffs, live agent panes, and an endpoint that types
    // into a running agent — on a shared network that is arbitrary code
    // execution for anyone who finds the port. The token is what makes `--host`
    // answerable; without one the honest reply is still no.
    // Loopback is its own boundary: reaching it already means being on this
    // machine, where the tmux socket and the databases are readable anyway. So
    // no pairing there — it would mean scanning a code to open a board on the
    // machine already running it, for no gain. Off-loopback, pairing is what
    // makes the bind answerable at all.
    let require_auth = !opts.is_loopback();

    // An already-paired phone must survive the move from the single shared
    // token to per-device ones. Without this it would simply stop connecting,
    // with nothing on screen to say why.
    if require_auth {
        match auth::migrate_legacy_token() {
            Ok(Some(_)) => println!("  adopted the previous access token as a paired device"),
            Ok(None) => {}
            Err(e) => tracing::warn!(error = %e, "could not migrate the legacy access token"),
        }
    }

    let mode = match &opts.project_path {
        Some(path) => {
            let path = path
                .canonicalize()
                .with_context(|| format!("resolving {}", path.display()))?;
            if !crate::git::is_git_repo(&path) {
                bail!("serve requires a git project directory: {}", path.display());
            }
            ServeMode::Project(path)
        }
        None => ServeMode::Global,
    };

    // Start the tunnel before the listener so a provider that is missing or
    // refuses fails here, rather than after printing a QR for a URL that will
    // never resolve.
    let _tunnel = match opts.tunnel {
        Some(scope) => {
            let plan = tunnel::plan(scope, opts.port, &tunnel::installed)?;
            println!("tunnel: {} — reachable from {}", plan.program, plan.reach);
            Some(tunnel::Tunnel::start(&plan)?)
        }
        None => None,
    };

    let state = ServerState::with_auth(mode, require_auth);

    // A code exists only while someone is looking at the terminal it was
    // printed in, so it is minted here rather than on demand.
    let pairing_code = if require_auth {
        Some(match &opts.pair_code {
            Some(supplied) => {
                state.pairing.seed(supplied);
                supplied.clone()
            }
            None => state.pairing.issue(),
        })
    } else {
        None
    };

    let app = routes::router(state.clone());

    let addr = SocketAddr::new(opts.host, opts.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    // The bound address rather than the requested one: port 0 is how a test or
    // a second instance asks the OS to pick, and the answer is only known here.
    let bound = listener.local_addr().unwrap_or(addr);
    print_banner(&bound, pairing_code.as_deref(), opts.is_loopback());
    tracing::info!(addr = %bound, loopback = opts.is_loopback(), "web server started");

    axum::serve(listener, app)
        .await
        .context("web server stopped")?;
    Ok(())
}

/// What to print so the URL can be typed into a phone.
///
/// The token goes in the **fragment**, which browsers do not transmit: the page
/// reads it, stores it, and strips it from the address bar. Putting it in the
/// path or the query would write the credential into every access log between
/// here and the browser.
fn print_banner(bound: &SocketAddr, pairing_code: Option<&str>, loopback: bool) {
    let host = if bound.ip().is_unspecified() {
        // `0.0.0.0` is not an address anything can open. Name a real one, or
        // the user is left to work out their own LAN address.
        local_ipv4().unwrap_or_else(|| bound.ip().to_string())
    } else {
        bound.ip().to_string()
    };

    let base = format!("http://{host}:{}", bound.port());
    let url = match pairing_code {
        Some(code) => format!("{base}/#pair={code}"),
        None => format!("{base}/"),
    };

    println!("agtx serve listening on {base}");

    // The QR is for a *phone*, so it is printed only when one could reach this
    // server. On loopback the URL resolves to the machine already running it,
    // and a code for `127.0.0.1` scans perfectly and goes nowhere.
    if !loopback {
        match qr::render(&url) {
            Some(code) => {
                println!();
                print!("{code}");
                println!();
            }
            None => tracing::warn!("could not encode the pairing URL as a QR code"),
        }
    }

    println!("  scan to pair: {url}");
    if pairing_code.is_some() {
        println!(
            "  the code is single-use and lasts {}s; already-paired devices need nothing",
            auth::PAIRING_TTL.as_secs()
        );
        println!("  reachable by anything on this network — that includes typing into your agents");
    }
}

/// This machine's first non-loopback IPv4, for the banner.
///
/// Read from the OS rather than guessed: `hostname -I` is Linux-only and
/// `ifconfig` output differs per platform, while a UDP socket "connected" to an
/// off-machine address makes the kernel pick the outbound interface without
/// sending anything.
fn local_ipv4() -> Option<String> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("192.0.2.1:80").ok()?; // TEST-NET-1: routed nowhere
    Some(sock.local_addr().ok()?.ip().to_string())
}
