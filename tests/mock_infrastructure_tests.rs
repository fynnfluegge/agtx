//! Tests for mock infrastructure
//!
//! These tests verify that the mock implementations work correctly.
//! They serve as a foundation for writing actual app logic tests.
//!
//! Run with: cargo test --features test-mocks

#![cfg(feature = "test-mocks")]

use std::path::Path;
use std::sync::Arc;

use agtx::agent::{AgentOperations, MockAgentOperations};
use agtx::git::{GitOperations, GitProviderOperations, MockGitOperations, MockGitProviderOperations};
use agtx::tmux::{MockTmuxOperations, TmuxOperations};

/// Test that mocks can be created and configured
/// This is a basic smoke test to verify the mock infrastructure works
#[test]
fn test_mock_infrastructure_works() {
    // Create mocks
    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_git_provider = MockGitProviderOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    // Configure expectations
    mock_tmux.expect_has_session()
        .returning(|_| false);

    mock_tmux.expect_create_session()
        .returning(|_, _| Ok(()));

    mock_git.expect_list_files()
        .returning(|_| vec!["src/main.rs".to_string(), "Cargo.toml".to_string()]);

    mock_git_provider.expect_get_pr_state()
        .returning(|_, _| Ok(agtx::git::PullRequestState::Open));

    mock_agent.expect_co_author_string()
        .return_const("Test Agent <test@example.com>".to_string());

    // Verify mocks work as expected
    assert!(!mock_tmux.has_session("test-session"));
    assert!(mock_tmux.create_session("test-session", "/tmp").is_ok());

    let files = mock_git.list_files(Path::new("/tmp"));
    assert_eq!(files.len(), 2);
    assert!(files.contains(&"src/main.rs".to_string()));

    let pr_state = mock_git_provider.get_pr_state(Path::new("/tmp"), 123).unwrap();
    assert_eq!(pr_state, agtx::git::PullRequestState::Open);

    assert_eq!(mock_agent.co_author_string(), "Test Agent <test@example.com>");
}

/// Test git operations mock for PR workflow
#[test]
fn test_git_operations_mock_for_pr_workflow() {
    let mut mock_git = MockGitOperations::new();

    // Setup expectations for a typical PR creation flow
    mock_git.expect_add_all()
        .times(1)
        .returning(|_| Ok(()));

    mock_git.expect_has_changes()
        .times(1)
        .returning(|_| true);

    mock_git.expect_commit()
        .times(1)
        .withf(|_: &Path, msg: &str| msg.contains("Test commit"))
        .returning(|_, _| Ok(()));

    mock_git.expect_push()
        .times(1)
        .withf(|_, branch, set_upstream| branch == "feature/test" && *set_upstream)
        .returning(|_, _, _| Ok(()));

    // Execute the workflow
    let worktree = Path::new("/tmp/worktree");

    mock_git.add_all(worktree).unwrap();
    assert!(mock_git.has_changes(worktree));
    mock_git.commit(worktree, "Test commit message").unwrap();
    mock_git.push(worktree, "feature/test", true).unwrap();
}

/// Test tmux operations mock for session management
#[test]
fn test_tmux_session_management() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Session doesn't exist initially
    mock_tmux.expect_has_session()
        .with(mockall::predicate::eq("my-project"))
        .times(1)
        .returning(|_| false);

    // Create session
    mock_tmux.expect_create_session()
        .with(
            mockall::predicate::eq("my-project"),
            mockall::predicate::eq("/home/user/project"),
        )
        .times(1)
        .returning(|_, _| Ok(()));

    // Create window in session
    mock_tmux.expect_create_window()
        .times(1)
        .returning(|_, _, _, _| Ok(()));

    // Session exists after creation
    mock_tmux.expect_has_session()
        .with(mockall::predicate::eq("my-project"))
        .times(1)
        .returning(|_| true);

    // Execute
    assert!(!mock_tmux.has_session("my-project"));
    mock_tmux.create_session("my-project", "/home/user/project").unwrap();
    mock_tmux.create_window("my-project", "task-1", "/home/user/project/.agtx/worktrees/task-1", None).unwrap();
    assert!(mock_tmux.has_session("my-project"));
}

/// Test agent operations mock
#[test]
fn test_agent_operations_mock() {
    let mut mock_agent = MockAgentOperations::new();

    mock_agent.expect_generate_text()
        .withf(|_: &Path, prompt: &str| prompt.contains("PR description"))
        .times(1)
        .returning(|_, _| Ok("This PR adds a new feature for testing.".to_string()));

    mock_agent.expect_co_author_string()
        .return_const("Claude <noreply@anthropic.com>".to_string());

    let result = mock_agent.generate_text(
        Path::new("/tmp"),
        "Generate a PR description for this change",
    ).unwrap();

    assert!(result.contains("new feature"));
    assert_eq!(mock_agent.co_author_string(), "Claude <noreply@anthropic.com>");
}

/// Test that mocks can be wrapped in Arc for thread-safe sharing
#[test]
fn test_mocks_can_be_arc_wrapped() {
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_list_files()
        .returning(|_| vec!["file1.rs".to_string()]);

    // Wrap in Arc (this is how App uses them)
    let arc_git: Arc<dyn GitOperations> = Arc::new(mock_git);

    // Can be cloned and used
    let arc_git_clone: Arc<dyn GitOperations> = Arc::clone(&arc_git);

    let files = arc_git_clone.list_files(Path::new("/tmp"));
    assert_eq!(files.len(), 1);
}
