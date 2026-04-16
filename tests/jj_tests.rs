use agtx::git::{self, GitOperations, RealJjOps};
use std::path::Path;
use std::process::Command;
use tempfile::TempDir;

fn jj_available() -> bool {
    Command::new("jj")
        .args(["--version"])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Set up a pure jj repo (no-colocate: only `.jj`, no `.git`) with an initial
/// commit and a `main` bookmark that `trunk()` resolves to.
fn setup_jj_repo() -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path();

    Command::new("jj")
        .args(["git", "init", "--no-colocate"])
        .arg(path)
        .output()
        .expect("Failed to init jj repo");

    Command::new("jj")
        .current_dir(path)
        .args(["config", "set", "--repo", "user.name", "Test User"])
        .output()
        .expect("Failed to set user.name");

    Command::new("jj")
        .current_dir(path)
        .args(["config", "set", "--repo", "user.email", "test@test.com"])
        .output()
        .expect("Failed to set user.email");

    std::fs::write(path.join("README.md"), "# Test").unwrap();

    // Describe the working copy commit (jj snapshots the file)
    Command::new("jj")
        .current_dir(path)
        .args(["describe", "-m", "Initial commit"])
        .output()
        .expect("Failed to describe");

    // Move to a new empty working copy; the described commit becomes immutable
    Command::new("jj")
        .current_dir(path)
        .args(["new"])
        .output()
        .expect("Failed to create new commit");

    // Create a `main` bookmark on the initial commit
    Command::new("jj")
        .current_dir(path)
        .args(["bookmark", "create", "main", "-r", "@-"])
        .output()
        .expect("Failed to create main bookmark");

    // Point trunk() at `main`
    Command::new("jj")
        .current_dir(path)
        .args(["config", "set", "--repo", "revsets.trunk", "main"])
        .output()
        .expect("Failed to set revsets.trunk");

    temp_dir
}

/// Set up a colocated jj repo (git + jj sharing the same directory).
fn setup_colocated_jj_repo() -> TempDir {
    let temp_dir = TempDir::new().unwrap();
    let path = temp_dir.path();

    Command::new("git")
        .current_dir(path)
        .args(["init"])
        .output()
        .expect("Failed to git init");

    Command::new("git")
        .current_dir(path)
        .args(["config", "user.email", "test@test.com"])
        .output()
        .unwrap();

    Command::new("git")
        .current_dir(path)
        .args(["config", "user.name", "Test User"])
        .output()
        .unwrap();

    std::fs::write(path.join("README.md"), "# Test").unwrap();
    Command::new("git").current_dir(path).args(["add", "."]).output().unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["commit", "-m", "Initial commit"])
        .output()
        .unwrap();
    Command::new("git")
        .current_dir(path)
        .args(["branch", "-M", "main"])
        .output()
        .unwrap();

    // Colocate jj using the existing git repo
    Command::new("jj")
        .current_dir(path)
        .args(["git", "init", "--git-repo", "."])
        .output()
        .expect("Failed to colocate jj");

    temp_dir
}

// =============================================================================
// Detection tests
// =============================================================================

#[test]
fn test_is_jj_repo_true() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    assert!(git::is_jj_repo(temp_dir.path()));
}

#[test]
fn test_is_jj_repo_false_for_plain_dir() {
    if !jj_available() { return; }
    let temp_dir = TempDir::new().unwrap();
    assert!(!git::is_jj_repo(temp_dir.path()));
}

#[test]
fn test_is_jj_repo_false_for_git_only() {
    if !jj_available() { return; }
    let temp_dir = TempDir::new().unwrap();
    Command::new("git").current_dir(temp_dir.path()).args(["init"]).output().unwrap();
    assert!(!git::is_jj_repo(temp_dir.path()));
}

#[test]
fn test_detect_vcs_pure_jj() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    // --no-colocate: only .jj present, git can't see the repo
    assert_eq!(git::detect_vcs(temp_dir.path()), git::VcsBackend::Jj);
}

#[test]
fn test_detect_vcs_colocated() {
    if !jj_available() { return; }
    let temp_dir = setup_colocated_jj_repo();
    assert_eq!(git::detect_vcs(temp_dir.path()), git::VcsBackend::ColocatedJj);
}

#[test]
fn test_detect_vcs_git_only() {
    if !jj_available() { return; }
    let temp_dir = TempDir::new().unwrap();
    Command::new("git").current_dir(temp_dir.path()).args(["init"]).output().unwrap();
    assert_eq!(git::detect_vcs(temp_dir.path()), git::VcsBackend::Git);
}

#[test]
fn test_detect_vcs_plain_dir_fallback() {
    let temp_dir = TempDir::new().unwrap();
    // Neither git nor jj — falls back to Git variant
    assert_eq!(git::detect_vcs(temp_dir.path()), git::VcsBackend::Git);
}

// =============================================================================
// RealJjOps workspace tests
// =============================================================================

#[test]
fn test_jj_create_and_remove_workspace() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "test-task").unwrap();
    let ws_path = Path::new(&ws);

    assert!(ws_path.exists());
    assert!(ops.worktree_exists(temp_dir.path(), "test-task"));

    ops.remove_worktree(temp_dir.path(), &ws).unwrap();
    assert!(!ops.worktree_exists(temp_dir.path(), "test-task"));
}

#[test]
fn test_jj_create_workspace_idempotent() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws1 = ops.create_worktree(temp_dir.path(), "idem-task").unwrap();
    let ws2 = ops.create_worktree(temp_dir.path(), "idem-task").unwrap();
    assert_eq!(ws1, ws2);
    assert!(Path::new(&ws1).exists());
}

#[test]
fn test_jj_worktree_exists_false_for_nonexistent() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;
    assert!(!ops.worktree_exists(temp_dir.path(), "no-such-task"));
}

#[test]
fn test_jj_create_multiple_workspaces() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws1 = ops.create_worktree(temp_dir.path(), "task-1").unwrap();
    let ws2 = ops.create_worktree(temp_dir.path(), "task-2").unwrap();
    let ws3 = ops.create_worktree(temp_dir.path(), "task-3").unwrap();

    assert_ne!(ws1, ws2);
    assert_ne!(ws2, ws3);
    assert!(Path::new(&ws1).exists());
    assert!(Path::new(&ws2).exists());
    assert!(Path::new(&ws3).exists());

    ops.remove_worktree(temp_dir.path(), &ws1).unwrap();
    ops.remove_worktree(temp_dir.path(), &ws2).unwrap();
    ops.remove_worktree(temp_dir.path(), &ws3).unwrap();

    assert!(!Path::new(&ws1).exists());
    assert!(!Path::new(&ws2).exists());
    assert!(!Path::new(&ws3).exists());
}

// =============================================================================
// RealJjOps diff / status tests
// =============================================================================

#[test]
fn test_jj_has_changes_false_on_clean_workspace() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "clean-task").unwrap();
    let ws_path = Path::new(&ws);

    assert!(!ops.has_changes(ws_path));
}

#[test]
fn test_jj_has_changes_true_after_file_write() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "dirty-task").unwrap();
    let ws_path = Path::new(&ws);

    std::fs::write(ws_path.join("new_file.txt"), "hello").unwrap();

    assert!(ops.has_changes(ws_path));
}

#[test]
fn test_jj_diff_always_empty() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "diff-clean").unwrap();
    let ws_path = Path::new(&ws);

    // jj has no staging area — nothing is ever "unstaged"
    assert!(ops.diff(ws_path).is_empty());

    // still empty even with working-copy changes
    std::fs::write(ws_path.join("feature.txt"), "new feature").unwrap();
    assert!(ops.diff(ws_path).is_empty());
}

#[test]
fn test_jj_diff_cached_shows_changes() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "diff-cached").unwrap();
    let ws_path = Path::new(&ws);

    // clean workspace — no staged changes yet
    assert!(ops.diff_cached(ws_path).is_empty());

    // after a file write, diff_cached shows the full working-copy diff
    std::fs::write(ws_path.join("staged.txt"), "content").unwrap();
    let diff = ops.diff_cached(ws_path);
    assert!(!diff.is_empty());
    assert!(diff.contains("staged.txt"));
}

#[test]
fn test_jj_list_untracked_files_empty() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "untracked").unwrap();
    let ws_path = Path::new(&ws);

    // jj tracks all files — untracked list is always empty
    assert!(ops.list_untracked_files(ws_path).is_empty());
}

// =============================================================================
// RealJjOps add_all / commit tests
// =============================================================================

#[test]
fn test_jj_add_all_triggers_snapshot() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "snapshot").unwrap();
    let ws_path = Path::new(&ws);

    std::fs::write(ws_path.join("snap.txt"), "data").unwrap();
    // add_all should complete without error
    ops.add_all(ws_path).unwrap();
    // changes should still be visible after snapshot
    assert!(ops.has_changes(ws_path));
}

#[test]
fn test_jj_commit_creates_immutable_change() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "commit-task").unwrap();
    let ws_path = Path::new(&ws);

    std::fs::write(ws_path.join("work.txt"), "some work").unwrap();
    ops.add_all(ws_path).unwrap();
    assert!(ops.has_changes(ws_path));

    ops.commit(ws_path, "Do some work").unwrap();

    // After commit, working copy should be clean
    assert!(!ops.has_changes(ws_path));
}

#[test]
fn test_jj_commit_noop_on_clean_workspace() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "commit-clean").unwrap();
    let ws_path = Path::new(&ws);

    // Committing with no changes should not error
    // jj commit on an empty change is valid (creates an empty commit)
    let result = ops.commit(ws_path, "Empty commit");
    assert!(result.is_ok());
}

// =============================================================================
// RealJjOps list_files tests
// =============================================================================

#[test]
fn test_jj_list_files_includes_tracked_files() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    // The root repo has README.md from initial commit
    let files = ops.list_files(temp_dir.path());
    assert!(files.iter().any(|f| f.contains("README.md")));
}

#[test]
fn test_jj_list_files_from_workspace() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    let ws = ops.create_worktree(temp_dir.path(), "list-files").unwrap();
    let ws_path = Path::new(&ws);

    // Add a file so the workspace has something to list
    std::fs::write(ws_path.join("ws_file.txt"), "content").unwrap();

    let files = ops.list_files(ws_path);
    assert!(files.iter().any(|f| f.contains("ws_file.txt")));
}

// =============================================================================
// RealJjOps delete_branch (bookmark) tests
// =============================================================================

#[test]
fn test_jj_delete_nonexistent_bookmark_is_ok() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    // Deleting a bookmark that doesn't exist should not error
    let result = ops.delete_branch(temp_dir.path(), "no-such-bookmark");
    assert!(result.is_ok());
}

#[test]
fn test_jj_delete_existing_bookmark() {
    if !jj_available() { return; }
    let temp_dir = setup_jj_repo();
    let ops = RealJjOps;

    // Create a bookmark to delete
    Command::new("jj")
        .current_dir(temp_dir.path())
        .args(["bookmark", "create", "to-delete", "-r", "@-"])
        .output()
        .unwrap();

    let result = ops.delete_branch(temp_dir.path(), "to-delete");
    assert!(result.is_ok());
}
