use agtx::git;
use agtx::git::{GitOperations, RealGitOps};
use std::path::PathBuf;
use std::process::Command;
use tempfile::TempDir;

fn list_worktrees(project: &std::path::Path) -> String {
    let out = Command::new("git")
        .current_dir(project)
        .args(["worktree", "list"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&out.stdout).to_string()
}

/// Remove a task's worktree through the implementation production actually uses.
///
/// `RealGitOps::remove_worktree` takes the worktree path; these tests were
/// written against a `git::remove_worktree(project, task_id, worktree_dir)` free
/// function that no production code ever called.
fn remove_task_worktree(project: &std::path::Path, task_id: &str) -> anyhow::Result<()> {
    let wt = git::worktree_path(project, task_id, git::DEFAULT_WORKTREE_DIR);
    RealGitOps.remove_worktree(project, &wt.to_string_lossy())
}

// =============================================================================
// Pure function tests (no git repo needed)
// =============================================================================

#[test]
fn test_worktree_path() {
    let project = PathBuf::from("/home/user/project");
    let path = git::worktree_path(&project, "task-123", git::DEFAULT_WORKTREE_DIR);
    assert_eq!(
        path,
        PathBuf::from("/home/user/project/.agtx/worktrees/task-123")
    );
}

#[test]
fn test_worktree_path_with_special_chars() {
    let project = PathBuf::from("/home/user/my-project");
    let path = git::worktree_path(&project, "fix-bug-456", git::DEFAULT_WORKTREE_DIR);
    assert_eq!(
        path,
        PathBuf::from("/home/user/my-project/.agtx/worktrees/fix-bug-456")
    );
}

#[test]
fn test_worktree_path_nested_project() {
    let project = PathBuf::from("/home/user/projects/rust/agtx");
    let path = git::worktree_path(&project, "feature-abc", git::DEFAULT_WORKTREE_DIR);
    assert_eq!(
        path,
        PathBuf::from("/home/user/projects/rust/agtx/.agtx/worktrees/feature-abc")
    );
}

#[test]
fn test_worktree_path_with_custom_dir() {
    let project = PathBuf::from("/home/user/project");
    let path = git::worktree_path_with_dir(&project, "task-123", ".worktrees");
    assert_eq!(
        path,
        PathBuf::from("/home/user/project/.worktrees/task-123")
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
    remove_task_worktree(temp_dir.path(), "test-task").unwrap();

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

    // A failed removal is reported rather than swallowed. It used to return
    // Ok(()) whatever git did, which is what let a stale registration sit in
    // `git worktree list` unnoticed.
    let result = remove_task_worktree(temp_dir.path(), "does-not-exist");
    assert!(
        result.is_err(),
        "a removal that git rejects must not report success"
    );
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
    remove_task_worktree(temp_dir.path(), "task-1").unwrap();
    remove_task_worktree(temp_dir.path(), "task-2").unwrap();
    remove_task_worktree(temp_dir.path(), "task-3").unwrap();

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
    let result = remove_task_worktree(temp_dir.path(), "dirty-task");
    assert!(result.is_ok());
}

/// A removal that fails must still drop the registration it left dangling.
/// Without the prune, one failed removal leaves `git worktree list` wrong
/// permanently — not just until the next attempt.
///
/// Measured against git 2.49 to pick a failure this actually fixes:
///
/// | worktree state          | `remove --force` | still listed | prune fixes |
/// |-------------------------|------------------|--------------|-------------|
/// | directory deleted       | exit 0           | no           | n/a — git handles it |
/// | `.git` link missing     | exit 128         | yes          | **yes**     |
/// | directory now a file    | exit 128         | yes          | **yes**     |
/// | locked                  | exit 128         | yes          | no — prune respects locks |
///
/// So the case worth pinning is a half-deleted tree, which is the state a
/// crashed cleanup leaves behind — and the one `create_worktree` then has to
/// decide about on the next task with that slug.
#[test]
fn test_remove_worktree_prunes_after_a_failed_removal() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "half-deleted").unwrap();

    // Break the worktree the way an interrupted delete does: the directory is
    // still there, its link back to the repository is not.
    std::fs::remove_file(worktree_path.join(".git")).unwrap();
    assert!(
        list_worktrees(temp_dir.path()).contains("half-deleted"),
        "precondition: git should still list the worktree"
    );

    let result = remove_task_worktree(temp_dir.path(), "half-deleted");
    assert!(
        result.is_err(),
        "a removal git rejects must be reported, not swallowed"
    );
    assert!(
        !list_worktrees(temp_dir.path()).contains("half-deleted"),
        "the dangling registration must be pruned, got:\n{}",
        list_worktrees(temp_dir.path())
    );
}

/// Removal must unlink a symlink, never follow it into the directory it points
/// at. This is the precondition for sharing a `node_modules` across worktrees:
/// if cleanup followed the link, finishing one task would destroy the install
/// every other worktree depends on — and the project's own.
///
/// The test that must never be deleted. Assert on the *shared content*, not on
/// the worktree being gone: a version that deleted the target would still
/// remove the worktree and still return Ok.
#[test]
fn test_remove_worktree_does_not_follow_symlinks_out_of_the_worktree() {
    let temp_dir = setup_git_repo();
    let project = temp_dir.path();

    // A populated directory outside the worktree, standing in for the project's
    // own node_modules.
    let shared = project.join("node_modules");
    std::fs::create_dir_all(shared.join("left-pad")).unwrap();
    std::fs::write(shared.join("left-pad/index.js"), "module.exports = 1;").unwrap();

    let worktree_path = git::create_worktree(project, "linker").unwrap();
    std::os::unix::fs::symlink(&shared, worktree_path.join("node_modules")).unwrap();
    assert!(worktree_path
        .join("node_modules/left-pad/index.js")
        .exists());

    remove_task_worktree(project, "linker").unwrap();

    assert!(
        !worktree_path.exists(),
        "the worktree itself should be gone"
    );
    assert!(
        shared.join("left-pad/index.js").exists(),
        "the shared directory the symlink pointed at must survive"
    );
}

/// The same property for the plain recursive delete, which is what a background
/// trash sweep would use if tranche 2 is ever built.
#[test]
fn test_remove_dir_all_does_not_follow_symlinks() {
    let temp_dir = TempDir::new().unwrap();
    let shared = temp_dir.path().join("shared");
    std::fs::create_dir_all(&shared).unwrap();
    std::fs::write(shared.join("pkg.txt"), "content").unwrap();

    let wt = temp_dir.path().join("wt");
    std::fs::create_dir_all(&wt).unwrap();
    std::os::unix::fs::symlink(&shared, wt.join("node_modules")).unwrap();

    std::fs::remove_dir_all(&wt).unwrap();

    assert!(!wt.exists());
    assert!(
        shared.join("pkg.txt").exists(),
        "remove_dir_all must unlink the symlink, not recurse through it"
    );
}

/// Under `skip_worktree` a task's `worktree_path` *is* the project root, so
/// every cleanup path asks for the user's own checkout to be removed. That must
/// be a no-op — and it must be asserted on the directory, not the return value:
/// a version that moved the repository aside and reported success would pass a
/// return-value check.
#[test]
fn test_remove_worktree_refuses_the_main_working_tree() {
    let temp_dir = setup_git_repo();
    let project = temp_dir.path();
    std::fs::write(project.join("uncommitted.txt"), "work in progress").unwrap();

    let result = RealGitOps.remove_worktree(project, &project.to_string_lossy());

    assert!(result.is_ok(), "refusal is a no-op, not an error");
    assert!(project.exists(), "the project directory must survive");
    assert!(project.join(".git").exists(), "the repository must survive");
    assert!(
        project.join("uncommitted.txt").exists(),
        "uncommitted work must survive"
    );
}

/// The guard is not just a project-root string comparison: a path that reaches
/// the same repository by another route is still a main working tree.
#[test]
fn test_is_main_working_tree_distinguishes_worktrees() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "linked").unwrap();

    assert!(
        git::is_main_working_tree(temp_dir.path()),
        "the project root is a main working tree"
    );
    assert!(
        !git::is_main_working_tree(&worktree_path),
        "a linked worktree is not"
    );
    assert!(
        !git::is_main_working_tree(&temp_dir.path().join("does-not-exist")),
        "a missing path is not, so it does not block its own removal"
    );

    // The default worktree_dir is inside the project, so a worktree that has
    // lost its `.git` link makes git walk up and answer for the *main*
    // repository. Checking only `--git-dir == --git-common-dir` reports true
    // here and refuses to remove exactly the trees that most need it.
    std::fs::remove_file(worktree_path.join(".git")).unwrap();
    assert!(
        !git::is_main_working_tree(&worktree_path),
        "a half-deleted worktree inside the project is not the main working tree"
    );
}

// =============================================================================
// initialize_worktree tests
// =============================================================================

#[test]
fn test_initialize_worktree_no_config() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-none").unwrap();

    let warnings = git::initialize_worktree(temp_dir.path(), &worktree_path, None, None, &[]);
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
        &[],
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
        &[],
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
        &[],
    );
    assert!(warnings.is_empty());
    assert!(worktree_path.join("initialized.marker").exists());
}

#[test]
fn test_initialize_worktree_init_script_failure() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "init-script-fail").unwrap();

    let warnings =
        git::initialize_worktree(temp_dir.path(), &worktree_path, None, Some("exit 1"), &[]);
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
        &[],
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
        &[],
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

    let warnings =
        git::initialize_worktree(temp_dir.path(), &worktree_path, Some(", , "), None, &[]);
    assert!(warnings.is_empty());
}

#[test]
fn test_initialize_worktree_copy_directory_supported() {
    let temp_dir = setup_git_repo();
    let config_dir = temp_dir.path().join("config");
    std::fs::create_dir_all(&config_dir).unwrap();
    std::fs::write(config_dir.join("app.toml"), "key = 1").unwrap();

    let worktree_path = git::create_worktree(temp_dir.path(), "init-dir").unwrap();

    let warnings =
        git::initialize_worktree(temp_dir.path(), &worktree_path, Some("config"), None, &[]);
    assert_eq!(warnings.len(), 0);
    // Directory and its contents should be copied
    assert!(worktree_path.join("config").join("app.toml").exists());
    let content = std::fs::read_to_string(worktree_path.join("config").join("app.toml")).unwrap();
    assert_eq!(content, "key = 1");
}

// =============================================================================
// Conflict detection tests
// =============================================================================

#[test]
fn test_check_merge_conflicts_no_conflict() {
    let temp_dir = setup_git_repo();
    let path = temp_dir.path();

    // Create a feature branch with a non-conflicting change
    Command::new("git")
        .current_dir(path)
        .args(["checkout", "-b", "task/feature"])
        .output()
        .unwrap();

    std::fs::write(path.join("new_file.txt"), "feature content").unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "add new file"])
        .output()
        .unwrap();

    // Switch back to main
    Command::new("git")
        .current_dir(path)
        .args(["checkout", "main"])
        .output()
        .unwrap();

    let (has_conflicts, files) = git::check_merge_conflicts(path, "main", "task/feature").unwrap();
    assert!(!has_conflicts);
    assert!(files.is_empty());
}

#[test]
fn test_check_merge_conflicts_with_conflict() {
    let temp_dir = setup_git_repo();
    let path = temp_dir.path();

    // Create a feature branch that modifies README.md
    Command::new("git")
        .current_dir(path)
        .args(["checkout", "-b", "task/feature"])
        .output()
        .unwrap();

    std::fs::write(path.join("README.md"), "# Feature branch change").unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "modify readme on feature"])
        .output()
        .unwrap();

    // Switch back to main and make a conflicting change
    Command::new("git")
        .current_dir(path)
        .args(["checkout", "main"])
        .output()
        .unwrap();

    std::fs::write(path.join("README.md"), "# Main branch change").unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["add", "."])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "modify readme on main"])
        .output()
        .unwrap();

    let (has_conflicts, files) = git::check_merge_conflicts(path, "main", "task/feature").unwrap();
    assert!(has_conflicts);
    assert!(files.iter().any(|f| f.contains("README.md")));
}

#[test]
fn test_check_merge_conflicts_nonexistent_branch() {
    let temp_dir = setup_git_repo();
    let result = git::check_merge_conflicts(temp_dir.path(), "main", "nonexistent");
    // Should return error (git merge-tree fails on non-existent ref)
    assert!(result.is_err() || result.unwrap().0);
}

#[test]
fn test_detect_main_branch_public() {
    let temp_dir = setup_git_repo();
    let branch = git::detect_main_branch(temp_dir.path()).unwrap();
    assert_eq!(branch, "main");
}

// =============================================================================
// Path traversal validation tests (Fix 2)
// =============================================================================

#[test]
fn test_initialize_worktree_rejects_dotdot_traversal() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "traversal-test").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some("../../.ssh/id_rsa"),
        None,
        &[],
    );
    // Should produce a warning about path traversal, not copy the file
    assert!(!warnings.is_empty());
    assert!(warnings[0].contains(".."));
}

#[test]
fn test_initialize_worktree_rejects_multiple_traversal_paths() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "multi-traversal").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some("../../.ssh/id_rsa, ../../.aws/credentials, ../../../etc/passwd"),
        None,
        &[],
    );
    // All three should be rejected
    assert_eq!(warnings.len(), 3);
    for w in &warnings {
        assert!(w.contains(".."));
    }
}

#[test]
fn test_initialize_worktree_accepts_valid_nested_path() {
    let temp_dir = setup_git_repo();
    let src_dir = temp_dir.path().join("src");
    std::fs::create_dir_all(&src_dir).unwrap();
    std::fs::write(src_dir.join("lib.rs"), "fn main() {}").unwrap();

    let worktree_path = git::create_worktree(temp_dir.path(), "valid-nested").unwrap();

    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        Some("src/lib.rs"),
        None,
        &[],
    );
    assert!(warnings.is_empty());
    assert!(worktree_path.join("src").join("lib.rs").exists());
}

#[test]
fn test_initialize_worktree_copy_dirs_rejects_traversal() {
    let temp_dir = setup_git_repo();
    let worktree_path = git::create_worktree(temp_dir.path(), "dir-traversal").unwrap();

    // Create a directory outside the project that a symlink could point to
    let outside_dir = temp_dir.path().join("..");
    // The copy_dirs path traversal check should catch this
    let warnings = git::initialize_worktree(
        temp_dir.path(),
        &worktree_path,
        None,
        None,
        &["../outside".to_string()],
    );
    // The directory doesn't exist so it's silently skipped (is_dir check fails),
    // but if it did exist and resolved outside, it would be blocked
    // This test verifies no panic occurs
    let _ = warnings;
}

#[test]
fn test_initialize_worktree_symlink_traversal_blocked() {
    let temp_dir = setup_git_repo();

    // Create a file outside the project
    let outside_dir = TempDir::new().unwrap();
    std::fs::write(outside_dir.path().join("secret.txt"), "sensitive data").unwrap();

    // Create a symlink inside the project pointing outside
    let link_path = temp_dir.path().join("sneaky-link");
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside_dir.path().join("secret.txt"), &link_path).unwrap();

    #[cfg(unix)]
    {
        let worktree_path = git::create_worktree(temp_dir.path(), "symlink-test").unwrap();

        let warnings = git::initialize_worktree(
            temp_dir.path(),
            &worktree_path,
            Some("sneaky-link"),
            None,
            &[],
        );
        // The canonicalized path of the symlink resolves outside the project root
        assert!(!warnings.is_empty());
        assert!(warnings[0].contains("outside project root"));
    }
}
