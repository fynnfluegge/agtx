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
//! | cursor, grok | launch flag `--trust` | n/a |
//! | opencode | no trust concept | n/a |
//! | copilot | unmeasured | n/a |
//!
//! Versions the table was measured against: claude 2.1.246, codex 0.144.5,
//! gemini 0.46.0, agy 1.1.21, cursor-agent 2026.08.11, opencode 1.18.20.
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
        }
    }

    /// Every path this store currently trusts, in the form the agent stores them.
    fn trusted_paths(self, home: &Path) -> Vec<String> {
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
        // cursor and grok pass `--trust` at launch; opencode has no trust gate;
        // copilot is unmeasured. None of them are a blocker agtx can reason about.
        _ => None,
    }
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
/// True only for antigravity: it matches exactly, so a project-level consent the
/// user has already given cannot cover a new worktree on its own. Every other
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

/// Drop `worktree` from an agent's trust store.
///
/// Called when a worktree is removed. Without it these lists grow one entry per
/// task forever, pointing at directories that no longer exist. Only stores agtx
/// writes to are pruned — agtx does not edit records it did not create.
pub fn forget(agent: &str, worktree: &Path, home: &Path) -> Result<()> {
    let Some(store) = store_for(agent) else {
        return Ok(());
    };
    if store.matching() != Match::Exact {
        return Ok(());
    }
    let file = store.path(home);
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

#[cfg(test)]
#[path = "trust_tests.rs"]
mod trust_tests;
