//! Tests for [`super`] — agent workspace-trust discovery and seeding.
//!
//! Every store shape here is a reduction of a real file measured on a live
//! machine, not an invention: the gemini map really is lowercased, antigravity
//! really does hold two spellings of the same directory, and codex's config
//! really is a general TOML file with unrelated sections in it.

use super::*;
use std::fs;

fn home() -> tempfile::TempDir {
    tempfile::tempdir().unwrap()
}

fn write(home: &Path, rel: &str, body: &str) {
    let p = home.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

// ---------------------------------------------------------------- claude ----

#[test]
fn claude_trust_is_inherited_from_an_ancestor() {
    let h = home();
    write(
        h.path(),
        ".claude.json",
        r#"{"projects":{"/proj":{"hasTrustDialogAccepted":true}}}"#,
    );
    assert_eq!(
        status("claude", Path::new("/proj/.agtx/worktrees/w1"), h.path()),
        Trust::Trusted
    );
}

#[test]
fn claude_untrusted_project_does_not_cover_its_worktrees() {
    let h = home();
    write(
        h.path(),
        ".claude.json",
        r#"{"projects":{"/proj":{"hasTrustDialogAccepted":false}}}"#,
    );
    assert_eq!(
        status("claude", Path::new("/proj"), h.path()),
        Trust::Untrusted
    );
    assert_eq!(
        status("claude", Path::new("/proj/wt"), h.path()),
        Trust::Untrusted
    );
}

/// The real file records both answers; only `true` is consent.
#[test]
fn claude_reads_the_flag_not_merely_the_entry() {
    let h = home();
    write(
        h.path(),
        ".claude.json",
        r#"{"projects":{"/a":{"hasTrustDialogAccepted":false},"/b":{"hasTrustDialogAccepted":true}}}"#,
    );
    assert_eq!(
        status("claude", Path::new("/a"), h.path()),
        Trust::Untrusted
    );
    assert_eq!(status("claude", Path::new("/b"), h.path()), Trust::Trusted);
}

/// `/a/bc` is not inside `/a/b`. A naive `starts_with` says it is.
#[test]
fn ancestor_matching_respects_path_boundaries() {
    let h = home();
    write(
        h.path(),
        ".claude.json",
        r#"{"projects":{"/a/b":{"hasTrustDialogAccepted":true}}}"#,
    );
    assert_eq!(
        status("claude", Path::new("/a/b/c"), h.path()),
        Trust::Trusted
    );
    assert_eq!(
        status("claude", Path::new("/a/bc"), h.path()),
        Trust::Untrusted
    );
}

// ----------------------------------------------------------------- codex ----

#[test]
fn codex_trust_is_read_from_a_general_toml_config() {
    let h = home();
    write(
        h.path(),
        ".codex/config.toml",
        r#"
model = "gpt-5.6-sol"

[mcp_servers.something]
command = "elsewhere"

[projects."/proj"]
trust_level = "trusted"
"#,
    );
    assert_eq!(
        status("codex", Path::new("/proj/wt"), h.path()),
        Trust::Trusted
    );
    assert_eq!(
        status("codex", Path::new("/other"), h.path()),
        Trust::Untrusted
    );
}

/// A `[projects."…"]` header alone is not consent — the level has to say so.
#[test]
fn codex_ignores_a_project_section_that_is_not_trusted() {
    let h = home();
    write(
        h.path(),
        ".codex/config.toml",
        "[projects.\"/proj\"]\ntrust_level = \"untrusted\"\n",
    );
    assert_eq!(
        status("codex", Path::new("/proj"), h.path()),
        Trust::Untrusted
    );
}

/// A following section must not inherit the previous one's key.
#[test]
fn codex_does_not_leak_trust_across_sections() {
    let h = home();
    write(
        h.path(),
        ".codex/config.toml",
        "[projects.\"/a\"]\n[other]\ntrust_level = \"trusted\"\n",
    );
    assert_eq!(status("codex", Path::new("/a"), h.path()), Trust::Untrusted);
}

// ---------------------------------------------------------------- gemini ----

/// Gemini lowercases what it stores, so a comparison on the raw path misses.
#[test]
fn gemini_paths_are_matched_case_insensitively() {
    let h = home();
    write(
        h.path(),
        ".gemini/trustedFolders.json",
        r#"{"/users/fynn/workspace/proj":"TRUST_FOLDER"}"#,
    );
    assert_eq!(
        status(
            "gemini",
            Path::new("/Users/Fynn/workspace/proj/.agtx/worktrees/w1"),
            h.path()
        ),
        Trust::Trusted
    );
}

#[test]
fn gemini_ignores_entries_that_are_not_trust_folder() {
    let h = home();
    write(
        h.path(),
        ".gemini/trustedFolders.json",
        r#"{"/p":"TRUST_NONE"}"#,
    );
    assert_eq!(
        status("gemini", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
}

// ----------------------------------------------------------- antigravity ----

/// The measured behaviour that drives the whole seeding design: `/Users/fynn` is
/// in the live list, and a brand-new directory directly under it still prompted.
#[test]
fn antigravity_trust_is_exact_and_never_inherited() {
    let h = home();
    write(
        h.path(),
        ".gemini/antigravity-cli/settings.json",
        r#"{"trustedWorkspaces":["/proj"]}"#,
    );
    assert_eq!(
        status("antigravity", Path::new("/proj"), h.path()),
        Trust::Trusted
    );
    assert_eq!(
        status("antigravity", Path::new("/proj/wt"), h.path()),
        Trust::Untrusted
    );
}

#[test]
fn only_antigravity_needs_seeding() {
    assert!(needs_seeding("antigravity"));
    for other in [
        "claude", "codex", "gemini", "cursor", "grok", "opencode", "copilot",
    ] {
        assert!(!needs_seeding(other), "{other}");
    }
}

#[test]
fn seeding_replays_project_consent_onto_a_worktree() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join(".agtx/worktrees/w1");
    fs::create_dir_all(&wt).unwrap();
    write(
        h.path(),
        ".gemini/antigravity-cli/settings.json",
        &format!(r#"{{"trustedWorkspaces":["{}"]}}"#, root.to_string_lossy()),
    );

    assert_eq!(status("antigravity", &wt, h.path()), Trust::Untrusted);
    assert!(seed_from_project("antigravity", &root, &wt, h.path()).unwrap());
    assert_eq!(status("antigravity", &wt, h.path()), Trust::Trusted);
}

/// The whole point of the design: agtx replays a decision, it never makes one.
#[test]
fn seeding_does_nothing_when_the_project_was_never_trusted() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    write(
        h.path(),
        ".gemini/antigravity-cli/settings.json",
        r#"{"trustedWorkspaces":[]}"#,
    );
    assert!(!seed_from_project("antigravity", &root, &wt, h.path()).unwrap());
    assert_eq!(status("antigravity", &wt, h.path()), Trust::Untrusted);
}

/// Redeploying a worktree must not grow the list.
#[test]
fn seeding_is_idempotent() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    write(
        h.path(),
        ".gemini/antigravity-cli/settings.json",
        &format!(r#"{{"trustedWorkspaces":["{}"]}}"#, root.to_string_lossy()),
    );
    assert!(seed_from_project("antigravity", &root, &wt, h.path()).unwrap());
    let after_first = Store::AntigravityWorkspaces.trusted_paths(h.path()).len();
    assert!(!seed_from_project("antigravity", &root, &wt, h.path()).unwrap());
    assert_eq!(
        Store::AntigravityWorkspaces.trusted_paths(h.path()).len(),
        after_first
    );
}

/// Other people's entries are not ours to drop.
#[test]
fn seeding_preserves_existing_entries() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    write(
        h.path(),
        ".gemini/antigravity-cli/settings.json",
        &format!(
            r#"{{"trustedWorkspaces":["/somewhere/else","{}"]}}"#,
            root.to_string_lossy()
        ),
    );
    seed_from_project("antigravity", &root, &wt, h.path()).unwrap();
    let paths = Store::AntigravityWorkspaces.trusted_paths(h.path());
    assert!(paths.iter().any(|p| p == "/somewhere/else"));
    assert!(paths.iter().any(|p| p == &root.to_string_lossy()));
}

#[test]
fn forget_removes_only_the_named_worktree() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    write(
        h.path(),
        ".gemini/antigravity-cli/settings.json",
        &format!(
            r#"{{"trustedWorkspaces":["{}","{}"]}}"#,
            root.to_string_lossy(),
            wt.to_string_lossy()
        ),
    );
    forget("antigravity", &wt, h.path()).unwrap();
    let paths = Store::AntigravityWorkspaces.trusted_paths(h.path());
    assert!(paths.iter().any(|p| p == &root.to_string_lossy()));
    assert!(!paths.iter().any(|p| p == &wt.to_string_lossy()));
}

/// Pruning an inheriting agent's store would delete a record agtx never wrote.
#[test]
fn forget_never_touches_a_store_agtx_does_not_seed() {
    let h = home();
    let body = r#"{"projects":{"/proj/wt":{"hasTrustDialogAccepted":true}}}"#;
    write(h.path(), ".claude.json", body);
    forget("claude", Path::new("/proj/wt"), h.path()).unwrap();
    assert_eq!(
        fs::read_to_string(h.path().join(".claude.json")).unwrap(),
        body
    );
}

// ------------------------------------------------------------ no-op agents --

/// Flag-based, conceptless and unmeasured agents must never read as a blocker.
#[test]
fn agents_without_a_readable_store_are_not_applicable() {
    let h = home();
    for a in ["cursor", "grok", "opencode", "copilot", "somethingelse"] {
        assert_eq!(
            status(a, Path::new("/anywhere"), h.path()),
            Trust::NotApplicable,
            "{a}"
        );
    }
}

/// A user who has never launched the agent has no file at all.
#[test]
fn a_missing_store_reads_as_untrusted_not_as_an_error() {
    let h = home();
    assert_eq!(
        status("claude", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
    assert_eq!(status("codex", Path::new("/p"), h.path()), Trust::Untrusted);
    assert_eq!(
        status("gemini", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
    assert_eq!(
        status("antigravity", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
}

/// A half-written or hand-edited file must not panic the refresh thread.
#[test]
fn malformed_stores_degrade_to_untrusted() {
    let h = home();
    write(h.path(), ".claude.json", "{not json");
    write(h.path(), ".gemini/trustedFolders.json", "[]");
    write(h.path(), ".gemini/antigravity-cli/settings.json", "null");
    assert_eq!(
        status("claude", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
    assert_eq!(
        status("gemini", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
    assert_eq!(
        status("antigravity", Path::new("/p"), h.path()),
        Trust::Untrusted
    );
}
