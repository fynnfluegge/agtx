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
    assert_eq!(fetched.description.as_deref(), Some("Implement OAuth with Google"));
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

    db.create_notification(&Notification::new("phase completed")).unwrap();
    db.create_notification(&Notification::new("task ready")).unwrap();

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

    db.create_notification(&Notification::new("event A")).unwrap();
    db.create_notification(&Notification::new("event B")).unwrap();

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
    db.create_notification(&Notification::new("second")).unwrap();
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
    db.backdate_transition_processed_at(&req.id, &two_hours_ago).unwrap();

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
    assert!(fetched.is_some(), "recently processed request should be kept");
}
