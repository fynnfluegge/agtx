use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};

use crate::agent;
use crate::config::{GlobalConfig, MergedConfig, ProjectConfig, ThemeConfig};
use crate::db::{Database, Task, TaskStatus};
use crate::git;
use crate::tmux;
use crate::AppMode;

use super::board::BoardState;
use super::input::InputMode;

/// Helper to convert hex color string to ratatui Color
fn hex_to_color(hex: &str) -> Color {
    ThemeConfig::parse_hex(hex)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .unwrap_or(Color::White)
}

type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

/// Application state (separate from terminal for borrow checker)
struct AppState {
    mode: AppMode,
    should_quit: bool,
    board: BoardState,
    input_mode: InputMode,
    input_buffer: String,
    // For task creation/editing
    pending_task_title: String,
    editing_task_id: Option<String>, // Some(id) when editing, None when creating
    db: Option<Database>,
    #[allow(dead_code)]
    global_db: Database,
    config: MergedConfig,
    project_path: Option<PathBuf>,
    project_name: String,
    available_agents: Vec<agent::Agent>,
    // Sidebar
    sidebar_visible: bool,
    sidebar_focused: bool,
    projects: Vec<ProjectInfo>,
    selected_project: usize,
    // Dashboard state
    show_project_list: bool,
    // Task shell popup
    shell_popup: Option<ShellPopup>,
    // File search dropdown
    file_search: Option<FileSearchState>,
    // Task search popup
    task_search: Option<TaskSearchState>,
    // PR creation confirmation popup
    pr_confirm_popup: Option<PrConfirmPopup>,
    // Moving Review back to Running
    review_to_running_task_id: Option<String>,
}

/// State for task search popup
#[derive(Debug, Clone)]
struct TaskSearchState {
    query: String,
    matches: Vec<(String, String, TaskStatus)>, // (id, title, status)
    selected: usize,
}

/// State for PR creation confirmation popup
#[derive(Debug, Clone)]
struct PrConfirmPopup {
    task_id: String,
    pr_title: String,
    pr_body: String,
    editing_title: bool, // true = editing title, false = editing body
    generating: bool,    // true while Claude is generating description
}

/// State for the shell popup that shows a detached tmux window
#[derive(Debug, Clone)]
struct ShellPopup {
    task_title: String,
    window_name: String,
    scroll_offset: i32, // Negative means scroll up (see more history)
}

/// Info about a project for the sidebar
#[derive(Debug, Clone)]
struct ProjectInfo {
    name: String,
    path: String,
}

/// State for file search dropdown
#[derive(Debug, Clone)]
struct FileSearchState {
    pattern: String,
    matches: Vec<String>,
    selected: usize,
    start_pos: usize, // Position in input_buffer where # was typed
}

pub struct App {
    terminal: Terminal,
    state: AppState,
}

impl App {
    pub fn new(mode: AppMode) -> Result<Self> {
        // Setup terminal
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let terminal = Terminal::new(backend)?;

        // Load configs
        let global_config = GlobalConfig::load().unwrap_or_default();
        let global_db = Database::open_global()?;

        // Detect available agents
        let available_agents = agent::detect_available_agents();

        // Setup based on mode
        let (db, project_path, project_name, project_config) = match &mode {
            AppMode::Dashboard => (None, None, "Dashboard".to_string(), ProjectConfig::default()),
            AppMode::Project(path) => {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                let name = canonical
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let project_config = ProjectConfig::load(&canonical).unwrap_or_default();
                let db = Database::open_project(&canonical)?;

                // Register project in global database
                let project = crate::db::Project::new(&name, canonical.to_string_lossy());
                global_db.upsert_project(&project)?;

                // Ensure tmux session exists for this project
                ensure_project_tmux_session(&name, &canonical);

                (Some(db), Some(canonical), name, project_config)
            }
        };

        let config = MergedConfig::merge(&global_config, &project_config);

        let mut app = Self {
            terminal,
            state: AppState {
                mode,
                should_quit: false,
                board: BoardState::new(),
                input_mode: InputMode::Normal,
                input_buffer: String::new(),
                pending_task_title: String::new(),
                editing_task_id: None,
                db,
                global_db,
                config,
                project_path,
                project_name: project_name.clone(),
                available_agents,
                sidebar_visible: true,
                sidebar_focused: false,
                projects: vec![],
                selected_project: 0,
                show_project_list: false,
                shell_popup: None,
                file_search: None,
                task_search: None,
                pr_confirm_popup: None,
                review_to_running_task_id: None,
            },
        };

        // Load tasks if in project mode
        app.refresh_tasks()?;
        // Load projects from global database
        app.refresh_projects()?;

        Ok(app)
    }

    pub async fn run(&mut self) -> Result<()> {
        while !self.state.should_quit {
            self.draw()?;

            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key)?;
                    }
                }
            }

            // Periodically refresh session status
            self.refresh_sessions()?;
        }

        Ok(())
    }

    fn draw(&mut self) -> Result<()> {
        let state = &self.state;
        self.terminal.draw(|frame| {
            let area = frame.area();

            match &state.mode {
                AppMode::Dashboard => Self::draw_dashboard(state, frame, area),
                AppMode::Project(_) => Self::draw_board(state, frame, area),
            }
        })?;

        Ok(())
    }

    fn draw_board(state: &AppState, frame: &mut Frame, area: Rect) {
        // Main layout with optional sidebar
        let main_chunks = if state.sidebar_visible {
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Length(25), // Sidebar
                    Constraint::Min(0),     // Main content
                ])
                .split(area)
        } else {
            // No sidebar - use full area
            Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Min(0)])
                .split(area)
        };

        // Draw sidebar if visible
        if state.sidebar_visible {
            Self::draw_sidebar(state, frame, main_chunks[0]);
        }

        let content_area = if state.sidebar_visible {
            main_chunks[1]
        } else {
            main_chunks[0]
        };

        // Main layout: header, board, footer
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Header
                Constraint::Min(0),    // Board
                Constraint::Length(3), // Footer
            ])
            .split(content_area);

        // Header
        let header = Paragraph::new(format!(" {} ", state.project_name))
            .style(Style::default().fg(Color::Cyan).bold())
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, chunks[0]);

        // Board columns (5 columns: Backlog, Planning, Running, Review, Done)
        let columns = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(20),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
            ])
            .split(chunks[1]);

        for (i, status) in TaskStatus::columns().iter().enumerate() {
            let tasks: Vec<&Task> = state.board.tasks.iter().filter(|t| t.status == *status).collect();

            let is_selected_column = state.board.selected_column == i;

            let title = format!(" {} ({}) ", status.as_str(), tasks.len());
            let border_style = if is_selected_column {
                Style::default().fg(hex_to_color(&state.config.theme.color_selected))
            } else {
                Style::default().fg(hex_to_color(&state.config.theme.color_normal))
            };

            // Calculate card height (title + preview lines + borders)
            let card_height: u16 = 10; // 1 title + 7 preview lines + 2 borders
            let max_visible_cards = (columns[i].height.saturating_sub(2) / card_height) as usize;

            // Calculate scroll offset to keep selected task visible
            let scroll_offset = if is_selected_column && tasks.len() > max_visible_cards {
                let selected = state.board.selected_row;
                if selected >= max_visible_cards {
                    selected - max_visible_cards + 1
                } else {
                    0
                }
            } else {
                0
            };

            // Check if we need a scrollbar
            let needs_scrollbar = tasks.len() > max_visible_cards;
            let content_width = if needs_scrollbar {
                columns[i].width.saturating_sub(3) // Leave room for scrollbar
            } else {
                columns[i].width.saturating_sub(2)
            };

            // Draw column border
            let column_block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(border_style);
            let inner_area = column_block.inner(columns[i]);
            frame.render_widget(column_block, columns[i]);

            // Render task cards with scroll offset
            let visible_tasks: Vec<_> = tasks.iter().skip(scroll_offset).take(max_visible_cards).collect();
            for (j, task) in visible_tasks.iter().enumerate() {
                let actual_index = scroll_offset + j;
                let is_selected = is_selected_column && state.board.selected_row == actual_index;

                let card_area = Rect {
                    x: inner_area.x,
                    y: inner_area.y + (j as u16 * card_height),
                    width: if needs_scrollbar { inner_area.width.saturating_sub(1) } else { inner_area.width },
                    height: card_height.min(inner_area.height.saturating_sub(j as u16 * card_height)),
                };

                if card_area.height < 3 {
                    break;
                }

                Self::draw_task_card(frame, task, card_area, is_selected, &state.config.theme);
            }

            // Draw scrollbar if needed
            if needs_scrollbar {
                let scrollbar_area = Rect {
                    x: inner_area.x + inner_area.width - 1,
                    y: inner_area.y,
                    width: 1,
                    height: inner_area.height,
                };

                let total_tasks = tasks.len();
                let scrollbar_height = inner_area.height as usize;
                let thumb_height = (max_visible_cards * scrollbar_height / total_tasks).max(1);
                let thumb_pos = (scroll_offset * scrollbar_height / total_tasks).min(scrollbar_height - thumb_height);

                for y in 0..scrollbar_height {
                    let char = if y >= thumb_pos && y < thumb_pos + thumb_height {
                        "█"
                    } else {
                        "░"
                    };
                    let style = Style::default().fg(Color::DarkGray);
                    frame.render_widget(
                        Paragraph::new(char).style(style),
                        Rect {
                            x: scrollbar_area.x,
                            y: scrollbar_area.y + y as u16,
                            width: 1,
                            height: 1,
                        },
                    );
                }
            }
        }

        // Footer with help
        let footer_text = match state.input_mode {
            InputMode::Normal => {
                if state.sidebar_focused {
                    " [j/k] navigate  [Enter] open  [l] board  [e] hide sidebar  [q] quit "
                } else {
                    " [o] new  [/] search  [Enter] open  [d] del  [m] move  [r] resume  [e] sidebar  [q] quit"
                }
            }
            InputMode::InputTitle => " Enter task title... [Esc] cancel [Enter] next ",
            InputMode::InputDescription => " Enter prompt for agent... [Esc] cancel [\\+Enter] newline [Enter] save ",
        };

        let footer = Paragraph::new(footer_text)
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);

        // Input overlay if in input mode
        if state.input_mode == InputMode::InputTitle || state.input_mode == InputMode::InputDescription {
            let input_area = centered_rect(50, 40, area);
            frame.render_widget(Clear, input_area);

            let is_editing = state.editing_task_id.is_some();
            let (title, label) = match state.input_mode {
                InputMode::InputTitle => {
                    if is_editing { (" Edit Task ", "Title: ") } else { (" New Task ", "Title: ") }
                }
                InputMode::InputDescription => {
                    if is_editing { (" Edit Task ", "Prompt: ") } else { (" New Task ", "Prompt: ") }
                }
                _ => ("", ""),
            };

            // Show title if we're on description step
            let content = if state.input_mode == InputMode::InputDescription {
                format!(
                    "Title: {}\n\n{}{}█",
                    state.pending_task_title,
                    label,
                    state.input_buffer
                )
            } else {
                format!("{}{}█", label, state.input_buffer)
            };

            let input = Paragraph::new(content)
                .style(Style::default().fg(Color::Yellow))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow)),
                );
            frame.render_widget(input, input_area);

            // File search dropdown
            if let Some(ref search) = state.file_search {
                if !search.matches.is_empty() {
                    let dropdown_height = (search.matches.len() as u16 + 2).min(12);
                    let dropdown_area = Rect {
                        x: input_area.x + 2,
                        y: input_area.y + input_area.height,
                        width: input_area.width.saturating_sub(4),
                        height: dropdown_height,
                    };

                    // Make sure dropdown doesn't go off screen
                    let dropdown_area = if dropdown_area.y + dropdown_area.height > area.height {
                        Rect {
                            y: input_area.y.saturating_sub(dropdown_height),
                            ..dropdown_area
                        }
                    } else {
                        dropdown_area
                    };

                    frame.render_widget(Clear, dropdown_area);

                    let items: Vec<ListItem> = search.matches
                        .iter()
                        .enumerate()
                        .map(|(i, path)| {
                            let style = if i == search.selected {
                                Style::default().bg(Color::Yellow).fg(Color::Black)
                            } else {
                                Style::default().fg(Color::White)
                            };
                            ListItem::new(format!(" {} ", path)).style(style)
                        })
                        .collect();

                    let list = List::new(items)
                        .block(
                            Block::default()
                                .title(" Files [↑↓] select [Tab/Enter] insert [Esc] cancel ")
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(Color::Cyan)),
                        );
                    frame.render_widget(list, dropdown_area);
                }
            }
        }

        // Shell popup overlay
        if let Some(popup) = &state.shell_popup {
            let popup_area = centered_rect(75, 75, area);
            frame.render_widget(Clear, popup_area);

            // Capture tmux pane content with more history
            let pane_content = capture_tmux_pane_with_history(&popup.window_name, 500);

            // Layout: header, content, footer
            let popup_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Title bar
                    Constraint::Min(0),    // Shell content
                    Constraint::Length(1), // Footer
                ])
                .split(popup_area);

            // Title bar
            let title = format!(" {} ", popup.task_title);
            let title_bar = Paragraph::new(title)
                .style(Style::default().fg(Color::Black).bg(Color::Cyan));
            frame.render_widget(title_bar, popup_chunks[0]);

            // Shell content - parse ANSI escape sequences for colors
            let styled_lines = parse_ansi_to_lines(&pane_content);
            let visible_height = popup_chunks[1].height.saturating_sub(2) as usize;

            // Apply scroll offset
            let total_lines = styled_lines.len();
            let start_line = if popup.scroll_offset < 0 {
                // Scrolling up into history
                total_lines.saturating_sub(visible_height).saturating_sub((-popup.scroll_offset) as usize)
            } else {
                // At bottom (current)
                total_lines.saturating_sub(visible_height)
            };

            let visible_lines: Vec<Line> = styled_lines
                .into_iter()
                .skip(start_line)
                .take(visible_height)
                .collect();

            let content = Paragraph::new(visible_lines)
                .block(
                    Block::default()
                        .borders(Borders::LEFT | Borders::RIGHT)
                        .border_style(Style::default().fg(Color::Cyan)),
                );
            frame.render_widget(content, popup_chunks[1]);

            // Footer with scroll indicator
            let scroll_indicator = if popup.scroll_offset < 0 {
                format!(" [↑↓/jk] scroll [g] bottom [Esc] close | Line {} ", start_line + 1)
            } else {
                " [↑↓/jk] scroll [Esc] close | At bottom ".to_string()
            };
            let footer = Paragraph::new(scroll_indicator)
                .style(Style::default().fg(Color::Black).bg(Color::DarkGray));
            frame.render_widget(footer, popup_chunks[2]);
        }

        // Task search popup
        if let Some(ref search) = state.task_search {
            let popup_area = centered_rect(50, 50, area);
            frame.render_widget(Clear, popup_area);

            let popup_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3), // Search input
                    Constraint::Min(0),    // Results
                ])
                .split(popup_area);

            // Search input
            let input = Paragraph::new(format!(" 🔍 {}█", search.query))
                .style(Style::default().fg(Color::Yellow))
                .block(
                    Block::default()
                        .title(" Search Tasks ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::Yellow)),
                );
            frame.render_widget(input, popup_chunks[0]);

            // Results list
            let items: Vec<ListItem> = search.matches
                .iter()
                .enumerate()
                .map(|(i, (_, title, status))| {
                    let is_selected = i == search.selected;
                    let style = if is_selected {
                        Style::default().bg(Color::Yellow).fg(Color::Black)
                    } else {
                        Style::default().fg(Color::White)
                    };

                    let status_icon = match status {
                        TaskStatus::Backlog => "📋",
                        TaskStatus::Planning => "📝",
                        TaskStatus::Running => "⚡",
                        TaskStatus::Review => "👀",
                        TaskStatus::Done => "✅",
                    };

                    ListItem::new(format!(" {} {} ", status_icon, title)).style(style)
                })
                .collect();

            let list = List::new(items)
                .block(
                    Block::default()
                        .title(" [↑↓] select [Enter] jump [Esc] cancel ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(Color::DarkGray)),
                );
            frame.render_widget(list, popup_chunks[1]);
        }

        // PR confirmation popup
        if let Some(ref popup) = state.pr_confirm_popup {
            let popup_area = centered_rect(60, 60, area);
            frame.render_widget(Clear, popup_area);

            let popup_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(3),  // Title input
                    Constraint::Min(0),     // Body input
                    Constraint::Length(1),  // Help line
                ])
                .margin(1)
                .split(popup_area);

            // Main border
            let main_block = Block::default()
                .title(" Create Pull Request ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Green));
            frame.render_widget(main_block, popup_area);

            // Title input
            let title_style = if popup.editing_title {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };
            let title_border = if popup.editing_title {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let title_cursor = if popup.editing_title { "█" } else { "" };
            let title_input = Paragraph::new(format!("{}{}", popup.pr_title, title_cursor))
                .style(title_style)
                .block(
                    Block::default()
                        .title(" Title ")
                        .borders(Borders::ALL)
                        .border_style(title_border),
                );
            frame.render_widget(title_input, popup_chunks[0]);

            // Body input
            let body_style = if !popup.editing_title {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };
            let body_border = if !popup.editing_title {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let body_cursor = if !popup.editing_title { "█" } else { "" };
            let body_input = Paragraph::new(format!("{}{}", popup.pr_body, body_cursor))
                .style(body_style)
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(" Description ")
                        .borders(Borders::ALL)
                        .border_style(body_border),
                );
            frame.render_widget(body_input, popup_chunks[1]);

            // Help line
            let help = Paragraph::new(" [Tab] switch field  [Ctrl+Enter] create PR  [Esc] cancel ")
                .style(Style::default().fg(Color::DarkGray));
            frame.render_widget(help, popup_chunks[2]);
        }
    }

    fn draw_task_card(frame: &mut Frame, task: &Task, area: Rect, is_selected: bool, theme: &ThemeConfig) {
        let border_style = if is_selected {
            Style::default().fg(hex_to_color(&theme.color_selected))
        } else {
            Style::default().fg(hex_to_color(&theme.color_normal))
        };

        let title_style = if is_selected {
            Style::default().fg(hex_to_color(&theme.color_selected)).bold()
        } else {
            Style::default().fg(hex_to_color(&theme.color_text)).bold()
        };

        // Truncate title to fit
        let max_title_len = area.width.saturating_sub(4) as usize;
        let title: String = if task.title.len() > max_title_len {
            format!("{}...", &task.title[..max_title_len.saturating_sub(3)])
        } else {
            task.title.clone()
        };

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style);
        let inner = card_block.inner(area);
        frame.render_widget(card_block, area);

        // Title line
        let title_line = Paragraph::new(title).style(title_style);
        let title_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 1,
        };
        frame.render_widget(title_line, title_area);

        // Preview area (below title)
        if inner.height > 1 {
            let preview_area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };

            if let Some(session_name) = &task.session_name {
                // Capture tmux pane content for preview
                let pane_content = capture_tmux_pane(session_name);
                let lines = parse_ansi_to_lines(&pane_content);

                // Skip last 5 lines (input prompt area) and take lines that fit
                let skip_last = 5;
                let max_width = preview_area.width as usize;
                let preview_lines: Vec<Line> = lines
                    .into_iter()
                    .rev()
                    .skip(skip_last)
                    .take(preview_area.height as usize)
                    .map(|line| {
                        // Truncate each line to fit the preview width
                        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
                        if text.len() > max_width {
                            Line::from(format!("{}…", &text[..max_width.saturating_sub(1)]))
                        } else {
                            line
                        }
                    })
                    .collect::<Vec<_>>()
                    .into_iter()
                    .rev()
                    .collect();

                let preview = Paragraph::new(preview_lines)
                    .style(Style::default().fg(Color::DarkGray));
                frame.render_widget(preview, preview_area);
            } else {
                // No session - show description preview or placeholder
                let preview_text = task.description.as_deref().unwrap_or("No agent session");

                // Truncate description to fit preview area
                let max_chars = (preview_area.width as usize) * (preview_area.height as usize);
                let truncated: String = if preview_text.len() > max_chars {
                    format!("{}...", &preview_text.chars().take(max_chars.saturating_sub(3)).collect::<String>())
                } else {
                    preview_text.to_string()
                };

                let preview = Paragraph::new(truncated)
                    .style(Style::default().fg(Color::DarkGray).italic())
                    .wrap(Wrap { trim: true });
                frame.render_widget(preview, preview_area);
            }
        }
    }

    fn draw_sidebar(state: &AppState, frame: &mut Frame, area: Rect) {
        // Show projects from database
        let current_path = state.project_path.as_ref().map(|p| p.to_string_lossy().to_string());

        let items: Vec<ListItem> = state
            .projects
            .iter()
            .enumerate()
            .map(|(i, project)| {
                let is_selected = i == state.selected_project && state.sidebar_focused;
                let is_current = current_path.as_ref() == Some(&project.path);

                let style = if is_selected {
                    Style::default().bg(Color::Yellow).fg(Color::Black)
                } else if is_current {
                    Style::default().fg(Color::Cyan)
                } else {
                    Style::default()
                };

                let marker = if is_current { " ●" } else { "" };
                ListItem::new(format!(" {}{}", project.name, marker)).style(style)
            })
            .collect();

        let title = format!(" 📁 Projects ({}) ", state.projects.len());
        let border_color = if state.sidebar_focused {
            hex_to_color(&state.config.theme.color_selected)
        } else {
            hex_to_color(&state.config.theme.color_normal)
        };
        let list = List::new(items).block(
            Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color)),
        );

        frame.render_widget(list, area);
    }

    fn draw_dashboard(state: &AppState, frame: &mut Frame, area: Rect) {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3),
                Constraint::Min(0),
                Constraint::Length(3),
            ])
            .split(area);

        // Header
        let header = Paragraph::new(" agtx Dashboard ")
            .style(Style::default().fg(Color::Cyan).bold())
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(header, chunks[0]);

        // Content area split: message + options/project list
        let content_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5), // Message
                Constraint::Min(0),    // Project list or options
            ])
            .split(chunks[1]);

        // Message
        let message = Paragraph::new("\n  No project found in current directory.\n")
            .style(Style::default().fg(Color::Yellow))
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
        frame.render_widget(message, content_chunks[0]);

        // Project list or options
        if state.show_project_list && !state.projects.is_empty() {
            // Show project list
            let items: Vec<ListItem> = state
                .projects
                .iter()
                .enumerate()
                .map(|(i, project)| {
                    let is_selected = i == state.selected_project;
                    let style = if is_selected {
                        Style::default().bg(Color::DarkGray).fg(Color::White)
                    } else {
                        Style::default()
                    };
                    ListItem::new(format!("  {}", project.name)).style(style)
                })
                .collect();

            let list = List::new(items).block(
                Block::default()
                    .title(" Projects [j/k] navigate [Enter] open [Esc] back ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Yellow)),
            );
            frame.render_widget(list, content_chunks[1]);
        } else {
            // Show options
            let options = Paragraph::new(
                "\n  [p] Open existing project\n  [n] Create new project in current directory\n",
            )
            .block(Block::default().title(" Options ").borders(Borders::ALL).border_style(Style::default().fg(Color::DarkGray)));
            frame.render_widget(options, content_chunks[1]);
        }

        // Footer
        let footer = Paragraph::new(" [p] projects  [n] new project  [q] quit ")
            .style(Style::default().fg(Color::DarkGray))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Handle PR confirmation popup if open
        if self.state.pr_confirm_popup.is_some() {
            return self.handle_pr_confirm_key(key);
        }

        // Handle task search popup if open
        if self.state.task_search.is_some() {
            return self.handle_task_search_key(key);
        }

        // Handle shell popup if open
        if self.state.shell_popup.is_some() {
            return self.handle_shell_popup_key(key);
        }

        // Handle based on mode (Dashboard vs Project)
        match &self.state.mode {
            AppMode::Dashboard => self.handle_dashboard_key(key.code),
            AppMode::Project(_) => {
                match self.state.input_mode {
                    InputMode::Normal => self.handle_normal_key(key.code),
                    InputMode::InputTitle => self.handle_title_input(key.code),
                    InputMode::InputDescription => self.handle_description_input(key),
                }
            }
        }
    }

    fn handle_pr_confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        use crossterm::event::KeyModifiers;

        if let Some(ref mut popup) = self.state.pr_confirm_popup {
            match key.code {
                KeyCode::Esc => {
                    self.state.pr_confirm_popup = None;
                }
                KeyCode::Tab => {
                    // Switch between title and body editing
                    popup.editing_title = !popup.editing_title;
                }
                KeyCode::Enter => {
                    if popup.editing_title {
                        // Move to body editing
                        popup.editing_title = false;
                    } else if key.modifiers.contains(KeyModifiers::CONTROL) {
                        // Ctrl+Enter: Submit and create PR
                        let task_id = popup.task_id.clone();
                        let pr_title = popup.pr_title.clone();
                        let pr_body = popup.pr_body.clone();
                        self.state.pr_confirm_popup = None;
                        self.create_pr_and_move_to_review_with_content(&task_id, &pr_title, &pr_body)?;
                    } else {
                        // Regular Enter in body: add newline
                        popup.pr_body.push('\n');
                    }
                }
                KeyCode::Backspace => {
                    if popup.editing_title {
                        popup.pr_title.pop();
                    } else {
                        popup.pr_body.pop();
                    }
                }
                KeyCode::Char(c) => {
                    if popup.editing_title {
                        popup.pr_title.push(c);
                    } else {
                        popup.pr_body.push(c);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn create_pr_and_move_to_review_with_content(&mut self, task_id: &str, pr_title: &str, pr_body: &str) -> Result<()> {
        if let (Some(db), Some(project_path)) = (&self.state.db, self.state.project_path.clone()) {
            if let Some(mut task) = db.get_task(task_id)? {
                // Kill tmux window first
                if let Some(session_name) = &task.session_name {
                    let _ = std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["kill-window", "-t", session_name])
                        .output();
                }
                task.session_name = None;

                // Create PR with provided title and body
                match create_pr_with_content(&task, &project_path, pr_title, pr_body) {
                    Ok((pr_number, pr_url)) => {
                        task.pr_number = Some(pr_number);
                        task.pr_url = Some(pr_url);
                    }
                    Err(e) => {
                        // TODO: Show error to user
                        eprintln!("Failed to create PR: {}", e);
                    }
                }

                // Move to Review
                task.status = TaskStatus::Review;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn handle_task_search_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        use crossterm::event::KeyModifiers;

        let should_close = match key.code {
            KeyCode::Esc => {
                self.state.task_search = None;
                true
            }
            KeyCode::Enter => {
                // Jump to selected task
                if let Some(ref search) = self.state.task_search {
                    if let Some((task_id, _, status)) = search.matches.get(search.selected).cloned() {
                        // Find column index for this status
                        let col_idx = TaskStatus::columns().iter().position(|s| *s == status).unwrap_or(0);
                        self.state.board.selected_column = col_idx;

                        // Find row index for this task
                        let tasks_in_col: Vec<_> = self.state.board.tasks.iter()
                            .filter(|t| t.status == status)
                            .collect();
                        if let Some(row_idx) = tasks_in_col.iter().position(|t| t.id == task_id) {
                            self.state.board.selected_row = row_idx;
                        }
                    }
                }
                self.state.task_search = None;
                true
            }
            KeyCode::Up | KeyCode::BackTab => {
                if let Some(ref mut search) = self.state.task_search {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                false
            }
            KeyCode::Down | KeyCode::Tab => {
                if let Some(ref mut search) = self.state.task_search {
                    if search.selected < search.matches.len().saturating_sub(1) {
                        search.selected += 1;
                    }
                }
                false
            }
            KeyCode::Char('k') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref mut search) = self.state.task_search {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                false
            }
            KeyCode::Char('j') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref mut search) = self.state.task_search {
                    if search.selected < search.matches.len().saturating_sub(1) {
                        search.selected += 1;
                    }
                }
                false
            }
            KeyCode::Backspace => {
                if let Some(ref mut search) = self.state.task_search {
                    search.query.pop();
                }
                let query = self.state.task_search.as_ref().map(|s| s.query.clone()).unwrap_or_default();
                let matches = self.get_all_task_matches(&query);
                if let Some(ref mut search) = self.state.task_search {
                    search.matches = matches;
                    search.selected = 0;
                }
                false
            }
            KeyCode::Char(c) => {
                if let Some(ref mut search) = self.state.task_search {
                    search.query.push(c);
                }
                let query = self.state.task_search.as_ref().map(|s| s.query.clone()).unwrap_or_default();
                let matches = self.get_all_task_matches(&query);
                if let Some(ref mut search) = self.state.task_search {
                    search.matches = matches;
                    search.selected = 0;
                }
                false
            }
            _ => false,
        };

        if should_close {
            self.state.task_search = None;
        }

        Ok(())
    }

    fn get_all_task_matches(&self, query: &str) -> Vec<(String, String, TaskStatus)> {
        let query_lower = query.to_lowercase();

        let mut matches: Vec<(String, String, TaskStatus, i32)> = self.state.board.tasks
            .iter()
            .filter_map(|task| {
                let title_lower = task.title.to_lowercase();
                let score = if query.is_empty() {
                    1
                } else {
                    fuzzy_score(&title_lower, &query_lower)
                };

                if score > 0 {
                    Some((task.id.clone(), task.title.clone(), task.status, score))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score (higher is better)
        matches.sort_by(|a, b| b.3.cmp(&a.3));

        matches.into_iter()
            .take(10)
            .map(|(id, title, status, _)| (id, title, status))
            .collect()
    }

    fn handle_shell_popup_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        use crossterm::event::KeyModifiers;

        if let Some(ref mut popup) = self.state.shell_popup {
            let window_name = popup.window_name.clone();
            let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

            match key.code {
                KeyCode::Esc => {
                    // Close popup
                    self.state.shell_popup = None;
                }
                // Scroll with Ctrl+k or Ctrl+Up
                KeyCode::Char('k') | KeyCode::Up if has_ctrl => {
                    popup.scroll_offset -= 5;
                }
                // Scroll with Ctrl+j or Ctrl+Down
                KeyCode::Char('j') | KeyCode::Down if has_ctrl => {
                    popup.scroll_offset = (popup.scroll_offset + 5).min(0);
                }
                KeyCode::PageUp => {
                    popup.scroll_offset -= 20;
                }
                KeyCode::PageDown => {
                    popup.scroll_offset = (popup.scroll_offset + 20).min(0);
                }
                // Ctrl+g = go to bottom (current)
                KeyCode::Char('g') if has_ctrl => {
                    popup.scroll_offset = 0;
                }
                _ => {
                    // Forward all other keys to tmux window
                    send_key_to_tmux(&window_name, key.code);
                }
            }
        }
        Ok(())
    }

    fn handle_dashboard_key(&mut self, key: KeyCode) -> Result<()> {
        match key {
            KeyCode::Char('q') => self.state.should_quit = true,
            _ => {}
        }
        Ok(())
    }

    fn handle_normal_key(&mut self, key: KeyCode) -> Result<()> {
        // Handle sidebar navigation if focused
        if self.state.sidebar_focused && self.state.sidebar_visible {
            match key {
                KeyCode::Char('q') => self.state.should_quit = true,
                KeyCode::Char('e') => {
                    // Toggle sidebar visibility
                    self.state.sidebar_visible = false;
                    self.state.sidebar_focused = false;
                }
                KeyCode::Char('l') | KeyCode::Right | KeyCode::Esc => {
                    // Move focus back to board
                    self.state.sidebar_focused = false;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.state.selected_project < self.state.projects.len().saturating_sub(1) {
                        self.state.selected_project += 1;
                        // Switch to project immediately on cursor move
                        if let Some(project) = self.state.projects.get(self.state.selected_project).cloned() {
                            self.switch_to_project_keep_sidebar(&project)?;
                        }
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if self.state.selected_project > 0 {
                        self.state.selected_project -= 1;
                        // Switch to project immediately on cursor move
                        if let Some(project) = self.state.projects.get(self.state.selected_project).cloned() {
                            self.switch_to_project_keep_sidebar(&project)?;
                        }
                    }
                }
                KeyCode::Enter => {
                    // Enter focuses the board (sidebar stays visible)
                    self.state.sidebar_focused = false;
                }
                _ => {}
            }
            return Ok(());
        }

        // Handle board navigation
        match key {
            KeyCode::Char('q') => self.state.should_quit = true,
            KeyCode::Char('e') => {
                // Toggle sidebar visibility
                self.state.sidebar_visible = !self.state.sidebar_visible;
                if self.state.sidebar_visible {
                    self.refresh_projects()?;
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                // Move to sidebar only if visible AND in first column (Backlog)
                if self.state.sidebar_visible && self.state.board.selected_column == 0 {
                    self.state.sidebar_focused = true;
                    self.refresh_projects()?;
                } else {
                    self.state.board.move_left();
                }
            }
            KeyCode::Char('l') | KeyCode::Right => self.state.board.move_right(),
            KeyCode::Char('j') | KeyCode::Down => self.state.board.move_down(),
            KeyCode::Char('k') | KeyCode::Up => self.state.board.move_up(),
            KeyCode::Char('o') => {
                // New task
                self.state.input_mode = InputMode::InputTitle;
                self.state.input_buffer.clear();
                self.state.pending_task_title.clear();
                self.state.editing_task_id = None;
            }
            KeyCode::Enter => {
                // If in Backlog, edit the task; otherwise open shell popup
                if let Some(task) = self.state.board.selected_task() {
                    if task.status == TaskStatus::Backlog {
                        // Edit task
                        self.state.editing_task_id = Some(task.id.clone());
                        self.state.input_buffer = task.title.clone();
                        self.state.pending_task_title.clear();
                        self.state.input_mode = InputMode::InputTitle;
                    } else if task.session_name.is_some() {
                        // Open shell popup
                        self.open_selected_task()?;
                    }
                }
            }
            KeyCode::Char('d') => self.delete_selected_task()?,
            KeyCode::Char('m') => self.move_task_right()?,
            KeyCode::Char('r') => {
                // Move Review task back to Running (for PR changes)
                if let Some(task) = self.state.board.selected_task() {
                    if task.status == TaskStatus::Review {
                        let task_id = task.id.clone();
                        self.move_review_to_running(&task_id)?;
                    }
                }
            }
            KeyCode::Char('/') => {
                // Open task search
                self.state.task_search = Some(TaskSearchState {
                    query: String::new(),
                    matches: self.get_all_task_matches(""),
                    selected: 0,
                });
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_title_input(&mut self, key: KeyCode) -> Result<()> {
        match key {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
                self.state.pending_task_title.clear();
                self.state.editing_task_id = None;
            }
            KeyCode::Enter => {
                if !self.state.input_buffer.is_empty() {
                    // Save title and move to description input
                    self.state.pending_task_title = self.state.input_buffer.clone();

                    // If editing, pre-fill description
                    if let Some(task_id) = &self.state.editing_task_id {
                        if let Some(db) = &self.state.db {
                            if let Ok(Some(task)) = db.get_task(task_id) {
                                self.state.input_buffer = task.description.unwrap_or_default();
                            } else {
                                self.state.input_buffer.clear();
                            }
                        } else {
                            self.state.input_buffer.clear();
                        }
                    } else {
                        self.state.input_buffer.clear();
                    }

                    self.state.input_mode = InputMode::InputDescription;
                }
            }
            KeyCode::Backspace => {
                self.state.input_buffer.pop();
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.push(c);
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_description_input(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Handle file search mode if active
        if let Some(ref mut search) = self.state.file_search {
            match key.code {
                KeyCode::Esc => {
                    // Cancel file search
                    self.state.file_search = None;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    // Select current match
                    if let Some(selected_file) = search.matches.get(search.selected).cloned() {
                        // Remove the #pattern from input and insert the file path
                        self.state.input_buffer.truncate(search.start_pos);
                        self.state.input_buffer.push_str(&selected_file);
                    }
                    self.state.file_search = None;
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                KeyCode::Down | KeyCode::Char('j') => {
                    if search.selected < search.matches.len().saturating_sub(1) {
                        search.selected += 1;
                    }
                }
                KeyCode::Backspace => {
                    if search.pattern.is_empty() {
                        // Cancel search if pattern is empty
                        self.state.input_buffer.pop(); // Remove the #
                        self.state.file_search = None;
                    } else {
                        search.pattern.pop();
                        self.state.input_buffer.pop();
                        self.update_file_search_matches();
                    }
                }
                KeyCode::Char(c) => {
                    search.pattern.push(c);
                    self.state.input_buffer.push(c);
                    self.update_file_search_matches();
                }
                _ => {}
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
                self.state.pending_task_title.clear();
                self.state.editing_task_id = None;
            }
            KeyCode::Enter => {
                // Check if line ends with backslash for line continuation
                if self.state.input_buffer.ends_with('\\') {
                    // Remove backslash and insert newline
                    self.state.input_buffer.pop();
                    self.state.input_buffer.push('\n');
                } else {
                    // Save task (create or update)
                    self.save_task()?;
                    self.state.input_mode = InputMode::Normal;
                    self.state.input_buffer.clear();
                    self.state.pending_task_title.clear();
                    self.state.editing_task_id = None;
                }
            }
            KeyCode::Backspace => {
                self.state.input_buffer.pop();
            }
            KeyCode::Char('#') => {
                // Start file search
                let start_pos = self.state.input_buffer.len();
                self.state.input_buffer.push('#');
                self.state.file_search = Some(FileSearchState {
                    pattern: String::new(),
                    matches: vec![],
                    selected: 0,
                    start_pos,
                });
                self.update_file_search_matches();
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.push(c);
            }
            _ => {}
        }
        Ok(())
    }

    fn update_file_search_matches(&mut self) {
        if let (Some(ref mut search), Some(ref project_path)) = (&mut self.state.file_search, &self.state.project_path) {
            let pattern = &search.pattern;
            search.matches = fuzzy_find_files(project_path, pattern, 10);
            search.selected = 0;
        }
    }

    fn save_task(&mut self) -> Result<()> {
        if let Some(db) = &self.state.db {
            if let Some(task_id) = &self.state.editing_task_id {
                // Editing existing task
                if let Some(mut task) = db.get_task(task_id)? {
                    task.title = self.state.pending_task_title.clone();
                    task.description = if self.state.input_buffer.is_empty() {
                        None
                    } else {
                        Some(self.state.input_buffer.clone())
                    };
                    task.updated_at = chrono::Utc::now();
                    db.update_task(&task)?;
                }
            } else {
                // Creating new task
                let project_id = self.state.project_name.clone();
                let agent = self.state.config.default_agent.clone();

                let mut task = Task::new(&self.state.pending_task_title, agent, project_id);
                if !self.state.input_buffer.is_empty() {
                    task.description = Some(self.state.input_buffer.clone());
                }
                // Task starts in Backlog without tmux window
                db.create_task(&task)?;
            }
            self.refresh_tasks()?;
        }
        Ok(())
    }

    fn delete_selected_task(&mut self) -> Result<()> {
        if let Some(task) = self.state.board.selected_task() {
            if let Some(db) = &self.state.db {
                db.delete_task(&task.id)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn move_task_right(&mut self) -> Result<()> {
        if let (Some(task), Some(project_path)) = (
            self.state.board.selected_task_mut(),
            self.state.project_path.clone(),
        ) {
            let current_status = task.status;
            let next_status = match current_status {
                TaskStatus::Backlog => Some(TaskStatus::Planning),
                TaskStatus::Planning => Some(TaskStatus::Running),
                TaskStatus::Running => Some(TaskStatus::Review),
                TaskStatus::Review => Some(TaskStatus::Done),
                TaskStatus::Done => None,
            };

            if let Some(new_status) = next_status {
                // Create worktree and tmux window when moving from Backlog to Planning
                if current_status == TaskStatus::Backlog && new_status == TaskStatus::Planning {
                    // Sanitize title for worktree/window name
                    let title_slug: String = task.title
                        .chars()
                        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                        .take(30)
                        .collect();
                    let title_slug = title_slug.trim_matches('-').to_string();
                    let window_name = format!("task-{}", title_slug);
                    let target = format!("{}:{}", self.state.project_name, window_name);

                    // Create git worktree from main branch
                    let worktree_path = git::create_worktree(&project_path, &title_slug)?;
                    let worktree_path_str = worktree_path.to_string_lossy().to_string();

                    // Build the prompt from task title and description
                    let prompt = if let Some(desc) = &task.description {
                        format!("{}\n\n{}", task.title, desc)
                    } else {
                        task.title.clone()
                    };

                    // Escape single quotes in prompt for shell
                    let escaped_prompt = prompt.replace('\'', "'\"'\"'");

                    // Create tmux window and start Claude Code with initial prompt
                    let claude_cmd = format!("claude --dangerously-skip-permissions '{}'", escaped_prompt);

                    std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["new-window", "-d", "-t", &self.state.project_name, "-n", &window_name])
                        .args(["-c", &worktree_path_str])
                        .args(["sh", "-c", &claude_cmd])
                        .output()?;

                    // Wait briefly for Claude to start and show the bypass warning
                    std::thread::sleep(std::time::Duration::from_millis(500));

                    // Send "2" to select "Yes, I accept" and Enter to confirm
                    std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["send-keys", "-t", &target, "2"])
                        .output()?;
                    std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["send-keys", "-t", &target, "Enter"])
                        .output()?;

                    task.session_name = Some(target);
                    task.worktree_path = Some(worktree_path_str);
                    task.branch_name = Some(format!("task/{}", title_slug));
                }

                // When moving from Planning to Running, approve the plan to start implementation
                if current_status == TaskStatus::Planning && new_status == TaskStatus::Running {
                    if let Some(session_name) = &task.session_name {
                        // Send Enter to approve the plan and start implementing
                        std::process::Command::new("tmux")
                            .args(["-L", tmux::AGENT_SERVER])
                            .args(["send-keys", "-t", session_name, "Enter"])
                            .output()?;
                    }
                }

                // When moving from Running to Review: Show PR confirmation popup
                if current_status == TaskStatus::Running && new_status == TaskStatus::Review {
                    let task_id = task.id.clone();
                    let task_title = task.title.clone();
                    let worktree_path = task.worktree_path.clone();
                    let branch_name = task.branch_name.clone();

                    // Generate PR description using Claude
                    let (pr_title, pr_body) = generate_pr_description(
                        &task_title,
                        worktree_path.as_deref(),
                        branch_name.as_deref(),
                    );

                    // Store task info and show popup
                    self.state.pr_confirm_popup = Some(PrConfirmPopup {
                        task_id,
                        pr_title,
                        pr_body,
                        editing_title: true,
                        generating: false,
                    });
                    // Don't move yet - wait for popup confirmation
                    return Ok(());
                }

                // When moving from Review to Done: Check if PR is merged
                if current_status == TaskStatus::Review && new_status == TaskStatus::Done {
                    if let Some(pr_number) = task.pr_number {
                        if !is_pr_merged(pr_number, &project_path)? {
                            // PR not merged yet - can't move to Done
                            // TODO: Show error message
                            return Ok(());
                        }
                        // PR is merged - cleanup resources
                        // Kill tmux window if exists
                        if let Some(session_name) = &task.session_name {
                            let _ = std::process::Command::new("tmux")
                                .args(["-L", tmux::AGENT_SERVER])
                                .args(["kill-window", "-t", session_name])
                                .output();
                        }

                        // Remove worktree if exists
                        if let Some(worktree) = &task.worktree_path {
                            let _ = std::process::Command::new("git")
                                .current_dir(&project_path)
                                .args(["worktree", "remove", "--force", worktree])
                                .output();
                        }

                        // Delete local branch if exists
                        if let Some(branch) = &task.branch_name {
                            let _ = std::process::Command::new("git")
                                .current_dir(&project_path)
                                .args(["branch", "-D", branch])
                                .output();
                        }

                        task.session_name = None;
                        task.worktree_path = None;
                    } else {
                        // No PR - can't move to Done without PR being merged
                        return Ok(());
                    }
                }

                task.status = new_status;
                task.updated_at = chrono::Utc::now();

                if let Some(db) = &self.state.db {
                    db.update_task(task)?;
                }
            }
        }
        self.refresh_tasks()?;
        Ok(())
    }

    /// Move task from Review back to Running (only allowed transition backwards)
    fn move_review_to_running(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(project_path)) = (&self.state.db, &self.state.project_path) {
            if let Some(mut task) = db.get_task(task_id)? {
                if task.status != TaskStatus::Review {
                    return Ok(());
                }

                // Re-spawn tmux window in existing worktree
                if let (Some(worktree_path), Some(branch_name)) = (&task.worktree_path, &task.branch_name) {
                    let title_slug: String = task.title
                        .chars()
                        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
                        .take(30)
                        .collect();
                    let title_slug = title_slug.trim_matches('-').to_string();
                    let window_name = format!("task-{}", title_slug);
                    let target = format!("{}:{}", self.state.project_name, window_name);

                    // Build prompt with PR feedback context
                    let prompt = format!(
                        "Continue working on: {}\n\nPR #{} needs changes. Please address the feedback.",
                        task.title,
                        task.pr_number.unwrap_or(0)
                    );
                    let escaped_prompt = prompt.replace('\'', "'\"'\"'");

                    // Create tmux window in the existing worktree
                    let claude_cmd = format!("claude --dangerously-skip-permissions '{}'", escaped_prompt);

                    std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["new-window", "-d", "-t", &self.state.project_name, "-n", &window_name])
                        .args(["-c", worktree_path])
                        .args(["sh", "-c", &claude_cmd])
                        .output()?;

                    // Wait briefly for Claude to start and show the bypass warning
                    std::thread::sleep(std::time::Duration::from_millis(500));

                    // Send "2" to select "Yes, I accept" and Enter to confirm
                    std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["send-keys", "-t", &target, "2"])
                        .output()?;
                    std::process::Command::new("tmux")
                        .args(["-L", tmux::AGENT_SERVER])
                        .args(["send-keys", "-t", &target, "Enter"])
                        .output()?;

                    task.session_name = Some(target);
                }

                task.status = TaskStatus::Running;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn open_selected_task(&mut self) -> Result<()> {
        if let Some(task) = self.state.board.selected_task() {
            if let Some(window_name) = &task.session_name.clone() {
                // Open shell popup to view/control the detached tmux window
                self.state.shell_popup = Some(ShellPopup {
                    task_title: task.title.clone(),
                    window_name: window_name.clone(),
                    scroll_offset: 0,
                });
            }
        }
        Ok(())
    }

    fn refresh_tasks(&mut self) -> Result<()> {
        if let Some(db) = &self.state.db {
            self.state.board.tasks = db.get_all_tasks()?;
        }
        Ok(())
    }

    fn refresh_projects(&mut self) -> Result<()> {
        // Load projects from global database
        let db_projects = self.state.global_db.get_all_projects()?;

        self.state.projects = db_projects
            .into_iter()
            .map(|p| ProjectInfo {
                name: p.name,
                path: p.path,
            })
            .collect();

        // Sort alphabetically by name (case-insensitive)
        self.state
            .projects
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));

        // Find current project in list and select it
        if let Some(project_path) = &self.state.project_path {
            let current_path = project_path.to_string_lossy();
            if let Some(pos) = self.state.projects.iter().position(|p| p.path == current_path) {
                self.state.selected_project = pos;
            }
        }

        Ok(())
    }

    fn refresh_sessions(&mut self) -> Result<()> {
        // TODO: Periodically check tmux sessions and update task status
        Ok(())
    }

    fn switch_to_project(&mut self, project: &ProjectInfo) -> Result<()> {
        self.switch_to_project_keep_sidebar(project)?;
        // Unfocus sidebar
        self.state.sidebar_focused = false;
        Ok(())
    }

    fn switch_to_project_keep_sidebar(&mut self, project: &ProjectInfo) -> Result<()> {
        let project_path = PathBuf::from(&project.path);

        // Check if project path exists
        if !project_path.exists() {
            // Skip non-existent projects silently
            return Ok(());
        }

        // Update current project
        self.state.project_name = project.name.clone();
        self.state.project_path = Some(project_path.clone());

        // Open project database (create if needed)
        match Database::open_project(&project_path) {
            Ok(db) => self.state.db = Some(db),
            Err(_) => {
                // If we can't open the db, skip this project
                return Ok(());
            }
        }

        // Update last_opened in global db
        let proj = crate::db::Project::new(&project.name, &project.path);
        let _ = self.state.global_db.upsert_project(&proj);

        // Ensure tmux session exists
        ensure_project_tmux_session(&project.name, &project_path);

        // Reload tasks for new project
        self.refresh_tasks()?;

        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(self.terminal.backend_mut(), LeaveAlternateScreen);
    }
}

/// Ensure tmux session exists for a project
fn ensure_project_tmux_session(project_name: &str, project_path: &Path) {
    // Check if session already exists
    let session_exists = std::process::Command::new("tmux")
        .args(["-L", tmux::AGENT_SERVER])
        .args(["has-session", "-t", project_name])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    if !session_exists {
        // Create detached session for this project
        let _ = std::process::Command::new("tmux")
            .args(["-L", tmux::AGENT_SERVER])
            .args(["new-session", "-d", "-s", project_name])
            .args(["-c", &project_path.to_string_lossy().to_string()])
            .output();
    }
}

/// Helper function to create a centered rect
fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Capture content from a tmux pane (with ANSI escape sequences)
fn capture_tmux_pane(window_name: &str) -> Vec<u8> {
    std::process::Command::new("tmux")
        .args(["-L", tmux::AGENT_SERVER])
        .args(["capture-pane", "-t", window_name, "-p", "-e"])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default()
}

/// Capture content from a tmux pane with history (with ANSI escape sequences)
fn capture_tmux_pane_with_history(window_name: &str, history_lines: i32) -> Vec<u8> {
    std::process::Command::new("tmux")
        .args(["-L", tmux::AGENT_SERVER])
        .args(["capture-pane", "-t", window_name, "-p", "-e"])
        .args(["-S", &(-history_lines).to_string()])
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default()
}

/// Check if a PR is merged
fn is_pr_merged(pr_number: i32, project_path: &Path) -> Result<bool> {
    let output = std::process::Command::new("gh")
        .current_dir(project_path)
        .args(["pr", "view", &pr_number.to_string(), "--json", "state"])
        .output()?;

    if !output.status.success() {
        return Ok(false);
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(stdout.contains("MERGED"))
}

/// Generate PR title and description using Claude
fn generate_pr_description(task_title: &str, worktree_path: Option<&str>, branch_name: Option<&str>) -> (String, String) {
    // Default values
    let default_title = task_title.to_string();
    let mut default_body = String::new();

    // Try to get git diff for context
    if let Some(worktree) = worktree_path {
        // Get diff from main
        let diff_output = std::process::Command::new("git")
            .current_dir(worktree)
            .args(["diff", "main", "--stat"])
            .output();

        if let Ok(output) = diff_output {
            let diff_stat = String::from_utf8_lossy(&output.stdout);
            if !diff_stat.is_empty() {
                default_body.push_str("## Changes\n```\n");
                default_body.push_str(&diff_stat);
                default_body.push_str("```\n");
            }
        }

        // Try to use Claude to generate a better description
        let prompt = format!(
            "Generate a concise PR description for these changes. Task: '{}'. Output only the description, no markdown code blocks around it. Keep it brief (2-3 sentences max).",
            task_title
        );

        let claude_output = std::process::Command::new("claude")
            .current_dir(worktree)
            .args(["--print", &prompt])
            .output();

        if let Ok(output) = claude_output {
            if output.status.success() {
                let generated = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if !generated.is_empty() {
                    default_body = format!("{}\n\n{}", generated, default_body);
                }
            }
        }
    }

    (default_title, default_body)
}

/// Create a PR with provided title and body, return (pr_number, pr_url)
fn create_pr_with_content(task: &Task, project_path: &Path, pr_title: &str, pr_body: &str) -> Result<(i32, String)> {
    // First push the branch
    if let Some(branch) = &task.branch_name {
        let worktree = task.worktree_path.as_deref().unwrap_or(".");
        std::process::Command::new("git")
            .current_dir(worktree)
            .args(["push", "-u", "origin", branch])
            .output()?;
    }

    // Create PR
    let output = std::process::Command::new("gh")
        .current_dir(project_path)
        .args([
            "pr", "create",
            "--title", pr_title,
            "--body", pr_body,
            "--head", task.branch_name.as_deref().unwrap_or(""),
        ])
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("Failed to create PR: {}", stderr);
    }

    let pr_url = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Get PR number from URL
    let pr_number = pr_url
        .split('/')
        .last()
        .and_then(|s| s.parse::<i32>().ok())
        .unwrap_or(0);

    Ok((pr_number, pr_url))
}

/// Send a key to a tmux pane
fn send_key_to_tmux(window_name: &str, key: KeyCode) {
    let key_str = match key {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Backspace => "BSpace".to_string(),
        KeyCode::Tab => "Tab".to_string(),
        KeyCode::Up => "Up".to_string(),
        KeyCode::Down => "Down".to_string(),
        KeyCode::Left => "Left".to_string(),
        KeyCode::Right => "Right".to_string(),
        KeyCode::Home => "Home".to_string(),
        KeyCode::End => "End".to_string(),
        KeyCode::PageUp => "PageUp".to_string(),
        KeyCode::PageDown => "PageDown".to_string(),
        KeyCode::Delete => "DC".to_string(),
        KeyCode::Insert => "IC".to_string(),
        KeyCode::F(n) => format!("F{}", n),
        _ => return,
    };

    let _ = std::process::Command::new("tmux")
        .args(["-L", tmux::AGENT_SERVER])
        .args(["send-keys", "-t", window_name, &key_str])
        .output();
}

/// Parse ANSI escape sequences to ratatui Lines with colors
fn parse_ansi_to_lines(bytes: &[u8]) -> Vec<Line<'static>> {
    let text = String::from_utf8_lossy(bytes);
    let mut lines: Vec<Line<'static>> = Vec::new();
    let mut current_style = Style::default();

    for line_str in text.lines() {
        let mut spans: Vec<Span<'static>> = Vec::new();
        let mut current_text = String::new();
        let mut chars = line_str.chars().peekable();

        while let Some(c) = chars.next() {
            if c == '\x1b' {
                // Start of escape sequence
                if !current_text.is_empty() {
                    spans.push(Span::styled(current_text.clone(), current_style));
                    current_text.clear();
                }

                // Parse escape sequence
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    let mut seq = String::new();
                    while let Some(&ch) = chars.peek() {
                        if ch.is_ascii_digit() || ch == ';' {
                            seq.push(chars.next().unwrap());
                        } else {
                            break;
                        }
                    }
                    // Get the final character
                    if let Some(final_char) = chars.next() {
                        if final_char == 'm' {
                            // SGR sequence - parse color codes
                            current_style = parse_sgr(&seq, current_style);
                        }
                    }
                }
            } else {
                current_text.push(c);
            }
        }

        if !current_text.is_empty() {
            spans.push(Span::styled(current_text, current_style));
        }

        if spans.is_empty() {
            lines.push(Line::from(""));
        } else {
            lines.push(Line::from(spans));
        }
    }

    lines
}

/// Parse SGR (Select Graphic Rendition) codes
fn parse_sgr(seq: &str, mut style: Style) -> Style {
    if seq.is_empty() {
        return Style::default();
    }

    let codes: Vec<u8> = seq
        .split(';')
        .filter_map(|s| s.parse().ok())
        .collect();

    let mut i = 0;
    while i < codes.len() {
        match codes[i] {
            0 => style = Style::default(),
            1 => style = style.bold(),
            2 => style = style.dim(),
            3 => style = style.italic(),
            4 => style = style.underlined(),
            7 => style = style.reversed(),
            // Foreground colors
            30 => style = style.fg(Color::Black),
            31 => style = style.fg(Color::Red),
            32 => style = style.fg(Color::Green),
            33 => style = style.fg(Color::Yellow),
            34 => style = style.fg(Color::Blue),
            35 => style = style.fg(Color::Magenta),
            36 => style = style.fg(Color::Cyan),
            37 => style = style.fg(Color::Gray),
            39 => style = style.fg(Color::Reset),
            90 => style = style.fg(Color::DarkGray),
            91 => style = style.fg(Color::LightRed),
            92 => style = style.fg(Color::LightYellow),
            93 => style = style.fg(Color::LightYellow),
            94 => style = style.fg(Color::LightBlue),
            95 => style = style.fg(Color::LightMagenta),
            96 => style = style.fg(Color::LightCyan),
            97 => style = style.fg(Color::White),
            // Background colors
            40 => style = style.bg(Color::Black),
            41 => style = style.bg(Color::Red),
            42 => style = style.bg(Color::Green),
            43 => style = style.bg(Color::Yellow),
            44 => style = style.bg(Color::Blue),
            45 => style = style.bg(Color::Magenta),
            46 => style = style.bg(Color::Cyan),
            47 => style = style.bg(Color::Gray),
            49 => style = style.bg(Color::Reset),
            100 => style = style.bg(Color::DarkGray),
            101 => style = style.bg(Color::LightRed),
            102 => style = style.bg(Color::LightYellow),
            103 => style = style.bg(Color::LightYellow),
            104 => style = style.bg(Color::LightBlue),
            105 => style = style.bg(Color::LightMagenta),
            106 => style = style.bg(Color::LightCyan),
            107 => style = style.bg(Color::White),
            // 256-color mode: 38;5;n or 48;5;n
            38 if i + 2 < codes.len() && codes[i + 1] == 5 => {
                style = style.fg(Color::Indexed(codes[i + 2]));
                i += 2;
            }
            48 if i + 2 < codes.len() && codes[i + 1] == 5 => {
                style = style.bg(Color::Indexed(codes[i + 2]));
                i += 2;
            }
            // RGB mode: 38;2;r;g;b or 48;2;r;g;b
            38 if i + 4 < codes.len() && codes[i + 1] == 2 => {
                style = style.fg(Color::Rgb(codes[i + 2], codes[i + 3], codes[i + 4]));
                i += 4;
            }
            48 if i + 4 < codes.len() && codes[i + 1] == 2 => {
                style = style.bg(Color::Rgb(codes[i + 2], codes[i + 3], codes[i + 4]));
                i += 4;
            }
            _ => {}
        }
        i += 1;
    }

    style
}

/// Fuzzy find files in a directory (respects .gitignore)
fn fuzzy_find_files(project_path: &Path, pattern: &str, max_results: usize) -> Vec<String> {
    use std::process::Command;

    // Use git ls-files to get tracked files (respects .gitignore)
    let output = Command::new("git")
        .current_dir(project_path)
        .args(["ls-files", "--cached", "--others", "--exclude-standard"])
        .output();

    if let Ok(output) = output {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let files: Vec<String> = stdout.lines().map(String::from).collect();

        if pattern.is_empty() {
            // Show first N files when pattern is empty
            return files.into_iter().take(max_results).collect();
        }

        let pattern_lower = pattern.to_lowercase();
        let mut matches: Vec<(String, i32)> = files
            .into_iter()
            .filter_map(|path| {
                let path_lower = path.to_lowercase();

                // Simple fuzzy matching: check if all pattern chars appear in order
                let score = fuzzy_score(&path_lower, &pattern_lower);
                if score > 0 {
                    Some((path, score))
                } else {
                    None
                }
            })
            .collect();

        // Sort by score (higher is better)
        matches.sort_by(|a, b| b.1.cmp(&a.1));

        return matches.into_iter().take(max_results).map(|(path, _)| path).collect();
    }

    vec![]
}

/// Calculate fuzzy match score (higher is better, 0 means no match)
fn fuzzy_score(haystack: &str, needle: &str) -> i32 {
    if needle.is_empty() {
        return 1;
    }

    let mut score = 0;
    let mut needle_chars = needle.chars().peekable();
    let mut prev_matched = false;
    let mut prev_was_separator = true;

    for c in haystack.chars() {
        let is_separator = c == '/' || c == '_' || c == '-' || c == '.';

        if let Some(&nc) = needle_chars.peek() {
            if c == nc {
                needle_chars.next();
                score += 1;

                // Bonus for matching after separator (start of word)
                if prev_was_separator {
                    score += 5;
                }
                // Bonus for consecutive matches
                if prev_matched {
                    score += 3;
                }
                prev_matched = true;
            } else {
                prev_matched = false;
            }
        }

        prev_was_separator = is_separator;
    }

    // Only return score if all needle chars were found
    if needle_chars.peek().is_none() {
        score
    } else {
        0
    }
}
