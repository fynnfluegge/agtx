//! `agtx serve` — the board over HTTP.
//!
//! The server never links `App`. It reads SQLite, git and tmux, exactly as the
//! MCP server does, so it can run beside a TUI or without one. What it cannot
//! do is *execute* a transition: that machinery lives on `App` (see the mobile
//! plan's "one hard constraint"), which is why the heartbeat exists — a client
//! must be able to tell a queued tap from a completed one.

pub mod assets;
pub mod routes;
pub mod state;
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
        }
    }
}

impl ServeOptions {
    pub fn is_loopback(&self) -> bool {
        self.host.is_loopback()
    }
}

pub async fn serve(opts: ServeOptions) -> Result<()> {
    // Read-only or not, this exposes task descriptions, full diffs and live
    // agent panes — the contents of the user's source tree. Authentication
    // arrives with pairing in Step 6 of the mobile plan; until then, binding
    // anywhere but loopback would publish all of that unguarded. Refusing is
    // the honest failure: the alternative is a flag that appears to work and
    // quietly has no access control behind it.
    if !opts.is_loopback() {
        bail!(
            "agtx serve binds loopback only for now: --host {} would expose task diffs and agent \
             panes with no authentication. Per-device tokens land with `--tunnel` in a later \
             release; until then reach it over an SSH tunnel:\n    \
             ssh -N -L {}:127.0.0.1:{} <this-machine>",
            opts.host,
            opts.port,
            opts.port
        );
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

    let state = ServerState::new(mode);
    let app = routes::router(state);

    let addr = SocketAddr::new(opts.host, opts.port);
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .with_context(|| format!("binding {addr}"))?;

    // The bound address rather than the requested one: port 0 is how a test or
    // a second instance asks the OS to pick, and the answer is only known here.
    let bound = listener.local_addr().unwrap_or(addr);
    println!("agtx serve listening on http://{bound}");
    tracing::info!(addr = %bound, "web server started");

    axum::serve(listener, app)
        .await
        .context("web server stopped")?;
    Ok(())
}
