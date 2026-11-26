//! Orchestrator MCP registration for file-based agents.
//!
//! The task path writes each agent's project-scoped MCP config into its
//! worktree (`tui::app::write_mcp_config`). The orchestrator runs in the
//! **project root**, with no worktree, so this module registers the same
//! entry there — merging into whatever config file is already present,
//! because a root file is the user's, not a throwaway worktree's — and hands
//! back the shell fragment that puts the file back the way it was once the
//! orchestrator exits.
//!
//! `entry_json`/`entry_toml` are the single source of truth for the entry
//! shapes: the worktree writers and this module both build from them, so the
//! two paths cannot drift apart.
//!
//! Claude is not covered: it registers via `claude mcp add-json` inside its
//! own command string (`operations.rs`), which also carries its cleanup.

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

use super::McpConfigKind;

/// Where a file-based agent's orchestrator config lives, relative to the
/// launch directory (the project root).
///
/// `ClaudeJson` → `None`: it registers via CLI inside its command string.
pub(crate) fn orchestrator_mcp_target(kind: McpConfigKind) -> Option<&'static str> {
    match kind {
        McpConfigKind::ClaudeJson => None,
        McpConfigKind::OpenCode => Some("opencode.json"),
        McpConfigKind::CodexToml => Some(".codex/config.toml"),
        McpConfigKind::GeminiJson => Some(".gemini/settings.json"),
        McpConfigKind::CursorJson => Some(".cursor/mcp.json"),
        McpConfigKind::GrokTomlMerge => Some(".grok/config.toml"),
        McpConfigKind::AntigravityJsonMerge => Some(".agents/mcp_config.json"),
        McpConfigKind::PiJsonMerge => Some(".pi/mcp.json"),
    }
}

/// The agtx MCP server entry in the shape a file-based agent's JSON config
/// file expects: the server-map key and the `agtx` value. `None` for the
/// kinds that carry no JSON entry (Claude registers via CLI; the TOML kinds
/// use [`entry_toml`]).
pub(crate) fn entry_json(
    kind: McpConfigKind,
    agtx_bin: &str,
    project_path: &str,
) -> Option<(&'static str, Value)> {
    match kind {
        McpConfigKind::OpenCode => Some((
            "mcp",
            json!({
                "type": "local",
                "command": [agtx_bin, "mcp-serve", project_path]
            }),
        )),
        McpConfigKind::GeminiJson => Some((
            "mcpServers",
            json!({
                "command": agtx_bin,
                "args": ["mcp-serve", project_path],
                "trust": true
            }),
        )),
        McpConfigKind::CursorJson
        | McpConfigKind::AntigravityJsonMerge
        | McpConfigKind::PiJsonMerge => Some((
            "mcpServers",
            json!({
                "command": agtx_bin,
                "args": ["mcp-serve", project_path]
            }),
        )),
        McpConfigKind::ClaudeJson
        | McpConfigKind::CodexToml
        | McpConfigKind::GrokTomlMerge => None,
    }
}

/// The agtx MCP server entry as a TOML table, for the TOML-based agents.
/// Values are escaped for the double-quoted TOML strings.
pub(crate) fn entry_toml(kind: McpConfigKind, agtx_bin: &str, project_path: &str) -> Option<String> {
    match kind {
        McpConfigKind::CodexToml | McpConfigKind::GrokTomlMerge => {
            let esc = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
            Some(format!(
                "[mcp_servers.agtx]\ncommand = \"{}\"\nargs = [\"mcp-serve\", \"{}\"]\n",
                esc(agtx_bin),
                esc(project_path)
            ))
        }
        _ => None,
    }
}

/// Merge the agtx MCP server entry into the agent config file inside `dir`
/// (the orchestrator's launch directory — the project root) and return the
/// shell fragment that restores the file to its pre-launch state after the
/// orchestrator exits.
///
/// `None` → nothing was registered → the caller launches bare. That is the
/// outcome whenever touching the file would be unsafe: a pre-existing file
/// we cannot back up, one we cannot parse into the shape we merge into, an
/// already-registered TOML table, or a write failure.
///
/// Never overwrites user content: JSON files are parsed and the `agtx` entry
/// is inserted (sibling keys and servers survive); a pre-existing file is
/// backed up first so the cleanup fragment can restore it byte-for-byte.
pub(crate) fn register(
    dir: &Path,
    rel_path: &str,
    kind: McpConfigKind,
    agtx_bin: &str,
    project_path: &str,
) -> Option<String> {
    let file = dir.join(rel_path);
    let pre_existed = file.exists();
    let backup_path = pre_existed.then(|| backup(&file)).flatten();
    // A pre-existing file we could not back up is a file we must not
    // register into: the cleanup fragment would have no backup to restore,
    // so it would `rm` the user's config when the orchestrator exits.
    // Bail instead — the agent launches bare and the file stays untouched.
    // (A *new* file needs no backup; cleanup removes the file we create.)
    if pre_existed && backup_path.is_none() {
        tracing::warn!("orchestrator MCP: cannot back up {}; not registering", file.display());
        return None;
    }

    let content = if let Some((key, entry)) = entry_json(kind, agtx_bin, project_path) {
        merge_json(&file, key, entry, pre_existed, &backup_path)?
    } else if let Some(table) = entry_toml(kind, agtx_bin, project_path) {
        merge_toml(&file, &table, pre_existed, &backup_path)?
    } else {
        return None; // ClaudeJson: registers via CLI, not a config file
    };

    if let Some(parent) = file.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            tracing::warn!("orchestrator MCP: cannot create {}", parent.display());
            drop_backup(&backup_path);
            return None;
        }
    }
    if std::fs::write(&file, &content).is_err() {
        tracing::warn!("orchestrator MCP: cannot write {}", file.display());
        drop_backup(&backup_path);
        return None;
    }

    Some(cleanup_fragment(&file, backup_path.as_deref()))
}

/// Build the merged JSON content, or `None` to launch bare. A pre-existing
/// file that is not a JSON object we can merge into is left alone:
/// displacing it would drop the user's content for the session (the agent
/// would launch with a degraded config), and a killed window would leave the
/// displacement stuck.
fn merge_json(
    file: &Path,
    key: &str,
    entry: Value,
    pre_existed: bool,
    backup_path: &Option<PathBuf>,
) -> Option<String> {
    let mut root = match read_json_object(file) {
        Some(map) => map,
        None if pre_existed => {
            tracing::warn!(
                "orchestrator MCP: existing {} is not a JSON object; not registering",
                file.display()
            );
            drop_backup(backup_path);
            return None;
        }
        None => Map::new(),
    };
    let mut servers = match root.get(key).and_then(|v| v.as_object().cloned()) {
        Some(obj) => obj,
        None if root.contains_key(key) => {
            tracing::warn!(
                "orchestrator MCP: existing {} key in {} is not an object; not registering",
                key,
                file.display()
            );
            drop_backup(backup_path);
            return None;
        }
        None => Map::new(),
    };
    servers.insert("agtx".to_string(), entry);
    root.insert(key.to_string(), Value::Object(servers));
    Some(
        serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default(),
    )
}

/// Build the merged TOML content (the table appended to the existing file),
/// or `None` to launch bare. A pre-existing file that does not parse as
/// TOML, or whose `mcp_servers` is not a plain table without `agtx`, is left
/// alone: the appended table would then duplicate a key or land under
/// something that cannot take one, and the agent's config would stop
/// parsing.
fn merge_toml(
    file: &Path,
    table: &str,
    pre_existed: bool,
    backup_path: &Option<PathBuf>,
) -> Option<String> {
    if !pre_existed {
        return Some(table.to_string());
    }
    let existing = match std::fs::read_to_string(file) {
        Ok(s) => s,
        Err(e) => {
            tracing::warn!(
                "orchestrator MCP: cannot read existing {}: {}; not registering",
                file.display(),
                e
            );
            drop_backup(backup_path);
            return None;
        }
    };
    let parsed = match toml::from_str::<toml::Value>(&existing) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                "orchestrator MCP: existing {} is not valid TOML ({e}); not registering",
                file.display()
            );
            drop_backup(backup_path);
            return None;
        }
    };
    let servers = parsed.as_table().and_then(|t| t.get("mcp_servers"));
    if let Some(v) = servers {
        let Some(t) = v.as_table() else {
            // e.g. `[[mcp_servers]]`: a sub-table under it is invalid TOML.
            tracing::warn!(
                "orchestrator MCP: existing {} mcp_servers is not a table; not registering",
                file.display()
            );
            drop_backup(backup_path);
            return None;
        };
        if t.contains_key("agtx") {
            // Already registered, in any spelling; the entry is deterministic
            // per project, so leave the user's file alone — and there is
            // nothing to clean up.
            drop_backup(backup_path);
            return None;
        }
    }
    let merged = if existing.trim().is_empty() {
        table.to_string()
    } else {
        format!("{}\n\n{}", existing.trim_end(), table)
    };
    Some(merged)
}

/// Remove a backup taken before a write that did not happen: an orphan in the
/// system temp dir would otherwise outlive the orchestrator session by
/// months.
fn drop_backup(backup_path: &Option<PathBuf>) {
    if let Some(bak) = backup_path {
        let _ = std::fs::remove_file(bak);
    }
}

/// Copy `file` to a unique name under the system temp dir; the cleanup
/// fragment restores from it after the orchestrator exits.
fn backup(file: &Path) -> Option<PathBuf> {
    let dest = std::env::temp_dir().join(format!("agtx-orch-mcp-{}", uuid::Uuid::new_v4()));
    std::fs::copy(file, &dest).ok()?;
    Some(dest)
}

/// The shell fragment that runs after the orchestrator exits: restore the
/// user's pre-launch file, or remove the one we created.
fn cleanup_fragment(file: &Path, backup_path: Option<&Path>) -> String {
    match backup_path {
        Some(bak) => format!(
            "cp -f {} {} && rm -f {}",
            quote(bak),
            quote(file),
            quote(bak)
        ),
        None => format!("rm -f {}", quote(file)),
    }
}

/// Wrap as one POSIX single-quoted shell word, escaping inner quotes by
/// close-literal-reopen. This fragment lives inside the orchestrator command
/// string, which `create_window` wraps in `sh -c '…'` — so, like the claude
/// JSON, the wrapping quotes must **not** be pre-escaped here.
fn quote(word: &Path) -> String {
    format!(
        "'{}'",
        word.display().to_string().replace('\'', "'\"'\"'")
    )
}

/// Parse a JSON file as an object; `None` if it is missing, unreadable, not
/// valid JSON, or not a JSON object.
fn read_json_object(path: &Path) -> Option<Map<String, Value>> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The first single-quoted word of a restore cleanup fragment is the
    /// backup. These tests build the fragment but never run it, so they remove
    /// it themselves — a test leaving files in the system temp dir is a leak.
    /// (Backup names are temp-dir + UUID, so no quote can appear inside.)
    fn remove_backup_from(cleanup: &str) {
        let rest = cleanup.strip_prefix("cp -f '").unwrap_or("");
        if let Some(end) = rest.find('\'') {
            let _ = std::fs::remove_file(&rest[..end]);
        }
    }

    #[test]
    fn every_file_based_kind_maps_to_a_target() {
        assert!(
            orchestrator_mcp_target(McpConfigKind::ClaudeJson).is_none(),
            "Claude registers via CLI, not a config file"
        );
        for kind in [
            McpConfigKind::OpenCode,
            McpConfigKind::CodexToml,
            McpConfigKind::GeminiJson,
            McpConfigKind::CursorJson,
            McpConfigKind::GrokTomlMerge,
            McpConfigKind::AntigravityJsonMerge,
            McpConfigKind::PiJsonMerge,
        ] {
            let rel = orchestrator_mcp_target(kind).expect("file-based kind must have a target");
            assert!(
                !rel.contains("..") && !rel.starts_with('/'),
                "{kind:?} target must stay inside the launch dir: {rel}"
            );
        }
    }

    #[test]
    fn gemini_entry_carries_trust() {
        let dir = tempfile::tempdir().unwrap();
        let cleanup = register(
            dir.path(),
            ".gemini/settings.json",
            McpConfigKind::GeminiJson,
            "/usr/bin/agtx",
            "/tmp/proj",
        )
        .unwrap();
        let cfg: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".gemini/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(cfg["mcpServers"]["agtx"]["trust"], true);
        assert_eq!(cfg["mcpServers"]["agtx"]["command"], "/usr/bin/agtx");
        assert_eq!(cfg["mcpServers"]["agtx"]["args"][1], "/tmp/proj");
        // Fresh file → cleanup removes it.
        assert!(cleanup.starts_with("rm -f '"), "{cleanup}");
    }

    #[test]
    fn pre_existing_json_survives_and_cleanup_restores_backup() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("opencode.json");
        std::fs::write(
            &cfg_path,
            serde_json::json!({ "theme": "dark", "mcp": { "other": { "type": "local" } } })
                .to_string(),
        )
        .unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let cleanup = register(
            dir.path(),
            "opencode.json",
            McpConfigKind::OpenCode,
            "/usr/bin/agtx",
            "/tmp/proj",
        )
        .unwrap();

        let merged: Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
        assert_eq!(merged["theme"], "dark", "user keys must survive");
        assert_eq!(
            merged["mcp"]["other"]["type"], "local",
            "other servers must survive"
        );
        assert_eq!(merged["mcp"]["agtx"]["type"], "local");
        assert_eq!(merged["mcp"]["agtx"]["command"][2], "/tmp/proj");

        // Pre-existing file → cleanup restores the backup, then removes it.
        assert!(
            cleanup.contains("cp -f '") && cleanup.contains(" && rm -f '"),
            "{cleanup}"
        );
        assert!(before.contains("dark"), "sanity");
        remove_backup_from(&cleanup);
    }

    #[test]
    fn unparseable_pre_existing_json_registers_nothing() {
        // e.g. JSONC with comments: valid to a lenient reader, not to us.
        // Registration must bail — the file must not be displaced with an
        // agtx-only entry, and the agent launches bare.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".gemini/settings.json");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "{ \"theme\": \"dark\", // user comment\n }").unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let result = register(
            dir.path(),
            ".gemini/settings.json",
            McpConfigKind::GeminiJson,
            "/usr/bin/agtx",
            "/tmp/proj",
        );

        assert!(result.is_none(), "unparseable existing file → no registration");
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            before,
            "the pre-existing file must be untouched"
        );
    }

    #[test]
    fn json_key_not_an_object_registers_nothing() {
        // `"mcp"` present but not a map: we cannot merge into it without
        // displacing the user's value.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("opencode.json");
        std::fs::write(&cfg_path, "{ \"mcp\": \"bogus\" }").unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let result = register(
            dir.path(),
            "opencode.json",
            McpConfigKind::OpenCode,
            "/usr/bin/agtx",
            "/tmp/proj",
        );

        assert!(result.is_none(), "non-object key value → no registration");
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            before,
            "the pre-existing file must be untouched"
        );
    }

    #[test]
    fn toml_table_appended_after_user_content() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".codex").join("config.toml");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "[sandbox]\nmode = \"workspace-write\"\n").unwrap();

        let cleanup = register(
            dir.path(),
            ".codex/config.toml",
            McpConfigKind::CodexToml,
            "/usr/bin/agtx",
            "/tmp/proj",
        )
        .unwrap();

        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(content.contains("[sandbox]"), "user table must survive");
        assert!(content.contains("[mcp_servers.agtx]"), "{content}");
        assert!(content.contains(r#"command = "/usr/bin/agtx""#), "{content}");
        assert!(content.contains(r#"args = ["mcp-serve", "/tmp/proj"]"#), "{content}");
        assert!(cleanup.starts_with("cp -f '"), "pre-existing file → restore: {cleanup}");
        remove_backup_from(&cleanup);
    }

    #[test]
    fn empty_pre_existing_toml_file_gets_table() {
        // A user's empty config file: appending is safe, and the empty
        // document must parse (not trip the bail discipline).
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".codex").join("config.toml");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "").unwrap();

        let cleanup = register(
            dir.path(),
            ".codex/config.toml",
            McpConfigKind::CodexToml,
            "/usr/bin/agtx",
            "/tmp/proj",
        )
        .expect("an empty pre-existing file is mergeable; must not bail");

        let content = std::fs::read_to_string(&cfg_path).unwrap();
        assert!(content.contains("[mcp_servers.agtx]"), "{content}");
        assert!(cleanup.starts_with("cp -f '"), "pre-existing file → restore: {cleanup}");
        remove_backup_from(&cleanup);
    }

    #[test]
    fn toml_already_registered_leaves_file_alone() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".grok").join("config.toml");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "[mcp_servers.agtx]\ncommand = \"other\"\n").unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let result = register(
            dir.path(),
            ".grok/config.toml",
            McpConfigKind::GrokTomlMerge,
            "/usr/bin/agtx",
            "/tmp/proj",
        );

        assert!(result.is_none(), "already registered → nothing to do");
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            before,
            "file must be untouched"
        );
    }

    #[test]
    fn toml_already_registered_inline_leaves_file_alone() {
        // Same server registered as an inline table: the string search for
        // `[mcp_servers.agtx]` misses it, but appending under it would
        // duplicate the key. Detection must be by parse, not by string.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".codex").join("config.toml");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "[mcp_servers]\nagtx = { command = \"other\" }\n").unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let result = register(
            dir.path(),
            ".codex/config.toml",
            McpConfigKind::CodexToml,
            "/usr/bin/agtx",
            "/tmp/proj",
        );

        assert!(result.is_none(), "inline-registered → nothing to do");
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            before,
            "file must be untouched"
        );
    }

    #[test]
    fn invalid_pre_existing_toml_registers_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join(".codex").join("config.toml");
        std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
        std::fs::write(&cfg_path, "not [valid toml = =\n").unwrap();
        let before = std::fs::read_to_string(&cfg_path).unwrap();

        let result = register(
            dir.path(),
            ".codex/config.toml",
            McpConfigKind::CodexToml,
            "/usr/bin/agtx",
            "/tmp/proj",
        );

        assert!(result.is_none(), "unparseable existing TOML → no registration");
        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            before,
            "the pre-existing file must be untouched"
        );
    }

    #[test]
    fn pre_existing_path_that_cannot_be_backed_up_registers_nothing() {
        // A directory where the config file should be: `exists()` is true but
        // the backup copy fails. Registration must bail — not merge into
        // something we cannot restore, and not leak a backup.
        let dir = tempfile::tempdir().unwrap();
        let cfg_path = dir.path().join("opencode.json");
        std::fs::create_dir_all(&cfg_path).unwrap();

        let result = register(
            dir.path(),
            "opencode.json",
            McpConfigKind::OpenCode,
            "/usr/bin/agtx",
            "/tmp/proj",
        );

        assert!(result.is_none(), "no backup possible → no registration");
        assert!(
            cfg_path.is_dir(),
            "the pre-existing path must be untouched"
        );
    }

    #[test]
    fn apostrophe_in_project_path_lands_intact_in_file_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let project = "/tmp/it's a repo";
        let cleanup = register(
            dir.path(),
            "opencode.json",
            McpConfigKind::OpenCode,
            "/usr/bin/agtx",
            project,
        )
        .unwrap();
        let cfg: Value =
            serde_json::from_str(&std::fs::read_to_string(dir.path().join("opencode.json"))
                .unwrap())
            .unwrap();
        assert_eq!(
            cfg["mcp"]["agtx"]["command"][2], project,
            "the project path must be intact in the written file"
        );
        assert!(cleanup.starts_with("rm -f '"), "{cleanup}");
    }
}
