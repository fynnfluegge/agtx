pub mod control;
pub mod input;
mod operations;

pub use control::{
    is_window_close, output_pane_id, tmux_quote, ControlClient, Frame, FrameParser, OutputWatch,
};
pub use input::{
    InputConfig, InputError, PaneInput, PaneInputSink, DEFAULT_BATCH_WINDOW, DEFAULT_QUEUE_CAPACITY,
};
pub use operations::*;

#[cfg(any(test, feature = "test-mocks"))]
pub use input::RecordingSink;

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

/// The last `n` lines of a pane capture, after dropping the blank rows below
/// the last line of output.
///
/// `capture-pane -p` emits one line per pane *row*, so the raw end of a capture
/// is padding whenever the output has not filled the pane. Trailing blank rows
/// go first, so `n` counts lines of output rather than the empty rows under a
/// short one.
///
/// Nor is `capture-pane -S -N` a tail: it starts N lines back in the scrollback
/// and runs to the bottom of the *visible* screen. An agent that draws
/// full-screen keeps no tmux scrollback, so the capture is the whole screen
/// whatever N is — measured, a request for 15 lines returns 49.
pub fn pane_tail(content: &str, n: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
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

/// Sanitize a project name for use as a tmux session name.
/// Replaces any character that is not alphanumeric, `-`, or `_` with `-`,
/// collapses consecutive replacements, and trims leading/trailing dashes.
/// Returns `"project"` if the result would be empty.
pub fn safe_session_name(name: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
            slug.push(c);
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "project".to_string()
    } else {
        slug
    }
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

#[cfg(test)]
mod tests {
    use super::pane_tail;

    #[test]
    fn a_pane_read_returns_only_the_lines_asked_for() {
        let screen: String = (1..=49).map(|i| format!("line {i}\n")).collect();
        let tail = pane_tail(&screen, 15);
        assert_eq!(tail.lines().count(), 15);
        assert!(tail.starts_with("line 35"));
        assert!(tail.ends_with("line 49"));
    }

    #[test]
    fn blank_rows_below_the_output_do_not_count() {
        let screen = "prompt\n❯ working\n\n   \n\n";
        assert_eq!(pane_tail(screen, 1), "❯ working");
    }

    #[test]
    fn asking_for_more_than_there_is_returns_all_of_it() {
        assert_eq!(pane_tail("a\nb\n", 50), "a\nb");
        assert_eq!(pane_tail("\n\n", 5), "");
    }
}
