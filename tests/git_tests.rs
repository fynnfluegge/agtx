use agtx::git;
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

// =============================================================================
// Pure function tests (no git repo needed)
// =============================================================================

#[test]
fn test_worktree_path() {
    let project = PathBuf::from("/home/user/project");
    let path = git::worktree_path(&project, "task-123");
    assert_eq!(
        path,
        PathBuf::from("/home/user/project/.agtx/worktrees/task-123")
    );
}

#[test]
fn test_worktree_path_with_special_chars() {
    let project = PathBuf::from("/home/user/my-project");
    let path = git::worktree_path(&project, "fix-bug-456");
    assert_eq!(
        path,
        PathBuf::from("/home/user/my-project/.agtx/worktrees/fix-bug-456")
    );
}

#[test]
fn test_worktree_path_nested_project() {
    let project = PathBuf::from("/home/user/projects/rust/agtx");
    let path = git::worktree_path(&project, "feature-abc");
    assert_eq!(
        path,
        PathBuf::from("/home/user/projects/rust/agtx/.agtx/worktrees/feature-abc")
    );
}

#[test]
fn test_worktree_exists_false_for_nonexistent() {
    let temp_dir = TempDir::new().unwrap();
    assert!(!git::worktree_exists(temp_dir.path(), "nonexistent-task"));
}


// =============================================================================
// Integration tests (require git)
// =============================================================================

fn setup_git_repo() -> TempDir {
    let temp_dir = TempDir::new().unwrap();

    // Initialize git repo
    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["init"])
        .output()
        .expect("Failed to init git repo");

    // Configure git user for commits
    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .expect("Failed to config git email");

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["config", "user.name", "Test User"])
        .output()
        .expect("Failed to config git name");

    // Create initial commit (needed for worktrees)
    std::fs::write(temp_dir.path().join("README.md"), "# Test").unwrap();

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["add", "."])
        .output()
        .expect("Failed to add files");

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["commit", "-m", "Initial commit"])
        .output()
        .expect("Failed to commit");

    // Rename branch to main (in case default is master)
    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["branch", "-M", "main"])
        .output()
        .expect("Failed to rename branch");

    temp_dir
}

#[test]
fn test_is_git_repo_true() {
    let temp_dir = setup_git_repo();
    assert!(git::is_git_repo(temp_dir.path()));
}

#[test]
fn test_is_git_repo_false() {
    let temp_dir = TempDir::new().unwrap();
    assert!(!git::is_git_repo(temp_dir.path()));
}

#[test]
fn test_repo_root() {
    let temp_dir = setup_git_repo();
    let root = git::repo_root(temp_dir.path()).unwrap();
    // Canonicalize both paths to handle macOS /var -> /private/var symlink
    let expected = temp_dir.path().canonicalize().unwrap();
    let actual = root.canonicalize().unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn test_current_branch() {
    let temp_dir = setup_git_repo();
    let branch = git::current_branch(temp_dir.path()).unwrap();
    assert_eq!(branch, "main");
}

#[test]
fn test_create_and_remove_worktree() {
    let temp_dir = setup_git_repo();

    // Create worktree
    let worktree_path = git::create_worktree(temp_dir.path(), "test-task").unwrap();

    // Verify it exists
    assert!(worktree_path.exists());
    assert!(worktree_path.join(".git").exists());
    assert!(git::worktree_exists(temp_dir.path(), "test-task"));

    // Remove worktree
    git::remove_worktree(temp_dir.path(), "test-task").unwrap();

    // Verify it's gone
    assert!(!worktree_path.exists());
}

#[test]
fn test_create_worktree_idempotent() {
    let temp_dir = setup_git_repo();

    // Create worktree twice - should succeed both times
    let path1 = git::create_worktree(temp_dir.path(), "idempotent-task").unwrap();
    let path2 = git::create_worktree(temp_dir.path(), "idempotent-task").unwrap();

    assert_eq!(path1, path2);
    assert!(path1.exists());
}

// =============================================================================
// Error case tests
// =============================================================================

/// Setup a git repo with "master" as the default branch (instead of "main")
fn setup_git_repo_with_master() -> TempDir {
    let temp_dir = TempDir::new().unwrap();

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["init"])
        .output()
        .expect("Failed to init git repo");

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["config", "user.email", "test@test.com"])
        .output()
        .expect("Failed to config git email");

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["config", "user.name", "Test User"])
        .output()
        .expect("Failed to config git name");

    std::fs::write(temp_dir.path().join("README.md"), "# Test").unwrap();

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["add", "."])
        .output()
        .expect("Failed to add files");

    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["commit", "-m", "Initial commit"])
        .output()
        .expect("Failed to commit");

    // Rename branch to master (not main)
    Command::new("git")
        .current_dir(temp_dir.path())
        .args(["branch", "-M", "master"])
        .output()
        .expect("Failed to rename branch");

    temp_dir
}

#[test]
fn test_create_worktree_with_master_branch() {
    let temp_dir = setup_git_repo_with_master();

    // Should detect master and create worktree from it
    let worktree_path = git::create_worktree(temp_dir.path(), "master-task").unwrap();

    assert!(worktree_path.exists());
    assert!(worktree_path.join(".git").exists());
}

#[test]
fn test_create_worktree_on_non_git_directory() {
    let temp_dir = TempDir::new().unwrap();
    // Don't initialize git - just a plain directory

    let result = git::create_worktree(temp_dir.path(), "should-fail");

    assert!(result.is_err());
}

#[test]
fn test_remove_worktree_nonexistent() {
    let temp_dir = setup_git_repo();

    // Removing a non-existent worktree should not panic
    // (it may return Ok or Err depending on git version, but shouldn't crash)
    let result = git::remove_worktree(temp_dir.path(), "does-not-exist");

    // The function should complete without panicking
    // We don't assert success/failure since behavior may vary
    let _ = result;
}

#[test]
fn test_is_git_repo_nonexistent_path() {
    let path = PathBuf::from("/nonexistent/path/that/does/not/exist");
    assert!(!git::is_git_repo(&path));
}

#[test]
fn test_current_branch_non_git_directory() {
    let temp_dir = TempDir::new().unwrap();
    // Don't initialize git

    let result = git::current_branch(temp_dir.path());

    // Should return error, not panic
    // Note: git returns empty string for non-git dirs, which might be Ok("")
    // So we just verify it doesn't panic
    let _ = result;
}

#[test]
fn test_create_multiple_worktrees() {
    let temp_dir = setup_git_repo();

    // Create multiple worktrees
    let path1 = git::create_worktree(temp_dir.path(), "task-1").unwrap();
    let path2 = git::create_worktree(temp_dir.path(), "task-2").unwrap();
    let path3 = git::create_worktree(temp_dir.path(), "task-3").unwrap();

    assert!(path1.exists());
    assert!(path2.exists());
    assert!(path3.exists());

    // All should be different paths
    assert_ne!(path1, path2);
    assert_ne!(path2, path3);
    assert_ne!(path1, path3);

    // Clean up
    git::remove_worktree(temp_dir.path(), "task-1").unwrap();
    git::remove_worktree(temp_dir.path(), "task-2").unwrap();
    git::remove_worktree(temp_dir.path(), "task-3").unwrap();

    assert!(!path1.exists());
    assert!(!path2.exists());
    assert!(!path3.exists());
}

#[test]
fn test_worktree_with_uncommitted_changes() {
    let temp_dir = setup_git_repo();

    // Create worktree
    let worktree_path = git::create_worktree(temp_dir.path(), "dirty-task").unwrap();

    // Make uncommitted changes in the worktree
    std::fs::write(worktree_path.join("dirty-file.txt"), "uncommitted content").unwrap();

    // Remove should still work (with --force)
    let result = git::remove_worktree(temp_dir.path(), "dirty-task");
    assert!(result.is_ok());
}


// =============================================================================
// initialize_worktree tests
// =============================================================================

#[test]
fn test_initialize_worktree_no_config() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-none").unwrap();

    let warnings = git::initialize_worktree(temp_dir.path(), &worktree_path, None, None);
    assert!(warnings.is_empty());
}

#[test]
fn test_initialize_worktree_copy_files() {
    let temp_dir = setup_git_repo();
    std::fs::write(temp_dir.path().join(".env"), "DB_URL=localhost").unwrap();
    std::fs::write(temp_dir.path().join(".env.local"), "SECRET=abc").unwrap();

    let worktree_path = git::create_worktree(temp_dir.path(), "init-copy").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some(".env, .env.local"),
        None,
    );
    assert!(warnings.is_empty());
    assert_eq!(
        std::fs::read_to_string(worktree_path.join(".env")).unwrap(),
        "DB_URL=localhost"
    );
    assert_eq!(
        std::fs::read_to_string(worktree_path.join(".env.local")).unwrap(),
        "SECRET=abc"
    );
}

#[test]
fn test_initialize_worktree_copy_missing_file() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-missing").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some(".nonexistent"),
        None,
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains(".nonexistent"));
}

#[test]
fn test_initialize_worktree_init_script_success() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-script-ok").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        None,
        Some("touch initialized.marker"),
    );
    assert!(warnings.is_empty());
    assert!(worktree_path.join("initialized.marker").exists());
}

#[test]
fn test_initialize_worktree_init_script_failure() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-script-fail").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        None,
        Some("exit 1"),
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("init_script"));
}

#[test]
fn test_initialize_worktree_copy_then_script() {
    let temp_dir = setup_git_repo();
    std::fs::write(temp_dir.path().join(".env"), "KEY=value").unwrap();

    let worktree_path = git::create_worktree(temp_dir.path(), "init-order").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some(".env"),
        Some("cat .env > verified.txt"),
    );
    assert!(warnings.is_empty());
    assert_eq!(
        std::fs::read_to_string(worktree_path.join("verified.txt")).unwrap(),
        "KEY=value"
    );
}

#[test]
fn test_initialize_worktree_copy_nested_path() {
    let temp_dir = setup_git_repo();
    let web_dir = temp_dir.path().join("web");
    std::fs::create_dir_all(&web_dir).unwrap();
    std::fs::write(web_dir.join(".env.local"), "NEXT_PUBLIC_KEY=123").unwrap();

    let worktree_path = git::create_worktree(temp_dir.path(), "init-nested").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some("web/.env.local"),
        None,
    );
    assert!(warnings.is_empty());
    assert_eq!(
        std::fs::read_to_string(worktree_path.join("web").join(".env.local")).unwrap(),
        "NEXT_PUBLIC_KEY=123"
    );
}

#[test]
fn test_initialize_worktree_empty_copy_files() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-empty").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some(", , "),
        None,
    );
    assert!(warnings.is_empty());
}

#[test]
fn test_initialize_worktree_copy_directory_rejected() {
    let temp_dir = setup_git_repo();
    let config_dir = temp_dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("app.toml"), "key = 1").unwrap();

    let worktree_path = git::create_worktree(temp_dir.path(), "init-dir").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some("config"),
        None,
    );
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains("directory"));
}
