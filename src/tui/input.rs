/// Input mode for the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode - navigating the board
    Normal,
    /// Entering task title
    InputTitle,
    /// Entering task description/prompt
    InputDescription,
}

impl Default for InputMode {
    fn default() -> Self {
        Self::Normal
    }
}
