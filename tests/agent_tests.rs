use agtx::agent::{known_agents, parse_agent_selection};
use agtx::skills::{agent_native_skill_dir, transform_plugin_command};

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
    for expected in &["claude", "codex", "copilot", "gemini", "opencode", "cursor"] {
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
        "codex --full-auto"
    );
    assert_eq!(
        by_name("gemini").build_interactive_command(""),
        "gemini --approval-mode yolo"
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
        "gemini --approval-mode yolo --resume"
    );
    assert_eq!(
        by_name("opencode").build_resume_command(),
        "opencode --continue"
    );
    assert_eq!(
        by_name("cursor").build_resume_command(),
        "agent --yolo --continue"
    );
}

#[test]
fn test_build_resume_command_unknown_agent_falls_back_to_interactive() {
    use agtx::agent::Agent;
    let agent = Agent::new("custom-agent", "my-agent", "A custom agent", "Custom <noreply@example.com>");
    // Unknown agent should fall back to build_interactive_command("")
    assert_eq!(agent.build_resume_command(), agent.build_interactive_command(""));
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
