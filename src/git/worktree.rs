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

    // If worktree already exists and is valid, return it
    if worktree_path.exists() && worktree_path.join(".git").exists() {
        return Ok(worktree_path);
    }

    // Clean up any partial worktree
    if worktree_path.exists() {
        let _ = std::fs::remove_dir_all(&worktree_path);
    }

    // Ensure parent directory exists
    if let Some(parent) = worktree_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Detect the main branch (main or master)
    let main_branch = detect_main_branch(project_path)?;

    // Create worktree with a new branch based on main
    let branch_name = format!("task/{}", task_slug);

    // First, try to delete the branch if it exists (from a previous failed attempt)
    let _ = Command::new("git")
        .current_dir(project_path)
        .args(["branch", "-D", &branch_name])
        .output();

    let output = Command::new("git")
        .current_dir(project_path)
        .args(["worktree", "add"])
        .arg(&worktree_path)
        .args(["-b", &branch_name, &main_branch])
        .output()
        .context("Failed to create git worktree")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create worktree: {}", stderr);
    }

    Ok(worktree_path)
}

/// Initialize a worktree by copying files and running an init script.
///
/// Returns a Vec of warning messages for any issues encountered.
/// Does not fail fatally — errors are collected and returned for the caller to display.
pub fn initialize_worktree(
    project_path: &Path,
    worktree_path: &Path,
    copy_files: Option<&str>,
    init_script: Option<&str>,
) -> Vec<String> {
    let mut warnings = Vec::new();

    if let Some(files_str) = copy_files {
        for entry in files_str.split(',') {
            let file_name = entry.trim();
            if file_name.is_empty() {
                continue;
            }
            let src = project_path.join(file_name);
            let dst = worktree_path.join(file_name);

            if let Some(parent) = dst.parent() {
                if !parent.exists() {
                    if let Err(e) = std::fs::create_dir_all(parent) {
                        warnings.push(format!(
                            "Failed to create directory for '{}': {}",
                            file_name, e
                        ));
                        continue;
                    }
                }
            }

            if !src.exists() {
                warnings.push(format!(
                    "copy_files: '{}' not found in project root, skipping",
                    file_name
                ));
                continue;
            }

            if src.is_dir() {
                warnings.push(format!(
                    "copy_files: '{}' is a directory, only individual files are supported",
                    file_name
                ));
                continue;
            }

            if let Err(e) = std::fs::copy(&src, &dst) {
                warnings.push(format!("Failed to copy '{}' to worktree: {}", file_name, e));
            }
        }
    }

    if let Some(script) = init_script {
        let script = script.trim();
        if !script.is_empty() {
            match Command::new("sh").arg("-c").arg(script).current_dir(worktree_path).output() {
                Ok(result) => {
                    if !result.status.success() {
                        let stderr = String::from_utf8_lossy(&result.stderr);
                        warnings.push(format!(
                            "init_script exited with {}: {}",
                            result.status,
                            stderr.trim()
                        ));
                    }
                }
                Err(e) => {
                    warnings.push(format!("Failed to run init_script: {}", e));
                }
            }
        }
    }

    warnings
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
