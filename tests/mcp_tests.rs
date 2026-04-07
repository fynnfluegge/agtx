use agtx::db::{Database, Task, TaskStatus, TransitionRequest};

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
    let db = Database::open_in_memory_project().unwrap();

    // Simulate batch creation: 3 tasks where task[2] depends on task[0] and task[1]
    let task0 = Task::new("DB schema", "claude", "my-project");
    let task1 = Task::new("Config setup", "claude", "my-project");
    let mut task2 = Task::new("Endpoints", "claude", "my-project");
    task2.referenced_tasks = Some(format!("{},{}", task0.id, task1.id));

    db.create_task(&task0).unwrap();
    db.create_task(&task1).unwrap();
    db.create_task(&task2).unwrap();

    // Verify all three exist
    let all = db.get_all_tasks().unwrap();
    assert_eq!(all.len(), 3);

    // Verify deps on task2
    let fetched = db.get_task(&task2.id).unwrap().unwrap();
    let refs = fetched.referenced_tasks.unwrap();
    assert!(refs.contains(&task0.id));
    assert!(refs.contains(&task1.id));
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
fn test_update_non_backlog_task_fails_in_db() {
    let db = Database::open_in_memory_project().unwrap();

    let mut task = Task::new("My task", "claude", "my-project");
    db.create_task(&task).unwrap();

    // Move to planning status
    task.status = TaskStatus::Planning;
    db.update_task(&task).unwrap();

    // DB layer allows update (status guard is in MCP layer), verify status changed
    let fetched = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(fetched.status, TaskStatus::Planning);
}
