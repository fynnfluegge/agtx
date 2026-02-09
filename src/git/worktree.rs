use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Directory name for agtx data within a project
const AGTX_DIR: &str = ".agtx";
const WORKTREES_DIR: &str = "worktrees";

/// Create a new git worktree for a task from the main branch
pub fn create_worktree(project_path: &Path, task_slug: &str) -> Result<PathBuf> {
    let worktree_path = project_path
        .join(AGTX_DIR)
        .join(WORKTREES_DIR)
        .join(task_slug);

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Detect the main branch (main or master)
    let main_branch = detect_main_branch(project_path)?;

    // Create worktree with a new branch based on main
    let branch_name = format!("task/{}", task_slug);
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["worktree", "add"])
        .arg(&worktree_path)
        .args(["-b", &branch_name, &main_branch])
        .output()
        .context("Failed to create git worktree")?;

    if !output.status.success() {
        anyhow::bail!(
            "Failed to create worktree: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    Ok(worktree_path)
}

/// Detect the main branch name (main or master)
fn detect_main_branch(project_path: &Path) -> Result<String> {
    // Check if 'main' exists
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--verify", "main"])
        .output()
        .context("Failed to check for main branch")?;

    if output.status.success() {
        return Ok("main".to_string());
    }

    // Check if 'master' exists
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--verify", "master"])
        .output()
        .context("Failed to check for master branch")?;

    if output.status.success() {
        return Ok("master".to_string());
    }

    // Fallback: get the current branch
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .output()
        .context("Failed to get current branch")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Remove a git worktree
pub fn remove_worktree(project_path: &Path, task_id: &str) -> Result<()> {
    let worktree_path = project_path
        .join(AGTX_DIR)
        .join(WORKTREES_DIR)
        .join(task_id);

    // Remove the worktree
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["worktree", "remove"])
        .arg(&worktree_path)
        .args(["--force"]) // Force in case of uncommitted changes
        .output()
        .context("Failed to remove git worktree")?;

    if !output.status.success() {
        // Try to prune if remove failed
        Command::new("git")
            .current_dir(project_path)
            .args(["worktree", "prune"])
            .output()?;
    }

    Ok(())
}

/// List all worktrees for a project
pub fn list_worktrees(project_path: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["worktree", "list", "--porcelain"])
        .output()
        .context("Failed to list git worktrees")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;

    for line in stdout.lines() {
        if line.starts_with("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(WorktreeInfo {
                path: PathBuf::from(line.strip_prefix("worktree ").unwrap()),
                branch: None,
                is_bare: false,
            });
        } else if line.starts_with("branch ") {
            if let Some(ref mut wt) = current {
                wt.branch = Some(
                    line.strip_prefix("branch refs/heads/")
                        .unwrap_or(line.strip_prefix("branch ").unwrap())
                        .to_string(),
                );
            }
        } else if line == "bare" {
            if let Some(ref mut wt) = current {
                wt.is_bare = true;
            }
        }
    }

    if let Some(wt) = current {
        worktrees.push(wt);
    }

    Ok(worktrees)
}

/// Get the worktree path for a task
pub fn worktree_path(project_path: &Path, task_id: &str) -> PathBuf {
    project_path
        .join(AGTX_DIR)
        .join(WORKTREES_DIR)
        .join(task_id)
}

/// Check if a worktree exists for a task
pub fn worktree_exists(project_path: &Path, task_id: &str) -> bool {
    worktree_path(project_path, task_id).exists()
}

/// Information about a git worktree
#[derive(Debug, Clone)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_bare: bool,
}

impl WorktreeInfo {
    /// Extract task ID from worktree path if it's an agtx worktree
    pub fn task_id(&self) -> Option<&str> {
        self.path
            .file_name()
            .and_then(|n| n.to_str())
            .filter(|_| {
                self.path
                    .parent()
                    .and_then(|p| p.file_name())
                    .map(|n| n == WORKTREES_DIR)
                    .unwrap_or(false)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_path() {
        let project = PathBuf::from("/home/user/project");
        let path = worktree_path(&project, "task-123");
        assert_eq!(
            path,
            PathBuf::from("/home/user/project/.agtx/worktrees/task-123")
        );
    }
}
