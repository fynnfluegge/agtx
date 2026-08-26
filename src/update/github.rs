//! The one network call: `GET /repos/{repo}/releases/latest`.
//!
//! Uses the `curl` binary rather than an HTTP crate. `reqwest`/`ureq` would add
//! a TLS stack and a large dependency tree to a binary that has neither, for
//! one GET per day. `curl` is already a hard requirement of the only supported
//! install path (`install.sh`), ships by default on macOS and every Linux agtx
//! runs on, and — the part that matters in practice — honours `HTTPS_PROXY`,
//! `NO_PROXY` and the system CA store, which is what corporate networks need
//! and what a bundled rustls stack gets wrong. A missing `curl` is a silent
//! no-op, not an error.
//!
//! If a reason to drop the subprocess ever appears, this file is the only one
//! that changes.

use anyhow::{bail, Context, Result};
use std::process::Command;
use std::time::Duration;

/// Hard ceiling on the request. The check runs on a background thread and never
/// blocks a frame, but a hung connection should not keep a thread alive for the
/// life of the session either.
const TIMEOUT: Duration = Duration::from_secs(5);

pub struct Release {
    pub tag_name: String,
    pub html_url: String,
}

/// `None` when the network, the API or `curl` itself is unavailable — every
/// failure here is a missing notice, never a visible error.
pub fn fetch_latest_release(repo: &str) -> Result<Release> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            &TIMEOUT.as_secs().to_string(),
            "-H",
            "Accept: application/vnd.github+json",
            // GitHub rejects requests without one. curl sends its own, but
            // being explicit makes agtx traffic identifiable in rate-limit logs.
            "-H",
            &format!("User-Agent: agtx/{}", env!("CARGO_PKG_VERSION")),
            &url,
        ])
        .output()
        .context("failed to run curl (is it installed?)")?;

    if !output.status.success() {
        bail!(
            "curl failed for {}: {}",
            url,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }

    parse_release(&String::from_utf8_lossy(&output.stdout))
}

/// Split out so the JSON handling is testable without a network: a rate-limit
/// body, an HTML error page and a truncated response must all be an `Err`, not
/// a panic.
pub fn parse_release(body: &str) -> Result<Release> {
    let json: serde_json::Value =
        serde_json::from_str(body).context("release response was not JSON")?;

    let tag_name = json
        .get("tag_name")
        .and_then(|v| v.as_str())
        .context("release response has no tag_name")?
        .to_string();
    let html_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();

    Ok(Release { tag_name, html_url })
}
