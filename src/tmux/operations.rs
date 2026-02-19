//! Traits for tmux operations to enable testing with mocks.

use anyhow::Result;

#[cfg(any(test, feature = "test-mocks"))]
use mockall::automock;

/// Operations for tmux window management
#[cfg_attr(any(test, feature = "test-mocks"), automock)]
pub trait TmuxOperations: Send + Sync {
    /// Create a new tmux window in a session with an optional command to run
    fn create_window(
        &self,
        session: &str,
        window_name: &str,
        working_dir: &str,
        command: Option<String>,
    ) -> Result<()>;

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

/// Real implementation using actual tmux commands
pub struct RealTmuxOps;

impl TmuxOperations for RealTmuxOps {
    fn create_window(
        &self,
        session: &str,
        window_name: &str,
        working_dir: &str,
        command: Option<String>,
    ) -> Result<()> {
        let mut cmd = std::process::Command::new("tmux");
        cmd.args(["-L", super::AGENT_SERVER])
            .args(["new-window", "-d", "-t", session, "-n", window_name])
            .args(["-c", working_dir]);

        if let Some(ref shell_cmd) = command {
            cmd.args(["sh", "-c", shell_cmd]);
        }

        let output = cmd.output()?;

        if !output.status.success() {
            anyhow::bail!("Failed to create tmux window");
        }
        Ok(())
    }

    fn kill_window(&self, target: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["kill-window", "-t", target])
            .output()?;
        Ok(())
    }

    fn window_exists(&self, target: &str) -> Result<bool> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["list-windows", "-t", target])
            .output()?;
        Ok(output.status.success())
    }

    fn send_keys(&self, target: &str, keys: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, keys, "Enter"])
            .output()?;
        Ok(())
    }

    fn capture_pane(&self, target: &str) -> Result<String> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["capture-pane", "-t", target, "-p"])
            .output()?;
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    fn resize_window(&self, target: &str, width: u16, height: u16) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["resize-window", "-t", target])
            .args(["-x", &width.to_string()])
            .args(["-y", &height.to_string()])
            .output()?;
        Ok(())
    }
}
