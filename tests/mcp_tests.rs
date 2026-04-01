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

// === Subtask dep-blocking tests ===
// These test the DB queries that allowed_actions uses for dep-blocking and parent-blocking.

#[test]
#[cfg(feature = "test-mocks")]
fn test_dep_blocking_with_unresolved_dep() {
    let db = Database::open_in_memory_project().unwrap();

    let mut dep = Task::new("Dep task", "claude", "proj");
    dep.status = TaskStatus::Running; // not Done
    db.create_task(&dep).unwrap();

    let mut child = Task::new("Child task", "claude", "proj");
    child.subtask_deps = Some(dep.id.clone());
    child.status = TaskStatus::Running;
    db.create_task(&child).unwrap();

    // Simulate allowed_actions dep-blocking check
    let is_blocked = child.subtask_deps.as_ref().map_or(false, |deps_str| {
        deps_str.split(',').filter(|s| !s.is_empty()).any(|dep_id| {
            db.get_task(dep_id)
                .ok()
                .flatten()
                .map(|t| !matches!(t.status, TaskStatus::Done))
                .unwrap_or(true)
        })
    });
    assert!(is_blocked, "move_forward should be blocked when dep is Running");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_dep_blocking_clears_when_dep_done() {
    let db = Database::open_in_memory_project().unwrap();

    let mut dep = Task::new("Dep task", "claude", "proj");
    dep.status = agtx::db::TaskStatus::Running;
    db.create_task(&dep).unwrap();

    let mut child = Task::new("Child task", "claude", "proj");
    child.subtask_deps = Some(dep.id.clone());
    child.status = TaskStatus::Running;
    db.create_task(&child).unwrap();

    // Move dep to Done
    let mut dep_updated = dep.clone();
    dep_updated.status = TaskStatus::Done;
    db.update_task(&dep_updated).unwrap();

    let is_blocked = child.subtask_deps.as_ref().map_or(false, |deps_str| {
        deps_str.split(',').filter(|s| !s.is_empty()).any(|dep_id| {
            db.get_task(dep_id)
                .ok()
                .flatten()
                .map(|t| !matches!(t.status, TaskStatus::Done))
                .unwrap_or(true)
        })
    });
    assert!(!is_blocked, "move_forward unblocked when dep is Done");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_parent_blocking_with_active_children() {
    let db = Database::open_in_memory_project().unwrap();

    let mut parent = Task::new("Parent", "claude", "proj");
    parent.status = TaskStatus::Running;
    db.create_task(&parent).unwrap();

    let mut child = Task::new("Child", "claude", "proj");
    child.parent_task_id = Some(parent.id.clone());
    child.status = TaskStatus::Running; // not Done
    db.create_task(&child).unwrap();

    // Simulate parent-blocking check
    let children = db.get_child_tasks(&parent.id).unwrap();
    let all_done = children
        .iter()
        .all(|c| matches!(c.status, TaskStatus::Done));
    assert!(!all_done, "parent blocked while child is Running");
    assert!(!children.is_empty());
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_parent_blocking_clears_when_all_children_done() {
    let db = Database::open_in_memory_project().unwrap();

    let mut parent = Task::new("Parent", "claude", "proj");
    parent.status = TaskStatus::Running;
    db.create_task(&parent).unwrap();

    let mut child = Task::new("Child", "claude", "proj");
    child.parent_task_id = Some(parent.id.clone());
    child.status = TaskStatus::Done;
    db.create_task(&child).unwrap();

    let children = db.get_child_tasks(&parent.id).unwrap();
    let all_done = children
        .iter()
        .all(|c| matches!(c.status, TaskStatus::Done));
    assert!(all_done, "parent unblocked when all children are Done");
}
