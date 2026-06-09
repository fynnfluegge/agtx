/// Default skill content for agtx phases.
/// Skills follow the Agent Skills spec (SKILL.md with YAML frontmatter + markdown).
/// Content is loaded from .md files at compile time via include_str!().

pub const RESEARCH_SKILL: &str = include_str!("../plugins/agtx/skills/research.md");
pub const PLAN_SKILL: &str = include_str!("../plugins/agtx/skills/plan.md");
pub const EXECUTE_SKILL: &str = include_str!("../plugins/agtx/skills/execute.md");
pub const REVIEW_SKILL: &str = include_str!("../plugins/agtx/skills/review.md");
pub const ORCHESTRATE_SKILL: &str = include_str!("../plugins/agtx/skills/orchestrate.md");
pub const MERGE_CONFLICTS_SKILL: &str = include_str!("../plugins/agtx/skills/merge-conflicts.md");

/// Default built-in skills: (directory_name, SKILL.md content)
/// Used for worktree phases (Research, Planning, Running, Review)
pub const BUILTIN_SKILLS: &[(&str, &str)] = &[
    ("agtx-research", RESEARCH_SKILL),
    ("agtx-plan", PLAN_SKILL),
    ("agtx-execute", EXECUTE_SKILL),
    ("agtx-review", REVIEW_SKILL),
    ("agtx-orchestrate", ORCHESTRATE_SKILL),
    ("agtx-merge-conflicts", MERGE_CONFLICTS_SKILL),
];

/// Load a bundled plugin by name from compile-time embedded TOML.
pub fn load_bundled_plugin(name: &str) -> Option<crate::config::WorkflowPlugin> {
    BUNDLED_PLUGINS
        .iter()
        .find(|(n, _, _)| *n == name)
        .and_then(|(_, _, content)| toml::from_str(content).ok())
}

/// A custom (on-disk) workflow plugin discovered in a plugins directory.
pub struct CustomPlugin {
    pub name: String,
    pub description: String,
    pub plugin: crate::config::WorkflowPlugin,
}

/// Discover custom plugins on disk that are not shipped as bundled plugins.
///
/// Scans the global plugins directory (`~/.config/agtx/plugins/`) then the
/// project-local one (`{project}/.agtx/plugins/`). A project-local plugin
/// shadows a global one of the same name, matching [`crate::config::WorkflowPlugin::load`].
/// Names matching a bundled plugin are skipped: the bundled entry already
/// represents them in pickers, and `load` resolves the on-disk copy transparently.
pub fn discover_custom_plugins(project_path: Option<&std::path::Path>) -> Vec<CustomPlugin> {
    use crate::config::WorkflowPlugin;
    let mut dirs: Vec<std::path::PathBuf> = Vec::new();
    if let Some(dir) = WorkflowPlugin::global_plugins_dir() {
        dirs.push(dir);
    }
    if let Some(pp) = project_path {
        dirs.push(WorkflowPlugin::project_plugins_dir(pp));
    }
    discover_custom_plugins_in(&dirs)
}

/// Discover custom plugins across the given plugins directories, in order of
/// increasing precedence (later directories shadow earlier ones by name).
fn discover_custom_plugins_in(dirs: &[std::path::PathBuf]) -> Vec<CustomPlugin> {
    use std::collections::BTreeMap;
    let mut found: BTreeMap<String, CustomPlugin> = BTreeMap::new();
    for dir in dirs {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            if BUNDLED_PLUGINS.iter().any(|(n, _, _)| *n == name) {
                continue;
            }
            if crate::config::WorkflowPlugin::validate_plugin_name(name).is_err() {
                continue;
            }
            let Ok(content) = std::fs::read_to_string(path.join("plugin.toml")) else {
                continue;
            };
            let Ok(plugin) = toml::from_str::<crate::config::WorkflowPlugin>(&content) else {
                continue;
            };
            let description = plugin
                .description
                .clone()
                .unwrap_or_else(|| "Custom workflow plugin".to_string());
            found.insert(
                name.to_string(),
                CustomPlugin {
                    name: name.to_string(),
                    description,
                    plugin,
                },
            );
        }
    }
    found.into_values().collect()
}

/// Agent-native command/skill directory paths.
/// Returns (base_dir_relative_to_worktree, namespace_subdir) or None if agent has no native discovery.
/// Returns (base_dir_relative_to_worktree, namespace_subdir) or None if agent has no native discovery.
/// For Codex, namespace is empty because skills go directly under `.codex/skills/{skill-name}/SKILL.md`.
pub fn agent_native_skill_dir(agent_name: &str) -> Option<(&'static str, &'static str)> {
    match agent_name {
        "claude" => Some((".claude/commands", "agtx")),
        "gemini" => Some((".gemini/commands", "agtx")),
        "opencode" => Some((".opencode/command", "")),
        "codex" => Some((".codex/skills", "")),
        "cursor" => Some((".cursor/skills", "")),
        "copilot" => Some((".github/agents", "agtx")),
        _ => None,
    }
}

/// Transform SKILL.md frontmatter `name: agtx-plan` to command name `agtx:plan`.
/// Replaces the first hyphen with `:`.
pub fn skill_name_to_command(skill_name: &str) -> String {
    if let Some(pos) = skill_name.find('-') {
        format!("{}:{}", &skill_name[..pos], &skill_name[pos + 1..])
    } else {
        skill_name.to_string()
    }
}

/// Map internal skill directory name to agent-native command file name.
/// Format depends on the agent:
/// - Claude/Gemini: "agtx-plan" → "plan.md" / "plan.toml" (namespace subdir handles prefix)
/// - OpenCode: "agtx-plan" → "agtx-plan.md" (flat directory, full name)
/// - Codex: uses SKILL.md in skill directories (handled separately)
pub fn skill_dir_to_filename(skill_dir_name: &str, agent_name: &str) -> String {
    match agent_name {
        "gemini" => {
            let short = skill_dir_name
                .strip_prefix("agtx-")
                .unwrap_or(skill_dir_name);
            format!("{}.toml", short)
        }
        "opencode" => {
            // Flat structure: command/agtx-plan.md (invoked as /agtx-plan)
            format!("{}.md", skill_dir_name)
        }
        _ => {
            let short = skill_dir_name
                .strip_prefix("agtx-")
                .unwrap_or(skill_dir_name);
            format!("{}.md", short)
        }
    }
}

/// Generate the send_keys command to invoke a skill interactively.
/// Returns None for agents without interactive skill invocation.
/// Transform a canonical plugin command (Claude/Gemini format) for a specific agent.
///
/// Plugin commands in plugin.toml are stored in canonical form: `/namespace:command args`
/// This transforms them for each agent's expected syntax:
/// - Claude/Gemini: unchanged (`/gsd:plan-phase 1`)
/// - OpenCode: colon → hyphen (`/gsd-plan-phase 1`)
/// - Codex: slash → dollar + colon → hyphen (`$gsd-plan-phase 1`)
/// - Unsupported agents: None (will fall back to file-path reference)
pub fn transform_plugin_command(canonical_cmd: &str, agent_name: &str) -> Option<String> {
    match agent_name {
        "claude" | "gemini" => Some(canonical_cmd.to_string()),
        "opencode" => {
            // /gsd:plan-phase 1 → /gsd-plan-phase 1
            Some(canonical_cmd.replacen(':', "-", 1))
        }
        "codex" => {
            // /gsd:plan-phase 1 → $gsd-plan-phase 1
            let transformed = canonical_cmd.replacen(':', "-", 1);
            if let Some(rest) = transformed.strip_prefix('/') {
                Some(format!("${}", rest))
            } else {
                Some(transformed)
            }
        }
        "cursor" => {
            // /agtx:plan → /agtx-plan (slash kept, colon → hyphen)
            Some(canonical_cmd.replacen(':', "-", 1))
        }
        _ => None,
    }
}

/// Strip YAML frontmatter from a skill file, returning just the body content.
pub fn strip_frontmatter(content: &str) -> &str {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let after = &content[3 + end + 3..];
            return after.trim_start_matches('\n');
        }
    }
    content
}

/// Convert skill content to Gemini TOML command format.
/// Gemini commands are .toml files with `description` and `prompt` fields.
pub fn skill_to_gemini_toml(description: &str, skill_content: &str) -> String {
    let body = strip_frontmatter(skill_content);
    // Escape backslashes and triple-quotes for TOML multi-line strings
    let escaped = body.replace('\\', "\\\\").replace("\"\"\"", "\\\"\\\"\\\"");
    format!(
        "description = \"{}\"\n\nprompt = \"\"\"\n{}\n\"\"\"\n",
        description.replace('"', "\\\""),
        escaped
    )
}

/// Bundled plugin configurations: (name, description, plugin.toml content)
/// These are embedded at compile time so the TUI can install them without external files.
pub const BUNDLED_PLUGINS: &[(&str, &str, &str)] = &[
    (
        "agtx",
        "Built-in workflow with skills and prompts",
        include_str!("../plugins/agtx/plugin.toml"),
    ),
    (
        "agtx-terse",
        "Token-efficient workflow - same workflow with compressed output and minimal tokens",
        include_str!("../plugins/agtx-terse/plugin.toml"),
    ),
    (
        "gsd",
        "Get Shit Done - structured spec-driven development",
        include_str!("../plugins/gsd/plugin.toml"),
    ),
    (
        "spec-kit",
        "Spec-Driven Development by GitHub",
        include_str!("../plugins/spec-kit/plugin.toml"),
    ),
    (
        "openspec",
        "OpenSpec - lightweight AI-guided specification framework",
        include_str!("../plugins/openspec/plugin.toml"),
    ),
    (
        "void",
        "Plain agent session - no prompting or skills",
        include_str!("../plugins/void/plugin.toml"),
    ),
    (
        "bmad",
        "BMAD Method - AI-driven agile development",
        include_str!("../plugins/bmad/plugin.toml"),
    ),
    (
        "superpowers",
        "Superpowers - brainstorming, plans, TDD, subagent-driven development",
        include_str!("../plugins/superpowers/plugin.toml"),
    ),
    (
        "oh-my-claudecode",
        "oh-my-claudecode - Multi-agent orchestration",
        include_str!("../plugins/oh-my-claudecode/plugin.toml"),
    ),
    (
        "agent-skills",
        "Agent Skills - production-grade engineering skills for the full spec-to-ship lifecycle",
        include_str!("../plugins/agent-skills/plugin.toml"),
    ),
];

/// Extract the description from YAML frontmatter.
pub fn extract_description(content: &str) -> Option<String> {
    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---") {
            let frontmatter = &content[3..3 + end];
            for line in frontmatter.lines() {
                if let Some(desc) = line.strip_prefix("description:") {
                    return Some(desc.trim().to_string());
                }
            }
        }
    }
    None
}

/// Enumerate built-in skills as agent-native `(command, description)` pairs.
/// Uses compile-time embedded `BUILTIN_SKILLS` — no filesystem access needed.
pub fn enumerate_available_skills(agent_name: &str) -> Vec<(String, String)> {
    let mut results = Vec::new();
    for (skill_name, skill_content) in BUILTIN_SKILLS {
        let canonical = format!("/{}", skill_name_to_command(skill_name));
        let command = match transform_plugin_command(&canonical, agent_name) {
            Some(cmd) => cmd,
            None => canonical,
        };
        let description =
            extract_description(skill_content).unwrap_or_else(|| skill_name.replace('-', " "));
        results.push((command, description));
    }
    results
}

/// Extract description from a markdown file with YAML frontmatter on disk.
fn extract_description_from_file(path: &std::path::Path) -> Option<String> {
    let mut buf = vec![0u8; 512];
    let mut file = std::fs::File::open(path).ok()?;
    let n = std::io::Read::read(&mut file, &mut buf).ok()?;
    let content = std::str::from_utf8(&buf[..n]).ok()?;
    extract_description(content)
}

/// Extract description from a Gemini TOML command file on disk.
fn extract_description_from_toml(path: &std::path::Path) -> Option<String> {
    let mut buf = vec![0u8; 512];
    let mut file = std::fs::File::open(path).ok()?;
    let n = std::io::Read::read(&mut file, &mut buf).ok()?;
    let content = std::str::from_utf8(&buf[..n]).ok()?;
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("description") {
            let rest = rest.trim_start();
            if let Some(rest) = rest.strip_prefix('=') {
                let rest = rest.trim();
                let rest = rest.trim_start_matches('"').trim_end_matches('"');
                if !rest.is_empty() {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// Scan the active agent's native command directory for available skills.
/// Returns `(command, description)` tuples in agent-native invocation format.
pub fn scan_agent_skills(
    agent_name: &str,
    project_path: &std::path::Path,
) -> Vec<(String, String)> {
    let mut results = Vec::new();

    match agent_name {
        "claude" | "copilot" => {
            // Namespaced subdirectories with .md files
            let (base_dir, _) = match agent_native_skill_dir(agent_name) {
                Some(v) => v,
                None => return results,
            };
            let base = project_path.join(base_dir);
            let entries = match std::fs::read_dir(&base) {
                Ok(e) => e,
                Err(_) => return results,
            };
            for ns_entry in entries.flatten() {
                if !ns_entry.path().is_dir() {
                    continue;
                }
                let namespace = ns_entry.file_name().to_string_lossy().to_string();
                let files = match std::fs::read_dir(ns_entry.path()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for file_entry in files.flatten() {
                    let path = file_entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("md") {
                        continue;
                    }
                    let stem = match path.file_stem().and_then(|s| s.to_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    let command = format!("/{}:{}", namespace, stem);
                    let description = extract_description_from_file(&path)
                        .unwrap_or_else(|| stem.replace('-', " "));
                    results.push((command, description));
                }
            }
        }
        "gemini" => {
            // Namespaced subdirectories with .toml files
            let (base_dir, _) = match agent_native_skill_dir(agent_name) {
                Some(v) => v,
                None => return results,
            };
            let base = project_path.join(base_dir);
            let entries = match std::fs::read_dir(&base) {
                Ok(e) => e,
                Err(_) => return results,
            };
            for ns_entry in entries.flatten() {
                if !ns_entry.path().is_dir() {
                    continue;
                }
                let namespace = ns_entry.file_name().to_string_lossy().to_string();
                let files = match std::fs::read_dir(ns_entry.path()) {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for file_entry in files.flatten() {
                    let path = file_entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                        continue;
                    }
                    let stem = match path.file_stem().and_then(|s| s.to_str()) {
                        Some(s) => s.to_string(),
                        None => continue,
                    };
                    let command = format!("/{}:{}", namespace, stem);
                    let description = extract_description_from_toml(&path)
                        .unwrap_or_else(|| stem.replace('-', " "));
                    results.push((command, description));
                }
            }
        }
        "codex" => {
            // Skill subdirectories with SKILL.md
            let base = project_path.join(".codex/skills");
            let entries = match std::fs::read_dir(&base) {
                Ok(e) => e,
                Err(_) => return results,
            };
            for dir_entry in entries.flatten() {
                if !dir_entry.path().is_dir() {
                    continue;
                }
                let dirname = dir_entry.file_name().to_string_lossy().to_string();
                let skill_file = dir_entry.path().join("SKILL.md");
                if skill_file.exists() {
                    let command = format!("${}", dirname);
                    let description = extract_description_from_file(&skill_file)
                        .unwrap_or_else(|| dirname.replace('-', " "));
                    results.push((command, description));
                }
            }
        }
        "cursor" => {
            // Skill subdirectories with SKILL.md, invoked as /skill-name
            let base = project_path.join(".cursor/skills");
            let entries = match std::fs::read_dir(&base) {
                Ok(e) => e,
                Err(_) => return results,
            };
            for dir_entry in entries.flatten() {
                if !dir_entry.path().is_dir() {
                    continue;
                }
                let dirname = dir_entry.file_name().to_string_lossy().to_string();
                let skill_file = dir_entry.path().join("SKILL.md");
                if skill_file.exists() {
                    let command = format!("/{}", dirname);
                    let description = extract_description_from_file(&skill_file)
                        .unwrap_or_else(|| dirname.replace('-', " "));
                    results.push((command, description));
                }
            }
        }
        "opencode" => {
            // Flat directory with .md files
            let base = project_path.join(".config/opencode/command");
            let entries = match std::fs::read_dir(&base) {
                Ok(e) => e,
                Err(_) => return results,
            };
            for file_entry in entries.flatten() {
                let path = file_entry.path();
                if path.extension().and_then(|e| e.to_str()) != Some("md") {
                    continue;
                }
                let stem = match path.file_stem().and_then(|s| s.to_str()) {
                    Some(s) => s.to_string(),
                    None => continue,
                };
                let command = format!("/{}", stem);
                let description = stem.replace('-', " ");
                results.push((command, description));
            }
        }
        _ => {} // Copilot has no interactive invocation, unknown agents skipped
    }

    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn write_plugin(dir: &std::path::Path, name: &str, body: &str) {
        let plugin_dir = dir.join(name);
        fs::create_dir_all(&plugin_dir).unwrap();
        let content = format!("name = \"{}\"\n{}", name, body);
        fs::write(plugin_dir.join("plugin.toml"), content).unwrap();
    }

    #[test]
    fn discovers_custom_plugin_with_description() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "my-flow", "description = \"My custom flow\"\n");

        let found = discover_custom_plugins_in(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "my-flow");
        assert_eq!(found[0].description, "My custom flow");
    }

    #[test]
    fn falls_back_to_default_description_when_missing() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "no-desc", "");

        let found = discover_custom_plugins_in(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "Custom workflow plugin");
    }

    #[test]
    fn skips_names_colliding_with_bundled_plugins() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(tmp.path(), "gsd", "description = \"shadow\"\n");
        write_plugin(tmp.path(), "mine", "description = \"mine\"\n");

        let found = discover_custom_plugins_in(&[tmp.path().to_path_buf()]);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["mine"]);
    }

    #[test]
    fn ignores_dirs_without_plugin_toml_and_invalid_names() {
        let tmp = tempfile::tempdir().unwrap();
        fs::create_dir_all(tmp.path().join("empty-dir")).unwrap();
        fs::write(tmp.path().join("loose-file.toml"), "x").unwrap();
        write_plugin(tmp.path(), "valid", "description = \"ok\"\n");

        let found = discover_custom_plugins_in(&[tmp.path().to_path_buf()]);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["valid"]);
    }

    #[test]
    fn project_local_shadows_global_by_name() {
        let global = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        write_plugin(global.path(), "shared", "description = \"global\"\n");
        write_plugin(local.path(), "shared", "description = \"local\"\n");

        let dirs: Vec<PathBuf> = vec![global.path().to_path_buf(), local.path().to_path_buf()];
        let found = discover_custom_plugins_in(&dirs);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].description, "local");
    }

    #[test]
    fn skips_bundled_name_in_global_keeps_local_custom() {
        let global = tempfile::tempdir().unwrap();
        let local = tempfile::tempdir().unwrap();
        write_plugin(global.path(), "gsd", "description = \"shadow\"\n");
        write_plugin(local.path(), "mine", "description = \"mine\"\n");

        let dirs: Vec<PathBuf> = vec![global.path().to_path_buf(), local.path().to_path_buf()];
        let found = discover_custom_plugins_in(&dirs);
        let names: Vec<&str> = found.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["mine"]);
    }

    #[test]
    fn discovered_plugin_carries_supported_agents_for_wizard_filter() {
        let tmp = tempfile::tempdir().unwrap();
        write_plugin(
            tmp.path(),
            "claude-only",
            "description = \"c\"\nsupported_agents = [\"claude\"]\n",
        );

        let found = discover_custom_plugins_in(&[tmp.path().to_path_buf()]);
        assert_eq!(found.len(), 1);
        assert!(found[0].plugin.supports_agent("claude"));
        assert!(!found[0].plugin.supports_agent("codex"));
    }
}
