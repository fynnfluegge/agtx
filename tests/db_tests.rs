use agtx::db::{Task, TaskStatus, Project};

// === TaskStatus Tests ===

#[test]
fn test_task_status_as_str() {
    assert_eq!(TaskStatus::Backlog.as_str(), "backlog");
    assert_eq!(TaskStatus::Explore.as_str(), "explore");
    assert_eq!(TaskStatus::Planning.as_str(), "planning");
    assert_eq!(TaskStatus::Running.as_str(), "running");
    assert_eq!(TaskStatus::Review.as_str(), "review");
    assert_eq!(TaskStatus::Done.as_str(), "done");
}

#[test]
fn test_task_status_from_str() {
    assert_eq!(TaskStatus::from_str("backlog"), Some(TaskStatus::Backlog));
    assert_eq!(TaskStatus::from_str("explore"), Some(TaskStatus::Explore));
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
    assert_eq!(columns.len(), 6);
    assert_eq!(columns[0], TaskStatus::Backlog);
    assert_eq!(columns[1], TaskStatus::Explore);
    assert_eq!(columns[2], TaskStatus::Planning);
    assert_eq!(columns[3], TaskStatus::Running);
    assert_eq!(columns[4], TaskStatus::Review);
    assert_eq!(columns[5], TaskStatus::Done);
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
fn test_task_unique_ids() {
    let task1 = Task::new("Task 1", "claude", "proj");
    let task2 = Task::new("Task 2", "claude", "proj");

    assert_ne!(task1.id, task2.id);
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
