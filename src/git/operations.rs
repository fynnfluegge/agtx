//! Traits for git operations to enable testing with mocks.

use anyhow::Result;
use std::path::Path;

#[cfg(feature = "test-mocks")]
use mockall::automock;

/// Operations for git worktree management
#[cfg_attr(feature = "test-mocks", automock)]
pub trait GitOperations: Send + Sync {
    /// Create a worktree for a task
    fn create_worktree(&self, project_path: &Path, task_slug: &str) -> Result<String>;

    /// Remove a worktree
    fn remove_worktree(&self, project_path: &Path, worktree_path: &str) -> Result<()>;

    /// Check if worktree exists
    fn worktree_exists(&self, project_path: &Path, task_slug: &str) -> bool;

    /// Delete a branch
    fn delete_branch(&self, project_path: &Path, branch_name: &str) -> Result<()>;

    /// Get unstaged diff
    fn diff(&self, worktree_path: &Path) -> String;

    /// Get staged diff
    fn diff_cached(&self, worktree_path: &Path) -> String;

    /// List untracked files
    fn list_untracked_files(&self, worktree_path: &Path) -> String;

    /// Get diff for untracked file (comparing to /dev/null)
    fn diff_untracked_file(&self, worktree_path: &Path, file: &str) -> String;

    /// Get diff stats from main branch
    fn diff_stat_from_main(&self, worktree_path: &Path) -> String;

    /// Stage all changes
    fn add_all(&self, worktree_path: &Path) -> Result<()>;

    /// Check if there are uncommitted changes (returns true if there are changes)
    fn has_changes(&self, worktree_path: &Path) -> bool;

    /// Commit with message
    fn commit(&self, worktree_path: &Path, message: &str) -> Result<()>;

    /// Push branch to origin
    fn push(&self, worktree_path: &Path, branch: &str, set_upstream: bool) -> Result<()>;

    /// List all files (tracked + untracked, respects .gitignore)
    fn list_files(&self, project_path: &Path) -> Vec<String>;

    /// Initialize a worktree by copying files and running init script
    /// Returns a list of warning messages for any issues encountered
    fn initialize_worktree(
        &self,
        project_path: &Path,
        worktree_path: &Path,
        copy_files: Option<String>,
        init_script: Option<String>,
    ) -> Vec<String>;
}

/// Real implementation using actual git commands
pub struct RealGitOps;

impl GitOperations for RealGitOps {
    fn create_worktree(&self, project_path: &Path, task_slug: &str) -> Result<String> {
        let path = super::create_worktree(project_path, task_slug)?;
        Ok(path.to_string_lossy().to_string())
    }

    fn remove_worktree(&self, project_path: &Path, worktree_path: &str) -> Result<()> {
        std::process::Command::new("git")
            .current_dir(project_path)
            .args(["worktree", "remove", "--force", worktree_path])
            .output()?;
        Ok(())
    }

    fn worktree_exists(&self, project_path: &Path, task_slug: &str) -> bool {
        super::worktree_exists(project_path, task_slug)
    }

    fn delete_branch(&self, project_path: &Path, branch_name: &str) -> Result<()> {
        std::process::Command::new("git")
            .current_dir(project_path)
            .args(["branch", "-D", branch_name])
            .output()?;
        Ok(())
    }

    fn diff(&self, worktree_path: &Path) -> String {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["diff"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn diff_cached(&self, worktree_path: &Path) -> String {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["diff", "--cached"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn list_untracked_files(&self, worktree_path: &Path) -> String {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["ls-files", "--others", "--exclude-standard"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn diff_untracked_file(&self, worktree_path: &Path, file: &str) -> String {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["diff", "--no-index", "/dev/null", file])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn diff_stat_from_main(&self, worktree_path: &Path) -> String {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["diff", "main", "--stat"])
            .output()
            .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
            .unwrap_or_default()
    }

    fn add_all(&self, worktree_path: &Path) -> Result<()> {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["add", "-A"])
            .output()?;
        Ok(())
    }

    fn has_changes(&self, worktree_path: &Path) -> bool {
        std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["status", "--porcelain"])
            .output()
            .map(|o| !o.stdout.is_empty())
            .unwrap_or(false)
    }

    fn commit(&self, worktree_path: &Path, message: &str) -> Result<()> {
        let output = std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(["commit", "-m", message])
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Only fail if it's not "nothing to commit"
            if !stderr.contains("nothing to commit") {
                anyhow::bail!("Failed to commit changes: {}", stderr);
            }
        }
        Ok(())
    }

    fn push(&self, worktree_path: &Path, branch: &str, set_upstream: bool) -> Result<()> {
        let mut args = vec!["push"];
        if set_upstream {
            args.push("-u");
        }
        args.push("origin");
        args.push(branch);

        let output = std::process::Command::new("git")
            .current_dir(worktree_path)
            .args(&args)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("Failed to push branch: {}", stderr);
        }
        Ok(())
    }

    fn list_files(&self, project_path: &Path) -> Vec<String> {
        std::process::Command::new("git")
            .current_dir(project_path)
            .args(["ls-files", "--cached", "--others", "--exclude-standard"])
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
    ) -> Vec<String> {
        super::initialize_worktree(project_path, worktree_path, copy_files.as_deref(), init_script.as_deref())
    }
}
