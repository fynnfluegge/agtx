//! Release artifact naming — the third copy of strings that also live in
//! `install.sh` and `.github/workflows/release.yml`.
//!
//! All three must agree character for character or `agtx update` 404s.
//! `tests/update_tests.rs` greps the other two and asserts they match these,
//! because the alternative is finding the drift in a user's failed update.

/// Overridable so tests (and a fork) can point elsewhere without a rebuild.
pub const DEFAULT_REPO: &str = "fynnfluegge/agtx";

pub fn repo() -> String {
    std::env::var("AGTX_UPDATE_REPO").unwrap_or_else(|_| DEFAULT_REPO.to_string())
}

/// The OS token used in release archive names — matches `install.sh`'s
/// `detect_os`, which is `uname -s` folded to `linux` / `darwin`.
pub fn host_os() -> Option<&'static str> {
    match std::env::consts::OS {
        "linux" => Some("linux"),
        "macos" => Some("darwin"),
        _ => None,
    }
}

/// The arch token — matches `install.sh`'s `detect_arch`.
pub fn host_arch() -> Option<&'static str> {
    match std::env::consts::ARCH {
        "x86_64" => Some("x86_64"),
        "aarch64" => Some("aarch64"),
        _ => None,
    }
}

/// `agtx-v0.2.8-aarch64-darwin.tar.gz`
///
/// `tag` is passed through verbatim (with its `v`), because that is what
/// `release.yml` interpolates from `github.ref_name`.
pub fn archive_name(tag: &str, arch: &str, os: &str) -> String {
    format!("agtx-{}-{}-{}.tar.gz", tag, arch, os)
}

pub fn download_url(repo: &str, tag: &str, archive: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/{}/{}",
        repo, tag, archive
    )
}
