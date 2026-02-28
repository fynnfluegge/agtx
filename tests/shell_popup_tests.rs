use agtx::tui::shell_popup::{
    compute_visible_lines, build_footer_text, render_shell_popup,
    trim_content_to_cursor, trim_trailing_empty_lines,
    ShellPopup, ShellPopupColors, MAX_TRAILING_EMPTY_LINES,
};
use ratatui::backend::TestBackend;
use ratatui::prelude::*;
use ratatui::Terminal;

#[test]
fn test_shell_popup_new() {
    let popup = ShellPopup::new("Test Task".to_string(), "test-window".to_string());

    assert_eq!(popup.task_title, "Test Task");
    assert_eq!(popup.window_name, "test-window");
    assert_eq!(popup.scroll_offset, 0);
    assert!(popup.cached_content.is_empty());
    assert!(popup.last_pane_size.is_none());
}

#[test]
fn test_shell_popup_scroll_up() {
    let mut popup = ShellPopup::new("Test".to_string(), "window".to_string());
    assert_eq!(popup.scroll_offset, 0);

    popup.scroll_up(5);
    assert_eq!(popup.scroll_offset, -5);

    popup.scroll_up(10);
    assert_eq!(popup.scroll_offset, -15);
}

#[test]
fn test_shell_popup_scroll_down_clamped() {
    let mut popup = ShellPopup::new("Test".to_string(), "window".to_string());
    popup.scroll_offset = -20;

    popup.scroll_down(5);
    assert_eq!(popup.scroll_offset, -15);

    popup.scroll_down(20);
    assert_eq!(popup.scroll_offset, 0); // Clamped to 0
}

#[test]
fn test_shell_popup_scroll_down_from_zero() {
    let mut popup = ShellPopup::new("Test".to_string(), "window".to_string());
    assert_eq!(popup.scroll_offset, 0);

    // Scrolling down when already at bottom should stay at 0
    popup.scroll_down(10);
    assert_eq!(popup.scroll_offset, 0);
}

#[test]
fn test_shell_popup_scroll_to_bottom() {
    let mut popup = ShellPopup::new("Test".to_string(), "window".to_string());
    popup.scroll_offset = -100;

    popup.scroll_to_bottom();
    assert_eq!(popup.scroll_offset, 0);
}

#[test]
fn test_shell_popup_is_at_bottom() {
    let mut popup = ShellPopup::new("Test".to_string(), "window".to_string());
    assert!(popup.is_at_bottom());

    popup.scroll_up(5);
    assert!(!popup.is_at_bottom());

    popup.scroll_to_bottom();
    assert!(popup.is_at_bottom());
}

#[test]
fn test_compute_visible_lines_empty() {
    let lines: Vec<Line> = vec![];
    let (visible, start, total) = compute_visible_lines(lines, 10, 0);

    assert!(visible.is_empty());
    assert_eq!(start, 0);
    assert_eq!(total, 1); // Minimum 1
}

#[test]
fn test_compute_visible_lines_fewer_than_height() {
    let lines: Vec<Line> = vec![
        Line::from("line 1"),
        Line::from("line 2"),
        Line::from("line 3"),
    ];
    let (visible, start, total) = compute_visible_lines(lines, 10, 0);

    assert_eq!(visible.len(), 3);
    assert_eq!(start, 0);
    assert_eq!(total, 3);
}

#[test]
fn test_compute_visible_lines_exact_height() {
    let lines: Vec<Line> = (0..10).map(|i| Line::from(format!("line {}", i))).collect();

    let (visible, start, total) = compute_visible_lines(lines, 10, 0);

    assert_eq!(visible.len(), 10);
    assert_eq!(start, 0);
    assert_eq!(total, 10);
}

#[test]
fn test_compute_visible_lines_scrolled_up() {
    let lines: Vec<Line> = (0..20).map(|i| Line::from(format!("line {}", i))).collect();

    // Visible height 10, scrolled up by 5 lines
    let (visible, start, total) = compute_visible_lines(lines, 10, -5);

    assert_eq!(visible.len(), 10);
    assert_eq!(start, 5); // 20 - 10 - 5 = 5
    assert_eq!(total, 20);
}

#[test]
fn test_compute_visible_lines_at_bottom() {
    let lines: Vec<Line> = (0..20).map(|i| Line::from(format!("line {}", i))).collect();

    let (visible, start, total) = compute_visible_lines(lines, 10, 0);

    assert_eq!(visible.len(), 10);
    assert_eq!(start, 10); // Shows last 10 lines
    assert_eq!(total, 20);
}

#[test]
fn test_compute_visible_lines_keeps_trailing_empty_at_bottom() {
    let mut lines: Vec<Line> = (0..10).map(|i| Line::from(format!("line {}", i))).collect();
    // Add trailing empty lines (simulating cursor on empty line)
    lines.push(Line::from(""));
    lines.push(Line::from("   "));
    lines.push(Line::from(""));

    // At bottom (scroll_offset = 0) - should keep trailing empty lines
    let (_visible, _start, total) = compute_visible_lines(lines, 20, 0);

    assert_eq!(total, 13); // Keeps all lines including trailing empty
}

#[test]
fn test_compute_visible_lines_strips_trailing_empty_when_scrolled() {
    let mut lines: Vec<Line> = (0..10).map(|i| Line::from(format!("line {}", i))).collect();
    // Add trailing empty lines
    lines.push(Line::from(""));
    lines.push(Line::from("   "));
    lines.push(Line::from(""));

    // Scrolled up (scroll_offset < 0) - should trim trailing empty for cleaner history
    let (_visible, _start, total) = compute_visible_lines(lines, 20, -5);

    assert_eq!(total, 10); // Trims trailing empty when scrolled up
}

#[test]
fn test_compute_visible_lines_scrolled_to_top() {
    let lines: Vec<Line> = (0..30).map(|i| Line::from(format!("line {}", i))).collect();

    // Scroll up enough to be at top
    let (visible, start, total) = compute_visible_lines(lines, 10, -20);

    assert_eq!(visible.len(), 10);
    assert_eq!(start, 0); // At the very top
    assert_eq!(total, 30);
}

#[test]
fn test_build_footer_text_at_bottom() {
    let footer = build_footer_text(0, 10);
    assert!(footer.contains("At bottom"));
    assert!(!footer.contains("Line"));
}

#[test]
fn test_build_footer_text_scrolled_up() {
    let footer = build_footer_text(-5, 10);
    assert!(footer.contains("Line 11")); // start_line + 1
    assert!(footer.contains("bottom")); // Ctrl+g option visible
}

#[test]
fn test_build_footer_text_at_top() {
    let footer = build_footer_text(-100, 0);
    assert!(footer.contains("Line 1"));
}

// === Rendering Tests ===

#[test]
fn test_render_shell_popup_basic() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut popup = ShellPopup::new("Test Task".to_string(), "test-window".to_string());
    popup.cached_content = b"Hello, World!\nLine 2\nLine 3".to_vec();

    let colors = ShellPopupColors::default();
    let lines: Vec<Line> = vec![
        Line::from("Hello, World!"),
        Line::from("Line 2"),
        Line::from("Line 3"),
    ];

    terminal.draw(|frame| {
        let area = Rect::new(0, 0, 80, 24);
        render_shell_popup(&popup, frame, area, lines, &colors);
    }).unwrap();

    // Verify the popup was rendered by checking the buffer
    let buffer = terminal.backend().buffer();

    // Check that the title is rendered somewhere in the buffer
    let buffer_content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(buffer_content.contains("Test Task"));
}

#[test]
fn test_render_shell_popup_with_content() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let popup = ShellPopup::new("My Task".to_string(), "window".to_string());
    let colors = ShellPopupColors::default();

    let lines: Vec<Line> = vec![
        Line::from("$ echo hello"),
        Line::from("hello"),
        Line::from("$ "),
    ];

    terminal.draw(|frame| {
        let area = Rect::new(0, 0, 80, 24);
        render_shell_popup(&popup, frame, area, lines, &colors);
    }).unwrap();

    let buffer = terminal.backend().buffer();
    let buffer_content: String = buffer.content().iter().map(|c| c.symbol()).collect();

    // Should contain task title
    assert!(buffer_content.contains("My Task"));
    // Should contain "At bottom" since scroll_offset is 0
    assert!(buffer_content.contains("At bottom"));
}

#[test]
fn test_render_shell_popup_scrolled_up() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let mut popup = ShellPopup::new("Scrolled Task".to_string(), "window".to_string());
    popup.scroll_up(10); // Scroll up into history

    let colors = ShellPopupColors::default();
    let lines: Vec<Line> = (0..30).map(|i| Line::from(format!("Line {}", i))).collect();

    terminal.draw(|frame| {
        let area = Rect::new(0, 0, 80, 24);
        render_shell_popup(&popup, frame, area, lines, &colors);
    }).unwrap();

    let buffer = terminal.backend().buffer();
    let buffer_content: String = buffer.content().iter().map(|c| c.symbol()).collect();

    // Should show "Line X" indicator since we scrolled up
    assert!(buffer_content.contains("Line"));
    // Should show the bottom shortcut since we're not at bottom
    assert!(buffer_content.contains("bottom"));
}

#[test]
fn test_render_shell_popup_empty_content() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let popup = ShellPopup::new("Empty Task".to_string(), "window".to_string());
    let colors = ShellPopupColors::default();
    let lines: Vec<Line> = vec![];

    terminal.draw(|frame| {
        let area = Rect::new(0, 0, 80, 24);
        render_shell_popup(&popup, frame, area, lines, &colors);
    }).unwrap();

    let buffer = terminal.backend().buffer();
    let buffer_content: String = buffer.content().iter().map(|c| c.symbol()).collect();

    // Should still render title and footer
    assert!(buffer_content.contains("Empty Task"));
    assert!(buffer_content.contains("At bottom"));
}

#[test]
fn test_render_shell_popup_custom_colors() {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();

    let popup = ShellPopup::new("Colored Task".to_string(), "window".to_string());

    let colors = ShellPopupColors {
        border: Color::Red,
        header_fg: Color::White,
        header_bg: Color::Blue,
        footer_fg: Color::Yellow,
        footer_bg: Color::Magenta,
    };

    let lines: Vec<Line> = vec![Line::from("content")];

    terminal.draw(|frame| {
        let area = Rect::new(0, 0, 80, 24);
        render_shell_popup(&popup, frame, area, lines, &colors);
    }).unwrap();

    // Just verify it doesn't crash with custom colors
    let buffer = terminal.backend().buffer();
    let buffer_content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(buffer_content.contains("Colored Task"));
}

#[test]
fn test_render_shell_popup_small_area() {
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();

    let popup = ShellPopup::new("Small Popup".to_string(), "window".to_string());
    let colors = ShellPopupColors::default();
    let lines: Vec<Line> = vec![Line::from("test")];

    terminal.draw(|frame| {
        let area = Rect::new(0, 0, 40, 10);
        render_shell_popup(&popup, frame, area, lines, &colors);
    }).unwrap();

    // Should handle small areas gracefully
    let buffer = terminal.backend().buffer();
    let buffer_content: String = buffer.content().iter().map(|c| c.symbol()).collect();
    assert!(buffer_content.contains("Small Popup"));
}

// === Trimming Tests ===

#[test]
fn test_trim_trailing_empty_lines_no_empty() {
    let lines = vec!["line 1", "line 2", "line 3"];
    let result = trim_trailing_empty_lines(&lines);
    assert_eq!(result, 3); // Keep all lines
}

#[test]
fn test_trim_trailing_empty_lines_few_empty() {
    let lines = vec!["line 1", "line 2", "", ""];
    let result = trim_trailing_empty_lines(&lines);
    // Last content at index 1, so keep 1 + 1 + MAX_TRAILING = 2 + 3 = 5, but only 4 lines exist
    assert_eq!(result, 4);
}

#[test]
fn test_trim_trailing_empty_lines_many_empty() {
    let lines = vec!["line 1", "line 2", "", "", "", "", "", "", "", ""];
    let result = trim_trailing_empty_lines(&lines);
    // Last content at index 1, keep up to index 1 + 1 + MAX_TRAILING = 5
    assert_eq!(result, 2 + MAX_TRAILING_EMPTY_LINES);
}

#[test]
fn test_trim_trailing_empty_lines_all_empty() {
    let lines = vec!["", "", "", "", "", "", "", "", "", ""];
    let result = trim_trailing_empty_lines(&lines);
    // All empty, keep just MAX_TRAILING
    assert_eq!(result, MAX_TRAILING_EMPTY_LINES);
}

#[test]
fn test_trim_trailing_empty_lines_empty_input() {
    let lines: Vec<&str> = vec![];
    let result = trim_trailing_empty_lines(&lines);
    assert_eq!(result, 0);
}

#[test]
fn test_trim_trailing_empty_lines_whitespace_only() {
    let lines = vec!["line 1", "   ", "\t", "  \t  "];
    let result = trim_trailing_empty_lines(&lines);
    // "   ", "\t", "  \t  " are all whitespace-only, treated as empty
    // Last real content at index 0
    assert_eq!(result, 1 + MAX_TRAILING_EMPTY_LINES);
}

#[test]
fn test_trim_content_to_cursor_no_cursor_info() {
    let content = b"line 1\nline 2\n\n\n\n\n\n\n\n\n".to_vec();
    let result = trim_content_to_cursor(content, None);
    let result_str = String::from_utf8_lossy(&result);
    // Count newlines + 1 to avoid lines() quirk with trailing newlines
    let line_count = result_str.matches('\n').count() + 1;
    // Should fall back to trimming empty lines
    // 2 content lines + MAX_TRAILING empty lines
    assert_eq!(line_count, 2 + MAX_TRAILING_EMPTY_LINES);
}

#[test]
fn test_trim_content_to_cursor_with_cursor_info() {
    // Simulate: 10 lines captured, pane_height=5, cursor at line 2 of visible area
    // visible_pane_start = 10 - 5 = 5
    // cursor_in_capture = 5 + 2 = 7
    // Lines below cursor are empty (unused pane buffer) → trimmed at cursor
    let content = b"line 0\nline 1\nline 2\nline 3\nline 4\nline 5\nline 6\nline 7\n\n".to_vec();
    let cursor_info = Some((2, 5)); // cursor_y=2, pane_height=5
    let result = trim_content_to_cursor(content, cursor_info);
    let result_str = String::from_utf8_lossy(&result);
    let lines: Vec<&str> = result_str.lines().collect();

    assert_eq!(lines.len(), 8); // Lines 0-7
    assert_eq!(lines[0], "line 0");
    assert_eq!(lines[7], "line 7");
}

#[test]
fn test_trim_content_to_cursor_tui_cursor_mid_screen() {
    // TUI app (OpenCode, Gemini) with cursor in the middle — content below cursor is NOT empty
    // Should keep all content, not trim at cursor
    let content = b"header\nstatus bar\n\ninput field\n\noutput area\nmore output\nbottom bar".to_vec();
    let cursor_info = Some((3, 8)); // cursor_y=3 (mid-screen), pane_height=8
    let result = trim_content_to_cursor(content, cursor_info);
    let result_str = String::from_utf8_lossy(&result);
    let lines: Vec<&str> = result_str.lines().collect();

    assert_eq!(lines.len(), 8); // All lines kept
    assert_eq!(lines[0], "header");
    assert_eq!(lines[7], "bottom bar");
}

#[test]
fn test_trim_content_to_cursor_cursor_at_bottom_with_empty() {
    // Cursor at bottom of pane, but those lines are empty
    // Should trim the empty lines via second pass
    let content = b"line 0\nline 1\nline 2\n\n\n\n\n".to_vec();
    let cursor_info = Some((6, 7)); // cursor at line 6 of 7-line pane (bottom)
    let result = trim_content_to_cursor(content, cursor_info);
    let result_str = String::from_utf8_lossy(&result);
    // Count newlines + 1 to avoid lines() quirk with trailing newlines
    let line_count = result_str.matches('\n').count() + 1;

    // Content has 3 real lines, then empty
    // Second pass should trim to 3 + MAX_TRAILING
    assert_eq!(line_count, 3 + MAX_TRAILING_EMPTY_LINES);
}

#[test]
fn test_trim_content_to_cursor_empty_content() {
    let content = b"".to_vec();
    let result = trim_content_to_cursor(content.clone(), Some((0, 10)));
    assert_eq!(result, content);
}

#[test]
fn test_trim_content_to_cursor_zero_pane_height() {
    let content = b"line 1\nline 2\n\n\n\n".to_vec();
    let result = trim_content_to_cursor(content, Some((0, 0)));
    let result_str = String::from_utf8_lossy(&result);
    // Count newlines + 1 to avoid lines() quirk with trailing newlines
    let line_count = result_str.matches('\n').count() + 1;
    // pane_height=0 should fall through to second pass only
    assert_eq!(line_count, 2 + MAX_TRAILING_EMPTY_LINES);
}
