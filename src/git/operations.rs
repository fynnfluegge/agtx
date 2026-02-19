//! Traits for git operations to enable testing with mocks.

use anyhow::Result;
use std::path::Path;

#[cfg(any(test, feature = "test-mocks"))]
use mockall::automock;

/// Operations for git worktree management
#[cfg_attr(any(test, feature = "test-mocks"), automock)]
pub trait GitOperations {
    /// Create a worktree for a task
    fn create_worktree(&self, project_path: &Path, task_slug: &str) -> Result<String>;

    /// Remove a worktree
    fn remove_worktree(&self, project_path: &Path, worktree_path: &str) -> Result<()>;

    /// Check if worktree exists
    fn worktree_exists(&self, project_path: &Path, task_slug: &str) -> bool;
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
}
