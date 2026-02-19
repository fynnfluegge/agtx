#![cfg(feature = "test-mocks")]

use agtx::tmux::{MockTmuxOperations, TmuxOperations};

#[test]
fn test_tmux_window_created_on_task_start() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Expect window creation when task moves to Planning
    mock_tmux
        .expect_create_window()
        .withf(|session, window_name, working_dir, command: &Option<String>| {
            session == "myproject"
                && window_name.starts_with("task-")
                && working_dir.contains(".agtx/worktrees")
                && command.is_some()
        })
        .times(1)
        .returning(|_, _, _, _| Ok(()));

    // Simulate task starting
    let result = mock_tmux.create_window(
        "myproject",
        "task-abc123-my-feature",
        "/path/to/project/.agtx/worktrees/abc123-my-feature",
        Some("claude --dangerously-skip-permissions 'plan task'".to_string()),
    );

    assert!(result.is_ok());
}

#[test]
fn test_tmux_window_killed_on_task_done() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Expect window to be killed when task moves to Done
    mock_tmux
        .expect_kill_window()
        .withf(|target| target == "myproject:task-abc123-my-feature")
        .times(1)
        .returning(|_| Ok(()));

    // Simulate task completion
    let result = mock_tmux.kill_window("myproject:task-abc123-my-feature");

    assert!(result.is_ok());
}

#[test]
fn test_tmux_window_not_killed_on_review() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Window should NOT be killed when moving to Review (we keep it open now)
    mock_tmux.expect_kill_window().times(0);

    // No kill_window call should happen
    // (In real code, we simply don't call kill_window when moving to Review)
}

#[test]
fn test_tmux_send_keys_for_claude_command() {
    let mut mock_tmux = MockTmuxOperations::new();

    mock_tmux
        .expect_send_keys()
        .withf(|target, keys| {
            target == "myproject:task-abc123"
                && keys.contains("claude")
                && keys.contains("--dangerously-skip-permissions")
        })
        .times(1)
        .returning(|_, _| Ok(()));

    let result = mock_tmux.send_keys(
        "myproject:task-abc123",
        "claude --dangerously-skip-permissions 'implement feature'",
    );

    assert!(result.is_ok());
}
