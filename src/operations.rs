//! Traits for external operations (tmux, git) to enable testing with mocks.

use anyhow::Result;
use std::path::Path;

#[cfg(any(test, feature = "test-mocks"))]
use mockall::automock;

/// Operations for tmux window management
#[cfg_attr(any(test, feature = "test-mocks"), automock)]
pub trait TmuxOperations {
    /// Create a new tmux window in a session
    fn create_window(&self, session: &str, window_name: &str, working_dir: &str) -> Result<()>;

    /// Kill a tmux window
    fn kill_window(&self, target: &str) -> Result<()>;

    /// Check if a window exists
    fn window_exists(&self, target: &str) -> Result<bool>;

    /// Send keys to a window
    fn send_keys(&self, target: &str, keys: &str) -> Result<()>;

    /// Capture pane content
    fn capture_pane(&self, target: &str) -> Result<String>;

    /// Resize a tmux window
    fn resize_window(&self, target: &str, width: u16, height: u16) -> Result<()>;
}

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

/// Real implementation using actual tmux commands
pub struct RealTmuxOps;

impl TmuxOperations for RealTmuxOps {
    fn create_window(&self, session: &str, window_name: &str, working_dir: &str) -> Result<()> {
        let output = std::process::Command::new("tmux")
            .args(["-L", crate::tmux::AGENT_SERVER])
            .args(["new-window", "-d", "-t", session, "-n", window_name])
            .args(["-c", working_dir])
            .output()?;

        if !output.status.success() {
            anyhow::bail!("Failed to create tmux window");
        }
        Ok(())
    }

    fn kill_window(&self, target: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", crate::tmux::AGENT_SERVER])
            .args(["kill-window", "-t", target])
            .output()?;
        Ok(())
    }

    fn window_exists(&self, target: &str) -> Result<bool> {
        let output = std::process::Command::new("tmux")
            .args(["-L", crate::tmux::AGENT_SERVER])
            .args(["list-windows", "-t", target])
            .output()?;
        Ok(output.status.success())
    }

    fn send_keys(&self, target: &str, keys: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", crate::tmux::AGENT_SERVER])
            .args(["send-keys", "-t", target, keys, "Enter"])
            .output()?;
        Ok(())
    }

    fn capture_pane(&self, target: &str) -> Result<String> {
        let output = std::process::Command::new("tmux")
            .args(["-L", crate::tmux::AGENT_SERVER])
            .args(["capture-pane", "-t", target, "-p"])
            .output()?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn resize_window(&self, target: &str, width: u16, height: u16) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", crate::tmux::AGENT_SERVER])
            .args(["resize-window", "-t", target])
            .args(["-x", &width.to_string()])
            .args(["-y", &height.to_string()])
            .output()?;
        Ok(())
    }
}

/// Real implementation using actual git commands
pub struct RealGitOps;

impl GitOperations for RealGitOps {
    fn create_worktree(&self, project_path: &Path, task_slug: &str) -> Result<String> {
        let path = crate::git::create_worktree(project_path, task_slug)?;
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
        crate::git::worktree_exists(project_path, task_slug)
    }
}
