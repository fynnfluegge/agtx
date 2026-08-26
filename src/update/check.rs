//! When to ask GitHub, and what to do with the answer.
//!
//! Pure except for the two functions that touch the cache file. `now` is always
//! an argument, never `Utc::now()`, so the TTL is testable.

use super::version::Version;
use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// How long a check result is trusted before agtx asks again.
///
/// Not an optimisation. Unauthenticated GitHub allows 60 requests/hour **per
/// IP**, and agtx is launched dozens of times a day across many project
/// directories — an uncached check would rate-limit a single developer, let
/// alone a shared NAT. The cache is what makes a notice on every launch
/// affordable: read every time, refreshed once a day.
pub const CACHE_TTL_HOURS: i64 = 24;

/// Persisted at `~/.config/agtx/update.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCache {
    pub last_checked: DateTime<Utc>,
    /// The tag as GitHub reported it, e.g. `v0.2.8`. Stored verbatim so the
    /// download URL can be rebuilt without guessing at the `v`.
    pub latest_tag: String,
    pub html_url: String,
}

/// What the TUI and the CLI both render.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateInfo {
    pub current: Version,
    pub latest: Version,
    pub tag: String,
    pub html_url: String,
}

/// `~/.config/agtx/update.json`.
///
/// Built from `GlobalConfig::config_path()`'s parent, **not** from
/// `directories`' `config_dir()` — see the config-path split in CLAUDE.md. The
/// latter puts this in `~/Library/Application Support/` on macOS, away from the
/// `config.toml` and `logs/` it belongs next to.
pub fn cache_path() -> Result<PathBuf> {
    let config = crate::config::GlobalConfig::config_path()?;
    Ok(config
        .parent()
        .context("config path has no parent directory")?
        .join("update.json"))
}

/// A corrupt or missing cache reads as "no cache", never as an error: the only
/// consequence is one extra API call.
pub fn load_cache(path: &Path) -> Option<UpdateCache> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

pub fn save_cache(path: &Path, cache: &UpdateCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, serde_json::to_string_pretty(cache)?)?;
    Ok(())
}

/// Whether the network call is due.
pub fn should_check(cache: Option<&UpdateCache>, now: DateTime<Utc>) -> bool {
    match cache {
        None => true,
        // A `last_checked` in the future means a clock moved backwards; treat
        // it as stale rather than locking the check out until the date catches up.
        Some(c) => {
            now.signed_duration_since(c.last_checked) >= Duration::hours(CACHE_TTL_HOURS)
                || c.last_checked > now
        }
    }
}

/// Turn a cache entry into a notice, or nothing.
pub fn available(current: &Version, cache: &UpdateCache) -> Option<UpdateInfo> {
    let latest = Version::parse(&cache.latest_tag)?;
    if !latest.supersedes(current) {
        return None;
    }
    Some(UpdateInfo {
        current: current.clone(),
        latest,
        tag: cache.latest_tag.clone(),
        html_url: cache.html_url.clone(),
    })
}

/// Whether the background check should run at all.
///
/// Two opt-outs, and both are needed: the config field is for a user who never
/// wants it, the env var is for CI, the `docker/` images and the smoke runner,
/// which must not make a network call per launch.
pub fn checks_enabled(config_enabled: bool) -> bool {
    if !config_enabled {
        return false;
    }
    !matches!(
        std::env::var("AGTX_NO_UPDATE_CHECK").as_deref(),
        Ok("1") | Ok("true")
    )
}
