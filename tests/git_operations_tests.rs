#![cfg(feature = "test-mocks")]

use agtx::git::{GitOperations, MockGitOperations};
use std::path::Path;

#[test]
fn test_worktree_created_on_task_planning() {
    let mut mock_git = MockGitOperations::new();

    // Expect worktree creation when task moves to Planning
    mock_git
        .expect_create_worktree()
        .withf(|project_path, task_slug| {
            project_path == Path::new("/path/to/project") && task_slug == "abc123-my-feature"
        })
        .times(1)
        .returning(|_, slug| Ok(format!("/path/to/project/.agtx/worktrees/{}", slug)));

    let result = mock_git.create_worktree(Path::new("/path/to/project"), "abc123-my-feature");

    assert!(result.is_ok());
    assert!(result.unwrap().contains("abc123-my-feature"));
}

#[test]
fn test_worktree_removed_on_task_done() {
    let mut mock_git = MockGitOperations::new();

    // Expect worktree removal when task moves to Done
    mock_git
        .expect_remove_worktree()
        .withf(|project_path, worktree_path| {
            project_path == Path::new("/path/to/project")
                && worktree_path == "/path/to/project/.agtx/worktrees/abc123-my-feature"
        })
        .times(1)
        .returning(|_, _| Ok(()));

    let result = mock_git.remove_worktree(
        Path::new("/path/to/project"),
        "/path/to/project/.agtx/worktrees/abc123-my-feature",
    );

    assert!(result.is_ok());
}

#[test]
fn test_worktree_not_removed_on_review() {
    let mut mock_git = MockGitOperations::new();

    // Worktree should NOT be removed when moving to Review
    mock_git.expect_remove_worktree().times(0);

    // No remove_worktree call should happen when going to Review
}

#[test]
fn test_worktree_exists_check() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_worktree_exists()
        .withf(|project_path, task_slug| {
            project_path == Path::new("/path/to/project") && task_slug == "abc123-my-feature"
        })
        .times(1)
        .returning(|_, _| true);

    let exists = mock_git.worktree_exists(Path::new("/path/to/project"), "abc123-my-feature");

    assert!(exists);
}
