/// Default skill content for agtx phases.
/// Skills follow the Agent Skills spec (SKILL.md with YAML frontmatter + markdown).
/// Content is loaded from .md files at compile time via include_str!().

pub const RESEARCH_SKILL: &str = include_str!("../skills/research.md");
pub const PLAN_SKILL: &str = include_str!("../skills/plan.md");
pub const EXECUTE_SKILL: &str = include_str!("../skills/execute.md");
pub const REVIEW_SKILL: &str = include_str!("../skills/review.md");

/// Default built-in skills: (directory_name, SKILL.md content)
/// Used for worktree phases (Research, Planning, Running, Review)
pub const DEFAULT_SKILLS: &[(&str, &str)] = &[
    ("agtx-research", RESEARCH_SKILL),
    ("agtx-plan", PLAN_SKILL),
    ("agtx-execute", EXECUTE_SKILL),
    ("agtx-review", REVIEW_SKILL),
];

/// Default task prompts per phase transition.
/// These are sent as the task content message AFTER the skill command.
/// For agents with native skill invocation, the skill is invoked separately via send_keys.
pub const DEFAULT_PROMPT_RESEARCH: &str =
    "Task: {task}\n\nWrite your findings to .agtx/research/{task_id}.md";
pub const DEFAULT_PROMPT_PLANNING: &str =
    "Task: {task}";
pub const DEFAULT_PROMPT_PLANNING_WITH_RESEARCH: &str =
    "Task: {task}\n\nResearch findings are available in .agtx/research.md — use them as context.";
pub const DEFAULT_PROMPT_RUNNING: &str =
    "Plan approved. Implement the changes described in .agtx/plan.md";
pub const DEFAULT_PROMPT_REVIEW: &str =
    "Implementation complete. Review the changes.";

/// Agent-native command/skill directory paths.
/// Returns (base_dir_relative_to_worktree, namespace_subdir) or None if agent has no native discovery.
/// Returns (base_dir_relative_to_worktree, namespace_subdir) or None if agent has no native discovery.
/// For Codex, namespace is empty because skills go directly under `.codex/skills/{skill-name}/SKILL.md`.
pub fn agent_native_skill_dir(agent_name: &str) -> Option<(&'static str, &'static str)> {
    match agent_name {
        "claude" => Some((".claude/commands", "agtx")),
        "gemini" => Some((".gemini/commands", "agtx")),
        "opencode" => Some((".opencode/commands", "")),
        "codex" => Some((".codex/skills", "")),
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
            let short = skill_dir_name.strip_prefix("agtx-").unwrap_or(skill_dir_name);
            format!("{}.toml", short)
        }
        "opencode" => {
            // Flat structure: command/agtx-plan.md (invoked as /agtx-plan)
            format!("{}.md", skill_dir_name)
        }
        _ => {
            let short = skill_dir_name.strip_prefix("agtx-").unwrap_or(skill_dir_name);
            format!("{}.md", short)
        }
    }
}

/// Generate the send_keys command to invoke a skill interactively.
/// Returns None for agents without interactive skill invocation.
///
/// Each agent has a different syntax:
/// - Claude: `/agtx:plan` (slash command from .claude/commands/)
/// - Gemini: `/agtx:plan` (slash command from .gemini/commands/)
/// - OpenCode: `/agtx-plan` (slash command, hyphen separator)
/// - Codex: `$agtx-plan` (dollar-sign mention from .codex/skills/)
pub fn skill_invocation_command(agent_name: &str, skill_dir_name: &str) -> Option<String> {
    match agent_name {
        "claude" | "gemini" => {
            let cmd = skill_name_to_command(skill_dir_name);
            Some(format!("/{}", cmd))
        }
        "opencode" => {
            Some(format!("/{}", skill_dir_name))
        }
        "codex" => {
            Some(format!("${}", skill_dir_name))
        }
        _ => None,
    }
}

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
        _ => None,
    }
}

/// Generate skill reference text for prompts (fallback for agents without interactive invocation).
/// For agents with skill invocation: empty string (skill is invoked via send_keys).
/// For agents without: "Follow the instructions in .agtx/skills/agtx-plan/SKILL.md"
pub fn skill_reference(agent_name: &str, skill_dir_name: &str) -> String {
    if skill_invocation_command(agent_name, skill_dir_name).is_some() {
        String::new()
    } else {
        format!(
            "Follow the instructions in .agtx/skills/{}/SKILL.md",
            skill_dir_name
        )
    }
}

/// Map a phase name to the corresponding skill directory name.
pub fn phase_to_skill_dir(phase: &str) -> &'static str {
    match phase {
        "research" => "agtx-research",
        "planning" | "planning_with_research" => "agtx-plan",
        "running" => "agtx-execute",
        "review" => "agtx-review",
        _ => "",
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
        "void",
        "Plain agent session - no prompting or skills",
        include_str!("../plugins/void/plugin.toml"),
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
pub fn scan_agent_skills(agent_name: &str, project_path: &std::path::Path) -> Vec<(String, String)> {
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
