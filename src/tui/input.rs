/// Input mode for the TUI
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    /// Normal mode - navigating the board
    Normal,
    /// Entering task title
    InputTitle,
    /// Selecting workflow plugin for the task
    SelectPlugin,
    /// Entering task description/prompt
    InputDescription,
    /// Editing a project's worktree init script
    InputInitScript,
}

impl Default for InputMode {
    fn default() -> Self {
        Self::Normal
    }
}
