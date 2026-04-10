use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};
use std::collections::{HashMap, HashSet};
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc,
};
use std::time::Instant;

use crate::agent::{self, AgentOperations};
use crate::config::{GlobalConfig, MergedConfig, ProjectConfig, ThemeConfig, WorkflowPlugin};
use crate::db::{Database, PhaseStatus, Task, TaskStatus, TransitionRequest};
use crate::git::{
    self, GitOperations, GitProviderOperations, PullRequestState, RealGitHubOps, RealGitOps,
};
use crate::skills;
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
fn build_footer_text(
    input_mode: InputMode,
    sidebar_focused: bool,
    selected_column: usize,
    has_cyclic_plugin: bool,
) -> String {
    match input_mode {
        InputMode::Normal => {
            if sidebar_focused {
                " [j/k] navigate  [Enter] open  [l] board  [e] hide sidebar  [q] quit ".to_string()
            } else {
                match selected_column {
                    0 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [R] research  [m] plan  [M] run  [e] sidebar  [q] quit".to_string(),
                    1 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] run  [e] sidebar  [q] quit".to_string(),
                    2 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] move  [r] move left  [e] sidebar  [q] quit".to_string(),
                    3 if has_cyclic_plugin => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] done  [r] resume  [p] next phase  [e] sidebar  [q] quit".to_string(),
                    3 => " [o] new  [/] search  [Enter] open  [x] del  [d] diff  [m] move  [r] move left  [e] sidebar  [q] quit".to_string(),
                    _ => " [o] new  [/] search  [Enter] open  [x] del  [e] sidebar  [q] quit".to_string(),
                }
            }
        }
        InputMode::InputTitle => " Enter task title... [Esc] cancel [Enter] next ".to_string(),
        InputMode::SelectPlugin => {
            " [j/k] select plugin  [Tab] cycle  [Enter] next  [Esc] cancel ".to_string()
        }
        InputMode::InputDescription => {
            " [#] files  [/] skills  [!] tasks  [Esc] cancel  [\\+Enter] newline  [Enter] save "
                .to_string()
        }
    }
}

type Terminal = ratatui::Terminal<AppBackend>;

/// Backend abstraction: real CrosstermBackend in production, TestBackend in tests.
enum AppBackend {
    Crossterm(CrosstermBackend<Stdout>),
    #[cfg(feature = "test-mocks")]
    Test(ratatui::backend::TestBackend),
}

impl ratatui::backend::Backend for AppBackend {
    type Error = io::Error;

    fn draw<'a, I>(&mut self, content: I) -> io::Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
    {
        match self {
            Self::Crossterm(b) => b.draw(content),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .draw(content)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn hide_cursor(&mut self) -> io::Result<()> {
        match self {
            Self::Crossterm(b) => b.hide_cursor(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .hide_cursor()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn show_cursor(&mut self) -> io::Result<()> {
        match self {
            Self::Crossterm(b) => b.show_cursor(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .show_cursor()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn get_cursor_position(&mut self) -> io::Result<ratatui::layout::Position> {
        match self {
            Self::Crossterm(b) => b.get_cursor_position(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .get_cursor_position()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn set_cursor_position<P: Into<ratatui::layout::Position>>(
        &mut self,
        position: P,
    ) -> io::Result<()> {
        match self {
            Self::Crossterm(b) => b.set_cursor_position(position),
            // TestBackend's set_cursor_position is also generic, so just forward
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .set_cursor_position(position)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn clear(&mut self) -> io::Result<()> {
        match self {
            Self::Crossterm(b) => b.clear(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .clear()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn clear_region(&mut self, clear_type: ratatui::backend::ClearType) -> io::Result<()> {
        match self {
            Self::Crossterm(b) => b.clear_region(clear_type),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .clear_region(clear_type)
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn size(&self) -> io::Result<ratatui::layout::Size> {
        match self {
            Self::Crossterm(b) => b.size(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .size()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn window_size(&mut self) -> io::Result<ratatui::backend::WindowSize> {
        match self {
            Self::Crossterm(b) => b.window_size(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .window_size()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Self::Crossterm(b) => b.flush(),
            #[cfg(feature = "test-mocks")]
            Self::Test(b) => b
                .flush()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e)),
        }
    }
}

/// Shell popup dimensions - used for both rendering and tmux window sizing
const SHELL_POPUP_WIDTH: u16 = 128; // Total width including borders
const SHELL_POPUP_CONTENT_WIDTH: u16 = 126; // Content width (SHELL_POPUP_WIDTH - 2 for borders)
const SHELL_POPUP_HEIGHT_PERCENT: u16 = 75; // Percentage of terminal height

/// Application state (separate from terminal for borrow checker)
struct AppState {
    mode: AppMode,
    flags: crate::FeatureFlags,
    should_quit: bool,
    board: BoardState,
    input_mode: InputMode,
    input_buffer: String,
    input_cursor: usize, // Cursor position in input_buffer
    // For task creation/editing wizard
    pending_task_title: String,
    editing_task_id: Option<String>, // Some(id) when editing, None when creating
    wizard_selected_plugin: usize,
    wizard_plugin_options: Vec<PluginOption>,
    wizard_referenced_task_ids: HashSet<String>,
    db: Option<Database>,
    #[allow(dead_code)]
    global_db: Database,
    config: MergedConfig,
    project_path: Option<PathBuf>,
    project_name: String,
    tmux_project_name: String,
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
    // Skill search dropdown
    skill_search: Option<SkillSearchState>,
    // Task reference search dropdown
    task_ref_search: Option<TaskRefSearchState>,
    // References inserted via file search, skill search, or task search (for highlighting)
    highlighted_references: HashSet<String>,
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
    // Confirmation popup for moving task when phase is incomplete
    move_confirm_popup: Option<MoveConfirmPopup>,
    // Flag to skip move confirmation (set after user confirms popup)
    skip_move_confirm: bool,
    // Confirmation popup for deleting a task
    delete_confirm_popup: Option<DeleteConfirmPopup>,
    // Confirmation popup for asking if user wants to create PR when moving to Review
    review_confirm_popup: Option<ReviewConfirmPopup>,
    // Channel for receiving background worktree setup results
    setup_rx: Option<mpsc::Receiver<SetupResult>>,
    // Phase detection
    phase_status_cache: HashMap<String, (PhaseStatus, Instant)>,
    spinner_frame: usize,
    // Idle detection: (content_hash, last_change_time) per task
    pane_content_hashes: HashMap<String, (u64, Instant)>,
    // Guard: task IDs for which merge-conflict check has already been performed
    merge_conflict_checked: HashSet<String>,
    // Guard: task IDs for which stuck-task notification has been fired (reset on phase advance)
    stuck_task_notified: HashSet<String>,
    // When each task first became Idle (for stuck-task detection)
    stuck_task_idle_since: HashMap<String, Instant>,
    cached_plugin: Option<Option<WorkflowPlugin>>,
    // Transient warning message shown in footer (auto-clears after a few seconds)
    warning_message: Option<(String, Instant)>,
    // Plugin selection popup
    plugin_select_popup: Option<PluginSelectPopup>,
    // Orchestrator agent tmux target (e.g. "project:orchestrator")
    orchestrator_session: Option<String>,
    // Set to true once the orchestrator agent is ready and has received the skill command.
    // Gates notification delivery so we don't send into a pane that's still initializing.
    orchestrator_ready: Arc<AtomicBool>,
    // Orchestrator idle detection for push notifications
    orchestrator_last_content: String,
    orchestrator_stable_since: Option<Instant>,
    orchestrator_last_check: Instant,
    // Background session refresh channel (non-blocking phase status polling)
    session_refresh_rx: Option<mpsc::Receiver<SessionRefreshResult>>,
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
    UncommittedChanges,
    Unknown,
}

/// State for confirming move when phase is incomplete (agent still working)
#[derive(Debug, Clone)]
struct MoveConfirmPopup {
    task_id: String,
    from_status: TaskStatus,
    to_status: TaskStatus,
}

/// Result from background worktree setup (research, planning, move-to-running)
struct SetupResult {
    task_id: String,
    session_name: String,
    worktree_path: String,
    branch_name: String,
    new_status: Option<TaskStatus>,
    agent: String,
    plugin: Option<String>,
    error: Option<String>,
}

/// Pre-fetched info about a referenced task for worktree setup (avoids DB access in thread).
#[derive(Debug, Clone)]
struct ReferencedTaskInfo {
    slug: String,
    branch_name: Option<String>,
    worktree_path: Option<String>,
}

/// Per-task result from the background session refresh thread.
struct SessionTaskStatus {
    task_id: String,
    phase_status: PhaseStatus,
    /// Content hash from tmux capture (for idle detection on main thread).
    content_hash: Option<u64>,
    /// Task status (needed for merge-conflict check on main thread).
    status: TaskStatus,
    /// Worktree path (needed for merge-conflict check).
    worktree_path: Option<String>,
    /// Tmux session name (needed for merge-conflict check).
    session_name: Option<String>,
    /// Agent name (needed for merge-conflict skill dispatch).
    agent: String,
    /// Whether this task was already Ready before this refresh cycle.
    was_ready: bool,
}

/// Results sent back from the background session refresh thread.
struct SessionRefreshResult {
    statuses: Vec<SessionTaskStatus>,
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
    trigger_char: char, // The character that triggered the search (# or @)
}

/// A discovered skill command from an agent's native directory
#[derive(Debug, Clone)]
struct SkillEntry {
    command: String,     // agent-native: "/agtx:plan" or "$agtx-plan"
    description: String, // from frontmatter or file stem
}

/// State for skill search dropdown (triggered by `/`)
#[derive(Debug, Clone)]
struct SkillSearchState {
    pattern: String,
    matches: Vec<SkillEntry>,
    all_skills: Vec<SkillEntry>, // cached full list for re-filtering
    selected: usize,
    start_pos: usize, // cursor position where `/` was typed
}

/// State for task reference search dropdown (triggered by `!`)
#[derive(Debug, Clone)]
struct TaskRefSearchState {
    pattern: String,
    matches: Vec<(String, String, TaskStatus)>, // (id, title, status)
    selected: usize,
    start_pos: usize, // cursor position where `!` was typed
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

/// State for plugin selection popup
#[derive(Debug, Clone)]
struct PluginSelectPopup {
    selected: usize,
    options: Vec<PluginOption>,
}

#[derive(Debug, Clone)]
struct PluginOption {
    name: String,        // "" for none, "gsd", "spec-kit", etc.
    label: String,       // Display name
    description: String, // One-line description
    active: bool,        // Currently active for this project
}

pub struct App {
    terminal: Terminal,
    state: AppState,
}

impl App {
    pub fn new(mode: AppMode, flags: crate::FeatureFlags) -> Result<Self> {
        Self::with_ops(
            mode,
            flags,
            Arc::new(RealTmuxOps),
            Arc::new(RealGitOps),
            Arc::new(RealGitHubOps),
            Arc::new(agent::RealAgentRegistry::new("claude")),
        )
    }

    pub fn with_ops(
        mode: AppMode,
        flags: crate::FeatureFlags,
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
        let terminal = ratatui::Terminal::new(AppBackend::Crossterm(backend))?;

        // Load configs
        let global_config = GlobalConfig::load().unwrap_or_default();
        let global_db = Database::open_global()?;

        // Detect available agents
        let available_agents = agent::detect_available_agents();

        // Setup based on mode
        let (db, project_path, project_name, tmux_project_name, project_config) = match &mode {
            AppMode::Dashboard => (
                None,
                None,
                "Dashboard".to_string(),
                tmux::safe_session_name("Dashboard"),
                ProjectConfig::default(),
            ),
            AppMode::Project(path) => {
                let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                let name = canonical
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("unknown")
                    .to_string();
                let tmux_name = tmux::safe_session_name(&name);
                let project_config = ProjectConfig::load(&canonical).unwrap_or_default();
                let db = Database::open_project(&canonical)?;

                // Register project in global database
                let project = crate::db::Project::new(&name, canonical.to_string_lossy());
                global_db.upsert_project(&project)?;

                // Ensure tmux session exists for this project
                ensure_project_tmux_session(&tmux_name, &canonical, tmux_ops.as_ref());

                (Some(db), Some(canonical), name, tmux_name, project_config)
            }
        };

        let config = MergedConfig::merge(&global_config, &project_config);

        let mut app = Self {
            terminal,
            state: AppState {
                mode,
                flags,
                should_quit: false,
                board: BoardState::new(),
                input_mode: InputMode::Normal,
                input_buffer: String::new(),
                input_cursor: 0,
                pending_task_title: String::new(),
                editing_task_id: None,
                wizard_selected_plugin: 0,
                wizard_plugin_options: vec![],
                wizard_referenced_task_ids: HashSet::new(),
                db,
                global_db,
                config,
                project_path,
                project_name: project_name.clone(),
                tmux_project_name: tmux_project_name.clone(),
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
                skill_search: None,
                task_ref_search: None,
                highlighted_references: HashSet::new(),
                task_search: None,
                pr_confirm_popup: None,
                review_to_running_task_id: None,
                diff_popup: None,
                pr_generation_rx: None,
                pr_status_popup: None,
                pr_creation_rx: None,
                setup_rx: None,
                done_confirm_popup: None,
                move_confirm_popup: None,
                skip_move_confirm: false,
                delete_confirm_popup: None,
                review_confirm_popup: None,
                phase_status_cache: HashMap::new(),
                spinner_frame: 0,
                pane_content_hashes: HashMap::new(),
                merge_conflict_checked: HashSet::new(),
                stuck_task_notified: HashSet::new(),
                stuck_task_idle_since: HashMap::new(),
                cached_plugin: None,
                warning_message: None,
                plugin_select_popup: None,
                orchestrator_session: None,
                orchestrator_ready: Arc::new(AtomicBool::new(false)),
                orchestrator_last_content: String::new(),
                orchestrator_stable_since: None,
                orchestrator_last_check: Instant::now(),
                session_refresh_rx: None,
            },
        };

        // Load and cache workflow plugin
        app.state.cached_plugin = Some(load_plugin_if_configured(
            &app.state.config,
            app.state.project_path.as_deref(),
        ));

        // Load tasks if in project mode
        app.refresh_tasks()?;
        // Load projects from global database
        app.refresh_projects()?;

        // Recover tasks whose tmux windows were lost (server restart, manual kill, etc.)
        {
            let tasks_to_recover: Vec<_> = app
                .state
                .board
                .tasks
                .iter()
                .filter(|t| {
                    matches!(
                        t.status,
                        TaskStatus::Planning | TaskStatus::Running | TaskStatus::Review
                    ) && t.session_name.is_some()
                        && t.worktree_path.is_some()
                })
                .filter(|t| {
                    let sn = t.session_name.as_ref().unwrap();
                    !app.state.tmux_ops.window_exists(sn).unwrap_or(true)
                })
                .cloned()
                .collect();

            for task in &tasks_to_recover {
                let agent_ops = app.state.agent_registry.get(&task.agent);
                let _ = recover_task_session(
                    task,
                    &app.state.tmux_project_name,
                    app.state
                        .project_path
                        .as_deref()
                        .unwrap_or(Path::new(".")),
                    app.state.tmux_ops.as_ref(),
                    agent_ops.as_ref(),
                );
            }
        }

        // Detect existing orchestrator session (survives agtx restarts)
        if app.state.flags.experimental {
            let orch_target = format!("{}:orchestrator", app.state.tmux_project_name);
            if app
                .state
                .tmux_ops
                .window_exists(&orch_target)
                .unwrap_or(false)
            {
                app.state.orchestrator_session = Some(orch_target);
                app.state.orchestrator_ready.store(true, Ordering::Release);

                // Catch up: notify orchestrator about tasks that completed while TUI was down
                if let Some(ref db) = app.state.db {
                    // Check existing notifications to avoid duplicates
                    let existing: HashSet<String> = db
                        .peek_notifications()
                        .unwrap_or_default()
                        .into_iter()
                        .map(|n| n.message)
                        .collect();

                    for task in &app.state.board.tasks {
                        if !matches!(task.status, TaskStatus::Planning | TaskStatus::Running) {
                            continue;
                        }
                        let plugin = match &task.plugin {
                            Some(name) => {
                                WorkflowPlugin::load(name, app.state.project_path.as_deref()).ok()
                            }
                            None => skills::load_bundled_plugin("agtx"),
                        };
                        if let Some(ref wt) = task.worktree_path {
                            if phase_artifact_exists(wt, task.status, &plugin, task.cycle) {
                                let short_id = if task.id.len() >= 8 {
                                    &task.id[..8]
                                } else {
                                    &task.id
                                };
                                let message = format!(
                                    "Task \"{}\" ({}) completed phase: {}",
                                    task.title,
                                    short_id,
                                    task.status.as_str()
                                );
                                if !existing.contains(&message) {
                                    let notif = crate::db::Notification::new(message);
                                    let _ = db.create_notification(&notif);
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(app)
    }

    /// Create an App instance for testing with in-memory databases and TestBackend.
    /// No real terminal, no real filesystem databases, no agent detection.
    #[cfg(feature = "test-mocks")]
    pub fn new_for_test(
        project_path: Option<PathBuf>,
        tmux_ops: Arc<dyn TmuxOperations>,
        git_ops: Arc<dyn GitOperations>,
        git_provider_ops: Arc<dyn GitProviderOperations>,
        agent_registry: Arc<dyn agent::AgentRegistry>,
    ) -> Result<Self> {
        let backend = ratatui::backend::TestBackend::new(80, 24);
        let terminal = ratatui::Terminal::new(AppBackend::Test(backend))?;

        let global_db = Database::open_in_memory_global()?;
        let (db, mode, project_name, tmux_project_name) = if let Some(ref path) = project_path {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("test-project")
                .to_string();
            let db = Database::open_in_memory_project()?;
            (
                Some(db),
                AppMode::Project(path.clone()),
                name.clone(),
                tmux::safe_session_name(&name),
            )
        } else {
            (
                None,
                AppMode::Dashboard,
                "Dashboard".to_string(),
                tmux::safe_session_name("Dashboard"),
            )
        };

        let config = MergedConfig::merge(&GlobalConfig::default(), &ProjectConfig::default());

        Ok(Self {
            terminal,
            state: AppState {
                mode,
                flags: crate::FeatureFlags::default(),
                should_quit: false,
                board: BoardState::new(),
                input_mode: InputMode::Normal,
                input_buffer: String::new(),
                input_cursor: 0,
                pending_task_title: String::new(),
                editing_task_id: None,
                wizard_selected_plugin: 0,
                wizard_plugin_options: vec![],
                wizard_referenced_task_ids: HashSet::new(),
                db,
                global_db,
                config,
                project_path,
                project_name,
                tmux_project_name,
                available_agents: vec![],
                tmux_ops,
                git_ops,
                git_provider_ops,
                agent_registry,
                sidebar_visible: false,
                sidebar_focused: false,
                projects: vec![],
                selected_project: 0,
                show_project_list: false,
                shell_popup: None,
                file_search: None,
                skill_search: None,
                task_ref_search: None,
                highlighted_references: HashSet::new(),
                task_search: None,
                pr_confirm_popup: None,
                review_to_running_task_id: None,
                diff_popup: None,
                pr_generation_rx: None,
                pr_status_popup: None,
                pr_creation_rx: None,
                setup_rx: None,
                done_confirm_popup: None,
                move_confirm_popup: None,
                skip_move_confirm: false,
                delete_confirm_popup: None,
                review_confirm_popup: None,
                phase_status_cache: HashMap::new(),
                spinner_frame: 0,
                pane_content_hashes: HashMap::new(),
                merge_conflict_checked: HashSet::new(),
                stuck_task_notified: HashSet::new(),
                stuck_task_idle_since: HashMap::new(),
                cached_plugin: None,
                warning_message: None,
                plugin_select_popup: None,
                orchestrator_session: None,
                orchestrator_ready: Arc::new(AtomicBool::new(false)),
                orchestrator_last_content: String::new(),
                orchestrator_stable_since: None,
                orchestrator_last_check: Instant::now(),
                session_refresh_rx: None,
            },
        })
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

            // Check for worktree setup completion
            if let Some(ref rx) = self.state.setup_rx {
                if let Ok(result) = rx.try_recv() {
                    self.state.setup_rx = None;
                    if let Some(err) = result.error {
                        self.state.warning_message = Some((err, Instant::now()));
                    } else {
                        // Update task with worktree info from background setup
                        if let Some(db) = &self.state.db {
                            if let Ok(Some(mut task)) = db.get_task(&result.task_id) {
                                task.session_name = Some(result.session_name);
                                task.worktree_path = Some(result.worktree_path);
                                task.branch_name = Some(result.branch_name);
                                task.agent = result.agent;
                                task.plugin = result.plugin;
                                if let Some(status) = result.new_status {
                                    task.status = status;
                                }
                                task.updated_at = chrono::Utc::now();
                                let _ = db.update_task(&task);
                            }
                        }
                        self.refresh_tasks()?;
                    }
                }
            }

            // Process MCP transition requests from the command queue
            self.process_transition_requests()?;

            if event::poll(std::time::Duration::from_millis(100))? {
                if let Event::Key(key) = event::read()? {
                    if key.kind == KeyEventKind::Press {
                        self.handle_key(key)?;
                    }
                }
            }

            // Refresh shell popup content periodically (every poll cycle when open)
            if let Some(ref mut popup) = self.state.shell_popup {
                popup.cached_content = capture_tmux_pane_with_history(
                    &popup.window_name,
                    500,
                    self.state.tmux_ops.as_ref(),
                );
            }

            // Apply results from background session refresh (non-blocking)
            if let Some(ref rx) = self.state.session_refresh_rx {
                match rx.try_recv() {
                    Ok(result) => {
                        self.state.session_refresh_rx = None;
                        self.apply_session_refresh(result);
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        // Thread panicked or dropped sender — clear to allow future spawns
                        self.state.session_refresh_rx = None;
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {}
                }
            }
            // Spawn background refresh if not already running and cache expired
            self.maybe_spawn_session_refresh();

            // Deliver queued notifications to orchestrator when idle
            self.deliver_orchestrator_notifications();

            // Clear expired warning messages
            if let Some((_, created)) = &self.state.warning_message {
                if created.elapsed() >= std::time::Duration::from_secs(5) {
                    self.state.warning_message = None;
                }
            }
        }

        Ok(())
    }

    pub fn draw(&mut self) -> Result<()> {
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
        let plugin_label = state.config.workflow_plugin.as_deref().unwrap_or("agtx");
        let left = Span::styled(
            format!(" {} ", state.project_name),
            Style::default().fg(Color::Cyan).bold(),
        );
        let mut right_spans: Vec<Span> = Vec::new();
        if state.flags.experimental {
            let orch_active = state.orchestrator_session.is_some();
            if orch_active {
                right_spans.push(Span::styled("● ", Style::default().fg(Color::Green)));
                right_spans.push(Span::styled(
                    "orchestrator ",
                    Style::default().fg(Color::Green),
                ));
            }
            right_spans.push(Span::styled(
                "[O] ",
                Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
            ));
        }
        right_spans.extend([
            Span::styled(
                format!("{} ", plugin_label),
                Style::default().fg(hex_to_color(&state.config.theme.color_accent)),
            ),
            Span::styled(
                "[P] ",
                Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
            ),
        ]);
        let left_len = state.project_name.len() + 2;
        let right_len: usize = right_spans.iter().map(|s| s.content.len()).sum();
        let padding = (chunks[0].width as usize).saturating_sub(left_len + right_len + 2); // 2 for borders
        let mut spans = vec![left, Span::raw(" ".repeat(padding))];
        spans.extend(right_spans);
        let header =
            Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL));
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
            let tasks: Vec<&Task> = state
                .board
                .tasks
                .iter()
                .filter(|t| t.status == *status)
                .collect();

            let is_selected_column = state.board.selected_column == i;

            let title = format!(" {} ({}) ", status.display_name(), tasks.len());
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
            let visible_tasks: Vec<_> = tasks
                .iter()
                .skip(scroll_offset)
                .take(max_visible_cards)
                .collect();
            for (j, task) in visible_tasks.iter().enumerate() {
                let actual_index = scroll_offset + j;
                let is_selected = is_selected_column && state.board.selected_row == actual_index;

                let card_area = Rect {
                    x: inner_area.x,
                    y: inner_area.y + (j as u16 * card_height),
                    width: if needs_scrollbar {
                        inner_area.width.saturating_sub(1)
                    } else {
                        inner_area.width
                    },
                    height: card_height
                        .min(inner_area.height.saturating_sub(j as u16 * card_height)),
                };

                if card_area.height < 3 {
                    break;
                }

                Self::draw_task_card(
                    frame,
                    task,
                    card_area,
                    is_selected,
                    &state.config.theme,
                    state.phase_status_cache.get(&task.id),
                    state.spinner_frame,
                );
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
                let thumb_pos = (scroll_offset * scrollbar_height / total_tasks)
                    .min(scrollbar_height - thumb_height);

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

        // Footer with help (or transient warning)
        let has_cyclic_plugin = state
            .board
            .selected_task()
            .and_then(|t| t.plugin.as_ref())
            .and_then(|name| WorkflowPlugin::load(name, state.project_path.as_deref()).ok())
            .map_or(false, |p| p.cyclic);
        let (footer_text, footer_style) = if let Some((ref msg, created)) = state.warning_message {
            if created.elapsed() < std::time::Duration::from_secs(5) {
                (msg.clone(), Style::default().fg(Color::Yellow))
            } else {
                (
                    build_footer_text(
                        state.input_mode,
                        state.sidebar_focused,
                        state.board.selected_column,
                        has_cyclic_plugin,
                    ),
                    Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
                )
            }
        } else {
            (
                build_footer_text(
                    state.input_mode,
                    state.sidebar_focused,
                    state.board.selected_column,
                    has_cyclic_plugin,
                ),
                Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
            )
        };

        let footer = Paragraph::new(footer_text.as_str())
            .style(footer_style)
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);

        // Input overlay if in input mode
        if matches!(
            state.input_mode,
            InputMode::InputTitle | InputMode::SelectPlugin | InputMode::InputDescription
        ) {
            let input_area = centered_rect(55, 55, area);
            frame.render_widget(Clear, input_area);

            let is_editing = state.editing_task_id.is_some();
            let block_title = if is_editing {
                " Edit Task "
            } else {
                " New Task "
            };
            let text_color = hex_to_color(&state.config.theme.color_text);
            let highlight_color = hex_to_color(&state.config.theme.color_accent);
            let dimmed_color = hex_to_color(&state.config.theme.color_dimmed);
            let selected_color = hex_to_color(&state.config.theme.color_selected);
            let desc_color = hex_to_color(&state.config.theme.color_description);

            // Determine current step index: Title=0, Plugin=1, Prompt=2
            let step = match state.input_mode {
                InputMode::InputTitle => 0,
                InputMode::SelectPlugin => 1,
                InputMode::InputDescription => 2,
                _ => 0,
            };
            let step_labels = ["Title", "Plugin", "Prompt"];

            let mut lines: Vec<Line> = Vec::new();

            // Step indicator breadcrumb
            let mut breadcrumb_spans: Vec<Span> = Vec::new();
            breadcrumb_spans.push(Span::raw("  "));
            for (i, label) in step_labels.iter().enumerate() {
                let style = if i == step {
                    Style::default()
                        .fg(selected_color)
                        .add_modifier(Modifier::BOLD)
                } else if i < step {
                    Style::default().fg(highlight_color)
                } else {
                    Style::default().fg(dimmed_color)
                };
                breadcrumb_spans.push(Span::styled(*label, style));
                if i < step_labels.len() - 1 {
                    breadcrumb_spans.push(Span::styled("  ›  ", Style::default().fg(dimmed_color)));
                }
            }
            lines.push(Line::from(breadcrumb_spans));

            // Separator
            let inner_width = input_area.width.saturating_sub(4) as usize;
            lines.push(Line::from(Span::styled(
                format!("  {}", "─".repeat(inner_width.saturating_sub(2))),
                Style::default().fg(dimmed_color),
            )));
            lines.push(Line::from(""));

            // Completed fields shown as read-only context
            if step >= 1 {
                lines.push(Line::from(vec![
                    Span::styled("  Title: ", Style::default().fg(dimmed_color)),
                    Span::styled(&state.pending_task_title, Style::default().fg(text_color)),
                ]));
            }
            if step >= 2 {
                let plugin_name = state
                    .wizard_plugin_options
                    .get(state.wizard_selected_plugin)
                    .map(|o| o.label.as_str())
                    .unwrap_or("agtx");
                lines.push(Line::from(vec![
                    Span::styled("  Plugin: ", Style::default().fg(dimmed_color)),
                    Span::styled(plugin_name, Style::default().fg(text_color)),
                ]));
            }
            if step >= 1 {
                lines.push(Line::from(""));
            }

            // Active area content
            match state.input_mode {
                InputMode::InputTitle => {
                    let (before_cursor, after_cursor) = state
                        .input_buffer
                        .split_at(state.input_cursor.min(state.input_buffer.len()));
                    lines.push(Line::from(vec![
                        Span::styled(
                            "  Title: ",
                            Style::default()
                                .fg(selected_color)
                                .add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(before_cursor, Style::default().fg(text_color)),
                        Span::styled("█", Style::default().fg(selected_color)),
                        Span::styled(after_cursor, Style::default().fg(text_color)),
                    ]));
                }
                InputMode::SelectPlugin => {
                    let active_plugin = state.config.workflow_plugin.as_deref().unwrap_or("");
                    for (i, opt) in state.wizard_plugin_options.iter().enumerate() {
                        let is_sel = i == state.wizard_selected_plugin;
                        let marker = if is_sel { "  > " } else { "    " };
                        let is_project_default = (opt.name.is_empty() && active_plugin.is_empty())
                            || opt.name == active_plugin;
                        let check = if is_project_default { " ✓" } else { "" };
                        let name_style = if is_sel {
                            Style::default()
                                .fg(selected_color)
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(text_color)
                        };
                        lines.push(Line::from(vec![
                            Span::styled(marker, name_style),
                            Span::styled(format!("{:<14}", &opt.label), name_style),
                            Span::styled(&opt.description, Style::default().fg(desc_color)),
                            Span::styled(check, Style::default().fg(Color::Green)),
                        ]));
                    }
                }
                InputMode::InputDescription => {
                    let (before_cursor, after_cursor) = state
                        .input_buffer
                        .split_at(state.input_cursor.min(state.input_buffer.len()));
                    let full_text = format!("  Prompt: {}█{}", before_cursor, after_cursor);
                    // Split on newlines to handle multi-line descriptions
                    for part in full_text.split('\n') {
                        if !state.highlighted_references.is_empty() {
                            let styled = build_highlighted_text(
                                part,
                                &state.highlighted_references,
                                text_color,
                                highlight_color,
                            );
                            for line in styled.lines {
                                let owned_spans: Vec<Span<'static>> = line
                                    .spans
                                    .into_iter()
                                    .map(|s| Span::styled(s.content.into_owned(), s.style))
                                    .collect();
                                lines.push(Line::from(owned_spans));
                            }
                        } else {
                            lines.push(Line::from(Span::styled(
                                part.to_string(),
                                Style::default().fg(text_color),
                            )));
                        }
                    }
                }
                _ => {}
            }

            let content = Paragraph::new(Text::from(lines))
                .style(Style::default().fg(text_color))
                .wrap(Wrap { trim: false })
                .block(
                    Block::default()
                        .title(block_title)
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(selected_color)),
                );
            frame.render_widget(content, input_area);

            // Search dropdowns (only in InputDescription mode)
            if state.input_mode == InputMode::InputDescription {
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
                        let dropdown_area = if dropdown_area.y + dropdown_area.height > area.height
                        {
                            Rect {
                                y: input_area.y.saturating_sub(dropdown_height),
                                ..dropdown_area
                            }
                        } else {
                            dropdown_area
                        };

                        frame.render_widget(Clear, dropdown_area);
                        let file_selected_color = hex_to_color(&state.config.theme.color_selected);
                        let items: Vec<ListItem> = search
                            .matches
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
                        let list = List::new(items).block(
                            Block::default()
                                .title(" Files [↑↓] select [Tab/Enter] insert [Esc] cancel ")
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(Color::Cyan)),
                        );
                        frame.render_widget(list, dropdown_area);
                    }
                }

                // Skill search dropdown
                if let Some(ref search) = state.skill_search {
                    if !search.matches.is_empty() {
                        let dropdown_height = (search.matches.len() as u16 + 2).min(12);
                        let dropdown_area = Rect {
                            x: input_area.x + 2,
                            y: input_area.y + input_area.height,
                            width: input_area.width.saturating_sub(4),
                            height: dropdown_height,
                        };
                        let dropdown_area = if dropdown_area.y + dropdown_area.height > area.height
                        {
                            Rect {
                                y: input_area.y.saturating_sub(dropdown_height),
                                ..dropdown_area
                            }
                        } else {
                            dropdown_area
                        };

                        frame.render_widget(Clear, dropdown_area);
                        let skill_sel_color = hex_to_color(&state.config.theme.color_selected);
                        let accent = hex_to_color(&state.config.theme.color_accent);
                        let dim = hex_to_color(&state.config.theme.color_dimmed);
                        let items: Vec<ListItem> = search
                            .matches
                            .iter()
                            .enumerate()
                            .map(|(i, entry)| {
                                let (style, cmd_style, dsc_style) = if i == search.selected {
                                    let s = Style::default().bg(skill_sel_color).fg(Color::Black);
                                    (s, s, s)
                                } else {
                                    (
                                        Style::default(),
                                        Style::default().fg(accent),
                                        Style::default().fg(dim),
                                    )
                                };
                                let cmd_padded = format!(" {:<24}", entry.command);
                                ListItem::new(Line::from(vec![
                                    Span::styled(cmd_padded, cmd_style),
                                    Span::styled(&entry.description, dsc_style),
                                ]))
                                .style(style)
                            })
                            .collect();
                        let list = List::new(items).block(
                            Block::default()
                                .title(" Skills [↑↓] select [Tab/Enter] insert [Esc] cancel ")
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(Color::Cyan)),
                        );
                        frame.render_widget(list, dropdown_area);
                    }
                }

                // Task reference search dropdown
                if let Some(ref search) = state.task_ref_search {
                    if !search.matches.is_empty() {
                        let dropdown_height = (search.matches.len() as u16 + 2).min(12);
                        let dropdown_area = Rect {
                            x: input_area.x + 2,
                            y: input_area.y + input_area.height,
                            width: input_area.width.saturating_sub(4),
                            height: dropdown_height,
                        };
                        let dropdown_area = if dropdown_area.y + dropdown_area.height > area.height
                        {
                            Rect {
                                y: input_area.y.saturating_sub(dropdown_height),
                                ..dropdown_area
                            }
                        } else {
                            dropdown_area
                        };

                        frame.render_widget(Clear, dropdown_area);
                        let task_sel_color = hex_to_color(&state.config.theme.color_selected);
                        let accent = hex_to_color(&state.config.theme.color_accent);
                        let dim = hex_to_color(&state.config.theme.color_dimmed);
                        let items: Vec<ListItem> = search
                            .matches
                            .iter()
                            .enumerate()
                            .map(|(i, (_, title, status))| {
                                let (style, title_style, status_style) = if i == search.selected {
                                    let s = Style::default().bg(task_sel_color).fg(Color::Black);
                                    (s, s, s)
                                } else {
                                    (
                                        Style::default(),
                                        Style::default().fg(accent),
                                        Style::default().fg(dim),
                                    )
                                };
                                let status_badge = format!("  [{}]", status.as_str());
                                ListItem::new(Line::from(vec![
                                    Span::styled(format!(" {}", title), title_style),
                                    Span::styled(status_badge, status_style),
                                ]))
                                .style(style)
                            })
                            .collect();
                        let list = List::new(items).block(
                            Block::default()
                                .title(" Tasks [↑↓] select [Tab/Enter] insert [Esc] cancel ")
                                .borders(Borders::ALL)
                                .border_style(Style::default().fg(Color::Cyan)),
                        );
                        frame.render_widget(list, dropdown_area);
                    }
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
            let items: Vec<ListItem> = search
                .matches
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

            let list = List::new(items).block(
                Block::default()
                    .title(" [↑↓] select [Enter] jump [Esc] cancel ")
                    .borders(Borders::ALL)
                    .border_style(
                        Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
                    ),
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
                    .border_style(
                        Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
                    );
                frame.render_widget(main_block, popup_area);

                // Spinner animation based on frame count
                let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                let spinner_idx = (std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis()
                    / 100) as usize
                    % spinner_chars.len();
                let spinner = spinner_chars[spinner_idx];

                let agent_name = state.config.default_agent.clone();
                let loading_text = format!(
                    "{} Generating PR description with {}...",
                    spinner, agent_name
                );
                let loading = Paragraph::new(loading_text)
                    .style(Style::default().fg(Color::Cyan))
                    .alignment(ratatui::layout::Alignment::Center);

                // Center vertically within the popup
                let inner = popup_area.inner(ratatui::layout::Margin {
                    horizontal: 2,
                    vertical: popup_area.height.saturating_sub(3) / 2,
                });
                frame.render_widget(loading, inner);
            } else {
                let popup_chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(3), // Title input
                        Constraint::Min(0),    // Body input
                        Constraint::Length(1), // Help line
                    ])
                    .margin(1)
                    .split(popup_area);

                // Main border
                let main_block = Block::default()
                    .title(" Create Pull Request ")
                    .borders(Borders::ALL)
                    .border_style(
                        Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)),
                    );
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
                PrCreationStatus::Creating => (
                    " Creating Pull Request ",
                    hex_to_color(&state.config.theme.color_selected),
                ),
                PrCreationStatus::Pushing => (
                    " Pushing Changes ",
                    hex_to_color(&state.config.theme.color_selected),
                ),
                PrCreationStatus::Success => (" Pull Request Created ", Color::Green),
                PrCreationStatus::Error => (" Error Creating PR ", Color::Red),
            };

            let main_block = Block::default()
                .title(title)
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color));
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });

            match popup.status {
                PrCreationStatus::Creating => {
                    let spinner_chars = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
                    let spinner_idx = (std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis()
                        / 100) as usize
                        % spinner_chars.len();
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
                        .as_millis()
                        / 100) as usize
                        % spinner_chars.len();
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
                .border_style(
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
                );
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });
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
                DoneConfirmPrState::UncommittedChanges => String::from(
                    "There are uncommitted changes in the worktree.\n\nAre you sure you want to move this task to Done?\n\nUncommitted changes will be lost.\nWorktree will be deleted, tmux coding session killed.\nBranch kept locally.\n\n[y] Yes, move to Done    [n/Esc] Cancel"
                ),
            };
            let content = Paragraph::new(text)
                .style(Style::default().fg(Color::White))
                .alignment(ratatui::layout::Alignment::Center)
                .wrap(Wrap { trim: false });
            frame.render_widget(content, inner);
        }

        // Move confirmation popup (phase incomplete)
        if let Some(ref popup) = state.move_confirm_popup {
            let popup_area = centered_rect(50, 20, area);
            frame.render_widget(Clear, popup_area);

            let phase_name = match popup.from_status {
                TaskStatus::Planning => "Planning",
                TaskStatus::Running => "Running",
                TaskStatus::Review => "Review",
                _ => "Current",
            };
            let main_block = Block::default()
                .title(format!(" {} Phase Incomplete ", phase_name))
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
                );
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });
            let text = format!(
                "The agent is still working and the {} artifact\nhas not been created yet.\n\nAre you sure you want to move this task forward?\n\n[y] Yes, move    [n/Esc] Cancel",
                phase_name.to_lowercase()
            );
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

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });
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
                .border_style(
                    Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)),
                );
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 2,
            });
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

        // Plugin selection popup
        if let Some(ref popup) = state.plugin_select_popup {
            let popup_area = centered_rect(50, 40, area);
            frame.render_widget(Clear, popup_area);

            let main_block = Block::default()
                .title(" Select Workflow Plugin ")
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)),
                );
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });
            let mut lines: Vec<Line> = Vec::new();

            for (i, opt) in popup.options.iter().enumerate() {
                let is_selected = i == popup.selected;
                let marker = if is_selected { "> " } else { "  " };
                let check = if opt.active { " ✓" } else { "" };

                let name_style = if is_selected {
                    Style::default()
                        .fg(hex_to_color(&state.config.theme.color_selected))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(hex_to_color(&state.config.theme.color_text))
                };

                lines.push(Line::from(vec![
                    Span::styled(marker, name_style),
                    Span::styled(&opt.label, name_style),
                    Span::styled(check, Style::default().fg(Color::Green)),
                ]));

                let desc_style =
                    Style::default().fg(hex_to_color(&state.config.theme.color_description));
                lines.push(Line::from(Span::styled(
                    format!("  {}", opt.description),
                    desc_style,
                )));
                lines.push(Line::from(""));
            }

            lines.push(Line::from(Span::styled(
                "  [Enter] select  [Esc] cancel",
                Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
            )));

            let content = Paragraph::new(lines);
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
            let title_bar = Paragraph::new(title).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(hex_to_color(&state.config.theme.color_popup_header)),
            );
            frame.render_widget(title_bar, popup_chunks[0]);

            // Diff content with syntax highlighting
            let lines: Vec<Line> = popup
                .diff_content
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

            let diff_content =
                Paragraph::new(lines).block(Block::default().borders(Borders::ALL).border_style(
                    Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)),
                ));
            frame.render_widget(diff_content, popup_chunks[1]);

            // Footer with scroll info
            let total_lines = popup.diff_content.lines().count();
            let footer_text = format!(
                " [j/k] scroll  [d/u] page  [g/G] top/bottom  [q/Esc] close  ({}/{}) ",
                popup.scroll_offset + 1,
                total_lines
            );
            let footer = Paragraph::new(footer_text).style(
                Style::default()
                    .fg(Color::Black)
                    .bg(hex_to_color(&state.config.theme.color_dimmed)),
            );
            frame.render_widget(footer, popup_chunks[2]);
        }
    }

    fn draw_shell_popup(popup: &ShellPopup, frame: &mut Frame, area: Rect, theme: &ThemeConfig) {
        let popup_area =
            centered_rect_fixed_width(SHELL_POPUP_WIDTH, SHELL_POPUP_HEIGHT_PERCENT, area);

        // Parse ANSI escape sequences for colors
        let styled_lines = parse_ansi_to_lines(&popup.cached_content);

        // Build colors from theme
        let colors = shell_popup::ShellPopupColors {
            border: hex_to_color(&theme.color_popup_border),
            header_fg: Color::Black,
            header_bg: hex_to_color(&theme.color_popup_header),
            footer_fg: Color::Black,
            footer_bg: hex_to_color(&theme.color_dimmed),
            escalation_fg: Color::Black,
            escalation_bg: Color::Yellow,
        };

        shell_popup::render_shell_popup(popup, frame, popup_area, styled_lines, &colors);
    }

    fn draw_task_card(
        frame: &mut Frame,
        task: &Task,
        area: Rect,
        is_selected: bool,
        theme: &ThemeConfig,
        phase_status: Option<&(PhaseStatus, Instant)>,
        spinner_frame: usize,
    ) {
        let border_style = if is_selected {
            Style::default().fg(hex_to_color(&theme.color_selected))
        } else {
            Style::default().fg(hex_to_color(&theme.color_normal))
        };

        let title_style = if is_selected {
            Style::default()
                .fg(hex_to_color(&theme.color_selected))
                .bold()
        } else {
            Style::default().fg(hex_to_color(&theme.color_text)).bold()
        };

        // Truncate title to fit (char-safe for UTF-8)
        let max_title_len = area.width.saturating_sub(4) as usize;
        let title: String = if task.title.chars().count() > max_title_len {
            let truncated: String = task
                .title
                .chars()
                .take(max_title_len.saturating_sub(3))
                .collect();
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

        // Title line with optional phase indicator
        let show_indicator = matches!(
            task.status,
            TaskStatus::Planning | TaskStatus::Running | TaskStatus::Review
        ) || (task.status == TaskStatus::Backlog
            && task.session_name.is_some());

        if show_indicator {
            const SPINNER_FRAMES: &[&str] = &[
                "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}",
                "\u{2827}", "\u{2807}", "\u{280f}",
            ];
            let indicator = match phase_status {
                Some((PhaseStatus::Ready, _)) => {
                    Span::styled("\u{2713} ", Style::default().fg(Color::Green))
                }
                Some((PhaseStatus::Working, _)) => {
                    let spinner = SPINNER_FRAMES[spinner_frame % SPINNER_FRAMES.len()];
                    Span::styled(format!("{} ", spinner), Style::default().fg(Color::Yellow))
                }
                Some((PhaseStatus::Idle, _)) => Span::styled(
                    "\u{23f8} ",
                    Style::default().fg(hex_to_color(&theme.color_dimmed)),
                ),
                Some((PhaseStatus::Exited, _)) => {
                    Span::styled("\u{2717} ", Style::default().fg(Color::Red))
                }
                None => Span::raw(""),
            };
            // Escalation warning indicator
            let warn_span = if task.escalation_note.is_some() {
                Span::styled(
                    "\u{26a0} ",
                    Style::default()
                        .fg(hex_to_color(&theme.color_accent))
                        .bold(),
                )
            } else {
                Span::raw("")
            };
            let title_spans =
                Line::from(vec![indicator, warn_span, Span::styled(title, title_style)]);
            let title_line = Paragraph::new(title_spans);
            let title_area = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(title_line, title_area);
        } else {
            let title_line = Paragraph::new(title).style(title_style);
            let title_area = Rect {
                x: inner.x,
                y: inner.y,
                width: inner.width,
                height: 1,
            };
            frame.render_widget(title_line, title_area);
        }

        // Footer line with agent name (for active tasks)
        let show_agent = task.status != TaskStatus::Backlog || task.session_name.is_some();
        let footer_height = if show_agent && inner.height > 2 {
            1u16
        } else {
            0u16
        };

        // Preview area (below title) - always show description
        if inner.height > 1 + footer_height {
            let preview_area = Rect {
                x: inner.x,
                y: inner.y + 1,
                width: inner.width,
                height: inner.height.saturating_sub(1 + footer_height),
            };

            // Show description or placeholder
            let preview_text = task.description.as_deref().unwrap_or("No description");

            // Truncate description to fit preview area
            let max_chars = (preview_area.width as usize) * (preview_area.height as usize);
            let truncated: String = if preview_text.chars().count() > max_chars {
                format!(
                    "{}...",
                    preview_text
                        .chars()
                        .take(max_chars.saturating_sub(3))
                        .collect::<String>()
                )
            } else {
                preview_text.to_string()
            };

            let preview = Paragraph::new(truncated)
                .style(
                    Style::default()
                        .fg(hex_to_color(&theme.color_description))
                        .italic(),
                )
                .wrap(Wrap { trim: true });
            frame.render_widget(preview, preview_area);
        }

        // Agent footer
        if footer_height > 0 {
            let footer_area = Rect {
                x: inner.x,
                y: inner.y + inner.height.saturating_sub(1),
                width: inner.width,
                height: 1,
            };
            let agent_style = match task.agent.as_str() {
                "claude" => Style::default().fg(Color::Rgb(227, 148, 62)), // orange
                "gemini" => Style::default().fg(Color::Rgb(234, 130, 180)), // pink
                "opencode" => Style::default().fg(Color::White).bg(Color::Rgb(80, 80, 80)), // white on grey
                "codex" => Style::default().fg(Color::White).bg(Color::Rgb(20, 20, 20)), // white on black
                _ => Style::default().fg(Color::White),
            };
            let agent_label = Paragraph::new(format!(" {} ", task.agent))
                .style(agent_style)
                .alignment(Alignment::Right);
            frame.render_widget(agent_label, footer_area);
        }
    }

    fn draw_sidebar(state: &AppState, frame: &mut Frame, area: Rect) {
        // Show projects from database
        let current_path = state
            .project_path
            .as_ref()
            .map(|p| p.to_string_lossy().to_string());

        let items: Vec<ListItem> = state
            .projects
            .iter()
            .enumerate()
            .map(|(i, project)| {
                let is_selected = i == state.selected_project && state.sidebar_focused;
                let is_current = current_path.as_ref() == Some(&project.path);

                let style = if is_selected {
                    Style::default()
                        .bg(hex_to_color(&state.config.theme.color_selected))
                        .fg(Color::Black)
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
        let dimmed_color = hex_to_color(&state.config.theme.color_dimmed);
        let selected_color = hex_to_color(&state.config.theme.color_selected);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(10), // Logo + subtitle
                Constraint::Min(0),     // Project list or options
                Constraint::Length(3),  // Footer
            ])
            .split(area);

        // ASCII art logo — hardcoded gold to match docs/banner.svg
        let logo_color = Color::Rgb(234, 212, 154); // #ead49a
        let logo = vec![
            Line::from(""),
            Line::from(Span::styled(
                " █████╗  ██████╗████████╗██╗  ██╗",
                Style::default().fg(logo_color).bold(),
            )),
            Line::from(Span::styled(
                "██╔══██╗██╔════╝╚══██╔══╝╚██╗██╔╝",
                Style::default().fg(logo_color).bold(),
            )),
            Line::from(Span::styled(
                "███████║██║  ███╗  ██║    ╚███╔╝ ",
                Style::default().fg(logo_color).bold(),
            )),
            Line::from(Span::styled(
                "██╔══██║██║   ██║  ██║    ██╔██╗ ",
                Style::default().fg(logo_color).bold(),
            )),
            Line::from(Span::styled(
                "██║  ██║╚██████╔╝  ██║   ██╔╝ ██╗",
                Style::default().fg(logo_color).bold(),
            )),
            Line::from(Span::styled(
                "╚═╝  ╚═╝ ╚═════╝   ╚═╝   ╚═╝  ╚═╝",
                Style::default().fg(logo_color).bold(),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Autonomous multi-session spec-driven AI coding orchestration in the terminal",
                Style::default().fg(dimmed_color),
            )),
        ];
        let logo_widget = Paragraph::new(logo).alignment(Alignment::Center);
        frame.render_widget(logo_widget, chunks[0]);

        // Project list or options
        if state.show_project_list && !state.projects.is_empty() {
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
            frame.render_widget(list, chunks[1]);
        } else {
            let options = Paragraph::new(
                "\n  [p] Open existing project\n  [n] Create new project in current directory\n",
            )
            .block(
                Block::default()
                    .title(" Options ")
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(dimmed_color)),
            );
            frame.render_widget(options, chunks[1]);
        }

        // Footer
        let footer = Paragraph::new(" [p] projects  [n] new project  [q] quit ")
            .style(Style::default().fg(dimmed_color))
            .block(Block::default().borders(Borders::ALL));
        frame.render_widget(footer, chunks[2]);
    }

    pub fn handle_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Handle PR status popup if open (loading/success/error)
        if let Some(ref popup) = self.state.pr_status_popup {
            // Only allow closing if not in Creating/Pushing state
            if popup.status != PrCreationStatus::Creating
                && popup.status != PrCreationStatus::Pushing
            {
                if matches!(key.code, KeyCode::Enter | KeyCode::Esc) {
                    self.state.pr_status_popup = None;
                }
            }
            return Ok(());
        }

        // Handle Move confirmation popup if open (phase incomplete)
        if self.state.move_confirm_popup.is_some() {
            return self.handle_move_confirm_key(key);
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

        // Handle plugin selection popup if open
        if self.state.plugin_select_popup.is_some() {
            return self.handle_plugin_select_key(key);
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
            AppMode::Project(_) => match self.state.input_mode {
                InputMode::Normal => self.handle_normal_key(key.code),
                InputMode::InputTitle => self.handle_title_input(key),
                InputMode::SelectPlugin => self.handle_plugin_select_wizard(key),
                InputMode::InputDescription => self.handle_description_input(key),
            },
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

    fn handle_move_confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        if self.state.move_confirm_popup.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    self.state.move_confirm_popup = None;
                    self.state.skip_move_confirm = true;
                    self.move_task_right()?;
                    self.state.skip_move_confirm = false;
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.state.move_confirm_popup = None;
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

    fn open_plugin_select_popup(&mut self) {
        let current = self
            .state
            .config
            .workflow_plugin
            .as_deref()
            .unwrap_or("agtx");
        let mut options = vec![PluginOption {
            name: "agtx".to_string(),
            label: "agtx".to_string(),
            description: "Built-in workflow with skills and prompts".to_string(),
            active: current == "agtx",
        }];
        for (name, desc, _content) in skills::BUNDLED_PLUGINS {
            if *name == "agtx" {
                continue;
            }
            options.push(PluginOption {
                name: name.to_string(),
                label: name.to_string(),
                description: desc.to_string(),
                active: current == *name,
            });
        }
        let selected = options.iter().position(|o| o.active).unwrap_or(0);
        self.state.plugin_select_popup = Some(PluginSelectPopup { selected, options });
    }

    fn handle_plugin_select_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        if let Some(ref mut popup) = self.state.plugin_select_popup {
            match key.code {
                KeyCode::Char('j') | KeyCode::Down => {
                    if popup.selected < popup.options.len().saturating_sub(1) {
                        popup.selected += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if popup.selected > 0 {
                        popup.selected -= 1;
                    }
                }
                KeyCode::Enter => {
                    let name = popup.options[popup.selected].name.clone();
                    self.state.plugin_select_popup = None;
                    self.install_plugin(&name)?;
                }
                KeyCode::Esc => {
                    self.state.plugin_select_popup = None;
                }
                _ => {}
            }
        }
        Ok(())
    }

    fn install_plugin(&mut self, plugin_name: &str) -> Result<()> {
        let Some(project_path) = self.state.project_path.clone() else {
            return Ok(());
        };

        // Load current project config
        let mut project_config = ProjectConfig::load(&project_path).unwrap_or_default();

        if plugin_name.is_empty() || plugin_name == "agtx" {
            // agtx is the default — clear explicit setting
            project_config.workflow_plugin = None;
        } else {
            // Find bundled plugin content and write it
            if let Some((_name, _desc, content)) = skills::BUNDLED_PLUGINS
                .iter()
                .find(|(n, _, _)| *n == plugin_name)
            {
                let plugin_dir = project_path.join(".agtx").join("plugins").join(plugin_name);
                let _ = std::fs::create_dir_all(&plugin_dir);
                let _ = std::fs::write(plugin_dir.join("plugin.toml"), content);
            }
            project_config.workflow_plugin = Some(plugin_name.to_string());
        }

        // Save project config
        project_config.save(&project_path)?;

        // Refresh merged config and cached plugin
        let global_config = GlobalConfig::load().unwrap_or_default();
        self.state.config = MergedConfig::merge(&global_config, &project_config);
        self.state.cached_plugin = Some(load_plugin_if_configured(
            &self.state.config,
            Some(&project_path),
        ));

        Ok(())
    }

    fn force_move_to_done(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(project_path)) = (&self.state.db, self.state.project_path.clone()) {
            if let Some(mut task) = db.get_task(task_id)? {
                let session_name = task.session_name.clone();
                let worktree_path = task.worktree_path.clone();
                let branch_name = task.branch_name.clone();

                // Update task status immediately
                task.session_name = None;
                task.worktree_path = None;
                task.status = TaskStatus::Done;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;

                // Cleanup in background (archive, kill tmux, remove worktree)
                let tmux_ops = Arc::clone(&self.state.tmux_ops);
                let git_ops = Arc::clone(&self.state.git_ops);
                let task_id = task.id.clone();
                let cleanup_script = self.state.config.cleanup_script.clone();
                std::thread::spawn(move || {
                    cleanup_task_resources(
                        &task_id,
                        &branch_name,
                        &session_name,
                        &worktree_path,
                        cleanup_script.as_deref(),
                        &project_path,
                        tmux_ops.as_ref(),
                        git_ops.as_ref(),
                    );
                });
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
                let agent_ops = self
                    .state
                    .agent_registry
                    .get(&self.state.config.default_agent);
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
                        self.create_pr_and_move_to_review_with_content(
                            &task_id, &pr_title, &pr_body,
                        )?;
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

    fn create_pr_and_move_to_review_with_content(
        &mut self,
        task_id: &str,
        pr_title: &str,
        pr_body: &str,
    ) -> Result<()> {
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
                let agent_ops = self
                    .state
                    .agent_registry
                    .get(&self.state.config.default_agent);

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
                    if let Some((task_id, _, status)) = search.matches.get(search.selected).cloned()
                    {
                        // Find column index for this status
                        let col_idx = TaskStatus::columns()
                            .iter()
                            .position(|s| *s == status)
                            .unwrap_or(0);
                        self.state.board.selected_column = col_idx;

                        // Find row index for this task
                        let tasks_in_col: Vec<_> = self
                            .state
                            .board
                            .tasks
                            .iter()
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
            KeyCode::Char('k') | KeyCode::Char('p')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                if let Some(ref mut search) = self.state.task_search {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                false
            }
            KeyCode::Char('j') | KeyCode::Char('n')
                if key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
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
                let query = self
                    .state
                    .task_search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default();
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
                let query = self
                    .state
                    .task_search
                    .as_ref()
                    .map(|s| s.query.clone())
                    .unwrap_or_default();
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

        let mut matches: Vec<(String, String, TaskStatus, i32)> = self
            .state
            .board
            .tasks
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

        matches
            .into_iter()
            .take(10)
            .map(|(id, title, status, _)| (id, title, status))
            .collect()
    }

    fn handle_shell_popup_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        use crossterm::event::KeyModifiers;

        if let Some(ref mut popup) = self.state.shell_popup {
            let window_name = popup.window_name.clone();
            let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

            // Dismiss escalation note on any key press (before forwarding)
            if popup.escalation_note.is_some() {
                let task_id = popup.task_id.clone();
                popup.escalation_note = None;
                if let Some(id) = task_id {
                    if let Some(db) = &self.state.db {
                        if let Ok(Some(mut task)) = db.get_task(&id) {
                            task.escalation_note = None;
                            task.updated_at = chrono::Utc::now();
                            let _ = db.update_task(&task);
                        }
                    }
                    // Update the in-memory task list too
                    if let Some(t) = self.state.board.tasks.iter_mut().find(|t| t.id == id) {
                        t.escalation_note = None;
                    }
                }
                // Return early so the keypress only dismisses the banner, not forwarded
                return Ok(());
            }

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
                    popup.cached_content = capture_tmux_pane_with_history(
                        &window_name,
                        500,
                        self.state.tmux_ops.as_ref(),
                    );
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
        if self.state.show_project_list {
            match key {
                KeyCode::Char('q') => self.state.should_quit = true,
                KeyCode::Char('j') | KeyCode::Down => {
                    if self.state.selected_project < self.state.projects.len().saturating_sub(1) {
                        self.state.selected_project += 1;
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if self.state.selected_project > 0 {
                        self.state.selected_project -= 1;
                    }
                }
                KeyCode::Enter => {
                    if let Some(project) = self
                        .state
                        .projects
                        .get(self.state.selected_project)
                        .cloned()
                    {
                        self.switch_to_project(&project)?;
                        self.state.mode = AppMode::Project(PathBuf::from(&project.path));
                        self.state.sidebar_visible = false;
                    }
                }
                KeyCode::Esc => {
                    self.state.show_project_list = false;
                }
                _ => {}
            }
        } else {
            match key {
                KeyCode::Char('q') => self.state.should_quit = true,
                KeyCode::Char('p') => {
                    self.state.show_project_list = true;
                }
                KeyCode::Char('n') => {
                    let current_dir = std::env::current_dir()?;
                    if crate::git::is_git_repo(&current_dir) {
                        let canonical = current_dir.canonicalize().unwrap_or(current_dir);
                        let name = canonical
                            .file_name()
                            .and_then(|n| n.to_str())
                            .unwrap_or("unknown")
                            .to_string();
                        let project = ProjectInfo {
                            name: name.clone(),
                            path: canonical.to_string_lossy().to_string(),
                        };
                        self.switch_to_project(&project)?;
                        self.state.mode = AppMode::Project(canonical);
                        self.state.sidebar_visible = false;
                    }
                }
                _ => {}
            }
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
                        if let Some(project) = self
                            .state
                            .projects
                            .get(self.state.selected_project)
                            .cloned()
                        {
                            self.switch_to_project_keep_sidebar(&project)?;
                        }
                    }
                }
                KeyCode::Char('k') | KeyCode::Up => {
                    if self.state.selected_project > 0 {
                        self.state.selected_project -= 1;
                        // Switch to project immediately on cursor move
                        if let Some(project) = self
                            .state
                            .projects
                            .get(self.state.selected_project)
                            .cloned()
                        {
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
                if let Some(task) = self.state.board.selected_task() {
                    if task.status == TaskStatus::Backlog && task.session_name.is_some() {
                        // Backlog task with active research session — open tmux popup
                        self.open_selected_task()?;
                    } else if task.status == TaskStatus::Backlog {
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
            KeyCode::Char('R') => {
                if let Some(task) = self.state.board.selected_task() {
                    if task.status == TaskStatus::Backlog && task.session_name.is_none() {
                        let task_id = task.id.clone();
                        self.start_research(&task_id)?;
                    }
                }
            }
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
            KeyCode::Char('p') => {
                // Cyclic: Review → Planning (next phase) — only when plugin is cyclic
                if let Some(task) = self.state.board.selected_task() {
                    if task.status == TaskStatus::Review {
                        let plugin = self.load_task_plugin(&task);
                        if plugin.as_ref().map_or(false, |p| p.cyclic) {
                            let task_id = task.id.clone();
                            self.move_review_to_planning(&task_id)?;
                        }
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
            KeyCode::Char('P') => {
                // Open plugin selection popup
                self.open_plugin_select_popup();
            }
            KeyCode::Char('O') if self.state.flags.experimental => {
                // Toggle orchestrator agent (experimental)
                self.toggle_orchestrator()?;
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_title_input(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let has_alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
        match key.code {
            KeyCode::Esc => {
                self.cancel_wizard();
            }
            KeyCode::Enter => {
                if !self.state.input_buffer.is_empty() {
                    self.state.pending_task_title = self.state.input_buffer.clone();
                    self.state.input_buffer.clear();
                    self.state.input_cursor = 0;
                    self.advance_from_title();
                }
            }
            KeyCode::Left if has_alt => {
                self.state.input_cursor =
                    word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Right if has_alt => {
                self.state.input_cursor =
                    word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            // macOS: Option+Left/Right sends Alt+b / Alt+f
            KeyCode::Char('b') if has_alt => {
                self.state.input_cursor =
                    word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Char('f') if has_alt => {
                self.state.input_cursor =
                    word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            // Alt+Backspace: delete word backward (macOS Option+Delete)
            KeyCode::Backspace if has_alt => {
                let new_pos = word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
                self.state
                    .input_buffer
                    .drain(new_pos..self.state.input_cursor);
                self.state.input_cursor = new_pos;
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

    fn handle_plugin_select_wizard(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Esc => {
                self.cancel_wizard();
            }
            KeyCode::Char('j') | KeyCode::Down => {
                let max = self.state.wizard_plugin_options.len().saturating_sub(1);
                if self.state.wizard_selected_plugin < max {
                    self.state.wizard_selected_plugin += 1;
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.state.wizard_selected_plugin > 0 {
                    self.state.wizard_selected_plugin -= 1;
                }
            }
            KeyCode::Tab => {
                let len = self.state.wizard_plugin_options.len();
                if len > 0 {
                    self.state.wizard_selected_plugin =
                        (self.state.wizard_selected_plugin + 1) % len;
                }
            }
            KeyCode::Enter => {
                self.init_description_input();
            }
            _ => {}
        }
        Ok(())
    }

    fn handle_description_input(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        // Handle task reference search mode if active
        if let Some(ref mut search) = self.state.task_ref_search {
            match key.code {
                KeyCode::Esc => {
                    // Cancel task ref search, remove `!` + pattern from buffer
                    let remove_end = search.start_pos + 1 + search.pattern.len();
                    let suffix = self.state.input_buffer[remove_end..].to_string();
                    self.state.input_buffer.truncate(search.start_pos);
                    self.state.input_buffer.push_str(&suffix);
                    self.state.input_cursor = search.start_pos;
                    self.state.task_ref_search = None;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    if let Some((task_id, title, _status)) =
                        search.matches.get(search.selected).cloned()
                    {
                        // Replace `!` + pattern with ![task-title]
                        let pattern_end = search.start_pos + 1 + search.pattern.len();
                        let suffix = self.state.input_buffer[pattern_end..].to_string();
                        self.state.input_buffer.truncate(search.start_pos);
                        let ref_text = format!("![{}]", title);
                        self.state.input_buffer.push_str(&ref_text);
                        self.state.input_cursor = self.state.input_buffer.len();
                        self.state.input_buffer.push_str(&suffix);
                        self.state.highlighted_references.insert(ref_text);
                        self.state.wizard_referenced_task_ids.insert(task_id);
                    }
                    self.state.task_ref_search = None;
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
                KeyCode::Backspace => {
                    if search.pattern.is_empty() {
                        // Remove the `!` trigger character
                        if self.state.input_cursor > 0 {
                            self.state.input_cursor -= 1;
                            self.state.input_buffer.remove(self.state.input_cursor);
                        }
                        self.state.task_ref_search = None;
                    } else {
                        search.pattern.pop();
                        if self.state.input_cursor > 0 {
                            self.state.input_cursor -= 1;
                            self.state.input_buffer.remove(self.state.input_cursor);
                        }
                        let query = search.pattern.clone();
                        let matches = self.get_all_task_matches(&query);
                        if let Some(ref mut search) = self.state.task_ref_search {
                            search.matches = matches;
                            search.selected = 0;
                        }
                    }
                }
                KeyCode::Char(c) => {
                    if let Some(ref mut search) = self.state.task_ref_search {
                        search.pattern.push(c);
                    }
                    self.state.input_buffer.insert(self.state.input_cursor, c);
                    self.state.input_cursor += 1;
                    let query = self
                        .state
                        .task_ref_search
                        .as_ref()
                        .map(|s| s.pattern.clone())
                        .unwrap_or_default();
                    let matches = self.get_all_task_matches(&query);
                    if let Some(ref mut search) = self.state.task_ref_search {
                        search.matches = matches;
                        search.selected = 0;
                    }
                }
                _ => {}
            }
            return Ok(());
        }

        // Handle skill search mode if active
        if let Some(ref mut search) = self.state.skill_search {
            match key.code {
                KeyCode::Esc => {
                    // Cancel skill search, remove `/` + pattern from buffer
                    let remove_end = search.start_pos + 1 + search.pattern.len();
                    let suffix = self.state.input_buffer[remove_end..].to_string();
                    self.state.input_buffer.truncate(search.start_pos);
                    self.state.input_buffer.push_str(&suffix);
                    self.state.input_cursor = search.start_pos;
                    self.state.skill_search = None;
                }
                KeyCode::Enter | KeyCode::Tab => {
                    if let Some(entry) = search.matches.get(search.selected).cloned() {
                        // Replace `/` + pattern with the full command
                        let pattern_end = search.start_pos + 1 + search.pattern.len();
                        let suffix = self.state.input_buffer[pattern_end..].to_string();
                        self.state.input_buffer.truncate(search.start_pos);
                        self.state.input_buffer.push_str(&entry.command);
                        self.state.input_cursor = self.state.input_buffer.len();
                        self.state.input_buffer.push_str(&suffix);
                        self.state.highlighted_references.insert(entry.command);
                    }
                    self.state.skill_search = None;
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
                KeyCode::Char('k') | KeyCode::Char('p')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                KeyCode::Char('j') | KeyCode::Char('n')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    if search.selected < search.matches.len().saturating_sub(1) {
                        search.selected += 1;
                    }
                }
                KeyCode::Backspace => {
                    if search.pattern.is_empty() {
                        // Cancel search if pattern is empty
                        self.state.input_buffer.remove(search.start_pos); // Remove the `!`
                        self.state.input_cursor = search.start_pos;
                        self.state.skill_search = None;
                    } else {
                        search.pattern.pop();
                        self.state.input_cursor = self.state.input_cursor.saturating_sub(1);
                        self.state.input_buffer.remove(self.state.input_cursor);
                        self.update_skill_search_matches();
                    }
                }
                KeyCode::Char(c) => {
                    search.pattern.push(c);
                    self.state.input_buffer.insert(self.state.input_cursor, c);
                    self.state.input_cursor += 1;
                    self.update_skill_search_matches();
                }
                _ => {}
            }
            return Ok(());
        }

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
                        self.state.highlighted_references.insert(selected_file);
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
                KeyCode::Char('k') | KeyCode::Char('p')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
                    if search.selected > 0 {
                        search.selected -= 1;
                    }
                }
                KeyCode::Char('j') | KeyCode::Char('n')
                    if key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL) =>
                {
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
                self.cancel_wizard();
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
                    self.cancel_wizard();
                }
            }
            KeyCode::Left if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor =
                    word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Right if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor =
                    word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            // macOS: Option+Left/Right sends Alt+b / Alt+f
            KeyCode::Char('b') if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor =
                    word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
            }
            KeyCode::Char('f') if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.state.input_cursor =
                    word_boundary_right(&self.state.input_buffer, self.state.input_cursor);
            }
            // Alt+Backspace: delete word backward (macOS Option+Delete)
            KeyCode::Backspace if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                let new_pos = word_boundary_left(&self.state.input_buffer, self.state.input_cursor);
                self.state
                    .input_buffer
                    .drain(new_pos..self.state.input_cursor);
                self.state.input_cursor = new_pos;
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
                let trigger = if let KeyCode::Char(c) = key.code {
                    c
                } else {
                    '#'
                };
                let start_pos = self.state.input_cursor;
                self.state
                    .input_buffer
                    .insert(self.state.input_cursor, trigger);
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
            KeyCode::Char('/')
                if self.state.input_cursor == 0
                    || matches!(
                        self.state
                            .input_buffer
                            .as_bytes()
                            .get(self.state.input_cursor.wrapping_sub(1)),
                        Some(&b'\n') | Some(&b' ')
                    ) =>
            {
                // Start skill search at cursor position (at start of line or after space)
                let start_pos = self.state.input_cursor;
                self.state.input_buffer.insert(self.state.input_cursor, '/');
                self.state.input_cursor += 1;

                // Start with bundled skills (always available, no filesystem needed)
                let mut seen = std::collections::HashSet::new();
                let mut all_skills: Vec<SkillEntry> =
                    skills::enumerate_available_skills(&self.state.config.default_agent)
                        .into_iter()
                        .map(|(command, description)| {
                            seen.insert(command.clone());
                            SkillEntry {
                                command,
                                description,
                            }
                        })
                        .collect();

                // Merge filesystem-discovered skills (project root), dedup by command
                if let Some(ref project_path) = self.state.project_path {
                    for (command, description) in
                        skills::scan_agent_skills(&self.state.config.default_agent, project_path)
                    {
                        if seen.insert(command.clone()) {
                            all_skills.push(SkillEntry {
                                command,
                                description,
                            });
                        }
                    }
                }

                self.state.skill_search = Some(SkillSearchState {
                    pattern: String::new(),
                    matches: all_skills.clone(),
                    all_skills,
                    selected: 0,
                    start_pos,
                });
            }
            KeyCode::Char('!')
                if self.state.input_cursor == 0
                    || self
                        .state
                        .input_buffer
                        .as_bytes()
                        .get(self.state.input_cursor.wrapping_sub(1))
                        == Some(&b'\n')
                    || self
                        .state
                        .input_buffer
                        .as_bytes()
                        .get(self.state.input_cursor.wrapping_sub(1))
                        == Some(&b' ') =>
            {
                // Start task reference search at cursor position (at start of line or after space)
                let start_pos = self.state.input_cursor;
                self.state.input_buffer.insert(self.state.input_cursor, '!');
                self.state.input_cursor += 1;

                let matches = self.get_all_task_matches("");
                self.state.task_ref_search = Some(TaskRefSearchState {
                    pattern: String::new(),
                    matches,
                    selected: 0,
                    start_pos,
                });
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
        if let (Some(ref mut search), Some(ref project_path)) =
            (&mut self.state.file_search, &self.state.project_path)
        {
            let pattern = &search.pattern;
            search.matches =
                fuzzy_find_files(project_path, pattern, 10, self.state.git_ops.as_ref());
            search.selected = 0;
        }
    }

    fn update_skill_search_matches(&mut self) {
        if let Some(ref mut search) = self.state.skill_search {
            let pattern = search.pattern.to_lowercase();
            if pattern.is_empty() {
                search.matches = search.all_skills.clone();
            } else {
                let mut scored: Vec<_> = search
                    .all_skills
                    .iter()
                    .filter_map(|entry| {
                        let cmd_score = fuzzy_score(&entry.command.to_lowercase(), &pattern);
                        let desc_score = fuzzy_score(&entry.description.to_lowercase(), &pattern);
                        let score = std::cmp::max(cmd_score, desc_score);
                        if score > 0 {
                            Some((entry.clone(), score))
                        } else {
                            None
                        }
                    })
                    .collect();
                scored.sort_by(|a, b| b.1.cmp(&a.1));
                search.matches = scored.into_iter().take(10).map(|(e, _)| e).collect();
            }
            search.selected = 0;
        }
    }

    fn save_task(&mut self) -> Result<()> {
        if let Some(db) = &self.state.db {
            let agent = self.state.config.default_agent.clone();
            let plugin_name = self
                .state
                .wizard_plugin_options
                .get(self.state.wizard_selected_plugin)
                .map(|o| o.name.clone())
                .unwrap_or_default();
            let plugin = if plugin_name.is_empty() {
                None
            } else {
                Some(plugin_name)
            };
            let refs = if self.state.wizard_referenced_task_ids.is_empty() {
                None
            } else {
                Some(
                    self.state
                        .wizard_referenced_task_ids
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(","),
                )
            };

            if let Some(task_id) = &self.state.editing_task_id {
                // Editing existing task
                if let Some(mut task) = db.get_task(task_id)? {
                    task.title = self.state.pending_task_title.clone();
                    task.description = if self.state.input_buffer.is_empty() {
                        None
                    } else {
                        Some(self.state.input_buffer.clone())
                    };
                    task.agent = agent;
                    task.plugin = plugin;
                    task.referenced_tasks = refs;
                    task.updated_at = chrono::Utc::now();
                    db.update_task(&task)?;
                }
            } else {
                // Creating new task
                let project_id = self.state.project_name.clone();

                let mut task = Task::new(&self.state.pending_task_title, agent, project_id);
                if !self.state.input_buffer.is_empty() {
                    task.description = Some(self.state.input_buffer.clone());
                }
                task.plugin = plugin;
                task.referenced_tasks = refs;
                // Task starts in Backlog without tmux window
                db.create_task(&task)?;

                // No orchestrator notification for Backlog — orchestrator only manages Planning/Running
            }
            self.refresh_tasks()?;
        }
        Ok(())
    }

    /// Initialize plugin selection options for the wizard, filtered by selected agent.
    fn init_plugin_selection(&mut self) {
        let current = if let Some(task_id) = &self.state.editing_task_id {
            self.state
                .db
                .as_ref()
                .and_then(|db| db.get_task(task_id).ok().flatten())
                .and_then(|t| t.plugin.clone())
                .or_else(|| self.state.config.workflow_plugin.clone())
                .unwrap_or_else(|| "agtx".to_string())
        } else {
            self.state
                .config
                .workflow_plugin
                .as_deref()
                .unwrap_or("agtx")
                .to_string()
        };

        let selected_agent_name = &self.state.config.default_agent;

        let mut options = vec![PluginOption {
            name: "agtx".to_string(),
            label: "agtx".to_string(),
            description: "Built-in workflow with skills and prompts".to_string(),
            active: current == "agtx",
        }];
        for (name, desc, content) in skills::BUNDLED_PLUGINS {
            if *name == "agtx" {
                continue;
            }
            // Filter by agent compatibility
            if let Ok(plugin) = toml::from_str::<WorkflowPlugin>(content) {
                if !plugin.supports_agent(selected_agent_name) {
                    continue;
                }
            }
            options.push(PluginOption {
                name: name.to_string(),
                label: name.to_string(),
                description: desc.to_string(),
                active: current == *name,
            });
        }
        let selected = options.iter().position(|o| o.active).unwrap_or(0);
        self.state.wizard_plugin_options = options;
        self.state.wizard_selected_plugin = selected;
    }

    /// Initialize the description input step of the wizard.
    fn init_description_input(&mut self) {
        if let Some(task_id) = &self.state.editing_task_id {
            if let Some(db) = &self.state.db {
                if let Ok(Some(task)) = db.get_task(task_id) {
                    self.state.input_buffer = task.description.unwrap_or_default();
                    // Restore referenced task IDs if editing
                    self.state.wizard_referenced_task_ids = task
                        .referenced_tasks
                        .as_deref()
                        .map(|s| {
                            s.split(',')
                                .filter(|id| !id.is_empty())
                                .map(String::from)
                                .collect()
                        })
                        .unwrap_or_default();
                } else {
                    self.state.input_buffer.clear();
                }
            } else {
                self.state.input_buffer.clear();
            }
        }
        self.state.input_cursor = self.state.input_buffer.len();
        self.state.input_mode = InputMode::InputDescription;
    }

    /// Cancel the task creation/edit wizard entirely.
    fn cancel_wizard(&mut self) {
        self.state.input_mode = InputMode::Normal;
        self.state.input_buffer.clear();
        self.state.input_cursor = 0;
        self.state.pending_task_title.clear();
        self.state.editing_task_id = None;
        self.state.highlighted_references.clear();
        self.state.wizard_plugin_options.clear();
        self.state.wizard_referenced_task_ids.clear();
        self.state.task_ref_search = None;
    }

    /// Advance from title step to plugin selection or description.
    fn advance_from_title(&mut self) {
        // Skip plugin selection when no agents detected (e.g. test mode)
        if self.state.available_agents.is_empty() {
            self.init_description_input();
            return;
        }
        self.init_plugin_selection();
        if self.state.wizard_plugin_options.len() <= 1 {
            self.init_description_input();
        } else {
            self.state.input_mode = InputMode::SelectPlugin;
        }
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
                    self.state.config.cleanup_script.as_deref(),
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
                let mut exclude_prefixes: Vec<&str> = crate::git::AGENT_CONFIG_DIRS.to_vec();
                let plugin = self.load_task_plugin(task);
                let plugin_dirs: Vec<String> =
                    plugin.map_or_else(Vec::new, |p| p.copy_dirs.clone());
                let plugin_dir_refs: Vec<&str> = plugin_dirs.iter().map(|s| s.as_str()).collect();
                exclude_prefixes.extend(plugin_dir_refs);
                collect_task_diff(
                    worktree_path,
                    self.state.git_ops.as_ref(),
                    &exclude_prefixes,
                )
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
            if self.check_phase_incomplete(&task, current_status, new_status) {
                return Ok(());
            }

            let handled = match (current_status, new_status) {
                (TaskStatus::Backlog, TaskStatus::Planning) => {
                    self.transition_to_planning(&mut task, &project_path)?
                }
                (TaskStatus::Planning, TaskStatus::Running) => {
                    self.transition_to_running(&mut task)?
                }
                (TaskStatus::Running, TaskStatus::Review) => {
                    self.transition_to_review(&mut task, &project_path)?
                }
                (TaskStatus::Review, TaskStatus::Done) => {
                    self.transition_to_done(&mut task, &project_path)?
                }
                _ => false,
            };

            if !handled {
                task.status = new_status;
                task.updated_at = chrono::Utc::now();

                // Clear context from previous phase on transition
                task.escalation_note = None;

                if let Some(db) = &self.state.db {
                    db.update_task(&task)?;
                }

                // Clear stale phase context
                self.state.stuck_task_notified.remove(&task.id);
                self.state.stuck_task_idle_since.remove(&task.id);
                self.state.phase_status_cache.remove(&task.id);
            }
        }
        self.refresh_tasks()?;
        Ok(())
    }

    /// Check if the current phase is incomplete (artifact missing + agent still running).
    /// Returns true if a confirmation popup was shown and the caller should return early.
    fn check_phase_incomplete(
        &mut self,
        task: &Task,
        current_status: TaskStatus,
        new_status: TaskStatus,
    ) -> bool {
        if self.state.skip_move_confirm {
            return false;
        }
        if !matches!(
            current_status,
            TaskStatus::Planning | TaskStatus::Running | TaskStatus::Review
        ) {
            return false;
        }
        let plugin = self.load_task_plugin(task);
        let Some(ref wt_path) = task.worktree_path else {
            return false;
        };
        if phase_artifact_exists(wt_path, current_status, &plugin, task.cycle) {
            return false;
        }
        let agent_running = task.session_name.as_ref().map_or(false, |target| {
            self.state.tmux_ops.window_exists(target).unwrap_or(false)
                && is_agent_active(&*self.state.tmux_ops, target)
        });
        if agent_running {
            self.state.move_confirm_popup = Some(MoveConfirmPopup {
                task_id: task.id.clone(),
                from_status: current_status,
                to_status: new_status,
            });
            return true;
        }
        false
    }

    /// Backlog → Planning: create worktree and tmux window, or reuse existing research session.
    /// Returns Ok(true) if handled separately (setup spawned, warning shown), Ok(false) to continue with db update.
    fn transition_to_planning(&mut self, task: &mut Task, project_path: &Path) -> Result<bool> {
        if task.plugin.is_none() {
            task.plugin = self.state.config.workflow_plugin.clone();
        }
        let plugin = self.load_task_plugin(task);

        // Block if planning phase doesn't accept {task} and no prior phase artifact exists
        if plugin
            .as_ref()
            .map_or(false, |p| !p.phase_accepts_task("planning"))
        {
            let has_research = task
                .worktree_path
                .as_ref()
                .map_or(false, |wt| research_artifact_exists(wt, &task.id, &plugin));
            if !has_research {
                self.state.warning_message = Some((
                    format!("Research phase required first — press R to start research"),
                    std::time::Instant::now(),
                ));
                return Ok(true);
            }
        }

        let (planning_agent, agent_switch) =
            needs_agent_switch(&self.state.config, task, "planning");

        let has_live_session = task.session_name.as_ref().map_or(false, |s| {
            self.state.tmux_ops.window_exists(s).unwrap_or(false)
        });

        if has_live_session {
            // Reuse existing session from research
            let target = task.session_name.clone().unwrap();
            let task_content = task.content_text();
            let planning_phase = determine_phase_variant(
                "planning",
                task.worktree_path.as_deref(),
                &task.id,
                &plugin,
                task.cycle,
            );
            let skill_cmd = resolve_skill_command(
                &plugin,
                planning_phase,
                &planning_agent,
                &task_content,
                task.cycle,
            );
            let prompt =
                resolve_prompt(&plugin, planning_phase, &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, planning_phase);
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                target,
                task.agent.clone(),
                planning_agent.clone(),
                agent_switch,
                skill_cmd,
                prompt,
                prompt_trigger,
                task_content,
                auto_dismiss,
                task.worktree_path.clone(),
                project_path.to_path_buf(),
                plugin,
            );
            task.agent = planning_agent;
            return Ok(false);
        }

        if self.state.setup_rx.is_some() {
            return Ok(true);
        }

        // Create worktree + tmux window from scratch (non-blocking)
        let task_content = task.content_text();
        let prompt = resolve_prompt(&plugin, "planning", &task_content, &task.id, task.cycle);
        let skill_cmd = resolve_skill_command(
            &plugin,
            "planning",
            &planning_agent,
            &task_content,
            task.cycle,
        );
        let prompt_trigger = resolve_prompt_trigger(&plugin, "planning");
        let all_agents = collect_phase_agents(&self.state.config);
        let project_name = self.state.project_name.clone();
        let tmux_project_name = self.state.tmux_project_name.clone();
        let base_branch = self.state.config.base_branch.clone();
        let worktree_dir = self.state.config.worktree_dir.clone();
        let copy_files = self.state.config.copy_files.clone();
        let init_script = self.state.config.init_script.clone();
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let git_ops = Arc::clone(&self.state.git_ops);
        let agent_ops = self.state.agent_registry.get(&planning_agent);
        let task_id = task.id.clone();
        let task_title = task.title.clone();
        let plugin_name = task.plugin.clone();
        let planning_agent_clone = planning_agent.clone();
        let auto_dismiss = plugin
            .as_ref()
            .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
        let project_path = project_path.to_path_buf();

        // Pre-fetch referenced task info (DB isn't Send, so fetch before spawning thread)
        let referenced_tasks: Vec<ReferencedTaskInfo> = task
            .referenced_tasks
            .as_deref()
            .map(|refs_str| {
                refs_str
                    .split(',')
                    .filter(|s| !s.is_empty())
                    .filter_map(|ref_id| {
                        self.state
                            .db
                            .as_ref()
                            .and_then(|db| db.get_task(ref_id).ok().flatten())
                            .map(|ref_task| ReferencedTaskInfo {
                                slug: generate_task_slug(&ref_task.id, &ref_task.title),
                                branch_name: ref_task.branch_name.clone(),
                                worktree_path: ref_task.worktree_path.clone(),
                            })
                    })
                    .collect()
            })
            .unwrap_or_default();

        let (tx, rx) = mpsc::channel();
        self.state.setup_rx = Some(rx);

        std::thread::spawn(move || {
            let mut tmp_task = Task::new(&task_title, &planning_agent_clone, &project_name);
            tmp_task.id = task_id.clone();
            tmp_task.plugin = plugin_name.clone();

            let result = setup_task_worktree(
                &mut tmp_task,
                &project_path,
                &tmux_project_name,
                &prompt,
                &base_branch,
                &worktree_dir,
                copy_files,
                init_script,
                &plugin,
                &planning_agent_clone,
                &all_agents,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
                agent_ops.as_ref(),
                &referenced_tasks,
            );

            match result {
                Ok(target) => {
                    let _ = tx.send(SetupResult {
                        task_id: task_id.clone(),
                        session_name: tmp_task.session_name.unwrap_or_default(),
                        worktree_path: tmp_task.worktree_path.unwrap_or_default(),
                        branch_name: tmp_task.branch_name.unwrap_or_default(),
                        new_status: Some(TaskStatus::Planning),
                        agent: planning_agent_clone.clone(),
                        plugin: plugin_name,
                        error: None,
                    });
                    if let Some(target) = wait_for_agent_ready(&tmux_ops, &target) {
                        send_skill_and_prompt(
                            &tmux_ops,
                            &target,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content,
                            &planning_agent_clone,
                            &auto_dismiss,
                        );
                    }
                }
                Err(e) => {
                    let _ = tx.send(SetupResult {
                        task_id,
                        session_name: String::new(),
                        worktree_path: String::new(),
                        branch_name: String::new(),
                        new_status: None,
                        agent: planning_agent_clone,
                        plugin: plugin_name,
                        error: Some(format!("Planning setup failed: {}", e)),
                    });
                }
            }
        });

        Ok(true)
    }

    /// Planning → Running: send execution skill/prompt to agent.
    /// Always returns Ok(false) to continue with db update.
    fn transition_to_running(&mut self, task: &mut Task) -> Result<bool> {
        if let Some(session_name) = &task.session_name {
            let plugin = self.load_task_plugin(task);
            let (running_agent, agent_switch) =
                needs_agent_switch(&self.state.config, task, "running");
            let task_content = task.content_text();
            let run_phase = determine_phase_variant(
                "running",
                task.worktree_path.as_deref(),
                &task.id,
                &plugin,
                task.cycle,
            );
            let skill_cmd = resolve_skill_command(
                &plugin,
                run_phase,
                &running_agent,
                &task_content,
                task.cycle,
            );
            let prompt = resolve_prompt(&plugin, run_phase, &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, run_phase);
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                session_name.clone(),
                task.agent.clone(),
                running_agent.clone(),
                agent_switch,
                skill_cmd,
                prompt,
                prompt_trigger,
                task_content,
                auto_dismiss,
                task.worktree_path.clone(),
                self.state.project_path.clone().unwrap_or_default(),
                plugin,
            );
            task.agent = running_agent;
        }
        Ok(false)
    }

    /// Running → Review: send review skill/prompt, then handle PR state.
    /// Returns Ok(true) always (PR push or review confirm popup shown).
    fn transition_to_review(&mut self, task: &mut Task, project_path: &Path) -> Result<bool> {
        let (review_agent, agent_switch) = needs_agent_switch(&self.state.config, task, "review");
        if let Some(session_name) = &task.session_name {
            let plugin = self.load_task_plugin(task);
            let task_content = task.content_text();
            let skill_cmd =
                resolve_skill_command(&plugin, "review", &review_agent, &task_content, task.cycle);
            let prompt = resolve_prompt(&plugin, "review", &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, "review");
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                session_name.clone(),
                task.agent.clone(),
                review_agent.clone(),
                agent_switch,
                skill_cmd,
                prompt,
                prompt_trigger,
                task_content,
                auto_dismiss,
                task.worktree_path.clone(),
                project_path.to_path_buf(),
                plugin,
            );
        }
        task.agent = review_agent.clone();

        // PR already exists (task was resumed from Review) — push new changes
        if task.pr_number.is_some() {
            self.state.pr_status_popup = Some(PrStatusPopup {
                status: PrCreationStatus::Pushing,
                pr_url: None,
                error_message: None,
            });

            let task_clone = task.clone();
            let project_path_clone = project_path.to_path_buf();
            let git_ops = Arc::clone(&self.state.git_ops);
            let agent_ops = self.state.agent_registry.get(&review_agent);

            let (tx, rx) = mpsc::channel();
            self.state.pr_creation_rx = Some(rx);

            std::thread::spawn(move || {
                let result =
                    push_changes_to_existing_pr(&task_clone, git_ops.as_ref(), agent_ops.as_ref());
                match result {
                    Ok(pr_url) => {
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

            return Ok(true);
        }

        // No PR yet — show confirmation popup
        self.state.review_confirm_popup = Some(ReviewConfirmPopup {
            task_id: task.id.clone(),
            task_title: task.title.clone(),
        });
        Ok(true)
    }

    /// Review → Done: check PR state, uncommitted changes, or clean up.
    /// Returns Ok(true) if a confirmation popup was shown, Ok(false) to continue with db update.
    fn transition_to_done(&mut self, task: &mut Task, project_path: &Path) -> Result<bool> {
        if let Some(pr_number) = task.pr_number {
            let pr_state = self
                .state
                .git_provider_ops
                .get_pr_state(project_path, pr_number)?;
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
            return Ok(true);
        }

        // No PR — check for uncommitted changes
        let has_uncommitted = task
            .worktree_path
            .as_ref()
            .map_or(false, |wt| self.state.git_ops.has_changes(Path::new(wt)));
        if has_uncommitted {
            self.state.done_confirm_popup = Some(DoneConfirmPopup {
                task_id: task.id.clone(),
                pr_number: 0,
                pr_state: DoneConfirmPrState::UncommittedChanges,
            });
            return Ok(true);
        }

        // Clean — spawn background cleanup
        let session_name = task.session_name.clone();
        let worktree_path = task.worktree_path.clone();
        let branch_name = task.branch_name.clone();
        task.session_name = None;
        task.worktree_path = None;

        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let git_ops = Arc::clone(&self.state.git_ops);
        let task_id_clone = task.id.clone();
        let project_path_clone = project_path.to_path_buf();
        let cleanup_script = self.state.config.cleanup_script.clone();
        std::thread::spawn(move || {
            cleanup_task_resources(
                &task_id_clone,
                &branch_name,
                &session_name,
                &worktree_path,
                cleanup_script.as_deref(),
                &project_path_clone,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
            );
        });
        Ok(false)
    }

    /// Start a research session for a Backlog task (creates worktree, reused in planning)
    fn start_research(&mut self, task_id: &str) -> Result<()> {
        // Don't start if a setup is already in progress
        if self.state.setup_rx.is_some() {
            return Ok(());
        }

        let mut task = {
            let Some(db) = &self.state.db else {
                return Ok(());
            };
            let Some(task) = db.get_task(task_id)? else {
                return Ok(());
            };
            task
        };
        let Some(project_path) = self.state.project_path.clone() else {
            return Ok(());
        };

        // Stamp plugin on task for research (only if not already set at task creation)
        if task.plugin.is_none() {
            task.plugin = self.state.config.workflow_plugin.clone();
        }
        let plugin_name = task.plugin.clone();
        let plugin = self.load_task_plugin(&task);

        // Block if plugin has no research command (e.g. OpenSpec uses planning as first phase)
        let has_research_cmd = plugin.as_ref().map_or(false, |p| {
            p.commands.research.is_some() || p.commands.preresearch.is_some()
        });
        if !has_research_cmd {
            self.state.warning_message = Some((
                "This plugin has no research phase — move to Planning instead".to_string(),
                std::time::Instant::now(),
            ));
            return Ok(());
        }

        let agent_name = self.state.config.agent_for_phase("research").to_string();

        let task_content = task.content_text();

        let all_agents = collect_phase_agents(&self.state.config);
        let project_name = self.state.project_name.clone();
        let tmux_project_name = self.state.tmux_project_name.clone();
        let base_branch = self.state.config.base_branch.clone();
        let worktree_dir = self.state.config.worktree_dir.clone();
        let copy_files = self.state.config.copy_files.clone();
        let init_script = self.state.config.init_script.clone();

        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let git_ops = Arc::clone(&self.state.git_ops);
        let agent_ops = self.state.agent_registry.get(&agent_name);

        let task_id = task.id.clone();
        let task_title = task.title.clone();
        let task_cycle = task.cycle;
        let auto_dismiss = plugin
            .as_ref()
            .map_or_else(Vec::new, |p| p.auto_dismiss.clone());

        let (tx, rx) = mpsc::channel();
        self.state.setup_rx = Some(rx);

        std::thread::spawn(move || {
            // Create a temporary task to pass to setup_task_worktree
            let mut tmp_task = Task::new(&task_title, &agent_name, &project_name);
            tmp_task.id = task_id.clone();
            tmp_task.plugin = plugin_name.clone();

            // setup_task_worktree creates the worktree and copies files (including preresearch artifacts if they exist at root)
            // We pass an empty prompt here — the actual prompt is resolved after worktree creation
            let result = setup_task_worktree(
                &mut tmp_task,
                &project_path,
                &tmux_project_name,
                "",
                &base_branch,
                &worktree_dir,
                copy_files,
                init_script,
                &plugin,
                &agent_name,
                &all_agents,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
                agent_ops.as_ref(),
                &[],
            );

            match result {
                Ok(target) => {
                    let worktree_path = tmp_task.worktree_path.clone().unwrap_or_default();

                    // Determine preresearch vs research by checking if preresearch artifacts
                    // exist in the worktree (they would have been copied from project root via copy_files)
                    let use_preresearch = plugin.as_ref().map_or(false, |p| {
                        p.commands.preresearch.is_some()
                            && !p.artifacts.preresearch.is_empty()
                            && !p
                                .artifacts
                                .preresearch
                                .iter()
                                .all(|a| Path::new(&worktree_path).join(a).exists())
                    });
                    let research_phase = if use_preresearch {
                        "preresearch"
                    } else {
                        "research"
                    };

                    let prompt = resolve_prompt(
                        &plugin,
                        research_phase,
                        &task_content,
                        &task_id,
                        task_cycle,
                    );
                    let skill_cmd = resolve_skill_command(
                        &plugin,
                        research_phase,
                        &agent_name,
                        &task_content,
                        task_cycle,
                    );
                    let prompt_trigger = resolve_prompt_trigger(&plugin, research_phase);

                    let _ = tx.send(SetupResult {
                        task_id: task_id.clone(),
                        session_name: tmp_task.session_name.unwrap_or_default(),
                        worktree_path,
                        branch_name: tmp_task.branch_name.unwrap_or_default(),
                        new_status: None, // stays in Backlog
                        agent: agent_name.clone(),
                        plugin: plugin_name,
                        error: None,
                    });

                    // Wait for agent ready and send skill+prompt
                    if let Some(target) = wait_for_agent_ready(&tmux_ops, &target) {
                        send_skill_and_prompt(
                            &tmux_ops,
                            &target,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content,
                            &agent_name,
                            &auto_dismiss,
                        );
                    }
                }
                Err(e) => {
                    let _ = tx.send(SetupResult {
                        task_id,
                        session_name: String::new(),
                        worktree_path: String::new(),
                        branch_name: String::new(),
                        new_status: None,
                        agent: agent_name,
                        plugin: plugin_name,
                        error: Some(format!("Research setup failed: {}", e)),
                    });
                }
            }
        });

        Ok(())
    }

    /// Move task directly from Backlog to Running (skip Planning)
    fn move_backlog_to_running(&mut self) -> Result<()> {
        let task_id = match self.state.board.selected_task() {
            Some(t) if t.status == TaskStatus::Backlog => t.id.clone(),
            _ => return Ok(()),
        };
        self.move_backlog_to_running_by_id(&task_id)
    }

    fn move_backlog_to_running_by_id(&mut self, task_id: &str) -> Result<()> {
        // Don't start if a setup is already in progress
        if self.state.setup_rx.is_some() {
            anyhow::bail!("Another task setup is already in progress, try again shortly");
        }

        let (mut task, project_path) = match (
            self.state
                .db
                .as_ref()
                .and_then(|db| db.get_task(task_id).ok().flatten()),
            self.state.project_path.clone(),
        ) {
            (Some(t), Some(p)) => (t, p),
            _ => return Ok(()),
        };

        if task.status != TaskStatus::Backlog {
            anyhow::bail!(
                "Task must be in Backlog to move to Running (current: {})",
                task.status.as_str()
            );
        }

        // Stamp plugin on task before checking research requirement
        if task.plugin.is_none() {
            task.plugin = self.state.config.workflow_plugin.clone();
        }

        // Block if running phase doesn't accept {task} and no prior phase artifact exists
        let plugin_check = self.load_task_plugin(&task);
        if plugin_check
            .as_ref()
            .map_or(false, |p| !p.phase_accepts_task("running"))
        {
            let has_prior = task.worktree_path.as_ref().map_or(false, |wt| {
                research_artifact_exists(wt, &task.id, &plugin_check)
                    || phase_artifact_exists(wt, TaskStatus::Planning, &plugin_check, task.cycle)
            });
            if !has_prior {
                self.state.warning_message = Some((
                    format!("Research or planning phase required first"),
                    std::time::Instant::now(),
                ));
                return Ok(());
            }
        }

        // Build prompt - skip planning, go straight to implementation
        let task_content = task.content_text();

        let plugin_name = task.plugin.clone();
        let plugin = self.load_task_plugin(&task);
        let running_agent = self.state.config.agent_for_phase("running").to_string();
        let all_agents = collect_phase_agents(&self.state.config);
        let prompt = resolve_prompt(&plugin, "running", &task_content, &task.id, task.cycle);
        let skill_cmd = resolve_skill_command(
            &plugin,
            "running",
            &running_agent,
            &task_content,
            task.cycle,
        );
        let prompt_trigger = resolve_prompt_trigger(&plugin, "running");
        let project_name = self.state.project_name.clone();
        let tmux_project_name = self.state.tmux_project_name.clone();
        let base_branch = self.state.config.base_branch.clone();
        let worktree_dir = self.state.config.worktree_dir.clone();
        let copy_files = self.state.config.copy_files.clone();
        let init_script = self.state.config.init_script.clone();
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let git_ops = Arc::clone(&self.state.git_ops);
        let agent_ops = self.state.agent_registry.get(&running_agent);
        let task_id = task.id.clone();
        let task_title = task.title.clone();
        let running_agent_clone = running_agent.clone();
        let auto_dismiss = plugin
            .as_ref()
            .map_or_else(Vec::new, |p| p.auto_dismiss.clone());

        let (tx, rx) = mpsc::channel();
        self.state.setup_rx = Some(rx);

        std::thread::spawn(move || {
            let mut tmp_task = Task::new(&task_title, &running_agent_clone, &project_name);
            tmp_task.id = task_id.clone();
            tmp_task.plugin = plugin_name.clone();

            let result = setup_task_worktree(
                &mut tmp_task,
                &project_path,
                &tmux_project_name,
                &prompt,
                &base_branch,
                &worktree_dir,
                copy_files,
                init_script,
                &plugin,
                &running_agent_clone,
                &all_agents,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
                agent_ops.as_ref(),
                &[],
            );

            match result {
                Ok(target) => {
                    let _ = tx.send(SetupResult {
                        task_id: task_id.clone(),
                        session_name: tmp_task.session_name.unwrap_or_default(),
                        worktree_path: tmp_task.worktree_path.unwrap_or_default(),
                        branch_name: tmp_task.branch_name.unwrap_or_default(),
                        new_status: Some(TaskStatus::Running),
                        agent: running_agent_clone.clone(),
                        plugin: plugin_name,
                        error: None,
                    });

                    if let Some(target) = wait_for_agent_ready(&tmux_ops, &target) {
                        send_skill_and_prompt(
                            &tmux_ops,
                            &target,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content,
                            &running_agent_clone,
                            &auto_dismiss,
                        );
                    }
                }
                Err(e) => {
                    let _ = tx.send(SetupResult {
                        task_id,
                        session_name: String::new(),
                        worktree_path: String::new(),
                        branch_name: String::new(),
                        new_status: None,
                        agent: running_agent_clone,
                        plugin: plugin_name,
                        error: Some(format!("Running setup failed: {}", e)),
                    });
                }
            }
        });

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

                // Switch agent if running phase uses a different agent than review
                let (running_agent, agent_switch) =
                    needs_agent_switch(&self.state.config, &task, "running");
                if agent_switch {
                    if let Some(session_name) = &task.session_name {
                        let session_clone = session_name.clone();
                        let tmux_ops = Arc::clone(&self.state.tmux_ops);
                        let agent_registry = Arc::clone(&self.state.agent_registry);
                        let running_agent_clone = running_agent.clone();
                        let current_agent_clone = task.agent.clone();
                        let wt_path = task.worktree_path.clone();
                        std::thread::spawn(move || {
                            let agent_ops = agent_registry.get(&running_agent_clone);
                            ensure_window_or_recover(
                                tmux_ops.as_ref(),
                                &session_clone,
                                agent_ops.as_ref(),
                                wt_path.as_deref(),
                            );
                            let new_cmd = agent_ops.build_interactive_command("");
                            switch_agent_in_tmux(
                                tmux_ops.as_ref(),
                                &session_clone,
                                &current_agent_clone,
                                &new_cmd,
                            );
                        });
                    }
                }

                task.agent = running_agent;
                task.status = TaskStatus::Running;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    fn move_review_to_planning(&mut self, task_id: &str) -> Result<()> {
        if let (Some(db), Some(_project_path)) = (&self.state.db, &self.state.project_path) {
            if let Some(mut task) = db.get_task(task_id)? {
                if task.status != TaskStatus::Review {
                    return Ok(());
                }

                // Increment cycle counter for the next phase
                task.cycle += 1;

                // Switch agent if planning phase uses a different agent than review
                let (planning_agent, agent_switch) =
                    needs_agent_switch(&self.state.config, &task, "planning");
                let plugin = self.load_task_plugin(&task);

                // Resolve skill command and prompt for the new planning phase
                let task_content = task
                    .description
                    .as_deref()
                    .unwrap_or(&task.title)
                    .to_string();
                let skill_cmd = resolve_skill_command(
                    &plugin,
                    "planning",
                    &planning_agent,
                    &task_content,
                    task.cycle,
                );
                let prompt =
                    resolve_prompt(&plugin, "planning", &task_content, &task.id, task.cycle);
                let prompt_trigger = resolve_prompt_trigger(&plugin, "planning");

                if let Some(session_name) = &task.session_name {
                    let session_clone = session_name.clone();
                    let tmux_ops = Arc::clone(&self.state.tmux_ops);
                    let agent_registry = Arc::clone(&self.state.agent_registry);
                    let planning_agent_clone = planning_agent.clone();
                    let current_agent_clone = task.agent.clone();
                    let task_content_clone = task_content.clone();
                    let auto_dismiss = plugin
                        .as_ref()
                        .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
                    let wt_path = task.worktree_path.clone();
                    std::thread::spawn(move || {
                        let agent_ops = agent_registry.get(&planning_agent_clone);
                        // Recover window if it was lost
                        ensure_window_or_recover(
                            tmux_ops.as_ref(),
                            &session_clone,
                            agent_ops.as_ref(),
                            wt_path.as_deref(),
                        );
                        if agent_switch {
                            let new_cmd = agent_ops.build_interactive_command("");
                            switch_agent_in_tmux(
                                tmux_ops.as_ref(),
                                &session_clone,
                                &current_agent_clone,
                                &new_cmd,
                            );
                            let _ = wait_for_agent_ready(&tmux_ops, &session_clone);
                        }
                        send_skill_and_prompt(
                            &tmux_ops,
                            &session_clone,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content_clone,
                            &planning_agent_clone,
                            &auto_dismiss,
                        );
                    });
                }

                task.agent = planning_agent;
                task.status = TaskStatus::Planning;
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

                // Switch agent if planning phase uses a different agent than running
                let (planning_agent, agent_switch) =
                    needs_agent_switch(&self.state.config, &task, "planning");
                if agent_switch {
                    if let Some(session_name) = &task.session_name {
                        let session_clone = session_name.clone();
                        let tmux_ops = Arc::clone(&self.state.tmux_ops);
                        let agent_registry = Arc::clone(&self.state.agent_registry);
                        let planning_agent_clone = planning_agent.clone();
                        let current_agent_clone = task.agent.clone();
                        let wt_path = task.worktree_path.clone();
                        std::thread::spawn(move || {
                            let agent_ops = agent_registry.get(&planning_agent_clone);
                            ensure_window_or_recover(
                                tmux_ops.as_ref(),
                                &session_clone,
                                agent_ops.as_ref(),
                                wt_path.as_deref(),
                            );
                            let new_cmd = agent_ops.build_interactive_command("");
                            switch_agent_in_tmux(
                                tmux_ops.as_ref(),
                                &session_clone,
                                &current_agent_clone,
                                &new_cmd,
                            );
                        });
                    }
                }

                task.agent = planning_agent;
                task.status = TaskStatus::Planning;
                task.updated_at = chrono::Utc::now();
                db.update_task(&task)?;
                self.refresh_tasks()?;
            }
        }
        Ok(())
    }

    // === MCP Transition Request Processing ===

    /// Poll the transition_requests table for unprocessed requests and execute them.
    fn process_transition_requests(&mut self) -> Result<()> {
        let Some(db) = &self.state.db else {
            return Ok(());
        };
        let pending = db.get_pending_transition_requests()?;
        if pending.is_empty() {
            return Ok(());
        }

        for req in pending {
            let result = self.execute_transition_request(&req);
            if let Some(db) = &self.state.db {
                match &result {
                    Ok(()) => {
                        let _ = db.mark_transition_processed(&req.id, None);
                    }
                    Err(e) => {
                        let _ = db.mark_transition_processed(&req.id, Some(&e.to_string()));
                    }
                }
            }
            self.refresh_tasks()?;
        }

        // Periodically clean up old processed requests
        if let Some(db) = &self.state.db {
            let _ = db.cleanup_old_transition_requests();
        }

        Ok(())
    }

    fn execute_transition_request(&mut self, req: &TransitionRequest) -> Result<()> {
        let Some(db) = &self.state.db else {
            anyhow::bail!("No project database");
        };
        let Some(project_path) = self.state.project_path.clone() else {
            anyhow::bail!("No project path");
        };

        let mut task = db
            .get_task(&req.task_id)?
            .ok_or_else(|| anyhow::anyhow!("Task not found: {}", req.task_id))?;

        match req.action.as_str() {
            "research" => {
                if task.status != TaskStatus::Backlog {
                    anyhow::bail!(
                        "Task must be in Backlog to start research (current: {})",
                        task.status.as_str()
                    );
                }
                if task.session_name.is_some() {
                    anyhow::bail!(
                        "Task already has an active session (research may already be running)"
                    );
                }
                self.start_research(&req.task_id)?;
            }
            "move_forward" => {
                self.execute_forward_transition(&mut task, &project_path)?;
            }
            "move_to_planning" => {
                if task.status != TaskStatus::Backlog {
                    anyhow::bail!(
                        "Task must be in Backlog to move to Planning (current: {})",
                        task.status.as_str()
                    );
                }
                self.execute_forward_transition(&mut task, &project_path)?;
            }
            "move_to_running" => {
                if task.status != TaskStatus::Planning && task.status != TaskStatus::Backlog {
                    anyhow::bail!(
                        "Task must be in Backlog or Planning to move to Running (current: {})",
                        task.status.as_str()
                    );
                }
                if task.status == TaskStatus::Backlog {
                    self.move_backlog_to_running_by_id(&req.task_id)?;
                } else {
                    self.execute_forward_transition(&mut task, &project_path)?;
                }
            }
            "move_to_review" => {
                if task.status != TaskStatus::Running {
                    anyhow::bail!(
                        "Task must be in Running to move to Review (current: {})",
                        task.status.as_str()
                    );
                }
                self.mcp_transition_to_review(&mut task)?;
            }
            "move_to_done" => {
                if task.status != TaskStatus::Review {
                    anyhow::bail!(
                        "Task must be in Review to move to Done (current: {})",
                        task.status.as_str()
                    );
                }
                self.force_move_to_done(&task.id)?;
            }
            "resume" => {
                if task.status != TaskStatus::Review {
                    anyhow::bail!(
                        "Task must be in Review to resume (current: {})",
                        task.status.as_str()
                    );
                }
                self.move_review_to_running(&req.task_id)?;
            }
            "escalate_to_user" => {
                if !matches!(task.status, TaskStatus::Planning | TaskStatus::Running) {
                    anyhow::bail!(
                        "escalate_to_user is only valid for Planning or Running tasks (current: {})",
                        task.status.as_str()
                    );
                }
                task.escalation_note = req
                    .reason
                    .clone()
                    .or_else(|| Some("Needs attention".to_string()));
                task.updated_at = chrono::Utc::now();
                if let Some(db) = &self.state.db {
                    db.update_task(&task)?;
                }
                self.refresh_tasks()?;
            }
            other => {
                anyhow::bail!("Unknown action: {}", other);
            }
        }

        Ok(())
    }

    /// Execute a forward transition (next column), mirroring move_task_right logic.
    fn execute_forward_transition(&mut self, task: &mut Task, project_path: &Path) -> Result<()> {
        let next_status = match task.status {
            TaskStatus::Backlog => TaskStatus::Planning,
            TaskStatus::Planning => TaskStatus::Running,
            TaskStatus::Running => TaskStatus::Review,
            TaskStatus::Review => TaskStatus::Done,
            TaskStatus::Done => anyhow::bail!("Task is already Done"),
        };

        // Skip the phase-incomplete confirmation for MCP requests
        let handled = match (task.status, next_status) {
            (TaskStatus::Backlog, TaskStatus::Planning) => {
                if self.state.setup_rx.is_some() {
                    anyhow::bail!("Another task setup is already in progress, try again shortly");
                }
                self.transition_to_planning(task, project_path)?
            }
            (TaskStatus::Planning, TaskStatus::Running) => self.transition_to_running(task)?,
            (TaskStatus::Running, TaskStatus::Review) => {
                self.mcp_transition_to_review(task)?;
                return Ok(());
            }
            (TaskStatus::Review, TaskStatus::Done) => {
                self.force_move_to_done(&task.id)?;
                return Ok(());
            }
            _ => false,
        };

        if !handled {
            task.status = next_status;
            task.updated_at = chrono::Utc::now();
            if let Some(db) = &self.state.db {
                db.update_task(task)?;
            }
        }

        Ok(())
    }

    /// MCP version of transition_to_review: sends review prompt but skips PR popup.
    fn mcp_transition_to_review(&mut self, task: &mut Task) -> Result<()> {
        let (review_agent, agent_switch) = needs_agent_switch(&self.state.config, task, "review");
        if let Some(session_name) = &task.session_name {
            let plugin = self.load_task_plugin(task);
            let task_content = task.content_text();
            let skill_cmd =
                resolve_skill_command(&plugin, "review", &review_agent, &task_content, task.cycle);
            let prompt = resolve_prompt(&plugin, "review", &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, "review");
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                session_name.clone(),
                task.agent.clone(),
                review_agent.clone(),
                agent_switch,
                skill_cmd,
                prompt,
                prompt_trigger,
                task_content,
                auto_dismiss,
                task.worktree_path.clone(),
                self.state.project_path.clone().unwrap_or_default(),
                plugin,
            );
        }
        task.agent = review_agent;
        task.status = TaskStatus::Review;
        task.updated_at = chrono::Utc::now();
        if let Some(db) = &self.state.db {
            db.update_task(task)?;
        }
        Ok(())
    }

    /// Toggle orchestrator agent: spawn if not running, view if running.
    fn toggle_orchestrator(&mut self) -> Result<()> {
        let project_path = match &self.state.project_path {
            Some(p) => p.clone(),
            None => {
                self.state.warning_message = Some((
                    "Orchestrator requires a project (not dashboard mode)".to_string(),
                    Instant::now(),
                ));
                return Ok(());
            }
        };

        let tmux_project_name = self.state.tmux_project_name.clone();
        let window_name = "orchestrator";
        let orch_target = format!("{}:{}", tmux_project_name, window_name);

        // If orchestrator is running, open the popup to view it
        if let Some(ref orch_target) = self.state.orchestrator_session {
            if self
                .state
                .tmux_ops
                .window_exists(orch_target)
                .unwrap_or(false)
            {
                let mut popup = ShellPopup::new("Orchestrator".to_string(), orch_target.clone());
                if let Ok((_term_width, term_height)) = crossterm::terminal::size() {
                    let pane_width = SHELL_POPUP_CONTENT_WIDTH;
                    let popup_height =
                        (term_height as u32 * SHELL_POPUP_HEIGHT_PERCENT as u32 / 100) as u16;
                    let pane_height = popup_height.saturating_sub(4);
                    let _ = self
                        .state
                        .tmux_ops
                        .resize_window(orch_target, pane_width, pane_height);
                    popup.last_pane_size = Some((pane_width, pane_height));
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }
                popup.cached_content =
                    capture_tmux_pane_with_history(orch_target, 500, self.state.tmux_ops.as_ref());
                self.state.shell_popup = Some(popup);
                return Ok(());
            } else {
                // Window gone, clear the session
                self.state.orchestrator_session = None;
                self.state
                    .orchestrator_ready
                    .store(false, Ordering::Release);
            }
        }

        // Spawn new orchestrator
        let default_agent = self.state.config.default_agent.clone();
        let agent = self.state.agent_registry.get(&default_agent);
        let project_path_str = project_path.to_string_lossy().to_string();

        // Build MCP registration JSON for the agtx server
        let agtx_bin = std::env::current_exe()
            .unwrap_or_else(|_| PathBuf::from("agtx"))
            .to_string_lossy()
            .to_string();
        let mcp_json = serde_json::json!({
            "type": "stdio",
            "command": agtx_bin,
            "args": ["mcp-serve", &project_path_str]
        });
        let mcp_json_str = mcp_json.to_string().replace('\'', "'\\''");

        let agent_cmd = agent.build_orchestrator_command(&mcp_json_str, &agtx_bin);

        // Ensure project tmux session exists
        ensure_project_tmux_session(
            &tmux_project_name,
            &project_path,
            self.state.tmux_ops.as_ref(),
        );

        // Create orchestrator tmux window in the project root (no worktree)
        self.state.tmux_ops.create_window(
            &tmux_project_name,
            window_name,
            &project_path_str,
            Some(agent_cmd),
        )?;

        self.state.orchestrator_session = Some(orch_target.clone());
        self.state
            .orchestrator_ready
            .store(false, Ordering::Release);

        // Open the popup immediately so the user can see the orchestrator starting
        let mut popup = ShellPopup::new("Orchestrator".to_string(), orch_target.clone());
        if let Ok((_term_width, term_height)) = crossterm::terminal::size() {
            let pane_width = SHELL_POPUP_CONTENT_WIDTH;
            let popup_height =
                (term_height as u32 * SHELL_POPUP_HEIGHT_PERCENT as u32 / 100) as u16;
            let pane_height = popup_height.saturating_sub(4);
            let _ = self
                .state
                .tmux_ops
                .resize_window(&orch_target, pane_width, pane_height);
            popup.last_pane_size = Some((pane_width, pane_height));
        }
        popup.cached_content =
            capture_tmux_pane_with_history(&orch_target, 500, self.state.tmux_ops.as_ref());
        self.state.shell_popup = Some(popup);

        // Deploy orchestrate skill to project root so the agent can discover it
        deploy_skill(
            &project_path,
            "agtx-orchestrate",
            skills::ORCHESTRATE_SKILL,
            &default_agent,
        );

        // Send the /agtx:orchestrate command once the agent is ready
        let skill_cmd = skills::transform_plugin_command("/agtx:orchestrate", &default_agent)
            .unwrap_or_else(|| "/agtx:orchestrate".to_string());
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let ready_flag = Arc::clone(&self.state.orchestrator_ready);
        let target = orch_target;
        std::thread::spawn(move || {
            if let Some(ready_target) = wait_for_agent_ready(&tmux_ops, &target) {
                let _ = tmux_ops.send_keys(&ready_target, &skill_cmd);
                ready_flag.store(true, Ordering::Release);
            }
        });

        Ok(())
    }

    fn open_selected_task(&mut self) -> Result<()> {
        if let Some(task) = self.state.board.selected_task() {
            if let Some(window_name) = &task.session_name.clone() {
                // If the tmux window is gone, try to recover it before opening
                if !self
                    .state
                    .tmux_ops
                    .window_exists(window_name)
                    .unwrap_or(true)
                {
                    let agent_ops = self.state.agent_registry.get(&task.agent);
                    let project_path = self
                        .state
                        .project_path
                        .as_deref()
                        .unwrap_or(Path::new("."));
                    let _ = recover_task_session(
                        task,
                        &self.state.tmux_project_name,
                        project_path,
                        self.state.tmux_ops.as_ref(),
                        agent_ops.as_ref(),
                    );
                    // Clear stale phase status so it gets re-evaluated
                    self.state.phase_status_cache.remove(&task.id);
                    self.state.pane_content_hashes.remove(&task.id);
                }

                let task_id = task.id.clone();
                let escalation_note = task.escalation_note.clone();
                let mut popup = ShellPopup::new(task.title.clone(), window_name.clone());
                popup.task_id = Some(task_id);
                popup.escalation_note = escalation_note;

                // Resize tmux window to match popup dimensions (uses same constants as draw_shell_popup)
                if let Ok((_term_width, term_height)) = crossterm::terminal::size() {
                    let pane_width = SHELL_POPUP_CONTENT_WIDTH;
                    let popup_height =
                        (term_height as u32 * SHELL_POPUP_HEIGHT_PERCENT as u32 / 100) as u16;
                    let pane_height = popup_height.saturating_sub(4); // -4 for borders + header/footer

                    let target = format!("{}:{}", self.state.tmux_project_name, window_name);
                    // TODO the resize should be done on target which is
                    // session_name:window_name, but for some reason that doesn't work
                    // doing tmux -L agtx resize-window -t session:window -x 30 -y 30 works
                    let _ =
                        self.state
                            .tmux_ops
                            .resize_window(&window_name, pane_width, pane_height);
                    popup.last_pane_size = Some((pane_width, pane_height));
                    // Give TUI apps (OpenCode, Gemini Ink) time to re-render after resize
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }

                // Capture initial content
                popup.cached_content =
                    capture_tmux_pane_with_history(window_name, 500, self.state.tmux_ops.as_ref());

                self.state.shell_popup = Some(popup);
            }
        }
        Ok(())
    }

    /// Load the plugin that a specific task was created with.
    /// Falls back to bundled agtx plugin for tasks with no explicit plugin.
    fn load_task_plugin(&self, task: &Task) -> Option<WorkflowPlugin> {
        load_task_plugin(
            task,
            self.state.project_path.as_deref(),
            &self.state.config.default_agent,
        )
    }

    pub fn refresh_tasks(&mut self) -> Result<()> {
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
            if let Some(pos) = self
                .state
                .projects
                .iter()
                .position(|p| p.path == current_path)
            {
                self.state.selected_project = pos;
            }
        }

        Ok(())
    }

    /// Push queued notifications to the orchestrator's tmux pane, but only when idle.
    /// Runs every 2s. Idle = pane content unchanged for ≥3s.
    fn deliver_orchestrator_notifications(&mut self) {
        // Only check every 2 seconds
        if self.state.orchestrator_last_check.elapsed() < std::time::Duration::from_secs(2) {
            return;
        }
        self.state.orchestrator_last_check = Instant::now();

        let orch_target = match &self.state.orchestrator_session {
            Some(t) => t.clone(),
            None => return,
        };

        // Don't deliver until the agent is ready and has received the skill command
        if !self.state.orchestrator_ready.load(Ordering::Acquire) {
            return;
        }

        // Check window still exists
        if !self
            .state
            .tmux_ops
            .window_exists(&orch_target)
            .unwrap_or(false)
        {
            return;
        }

        // Capture current pane content (bottom portion for comparison)
        let current_content = self
            .state
            .tmux_ops
            .capture_pane(&orch_target)
            .unwrap_or_default();

        let result = check_orchestrator_idle(
            &current_content,
            &self.state.orchestrator_last_content,
            self.state.orchestrator_stable_since,
        );

        match result {
            OrchestratorIdleResult::Idle => {
                // Fall through to deliver notifications
            }
            OrchestratorIdleResult::Busy => {
                self.state.orchestrator_last_content = current_content;
                self.state.orchestrator_stable_since = Some(Instant::now());
                return;
            }
            OrchestratorIdleResult::Waiting => {
                if self.state.orchestrator_stable_since.is_none() {
                    self.state.orchestrator_stable_since = Some(Instant::now());
                }
                return;
            }
        }

        // Orchestrator is idle — deliver pending notifications
        let db = match &self.state.db {
            Some(db) => db,
            None => return,
        };

        let notifications = match db.consume_notifications() {
            Ok(n) if !n.is_empty() => n,
            _ => return,
        };

        let messages: Vec<String> = notifications.iter().map(|n| n.message.clone()).collect();
        let combined = format!("[agtx] {}", messages.join(" | "));
        let _ = self.state.tmux_ops.send_keys(&orch_target, &combined);

        // Reset idle tracking since we just sent input
        self.state.orchestrator_last_content.clear();
        self.state.orchestrator_stable_since = None;
    }

    /// Spawn a background thread to check phase statuses if no refresh is already running
    /// and the cache has expired for at least one task.
    fn maybe_spawn_session_refresh(&mut self) {
        // Don't spawn if a refresh is already in flight
        if self.state.session_refresh_rx.is_some() {
            self.state.spinner_frame = self.state.spinner_frame.wrapping_add(1);
            return;
        }

        let now = Instant::now();
        const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

        // Collect tasks that need checking (cache expired or never checked)
        let tasks_to_check: Vec<_> = self
            .state
            .board
            .tasks
            .iter()
            .filter(|t| {
                matches!(
                    t.status,
                    TaskStatus::Planning | TaskStatus::Running | TaskStatus::Review
                ) || (t.status == TaskStatus::Backlog && t.session_name.is_some())
            })
            .filter(|t| t.worktree_path.is_some() || t.session_name.is_some())
            .filter(|t| {
                self.state
                    .phase_status_cache
                    .get(&t.id)
                    .map_or(true, |(_, ts)| now.duration_since(*ts) >= CACHE_TTL)
            })
            .map(|t| {
                // was_ready: true if previously Ready OR not in cache yet (first poll after startup).
                // This avoids false "newly ready" notifications for tasks that were already done before restart.
                let was_ready = self
                    .state
                    .phase_status_cache
                    .get(&t.id)
                    .map_or(true, |(prev, _)| *prev == PhaseStatus::Ready);
                (
                    t.id.clone(),
                    t.status,
                    t.worktree_path.clone(),
                    t.plugin.clone(),
                    t.session_name.clone(),
                    t.cycle,
                    was_ready,
                    t.agent.clone(),
                )
            })
            .collect();

        if tasks_to_check.is_empty() {
            self.state.spinner_frame = self.state.spinner_frame.wrapping_add(1);
            return;
        }

        let project_path = self.state.project_path.clone();
        let tmux_ops = Arc::clone(&self.state.tmux_ops);

        let (tx, rx) = mpsc::channel();
        self.state.session_refresh_rx = Some(rx);

        std::thread::spawn(move || {
            let mut plugin_cache: HashMap<Option<String>, Option<WorkflowPlugin>> = HashMap::new();
            let mut statuses = Vec::new();

            for (
                task_id,
                status,
                worktree_path,
                task_plugin,
                session_name,
                cycle,
                was_ready,
                agent,
            ) in tasks_to_check
            {
                let plugin =
                    plugin_cache
                        .entry(task_plugin.clone())
                        .or_insert_with(|| match &task_plugin {
                            Some(name) => WorkflowPlugin::load(name, project_path.as_deref())
                                .ok()
                                .or_else(|| skills::load_bundled_plugin(name)),
                            None => skills::load_bundled_plugin("agtx"),
                        });

                let phase_status = if status == TaskStatus::Backlog {
                    // Preresearch copy-back
                    if let (Some(ref wt), Some(ref pp)) = (&worktree_path, &project_path) {
                        if let Some(ref p) = plugin {
                            if let Some(entries) = p.copy_back.get("preresearch") {
                                let all_at_root = !entries.is_empty()
                                    && entries.iter().all(|e| pp.join(e).exists());
                                if !all_at_root && !p.artifacts.preresearch.is_empty() {
                                    let any_artifact = p.artifacts.preresearch.iter().all(|a| {
                                        let path = Path::new(wt).join(a);
                                        if a.contains('*') {
                                            glob_path_exists(&path.to_string_lossy())
                                        } else {
                                            path.exists()
                                        }
                                    });
                                    if any_artifact {
                                        copy_back_to_project(Path::new(wt), pp, entries);
                                    }
                                }
                            }
                        }
                    }

                    let found = worktree_path
                        .as_ref()
                        .map_or(false, |wt| research_artifact_exists(wt, &task_id, plugin));
                    if found {
                        PhaseStatus::Ready
                    } else {
                        PhaseStatus::Working
                    }
                } else if let Some(ref wt) = worktree_path {
                    if phase_artifact_exists(wt, status, plugin, cycle) {
                        PhaseStatus::Ready
                    } else {
                        PhaseStatus::Working
                    }
                } else {
                    PhaseStatus::Working
                };

                // Copy-back on Working → Ready transition
                if phase_status == PhaseStatus::Ready && !was_ready {
                    if let (Some(ref wt), Some(ref pp)) = (&worktree_path, &project_path) {
                        let phase_name = if status == TaskStatus::Backlog {
                            "research"
                        } else {
                            status.as_str()
                        };
                        if let Some(ref p) = plugin {
                            if let Some(entries) = p.copy_back.get(phase_name) {
                                copy_back_to_project(Path::new(wt), pp, entries);
                            }
                        }
                    }
                }

                // Check if tmux window still exists — if not, mark as Exited
                // (unless phase artifact was found, in which case it completed before the crash)
                let window_gone = session_name
                    .as_ref()
                    .map_or(false, |sn| !tmux_ops.window_exists(sn).unwrap_or(true));
                let phase_status = if window_gone && phase_status == PhaseStatus::Working {
                    PhaseStatus::Exited
                } else {
                    phase_status
                };

                // Capture tmux content hash for idle detection (only when Working and window alive)
                let content_hash = if phase_status == PhaseStatus::Working && !window_gone {
                    session_name.as_ref().and_then(|sn| {
                        tmux_ops.capture_pane(sn).ok().map(|content| {
                            use std::hash::{Hash, Hasher};
                            let mut hasher = std::collections::hash_map::DefaultHasher::new();
                            content.hash(&mut hasher);
                            hasher.finish()
                        })
                    })
                } else {
                    None
                };

                statuses.push(SessionTaskStatus {
                    task_id,
                    phase_status,
                    content_hash,
                    status,
                    worktree_path,
                    session_name,
                    agent,
                    was_ready,
                });
            }

            let _ = tx.send(SessionRefreshResult { statuses });
        });
    }

    /// Apply results from the background session refresh thread.
    fn apply_session_refresh(&mut self, result: SessionRefreshResult) {
        let now = Instant::now();

        for task_status in result.statuses {
            let mut phase = task_status.phase_status;

            if phase == PhaseStatus::Working {
                // Idle detection: check if content hash has been stable for 15s
                if let Some(hash) = task_status.content_hash {
                    let entry = self
                        .state
                        .pane_content_hashes
                        .entry(task_status.task_id.clone())
                        .or_insert((hash, now));
                    if entry.0 != hash {
                        *entry = (hash, now);
                    } else if now.duration_since(entry.1) >= std::time::Duration::from_secs(15) {
                        phase = PhaseStatus::Idle;
                    }
                }
            } else if phase == PhaseStatus::Ready {
                self.state.pane_content_hashes.remove(&task_status.task_id);
            } else if phase == PhaseStatus::Exited {
                self.state.pane_content_hashes.remove(&task_status.task_id);
            }

            let newly_ready = phase == PhaseStatus::Ready && !task_status.was_ready;
            self.state
                .phase_status_cache
                .insert(task_status.task_id.clone(), (phase, now));

            // Notify orchestrator when a phase completes (newly Ready)
            if newly_ready {
                if self.state.orchestrator_session.is_some() {
                    if let Some(db) = &self.state.db {
                        let task_title = self
                            .state
                            .board
                            .tasks
                            .iter()
                            .find(|t| t.id == task_status.task_id)
                            .map(|t| t.title.as_str())
                            .unwrap_or("unknown");
                        let phase_name = if task_status.status == TaskStatus::Backlog {
                            "research"
                        } else {
                            task_status.status.as_str()
                        };
                        let short_id = if task_status.task_id.len() >= 8 {
                            &task_status.task_id[..8]
                        } else {
                            &task_status.task_id
                        };
                        let notif = crate::db::Notification::new(format!(
                            "Task \"{}\" ({}) completed phase: {}",
                            task_title, short_id, phase_name
                        ));
                        let _ = db.create_notification(&notif);
                    }
                }
            }

            // Auto merge-conflict check for Review tasks
            if task_status.status == TaskStatus::Review
                && !self
                    .state
                    .merge_conflict_checked
                    .contains(&task_status.task_id)
            {
                let should_check = match phase {
                    PhaseStatus::Ready => newly_ready,
                    PhaseStatus::Idle => self
                        .state
                        .pane_content_hashes
                        .get(&task_status.task_id)
                        .map_or(false, |(_, last_change)| {
                            now.duration_since(*last_change) >= std::time::Duration::from_secs(30)
                        }),
                    _ => false,
                };

                if should_check {
                    if let (Some(ref wt), Some(ref sn)) =
                        (&task_status.worktree_path, &task_status.session_name)
                    {
                        if self.state.tmux_ops.window_exists(sn).unwrap_or(false) {
                            self.state
                                .merge_conflict_checked
                                .insert(task_status.task_id.clone());

                            let git_ops = Arc::clone(&self.state.git_ops);
                            let tmux_ops = Arc::clone(&self.state.tmux_ops);
                            let wt = wt.clone();
                            let sn = sn.clone();
                            let agent_name = task_status.agent.clone();

                            std::thread::spawn(move || {
                                match git_ops.fetch_and_check_conflicts(Path::new(&wt)) {
                                    Ok(true) => {
                                        let skill_cmd = skills::transform_plugin_command(
                                            "/agtx:merge-conflicts",
                                            &agent_name,
                                        );
                                        send_skill_and_prompt(
                                            &tmux_ops,
                                            &sn,
                                            &skill_cmd,
                                            "The feature branch has merge conflicts with the default branch. Please resolve them now.",
                                            &None,
                                            "",
                                            &agent_name,
                                            &[],
                                        );
                                    }
                                    Ok(false) | Err(_) => {}
                                }
                            });
                        }
                    }
                }
            }

            // Stuck-task notification: fire once when Planning/Running task has been Idle for 1+ min
            // Void plugin tasks are fully user-managed — no stuck notifications
            let task_plugin = self
                .state
                .board
                .tasks
                .iter()
                .find(|t| t.id == task_status.task_id)
                .and_then(|t| t.plugin.as_deref());
            if matches!(
                task_status.status,
                TaskStatus::Planning | TaskStatus::Running
            ) && phase == PhaseStatus::Idle
                && self.state.orchestrator_session.is_some()
                && should_send_stuck_notification(task_plugin)
            {
                let stuck_key = format!("{}:{}", task_status.task_id, task_status.status.as_str());
                if !self.state.stuck_task_notified.contains(&stuck_key) {
                    // Track when this task first became Idle
                    let idle_since = self
                        .state
                        .stuck_task_idle_since
                        .entry(task_status.task_id.clone())
                        .or_insert(now);

                    if now.duration_since(*idle_since) >= std::time::Duration::from_secs(60) {
                        self.state.stuck_task_notified.insert(stuck_key);

                        if let Some(db) = &self.state.db {
                            let task_title = self
                                .state
                                .board
                                .tasks
                                .iter()
                                .find(|t| t.id == task_status.task_id)
                                .map(|t| t.title.as_str())
                                .unwrap_or("unknown");
                            let short_id = if task_status.task_id.len() >= 8 {
                                &task_status.task_id[..8]
                            } else {
                                &task_status.task_id
                            };
                            let notif = crate::db::Notification::new(format!(
                                "Task \"{}\" ({}) has been idle for 1m in phase: {}",
                                task_title,
                                short_id,
                                task_status.status.as_str()
                            ));
                            let _ = db.create_notification(&notif);
                        }
                    }
                }
            } else if phase != PhaseStatus::Idle {
                // Task is no longer idle — reset the idle-since timer
                self.state
                    .stuck_task_idle_since
                    .remove(&task_status.task_id);
            }
        }

        self.state.spinner_frame = self.state.spinner_frame.wrapping_add(1);
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
        self.state.tmux_project_name = tmux::safe_session_name(&project.name);
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
        ensure_project_tmux_session(
            &self.state.tmux_project_name,
            &project_path,
            self.state.tmux_ops.as_ref(),
        );

        // Clear per-task caches from previous project
        self.state.merge_conflict_checked.clear();
        self.state.stuck_task_notified.clear();
        self.state.stuck_task_idle_since.clear();

        // Reload tasks for new project
        self.refresh_tasks()?;

        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        match self.terminal.backend_mut() {
            AppBackend::Crossterm(backend) => {
                let _ = disable_raw_mode();
                let _ = execute!(backend, LeaveAlternateScreen);
            }
            #[cfg(feature = "test-mocks")]
            AppBackend::Test(_) => {}
        }
    }
}

/// Ensure tmux session exists for a project
// =============================================================================
// Orchestrator idle detection (extracted for testability)
// =============================================================================

/// Result of checking whether the orchestrator is idle and ready for notifications.
#[derive(Debug, PartialEq)]
enum OrchestratorIdleResult {
    /// Agent is idle — safe to deliver notifications.
    Idle,
    /// Content changed and no idle signal — agent is actively working.
    Busy,
    /// Content unchanged but not stable long enough — keep waiting.
    Waiting,
}

/// Idle detection duration for the stability fallback (no `[agtx:idle]` signal).
const ORCHESTRATOR_IDLE_FALLBACK_SECS: u64 = 15;

/// Pure idle-detection logic for the orchestrator pane.
///
/// Checks two conditions (first match wins):
/// 1. **Explicit signal**: pane content contains `[agtx:idle]` → `Idle`
/// 2. **Stability fallback**: pane unchanged for ≥15s → `Idle`
///
/// Returns `Busy` when content changed without the idle signal,
/// `Waiting` when content is unchanged but the timer hasn't elapsed.
fn check_orchestrator_idle(
    current_content: &str,
    last_content: &str,
    stable_since: Option<Instant>,
) -> OrchestratorIdleResult {
    let has_idle_signal = current_content.contains("[agtx:idle]");
    let content_changed = current_content != last_content;

    if content_changed {
        if has_idle_signal {
            OrchestratorIdleResult::Idle
        } else {
            OrchestratorIdleResult::Busy
        }
    } else {
        // Content unchanged — check stability timer
        match stable_since {
            Some(t)
                if t.elapsed()
                    >= std::time::Duration::from_secs(ORCHESTRATOR_IDLE_FALLBACK_SECS) =>
            {
                OrchestratorIdleResult::Idle
            }
            _ => OrchestratorIdleResult::Waiting,
        }
    }
}

fn ensure_project_tmux_session(
    project_name: &str,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
) {
    if !tmux_ops.has_session(project_name) {
        let _ = tmux_ops.create_session(project_name, &project_path.to_string_lossy());
    }
}

/// Recover a task's tmux session by creating a new window with the agent's resume command.
/// Used when the tmux window has been lost (server restart, manual kill, etc.)
/// but the task's worktree and agent session data still exist on disk.
/// Returns the tmux target string on success.
fn recover_task_session(
    task: &Task,
    project_name: &str,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    agent_ops: &dyn AgentOperations,
) -> Result<String> {
    let worktree_path = task
        .worktree_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Task has no worktree path"))?;

    if !Path::new(worktree_path).exists() {
        anyhow::bail!("Worktree no longer exists: {}", worktree_path);
    }

    let target = task
        .session_name
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("Task has no session name"))?;
    let (session, window) = target
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("Invalid session name format: {}", target))?;

    ensure_project_tmux_session(project_name, project_path, tmux_ops);

    let resume_cmd = agent_ops.build_resume_command();

    tmux_ops.create_window(session, window, worktree_path, Some(resume_cmd))?;

    Ok(target.clone())
}

/// Copy files/dirs from worktree back to project root.
/// Used by plugins with `[copy_back]` to sync artifacts after phase completion.
fn copy_back_to_project(worktree: &Path, project_root: &Path, entries: &[String]) {
    for entry in entries {
        let src = worktree.join(entry);
        let dst = project_root.join(entry);
        if !src.exists() {
            continue;
        }
        if src.is_dir() {
            let _ = crate::git::copy_dir_recursive(&src, &dst);
        } else {
            // Ensure parent directory exists for nested file paths
            if let Some(parent) = dst.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::copy(&src, &dst);
        }
    }
}


/// Generate a URL-safe slug from task ID and title
fn generate_task_slug(task_id: &str, title: &str) -> String {
    let title_slug: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .take(30)
        .collect();
    let title_slug = title_slug.trim_matches('-').to_string();

    // Add task ID prefix to ensure uniqueness
    let id_prefix: String = task_id.chars().take(8).collect();
    format!("{}-{}", id_prefix, title_slug)
}

fn run_cleanup_script_for_worktree(cleanup_script: Option<&str>, worktree_path: &Path) {
    let Some(script) = cleanup_script else {
        return;
    };
    let script = script.trim();
    if script.is_empty() {
        return;
    }

    match git::run_worktree_script(script, worktree_path, &[]) {
        Err(e) => eprintln!("cleanup_script failed to run: {}", e),
        Ok(output) => {
            if !output.status.success() {
                eprintln!(
                    "cleanup_script exited with {}: {}",
                    output.status,
                    output.stderr.trim()
                );
            }
        }
    }
}

/// Cleanup task resources (tmux window, cleanup script, git worktree) and mark as done
/// Modifies the task in place, ready for database update
fn cleanup_task_for_done(
    task: &mut Task,
    cleanup_script: Option<&str>,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    // Archive artifacts before removing worktree
    if let Some(worktree) = &task.worktree_path {
        let artifacts_dir = Path::new(worktree).join(".agtx");
        if artifacts_dir.exists() {
            let slug = task
                .branch_name
                .as_deref()
                .and_then(|b| b.strip_prefix("task/"))
                .unwrap_or(&task.id);
            let archive_dir = project_path.join(".agtx").join("archive").join(slug);
            if let Ok(()) = std::fs::create_dir_all(&archive_dir) {
                if let Ok(entries) = std::fs::read_dir(&artifacts_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
                            let _ = std::fs::copy(&path, archive_dir.join(entry.file_name()));
                        }
                    }
                }
            }
        }
    }

    if let Some(session_name) = &task.session_name {
        let _ = tmux_ops.kill_window(session_name);
    }
    if let Some(worktree) = &task.worktree_path {
        run_cleanup_script_for_worktree(cleanup_script, Path::new(worktree));
        let _ = git_ops.remove_worktree(project_path, worktree);
    }
    // Keep the branch so task can be reopened later
    task.session_name = None;
    task.worktree_path = None;
    task.status = TaskStatus::Done;
    task.updated_at = chrono::Utc::now();
}

/// Background-safe cleanup: archive artifacts, kill tmux window, run cleanup script, remove worktree.
/// Takes owned/cloned values so it can run in a spawned thread.
fn cleanup_task_resources(
    task_id: &str,
    branch_name: &Option<String>,
    session_name: &Option<String>,
    worktree_path: &Option<String>,
    cleanup_script: Option<&str>,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    // Archive artifacts before removing worktree
    if let Some(worktree) = worktree_path {
        let artifacts_dir = Path::new(worktree).join(".agtx");
        if artifacts_dir.exists() {
            let slug = branch_name
                .as_deref()
                .and_then(|b| b.strip_prefix("task/"))
                .unwrap_or(task_id);
            let archive_dir = project_path.join(".agtx").join("archive").join(slug);
            if let Ok(()) = std::fs::create_dir_all(&archive_dir) {
                if let Ok(entries) = std::fs::read_dir(&artifacts_dir) {
                    for entry in entries.flatten() {
                        let path = entry.path();
                        if path.is_file() && path.extension().map_or(false, |ext| ext == "md") {
                            let _ = std::fs::copy(&path, archive_dir.join(entry.file_name()));
                        }
                    }
                }
            }
        }
    }

    if let Some(session_name) = session_name {
        let _ = tmux_ops.kill_window(session_name);
    }
    if let Some(worktree) = worktree_path {
        run_cleanup_script_for_worktree(cleanup_script, Path::new(worktree));
        let _ = git_ops.remove_worktree(project_path, worktree);
    }
}

/// Set up a worktree and tmux window for a task.
/// Creates worktree, initializes it (copy files + init script), creates tmux window with agent.
/// Updates task fields (session_name, worktree_path, branch_name) in place.
/// Returns the tmux target string on success.
///
/// `prompt` is used only for agents without native skill invocation (fallback).
/// For agents with skill support, the agent starts with no prompt and the skill command
/// is sent later via send_keys (see the acceptance thread in move_task_right).
fn setup_task_worktree(
    task: &mut Task,
    project_path: &Path,
    tmux_project_name: &str,
    prompt: &str,
    base_branch: &str,
    worktree_dir: &str,
    copy_files: Option<String>,
    init_script: Option<String>,
    plugin: &Option<WorkflowPlugin>,
    agent_name: &str,
    all_phase_agents: &[String],
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
    agent_ops: &dyn AgentOperations,
    referenced_tasks: &[ReferencedTaskInfo],
) -> Result<String> {
    let unique_slug = generate_task_slug(&task.id, &task.title);
    let window_name = format!("task-{}", unique_slug);
    let target = format!("{}:{}", tmux_project_name, window_name);

    // Create git worktree from the configured base branch
    let worktree_path_str =
        match git_ops.create_worktree(project_path, &unique_slug, base_branch, worktree_dir) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("Failed to create worktree: {}", e);
                project_path
                    .join(worktree_dir)
                    .join(&unique_slug)
                    .to_string_lossy()
                    .to_string()
            }
        };

    // Initialize worktree: copy files and run init script
    // Merge plugin-level copy_files with project-level copy_files
    let worktree_path = Path::new(&worktree_path_str);
    let copy_dirs = plugin
        .as_ref()
        .map_or_else(Vec::new, |p| p.copy_dirs.clone());
    let merged_copy_files = {
        let mut parts: Vec<String> = Vec::new();
        if let Some(ref cf) = copy_files {
            if !cf.trim().is_empty() {
                parts.push(cf.clone());
            }
        }
        if let Some(ref p) = plugin {
            if !p.copy_files.is_empty() {
                parts.push(p.copy_files.join(","));
            }
        }
        if parts.is_empty() {
            None
        } else {
            Some(parts.join(","))
        }
    };
    let init_warnings = git_ops.initialize_worktree(
        project_path,
        worktree_path,
        merged_copy_files,
        init_script,
        copy_dirs,
    );
    // Warnings from copy_files are expected (e.g. files don't exist yet on first run)
    let _ = &init_warnings;

    // Write skills to worktree .agtx/skills/ and agent-native discovery paths
    // Deploy for all unique agents configured across phases
    let agent_refs: Vec<&str> = all_phase_agents.iter().map(|s| s.as_str()).collect();
    write_skills_to_worktree(&worktree_path_str, project_path, plugin, &agent_refs);

    // Copy referenced task artifacts into .agtx/references/
    if !referenced_tasks.is_empty() {
        let refs_dir = worktree_path.join(".agtx").join("references");
        for ref_info in referenced_tasks {
            // 1. Git diff of referenced task's branch
            if let Some(ref branch) = ref_info.branch_name {
                if let Ok(output) = std::process::Command::new("git")
                    .args(["diff", &format!("main..{}", branch)])
                    .current_dir(project_path)
                    .output()
                {
                    if output.status.success() && !output.stdout.is_empty() {
                        let _ = std::fs::create_dir_all(&refs_dir);
                        let diff_path = refs_dir.join(format!("{}.diff", ref_info.slug));
                        let _ = std::fs::write(&diff_path, &output.stdout);
                    }
                }
            }
            // 2. Copy artifact files from referenced task's worktree (if it still exists)
            if let Some(ref wt) = ref_info.worktree_path {
                let wt_path = Path::new(wt);
                if wt_path.exists() {
                    let dest = refs_dir.join(&ref_info.slug);
                    let _ = std::fs::create_dir_all(&dest);
                    // Copy common artifact locations
                    for pattern in &[".agtx/skills", ".planning"] {
                        let src = wt_path.join(pattern);
                        if src.exists() {
                            let target_dir = dest.join(pattern);
                            let _ = crate::git::copy_dir_recursive(&src, &target_dir);
                        }
                    }
                }
            }
        }
    }

    // Run plugin init_script (in addition to project init_script)
    // Supports {agent} placeholder for agent-specific initialization
    if let Some(ref p) = plugin {
        if let Some(ref script) = p.init_script {
            let script = script.replace("{agent}", agent_name);
            let output = std::process::Command::new("sh")
                .arg("-c")
                .arg(&script)
                .current_dir(&worktree_path_str)
                .output();
            match output {
                Ok(o) if !o.status.success() => {
                    let stderr = String::from_utf8_lossy(&o.stderr);
                    anyhow::bail!(
                        "Plugin init_script failed (exit {}): {}\n{}",
                        o.status.code().unwrap_or(-1),
                        script,
                        stderr.trim()
                    );
                }
                Err(e) => {
                    anyhow::bail!("Plugin init_script failed to run: {}\n{}", script, e);
                }
                _ => {}
            }
        }
    }

    // Build the interactive command. For agents with skill/command support,
    // start with no prompt — the skill command and task content are sent via send_keys.
    let has_skill_support =
        resolve_skill_command(plugin, "planning", agent_name, "", task.cycle).is_some();
    let agent_cmd = if has_skill_support {
        agent_ops.build_interactive_command("")
    } else {
        agent_ops.build_interactive_command(prompt)
    };

    // Ensure project tmux session exists
    ensure_project_tmux_session(tmux_project_name, project_path, tmux_ops);

    tmux_ops.create_window(
        tmux_project_name,
        &window_name,
        &worktree_path_str,
        Some(agent_cmd),
    )?;

    task.session_name = Some(target.clone());
    task.worktree_path = Some(worktree_path_str);
    task.branch_name = Some(format!("task/{}", unique_slug));

    Ok(target)
}

/// Delete task resources: kill tmux window, run cleanup script, remove worktree, delete branch
fn delete_task_resources(
    task: &Task,
    cleanup_script: Option<&str>,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    // Kill tmux window if exists
    if let Some(ref session_name) = task.session_name {
        let _ = tmux_ops.kill_window(session_name);
    }

    // Remove worktree and delete branch if exists
    if let Some(ref worktree) = task.worktree_path {
        if let Some(ref branch_name) = task.branch_name {
            run_cleanup_script_for_worktree(cleanup_script, Path::new(worktree));
            let _ = git_ops.remove_worktree(project_path, worktree);
            let _ = git_ops.delete_branch(project_path, branch_name);
        }
    }
}

/// Collect git diff content from a worktree
/// Returns formatted diff sections (unstaged, staged, untracked)
fn collect_task_diff(
    worktree_path: &str,
    git_ops: &dyn GitOperations,
    exclude_prefixes: &[&str],
) -> String {
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
            let file = file.trim();
            if file.is_empty() {
                continue;
            }
            // Skip files in copied directories (agent configs, plugin dirs)
            if exclude_prefixes
                .iter()
                .any(|prefix| file.starts_with(&format!("{}/", prefix.trim_end_matches('/'))))
            {
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
fn capture_tmux_pane_with_history(
    window_name: &str,
    history_lines: i32,
    tmux_ops: &dyn TmuxOperations,
) -> Vec<u8> {
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
        let commit_msg = format!(
            "{}\n\nCo-Authored-By: {}",
            pr_title,
            agent_ops.co_author_string()
        );
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
        let commit_msg = format!(
            "Address review comments\n\nCo-Authored-By: {}",
            agent_ops.co_author_string()
        );
        git_ops.commit(worktree_path, &commit_msg)?;
    }

    // Push the branch
    if let Some(branch) = &task.branch_name {
        git_ops.push(worktree_path, branch, false)?;
    }

    // Return the existing PR URL
    Ok(task
        .pr_url
        .clone()
        .unwrap_or_else(|| "Changes pushed to existing PR".to_string()))
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

    let codes: Vec<u8> = seq.split(';').filter_map(|s| s.parse().ok()).collect();

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
fn fuzzy_find_files(
    project_path: &Path,
    pattern: &str,
    max_results: usize,
    git_ops: &dyn GitOperations,
) -> Vec<String> {
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

    matches
        .into_iter()
        .take(max_results)
        .map(|(path, _)| path)
        .collect()
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

/// Resolve the task prompt for a given phase transition, using plugin prompt template.
/// Substitutes {task}, {task_id}, and {phase} placeholders. Returns empty if no template is configured.
fn resolve_prompt(
    plugin: &Option<WorkflowPlugin>,
    phase: &str,
    task_content: &str,
    task_id: &str,
    cycle: i32,
) -> String {
    let template = match phase {
        "preresearch" | "research" => plugin
            .as_ref()
            .and_then(|p| p.prompts.research.as_deref())
            .unwrap_or(""),
        "planning" => plugin
            .as_ref()
            .and_then(|p| p.prompts.planning.as_deref())
            .unwrap_or(""),
        "planning_with_research" => plugin
            .as_ref()
            .and_then(|p| p.prompts.planning_with_research.as_deref())
            .unwrap_or(""),
        "running" => plugin
            .as_ref()
            .and_then(|p| p.prompts.running.as_deref())
            .unwrap_or(""),
        "running_with_research_or_planning" => plugin
            .as_ref()
            .and_then(|p| p.prompts.running_with_research_or_planning.as_deref())
            .unwrap_or(""),
        "review" => plugin
            .as_ref()
            .and_then(|p| p.prompts.review.as_deref())
            .unwrap_or(""),
        _ => return task_content.to_string(),
    };

    if template.is_empty() {
        return String::new();
    }

    template
        .replace("{task}", task_content)
        .replace("{task_id}", task_id)
        .replace("{phase}", &cycle.to_string())
}

/// Resolve the skill command to send via send_keys for a given phase.
/// Returns the plugin command transformed for the target agent, or None if no command is configured.
fn resolve_skill_command(
    plugin: &Option<WorkflowPlugin>,
    phase: &str,
    agent_name: &str,
    task_content: &str,
    cycle: i32,
) -> Option<String> {
    let p = plugin.as_ref()?;

    // Commands are stored in canonical form (Claude/Gemini syntax) and transformed per agent
    // Commands may contain {task} and {phase} placeholders
    let cmd = match phase {
        "preresearch" => p
            .commands
            .preresearch
            .as_deref()
            .or(p.commands.research.as_deref()),
        "research" => p.commands.research.as_deref(),
        "planning" | "planning_with_research" => p.commands.planning.as_deref(),
        "running" | "running_with_research_or_planning" => p.commands.running.as_deref(),
        "review" => p.commands.review.as_deref(),
        _ => None,
    }?;

    if cmd.is_empty() {
        // Explicit empty command means "no command" (e.g., void plugin)
        return None;
    }

    // When a prior phase was done, strip {task} — agent already has context
    let expanded =
        if phase == "planning_with_research" || phase == "running_with_research_or_planning" {
            cmd.replace("{task}", "").trim().to_string()
        } else {
            // Collapse task content to single line for commands (newlines → spaces)
            let task_oneline = task_content
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            cmd.replace("{task}", &task_oneline)
        };
    let expanded = expanded.replace("{phase}", &cycle.to_string());
    skills::transform_plugin_command(&expanded, agent_name)
}

/// Spawn a background thread that optionally switches agent, waits for readiness,
/// then sends a skill command and prompt to the tmux pane.
fn spawn_send_to_agent(
    tmux_ops: Arc<dyn TmuxOperations>,
    agent_registry: Arc<dyn agent::AgentRegistry>,
    target: String,
    current_agent: String,
    target_agent: String,
    needs_switch: bool,
    skill_cmd: Option<String>,
    prompt: String,
    prompt_trigger: Option<String>,
    task_content: String,
    auto_dismiss: Vec<crate::config::AutoDismiss>,
    worktree_path: Option<String>,
    project_path: std::path::PathBuf,
    plugin: Option<WorkflowPlugin>,
) {
    std::thread::spawn(move || {
        // If the tmux window is gone, recover it with the agent's resume command
        {
            let agent_ops = agent_registry.get(&target_agent);
            ensure_window_or_recover(
                tmux_ops.as_ref(),
                &target,
                agent_ops.as_ref(),
                worktree_path.as_deref(),
            );
        }

        if needs_switch {
            // Deploy skills for the incoming agent only if its native skill directory
            // doesn't exist yet. This handles the case where a worktree was created
            // with a different agent (e.g. Claude for planning) and a new agent
            // (e.g. OpenCode for review) is switched in later.
            if let Some(ref wt_path) = worktree_path {
                let already_deployed = skills::agent_native_skill_dir(&target_agent)
                    .map(|(base, namespace)| {
                        let dir = if namespace.is_empty() {
                            Path::new(wt_path).join(base)
                        } else {
                            Path::new(wt_path).join(base).join(namespace)
                        };
                        dir.exists()
                    })
                    .unwrap_or(true); // no native path for this agent — nothing to deploy
                if !already_deployed {
                    write_skills_to_worktree(wt_path, &project_path, &plugin, &[&target_agent]);
                }
            }
            let agent_ops = agent_registry.get(&target_agent);
            let new_cmd = agent_ops.build_interactive_command("");
            switch_agent_in_tmux(tmux_ops.as_ref(), &target, &current_agent, &new_cmd);
            let _ = wait_for_agent_ready(&tmux_ops, &target);
        }
        send_skill_and_prompt(
            &tmux_ops,
            &target,
            &skill_cmd,
            &prompt,
            &prompt_trigger,
            &task_content,
            &target_agent,
            &auto_dismiss,
        );
    });
}

/// Send skill command and prompt to the agent via tmux.
/// When there is no prompt_trigger, combines skill command + prompt into a single message
/// (separated by a newline). When a prompt_trigger is set, sends them as two separate messages
/// with the prompt sent only after the trigger text appears in the pane.
fn send_skill_and_prompt(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    skill_cmd: &Option<String>,
    prompt: &str,
    prompt_trigger: &Option<String>,
    task_content: &str,
    agent_name: &str,
    auto_dismiss: &[crate::config::AutoDismiss],
) {
    // Gemini, Codex & OpenCode: always combine skill+prompt into a single message.
    // Gemini: sending separately causes it to execute the skill and queue the
    //   prompt, which gets lost or arrives too late.
    // Codex: skill mentions ($skill-name) are inline references that must be
    //   part of a message — sending just "$skill" standalone does nothing.
    // OpenCode: same Ink TUI behavior as Gemini/Codex, needs combined send + double Enter.
    if matches!(agent_name, "gemini" | "codex" | "cursor" | "opencode") {
        let text_to_send = if let Some(cmd) = skill_cmd {
            if !prompt.is_empty() {
                Some(format!("{}\n\n{}", cmd, prompt))
            } else {
                Some(cmd.clone())
            }
        } else if !prompt.is_empty() {
            Some(prompt.to_string())
        } else {
            let oneline = task_content
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            if !oneline.is_empty() {
                Some(oneline)
            } else {
                None
            }
        };

        if let Some(text) = text_to_send {
            let _ = tmux_ops.send_keys_literal(target, &text);
            // Wait for text to appear in pane before sending Enter (Ink TUIs need time to render)
            let check_str = text.lines().next().unwrap_or(&text);
            for _ in 0..20 {
                // up to 4s
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Ok(content) = tmux_ops.capture_pane(target) {
                    if content.contains(check_str) {
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = tmux_ops.send_keys_literal(target, "Enter");

            // Codex and OpenCode show a command picker popup when a skill is typed.
            // The first Enter confirms/closes the picker; a second Enter is needed
            // to actually submit the message.
            // - Codex: wait for "Press enter to insert" to disappear
            // - OpenCode: wait a short delay (picker closes immediately on Enter)
            if agent_name == "codex" {
                for _ in 0..20 {
                    // up to 4s
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Ok(content) = tmux_ops.capture_pane(target) {
                        if !content.contains("Press enter to insert") {
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                let _ = tmux_ops.send_keys_literal(target, "Enter");
            } else if agent_name == "opencode" {
                std::thread::sleep(std::time::Duration::from_millis(400));
                let _ = tmux_ops.send_keys_literal(target, "Enter");
            }
        }
        return;
    }

    match (skill_cmd, prompt_trigger) {
        // Skill + prompt trigger: must send separately, wait for trigger between them
        (Some(cmd), Some(trigger)) => {
            let _ = tmux_ops.send_keys(target, cmd);
            if !prompt.is_empty() {
                if wait_for_prompt_trigger(tmux_ops, target, trigger, auto_dismiss) {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let _ = tmux_ops.send_keys(target, prompt);
                }
            }
        }
        // Skill + prompt, no trigger: send separately, wait for agent to finish processing
        (Some(cmd), None) => {
            let _ = tmux_ops.send_keys(target, cmd);
            if !prompt.is_empty() {
                // Wait for agent to process the skill command and become idle again.
                // Requires at least 1 content change (agent started processing) before
                // counting stability, to avoid false-positive when the command hasn't
                // been picked up yet.
                let mut last_content = String::new();
                let mut stable_ticks = 0u32;
                let mut change_count = 0u32;
                for _ in 0..75 {
                    // 15s max
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Ok(content) = tmux_ops.capture_pane(target) {
                        if content != last_content {
                            change_count += 1;
                            stable_ticks = 0;
                            last_content = content;
                        } else if change_count >= 1 {
                            stable_ticks += 1;
                            if stable_ticks >= 10 {
                                // 2s of no changes after agent responded
                                break;
                            }
                        }
                    }
                }
                let _ = tmux_ops.send_keys(target, prompt);
            }
        }
        // No skill command, just prompt
        (None, _) => {
            if !prompt.is_empty() {
                let _ = tmux_ops.send_keys(target, prompt);
            } else {
                // No command and no prompt (e.g. void plugin): prefill task in input
                let oneline = task_content
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ");
                if !oneline.is_empty() {
                    let _ = tmux_ops.send_keys_literal(target, &oneline);
                }
            }
        }
    }
}

/// Resolve the prompt trigger text for a given phase.
/// When set, the system polls the tmux pane for this text before sending the prompt.
fn resolve_prompt_trigger(plugin: &Option<WorkflowPlugin>, phase: &str) -> Option<String> {
    plugin
        .as_ref()
        .and_then(|p| match phase {
            "preresearch" | "research" => p.prompt_triggers.research.clone(),
            "planning" | "planning_with_research" => p.prompt_triggers.planning.clone(),
            "running" | "running_with_research_or_planning" => p.prompt_triggers.running.clone(),
            "review" => p.prompt_triggers.review.clone(),
            _ => None,
        })
        .filter(|s| !s.is_empty())
}

/// Wait for a specific text to appear in a tmux pane, then return.
/// Returns true if the trigger was found, false if timed out.
/// Auto-dismiss rules are checked while waiting: when all detect patterns match
/// and the pane is stable for ~2s, the response keystrokes are sent automatically.
fn wait_for_prompt_trigger(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    trigger: &str,
    auto_dismiss: &[crate::config::AutoDismiss],
) -> bool {
    let mut last_content = String::new();
    let mut stable_ticks = 0u32;

    for _ in 0..600 {
        // ~5 minutes (600 * 500ms)
        std::thread::sleep(std::time::Duration::from_millis(500));
        if let Ok(content) = tmux_ops.capture_pane(target) {
            if content == last_content {
                stable_ticks += 1;
            } else {
                stable_ticks = 0;
                last_content = content.clone();
            }

            // Auto-dismiss interactive prompts that block the trigger.
            // Requires stability (2s) to ensure the UI is ready for input.
            if stable_ticks >= 4 {
                for rule in auto_dismiss {
                    if rule.detect.iter().all(|p| content.contains(p.as_str())) {
                        for key in rule.response.split('\n') {
                            let _ = tmux_ops.send_keys_literal(target, key);
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                        stable_ticks = 0;
                        last_content.clear();
                        break;
                    }
                }
                if last_content.is_empty() {
                    continue;
                }
            }

            // Trigger as soon as the text is present in the pane.
            if content.contains(trigger) {
                return true;
            }
        }
    }
    false
}

/// Returns true if a stuck-task notification should be sent for this task.
/// Void plugin tasks are fully user-managed and never produce stuck notifications.
fn should_send_stuck_notification(plugin_name: Option<&str>) -> bool {
    plugin_name != Some("void")
}

/// Check if the phase artifact exists for a task in its worktree.
/// Tries both zero-padded (e.g. "01") and non-padded (e.g. "1") {phase} substitution.
fn phase_artifact_exists(
    worktree_path: &str,
    status: TaskStatus,
    plugin: &Option<WorkflowPlugin>,
    cycle: i32,
) -> bool {
    let rel_template = plugin.as_ref().and_then(|p| match status {
        TaskStatus::Planning => p.artifacts.planning.as_deref(),
        TaskStatus::Running => p.artifacts.running.as_deref(),
        TaskStatus::Review => p.artifacts.review.as_deref(),
        _ => None,
    });

    let Some(rel_template) = rel_template else {
        return false;
    };
    artifact_path_exists(worktree_path, rel_template, cycle)
}

/// Check if the research artifact exists for a task.
/// Tries both zero-padded (e.g. "01") and non-padded (e.g. "1") {phase} substitution.
fn research_artifact_exists(
    worktree_path: &str,
    task_id: &str,
    plugin: &Option<WorkflowPlugin>,
) -> bool {
    let Some(template) = plugin
        .as_ref()
        .and_then(|p| p.artifacts.research.as_deref())
    else {
        return false;
    };
    let rel_template = template.replace("{task_id}", task_id);
    // Research is always cycle 1
    artifact_path_exists(worktree_path, &rel_template, 1)
}

/// Determine the phase variant name based on whether prior-phase artifacts exist.
/// Returns the base phase name or a `_with_*` variant.
fn determine_phase_variant(
    phase: &str,
    worktree_path: Option<&str>,
    task_id: &str,
    plugin: &Option<WorkflowPlugin>,
    cycle: i32,
) -> &'static str {
    match phase {
        "planning" => {
            let has_research =
                worktree_path.map_or(false, |wt| research_artifact_exists(wt, task_id, plugin));
            if has_research {
                "planning_with_research"
            } else {
                "planning"
            }
        }
        "running" => {
            let has_prior = worktree_path.map_or(false, |wt| {
                research_artifact_exists(wt, task_id, plugin)
                    || phase_artifact_exists(wt, TaskStatus::Planning, plugin, cycle)
            });
            if has_prior {
                "running_with_research_or_planning"
            } else {
                "running"
            }
        }
        _ => {
            // Phases without variants leak the &str — use a known static
            match phase {
                "review" => "review",
                "research" => "research",
                _ => "running",
            }
        }
    }
}

/// Check if an artifact path exists, trying both zero-padded and non-padded {phase} substitution.
fn artifact_path_exists(worktree_path: &str, rel_template: &str, cycle: i32) -> bool {
    // Try zero-padded first (e.g. "01"), then non-padded (e.g. "1")
    for phase_str in [format!("{:02}", cycle), cycle.to_string()] {
        let rel_path = rel_template.replace("{phase}", &phase_str);
        let full_path = Path::new(worktree_path).join(&rel_path);

        if rel_path.contains('*') {
            if glob_path_exists(&full_path.to_string_lossy()) {
                return true;
            }
        } else if full_path.exists() {
            return true;
        }
    }
    false
}

/// Simple glob matching for paths with `*` wildcards.
/// Supports directory-level wildcards (e.g. "/path/*/plan.md")
/// and file-level wildcards (e.g. "/path/*-PLAN.md").
fn glob_path_exists(pattern: &str) -> bool {
    let Some(star_pos) = pattern.find('*') else {
        return Path::new(pattern).exists();
    };

    // Split at the wildcard: parent_dir / * / remainder
    let parent = &pattern[..star_pos];
    let remainder = &pattern[star_pos + 1..];
    let parent = parent.trim_end_matches('/');

    let Ok(entries) = std::fs::read_dir(parent) else {
        return false;
    };

    // File-level wildcard: * is in the last path component (e.g. "*-CONTEXT.md")
    let is_file_wildcard = !remainder.starts_with('/');

    for entry in entries.flatten() {
        let path = entry.path();
        if is_file_wildcard {
            // Match against filenames: e.g. "*-CONTEXT.md" matches "01-CONTEXT.md"
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.ends_with(remainder) {
                    return true;
                }
            }
        } else if path.is_dir() {
            let candidate = format!("{}{}", path.display(), remainder);
            if remainder.contains('*') {
                if glob_path_exists(&candidate) {
                    return true;
                }
            } else if Path::new(&candidate).exists() {
                return true;
            }
        }
    }
    false
}

/// Check if a phase transition requires switching to a different agent.
/// Returns (target_agent_name, needs_switch).
/// Determine the target agent for a phase and whether a switch is needed.
/// Uses the phase-specific agent if configured, otherwise falls back to default_agent.
fn needs_agent_switch(config: &MergedConfig, task: &Task, phase: &str) -> (String, bool) {
    let target = config.agent_for_phase(phase);
    let switch = task.agent != target;
    (target.to_string(), switch)
}

/// Collect all unique agent names configured across phases.
/// Used to deploy skills for all agents that might be used during a task's lifecycle.
fn collect_phase_agents(config: &MergedConfig) -> Vec<String> {
    let mut agents: Vec<String> = vec![config.default_agent.clone()];
    for phase in &["research", "planning", "running", "review"] {
        let agent = config.agent_for_phase(phase).to_string();
        if !agents.contains(&agent) {
            agents.push(agent);
        }
    }
    agents
}

/// Known agent binary names as they appear in `pane_current_command`.
/// Used by `is_pane_at_shell` to detect when an agent process is running.
/// Does NOT include `node` — Node/Ink agents (Gemini, Cursor, OpenCode, Codex) are
/// detected via `AGENT_ACTIVE_INDICATORS` instead, so Check 2 in `wait_for_agent_ready`
/// can fire for them rather than Check 1 firing too early.
/// Note: on systems where agents are installed via asdf/nvm, all agents run as `node`
/// and Check 1 never fires — AGENT_ACTIVE_INDICATORS is the only reliable signal there.
const AGENT_COMMANDS: &[&str] = &[
    "claude", "codex", "gemini", "copilot", "opencode", "agent", "python3", "python",
];

/// Strings in pane content that indicate a Node/Ink agent TUI is active and ready.
/// Used by `is_agent_active` to detect agents like Gemini and Cursor that run
/// inside bash/node and don't change `pane_current_command` to their own name.
/// Also used by `wait_for_agent_ready` (Check 2) to detect readiness for these agents.
const AGENT_ACTIVE_INDICATORS: &[&str] = &[
    "Claude Code",       // Claude
    "Type your message", // Gemini
    "Ask anything",      // OpenCode
    "Cursor Agent",      // Cursor
    "OpenAI Codex",      // Codex
];

/// Check if the pane is running a shell (i.e. the agent has exited).
/// Returns true when `pane_current_command` reports a shell (bash, zsh, sh, fish)
/// rather than an agent process.
fn is_pane_at_shell(tmux_ops: &dyn TmuxOperations, target: &str) -> bool {
    if let Some(cmd) = tmux_ops.pane_current_command(target) {
        !AGENT_COMMANDS.iter().any(|a| cmd.contains(a))
    } else {
        false
    }
}

/// Check if an agent is actively running in the pane.
/// Uses both `pane_current_command` (works for Claude, Codex, Copilot) and
/// pane content indicators (works for Gemini which runs inside bash).
fn is_agent_active(tmux_ops: &dyn TmuxOperations, target: &str) -> bool {
    // Check 1: agent process visible in pane_current_command
    if !is_pane_at_shell(tmux_ops, target) {
        return true;
    }
    // Check 2: check the bottom of the visible pane for agent UI indicators.
    // Only the last few lines are checked to avoid false positives from
    // indicator strings appearing in conversation output higher up.
    if let Ok(content) = tmux_ops.capture_pane(target) {
        let lines: Vec<&str> = content.lines().collect();
        let bottom = lines.len().saturating_sub(5);
        let tail = &lines[bottom..];
        let tail_text = tail.join("\n");
        if AGENT_ACTIVE_INDICATORS
            .iter()
            .any(|s| tail_text.contains(s))
        {
            return true;
        }
    }
    false
}

/// If the tmux window for `target` is gone, recreate it with the agent's resume command.
/// Used before `switch_agent_in_tmux` and `send_skill_and_prompt` to handle dead windows.
fn ensure_window_or_recover(
    tmux_ops: &dyn TmuxOperations,
    target: &str,
    agent_ops: &dyn AgentOperations,
    worktree_path: Option<&str>,
) {
    if tmux_ops.window_exists(target).unwrap_or(true) {
        return;
    }
    let Some(wt_path) = worktree_path else { return };
    if !Path::new(wt_path).exists() {
        return;
    }
    let Some((session, window)) = target.split_once(':') else {
        return;
    };
    if !tmux_ops.has_session(session) {
        let _ = tmux_ops.create_session(session, wt_path);
    }
    let resume_cmd = agent_ops.build_resume_command();
    let _ = tmux_ops.create_window(session, window, wt_path, Some(resume_cmd));
}

/// Gracefully switch the agent running in a tmux window.
/// Terminates the current agent, waits for the shell prompt,
/// then starts the new agent.
///
/// Exit commands per agent:
///   - Claude, OpenCode: `/exit`
///   - Gemini, Codex: `/quit`
///   - Fallback: Ctrl+C + Ctrl+D as last resort
///
/// Detection uses `tmux display -p #{pane_current_command}` which reports
/// the actual process name (e.g. "claude", "node", "bash"), avoiding
/// false positives from parsing pane text content.
fn switch_agent_in_tmux(
    tmux_ops: &dyn TmuxOperations,
    target: &str,
    current_agent: &str,
    new_agent_cmd: &str,
) {
    // 1. Send the graceful exit command for the current agent.
    let exit_cmd = match current_agent {
        "codex" => None, // Codex has no exit command — Ctrl+C is the only way
        "gemini" => Some("/quit"),
        "cursor" => None,   // Ink/Node TUI — Ctrl+C is the only reliable exit
        _ => Some("/exit"), // claude, opencode, and others
    };

    if let Some(cmd) = exit_cmd {
        // For Gemini (Ink/Node TUI): send text first, wait for it to appear in pane,
        // then send Enter — same pattern as send_skill_and_prompt. Without this delay,
        // Enter fires before the Ink TUI has rendered the input, and /quit is lost.
        if current_agent == "gemini" {
            let _ = tmux_ops.send_keys_literal(target, cmd);
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Ok(content) = tmux_ops.capture_pane(target) {
                    if content.contains(cmd) {
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = tmux_ops.send_keys_literal(target, "Enter");
        } else {
            let _ = tmux_ops.send_keys(target, cmd);
        }
    } else {
        let _ = tmux_ops.send_keys_literal(target, "C-c");
    }

    // 2. Poll for agent exit. If the agent was busy, the exit command
    //    may have been queued — so we wait up to 3s for it to take effect.
    //    Uses is_agent_active (checks both pane_current_command AND pane content)
    //    so that Node/Ink agents like Gemini (which always show "bash" as the
    //    process name) are correctly detected as still running.
    let mut found_shell = false;
    for _ in 0..30 {
        // 3s
        std::thread::sleep(std::time::Duration::from_millis(100));
        if !is_agent_active(tmux_ops, target) {
            found_shell = true;
            break;
        }
    }

    // 3. If still running, the agent was likely busy. Ctrl+C to cancel, then retry exit.
    if !found_shell {
        let _ = tmux_ops.send_keys_literal(target, "C-c");
        std::thread::sleep(std::time::Duration::from_millis(1000));

        if let Some(cmd) = exit_cmd {
            if current_agent == "gemini" {
                let _ = tmux_ops.send_keys_literal(target, cmd);
                for _ in 0..20 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Ok(content) = tmux_ops.capture_pane(target) {
                        if content.contains(cmd) {
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                let _ = tmux_ops.send_keys_literal(target, "Enter");
            } else {
                let _ = tmux_ops.send_keys(target, cmd);
            }
        }

        // Wait for agent exit after retry
        for _ in 0..50 {
            // 5s
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !is_agent_active(tmux_ops, target) {
                found_shell = true;
                break;
            }
        }
    }

    // 4. Last resort: Ctrl+D to force exit
    if !found_shell {
        let _ = tmux_ops.send_keys_literal(target, "C-d");
        for _ in 0..20 {
            // 2s
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !is_agent_active(tmux_ops, target) {
                break;
            }
        }
    }

    // 4. Let the shell fully initialize before sending the new agent command
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // 5. Start the new agent
    let _ = tmux_ops.send_keys(target, new_agent_cmd);

    // 6. Wait for the new agent process to actually start (pane_current_command != shell).
    //    Without this, wait_for_agent_ready may see stale ">" from old pane content
    //    and return before the new agent has even launched.
    //    Includes `node` here so Gemini/Cursor (Node/Ink TUIs) are detected immediately.
    for _ in 0..10 {
        // 10s max
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Some(cmd) = tmux_ops.pane_current_command(target) {
            let process_started =
                AGENT_COMMANDS.iter().any(|a| cmd.contains(a)) || cmd.contains("node");
            if process_started {
                break;
            }
        }
    }
}

/// Wait for an agent in a tmux pane to be ready for input.
/// Handles the bypass warning prompt (sends acceptance) during the wait.
/// Always returns Some — the prompt is always sent (better late than never).
/// Number of consecutive stable polls (1s each) before considering the agent ready.
/// 3s of no pane content changes = agent has finished loading its TUI.
const CONTENT_STABLE_THRESHOLD: u32 = 3;

fn wait_for_agent_ready(tmux_ops: &Arc<dyn TmuxOperations>, target: &str) -> Option<String> {
    // Step 1: detect the ready signal (up to 30s).
    // Three detection methods, whichever fires first:
    //   1. Agent process detected via pane_current_command (Claude, Codex, Copilot)
    //   2. Known ready indicator in pane content (Gemini's "Type your message")
    //   3. Content stabilization: pane unchanged for 3s after >=3 changes (universal fallback)
    let mut last_content = String::new();
    let mut stable_ticks: u32 = 0;
    let mut change_count: u32 = 0;

    for _ in 0..30 {
        // 30s (30 * 1s)
        std::thread::sleep(std::time::Duration::from_secs(1));

        // Check 1: agent process detected via pane_current_command
        if !is_pane_at_shell(tmux_ops.as_ref(), target) {
            break;
        }

        // Check 2 & 3: pane content checks
        if let Ok(content) = tmux_ops.capture_pane(target) {
            // Handle Claude bypass prompt immediately
            if content.contains("Yes, I accept") || content.contains("I accept the risk") {
                let _ = tmux_ops.send_keys_literal(target, "2");
                std::thread::sleep(std::time::Duration::from_millis(50));
                let _ = tmux_ops.send_keys_literal(target, "Enter");
                // Fall through to settle wait below
                break;
            }

            // Check 2: known ready indicator in pane content
            if AGENT_ACTIVE_INDICATORS.iter().any(|s| content.contains(s)) {
                break;
            }

            // Check 3: content stabilization (unchanged for 3s)
            // Only count after content has changed multiple times (>=3), so we
            // don't false-positive on shell init output (e.g. asdf notice prints
            // once, then shell pauses while loading profiles/launching agent).
            if content != last_content {
                change_count += 1;
                stable_ticks = 0;
                last_content = content;
            } else if change_count >= 3 {
                stable_ticks += 1;
                if stable_ticks >= CONTENT_STABLE_THRESHOLD {
                    return Some(target.to_string());
                }
            }
        }
    }

    // Step 2: ready signal detected — wait for pane content to stop changing (up to 30s).
    // Needed for Node/Ink agents (Gemini, Cursor) where the process starts before the
    // TUI has finished rendering. Avoids sending the prompt into a half-drawn screen.
    let mut last_content = String::new();
    let mut stable_ticks: u32 = 0;
    for _ in 0..30 {
        // 30s hard timeout
        std::thread::sleep(std::time::Duration::from_secs(1));
        if let Ok(content) = tmux_ops.capture_pane(target) {
            if content != last_content {
                stable_ticks = 0;
                last_content = content;
            } else {
                stable_ticks += 1;
                if stable_ticks >= CONTENT_STABLE_THRESHOLD {
                    break;
                }
            }
        }
    }

    Some(target.to_string())
}

/// Load the workflow plugin for a task, checking agent compatibility.
/// Tries disk first (project-local → global), then falls back to bundled plugins.
fn load_task_plugin(
    task: &Task,
    project_path: Option<&Path>,
    default_agent: &str,
) -> Option<WorkflowPlugin> {
    let plugin = match &task.plugin {
        Some(name) => WorkflowPlugin::load(name, project_path)
            .ok()
            .or_else(|| skills::load_bundled_plugin(name)),
        None => skills::load_bundled_plugin("agtx"),
    };
    if let Some(ref p) = plugin {
        if !p.supports_agent(default_agent) {
            return None;
        }
    }
    plugin
}

/// Load workflow plugin if configured
fn load_plugin_if_configured(
    config: &MergedConfig,
    project_path: Option<&Path>,
) -> Option<WorkflowPlugin> {
    // For bundled plugins, always write the latest version to disk so updates ship with new releases
    if let (Some(name), Some(pp)) = (config.workflow_plugin.as_ref(), project_path) {
        if let Some((_name, _desc, content)) = skills::BUNDLED_PLUGINS
            .iter()
            .find(|(n, _, _)| *n == name.as_str())
        {
            let plugin_dir = pp.join(".agtx").join("plugins").join(name.as_str());
            let _ = std::fs::create_dir_all(&plugin_dir);
            let _ = std::fs::write(plugin_dir.join("plugin.toml"), content);
        }
    }
    config
        .workflow_plugin
        .as_ref()
        .and_then(|name| WorkflowPlugin::load(name, project_path).ok())
        .or_else(|| skills::load_bundled_plugin("agtx"))
}

/// Write skill files to a worktree's .agtx/skills/ directory and agent-native discovery paths.
/// `agent_names` determines which native paths to use (e.g. `.claude/commands/agtx/` for Claude).
/// When multiple agents are configured for different phases, skills are deployed for all of them.
fn write_skills_to_worktree(
    worktree_path: &str,
    project_path: &Path,
    plugin: &Option<WorkflowPlugin>,
    agent_names: &[&str],
) {
    let agtx_dir = Path::new(worktree_path).join(".agtx");
    let _ = std::fs::create_dir_all(&agtx_dir);

    // Write canonical .agtx/skills/ directory
    let skills_dir = agtx_dir.join("skills");
    if let Some(ref p) = plugin {
        // Copy skills from plugin directory, falling back to built-in defaults
        if let Some(plugin_dir) = WorkflowPlugin::plugin_dir(&p.name, Some(project_path)) {
            for (skill_name, default_content) in skills::BUILTIN_SKILLS {
                let src = plugin_dir.join(skill_name).join("SKILL.md");
                let dst_dir = skills_dir.join(skill_name);
                let _ = std::fs::create_dir_all(&dst_dir);
                if src.exists() {
                    let _ = std::fs::copy(&src, dst_dir.join("SKILL.md"));
                } else {
                    let _ = std::fs::write(dst_dir.join("SKILL.md"), default_content);
                }
            }
        } else {
            // Plugin dir not found, write defaults
            for (skill_name, skill_content) in skills::BUILTIN_SKILLS {
                let skill_dir = skills_dir.join(skill_name);
                let _ = std::fs::create_dir_all(&skill_dir);
                let _ = std::fs::write(skill_dir.join("SKILL.md"), skill_content);
            }
        }
    } else {
        // Write built-in default skills
        for (skill_name, skill_content) in skills::BUILTIN_SKILLS {
            let skill_dir = skills_dir.join(skill_name);
            let _ = std::fs::create_dir_all(&skill_dir);
            let _ = std::fs::write(skill_dir.join("SKILL.md"), skill_content);
        }
    }

    // Write to agent-native discovery paths (e.g. .claude/commands/agtx/)
    // Deploy for all configured agents so skills are available across phase transitions
    for agent_name in agent_names {
        if let Some((base_dir, namespace)) = skills::agent_native_skill_dir(agent_name) {
            let native_dir = if namespace.is_empty() {
                Path::new(worktree_path).join(base_dir)
            } else {
                Path::new(worktree_path).join(base_dir).join(namespace)
            };
            let _ = std::fs::create_dir_all(&native_dir);

            for (skill_dir_name, default_content) in skills::BUILTIN_SKILLS {
                let content =
                    resolve_skill_content(plugin, skill_dir_name, project_path, default_content);

                match *agent_name {
                    "gemini" => {
                        // Gemini uses .toml command files with description + prompt fields
                        let description = skills::extract_description(&content)
                            .unwrap_or_else(|| format!("agtx {} phase skill", skill_dir_name));
                        let toml_content = skills::skill_to_gemini_toml(&description, &content);
                        let filename = skills::skill_dir_to_filename(skill_dir_name, agent_name);
                        let _ = std::fs::write(native_dir.join(&filename), toml_content);
                    }
                    "codex" | "cursor" => {
                        // Codex/Cursor use SKILL.md in skill-name/ subdirectories
                        let skill_subdir = native_dir.join(skill_dir_name);
                        let _ = std::fs::create_dir_all(&skill_subdir);
                        let _ = std::fs::write(skill_subdir.join("SKILL.md"), &content);
                    }
                    "opencode" => {
                        // OpenCode uses flat .md command files: .opencode/command/agtx-research.md
                        // Commands have description frontmatter + prompt template
                        let oc_content = transform_skill_for_opencode(&content);
                        let filename = skills::skill_dir_to_filename(skill_dir_name, agent_name);
                        let _ = std::fs::write(native_dir.join(&filename), oc_content);
                    }
                    _ => {
                        // Claude and others: .md files with transformed frontmatter
                        let content = transform_skill_frontmatter(&content);
                        let filename = skills::skill_dir_to_filename(skill_dir_name, agent_name);
                        let _ = std::fs::write(native_dir.join(&filename), content);
                    }
                }
            }
        }
    }
}

/// Deploy a single skill to a target directory for the given agent.
/// Writes both the canonical `.agtx/skills/` copy and the agent-native discovery path.
fn deploy_skill(target_dir: &Path, skill_name: &str, content: &str, agent_name: &str) {
    // Write canonical copy
    let canonical_dir = target_dir.join(".agtx/skills").join(skill_name);
    let _ = std::fs::create_dir_all(&canonical_dir);
    let _ = std::fs::write(canonical_dir.join("SKILL.md"), content);

    // Write to agent-native discovery path
    if let Some((base_dir, namespace)) = skills::agent_native_skill_dir(agent_name) {
        let native_dir = if namespace.is_empty() {
            target_dir.join(base_dir)
        } else {
            target_dir.join(base_dir).join(namespace)
        };
        let _ = std::fs::create_dir_all(&native_dir);

        match agent_name {
            "claude" | "copilot" => {
                let transformed = transform_skill_frontmatter(content);
                let filename = skills::skill_dir_to_filename(skill_name, agent_name);
                let _ = std::fs::write(native_dir.join(&filename), transformed);
            }
            "gemini" => {
                let description = skills::extract_description(content)
                    .unwrap_or_else(|| format!("agtx {} skill", skill_name));
                let toml_content = skills::skill_to_gemini_toml(&description, content);
                let filename = skills::skill_dir_to_filename(skill_name, agent_name);
                let _ = std::fs::write(native_dir.join(&filename), toml_content);
            }
            "codex" | "cursor" => {
                let skill_subdir = native_dir.join(skill_name);
                let _ = std::fs::create_dir_all(&skill_subdir);
                let _ = std::fs::write(skill_subdir.join("SKILL.md"), content);
            }
            "opencode" => {
                let oc_content = transform_skill_for_opencode(content);
                let filename = skills::skill_dir_to_filename(skill_name, agent_name);
                let _ = std::fs::write(native_dir.join(&filename), oc_content);
            }
            _ => {}
        }
    }
}

/// Transform YAML frontmatter `name: agtx-plan` → `name: agtx:plan` for agent commands
fn transform_skill_frontmatter(content: &str) -> String {
    if let Some(start) = content.find("name: agtx-") {
        let after_name = &content[start + 6..]; // after "name: "
        if let Some(newline) = after_name.find('\n') {
            let old_name = after_name[..newline].trim();
            let new_name = skills::skill_name_to_command(old_name);
            return content.replacen(
                &format!("name: {}", old_name),
                &format!("name: {}", new_name),
                1,
            );
        }
    }
    content.to_string()
}

/// Transform skill content for OpenCode: strip frontmatter, keep as .md
/// OpenCode uses flat command files and hyphen-separated names (no colon namespace)
fn transform_skill_for_opencode(content: &str) -> String {
    // OpenCode commands use description frontmatter + prompt body
    let description =
        skills::extract_description(content).unwrap_or_else(|| "agtx skill".to_string());
    let body = skills::strip_frontmatter(content);
    format!("---\ndescription: \"{}\"\n---\n{}", description, body)
}

/// Resolve skill content: check plugin override, then fall back to default
fn resolve_skill_content(
    plugin: &Option<WorkflowPlugin>,
    skill_name: &str,
    project_path: &Path,
    default: &str,
) -> String {
    if let Some(ref p) = plugin {
        if let Some(plugin_dir) = WorkflowPlugin::plugin_dir(&p.name, Some(project_path)) {
            let src = plugin_dir.join(skill_name).join("SKILL.md");
            if src.exists() {
                if let Ok(content) = std::fs::read_to_string(&src) {
                    return content;
                }
            }
        }
    }
    default.to_string()
}

#[cfg(test)]
#[path = "app_tests.rs"]
mod tests;
