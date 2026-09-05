//! Release awareness and self-replacement.
//!
//! Everything here runs without a network. The one thing it cannot cover is
//! "does the published tarball install over the running binary" — that needs
//! real artifacts, and belongs in a post-release job.

use agtx::update::check::{self, UpdateCache};
use agtx::update::install::{self, ManagedBy};
use agtx::update::{github, release, Version};
use chrono::{Duration, TimeZone, Utc};

// ---------------------------------------------------------------- version

#[test]
fn parses_the_forms_agtx_produces() {
    for (input, expected) in [
        ("0.2.7", "0.2.7"),
        ("v0.2.7", "0.2.7"),
        (" v0.2.7\n", "0.2.7"),
        ("v0.3.0-rc1", "0.3.0-rc1"),
        ("0.2.7+build5", "0.2.7"),
        ("v10.20.30", "10.20.30"),
    ] {
        let v = Version::parse(input).unwrap_or_else(|| panic!("{input} should parse"));
        assert_eq!(v.to_string(), expected, "input {input}");
    }
}

#[test]
fn rejects_what_it_cannot_understand() {
    // A remote tag agtx cannot parse must be *no update*, never a panic and
    // never a notice about a version it does not understand.
    for input in [
        "", "v", "1.2", "1.2.3.4", "latest", "v1.2.x", "1.-2.3", "v1.2.3-", "nightly",
    ] {
        assert!(
            Version::parse(input).is_none(),
            "{input:?} should not parse"
        );
    }
}

#[test]
fn orders_numerically_not_lexically() {
    let older = Version::parse("v0.9.9").unwrap();
    let newer = Version::parse("v0.10.0").unwrap();
    assert!(newer > older, "0.10.0 must outrank 0.9.9");
    assert!(Version::parse("1.0.0").unwrap() > Version::parse("0.99.99").unwrap());
    assert_eq!(
        Version::parse("0.2.7").unwrap(),
        Version::parse("v0.2.7").unwrap()
    );
}

#[test]
fn a_prerelease_is_never_offered_to_a_stable_build() {
    let stable = Version::parse("0.2.7").unwrap();
    let pre = Version::parse("0.3.0-rc1").unwrap();
    assert!(pre > stable, "it is still numerically newer");
    assert!(
        !pre.supersedes(&stable),
        "but a stable build must not be pushed onto a release candidate"
    );
}

#[test]
fn a_prerelease_build_is_offered_the_stable_that_supersedes_it() {
    // How a tester gets back onto the release line.
    let pre = Version::parse("0.3.0-rc1").unwrap();
    let stable = Version::parse("0.3.0").unwrap();
    assert!(stable.supersedes(&pre));
}

#[test]
fn same_or_older_is_not_an_update() {
    let current = Version::parse("0.2.7").unwrap();
    assert!(!Version::parse("0.2.7").unwrap().supersedes(&current));
    assert!(!Version::parse("0.2.6").unwrap().supersedes(&current));
    assert!(Version::parse("0.2.8").unwrap().supersedes(&current));
}

#[test]
fn current_version_is_parseable() {
    // Guards the `expect` in `Version::current()`: if Cargo.toml ever grows a
    // version shape this parser does not handle, fail here rather than at
    // startup in a user's terminal.
    let v = Version::current();
    assert_eq!(v.to_string(), env!("CARGO_PKG_VERSION"));
}

// ------------------------------------------------------------------ cache

fn cache_at(hours_ago: i64, tag: &str) -> UpdateCache {
    UpdateCache {
        last_checked: Utc::now() - Duration::hours(hours_ago),
        latest_tag: tag.to_string(),
        html_url: format!("https://github.com/fynnfluegge/agtx/releases/tag/{tag}"),
    }
}

#[test]
fn no_cache_means_check() {
    assert!(check::should_check(None, Utc::now()));
}

#[test]
fn fresh_cache_suppresses_the_call() {
    let cache = cache_at(1, "v0.2.8");
    assert!(!check::should_check(Some(&cache), Utc::now()));
}

#[test]
fn expired_cache_allows_the_call() {
    let cache = cache_at(check::CACHE_TTL_HOURS + 1, "v0.2.8");
    assert!(check::should_check(Some(&cache), Utc::now()));
}

#[test]
fn a_cache_from_the_future_is_stale_not_permanent() {
    // A clock that moved backwards (or a timezone mishap) must not lock the
    // check out until the calendar catches up.
    let mut cache = cache_at(0, "v0.2.8");
    cache.last_checked = Utc::now() + Duration::days(30);
    assert!(check::should_check(Some(&cache), Utc::now()));
}

#[test]
fn corrupt_cache_reads_as_no_cache() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("update.json");
    std::fs::write(&path, "{ this is not json").unwrap();
    assert!(check::load_cache(&path).is_none());
    assert!(check::should_check(None, Utc::now()));
}

#[test]
fn cache_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("update.json");
    let cache = UpdateCache {
        last_checked: Utc.with_ymd_and_hms(2026, 8, 26, 9, 0, 0).unwrap(),
        latest_tag: "v0.2.8".to_string(),
        html_url: "https://example.invalid/tag".to_string(),
    };
    check::save_cache(&path, &cache).unwrap();
    let loaded = check::load_cache(&path).unwrap();
    assert_eq!(loaded.latest_tag, "v0.2.8");
    assert_eq!(loaded.last_checked, cache.last_checked);
}

#[test]
fn cache_lives_next_to_config_toml_not_in_the_data_dir() {
    // The config-path split in CLAUDE.md: `directories`' config_dir() puts this
    // in ~/Library/Application Support on macOS, away from config.toml and
    // logs/. It must derive from GlobalConfig::config_path()'s parent instead.
    let path = check::cache_path().unwrap();
    let config = agtx::config::GlobalConfig::config_path().unwrap();
    assert_eq!(path.parent(), config.parent());
    assert_eq!(path.file_name().unwrap(), "update.json");
}

#[test]
fn available_compares_against_the_cached_tag() {
    let current = Version::parse("0.2.7").unwrap();
    assert!(check::available(&current, &cache_at(1, "v0.2.8")).is_some());
    assert!(check::available(&current, &cache_at(1, "v0.2.7")).is_none());
    assert!(check::available(&current, &cache_at(1, "v0.2.6")).is_none());
    // An unparseable tag is not an update.
    assert!(check::available(&current, &cache_at(1, "nightly")).is_none());
}

#[test]
fn config_and_env_each_suppress_the_check() {
    // Not run in parallel with anything that reads the same var: set/remove is
    // process-global, so the assertions bracket it tightly.
    std::env::remove_var("AGTX_NO_UPDATE_CHECK");
    assert!(check::checks_enabled(true));
    assert!(!check::checks_enabled(false), "config field must win");

    std::env::set_var("AGTX_NO_UPDATE_CHECK", "1");
    assert!(!check::checks_enabled(true), "env var must win");
    std::env::remove_var("AGTX_NO_UPDATE_CHECK");
}

// ----------------------------------------------------------------- github

#[test]
fn parses_a_real_releases_latest_payload() {
    let body = r#"{
        "url": "https://api.github.com/repos/fynnfluegge/agtx/releases/1",
        "html_url": "https://github.com/fynnfluegge/agtx/releases/tag/v0.2.8",
        "tag_name": "v0.2.8",
        "name": "v0.2.8",
        "draft": false,
        "prerelease": false,
        "assets": []
    }"#;
    let rel = github::parse_release(body).unwrap();
    assert_eq!(rel.tag_name, "v0.2.8");
    assert_eq!(
        rel.html_url,
        "https://github.com/fynnfluegge/agtx/releases/tag/v0.2.8"
    );
}

#[test]
fn a_non_release_body_is_an_error_not_a_panic() {
    // The three shapes GitHub actually returns when something is wrong.
    let rate_limited =
        r#"{"message":"API rate limit exceeded","documentation_url":"https://docs.github.com"}"#;
    assert!(github::parse_release(rate_limited).is_err());
    assert!(github::parse_release("<html><body>502</body></html>").is_err());
    assert!(github::parse_release(r#"{"tag_name": "v0.2."#).is_err());
    assert!(github::parse_release("").is_err());
}

// ---------------------------------------------------------------- release

#[test]
fn archive_names_match_the_four_published_targets() {
    for (arch, os, expected) in [
        ("aarch64", "darwin", "agtx-v0.2.8-aarch64-darwin.tar.gz"),
        ("x86_64", "darwin", "agtx-v0.2.8-x86_64-darwin.tar.gz"),
        ("x86_64", "linux", "agtx-v0.2.8-x86_64-linux.tar.gz"),
        ("aarch64", "linux", "agtx-v0.2.8-aarch64-linux.tar.gz"),
    ] {
        assert_eq!(release::archive_name("v0.2.8", arch, os), expected);
    }
}

#[test]
fn this_host_maps_to_a_published_target() {
    // Every platform agtx builds for must resolve; anything else must be a
    // clean "no release for this host" rather than a wrong URL.
    if cfg!(any(target_os = "linux", target_os = "macos")) {
        assert!(release::host_os().is_some());
    }
    if cfg!(any(target_arch = "x86_64", target_arch = "aarch64")) {
        assert!(release::host_arch().is_some());
    }
}

#[test]
fn download_url_points_at_the_tag() {
    assert_eq!(
        release::download_url("fynnfluegge/agtx", "v0.2.8", "agtx-v0.2.8-x86_64-linux.tar.gz"),
        "https://github.com/fynnfluegge/agtx/releases/download/v0.2.8/agtx-v0.2.8-x86_64-linux.tar.gz"
    );
}

/// The repo slug and the archive naming exist in three places: here,
/// `install.sh`, and `release.yml`. They must agree character for character or
/// `agtx update` 404s — and the drift would otherwise be found by a user whose
/// update silently fails, not by CI.
#[test]
fn naming_matches_install_sh() {
    let sh = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/install.sh")).unwrap();
    assert!(
        sh.contains(&format!("REPO=\"{}\"", release::DEFAULT_REPO)),
        "install.sh REPO does not match update::release::DEFAULT_REPO"
    );
    // install.sh builds: ${BINARY_NAME}-${VERSION}-${ARCH}-${OS}.tar.gz
    assert!(
        sh.contains(r#"ARCHIVE_NAME="${BINARY_NAME}-${VERSION}-${ARCH}-${OS}.tar.gz""#),
        "install.sh archive naming changed — update::release::archive_name must follow"
    );
    // and the os/arch tokens the name interpolates
    for token in [
        "echo \"linux\"",
        "echo \"darwin\"",
        "echo \"x86_64\"",
        "echo \"aarch64\"",
    ] {
        assert!(sh.contains(token), "install.sh no longer emits {token}");
    }
}

#[test]
fn naming_matches_release_workflow() {
    let yml = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/.github/workflows/release.yml"
    ))
    .unwrap();
    for (arch, os) in [
        ("aarch64", "darwin"),
        ("x86_64", "darwin"),
        ("x86_64", "linux"),
        ("aarch64", "linux"),
    ] {
        // release.yml interpolates the tag as ${{ github.ref_name }}
        let expected = release::archive_name("${{ github.ref_name }}", arch, os);
        assert!(
            yml.contains(&expected),
            "release.yml does not publish {expected}"
        );
    }
    assert!(
        yml.contains(".sha256"),
        "release.yml must publish checksums — `agtx update` and install.sh both verify them"
    );
    assert!(
        yml.contains("Tag matches Cargo.toml"),
        "the tag/manifest guard is what keeps `agtx --version` honest"
    );
}

// ---------------------------------------------------------------- install

#[test]
fn package_managed_binaries_are_refused() {
    use std::path::Path;
    assert_eq!(
        install::managed_by(Path::new("/nix/store/abc123-agtx-0.2.7/bin/agtx")),
        Some(ManagedBy::Nix)
    );
    assert_eq!(
        install::managed_by(Path::new("/opt/homebrew/bin/agtx")),
        Some(ManagedBy::Homebrew)
    );
    assert_eq!(
        install::managed_by(Path::new("/usr/local/Cellar/agtx/0.2.7/bin/agtx")),
        Some(ManagedBy::Homebrew)
    );
    assert_eq!(
        install::managed_by(Path::new("/home/linuxbrew/.linuxbrew/bin/agtx")),
        Some(ManagedBy::Homebrew)
    );
    // The path install.sh actually uses, and a plain system path, are ours.
    assert_eq!(
        install::managed_by(Path::new("/Users/x/.local/bin/agtx")),
        None
    );
    assert_eq!(install::managed_by(Path::new("/usr/local/bin/agtx")), None);
}

#[test]
fn managed_advice_names_the_right_tool() {
    assert!(ManagedBy::Homebrew.advice().contains("brew upgrade"));
    assert!(ManagedBy::Nix.advice().contains("Nix"));
}

/// The digest of the empty input — the standard SHA-256 test vector.
const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

#[test]
fn parses_the_checksum_format_both_shasum_tools_write() {
    // sha256sum and `shasum -a 256` both write "<hash>  <name>"
    assert_eq!(
        install::parse_sha256_file(&format!(
            "{EMPTY_SHA256}  agtx-v0.2.8-x86_64-linux.tar.gz\n"
        )),
        Some(EMPTY_SHA256.to_string())
    );
    // Uppercase is normalised.
    assert_eq!(
        install::parse_sha256_file(&format!("{}  x", EMPTY_SHA256.to_uppercase())),
        Some(EMPTY_SHA256.to_string())
    );
    // A 404 page, an empty file and a truncated hash are all "no checksum".
    assert!(install::parse_sha256_file("Not Found").is_none());
    assert!(install::parse_sha256_file("").is_none());
    assert!(install::parse_sha256_file(&format!("{}  x", &EMPTY_SHA256[..63])).is_none());
}

#[test]
fn sha256_matches_a_known_digest() {
    assert_eq!(install::sha256_hex(b""), EMPTY_SHA256);
}

#[test]
fn replace_binary_swaps_and_cleans_up() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("agtx");
    let new = dir.path().join("agtx-new");
    std::fs::write(&target, b"old").unwrap();
    std::fs::write(&new, b"new").unwrap();

    install::replace_binary(&new, &target).unwrap();

    assert_eq!(std::fs::read(&target).unwrap(), b"new");
    assert!(!new.exists(), "the staged binary was moved, not copied");
    assert!(
        !target.with_extension("old").exists(),
        "the backup must be removed on success"
    );
}

#[test]
fn replace_binary_works_when_no_target_exists_yet() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("agtx");
    let new = dir.path().join("agtx-new");
    std::fs::write(&new, b"new").unwrap();

    install::replace_binary(&new, &target).unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), b"new");
}

#[test]
fn a_failed_swap_leaves_the_old_binary_in_place() {
    // The reason the backup step exists: if the second rename fails, the user
    // must still have a working agtx rather than no agtx at all.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("agtx");
    std::fs::write(&target, b"old").unwrap();
    let missing = dir.path().join("does-not-exist");

    assert!(install::replace_binary(&missing, &target).is_err());
    assert_eq!(
        std::fs::read(&target).unwrap(),
        b"old",
        "the original must be restored when the new binary cannot be moved in"
    );
}

#[test]
fn target_path_resolves_to_a_real_file() {
    let path = install::target_path().unwrap();
    assert!(
        path.exists(),
        "current_exe must resolve: {}",
        path.display()
    );
}

/// A verification that passes against a real release proves only the happy
/// path — a real release always matches. That a *corrupted* download aborts
/// can only be shown here.
#[test]
fn a_corrupted_archive_is_rejected() {
    // The real v1.0.0 x86_64-darwin checksum file, verbatim.
    let published = "b169b83adb8e837f8d5d8f6eb77d15101848109ec6f45217b5d32d6a77445dd3  agtx-v1.0.0-x86_64-darwin.tar.gz\n";
    let archive = "agtx-v1.0.0-x86_64-darwin.tar.gz";

    let err = install::verify_checksum(b"not the release tarball", published, archive)
        .expect_err("a body that does not hash to the published digest must abort");
    let msg = err.to_string();
    assert!(msg.contains("checksum mismatch"), "{msg}");
    assert!(
        msg.contains("b169b83adb8e837f8d5d8f6eb77d15101848109ec6f45217b5d32d6a77445dd3"),
        "the message must name the expected digest: {msg}"
    );
}

#[test]
fn a_matching_archive_passes() {
    let body = b"pretend tarball";
    let sums = format!(
        "{}  agtx-v1.0.0-x86_64-linux.tar.gz\n",
        install::sha256_hex(body)
    );
    install::verify_checksum(body, &sums, "agtx-v1.0.0-x86_64-linux.tar.gz").unwrap();
}

#[test]
fn a_checksum_file_that_is_not_one_aborts_rather_than_skipping() {
    // A 404 body reaching this point means the file existed but is garbage.
    // Skipping verification on unreadable input would defeat the check.
    assert!(install::verify_checksum(b"x", "Not Found", "a.tar.gz").is_err());
    assert!(install::verify_checksum(b"x", "", "a.tar.gz").is_err());
}
