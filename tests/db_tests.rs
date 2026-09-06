use agtx::db::{
    Database, Notification, NotificationKind, PhaseStatus, Project, Task, TaskRuntime, TaskStatus,
    TransitionRequest,
};

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
    assert!(!db
        .claim_transition_request("does-not-exist", "agtx-A")
        .unwrap());
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

    db.claim_transition_request(&fresh_claim.id, "agtx-A")
        .unwrap();
    db.claim_transition_request(&stale_claim.id, "agtx-A")
        .unwrap();
    let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
    db.backdate_transition_requested_at(&stale_claim.id, &two_hours_ago)
        .unwrap();

    db.cleanup_old_transition_requests().unwrap();

    assert!(db
        .get_transition_request(&stale_claim.id)
        .unwrap()
        .is_none());
    assert!(db
        .get_transition_request(&fresh_claim.id)
        .unwrap()
        .is_some());
    assert!(db
        .get_transition_request(&fresh_unclaimed.id)
        .unwrap()
        .is_some());
}

#[test]
fn test_get_pending_transition_requests_excludes_claimed() {
    let db = Database::open_in_memory_project().unwrap();

    let req_a = TransitionRequest::new("task-a", "move_forward");
    let req_b = TransitionRequest::new("task-b", "move_forward");
    db.create_transition_request(&req_a).unwrap();
    db.create_transition_request(&req_b).unwrap();
    db.claim_transition_request(&req_a.id, "other-instance")
        .unwrap();

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

    let wins: usize = handles
        .into_iter()
        .filter_map(|h| h.join().ok())
        .filter(|w| *w)
        .count();
    assert_eq!(wins, 1, "exactly one thread must win the claim; got {wins}");

    // Row must now be excluded from pending — a late claim must also fail.
    let followup = Database::open_project_at_path(&db_path).unwrap();
    assert!(
        followup
            .get_pending_transition_requests()
            .unwrap()
            .is_empty(),
        "claimed request must be filtered from pending"
    );
    assert!(
        !followup
            .claim_transition_request(&req.id, "late-comer")
            .unwrap(),
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
        setup
            .create_notification(&Notification::new(format!("msg-{i}")))
            .unwrap();
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

    assert_eq!(
        seen.len(),
        NOTIFS,
        "total consumed must equal total created"
    );
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

// === Stable Hash and DB Permissions Tests (Fix 3, Fix 7) ===

use std::path::Path;
use tempfile::TempDir;

/// Point `Database` at a throwaway data root for the tests below.
///
/// These are the only tests that open a *real* database rather than an
/// in-memory one. Without the redirect each run leaves an orphan
/// `projects/<hash>.db` in the user's own store, keyed by a temp path that no
/// longer exists — nothing ever collects them. The lock is needed because the
/// redirect target is process-global.
///
/// Hold the returned guard for the duration of the test.
fn redirect_data_dir() -> (std::path::PathBuf, std::sync::MutexGuard<'static, ()>) {
    static DATA_DIR: std::sync::OnceLock<TempDir> = std::sync::OnceLock::new();
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    // A panicking test poisons the lock; the data is unit, so recover and carry on.
    let guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let dir = DATA_DIR.get_or_init(|| TempDir::new().unwrap());
    std::env::set_var("AGTX_DATA_DIR", dir.path());
    (dir.path().to_path_buf(), guard)
}

#[test]
fn test_open_project_same_path_returns_same_db() {
    let (_data_dir, _guard) = redirect_data_dir();
    let temp_dir = TempDir::new().unwrap();
    let project_path = temp_dir.path();

    // Open twice with the same path — should get the same database (same tasks)
    let db1 = Database::open_project(project_path).unwrap();
    let task = Task::new("Persistence test", "claude", "proj");
    db1.create_task(&task).unwrap();
    drop(db1);

    let db2 = Database::open_project(project_path).unwrap();
    let retrieved = db2.get_task(&task.id).unwrap();
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().title, "Persistence test");
}

#[test]
fn test_open_project_different_paths_are_isolated() {
    let (_data_dir, _guard) = redirect_data_dir();
    let temp1 = TempDir::new().unwrap();
    let temp2 = TempDir::new().unwrap();

    let db1 = Database::open_project(temp1.path()).unwrap();
    let task = Task::new("Only in db1", "claude", "proj");
    db1.create_task(&task).unwrap();
    drop(db1);

    let db2 = Database::open_project(temp2.path()).unwrap();
    let tasks = db2.get_all_tasks().unwrap();
    assert!(tasks.is_empty());
}

#[cfg(unix)]
#[test]
fn test_project_db_file_permissions_are_0600() {
    use sha2::{Digest, Sha256};
    use std::os::unix::fs::PermissionsExt;

    let (data_dir, _guard) = redirect_data_dir();
    let temp_dir = TempDir::new().unwrap();
    let _db = Database::open_project(temp_dir.path()).unwrap();

    // Replicate the same hash logic used by Database::open_project to find
    // the exact DB file created for this temp dir, avoiding checking unrelated
    // DB files that may have been created by other tests or real usage.
    let path_str = temp_dir.path().to_string_lossy();
    let mut hasher = Sha256::new();
    hasher.update(path_str.as_bytes());
    let result = hasher.finalize();
    let path_hash = format!(
        "{:016x}",
        u64::from_be_bytes(result[..8].try_into().unwrap())
    );

    let db_path = data_dir.join("projects").join(format!("{}.db", path_hash));

    assert!(
        db_path.exists(),
        "Expected DB file not found at {:?}",
        db_path
    );
    let perms = std::fs::metadata(&db_path).unwrap().permissions();
    let mode = perms.mode() & 0o777;
    assert_eq!(mode, 0o600, "DB file should be owner-only read/write");
}

#[cfg(unix)]
#[test]
fn test_global_db_file_permissions_are_0600() {
    use std::os::unix::fs::PermissionsExt;

    let (data_dir, _guard) = redirect_data_dir();
    let _db = Database::open_global().unwrap();

    // The redirect makes this unconditional: before it, the test opened the
    // user's own index.db and skipped the assertion when it happened not to
    // exist yet.
    let db_path = data_dir.join("index.db");
    assert!(db_path.exists(), "Expected index.db at {:?}", db_path);

    let perms = std::fs::metadata(&db_path).unwrap().permissions();
    let mode = perms.mode() & 0o777;
    assert_eq!(
        mode, 0o600,
        "Global DB file should be owner-only read/write"
    );
}

// === Dependency-graph integration tests (real Database + deps_satisfied) ===

use agtx::tui::dep_graph::build_dep_graph;

/// Build a dependency graph from a real in-memory database, exercising the
/// same `deps_satisfied` rule the board uses. This complements the pure-model
/// unit tests in src/tui/dep_graph.rs.
#[test]
fn test_dep_graph_levels_and_unblocked_from_db() {
    let db = Database::open_in_memory_project().unwrap();

    // Level 0: a completed dependency.
    let mut base = Task::new("Base", "claude", "proj");
    base.status = TaskStatus::Done;
    db.create_task(&base).unwrap();

    // Level 1: a Backlog task whose only dep (base) is Done -> unblocked.
    let mut api = Task::new("API", "claude", "proj");
    api.referenced_tasks = Some(base.id.clone());
    db.create_task(&api).unwrap();

    // Level 2: a Backlog task depending on API (still Backlog) -> blocked.
    let mut ui = Task::new("UI", "claude", "proj");
    ui.referenced_tasks = Some(api.id.clone());
    db.create_task(&ui).unwrap();

    let tasks = db.get_all_tasks().unwrap();
    let graph = build_dep_graph(&tasks, |t| db.deps_satisfied(t));

    let node = |id: &str| {
        graph
            .nodes
            .iter()
            .find(|n| n.task_id == id)
            .expect("node present")
    };

    // Topological columns.
    assert_eq!(node(&base.id).level, 0);
    assert_eq!(node(&api.id).level, 1);
    assert_eq!(node(&ui.id).level, 2);

    // Only API is an actionable (unblocked) Backlog task: its dep is Done.
    assert!(node(&api.id).unblocked);
    // UI's dep (API) is still in Backlog, so UI is blocked.
    assert!(!node(&ui.id).unblocked);
    // Base is Done, so it is not "unblocked" (not actionable).
    assert!(!node(&base.id).unblocked);

    let unblocked = graph.unblocked_ids();
    assert_eq!(unblocked, vec![api.id.clone()]);
}

#[test]
fn test_dep_graph_unblocks_chain_as_deps_complete() {
    let db = Database::open_in_memory_project().unwrap();

    // a (Backlog) -> b (Backlog): b depends on a.
    let a = Task::new("A", "claude", "proj");
    db.create_task(&a).unwrap();
    let mut b = Task::new("B", "claude", "proj");
    b.referenced_tasks = Some(a.id.clone());
    db.create_task(&b).unwrap();

    // Initially only A is unblocked (no deps); B is blocked on A.
    let tasks = db.get_all_tasks().unwrap();
    let graph = build_dep_graph(&tasks, |t| db.deps_satisfied(t));
    let mut unblocked = graph.unblocked_ids();
    unblocked.sort();
    assert_eq!(unblocked, vec![a.id.clone()]);

    // Complete A (move to Review). Now B should become unblocked.
    let mut a_done = db.get_task(&a.id).unwrap().unwrap();
    a_done.status = TaskStatus::Review;
    db.update_task(&a_done).unwrap();

    let tasks = db.get_all_tasks().unwrap();
    let graph = build_dep_graph(&tasks, |t| db.deps_satisfied(t));
    // A is no longer Backlog, so it drops out of the unblocked set; B enters it.
    assert_eq!(graph.unblocked_ids(), vec![b.id.clone()]);
}

// === Task Runtime (published phase status) ===

fn runtime_for(task_id: &str, phase: PhaseStatus) -> TaskRuntime {
    TaskRuntime {
        task_id: task_id.to_string(),
        phase_status: phase,
        pane_hash: Some("abc123".to_string()),
        pane_changed_at: Some(chrono::Utc::now()),
        updated_at: chrono::Utc::now(),
    }
}

#[test]
#[cfg(feature = "test-mocks")]
fn task_runtime_round_trips() {
    let db = Database::open_in_memory_project().unwrap();
    let task = Task::new("Runtime", "claude", "proj");
    db.create_task(&task).unwrap();

    db.publish_task_runtime(&[runtime_for(&task.id, PhaseStatus::Blocked)])
        .unwrap();

    let got = db.get_task_runtime(&task.id).unwrap().expect("row written");
    assert_eq!(got.task_id, task.id);
    assert_eq!(got.phase_status, PhaseStatus::Blocked);
    assert_eq!(got.pane_hash.as_deref(), Some("abc123"));
    assert!(got.pane_changed_at.is_some());
}

/// Every `PhaseStatus` must survive the trip. A variant that serialises to a
/// string `from_str` does not accept reads back as the `Working` fallback, and
/// the board would report a blocked task as busy.
#[test]
#[cfg(feature = "test-mocks")]
fn every_phase_status_survives_the_database() {
    let db = Database::open_in_memory_project().unwrap();
    let task = Task::new("Round trip", "claude", "proj");
    db.create_task(&task).unwrap();

    for phase in [
        PhaseStatus::Working,
        PhaseStatus::Blocked,
        PhaseStatus::Idle,
        PhaseStatus::Ready,
        PhaseStatus::Exited,
    ] {
        db.publish_task_runtime(&[runtime_for(&task.id, phase)])
            .unwrap();
        let got = db.get_task_runtime(&task.id).unwrap().unwrap();
        assert_eq!(got.phase_status, phase, "{:?} did not round-trip", phase);
    }
}

/// The refresh rewrites every live task on each pass, so the row is a current
/// snapshot rather than a history — a second write must replace, not duplicate
/// or fail on the primary key.
#[test]
#[cfg(feature = "test-mocks")]
fn task_runtime_publish_replaces() {
    let db = Database::open_in_memory_project().unwrap();
    let task = Task::new("Upsert", "claude", "proj");
    db.create_task(&task).unwrap();

    db.publish_task_runtime(&[runtime_for(&task.id, PhaseStatus::Working)])
        .unwrap();
    db.publish_task_runtime(&[runtime_for(&task.id, PhaseStatus::Ready)])
        .unwrap();

    assert_eq!(db.list_task_runtime().unwrap().len(), 1);
    let got = db.get_task_runtime(&task.id).unwrap().unwrap();
    assert_eq!(got.phase_status, PhaseStatus::Ready);
}

#[test]
#[cfg(feature = "test-mocks")]
fn task_runtime_missing_row_is_none() {
    let db = Database::open_in_memory_project().unwrap();
    assert!(db.get_task_runtime("nope").unwrap().is_none());
}

/// A deleted task's last status must not be served to readers as current. The
/// refresh only ever publishes live tasks, so the pruning has to ride inside the
/// publish rather than depend on the deleting caller remembering it — MCP
/// deletes tasks from another process entirely.
#[test]
#[cfg(feature = "test-mocks")]
fn publishing_drops_runtime_for_deleted_tasks() {
    let db = Database::open_in_memory_project().unwrap();
    let live = Task::new("Live", "claude", "proj");
    let gone = Task::new("Gone", "claude", "proj");
    db.create_task(&live).unwrap();
    db.create_task(&gone).unwrap();
    db.publish_task_runtime(&[
        runtime_for(&live.id, PhaseStatus::Working),
        runtime_for(&gone.id, PhaseStatus::Ready),
    ])
    .unwrap();
    assert_eq!(db.list_task_runtime().unwrap().len(), 2);

    db.delete_task(&gone.id).unwrap();
    // The next pass carries only the surviving task, as the refresh would.
    db.publish_task_runtime(&[runtime_for(&live.id, PhaseStatus::Working)])
        .unwrap();

    let rows = db.list_task_runtime().unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].task_id, live.id);
}

// === Notification routing fields ===

/// A consumer outside the TUI filters on the event and threads replies by task,
/// so both must survive the write — against prose, neither is recoverable.
#[test]
#[cfg(feature = "test-mocks")]
fn notifications_carry_task_and_kind() {
    let db = Database::open_in_memory_project().unwrap();
    let notif = Notification::for_task(
        NotificationKind::TaskStuck,
        "task-42",
        "Task is blocked waiting for user input",
    );
    db.create_notification(&notif).unwrap();

    let peeked = db.peek_notifications().unwrap();
    assert_eq!(peeked.len(), 1);
    assert_eq!(peeked[0].task_id.as_deref(), Some("task-42"));
    assert_eq!(peeked[0].kind, Some(NotificationKind::TaskStuck));

    // `consume` names its columns explicitly where `peek` uses `SELECT *`, so
    // the two can drift apart silently.
    let consumed = db.consume_notifications().unwrap();
    assert_eq!(consumed[0].task_id.as_deref(), Some("task-42"));
    assert_eq!(consumed[0].kind, Some(NotificationKind::TaskStuck));
}

/// Rows written before the columns existed still read back.
#[test]
#[cfg(feature = "test-mocks")]
fn untagged_notifications_still_read() {
    let db = Database::open_in_memory_project().unwrap();
    db.create_notification(&Notification::new("no task, no kind"))
        .unwrap();

    let notifs = db.consume_notifications().unwrap();
    assert_eq!(notifs.len(), 1);
    assert!(notifs[0].task_id.is_none());
    assert!(notifs[0].kind.is_none());
}

/// The stored spelling and the serde spelling are the same string, so a reader
/// going through the database and one going through serialised output agree.
#[test]
fn notification_kind_spellings_match_serde() {
    for kind in [
        NotificationKind::PhaseCompleted,
        NotificationKind::TaskStuck,
    ] {
        let json = serde_json::to_string(&kind).unwrap();
        assert_eq!(json, format!("\"{}\"", kind.as_str()));
        assert_eq!(NotificationKind::from_str(kind.as_str()), Some(kind));
    }
}
