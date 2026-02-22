use crate::tmux::TmuxOperations;

pub const SPINNER_FRAMES: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];

/// Live status of a coding agent's tmux session.
/// Distinct from db::models::AgentStatus which is for persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    /// Agent process is actively running (producing output)
    Active,
    /// Agent is idle / waiting for user input
    Idle,
    /// Pane process has exited (shell prompt visible or pane dead)
    Exited,
    /// Unable to determine status (e.g. no tmux window)
    Unknown,
}

impl SessionStatus {
    /// Colored dot character for task card display
    pub fn indicator(&self) -> &'static str {
        match self {
            SessionStatus::Active => "●",
            SessionStatus::Idle => "○",
            SessionStatus::Exited => "✗",
            SessionStatus::Unknown => "?",
        }
    }
}

/// Strip ANSI escape sequences from a string.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            // Skip until we find the terminating letter [A-Za-z]
            for c2 in chars.by_ref() {
                if c2.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Detect the live status of an agent session by inspecting its tmux pane.
pub fn detect_session_status(target: &str, tmux_ops: &dyn TmuxOperations) -> SessionStatus {
    // Check window exists first
    if !tmux_ops.window_exists(target).unwrap_or(false) {
        return SessionStatus::Unknown;
    }

    // Check if pane process is dead
    if !tmux_ops.is_pane_alive(target) {
        return SessionStatus::Exited;
    }

    // Capture pane and analyze
    let content = match tmux_ops.capture_pane(target) {
        Ok(c) => c,
        Err(_) => return SessionStatus::Unknown,
    };

    let clean = strip_ansi(&content);

    // Get last non-empty lines for analysis
    let lines: Vec<&str> = clean.lines().rev().take(20).collect();
    let last_line = lines
        .iter()
        .find(|l| !l.trim().is_empty())
        .copied()
        .unwrap_or("");
    let trimmed = last_line.trim_end();

    // Check for shell prompts (agent has exited, dropped back to shell)
    if trimmed.ends_with('$') || trimmed.ends_with('%') {
        return SessionStatus::Exited;
    }

    // Check for agent idle prompts
    if trimmed.ends_with("> ") || trimmed == ">" {
        return SessionStatus::Idle;
    }
    if trimmed.ends_with("? ") || trimmed == "?" {
        return SessionStatus::Idle;
    }

    // Check for idle phrases in recent output
    let recent: String = lines
        .iter()
        .take(5)
        .copied()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    let idle_phrases = [
        "what would you like",
        "anything else",
        "what do you want",
        "how can i help",
        "is there anything",
        "let me know if",
    ];
    if idle_phrases.iter().any(|phrase| recent.contains(phrase)) {
        return SessionStatus::Idle;
    }

    SessionStatus::Active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_removes_color_codes() {
        assert_eq!(strip_ansi("\x1b[32m>\x1b[0m "), "> ");
    }

    #[test]
    fn test_strip_ansi_preserves_plain_text() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn test_strip_ansi_handles_empty_string() {
        assert_eq!(strip_ansi(""), "");
    }

    #[test]
    fn test_indicator_symbols() {
        assert_eq!(SessionStatus::Active.indicator(), "●");
        assert_eq!(SessionStatus::Idle.indicator(), "○");
        assert_eq!(SessionStatus::Exited.indicator(), "✗");
        assert_eq!(SessionStatus::Unknown.indicator(), "?");
    }
}
