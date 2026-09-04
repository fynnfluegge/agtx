//! Reaching the board from outside the local network.
//!
//! Both providers work the same way and it is why this is safe to do at all:
//! the child makes an **outbound** connection to the provider's edge and the
//! phone reaches the edge. agtx keeps listening on loopback; nothing here binds
//! a wider address, opens a port, or needs a firewall rule.
//!
//! **The two modes are not variations on one thing.** `tailscale serve` is
//! tailnet-only — reachable from anywhere, by your own signed-in devices.
//! `tailscale funnel` and `cloudflared` are the public internet. Tailscale's
//! own naming invites collapsing them, and "Tailscale, so it is private" is
//! exactly the mistake this module refuses to let a caller make: `public` has
//! to be typed in full.
//!
//! **The spawn path here is unverified.** Neither binary is installed on the
//! machine this was written on, so [`TunnelPlan`] — which provider, which
//! command, which teardown — is pure and fully tested, while starting the child
//! is not. That split is deliberate: the part that can be wrong in a quiet way
//! is the part under test.

use std::process::{Child, Command, Stdio};

use anyhow::{bail, Context, Result};

/// How far the tunnel reaches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelScope {
    /// Your tailnet only. Reachable from anywhere, by your devices alone.
    Private,
    /// The open internet, to anyone with the URL.
    Public,
}

impl TunnelScope {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "private" | "tailnet" => Some(TunnelScope::Private),
            "public" => Some(TunnelScope::Public),
            _ => None,
        }
    }
}

/// Which tool will be run, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TunnelPlan {
    pub program: String,
    pub args: Vec<String>,
    /// Run on shutdown to undo the exposure.
    ///
    /// `tailscale serve` configures the *daemon* and outlives the process that
    /// asked for it, so killing the child would leave the board exposed with
    /// nothing on screen to say so. `cloudflared` holds the tunnel itself and
    /// needs no teardown beyond dying.
    pub teardown: Option<Vec<String>>,
    pub scope: TunnelScope,
    /// What to tell the user about where this is reachable from.
    pub reach: &'static str,
}

/// Choose a provider for `scope`, given which binaries exist.
///
/// `available` is passed in rather than probed so the choice is testable
/// without installing anything.
pub fn plan(scope: TunnelScope, port: u16, available: &dyn Fn(&str) -> bool) -> Result<TunnelPlan> {
    let target = format!("http://127.0.0.1:{port}");

    match scope {
        TunnelScope::Private => {
            if !available("tailscale") {
                bail!(
                    "--tunnel private needs Tailscale, which is not installed. It is the only way \
                     to be reachable from anywhere *without* being reachable by everyone: install \
                     it from https://tailscale.com/download and sign this machine and your phone \
                     into the same tailnet."
                );
            }
            Ok(TunnelPlan {
                program: "tailscale".to_string(),
                // `serve`, never `funnel`. The difference is the whole point of
                // this arm.
                args: vec![
                    "serve".to_string(),
                    "--bg".to_string(),
                    "--https=443".to_string(),
                    target,
                ],
                teardown: Some(vec![
                    "tailscale".to_string(),
                    "serve".to_string(),
                    "--https=443".to_string(),
                    "off".to_string(),
                ]),
                scope,
                reach: "your tailnet — anywhere, but only devices signed into it",
            })
        }
        TunnelScope::Public => {
            if available("cloudflared") {
                return Ok(TunnelPlan {
                    program: "cloudflared".to_string(),
                    args: vec![
                        "tunnel".to_string(),
                        "--url".to_string(),
                        target,
                        "--no-autoupdate".to_string(),
                    ],
                    // The child *is* the tunnel; when it dies the exposure ends.
                    teardown: None,
                    scope,
                    reach: "the open internet — anyone with the URL",
                });
            }
            if available("tailscale") {
                return Ok(TunnelPlan {
                    program: "tailscale".to_string(),
                    args: vec![
                        "funnel".to_string(),
                        "--bg".to_string(),
                        "--https=443".to_string(),
                        target,
                    ],
                    teardown: Some(vec![
                        "tailscale".to_string(),
                        "funnel".to_string(),
                        "--https=443".to_string(),
                        "off".to_string(),
                    ]),
                    scope,
                    reach: "the open internet — anyone with the URL",
                });
            }
            bail!(
                "--tunnel public needs cloudflared or Tailscale, and neither is installed. \
                 Consider `--tunnel private` instead: it reaches just as far without publishing \
                 an endpoint that can type into your agents."
            )
        }
    }
}

/// A running tunnel, torn down on drop.
pub struct Tunnel {
    child: Option<Child>,
    teardown: Option<Vec<String>>,
}

impl Tunnel {
    pub fn start(plan: &TunnelPlan) -> Result<Self> {
        let child = Command::new(&plan.program)
            .args(&plan.args)
            .stdin(Stdio::null())
            // Inherited, not captured: `cloudflared` prints the hostname it was
            // assigned to stderr, and swallowing that would leave the user with
            // a running tunnel and no address for it.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()
            .with_context(|| format!("starting {}", plan.program))?;

        Ok(Self {
            // `tailscale serve --bg` returns immediately, having configured the
            // daemon; there is no long-lived child to hold. Keeping the handle
            // either way costs nothing and keeps the drop path uniform.
            child: Some(child),
            teardown: plan.teardown.clone(),
        })
    }
}

impl Drop for Tunnel {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Teardown after the kill, because for `tailscale serve` the exposure
        // lives in the daemon and would otherwise survive this process.
        if let Some(cmd) = self.teardown.take() {
            if let Some((program, args)) = cmd.split_first() {
                let _ = Command::new(program)
                    .args(args)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status();
            }
        }
    }
}

/// Whether a binary is on `PATH`.
pub fn installed(program: &str) -> bool {
    Command::new("which")
        .arg(program)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
