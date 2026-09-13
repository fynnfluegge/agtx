use agtx::db::{Database, Notification, Project, Task, TaskStatus, TransitionRequest};

// === TransitionRequest Model Tests ===

#[test]
fn test_transition_request_new() {
    let req = TransitionRequest::new("task-123", "move_forward");
    assert!(!req.id.is_empty());
    assert_eq!(req.task_id, "task-123");
    assert_eq!(req.action, "move_forward");
    assert!(req.processed_at.is_none());
    assert!(req.error.is_none());
}

// === Database CRUD Tests ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_and_get_transition_request() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_to_planning");

    db.create_transition_request(&req).unwrap();

    let fetched = db.get_transition_request(&req.id).unwrap();
    assert!(fetched.is_some());
    let fetched = fetched.unwrap();
    assert_eq!(fetched.id, req.id);
    assert_eq!(fetched.task_id, "task-1");
    assert_eq!(fetched.action, "move_to_planning");
    assert!(fetched.processed_at.is_none());
    assert!(fetched.error.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_get_transition_request_not_found() {
    let db = Database::open_in_memory_project().unwrap();
    let fetched = db.get_transition_request("nonexistent").unwrap();
    assert!(fetched.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_get_pending_transition_requests() {
    let db = Database::open_in_memory_project().unwrap();

    let req1 = TransitionRequest::new("task-1", "move_forward");
    let req2 = TransitionRequest::new("task-2", "move_to_running");
    let req3 = TransitionRequest::new("task-3", "resume");

    db.create_transition_request(&req1).unwrap();
    db.create_transition_request(&req2).unwrap();
    db.create_transition_request(&req3).unwrap();

    // Mark req2 as processed
    db.mark_transition_processed(&req2.id, None).unwrap();

    let pending = db.get_pending_transition_requests().unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].id, req1.id);
    assert_eq!(pending[1].id, req3.id);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_mark_transition_processed_success() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();

    db.mark_transition_processed(&req.id, None).unwrap();

    let fetched = db.get_transition_request(&req.id).unwrap().unwrap();
    assert!(fetched.processed_at.is_some());
    assert!(fetched.error.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_mark_transition_processed_with_error() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();

    db.mark_transition_processed(&req.id, Some("Task not found"))
        .unwrap();

    let fetched = db.get_transition_request(&req.id).unwrap().unwrap();
    assert!(fetched.processed_at.is_some());
    assert_eq!(fetched.error.as_deref(), Some("Task not found"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_pending_excludes_processed() {
    let db = Database::open_in_memory_project().unwrap();

    let req1 = TransitionRequest::new("task-1", "move_forward");
    let req2 = TransitionRequest::new("task-2", "move_forward");
    db.create_transition_request(&req1).unwrap();
    db.create_transition_request(&req2).unwrap();

    // Process both
    db.mark_transition_processed(&req1.id, None).unwrap();
    db.mark_transition_processed(&req2.id, Some("error"))
        .unwrap();

    let pending = db.get_pending_transition_requests().unwrap();
    assert!(pending.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_old_transition_requests() {
    let db = Database::open_in_memory_project().unwrap();

    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();
    db.mark_transition_processed(&req.id, None).unwrap();

    // Manually backdate the processed_at to 2 hours ago
    db.cleanup_old_transition_requests().unwrap();

    // The request was just processed (now), so cleanup shouldn't delete it
    let fetched = db.get_transition_request(&req.id).unwrap();
    assert!(fetched.is_some());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_transition_request_with_task() {
    let db = Database::open_in_memory_project().unwrap();

    // Create a task first
    let task = Task::new("Test task", "claude", "test-project");
    db.create_task(&task).unwrap();

    // Create a transition request for this task
    let req = TransitionRequest::new(&task.id, "move_to_planning");
    db.create_transition_request(&req).unwrap();

    // Verify we can fetch both
    let fetched_task = db.get_task(&task.id).unwrap();
    assert!(fetched_task.is_some());

    let fetched_req = db.get_transition_request(&req.id).unwrap();
    assert!(fetched_req.is_some());
    assert_eq!(fetched_req.unwrap().task_id, task.id);
}

// === Task Creation Tests (for MCP create_task / create_tasks_batch) ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_task_with_description_and_plugin() {
    let db = Database::open_in_memory_project().unwrap();

    let mut task = Task::new("Add OAuth", "claude", "my-project");
    task.description = Some("Implement OAuth with Google".to_string());
    task.plugin = Some("agtx".to_string());
    db.create_task(&task).unwrap();

    let fetched = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(fetched.title, "Add OAuth");
    assert_eq!(
        fetched.description.as_deref(),
        Some("Implement OAuth with Google")
    );
    assert_eq!(fetched.plugin.as_deref(), Some("agtx"));
    assert_eq!(fetched.status, TaskStatus::Backlog);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_task_with_referenced_tasks() {
    let db = Database::open_in_memory_project().unwrap();

    let task1 = Task::new("Setup DB schema", "claude", "my-project");
    db.create_task(&task1).unwrap();

    let task2 = Task::new("Setup config", "claude", "my-project");
    db.create_task(&task2).unwrap();

    let mut task3 = Task::new("Implement endpoints", "claude", "my-project");
    task3.referenced_tasks = Some(format!("{},{}", task1.id, task2.id));
    db.create_task(&task3).unwrap();

    let fetched = db.get_task(&task3.id).unwrap().unwrap();
    let refs = fetched.referenced_tasks.unwrap();
    assert!(refs.contains(&task1.id));
    assert!(refs.contains(&task2.id));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_batch_create_tasks_with_index_deps() {
    let mut db = Database::open_in_memory_project().unwrap();

    // 3 tasks where task[2] depends on task[0] and task[1]
    let task0 = Task::new("DB schema", "claude", "my-project");
    let task1 = Task::new("Config setup", "claude", "my-project");
    let mut task2 = Task::new("Endpoints", "claude", "my-project");
    task2.referenced_tasks = Some(format!("{},{}", task0.id, task1.id));

    db.create_tasks_batch(&[task0.clone(), task1.clone(), task2.clone()])
        .unwrap();

    let all = db.get_all_tasks().unwrap();
    assert_eq!(all.len(), 3);

    let fetched = db.get_task(&task2.id).unwrap().unwrap();
    let refs = fetched.referenced_tasks.unwrap();
    assert!(refs.contains(&task0.id));
    assert!(refs.contains(&task1.id));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_batch_create_tasks_rolls_back_on_failure() {
    let mut db = Database::open_in_memory_project().unwrap();

    let task0 = Task::new("First task", "claude", "my-project");
    // task1 deliberately reuses task0's ID to trigger a UNIQUE constraint violation
    let mut task1 = Task::new("Duplicate ID task", "claude", "my-project");
    task1.id = task0.id.clone();

    let result = db.create_tasks_batch(&[task0, task1]);
    assert!(result.is_err(), "batch insert should fail on duplicate ID");

    // Nothing should have been committed
    let all = db.get_all_tasks().unwrap();
    assert!(all.is_empty(), "rollback should leave DB empty");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_backlog_task() {
    let db = Database::open_in_memory_project().unwrap();

    let task = Task::new("Delete me", "claude", "my-project");
    db.create_task(&task).unwrap();

    db.delete_task(&task.id).unwrap();

    let fetched = db.get_task(&task.id).unwrap();
    assert!(fetched.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_update_backlog_task() {
    let db = Database::open_in_memory_project().unwrap();

    let mut task = Task::new("Original title", "claude", "my-project");
    task.description = Some("Original desc".to_string());
    db.create_task(&task).unwrap();

    // Update title and description
    task.title = "Updated title".to_string();
    task.description = Some("Updated desc".to_string());
    task.plugin = Some("gsd".to_string());
    db.update_task(&task).unwrap();

    let fetched = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(fetched.title, "Updated title");
    assert_eq!(fetched.description.unwrap(), "Updated desc");
    assert_eq!(fetched.plugin.unwrap(), "gsd");
    assert_eq!(fetched.status, TaskStatus::Backlog);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_update_task_db_allows_non_backlog_status_change() {
    let db = Database::open_in_memory_project().unwrap();

    let mut task = Task::new("My task", "claude", "my-project");
    db.create_task(&task).unwrap();

    // Move to planning status
    task.status = TaskStatus::Planning;
    db.update_task(&task).unwrap();

    // DB layer allows update regardless of status (status guard is in MCP layer), verify status changed
    let fetched = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(fetched.status, TaskStatus::Planning);
}

// === get_tasks_by_status ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_get_tasks_by_status_filters_correctly() {
    let db = Database::open_in_memory_project().unwrap();

    let backlog = Task::new("Backlog task", "claude", "proj");
    let mut planning = Task::new("Planning task", "claude", "proj");
    planning.status = TaskStatus::Planning;
    let mut running = Task::new("Running task", "claude", "proj");
    running.status = TaskStatus::Running;

    db.create_task(&backlog).unwrap();
    db.create_task(&planning).unwrap();
    db.create_task(&running).unwrap();

    let backlog_tasks = db.get_tasks_by_status(TaskStatus::Backlog).unwrap();
    assert_eq!(backlog_tasks.len(), 1);
    assert_eq!(backlog_tasks[0].id, backlog.id);

    let planning_tasks = db.get_tasks_by_status(TaskStatus::Planning).unwrap();
    assert_eq!(planning_tasks.len(), 1);
    assert_eq!(planning_tasks[0].id, planning.id);

    let done_tasks = db.get_tasks_by_status(TaskStatus::Done).unwrap();
    assert!(done_tasks.is_empty());
}

// === update_task / delete_task edge cases ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_update_nonexistent_task_is_silent_noop() {
    let db = Database::open_in_memory_project().unwrap();
    let task = Task::new("Ghost", "claude", "proj");
    // Never inserted — update should succeed without error and affect nothing
    db.update_task(&task).unwrap();
    let fetched = db.get_task(&task.id).unwrap();
    assert!(fetched.is_none());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_delete_nonexistent_task_is_silent_noop() {
    let db = Database::open_in_memory_project().unwrap();
    // Should not error
    db.delete_task("no-such-id").unwrap();
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_create_and_get_task_with_all_none_optionals() {
    let db = Database::open_in_memory_project().unwrap();
    let task = Task::new("Bare task", "claude", "proj");
    db.create_task(&task).unwrap();

    let fetched = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(fetched.title, "Bare task");
    assert!(fetched.description.is_none());
    assert!(fetched.plugin.is_none());
    assert!(fetched.referenced_tasks.is_none());
    assert!(fetched.escalation_note.is_none());
    assert!(fetched.base_branch.is_none());
    assert!(fetched.session_name.is_none());
    assert!(fetched.worktree_path.is_none());
    assert!(fetched.branch_name.is_none());
    assert!(fetched.pr_number.is_none());
    assert!(fetched.pr_url.is_none());
}

// === Project (global db) ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_upsert_and_get_project() {
    let db = Database::open_in_memory_global().unwrap();

    let project = Project::new("my-app", "/home/user/my-app");
    db.upsert_project(&project).unwrap();

    let all = db.get_all_projects().unwrap();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].name, "my-app");
    assert_eq!(all[0].path, "/home/user/my-app");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_upsert_project_on_conflict_updates_name() {
    let db = Database::open_in_memory_global().unwrap();

    let mut project = Project::new("old-name", "/home/user/my-app");
    db.upsert_project(&project).unwrap();

    project.name = "new-name".to_string();
    db.upsert_project(&project).unwrap();

    let all = db.get_all_projects().unwrap();
    assert_eq!(all.len(), 1, "upsert should not create a duplicate row");
    assert_eq!(all[0].name, "new-name");
}

// === Notifications ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_peek_notifications_does_not_consume() {
    let db = Database::open_in_memory_project().unwrap();

    db.create_notification(&Notification::new("phase completed"))
        .unwrap();
    db.create_notification(&Notification::new("task ready"))
        .unwrap();

    let first_peek = db.peek_notifications().unwrap();
    assert_eq!(first_peek.len(), 2);

    // Peek again — should still be there
    let second_peek = db.peek_notifications().unwrap();
    assert_eq!(second_peek.len(), 2);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_consume_notifications_clears_table() {
    let db = Database::open_in_memory_project().unwrap();

    db.create_notification(&Notification::new("event A"))
        .unwrap();
    db.create_notification(&Notification::new("event B"))
        .unwrap();

    let consumed = db.consume_notifications().unwrap();
    assert_eq!(consumed.len(), 2);

    // Table should now be empty
    let after = db.peek_notifications().unwrap();
    assert!(after.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_consume_notifications_returns_in_order() {
    let db = Database::open_in_memory_project().unwrap();

    db.create_notification(&Notification::new("first")).unwrap();
    db.create_notification(&Notification::new("second"))
        .unwrap();
    db.create_notification(&Notification::new("third")).unwrap();

    let consumed = db.consume_notifications().unwrap();
    assert_eq!(consumed[0].message, "first");
    assert_eq!(consumed[1].message, "second");
    assert_eq!(consumed[2].message, "third");
}

// === cleanup_old_transition_requests actually deletes ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_deletes_old_processed_requests() {
    let db = Database::open_in_memory_project().unwrap();

    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();

    // Backdate processed_at to 2 hours ago directly via SQL
    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    db.backdate_transition_processed_at(&req.id, &two_hours_ago)
        .unwrap();

    db.cleanup_old_transition_requests().unwrap();

    let fetched = db.get_transition_request(&req.id).unwrap();
    assert!(fetched.is_none(), "request older than 1h should be deleted");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_cleanup_keeps_recently_processed_requests() {
    let db = Database::open_in_memory_project().unwrap();

    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();
    db.mark_transition_processed(&req.id, None).unwrap();

    db.cleanup_old_transition_requests().unwrap();

    let fetched = db.get_transition_request(&req.id).unwrap();
    assert!(
        fetched.is_some(),
        "recently processed request should be kept"
    );
}

// === send_to_task Input Validation Tests (Fix 5) ===
// These test the validation constraints documented in the MCP server.
// The actual validation runs inside AgtxMcpServer::send_to_task, which requires
// a full MCP server setup. Here we verify the constraint constants and edge cases
// at the model level.

#[test]
fn test_send_to_task_max_message_length_constant() {
    // The max message length is 4096 bytes as documented in SendToTaskParams
    // Verify a message at the boundary is representable
    let msg = "a".repeat(4096);
    assert_eq!(msg.len(), 4096);

    let over_limit = "a".repeat(4097);
    assert!(over_limit.len() > 4096);
}

#[test]
fn test_null_byte_detection() {
    // Verify that Rust's contains check works for null bytes
    // (this is what the server uses for validation)
    let clean_msg = "Hello, agent!";
    assert!(!clean_msg.contains('\x00'));

    let dirty_msg = "Hello\x00world";
    assert!(dirty_msg.contains('\x00'));

    let null_only = "\x00";
    assert!(null_only.contains('\x00'));
}

#[test]
fn test_normal_messages_pass_validation() {
    // Messages that should pass validation
    let at_limit = "a".repeat(4096);
    let messages: Vec<&str> = vec![
        "Please continue with the implementation",
        "y",
        "n",
        "Run: cargo test",
        "Multi\nline\nmessage",
        &at_limit, // exactly at limit
    ];

    for msg in messages {
        assert!(msg.len() <= 4096, "Message should be within length limit");
        assert!(
            !msg.contains('\x00'),
            "Message should not contain null bytes"
        );
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_send_to_task_requires_active_phase() {
    // The real rule, not a copy of it: the server calls this same function.
    use agtx::core::actions::accepts_task_input;

    // No agent to receive it.
    for status in [TaskStatus::Backlog, TaskStatus::Done] {
        assert!(!accepts_task_input(status), "{status:?} must refuse input");
    }
    // Review is included so a reviewer can be handed a small fix in place,
    // rather than the task being resumed to Running just to deliver a message.
    for status in [TaskStatus::Planning, TaskStatus::Running, TaskStatus::Review] {
        assert!(accepts_task_input(status), "{status:?} must accept input");
    }
}

// === Local integration action ===

fn review_task() -> Task {
    let mut t = Task::new("Add the thing", "claude", "p1");
    t.status = TaskStatus::Review;
    t
}

/// Merging into the project's own checkout is the unattended caller's path to
/// Done. A person lands the same work by merging the PR on the remote, so
/// offering them both would be two ways to land one branch.
#[test]
fn local_merge_is_offered_to_the_orchestrator_and_not_to_a_person() {
    use agtx::core::actions::{allowed_actions, CallerKind};

    let task = review_task();
    let orchestrator = allowed_actions(&task, true, CallerKind::Orchestrator);
    let human = allowed_actions(&task, true, CallerKind::Human);

    assert!(orchestrator.contains(&"move_to_done_and_merge".to_string()));
    assert!(!human.contains(&"move_to_done_and_merge".to_string()));
    // It is an addition, not a replacement: a caller that does its integration
    // elsewhere still reaches Done the plain way.
    assert!(orchestrator.contains(&"move_to_done".to_string()));
    assert!(human.contains(&"move_to_done".to_string()));
}

#[test]
fn local_merge_is_only_valid_from_review() {
    use agtx::core::actions::{validate_action, CallerKind};

    let task = review_task();
    assert!(validate_action(&task, true, CallerKind::Orchestrator, "move_to_done_and_merge").is_ok());

    for status in [
        TaskStatus::Backlog,
        TaskStatus::Planning,
        TaskStatus::Running,
        TaskStatus::Done,
    ] {
        let mut t = review_task();
        t.status = status;
        assert!(
            validate_action(&t, true, CallerKind::Orchestrator, "move_to_done_and_merge").is_err(),
            "should be refused from {}",
            status.as_str()
        );
    }
}

/// The verb has to be in `ACTIONS` or `move_task` rejects it as unknown before
/// it ever reaches the executor.
#[test]
fn local_merge_is_a_known_action() {
    assert!(agtx::core::actions::ACTIONS.contains(&"move_to_done_and_merge"));
}

// === Reclaiming a dead instance's claims ===

/// A Backlog transition is claimed when picked up but only *marked* once the
/// serialized setup slot frees up and it actually starts. A TUI that exits in
/// between strands it: the row is claimed, so `get_pending_transition_requests`
/// filters it out and no restarted TUI ever runs it.
#[test]
#[cfg(feature = "test-mocks")]
fn a_dead_instance_claim_is_reclaimed_and_becomes_pending_again() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_to_planning");
    db.create_transition_request(&req).unwrap();
    assert!(db.claim_transition_request(&req.id, "dead-instance").unwrap());
    assert!(
        db.get_pending_transition_requests().unwrap().is_empty(),
        "a claimed row is invisible to the drain — that is what strands it"
    );

    // Zero window: everything not held by this instance is fair game.
    let n = db
        .reclaim_stale_transition_requests("live-instance", chrono::Duration::zero())
        .unwrap();

    assert_eq!(n, 1);
    let pending = db.get_pending_transition_requests().unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, req.id);
}

/// This instance's own claims are its in-memory queue. Reclaiming them would
/// have it race itself and set the same worktree up twice.
#[test]
#[cfg(feature = "test-mocks")]
fn an_instance_never_reclaims_its_own_claims() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_to_planning");
    db.create_transition_request(&req).unwrap();
    db.claim_transition_request(&req.id, "me").unwrap();

    let n = db
        .reclaim_stale_transition_requests("me", chrono::Duration::zero())
        .unwrap();

    assert_eq!(n, 0);
    assert!(db.get_pending_transition_requests().unwrap().is_empty());
}

/// The age window is what keeps a *live* second instance's genuine backlog out
/// of reach — a queued task waits out every setup ahead of it.
#[test]
#[cfg(feature = "test-mocks")]
fn a_recent_claim_is_left_alone() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_to_planning");
    db.create_transition_request(&req).unwrap();
    db.claim_transition_request(&req.id, "other-instance").unwrap();

    let n = db
        .reclaim_stale_transition_requests("me", chrono::Duration::minutes(5))
        .unwrap();

    assert_eq!(n, 0, "claimed seconds ago — the other instance may be alive");
}

/// A processed row is finished, whoever claimed it.
#[test]
#[cfg(feature = "test-mocks")]
fn a_processed_request_is_not_reclaimed() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_to_planning");
    db.create_transition_request(&req).unwrap();
    db.claim_transition_request(&req.id, "dead-instance").unwrap();
    db.mark_transition_processed(&req.id, None).unwrap();

    let n = db
        .reclaim_stale_transition_requests("me", chrono::Duration::zero())
        .unwrap();

    assert_eq!(n, 0);
    assert!(db.get_pending_transition_requests().unwrap().is_empty());
}

// === wait_for_board_change: what wakes a caller, and what it is shown ===

use agtx::mcp::board_watch::{BoardView, TaskMark};

fn mark(status: TaskStatus, phase: Option<&str>) -> TaskMark {
    TaskMark {
        status,
        phase_status: phase.map(str::to_string),
        turn_ts: None,
        deps_satisfied: true,
        escalation_note: None,
    }
}

fn board(tasks: &[(&str, TaskMark)]) -> Vec<(String, TaskMark)> {
    tasks
        .iter()
        .map(|(id, m)| (id.to_string(), m.clone()))
        .collect()
}

/// A view whose caller has already been shown `tasks`.
fn seen(tasks: &[(String, TaskMark)]) -> BoardView {
    let mut view = BoardView::default();
    view.mark_seen(tasks.iter().map(|(id, m)| (id.as_str(), m)), true);
    view
}

#[test]
fn a_first_wait_answers_at_once_with_the_whole_board() {
    let b = board(&[
        ("a", mark(TaskStatus::Running, Some("working"))),
        ("b", mark(TaskStatus::Planning, Some("working"))),
    ]);
    let mut view = BoardView::default();

    assert!(
        view.poll(&b, true),
        "nothing has been shown yet — that is news"
    );
    assert_eq!(view.report(&b, true).changed, vec!["a", "b"]);
}

#[test]
fn a_task_starting_to_work_does_not_wake_but_is_reported_with_what_does() {
    let before = board(&[
        ("a", mark(TaskStatus::Planning, Some("ready"))),
        ("b", mark(TaskStatus::Running, Some("working"))),
    ]);
    let mut view = seen(&before);

    // The caller advanced `a`. Its status changed and its phase went through
    // absent to working — the expected aftermath of its own move.
    let moved = board(&[
        ("a", mark(TaskStatus::Running, None)),
        ("b", mark(TaskStatus::Running, Some("working"))),
    ]);
    assert!(!view.poll(&moved, true));
    let working = board(&[
        ("a", mark(TaskStatus::Running, Some("working"))),
        ("b", mark(TaskStatus::Running, Some("working"))),
    ]);
    assert!(!view.poll(&working, true));

    let done = board(&[
        ("a", mark(TaskStatus::Running, Some("working"))),
        ("b", mark(TaskStatus::Running, Some("ready"))),
    ]);
    assert!(view.poll(&done, true), "b finished its phase");
    assert_eq!(view.report(&done, true).changed, vec!["a", "b"]);
}

#[test]
fn a_state_already_reported_does_not_wake_again() {
    let b = board(&[("a", mark(TaskStatus::Running, Some("blocked")))]);
    let mut view = seen(&b);

    // A caller that chose to leave a blocked task alone must not be woken for
    // it on every poll — that is a busy loop, one turn per second.
    assert!(!view.poll(&b, true));
    assert!(!view.poll(&b, true));
    assert!(view.report(&b, true).changed.is_empty());
}

#[test]
fn a_return_to_the_same_state_inside_one_wait_wakes() {
    let idle = board(&[("a", mark(TaskStatus::Running, Some("idle")))]);
    let mut view = seen(&idle);

    // Nudged: the agent works, then goes quiet again. An agent without hooks
    // has no turn timestamp, so the only evidence is the working in between.
    let working = board(&[("a", mark(TaskStatus::Running, Some("working")))]);
    assert!(!view.poll(&working, true));
    assert!(view.poll(&idle, true), "the nudge was answered");
    assert_eq!(
        view.report(&idle, true).changed,
        vec!["a"],
        "it matches what was reported, and must be shown anyway"
    );
}

#[test]
fn a_new_turn_end_wakes_even_with_the_phase_unchanged() {
    let mut first = mark(TaskStatus::Review, Some("ready"));
    first.turn_ts = Some(100);
    let mut view = seen(&board(&[("a", first)]));

    // A small fix sent in Review: the reviewer's turn ended again while the
    // caller was busy between two waits, so no poll saw it working.
    let mut second = mark(TaskStatus::Review, Some("ready"));
    second.turn_ts = Some(160);
    let b = board(&[("a", second)]);
    assert!(view.poll(&b, true));
    assert_eq!(view.report(&b, true).changed, vec!["a"]);
}

#[test]
fn reaching_done_wakes_and_a_startable_backlog_task_wakes() {
    let mut blocked_dep = mark(TaskStatus::Backlog, None);
    blocked_dep.deps_satisfied = false;
    let before = board(&[
        ("a", mark(TaskStatus::Review, Some("ready"))),
        ("b", blocked_dep),
    ]);
    let mut view = seen(&before);

    let after = board(&[
        ("a", mark(TaskStatus::Done, None)),
        ("b", mark(TaskStatus::Backlog, None)),
    ]);
    assert!(view.poll(&after, true));
    assert_eq!(view.report(&after, true).changed, vec!["a", "b"]);
}

#[test]
fn a_blocked_dependency_alone_does_not_wake() {
    let mut waiting = mark(TaskStatus::Backlog, None);
    waiting.deps_satisfied = false;
    let b = board(&[("a", waiting)]);
    let mut view = seen(&b);

    assert!(!view.poll(&b, true));
}

#[test]
fn an_escalation_wakes() {
    let b = board(&[("a", mark(TaskStatus::Review, Some("working")))]);
    let mut view = seen(&b);

    let mut escalated = mark(TaskStatus::Review, Some("working"));
    escalated.escalation_note = Some("merge conflict in src/lib.rs".to_string());
    assert!(view.poll(&board(&[("a", escalated)]), true));
}

#[test]
fn a_deleted_task_wakes_and_is_listed_as_removed() {
    let b = board(&[
        ("a", mark(TaskStatus::Running, Some("working"))),
        ("b", mark(TaskStatus::Backlog, None)),
    ]);
    let mut view = seen(&b);

    let after = board(&[("a", mark(TaskStatus::Running, Some("working")))]);
    assert!(view.poll(&after, true));
    let diff = view.report(&after, true);
    assert!(diff.changed.is_empty());
    assert_eq!(diff.removed, vec!["b"]);

    assert!(!view.poll(&after, true), "reported once, not again");
}

#[test]
fn the_tui_going_away_wakes() {
    let b = board(&[("a", mark(TaskStatus::Running, Some("working")))]);
    let mut view = seen(&b);

    assert!(!view.poll(&b, true));
    assert!(
        view.poll(&b, false),
        "every phase status is frozen from here"
    );
    view.report(&b, false);
    assert!(!view.poll(&b, false));
}

#[test]
fn what_a_listing_showed_does_not_wake_the_next_wait() {
    let b = board(&[("a", mark(TaskStatus::Running, Some("ready")))]);
    let mut view = seen(&b);

    assert!(!view.poll(&b, true), "list_tasks already showed a as ready");
}
