//! Traits for tmux operations to enable testing with mocks.

use anyhow::Result;

#[cfg(feature = "test-mocks")]
use mockall::automock;

/// Operations for tmux window management
#[cfg_attr(feature = "test-mocks", automock)]
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

    /// Send keys to a window (with Enter at the end)
    fn send_keys(&self, target: &str, keys: &str) -> Result<()>;

    /// Send keys to a window without pressing Enter
    fn send_keys_literal(&self, target: &str, keys: &str) -> Result<()>;

    /// Capture pane content
    fn capture_pane(&self, target: &str) -> Result<String>;

    /// Capture pane content with history (returns raw bytes for ANSI parsing)
    fn capture_pane_with_history(&self, target: &str, history_lines: i32) -> Vec<u8>;

    /// Get cursor position and pane height: (cursor_y, pane_height)
    fn get_cursor_info(&self, target: &str) -> Option<(usize, usize)>;

    /// Resize a tmux window
    fn resize_window(&self, target: &str, width: u16, height: u16) -> Result<()>;

    /// Check if a session exists
    fn has_session(&self, session: &str) -> bool;

    /// Create a new detached session
    fn create_session(&self, session: &str, working_dir: &str) -> Result<()>;
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
        // Send the text first
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, keys])
            .output()?;
        // Send Enter separately (like the original implementation)
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, "Enter"])
            .output()?;
        Ok(())
    }

    fn send_keys_literal(&self, target: &str, keys: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["send-keys", "-t", target, keys])
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

    fn capture_pane_with_history(&self, target: &str, history_lines: i32) -> Vec<u8> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["capture-pane", "-t", target, "-p", "-e", "-J"])
            .args(["-S", &format!("-{}", history_lines)])
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default()
    }

    fn get_cursor_info(&self, target: &str) -> Option<(usize, usize)> {
        let output = std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["display", "-p", "-t", target, "#{cursor_y} #{pane_height}"])
            .output()
            .ok()?;

        if output.status.success() {
            let output_str = String::from_utf8_lossy(&output.stdout);
            let parts: Vec<&str> = output_str.trim().split_whitespace().collect();
            if parts.len() == 2 {
                let cursor_y: usize = parts[0].parse().ok()?;
                let pane_height: usize = parts[1].parse().ok()?;
                return Some((cursor_y, pane_height));
            }
        }
        None
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

    fn has_session(&self, session: &str) -> bool {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["has-session", "-t", session])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    fn create_session(&self, session: &str, working_dir: &str) -> Result<()> {
        std::process::Command::new("tmux")
            .args(["-L", super::AGENT_SERVER])
            .args(["new-session", "-d", "-s", session])
            .args(["-c", working_dir])
            .output()?;
        Ok(())
    }
}
