use ratatui::prelude::*;
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use std::time::{Duration, Instant};

/// State for the shell popup that shows a detached tmux window
#[derive(Debug, Clone)]
pub struct ShellPopup {
    pub task_title: String,
    pub window_name: String,
    pub scroll_offset: i32, // Negative means scroll up (see more history)
    /// Cached pane content - updated periodically, not on every frame
    pub cached_content: Vec<u8>,
    /// [`cached_content`](Self::cached_content) already parsed into styled lines.
    ///
    /// Parsed once, on the watcher thread, rather than on every frame in `draw()`.
    /// Kept beside the bytes because the bytes are what change detection
    /// compares, and comparing them is far cheaper than parsing them.
    pub cached_lines: Vec<Line<'static>>,
    /// Last known pane dimensions for resize detection
    pub last_pane_size: Option<(u16, u16)>,
    /// Escalation note from the orchestrator, shown as a banner
    pub escalation_note: Option<String>,
    /// Task ID (used to clear escalation note on dismiss)
    pub task_id: Option<String>,
    /// Whether the popup fills the agtx terminal instead of using a centered window.
    pub fullscreen: bool,
    /// Pane capture is deliberately throttled so typing never performs two
    /// expensive tmux captures for every character.
    pub last_content_refresh: Instant,
    /// Cursor position inside the visible tmux pane and its pane height.
    pub metrics: Option<crate::tmux::PaneMetrics>,
    /// Index into [`cached_lines`](Self::cached_lines) of the row the tmux
    /// cursor sits on.
    ///
    /// Carried rather than recomputed at draw time, because
    /// [`trim_content_to_cursor`] drops trailing rows: the last cached line is
    /// then no longer the pane's last row, and `total_lines - pane_height`
    /// under-counts by however many were dropped. That error is invisible for a
    /// pane with no scrollback — the subtraction saturates at 0 — which is why
    /// it only ever showed up under agents that stay on the normal screen.
    pub cursor_line: Option<usize>,
}

impl ShellPopup {
    pub fn new(task_title: String, window_name: String) -> Self {
        Self {
            task_title,
            window_name,
            scroll_offset: 0,
            cached_content: Vec::new(),
            cached_lines: Vec::new(),
            last_pane_size: None,
            escalation_note: None,
            task_id: None,
            fullscreen: false,
            last_content_refresh: Instant::now(),
            metrics: None,
            cursor_line: None,
        }
    }

    /// Replace the cached pane, bytes and parsed lines together.
    ///
    /// One setter so the two cannot drift: rendering from lines that do not match
    /// the bytes change detection compares would show a frame that is never
    /// corrected, because the next capture would compare equal.
    pub fn set_content(
        &mut self,
        content: Vec<u8>,
        lines: Vec<Line<'static>>,
        cursor_line: Option<usize>,
    ) {
        self.cached_content = content;
        self.cached_lines = lines;
        self.cursor_line = cursor_line;
    }

    /// Scroll up into history, clamped to content bounds.
    pub fn scroll_up(&mut self, lines: i32) {
        // Derive the total line count from text lines rather than raw '\n' bytes,
        // so that a final line without a trailing newline is still counted.
        let content_str = String::from_utf8_lossy(&self.cached_content);
        let total_lines = content_str.lines().count() as i32;
        let min_offset = -(total_lines.max(0));
        self.scroll_offset = (self.scroll_offset - lines).max(min_offset);
    }

    /// Scroll down toward current content, clamped to 0.
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

    /// Does tmux hold any history above this pane's visible screen?
    ///
    /// `false` means agtx has nothing to scroll: the agent is drawing in the
    /// alternate screen and owns the session's scrollback itself. The popup's
    /// scroll keys are then handed to the agent instead — scrolling agtx's
    /// one-screen buffer would move nothing while looking like it had.
    ///
    /// Unknown metrics count as *has* scrollback, so a failed `display -p`
    /// leaves the existing behaviour alone rather than silently rerouting keys.
    pub fn has_scrollback(&self) -> bool {
        self.metrics.map(|m| m.history_size > 0).unwrap_or(true)
    }

    pub fn content_refresh_due(&self, interval: Duration) -> bool {
        self.last_content_refresh.elapsed() >= interval
    }

    pub fn mark_content_refreshed(&mut self) {
        self.last_content_refresh = Instant::now();
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
/// Takes a **slice**, not a `Vec`, and clones only the rows it returns.
///
/// The caller's lines are cached and reused across frames, so taking them by
/// value meant cloning every cached row — 100 of them, or 500 once scrolled — to
/// render the ~30 that fit. Each `Span` owns its text, so those clones are
/// allocations, on every frame a streaming pane produces.
pub fn compute_visible_lines<'a>(
    styled_lines: &[Line<'a>],
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
        .iter()
        .take(effective_line_count)
        .skip(start_line)
        .take(visible_height)
        .cloned()
        .collect();

    (visible_lines, start_line, total_lines)
}

/// Build the footer text for the shell popup
pub fn build_footer_text(scroll_offset: i32, start_line: usize) -> String {
    build_footer_text_for_mode(scroll_offset, start_line, false, true)
}

fn build_footer_text_for_mode(
    scroll_offset: i32,
    start_line: usize,
    fullscreen: bool,
    has_scrollback: bool,
) -> String {
    let view_action = if fullscreen { "windowed" } else { "fullscreen" };
    // `C-n/p` and `C-d/u` are both bound and both stay bound; the footer has
    // room to name one pair, and `C-d/u` is the one advertised. Do not "fix"
    // the mismatch by unbinding the other — see `handle_shell_popup_key`.
    //
    // With no tmux scrollback the scroll keys go to the agent, so advertising a
    // line number agtx cannot move to is worse than saying nothing: that is
    // exactly what made an empty buffer look like a broken scrollbar.
    if !has_scrollback {
        return format!("[C-d/u] scroll  [C-g] bottom  ·  [C-f] {view_action}  [C-q] close");
    }
    if scroll_offset < 0 {
        format!(
            "Line {}  ·  [C-d/u] scroll  [C-g] bottom  ·  [C-f] {}  [C-q] close",
            start_line + 1,
            view_action
        )
    } else {
        format!(
            "At bottom  ·  [C-d/u] scroll  ·  [C-f] {}  [C-q] close",
            view_action
        )
    }
}

/// Render popup shortcuts with the same key-first hierarchy as the board
/// footer: keys carry the accent while labels and separators stay subdued.
fn styled_footer(text: &str, colors: &ShellPopupColors) -> Line<'static> {
    let key_style = Style::default().fg(colors.border).bold();
    let label_style = Style::default().fg(colors.footer_bg);
    let mut spans = Vec::new();
    let mut rest = text;

    while let Some(open) = rest.find('[') {
        if open > 0 {
            spans.push(Span::styled(rest[..open].to_string(), label_style));
        }
        let Some(close) = rest[open..].find(']') else {
            spans.push(Span::styled(rest[open..].to_string(), label_style));
            rest = "";
            break;
        };
        let end = open + close + 1;
        spans.push(Span::styled(rest[open..end].to_string(), key_style));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), label_style));
    }

    Line::from(spans)
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
/// Trimmed content with empty buffer space removed, and the index of the line
/// the cursor is on — which the caller must keep, because trimming is what
/// makes it underivable afterwards.
pub fn trim_content_to_cursor(
    content: Vec<u8>,
    cursor_info: Option<(usize, usize)>,
) -> (Vec<u8>, Option<usize>) {
    let content_str = String::from_utf8_lossy(&content);
    let lines: Vec<&str> = content_str.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return (content, None);
    }

    // Where the cursor is, in the *untrimmed* capture. Trimming only ever drops
    // lines from the end, so this stays valid for the trimmed content too.
    let cursor_line = cursor_info.and_then(|(cursor_y, pane_height)| {
        (pane_height > 0).then(|| total_lines.saturating_sub(pane_height) + cursor_y)
    });

    // First pass: use cursor position if available
    let end_line_from_cursor = if let Some(cursor_line_in_capture) = cursor_line {
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
    };

    // Second pass: also trim excessive trailing empty lines
    // This handles cases where cursor is at bottom but there's no real content there
    let lines_after_cursor_trim = &lines[..end_line_from_cursor];
    let end_line = trim_trailing_empty_lines(lines_after_cursor_trim);

    let trimmed: String = lines[..end_line].join("\n");
    (trimmed.into_bytes(), cursor_line)
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
    let last_content_line = lines.iter().rposition(|line| !line.trim().is_empty());

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
    pub escalation_fg: Color,
    pub escalation_bg: Color,
}

impl Default for ShellPopupColors {
    fn default() -> Self {
        Self {
            border: Color::Green,
            header_fg: Color::Black,
            header_bg: Color::Cyan,
            footer_fg: Color::Black,
            footer_bg: Color::Gray,
            escalation_fg: Color::Black,
            escalation_bg: Color::Yellow,
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
    styled_lines: &[Line<'_>],
    colors: &ShellPopupColors,
) {
    frame.render_widget(Clear, popup_area);

    // Draw border around the popup
    let border_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(colors.border))
        .border_type(ratatui::widgets::BorderType::Rounded);
    let inner_area = border_block.inner(popup_area);
    frame.render_widget(border_block, popup_area);

    // Layout: header, optional escalation banner, content, footer (inside the border)
    let has_escalation = popup.escalation_note.is_some();
    let escalation_height = if has_escalation { 2u16 } else { 0u16 };

    let popup_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                 // Title bar
            Constraint::Length(escalation_height), // Escalation banner (0 if none)
            Constraint::Min(0),                    // Shell content
            Constraint::Length(1),                 // Footer
        ])
        .split(inner_area);

    // Title bar (pad to fill width)
    let title = format!("  {} ", popup.task_title);
    let padded_title = format!("{:<width$}", title, width = popup_chunks[0].width as usize);
    let title_bar =
        Paragraph::new(padded_title).style(Style::default().fg(colors.header_bg).bold());
    frame.render_widget(title_bar, popup_chunks[0]);

    // Escalation banner (if present)
    if let Some(ref note) = popup.escalation_note {
        let banner_text = format!(" \u{26a0}  {} ", note);
        let padded_banner = format!(
            "{:<width$}",
            banner_text,
            width = popup_chunks[1].width as usize
        );
        let hint = format!(
            "{:<width$}",
            " Press any key to dismiss",
            width = popup_chunks[1].width as usize
        );
        let banner_content = format!("{}\n{}", padded_banner, hint);
        let banner = Paragraph::new(banner_content).style(
            Style::default()
                .fg(colors.escalation_fg)
                .bg(colors.escalation_bg),
        );
        frame.render_widget(banner, popup_chunks[1]);
    }

    // Shell content
    let visible_height = popup_chunks[2].height as usize;

    // Use the testable helper to compute visible lines
    let (visible_lines, start_line, _total_lines) =
        compute_visible_lines(styled_lines, visible_height, popup.scroll_offset);

    let content = Paragraph::new(visible_lines);
    frame.render_widget(content, popup_chunks[2]);

    // A captured pane is text-only; explicitly restore tmux's live cursor when
    // it falls inside the current (non-scrolled) viewport.
    //
    // The row comes from `popup.cursor_line`, fixed when the capture was
    // trimmed, and *not* from `total_lines - pane_height`: trimming drops
    // trailing rows, so the last cached line is not the pane's last row.
    if popup.scroll_offset >= 0 {
        if let (Some(cursor_line), Some(cursor_x)) =
            (popup.cursor_line, popup.metrics.map(|m| m.cursor_x))
        {
            if cursor_line >= start_line && cursor_line < start_line + visible_height {
                let x = popup_chunks[2].x.saturating_add(
                    cursor_x.min(popup_chunks[2].width.saturating_sub(1) as usize) as u16,
                );
                let y = popup_chunks[2]
                    .y
                    .saturating_add((cursor_line - start_line) as u16);
                frame.set_cursor_position((x, y));
            }
        }
    }

    // Footer with scroll indicator and grouped, accented shortcuts.
    let footer_text = build_footer_text_for_mode(
        popup.scroll_offset,
        start_line,
        popup.fullscreen,
        popup.has_scrollback(),
    );
    let footer = Paragraph::new(styled_footer(&footer_text, colors)).alignment(Alignment::Center);
    frame.render_widget(footer, popup_chunks[3]);
}
