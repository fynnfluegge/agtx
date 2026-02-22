//! Unit tests for app.rs logic

use super::*;

#[cfg(feature = "test-mocks")]
use crate::agent::MockAgentOperations;
#[cfg(feature = "test-mocks")]
use crate::git::{MockGitOperations, MockGitProviderOperations};
#[cfg(feature = "test-mocks")]
use crate::tmux::MockTmuxOperations;

/// Test that generate_pr_description correctly combines git diff and agent-generated text
#[test]
#[cfg(feature = "test-mocks")]
fn test_generate_pr_description_with_diff_and_agent() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    // Setup: git returns a diff stat
    mock_git
        .expect_diff_stat_from_main()
        .withf(|path: &Path| path == Path::new("/tmp/worktree"))
        .times(1)
        .returning(|_| " src/main.rs | 10 +++++++---\n 1 file changed".to_string());

    // Setup: agent generates a description
    mock_agent
        .expect_generate_text()
        .withf(|path: &Path, prompt: &str| {
            path == Path::new("/tmp/worktree") && prompt.contains("Add login feature")
        })
        .times(1)
        .returning(|_, _| {
            Ok("This PR implements user authentication with session management.".to_string())
        });

    // Execute
    let (title, body) = generate_pr_description(
        "Add login feature",
        Some("/tmp/worktree"),
        None,
        &mock_git,
        &mock_agent,
    );

    // Verify
    assert_eq!(title, "Add login feature");
    assert!(body.contains("This PR implements user authentication"));
    assert!(body.contains("## Changes"));
    assert!(body.contains("src/main.rs"));
}

/// Test that generate_pr_description handles missing worktree gracefully
#[test]
#[cfg(feature = "test-mocks")]
fn test_generate_pr_description_without_worktree() {
    let mock_git = MockGitOperations::new();
    let mock_agent = MockAgentOperations::new();

    // No expectations set - functions should not be called when worktree is None

    let (title, body) = generate_pr_description(
        "Simple task",
        None, // No worktree
        None,
        &mock_git,
        &mock_agent,
    );

    assert_eq!(title, "Simple task");
    assert!(body.is_empty());
}

/// Test that generate_pr_description handles empty diff gracefully
#[test]
#[cfg(feature = "test-mocks")]
fn test_generate_pr_description_with_empty_diff() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    // Git returns empty diff (no changes from main)
    mock_git
        .expect_diff_stat_from_main()
        .returning(|_| String::new());

    // Agent still generates description
    mock_agent
        .expect_generate_text()
        .returning(|_, _| Ok("Minor documentation update.".to_string()));

    let (title, body) = generate_pr_description(
        "Update docs",
        Some("/tmp/worktree"),
        None,
        &mock_git,
        &mock_agent,
    );

    assert_eq!(title, "Update docs");
    assert!(body.contains("Minor documentation update"));
    assert!(!body.contains("## Changes")); // No changes section when diff is empty
}

/// Test that generate_pr_description handles agent failure gracefully
#[test]
#[cfg(feature = "test-mocks")]
fn test_generate_pr_description_agent_failure() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    mock_git
        .expect_diff_stat_from_main()
        .returning(|_| " file.rs | 5 +++++\n".to_string());

    // Agent fails to generate
    mock_agent
        .expect_generate_text()
        .returning(|_, _| Err(anyhow::anyhow!("Agent not available")));

    let (title, body) = generate_pr_description(
        "Fix bug",
        Some("/tmp/worktree"),
        None,
        &mock_git,
        &mock_agent,
    );

    assert_eq!(title, "Fix bug");
    // Body should still have the diff, just no agent-generated text
    assert!(body.contains("## Changes"));
    assert!(body.contains("file.rs"));
}

// =============================================================================
// Tests for ensure_project_tmux_session
// =============================================================================

/// Test that ensure_project_tmux_session creates session when it doesn't exist
#[test]
#[cfg(feature = "test-mocks")]
fn test_ensure_project_tmux_session_creates_when_missing() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Session doesn't exist
    mock_tmux
        .expect_has_session()
        .with(mockall::predicate::eq("my-project"))
        .times(1)
        .returning(|_| false);

    // Should create the session
    mock_tmux
        .expect_create_session()
        .with(
            mockall::predicate::eq("my-project"),
            mockall::predicate::eq("/home/user/project"),
        )
        .times(1)
        .returning(|_, _| Ok(()));

    ensure_project_tmux_session("my-project", Path::new("/home/user/project"), &mock_tmux);
}

/// Test that ensure_project_tmux_session skips creation when session exists
#[test]
#[cfg(feature = "test-mocks")]
fn test_ensure_project_tmux_session_skips_when_exists() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Session already exists
    mock_tmux
        .expect_has_session()
        .with(mockall::predicate::eq("existing-project"))
        .times(1)
        .returning(|_| true);

    // create_session should NOT be called
    // (mockall will fail if unexpected calls are made)

    ensure_project_tmux_session("existing-project", Path::new("/tmp/project"), &mock_tmux);
}

// =============================================================================
// Tests for create_pr_with_content
// =============================================================================

/// Test successful PR creation with changes
#[test]
#[cfg(feature = "test-mocks")]
fn test_create_pr_with_content_success() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_git_provider = MockGitProviderOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    let task = Task {
        id: "test-123".to_string(),
        title: "Test task".to_string(),
        description: None,
        status: TaskStatus::Running,
        agent: "claude".to_string(),
        project_id: "proj-1".to_string(),
        session_name: Some("test-session".to_string()),
        worktree_path: Some("/tmp/worktree".to_string()),
        branch_name: Some("feature/test".to_string()),
        pr_number: None,
        pr_url: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    // Expect: add all files
    mock_git
        .expect_add_all()
        .withf(|path: &Path| path == Path::new("/tmp/worktree"))
        .times(1)
        .returning(|_| Ok(()));

    // Expect: check for changes
    mock_git
        .expect_has_changes()
        .withf(|path: &Path| path == Path::new("/tmp/worktree"))
        .times(1)
        .returning(|_| true);

    // Expect: commit with co-author
    mock_git
        .expect_commit()
        .withf(|path: &Path, msg: &str| {
            path == Path::new("/tmp/worktree") && msg.contains("Test PR") && msg.contains("Co-Authored-By")
        })
        .times(1)
        .returning(|_, _| Ok(()));

    // Expect: push with upstream
    mock_git
        .expect_push()
        .withf(|path: &Path, branch: &str, set_upstream: &bool| {
            path == Path::new("/tmp/worktree") && branch == "feature/test" && *set_upstream
        })
        .times(1)
        .returning(|_, _, _| Ok(()));

    // Agent co-author string
    mock_agent
        .expect_co_author_string()
        .return_const("Claude <claude@anthropic.com>".to_string());

    // Expect: create PR
    mock_git_provider
        .expect_create_pr()
        .withf(|path: &Path, title: &str, body: &str, branch: &str| {
            path == Path::new("/project") && title == "Test PR" && body == "Test body" && branch == "feature/test"
        })
        .times(1)
        .returning(|_, _, _, _| Ok((42, "https://github.com/org/repo/pull/42".to_string())));

    let result = create_pr_with_content(
        &task,
        Path::new("/project"),
        "Test PR",
        "Test body",
        &mock_git,
        &mock_git_provider,
        &mock_agent,
    );

    assert!(result.is_ok());
    let (pr_number, pr_url) = result.unwrap();
    assert_eq!(pr_number, 42);
    assert_eq!(pr_url, "https://github.com/org/repo/pull/42");
}

/// Test PR creation with no changes to commit
#[test]
#[cfg(feature = "test-mocks")]
fn test_create_pr_with_content_no_changes() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_git_provider = MockGitProviderOperations::new();
    let mock_agent = MockAgentOperations::new();

    let task = Task {
        id: "test-123".to_string(),
        title: "Test task".to_string(),
        description: None,
        status: TaskStatus::Running,
        agent: "claude".to_string(),
        project_id: "proj-1".to_string(),
        session_name: Some("test-session".to_string()),
        worktree_path: Some("/tmp/worktree".to_string()),
        branch_name: Some("feature/test".to_string()),
        pr_number: None,
        pr_url: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mock_git
        .expect_add_all()
        .returning(|_| Ok(()));

    // No changes to commit
    mock_git
        .expect_has_changes()
        .returning(|_| false);

    // commit should NOT be called (no expectation set)

    mock_git
        .expect_push()
        .returning(|_, _, _| Ok(()));

    mock_git_provider
        .expect_create_pr()
        .returning(|_, _, _, _| Ok((1, "https://github.com/pr/1".to_string())));

    let result = create_pr_with_content(
        &task,
        Path::new("/project"),
        "PR Title",
        "PR Body",
        &mock_git,
        &mock_git_provider,
        &mock_agent,
    );

    assert!(result.is_ok());
}

/// Test PR creation failure on push
#[test]
#[cfg(feature = "test-mocks")]
fn test_create_pr_with_content_push_failure() {
    let mut mock_git = MockGitOperations::new();
    let mock_git_provider = MockGitProviderOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    let task = Task {
        id: "test-123".to_string(),
        title: "Test task".to_string(),
        description: None,
        status: TaskStatus::Running,
        agent: "claude".to_string(),
        project_id: "proj-1".to_string(),
        session_name: None,
        worktree_path: Some("/tmp/worktree".to_string()),
        branch_name: Some("feature/test".to_string()),
        pr_number: None,
        pr_url: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mock_git.expect_add_all().returning(|_| Ok(()));
    mock_git.expect_has_changes().returning(|_| true);
    mock_git.expect_commit().returning(|_, _| Ok(()));
    mock_agent
        .expect_co_author_string()
        .return_const("Claude <claude@anthropic.com>".to_string());

    // Push fails
    mock_git
        .expect_push()
        .returning(|_, _, _| Err(anyhow::anyhow!("Permission denied")));

    let result = create_pr_with_content(
        &task,
        Path::new("/project"),
        "PR",
        "Body",
        &mock_git,
        &mock_git_provider,
        &mock_agent,
    );

    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Permission denied"));
}

// =============================================================================
// Tests for push_changes_to_existing_pr
// =============================================================================

/// Test pushing changes to existing PR
#[test]
#[cfg(feature = "test-mocks")]
fn test_push_changes_to_existing_pr_success() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    let task = Task {
        id: "test-456".to_string(),
        title: "Existing PR task".to_string(),
        description: None,
        status: TaskStatus::Review,
        agent: "claude".to_string(),
        project_id: "proj-1".to_string(),
        session_name: Some("test-session".to_string()),
        worktree_path: Some("/tmp/worktree".to_string()),
        branch_name: Some("feature/existing".to_string()),
        pr_number: Some(99),
        pr_url: Some("https://github.com/org/repo/pull/99".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mock_git.expect_add_all().returning(|_| Ok(()));
    mock_git.expect_has_changes().returning(|_| true);

    // Commit message should include "Address review comments"
    mock_git
        .expect_commit()
        .withf(|_: &Path, msg: &str| msg.contains("Address review comments"))
        .returning(|_, _| Ok(()));

    // Push without setting upstream (false)
    mock_git
        .expect_push()
        .withf(|_: &Path, branch: &str, set_upstream: &bool| {
            branch == "feature/existing" && !*set_upstream
        })
        .returning(|_, _, _| Ok(()));

    mock_agent
        .expect_co_author_string()
        .return_const("Claude <claude@anthropic.com>".to_string());

    let result = push_changes_to_existing_pr(&task, &mock_git, &mock_agent);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "https://github.com/org/repo/pull/99");
}

/// Test pushing when no changes exist
#[test]
#[cfg(feature = "test-mocks")]
fn test_push_changes_to_existing_pr_no_changes() {
    let mut mock_git = MockGitOperations::new();
    let mock_agent = MockAgentOperations::new();

    let task = Task {
        id: "test-789".to_string(),
        title: "Task with no changes".to_string(),
        description: None,
        status: TaskStatus::Review,
        agent: "claude".to_string(),
        project_id: "proj-1".to_string(),
        session_name: None,
        worktree_path: Some("/tmp/worktree".to_string()),
        branch_name: Some("feature/no-changes".to_string()),
        pr_number: Some(50),
        pr_url: Some("https://github.com/org/repo/pull/50".to_string()),
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mock_git.expect_add_all().returning(|_| Ok(()));
    mock_git.expect_has_changes().returning(|_| false);
    // No commit expected
    mock_git.expect_push().returning(|_, _, _| Ok(()));

    let result = push_changes_to_existing_pr(&task, &mock_git, &mock_agent);

    assert!(result.is_ok());
}

/// Test push with no existing PR URL
#[test]
#[cfg(feature = "test-mocks")]
fn test_push_changes_to_existing_pr_no_url() {
    let mut mock_git = MockGitOperations::new();
    let mock_agent = MockAgentOperations::new();

    let task = Task {
        id: "test-abc".to_string(),
        title: "Task without PR URL".to_string(),
        description: None,
        status: TaskStatus::Review,
        agent: "claude".to_string(),
        project_id: "proj-1".to_string(),
        session_name: None,
        worktree_path: Some("/tmp/worktree".to_string()),
        branch_name: Some("feature/branch".to_string()),
        pr_number: None,
        pr_url: None, // No PR URL
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mock_git.expect_add_all().returning(|_| Ok(()));
    mock_git.expect_has_changes().returning(|_| false);
    mock_git.expect_push().returning(|_, _, _| Ok(()));

    let result = push_changes_to_existing_pr(&task, &mock_git, &mock_agent);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), "Changes pushed to existing PR");
}

// =============================================================================
// Tests for fuzzy_find_files
// =============================================================================

/// Test fuzzy file search with matching pattern
#[test]
#[cfg(feature = "test-mocks")]
fn test_fuzzy_find_files_basic() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_list_files()
        .returning(|_| vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "src/tui/app.rs".to_string(),
            "src/tui/board.rs".to_string(),
            "Cargo.toml".to_string(),
        ]);

    let results = fuzzy_find_files(Path::new("/project"), "app", 10, &mock_git);

    assert!(!results.is_empty());
    assert!(results.contains(&"src/tui/app.rs".to_string()));
}

/// Test fuzzy file search with empty pattern returns first N files
#[test]
#[cfg(feature = "test-mocks")]
fn test_fuzzy_find_files_empty_pattern() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_list_files()
        .returning(|_| vec![
            "a.rs".to_string(),
            "b.rs".to_string(),
            "c.rs".to_string(),
            "d.rs".to_string(),
            "e.rs".to_string(),
        ]);

    let results = fuzzy_find_files(Path::new("/project"), "", 3, &mock_git);

    assert_eq!(results.len(), 3);
    assert_eq!(results[0], "a.rs");
    assert_eq!(results[1], "b.rs");
    assert_eq!(results[2], "c.rs");
}

/// Test fuzzy file search with no matches
#[test]
#[cfg(feature = "test-mocks")]
fn test_fuzzy_find_files_no_matches() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_list_files()
        .returning(|_| vec!["main.rs".to_string(), "lib.rs".to_string()]);

    let results = fuzzy_find_files(Path::new("/project"), "xyz123", 10, &mock_git);

    assert!(results.is_empty());
}

/// Test fuzzy file search with empty file list
#[test]
#[cfg(feature = "test-mocks")]
fn test_fuzzy_find_files_empty_list() {
    let mut mock_git = MockGitOperations::new();

    mock_git.expect_list_files().returning(|_| vec![]);

    let results = fuzzy_find_files(Path::new("/project"), "app", 10, &mock_git);

    assert!(results.is_empty());
}

/// Test fuzzy file search respects max_results
#[test]
#[cfg(feature = "test-mocks")]
fn test_fuzzy_find_files_max_results() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_list_files()
        .returning(|_| vec![
            "src/app1.rs".to_string(),
            "src/app2.rs".to_string(),
            "src/app3.rs".to_string(),
            "src/app4.rs".to_string(),
            "src/app5.rs".to_string(),
        ]);

    let results = fuzzy_find_files(Path::new("/project"), "app", 2, &mock_git);

    assert_eq!(results.len(), 2);
}

// =============================================================================
// Tests for fuzzy_score
// =============================================================================

/// Test fuzzy score with exact match
#[test]
fn test_fuzzy_score_exact_match() {
    let score = fuzzy_score("main.rs", "main.rs");
    assert!(score > 0);
}

/// Test fuzzy score with partial match
#[test]
fn test_fuzzy_score_partial_match() {
    let score = fuzzy_score("src/main.rs", "main");
    assert!(score > 0);
}

/// Test fuzzy score with no match
#[test]
fn test_fuzzy_score_no_match() {
    let score = fuzzy_score("main.rs", "xyz");
    assert_eq!(score, 0);
}

/// Test fuzzy score with empty needle
#[test]
fn test_fuzzy_score_empty_needle() {
    let score = fuzzy_score("main.rs", "");
    assert_eq!(score, 1);
}

/// Test fuzzy score bonus for word start
#[test]
fn test_fuzzy_score_word_boundary_bonus() {
    // "app" at start of segment should score higher than in middle
    let score_start = fuzzy_score("src/app.rs", "app");
    let score_middle = fuzzy_score("src/myapp.rs", "app");
    assert!(score_start > score_middle);
}

/// Test fuzzy score bonus for consecutive matches
#[test]
fn test_fuzzy_score_consecutive_bonus() {
    // Consecutive "main" should score higher than scattered chars within a word
    let score_consecutive = fuzzy_score("main.rs", "main");
    let score_scattered = fuzzy_score("myaweirdin.rs", "main");
    assert!(score_consecutive > score_scattered);
}

// =============================================================================
// Tests for send_key_to_tmux
// =============================================================================

/// Test sending character key to tmux
#[test]
#[cfg(feature = "test-mocks")]
fn test_send_key_to_tmux_char() {
    let mut mock_tmux = MockTmuxOperations::new();

    mock_tmux
        .expect_send_keys_literal()
        .with(
            mockall::predicate::eq("test-window"),
            mockall::predicate::eq("a"),
        )
        .times(1)
        .returning(|_, _| Ok(()));

    send_key_to_tmux("test-window", KeyCode::Char('a'), &mock_tmux);
}

/// Test sending Enter key to tmux
#[test]
#[cfg(feature = "test-mocks")]
fn test_send_key_to_tmux_enter() {
    let mut mock_tmux = MockTmuxOperations::new();

    mock_tmux
        .expect_send_keys_literal()
        .with(
            mockall::predicate::eq("test-window"),
            mockall::predicate::eq("Enter"),
        )
        .times(1)
        .returning(|_, _| Ok(()));

    send_key_to_tmux("test-window", KeyCode::Enter, &mock_tmux);
}

/// Test sending special keys to tmux
#[test]
#[cfg(feature = "test-mocks")]
fn test_send_key_to_tmux_special_keys() {
    let mut mock_tmux = MockTmuxOperations::new();

    // Test Escape
    mock_tmux
        .expect_send_keys_literal()
        .with(mockall::predicate::eq("win"), mockall::predicate::eq("Escape"))
        .returning(|_, _| Ok(()));

    send_key_to_tmux("win", KeyCode::Esc, &mock_tmux);

    // Test Backspace
    let mut mock_tmux2 = MockTmuxOperations::new();
    mock_tmux2
        .expect_send_keys_literal()
        .with(mockall::predicate::eq("win"), mockall::predicate::eq("BSpace"))
        .returning(|_, _| Ok(()));

    send_key_to_tmux("win", KeyCode::Backspace, &mock_tmux2);
}

/// Test sending function key to tmux
#[test]
#[cfg(feature = "test-mocks")]
fn test_send_key_to_tmux_function_key() {
    let mut mock_tmux = MockTmuxOperations::new();

    mock_tmux
        .expect_send_keys_literal()
        .with(mockall::predicate::eq("win"), mockall::predicate::eq("F5"))
        .returning(|_, _| Ok(()));

    send_key_to_tmux("win", KeyCode::F(5), &mock_tmux);
}

// =============================================================================
// Tests for capture_tmux_pane_with_history
// =============================================================================

/// Test capturing tmux pane content
#[test]
#[cfg(feature = "test-mocks")]
fn test_capture_tmux_pane_with_history() {
    let mut mock_tmux = MockTmuxOperations::new();

    mock_tmux
        .expect_capture_pane_with_history()
        .with(mockall::predicate::eq("test-window"), mockall::predicate::eq(500))
        .returning(|_, _| b"Line 1\nLine 2\nLine 3\n".to_vec());

    mock_tmux
        .expect_get_cursor_info()
        .with(mockall::predicate::eq("test-window"))
        .returning(|_| Some((2, 3))); // cursor at line 2, pane has 3 lines

    let content = capture_tmux_pane_with_history("test-window", 500, &mock_tmux);

    // Content should be trimmed to cursor position
    assert!(!content.is_empty());
}

// =============================================================================
// Tests for centered_rect helpers (pure functions, no mocks needed)
// =============================================================================

/// Test centered_rect creates correct dimensions
#[test]
fn test_centered_rect() {
    let area = Rect::new(0, 0, 100, 50);
    let popup = centered_rect(50, 50, area);

    // Should be centered horizontally and vertically
    assert!(popup.x > 0);
    assert!(popup.y > 0);
    assert!(popup.width < 100);
    assert!(popup.height < 50);
}

/// Test centered_rect_fixed_width creates correct dimensions
#[test]
fn test_centered_rect_fixed_width() {
    let area = Rect::new(0, 0, 100, 50);
    let popup = centered_rect_fixed_width(40, 50, area);

    // Width should be fixed at 40
    assert_eq!(popup.width, 40);
    // Should be centered
    assert_eq!(popup.x, 30); // (100 - 40) / 2
}

/// Test centered_rect_fixed_width caps width to terminal size
#[test]
fn test_centered_rect_fixed_width_capped() {
    let area = Rect::new(0, 0, 30, 50); // Small terminal
    let popup = centered_rect_fixed_width(100, 50, area); // Request large width

    // Width should be capped
    assert!(popup.width <= 30);
}

// =============================================================================
// Tests for hex_to_color
// =============================================================================

/// Test hex_to_color with valid hex
#[test]
fn test_hex_to_color_valid() {
    let color = hex_to_color("#FF0000");
    assert_eq!(color, Color::Rgb(255, 0, 0));
}

/// Test hex_to_color with invalid hex falls back to white
#[test]
fn test_hex_to_color_invalid() {
    let color = hex_to_color("invalid");
    assert_eq!(color, Color::White);
}

// =============================================================================
// Tests for generate_task_slug
// =============================================================================

/// Test generate_task_slug with normal title
#[test]
fn test_generate_task_slug_normal() {
    let slug = generate_task_slug("12345678-abcd-efgh", "Add login feature");
    assert!(slug.starts_with("12345678-"));
    assert!(slug.contains("Add-login-feature"));
}

/// Test generate_task_slug with special characters
#[test]
fn test_generate_task_slug_special_chars() {
    let slug = generate_task_slug("abc12345", "Fix bug #123 (urgent!)");
    assert!(slug.starts_with("abc12345-"));
    // Special chars should be replaced with dashes
    assert!(!slug.contains("#"));
    assert!(!slug.contains("("));
    assert!(!slug.contains("!"));
}

/// Test generate_task_slug truncates long titles
#[test]
fn test_generate_task_slug_long_title() {
    let long_title = "This is a very long task title that should be truncated to thirty characters";
    let slug = generate_task_slug("abcd1234", long_title);
    // 8 char id prefix + "-" + max 30 chars = max 39 chars
    assert!(slug.len() <= 39);
}

/// Test generate_task_slug with empty title
#[test]
fn test_generate_task_slug_empty_title() {
    let slug = generate_task_slug("12345678", "");
    assert_eq!(slug, "12345678-");
}

// =============================================================================
// Tests for cleanup_task_for_done
// =============================================================================

/// Test cleanup_task_for_done cleans up resources
#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_for_done_with_resources() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();

    mock_tmux
        .expect_kill_window()
        .with(mockall::predicate::eq("project:task-window"))
        .times(1)
        .returning(|_| Ok(()));

    mock_git
        .expect_remove_worktree()
        .with(
            mockall::predicate::eq(Path::new("/project")),
            mockall::predicate::eq("/tmp/worktree"),
        )
        .times(1)
        .returning(|_, _| Ok(()));

    let mut task = Task::new("Test task", "claude", "project-1");
    task.session_name = Some("project:task-window".to_string());
    task.worktree_path = Some("/tmp/worktree".to_string());
    task.status = TaskStatus::Review;

    cleanup_task_for_done(
        &mut task,
        Path::new("/project"),
        &mock_tmux,
        &mock_git,
    );

    assert!(task.session_name.is_none());
    assert!(task.worktree_path.is_none());
    assert_eq!(task.status, TaskStatus::Done);
}

/// Test cleanup_task_for_done handles missing resources gracefully
#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_for_done_no_resources() {
    use crate::db::Task;

    let mock_tmux = MockTmuxOperations::new();
    let mock_git = MockGitOperations::new();
    // No expectations - functions should not be called

    let mut task = Task::new("Test task", "claude", "project-1");
    // No session_name or worktree_path set

    cleanup_task_for_done(
        &mut task,
        Path::new("/project"),
        &mock_tmux,
        &mock_git,
    );

    assert_eq!(task.status, TaskStatus::Done);
}

// =============================================================================
// Tests for delete_task_resources
// =============================================================================

/// Test delete_task_resources cleans up all resources
#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_task_resources_full_cleanup() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();

    mock_tmux
        .expect_kill_window()
        .with(mockall::predicate::eq("project:task-window"))
        .times(1)
        .returning(|_| Ok(()));

    mock_git
        .expect_remove_worktree()
        .times(1)
        .returning(|_, _| Ok(()));

    mock_git
        .expect_delete_branch()
        .with(
            mockall::predicate::eq(Path::new("/project")),
            mockall::predicate::eq("task/abc-feature"),
        )
        .times(1)
        .returning(|_, _| Ok(()));

    let mut task = Task::new("Feature task", "claude", "project-1");
    task.session_name = Some("project:task-window".to_string());
    task.worktree_path = Some("/tmp/worktree".to_string());
    task.branch_name = Some("task/abc-feature".to_string());

    delete_task_resources(
        &task,
        Path::new("/project"),
        &mock_tmux,
        &mock_git,
    );
}

/// Test delete_task_resources handles task without resources
#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_task_resources_no_resources() {
    use crate::db::Task;

    let mock_tmux = MockTmuxOperations::new();
    let mock_git = MockGitOperations::new();
    // No expectations - nothing should be called

    let task = Task::new("Simple task", "claude", "project-1");
    // No session_name, worktree_path, or branch_name

    delete_task_resources(
        &task,
        Path::new("/project"),
        &mock_tmux,
        &mock_git,
    );
}

// =============================================================================
// Tests for collect_task_diff
// =============================================================================

/// Test collect_task_diff with all types of changes
#[test]
#[cfg(feature = "test-mocks")]
fn test_collect_task_diff_all_changes() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_diff()
        .returning(|_| "diff --git a/file.rs\n-old\n+new".to_string());

    mock_git
        .expect_diff_cached()
        .returning(|_| "diff --git a/staged.rs\n+added".to_string());

    mock_git
        .expect_list_untracked_files()
        .returning(|_| "new_file.rs\n".to_string());

    mock_git
        .expect_diff_untracked_file()
        .returning(|_, _| "+++ new_file.rs\n+content".to_string());

    let result = collect_task_diff("/tmp/worktree", &mock_git);

    assert!(result.contains("Unstaged Changes"));
    assert!(result.contains("Staged Changes"));
    assert!(result.contains("Untracked Files"));
}

/// Test collect_task_diff with no changes
#[test]
#[cfg(feature = "test-mocks")]
fn test_collect_task_diff_no_changes() {
    let mut mock_git = MockGitOperations::new();

    mock_git.expect_diff().returning(|_| String::new());
    mock_git.expect_diff_cached().returning(|_| String::new());
    mock_git.expect_list_untracked_files().returning(|_| String::new());

    let result = collect_task_diff("/tmp/worktree", &mock_git);

    assert!(result.contains("(no changes)"));
    assert!(result.contains("/tmp/worktree"));
}

/// Test collect_task_diff with only unstaged changes
#[test]
#[cfg(feature = "test-mocks")]
fn test_collect_task_diff_only_unstaged() {
    let mut mock_git = MockGitOperations::new();

    mock_git
        .expect_diff()
        .returning(|_| "diff --git a/modified.rs".to_string());

    mock_git.expect_diff_cached().returning(|_| String::new());
    mock_git.expect_list_untracked_files().returning(|_| String::new());

    let result = collect_task_diff("/tmp/worktree", &mock_git);

    assert!(result.contains("Unstaged Changes"));
    assert!(!result.contains("Staged Changes"));
    assert!(!result.contains("Untracked Files"));
}

// =============================================================================
// Tests for build_highlighted_text
// =============================================================================

/// Test build_highlighted_text with no file paths produces plain text
#[test]
fn test_build_highlighted_text_no_paths() {
    let paths = HashSet::new();
    let text = build_highlighted_text("hello world", &paths, Color::White, Color::Cyan);
    let lines: Vec<&Line> = text.lines.iter().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans.len(), 1);
    assert_eq!(lines[0].spans[0].content, "hello world");
}

/// Test build_highlighted_text highlights a single file path
#[test]
fn test_build_highlighted_text_single_path() {
    let mut paths = HashSet::new();
    paths.insert("src/main.rs".to_string());
    let text = build_highlighted_text(
        "Please edit src/main.rs for me",
        &paths,
        Color::White,
        Color::Cyan,
    );
    let lines: Vec<&Line> = text.lines.iter().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans.len(), 3);
    assert_eq!(lines[0].spans[0].content, "Please edit ");
    assert_eq!(lines[0].spans[1].content, "src/main.rs");
    assert_eq!(lines[0].spans[2].content, " for me");
    // The highlighted span should be bold
    assert!(lines[0].spans[1].style.add_modifier.contains(Modifier::BOLD));
}

/// Test build_highlighted_text with multiple file paths on one line
#[test]
fn test_build_highlighted_text_multiple_paths() {
    let mut paths = HashSet::new();
    paths.insert("a.rs".to_string());
    paths.insert("b.rs".to_string());
    let text = build_highlighted_text("fix a.rs and b.rs", &paths, Color::White, Color::Cyan);
    let lines: Vec<&Line> = text.lines.iter().collect();
    assert_eq!(lines.len(), 1);
    // Should be: "fix " | "a.rs" | " and " | "b.rs"
    assert_eq!(lines[0].spans.len(), 4);
    assert_eq!(lines[0].spans[1].content, "a.rs");
    assert_eq!(lines[0].spans[3].content, "b.rs");
}

/// Test build_highlighted_text with multiline input
#[test]
fn test_build_highlighted_text_multiline() {
    let mut paths = HashSet::new();
    paths.insert("app.rs".to_string());
    let text = build_highlighted_text("line1\nfix app.rs\nline3", &paths, Color::White, Color::Cyan);
    let lines: Vec<&Line> = text.lines.iter().collect();
    assert_eq!(lines.len(), 3);
    // First line: no highlight
    assert_eq!(lines[0].spans.len(), 1);
    assert_eq!(lines[0].spans[0].content, "line1");
    // Second line: has highlight
    assert_eq!(lines[1].spans.len(), 2);
    assert_eq!(lines[1].spans[0].content, "fix ");
    assert_eq!(lines[1].spans[1].content, "app.rs");
    // Third line: no highlight
    assert_eq!(lines[2].spans.len(), 1);
    assert_eq!(lines[2].spans[0].content, "line3");
}

/// Test build_highlighted_text when path is at the start of line
#[test]
fn test_build_highlighted_text_path_at_start() {
    let mut paths = HashSet::new();
    paths.insert("src/lib.rs".to_string());
    let text = build_highlighted_text("src/lib.rs is important", &paths, Color::White, Color::Cyan);
    let lines: Vec<&Line> = text.lines.iter().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans.len(), 2);
    assert_eq!(lines[0].spans[0].content, "src/lib.rs");
    assert_eq!(lines[0].spans[1].content, " is important");
}

/// Test build_highlighted_text when path is the entire line
#[test]
fn test_build_highlighted_text_path_is_entire_line() {
    let mut paths = HashSet::new();
    paths.insert("Cargo.toml".to_string());
    let text = build_highlighted_text("Cargo.toml", &paths, Color::White, Color::Cyan);
    let lines: Vec<&Line> = text.lines.iter().collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans.len(), 1);
    assert_eq!(lines[0].spans[0].content, "Cargo.toml");
    assert!(lines[0].spans[0].style.add_modifier.contains(Modifier::BOLD));
}

// =============================================================================
// Tests for word_boundary_left / word_boundary_right
// =============================================================================

/// Test word_boundary_left from end of string
#[test]
fn test_word_boundary_left_from_end() {
    assert_eq!(word_boundary_left("hello world", 11), 6);
}

/// Test word_boundary_left skips to previous word
#[test]
fn test_word_boundary_left_between_words() {
    assert_eq!(word_boundary_left("hello world", 6), 0);
}

/// Test word_boundary_left from middle of word
#[test]
fn test_word_boundary_left_mid_word() {
    assert_eq!(word_boundary_left("hello world", 8), 6);
}

/// Test word_boundary_left at start stays at 0
#[test]
fn test_word_boundary_left_at_start() {
    assert_eq!(word_boundary_left("hello", 0), 0);
}

/// Test word_boundary_left with multiple spaces
#[test]
fn test_word_boundary_left_multiple_spaces() {
    assert_eq!(word_boundary_left("hello   world", 13), 8);
}

/// Test word_boundary_left with path separators
#[test]
fn test_word_boundary_left_path() {
    // From end of "src/main.rs", should jump back over "rs"
    assert_eq!(word_boundary_left("src/main.rs", 11), 9);
}

/// Test word_boundary_right from start of string
#[test]
fn test_word_boundary_right_from_start() {
    assert_eq!(word_boundary_right("hello world", 0), 6);
}

/// Test word_boundary_right from space between words
#[test]
fn test_word_boundary_right_from_space() {
    assert_eq!(word_boundary_right("hello world", 5), 6);
}

/// Test word_boundary_right from middle of word
#[test]
fn test_word_boundary_right_mid_word() {
    assert_eq!(word_boundary_right("hello world", 3), 6);
}

/// Test word_boundary_right at end stays at end
#[test]
fn test_word_boundary_right_at_end() {
    assert_eq!(word_boundary_right("hello", 5), 5);
}

/// Test word_boundary_right with multiple spaces
#[test]
fn test_word_boundary_right_multiple_spaces() {
    assert_eq!(word_boundary_right("hello   world", 0), 8);
}

/// Test word_boundary_right with path separators
#[test]
fn test_word_boundary_right_path() {
    // From start of "src/main.rs", should jump over "src" then the separator
    assert_eq!(word_boundary_right("src/main.rs", 0), 4);
}

/// Test word_boundary_left with empty string
#[test]
fn test_word_boundary_left_empty() {
    assert_eq!(word_boundary_left("", 0), 0);
}

/// Test word_boundary_right with empty string
#[test]
fn test_word_boundary_right_empty() {
    assert_eq!(word_boundary_right("", 0), 0);
}

/// Test word_boundary roundtrip: jumping right then left returns close to start
#[test]
fn test_word_boundary_roundtrip() {
    let s = "hello world foo";
    let pos = word_boundary_right(s, 0); // -> 6 (start of "world")
    let pos = word_boundary_right(s, pos); // -> 12 (start of "foo")
    let pos = word_boundary_left(s, pos); // -> 6 (start of "world")
    let pos = word_boundary_left(s, pos); // -> 0 (start of "hello")
    assert_eq!(pos, 0);
}

// =============================================================================
// Tests for build_footer_text
// =============================================================================

#[test]
fn test_footer_text_sidebar_focused() {
    let text = build_footer_text(InputMode::Normal, true, 0);
    assert!(text.contains("[j/k] navigate"));
    assert!(text.contains("[e] hide sidebar"));
    assert!(!text.contains("[o] new"));
}

#[test]
fn test_footer_text_backlog_column() {
    let text = build_footer_text(InputMode::Normal, false, 0);
    assert!(text.contains("[M] run"));
    assert!(text.contains("[m] plan"));
    assert!(!text.contains("[r] move left"));
}

#[test]
fn test_footer_text_planning_column() {
    let text = build_footer_text(InputMode::Normal, false, 1);
    assert!(text.contains("[m] run"));
    assert!(!text.contains("[M] run"));
    assert!(!text.contains("[r] move left"));
}

#[test]
fn test_footer_text_running_column() {
    let text = build_footer_text(InputMode::Normal, false, 2);
    assert!(text.contains("[r] move left"));
    assert!(text.contains("[m] move"));
}

#[test]
fn test_footer_text_review_column() {
    let text = build_footer_text(InputMode::Normal, false, 3);
    assert!(text.contains("[r] move left"));
    assert!(text.contains("[m] move"));
}

#[test]
fn test_footer_text_done_column() {
    let text = build_footer_text(InputMode::Normal, false, 4);
    assert!(!text.contains("[m] move"));
    assert!(!text.contains("[r]"));
    assert!(!text.contains("[d] diff"));
}

#[test]
fn test_footer_text_input_title() {
    let text = build_footer_text(InputMode::InputTitle, false, 0);
    assert!(text.contains("Enter task title"));
    assert!(text.contains("[Esc] cancel"));
}

#[test]
fn test_footer_text_input_description() {
    let text = build_footer_text(InputMode::InputDescription, false, 0);
    assert!(text.contains("[#] file search"));
    assert!(text.contains("[\\+Enter] newline"));
}

// =============================================================================
// Tests for setup_task_worktree
// =============================================================================

/// Test setup_task_worktree creates worktree, initializes it, and creates tmux window
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_success() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    // Expect worktree creation
    mock_git
        .expect_create_worktree()
        .returning(|_, slug| Ok(format!("/project/.agtx/worktrees/{}", slug)));

    // Expect worktree initialization
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _| vec![]);

    // Expect agent command building
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude --dangerously-skip-permissions '{}'", prompt));

    // Expect tmux session check and window creation
    mock_tmux
        .expect_has_session()
        .returning(|_| true);

    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _| Ok(()));

    let mut task = Task::new("Add login feature", "claude", "project-1");
    task.status = TaskStatus::Backlog;

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "implement this",
        None,
        None,
        &mock_tmux,
        &mock_git,
        &mock_agent,
    );

    assert!(result.is_ok());
    let target = result.unwrap();
    assert!(target.starts_with("my-project:task-"));
    assert!(task.session_name.is_some());
    assert!(task.worktree_path.is_some());
    assert!(task.branch_name.is_some());
    assert!(task.branch_name.as_ref().unwrap().starts_with("task/"));
}

/// Test setup_task_worktree sets correct task fields
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_sets_task_fields() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    mock_git
        .expect_create_worktree()
        .returning(|_, slug| Ok(format!("/project/.agtx/worktrees/{}", slug)));
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);
    mock_tmux.expect_create_window().returning(|_, _, _, _| Ok(()));

    let mut task = Task::new("Fix bug", "claude", "project-1");

    let target = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "fix the bug",
        Some("CLAUDE.md".to_string()),
        Some("./init.sh".to_string()),
        &mock_tmux,
        &mock_git,
        &mock_agent,
    ).unwrap();

    // session_name should be the returned target
    assert_eq!(task.session_name.as_ref().unwrap(), &target);
    // worktree_path should contain the slug
    assert!(task.worktree_path.as_ref().unwrap().contains(".agtx/worktrees/"));
    // branch_name should be task/{slug}
    let slug = &task.branch_name.as_ref().unwrap()["task/".len()..];
    assert!(task.worktree_path.as_ref().unwrap().ends_with(slug));
}

/// Test setup_task_worktree handles worktree creation failure gracefully
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_worktree_creation_fails() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    // Worktree creation fails
    mock_git
        .expect_create_worktree()
        .returning(|_, _| Err(anyhow::anyhow!("worktree already exists")));

    // Should still initialize and create window with fallback path
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);
    mock_tmux.expect_create_window().returning(|_, _, _, _| Ok(()));

    let mut task = Task::new("Test task", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "do something",
        None,
        None,
        &mock_tmux,
        &mock_git,
        &mock_agent,
    );

    // Should succeed despite worktree creation failure (uses fallback path)
    assert!(result.is_ok());
    assert!(task.worktree_path.is_some());
    assert!(task.worktree_path.as_ref().unwrap().contains(".agtx/worktrees/"));
}

/// Test setup_task_worktree fails when tmux window creation fails
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_tmux_window_fails() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    mock_git
        .expect_create_worktree()
        .returning(|_, slug| Ok(format!("/project/.agtx/worktrees/{}", slug)));
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);

    // Tmux window creation fails
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _| Err(anyhow::anyhow!("tmux not running")));

    let mut task = Task::new("Test task", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "do something",
        None,
        None,
        &mock_tmux,
        &mock_git,
        &mock_agent,
    );

    // Should propagate the error
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("tmux not running"));
}

/// Test setup_task_worktree creates tmux session when missing
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_creates_session_when_missing() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    mock_git
        .expect_create_worktree()
        .returning(|_, slug| Ok(format!("/project/.agtx/worktrees/{}", slug)));
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));

    // Session doesn't exist yet
    mock_tmux
        .expect_has_session()
        .returning(|_| false);
    mock_tmux
        .expect_create_session()
        .returning(|_, _| Ok(()));
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _| Ok(()));

    let mut task = Task::new("New task", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "do work",
        None,
        None,
        &mock_tmux,
        &mock_git,
        &mock_agent,
    );

    assert!(result.is_ok());
}

/// Test setup_task_worktree passes copy_files and init_script to initialize_worktree
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_passes_init_config() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();

    mock_git
        .expect_create_worktree()
        .returning(|_, slug| Ok(format!("/project/.agtx/worktrees/{}", slug)));

    // Verify copy_files and init_script are passed through
    mock_git
        .expect_initialize_worktree()
        .withf(|_, _, copy_files, init_script| {
            copy_files.as_deref() == Some("CLAUDE.md,.env")
                && init_script.as_deref() == Some("./setup.sh")
        })
        .returning(|_, _, _, _| vec!["warning: .env not found".to_string()]);

    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);
    mock_tmux.expect_create_window().returning(|_, _, _, _| Ok(()));

    let mut task = Task::new("Task with config", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "implement feature",
        Some("CLAUDE.md,.env".to_string()),
        Some("./setup.sh".to_string()),
        &mock_tmux,
        &mock_git,
        &mock_agent,
    );

    assert!(result.is_ok());
}
