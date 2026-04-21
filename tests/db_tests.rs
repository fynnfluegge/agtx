use agtx::db::{Database, Notification, Project, Task, TaskStatus, TransitionRequest};

// === TaskStatus Tests ===

#[test]
fn test_task_status_as_str() {
    assert_eq!(TaskStatus::Backlog.as_str(), "backlog");
    assert_eq!(TaskStatus::Planning.as_str(), "planning");
    assert_eq!(TaskStatus::Running.as_str(), "running");
    assert_eq!(TaskStatus::Review.as_str(), "review");
    assert_eq!(TaskStatus::Done.as_str(), "done");
}

#[test]
fn test_task_status_from_str() {
    assert_eq!(TaskStatus::from_str("backlog"), Some(TaskStatus::Backlog));
    assert_eq!(TaskStatus::from_str("planning"), Some(TaskStatus::Planning));
    assert_eq!(TaskStatus::from_str("running"), Some(TaskStatus::Running));
    assert_eq!(TaskStatus::from_str("review"), Some(TaskStatus::Review));
    assert_eq!(TaskStatus::from_str("done"), Some(TaskStatus::Done));
    assert_eq!(TaskStatus::from_str("invalid"), None);
    assert_eq!(TaskStatus::from_str(""), None);
}

#[test]
fn test_task_status_columns() {
    let columns = TaskStatus::columns();
    assert_eq!(columns.len(), 5);
    assert_eq!(columns[0], TaskStatus::Backlog);
    assert_eq!(columns[1], TaskStatus::Planning);
    assert_eq!(columns[2], TaskStatus::Running);
    assert_eq!(columns[3], TaskStatus::Review);
    assert_eq!(columns[4], TaskStatus::Done);
}

#[test]
fn test_task_status_roundtrip() {
    for status in TaskStatus::columns() {
        let s = status.as_str();
        let parsed = TaskStatus::from_str(s);
        assert_eq!(parsed, Some(*status));
    }
}

// === Task Tests ===

#[test]
fn test_task_new() {
    let task = Task::new("Test Task", "claude", "project-123");

    assert!(!task.id.is_empty());
    assert_eq!(task.title, "Test Task");
    assert_eq!(task.agent, "claude");
    assert_eq!(task.project_id, "project-123");
    assert_eq!(task.status, TaskStatus::Backlog);
    assert!(task.description.is_none());
    assert!(task.session_name.is_none());
    assert!(task.worktree_path.is_none());
    assert!(task.branch_name.is_none());
    assert!(task.pr_number.is_none());
    assert!(task.pr_url.is_none());
}

#[test]
fn test_task_generate_session_name() {
    let task = Task::new("Add User Authentication", "claude", "proj");
    let session_name = task.generate_session_name("myproject");

    // Should contain task id prefix (8 chars)
    assert!(session_name.starts_with("task-"));
    assert!(session_name.contains("--myproject--"));
    assert!(session_name.contains("add-user-authenticat")); // truncated to 20 chars
}

#[test]
fn test_task_generate_session_name_special_chars() {
    let task = Task::new("Fix bug #123 (urgent!)", "claude", "proj");
    let session_name = task.generate_session_name("test");

    // Special chars should be converted to dashes
    assert!(!session_name.contains("#"));
    assert!(!session_name.contains("("));
    assert!(!session_name.contains(")"));
    assert!(!session_name.contains("!"));
}

#[test]
fn test_task_generate_session_name_project_dots() {
    let task = Task::new("Task Title", "claude", "proj");
    let session_name = task.generate_session_name("lazygit.nvim");

    assert!(session_name.contains("--lazygit-nvim--"));
    assert!(!session_name.contains(".nvim"));
}

#[test]
fn test_task_unique_ids() {
    let task1 = Task::new("Task 1", "claude", "proj");
    let task2 = Task::new("Task 2", "claude", "proj");

    assert_ne!(task1.id, task2.id);
}

#[test]
fn test_task_content_text_with_description() {
    let mut task = Task::new("My Title", "claude", "proj");
    task.description = Some("Detailed description".to_string());
    assert_eq!(task.content_text(), "Detailed description");
}

#[test]
fn test_task_content_text_without_description() {
    let task = Task::new("My Title", "claude", "proj");
    assert_eq!(task.content_text(), "My Title");
}

// === Project Tests ===

#[test]
fn test_project_new() {
    let project = Project::new("myproject", "/path/to/project");

    assert!(!project.id.is_empty());
    assert_eq!(project.name, "myproject");
    assert_eq!(project.path, "/path/to/project");
    assert!(project.github_url.is_none());
    assert!(project.default_agent.is_none());
}

#[test]
fn test_project_unique_ids() {
    let project1 = Project::new("proj1", "/path1");
    let project2 = Project::new("proj2", "/path2");

    assert_ne!(project1.id, project2.id);
}

// === In-Memory Database Tests ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_in_memory_project_db_creates_successfully() {
    let db = Database::open_in_memory_project().unwrap();
    // Should be able to create and retrieve a task
    let task = Task::new("Test Task", "claude", "proj-1");
    db.create_task(&task).unwrap();
    let retrieved = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(retrieved.title, "Test Task");
    assert_eq!(retrieved.status, TaskStatus::Backlog);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_in_memory_project_db_update_task() {
    let db = Database::open_in_memory_project().unwrap();
    let mut task = Task::new("Original", "claude", "proj-1");
    db.create_task(&task).unwrap();

    task.status = TaskStatus::Running;
    task.session_name = Some("session-1".to_string());
    db.update_task(&task).unwrap();

    let retrieved = db.get_task(&task.id).unwrap().unwrap();
    assert_eq!(retrieved.status, TaskStatus::Running);
    assert_eq!(retrieved.session_name.as_deref(), Some("session-1"));
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_in_memory_project_db_list_tasks() {
    let db = Database::open_in_memory_project().unwrap();
    let task1 = Task::new("Task 1", "claude", "proj-1");
    let task2 = Task::new("Task 2", "gemini", "proj-1");
    db.create_task(&task1).unwrap();
    db.create_task(&task2).unwrap();

    let tasks = db.get_tasks_by_status(TaskStatus::Backlog).unwrap();
    assert_eq!(tasks.len(), 2);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_in_memory_global_db_creates_successfully() {
    let db = Database::open_in_memory_global().unwrap();
    let project = Project::new("myproject", "/path/to/project");
    db.upsert_project(&project).unwrap();
    let projects = db.get_all_projects().unwrap();
    assert_eq!(projects.len(), 1);
    assert_eq!(projects[0].name, "myproject");
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_in_memory_dbs_are_isolated() {
    let db1 = Database::open_in_memory_project().unwrap();
    let db2 = Database::open_in_memory_project().unwrap();
    let task = Task::new("Only in db1", "claude", "proj-1");
    db1.create_task(&task).unwrap();

    // db2 should be empty — each in-memory DB is independent
    let tasks = db2.get_tasks_by_status(TaskStatus::Backlog).unwrap();
    assert_eq!(tasks.len(), 0);
}

// === Notification Tests ===

#[test]
#[cfg(feature = "test-mocks")]
fn test_notifications_create_and_consume() {
    let db = Database::open_in_memory_project().unwrap();

    let n1 = Notification::new("Task created: foo");
    let n2 = Notification::new("Phase completed: bar");
    db.create_notification(&n1).unwrap();
    db.create_notification(&n2).unwrap();

    // First consume returns both
    let notifs = db.consume_notifications().unwrap();
    assert_eq!(notifs.len(), 2);
    assert_eq!(notifs[0].message, "Task created: foo");
    assert_eq!(notifs[1].message, "Phase completed: bar");

    // Second consume returns empty (they were deleted)
    let notifs = db.consume_notifications().unwrap();
    assert_eq!(notifs.len(), 0);
}

#[test]
#[cfg(feature = "test-mocks")]
fn test_notifications_empty_queue() {
    let db = Database::open_in_memory_project().unwrap();
    let notifs = db.consume_notifications().unwrap();
    assert_eq!(notifs.len(), 0);
}

// === Dependency Satisfaction Tests ===

#[test]
fn test_deps_satisfied_no_refs() {
    let db = Database::open_in_memory_project().unwrap();
    let task = Task::new("No deps", "claude", "proj");
    db.create_task(&task).unwrap();
    assert!(db.deps_satisfied(&task));
}

#[test]
fn test_deps_satisfied_all_review_or_done() {
    let db = Database::open_in_memory_project().unwrap();

    let mut dep1 = Task::new("Dep 1", "claude", "proj");
    dep1.status = TaskStatus::Review;
    db.create_task(&dep1).unwrap();

    let mut dep2 = Task::new("Dep 2", "claude", "proj");
    dep2.status = TaskStatus::Done;
    db.create_task(&dep2).unwrap();

    let mut task = Task::new("Main task", "claude", "proj");
    task.referenced_tasks = Some(format!("{},{}", dep1.id, dep2.id));
    db.create_task(&task).unwrap();

    assert!(db.deps_satisfied(&task));
}

#[test]
fn test_deps_not_satisfied_dep_in_backlog() {
    let db = Database::open_in_memory_project().unwrap();

    let dep1 = Task::new("Dep in backlog", "claude", "proj");
    db.create_task(&dep1).unwrap();

    let mut dep2 = Task::new("Dep done", "claude", "proj");
    dep2.status = TaskStatus::Done;
    db.create_task(&dep2).unwrap();

    let mut task = Task::new("Blocked task", "claude", "proj");
    task.referenced_tasks = Some(format!("{},{}", dep1.id, dep2.id));
    db.create_task(&task).unwrap();

    assert!(!db.deps_satisfied(&task));
}

#[test]
fn test_deps_satisfied_missing_ref_treated_as_ok() {
    let db = Database::open_in_memory_project().unwrap();

    let mut task = Task::new("Task with missing ref", "claude", "proj");
    task.referenced_tasks = Some("nonexistent-id".to_string());
    db.create_task(&task).unwrap();

    // Missing refs are treated as satisfied (task may have been deleted)
    assert!(db.deps_satisfied(&task));
}

#[test]
fn test_deps_not_satisfied_dep_in_planning() {
    let db = Database::open_in_memory_project().unwrap();

    let mut dep = Task::new("Dep in planning", "claude", "proj");
    dep.status = TaskStatus::Planning;
    db.create_task(&dep).unwrap();

    let mut task = Task::new("Blocked task", "claude", "proj");
    task.referenced_tasks = Some(dep.id.clone());
    db.create_task(&task).unwrap();

    assert!(!db.deps_satisfied(&task));
}

// === transition_request claim tests ===

#[test]
fn test_claim_transition_request_first_claimant_wins() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();

    assert!(db.claim_transition_request(&req.id, "agtx-A").unwrap());
    assert!(!db.claim_transition_request(&req.id, "agtx-B").unwrap());
    assert!(!db.claim_transition_request(&req.id, "agtx-A").unwrap());
}

#[test]
fn test_claim_transition_request_fails_if_already_processed() {
    let db = Database::open_in_memory_project().unwrap();
    let req = TransitionRequest::new("task-1", "move_forward");
    db.create_transition_request(&req).unwrap();
    db.mark_transition_processed(&req.id, None).unwrap();

    assert!(!db.claim_transition_request(&req.id, "agtx-A").unwrap());
}

#[test]
fn test_claim_transition_request_fails_for_unknown_id() {
    let db = Database::open_in_memory_project().unwrap();
    assert!(!db.claim_transition_request("does-not-exist", "agtx-A").unwrap());
}

#[test]
fn test_cleanup_old_transition_requests_sweeps_stale_claims() {
    let db = Database::open_in_memory_project().unwrap();

    let fresh_claim = TransitionRequest::new("task-fresh", "move_forward");
    let stale_claim = TransitionRequest::new("task-stale", "move_forward");
    let fresh_unclaimed = TransitionRequest::new("task-unclaimed", "move_forward");
    db.create_transition_request(&fresh_claim).unwrap();
    db.create_transition_request(&stale_claim).unwrap();
    db.create_transition_request(&fresh_unclaimed).unwrap();

    db.claim_transition_request(&fresh_claim.id, "agtx-A").unwrap();
    db.claim_transition_request(&stale_claim.id, "agtx-A").unwrap();
    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    db.backdate_transition_requested_at(&stale_claim.id, &two_hours_ago).unwrap();

    db.cleanup_old_transition_requests().unwrap();

    assert!(db.get_transition_request(&stale_claim.id).unwrap().is_none());
    assert!(db.get_transition_request(&fresh_claim.id).unwrap().is_some());
    assert!(db.get_transition_request(&fresh_unclaimed.id).unwrap().is_some());
}

#[test]
fn test_get_pending_transition_requests_excludes_claimed() {
    let db = Database::open_in_memory_project().unwrap();

    let req_a = TransitionRequest::new("task-a", "move_forward");
    let req_b = TransitionRequest::new("task-b", "move_forward");
    db.create_transition_request(&req_a).unwrap();
    db.create_transition_request(&req_b).unwrap();
    db.claim_transition_request(&req_a.id, "other-instance").unwrap();

    let pending = db.get_pending_transition_requests().unwrap();
    let ids: Vec<&str> = pending.iter().map(|r| r.id.as_str()).collect();

    assert!(!ids.contains(&req_a.id.as_str()));
    assert!(ids.contains(&req_b.id.as_str()));
}

// N threads race one claim → exactly one winner (read-then-update would allow multiple).
#[test]
#[cfg(feature = "test-mocks")]
fn test_claim_transition_request_atomic_under_concurrent_claims() {
    use std::sync::{Arc, Barrier};
    use std::thread;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();

    let setup = Database::open_project_at_path(&db_path).unwrap();
    let req = TransitionRequest::new("task-race", "move_forward");
    setup.create_transition_request(&req).unwrap();
    drop(setup);

    const THREADS: usize = 16;
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::with_capacity(THREADS);
    for i in 0..THREADS {
        let path = db_path.clone();
        let req_id = req.id.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let db = Database::open_project_at_path(&path).unwrap();
            let claimant = format!("agtx-{}", i);
            barrier.wait();
            db.claim_transition_request(&req_id, &claimant).unwrap()
        }));
    }

    let wins: usize = handles.into_iter().filter_map(|h| h.join().ok()).filter(|w| *w).count();
    assert_eq!(wins, 1, "exactly one thread must win the claim; got {wins}");

    // Row must now be excluded from pending — a late claim must also fail.
    let followup = Database::open_project_at_path(&db_path).unwrap();
    assert!(
        followup.get_pending_transition_requests().unwrap().is_empty(),
        "claimed request must be filtered from pending"
    );
    assert!(
        !followup.claim_transition_request(&req.id, "late-comer").unwrap(),
        "a later claim after the race must return false"
    );
}

// N consumers drain the queue → each row returned exactly once (SELECT-then-DELETE would dupe).
#[test]
#[cfg(feature = "test-mocks")]
fn test_consume_notifications_atomic_under_concurrent_consumers() {
    use std::collections::HashSet;
    use std::sync::{Arc, Barrier};
    use std::thread;

    let tmp = tempfile::NamedTempFile::new().unwrap();
    let db_path = tmp.path().to_path_buf();

    const NOTIFS: usize = 64;
    let setup = Database::open_project_at_path(&db_path).unwrap();
    for i in 0..NOTIFS {
        setup.create_notification(&Notification::new(format!("msg-{i}"))).unwrap();
    }
    drop(setup);

    const THREADS: usize = 8;
    let barrier = Arc::new(Barrier::new(THREADS));
    let mut handles = Vec::with_capacity(THREADS);
    for _ in 0..THREADS {
        let path = db_path.clone();
        let barrier = Arc::clone(&barrier);
        handles.push(thread::spawn(move || {
            let db = Database::open_project_at_path(&path).unwrap();
            barrier.wait();
            db.consume_notifications().unwrap()
        }));
    }

    let mut seen: Vec<Notification> = Vec::new();
    for h in handles {
        seen.extend(h.join().unwrap());
    }

    assert_eq!(seen.len(), NOTIFS, "total consumed must equal total created");
    let unique_ids: HashSet<&str> = seen.iter().map(|n| n.id.as_str()).collect();
    assert_eq!(
        unique_ids.len(),
        NOTIFS,
        "each notification must be consumed exactly once (no peer double-reads)"
    );
    assert!(
        Database::open_project_at_path(&db_path)
            .unwrap()
            .peek_notifications()
            .unwrap()
            .is_empty(),
        "DB must be drained after concurrent consume"
    );
}
