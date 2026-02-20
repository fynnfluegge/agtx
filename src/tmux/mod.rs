mod operations;

pub use operations::*;

#[cfg(feature = "test-mocks")]
pub use operations::MockTmuxOperations;

use anyhow::{Context, Result};
use std::process::Command;

/// The tmux server name for agent sessions
pub const AGENT_SERVER: &str = "agtx";

/// Spawn a new agent session in the agents tmux server
pub fn spawn_session(
    session_name: &str,
    working_dir: &str,
    agent_command: &str,
    args: &[&str],
) -> Result<()> {
    // Build the full shell command to run in the session
    // We need to properly escape/quote for shell execution
    let mut shell_command = agent_command.to_string();
    for arg in args {
        shell_command.push(' ');
        // Always single-quote arguments to preserve them exactly
        shell_command.push('\'');
        // Escape any single quotes in the argument
        shell_command.push_str(&arg.replace('\'', "'\"'\"'"));
        shell_command.push('\'');
    }

    let output = Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args(["new-session", "-d"])
        .args(["-s", session_name])
        .args(["-c", working_dir])
        .args(["sh", "-c", &shell_command])
        .output()
        .context("Failed to spawn tmux session")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("tmux new-session failed: {}", stderr);
    }

    Ok(())
}

/// List all sessions on the agents server
pub fn list_sessions() -> Result<Vec<SessionInfo>> {
    let output = Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args([
            "list-sessions",
            "-F",
            "#{session_name}\t#{session_activity}\t#{session_created}",
        ])
        .output()
        .context("Failed to list tmux sessions")?;

    if !output.status.success() {
        // No server running or no sessions - that's fine
        return Ok(vec![]);
    }

    let sessions = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let parts: Vec<&str> = line.split('\t').collect();
            if parts.len() >= 3 {
                Some(SessionInfo {
                    name: parts[0].to_string(),
                    last_activity: parts[1].parse().unwrap_or(0),
                    created: parts[2].parse().unwrap_or(0),
                })
            } else {
                None
            }
        })
        .collect();

    Ok(sessions)
}

/// Check if a specific session exists
pub fn session_exists(session_name: &str) -> Result<bool> {
    let output = Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args(["has-session", "-t", session_name])
        .output()
        .context("Failed to check tmux session")?;

    Ok(output.status.success())
}

/// Capture the last N lines of output from a session's pane
pub fn capture_pane(session_name: &str, lines: i32) -> Result<String> {
    let output = Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args(["capture-pane", "-t", session_name, "-p"])
        .args(["-S", &(-lines).to_string()])
        .output()
        .context("Failed to capture tmux pane")?;

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Send keys to a session
pub fn send_keys(session_name: &str, keys: &str) -> Result<()> {
    Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args(["send-keys", "-t", session_name, keys, "Enter"])
        .output()
        .context("Failed to send keys to tmux session")?;

    Ok(())
}

/// Attach directly to an agent session
/// This blocks until the user detaches (Ctrl-b d) or the session ends
/// Works regardless of whether user is inside tmux or not
pub fn attach_session(session_name: &str) -> Result<()> {
    Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args(["attach", "-t", session_name])
        .status()
        .context("Failed to attach to tmux session")?;

    Ok(())
}

/// Kill a session
pub fn kill_session(session_name: &str) -> Result<()> {
    Command::new("tmux")
        .args(["-L", AGENT_SERVER])
        .args(["kill-session", "-t", session_name])
        .output()
        .context("Failed to kill tmux session")?;

    Ok(())
}

/// Information about a tmux session
#[derive(Debug, Clone)]
pub struct SessionInfo {
    pub name: String,
    pub last_activity: u64,
    pub created: u64,
}

impl SessionInfo {
    /// Parse task ID from session name (task-{id}--{project}--{slug})
    pub fn task_id(&self) -> Option<&str> {
        self.name
            .strip_prefix("task-")
            .and_then(|s| s.split("--").next())
    }

    /// Parse project name from session name
    pub fn project_name(&self) -> Option<&str> {
        self.name.split("--").nth(1)
    }
}
