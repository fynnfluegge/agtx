//! Tests for agent status detection
//!
//! Run with: cargo test --features test-mocks

#![cfg(feature = "test-mocks")]

use agtx::tmux::{MockTmuxOperations, TmuxOperations};
use agtx::tui::status::{detect_session_status, SessionStatus};

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

#[test]
fn test_detect_active_when_pane_alive_no_idle_pattern() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| true);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok("Analyzing src/main.rs...\nReading file contents...\n".to_string()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Active);
}

#[test]
fn test_detect_idle_claude_prompt() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| true);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok("Changes applied successfully.\n\n> ".to_string()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Idle);
}

#[test]
fn test_detect_idle_question_prompt() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| true);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok("Do you want to proceed?\n? ".to_string()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Idle);
}

#[test]
fn test_detect_idle_completion_phrase() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| true);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok("All done! What would you like to do next?\n\n> ".to_string()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Idle);
}

#[test]
fn test_detect_exited_shell_prompt() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| true);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok("user@host:~/project$ ".to_string()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Exited);
}

#[test]
fn test_detect_exited_dead_pane() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| false);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok(String::new()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Exited);
}

#[test]
fn test_detect_unknown_no_window() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_window_exists()
        .returning(|_| Ok(false));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Unknown);
}

#[test]
fn test_detect_strips_ansi_escapes() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive()
        .returning(|_| true);
    mock.expect_window_exists()
        .returning(|_| Ok(true));
    // ANSI-colored prompt: ESC[32m>ESC[0m followed by space
    mock.expect_capture_pane()
        .returning(|_| Ok("Done.\n\n\x1b[32m>\x1b[0m ".to_string()));

    let status = detect_session_status("proj:win", &mock);
    assert_eq!(status, SessionStatus::Idle);
}

// --- Cache TTL and edge case tests ---

use std::collections::HashMap;
use std::time::{Duration, Instant};

#[test]
fn test_cache_ttl_prevents_repeated_detection() {
    let now = Instant::now();
    let mut cache: HashMap<String, (SessionStatus, Instant)> = HashMap::new();

    // Insert a fresh cache entry
    cache.insert("proj:win".to_string(), (SessionStatus::Active, now));

    // Within TTL (2s), should use cached value
    let ttl = Duration::from_secs(2);
    let entry = cache.get("proj:win").unwrap();
    assert!(now.duration_since(entry.1) < ttl);
    assert_eq!(entry.0, SessionStatus::Active);
}

#[test]
fn test_cache_expires_after_ttl() {
    // Can't easily test real time passage, but we can test the logic
    let old_time = Instant::now() - Duration::from_secs(3);
    let mut cache: HashMap<String, (SessionStatus, Instant)> = HashMap::new();
    cache.insert(
        "proj:win".to_string(),
        (SessionStatus::Active, old_time),
    );

    let now = Instant::now();
    let ttl = Duration::from_secs(2);
    let entry = cache.get("proj:win").unwrap();
    assert!(now.duration_since(entry.1) >= ttl);
}

#[test]
fn test_detect_idle_with_trailing_whitespace() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_is_pane_alive().returning(|_| true);
    mock.expect_window_exists().returning(|_| Ok(true));
    mock.expect_capture_pane()
        .returning(|_| Ok("All tasks complete.\n\n>   \n\n".to_string()));

    let status = detect_session_status("proj:win", &mock);
    // The ">" is on its own line — should detect as idle
    assert_eq!(status, SessionStatus::Idle);
}
