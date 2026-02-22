use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};
use std::collections::HashSet;
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc};

use crate::agent::{self, AgentOperations};
use crate::config::{GlobalConfig, MergedConfig, ProjectConfig, ThemeConfig};
use crate::db::{Database, Task, TaskStatus};
use crate::git::{self, GitOperations, GitProviderOperations, PullRequestState, RealGitHubOps, RealGitOps};
use crate::tmux::{self, RealTmuxOps, TmuxOperations};
use crate::AppMode;

use super::board::BoardState;
use super::input::InputMode;
use super::shell_popup::{self, ShellPopup};

/// Helper to convert hex color string to ratatui Color
fn hex_to_color(hex: &str) -> Color {
    ThemeConfig::parse_hex(hex)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .unwrap_or(Color::White)
}

/// Build footer help text based on current UI state
fn build_footer_text(input_mode: InputMode, sidebar_focused: bool, selected_column: usize) -> String {
    match input_mode {
        InputMode::Normal => {
            if sidebar_focused {
                " [j/k] navigate  [Enter] open  [l] board  [e] hide sidebar  [q] quit ".to_string()
            } else {
                match selected_column {
                    0 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] plan  [M] run  [e] sidebar  [q] quit".to_string(),
                    1 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] run  [e] sidebar  [q] quit".to_string(),
                    2 | 3 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] move  [r] move left  [e] sidebar  [q] quit".to_string(),
                    _ => " [o] new  [/] search  [Enter] open  [x] del  [e] sidebar  [q] quit".to_string(),
                }
            }
        }
        InputMode::InputTitle => " Enter task title... [Esc] cancel [Enter] next ".to_string(),
        InputMode::InputDescription => " Enter prompt for agent... [#] file search [Esc] cancel [\\+Enter] newline [Enter] save ".to_string(),
    }
}

type Terminal = ratatui::Terminal<CrosstermBackend<Stdout>>;

/// Shell popup dimensions - used for both rendering and tmux window sizing
const SHELL_POPUP_WIDTH: u16 = 82;          // Total width including borders
const SHELL_POPUP_CONTENT_WIDTH: u16 = 80;  // Content width (SHELL_POPUP_WIDTH - 2 for borders)
const SHELL_POPUP_HEIGHT_PERCENT: u16 = 75; // Percentage of terminal height

/// Application state (separate from terminal for borrow checker)
struct AppState {
    mode: AppMode,
    should_quit: bool,
    board: BoardState,
    input_mode: InputMode,
    input_buffer: String,
    input_cursor: usize, // Cursor position in input_buffer
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
    // Tmux operations (injectable for testing)
    tmux_ops: Arc<dyn TmuxOperations>,
    // Git operations (injectable for testing)
    git_ops: Arc<dyn git::GitOperations>,
    // Git provider operations (injectable for testing)
    git_provider_ops: Arc<dyn GitProviderOperations>,
    // Agent registry (injectable for testing)
    agent_registry: Arc<dyn agent::AgentRegistry>,
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
    // File paths inserted via file search (for highlighting)
    highlighted_file_paths: HashSet<String>,
    // Task search popup
    task_search: Option<TaskSearchState>,
    // PR creation confirmation popup
    pr_confirm_popup: Option<PrConfirmPopup>,
    // Moving Review back to Running
    review_to_running_task_id: Option<String>,
    // Git diff popup
    diff_popup: Option<DiffPopup>,
    // Channel for receiving PR description generation results
    pr_generation_rx: Option<mpsc::Receiver<(String, String)>>,
    // PR creation status popup
    pr_status_popup: Option<PrStatusPopup>,
    // Channel for receiving PR creation results
    pr_creation_rx: Option<mpsc::Receiver<Result<(i32, String), String>>>,
    // Confirmation popup for moving to Done with open PR
    done_confirm_popup: Option<DoneConfirmPopup>,
    // Confirmation popup for deleting a task
    delete_confirm_popup: Option<DeleteConfirmPopup>,
    // Confirmation popup for asking if user wants to create PR when moving to Review
    review_confirm_popup: Option<ReviewConfirmPopup>,
}

/// State for confirming move to Done
#[derive(Debug, Clone)]
struct DoneConfirmPopup {
    task_id: String,
    pr_number: i32,
    pr_state: DoneConfirmPrState,
}

#[derive(Debug, Clone)]
enum DoneConfirmPrState {
    Open,
    Merged,
    Closed,
    Unknown,
}

/// State for PR creation status popup (loading/success/error)
#[derive(Debug, Clone)]
struct PrStatusPopup {
    status: PrCreationStatus,
    pr_url: Option<String>,
    error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
enum PrCreationStatus {
    Creating,
    Pushing, // Pushing to existing PR
    Success,
    Error,
}

/// State for git diff popup
#[derive(Debug, Clone)]
struct DiffPopup {
    task_title: String,
    diff_content: String,
    scroll_offset: usize,
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
    generating: bool,    // true while agent is generating description
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
    start_pos: usize,   // Position in input_buffer where trigger was typed
    trigger_char: char,  // The character that triggered the search (# or @)
}

/// State for delete confirmation popup
#[derive(Debug, Clone)]
struct DeleteConfirmPopup {
    task_id: String,
    task_title: String,
}

/// State for asking if user wants to create PR when moving to Review
#[derive(Debug, Clone)]
struct ReviewConfirmPopup {
    task_id: String,
    task_title: String,
}

pub struct App {
    terminal: Terminal,
    state: AppState,
}

impl App {
    pub fn new(mode: AppMode) -> Result<Self> {
        Self::with_ops(
            mode,
            Arc::new(RealTmuxOps),
            Arc::new(RealGitOps),
            Arc::new(RealGitHubOps),
            Arc::new(agent::RealAgentRegistry::new("claude")),
        )
    }

    pub fn with_ops(
        mode: AppMode,
        tmux_ops: Arc<dyn TmuxOperations>,
        git_ops: Arc<dyn GitOperations>,
        git_provider_ops: Arc<dyn GitProviderOperations>,
        agent_registry: Arc<dyn agent::AgentRegistry>,
    ) -> Result<Self> {
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
                ensure_project_tmux_session(&name, &canonical, tmux_ops.as_ref());

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
                input_cursor: 0,
                pending_task_title: String::new(),
                editing_task_id: None,
                db,
                global_db,
                config,
                project_path,
                project_name: project_name.clone(),
                available_agents,
                tmux_ops,
                git_ops,
                git_provider_ops,
                agent_registry,
                sidebar_visible: true,
                sidebar_focused: false,
                projects: vec![],
                selected_project: 0,
                show_project_list: false,
                shell_popup: None,
                file_search: None,
                highlighted_file_paths: HashSet::new(),
                task_search: None,
                pr_confirm_popup: None,
                review_to_running_task_id: None,
                diff_popup: None,
                pr_generation_rx: None,
                pr_status_popup: None,
                pr_creation_rx: None,
                done_confirm_popup: None,
                delete_confirm_popup: None,
                review_confirm_popup: None,
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

            // Check for PR generation completion
            if let Some(ref rx) = self.state.pr_generation_rx {
                if let Ok((pr_title, pr_body)) = rx.try_recv() {
                    if let Some(ref mut popup) = self.state.pr_confirm_popup {
                        popup.pr_title = pr_title;
                        popup.pr_body = pr_body;
                        popup.generating = false;
                    }
                    self.state.pr_generation_rx = None;
                }
            }

            // Check for PR creation completion
            if let Some(ref rx) = self.state.pr_creation_rx {
                if let Ok(result) = rx.try_recv() {
                    match result {
                        Ok((_, pr_url)) => {
                            self.state.pr_status_popup = Some(PrStatusPopup {
                                status: PrCreationStatus::Success,
                                pr_url: Some(pr_url),
                                error_message: None,
                            });
                        }
                        Err(err) => {
                            self.state.pr_status_popup = Some(PrStatusPopup {
                                status: PrCreationStatus::Error,
                                pr_url: None,
                                error_message: Some(err),
                            });
                        }
                    }
                    self.state.pr_creation_rx = None;
                    self.refresh_tasks()?;
                }
            }

            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key)?;
                    }
                }
            }

            // Refresh shell popup content periodically (every poll cycle when open)
            if let Some(ref mut popup) = self.state.shell_popup {
                popup.cached_content = capture_tmux_pane_with_history(&popup.window_name, 500, self.state.tmux_ops.as_ref());
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
            let (border_style, title_style) = if is_selected_column {
                (
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
                )
            } else {
                (
                    Style::default().fg(hex_to_color(&state.config.theme.color_normal)),
                    Style::default().fg(hex_to_color(&state.config.theme.color_column_header)),
                )
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
                .title_style(title_style)
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
                    let style = Style::default().fg(hex_to_color(&state.config.theme.color_dimmed));
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
        let footer_text = build_footer_text(state.input_mode, state.sidebar_focused, state.board.selected_column);

        let footer = Paragraph::new(footer_text.as_str())
            .style(Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)))
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
            // Insert cursor (█) at the correct position
            let (before_cursor, after_cursor) = state.input_buffer.split_at(
                state.input_cursor.min(state.input_buffer.len())
            );
            let text_color = hex_to_color(&state.config.theme.color_text);
            let highlight_color = hex_to_color(&state.config.theme.color_accent);
            let full_text = if state.input_mode == InputMode::InputDescription {
                format!(
                    "Title: {}\n\n{}{}█{}",
                    state.pending_task_title,
                    label,
                    before_cursor,
                    after_cursor
                )
            } else {
                format!("{}{}█{}", label, before_cursor, after_cursor)
            };

            let styled_text = if state.input_mode == InputMode::InputDescription && !state.highlighted_file_paths.is_empty() {
                build_highlighted_text(&full_text, &state.highlighted_file_paths, text_color, highlight_color)
            } else {
                Text::raw(full_text)
            };

            let input = Paragraph::new(styled_text)
                .style(Style::default().fg(text_color))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(title)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_selected))),
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

                    let file_selected_color = hex_to_color(&state.config.theme.color_selected);
                    let items: Vec<ListItem> = search.matches
                        .iter()
                        .enumerate()
                        .map(|(i, path)| {
                            let style = if i == search.selected {
                                Style::default().bg(file_selected_color).fg(Color::Black)
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
            Self::draw_shell_popup(popup, frame, area, &state.config.theme);
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

            let selected_color = hex_to_color(&state.config.theme.color_selected);

            // Search input
            let input = Paragraph::new(format!(" 🔍 {}█", search.query))
                .style(Style::default().fg(selected_color))
                .block(
                    Block::default()
                        .title(" Search Tasks ")
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(selected_color)),
                );
            frame.render_widget(input, popup_chunks[0]);

            // Results list
            let items: Vec<ListItem> = search.matches
                .iter()
                .enumerate()
                .map(|(i, (_, title, status))| {
                    let is_selected = i == search.selected;
                    let style = if is_selected {
                        Style::default().bg(selected_color).fg(Color::Black)
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
                        .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_dimmed))),
                );
            frame.render_widget(list, popup_chunks[1]);
        }

        // PR confirmation popup
        if let Some(ref popup) = state.pr_confirm_popup {
            let popup_area = centered_rect(60, 60, area);
            frame.render_widget(Clear, popup_area);

            // Show loading state while generating
            if popup.generating {
                let main_block = Block::default()
                    .title(" Create Pull Request ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_selected)));
                frame.render_widget(main_block, popup_area);

                // Spinner animation based on frame count
                let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let spinner_idx = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() / 100) as usize % spinner_chars.len();
                let spinner = spinner_chars[spinner_idx];

                let agent_name = state.config.default_agent.clone();
                let loading_text = format!("{} Generating PR description with {}...", spinner, agent_name);
                let loading = Paragraph::new(loading_text)
                    .style(Style::default().fg(Color::Cyan))
                    .alignment(ratatui::layout::Alignment::Center);

                // Center vertically within the popup
                let inner = popup_area.inner(ratatui::layout::Margin {
                    horizontal: 2,
                    vertical: popup_area.height.saturating_sub(3) / 2
                });
                frame.render_widget(loading, inner);
            } else {
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
                    .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)));
                frame.render_widget(main_block, popup_area);

                // Title input
                let title_style = if popup.editing_title {
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected))
                } else {
                    Style::default().fg(Color::White)
                };
                let title_border = if popup.editing_title {
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected))
                } else {
                    Style::default().fg(hex_to_color(&state.config.theme.color_dimmed))
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
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected))
                } else {
                    Style::default().fg(Color::White)
                };
                let body_border = if !popup.editing_title {
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected))
                } else {
                    Style::default().fg(hex_to_color(&state.config.theme.color_dimmed))
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
                let help = Paragraph::new(" [Tab] switch field  [Ctrl+s] create PR  [Esc] cancel ")
                    .style(Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)));
                frame.render_widget(help, popup_chunks[2]);
            }
        }

        // PR creation status popup (loading/success/error)
        if let Some(ref popup) = state.pr_status_popup {
            let popup_area = centered_rect(50, 20, area);
            frame.render_widget(Clear, popup_area);

            let (title, border_color) = match popup.status {
                PrCreationStatus::Creating => (" Creating Pull Request ", hex_to_color(&state.config.theme.color_selected)),
                PrCreationStatus::Pushing => (" Pushing Changes ", hex_to_color(&state.config.theme.color_selected)),
                PrCreationStatus::Success => (" Pull Request Created ", Color::Green),
                PrCreationStatus::Error => (" Error Creating PR ", Color::Red),
            };

            let main_block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color));
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 });

            match popup.status {
                PrCreationStatus::Creating => {
                    let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let spinner_idx = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() / 100) as usize % spinner_chars.len();
                    let spinner = spinner_chars[spinner_idx];

                    let text = format!("{} Pushing branch and creating PR...", spinner);
                    let content = Paragraph::new(text)
                        .style(Style::default().fg(Color::Cyan))
                        .alignment(ratatui::layout::Alignment::Center);
                    frame.render_widget(content, inner);
                }
                PrCreationStatus::Pushing => {
                    let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let spinner_idx = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() / 100) as usize % spinner_chars.len();
                    let spinner = spinner_chars[spinner_idx];

                    let text = format!("{} PR exists. Pushing changes...", spinner);
                    let content = Paragraph::new(text)
                        .style(Style::default().fg(Color::Cyan))
                        .alignment(ratatui::layout::Alignment::Center);
                    frame.render_widget(content, inner);
                }
                PrCreationStatus::Success => {
                    let url = popup.pr_url.as_deref().unwrap_or("unknown");
                    // Check if this was a push to existing PR or new PR creation
                    let message = if url.starts_with("http") {
                        format!("Success!\n\n{}\n\n[Enter] to close", url)
                    } else {
                        format!("{}\n\n[Enter] to close", url)
                    };
                    let content = Paragraph::new(message)
                        .style(Style::default().fg(Color::Green))
                        .alignment(ratatui::layout::Alignment::Center);
                    frame.render_widget(content, inner);
                }
                PrCreationStatus::Error => {
                    let err = popup.error_message.as_deref().unwrap_or("Unknown error");
                    let text = format!("Failed to create PR:\n\n{}\n\n[Enter] to close", err);
                    let content = Paragraph::new(text)
                        .style(Style::default().fg(Color::Red))
                        .alignment(ratatui::layout::Alignment::Center)
                        .wrap(Wrap { trim: false });
                    frame.render_widget(content, inner);
                }
            }
        }

        // Done confirmation popup
        if let Some(ref popup) = state.done_confirm_popup {
            let popup_area = centered_rect(50, 25, area);
            frame.render_widget(Clear, popup_area);

            let main_block = Block::default()
                .title(" Move to Done? ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_selected)));
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 });
            let text = match popup.pr_state {
                DoneConfirmPrState::Open => format!(
                    "PR #{} is still open.\n\nAre you sure you want to move this task to Done?\n\nWorktree will be deleted, tmux coding session killed.\nBranch kept locally.\n\n[y] Yes, move to Done    [n/Esc] Cancel",
                    popup.pr_number
                ),
                DoneConfirmPrState::Merged => format!(
                    "PR #{} was merged.\n\nWorktree will be deleted, tmux coding session killed.\nBranch kept locally.\n\n[y] Yes, move to Done    [n/Esc] Cancel",
                    popup.pr_number
                ),
                DoneConfirmPrState::Closed => format!(
                    "PR #{} was closed.\n\nWorktree will be deleted, tmux coding session killed.\nBranch kept locally.\n\n[y] Yes, move to Done    [n/Esc] Cancel",
                    popup.pr_number
                ),
                DoneConfirmPrState::Unknown => format!(
                    "PR #{} state unknown.\n\nAre you sure you want to move this task to Done?\n\nWorktree will be deleted, tmux coding session killed.\nBranch kept locally.\n\n[y] Yes, move to Done    [n/Esc] Cancel",
                    popup.pr_number
                ),
            };
            let content = Paragraph::new(text)
                .style(Style::default().fg(Color::White))
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: false });
            frame.render_widget(content, inner);
        }

        // Delete confirmation popup
        if let Some(ref popup) = state.delete_confirm_popup {
            let popup_area = centered_rect(50, 25, area);
            frame.render_widget(Clear, popup_area);

            let main_block = Block::default()
                .title(" Delete Task? ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Red));
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 });
            let text = format!(
                "Are you sure you want to delete:\n\n\"{}\"\n\nThis will also remove the worktree and tmux session.\n\n[y] Yes, delete    [n/Esc] Cancel",
                popup.task_title
            );
            let content = Paragraph::new(text)
                .style(Style::default().fg(Color::White))
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: false });
            frame.render_widget(content, inner);
        }

        // Review confirmation popup (ask if user wants to create PR)
        if let Some(ref popup) = state.review_confirm_popup {
            let popup_area = centered_rect(50, 25, area);
            frame.render_widget(Clear, popup_area);

            let main_block = Block::default()
                .title(" Move to Review ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)));
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin { horizontal: 2, vertical: 2 });
            let text = format!(
                "Moving task to Review:\n\n\"{}\"\n\nDo you want to create a Pull Request?\n\n[y] Yes, create PR    [n] No, just move    [Esc] Cancel",
                popup.task_title
            );
            let content = Paragraph::new(text)
                .style(Style::default().fg(Color::White))
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: false });
            frame.render_widget(content, inner);
        }

        // Git diff popup
        if let Some(ref popup) = state.diff_popup {
            let popup_area = centered_rect(80, 80, area);
            frame.render_widget(Clear, popup_area);

            let popup_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(1), // Title bar
                    Constraint::Min(0),    // Diff content
                    Constraint::Length(1), // Footer
                ])
                .split(popup_area);

            // Title bar
            let title = format!(" Diff: {} ", popup.task_title);
            let title_bar = Paragraph::new(title)
                .style(Style::default().fg(Color::Black).bg(hex_to_color(&state.config.theme.color_popup_header)));
            frame.render_widget(title_bar, popup_chunks[0]);

            // Diff content with syntax highlighting
            let lines: Vec<Line> = popup.diff_content
                .lines()
                .skip(popup.scroll_offset)
                .take(popup_chunks[1].height.saturating_sub(2) as usize)
                .map(|line| {
                    let style = if line.starts_with('+') && !line.starts_with("+++") {
                        Style::default().fg(Color::Green)
                    } else if line.starts_with('-') && !line.starts_with("---") {
                        Style::default().fg(Color::Red)
                    } else if line.starts_with("@@") {
                        Style::default().fg(Color::Cyan)
                    } else if line.starts_with("diff ") || line.starts_with("index ") {
                        Style::default().fg(hex_to_color(&state.config.theme.color_selected))
                    } else {
                        Style::default().fg(Color::White)
                    };
                    Line::from(Span::styled(line, style))
                })
                .collect();

            let diff_content = Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_popup_border))),
                );
            frame.render_widget(diff_content, popup_chunks[1]);

            // Footer with scroll info
            let total_lines = popup.diff_content.lines().count();
            let footer_text = format!(
                " [j/k] scroll  [d/u] page  [g/G] top/bottom  [q/Esc] close  ({}/{}) ",
                popup.scroll_offset + 1,
                total_lines
            );
            let footer = Paragraph::new(footer_text)
                .style(Style::default().fg(Color::Black).bg(hex_to_color(&state.config.theme.color_dimmed)));
            frame.render_widget(footer, popup_chunks[2]);
        }
    }

    fn draw_shell_popup(popup: &ShellPopup, frame: &mut Frame, area: Rect, theme: &ThemeConfig) {
        let popup_area = centered_rect_fixed_width(SHELL_POPUP_WIDTH, SHELL_POPUP_HEIGHT_PERCENT, area);

        // Parse ANSI escape sequences for colors
        let styled_lines = parse_ansi_to_lines(&popup.cached_content);

        // Build colors from theme
        let colors = shell_popup::ShellPopupColors {
            border: hex_to_color(&theme.color_popup_border),
            header_fg: Color::Black,
            header_bg: hex_to_color(&theme.color_popup_header),
            footer_fg: Color::Black,
            footer_bg: hex_to_color(&theme.color_dimmed),
        };

        shell_popup::render_shell_popup(popup, frame, popup_area, styled_lines, &colors);
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

        // Truncate title to fit (char-safe for UTF-8)
        let max_title_len = area.width.saturating_sub(4) as usize;
        let title: String = if task.title.chars().count() > max_title_len {
            let truncated: String = task.title.chars().take(max_title_len.saturating_sub(3)).collect();
            format!("{}...", truncated)
        } else {
            task.title.clone()
        };

        let border_type = if is_selected {
            BorderType::Thick
        } else {
            BorderType::Plain
        };

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .border_type(border_type);
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

        // Preview area (below title) - always show description
        if inner.height > 1 {
            let preview_area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1),
            };

            // Show description or placeholder
            let preview_text = task.description.as_deref().unwrap_or("No description");

            // Truncate description to fit preview area
            let max_chars = (preview_area.width as usize) * (preview_area.height as usize);
            let truncated: String = if preview_text.chars().count() > max_chars {
                format!("{}...", preview_text.chars().take(max_chars.saturating_sub(3)).collect::<String>())
            } else {
                preview_text.to_string()
            };

            let preview = Paragraph::new(truncated)
                .style(Style::default().fg(hex_to_color(&theme.color_description)).italic())
                .wrap(Wrap { trim: true });
            frame.render_widget(preview, preview_area);
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
                    Style::default().bg(hex_to_color(&state.config.theme.color_selected)).fg(Color::Black)
                } else if is_current {
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected))
                } else {
                    Style::default().fg(hex_to_color(&state.config.theme.color_text))
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

        let dimmed_color = hex_to_color(&state.config.theme.color_dimmed);
        let selected_color = hex_to_color(&state.config.theme.color_selected);

        // Message
        let message = Paragraph::new("\n  No project found in current directory.\n")
            .style(Style::default().fg(selected_color))
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(dimmed_color)));
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
                        Style::default().bg(dimmed_color).fg(Color::White)
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
                    .border_style(Style::default().fg(selected_color)),
            );
            frame.render_widget(list, content_chunks[1]);
        } else {
            // Show options
            let options = Paragraph::new(
                "\n  [p] Open existing project\n  [n] Create new project in current directory\n",
            )
            .block(Block::default().title(" Options ").borders(Borders::ALL).border_style(Style::default().fg(dimmed_color)));
            frame.render_widget(options, content_chunks[1]);
        }

        // Footer
        let footer = Paragraph::new(" [p] projects  [n] new project  [q] quit ")
            .style(Style::default().fg(dimmed_color))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }

    fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Handle PR status popup if open (loading/success/error)
        if let Some(ref popup) = self.state.pr_status_popup {
            // Only allow closing if not in Creating/Pushing state
            if popup.status != PrCreationStatus::Creating && popup.status != PrCreationStatus::Pushing {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                    self.state.pr_status_popup = None;
                }
            }
            return Ok(());
        }

        // Handle Done confirmation popup if open
        if self.state.done_confirm_popup.is_some() {
            return self.handle_done_confirm_key(key);
        }

        // Handle Delete confirmation popup if open
        if self.state.delete_confirm_popup.is_some() {
            return self.handle_delete_confirm_key(key);
        }

        // Handle Review confirmation popup if open
        if self.state.review_confirm_popup.is_some() {
            return self.handle_review_confirm_key(key);
        }

        // Handle diff popup if open
        if self.state.diff_popup.is_some() {
            return self.handle_diff_popup_key(key);
        }

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
                    InputMode::InputTitle => self.handle_title_input(key),
                    InputMode::InputDescription => self.handle_description_input(key),
                }
            }
        }
    }

    fn handle_done_confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        if let Some(popup) = self.state.done_confirm_popup.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Confirmed - force move to Done
                    self.state.done_confirm_popup = None;
                    self.force_move_to_done(&popup.task_id)?;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Cancelled
                    self.state.done_confirm_popup = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_delete_confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        if let Some(popup) = self.state.delete_confirm_popup.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Confirmed - delete the task
                    self.state.delete_confirm_popup = None;
                    self.perform_delete_task(&popup.task_id)?;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    // Cancelled
                    self.state.delete_confirm_popup = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn handle_review_confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        if let Some(popup) = self.state.review_confirm_popup.clone() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Yes - create PR and move to review
                    self.state.review_confirm_popup = None;
                    self.move_running_to_review_with_pr(&popup.task_id)?;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    // No - just move to review without PR
                    self.state.review_confirm_popup = None;
                    self.move_running_to_review_without_pr(&popup.task_id)?;
                }
                KeyCode::Esc => {
                    // Cancelled - don't move
                    self.state.review_confirm_popup = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn force_move_to_done(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(project_path)) = (&self.state.db, self.state.project_path.clone()) {
            if let Some(mut task) = db.get_task(task_id)? {
                cleanup_task_for_done(
                    &mut task,
                    &project_path,
                    self.state.tmux_ops.as_ref(),
                    self.state.git_ops.as_ref(),
                );
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn move_running_to_review_with_pr(&mut self, task_id: &str) -> Result<()> {
        if let Some(db) = &self.state.db {
            if let Some(task) = db.get_task(task_id)? {
                let task_title = task.title.clone();
                let worktree_path = task.worktree_path.clone();

                // Show popup immediately with loading state
                self.state.pr_confirm_popup = Some(PrConfirmPopup {
                    task_id: task_id.to_string(),
                    pr_title: task_title.clone(),
                    pr_body: String::new(),
                    editing_title: true,
                    generating: true,
                });

                // Spawn background thread to generate PR description
                let (tx, rx) = mpsc::channel();
                self.state.pr_generation_rx = Some(rx);

                let title_for_thread = task_title.clone();
                let worktree_for_thread = worktree_path.clone();
                let git_ops = Arc::clone(&self.state.git_ops);
                let agent_ops = self.state.agent_registry.get(&self.state.config.default_agent);
                std::thread::spawn(move || {
                    let (pr_title, pr_body) = generate_pr_description(
                        &title_for_thread,
                        worktree_for_thread.as_deref(),
                        None,
                        git_ops.as_ref(),
                        agent_ops.as_ref(),
                    );
                    let _ = tx.send((pr_title, pr_body));
                });
            }
        }
        Ok(())
    }

    fn move_running_to_review_without_pr(&mut self, task_id: &str) -> Result<()> {
        if let Some(db) = &self.state.db {
            if let Some(mut task) = db.get_task(task_id)? {
                task.status = TaskStatus::Review;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
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
                KeyCode::Char('s') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if !popup.generating {
                        // Ctrl+s: Submit and create PR
                        let task_id = popup.task_id.clone();
                        let pr_title = popup.pr_title.clone();
                        let pr_body = popup.pr_body.clone();
                        self.state.pr_confirm_popup = None;
                        self.create_pr_and_move_to_review_with_content(&task_id, &pr_title, &pr_body)?;
                    }
                }
                KeyCode::Enter => {
                    if popup.editing_title && !popup.generating {
                        // Enter in title: move to body editing
                        popup.editing_title = false;
                    } else if !popup.generating {
                        // Enter in body: add newline
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
                // Keep tmux window open - session_name stays set for resume

                // Show loading popup
                self.state.pr_status_popup = Some(PrStatusPopup {
                    status: PrCreationStatus::Creating,
                    pr_url: None,
                    error_message: None,
                });

                // Clone data for background thread
                let task_clone = task.clone();
                let project_path_clone = project_path.clone();
                let pr_title_clone = pr_title.to_string();
                let pr_body_clone = pr_body.to_string();
                let git_ops = Arc::clone(&self.state.git_ops);
                let git_provider_ops = Arc::clone(&self.state.git_provider_ops);
                let agent_ops = self.state.agent_registry.get(&self.state.config.default_agent);

                // Create channel for result
                let (tx, rx) = mpsc::channel();
                self.state.pr_creation_rx = Some(rx);

                // Spawn background thread to create PR
                std::thread::spawn(move || {
                    let result = create_pr_with_content(
                        &task_clone,
                        &project_path_clone,
                        &pr_title_clone,
                        &pr_body_clone,
                        git_ops.as_ref(),
                        git_provider_ops.as_ref(),
                        agent_ops.as_ref(),
                    );
                    match result {
                        Ok((pr_number, pr_url)) => {
                            // Update task in database from background thread
                            // Keep session_name so popup can still be opened in Review
                            if let Ok(db) = crate::db::Database::open_project(&project_path_clone) {
                                let mut updated_task = task_clone;
                                updated_task.pr_number = Some(pr_number);
                                updated_task.pr_url = Some(pr_url.clone());
                                updated_task.status = TaskStatus::Review;
                                updated_task.updated_at = chrono::Utc::now();
                                let _ = db.update_task(&updated_task);
                            }
                            let _ = tx.send(Ok((pr_number, pr_url)));
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e.to_string()));
                        }
                    }
                });
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
                // Jump to selected task and open it
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
                // Open the selected task (same as pressing Enter on a task)
                self.open_selected_task()?;
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
            KeyCode::Char('k') | KeyCode::Char('p') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(ref mut search) = self.state.task_search {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                false
            }
            KeyCode::Char('j') | KeyCode::Char('n') if key.modifiers.contains(KeyModifiers::CONTROL) => {
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
                // Ctrl+q = close popup
                KeyCode::Char('q') if has_ctrl => {
                    self.state.shell_popup = None;
                }
                // Scroll up with Ctrl+k or Ctrl+p or Ctrl+Up
                KeyCode::Char('k') | KeyCode::Char('p') | KeyCode::Up if has_ctrl => {
                    popup.scroll_up(5);
                }
                // Scroll down with Ctrl+j or Ctrl+n or Ctrl+Down
                KeyCode::Char('j') | KeyCode::Char('n') | KeyCode::Down if has_ctrl => {
                    popup.scroll_down(5);
                }
                // Page up with Ctrl+u or PageUp
                KeyCode::Char('u') if has_ctrl => {
                    popup.scroll_up(20);
                }
                KeyCode::PageUp => {
                    popup.scroll_up(20);
                }
                // Page down with Ctrl+d or PageDown
                KeyCode::Char('d') if has_ctrl => {
                    popup.scroll_down(20);
                }
                KeyCode::PageDown => {
                    popup.scroll_down(20);
                }
                // Ctrl+g = go to bottom (current)
                KeyCode::Char('g') if has_ctrl => {
                    popup.scroll_to_bottom();
                }
                _ => {
                    // Forward all other keys to tmux window (including Esc)
                    send_key_to_tmux(&window_name, key.code, self.state.tmux_ops.as_ref());
                    // After sending a key, refresh content to show the result
                    popup.cached_content = capture_tmux_pane_with_history(&window_name, 500, self.state.tmux_ops.as_ref());
                }
            }
        }
        Ok(())
    }

    fn handle_diff_popup_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        if let Some(ref mut popup) = self.state.diff_popup {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => {
                    self.state.diff_popup = None;
                }
                KeyCode::Char('j') | KeyCode::Down => {
                    popup.scroll_offset = popup.scroll_offset.saturating_add(1);
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    popup.scroll_offset = popup.scroll_offset.saturating_sub(1);
                }
                KeyCode::Char('d') | KeyCode::PageDown => {
                    popup.scroll_offset = popup.scroll_offset.saturating_add(20);
                }
                KeyCode::Char('u') | KeyCode::PageUp => {
                    popup.scroll_offset = popup.scroll_offset.saturating_sub(20);
                }
                KeyCode::Char('g') => {
                    popup.scroll_offset = 0;
                }
                KeyCode::Char('G') => {
                    // Go to end
                    let line_count = popup.diff_content.lines().count();
                    popup.scroll_offset = line_count.saturating_sub(10);
                }
                _ => {}
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
                        self.state.input_cursor = self.state.input_buffer.len();
                        self.state.pending_task_title.clear();
                        self.state.input_mode = InputMode::InputTitle;
                    } else if task.session_name.is_some() {
                        // Open shell popup
                        self.open_selected_task()?;
                    }
                }
            }
            KeyCode::Char('x') => self.delete_selected_task()?,
            KeyCode::Char('d') => self.show_task_diff()?,
            KeyCode::Char('m') => self.move_task_right()?,
            KeyCode::Char('M') => self.move_backlog_to_running()?,
            KeyCode::Char('r') => {
                if let Some(task) = self.state.board.selected_task() {
                    let task_id = task.id.clone();
                    match task.status {
                        // Move Review task back to Running (for PR changes)
                        TaskStatus::Review => self.move_review_to_running(&task_id)?,
                        // Move Running task back to Planning
                        TaskStatus::Running => self.move_running_to_planning(&task_id)?,
                        _ => {}
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

    fn handle_title_input(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let has_alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
                self.state.input_cursor = 0;
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

                    self.state.input_cursor = self.state.input_buffer.len();
                    self.state.input_mode = InputMode::InputDescription;
                }
            }
            KeyCode::Left if has_alt => {
                self.state.input_cursor = word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Right if has_alt => {
                self.state.input_cursor = word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            // macOS: Option+Left/Right sends Alt+b / Alt+f
            KeyCode::Char('b') if has_alt => {
                self.state.input_cursor = word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Char('f') if has_alt => {
                self.state.input_cursor = word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Left => {
                if self.state.input_cursor > 0 {
                    self.state.input_cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_cursor += 1;
                }
            }
            KeyCode::Home => {
                self.state.input_cursor = 0;
            }
            KeyCode::End => {
                self.state.input_cursor = self.state.input_buffer.len();
            }
            KeyCode::Backspace => {
                if self.state.input_cursor > 0 {
                    self.state.input_cursor -= 1;
                    self.state.input_buffer.remove(self.state.input_cursor);
                }
            }
            KeyCode::Delete => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_buffer.remove(self.state.input_cursor);
                }
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.insert(self.state.input_cursor, c);
                self.state.input_cursor += 1;
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
                        // Replace trigger+pattern with the selected file path, preserving text after
                        let pattern_end = search.start_pos + 1 + search.pattern.len(); // +1 for trigger char
                        let suffix = self.state.input_buffer[pattern_end..].to_string();
                        self.state.input_buffer.truncate(search.start_pos);
                        self.state.input_buffer.push_str(&selected_file);
                        self.state.input_cursor = self.state.input_buffer.len();
                        self.state.input_buffer.push_str(&suffix);
                        self.state.highlighted_file_paths.insert(selected_file);
                    }
                    self.state.file_search = None;
                }
                KeyCode::Up => {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if search.selected < search.matches.len().saturating_sub(1) {
                        search.selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Char('p') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                KeyCode::Char('j') | KeyCode::Char('n') if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL) => {
                    if search.selected < search.matches.len().saturating_sub(1) {
                        search.selected += 1;
                    }
                }
                KeyCode::Backspace => {
                    if search.pattern.is_empty() {
                        // Cancel search if pattern is empty
                        self.state.input_buffer.pop(); // Remove the trigger char
                        self.state.input_cursor = self.state.input_cursor.saturating_sub(1);
                        self.state.file_search = None;
                    } else {
                        search.pattern.pop();
                        self.state.input_buffer.pop();
                        self.state.input_cursor = self.state.input_cursor.saturating_sub(1);
                        self.update_file_search_matches();
                    }
                }
                KeyCode::Char(c) => {
                    search.pattern.push(c);
                    self.state.input_buffer.push(c);
                    self.state.input_cursor += 1;
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
                self.state.input_cursor = 0;
                self.state.pending_task_title.clear();
                self.state.editing_task_id = None;
                self.state.highlighted_file_paths.clear();
            }
            KeyCode::Enter => {
                // Check if line ends with backslash for line continuation
                if self.state.input_buffer.ends_with('\\') {
                    // Remove backslash and insert newline
                    self.state.input_buffer.pop();
                    self.state.input_buffer.push('\n');
                    self.state.input_cursor = self.state.input_buffer.len();
                } else {
                    // Save task (create or update)
                    self.save_task()?;
                    self.state.input_mode = InputMode::Normal;
                    self.state.input_buffer.clear();
                    self.state.input_cursor = 0;
                    self.state.pending_task_title.clear();
                    self.state.editing_task_id = None;
                    self.state.highlighted_file_paths.clear();
                }
            }
            KeyCode::Left if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor = word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Right if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor = word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            // macOS: Option+Left/Right sends Alt+b / Alt+f
            KeyCode::Char('b') if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor = word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Char('f') if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor = word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Left => {
                if self.state.input_cursor > 0 {
                    self.state.input_cursor -= 1;
                }
            }
            KeyCode::Right => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_cursor += 1;
                }
            }
            KeyCode::Home => {
                self.state.input_cursor = 0;
            }
            KeyCode::End => {
                self.state.input_cursor = self.state.input_buffer.len();
            }
            KeyCode::Backspace => {
                if self.state.input_cursor > 0 {
                    self.state.input_cursor -= 1;
                    self.state.input_buffer.remove(self.state.input_cursor);
                }
            }
            KeyCode::Delete => {
                if self.state.input_cursor < self.state.input_buffer.len() {
                    self.state.input_buffer.remove(self.state.input_cursor);
                }
            }
            KeyCode::Char('#') | KeyCode::Char('@') => {
                // Start file search at cursor position
                let trigger = if let KeyCode::Char(c) = key.code { c } else { '#' };
                let start_pos = self.state.input_cursor;
                self.state.input_buffer.insert(self.state.input_cursor, trigger);
                self.state.input_cursor += 1;
                self.state.file_search = Some(FileSearchState {
                    pattern: String::new(),
                    matches: vec![],
                    selected: 0,
                    start_pos,
                    trigger_char: trigger,
                });
                self.update_file_search_matches();
            }
            KeyCode::Char(c) => {
                self.state.input_buffer.insert(self.state.input_cursor, c);
                self.state.input_cursor += 1;
            }
            _ => {}
        }
        Ok(())
    }

    fn update_file_search_matches(&mut self) {
        if let (Some(ref mut search), Some(ref project_path)) = (&mut self.state.file_search, &self.state.project_path) {
            let pattern = &search.pattern;
            search.matches = fuzzy_find_files(project_path, pattern, 10, self.state.git_ops.as_ref());
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
        if let Some(task) = self.state.board.selected_task().cloned() {
            // Show confirmation popup
            self.state.delete_confirm_popup = Some(DeleteConfirmPopup {
                task_id: task.id.clone(),
                task_title: task.title.clone(),
            });
        }
        Ok(())
    }

    fn perform_delete_task(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(project_path)) = (&self.state.db, &self.state.project_path) {
            if let Some(task) = db.get_task(task_id)? {
                delete_task_resources(
                    &task,
                    project_path,
                    self.state.tmux_ops.as_ref(),
                    self.state.git_ops.as_ref(),
                );
                db.delete_task(&task.id)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn show_task_diff(&mut self) -> Result<()> {
        if let Some(task) = self.state.board.selected_task() {
            let diff_content = if let Some(worktree_path) = &task.worktree_path {
                collect_task_diff(worktree_path, self.state.git_ops.as_ref())
            } else {
                "(task has no worktree yet)".to_string()
            };

            self.state.diff_popup = Some(DiffPopup {
                task_title: task.title.clone(),
                diff_content,
                scroll_offset: 0,
            });
        }
        Ok(())
    }

    fn move_task_right(&mut self) -> Result<()> {
        // Clone task to avoid borrow issues
        let (mut task, project_path) = match (
            self.state.board.selected_task().cloned(),
            self.state.project_path.clone(),
        ) {
            (Some(t), Some(p)) => (t, p),
            _ => return Ok(()),
        };

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
                // Build the prompt from task title and description
                // Instruct agent to plan first and wait for approval
                let task_content = if let Some(desc) = &task.description {
                    format!("{}\n\n{}", task.title, desc)
                } else {
                    task.title.clone()
                };
                let prompt = format!(
                    "Task: {}\n\nPlease analyze this task and create a detailed implementation plan. \
                    List the files you'll need to modify and the changes you'll make. \
                    Wait for my approval before making any changes.",
                    task_content
                );

                let target = setup_task_worktree(
                    &mut task,
                    &project_path,
                    &self.state.project_name,
                    &prompt,
                    self.state.config.copy_files.clone(),
                    self.state.config.init_script.clone(),
                    self.state.tmux_ops.as_ref(),
                    self.state.git_ops.as_ref(),
                    self.state.agent_registry.get(&self.state.config.default_agent).as_ref(),
                )?;

                // Wait for agent to show the bypass warning prompt, then accept it and rename session
                // Poll until we see "Yes, I accept" option or timeout after 5 seconds
                let target_clone = target.clone();
                let task_id_clone = task.id.clone();
                let tmux_ops = Arc::clone(&self.state.tmux_ops);
                std::thread::spawn(move || {
                    let mut accepted = false;
                    for _ in 0..50 {
                        std::thread::sleep(std::time::Duration::from_millis(100));

                        // Check pane content for the bypass prompt
                        if let Ok(content) = tmux_ops.capture_pane(&target_clone) {
                            // Look for the bypass warning prompt options
                            if content.contains("Yes, I accept") || content.contains("I accept the risk") {
                                // Found the prompt, send "2" and Enter
                                let _ = tmux_ops.send_keys_literal(&target_clone, "2");
                                std::thread::sleep(std::time::Duration::from_millis(50));
                                let _ = tmux_ops.send_keys_literal(&target_clone, "Enter");
                                accepted = true;
                                break;
                            }
                        }
                    }

                    // After accepting, wait for agent to be ready and rename the session
                    if accepted {
                        std::thread::sleep(std::time::Duration::from_millis(1000));
                        // Send /rename command to name the session with task ID for later resume
                        let rename_cmd = format!("/rename {}", task_id_clone);
                        let _ = tmux_ops.send_keys(&target_clone, &rename_cmd);
                    }
                });
            }

            // When moving from Planning to Running, tell agent to start implementing
            if current_status == TaskStatus::Planning && new_status == TaskStatus::Running {
                if let Some(session_name) = &task.session_name {
                    // Send message to start implementation (send_keys adds Enter at the end)
                    let _ = self.state.tmux_ops.send_keys(session_name, "Looks good, please proceed with the implementation.");
                }
            }

            // When moving from Running to Review: Ask if user wants to create PR
            if current_status == TaskStatus::Running && new_status == TaskStatus::Review {
                // Check if PR already exists (task was resumed from Review)
                if task.pr_number.is_some() {
                    // PR already exists - just commit and push the new changes
                    self.state.pr_status_popup = Some(PrStatusPopup {
                        status: PrCreationStatus::Pushing,
                        pr_url: None,
                        error_message: None,
                    });

                    let task_clone = task.clone();
                    let project_path_clone = project_path.clone();
                    let git_ops = Arc::clone(&self.state.git_ops);
                    let agent_ops = self.state.agent_registry.get(&self.state.config.default_agent);

                    let (tx, rx) = mpsc::channel();
                    self.state.pr_creation_rx = Some(rx);

                    std::thread::spawn(move || {
                        let result = push_changes_to_existing_pr(&task_clone, git_ops.as_ref(), agent_ops.as_ref());
                        match result {
                            Ok(pr_url) => {
                                // Update task in database
                                // Keep session_name so popup can still be opened in Review
                                if let Ok(db) = crate::db::Database::open_project(&project_path_clone) {
                                    let mut updated_task = task_clone;
                                    updated_task.status = TaskStatus::Review;
                                    updated_task.updated_at = chrono::Utc::now();
                                    let _ = db.update_task(&updated_task);
                                }
                                let _ = tx.send(Ok((0, pr_url)));
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e.to_string()));
                            }
                        }
                    });

                    // Keep tmux window open - session_name stays set for resume

                    return Ok(());
                }

                // No PR yet - show confirmation popup asking if user wants to create PR
                self.state.review_confirm_popup = Some(ReviewConfirmPopup {
                    task_id: task.id.clone(),
                    task_title: task.title.clone(),
                });
                return Ok(());
            }

            // When moving from Review to Done: Show confirmation with PR state
            if current_status == TaskStatus::Review && new_status == TaskStatus::Done {
                if let Some(pr_number) = task.pr_number {
                    let pr_state = self.state.git_provider_ops.get_pr_state(&project_path, pr_number)?;

                    let confirm_state = match pr_state {
                        PullRequestState::Merged => DoneConfirmPrState::Merged,
                        PullRequestState::Closed => DoneConfirmPrState::Closed,
                        PullRequestState::Open => DoneConfirmPrState::Open,
                        PullRequestState::Unknown => DoneConfirmPrState::Unknown,
                    };

                    self.state.done_confirm_popup = Some(DoneConfirmPopup {
                        task_id: task.id.clone(),
                        pr_number,
                        pr_state: confirm_state,
                    });
                    return Ok(());
                }
                // No PR - allow moving to Done directly (task might have been abandoned early)
                // Cleanup resources (but don't set status yet - that's done below)
                cleanup_task_for_done(
                    &mut task,
                    &project_path,
                    self.state.tmux_ops.as_ref(),
                    self.state.git_ops.as_ref(),
                );
            }

            task.status = new_status;
            task.updated_at = chrono::Utc::now();

            if let Some(db) = &self.state.db {
                db.update_task(&task)?;
            }
        }
        self.refresh_tasks()?;
        Ok(())
    }

    /// Move task directly from Backlog to Running (skip Planning)
    fn move_backlog_to_running(&mut self) -> Result<()> {
        let (mut task, project_path) = match (
            self.state.board.selected_task().cloned(),
            self.state.project_path.clone(),
        ) {
            (Some(t), Some(p)) => (t, p),
            _ => return Ok(()),
        };

        if task.status != TaskStatus::Backlog {
            return Ok(());
        }

        // Build prompt - skip planning, go straight to implementation
        let task_content = if let Some(desc) = &task.description {
            format!("{}\n\n{}", task.title, desc)
        } else {
            task.title.clone()
        };
        let prompt = format!(
            "Task: {}\n\nPlease implement this task directly. No need to plan first - go ahead and make the changes.",
            task_content
        );

        let target = setup_task_worktree(
            &mut task,
            &project_path,
            &self.state.project_name,
            &prompt,
            self.state.config.copy_files.clone(),
            self.state.config.init_script.clone(),
            self.state.tmux_ops.as_ref(),
            self.state.git_ops.as_ref(),
            self.state.agent_registry.get(&self.state.config.default_agent).as_ref(),
        )?;

        // Wait for agent to show the bypass warning prompt, then accept it and rename session
        let target_clone = target.clone();
        let task_id_clone = task.id.clone();
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        std::thread::spawn(move || {
            let mut accepted = false;
            for _ in 0..50 {
                std::thread::sleep(std::time::Duration::from_millis(100));

                if let Ok(content) = tmux_ops.capture_pane(&target_clone) {
                    if content.contains("Yes, I accept") || content.contains("I accept the risk") {
                        let _ = tmux_ops.send_keys_literal(&target_clone, "2");
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        let _ = tmux_ops.send_keys_literal(&target_clone, "Enter");
                        accepted = true;
                        break;
                    }
                }
            }

            if accepted {
                std::thread::sleep(std::time::Duration::from_millis(1000));
                let rename_cmd = format!("/rename {}", task_id_clone);
                let _ = tmux_ops.send_keys(&target_clone, &rename_cmd);
            }
        });

        task.status = TaskStatus::Running;
        task.updated_at = chrono::Utc::now();

        if let Some(db) = &self.state.db {
            db.update_task(&task)?;
        }
        self.refresh_tasks()?;
        Ok(())
    }

    /// Move task from Review back to Running (only allowed transition backwards)
    /// The tmux window should still be open from when it was in Running state
    fn move_review_to_running(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(_project_path)) = (&self.state.db, &self.state.project_path) {
            if let Some(mut task) = db.get_task(task_id)? {
                if task.status != TaskStatus::Review {
                    return Ok(());
                }

                // Just move the task back to Running - the tmux window should still be open
                task.status = TaskStatus::Running;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn move_running_to_planning(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(_project_path)) = (&self.state.db, &self.state.project_path) {
            if let Some(mut task) = db.get_task(task_id)? {
                if task.status != TaskStatus::Running {
                    return Ok(());
                }

                // Just move the task back to Planning - the tmux window should still be open
                task.status = TaskStatus::Planning;
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
                let mut popup = ShellPopup::new(task.title.clone(), window_name.clone());

                // Resize tmux window to match popup dimensions (uses same constants as draw_shell_popup)
                if let Ok((_term_width, term_height)) = crossterm::terminal::size() {
                    let pane_width = SHELL_POPUP_CONTENT_WIDTH;
                    let popup_height = (term_height as u32 * SHELL_POPUP_HEIGHT_PERCENT as u32 / 100) as u16;
                    let pane_height = popup_height.saturating_sub(4); // -4 for borders + header/footer

                    let target = format!("{}:{}", self.state.project_name, window_name);
                    // TODO the resize should be done on target which is
                    // session_name:window_name, but for some reason that doesn't work
                    // doing tmux -L agtx resize-window -t session:window -x 30 -y 30 works
                    let _ = self.state.tmux_ops.resize_window(&window_name, pane_width, pane_height);
                    popup.last_pane_size = Some((pane_width, pane_height));
                }

                // Capture initial content
                popup.cached_content = capture_tmux_pane_with_history(window_name, 500, self.state.tmux_ops.as_ref());

                self.state.shell_popup = Some(popup);
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
        ensure_project_tmux_session(&project.name, &project_path, self.state.tmux_ops.as_ref());

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
fn ensure_project_tmux_session(project_name: &str, project_path: &Path, tmux_ops: &dyn TmuxOperations) {
    if !tmux_ops.has_session(project_name) {
        let _ = tmux_ops.create_session(project_name, &project_path.to_string_lossy());
    }
}

/// Generate a URL-safe slug from task ID and title
fn generate_task_slug(task_id: &str, title: &str) -> String {
    let title_slug: String = title
        .chars()
        .map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' })
        .take(30)
        .collect();
    let title_slug = title_slug.trim_matches('-').to_string();

    // Add task ID prefix to ensure uniqueness
    let id_prefix: String = task_id.chars().take(8).collect();
    format!("{}-{}", id_prefix, title_slug)
}

/// Cleanup task resources (tmux window, git worktree) and mark as done
/// Modifies the task in place, ready for database update
fn cleanup_task_for_done(
    task: &mut Task,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    if let Some(session_name) = &task.session_name {
        let _ = tmux_ops.kill_window(session_name);
    }
    if let Some(worktree) = &task.worktree_path {
        let _ = git_ops.remove_worktree(project_path, worktree);
    }
    // Keep the branch so task can be reopened later
    task.session_name = None;
    task.worktree_path = None;
    task.status = TaskStatus::Done;
    task.updated_at = chrono::Utc::now();
}

/// Set up a worktree and tmux window for a task.
/// Creates worktree, initializes it (copy files + init script), creates tmux window with agent.
/// Updates task fields (session_name, worktree_path, branch_name) in place.
/// Returns the tmux target string on success.
fn setup_task_worktree(
    task: &mut Task,
    project_path: &Path,
    project_name: &str,
    prompt: &str,
    copy_files: Option<String>,
    init_script: Option<String>,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
    agent_ops: &dyn AgentOperations,
) -> Result<String> {
    let unique_slug = generate_task_slug(&task.id, &task.title);
    let window_name = format!("task-{}", unique_slug);
    let target = format!("{}:{}", project_name, window_name);

    // Create git worktree from main branch
    let worktree_path_str = match git_ops.create_worktree(project_path, &unique_slug) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Failed to create worktree: {}", e);
            project_path.join(".agtx").join("worktrees").join(&unique_slug)
                .to_string_lossy().to_string()
        }
    };

    // Initialize worktree: copy files and run init script
    let worktree_path = Path::new(&worktree_path_str);
    let init_warnings = git_ops.initialize_worktree(
        project_path,
        worktree_path,
        copy_files,
        init_script,
    );
    for warning in &init_warnings {
        eprintln!("Worktree init: {}", warning);
    }

    // Build the interactive command using injected agent ops
    let agent_cmd = agent_ops.build_interactive_command(prompt);

    // Ensure project tmux session exists
    ensure_project_tmux_session(project_name, project_path, tmux_ops);

    tmux_ops.create_window(
        project_name,
        &window_name,
        &worktree_path_str,
        Some(agent_cmd),
    )?;

    task.session_name = Some(target.clone());
    task.worktree_path = Some(worktree_path_str);
    task.branch_name = Some(format!("task/{}", unique_slug));

    Ok(target)
}

/// Delete task resources: kill tmux window, remove worktree, delete branch
fn delete_task_resources(
    task: &Task,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    // Kill tmux window if exists
    if let Some(ref session_name) = task.session_name {
        let _ = tmux_ops.kill_window(session_name);
    }

    // Remove worktree and delete branch if exists
    if task.worktree_path.is_some() {
        if let Some(ref branch_name) = task.branch_name {
            let slug = branch_name.strip_prefix("task/").unwrap_or(branch_name);
            let _ = git_ops.remove_worktree(project_path, slug);
            let _ = git_ops.delete_branch(project_path, branch_name);
        }
    }
}

/// Collect git diff content from a worktree
/// Returns formatted diff sections (unstaged, staged, untracked)
fn collect_task_diff(worktree_path: &str, git_ops: &dyn GitOperations) -> String {
    let worktree = Path::new(worktree_path);
    let mut sections = Vec::new();

    // Unstaged changes (modified tracked files)
    let unstaged = git_ops.diff(worktree);
    if !unstaged.trim().is_empty() {
        sections.push(format!("=== Unstaged Changes ===\n\n{}", unstaged));
    }

    // Staged changes
    let staged = git_ops.diff_cached(worktree);
    if !staged.trim().is_empty() {
        sections.push(format!("=== Staged Changes ===\n\n{}", staged));
    }

    // Untracked files - show as diff (new file content)
    let untracked = git_ops.list_untracked_files(worktree);
    if !untracked.trim().is_empty() {
        let mut untracked_section = String::from("=== Untracked Files ===\n");
        for file in untracked.lines() {
            if file.trim().is_empty() {
                continue;
            }
            // Show diff for untracked file (as if adding new file)
            let file_diff = git_ops.diff_untracked_file(worktree, file);
            if !file_diff.trim().is_empty() {
                untracked_section.push_str(&format!("\n{}", file_diff));
            } else {
                // Fallback: just show file name
                untracked_section.push_str(&format!("\n+++ new file: {}\n", file));
            }
        }
        sections.push(untracked_section);
    }

    if sections.is_empty() {
        format!("(no changes)\n\nWorktree: {}", worktree_path)
    } else {
        sections.join("\n\n")
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

/// Create a centered popup with fixed width and percentage height
fn centered_rect_fixed_width(fixed_width: u16, percent_y: u16, r: Rect) -> Rect {
    // Cap width to terminal width minus some margin
    let width = fixed_width.min(r.width.saturating_sub(4));

    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    // Calculate horizontal centering
    let horizontal_margin = r.width.saturating_sub(width) / 2;

    Rect {
        x: r.x + horizontal_margin,
        y: popup_layout[1].y,
        width,
        height: popup_layout[1].height,
    }
}

/// Capture content from a tmux pane with history (with ANSI escape sequences)
fn capture_tmux_pane_with_history(window_name: &str, history_lines: i32, tmux_ops: &dyn TmuxOperations) -> Vec<u8> {
    let content = tmux_ops.capture_pane_with_history(window_name, history_lines);

    // Get the cursor position and pane height to know where the "real" content ends
    // Lines below the cursor are unused pane buffer space
    let cursor_info = tmux_ops.get_cursor_info(window_name);

    // Trim content to only include lines up to cursor position
    shell_popup::trim_content_to_cursor(content, cursor_info)
}

/// Generate PR title and description using the configured agent
pub(crate) fn generate_pr_description(
    task_title: &str,
    worktree_path: Option<&str>,
    _branch_name: Option<&str>,
    git_ops: &dyn GitOperations,
    agent_ops: &dyn AgentOperations,
) -> (String, String) {
    // Default values
    let default_title = task_title.to_string();
    let mut default_body = String::new();

    // Try to get git diff for context
    if let Some(worktree) = worktree_path {
        let worktree_path = Path::new(worktree);
        // Get diff from main
        let diff_stat = git_ops.diff_stat_from_main(worktree_path);

        if !diff_stat.is_empty() {
            default_body.push_str("## Changes\n```\n");
            default_body.push_str(&diff_stat);
            default_body.push_str("```\n");
        }

        // Try to use the agent to generate a better description
        let prompt = format!(
            "Generate a concise PR description for these changes. Task: '{}'. Output only the description, no markdown code blocks around it. Keep it brief (2-3 sentences max).",
            task_title
        );

        if let Ok(generated) = agent_ops.generate_text(worktree_path, &prompt) {
            if !generated.is_empty() {
                default_body = format!("{}\n\n{}", generated, default_body);
            }
        }
    }

    (default_title, default_body)
}

/// Create a PR with provided title and body, return (pr_number, pr_url)
fn create_pr_with_content(
    task: &Task,
    project_path: &Path,
    pr_title: &str,
    pr_body: &str,
    git_ops: &dyn GitOperations,
    git_provider_ops: &dyn GitProviderOperations,
    agent_ops: &dyn AgentOperations,
) -> Result<(i32, String)> {
    let worktree = task.worktree_path.as_deref().unwrap_or(".");
    let worktree_path = Path::new(worktree);

    // Stage all changes
    git_ops.add_all(worktree_path)?;

    // Check if there are changes to commit
    let has_changes = git_ops.has_changes(worktree_path);

    // Commit if there are staged changes
    if has_changes {
        let commit_msg = format!("{}\n\nCo-Authored-By: {}", pr_title, agent_ops.co_author_string());
        git_ops.commit(worktree_path, &commit_msg)?;
    }

    // Push the branch
    if let Some(branch) = &task.branch_name {
        git_ops.push(worktree_path, branch, true)?;
    }

    // Create PR
    git_provider_ops.create_pr(
        project_path,
        pr_title,
        pr_body,
        task.branch_name.as_deref().unwrap_or(""),
    )
}

/// Push changes to an existing PR (commit and push only, no PR creation)
fn push_changes_to_existing_pr(
    task: &Task,
    git_ops: &dyn GitOperations,
    agent_ops: &dyn AgentOperations,
) -> Result<String> {
    let worktree = task.worktree_path.as_deref().unwrap_or(".");
    let worktree_path = Path::new(worktree);

    // Stage all changes
    git_ops.add_all(worktree_path)?;

    // Check if there are changes to commit
    let has_changes = git_ops.has_changes(worktree_path);

    // Commit if there are staged changes
    if has_changes {
        let commit_msg = format!("Address review comments\n\nCo-Authored-By: {}", agent_ops.co_author_string());
        git_ops.commit(worktree_path, &commit_msg)?;
    }

    // Push the branch
    if let Some(branch) = &task.branch_name {
        git_ops.push(worktree_path, branch, false)?;
    }

    // Return the existing PR URL
    Ok(task.pr_url.clone().unwrap_or_else(|| "Changes pushed to existing PR".to_string()))
}

/// Send a key to a tmux pane
fn send_key_to_tmux(window_name: &str, key: KeyCode, tmux_ops: &dyn TmuxOperations) {
    let key_str = match key {
        KeyCode::Char(c) => c.to_string(),
        KeyCode::Enter => "Enter".to_string(),
        KeyCode::Esc => "Escape".to_string(),
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

    let _ = tmux_ops.send_keys_literal(window_name, &key_str);
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

/// Find the previous word boundary (for Option+Left)
fn word_boundary_left(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    let mut i = pos - 1;
    // Skip whitespace/punctuation
    while i > 0 && !bytes[i].is_ascii_alphanumeric() {
        i -= 1;
    }
    // Skip word characters
    while i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        i -= 1;
    }
    i
}

/// Find the next word boundary (for Option+Right)
fn word_boundary_right(s: &str, pos: usize) -> usize {
    let len = s.len();
    if pos >= len {
        return len;
    }
    let bytes = s.as_bytes();
    let mut i = pos;
    // Skip current word characters
    while i < len && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    // Skip whitespace/punctuation
    while i < len && !bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    i
}

/// Build styled Text with highlighted file paths
fn build_highlighted_text<'a>(
    text: &str,
    file_paths: &HashSet<String>,
    text_color: Color,
    highlight_color: Color,
) -> Text<'a> {
    let normal_style = Style::default().fg(text_color);
    let highlight_style = Style::default().fg(highlight_color).bold();

    let lines: Vec<Line> = text
        .split('\n')
        .map(|line| {
            let mut spans: Vec<Span> = Vec::new();
            let mut remaining = line;

            while !remaining.is_empty() {
                // Find the earliest file path match in the remaining text
                let mut earliest: Option<(usize, &str)> = None;
                for path in file_paths {
                    if let Some(pos) = remaining.find(path.as_str()) {
                        if earliest.is_none() || pos < earliest.unwrap().0 {
                            earliest = Some((pos, path.as_str()));
                        }
                    }
                }

                if let Some((pos, path)) = earliest {
                    if pos > 0 {
                        spans.push(Span::styled(remaining[..pos].to_string(), normal_style));
                    }
                    spans.push(Span::styled(path.to_string(), highlight_style));
                    remaining = &remaining[pos + path.len()..];
                } else {
                    spans.push(Span::styled(remaining.to_string(), normal_style));
                    break;
                }
            }

            Line::from(spans)
        })
        .collect();

    Text::from(lines)
}

/// Fuzzy find files in a directory (respects .gitignore)
fn fuzzy_find_files(project_path: &Path, pattern: &str, max_results: usize, git_ops: &dyn GitOperations) -> Vec<String> {
    // Use git ls-files to get tracked files (respects .gitignore)
    let files = git_ops.list_files(project_path);

    if files.is_empty() {
        return vec![];
    }

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

    matches.into_iter().take(max_results).map(|(path, _)| path).collect()
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

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
