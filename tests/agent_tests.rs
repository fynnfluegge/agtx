use agtx::agent::{self, Agent};

// =============================================================================
// Tests for build_interactive_command
// =============================================================================

#[test]
fn test_build_interactive_command_claude_no_flags() {
    let agent = agent::get_agent("claude").unwrap();
    let cmd = agent.build_interactive_command("fix the bug", &[]);
    assert_eq!(cmd, "claude 'fix the bug'");
}

#[test]
fn test_build_interactive_command_claude_with_flags() {
    let agent = agent::get_agent("claude").unwrap();
    let flags = vec!["--dangerously-skip-permissions".to_string()];
    let cmd = agent.build_interactive_command("fix the bug", &flags);
    assert_eq!(cmd, "claude --dangerously-skip-permissions 'fix the bug'");
}

#[test]
fn test_build_interactive_command_aider_with_flags() {
    let agent = agent::get_agent("aider").unwrap();
    let flags = vec!["--no-auto-commits".to_string()];
    let cmd = agent.build_interactive_command("fix the bug", &flags);
    assert_eq!(cmd, "aider --no-auto-commits --message 'fix the bug'");
}

#[test]
fn test_build_interactive_command_codex_no_flags() {
    let agent = agent::get_agent("codex").unwrap();
    let cmd = agent.build_interactive_command("implement feature", &[]);
    assert_eq!(cmd, "codex 'implement feature'");
}

#[test]
fn test_build_interactive_command_gh_copilot() {
    let agent = agent::get_agent("gh-copilot").unwrap();
    let cmd = agent.build_interactive_command("suggest a fix", &[]);
    assert_eq!(cmd, "gh copilot suggest 'suggest a fix'");
}

#[test]
fn test_build_interactive_command_q_agent() {
    let agent = agent::get_agent("q").unwrap();
    let cmd = agent.build_interactive_command("help me", &[]);
    assert_eq!(cmd, "q chat 'help me'");
}

#[test]
fn test_build_interactive_command_unknown_agent() {
    let agent = Agent::new("custom", "my-tool", "Custom tool", "Custom <noreply@example.com>");
    let cmd = agent.build_interactive_command("do work", &[]);
    assert_eq!(cmd, "my-tool 'do work'");
}

#[test]
fn test_build_interactive_command_escapes_single_quotes() {
    let agent = agent::get_agent("claude").unwrap();
    let cmd = agent.build_interactive_command("fix the user's bug", &[]);
    assert_eq!(cmd, "claude 'fix the user'\"'\"'s bug'");
}

// =============================================================================
// Tests for build_spawn_args
// =============================================================================

#[test]
fn test_build_spawn_args_claude() {
    let agent = agent::get_agent("claude").unwrap();
    let args = agent::build_spawn_args(&agent, "do task", "task-123");
    assert!(args.contains(&"--session".to_string()));
    assert!(args.contains(&"task-123".to_string()));
    assert!(args.contains(&"do task".to_string()));
}

#[test]
fn test_build_spawn_args_aider() {
    let agent = agent::get_agent("aider").unwrap();
    let args = agent::build_spawn_args(&agent, "do task", "task-456");
    assert!(args.contains(&"--message".to_string()));
    assert!(args.contains(&"do task".to_string()));
    assert!(!args.contains(&"--session".to_string()));
}

#[test]
fn test_build_spawn_args_gh_copilot() {
    let agent = agent::get_agent("gh-copilot").unwrap();
    let args = agent::build_spawn_args(&agent, "suggest", "task-789");
    assert!(args.contains(&"copilot".to_string()));
    assert!(args.contains(&"suggest".to_string()));
}

// =============================================================================
// Tests for known_agents and get_agent
// =============================================================================

#[test]
fn test_known_agents_returns_seven() {
    let agents = agent::known_agents();
    assert_eq!(agents.len(), 7);
}

#[test]
fn test_get_agent_found_and_not_found() {
    assert!(agent::get_agent("claude").is_some());
    assert!(agent::get_agent("aider").is_some());
    assert!(agent::get_agent("nonexistent").is_none());
}
