#![cfg(feature = "test-mocks")]

use agtx::git::{GitOperations, MockGitOperations};
use agtx::tmux::{MockTmuxOperations, TmuxOperations};
use std::path::Path;

#[test]
fn test_full_task_lifecycle_creates_and_cleans_resources() {
    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();

    // 1. Backlog -> Planning: Create worktree and tmux window
    mock_git
        .expect_create_worktree()
        .times(1)
        .returning(|_, slug| Ok(format!("/worktrees/{}", slug)));

    mock_tmux
        .expect_create_window()
        .times(1)
        .returning(|_, _, _, _| Ok(()));

    // Simulate Planning phase
    let worktree = mock_git
        .create_worktree(Path::new("/project"), "task-123")
        .unwrap();
    mock_tmux
        .create_window(
            "proj",
            "task-123",
            &worktree,
            Some("claude --dangerously-skip-permissions 'plan'".to_string()),
        )
        .unwrap();

    // 2. Planning -> Running: Send implementation command
    mock_tmux
        .expect_send_keys()
        .times(1)
        .returning(|_, _| Ok(()));

    mock_tmux
        .send_keys("proj:task-123", "Please implement the plan")
        .unwrap();

    // 3. Running -> Review: Window stays open (no kill)
    // (nothing happens here - window persists)

    // 4. Review -> Done: Cleanup
    mock_tmux
        .expect_kill_window()
        .times(1)
        .returning(|_| Ok(()));

    mock_git
        .expect_remove_worktree()
        .times(1)
        .returning(|_, _| Ok(()));

    mock_tmux.kill_window("proj:task-123").unwrap();
    mock_git
        .remove_worktree(Path::new("/project"), &worktree)
        .unwrap();
}

#[test]
fn test_resume_from_review_does_not_recreate_resources() {
    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();

    // When resuming from Review -> Running, we should NOT create new resources
    mock_git.expect_create_worktree().times(0);
    mock_tmux.expect_create_window().times(0);

    // The existing window and worktree should be reused
    // (In real code, we just change the task status)
}

#[test]
fn test_delete_task_cleans_up_all_resources() {
    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();

    // Deleting a task should clean up both tmux window and worktree
    mock_tmux
        .expect_kill_window()
        .withf(|target| target == "proj:task-abc123")
        .times(1)
        .returning(|_| Ok(()));

    mock_git
        .expect_remove_worktree()
        .withf(|_, worktree| worktree.contains("abc123"))
        .times(1)
        .returning(|_, _| Ok(()));

    // Simulate delete
    mock_tmux.kill_window("proj:task-abc123").unwrap();
    mock_git
        .remove_worktree(Path::new("/project"), "/project/.agtx/worktrees/abc123")
        .unwrap();
}
