//! Tests for agent status detection
//!
//! Run with: cargo test --features test-mocks

#![cfg(feature = "test-mocks")]

use agtx::tmux::{MockTmuxOperations, TmuxOperations};

#[test]
fn test_is_pane_alive_returns_true_for_running_pane() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .with(mockall::predicate::eq("project:window"))
        .returning(|_| true);

    assert!(mock.is_pane_alive("project:window"));
}

#[test]
fn test_is_pane_alive_returns_false_for_dead_pane() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .with(mockall::predicate::eq("project:window"))
        .returning(|_| false);

    assert!(!mock.is_pane_alive("project:window"));
}
