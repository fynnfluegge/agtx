//! Release awareness and self-replacement.
//!
//! Split the way `agent::hook_status` and `tui::dep_graph` are: `version` and
//! `check` are pure (no network, no TUI, no clock of their own) so the decision
//! logic is unit-testable, and `github`/`install` are the only files that touch
//! the world.
//!
//! Two entry points share it: the background check that draws the header notice
//! (`tui::app`) and the `agtx update` subcommand (`main.rs`).

pub mod check;
pub mod github;
pub mod install;
pub mod release;
pub mod version;

pub use check::{UpdateCache, UpdateInfo};
pub use version::Version;

use anyhow::Result;
use chrono::Utc;

/// The whole check, as run on a background thread.
///
/// Reads the cache, refreshes it over the network when the TTL has expired,
/// and compares. Every failure path yields `Ok(None)` rather than an error the
/// caller has to decide how to render — offline is the normal case, not a
/// fault.
pub fn check_for_update() -> Result<Option<UpdateInfo>> {
    let current = Version::current();
    let path = check::cache_path()?;
    let mut cache = check::load_cache(&path);

    if check::should_check(cache.as_ref(), Utc::now()) {
        match github::fetch_latest_release(&release::repo()) {
            Ok(rel) => {
                let fresh = UpdateCache {
                    last_checked: Utc::now(),
                    latest_tag: rel.tag_name,
                    html_url: rel.html_url,
                };
                // A failed write is not fatal; it just means the next launch
                // asks again.
                if let Err(e) = check::save_cache(&path, &fresh) {
                    tracing::debug!("could not write update cache: {e}");
                }
                cache = Some(fresh);
            }
            Err(e) => {
                tracing::debug!("update check failed: {e}");
                // Fall through to the stale cache: a week-old "0.2.8 is out" is
                // still true and still useful on a plane.
            }
        }
    }

    Ok(cache.as_ref().and_then(|c| check::available(&current, c)))
}
