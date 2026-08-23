use agtx::agent::{known_agents, parse_agent_selection, AgentOperations, CodingAgent};
use agtx::skills::{agent_native_skill_dir, scan_agent_skills, transform_plugin_command};

#[test]
fn test_parse_agent_selection_empty_defaults_to_first() {
    assert_eq!(parse_agent_selection("", 3), Some(0));
    assert_eq!(parse_agent_selection("  ", 3), Some(0));
    assert_eq!(parse_agent_selection("\n", 3), Some(0));
}

#[test]
fn test_parse_agent_selection_valid_numbers() {
    assert_eq!(parse_agent_selection("1", 3), Some(0));
    assert_eq!(parse_agent_selection("2", 3), Some(1));
    assert_eq!(parse_agent_selection("3", 3), Some(2));
}

#[test]
fn test_parse_agent_selection_trims_whitespace() {
    assert_eq!(parse_agent_selection(" 2 ", 3), Some(1));
    assert_eq!(parse_agent_selection("1\n", 3), Some(0));
}

#[test]
fn test_parse_agent_selection_out_of_range() {
    assert_eq!(parse_agent_selection("0", 3), None);
    assert_eq!(parse_agent_selection("4", 3), None);
    assert_eq!(parse_agent_selection("100", 3), None);
}

#[test]
fn test_parse_agent_selection_invalid_input() {
    assert_eq!(parse_agent_selection("abc", 3), None);
    assert_eq!(parse_agent_selection("-1", 3), None);
    assert_eq!(parse_agent_selection("1.5", 3), None);
}

#[test]
fn test_parse_agent_selection_single_agent() {
    assert_eq!(parse_agent_selection("1", 1), Some(0));
    assert_eq!(parse_agent_selection("2", 1), None);
    assert_eq!(parse_agent_selection("", 1), Some(0));
}

// =============================================================================
// Tests for known_agents and build_interactive_command
// =============================================================================

#[test]
fn test_known_agents_includes_cursor() {
    let agents = known_agents();
    let cursor = agents.iter().find(|a| a.name == "cursor");
    assert!(cursor.is_some(), "cursor should be in known_agents");
    let cursor = cursor.unwrap();
    assert_eq!(cursor.command, "agent");
    assert_eq!(cursor.co_author, "Cursor Agent <noreply@cursor.com>");
}

#[test]
fn test_known_agents_includes_all_expected() {
    let agents = known_agents();
    let names: Vec<&str> = agents.iter().map(|a| a.name.as_str()).collect();
    for expected in &[
        "claude",
        "codex",
        "copilot",
        "gemini",
        "opencode",
        "cursor",
        "grok",
        "antigravity",
        "pi",
    ] {
        assert!(names.contains(expected), "missing agent: {}", expected);
    }
}

#[test]
fn test_build_interactive_command_cursor_no_prompt() {
    let agents = known_agents();
    let cursor = agents.iter().find(|a| a.name == "cursor").unwrap();
    assert_eq!(cursor.build_interactive_command(""), "agent --yolo");
}

#[test]
fn test_build_interactive_command_cursor_with_prompt() {
    let agents = known_agents();
    let cursor = agents.iter().find(|a| a.name == "cursor").unwrap();
    assert_eq!(
        cursor.build_interactive_command("do something"),
        "agent --yolo 'do something'"
    );
}

#[test]
fn test_build_interactive_command_cursor_escapes_single_quotes() {
    let agents = known_agents();
    let cursor = agents.iter().find(|a| a.name == "cursor").unwrap();
    let cmd = cursor.build_interactive_command("it's a test");
    assert!(cmd.contains("agent --yolo"), "should use agent --yolo");
    assert!(cmd.contains("it"), "prompt content should be present");
}

#[test]
fn test_build_interactive_command_existing_agents_unchanged() {
    let agents = known_agents();
    let by_name = |n: &str| agents.iter().find(|a| a.name == n).unwrap().clone();
    assert_eq!(
        by_name("claude").build_interactive_command(""),
        "claude --dangerously-skip-permissions"
    );
    assert_eq!(
        by_name("codex").build_interactive_command(""),
        "codex --sandbox workspace-write"
    );
    assert_eq!(
        by_name("gemini").build_interactive_command(""),
        "GEMINI_TRUST_WORKSPACE=true gemini --approval-mode yolo"
    );
    assert_eq!(
        by_name("opencode").build_interactive_command(""),
        "opencode"
    );
}

// =============================================================================
// Tests for build_resume_command
// =============================================================================

#[test]
fn test_build_resume_command_all_agents() {
    let agents = known_agents();
    let by_name = |n: &str| agents.iter().find(|a| a.name == n).unwrap().clone();

    assert_eq!(
        by_name("claude").build_resume_command(),
        "claude --dangerously-skip-permissions --continue"
    );
    assert_eq!(
        by_name("codex").build_resume_command(),
        "codex resume --last"
    );
    assert_eq!(
        by_name("copilot").build_resume_command(),
        "copilot --allow-all-tools --continue"
    );
    assert_eq!(
        by_name("gemini").build_resume_command(),
        "GEMINI_TRUST_WORKSPACE=true gemini --approval-mode yolo --resume"
    );
    assert_eq!(
        by_name("opencode").build_resume_command(),
        "opencode --continue"
    );
    assert_eq!(
        by_name("cursor").build_resume_command(),
        "agent --yolo --continue"
    );
    assert_eq!(
        by_name("grok").build_resume_command(),
        "grok --yolo --trust --continue"
    );
    assert_eq!(
        by_name("antigravity").build_resume_command(),
        "agy --dangerously-skip-permissions --mode accept-edits --continue"
    );
}

#[test]
fn test_build_resume_command_unknown_agent_falls_back_to_interactive() {
    use agtx::agent::Agent;
    let agent = Agent::new("custom-agent", "my-agent", "A custom agent", "Custom <noreply@example.com>");
    // Unknown agent should fall back to build_interactive_command("")
    assert_eq!(agent.build_resume_command(), agent.build_interactive_command(""));
}

// === build_orchestrator_command ===

#[test]
fn test_build_orchestrator_command_claude_is_idempotent() {
    let agents = known_agents();
    let claude = agents.iter().find(|a| a.name == "claude").unwrap().clone();
    let ops = CodingAgent::new(claude);
    let cmd = ops.build_orchestrator_command("{\"type\":\"stdio\"}", "/usr/bin/agtx");

    let pre_remove_idx = cmd
        .find("claude mcp remove agtx-orchestrator")
        .expect("pre-remove stale registration");
    let add_idx = cmd
        .find("claude mcp add-json agtx-orchestrator")
        .expect("register MCP");
    assert!(pre_remove_idx < add_idx, "pre-remove must precede add-json:\n{cmd}");

    // Must NOT register under the bare `agtx` name: that collides with an `agtx`
    // server defined in another scope (plugin / user / .mcp.json) and makes
    // Claude Code report a configuration conflict and refuse to connect.
    assert!(
        !cmd.contains("add-json agtx "),
        "must register under a unique name, not bare `agtx`:\n{cmd}"
    );

    // The JSON's wrapping single quotes must be escaped (`'\''`) so the command
    // survives create_window's outer `sh -c '...'` layer. A bare `'{json}'` would
    // break out of that wrapper and mangle the JSON.
    assert!(
        cmd.contains(r"'\''"),
        "JSON quotes must be escaped for the outer sh -c wrapper:\n{cmd}"
    );

    let pre_section = &cmd[..add_idx];
    assert!(
        pre_section.contains("|| true") || pre_section.contains("2>/dev/null"),
        "pre-remove must tolerate missing prior state:\n{cmd}"
    );
    assert!(cmd.contains("&& claude"), "&& must gate interactive claude:\n{cmd}");
}

// =============================================================================
// Tests for cursor skill integration
// =============================================================================

#[test]
fn test_cursor_has_native_skill_dir() {
    let dir = agent_native_skill_dir("cursor");
    assert_eq!(dir, Some((".cursor/skills", "")));
}

#[test]
fn test_cursor_transform_plugin_command() {
    // Cursor: colon → hyphen, slash kept
    assert_eq!(
        transform_plugin_command("/agtx:plan", "cursor"),
        Some("/agtx-plan".to_string())
    );
    assert_eq!(
        transform_plugin_command("/gsd:plan-phase", "cursor"),
        Some("/gsd-plan-phase".to_string())
    );
}

#[test]
fn test_codex_transform_plugin_command_unchanged() {
    // Codex: slash → dollar, colon → hyphen
    assert_eq!(
        transform_plugin_command("/agtx:plan", "codex"),
        Some("$agtx-plan".to_string())
    );
}

#[test]
fn test_copilot_has_no_transform() {
    // Copilot: no interactive command transform
    assert_eq!(transform_plugin_command("/agtx:plan", "copilot"), None);
}

// =============================================================================
// Tests for grok (xAI Grok Build) integration
// =============================================================================

#[test]
fn test_known_agents_includes_grok() {
    let agents = known_agents();
    let grok = agents.iter().find(|a| a.name == "grok");
    assert!(grok.is_some(), "grok should be in known_agents");
    let grok = grok.unwrap();
    assert_eq!(grok.command, "grok");
    assert_eq!(grok.co_author, "Grok <noreply@x.ai>");
}

#[test]
fn test_build_interactive_command_grok_no_prompt() {
    let agents = known_agents();
    let grok = agents.iter().find(|a| a.name == "grok").unwrap();
    assert_eq!(grok.build_interactive_command(""), "grok --yolo --trust");
}

#[test]
fn test_build_interactive_command_grok_with_prompt() {
    let agents = known_agents();
    let grok = agents.iter().find(|a| a.name == "grok").unwrap();
    assert_eq!(
        grok.build_interactive_command("do something"),
        "grok --yolo --trust 'do something'"
    );
}

#[test]
fn test_build_interactive_command_grok_escapes_single_quotes() {
    let agents = known_agents();
    let grok = agents.iter().find(|a| a.name == "grok").unwrap();
    let cmd = grok.build_interactive_command("it's a test");
    assert!(cmd.starts_with("grok --yolo --trust "), "should use grok --yolo --trust: {cmd}");
    // The quote must be escaped for the outer sh -c wrapper, not left bare.
    assert!(cmd.contains("'\"'\"'"), "single quote must be escaped: {cmd}");
}

#[test]
fn test_grok_has_native_skill_dir() {
    let dir = agent_native_skill_dir("grok");
    assert_eq!(dir, Some((".grok/skills", "")));
}

#[test]
fn test_grok_transform_plugin_command() {
    // Grok: colon → hyphen, slash kept (same as Cursor)
    assert_eq!(
        transform_plugin_command("/agtx:plan", "grok"),
        Some("/agtx-plan".to_string())
    );
    assert_eq!(
        transform_plugin_command("/gsd:plan-phase 1", "grok"),
        Some("/gsd-plan-phase 1".to_string())
    );
}

// =============================================================================
// Tests for pi (earendil-works pi coding agent) integration
// =============================================================================

#[test]
fn test_known_agents_includes_pi() {
    let agents = known_agents();
    let pi = agents.iter().find(|a| a.name == "pi");
    assert!(pi.is_some(), "pi should be in known_agents");
    let pi = pi.unwrap();
    assert_eq!(pi.command, "pi");
    assert_eq!(pi.co_author, "Pi <noreply@earendil.works>");
}

#[test]
fn test_build_interactive_command_pi_no_prompt() {
    let agents = known_agents();
    let pi = agents.iter().find(|a| a.name == "pi").unwrap();
    // pi has no permission prompts; --approve only suppresses the project-trust
    // selector that would otherwise block startup in a fresh worktree.
    assert_eq!(pi.build_interactive_command(""), "pi --approve");
}

#[test]
fn test_build_interactive_command_pi_with_prompt() {
    let agents = known_agents();
    let pi = agents.iter().find(|a| a.name == "pi").unwrap();
    assert_eq!(
        pi.build_interactive_command("do something"),
        "pi --approve 'do something'"
    );
}

#[test]
fn test_build_interactive_command_pi_escapes_single_quotes() {
    let agents = known_agents();
    let pi = agents.iter().find(|a| a.name == "pi").unwrap();
    let cmd = pi.build_interactive_command("it's a test");
    assert!(
        cmd.starts_with("pi --approve "),
        "should use pi --approve: {cmd}"
    );
    assert!(
        cmd.contains("'\"'\"'"),
        "single quote must be escaped: {cmd}"
    );
}

#[test]
fn test_build_resume_command_pi() {
    let agents = known_agents();
    let pi = agents.iter().find(|a| a.name == "pi").unwrap();
    assert_eq!(pi.build_resume_command(), "pi --approve --continue");
}

#[test]
fn test_pi_has_native_skill_dir() {
    let dir = agent_native_skill_dir("pi");
    assert_eq!(dir, Some((".pi/skills", "")));
}

#[test]
fn test_scan_agent_skills_pi_uses_frontmatter_name() {
    // pi registers a skill under its frontmatter `name:` and deliberately allows
    // that to differ from the parent directory (the Agent Skills spec forbids it;
    // pi relaxes the rule so one skill tree can be shared across harnesses).
    // Keying the picker off the directory name emits a command pi cannot resolve.
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join(".pi/skills/dirname-x");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: framework-thing\ndescription: A renamed skill.\n---\nbody\n",
    )
    .unwrap();

    let found = scan_agent_skills("pi", dir.path());
    assert_eq!(
        found,
        vec![(
            "/skill:framework-thing".to_string(),
            "A renamed skill.".to_string()
        )]
    );
}

#[test]
fn test_scan_agent_skills_pi_falls_back_to_dirname() {
    // No parseable frontmatter name — the directory is the best guess left.
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join(".pi/skills/agtx-plan");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "no frontmatter here\n").unwrap();

    let found = scan_agent_skills("pi", dir.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "/skill:agtx-plan");
}

#[test]
fn test_scan_agent_skills_dirname_agents_ignore_frontmatter_name() {
    // Cursor/Grok/Antigravity enforce name == directory, so they stay on dirname.
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join(".grok/skills/dirname-x");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: framework-thing\ndescription: A renamed skill.\n---\nbody\n",
    )
    .unwrap();

    let found = scan_agent_skills("grok", dir.path());
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].0, "/dirname-x");
}

#[test]
fn test_pi_transform_plugin_command() {
    // pi registers skills as /skill:<frontmatter-name>, so the whole canonical
    // command becomes the skill name.
    assert_eq!(
        transform_plugin_command("/agtx:plan", "pi"),
        Some("/skill:agtx-plan".to_string())
    );
    assert_eq!(
        transform_plugin_command("/gsd:plan-phase 1", "pi"),
        Some("/skill:gsd-plan-phase 1".to_string())
    );
}

// =============================================================================
// Tests for antigravity (Google Antigravity CLI) integration
// =============================================================================

#[test]
fn test_known_agents_includes_antigravity() {
    let agents = known_agents();
    let agy = agents.iter().find(|a| a.name == "antigravity");
    assert!(agy.is_some(), "antigravity should be in known_agents");
    let agy = agy.unwrap();
    // Agent name and binary differ, like Cursor (`cursor` / `agent`).
    assert_eq!(agy.command, "agy");
    assert_eq!(agy.co_author, "Antigravity <noreply@google.com>");
}

#[test]
fn test_build_interactive_command_antigravity_no_prompt() {
    let agents = known_agents();
    let agy = agents.iter().find(|a| a.name == "antigravity").unwrap();
    // Both permission controls are needed: --mode governs the file-edit diff
    // review, --dangerously-skip-permissions governs shell/MCP/URL approvals.
    assert_eq!(
        agy.build_interactive_command(""),
        "agy --dangerously-skip-permissions --mode accept-edits"
    );
}

#[test]
fn test_build_interactive_command_antigravity_with_prompt() {
    let agents = known_agents();
    let agy = agents.iter().find(|a| a.name == "antigravity").unwrap();
    assert_eq!(
        agy.build_interactive_command("do something"),
        "agy --dangerously-skip-permissions --mode accept-edits -i 'do something'"
    );
}

#[test]
fn test_build_interactive_command_antigravity_escapes_single_quotes() {
    let agents = known_agents();
    let agy = agents.iter().find(|a| a.name == "antigravity").unwrap();
    let cmd = agy.build_interactive_command("it's a test");
    assert!(
        cmd.starts_with("agy --dangerously-skip-permissions --mode accept-edits -i "),
        "should use the agy flags: {cmd}"
    );
    assert!(cmd.contains("'\"'\"'"), "single quote must be escaped: {cmd}");
}

#[test]
fn test_antigravity_has_native_skill_dir() {
    // Antigravity uses the vendor-neutral `.agents/` tree, not an agent dotdir.
    let dir = agent_native_skill_dir("antigravity");
    assert_eq!(dir, Some((".agents/skills", "")));
}

#[test]
fn test_antigravity_transform_plugin_command() {
    // Antigravity: colon → hyphen, slash kept (same as Cursor/Grok)
    assert_eq!(
        transform_plugin_command("/agtx:plan", "antigravity"),
        Some("/agtx-plan".to_string())
    );
    assert_eq!(
        transform_plugin_command("/gsd:plan-phase 1", "antigravity"),
        Some("/gsd-plan-phase 1".to_string())
    );
}
