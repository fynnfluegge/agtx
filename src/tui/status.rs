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
