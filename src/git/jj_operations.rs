//! Jujutsu (jj) implementation of VCS operations.
//! Minimum supported jj version: 0.36

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

use super::operations::GitOperations;

const AGTX_DIR: &str = ".agtx";
const WORKTREES_DIR: &str = "worktrees";

fn worktree_path(project_path: &Path, task_slug: &str) -> PathBuf {
    project_path.join(AGTX_DIR).join(WORKTREES_DIR).join(task_slug)
}

pub struct RealJjOps;

impl GitOperations for RealJjOps {
    fn create_worktree(&self, project_path: &Path, task_slug: &str) -> Result<String> {
        let path = worktree_path(project_path, task_slug);

        // If workspace already exists, return it
        if path.exists() {
            return Ok(path.to_string_lossy().to_string());
        }

        // Clean up any partial directory
        if path.exists() {
            let _ = std::fs::remove_dir_all(&path);
        }

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Create workspace at trunk() so it starts from the main branch
        let output = Command::new("jj")
            .current_dir(project_path)
            .args(["workspace", "add", "--revision", "trunk()"])
            .arg(&path)
            .output()
            .context("Failed to create jj workspace")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to create jj workspace: {}", stderr);
        }

        Ok(path.to_string_lossy().to_string())
    }

    fn remove_worktree(&self, project_path: &Path, worktree_path: &str) -> Result<()> {
        let path = Path::new(worktree_path);
        // Derive workspace name from the last path component
        let workspace_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(worktree_path);

        // Unregister from jj (ignore errors — may already be forgotten)
        let _ = Command::new("jj")
            .current_dir(project_path)
            .args(["workspace", "forget", workspace_name])
            .output();

        // Remove the directory
        if path.exists() {
            std::fs::remove_dir_all(path)?;
        }

        Ok(())
    }

    fn worktree_exists(&self, project_path: &Path, task_slug: &str) -> bool {
        worktree_path(project_path, task_slug).exists()
    }

    fn delete_branch(&self, project_path: &Path, branch_name: &str) -> Result<()> {
        // Ignore errors — bookmark may not exist
        let _ = Command::new("jj")
            .current_dir(project_path)
            .args(["bookmark", "delete", branch_name])
            .output();
        Ok(())
    }

    fn diff(&self, _worktree_path: &Path) -> String {
        // jj has no staging area — everything is auto-tracked, nothing is "unstaged"
        String::new()
    }

    fn diff_cached(&self, worktree_path: &Path) -> String {
        // jj has no staging area — everything is effectively "staged"
        Command::new("jj")
            .current_dir(worktree_path)
            .args(["diff", "--git"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn list_untracked_files(&self, _worktree_path: &Path) -> String {
        // jj tracks all files automatically; no concept of untracked files
        String::new()
    }

    fn diff_untracked_file(&self, worktree_path: &Path, file: &str) -> String {
        Command::new("jj")
            .current_dir(worktree_path)
            .args(["diff", "--git", "--", file])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn diff_stat_from_main(&self, worktree_path: &Path) -> String {
        Command::new("jj")
            .current_dir(worktree_path)
            .args(["diff", "--stat", "-r", "trunk()..@"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn add_all(&self, worktree_path: &Path) -> Result<()> {
        // jj automatically snapshots the working copy; running `jj st` triggers it
        Command::new("jj")
            .current_dir(worktree_path)
            .args(["st"])
            .output()
            .context("Failed to snapshot jj working copy")?;
        Ok(())
    }

    fn has_changes(&self, worktree_path: &Path) -> bool {
        Command::new("jj")
            .current_dir(worktree_path)
            .args(["diff", "--summary"])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    }

    fn commit(&self, worktree_path: &Path, message: &str) -> Result<()> {
        let output = Command::new("jj")
            .current_dir(worktree_path)
            .args(["commit", "-m", message])
            .output()
            .context("Failed to commit in jj")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("jj commit failed: {}", stderr);
        }
        Ok(())
    }

    fn push(&self, worktree_path: &Path, branch: &str, _set_upstream: bool) -> Result<()> {
        // Create the bookmark at @- (the just-committed revision) before pushing
        let bookmark_output = Command::new("jj")
            .current_dir(worktree_path)
            .args(["bookmark", "create", branch, "-r", "@-"])
            .output()
            .context("Failed to create jj bookmark")?;

        if !bookmark_output.status.success() {
            // Bookmark may already exist — try moving it instead
            let move_output = Command::new("jj")
                .current_dir(worktree_path)
                .args(["bookmark", "move", branch, "--to", "@-"])
                .output()
                .context("Failed to move jj bookmark")?;

            if !move_output.status.success() {
                let stderr = String::from_utf8_lossy(&move_output.stderr);
                anyhow::bail!("Failed to set jj bookmark '{}': {}", branch, stderr);
            }
        }

        let push_output = Command::new("jj")
            .current_dir(worktree_path)
            .args(["git", "push", "--bookmark", branch])
            .output()
            .context("Failed to push jj bookmark")?;

        if !push_output.status.success() {
            let stderr = String::from_utf8_lossy(&push_output.stderr);
            anyhow::bail!("jj git push failed: {}", stderr);
        }
        Ok(())
    }

    fn fetch_and_check_conflicts(&self, worktree_path: &Path) -> Result<bool> {
        // Fetch latest from remote
        let fetch = Command::new("jj")
            .current_dir(worktree_path)
            .args(["git", "fetch"])
            .output()
            .context("Failed to run jj git fetch")?;

        if !fetch.status.success() {
            let stderr = String::from_utf8_lossy(&fetch.stderr);
            anyhow::bail!("jj git fetch failed: {}", stderr);
        }

        // Attempt rebase onto trunk() to detect conflicts
        let rebase = Command::new("jj")
            .current_dir(worktree_path)
            .args(["rebase", "-d", "trunk()"])
            .output()
            .context("Failed to run jj rebase")?;

        if !rebase.status.success() {
            // Rebase command itself failed — undo and report as conflicted
            let _ = Command::new("jj")
                .current_dir(worktree_path)
                .args(["undo"])
                .output();
            return Ok(true);
        }

        // Check if any commits now have unresolved conflicts
        let conflict_check = Command::new("jj")
            .current_dir(worktree_path)
            .args(["log", "-r", "conflicts()", "--no-graph", "-T", "commit_id"])
            .output()
            .context("Failed to check for jj conflicts")?;

        let has_conflicts =
            !String::from_utf8_lossy(&conflict_check.stdout).trim().is_empty();

        if !has_conflicts {
            // Clean rebase — undo it to restore original state (agent hasn't asked for a rebase)
            let _ = Command::new("jj")
                .current_dir(worktree_path)
                .args(["undo"])
                .output();
        }
        // If conflicts exist, leave the rebased state so the agent can resolve them

        Ok(has_conflicts)
    }

    fn list_files(&self, project_path: &Path) -> Vec<String> {
        Command::new("jj")
            .current_dir(project_path)
            .args(["file", "list"])
            .output()
            .map(|o| {
                String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default()
    }

    fn initialize_worktree(
        &self,
        project_path: &Path,
        worktree_path: &Path,
        copy_files: Option<String>,
        init_script: Option<String>,
        copy_dirs: Vec<String>,
    ) -> Vec<String> {
        super::initialize_worktree(
            project_path,
            worktree_path,
            copy_files.as_deref(),
            init_script.as_deref(),
            &copy_dirs,
        )
    }
}
