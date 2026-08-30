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
fn only_exact_matching_agents_need_seeding() {
    assert!(needs_seeding("antigravity"));
    assert!(needs_seeding("kimi"));
    for other in [
        "claude", "codex", "gemini", "cursor", "grok", "opencode", "copilot", "pi",
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

// ------------------------------------------------------------------ kimi ----

/// Guard on the *shape* of the key rather than a frozen hash: a hand-computed
/// digest could only restate this implementation. What it does pin is every
/// lexical rule — `wd_` + slug + `_` + exactly 12 lowercase hex characters.
#[test]
fn kimi_trust_key_has_the_documented_shape() {
    let key = kimi_trust_key("/Users/fynn/workspace/agtx");
    let rest = key
        .strip_prefix("wd_")
        .unwrap_or_else(|| panic!("wd_ prefix: {key}"));
    let (slug, hash) = rest
        .rsplit_once('_')
        .unwrap_or_else(|| panic!("slug_hash: {key}"));
    assert_eq!(slug, "agtx", "the slug is the basename, lowercased");
    assert_eq!(hash.len(), 12, "sha256 truncated to 12 hex chars: {key}");
    assert!(
        hash.chars()
            .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()),
        "lowercase hex only: {key}"
    );
}

/// The hash covers the *normalized whole path*, so spellings that normalize
/// alike collide deliberately and different directories do not.
#[test]
fn kimi_trust_key_is_derived_from_the_whole_path() {
    // Same basename, different parents: the slug matches, the hash must not.
    let a = kimi_trust_key("/one/proj");
    let b = kimi_trust_key("/two/proj");
    assert_ne!(a, b, "the hash must cover the full path, not just the slug");
    assert!(a.starts_with("wd_proj_") && b.starts_with("wd_proj_"));
    // A trailing slash is stripped before hashing — the same directory.
    assert_eq!(kimi_trust_key("/one/proj/"), a);
    // Backslashes are rewritten to forward slashes first.
    assert_eq!(kimi_trust_key(r"\one\proj"), a);
}

/// `[^a-z0-9._-]+` replaces each *run* with one dash, not one per character.
fn slug_of(root: &str) -> String {
    kimi_trust_key(root)
        .strip_prefix("wd_")
        .unwrap()
        .rsplit_once('_')
        .unwrap()
        .0
        .to_string()
}

#[test]
fn kimi_trust_key_slugifies_and_collapses_runs() {
    assert_eq!(slug_of("/tmp/My Project (v2)!!"), "my-project-v2");
}

/// The trim runs twice: once as part of the replacement chain, and again after
/// the 40-character truncation, so a cut through a run leaves no trailing dash.
#[test]
fn kimi_trust_key_trims_after_truncating() {
    // 39 kept characters, then a separator run — the cut lands inside the run.
    let name = format!("{}   tail", "a".repeat(39));
    let slug = slug_of(&format!("/tmp/{name}"));
    assert_eq!(slug, "a".repeat(39), "no dash left exposed by the cut");
    assert!(slug.len() <= 40);
}

#[test]
fn kimi_trust_key_falls_back_to_workspace_for_an_empty_basename() {
    for root in ["/", ".", "..", "///"] {
        assert_eq!(slug_of(root), "workspace", "{root}");
    }
}

/// `AGTX_AGENT_HOME` is the suite's guarantee that it never touches real agent
/// config. A developer's own `KIMI_CODE_HOME` must not defeat it.
#[test]
fn kimi_home_yields_to_the_test_redirect() {
    let h = home();
    let prev_kimi = std::env::var_os("KIMI_CODE_HOME");
    let prev_agtx = std::env::var_os("AGTX_AGENT_HOME");

    std::env::remove_var("AGTX_AGENT_HOME");
    std::env::set_var("KIMI_CODE_HOME", "/elsewhere/kimi");
    assert_eq!(kimi_home(h.path()), Path::new("/elsewhere/kimi"));

    std::env::set_var("AGTX_AGENT_HOME", h.path());
    assert_eq!(
        kimi_home(h.path()),
        h.path().join(".kimi-code"),
        "AGTX_AGENT_HOME must win over a developer's own KIMI_CODE_HOME"
    );

    std::env::remove_var("KIMI_CODE_HOME");
    assert_eq!(kimi_home(h.path()), h.path().join(".kimi-code"));

    match prev_kimi {
        Some(v) => std::env::set_var("KIMI_CODE_HOME", v),
        None => std::env::remove_var("KIMI_CODE_HOME"),
    }
    match prev_agtx {
        Some(v) => std::env::set_var("AGTX_AGENT_HOME", v),
        None => std::env::remove_var("AGTX_AGENT_HOME"),
    }
}

/// Consent given outside agtx is discovered by reading kimi's own records: the
/// directory is scanned and each record's `root` is what is compared.
#[test]
fn kimi_reads_a_record_written_by_the_agent_itself() {
    let h = home();
    write(
        h.path(),
        &format!(".kimi-code/workspace-trust/{}", kimi_trust_key("/proj")),
        r#"{"root":"/proj","trustedAt":1756512000000}"#,
    );
    assert_eq!(status("kimi", Path::new("/proj"), h.path()), Trust::Trusted);
}

/// What makes kimi the second seeding agent: the key is the working directory
/// verbatim, with no ancestor walk.
#[test]
fn kimi_trust_is_exact_and_never_inherited() {
    let h = home();
    write(
        h.path(),
        &format!(".kimi-code/workspace-trust/{}", kimi_trust_key("/proj")),
        r#"{"root":"/proj"}"#,
    );
    assert_eq!(status("kimi", Path::new("/proj"), h.path()), Trust::Trusted);
    assert_eq!(
        status("kimi", Path::new("/proj/wt"), h.path()),
        Trust::Untrusted
    );
}

/// Trust the project root for kimi, the way the user's own first `kimi` run does.
fn trust_kimi_root(h: &Path, root: &Path) {
    write(
        h,
        &format!(
            ".kimi-code/workspace-trust/{}",
            kimi_trust_key(&root.to_string_lossy())
        ),
        &format!(r#"{{"root":"{}"}}"#, root.to_string_lossy()),
    );
}

#[test]
fn kimi_seeding_replays_project_consent_onto_a_worktree() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join(".agtx/worktrees/w1");
    fs::create_dir_all(&wt).unwrap();
    trust_kimi_root(h.path(), &root);

    assert_eq!(status("kimi", &wt, h.path()), Trust::Untrusted);
    assert!(seed_from_project("kimi", &root, &wt, h.path()).unwrap());
    assert_eq!(status("kimi", &wt, h.path()), Trust::Trusted);
}

/// The seeded record has to be readable *by kimi*, not merely by agtx: found at
/// the key kimi computes, and parsing as JSON — which is the whole of kimi's own
/// `readWorkspaceTrust` check.
#[test]
fn kimi_seeding_writes_the_record_at_the_computed_key() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    trust_kimi_root(h.path(), &root);
    seed_from_project("kimi", &root, &wt, h.path()).unwrap();

    let store = h.path().join(".kimi-code/workspace-trust");
    let file = store.join(kimi_trust_key(&wt.to_string_lossy()));
    let v: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&file).expect("record at the computed key"))
            .expect("valid JSON");
    assert_eq!(v["root"], wt.to_string_lossy().to_string());
    assert!(v["trustedAt"].is_number(), "unix millis, as kimi writes");

    // The write-then-rename must leave nothing behind for kimi to trip over —
    // every file in this directory is a trust record to it.
    for entry in fs::read_dir(&store).unwrap() {
        let name = entry.unwrap().file_name();
        assert!(
            !name.to_string_lossy().starts_with(".agtx-tmp"),
            "temp file left behind: {name:?}"
        );
    }
}

/// The policy assertion: agtx replays a decision, it never makes one.
#[test]
fn kimi_seeding_does_nothing_when_the_project_was_never_trusted() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    assert!(!seed_from_project("kimi", &root, &wt, h.path()).unwrap());
    assert_eq!(status("kimi", &wt, h.path()), Trust::Untrusted);
    assert!(
        !h.path().join(".kimi-code/workspace-trust").exists(),
        "an untrusted project must not even create the store directory"
    );
}

/// Redeploying a worktree must not accumulate records.
#[test]
fn kimi_seeding_is_idempotent() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    trust_kimi_root(h.path(), &root);
    seed_from_project("kimi", &root, &wt, h.path()).unwrap();
    let after_first = Store::KimiWorkspaceTrust.trusted_paths(h.path()).len();
    // Already trusted, so the second call is a no-op before it writes anything.
    assert!(!seed_from_project("kimi", &root, &wt, h.path()).unwrap());
    assert_eq!(
        Store::KimiWorkspaceTrust.trusted_paths(h.path()).len(),
        after_first
    );
}

/// The leak regression test. Before `forget` dispatched on the store it gated on
/// `Match::Exact` and then ran antigravity's JSON surgery, which would try to
/// parse kimi's *directory* and prune nothing — one dead record per task forever,
/// the exact leak the `forget` call sites were added to stop.
#[test]
fn kimi_forget_removes_the_seeded_record() {
    let h = home();
    let root = h.path().join("proj");
    let wt = root.join("wt");
    fs::create_dir_all(&wt).unwrap();
    trust_kimi_root(h.path(), &root);
    seed_from_project("kimi", &root, &wt, h.path()).unwrap();
    assert_eq!(status("kimi", &wt, h.path()), Trust::Trusted);

    forget("kimi", &wt, h.path()).unwrap();
    assert_eq!(status("kimi", &wt, h.path()), Trust::Untrusted);
    // The project's own consent is not agtx's to drop.
    assert_eq!(status("kimi", &root, h.path()), Trust::Trusted);
}

/// A missing record is the ordinary case when seeding never ran, not an error.
#[test]
fn kimi_forget_is_a_no_op_with_no_store() {
    let h = home();
    forget("kimi", Path::new("/proj/wt"), h.path()).unwrap();
}

// ------------------------------------------------------------ no-op agents --

/// Flag-based, conceptless and unmeasured agents must never read as a blocker.
#[test]
fn agents_without_a_readable_store_are_not_applicable() {
    let h = home();
    for a in [
        "cursor",
        "grok",
        "pi",
        "opencode",
        "copilot",
        "somethingelse",
    ] {
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
    assert_eq!(status("kimi", Path::new("/p"), h.path()), Trust::Untrusted);
}

/// A half-written or hand-edited file must not panic the refresh thread.
#[test]
fn malformed_stores_degrade_to_untrusted() {
    let h = home();
    write(h.path(), ".claude.json", "{not json");
    write(h.path(), ".gemini/trustedFolders.json", "[]");
    write(h.path(), ".gemini/antigravity-cli/settings.json", "null");
    // kimi's store is a directory, so its malformed case is a record in it that
    // does not parse — which kimi itself also reads as no consent.
    write(
        h.path(),
        &format!(".kimi-code/workspace-trust/{}", kimi_trust_key("/p")),
        "{not json",
    );
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
    assert_eq!(status("kimi", Path::new("/p"), h.path()), Trust::Untrusted);
}
