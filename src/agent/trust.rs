//! Reading — and where unavoidable, seeding — each agent's own workspace-trust store.
//!
//! Every supported agent gates a directory it has not seen behind an interactive
//! prompt, and every task gets a fresh worktree. agtx used to clear those prompts
//! by watching the tmux pane and sending the answer. That is a security decision
//! taken on the user's behalf, and measurement showed it is usually unnecessary:
//! most agents record trust per *repository root* or per *ancestor directory*, so
//! a user who opens the agent once in the project never sees the prompt again in a
//! worktree beneath it.
//!
//! This module answers "is path P already trusted for agent A?" by reading the
//! agent's own file — which is also how consent given **outside** agtx is
//! discovered, since the agent records it either way.
//!
//! | Agent | Store | Match |
//! |---|---|---|
//! | claude | `~/.claude.json` → `projects["<dir>"].hasTrustDialogAccepted` | ancestor |
//! | codex | `~/.codex/config.toml` → `[projects."<dir>"] trust_level` | ancestor (git repo root) |
//! | gemini | `~/.gemini/trustedFolders.json` → `{"<dir>": "TRUST_FOLDER"}`, **lowercased** | ancestor |
//! | antigravity | `~/.gemini/antigravity-cli/settings.json` → `trustedWorkspaces: [...]` | **exact** |
//! | kimi | `~/.kimi-code/workspace-trust/wd_<slug>_<hash>` → `{"root": "<dir>"}` | **exact** |
//! | cursor, grok | launch flag `--trust` | n/a |
//! | pi | launch flag `--approve` (store is `~/.pi/agent/trust.json`) | n/a |
//! | opencode | no trust concept | n/a |
//! | copilot | unmeasured | n/a |
//!
//! Versions the table was measured against: claude 2.1.246, codex 0.144.5,
//! gemini 0.46.0, agy 1.1.21, cursor-agent 2026.08.11, opencode 1.18.20, pi 0.84.3.
//!
//! kimi is the one store that is a **directory of one-record files** rather than
//! a single document, so its `trusted_paths` scans instead of parsing, and its
//! `forget` unlinks instead of editing. Each record carries the directory
//! verbatim under `root`, which is what lets it be read generically.
//!
//! **copilot is deliberately `NotApplicable`, not "assumed inherited".** It is not
//! installed on any machine this was measured on, so nothing is known about its
//! dialogs or its store. Assuming an unmeasured agent behaves like its neighbours
//! is what produced the antigravity and cursor dialog bugs.

use anyhow::Result;
use std::path::{Path, PathBuf};

/// What agtx knows about an agent's trust for one directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trust {
    /// The agent's own store already covers this path — it will not prompt.
    Trusted,
    /// The agent has a trust store and this path is not covered: it *will* prompt.
    Untrusted,
    /// This agent has no trust gate agtx can reason about — a launch flag handles
    /// it, it has no such concept, or it has never been measured. Never treated as
    /// a blocker.
    NotApplicable,
}

/// How an agent matches a path against its store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Match {
    /// A trusted ancestor covers everything beneath it.
    Ancestor,
    /// Only the exact path counts — every worktree needs its own entry.
    Exact,
}

/// Where an agent keeps its trust records, and how it matches them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Store {
    /// `~/.claude.json`, `projects["<dir>"].hasTrustDialogAccepted == true`.
    ClaudeJson,
    /// `~/.codex/config.toml`, `[projects."<dir>"] trust_level = "trusted"`.
    CodexToml,
    /// `~/.gemini/trustedFolders.json`, `{"<lowercased dir>": "TRUST_FOLDER"}`.
    GeminiTrustedFolders,
    /// `~/.gemini/antigravity-cli/settings.json`, `trustedWorkspaces: ["<dir>", …]`.
    AntigravityWorkspaces,
    /// `<kimi_home>/workspace-trust/`, one extensionless JSON file per trusted
    /// directory, named `wd_<slug>_<sha256(root)[..12]>` and holding
    /// `{"root": "<dir>", "trustedAt": <unix millis>}`.
    ///
    /// Unlike the other four this is a *directory*, so [`Store::path`] returns
    /// the directory and everything that reads it scans rather than parses.
    KimiWorkspaceTrust,
}

impl Store {
    fn matching(self) -> Match {
        match self {
            // Verified: a fresh `git worktree add` under an already-trusted project
            // opened straight at the composer, with no new `projects` entry written.
            Store::ClaudeJson => Match::Ancestor,
            // Verified: codex's own dialog says "Trusting will apply to the
            // repository root", and a worktree under a trusted root neither
            // prompted nor lost its project-local config.
            Store::CodexToml => Match::Ancestor,
            // Verified: `/users/fynn` alone covers everything beneath it, which is
            // also why gemini's dialog offers "Trust parent folder".
            Store::GeminiTrustedFolders => Match::Ancestor,
            // Verified against agy 1.1.21: `/Users/fynn` is in the list, and a
            // brand-new directory created *directly under it* still prompted. No
            // inheritance at any depth.
            Store::AntigravityWorkspaces => Match::Exact,
            // `WorkspaceTrustService` sets `this.root = workspace.cwd` and looks
            // up `encodeWorkDirKey(canonicalWorkspaceRoot(root))` — a hash of
            // that one directory. There is no ancestor walk, so a trusted
            // project root cannot cover a worktree beneath it.
            Store::KimiWorkspaceTrust => Match::Exact,
        }
    }

    fn path(self, home: &Path) -> PathBuf {
        match self {
            Store::ClaudeJson => home.join(".claude.json"),
            Store::CodexToml => home.join(".codex").join("config.toml"),
            Store::GeminiTrustedFolders => home.join(".gemini").join("trustedFolders.json"),
            Store::AntigravityWorkspaces => home
                .join(".gemini")
                .join("antigravity-cli")
                .join("settings.json"),
            // A directory, not a file — the only such store here.
            Store::KimiWorkspaceTrust => kimi_home(home).join("workspace-trust"),
        }
    }

    /// Every path this store currently trusts, in the form the agent stores them.
    fn trusted_paths(self, home: &Path) -> Vec<String> {
        // Handled before the read below, because this store is a directory of
        // one-record files rather than one document. Each record carries the
        // trusted directory verbatim under `root`, so the generic `is_covered`
        // comparison works on the result unchanged — and consent the user gave
        // outside agtx is discovered the same way it is for every other agent.
        if self == Store::KimiWorkspaceTrust {
            return std::fs::read_dir(self.path(home))
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(|entry| std::fs::read_to_string(entry.path()).ok())
                .filter_map(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .filter_map(|v| v.get("root")?.as_str().map(str::to_string))
                .collect();
        }
        let raw = match std::fs::read_to_string(self.path(home)) {
            Ok(r) => r,
            Err(_) => return Vec::new(),
        };
        match self {
            Store::ClaudeJson => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v.get("projects")?.as_object().cloned())
                .map(|projects| {
                    projects
                        .into_iter()
                        .filter(|(_, v)| {
                            v.get("hasTrustDialogAccepted")
                                .and_then(|b| b.as_bool())
                                .unwrap_or(false)
                        })
                        .map(|(k, _)| k)
                        .collect()
                })
                .unwrap_or_default(),
            // Parsed by hand rather than with a TOML crate: the file is the user's
            // own general config and may contain constructs agtx has no business
            // failing on. Only `[projects."<path>"]` headers followed by a trusted
            // `trust_level` matter here.
            Store::CodexToml => {
                let mut out = Vec::new();
                let mut current: Option<String> = None;
                for line in raw.lines() {
                    let line = line.trim();
                    if let Some(rest) = line
                        .strip_prefix("[projects.\"")
                        .and_then(|r| r.strip_suffix("\"]"))
                    {
                        current = Some(rest.to_string());
                    } else if line.starts_with('[') {
                        current = None;
                    } else if let Some(path) = current.as_ref() {
                        if line.starts_with("trust_level") && line.contains("\"trusted\"") {
                            out.push(path.clone());
                            current = None;
                        }
                    }
                }
                out
            }
            Store::GeminiTrustedFolders => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v.as_object().cloned())
                .map(|m| {
                    m.into_iter()
                        .filter(|(_, v)| v.as_str() == Some("TRUST_FOLDER"))
                        .map(|(k, _)| k)
                        .collect()
                })
                .unwrap_or_default(),
            Store::AntigravityWorkspaces => serde_json::from_str::<serde_json::Value>(&raw)
                .ok()
                .and_then(|v| v.get("trustedWorkspaces")?.as_array().cloned())
                .map(|a| {
                    a.into_iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default(),
            // Returned above, before the file read this arm follows.
            Store::KimiWorkspaceTrust => Vec::new(),
        }
    }

    /// Gemini lowercases what it stores; nothing else does.
    fn normalize(self, path: &str) -> String {
        match self {
            Store::GeminiTrustedFolders => path.to_lowercase(),
            _ => path.to_string(),
        }
    }
}

fn store_for(agent: &str) -> Option<Store> {
    match agent {
        "claude" => Some(Store::ClaudeJson),
        "codex" => Some(Store::CodexToml),
        "gemini" => Some(Store::GeminiTrustedFolders),
        "antigravity" => Some(Store::AntigravityWorkspaces),
        "kimi" => Some(Store::KimiWorkspaceTrust),
        // cursor and grok pass `--trust` at launch and pi passes `--approve`;
        // opencode has no trust gate; copilot is unmeasured. None of them are a
        // blocker agtx can reason about. pi does keep a store
        // (`~/.pi/agent/trust.json`), but agtx never needs to read or seed it:
        // the flag decides the run, and unlike codex it writes nothing global.
        _ => None,
    }
}

/// Kimi's data root: `$KIMI_CODE_HOME` if set, else `<home>/.kimi-code`.
///
/// The environment variable is honoured **only when `AGTX_AGENT_HOME` is unset**.
/// Same reasoning as `agent_trust_home()` itself: a developer who exports
/// `KIMI_CODE_HOME` for their own use would otherwise have the test suite — which
/// redirects the home directory precisely so it cannot touch real agent config —
/// write into their live kimi store anyway.
fn kimi_home(home: &Path) -> PathBuf {
    if std::env::var_os("AGTX_AGENT_HOME").is_none() {
        if let Some(dir) = std::env::var_os("KIMI_CODE_HOME") {
            if !dir.is_empty() {
                return PathBuf::from(dir);
            }
        }
    }
    home.join(".kimi-code")
}

/// The filename kimi stores a directory's trust record under.
///
/// A pure port of `encodeWorkDirKey ∘ canonicalWorkspaceRoot`:
///
/// ```text
/// normalized: backslashes → "/", trailing "/" stripped
/// slug:       basename(normalized) → lowercase → s/[^a-z0-9._-]+/-/g
///             → trim "-" → take 40 → trim "-"
///             → "" | "." | ".." becomes "workspace"
/// key:        wd_<slug>_<sha256(normalized)[..12 hex chars]>
/// ```
///
/// **Not `realpath`.** `canonicalWorkspaceRoot` uses pathe's lexical `resolve()`,
/// so `/tmp/x` stays `/tmp/x` rather than becoming `/private/tmp/x` on macOS.
/// Canonicalising here would compute a key kimi never looks up — the same
/// mismatch that invalidated the first codex measurement. [`seed_kimi`] writes
/// one record per [`path_forms`] spelling instead, each keyed from *that*
/// spelling.
fn kimi_trust_key(root: &str) -> String {
    use sha2::{Digest, Sha256};

    let normalized = root.replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let base = normalized.rsplit('/').next().unwrap_or(normalized);

    // The JS regex `[^a-z0-9._-]+` replaces each *run* with a single dash, so
    // collapse runs rather than mapping character-for-character. Done after
    // lowercasing, and the result is pure ASCII — which is what makes the
    // 40-byte truncation below equal JS's 40-UTF-16-unit `slice`.
    let mut slug = String::new();
    let mut in_run = false;
    for c in base.to_lowercase().chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
            slug.push(c);
            in_run = false;
        } else if !in_run {
            slug.push('-');
            in_run = true;
        }
    }
    // Trimmed twice, and that is deliberate: the first trim is the JS `replace`
    // chain's own, and the second catches a dash left exposed by a truncation
    // that cut through a run.
    let slug = slug.trim_matches('-');
    let slug = &slug[..slug.len().min(40)];
    let slug = slug.trim_matches('-');
    let slug = match slug {
        "" | "." | ".." => "workspace",
        other => other,
    };

    let digest = Sha256::digest(normalized.as_bytes());
    let hash: String = digest.iter().take(6).map(|b| format!("{b:02x}")).collect();
    format!("wd_{slug}_{hash}")
}

/// The forms of `path` an agent might have recorded.
///
/// Agents store the working directory **as they received it**, not canonicalised:
/// antigravity's live list holds `/tmp/agy-probe-agy1` next to
/// `/private/var/folders/…`. On macOS `/tmp` and `/var` are symlinks, so the same
/// directory has two spellings and a comparison on one of them silently misses.
/// This is the same mismatch that invalidated the first codex measurement.
fn path_forms(path: &Path) -> Vec<String> {
    let mut forms = vec![path.to_string_lossy().to_string()];
    if let Ok(real) = std::fs::canonicalize(path) {
        let real = real.to_string_lossy().to_string();
        // `canonicalize` yields a `\\?\` prefix on Windows; harmless here because
        // it only ever widens the set of spellings we compare against.
        if !forms.contains(&real) {
            forms.push(real);
        }
    }
    forms
}

fn is_covered(store: Store, trusted: &[String], path: &Path) -> bool {
    let matching = store.matching();
    for form in path_forms(path) {
        let candidate = store.normalize(&form);
        for entry in trusted {
            let entry = store.normalize(entry);
            if entry == candidate {
                return true;
            }
            if matching == Match::Ancestor {
                // Compare on path boundaries: `/a/bc` must not be covered by `/a/b`.
                let prefix = if entry.ends_with('/') {
                    entry.clone()
                } else {
                    format!("{entry}/")
                };
                if candidate.starts_with(&prefix) {
                    return true;
                }
            }
        }
    }
    false
}

/// Whether `agent` will already trust `path`, per the agent's own records.
pub fn status(agent: &str, path: &Path, home: &Path) -> Trust {
    let Some(store) = store_for(agent) else {
        return Trust::NotApplicable;
    };
    if is_covered(store, &store.trusted_paths(home), path) {
        Trust::Trusted
    } else {
        Trust::Untrusted
    }
}

/// Whether this agent needs a per-directory entry that agtx can write for it.
///
/// True for antigravity and kimi: both match exactly, so a project-level consent
/// the user has already given cannot cover a new worktree on its own. Every other
/// agent either inherits from an ancestor (nothing to write) or has no store.
pub fn needs_seeding(agent: &str) -> bool {
    store_for(agent).map(Store::matching) == Some(Match::Exact)
}

/// Record `worktree` as trusted for `agent`, if the user has already trusted
/// `project_root` for that agent.
///
/// This **replays an existing consent**, it does not create one: with an untrusted
/// project root it does nothing and returns `false`, leaving the decision where it
/// belongs. Writing to the agent's file is defensible here precisely because the
/// answer already exists in it — and because the entry lands there either way, put
/// by the agent itself the moment anyone answers its dialog.
///
/// Returns whether an entry was written.
pub fn seed_from_project(
    agent: &str,
    project_root: &Path,
    worktree: &Path,
    home: &Path,
) -> Result<bool> {
    let Some(store) = store_for(agent) else {
        return Ok(false);
    };
    if store.matching() != Match::Exact {
        return Ok(false);
    }
    if status(agent, project_root, home) != Trust::Trusted {
        return Ok(false);
    }
    if status(agent, worktree, home) == Trust::Trusted {
        return Ok(false);
    }
    match store {
        Store::AntigravityWorkspaces => seed_antigravity(worktree, home).map(|()| true),
        Store::KimiWorkspaceTrust => seed_kimi(worktree, home).map(|()| true),
        _ => Ok(false),
    }
}

/// Append the worktree to antigravity's `trustedWorkspaces`.
///
/// Read-modify-write with an atomic rename: `agy` rewrites this file wholesale, so
/// a partial write would be visible to it. Both spellings of the path are added —
/// see [`path_forms`].
fn seed_antigravity(worktree: &Path, home: &Path) -> Result<()> {
    let file = Store::AntigravityWorkspaces.path(home);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut root: serde_json::Value = std::fs::read_to_string(&file)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .filter(|v: &serde_json::Value| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));

    let list = root
        .as_object_mut()
        .expect("object by construction")
        .entry("trustedWorkspaces")
        .or_insert_with(|| serde_json::json!([]));
    if !list.is_array() {
        *list = serde_json::json!([]);
    }
    let arr = list.as_array_mut().expect("array by construction");
    for form in path_forms(worktree) {
        if !arr.iter().any(|v| v.as_str() == Some(form.as_str())) {
            arr.push(serde_json::Value::String(form));
        }
    }

    let tmp = file.with_extension(format!("agtx-tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// Write kimi's trust record for the worktree, one file per path spelling.
///
/// Kimi's `readWorkspaceTrust` returns true if the file merely **exists and
/// parses as JSON** — the contents are not validated — but the record carries
/// `root` verbatim anyway, because that is what lets [`Store::trusted_paths`]
/// read the store back generically.
///
/// One record per [`path_forms`] spelling, as [`seed_antigravity`] does: on macOS
/// `/tmp` and `/private/tmp` are both live spellings of one directory and kimi
/// hashes whichever it is handed. Each key is computed from *its own* spelling —
/// canonicalising first would produce a key kimi never looks up.
fn seed_kimi(worktree: &Path, home: &Path) -> Result<()> {
    let dir = Store::KimiWorkspaceTrust.path(home);
    std::fs::create_dir_all(&dir)?;
    let trusted_at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    for form in path_forms(worktree) {
        let record = serde_json::json!({ "root": form, "trustedAt": trusted_at });
        let file = dir.join(kimi_trust_key(&form));
        // Written-then-renamed like the others: kimi reads these files while it
        // starts, and a half-written one would parse as untrusted.
        let tmp = dir.join(format!(
            ".agtx-tmp{}-{}",
            std::process::id(),
            kimi_trust_key(&form)
        ));
        std::fs::write(&tmp, serde_json::to_string(&record)?)?;
        std::fs::rename(&tmp, &file)?;
    }
    Ok(())
}

/// Drop `worktree` from an agent's trust store.
///
/// Called when a worktree is removed. Without it these lists grow one entry per
/// task forever, pointing at directories that no longer exist. Only stores agtx
/// writes to are pruned — agtx does not edit records it did not create.
pub fn forget(agent: &str, worktree: &Path, home: &Path) -> Result<()> {
    let Some(store) = store_for(agent) else {
        return Ok(());
    };
    // Dispatched on the store, not merely gated on `Match::Exact`: there are two
    // exact-matching stores now and their shapes have nothing in common — one is
    // a JSON array to edit, the other a directory of files to unlink. Falling
    // through to antigravity's JSON surgery would try to parse kimi's directory
    // and silently prune nothing, leaving one dead record per task forever,
    // which is the exact leak the `forget` call sites were added to stop.
    match store {
        Store::AntigravityWorkspaces => forget_antigravity(worktree, home),
        Store::KimiWorkspaceTrust => forget_kimi(worktree, home),
        // An inheriting store holds records agtx never wrote, so it is not
        // agtx's to prune.
        _ => Ok(()),
    }
}

/// Remove the worktree's entries from antigravity's `trustedWorkspaces` array.
fn forget_antigravity(worktree: &Path, home: &Path) -> Result<()> {
    let file = Store::AntigravityWorkspaces.path(home);
    let Ok(raw) = std::fs::read_to_string(&file) else {
        return Ok(());
    };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(());
    };
    let forms = path_forms(worktree);
    if let Some(arr) = root
        .get_mut("trustedWorkspaces")
        .and_then(|v| v.as_array_mut())
    {
        let before = arr.len();
        arr.retain(|v| !v.as_str().is_some_and(|s| forms.iter().any(|f| f == s)));
        if arr.len() == before {
            return Ok(());
        }
    } else {
        return Ok(());
    }
    let tmp = file.with_extension(format!("agtx-tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string_pretty(&root)?)?;
    std::fs::rename(&tmp, &file)?;
    Ok(())
}

/// Unlink the worktree's trust records from kimi's `workspace-trust` directory.
///
/// One key per path spelling, matching what [`seed_kimi`] wrote. A missing file
/// is not an error — the user may have removed it, or the seed may never have
/// run because the project root was untrusted.
fn forget_kimi(worktree: &Path, home: &Path) -> Result<()> {
    let dir = Store::KimiWorkspaceTrust.path(home);
    for form in path_forms(worktree) {
        let _ = std::fs::remove_file(dir.join(kimi_trust_key(&form)));
    }
    Ok(())
}

#[cfg(test)]
#[path = "trust_tests.rs"]
mod trust_tests;
