//! Semver-lite parsing and comparison for agtx release tags.
//!
//! Deliberately not the `semver` crate: the only versions this ever sees are
//! the ones agtx itself produces (`Cargo.toml` and its matching `vX.Y.Z` tag,
//! enforced by the release workflow), so the grammar is ours and small. Build
//! metadata (`+meta`) is accepted and ignored; prerelease identifiers are kept
//! because the update *policy* depends on them.

use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    /// `Some("rc1")` for `0.3.0-rc1`. A prerelease sorts *before* the same
    /// numeric version without one.
    pub pre: Option<String>,
}

impl Version {
    /// Parse `0.2.7`, `v0.2.7`, `v0.3.0-rc1`, `0.2.7+build5`.
    ///
    /// Returns `None` for anything else. A remote tag agtx cannot parse is not
    /// an update — it is a log line. A version check must never be able to make
    /// the TUI shout about a tag it does not understand.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim();
        let s = s.strip_prefix('v').unwrap_or(s);
        // Build metadata does not participate in precedence (semver §10).
        let s = s.split('+').next()?;
        let (core, pre) = match s.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_string())),
            Some(_) => return None, // trailing '-' with nothing after it
            None => (s, None),
        };

        let mut parts = core.split('.');
        let major = parse_num(parts.next()?)?;
        let minor = parse_num(parts.next()?)?;
        let patch = parse_num(parts.next()?)?;
        if parts.next().is_some() {
            return None;
        }

        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    /// The version this build reports, from `Cargo.toml`.
    ///
    /// Infallible in practice: `CARGO_PKG_VERSION` is a compile-time constant
    /// and the release workflow fails the build when it disagrees with the tag.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION"))
            .expect("CARGO_PKG_VERSION is not a version agtx can parse")
    }

    pub fn is_prerelease(&self) -> bool {
        self.pre.is_some()
    }

    /// Whether `self` should be offered as an update over `current`.
    ///
    /// The one policy decision here: **a stable build is never offered a
    /// prerelease.** GitHub's `releases/latest` already excludes prereleases,
    /// so this is belt and braces for a redirected `AGTX_UPDATE_REPO` — but it
    /// is the behaviour that would be wrong to leave implicit. A prerelease
    /// build *is* offered the stable release that supersedes it, which is how a
    /// tester gets back onto the release line.
    pub fn supersedes(&self, current: &Version) -> bool {
        if self.is_prerelease() && !current.is_prerelease() {
            return false;
        }
        self > current
    }
}

fn parse_num(s: &str) -> Option<u64> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        // Numeric first, then: no prerelease outranks any prerelease, and two
        // prereleases fall back to a plain string compare (rc1 < rc2). Good
        // enough for tags we mint ourselves; nothing depends on rc2 < rc10.
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (None, Some(_)) => Ordering::Greater,
                (Some(_), None) => Ordering::Less,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{}", pre)?;
        }
        Ok(())
    }
}
