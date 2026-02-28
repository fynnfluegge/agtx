use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};

/// State for the shell popup that shows a detached tmux window
#[derive(Debug, Clone)]
pub struct ShellPopup {
    pub task_title: String,
    pub window_name: String,
    pub scroll_offset: i32, // Negative means scroll up (see more history)
    /// Cached pane content - updated periodically, not on every frame
    pub cached_content: Vec<u8>,
    /// Last known pane dimensions for resize detection
    pub last_pane_size: Option<(u16, u16)>,
}

impl ShellPopup {
    pub fn new(task_title: String, window_name: String) -> Self {
        Self {
            task_title,
            window_name,
            scroll_offset: 0,
            cached_content: Vec::new(),
            last_pane_size: None,
        }
    }

    /// Scroll up into history
    pub fn scroll_up(&mut self, lines: i32) {
        self.scroll_offset -= lines;
    }

    /// Scroll down toward current content
    pub fn scroll_down(&mut self, lines: i32) {
        self.scroll_offset = (self.scroll_offset + lines).min(0);
    }

    /// Jump to bottom (current content)
    pub fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    /// Check if we're at the bottom
    pub fn is_at_bottom(&self) -> bool {
        self.scroll_offset >= 0
    }
}

/// Computed view data for rendering - separates computation from rendering
#[derive(Debug)]
pub struct ShellPopupView<'a> {
    pub title: String,
    pub lines: Vec<Line<'a>>,
    pub start_line: usize,
    pub total_lines: usize,
    pub is_at_bottom: bool,
}

/// Compute the visible lines for the shell popup
/// This is the core testable logic, separated from rendering
pub fn compute_visible_lines<'a>(
    styled_lines: Vec<Line<'a>>,
    visible_height: usize,
    scroll_offset: i32,
) -> (Vec<Line<'a>>, usize, usize) {
    let total_input_lines = styled_lines.len();

    // When at bottom (scroll_offset >= 0), show all lines including trailing empty ones
    // so the user can see where the cursor/prompt is.
    // When scrolled up, trim trailing empty lines for cleaner history view.
    let effective_line_count = if scroll_offset >= 0 {
        // At bottom - keep all lines to show cursor position
        total_input_lines
    } else {
        // Scrolled up - trim trailing empty lines for cleaner view
        styled_lines
            .iter()
            .rposition(|line| {
                !line.spans.is_empty() && !line.spans.iter().all(|s| s.content.trim().is_empty())
            })
            .map(|i| i + 1)
            .unwrap_or(total_input_lines)
    };

    let total_lines = effective_line_count.max(1);

    // Apply scroll offset
    let start_line = if scroll_offset < 0 {
        // Scrolling up into history
        total_lines
            .saturating_sub(visible_height)
            .saturating_sub((-scroll_offset) as usize)
    } else {
        // At bottom (current)
        total_lines.saturating_sub(visible_height)
    };

    let visible_lines: Vec<Line<'a>> = styled_lines
        .into_iter()
        .take(effective_line_count)
        .skip(start_line)
        .take(visible_height)
        .collect();

    (visible_lines, start_line, total_lines)
}

/// Build the footer text for the shell popup
pub fn build_footer_text(scroll_offset: i32, start_line: usize) -> String {
    if scroll_offset < 0 {
        format!(
            " [Ctrl+j/k] scroll [Ctrl+d/u] page [Ctrl+g] bottom [Ctrl+q] close | Line {} ",
            start_line + 1
        )
    } else {
        " [Ctrl+j/k] scroll [Ctrl+d/u] page [Ctrl+q] close | At bottom ".to_string()
    }
}

/// Maximum number of trailing empty lines to keep after content
pub const MAX_TRAILING_EMPTY_LINES: usize = 3;

/// Trim captured content to only include lines up to the cursor position.
/// This removes unused pane buffer space below the cursor.
///
/// # Arguments
/// * `content` - Raw captured pane content as bytes
/// * `cursor_info` - Optional (cursor_y, pane_height) from tmux
///
/// # Returns
/// Trimmed content with empty buffer space removed
pub fn trim_content_to_cursor(content: Vec<u8>, cursor_info: Option<(usize, usize)>) -> Vec<u8> {
    let content_str = String::from_utf8_lossy(&content);
    let lines: Vec<&str> = content_str.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return content;
    }

    // First pass: use cursor position if available
    let end_line_from_cursor = if let Some((cursor_y, pane_height)) = cursor_info {
        if pane_height > 0 {
            // The captured content ends at the bottom of the visible pane
            // visible_pane_start = where the visible pane begins in our capture
            // cursor position in capture = visible_pane_start + cursor_y
            let visible_pane_start = total_lines.saturating_sub(pane_height);
            let cursor_line_in_capture = visible_pane_start + cursor_y;
            let trim_at = (cursor_line_in_capture + 1).min(total_lines);

            // Only trim at cursor if everything below it is blank.
            // TUI apps (OpenCode, Gemini) place the cursor mid-screen with
            // real content below — trimming there would cut the UI in half.
            let has_content_below = lines[trim_at..].iter().any(|l| !l.trim().is_empty());
            if has_content_below {
                total_lines
            } else {
                trim_at
            }
        } else {
            total_lines
        }
    } else {
        total_lines
    };

    // Second pass: also trim excessive trailing empty lines
    // This handles cases where cursor is at bottom but there's no real content there
    let lines_after_cursor_trim = &lines[..end_line_from_cursor];
    let end_line = trim_trailing_empty_lines(lines_after_cursor_trim);

    let trimmed: String = lines[..end_line].join("\n");
    trimmed.into_bytes()
}

/// Trim excessive trailing empty lines, keeping a small buffer for the prompt area.
///
/// # Arguments
/// * `lines` - Slice of line strings to process
///
/// # Returns
/// The number of lines to keep (index to slice up to)
pub fn trim_trailing_empty_lines(lines: &[&str]) -> usize {
    if lines.is_empty() {
        return 0;
    }

    // Find the last non-empty line
    let last_content_line = lines
        .iter()
        .rposition(|line| !line.trim().is_empty());

    match last_content_line {
        Some(idx) => {
            // Keep the content plus a small buffer for prompt area
            (idx + 1 + MAX_TRAILING_EMPTY_LINES).min(lines.len())
        }
        None => {
            // All lines are empty, keep just a few
            MAX_TRAILING_EMPTY_LINES.min(lines.len())
        }
    }
}

/// Colors used for rendering the shell popup
#[derive(Debug, Clone)]
pub struct ShellPopupColors {
    pub border: Color,
    pub header_fg: Color,
    pub header_bg: Color,
    pub footer_fg: Color,
    pub footer_bg: Color,
}

impl Default for ShellPopupColors {
    fn default() -> Self {
        Self {
            border: Color::Green,
            header_fg: Color::Black,
            header_bg: Color::Cyan,
            footer_fg: Color::Black,
            footer_bg: Color::Gray,
        }
    }
}

/// Render the shell popup to the frame
///
/// This function handles the complete rendering of the shell popup:
/// - Border with title
/// - Header bar with task title
/// - Content area with parsed terminal output
/// - Footer with scroll status and keybindings
pub fn render_shell_popup(
    popup: &ShellPopup,
    frame: &mut Frame,
    popup_area: Rect,
    styled_lines: Vec<Line<'_>>,
    colors: &ShellPopupColors,
) {
    frame.render_widget(Clear, popup_area);

    // Draw border around the popup
    let border_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors.border));
    let inner_area = border_block.inner(popup_area);
    frame.render_widget(border_block, popup_area);

    // Layout: header, content, footer (inside the border)
    let popup_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // Title bar
            Constraint::Min(0),    // Shell content
            Constraint::Length(1), // Footer
        ])
        .split(inner_area);

    // Title bar (pad to fill width)
    let title = format!(" {} ", popup.task_title);
    let padded_title = format!("{:<width$}", title, width = popup_chunks[0].width as usize);
    let title_bar = Paragraph::new(padded_title)
        .style(Style::default().fg(colors.header_fg).bg(colors.header_bg));
    frame.render_widget(title_bar, popup_chunks[0]);

    // Shell content
    let visible_height = popup_chunks[1].height as usize;

    // Use the testable helper to compute visible lines
    let (visible_lines, start_line, _total_lines) =
        compute_visible_lines(styled_lines, visible_height, popup.scroll_offset);

    let content = Paragraph::new(visible_lines);
    frame.render_widget(content, popup_chunks[1]);

    // Footer with scroll indicator (pad to fill width)
    let footer_text = build_footer_text(popup.scroll_offset, start_line);
    let padded_footer = format!("{:<width$}", footer_text, width = popup_chunks[2].width as usize);
    let footer = Paragraph::new(padded_footer)
        .style(Style::default().fg(colors.footer_fg).bg(colors.footer_bg));
    frame.render_widget(footer, popup_chunks[2]);
}
