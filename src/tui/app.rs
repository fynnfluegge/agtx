use anyhow::Result;
use crossterm::{
    event::{DisableBracketedPaste, EnableBracketedPaste, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{prelude::*, widgets::*};
use std::cell::Cell;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Stdout};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Arc, Condvar, Mutex,
};
use std::time::Instant;

use crate::agent::hook_status::{self, HookState};
use crate::agent::{self, AgentOperations};
use crate::config::{GlobalConfig, MergedConfig, ProjectConfig, ThemeConfig, WorkflowPlugin};
use crate::db::{Database, PhaseStatus, Task, TaskStatus, TransitionRequest};
use crate::git::{
    self, GitOperations, GitProviderOperations, PullRequestState, RealGitHubOps, RealGitOps,
};
use crate::skills;
use crate::tmux::{
    self, InputConfig, InputError, PaneInput, PaneInputSink, RealTmuxOps, TmuxOperations,
};
use crate::AppMode;

use super::board::BoardState;
use super::config_editor::{ConfigEditor, EditorAction, FieldKind};
use super::help;
use super::shell_popup::{self, ShellPopup};
use super::text_input::TextInput;
use super::theme::TuiStyles;
use super::wizard::{PickOption, WizardState, WizardStep};

/// Helper to convert hex color string to ratatui Color
fn hex_to_color(hex: &str) -> Color {
    ThemeConfig::parse_hex(hex)
        .map(|(r, g, b)| Color::Rgb(r, g, b))
        .unwrap_or(Color::White)
}

/// Build footer help text based on current UI state
fn build_footer_text(
    wizard_step: Option<WizardStep>,
    sidebar_focused: bool,
    selected_column: usize,
    has_cyclic_plugin: bool,
    fullscreen_on_enter: bool,
) -> String {
    // Only the column-specific actions plus the two universal ones; the `?`
    // overlay carries the full list. One line has to survive a narrow terminal,
    // and a truncated footer silently hides whatever comes last.
    match wizard_step {
        None => {
            if sidebar_focused {
                "[j/k] navigate  [Enter] board  ·  [e] hide  [?] help  [q] quit".to_string()
            } else {
                let fullscreen = if fullscreen_on_enter {
                    ""
                } else {
                    "  [C-f] fullscreen"
                };
                match selected_column {
                    0 => "[o] new  [Enter] edit  [d] diff  ·  [m] plan  [M] run  [R] research  ·  [?] help  [q] quit".to_string(),
                    1 => format!("[o] new  [Enter] open{fullscreen}  [d] diff  ·  [m] run  ·  [?] help  [q] quit"),
                    2 => format!("[o] new  [Enter] open{fullscreen}  [d] diff  ·  [r] back  [m] move  ·  [?] help  [q] quit"),
                    3 if has_cyclic_plugin => format!(
                        "[o] new  [Enter] open{fullscreen}  ·  [r] resume  [p] next phase  [m] done  ·  [?] help  [q] quit"
                    ),
                    3 => format!("[o] new  [Enter] open{fullscreen}  [d] diff  ·  [r] back  [m] move  ·  [?] help  [q] quit"),
                    _ => "[o] new  [Enter] open  [x] delete  ·  [?] help  [q] quit".to_string(),
                }
            }
        }
        // Every step advertises how to leave it in both directions. `Esc` is
        // the back key now, and only cancels from the first step — which is
        // why the first step says "cancel" and the others say "back".
        Some(WizardStep::Title) => {
            " Enter task title...  [Enter] next  [C-s] save  [Esc] cancel ".to_string()
        }
        Some(WizardStep::Agent) | Some(WizardStep::Plugin) => {
            " [j/k] select  [/] filter  [Enter] next  [S-Tab] back  [C-s] save  [Esc] cancel "
                .to_string()
        }
        Some(WizardStep::Prompt) => {
            " [#] files  [/] skills  [!] tasks  ·  [\\+Enter] newline  [S-Tab] back  [Enter] save  [Esc] cancel "
                .to_string()
        }
    }
}

/// Turn the existing contextual help string into modern keycap/value spans.
/// Keeping `build_footer_text` as the source preserves its tested behavior.
fn styled_footer(text: &str, styles: TuiStyles) -> Line<'static> {
    let mut spans = Vec::new();
    let mut rest = text.trim();
    while let Some(open) = rest.find('[') {
        if open > 0 {
            spans.push(Span::styled(rest[..open].to_string(), styles.muted()));
        }
        let Some(close) = rest[open..].find(']') else {
            spans.push(Span::styled(rest[open..].to_string(), styles.muted()));
            rest = "";
            break;
        };
        let end = open + close + 1;
        spans.push(Span::styled(rest[open..end].to_string(), styles.keycap()));
        rest = &rest[end..];
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), styles.muted()));
    }
    Line::from(spans)
}

fn visible_column_range(selected: usize, width: u16) -> std::ops::Range<usize> {
    let visible = if width >= 140 {
        5
    } else if width >= 96 {
        3
    } else {
        2
    };
    let start = selected
        .saturating_sub(visible / 2)
        .min(5usize.saturating_sub(visible));
    start..start + visible
}

/// Terminal cells are typically about twice as tall as they are wide, so a
/// card needs roughly half as many rows as columns to look visually square.
/// Bounds keep narrow cards usable and very wide cards from dominating the board.
fn card_height_for_width(card_width: u16) -> u16 {
    (card_width / 2).clamp(6, 12)
}

fn board_scrollbar_metrics(
    total_items: usize,
    visible_items: usize,
    scroll_offset: usize,
    track_height: usize,
) -> Option<(usize, usize)> {
    if track_height == 0 || total_items <= visible_items || visible_items == 0 {
        return None;
    }

    let min_thumb_height = track_height.min(2);
    let thumb_height = (visible_items * track_height / total_items)
        .max(min_thumb_height)
        .min(track_height);
    let max_thumb_pos = track_height.saturating_sub(thumb_height);
    let max_scroll_offset = total_items.saturating_sub(visible_items);
    let thumb_pos = scroll_offset.min(max_scroll_offset) * max_thumb_pos / max_scroll_offset;

    Some((thumb_pos, thumb_height))
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
/// Horizontal breathing room inside the task wizard, in cells per side.
///
/// Without it a wrapped prompt line starts hard against the left border and
/// every line ends hard against the right one. The literals below therefore
/// carry no leading spaces of their own — the padding is the only indent, so
/// wrapped continuation rows line up with the row they came from.
const WIZARD_PADDING: u16 = 2;

/// Longest task title the wizard accepts.
///
/// Not a database limit — the board draws a title on one card line, and a title
/// long enough to be truncated everywhere it appears is not a useful name.
const MAX_TASK_TITLE_CHARS: usize = 120;

const SHELL_POPUP_WIDTH: u16 = 128; // Total width including borders
const SHELL_POPUP_CONTENT_WIDTH: u16 = 126; // Content width (SHELL_POPUP_WIDTH - 2 for borders)
const SHELL_POPUP_HEIGHT_PERCENT: u16 = 75; // Percentage of terminal height

/// Application state (separate from terminal for borrow checker)
struct AppState {
    mode: AppMode,
    flags: crate::FeatureFlags,
    should_quit: bool,
    board: BoardState,
    /// The task creation / edit wizard, open or not. `Some` *is* the input
    /// mode: there is no second flag that could disagree with it.
    wizard: Option<WizardState>,
    /// The config editor, open or not. See `tui::config_editor`.
    config_editor: Option<ConfigEditor>,
    /// The `?` overlay's scroll offset, when it is open. See `tui::help`.
    help_scroll: Option<usize>,
    /// The furthest the overlay can actually scroll, recorded by the renderer
    /// because only it knows how many rows fit.
    ///
    /// Without it the handler clamps to the *table* length instead, so `C-g`
    /// parks the offset far past the last screenful and the next `C-u` moves
    /// nothing — which reads exactly like a broken scroll.
    help_max_scroll: Cell<usize>,
    /// The agent the open wizard's plugin list was filtered by, so a change on
    /// the agent step can rebuild it and nothing else has to.
    plugins_filtered_for: Option<String>,
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
    // Popup keystrokes go here, never straight to tmux: enqueueing is the only
    // thing the input thread is allowed to do with a key. See `tmux::input`.
    input_sink: Arc<dyn PaneInputSink>,
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
    // Trust-on-first-use confirmation popup
    trust_confirm_popup: Option<TrustConfirmPopup>,
    // Channel for receiving background worktree setup results
    setup_rx: Option<mpsc::Receiver<SetupResult>>,
    // Phase detection
    phase_status_cache: HashMap<String, (PhaseStatus, Instant)>,
    /// Paces the MCP transition-queue read, which is a SQLite query.
    last_transition_poll: Instant,
    /// Raised by the control connection when tmux reports a window closing.
    window_events: Arc<AtomicBool>,
    // Idle detection: (content_hash, last_change_time) per task
    pane_content_hashes: HashMap<String, (u64, Instant)>,
    /// Why a task is Blocked, as reported by its agent's hook payload.
    blocked_reasons: HashMap<String, String>,
    /// Tasks whose agent is parked on a trust / permission prompt agtx declined to
    /// answer. Kept apart from `blocked_reasons` because the orchestrator must
    /// treat these differently: a nudge would be typed straight into the dialog.
    trust_blocked: HashSet<String>,
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
    // Release check: spawned once at startup, consumed non-blocking like the
    // session refresh above. `None` in the receiver means "no newer release" —
    // every failure (offline, no curl, rate limit) arrives that way too, so a
    // missing notice is the only symptom a broken check can have.
    update_rx: Option<mpsc::Receiver<Option<crate::update::UpdateInfo>>>,
    update_available: Option<crate::update::UpdateInfo>,
    // Update popup ([u]) and the background install it can start
    update_popup: Option<UpdatePopup>,
    update_install_rx: Option<mpsc::Receiver<Result<String, String>>>,
    // Cache of dependency satisfaction per task ID (refreshed with tasks)
    deps_satisfied_cache: HashMap<String, bool>,
    // Full-screen dependency-graph overlay (Shift+D)
    dep_graph_popup: Option<DepGraphPopup>,
    // Queue of task IDs awaiting serialized worktree setup (batch-move from the
    // dependency view). Worktree setup runs one-at-a-time via `setup_rx`; this
    // queue is drained as each setup completes.
    setup_queue: VecDeque<String>,
    instance_id: String,
}

/// State for the dependency-graph overlay.
struct DepGraphPopup {
    graph: crate::tui::dep_graph::DepGraph,
    /// Cursor index into `graph.nodes`.
    selected: usize,
    /// Task IDs marked for batch-move (only unblocked nodes are markable).
    marked: HashSet<String>,
    /// Horizontal scroll offset, in levels (columns), for wide graphs. Owned and
    /// corrected by the draw pass each frame so the selected node stays on
    /// screen; the key handler only moves `selected` and lets render re-clamp.
    scroll_levels: Cell<usize>,
    /// Number of level-columns that fit on screen, recorded by the last draw.
    /// Used only for the footer hint. Starts at 1, corrected on first render.
    visible_levels: Cell<usize>,
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

/// The card / notification text for a task parked on a security prompt.
///
/// Names the agent and the action, because the fix is not something agtx can do:
/// the user answers in the agent's own pane, or trusts the project once and every
/// later worktree inherits it.
fn trust_blocked_reason(agent: &str, dialog: &str) -> String {
    format!(
        "{agent} is waiting on a trust prompt (\"{dialog}\") — answer it in the pane, \
         or run {agent} once in the project root and confirm there"
    )
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
    /// The agent's own report of its state, when it writes one.
    hook_status: Option<hook_status::AgentHookStatus>,
    /// Set when the pane is showing a trust / permission prompt that agtx is
    /// deliberately not answering (`auto_trust = false`). The task is waiting on a
    /// person, not on the agent, so it reads as `Blocked` rather than `Working`.
    awaiting_trust: Option<String>,
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
    start_pos: usize,   // Position in the input buffer where trigger was typed
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

/// The list behaviour shared by the three prompt dropdowns.
///
/// All of them are a match list with a cursor, and all of them are driven by
/// the same four keys — the only thing that differs is what a match *is*.
trait SearchList {
    fn match_count(&self) -> usize;
    fn selected_index(&mut self) -> &mut usize;

    fn select_prev(&mut self) {
        let selected = self.selected_index();
        *selected = selected.saturating_sub(1);
    }

    fn select_next(&mut self) {
        let last = self.match_count().saturating_sub(1);
        let selected = self.selected_index();
        if *selected < last {
            *selected += 1;
        }
    }
}

impl SearchList for FileSearchState {
    fn match_count(&self) -> usize {
        self.matches.len()
    }
    fn selected_index(&mut self) -> &mut usize {
        &mut self.selected
    }
}

impl SearchList for SkillSearchState {
    fn match_count(&self) -> usize {
        self.matches.len()
    }
    fn selected_index(&mut self) -> &mut usize {
        &mut self.selected
    }
}

impl SearchList for TaskRefSearchState {
    fn match_count(&self) -> usize {
        self.matches.len()
    }
    fn selected_index(&mut self) -> &mut usize {
        &mut self.selected
    }
}

/// State for delete confirmation popup
#[derive(Debug, Clone)]
struct DeleteConfirmPopup {
    task_id: String,
    task_title: String,
}

/// State for trust-on-first-use confirmation popup
#[derive(Debug, Clone)]
struct TrustConfirmPopup {
    project_path: std::path::PathBuf,
    /// The suppressed fields, verbatim.
    ///
    /// `App::new` strips these before anything can run them, so the popup
    /// re-reads the file to show them: consenting to a script you cannot see is
    /// not consent. Each entry is (field name, value).
    dangerous: Vec<(&'static str, String)>,
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
    options: Vec<PickOption>,
}

/// The `[u]` popup: what is available, and one key to install it.
#[derive(Debug, Clone)]
struct UpdatePopup {
    info: crate::update::UpdateInfo,
    /// The last line `install_release` reported, or the final outcome. The
    /// install runs on a background thread; this is the only thing the draw
    /// path reads.
    status: Option<String>,
    installing: bool,
}

impl AppState {
    /// Which wizard step is open, if any. The one place anything outside the
    /// wizard asks about input mode.
    fn wizard_step(&self) -> Option<WizardStep> {
        self.wizard.as_ref().map(|w| w.step())
    }
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
        execute!(stdout, EnterAlternateScreen, EnableBracketedPaste)?;
        // No Kitty-keyboard-protocol push here, deliberately. It was added so
        // `Shift+Enter` could be told apart from `Enter` — both are a bare CR
        // otherwise — but `supports_keyboard_enhancement()` **blocks for up to
        // two seconds** on any terminal that does not answer the query, which
        // is a real cost on every launch for a chord that then still did not
        // arrive through tmux. `\`+Enter and `C-j` need no negotiation.
        let backend = CrosstermBackend::new(stdout);
        let terminal = ratatui::Terminal::new(AppBackend::Crossterm(backend))?;

        // Load configs
        let global_config = GlobalConfig::load().unwrap_or_default();
        let global_db = Database::open_global()?;

        // Detect available agents
        let available_agents = agent::detect_available_agents();

        // Setup based on mode
        let (db, project_path, project_name, tmux_project_name, project_config, trust_warning) =
            match &mode {
                AppMode::Dashboard => (
                    None,
                    None,
                    "Dashboard".to_string(),
                    tmux::safe_session_name("Dashboard"),
                    ProjectConfig::default(),
                    None,
                ),
                AppMode::Project(path) => {
                    let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
                    let name = canonical
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let tmux_name = tmux::safe_session_name(&name);
                    let mut project_config = ProjectConfig::load(&canonical).unwrap_or_default();
                    let db = Database::open_project(&canonical)?;

                    // Trust-on-first-use: suppress dangerous config fields from untrusted projects
                    let trust_store = crate::config::TrustStore::load().unwrap_or_default();
                    let trust_warning = if !trust_store.is_trusted(&canonical) {
                        if project_config.init_script.is_some()
                            || project_config.copy_files.is_some()
                            || project_config.cleanup_script.is_some()
                        {
                            tracing::warn!(
                                project = %canonical.display(),
                                "Untrusted project config — init_script, cleanup_script, and copy_files suppressed"
                            );
                            project_config.init_script = None;
                            project_config.cleanup_script = None;
                            project_config.copy_files = None;
                            Some("Untrusted project config: init_script, cleanup_script, and copy_files disabled. Run `agtx trust` to enable.".to_string())
                        } else {
                            None
                        }
                    } else {
                        None
                    };

                    // Register project in global database
                    let project = crate::db::Project::new(&name, canonical.to_string_lossy());
                    global_db.upsert_project(&project)?;

                    // Ensure tmux session exists for this project
                    ensure_project_tmux_session(&tmux_name, &canonical, tmux_ops.as_ref());

                    (
                        Some(db),
                        Some(canonical),
                        name,
                        tmux_name,
                        project_config,
                        trust_warning,
                    )
                }
            };

        let config = MergedConfig::merge(&global_config, &project_config);

        // One broker for the process. Popup keys are enqueued onto it, so the
        // input thread never waits for a tmux subprocess. Whether the broker
        // then writes through a control connection or one process per key is a
        // runtime decision it makes for itself: this only says not to try the
        // control connection at all.
        let mut input_config = InputConfig::new(tmux::AGENT_SERVER, &tmux_project_name);
        input_config.control_mode = control_mode_enabled();
        // tmux reports a window closing on this same connection, even with
        // `no-output`. That is how an exited agent becomes an `Exited` card
        // promptly instead of on the status refresh's next tick.
        let window_events = Arc::new(AtomicBool::new(false));
        input_config.window_events = Some(Arc::clone(&window_events));
        let input_sink = tmux::input::spawn(input_config, Arc::clone(&tmux_ops));

        // If the project is untrusted, also suppress plugin init_scripts
        // by forcing no_init_scripts in the flags
        let mut flags = flags;
        if trust_warning.is_some() {
            flags.no_init_scripts = true;
        }

        let mut app = Self {
            terminal,
            state: AppState {
                mode,
                flags,
                should_quit: false,
                board: BoardState::new(),
                wizard: None,
                config_editor: None,
                help_scroll: None,
                help_max_scroll: Cell::new(0),
                plugins_filtered_for: None,
                db,
                global_db,
                config,
                project_path,
                project_name: project_name.clone(),
                tmux_project_name: tmux_project_name.clone(),
                available_agents,
                tmux_ops,
                input_sink,
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
                trust_confirm_popup: None,
                phase_status_cache: HashMap::new(),
                last_transition_poll: Instant::now(),
                window_events,
                pane_content_hashes: HashMap::new(),
                blocked_reasons: HashMap::new(),
                trust_blocked: HashSet::new(),
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
                update_rx: None,
                update_available: None,
                update_popup: None,
                update_install_rx: None,
                deps_satisfied_cache: HashMap::new(),
                dep_graph_popup: None,
                setup_queue: VecDeque::new(),
                instance_id: uuid::Uuid::new_v4().to_string(),
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

        // Re-deploy agent configs for worktrees set up by a different agtx binary.
        if let Some(ref project_path) = app.state.project_path {
            let candidates: Vec<(String, Option<String>)> = app
                .state
                .board
                .tasks
                .iter()
                .filter(|t| t.status != TaskStatus::Done)
                .filter_map(|t| t.worktree_path.clone().map(|wt| (wt, t.plugin.clone())))
                .collect();
            if !candidates.is_empty() {
                refresh_stale_worktree_configs(
                    candidates,
                    project_path.clone(),
                    collect_phase_agents(&app.state.config),
                    app.state.config.agent_hooks,
                );
            }
        }

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
                    app.state.project_path.as_deref().unwrap_or(Path::new(".")),
                    app.state.tmux_ops.as_ref(),
                    agent_ops.as_ref(),
                );
            }
        }

        if let Some(orch_target) = detect_existing_orchestrator(
            app.state.flags.experimental,
            app.state.tmux_ops.as_ref(),
            &app.state.tmux_project_name,
            app.state.db.as_ref(),
            &app.state.board.tasks,
            app.state.project_path.as_deref(),
        ) {
            app.state.orchestrator_session = Some(orch_target.clone());
            let tmux_ops = Arc::clone(&app.state.tmux_ops);
            let ready_flag = Arc::clone(&app.state.orchestrator_ready);
            let auto_trust = app.state.config.auto_trust;
            std::thread::spawn(move || {
                if wait_for_agent_ready(&tmux_ops, &orch_target, Some("claude"), auto_trust)
                    .is_some()
                {
                    ready_flag.store(true, Ordering::Release);
                }
            });
        }

        // Release check, on a background thread so a slow or hung network never
        // delays the first frame. Deliberately not in `new_for_test`: the test
        // suite must not make network calls.
        if crate::update::check::checks_enabled(app.state.config.update_check) {
            let (tx, rx) = mpsc::channel();
            app.state.update_rx = Some(rx);
            std::thread::spawn(move || {
                let _ = tx.send(crate::update::check_for_update().unwrap_or(None));
            });
        }

        app.open_first_run_editor();

        // Display trust confirmation popup if project config was suppressed
        if trust_warning.is_some() {
            if let Some(ref path) = app.state.project_path {
                // Re-read from disk: the merged config has already had these
                // stripped, which is the whole reason the popup exists.
                let on_disk = ProjectConfig::load(path).unwrap_or_default();
                app.state.trust_confirm_popup = Some(TrustConfirmPopup {
                    project_path: path.clone(),
                    dangerous: dangerous_fields(&on_disk),
                });
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
        Self::new_for_test_with_flags(
            project_path,
            tmux_ops,
            git_ops,
            git_provider_ops,
            agent_registry,
            crate::FeatureFlags::default(),
        )
    }

    #[cfg(feature = "test-mocks")]
    pub fn new_for_test_with_flags(
        project_path: Option<PathBuf>,
        tmux_ops: Arc<dyn TmuxOperations>,
        git_ops: Arc<dyn GitOperations>,
        git_provider_ops: Arc<dyn GitProviderOperations>,
        agent_registry: Arc<dyn agent::AgentRegistry>,
        flags: crate::FeatureFlags,
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

        let mut app = Self {
            terminal,
            state: AppState {
                mode,
                flags,
                should_quit: false,
                board: BoardState::new(),
                wizard: None,
                config_editor: None,
                help_scroll: None,
                help_max_scroll: Cell::new(0),
                plugins_filtered_for: None,
                db,
                global_db,
                config,
                project_path,
                project_name,
                tmux_project_name,
                available_agents: vec![],
                tmux_ops,
                // Tests assert what the UI *enqueued*; a broker thread would add
                // timing to something that is a pure translation.
                input_sink: Arc::new(crate::tmux::RecordingSink::new()),
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
                trust_confirm_popup: None,
                phase_status_cache: HashMap::new(),
                last_transition_poll: Instant::now(),
                window_events: Arc::new(AtomicBool::new(false)),
                pane_content_hashes: HashMap::new(),
                blocked_reasons: HashMap::new(),
                trust_blocked: HashSet::new(),
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
                update_rx: None,
                update_available: None,
                update_popup: None,
                update_install_rx: None,
                deps_satisfied_cache: HashMap::new(),
                dep_graph_popup: None,
                setup_queue: VecDeque::new(),
                instance_id: uuid::Uuid::new_v4().to_string(),
            },
        };
        // The same call the real constructor makes, so a test exercises the
        // first-run path rather than a copy of it.
        app.open_first_run_editor();
        Ok(app)
    }

    /// Swap in a recording sink so a test can assert what the UI enqueued.
    #[cfg(feature = "test-mocks")]
    pub fn set_input_sink(&mut self, sink: Arc<dyn PaneInputSink>) {
        self.state.input_sink = sink;
    }

    /// The event loop.
    ///
    /// It **blocks** on a channel that two threads feed — terminal input and
    /// pane captures — and draws only when something actually changed. The
    /// previous shape polled: `event::poll(interval)` woke the loop on a timer
    /// whether or not anything had happened, and every wake-up redrew the whole
    /// screen and re-parsed the pane capture through `parse_ansi_to_lines`. That
    /// made the poll interval a three-way trade between echo latency, DB
    /// polling rate and idle CPU, so tuning it for the first made the other two
    /// expensive.
    ///
    /// Splitting them makes each one answer to what it is actually for:
    ///
    /// - **latency** is the pane watcher's cadence, and it wakes the loop only
    ///   when the pane it captured differs from the last one it sent;
    /// - **housekeeping** — the DB queue, the session refresh, the spinner — runs
    ///   on its own tick, no faster than it needs to and no longer coupled to
    ///   how fast the user types;
    /// - **drawing** happens when state changed, so an idle board with an idle
    ///   popup costs one backstop frame a second.
    pub async fn run(&mut self) -> Result<()> {
        let (tx, rx) = mpsc::channel::<Wake>();
        spawn_terminal_reader(tx.clone());
        let watch = Arc::new(PaneWatch::default());
        spawn_pane_watcher(
            Arc::clone(&watch),
            tx,
            Arc::clone(&self.state.input_sink),
            Arc::clone(&self.state.tmux_ops),
        );

        // The first frame has nothing to be a change from.
        let mut dirty = true;
        let mut last_draw = Instant::now();
        let mut last_housekeeping = Instant::now() - HOUSEKEEPING_TICK;

        while !self.state.should_quit {
            // A missed `dirty` would leave a stale screen until the next
            // keystroke, which is a far worse failure than a wasted frame — so
            // the loop repaints once a second regardless. A backstop, not the
            // mechanism: one frame a second is nothing, and if it is ever what
            // makes the UI look right, something above it is wrong.
            if dirty || last_draw.elapsed() >= REDRAW_BACKSTOP {
                self.draw()?;
                dirty = false;
                last_draw = Instant::now();
            }

            // Point the watcher at whatever popup is open now. Done here rather
            // than at the three sites that open one and the several that close
            // one: this cannot be forgotten by a new call site.
            let (watch_target, watch_depth) = match self.state.shell_popup.as_ref() {
                Some(popup) => (
                    Some(popup.window_name.as_str()),
                    popup_capture_depth(popup.scroll_offset),
                ),
                None => (None, SHELL_POPUP_TAIL_LINES),
            };
            watch.follow(watch_target, watch_depth);

            match rx.recv_timeout(HOUSEKEEPING_TICK) {
                Ok(wake) => {
                    // Typing is the case the fast cadence exists for, so a key
                    // ends the watcher's wait rather than landing in the middle
                    // of a backed-off one.
                    if matches!(wake, Wake::Input(_)) {
                        watch.poke();
                    }
                    dirty |= self.handle_wake(wake)?;
                    // Drain what is already queued: a burst of keystrokes, or a
                    // key and the capture it caused, are one redraw, not four.
                    loop {
                        match rx.try_recv() {
                            Ok(wake) => dirty |= self.handle_wake(wake)?,
                            Err(mpsc::TryRecvError::Empty) => break,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                self.state.should_quit = true;
                                break;
                            }
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                // Both feeder threads are gone; without terminal input there is
                // no way left to quit.
                Err(mpsc::RecvTimeoutError::Disconnected) => self.state.should_quit = true,
            }

            dirty |= self.pump_background_results()?;

            if last_housekeeping.elapsed() >= HOUSEKEEPING_TICK {
                last_housekeeping = Instant::now();
                dirty |= self.run_housekeeping();
            }
        }

        watch.stop();
        Ok(())
    }

    /// Apply one wake-up. Returns whether the screen needs redrawing.
    fn handle_wake(&mut self, wake: Wake) -> Result<bool> {
        match wake {
            Wake::Input(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                self.handle_key(key)?;
                Ok(true)
            }
            Wake::Input(Event::Paste(text)) => {
                self.handle_paste(text)?;
                Ok(true)
            }
            Wake::Input(Event::Resize(..)) => Ok(true),
            Wake::Input(_) => Ok(false),
            Wake::Pane {
                window,
                content,
                lines,
                metrics,
            } => {
                let Some(popup) = self.state.shell_popup.as_mut() else {
                    return Ok(false);
                };
                // A capture for a popup that has since closed or switched panes
                // is not this popup's content.
                if popup.window_name != window {
                    return Ok(false);
                }
                // The watcher already suppresses unchanged captures; this also
                // covers the first one after a popup seeded its own content
                // synchronously at open time.
                if popup.cached_content == content && popup.metrics == metrics {
                    return Ok(false);
                }
                popup.set_content(content, lines);
                popup.metrics = metrics;
                Ok(true)
            }
        }
    }

    /// Collect anything the background threads have finished. Returns whether
    /// any of it changed what is on screen.
    fn pump_background_results(&mut self) -> Result<bool> {
        let mut changed = false;

        // Check for PR generation completion
        if let Some(ref rx) = self.state.pr_generation_rx {
            if let Ok((pr_title, pr_body)) = rx.try_recv() {
                if let Some(ref mut popup) = self.state.pr_confirm_popup {
                    popup.pr_title = pr_title;
                    popup.pr_body = pr_body;
                    popup.generating = false;
                }
                self.state.pr_generation_rx = None;
                changed = true;
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
                changed = true;
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
                // This setup finished; start the next queued batch task (if any).
                self.try_start_next_queued_setup()?;
                changed = true;
            }
        }

        // Apply results from background session refresh (non-blocking)
        if let Some(ref rx) = self.state.session_refresh_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.state.session_refresh_rx = None;
                    self.apply_session_refresh(result);
                    changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    // Thread panicked or dropped sender — clear to allow future spawns
                    self.state.session_refresh_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // Release check result (arrives once, then the channel is dropped)
        if let Some(ref rx) = self.state.update_rx {
            match rx.try_recv() {
                Ok(info) => {
                    self.state.update_rx = None;
                    self.state.update_available = info;
                    changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state.update_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        // Result of an install started from the update popup
        if let Some(ref rx) = self.state.update_install_rx {
            match rx.try_recv() {
                Ok(result) => {
                    self.state.update_install_rx = None;
                    if let Some(popup) = self.state.update_popup.as_mut() {
                        popup.installing = false;
                        popup.status = Some(match result {
                            Ok(msg) => msg,
                            Err(e) => format!("failed: {e}"),
                        });
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.state.update_install_rx = None;
                    if let Some(popup) = self.state.update_popup.as_mut() {
                        popup.installing = false;
                        popup.status = Some("failed: the update thread stopped".to_string());
                    }
                    changed = true;
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }

        Ok(changed)
    }

    /// Periodic work, on its own tick rather than once per wake-up.
    ///
    /// Every item here used to run on every loop iteration — including
    /// `process_transition_requests`, which queries SQLite — so its rate rose and
    /// fell with how fast the user happened to be typing. Returns whether
    /// anything on screen moved.
    fn run_housekeeping(&mut self) -> bool {
        let mut changed = false;
        let now = Instant::now();

        // Process MCP transition requests from the command queue, on their own
        // interval — this is a SQLite query, and it used to run on every tick.
        if now.duration_since(self.state.last_transition_poll) >= TRANSITION_POLL_INTERVAL {
            self.state.last_transition_poll = now;
            if let Err(e) = self.process_transition_requests() {
                tracing::warn!(error = %e, "failed to process transition requests");
            }
        }

        // tmux said a window closed. The refresh is what turns that into an
        // `Exited` card, and its per-task cache would otherwise hold the old
        // status for up to `PHASE_STATUS_CACHE_TTL` — so age the cache out and
        // let the refresh below run now.
        if self.state.window_events.swap(false, Ordering::Relaxed) {
            self.expire_phase_status_cache();
        }

        // Spawn background refresh if not already running and cache expired.
        self.maybe_spawn_session_refresh();

        // Nothing on the board animates any more: every indicator is static, so
        // an idle board asks for no frame at all between real changes.

        // Deliver queued notifications to orchestrator when idle
        self.deliver_orchestrator_notifications();

        // Clear expired warning messages
        if let Some((_, created)) = &self.state.warning_message {
            if created.elapsed() >= WARNING_MESSAGE_TTL {
                self.state.warning_message = None;
                changed = true;
            }
        }

        changed
    }

    /// Make every cached phase status look stale, so the next refresh re-checks
    /// every task rather than waiting out its per-task TTL.
    ///
    /// The timestamps are aged rather than the entries dropped: the previous
    /// status is what decides `was_ready`, and losing it would suppress the
    /// "newly ready" notification for a task that had just finished.
    fn expire_phase_status_cache(&mut self) {
        let Some(stale) = Instant::now().checked_sub(PHASE_STATUS_CACHE_TTL) else {
            return;
        };
        for (_, ts) in self.state.phase_status_cache.values_mut() {
            *ts = stale;
        }
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

        let styles = TuiStyles::from_theme(&state.config.theme);

        // Main layout: compact application bar, board, contextual command bar.
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2), // Header + divider
                Constraint::Min(0),    // Board
                Constraint::Length(2), // Divider + footer
            ])
            .split(content_area);

        // Header
        let plugin_label = state.config.workflow_plugin.as_deref().unwrap_or("agtx");
        let left = Span::styled(
            format!("  {}", state.project_name),
            Style::default().fg(styles.text).bold(),
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
        if let Some(ref update) = state.update_available {
            // color_selected, not a warning color: this is an affordance, not a
            // fault. `[u]` is the discovery point — the footer is already dense
            // enough that another hint there would be lost.
            right_spans.push(Span::styled(
                format!("⬆ {} ", update.latest),
                Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
            ));
            right_spans.push(Span::styled(
                "[u] ",
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
        let left_len = state.project_name.chars().count() + 2;
        // Character count, not byte count: the update notice's "⬆" is 3 bytes
        // and one column, and a byte-based width would over-pad the header.
        let right_len: usize = right_spans.iter().map(|s| s.content.chars().count()).sum();
        let padding = (chunks[0].width as usize).saturating_sub(left_len + right_len + 2);
        let mut spans = vec![left, Span::raw(" ".repeat(padding))];
        spans.extend(right_spans);
        frame.render_widget(
            Paragraph::new(Line::from(spans)),
            Rect {
                height: 1,
                ..chunks[0]
            },
        );
        frame.render_widget(
            Paragraph::new("─".repeat(chunks[0].width as usize)).style(styles.muted()),
            Rect {
                y: chunks[0].y + 1,
                height: 1,
                ..chunks[0]
            },
        );

        // Keep cards usable on narrow terminals by showing a window around the
        // selected column. Navigation still spans all five workflow columns.
        let visible_range = visible_column_range(state.board.selected_column, chunks[1].width);
        let mut constraints = Vec::new();
        for slot in 0..visible_range.len() {
            if slot > 0 {
                constraints.push(Constraint::Length(1));
            }
            constraints.push(Constraint::Ratio(1, visible_range.len() as u32));
        }
        let board_areas = Layout::horizontal(constraints).split(chunks[1]);

        let statuses = TaskStatus::columns();
        for (slot, i) in visible_range.enumerate() {
            let status = &statuses[i];
            let column_area = board_areas[slot * 2];
            let tasks: Vec<&Task> = state
                .board
                .tasks
                .iter()
                .filter(|t| t.status == *status)
                .collect();

            let is_selected_column = state.board.selected_column == i;

            let title_style = if is_selected_column {
                Style::default().fg(styles.selected).bold()
            } else {
                Style::default().fg(styles.column_header).bold()
            };

            let header_area = Rect {
                height: 2,
                ..column_area
            };
            let count = Span::styled(format!("  {}", tasks.len()), styles.muted());
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(
                        format!(" {}", status.display_name().to_uppercase()),
                        title_style,
                    ),
                    count,
                ])),
                Rect {
                    height: 1,
                    ..header_area
                },
            );
            let rule_color = if is_selected_column {
                styles.selected
            } else {
                styles.dimmed
            };
            frame.render_widget(
                Paragraph::new("─".repeat(header_area.width as usize))
                    .style(Style::default().fg(rule_color)),
                Rect {
                    y: header_area.y + 1,
                    height: 1,
                    ..header_area
                },
            );

            let inner_area = Rect {
                y: column_area.y + 2,
                height: column_area.height.saturating_sub(2),
                ..column_area
            };
            let card_height = card_height_for_width(inner_area.width);
            let max_visible_cards = (inner_area.height / card_height).max(1) as usize;

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

                let deps_blocked = state
                    .deps_satisfied_cache
                    .get(&task.id)
                    .map_or(false, |satisfied| !satisfied);
                Self::draw_task_card(
                    frame,
                    task,
                    card_area,
                    is_selected,
                    &state.config.theme,
                    state.phase_status_cache.get(&task.id),
                    deps_blocked,
                );
            }

            if tasks.is_empty() {
                let empty = if is_selected_column {
                    "  No tasks · [o] new"
                } else {
                    "  No tasks"
                };
                frame.render_widget(Paragraph::new(empty).style(styles.muted()), inner_area);
            }

            // Draw scrollbar if needed
            if needs_scrollbar {
                let scrollbar_area = Rect {
                    x: inner_area.x + inner_area.width - 1,
                    y: inner_area.y,
                    width: 1,
                    height: inner_area.height,
                };

                let scrollbar_height = inner_area.height as usize;
                if let Some((thumb_pos, thumb_height)) = board_scrollbar_metrics(
                    tasks.len(),
                    max_visible_cards,
                    scroll_offset,
                    scrollbar_height,
                ) {
                    for y in 0..scrollbar_height {
                        let is_thumb = y >= thumb_pos && y < thumb_pos + thumb_height;
                        let (glyph, style) = if is_thumb {
                            ("┃", Style::default().fg(styles.selected).bold())
                        } else {
                            ("│", styles.muted())
                        };
                        frame.render_widget(
                            Paragraph::new(glyph).style(style),
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
                        state.wizard_step(),
                        state.sidebar_focused,
                        state.board.selected_column,
                        has_cyclic_plugin,
                        state.config.fullscreen_on_enter,
                    ),
                    Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
                )
            }
        } else {
            (
                build_footer_text(
                    state.wizard_step(),
                    state.sidebar_focused,
                    state.board.selected_column,
                    has_cyclic_plugin,
                    state.config.fullscreen_on_enter,
                ),
                Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
            )
        };

        frame.render_widget(
            Paragraph::new("─".repeat(chunks[2].width as usize)).style(styles.muted()),
            Rect {
                height: 1,
                ..chunks[2]
            },
        );
        let footer_line = if footer_style.fg == Some(Color::Yellow) {
            Line::from(Span::styled(
                format!("  {}", footer_text.trim()),
                footer_style,
            ))
        } else {
            styled_footer(&footer_text, styles)
        };
        frame.render_widget(
            Paragraph::new(footer_line).alignment(Alignment::Center),
            Rect {
                y: chunks[2].y + 1,
                height: 1,
                ..chunks[2]
            },
        );

        // The task wizard overlay
        if let Some(ref wizard) = state.wizard {
            let input_area = centered_rect(60, 60, area);
            draw_wizard(state, wizard, frame, input_area);

            // Search dropdowns (only in InputDescription mode)
            if wizard.step() == WizardStep::Prompt {
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

        // Trust confirmation popup
        if let Some(ref popup) = state.trust_confirm_popup {
            // Sized to the content. At a fixed 30% height the answer line fell
            // off the bottom as soon as a project declared two fields — a
            // consent dialog that hides how to answer it.
            let path = popup.project_path.display().to_string();
            let widest = popup
                .dangerous
                .iter()
                .map(|(f, v)| f.len() + v.chars().count() + 5)
                .chain(std::iter::once(path.chars().count()))
                .max()
                .unwrap_or(60)
                .max(52);
            let width = (widest as u16 + 6).min(area.width.saturating_sub(4));
            // path + intro + blank + heading + fields + blank + answer, then
            // borders and the vertical margin.
            let rows = 4 + popup.dangerous.len().max(1) as u16 + 2;
            let height = (rows + 4).min(area.height.saturating_sub(2));
            let popup_area = Rect {
                x: area.x + area.width.saturating_sub(width) / 2,
                y: area.y + area.height.saturating_sub(height) / 2,
                width,
                height,
            };
            frame.render_widget(Clear, popup_area);

            let main_block = Block::default()
                .title(" Untrusted Project Config ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::Yellow));
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });
            // Left-aligned, and showing the actual values: this is a consent
            // dialog, and a centred summary of "dangerous fields" tells the
            // user nothing about what would run.
            let mut lines: Vec<Line<'static>> = vec![
                Line::from(Span::styled(
                    format!("{}", popup.project_path.display()),
                    Style::default().fg(Color::White).bold(),
                )),
                Line::from(Span::styled(
                    "ships an .agtx/config.toml agtx has not seen before.".to_string(),
                    Style::default().fg(Color::White),
                )),
                Line::from(String::new()),
            ];
            if popup.dangerous.is_empty() {
                lines.push(Line::from(Span::styled(
                    "It declares no scripts or file copies.".to_string(),
                    Style::default().fg(Color::White),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Trusting it lets agtx run:".to_string(),
                    Style::default().fg(Color::White),
                )));
                for (field, value) in &popup.dangerous {
                    lines.push(Line::from(vec![
                        Span::styled(
                            format!("  {field} = "),
                            Style::default().fg(hex_to_color(&state.config.theme.color_dimmed)),
                        ),
                        Span::styled(value.clone(), Style::default().fg(Color::Yellow)),
                    ]));
                }
            }
            lines.push(Line::from(String::new()));
            lines.push(Line::from(Span::styled(
                "[y] trust and enable    [n] leave disabled".to_string(),
                Style::default().fg(hex_to_color(&state.config.theme.color_selected)),
            )));

            let content = Paragraph::new(Text::from(lines))
                .style(Style::default().fg(Color::White))
                .wrap(Wrap { trim: false });
            frame.render_widget(content, inner);
        }

        // Update popup ([u])
        if let Some(ref popup) = state.update_popup {
            let popup_area = centered_rect(60, 30, area);
            frame.render_widget(Clear, popup_area);

            let main_block = Block::default()
                .title(" Update Available ")
                .borders(Borders::ALL)
                .border_style(
                    Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)),
                );
            frame.render_widget(main_block, popup_area);

            let inner = popup_area.inner(ratatui::layout::Margin {
                horizontal: 2,
                vertical: 1,
            });

            let selected = hex_to_color(&state.config.theme.color_selected);
            let dimmed = hex_to_color(&state.config.theme.color_dimmed);
            let mut lines: Vec<Line> = vec![
                Line::from(""),
                Line::from(vec![
                    Span::styled("  current ", Style::default().fg(dimmed)),
                    Span::styled(popup.info.current.to_string(), Style::default()),
                    Span::styled("  →  latest ", Style::default().fg(dimmed)),
                    Span::styled(
                        popup.info.latest.to_string(),
                        Style::default().fg(selected).add_modifier(Modifier::BOLD),
                    ),
                ]),
            ];
            if !popup.info.html_url.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("  {}", popup.info.html_url),
                    Style::default().fg(dimmed),
                )));
            }
            lines.push(Line::from(""));
            match (&popup.status, popup.installing) {
                (Some(status), _) => lines.push(Line::from(Span::styled(
                    format!("  {}", status),
                    Style::default().fg(selected),
                ))),
                (None, true) => lines.push(Line::from(Span::styled(
                    "  installing…",
                    Style::default().fg(selected),
                ))),
                (None, false) => lines.push(Line::from(Span::styled(
                    "  [Enter] install    [Esc] close",
                    Style::default().fg(dimmed),
                ))),
            }
            let content = Paragraph::new(lines).wrap(Wrap { trim: false });
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

        // The `?` overlay
        if let Some(scroll) = state.help_scroll {
            draw_help(state, scroll, frame, area);
        }

        // The config editor
        if let Some(ref editor) = state.config_editor {
            draw_config_editor(state, editor, frame, area);
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

        // Dependency-graph overlay
        if let Some(ref popup) = state.dep_graph_popup {
            Self::draw_dependency_graph(popup, frame, area, &state.config.theme);
        }
    }

    /// Render the dependency-graph overlay: topological columns of task cards,
    /// with unblocked Backlog tasks in green and marked tasks reversed.
    fn draw_dependency_graph(
        popup: &DepGraphPopup,
        frame: &mut Frame,
        area: Rect,
        theme: &ThemeConfig,
    ) {
        let popup_area = centered_rect(90, 90, area);
        frame.render_widget(Clear, popup_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1), // Title bar
                Constraint::Min(0),    // Columns
                Constraint::Length(1), // Footer
            ])
            .split(popup_area);

        // Title bar.
        let unblocked_count = popup.graph.nodes.iter().filter(|n| n.unblocked).count();
        let title = format!(
            " Dependency Graph — {} tasks, {} unblocked ",
            popup.graph.nodes.len(),
            unblocked_count
        );
        let title_bar = Paragraph::new(title).style(
            Style::default()
                .fg(Color::Black)
                .bg(hex_to_color(&theme.color_popup_header)),
        );
        frame.render_widget(title_bar, chunks[0]);

        let body = chunks[1];
        let level_count = popup.graph.level_count();
        if level_count == 0 {
            let empty = Paragraph::new("No tasks").block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(hex_to_color(&theme.color_popup_border))),
            );
            frame.render_widget(empty, body);
        } else {
            // Fit as many columns as possible at a fixed minimum width, starting
            // from the horizontal scroll offset.
            const COL_WIDTH: u16 = 26;
            let max_visible = (body.width / COL_WIDTH).max(1) as usize;
            // Record the viewport width so the key handler can keep the cursor in
            // view when scrolling horizontally.
            popup.visible_levels.set(max_visible);
            // Honor the stored offset, but always keep the selected node's level
            // on screen — covers the initial open (cursor may start in a far
            // level) and every navigation (the key handler just moves the cursor
            // and lets this clamp re-scroll).
            let sel_level = popup.graph.nodes.get(popup.selected).map_or(0, |n| n.level);
            let start = clamp_scroll_to_selected(
                popup.scroll_levels.get(),
                sel_level,
                max_visible,
                level_count,
            );
            // Persist the corrected offset so the footer hint stays in sync.
            popup.scroll_levels.set(start);
            let visible_cols = max_visible.min(level_count - start);
            let end = (start + visible_cols).min(level_count);

            let col_constraints: Vec<Constraint> = (start..end)
                .map(|_| Constraint::Ratio(1, (end - start) as u32))
                .collect();
            let col_areas = Layout::default()
                .direction(Direction::Horizontal)
                .constraints(col_constraints)
                .split(body);

            for (slot, level) in (start..end).enumerate() {
                let col_area = col_areas[slot];
                Self::draw_dep_level_column(popup, frame, col_area, level, theme);
            }
        }

        // Footer.
        let scroll_hint = if level_count > 0 {
            let scroll = popup.scroll_levels.get();
            let first = scroll + 1;
            let last = (scroll + popup.visible_levels.get()).min(level_count);
            format!(" cols {first}-{last}/{level_count} ")
        } else {
            String::new()
        };
        let footer_text = format!(
            " [hjkl] move  [Space] mark  [a] all unblocked  [c] clear  [Enter] move {} →research  [q] close {}",
            popup.marked.len(),
            scroll_hint
        );
        let footer = Paragraph::new(footer_text).style(
            Style::default()
                .fg(Color::Black)
                .bg(hex_to_color(&theme.color_dimmed)),
        );
        frame.render_widget(footer, chunks[2]);
    }

    /// Render a single topological column (level) of the dependency graph.
    fn draw_dep_level_column(
        popup: &DepGraphPopup,
        frame: &mut Frame,
        area: Rect,
        level: usize,
        theme: &ThemeConfig,
    ) {
        let Some(indices) = popup.graph.levels.get(level) else {
            return;
        };

        // Column header.
        let header = Paragraph::new(format!("Level {level}"))
            .style(
                Style::default()
                    .fg(hex_to_color(&theme.color_column_header))
                    .bold(),
            )
            .alignment(Alignment::Center);
        let header_area = Rect { height: 1, ..area };
        frame.render_widget(header, header_area);

        // Each card is 4 rows tall (border + title + status + hint).
        const CARD_HEIGHT: u16 = 4;
        let mut y = area.y + 1;
        for &idx in indices {
            if y + CARD_HEIGHT > area.y + area.height {
                break;
            }
            let Some(node) = popup.graph.nodes.get(idx) else {
                continue;
            };
            let card_area = Rect {
                x: area.x,
                y,
                width: area.width,
                height: CARD_HEIGHT,
            };
            let is_cursor = idx == popup.selected;
            let is_marked = popup.marked.contains(&node.task_id);

            // Choose the node color by status / unblocked state.
            let base_color = if node.unblocked {
                Color::Green
            } else if matches!(node.status, TaskStatus::Done) {
                hex_to_color(&theme.color_dimmed)
            } else if matches!(node.status, TaskStatus::Backlog) {
                // Blocked Backlog (deps not satisfied).
                hex_to_color(&theme.color_dimmed)
            } else {
                hex_to_color(&theme.color_normal)
            };

            let border_style = if is_cursor {
                Style::default().fg(hex_to_color(&theme.color_selected))
            } else {
                Style::default().fg(base_color)
            };
            let border_type = if is_cursor {
                BorderType::Thick
            } else {
                BorderType::Plain
            };

            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .border_type(border_type);
            let inner = block.inner(card_area);
            frame.render_widget(block, card_area);

            // Marker glyph: ✓ done, ⊘ blocked Backlog, ● unblocked, else space.
            let marker = if node.unblocked {
                "\u{25cf} " // ●
            } else if matches!(node.status, TaskStatus::Done) {
                "\u{2713} " // ✓
            } else if matches!(node.status, TaskStatus::Backlog) {
                "\u{2298} " // ⊘ blocked
            } else {
                "  "
            };

            let mut title_style = Style::default().fg(base_color);
            if is_marked {
                title_style = title_style.add_modifier(Modifier::REVERSED).bold();
            } else if is_cursor {
                title_style = title_style.bold();
            }

            // Title line (marker + truncated title).
            let title_text = truncate_str(&node.title, inner.width.saturating_sub(2) as usize);
            let title_line = Line::from(vec![
                Span::styled(marker, title_style),
                Span::styled(title_text, title_style),
            ]);
            // Status line.
            let status_line = Line::from(Span::styled(
                format!("[{}]", node.status.as_str()),
                Style::default().fg(hex_to_color(&theme.color_description)),
            ));
            // Dependency hint line.
            let hint = if node.dep_titles.is_empty() {
                String::new()
            } else {
                let joined = node.dep_titles.join(", ");
                format!(
                    "\u{2190} {}",
                    truncate_str(&joined, inner.width.saturating_sub(2) as usize)
                )
            };
            let hint_line = Line::from(Span::styled(
                hint,
                Style::default().fg(hex_to_color(&theme.color_dimmed)),
            ));

            let para = Paragraph::new(vec![title_line, status_line, hint_line]);
            frame.render_widget(para, inner);

            y += CARD_HEIGHT;
        }
    }

    fn draw_shell_popup(popup: &ShellPopup, frame: &mut Frame, area: Rect, theme: &ThemeConfig) {
        let popup_area = if popup.fullscreen {
            area
        } else {
            centered_rect_fixed_width(SHELL_POPUP_WIDTH, SHELL_POPUP_HEIGHT_PERCENT, area)
        };

        // Parse ANSI escape sequences for colors
        // Parsed by the watcher, and borrowed rather than cloned: only the rows
        // that fit on screen are copied, not the whole cached pane.
        let styled_lines = popup.cached_lines.as_slice();

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
        deps_blocked: bool,
    ) {
        let styles = TuiStyles::from_theme(theme);
        let border_style = if is_selected {
            Style::default().fg(styles.selected)
        } else {
            Style::default().fg(styles.dimmed)
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

        let card_block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .border_type(if is_selected {
                BorderType::Rounded
            } else {
                BorderType::Plain
            });
        let block_inner = card_block.inner(area);
        frame.render_widget(card_block, area);
        let inner = Rect {
            x: block_inner.x.saturating_add(1),
            width: block_inner.width.saturating_sub(2),
            ..block_inner
        };

        // Title line with optional phase indicator
        let show_indicator = matches!(
            task.status,
            TaskStatus::Planning | TaskStatus::Running | TaskStatus::Review
        ) || (task.status == TaskStatus::Backlog
            && task.session_name.is_some());

        if show_indicator {
            let indicator = match phase_status {
                Some((PhaseStatus::Ready, _)) => {
                    Span::styled("\u{2713} ", Style::default().fg(Color::Green))
                }
                // Static, not a spinner. An animated frame is a redraw of the
                // whole board, and on an otherwise idle board it was the *only*
                // thing forcing one — ten a second, forever, to rotate a glyph.
                // The card already says a task is running; the motion added
                // nothing that the icon does not.
                Some((PhaseStatus::Working, _)) => {
                    Span::styled("\u{25b6} ", Style::default().fg(Color::Yellow))
                }
                Some((PhaseStatus::Blocked, _)) => Span::styled(
                    "? ",
                    Style::default()
                        .fg(hex_to_color(&theme.color_accent))
                        .bold(),
                ),
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
        } else if deps_blocked {
            let lock_span = Span::styled(
                "\u{2298} ",
                Style::default().fg(hex_to_color(&theme.color_dimmed)),
            );
            let title_spans = Line::from(vec![lock_span, Span::styled(title, title_style)]);
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

        // Footer line with a compact phase label and agent badge.
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
            let preview_text = task.description.as_deref().unwrap_or("");

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
                .style(Style::default().fg(styles.description))
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
            // Pure white stays `Color::White` rather than becoming truecolor: it
            // is an ANSI palette entry, which a themed terminal renders as its
            // own white. Rgb(255,255,255) would override the user's theme.
            let to_color = |(r, g, b): (u8, u8, u8)| {
                if (r, g, b) == (255, 255, 255) {
                    Color::White
                } else {
                    Color::Rgb(r, g, b)
                }
            };
            let agent_style = match agent::spec(task.agent.as_str()) {
                Some(spec) => {
                    let style = Style::default().fg(to_color(spec.label_fg));
                    match spec.label_bg {
                        Some(bg) => style.bg(to_color(bg)),
                        None => style,
                    }
                }
                None => Style::default().fg(Color::White),
            };
            let phase_label = phase_status.map(|(status, _)| match status {
                PhaseStatus::Ready => "✓ Ready",
                PhaseStatus::Working => "● Working",
                PhaseStatus::Blocked => "? Blocked",
                PhaseStatus::Idle => "Ⅱ Idle",
                PhaseStatus::Exited => "× Exited",
            });
            let mut spans = Vec::new();
            if let Some(label) = phase_label {
                spans.push(Span::styled(label, styles.muted()));
            }
            let used: usize = spans.iter().map(|span| span.width()).sum();
            let agent = format!(" {} ", task.agent);
            let gap = (footer_area.width as usize).saturating_sub(used + agent.chars().count());
            spans.push(Span::raw(" ".repeat(gap)));
            spans.push(Span::styled(agent, agent_style));
            frame.render_widget(Paragraph::new(Line::from(spans)), footer_area);
        }
    }

    fn draw_sidebar(state: &AppState, frame: &mut Frame, area: Rect) {
        let styles = TuiStyles::from_theme(&state.config.theme);
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

                let marker = if is_current { "▌" } else { " " };
                ListItem::new(format!("{} {}", marker, project.name)).style(style)
            })
            .collect();

        let title_style = if state.sidebar_focused {
            Style::default().fg(styles.selected).bold()
        } else {
            Style::default().fg(styles.column_header).bold()
        };
        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" PROJECTS", title_style),
                Span::styled(format!("  {}", state.projects.len()), styles.muted()),
            ])),
            Rect { height: 1, ..area },
        );
        frame.render_widget(
            Paragraph::new("─".repeat(area.width.saturating_sub(1) as usize)).style(styles.muted()),
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
        );
        frame.render_widget(
            List::new(items),
            Rect {
                x: area.x + 1,
                y: area.y + 3,
                width: area.width.saturating_sub(2),
                height: area.height.saturating_sub(3),
            },
        );
        frame.render_widget(
            Block::default()
                .borders(Borders::RIGHT)
                .border_style(styles.muted()),
            area,
        );
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
        // A user in dashboard mode is between projects, which is the best
        // moment there is to be told. The logo block has exactly one spare row.
        let mut logo = logo;
        if let Some(ref update) = state.update_available {
            logo.push(Line::from(vec![
                Span::styled(
                    format!("⬆ agtx {} available ", update.latest),
                    Style::default().fg(selected_color),
                ),
                Span::styled("[u]", Style::default().fg(dimmed_color)),
            ]));
        }
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
        let footer_text = if state.update_available.is_some() {
            " [p] projects  [n] new project  [u] update  [q] quit "
        } else {
            " [p] projects  [n] new project  [q] quit "
        };
        let footer = Paragraph::new(footer_text)
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

        // Handle update popup if open
        if self.state.update_popup.is_some() {
            return self.handle_update_popup_key(key);
        }

        // Handle trust confirmation popup if open
        if self.state.trust_confirm_popup.is_some() {
            return self.handle_trust_confirm_key(key);
        }

        // Handle diff popup if open
        if self.state.diff_popup.is_some() {
            return self.handle_diff_popup_key(key);
        }

        // Handle dependency-graph overlay if open
        if self.state.dep_graph_popup.is_some() {
            return self.handle_dep_graph_key(key);
        }

        // Handle PR confirmation popup if open
        if self.state.pr_confirm_popup.is_some() {
            return self.handle_pr_confirm_key(key);
        }

        // Handle plugin selection popup if open
        if self.state.plugin_select_popup.is_some() {
            return self.handle_plugin_select_key(key);
        }

        // Handle the config editor if open
        if self.state.config_editor.is_some() {
            return self.handle_config_editor_key(key);
        }

        // Handle the help overlay if open
        if self.state.help_scroll.is_some() {
            return self.handle_help_key(key);
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
            AppMode::Project(_) if self.state.wizard.is_some() => self.handle_wizard_key(key),
            AppMode::Project(_) => {
                // Ctrl+f = open the selected task in the in-app fullscreen view.
                if key.code == KeyCode::Char('f')
                    && key
                        .modifiers
                        .contains(crossterm::event::KeyModifiers::CONTROL)
                {
                    self.open_selected_task_fullscreen()?;
                    return Ok(());
                }
                self.handle_normal_key(key.code)
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

    fn open_update_popup(&mut self) {
        if let Some(info) = self.state.update_available.clone() {
            self.state.update_popup = Some(UpdatePopup {
                info,
                status: None,
                installing: false,
            });
        }
    }

    /// `[u]` popup: Enter installs, Esc closes.
    ///
    /// The install runs on a background thread — it downloads and verifies a
    /// tarball, which must not block the event loop — and reports through
    /// `update_install_rx` like every other background operation here.
    fn handle_update_popup_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let Some(popup) = self.state.update_popup.as_mut() else {
            return Ok(());
        };
        // Ignore everything while the swap is in flight: a second Enter would
        // start a competing download into the same staging directory.
        if popup.installing {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.update_popup = None;
            }
            KeyCode::Enter => {
                // Already finished (success or failure) — Enter closes.
                if popup.status.is_some() {
                    self.state.update_popup = None;
                    return Ok(());
                }
                let tag = popup.info.tag.clone();
                let latest = popup.info.latest.to_string();
                popup.installing = true;
                let (tx, rx) = mpsc::channel();
                self.state.update_install_rx = Some(rx);
                std::thread::spawn(move || {
                    let result = crate::update::install::install_release(&tag, &mut |_| {})
                        .map(|_| {
                            // The running process still holds the old inode;
                            // tmux sessions and their agents are untouched.
                            format!("agtx {latest} installed — restart agtx to apply")
                        })
                        .map_err(|e| e.to_string());
                    let _ = tx.send(result);
                });
            }
            _ => {}
        }
        Ok(())
    }

    /// Answer the trust prompt.
    ///
    /// This decides whether shell commands the user has not read are allowed to
    /// run, so it takes a deliberate `y`. Accepting any key would let typing
    /// ahead after launch, or reaching for an unrelated shortcut, grant it.
    fn handle_trust_confirm_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => {}
            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc | KeyCode::Char('q') => {
                self.state.trust_confirm_popup = None;
                self.state.warning_message = Some((
                    "Left untrusted. init_script, cleanup_script and copy_files stay disabled."
                        .to_string(),
                    Instant::now(),
                ));
                return Ok(());
            }
            // Anything else leaves the question on screen rather than guessing.
            _ => return Ok(()),
        }

        if let Some(popup) = self.state.trust_confirm_popup.clone() {
            self.state.trust_confirm_popup = None;
            // Trust the project: save hash to trust store
            let mut store = crate::config::TrustStore::load().unwrap_or_default();
            if let Err(e) = store.trust_project(&popup.project_path) {
                self.state.warning_message =
                    Some((format!("Failed to trust project: {}", e), Instant::now()));
                return Ok(());
            }
            // Re-enable scripts by reloading project config and re-merging
            let project_config =
                crate::config::ProjectConfig::load(&popup.project_path).unwrap_or_default();
            let global_config = crate::config::GlobalConfig::load().unwrap_or_default();
            self.state.config = crate::config::MergedConfig::merge(&global_config, &project_config);
            self.state.flags.no_init_scripts = false;
            self.state.warning_message = Some((
                "Project trusted. init_script, cleanup_script, and copy_files are now active."
                    .to_string(),
                Instant::now(),
            ));
        }
        Ok(())
    }

    /// Keys while the `?` overlay is up. It is a reference, so it only scrolls
    /// and closes.
    fn handle_help_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let Some(scroll) = self.state.help_scroll else {
            return Ok(());
        };
        // The renderer clamps to what actually fits; this only has to keep the
        // offset inside the table.
        // Clamp to what the last frame could actually show, not to the table
        // length: an offset past the final screenful looks like a dead key.
        let last = self.state.help_max_scroll.get();
        let clamp = |row: i64| -> usize { row.clamp(0, last as i64) as usize };

        // The same chords the task pane scrolls with, from the same table.
        if let Some(action) = scroll_action_for(key) {
            self.state.help_scroll = Some(match action {
                PopupScroll::Up(lines) => clamp(scroll as i64 - lines as i64),
                PopupScroll::Down(lines) => clamp(scroll as i64 + lines as i64),
                PopupScroll::Bottom => last,
            });
            return Ok(());
        }

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('?') => {
                self.state.help_scroll = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.state.help_scroll = Some(clamp(scroll as i64 + 1));
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.state.help_scroll = Some(clamp(scroll as i64 - 1));
            }
            KeyCode::Home => self.state.help_scroll = Some(0),
            KeyCode::End => self.state.help_scroll = Some(last),
            _ => {}
        }
        Ok(())
    }

    /// Open the config editor on the configs as they are on disk.
    ///
    /// Loaded fresh rather than reconstructed from `state.config`: that is a
    /// *merged* view, and writing it back would bake every global default into
    /// the project file as an explicit override.
    fn open_config_editor(&mut self) {
        let global = GlobalConfig::load().unwrap_or_default();
        let project = self
            .state
            .project_path
            .as_ref()
            .map(|path| ProjectConfig::load(path).unwrap_or_default());

        let agents: Vec<String> = agent::known_agents().into_iter().map(|a| a.name).collect();

        let mut plugins: Vec<String> = skills::BUNDLED_PLUGINS
            .iter()
            .map(|(name, _, _)| name.to_string())
            .collect();
        for custom in skills::discover_custom_plugins(self.state.project_path.as_deref()) {
            if !plugins.contains(&custom.name) {
                plugins.push(custom.name);
            }
        }

        self.state.config_editor = Some(ConfigEditor::open(global, project, &agents, &plugins));
    }

    fn handle_config_editor_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let Some(editor) = self.state.config_editor.as_mut() else {
            return Ok(());
        };
        let action = editor.handle_key(key);
        // Preview the theme as it is edited. Theme is global-only, so the
        // merged config's copy can simply be replaced — no merge to redo, and
        // every draw call already reads it. `reload_config` on close is what
        // puts back an unsaved experiment.
        let previewed = editor.global.theme.clone();
        self.state.config.theme = previewed;

        match action {
            EditorAction::None => Ok(()),
            EditorAction::Close => {
                self.state.config_editor = None;
                // Drop any previewed theme that was never saved.
                self.reload_config();
                Ok(())
            }
            EditorAction::Save => self.save_config_editor(),
        }
    }

    /// Write both configs, restore the project's trust, and re-merge.
    fn save_config_editor(&mut self) -> Result<()> {
        let Some(editor) = self.state.config_editor.as_mut() else {
            return Ok(());
        };
        let global = editor.global.clone();
        let project = editor.project.clone();

        if let Err(e) = global.save() {
            editor.status = Some(format!("Could not save global config: {e}"));
            return Ok(());
        }

        if let (Some(project), Some(path)) = (project, self.state.project_path.clone()) {
            // Trust is a hash of this file, so read it before the write. See
            // `TrustStore::retrust_after_agtx_write` — the user authored these
            // changes in agtx's own UI, which is the consent the hash protects.
            let was_trusted = crate::config::TrustStore::load()
                .map(|store| store.is_trusted(&path))
                .unwrap_or(false);
            if let Err(e) = project.save(&path) {
                if let Some(editor) = self.state.config_editor.as_mut() {
                    editor.status = Some(format!("Could not save project config: {e}"));
                }
                return Ok(());
            }
            if let Err(e) = crate::config::TrustStore::retrust_after_agtx_write(&path, was_trusted)
            {
                self.state.warning_message = Some((
                    format!("Config saved, but re-trusting the project failed: {e}"),
                    Instant::now(),
                ));
            }
        }

        self.reload_config();
        if let Some(editor) = self.state.config_editor.as_mut() {
            editor.mark_saved("Saved. Worktree settings apply to new tasks.");
        }
        Ok(())
    }

    /// Re-read both configs from disk and rebuild everything derived from them.
    fn reload_config(&mut self) {
        let global = GlobalConfig::load().unwrap_or_default();
        let project = self
            .state
            .project_path
            .as_ref()
            .map(|path| ProjectConfig::load(path).unwrap_or_default())
            .unwrap_or_default();
        self.state.config = MergedConfig::merge(&global, &project);
        self.state.cached_plugin = Some(load_plugin_if_configured(
            &self.state.config,
            self.state.project_path.as_deref(),
        ));
    }

    fn open_plugin_select_popup(&mut self) {
        let current = self
            .state
            .config
            .workflow_plugin
            .as_deref()
            .unwrap_or("agtx");
        let mut options = vec![PickOption {
            name: "agtx".to_string(),
            label: "agtx".to_string(),
            description: "Built-in workflow with skills and prompts".to_string(),
            active: current == "agtx",
        }];
        for (name, desc, _content) in skills::BUNDLED_PLUGINS {
            if *name == "agtx" {
                continue;
            }
            options.push(PickOption {
                name: name.to_string(),
                label: name.to_string(),
                description: desc.to_string(),
                active: current == *name,
            });
        }
        let agent_name = &self.state.config.default_agent;
        for custom in skills::discover_custom_plugins(self.state.project_path.as_deref()) {
            if !custom.plugin.supports_agent(agent_name) {
                continue;
            }
            options.push(PickOption {
                name: custom.name.clone(),
                label: custom.name.clone(),
                description: custom.description,
                active: current == custom.name,
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

        // Read trust *before* the write: saving the config changes its hash,
        // which is what trust is. See `TrustStore::retrust_after_agtx_write`.
        let was_trusted = crate::config::TrustStore::load()
            .map(|store| store.is_trusted(&project_path))
            .unwrap_or(false);

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

        // The save just invalidated the trust hash. Restore the prior decision
        // so picking a plugin does not untrust the project behind the user's
        // back; a project that was already untrusted stays that way.
        if let Err(e) =
            crate::config::TrustStore::retrust_after_agtx_write(&project_path, was_trusted)
        {
            self.state.warning_message = Some((
                format!("Plugin set, but re-trusting the project failed: {}", e),
                Instant::now(),
            ));
        }

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
                let agent = task.agent.clone();

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
                let cleanup_script = if self.state.flags.no_init_scripts {
                    None
                } else {
                    self.state.config.cleanup_script.clone()
                };
                std::thread::spawn(move || {
                    cleanup_task_resources(
                        &task_id,
                        &agent,
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

            let scroll_action = scroll_action_for(key);
            if let Some(action) = scroll_action {
                handle_popup_scroll(
                    popup,
                    self.state.input_sink.as_ref(),
                    &mut self.state.warning_message,
                    &window_name,
                    action,
                );
                return Ok(());
            }

            match key.code {
                // Ctrl+q = close popup
                KeyCode::Char('q') if has_ctrl => {
                    // Anything still batched belongs to *this* pane. Deliver it
                    // before the popup — and with it the target — can change,
                    // and before agtx's own next write to this pane (a phase
                    // advance is one keystroke away on the board).
                    flush_pane_input_sync(
                        self.state.input_sink.as_ref(),
                        &mut self.state.warning_message,
                    );
                    self.state.shell_popup = None;
                }
                // Ctrl+f toggles between centered and fullscreen in-app views.
                KeyCode::Char('f') if has_ctrl => {
                    // The resize is a plain tmux subprocess on a different
                    // socket; queued text must reach the pane at the size it was
                    // typed at, so this waits rather than just enqueueing.
                    flush_pane_input_sync(
                        self.state.input_sink.as_ref(),
                        &mut self.state.warning_message,
                    );
                    popup.fullscreen = !popup.fullscreen;
                    let (pane_width, pane_height) = if popup.fullscreen {
                        crossterm::terminal::size()
                            .map(|(width, height)| {
                                (width.saturating_sub(2), height.saturating_sub(4))
                            })
                            .unwrap_or((SHELL_POPUP_CONTENT_WIDTH, 20))
                    } else {
                        let height = crossterm::terminal::size()
                            .map(|(_, height)| {
                                (height as u32 * SHELL_POPUP_HEIGHT_PERCENT as u32 / 100) as u16
                            })
                            .unwrap_or(24);
                        (SHELL_POPUP_CONTENT_WIDTH, height.saturating_sub(4))
                    };
                    let _ =
                        self.state
                            .tmux_ops
                            .resize_window(&window_name, pane_width, pane_height);
                    popup.last_pane_size = Some((pane_width, pane_height));
                    return Ok(());
                }
                _ => {
                    // Forward all other keys to tmux window (including Esc)
                    if let Some(input) = popup_key_input(&window_name, key) {
                        forward_pane_input(
                            self.state.input_sink.as_ref(),
                            &mut self.state.warning_message,
                            input,
                        );
                    }
                }
            }
        }
        Ok(())
    }

    fn handle_paste(&mut self, text: String) -> Result<()> {
        // Shell popup open: forward paste to the tmux pane with proper bracketed
        // paste sequences. It goes through the broker like every other request,
        // so it cannot overtake characters typed just before it.
        if let Some(ref popup) = self.state.shell_popup {
            let window_name = popup.window_name.clone();
            forward_pane_input(
                self.state.input_sink.as_ref(),
                &mut self.state.warning_message,
                PaneInput::Paste {
                    target: window_name,
                    text,
                },
            );
            return Ok(());
        }

        // Prompt step open: insert pasted text at the caret.
        if self.state.wizard_step() == Some(WizardStep::Prompt) {
            let prompt = self.prompt_mut();
            let cursor = prompt.cursor;
            prompt.buffer.insert_str(cursor, &text);
            prompt.cursor += text.len();
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
                KeyCode::Char('u') if self.state.update_available.is_some() => {
                    self.open_update_popup();
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
                self.state.wizard = Some(WizardState::creating());
                self.seed_wizard_lists();
            }
            KeyCode::Enter => {
                if let Some(task) = self.state.board.selected_task() {
                    if task.status == TaskStatus::Backlog && task.session_name.is_some() {
                        // Backlog task with active research session
                        if self.state.config.fullscreen_on_enter {
                            self.open_selected_task_fullscreen()?;
                        } else {
                            self.open_selected_task()?;
                        }
                    } else if task.status == TaskStatus::Backlog {
                        // Edit task
                        self.state.wizard =
                            Some(WizardState::editing(task.id.clone(), task.title.clone()));
                        self.seed_wizard_lists();
                    } else if task.session_name.is_some() {
                        // Open shell popup or fullscreen
                        if self.state.config.fullscreen_on_enter {
                            self.open_selected_task_fullscreen()?;
                        } else {
                            self.open_selected_task()?;
                        }
                    }
                }
            }
            KeyCode::Char('x') => self.delete_selected_task()?,
            KeyCode::Char('d') => self.show_task_diff()?,
            KeyCode::Char('D') => self.show_dependency_graph()?,
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
            KeyCode::Char(',') => self.open_config_editor(),
            KeyCode::Char('?') => self.state.help_scroll = Some(0),
            KeyCode::Char('u') if self.state.update_available.is_some() => {
                self.open_update_popup();
            }
            KeyCode::Char('O') if self.state.flags.experimental => {
                // Toggle orchestrator agent (experimental)
                self.toggle_orchestrator()?;
            }
            _ => {}
        }
        Ok(())
    }

    /// The wizard's state. Only reachable while it is open, which the key
    /// dispatch guarantees before routing anything here.
    fn wizard_mut(&mut self) -> &mut WizardState {
        self.state
            .wizard
            .as_mut()
            .expect("wizard handlers only run while the wizard is open")
    }

    /// Every key the wizard sees, whichever step it is on.
    ///
    /// The navigation keys are handled first and identically on all three
    /// steps — that is the whole point of one handler. Only what is left over
    /// is dispatched to the step.
    fn handle_wizard_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let ctrl = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);

        // A dropdown on the prompt step owns its keys entirely, including Esc
        // and Enter — closing the picker must not also step the wizard back.
        if self.wizard_mut().step() == WizardStep::Prompt
            && (self.handle_task_ref_search_key(key)
                || self.handle_skill_search_key(key)
                || self.handle_file_search_key(key))
        {
            return Ok(());
        }

        // Any keystroke means the user is acting on the complaint, so it goes.
        self.wizard_mut().validation = None;

        // An open filter owns Esc the way a prompt dropdown does: closing the
        // filter must not take the wizard with it.
        if key.code == KeyCode::Esc {
            if let Some(list) = self.wizard_mut().current_list_mut() {
                if list.is_filtering() {
                    list.stop_filter();
                    list.settle();
                    return Ok(());
                }
            }
        }

        match key.code {
            // Save from anywhere, so a correction made on step one does not
            // require walking forward through the rest of the flow again.
            KeyCode::Char('s') if ctrl => {
                self.try_save_wizard()?;
                return Ok(());
            }
            // Esc leaves the wizard outright, from any step. Stepping back is
            // Shift+Tab / Ctrl+B below — one key that always means "get me out
            // of here" beats one whose effect depends on where you are.
            KeyCode::Esc => {
                self.cancel_wizard();
                return Ok(());
            }
            // Legacy terminals send BackTab; the Kitty protocol sends Tab with
            // SHIFT. Both mean the same thing.
            KeyCode::BackTab => {
                self.wizard_mut().back();
                return Ok(());
            }
            KeyCode::Tab
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::SHIFT) =>
            {
                self.wizard_mut().back();
                return Ok(());
            }
            KeyCode::Char('b') if ctrl => {
                self.wizard_mut().back();
                return Ok(());
            }
            _ => {}
        }

        match self.wizard_mut().step() {
            WizardStep::Title => self.handle_title_input(key),
            WizardStep::Agent | WizardStep::Plugin => self.handle_list_step_key(key),
            WizardStep::Prompt => self.handle_prompt_input(key),
        }
    }

    fn handle_title_input(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Enter => self.advance_wizard()?,
            // Motion, deletion and ordinary characters are the same in every
            // text field; only the keys above mean something particular here.
            _ => {
                self.wizard_mut().title.handle_edit_key(key);
            }
        }
        Ok(())
    }

    /// Keys on the agent and plugin steps. Both are pick-one-from-a-list, so
    /// they share a handler.
    ///
    /// `/` starts a filter, and while one is open ordinary characters go to it
    /// — which is why navigation there is arrows and `C-n`/`C-p` rather than
    /// `j`/`k`. Without the filter open, `j`/`k` navigate as usual.
    fn handle_list_step_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let ctrl = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);
        if key.code == KeyCode::Enter {
            return self.advance_wizard();
        }
        let Some(list) = self.wizard_mut().current_list_mut() else {
            return Ok(());
        };

        if list.is_filtering() {
            match key.code {
                KeyCode::Down => list.select_next(),
                KeyCode::Up => list.select_prev(),
                KeyCode::Char('n') if ctrl => list.select_next(),
                KeyCode::Char('p') if ctrl => list.select_prev(),
                KeyCode::Tab => list.cycle(),
                _ => {
                    let filter = list.filter.as_mut().expect("filtering");
                    if filter.handle_edit_key(key) {
                        // Backspacing the filter away is how you leave it, so
                        // the whole list comes back rather than the step
                        // staying in a mode with nothing typed.
                        if filter.is_empty() && key.code == KeyCode::Backspace {
                            list.stop_filter();
                        }
                        list.settle();
                    }
                }
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Char('/') => list.start_filter(),
            KeyCode::Char('j') | KeyCode::Down => list.select_next(),
            KeyCode::Char('k') | KeyCode::Up => list.select_prev(),
            KeyCode::Char('n') if ctrl => list.select_next(),
            KeyCode::Char('p') if ctrl => list.select_prev(),
            KeyCode::Tab => list.cycle(),
            _ => {}
        }
        Ok(())
    }

    /// Move to the next step, seeding it if this is the first time through, or
    /// save when there is no next step.
    fn advance_wizard(&mut self) -> Result<()> {
        if self.wizard_mut().step() == WizardStep::Title {
            // Nothing downstream can proceed without a title, so refuse here
            // rather than letting the user reach the last step and be told.
            if let Some(problem) = self.title_problem() {
                self.wizard_mut().validation = Some(problem);
                return Ok(());
            }
        }
        // The plugin list is filtered by the agent the task will run on, so a
        // change on the agent step has to rebuild it — otherwise the user is
        // offered plugins their agent does not support.
        if self.wizard_mut().step() == WizardStep::Agent {
            self.reseed_plugins_for_agent();
        }
        if !self.wizard_mut().advance() {
            return self.try_save_wizard();
        }
        if self.wizard_mut().step() == WizardStep::Prompt {
            self.seed_prompt_step();
        }
        Ok(())
    }

    /// Save if the wizard has what it needs, and say why not if it does not.
    ///
    /// A silent refusal reads as a broken key rather than a rejected input, so
    /// every path out of here either saves or explains itself.
    /// What is wrong with the title, if anything.
    ///
    /// One function so `Enter` on the title step and `Ctrl+S` from anywhere
    /// refuse for the same reasons and say the same thing.
    fn title_problem(&self) -> Option<String> {
        let wizard = self.state.wizard.as_ref()?;
        let title = wizard.title.as_str().trim();
        if title.is_empty() {
            return Some("A title is required.".to_string());
        }
        if title.chars().count() > MAX_TASK_TITLE_CHARS {
            return Some(format!(
                "Title is {} characters; the limit is {MAX_TASK_TITLE_CHARS}.",
                title.chars().count()
            ));
        }
        // A duplicate is legal but almost never intended, and the board shows
        // titles alone — two identical cards are indistinguishable.
        let editing = wizard.editing_task_id.as_deref();
        let clash = self
            .state
            .board
            .tasks
            .iter()
            .any(|t| Some(t.id.as_str()) != editing && t.title.trim() == title);
        if clash {
            return Some(format!("Another task is already called \"{title}\"."));
        }
        None
    }

    fn try_save_wizard(&mut self) -> Result<()> {
        // The plugin list is filtered by the agent; saving from the agent step
        // itself would otherwise store a plugin that agent does not support.
        self.reseed_plugins_for_agent();
        if let Some(problem) = self.title_problem() {
            let wizard = self.wizard_mut();
            // Send the user where the problem is, not where the key was pressed.
            // `back` clears the message on the way, so it is set afterwards.
            while wizard.step() != WizardStep::Title && wizard.back() {}
            wizard.validation = Some(problem);
            return Ok(());
        }
        self.save_task()?;
        self.cancel_wizard();
        Ok(())
    }

    /// The wizard's prompt field.
    ///
    /// The dropdown handlers below only run while the prompt step is open, so a
    /// missing wizard there is a routing bug rather than a state to handle.
    fn prompt_mut(&mut self) -> &mut TextInput {
        &mut self
            .state
            .wizard
            .as_mut()
            .expect("prompt handlers only run while the wizard is open")
            .prompt
    }

    /// Splice `replacement` over the trigger character and the pattern typed
    /// after it, keeping whatever follows.
    ///
    /// All three dropdowns commit and cancel this way; only the text differs.
    /// The caret lands at the end of the inserted text rather than the end of
    /// the line, which is what lets the user keep typing mid-sentence.
    fn splice_search_region(&mut self, start: usize, pattern_len: usize, replacement: &str) {
        let prompt = self.prompt_mut();
        // +1 for the trigger character itself.
        let end = (start + 1 + pattern_len).min(prompt.buffer.len());
        let start = start.min(prompt.buffer.len());
        let suffix = prompt.buffer[end..].to_string();
        prompt.buffer.truncate(start);
        prompt.buffer.push_str(replacement);
        prompt.cursor = prompt.buffer.len();
        prompt.buffer.push_str(&suffix);
    }

    /// Keys belonging to the `!` task-reference dropdown. `false` when it is
    /// not open, so the caller falls through to the ordinary prompt handling.
    ///
    /// The dropdown state is **taken out** for the duration: every arm needs it
    /// alongside the wizard's prompt field, and several also call back into
    /// `self` to refresh the match list, which a held borrow would not allow.
    fn handle_task_ref_search_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        let Some(mut search) = self.state.task_ref_search.take() else {
            return false;
        };
        let ctrl = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
        let mut keep_open = true;
        let mut refresh = false;

        match key.code {
            KeyCode::Esc => {
                self.splice_search_region(search.start_pos, search.pattern.len(), "");
                keep_open = false;
            }
            KeyCode::Enter | KeyCode::Tab => {
                if let Some((task_id, title, _status)) =
                    search.matches.get(search.selected).cloned()
                {
                    let ref_text = format!("![{}]", title);
                    self.splice_search_region(search.start_pos, search.pattern.len(), &ref_text);
                    let wizard = self.wizard_mut();
                    wizard.highlighted_references.insert(ref_text);
                    wizard.referenced_task_ids.insert(task_id);
                }
                keep_open = false;
            }
            KeyCode::Up => search.select_prev(),
            KeyCode::Down => search.select_next(),
            KeyCode::Char('k') | KeyCode::Char('p') if ctrl => search.select_prev(),
            KeyCode::Char('j') | KeyCode::Char('n') if ctrl => search.select_next(),
            KeyCode::Backspace => {
                if search.pattern.is_empty() {
                    // Nothing typed yet: backspace removes the `!` and closes.
                    self.prompt_mut().backspace();
                    keep_open = false;
                } else {
                    search.pattern.pop();
                    self.prompt_mut().backspace();
                    refresh = true;
                }
            }
            // A chord the picker did not claim is not text: `Ctrl+X` must not
            // reach the pattern, and `Ctrl+J` means newline once the picker is
            // closed.
            KeyCode::Char(c) if !ctrl && !alt => {
                search.pattern.push(c);
                self.prompt_mut().insert_char(c);
                refresh = true;
            }
            _ => {}
        }

        if keep_open {
            if refresh {
                search.matches = self.get_all_task_matches(&search.pattern);
                search.selected = 0;
            }
            self.state.task_ref_search = Some(search);
        }
        true
    }

    /// Keys belonging to the `/` skill dropdown. See
    /// `handle_task_ref_search_key` for why the state is taken out.
    fn handle_skill_search_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        let Some(mut search) = self.state.skill_search.take() else {
            return false;
        };
        let ctrl = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
        let mut keep_open = true;
        let mut refresh = false;

        match key.code {
            KeyCode::Esc => {
                self.splice_search_region(search.start_pos, search.pattern.len(), "");
                keep_open = false;
            }
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(entry) = search.matches.get(search.selected).cloned() {
                    self.splice_search_region(
                        search.start_pos,
                        search.pattern.len(),
                        &entry.command,
                    );
                    self.wizard_mut()
                        .highlighted_references
                        .insert(entry.command);
                }
                keep_open = false;
            }
            KeyCode::Up => search.select_prev(),
            KeyCode::Down => search.select_next(),
            KeyCode::Char('k') | KeyCode::Char('p') if ctrl => search.select_prev(),
            KeyCode::Char('j') | KeyCode::Char('n') if ctrl => search.select_next(),
            KeyCode::Backspace => {
                if search.pattern.is_empty() {
                    self.prompt_mut().backspace();
                    keep_open = false;
                } else {
                    search.pattern.pop();
                    self.prompt_mut().backspace();
                    refresh = true;
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                search.pattern.push(c);
                self.prompt_mut().insert_char(c);
                refresh = true;
            }
            _ => {}
        }

        if keep_open {
            self.state.skill_search = Some(search);
            if refresh {
                self.update_skill_search_matches();
            }
        }
        true
    }

    /// Keys belonging to the `#` / `@` file dropdown. See
    /// `handle_task_ref_search_key` for why the state is taken out.
    fn handle_file_search_key(&mut self, key: crossterm::event::KeyEvent) -> bool {
        let Some(mut search) = self.state.file_search.take() else {
            return false;
        };
        let ctrl = key
            .modifiers
            .contains(crossterm::event::KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(crossterm::event::KeyModifiers::ALT);
        let mut keep_open = true;
        let mut refresh = false;

        match key.code {
            // Unlike the other two, Esc leaves the typed text in place — the
            // `#` was a real character the user may have meant.
            KeyCode::Esc => keep_open = false,
            KeyCode::Enter | KeyCode::Tab => {
                if let Some(selected_file) = search.matches.get(search.selected).cloned() {
                    self.splice_search_region(
                        search.start_pos,
                        search.pattern.len(),
                        &selected_file,
                    );
                    self.wizard_mut()
                        .highlighted_references
                        .insert(selected_file);
                }
                keep_open = false;
            }
            KeyCode::Up => search.select_prev(),
            KeyCode::Down => search.select_next(),
            KeyCode::Char('k') | KeyCode::Char('p') if ctrl => search.select_prev(),
            KeyCode::Char('j') | KeyCode::Char('n') if ctrl => search.select_next(),
            KeyCode::Backspace => {
                if search.pattern.is_empty() {
                    self.prompt_mut().backspace();
                    keep_open = false;
                } else {
                    search.pattern.pop();
                    self.prompt_mut().backspace();
                    refresh = true;
                }
            }
            KeyCode::Char(c) if !ctrl && !alt => {
                search.pattern.push(c);
                self.prompt_mut().insert_char(c);
                refresh = true;
            }
            _ => {}
        }

        if keep_open {
            self.state.file_search = Some(search);
            if refresh {
                self.update_file_search_matches();
            }
        }
        true
    }

    /// Keys on the wizard's prompt step, once no dropdown has claimed them.
    fn handle_prompt_input(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        match key.code {
            // Two chords that need no negotiation from the terminal, beside
            // the `\`+Enter escape below:
            //
            // - `Ctrl+J` always works: in raw mode crossterm parses 0x0A as
            //   Ctrl+J rather than as Enter.
            // - `Alt+Enter` arrives as ESC then CR on most terminals, though
            //   macOS Terminal and iTerm2 only send it once Option is
            //   configured as Meta.
            //
            // `Shift+Enter` is deliberately absent: a terminal sends a bare CR
            // for both it and Enter unless the Kitty keyboard protocol is
            // negotiated, so the binding would fire almost never and read as
            // "save" the rest of the time. See `with_ops`.
            KeyCode::Char('j')
                if key
                    .modifiers
                    .contains(crossterm::event::KeyModifiers::CONTROL) =>
            {
                self.prompt_mut().insert_char('\n');
            }
            KeyCode::Enter if key.modifiers.contains(crossterm::event::KeyModifiers::ALT) => {
                self.prompt_mut().insert_char('\n');
            }
            KeyCode::Enter => {
                // A trailing backslash is the line-continuation escape.
                if self.prompt_mut().buffer.ends_with('\\') {
                    let prompt = self.prompt_mut();
                    prompt.buffer.pop();
                    prompt.buffer.push('\n');
                    prompt.cursor = prompt.buffer.len();
                } else {
                    self.try_save_wizard()?;
                }
            }
            KeyCode::Char('#') | KeyCode::Char('@') => {
                let trigger = if let KeyCode::Char(c) = key.code {
                    c
                } else {
                    '#'
                };
                let start_pos = self.prompt_mut().cursor;
                self.prompt_mut().insert_char(trigger);
                self.state.file_search = Some(FileSearchState {
                    pattern: String::new(),
                    matches: vec![],
                    selected: 0,
                    start_pos,
                    trigger_char: trigger,
                });
                self.update_file_search_matches();
            }
            KeyCode::Char('/') if self.prompt_at_word_start() => {
                let start_pos = self.prompt_mut().cursor;
                self.prompt_mut().insert_char('/');

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
            KeyCode::Char('!') if self.prompt_at_word_start() => {
                let start_pos = self.prompt_mut().cursor;
                self.prompt_mut().insert_char('!');

                let matches = self.get_all_task_matches("");
                self.state.task_ref_search = Some(TaskRefSearchState {
                    pattern: String::new(),
                    matches,
                    selected: 0,
                    start_pos,
                });
            }
            // Everything the trigger guards above declined — including a `/`
            // mid-word, which must arrive as ordinary text — plus all motion
            // and deletion. See `tui::text_input`.
            _ => {
                self.prompt_mut().handle_edit_key(key);
            }
        }
        Ok(())
    }

    /// Whether the caret sits where `/` and `!` open their pickers: at the very
    /// start, or just after a space or newline. Mid-word they are ordinary
    /// characters — a path like `src/main.rs` must not open the skill list.
    fn prompt_at_word_start(&self) -> bool {
        let Some(wizard) = self.state.wizard.as_ref() else {
            return false;
        };
        let cursor = wizard.prompt.cursor;
        if cursor == 0 {
            return true;
        }
        matches!(
            wizard.prompt.buffer.as_bytes().get(cursor - 1),
            Some(&b'\n') | Some(&b' ')
        )
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
        let Some(wizard) = self.state.wizard.as_ref() else {
            return Ok(());
        };
        let Some(db) = self.state.db.as_ref() else {
            return Ok(());
        };
        // The agent step's pick when it offered one, else the configured
        // default. `Task::agent` is what every later phase reads.
        let agent = wizard
            .agent_name()
            .map(str::to_string)
            .unwrap_or_else(|| self.state.config.default_agent.clone());
        let plugin = wizard.plugin_name().map(str::to_string);
        let refs = if wizard.referenced_task_ids.is_empty() {
            None
        } else {
            Some(
                wizard
                    .referenced_task_ids
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(","),
            )
        };
        // `can_save` has already rejected a blank title, so trimming here only
        // strips the padding a real one may have picked up.
        let title = wizard.title.as_str().trim().to_string();
        let description = if wizard.prompt.is_empty() {
            None
        } else {
            Some(wizard.prompt.buffer.clone())
        };

        match wizard.editing_task_id.clone() {
            Some(task_id) => {
                if let Some(mut task) = db.get_task(&task_id)? {
                    task.title = title;
                    task.description = description;
                    task.agent = agent;
                    task.plugin = plugin;
                    task.referenced_tasks = refs;
                    task.updated_at = chrono::Utc::now();
                    db.update_task(&task)?;
                }
            }
            None => {
                let project_id = self.state.project_name.clone();
                let mut task = Task::new(&title, agent, project_id);
                task.description = description;
                task.plugin = plugin;
                task.referenced_tasks = refs;
                // Task starts in Backlog without tmux window.
                // No orchestrator notification — it only manages Planning/Running.
                db.create_task(&task)?;
            }
        }
        self.refresh_tasks()
    }

    /// On a first launch, ask which agent to use in the editor that owns that
    /// question rather than in a menu of its own.
    ///
    /// A no-op on every other launch, so the caller does not have to check.
    fn open_first_run_editor(&mut self) {
        if !self.state.flags.first_run {
            return;
        }
        self.open_config_editor();
        if let Some(editor) = self.state.config_editor.as_mut() {
            editor.focus(super::config_editor::FieldId::DefaultAgent);
            // Kept shorter than the footer, which is what sets the box width.
            editor.status = Some("Welcome — pick your default agent.".to_string());
        }
    }

    /// Fill both list steps as soon as the wizard opens.
    ///
    /// Done here rather than on the first `Enter` so the breadcrumb shows the
    /// real flow from the first frame — otherwise it reads "Title › Prompt"
    /// and then grows two steps once you commit to a title.
    fn seed_wizard_lists(&mut self) {
        self.seed_agent_step();
        self.seed_plugin_step();
    }

    /// Rebuild the plugin list when the agent it was filtered by has changed.
    ///
    /// Keeps the user's pick when that plugin survives the new filter, because
    /// switching agent is not a decision about the plugin.
    fn reseed_plugins_for_agent(&mut self) {
        let agent = self.wizard_mut().agent_name().map(str::to_string);
        if agent.as_deref() == self.state.plugins_filtered_for.as_deref() {
            return;
        }
        let previous = self
            .wizard_mut()
            .plugin
            .selected_option()
            .map(|o| o.name.clone());
        self.wizard_mut().plugin = super::wizard::ListPick::default();
        self.seed_plugin_step();
        if let Some(name) = previous {
            let wizard = self.wizard_mut();
            if let Some(index) = wizard.plugin.options.iter().position(|o| o.name == name) {
                wizard.plugin.selected = index;
            }
        }
    }

    /// Build the agent list and decide whether the step is worth showing.
    ///
    /// The choice is per task: `Task::agent` is a database field, and before
    /// this the only way to run one task on a different agent was to edit the
    /// config, start it, and edit back.
    fn seed_agent_step(&mut self) {
        if !self.wizard_mut().agent.take_seed() {
            return;
        }
        let default_agent = self.state.config.default_agent.clone();
        // Editing opens on whatever the task already runs on, which need not be
        // the project default.
        let current = self
            .wizard_mut()
            .editing_task_id
            .clone()
            .and_then(|id| {
                self.state
                    .db
                    .as_ref()
                    .and_then(|db| db.get_task(&id).ok().flatten())
            })
            .map(|t| t.agent)
            .unwrap_or_else(|| default_agent.clone());

        let options: Vec<PickOption> = self
            .state
            .available_agents
            .iter()
            .map(|agent| {
                PickOption::new(
                    &agent.name,
                    &agent.name,
                    &agent.description,
                    agent.name == current,
                )
            })
            .collect();

        let selected = options.iter().position(|o| o.active).unwrap_or(0);
        let offer_step = options.len() > 1;
        let wizard = self.wizard_mut();
        wizard.agent.options = options;
        wizard.agent.selected = selected;
        wizard.set_optional_step(WizardStep::Agent, offer_step);
    }

    /// Build the plugin list and decide whether the step is worth showing.
    ///
    /// Runs once per wizard: re-entering the title step and advancing again
    /// must not rebuild the list, or the user's pick resets to the project
    /// default they just chose against.
    fn seed_plugin_step(&mut self) {
        if !self.wizard_mut().plugin.take_seed() {
            return;
        }
        // With no detected agents there is no compatibility to filter on and
        // nothing meaningful to choose between.
        if self.state.available_agents.is_empty() {
            self.wizard_mut()
                .set_optional_step(WizardStep::Plugin, false);
            return;
        }

        let editing = self.wizard_mut().editing_task_id.clone();
        let current = match editing {
            Some(task_id) => self
                .state
                .db
                .as_ref()
                .and_then(|db| db.get_task(&task_id).ok().flatten())
                .and_then(|t| t.plugin.clone())
                .or_else(|| self.state.config.workflow_plugin.clone())
                .unwrap_or_else(|| "agtx".to_string()),
            None => self
                .state
                .config
                .workflow_plugin
                .as_deref()
                .unwrap_or("agtx")
                .to_string(),
        };

        // Filtered against the agent the *task* will run on, which the agent
        // step may just have changed — a plugin that does not support it should
        // not be offered.
        let selected_agent_name = self
            .wizard_mut()
            .agent_name()
            .map(str::to_string)
            .unwrap_or_else(|| self.state.config.default_agent.clone());

        let mut options = vec![PickOption::new(
            "agtx",
            "agtx",
            "Built-in workflow with skills and prompts",
            current == "agtx",
        )];
        for (name, desc, content) in skills::BUNDLED_PLUGINS {
            if *name == "agtx" {
                continue;
            }
            // Filter by agent compatibility
            if let Ok(plugin) = toml::from_str::<WorkflowPlugin>(content) {
                if !plugin.supports_agent(&selected_agent_name) {
                    continue;
                }
            }
            options.push(PickOption::new(name, name, desc, current == *name));
        }
        for custom in skills::discover_custom_plugins(self.state.project_path.as_deref()) {
            if !custom.plugin.supports_agent(&selected_agent_name) {
                continue;
            }
            let active = current == custom.name;
            options.push(PickOption::new(
                &custom.name,
                &custom.name,
                &custom.description,
                active,
            ));
        }
        let selected = options.iter().position(|o| o.active).unwrap_or(0);
        let offer_step = options.len() > 1;
        self.state.plugins_filtered_for = Some(selected_agent_name);
        let wizard = self.wizard_mut();
        wizard.plugin.options = options;
        wizard.plugin.selected = selected;
        wizard.set_optional_step(WizardStep::Plugin, offer_step);
    }

    /// Load an edited task's existing prompt and references.
    ///
    /// Runs once per wizard, for the same reason as `seed_plugin_step` and with
    /// worse consequences: reloading from the database on the way forward would
    /// throw away every edit made before the user stepped back.
    fn seed_prompt_step(&mut self) {
        if !self.wizard_mut().take_prompt_seed() {
            return;
        }
        let Some(task_id) = self.wizard_mut().editing_task_id.clone() else {
            return;
        };
        let existing = self
            .state
            .db
            .as_ref()
            .and_then(|db| db.get_task(&task_id).ok().flatten());
        let wizard = self.wizard_mut();
        match existing {
            Some(task) => {
                wizard.prompt.set_text(task.description.unwrap_or_default());
                wizard.referenced_task_ids = task
                    .referenced_tasks
                    .as_deref()
                    .map(|s| {
                        s.split(',')
                            .filter(|id| !id.is_empty())
                            .map(String::from)
                            .collect()
                    })
                    .unwrap_or_default();
            }
            None => wizard.prompt.clear(),
        }
    }

    /// Close the wizard, discarding it. Any open dropdown goes with it, and so
    /// does the note of which agent its plugin list was filtered by.
    fn cancel_wizard(&mut self) {
        self.state.wizard = None;
        self.state.plugins_filtered_for = None;
        self.state.task_ref_search = None;
        self.state.skill_search = None;
        self.state.file_search = None;
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
                let cleanup_script = if self.state.flags.no_init_scripts {
                    None
                } else {
                    self.state.config.cleanup_script.clone()
                };
                delete_task_resources(
                    &task,
                    cleanup_script.as_deref(),
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
            // Block moving out of Backlog when dependencies are not satisfied
            if current_status == TaskStatus::Backlog {
                if let Some(db) = &self.state.db {
                    if !db.deps_satisfied(&task) {
                        self.state.warning_message = Some((
                            "Dependencies not in Review/Done — cannot start task".to_string(),
                            Instant::now(),
                        ));
                        return Ok(());
                    }
                }
            }

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
                && is_agent_active(&*self.state.tmux_ops, target, Some(task.agent.as_str()))
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

        let has_live_session = task_has_live_session(&task, self.state.tmux_ops.as_ref());
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
                &task.id,
                true,
            );
            // Non-collapsed twin for the argv path (see `spawn_send_to_agent`).
            let skill_cmd_launch = resolve_skill_command(
                &plugin,
                planning_phase,
                &planning_agent,
                &task_content,
                task.cycle,
                &task.id,
                false,
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
                task.id.clone(),
                self.state.config.agent_hooks,
                self.state.config.auto_trust,
                target,
                task.agent.clone(),
                planning_agent.clone(),
                agent_switch,
                skill_cmd,
                skill_cmd_launch,
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
            &task.id,
            true,
        );
        // The launch lane hands the command to the process in argv, so it keeps the
        // task's own line structure. Only the send-after-ready fallback below needs
        // the flattened form. See `resolve_skill_command`.
        let skill_cmd_launch = resolve_skill_command(
            &plugin,
            "planning",
            &planning_agent,
            &task_content,
            task.cycle,
            &task.id,
            false,
        );
        let prompt_trigger = resolve_prompt_trigger(&plugin, "planning");
        let all_agents = collect_phase_agents(&self.state.config);
        let project_name = self.state.project_name.clone();
        let tmux_project_name = self.state.tmux_project_name.clone();
        let base_branch = task
            .base_branch
            .clone()
            .unwrap_or_else(|| self.state.config.base_branch.clone());
        let worktree_dir = self.state.config.worktree_dir.clone();
        let branch_prefix = self.state.config.branch_prefix.clone();
        let copy_files = self.state.config.copy_files.clone();
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.init_script.clone()
        };
        let skip_init_scripts = self.state.flags.no_init_scripts;
        let skip_worktree = self.state.config.skip_worktree;
        let agent_hooks = self.state.config.agent_hooks;
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

        let auto_trust = self.state.config.auto_trust;
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
                &branch_prefix,
                copy_files,
                init_script,
                &plugin,
                &planning_agent_clone,
                &all_agents,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
                agent_ops.as_ref(),
                &referenced_tasks,
                skip_init_scripts,
                skip_worktree,
                agent_hooks,
                skill_cmd_launch.as_deref(),
            );

            match result {
                Ok((target, launched_with_prompt)) => {
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
                    // Skip the whole send-after-ready dance when the agent was
                    // launched with the opening message already in argv.
                    if launched_with_prompt {
                        // nothing to send
                    } else if let Some(target) = wait_for_agent_ready(
                        &tmux_ops,
                        &target,
                        Some(&planning_agent_clone),
                        auto_trust,
                    ) {
                        send_skill_and_prompt(
                            &tmux_ops,
                            &target,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content,
                            &planning_agent_clone,
                            &auto_dismiss,
                            false,
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
                &task.id,
                true,
            );
            // Non-collapsed twin for the argv path (see `spawn_send_to_agent`).
            let skill_cmd_launch = resolve_skill_command(
                &plugin,
                run_phase,
                &running_agent,
                &task_content,
                task.cycle,
                &task.id,
                false,
            );
            let prompt = resolve_prompt(&plugin, run_phase, &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, run_phase);
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                task.id.clone(),
                self.state.config.agent_hooks,
                self.state.config.auto_trust,
                session_name.clone(),
                task.agent.clone(),
                running_agent.clone(),
                agent_switch,
                skill_cmd,
                skill_cmd_launch,
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
            let skill_cmd = resolve_skill_command(
                &plugin,
                "review",
                &review_agent,
                &task_content,
                task.cycle,
                &task.id,
                true,
            );
            // Non-collapsed twin for the argv path (see `spawn_send_to_agent`).
            let skill_cmd_launch = resolve_skill_command(
                &plugin,
                "review",
                &review_agent,
                &task_content,
                task.cycle,
                &task.id,
                false,
            );
            let prompt = resolve_prompt(&plugin, "review", &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, "review");
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                task.id.clone(),
                self.state.config.agent_hooks,
                self.state.config.auto_trust,
                session_name.clone(),
                task.agent.clone(),
                review_agent.clone(),
                agent_switch,
                skill_cmd,
                skill_cmd_launch,
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
        let agent = task.agent.clone();
        task.session_name = None;
        task.worktree_path = None;

        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let git_ops = Arc::clone(&self.state.git_ops);
        let task_id_clone = task.id.clone();
        let project_path_clone = project_path.to_path_buf();
        let cleanup_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.cleanup_script.clone()
        };
        std::thread::spawn(move || {
            cleanup_task_resources(
                &task_id_clone,
                &agent,
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
            // Block research when dependencies are not satisfied
            if task.status == TaskStatus::Backlog && !db.deps_satisfied(&task) {
                self.state.warning_message = Some((
                    "Dependencies not in Review/Done — cannot start task".to_string(),
                    Instant::now(),
                ));
                return Ok(());
            }
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
        let base_branch = task
            .base_branch
            .clone()
            .unwrap_or_else(|| self.state.config.base_branch.clone());
        let worktree_dir = self.state.config.worktree_dir.clone();
        let branch_prefix = self.state.config.branch_prefix.clone();
        let copy_files = self.state.config.copy_files.clone();
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.init_script.clone()
        };
        let skip_init_scripts = self.state.flags.no_init_scripts;
        let skip_worktree = self.state.config.skip_worktree;
        let agent_hooks = self.state.config.agent_hooks;

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

        let auto_trust = self.state.config.auto_trust;
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
                &branch_prefix,
                copy_files,
                init_script,
                &plugin,
                &agent_name,
                &all_agents,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
                agent_ops.as_ref(),
                &[],
                skip_init_scripts,
                skip_worktree,
                agent_hooks,
                // Research keeps the send-after-ready path for now: its skill is
                // resolved later in the thread, and the launch lane is planning-only.
                None,
            );

            match result {
                Ok((target, launched_with_prompt)) => {
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
                        &task_id,
                        true,
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
                    // Skip the whole send-after-ready dance when the agent was
                    // launched with the opening message already in argv.
                    if launched_with_prompt {
                        // nothing to send
                    } else if let Some(target) =
                        wait_for_agent_ready(&tmux_ops, &target, Some(&agent_name), auto_trust)
                    {
                        send_skill_and_prompt(
                            &tmux_ops,
                            &target,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content,
                            &agent_name,
                            &auto_dismiss,
                            false,
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

    /// Build and open the dependency-graph overlay for the current project's tasks.
    fn show_dependency_graph(&mut self) -> Result<()> {
        let Some(db) = self.state.db.as_ref() else {
            return Ok(());
        };
        let tasks = db.get_all_tasks().unwrap_or_default();
        if tasks.is_empty() {
            self.state.warning_message = Some((
                "No tasks to show in the dependency view".to_string(),
                Instant::now(),
            ));
            return Ok(());
        }
        let graph = crate::tui::dep_graph::build_dep_graph(&tasks, |t| db.deps_satisfied(t));
        // Start the cursor on the first unblocked node if there is one.
        let selected = graph.nodes.iter().position(|n| n.unblocked).unwrap_or(0);
        self.state.dep_graph_popup = Some(DepGraphPopup {
            graph,
            selected,
            marked: HashSet::new(),
            scroll_levels: Cell::new(0),
            visible_levels: Cell::new(1),
        });
        Ok(())
    }

    /// Key handling for the dependency-graph overlay.
    fn handle_dep_graph_key(&mut self, key: crossterm::event::KeyEvent) -> Result<()> {
        let Some(popup) = self.state.dep_graph_popup.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.dep_graph_popup = None;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                Self::dep_graph_move_vertical(popup, 1);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                Self::dep_graph_move_vertical(popup, -1);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                Self::dep_graph_move_horizontal(popup, 1);
            }
            KeyCode::Char('h') | KeyCode::Left => {
                Self::dep_graph_move_horizontal(popup, -1);
            }
            KeyCode::Char(' ') => {
                // Toggle mark on the selected node, only if it is unblocked.
                if let Some(node) = popup.graph.nodes.get(popup.selected) {
                    if node.unblocked {
                        let id = node.task_id.clone();
                        if !popup.marked.remove(&id) {
                            popup.marked.insert(id);
                        }
                    }
                }
            }
            KeyCode::Char('a') => {
                // Mark all unblocked nodes.
                for id in popup.graph.unblocked_ids() {
                    popup.marked.insert(id);
                }
            }
            KeyCode::Char('c') => {
                popup.marked.clear();
            }
            KeyCode::Enter => {
                // Collect targets: marked nodes, or the selected node if nothing
                // is marked and it is unblocked.
                let mut targets: Vec<String> = popup
                    .graph
                    .nodes
                    .iter()
                    .filter(|n| n.unblocked && popup.marked.contains(&n.task_id))
                    .map(|n| n.task_id.clone())
                    .collect();
                if targets.is_empty() {
                    if let Some(node) = popup.graph.nodes.get(popup.selected) {
                        if node.unblocked {
                            targets.push(node.task_id.clone());
                        }
                    }
                }
                if targets.is_empty() {
                    self.state.warning_message = Some((
                        "No unblocked tasks selected — press Space to mark one".to_string(),
                        Instant::now(),
                    ));
                    return Ok(());
                }
                self.state.dep_graph_popup = None;
                self.batch_move_unblocked(targets)?;
            }
            _ => {}
        }
        Ok(())
    }

    /// Move the cursor up/down within the current level (column).
    fn dep_graph_move_vertical(popup: &mut DepGraphPopup, delta: i32) {
        let Some(node) = popup.graph.nodes.get(popup.selected) else {
            return;
        };
        let level = node.level;
        let Some(col) = popup.graph.levels.get(level) else {
            return;
        };
        let pos = col.iter().position(|&i| i == popup.selected).unwrap_or(0);
        let new_pos = (pos as i32 + delta).clamp(0, col.len() as i32 - 1) as usize;
        if let Some(&idx) = col.get(new_pos) {
            popup.selected = idx;
        }
    }

    /// Move the cursor left/right across levels, keeping a similar row position.
    fn dep_graph_move_horizontal(popup: &mut DepGraphPopup, delta: i32) {
        let Some(node) = popup.graph.nodes.get(popup.selected) else {
            return;
        };
        let level = node.level;
        let cur_pos = popup
            .graph
            .levels
            .get(level)
            .and_then(|col| col.iter().position(|&i| i == popup.selected))
            .unwrap_or(0);
        let new_level = (level as i32 + delta).clamp(0, popup.graph.level_count() as i32 - 1);
        if new_level < 0 {
            return;
        }
        let new_level = new_level as usize;
        if let Some(col) = popup.graph.levels.get(new_level) {
            if !col.is_empty() {
                let new_pos = cur_pos.min(col.len() - 1);
                popup.selected = col[new_pos];
            }
        }
        // Scrolling is handled by the draw pass, which re-clamps `scroll_levels`
        // each frame to keep the selected node on screen.
    }

    /// Enqueue unblocked tasks for serialized worktree setup, then kick off the first.
    fn batch_move_unblocked(&mut self, task_ids: Vec<String>) -> Result<()> {
        for id in task_ids {
            if !self.state.setup_queue.contains(&id) {
                self.state.setup_queue.push_back(id);
            }
        }
        self.try_start_next_queued_setup()
    }

    /// If no worktree setup is currently running, start the next queued batch task.
    /// Routes each task to research when its plugin supports it, else to planning.
    fn try_start_next_queued_setup(&mut self) -> Result<()> {
        // A setup is in progress — it will call back here when it completes.
        if self.state.setup_rx.is_some() {
            return Ok(());
        }
        while let Some(task_id) = self.state.setup_queue.pop_front() {
            // Re-validate: the task may have changed status or deps since queuing.
            let Some(db) = self.state.db.as_ref() else {
                continue;
            };
            let Some(task) = db.get_task(&task_id).ok().flatten() else {
                continue;
            };
            if task.status != TaskStatus::Backlog || !db.deps_satisfied(&task) {
                continue;
            }

            // Prefer research if the plugin defines a research/preresearch command.
            let plugin = self.load_task_plugin(&task);
            let has_research_cmd = plugin.as_ref().map_or(false, |p| {
                p.commands.research.is_some() || p.commands.preresearch.is_some()
            });

            if has_research_cmd {
                self.start_research(&task_id)?;
            } else {
                self.start_planning_from_backlog(&task_id)?;
            }

            // start_research / start_planning_from_backlog set setup_rx when they
            // spawn a background setup. If so, stop draining — the completion
            // handler will resume the queue. If not (no-op), keep draining.
            if self.state.setup_rx.is_some() {
                break;
            }
        }
        self.refresh_tasks()?;
        Ok(())
    }

    /// Move a Backlog task into Planning (the planning fallback for plugins with
    /// no research phase). Mirrors `move_task_right`'s Backlog→Planning branch.
    fn start_planning_from_backlog(&mut self, task_id: &str) -> Result<()> {
        if self.state.setup_rx.is_some() {
            return Ok(());
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
            return Ok(());
        }
        let handled = self.transition_to_planning(&mut task, &project_path)?;
        if !handled {
            task.status = TaskStatus::Planning;
            task.updated_at = chrono::Utc::now();
            task.escalation_note = None;
            if let Some(db) = &self.state.db {
                db.update_task(&task)?;
            }
            self.state.stuck_task_notified.remove(&task.id);
            self.state.stuck_task_idle_since.remove(&task.id);
            self.state.phase_status_cache.remove(&task.id);
        }
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

        // Block when dependencies are not satisfied
        if let Some(db) = &self.state.db {
            if !db.deps_satisfied(&task) {
                self.state.warning_message = Some((
                    "Dependencies not in Review/Done — cannot start task".to_string(),
                    Instant::now(),
                ));
                return Ok(());
            }
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
            &task.id,
            true,
        );
        // Verbatim variant for the launch lane — see the planning path above.
        let skill_cmd_launch = resolve_skill_command(
            &plugin,
            "running",
            &running_agent,
            &task_content,
            task.cycle,
            &task.id,
            false,
        );
        let prompt_trigger = resolve_prompt_trigger(&plugin, "running");
        let auto_dismiss = plugin
            .as_ref()
            .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
        let clear_context_on_advance = plugin
            .as_ref()
            .map_or(false, |p| p.clear_context_on_advance);

        // If a live session already exists (e.g. from a prior research/planning phase),
        // reuse it instead of creating a duplicate tmux window.
        let has_live_session = task_has_live_session(&task, self.state.tmux_ops.as_ref());
        if has_live_session {
            let target = task.session_name.clone().unwrap();
            let (agent_switch_agent, agent_switch) =
                needs_agent_switch(&self.state.config, &task, "running");
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                task.id.clone(),
                self.state.config.agent_hooks,
                self.state.config.auto_trust,
                target,
                task.agent.clone(),
                agent_switch_agent.clone(),
                agent_switch,
                skill_cmd,
                skill_cmd_launch,
                prompt,
                prompt_trigger,
                task_content,
                auto_dismiss,
                task.worktree_path.clone(),
                project_path.clone(),
                plugin,
            );
            task.agent = agent_switch_agent;
            task.status = TaskStatus::Running;
            task.updated_at = chrono::Utc::now();
            if let Some(db) = &self.state.db {
                db.update_task(&task)?;
            }
            self.refresh_tasks()?;
            return Ok(());
        }

        let project_name = self.state.project_name.clone();
        let tmux_project_name = self.state.tmux_project_name.clone();
        let base_branch = task
            .base_branch
            .clone()
            .unwrap_or_else(|| self.state.config.base_branch.clone());
        let worktree_dir = self.state.config.worktree_dir.clone();
        let branch_prefix = self.state.config.branch_prefix.clone();
        let copy_files = self.state.config.copy_files.clone();
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.init_script.clone()
        };
        let skip_init_scripts = self.state.flags.no_init_scripts;
        let skip_worktree = self.state.config.skip_worktree;
        let agent_hooks = self.state.config.agent_hooks;
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let git_ops = Arc::clone(&self.state.git_ops);
        let agent_ops = self.state.agent_registry.get(&running_agent);
        let task_id = task.id.clone();
        let task_title = task.title.clone();
        let running_agent_clone = running_agent.clone();

        let (tx, rx) = mpsc::channel();
        self.state.setup_rx = Some(rx);

        let auto_trust = self.state.config.auto_trust;
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
                &branch_prefix,
                copy_files,
                init_script,
                &plugin,
                &running_agent_clone,
                &all_agents,
                tmux_ops.as_ref(),
                git_ops.as_ref(),
                agent_ops.as_ref(),
                &[],
                skip_init_scripts,
                skip_worktree,
                agent_hooks,
                skill_cmd_launch.as_deref(),
            );

            match result {
                Ok((target, launched_with_prompt)) => {
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

                    // Skip the whole send-after-ready dance when the agent was
                    // launched with the opening message already in argv.
                    if launched_with_prompt {
                        // nothing to send
                    } else if let Some(target) = wait_for_agent_ready(
                        &tmux_ops,
                        &target,
                        Some(&running_agent_clone),
                        auto_trust,
                    ) {
                        send_skill_and_prompt(
                            &tmux_ops,
                            &target,
                            &skill_cmd,
                            &prompt,
                            &prompt_trigger,
                            &task_content,
                            &running_agent_clone,
                            &auto_dismiss,
                            clear_context_on_advance,
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
                        let hook_task_id = task.id.clone();
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
                                &hook_task_id,
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
                    &task.id,
                    true,
                );
                // An agent switch launches a new process, so it can take the message
                // in argv — and that path keeps the task's own line structure.
                // Only the send-after-ready fallback needs the flattened form.
                let skill_cmd_launch = resolve_skill_command(
                    &plugin,
                    "planning",
                    &planning_agent,
                    &task_content,
                    task.cycle,
                    &task.id,
                    false,
                );
                let prompt =
                    resolve_prompt(&plugin, "planning", &task_content, &task.id, task.cycle);
                let prompt_trigger = resolve_prompt_trigger(&plugin, "planning");

                if let Some(session_name) = &task.session_name {
                    let session_clone = session_name.clone();
                    let hook_task_id = task.id.clone();
                    let tmux_ops = Arc::clone(&self.state.tmux_ops);
                    let agent_registry = Arc::clone(&self.state.agent_registry);
                    let planning_agent_clone = planning_agent.clone();
                    let current_agent_clone = task.agent.clone();
                    let task_content_clone = task_content.clone();
                    let skill_cmd_launch = skill_cmd_launch.clone();
                    let auto_dismiss = plugin
                        .as_ref()
                        .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
                    let wt_path = task.worktree_path.clone();
                    let auto_trust = self.state.config.auto_trust;
                    std::thread::spawn(move || {
                        let agent_ops = agent_registry.get(&planning_agent_clone);
                        // Recover window if it was lost
                        ensure_window_or_recover(
                            tmux_ops.as_ref(),
                            &session_clone,
                            agent_ops.as_ref(),
                            wt_path.as_deref(),
                            &hook_task_id,
                        );
                        // An agent switch starts a *new process*, so it takes the
                        // opening message in argv exactly like a first launch —
                        // same act, just into an existing window. A same-agent
                        // advance cannot: that process is already running.
                        let mut delivered_at_launch = false;
                        if agent_switch {
                            let launch_text =
                                compose_launch_text(skill_cmd_launch.as_deref(), &prompt);
                            delivered_at_launch = agent::spec::can_launch_with_prompt(
                                agent_ops.prompt_injection(),
                                &launch_text,
                            );
                            let new_cmd =
                                agent_ops.build_interactive_command(if delivered_at_launch {
                                    &launch_text
                                } else {
                                    ""
                                });
                            switch_agent_in_tmux(
                                tmux_ops.as_ref(),
                                &session_clone,
                                &current_agent_clone,
                                &new_cmd,
                            );
                            if !delivered_at_launch {
                                // The *new* agent is what has to become ready.
                                let _ = wait_for_agent_ready(
                                    &tmux_ops,
                                    &session_clone,
                                    Some(&planning_agent_clone),
                                    auto_trust,
                                );
                            }
                        }
                        if !delivered_at_launch {
                            send_skill_and_prompt(
                                &tmux_ops,
                                &session_clone,
                                &skill_cmd,
                                &prompt,
                                &prompt_trigger,
                                &task_content_clone,
                                &planning_agent_clone,
                                &auto_dismiss,
                                false,
                            );
                        }
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
                        let hook_task_id = task.id.clone();
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
                                &hook_task_id,
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
        // `self.state.db` is re-borrowed per use site to avoid holding it across `&mut self`.
        let pending = match self.state.db.as_ref() {
            Some(db) => db.get_pending_transition_requests()?,
            None => return Ok(()),
        };
        if pending.is_empty() {
            return Ok(());
        }
        let instance_id = self.state.instance_id.clone();

        for req in pending {
            let claimed = self
                .state
                .db
                .as_ref()
                .map(|db| db.claim_transition_request(&req.id, &instance_id))
                .and_then(Result::ok)
                .unwrap_or(false);
            if !claimed {
                continue;
            }

            let result = self.execute_transition_request(&req);
            if let Some(db) = &self.state.db {
                let _ = match &result {
                    Ok(()) => db.mark_transition_processed(&req.id, None),
                    Err(e) => db.mark_transition_processed(&req.id, Some(&e.to_string())),
                };
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
        tracing::info!(
            task_id = %req.task_id,
            action = %req.action,
            "Processing transition request"
        );

        let Some(db) = &self.state.db else {
            anyhow::bail!("No project database");
        };
        let Some(project_path) = self.state.project_path.clone() else {
            anyhow::bail!("No project path");
        };

        let mut task = db
            .get_task(&req.task_id)?
            .ok_or_else(|| anyhow::anyhow!("Task not found: {}", req.task_id))?;

        // Block forward transitions when dependencies are not satisfied
        let is_forward = matches!(
            req.action.as_str(),
            "move_forward" | "move_to_planning" | "move_to_running" | "research"
        );
        if is_forward && task.status == TaskStatus::Backlog && !db.deps_satisfied(&task) {
            anyhow::bail!("Cannot advance task: dependencies not in Review/Done");
        }

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
            let skill_cmd = resolve_skill_command(
                &plugin,
                "review",
                &review_agent,
                &task_content,
                task.cycle,
                &task.id,
                true,
            );
            // Non-collapsed twin for the argv path (see `spawn_send_to_agent`).
            let skill_cmd_launch = resolve_skill_command(
                &plugin,
                "review",
                &review_agent,
                &task_content,
                task.cycle,
                &task.id,
                false,
            );
            let prompt = resolve_prompt(&plugin, "review", &task_content, &task.id, task.cycle);
            let prompt_trigger = resolve_prompt_trigger(&plugin, "review");
            let auto_dismiss = plugin
                .as_ref()
                .map_or_else(Vec::new, |p| p.auto_dismiss.clone());
            spawn_send_to_agent(
                Arc::clone(&self.state.tmux_ops),
                Arc::clone(&self.state.agent_registry),
                task.id.clone(),
                self.state.config.agent_hooks,
                self.state.config.auto_trust,
                session_name.clone(),
                task.agent.clone(),
                review_agent.clone(),
                agent_switch,
                skill_cmd,
                skill_cmd_launch,
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
        if is_orchestrator_live(self.state.tmux_ops.as_ref(), &orch_target) {
            let first_time = self.state.orchestrator_session.as_deref() != Some(&orch_target);
            self.state.orchestrator_session = Some(orch_target.clone());

            if first_time {
                // Cross-instance reattach: verify ready, replay phase events (deduped).
                self.state
                    .orchestrator_ready
                    .store(false, Ordering::Release);
                if let Some(ref db) = self.state.db {
                    run_orchestrator_catchup(
                        db,
                        &self.state.board.tasks,
                        self.state.project_path.as_deref(),
                    );
                }
                let tmux_ops = Arc::clone(&self.state.tmux_ops);
                let ready_flag = Arc::clone(&self.state.orchestrator_ready);
                let target = orch_target.clone();
                let auto_trust = self.state.config.auto_trust;
                std::thread::spawn(move || {
                    if wait_for_agent_ready(&tmux_ops, &target, Some("claude"), auto_trust)
                        .is_some()
                    {
                        ready_flag.store(true, Ordering::Release);
                    }
                });
            }

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
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            let (content, metrics) =
                capture_tmux_pane_snapshot(&orch_target, 500, self.state.tmux_ops.as_ref());
            let lines = parse_ansi_to_lines(&content);
            popup.set_content(content, lines);
            popup.metrics = metrics;
            let _ = self.state.input_sink.flush();
            self.state.shell_popup = Some(popup);
            return Ok(());
        }

        if !kill_windows_by_name(self.state.tmux_ops.as_ref(), &orch_target) {
            self.state.warning_message = Some((
                format!(
                    "Could not clear lingering `{}` window; try `tmux -L agtx kill-window -t {}`",
                    orch_target, orch_target,
                ),
                Instant::now(),
            ));
            return Ok(());
        }
        self.state.orchestrator_session = None;
        self.state
            .orchestrator_ready
            .store(false, Ordering::Release);

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
        // Escaped for the single-quoted word it becomes inside the orchestrator
        // command (`add-json … '<json>'`). Only reachable when the project path
        // contains an apostrophe. The *outer* `sh -c` layer is handled by
        // `single_quote` in create_window — pre-escaping for it here would
        // double-escape and break the command.
        let mcp_json_str = mcp_json.to_string().replace('\'', "'\"'\"'");

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
            false,
            // The orchestrator is not a task, so it reports no hook status.
            &[],
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
        let (content, metrics) =
            capture_tmux_pane_snapshot(&orch_target, 500, self.state.tmux_ops.as_ref());
        let lines = parse_ansi_to_lines(&content);
        popup.set_content(content, lines);
        popup.metrics = metrics;
        let _ = self.state.input_sink.flush();
        self.state.shell_popup = Some(popup);

        // Deploy orchestrate skill to project root so the agent can discover it
        deploy_skill(
            &project_path,
            "agtx-orchestrate",
            skills::ORCHESTRATE_SKILL,
            &default_agent,
        );

        if let Some(ref db) = self.state.db {
            run_orchestrator_catchup(
                db,
                &self.state.board.tasks,
                self.state.project_path.as_deref(),
            );
        }

        // Send the /agtx:orchestrate command once the agent is ready
        let skill_cmd = skills::transform_plugin_command("/agtx:orchestrate", &default_agent)
            .unwrap_or_else(|| "/agtx:orchestrate".to_string());
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let ready_flag = Arc::clone(&self.state.orchestrator_ready);
        let target = orch_target;
        let auto_trust = self.state.config.auto_trust;
        std::thread::spawn(move || {
            if let Some(ready_target) =
                wait_for_agent_ready(&tmux_ops, &target, Some(&default_agent), auto_trust)
            {
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
                    let project_path = self.state.project_path.as_deref().unwrap_or(Path::new("."));
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
                // Qualified, like the orchestrator popup's target: a bare window
                // name resolves inside whichever session the caller is bound to,
                // which is not necessarily this project's. See `pane_target`.
                let target = pane_target(&self.state.tmux_project_name, window_name);
                let mut popup = ShellPopup::new(task.title.clone(), target.clone());
                popup.task_id = Some(task_id);
                popup.escalation_note = escalation_note;

                // Resize tmux window to match popup dimensions (uses same constants as draw_shell_popup)
                if let Ok((_term_width, term_height)) = crossterm::terminal::size() {
                    let pane_width = SHELL_POPUP_CONTENT_WIDTH;
                    let popup_height =
                        (term_height as u32 * SHELL_POPUP_HEIGHT_PERCENT as u32 / 100) as u16;
                    let pane_height = popup_height.saturating_sub(4); // -4 for borders + header/footer

                    let _ = self
                        .state
                        .tmux_ops
                        .resize_window(&target, pane_width, pane_height);
                    popup.last_pane_size = Some((pane_width, pane_height));
                    // Give TUI apps (OpenCode, Gemini Ink) time to re-render after resize
                    std::thread::sleep(std::time::Duration::from_millis(200));
                }

                // Capture initial content *and* the pane metrics, so the first
                // keypress already knows whether this pane has tmux scrollback.
                // Leaving them to the first refresh left a window in which
                // `has_scrollback()` fell back to its `true` default and the
                // scroll keys moved a buffer that could not move.
                let (content, metrics) =
                    capture_tmux_pane_snapshot(&target, 500, self.state.tmux_ops.as_ref());
                let lines = parse_ansi_to_lines(&content);
                popup.set_content(content, lines);
                popup.metrics = metrics;

                // A popup opening changes the target; nothing queued for the
                // previous one may follow it here.
                let _ = self.state.input_sink.flush();
                self.state.shell_popup = Some(popup);
            }
        }
        Ok(())
    }

    fn open_selected_task_fullscreen(&mut self) -> Result<()> {
        self.open_selected_task()?;
        if let Some(popup) = self.state.shell_popup.as_mut() {
            popup.fullscreen = true;
            if let Ok((width, height)) = crossterm::terminal::size() {
                let pane_size = (width.saturating_sub(2), height.saturating_sub(4));
                let _ =
                    self.state
                        .tmux_ops
                        .resize_window(&popup.window_name, pane_size.0, pane_size.1);
                popup.last_pane_size = Some(pane_size);
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
            // Refresh dependency satisfaction cache for backlog tasks with references
            self.state.deps_satisfied_cache.clear();
            for task in &self.state.board.tasks {
                if task.referenced_tasks.is_some() {
                    self.state
                        .deps_satisfied_cache
                        .insert(task.id.clone(), db.deps_satisfied(task));
                }
            }
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
        if !is_orchestrator_live(self.state.tmux_ops.as_ref(), &orch_target) {
            self.state.orchestrator_session = None;
            self.state
                .orchestrator_ready
                .store(false, Ordering::Release);
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
            return;
        }

        let now = Instant::now();
        const CACHE_TTL: std::time::Duration = PHASE_STATUS_CACHE_TTL;

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
            return;
        }

        let project_path = self.state.project_path.clone();
        let tmux_ops = Arc::clone(&self.state.tmux_ops);
        let input_sink = Arc::clone(&self.state.input_sink);
        let auto_trust = self.state.config.auto_trust;

        let (tx, rx) = mpsc::channel();
        self.state.session_refresh_rx = Some(rx);

        std::thread::spawn(move || {
            // Every window on the server, once, rather than a `list-windows`
            // process per task. `window_exists` is a membership test, and asking
            // tmux N times for the same answer was the largest remaining
            // per-task subprocess cost on the board.
            let live_windows = live_window_targets(tmux_ops.as_ref());

            let mut plugin_cache: HashMap<Option<String>, Option<WorkflowPlugin>> = HashMap::new();
            let mut statuses = Vec::new();
            let now_secs = chrono::Utc::now().timestamp();

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
                //
                // Answered from one listing taken for the whole pass: this used to
                // be a `list-windows` **process per task**, on a 2-second timer.
                let window_gone = window_is_gone(session_name.as_deref(), live_windows.as_ref());
                let phase_status = if window_gone && phase_status == PhaseStatus::Working {
                    PhaseStatus::Exited
                } else {
                    phase_status
                };

                // The agent's own report of what it is doing, when it writes one.
                // Authoritative over the pane heuristic below.
                let hook_status = if phase_status == PhaseStatus::Working && !window_gone {
                    worktree_path
                        .as_ref()
                        .and_then(|wt| hook_status::read_status(Path::new(wt), &task_id, now_secs))
                } else {
                    None
                };

                // Interactive prompts that block an agent are answered here as well
                // as during startup, because `wait_for_agent_ready` gives up after
                // ~60s and a slow agent can render its first-launch dialog later
                // than that — observed in an emulated swebench container, where
                // Claude showed the bypass warning well after the readiness window
                // had closed and nothing was left watching for it.
                // Only a *fresh `Working`* report proves the pane is dialog-free: the
                // agent is mid-turn, so it is past whatever gated its startup.
                // `read_status` ages out a stale `Working`, but a `waiting`/`ended`
                // record never expires (`hook_status.rs:102`) — so a leftover file
                // from an earlier phase used to suppress this capture entirely, and a
                // dialog rendered after a relaunch was never seen. That is exactly the
                // resume path.
                let hook_says_working = matches!(
                    hook_status.as_ref().map(|h| h.state),
                    Some(hook_status::HookState::Working)
                );
                let mut awaiting_trust: Option<String> = None;
                // One capture for this pass, shared by the dialog scan below and
                // the content hash further down. They ask different questions of
                // the same bytes, and reading the pane twice was a second `tmux`
                // process per task for every agent that reports no hook status —
                // which is the case the hash exists for in the first place.
                // `hook_says_working` implies a hook status exists, so this one
                // condition is exactly the union of the two below.
                let pane_text: Option<String> =
                    if phase_status == PhaseStatus::Working && !window_gone && !hook_says_working {
                        session_name.as_ref().and_then(|sn| {
                            capture_pane_text(sn, input_sink.as_ref(), tmux_ops.as_ref())
                        })
                    } else {
                        None
                    };
                if phase_status == PhaseStatus::Working && !window_gone && !hook_says_working {
                    if let Some(sn) = session_name.as_ref() {
                        if let Some(content) = pane_text.as_deref() {
                            // Detected whether or not it is answered: with
                            // `auto_trust` off this is what turns the card
                            // `Blocked`, so the user knows a decision is theirs to
                            // make rather than watching a task sit at "working".
                            if !auto_trust {
                                awaiting_trust = visible_security_dialog(Some(&agent), content)
                                    .map(str::to_string);
                            }
                            // A fresh state each poll: the attempt cap guards a
                            // single startup burst, whereas here the 2s cadence
                            // between polls is itself the pacing.
                            let mut st = LaunchDialogState::default();
                            dismiss_launch_dialog(
                                &tmux_ops,
                                sn,
                                Some(&agent),
                                content,
                                &mut st,
                                auto_trust,
                            );

                            // Mid-session approval prompts (Codex's "allow this
                            // MCP server to run tool") — a workaround for an
                            // interactive prompt, not part of status detection.
                            // Matched against this agent's own dialogs only: the
                            // refresh loop knows what runs in the pane, unlike
                            // `wait_for_agent_ready`.
                            answer_session_dialogs(&tmux_ops, sn, &agent, content);
                        }
                    }
                }

                // Pane hash is the fallback for agents that report nothing.
                // Skipping it when a hook status exists also drops one
                // `capture-pane` subprocess per task per refresh.
                let content_hash = if phase_status == PhaseStatus::Working
                    && !window_gone
                    && hook_status.is_none()
                {
                    pane_text.as_ref().map(|content| {
                        use std::hash::{Hash, Hasher};
                        let mut hasher = std::collections::hash_map::DefaultHasher::new();
                        content.hash(&mut hasher);
                        hasher.finish()
                    })
                } else {
                    None
                };

                statuses.push(SessionTaskStatus {
                    task_id,
                    phase_status,
                    content_hash,
                    hook_status,
                    awaiting_trust,
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

            // A trust prompt outranks every liveness signal below it. The agent is
            // alive and idle by any pane or hook measure, but it is waiting on a
            // person — showing that as `Working` or `Idle` is what left users
            // watching a task that would never move.
            if let Some(ref dialog) = task_status.awaiting_trust {
                if phase == PhaseStatus::Working || phase == PhaseStatus::Idle {
                    phase = PhaseStatus::Blocked;
                    self.state.pane_content_hashes.remove(&task_status.task_id);
                    self.state.trust_blocked.insert(task_status.task_id.clone());
                    self.state.blocked_reasons.insert(
                        task_status.task_id.clone(),
                        trust_blocked_reason(&task_status.agent, dialog),
                    );
                }
            } else {
                self.state.trust_blocked.remove(&task_status.task_id);
            }

            if phase == PhaseStatus::Working {
                match task_status.hook_status.as_ref().map(|h| h.state) {
                    // The agent told us what it is doing — believe it, and drop
                    // the pane history so a later fallback starts clean.
                    Some(HookState::Working) => {
                        self.state.pane_content_hashes.remove(&task_status.task_id);
                    }
                    Some(HookState::Blocked) => {
                        phase = PhaseStatus::Blocked;
                        self.state.pane_content_hashes.remove(&task_status.task_id);
                    }
                    // Turn ended with no artifact yet: what Idle has always meant.
                    Some(HookState::Waiting) => {
                        phase = PhaseStatus::Idle;
                        self.state.pane_content_hashes.remove(&task_status.task_id);
                    }
                    Some(HookState::Ended) => {
                        phase = PhaseStatus::Exited;
                        self.state.pane_content_hashes.remove(&task_status.task_id);
                    }
                    // No hook report: unchanged 15s pane-hash heuristic.
                    None => {
                        if let Some(hash) = task_status.content_hash {
                            let entry = self
                                .state
                                .pane_content_hashes
                                .entry(task_status.task_id.clone())
                                .or_insert((hash, now));
                            if entry.0 != hash {
                                *entry = (hash, now);
                            } else if now.duration_since(entry.1)
                                >= std::time::Duration::from_secs(15)
                            {
                                phase = PhaseStatus::Idle;
                            }
                        }
                    }
                }
            } else if phase == PhaseStatus::Ready {
                self.state.pane_content_hashes.remove(&task_status.task_id);
            } else if phase == PhaseStatus::Exited {
                self.state.pane_content_hashes.remove(&task_status.task_id);
            }

            // Keep the reason text alongside the status so the card and the
            // orchestrator notification can name what the agent is waiting for.
            match (phase, task_status.hook_status.as_ref()) {
                // A trust block already wrote its own reason, and no agent hook
                // knows about it — the agent has not started reporting yet.
                _ if task_status.awaiting_trust.is_some() => {}
                (PhaseStatus::Blocked, Some(h)) => {
                    if let Some(msg) = h.message.clone() {
                        self.state
                            .blocked_reasons
                            .insert(task_status.task_id.clone(), msg);
                    }
                }
                _ => {
                    self.state.blocked_reasons.remove(&task_status.task_id);
                }
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
                                            false,
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
            // A task parked on a trust prompt is deliberately excluded. The
            // orchestrator's remedies are `send_to_task` (a nudge) and
            // `escalate_to_user`, and the nudge would be typed **into the dialog** —
            // the exact corruption `LAUNCH_DIALOGS` scoping exists to prevent. It is
            // also not a task the orchestrator can unstick: only the user can answer.
            // The `Blocked` badge and its reason already say so on the board.
            let trust_blocked = self.state.trust_blocked.contains(&task_status.task_id);
            if matches!(
                task_status.status,
                TaskStatus::Planning | TaskStatus::Running
            ) && matches!(phase, PhaseStatus::Idle | PhaseStatus::Blocked)
                && !trust_blocked
                && self.state.orchestrator_session.is_some()
                && should_send_stuck_notification(task_plugin)
            {
                let stuck_key = format!("{}:{}", task_status.task_id, task_status.status.as_str());
                if !self.state.stuck_task_notified.contains(&stuck_key) {
                    // Blocked is agent-reported: the agent has *said* it is
                    // waiting on a human, so there is nothing to wait out. Idle is
                    // still a guess from pane output, so it keeps its 1m settle.
                    let blocked = phase == PhaseStatus::Blocked;
                    let idle_since = self
                        .state
                        .stuck_task_idle_since
                        .entry(task_status.task_id.clone())
                        .or_insert(now);

                    if blocked
                        || now.duration_since(*idle_since) >= std::time::Duration::from_secs(60)
                    {
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
                            let reason = self.state.blocked_reasons.get(&task_status.task_id);
                            let notif = crate::db::Notification::new(match (blocked, reason) {
                                (true, Some(r)) => format!(
                                    "Task \"{}\" ({}) is blocked in phase {} waiting for: {}",
                                    task_title,
                                    short_id,
                                    task_status.status.as_str(),
                                    r
                                ),
                                (true, None) => format!(
                                    "Task \"{}\" ({}) is blocked in phase {} waiting for user input",
                                    task_title,
                                    short_id,
                                    task_status.status.as_str()
                                ),
                                _ => format!(
                                    "Task \"{}\" ({}) has been idle for 1m in phase: {}",
                                    task_title,
                                    short_id,
                                    task_status.status.as_str()
                                ),
                            });
                            let _ = db.create_notification(&notif);
                        }
                    }
                }
            } else if !matches!(phase, PhaseStatus::Idle | PhaseStatus::Blocked) {
                // Task is working again — reset the idle-since timer
                self.state
                    .stuck_task_idle_since
                    .remove(&task_status.task_id);
            }
        }
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
        // The control client attaches to a session, and the old project's may be
        // killed later. Delivery does not depend on which one it is — commands
        // carry their own target — but the next *connect* does.
        self.state
            .input_sink
            .set_session(&self.state.tmux_project_name);

        // Clear per-task caches from previous project
        self.state.merge_conflict_checked.clear();
        self.state.stuck_task_notified.clear();
        self.state.stuck_task_idle_since.clear();

        // Reload config for the new project so per-phase agent overrides are respected
        let global_config = GlobalConfig::load().unwrap_or_default();
        let project_config = ProjectConfig::load(&project_path).unwrap_or_default();
        self.state.config = MergedConfig::merge(&global_config, &project_config);
        self.state.cached_plugin = Some(load_plugin_if_configured(
            &self.state.config,
            Some(&project_path),
        ));

        // Reload tasks for new project
        self.refresh_tasks()?;

        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        // Deliver what is still queued and stop the broker before the process
        // goes away — the last characters typed into a pane were typed on
        // purpose. Bounded by the queue depth, so quitting stays instant.
        self.state.input_sink.shutdown();
        match self.terminal.backend_mut() {
            AppBackend::Crossterm(backend) => {
                let _ = disable_raw_mode();
                let _ = execute!(backend, LeaveAlternateScreen, DisableBracketedPaste);
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

/// Returns true if the task already has a tmux window that is currently alive.
/// Used to decide whether to reuse an existing session instead of creating a new one.
fn task_has_live_session(task: &Task, tmux_ops: &dyn TmuxOperations) -> bool {
    task.session_name
        .as_ref()
        .map_or(false, |s| tmux_ops.window_exists(s).unwrap_or(false))
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

    tmux_ops.create_window(
        session,
        window,
        worktree_path,
        Some(resume_cmd),
        true,
        &agtx_task_env(&task.id, worktree_path),
    )?;

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

    tracing::info!(
        script = script,
        worktree = %worktree_path.display(),
        "Executing cleanup_script"
    );

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
                .and_then(|b| b.rsplit_once('/').map(|(_, s)| s))
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
    agent: &str,
    branch_name: &Option<String>,
    session_name: &Option<String>,
    worktree_path: &Option<String>,
    cleanup_script: Option<&str>,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    // Drop this worktree from the agent's trust store, before it is removed —
    // `forget` resolves the path, which needs the directory to still exist.
    //
    // Only stores agtx seeds are pruned; a record agtx did not write is not
    // agtx's to delete. Without this the lists grow one dead entry per task
    // forever: the machine this was written on had 19 in antigravity's, 27 in
    // codex's and ~20 in claude's, nearly all pointing at directories that were
    // long gone.
    if let (Some(worktree), Some(home)) = (worktree_path, agent_trust_home()) {
        let _ = agent::trust::forget(agent, Path::new(worktree), &home);
    }

    // Archive artifacts before removing worktree
    if let Some(worktree) = worktree_path {
        let artifacts_dir = Path::new(worktree).join(".agtx");
        if artifacts_dir.exists() {
            let slug = branch_name
                .as_deref()
                .and_then(|b| b.rsplit_once('/').map(|(_, s)| s))
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
    branch_prefix: &str,
    copy_files: Option<String>,
    init_script: Option<String>,
    plugin: &Option<WorkflowPlugin>,
    agent_name: &str,
    all_phase_agents: &[String],
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
    agent_ops: &dyn AgentOperations,
    referenced_tasks: &[ReferencedTaskInfo],
    skip_init_scripts: bool,
    skip_worktree: bool,
    agent_hooks: bool,
    skill_cmd: Option<&str>,
) -> Result<(String, bool)> {
    let unique_slug = generate_task_slug(&task.id, &task.title);
    let window_name = format!("task-{}", unique_slug);
    let target = format!("{}:{}", tmux_project_name, window_name);

    // When skip_worktree is set, use the project root directly instead of creating a git worktree.
    // Useful for isolated environments (e.g. Docker) where the repo is already the working copy.
    let worktree_path_str = if skip_worktree {
        project_path.to_string_lossy().to_string()
    } else {
        // Create git worktree from the configured base branch
        match git_ops.create_worktree(
            project_path,
            &unique_slug,
            base_branch,
            worktree_dir,
            branch_prefix,
        ) {
            Ok(path) => path,
            Err(e) => {
                eprintln!("Failed to create worktree: {}", e);
                project_path
                    .join(worktree_dir)
                    .join(&unique_slug)
                    .to_string_lossy()
                    .to_string()
            }
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
    write_skills_to_worktree(
        &worktree_path_str,
        project_path,
        plugin,
        &agent_refs,
        agent_hooks,
    );

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
    if !skip_init_scripts {
        if let Some(ref p) = plugin {
            if let Some(ref script) = p.init_script {
                let script = script.replace("{agent}", agent_name);
                tracing::info!(
                    script = %script,
                    agent = agent_name,
                    worktree = %worktree_path_str,
                    "Executing plugin init_script"
                );
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
    }

    // Hand the opening message to the agent process when it can take one. That
    // removes the send-after-ready race entirely for the first message: there is
    // no window in which a keystroke can be dropped, and nothing to poll for.
    // Agents without a verified launch form — or a task too large for argv —
    // fall back to the historical path (launch bare, wait for readiness, type).
    let launch_text = compose_launch_text(skill_cmd, prompt);
    let launched_with_prompt =
        agent::spec::can_launch_with_prompt(agent_ops.prompt_injection(), &launch_text);
    let agent_cmd = if launched_with_prompt {
        agent_ops.build_interactive_command(&launch_text)
    } else {
        let has_skill_support = resolve_skill_command(
            plugin, "planning", agent_name, "", task.cycle, &task.id, true,
        )
        .is_some();
        if has_skill_support {
            agent_ops.build_interactive_command("")
        } else {
            agent_ops.build_interactive_command(prompt)
        }
    };

    // Ensure project tmux session exists
    ensure_project_tmux_session(tmux_project_name, project_path, tmux_ops);

    tracing::info!(
        task_id = %task.id,
        agent = agent_name,
        worktree = %worktree_path_str,
        "Agent session spawned"
    );

    tmux_ops.create_window(
        tmux_project_name,
        &window_name,
        &worktree_path_str,
        Some(agent_cmd),
        true,
        &agtx_task_env(&task.id, &worktree_path_str),
    )?;

    task.session_name = Some(target.clone());
    task.worktree_path = Some(worktree_path_str);
    task.branch_name = Some(format!("{}/{}", branch_prefix, unique_slug));

    Ok((target, launched_with_prompt))
}

/// Delete task resources: kill tmux window, run cleanup script, remove worktree, delete branch
fn delete_task_resources(
    task: &Task,
    cleanup_script: Option<&str>,
    project_path: &Path,
    tmux_ops: &dyn TmuxOperations,
    git_ops: &dyn GitOperations,
) {
    // Same prune as the Done path: a deleted task's worktree is just as gone.
    if let (Some(worktree), Some(home)) = (&task.worktree_path, agent_trust_home()) {
        let _ = agent::trust::forget(&task.agent, Path::new(worktree), &home);
    }

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
/// Clamp a horizontal scroll offset (in level-columns) so the selected level is
/// visible within a viewport `visible` columns wide. Returns the new offset.
///
/// - If the selection is left of the window, scroll left to it.
/// - If it is at/past the right edge, scroll right so it sits on the last
///   visible column.
/// - The offset never exceeds what keeps the last column flush with the right
///   edge (no blank trailing space when the graph is wider than the viewport).
fn clamp_scroll_to_selected(
    scroll: usize,
    sel_level: usize,
    visible: usize,
    level_count: usize,
) -> usize {
    let visible = visible.max(1);
    let mut start = scroll.min(level_count.saturating_sub(1));
    if sel_level < start {
        start = sel_level;
    } else if sel_level >= start + visible {
        start = sel_level + 1 - visible;
    }
    // Don't scroll past the point where the final column is at the right edge.
    let max_start = level_count.saturating_sub(visible);
    start.min(max_start)
}

/// Truncate a string to at most `max` characters, appending an ellipsis when
/// it was shortened. Operates on chars so it is UTF-8 safe.
fn truncate_str(s: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    let char_count = s.chars().count();
    if char_count <= max {
        return s.to_string();
    }
    if max == 1 {
        return "\u{2026}".to_string();
    }
    let taken: String = s.chars().take(max - 1).collect();
    format!("{taken}\u{2026}")
}

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

/// Capture a pane's content with history (ANSI escape sequences included) and
/// the metrics describing it, in one pass.
fn capture_tmux_pane_snapshot(
    window_name: &str,
    history_lines: i32,
    tmux_ops: &dyn TmuxOperations,
) -> (Vec<u8>, Option<crate::tmux::PaneMetrics>) {
    let content = tmux_ops.capture_pane_with_history(window_name, history_lines);

    // Get the cursor position and pane height to know where the "real" content ends
    // Lines below the cursor are unused pane buffer space
    let metrics = tmux_ops.pane_metrics(window_name);

    trim_pane_snapshot(crate::tmux::PaneSnapshot { content, metrics })
}

/// Cut the unused pane buffer below the cursor off a capture.
///
/// Shared by both capture paths so they cannot disagree about what the popup is
/// shown — the whole point of the control-mode path is to be indistinguishable
/// from the subprocess one apart from its cost.
fn trim_pane_snapshot(
    snapshot: crate::tmux::PaneSnapshot,
) -> (Vec<u8>, Option<crate::tmux::PaneMetrics>) {
    let trim_info = snapshot.metrics.map(|m| m.trim_bounds());
    (
        shell_popup::trim_content_to_cursor(snapshot.content, trim_info),
        snapshot.metrics,
    )
}

/// Capture a pane for the popup: over the input broker's control connection when
/// there is one, otherwise the two `tmux` processes this has always cost.
///
/// The broker is asked first because it already holds a live `tmux -C` client
/// for keystrokes, and reusing it avoids the process startup that dominated the
/// delay between typing into a task pane and seeing the character appear. It
/// also flushes buffered keystrokes ahead of the capture, so a frame can never
/// be missing text the user has already typed.
fn capture_pane_for_popup(
    window_name: &str,
    history_lines: i32,
    input_sink: &dyn PaneInputSink,
    tmux_ops: &dyn TmuxOperations,
) -> (Vec<u8>, Option<crate::tmux::PaneMetrics>) {
    match input_sink.capture(window_name, crate::tmux::CaptureSpec::popup(history_lines)) {
        Some(snapshot) => trim_pane_snapshot(snapshot),
        None => capture_tmux_pane_snapshot(window_name, history_lines, tmux_ops),
    }
}

/// Windows that currently exist, as a set of `session:window` targets, or `None`
/// when tmux could not be asked.
///
/// The `Option` is the whole point and must not be flattened to an empty set.
/// The per-task check this replaces failed *safe* —
/// `!window_exists(sn).unwrap_or(true)` leaves a task alone when the check
/// errors — whereas an empty set reads as "every window is gone" and would mark
/// every running task `Exited` on one transient tmux hiccup. That is a visible,
/// wrong status change on the whole board, so a failed listing must mean
/// "unknown", not "none".
fn live_window_targets(tmux_ops: &dyn TmuxOperations) -> Option<std::collections::HashSet<String>> {
    tmux_ops
        .list_window_targets()
        .ok()
        .map(|targets| targets.into_iter().collect())
}

/// Has this task's tmux window gone away?
///
/// Split out because its **failure direction** is the whole subtlety: `None` for
/// the listing means tmux could not be asked, and must read as "leave the task
/// alone", never as "every window is gone". Getting that backwards marks a whole
/// board `Exited` on one transient tmux hiccup.
fn window_is_gone(session: Option<&str>, live: Option<&std::collections::HashSet<String>>) -> bool {
    match (session, live) {
        (Some(sn), Some(live)) => !live.contains(sn),
        _ => false,
    }
}

/// Read a pane as **plain text**, over the input connection when there is one.
///
/// The status refresh's counterpart to [`capture_pane_for_popup`]: no escapes,
/// no scrollback, no geometry — byte-identical to
/// [`TmuxOperations::capture_pane`], which is what the dialog matcher and the
/// content hash have always been fed.
///
/// Worth routing because this is the last per-task subprocess left on the status
/// refresh's timer: the fallback is a `tmux` process *per task*, and it scales
/// with the size of the board.
fn capture_pane_text(
    target: &str,
    input_sink: &dyn PaneInputSink,
    tmux_ops: &dyn TmuxOperations,
) -> Option<String> {
    if let Some(snapshot) = input_sink.capture(target, crate::tmux::CaptureSpec::text()) {
        return Some(String::from_utf8_lossy(&snapshot.content).into_owned());
    }
    tmux_ops.capture_pane(target).ok()
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

    // Create PR (use base_branch for stacked PRs)
    git_provider_ops.create_pr(
        project_path,
        pr_title,
        pr_body,
        task.branch_name.as_deref().unwrap_or(""),
        task.base_branch.clone(),
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

/// Translate a popup keystroke into a request for the pane input broker.
///
/// The split between [`PaneInput::Text`] and [`PaneInput::Key`] is the whole
/// point of the type: text goes out with `send-keys -l`, which suppresses tmux's
/// key-name lookup, and a key goes out without it, which is what makes `Enter`
/// an Enter.
///
/// An unmodified character is therefore **text**, where it used to be a key.
/// That is a fix, not just a reclassification: `send-keys -t x ";"` never
/// reached the pane at all, because a standalone semicolon is how tmux separates
/// commands — it was parsed as one and the keystroke vanished. The same lookup
/// is why a batched run of characters could not have been sent as a key.
fn popup_key_input(target: &str, key: crossterm::event::KeyEvent) -> Option<PaneInput> {
    use crossterm::event::KeyModifiers;
    let has_alt = key.modifiers.contains(KeyModifiers::ALT);
    let has_ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

    // Shift is already reflected in the character crossterm reports, so it is
    // not a modifier prefix here — `C-A` and `C-a` are different keys to tmux,
    // but `A` alone is just the character.
    if let KeyCode::Char(c) = key.code {
        if !has_ctrl && !has_alt {
            return Some(PaneInput::Text {
                target: target.to_string(),
                text: c.to_string(),
            });
        }
    }

    let base = match key.code {
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
        _ => return None,
    };

    let key_str = match (has_ctrl, has_alt) {
        (true, true) => format!("C-M-{}", base),
        (true, false) => format!("C-{}", base),
        (false, true) => format!("M-{}", base),
        (false, false) => base,
    };

    Some(PaneInput::Key {
        target: target.to_string(),
        key: key_str,
    })
}

/// Hand one request to the broker, surfacing the two ways that can fail.
///
/// Takes the sink and the warning slot rather than `&mut App` on purpose: it is
/// called with the popup mutably borrowed, and these are disjoint fields.
///
/// A full queue is **not** retried synchronously. Sending it now would put this
/// key ahead of everything already queued, and a keystroke arriving out of order
/// is worse than one that visibly did not arrive — so the queued prefix is kept
/// and the user is told.
fn forward_pane_input(
    sink: &dyn PaneInputSink,
    warning: &mut Option<(String, Instant)>,
    input: PaneInput,
) {
    match sink.send(input) {
        Ok(()) => {}
        Err(InputError::QueueFull) => {
            tracing::warn!("pane input queue full; keystroke dropped");
            *warning = Some((
                "Input is backing up; a keystroke was dropped rather than sent out of order."
                    .to_string(),
                Instant::now(),
            ));
        }
        Err(InputError::Disconnected) => {
            tracing::error!("pane input broker stopped");
            *warning = Some((
                "Pane input is not running; restart agtx to type into task panes.".to_string(),
                Instant::now(),
            ));
        }
        // `send` is an enqueue and cannot time out. The arm exists because the
        // variant shares an error type with `flush_sync`, which can.
        Err(InputError::Timeout) => {
            tracing::debug!("pane input enqueue reported a timeout");
        }
    }
}

/// Deliver everything queued and wait for tmux to have executed it.
///
/// Used before agtx does a synchronous tmux operation of its own on this thread:
/// that travels a different socket, so without the wait it can overtake keys the
/// user typed a moment earlier. The wait is bounded — see `FLUSH_SYNC_TIMEOUT` —
/// and reaching the bound means the ordering was not achieved, which is worth a
/// word to the user rather than a silent shrug.
fn flush_pane_input_sync(sink: &dyn PaneInputSink, warning: &mut Option<(String, Instant)>) {
    match sink.flush_sync() {
        Ok(()) => {}
        Err(InputError::Timeout) => {
            tracing::warn!("pane input did not drain before a synchronous tmux operation");
            *warning = Some((
                "tmux is slow to accept input — your last keystrokes may arrive late.".to_string(),
                Instant::now(),
            ));
        }
        Err(e) => tracing::debug!(error = %e, "acknowledged flush failed"),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum PopupScroll {
    Up(i32),
    Down(i32),
    Bottom,
}

/// The scroll chords, in one place.
///
/// Every scrollable surface in agtx reads them from here — the task pane and
/// the `?` overlay — so a user who learns `C-d` once does not find it inert
/// somewhere else, and the two cannot drift apart.
fn scroll_action_for(key: crossterm::event::KeyEvent) -> Option<PopupScroll> {
    let ctrl = key
        .modifiers
        .contains(crossterm::event::KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Char('p') | KeyCode::Up if ctrl => Some(PopupScroll::Up(5)),
        KeyCode::Char('n') | KeyCode::Down if ctrl => Some(PopupScroll::Down(5)),
        KeyCode::Char('u') if ctrl => Some(PopupScroll::Up(20)),
        KeyCode::PageUp => Some(PopupScroll::Up(20)),
        KeyCode::Char('d') if ctrl => Some(PopupScroll::Down(20)),
        KeyCode::PageDown => Some(PopupScroll::Down(20)),
        KeyCode::Char('g') if ctrl => Some(PopupScroll::Bottom),
        _ => None,
    }
}

/// Apply one logical popup scroll action to whichever component owns history.
///
/// A pane in the alternate screen keeps no tmux scrollback (`history_size == 0`),
/// so the session's history is the agent's, not ours. Rather than move a
/// one-screen buffer by zero lines and print a line number to match, agtx sends
/// the agent a key that scrolls *its* view.
///
/// **Never `Up`/`Down`.** Measured against Claude
/// Code 2.1.251: in its detailed transcript view (`ctrl+o`) all four scroll, but
/// in the main view `Up` recalls a previous prompt **into the composer** —
/// overwriting whatever the user had typed — while `PageUp` is inert. A scroll
/// key that silently rewrites the user's half-typed message is worse than one
/// that does nothing, so `C-n/p` use the safe Page Down/Up translation.
///
/// The chord is **translated, never passed through**, for the same reason:
/// forwarding a raw `C-d` would be an EOF that can end the session, and `C-u`
/// would kill the line in the composer.
fn handle_popup_scroll(
    popup: &mut ShellPopup,
    sink: &dyn PaneInputSink,
    warning: &mut Option<(String, Instant)>,
    target: &str,
    action: PopupScroll,
) {
    if popup.has_scrollback() {
        match action {
            PopupScroll::Up(lines) => popup.scroll_up(lines),
            PopupScroll::Down(lines) => popup.scroll_down(lines),
            PopupScroll::Bottom => popup.scroll_to_bottom(),
        }
        return;
    }

    let key = match action {
        PopupScroll::Up(_) => "PageUp",
        PopupScroll::Down(_) => "PageDown",
        PopupScroll::Bottom => "End",
    };
    forward_pane_input(
        sink,
        warning,
        PaneInput::Key {
            target: target.to_string(),
            key: key.to_string(),
        },
    );
}

/// Address a window the way every tmux command in agtx should: `session:window`.
///
/// A **bare** window name is resolved inside whichever session the issuing
/// client is bound to — the attached session for a control client, the
/// most-recently-used one for a subprocess. Neither is reliably this project's
/// after a project switch, and `orchestrator` is a window name every project
/// session has, so a bare target could deliver a keystroke to the wrong
/// project's agent. Qualifying makes the target absolute.
fn pane_target(session: &str, window: &str) -> String {
    if window.contains(':') {
        // Already qualified — the orchestrator builds its target this way.
        return window.to_string();
    }
    format!("{session}:{window}")
}

/// Is the persistent control-mode backend on for this run?
///
/// On unless `AGTX_TMUX_CONTROL` says otherwise. There is no config field: a
/// failed or lost connection already falls back to the subprocess backend on its
/// own, so the only thing a persisted setting could add is a second, staler copy
/// of that decision. The variable stays as a one-run escape hatch, so a bug
/// report can be bisected across the two lanes without editing anything.
fn control_mode_enabled() -> bool {
    control_mode_from_env(std::env::var("AGTX_TMUX_CONTROL").ok().as_deref())
}

/// The policy half, split from the read so it can be tested without touching
/// the process environment. `setenv` is not thread-safe against a concurrent
/// `getenv`, and `cargo test` runs hundreds of tests in parallel threads — a
/// test that mutates a global to check a `match` is a race for no reason.
fn control_mode_from_env(value: Option<&str>) -> bool {
    !matches!(value, Some("0") | Some("false") | Some("no"))
}

/// How fresh the popup's view of its pane is while the **user is typing**.
///
/// It means two different things depending on how the watcher is being driven:
/// under push it is a *rate limit* — the ceiling on how often a paint may cause
/// a capture — and on the poll fallback it is the sampling period itself.
///
/// Either way it is the term a keystroke's echo latency is made of, which is
/// only true because a capture over the control connection is cheap where two
/// `tmux` processes were not. Its cost is paid on the far side — a capture makes
/// the tmux server format the whole pane — so this is not free to lower. [`PANE_OUTPUT_MIN_INTERVAL`] is the same ceiling for the agent's own
/// output, where nobody is waiting on a single frame.
const SHELL_REFRESH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(10);

/// Scrollback lines a popup capture asks for **when the user has scrolled up**.
///
/// A pane in the alternate screen — where full-screen agent UIs live — has none,
/// and returns just the visible rows regardless of this.
const SHELL_POPUP_CAPTURE_LINES: i32 = 500;

/// Scrollback lines to ask for while the popup sits **at the bottom**.
///
/// Only the visible rows are rendered there, so the rest is fetched, formatted
/// by the tmux server, compared and parsed for nothing. Not zero, because the
/// first `C-u` needs somewhere to scroll to: 100 lines is ~3 pages, and by the
/// time the user scrolls past that the deeper capture has arrived.
///
/// Worth nothing for the agents that matter today — they take the alternate
/// screen, where `history_size` is 0 and every depth returns the same bytes. It
/// is for a pane that *does* accumulate history, where the deeper capture is
/// many times the size of the screen being rendered.
const SHELL_POPUP_TAIL_LINES: i32 = 100;

/// How deep the watcher should capture, given where the popup is scrolled.
fn popup_capture_depth(scroll_offset: i32) -> i32 {
    if scroll_offset < 0 {
        SHELL_POPUP_CAPTURE_LINES
    } else {
        SHELL_POPUP_TAIL_LINES
    }
}

/// What the **poll fallback** slows to once a pane has stopped changing.
///
/// Only reached when push is unavailable ([`attach_pane_push`]); with an output
/// watch attached, a still pane produces no captures at all and this never
/// applies. Once a pane has been unchanged for [`PANE_IDLE_ROUNDS`] nobody is
/// waiting on a millisecond, and this much staleness is invisible. A keystroke
/// pokes the watcher back
/// to [`SHELL_REFRESH_INTERVAL`] before the character is even delivered, so the
/// first character after a pause is as prompt as the tenth.
const PANE_IDLE_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// How long the watcher waits for a paint that never comes.
///
/// Under push this replaces the poll interval entirely, so it is a **safety
/// net**, not a cadence: `%output` says bytes reached the pty, not that the
/// rendered pane differs, and the converse happens too — a repaint with no new
/// bytes still changes what `capture-pane` returns. A missed signal is otherwise
/// invisible, because the popup simply stops updating. Same reasoning as
/// [`REDRAW_BACKSTOP`], and if it is ever what makes the popup look right,
/// something above it is wrong.
const PANE_PUSH_BACKSTOP: std::time::Duration = std::time::Duration::from_millis(500);

/// Slowest a capture may be driven by the **agent painting**, as opposed to by
/// the user typing.
///
/// The two deserve different answers and used to share one. Nobody reads text
/// scrolling past frame by frame, but everybody notices their own keystroke
/// arriving late — so the agent's output is sampled at a readable rate while
/// typing keeps [`SHELL_REFRESH_INTERVAL`].
///
/// Not a micro-optimisation: sharing one ceiling made a pane painting flat out
/// cost more than the polling it replaced. Every frame is a `capture-pane`, and
/// a capture makes the tmux server format the whole pane.
const PANE_OUTPUT_MIN_INTERVAL: std::time::Duration = std::time::Duration::from_millis(50);

/// How long after a keystroke captures stay on the fast cadence.
///
/// A keystroke's echo does not arrive with the keystroke: the key reaches the
/// pane almost immediately, the agent repaints a little later, and *that* is
/// what the watcher sees — as a paint, indistinguishable from the agent's own
/// output.
/// Rate-limiting paints to [`PANE_OUTPUT_MIN_INTERVAL`] without this window would
/// therefore delay the echo of every character by that much, which is the one
/// thing this whole lane exists to avoid.
const PANE_TYPING_WINDOW: std::time::Duration = std::time::Duration::from_millis(250);

/// How long to wait before trying to attach an output watch again.
///
/// Attaching costs two `tmux` processes (the pane id, then the client), and the
/// attempt sits in a loop that runs every [`SHELL_REFRESH_INTERVAL`]. A popup
/// left open on a window that has since closed would otherwise spawn a hundred
/// processes a second — the exact cost this whole change removes, reintroduced
/// through the failure path.
const PANE_PUSH_RETRY: std::time::Duration = std::time::Duration::from_secs(2);

/// Unchanged captures before the poll fallback backs off.
///
/// Counted in rounds, so the wall-clock settling time is this times
/// [`SHELL_REFRESH_INTERVAL`] — change one and the other moves with it. It only
/// has to outlast the gap between two keystrokes.
const PANE_IDLE_ROUNDS: u32 = 40;

/// How often periodic work runs: the MCP transition queue, the session refresh,
/// the spinner, expiring warnings.
///
/// What matters is that it is *decoupled* from input, so none of it speeds up
/// because the user is typing. Anything wanting a slower rate keeps its own
/// interval — see [`TRANSITION_POLL_INTERVAL`].
const HOUSEKEEPING_TICK: std::time::Duration = std::time::Duration::from_millis(100);

/// How long the screen may go unpainted while nothing reports a change.
const REDRAW_BACKSTOP: std::time::Duration = std::time::Duration::from_secs(1);

/// How often the MCP transition queue is read.
///
/// A SQLite query, previously run on every housekeeping tick — ten times a
/// second, forever, whether or not anything is connected. A queued transition is
/// not latency-critical: it is a request to move a task between columns, and the
/// phase status it acts on is itself refreshed on a 2-second cache.
const TRANSITION_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a phase status is trusted before the refresh looks again.
const PHASE_STATUS_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(2);

/// How long a warning banner stays up.
const WARNING_MESSAGE_TTL: std::time::Duration = std::time::Duration::from_secs(5);

/// Why the event loop woke up.
///
/// Terminal input and pane content arrive on separate threads and are merged
/// into one channel, so the loop can block on both at once instead of polling
/// each on a timer.
enum Wake {
    Input(Event),
    Pane {
        window: String,
        content: Vec<u8>,
        /// Parsed on the watcher thread — see `ShellPopup::cached_lines`.
        lines: Vec<Line<'static>>,
        metrics: Option<crate::tmux::PaneMetrics>,
    },
}

/// Which pane the watcher should be capturing, if any.
///
/// A condvar rather than a channel because the watcher spends most of its life
/// waiting on exactly one of two things — a target to appear, or its next
/// capture — and both are this same wait. With no popup open it blocks
/// indefinitely and costs nothing.
#[derive(Default)]
struct PaneWatch {
    inner: Mutex<PaneWatchState>,
    cv: Condvar,
}

#[derive(Default)]
struct PaneWatchState {
    target: Option<String>,
    /// tmux pane id of `target` (`%7`), matched against `%output` notifications.
    /// `None` means push is unavailable for this pane and the timer runs.
    pane_id: Option<String>,
    /// Scrollback depth to capture — see [`popup_capture_depth`].
    history_lines: i32,
    /// Bumped to make the watcher capture now, at the fast cadence.
    poke: u64,
    /// Bumped by the output watch every time *this* pane paints.
    signal: u64,
    stopped: bool,
}

impl PaneWatch {
    /// Point the watcher at `target` (or nothing). Cheap enough to call every
    /// iteration, which is the point: no popup call site has to remember to.
    fn follow(&self, target: Option<&str>, history_lines: i32) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        if state.target.as_deref() == target {
            if state.history_lines != history_lines {
                // The user scrolled: re-capture at the new depth now, rather
                // than leaving them looking at a buffer with nothing above it.
                state.history_lines = history_lines;
                state.poke = state.poke.wrapping_add(1);
                drop(state);
                self.cv.notify_all();
            }
            return;
        }
        state.history_lines = history_lines;
        state.target = target.map(str::to_string);
        // The id belongs to the old pane; the watcher resolves the new one.
        state.pane_id = None;
        state.poke = state.poke.wrapping_add(1);
        drop(state);
        self.cv.notify_all();
    }

    /// Record that `pane_id` painted. Called from the output-watch reader
    /// thread, for **every** pane in the session, so it filters before waking.
    fn mark_output(&self, pane_id: &str) {
        let Ok(mut state) = self.inner.lock() else {
            return;
        };
        if state.pane_id.as_deref() != Some(pane_id) {
            return;
        }
        state.signal = state.signal.wrapping_add(1);
        drop(state);
        self.cv.notify_all();
    }

    /// Wait for a poke, a paint, a target change or `wait` to elapse, and report
    /// whether a **poke** was what ended it. `None` means stop.
    ///
    /// A method, not an inline block, because the lock must not outlive the
    /// wait: the watcher takes it again immediately afterwards for the rate
    /// limit, and `Mutex` is not reentrant. Inline, the arm that does not hand
    /// its guard to `wait_timeout` kept the guard alive for the rest of the
    /// iteration and the watcher deadlocked against itself — holding the very
    /// lock the UI thread calls [`follow`](Self::follow) on every iteration, so
    /// the whole TUI froze. Here the guard cannot escape the call.
    fn wait_for_change(
        &self,
        poke: u64,
        signal: u64,
        wait: std::time::Duration,
    ) -> Option<WaitOutcome> {
        let Ok(state) = self.inner.lock() else {
            return None;
        };
        if state.stopped {
            return None;
        }
        if state.poke != poke || state.signal != signal {
            // Something landed while the capture was in flight; no reason to wait.
            return Some(WaitOutcome {
                poked: state.poke != poke,
                skipped: true,
            });
        }
        let Ok((state, _)) = self.cv.wait_timeout(state, wait) else {
            return None;
        };
        if state.stopped {
            return None;
        }
        Some(WaitOutcome {
            poked: state.poke != poke,
            skipped: false,
        })
    }

    /// Hold off for `wait`, unless a poke arrives first. `None` means stop.
    ///
    /// Same reasoning as [`wait_for_change`](Self::wait_for_change): the guard
    /// stays inside the call. A plain sleep would be simpler and wrong — it would
    /// hold the ceiling meant for the agent's paints against the user's own echo.
    fn wait_out_rate_limit(&self, wait: std::time::Duration) -> Option<bool> {
        let Ok(state) = self.inner.lock() else {
            return None;
        };
        let before = state.poke;
        let Ok((state, _)) = self.cv.wait_timeout(state, wait) else {
            return None;
        };
        if state.stopped {
            return None;
        }
        Some(state.poke != before)
    }

    fn set_pane_id(&self, pane_id: Option<String>) {
        if let Ok(mut state) = self.inner.lock() {
            state.pane_id = pane_id;
        }
    }

    /// Capture now and go back to the fast cadence: the user just typed.
    fn poke(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.poke = state.poke.wrapping_add(1);
        }
        self.cv.notify_all();
    }

    fn stop(&self) {
        if let Ok(mut state) = self.inner.lock() {
            state.stopped = true;
        }
        self.cv.notify_all();
    }

    fn target(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|s| s.target.clone())
    }

    /// Pokes issued so far. The watcher compares it across its wait to notice a
    /// poke that arrived *while* it was capturing; tests read it to assert that
    /// following the same target twice is not one.
    fn poke_count(&self) -> u64 {
        self.inner.lock().map(|s| s.poke).unwrap_or(0)
    }

    /// Paint signals accepted so far. Tests read it to assert the filtering.
    #[cfg(test)]
    fn signal_count(&self) -> u64 {
        self.inner.lock().map(|s| s.signal).unwrap_or(0)
    }
}

/// How a [`PaneWatch::wait_for_change`] ended.
struct WaitOutcome {
    /// The user typed. Puts the watcher back on the fast cadence.
    poked: bool,
    /// There was no wait at all: a poke or a paint had already landed while the
    /// capture was in flight, so the pane is known to be moving.
    skipped: bool,
}

/// Is this capture different from the last one the watcher sent?
///
/// Geometry counts as much as content: a cursor that moved without the text
/// changing still has to be repainted, and a capture whose target has changed is
/// not this pane's content at all.
fn pane_capture_changed(
    last: &Option<(String, Vec<u8>, Option<crate::tmux::PaneMetrics>)>,
    target: &str,
    content: &[u8],
    metrics: &Option<crate::tmux::PaneMetrics>,
) -> bool {
    match last {
        Some((window, seen, seen_metrics)) => {
            window != target || seen != content || seen_metrics != metrics
        }
        None => true,
    }
}

/// Still rounds to carry into the next wait.
///
/// A poke resets the count, and that is not bookkeeping — it is the half of the
/// back-off that makes it safe. The capture a poke triggers runs *before* the
/// keystroke that caused it has reached the pane and been echoed, so it sees
/// nothing new; without the reset the watcher would then wait out another idle
/// interval and the character would appear a whole one late.
fn pane_watch_rounds_after_wait(unchanged_rounds: u32, poked: bool) -> u32 {
    if poked {
        0
    } else {
        unchanged_rounds
    }
}

/// How long to wait before the next capture, given how long the pane has been
/// still. See [`PANE_IDLE_INTERVAL`] for why this has two speeds.
fn pane_watch_interval(unchanged_rounds: u32) -> std::time::Duration {
    if unchanged_rounds >= PANE_IDLE_ROUNDS {
        PANE_IDLE_INTERVAL
    } else {
        SHELL_REFRESH_INTERVAL
    }
}

/// Read terminal events on their own thread.
///
/// `event::read()` blocks, which is exactly what is wanted now: the loop learns
/// about a keystroke when there is one instead of asking every few milliseconds.
fn spawn_terminal_reader(tx: mpsc::Sender<Wake>) {
    let _ = std::thread::Builder::new()
        .name("agtx-terminal-input".to_string())
        .spawn(move || loop {
            match crossterm::event::read() {
                Ok(event) => {
                    if tx.send(Wake::Input(event)).is_err() {
                        break;
                    }
                }
                // The terminal is gone; the loop's channel will disconnect and
                // it will quit on its own.
                Err(e) => {
                    tracing::debug!(error = %e, "terminal input reader stopped");
                    break;
                }
            }
        });
}

/// Capture the open popup's pane, and wake the loop **only when it changed**.
///
/// Comparing here rather than in the loop is what makes an idle popup free: an
/// agent that is painting nothing produces no wake-ups at all, so no frame is
/// drawn and no capture is re-parsed.
///
/// **How it learns the pane painted** has two modes, and the second is not a
/// degraded copy of the first — it is the whole design when control mode is off:
///
/// - **push**, when an [`OutputWatch`](crate::tmux::OutputWatch) is attached:
///   the thread sleeps until tmux says *this* pane produced output. Typing at
///   human speed changes the pane 5–8 times a second where the timer sampled it
///   100 times, so most of those captures found nothing new; push removes
///   exactly that waste.
/// - **poll**, otherwise: the historical timer, fast while the pane changes and
///   backing off once it settles.
///
/// Under push, `SHELL_REFRESH_INTERVAL` stops being a poll period and becomes a
/// **rate limit**. The signal removes the floor — no capture when nothing
/// happened — while the interval keeps the ceiling, because a pane painting flat
/// out notifies far faster than it is worth capturing.
fn spawn_pane_watcher(
    watch: Arc<PaneWatch>,
    tx: mpsc::Sender<Wake>,
    input_sink: Arc<dyn PaneInputSink>,
    tmux_ops: Arc<dyn TmuxOperations>,
) {
    let _ = std::thread::Builder::new()
        .name("agtx-pane-watch".to_string())
        .spawn(move || {
            let mut last: Option<(String, Vec<u8>, Option<crate::tmux::PaneMetrics>)> = None;
            let mut unchanged: u32 = 0;
            let mut push: Option<PanePush> = None;
            let mut push_retry_at: Option<Instant> = None;
            let mut watched_target = String::new();
            // When the user last typed, which decides how fast paints are
            // sampled — see `PANE_TYPING_WINDOW`.
            let mut last_keystroke: Option<Instant> = None;
            loop {
                // With no popup open, release the output watch so tmux mirrors
                // nothing for a session nobody is looking at.
                //
                // Outside the lock, never under it: dropping the watch closes a
                // tmux client and reaps the child, and the UI thread calls
                // `follow()` on this same mutex every iteration — so holding it
                // here stalls the whole TUI until that client exits.
                if watch
                    .inner
                    .lock()
                    .map(|state| state.target.is_none())
                    .unwrap_or(true)
                {
                    push = None;
                }
                // Then wait for something to watch. No popup open is the common
                // case, and costs one blocked thread and nothing else.
                let (target, poke, signal, history_lines) = {
                    let Ok(mut state) = watch.inner.lock() else {
                        return;
                    };
                    while state.target.is_none() && !state.stopped {
                        let Ok(next) = watch.cv.wait(state) else {
                            return;
                        };
                        state = next;
                    }
                    if state.stopped {
                        return;
                    }
                    (
                        state.target.clone(),
                        state.poke,
                        state.signal,
                        state.history_lines,
                    )
                };
                let Some(target) = target else { continue };

                if target != watched_target {
                    // A new pane deserves an immediate attempt rather than
                    // whatever backoff the previous one had accumulated.
                    push_retry_at = None;
                    watched_target = target.clone();
                }
                push =
                    attach_pane_push(push, &target, &watch, tmux_ops.as_ref(), &mut push_retry_at);

                let captured_at = Instant::now();
                let (content, metrics) = capture_pane_for_popup(
                    &target,
                    history_lines,
                    input_sink.as_ref(),
                    tmux_ops.as_ref(),
                );
                if pane_capture_changed(&last, &target, &content, &metrics) {
                    unchanged = 0;
                    last = Some((target.clone(), content.clone(), metrics));
                    // Parsed here, off the thread that handles keystrokes, and
                    // once per *change* rather than once per frame.
                    let lines = parse_ansi_to_lines(&content);
                    if tx
                        .send(Wake::Pane {
                            window: target,
                            content,
                            lines,
                            metrics,
                        })
                        .is_err()
                    {
                        return;
                    }
                } else {
                    unchanged = unchanged.saturating_add(1);
                }

                let pushing = push.is_some();
                // Under push there is nothing to poll for: the next capture is
                // caused by the pane painting, and this is only the net for a
                // signal that never came.
                let wait = if pushing {
                    PANE_PUSH_BACKSTOP
                } else {
                    pane_watch_interval(unchanged)
                };
                // A poke, a paint, or a target change ends the wait early, so
                // backing off never costs the first keystroke after a pause.
                //
                // A poke also puts the watcher back on the fast cadence, and
                // that half is not optional: the capture a poke triggers runs
                // *before* the key it announced has reached the pane and been
                // echoed, so it sees no change. Without the reset the next wait
                // would be the slow one again and the echo would arrive a whole
                // idle interval late — the back-off charging exactly what it
                // was built not to.
                let Some(outcome) = watch.wait_for_change(poke, signal, wait) else {
                    return;
                };
                if outcome.skipped {
                    // The pane is moving; do not let a back-off accumulate.
                    unchanged = 0;
                }
                let poked = outcome.poked;
                if poked {
                    last_keystroke = Some(Instant::now());
                }
                unchanged = pane_watch_rounds_after_wait(unchanged, poked);

                // The rate limit, under push only — polling already paces itself.
                //
                // Waited on the condvar rather than slept, so a keystroke still
                // gets through: a plain sleep here would hold the ceiling meant
                // for the *agent's* paints against the user's own echo, adding up
                // to `PANE_OUTPUT_MIN_INTERVAL` to the first character typed
                // after a pause.
                if let Some(remaining) = push_rate_limit_wait(
                    pushing,
                    captured_at.elapsed(),
                    last_keystroke.map(|at| at.elapsed()),
                ) {
                    let Some(poked_during_limit) = watch.wait_out_rate_limit(remaining) else {
                        return;
                    };
                    if poked_during_limit {
                        last_keystroke = Some(Instant::now());
                    }
                }
            }
        });
}

/// How long to hold off the next capture, given how long ago the last one
/// started. `None` means capture now.
///
/// Only meaningful under push: it turns `SHELL_REFRESH_INTERVAL` from "how often
/// to look" into "no more often than this", which is what stops a pane painting
/// flat out from driving one capture per notification.
fn push_rate_limit_wait(
    pushing: bool,
    since_last_capture: std::time::Duration,
    since_last_keystroke: Option<std::time::Duration>,
) -> Option<std::time::Duration> {
    if !pushing {
        return None;
    }
    let floor = match since_last_keystroke {
        // The user is typing: their own echo is what is being waited on.
        Some(since) if since < PANE_TYPING_WINDOW => SHELL_REFRESH_INTERVAL,
        // Nobody is waiting on a single frame of the agent's output.
        _ => PANE_OUTPUT_MIN_INTERVAL,
    };
    if since_last_capture >= floor {
        return None;
    }
    Some(floor - since_last_capture)
}

/// The output-watch connection for the pane currently being followed.
struct PanePush {
    /// Session the watch is attached to. `%output` never crosses sessions, so a
    /// popup in another session needs a different client.
    session: String,
    watch: crate::tmux::OutputWatch,
}

/// Keep the output watch pointed at `target`'s session, reconnecting when the
/// popup moves and giving up — to the timer — when tmux will not cooperate.
///
/// `None` is a supported state, not a failure: `AGTX_TMUX_CONTROL=0`, a tmux
/// that refuses the attach, and a pane whose id cannot be read all land here and
/// get the poll path instead.
fn attach_pane_push(
    current: Option<PanePush>,
    target: &str,
    watch: &Arc<PaneWatch>,
    tmux_ops: &dyn TmuxOperations,
    retry_at: &mut Option<Instant>,
) -> Option<PanePush> {
    if !control_mode_enabled() || !output_push_enabled() {
        return None;
    }
    let session = pane_push_session(target);
    if let Some(push) = current {
        if push.session == session && push.watch.alive() {
            return Some(push);
        }
        // Dropping it closes the client, so nothing stays mirrored for a session
        // nobody is looking at.
        // Also outside any lock, for the same reason: this runs on the watcher
        // thread with nothing held, and closing the client can take a moment.
        drop(push);
        // A watch that died is retried like a first attempt, not immediately.
        *retry_at = Some(Instant::now() + PANE_PUSH_RETRY);
        return None;
    }
    // Both steps below spawn a `tmux` process, and this runs once per watcher
    // iteration. Without a backoff, a popup left open on a window that has since
    // closed would spawn them continuously — far worse than the polling this
    // replaces.
    if let Some(at) = retry_at {
        if Instant::now() < *at {
            return None;
        }
    }
    *retry_at = Some(Instant::now() + PANE_PUSH_RETRY);
    // The id is what every `%output` line is matched against; without it the
    // watch could only say "some pane in this session painted", which is not a
    // signal — it would fire on every other task's output.
    let pane_id = tmux_ops.pane_id(target)?;
    watch.set_pane_id(Some(pane_id));

    let signal_watch = Arc::clone(watch);
    match crate::tmux::OutputWatch::connect(crate::tmux::AGENT_SERVER, &session, move |id| {
        signal_watch.mark_output(id);
    }) {
        Ok(client) => {
            tracing::debug!(session, "pane output watch attached");
            *retry_at = None;
            Some(PanePush {
                session,
                watch: client,
            })
        }
        Err(e) => {
            tracing::debug!(error = %e, "pane output watch unavailable; polling instead");
            watch.set_pane_id(None);
            None
        }
    }
}

/// Is `%output` push on for this run?
///
/// On unless `AGTX_TMUX_PUSH` says otherwise, and there is no config field for
/// the same reason [`control_mode_enabled`] has none: an attach that fails falls
/// back to polling on its own, so a persisted setting could only hold a staler
/// copy of a decision made at runtime. The escape hatch exists because a bug
/// report needs to bisect the two capture lanes — push against poll — the way
/// `AGTX_TMUX_CONTROL` bisects the two input lanes.
fn output_push_enabled() -> bool {
    control_mode_from_env(std::env::var("AGTX_TMUX_PUSH").ok().as_deref())
}

/// The session half of a `session:window` target — the only thing `%output` is
/// scoped to, so it is what decides whether an existing watch can be reused.
fn pane_push_session(target: &str) -> String {
    target
        .split_once(':')
        .map(|(session, _)| session)
        .unwrap_or(target)
        .to_string()
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

/// Display width of a single character, matching what the renderer draws.
fn char_display_width(ch: char) -> usize {
    ratatui::text::Span::raw(ch.to_string()).width()
}

/// Map a byte offset in `text` to the (col, row) of the terminal cell it
/// will render on, using the same char-by-char wrap rule as `wrap_spans`.
///
/// - `prefix_width` is the display width preceding `text` on the first visual
///   row (e.g. the `"  Prompt: "` label). It is consumed only on row 0; after
///   any wrap or `'\n'`, the next visual row starts at column 0.
/// - `wrap_width` is the inner width of the block (border-subtracted). A
///   width of 0 short-circuits to (prefix_width, 0).
/// - `'\n'` in `text` is treated as a hard line break.
///
/// This function and `wrap_spans` MUST stay in lock-step: any change to the
/// wrap rule has to land in both, otherwise the cursor drifts off what was
/// actually drawn. Both use lazy wrap (wrap only when the next char would
/// overflow), so a cursor at end-of-row sits at col=wrap_width on that row
/// — which is still inside the inner area (wrap_width = area.width - 2).
fn wrapped_cursor_pos(
    text: &str,
    cursor_byte: usize,
    prefix_width: usize,
    wrap_width: usize,
) -> (usize, usize) {
    let cursor_byte = cursor_byte.min(text.len());
    let mut col = prefix_width;
    let mut row = 0usize;
    if wrap_width == 0 {
        return (col, row);
    }
    let mut byte = 0usize;
    for ch in text.chars() {
        if byte >= cursor_byte {
            break;
        }
        if ch == '\n' {
            row += 1;
            col = 0;
        } else {
            let w = char_display_width(ch);
            if col + w > wrap_width {
                row += 1;
                col = 0;
            }
            col += w;
        }
        byte += ch.len_utf8();
    }
    (col, row)
}

/// Pre-wrap a sequence of styled spans into visual `Line`s by display width.
///
/// Char-by-char lazy wrap (no word-boundary detection). This makes our layout
/// authoritative: each produced `Line` has display width ≤ `wrap_width`, so
/// `Paragraph::wrap(Wrap { trim: false })` leaves it untouched and the cursor
/// position computed by `wrapped_cursor_pos` lines up exactly with what was
/// drawn — no two-source-of-truth between renderer and cursor.
///
/// Span styles are preserved across wrap points by splitting the span text
/// at the wrap boundary and re-emitting both halves with the same style.
/// `'\n'` inside span content is NOT handled here — callers split on `'\n'`
/// before calling so each invocation wraps a single logical line.
fn wrap_spans(spans: Vec<Span<'static>>, wrap_width: usize) -> Vec<Line<'static>> {
    if wrap_width == 0 || spans.is_empty() {
        return vec![Line::from(spans)];
    }
    let mut visual: Vec<Vec<Span<'static>>> = vec![Vec::new()];
    let mut col = 0usize;
    for span in spans {
        let style = span.style;
        let mut chunk = String::new();
        for ch in span.content.chars() {
            let w = char_display_width(ch);
            if col + w > wrap_width {
                if !chunk.is_empty() {
                    visual
                        .last_mut()
                        .unwrap()
                        .push(Span::styled(std::mem::take(&mut chunk), style));
                }
                visual.push(Vec::new());
                col = 0;
            }
            chunk.push(ch);
            col += w;
        }
        if !chunk.is_empty() {
            visual.last_mut().unwrap().push(Span::styled(chunk, style));
        }
    }
    visual.into_iter().map(Line::from).collect()
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
/// `collapse` controls how `{task}` is substituted.
///
/// `true` flattens the task to a single line, which the **typed** send path
/// needs: it delivers the command with `send_keys`, where an embedded newline is
/// a real Enter and would submit the message half-written.
///
/// `false` substitutes it verbatim. The **launch lane** passes the command in
/// argv, where newlines are just bytes, so a task keeps the paragraphs and lists
/// its author wrote — `spec-kit`'s `/speckit.specify {task}` and `openspec`'s
/// `/opsx:propose {task}` are the two bundled plugins this affects.
#[allow(clippy::too_many_arguments)]
fn resolve_skill_command(
    plugin: &Option<WorkflowPlugin>,
    phase: &str,
    agent_name: &str,
    task_content: &str,
    cycle: i32,
    task_id: &str,
    collapse: bool,
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
            let task_text = if collapse {
                task_content
                    .lines()
                    .map(|l| l.trim())
                    .filter(|l| !l.is_empty())
                    .collect::<Vec<_>>()
                    .join(" ")
            } else {
                task_content.to_string()
            };
            cmd.replace("{task}", &task_text)
        };
    let expanded = expanded.replace("{phase}", &cycle.to_string());
    let expanded = expanded.replace("{task_id}", task_id);
    skills::transform_plugin_command(&expanded, agent_name)
}

/// Spawn a background thread that optionally switches agent, waits for readiness,
/// then sends a skill command and prompt to the tmux pane.
#[allow(clippy::too_many_arguments)]
fn spawn_send_to_agent(
    tmux_ops: Arc<dyn TmuxOperations>,
    agent_registry: Arc<dyn agent::AgentRegistry>,
    task_id: String,
    agent_hooks: bool,
    auto_trust: bool,
    target: String,
    current_agent: String,
    target_agent: String,
    needs_switch: bool,
    skill_cmd: Option<String>,
    // The same command resolved **without** collapsing `{task}`, for the argv
    // path. An agent switch starts a new process, so it takes the message in argv
    // and keeps the task's line structure; only the typed fallback needs the
    // flattened `skill_cmd`. See `resolve_skill_command`.
    skill_cmd_launch: Option<String>,
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
                &task_id,
            );
        }

        let mut delivered_at_launch = false;
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
                    write_skills_to_worktree(
                        wt_path,
                        &project_path,
                        &plugin,
                        &[&target_agent],
                        agent_hooks,
                    );
                }
            }
            let agent_ops = agent_registry.get(&target_agent);
            // A switch starts a *new process*, so the opening message goes in argv
            // exactly as it does on a first launch — same act, just into an
            // existing window. A same-agent advance below cannot: that process is
            // already running, so there is no argv to fill.
            let launch_text = compose_launch_text(skill_cmd_launch.as_deref(), &prompt);
            delivered_at_launch =
                agent::spec::can_launch_with_prompt(agent_ops.prompt_injection(), &launch_text);
            let new_cmd = agent_ops.build_interactive_command(if delivered_at_launch {
                &launch_text
            } else {
                ""
            });
            switch_agent_in_tmux(tmux_ops.as_ref(), &target, &current_agent, &new_cmd);
            if !delivered_at_launch {
                // The *new* agent is what has to become ready.
                let _ = wait_for_agent_ready(&tmux_ops, &target, Some(&target_agent), auto_trust);
            }
        }
        if !delivered_at_launch {
            let clear_context = plugin
                .as_ref()
                .map(|p| p.clear_context_on_advance)
                .unwrap_or(false);
            send_skill_and_prompt(
                &tmux_ops,
                &target,
                &skill_cmd,
                &prompt,
                &prompt_trigger,
                &task_content,
                &target_agent,
                &auto_dismiss,
                clear_context,
            );
        }
    });
}

/// Attempts and per-attempt budget for [`deliver_message`].
///
/// Worst case is `DELIVERY_ATTEMPTS × (settle + confirm)` = 3 × (10s + 2s) = 36s,
/// because the settle runs *inside* the attempt loop — and the OpenCodePicker
/// path calls this twice, so ~72s. That only happens when the pane never goes
/// quiet and nothing ever lands, i.e. a session that is already broken; every
/// call site is a background thread, so it delays that task's send and nothing
/// else.
const DELIVERY_ATTEMPTS: u32 = 3;
const DELIVERY_CONFIRM_POLLS: u32 = 10; // x 200ms = 2s

/// Attempts and per-attempt budget for [`submit_message`].
///
/// Smaller than the delivery budget on purpose: by the time this runs the text is
/// known to be in the composer, so this is only absorbing a composer that is still
/// mid-render, not a session that never attached its stdin.
const SUBMIT_ATTEMPTS: u32 = 3;
const SUBMIT_CONFIRM_POLLS: u32 = 5; // x 200ms = 1s
/// Pane-settle budget before each attempt: 1s of quiet, given up on after 10s.
const SETTLE_STABLE_POLLS: u32 = 5;
const SETTLE_MAX_POLLS: u32 = 50;

/// Longest prefix of a message used to confirm it landed on a pane that never
/// went quiet. Short on purpose: a composer wraps and re-indents what it echoes,
/// so a long needle straddles a line break and reads as absent.
const DELIVERY_NEEDLE_CHARS: usize = 16;

/// Whitespace-collapsed prefix of `text`, or `None` when there is nothing
/// distinctive enough to look for.
fn delivery_needle(text: &str) -> Option<String> {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let needle: String = flat.chars().take(DELIVERY_NEEDLE_CHARS).collect();
    (needle.chars().count() >= 4).then_some(needle)
}

/// Whether `needle` is visible in `pane`, comparing both whitespace-collapsed so
/// a wrap or re-indent in the composer does not hide it.
fn pane_shows(pane: &str, needle: &str) -> bool {
    let flat: String = pane.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.contains(needle)
}

/// Wait for the pane to stop changing, up to [`SETTLE_MAX_POLLS`].
///
/// Returns whether it settled; callers proceed either way, since a pane that
/// never settles (a spinner, a clock) must not block the send forever.
fn wait_for_pane_settled(tmux_ops: &Arc<dyn TmuxOperations>, target: &str) -> bool {
    let mut last = String::new();
    let mut stable = 0u32;
    for _ in 0..SETTLE_MAX_POLLS {
        std::thread::sleep(std::time::Duration::from_millis(200));
        let now = tmux_ops.capture_pane(target).unwrap_or_default();
        if now == last {
            stable += 1;
            if stable >= SETTLE_STABLE_POLLS {
                return true;
            }
        } else {
            stable = 0;
            last = now;
        }
    }
    false
}

/// Put `text` into the agent's composer, resending while nothing lands.
///
/// An agent TUI that has not attached its stdin reader yet silently discards
/// what it is sent, and bracketed paste does not help: the discard happens in
/// the application, not in the pty. `wait_for_agent_ready` narrows that window
/// but cannot close it — it has no signal for an agent that reports its npm
/// wrapper in `pane_current_command` and declares no readiness indicator, so it
/// falls through to a timeout and sends into whatever is there.
///
/// Landing is judged by **the pane changing**, not by finding the text in it: a
/// composer wraps, re-indents and box-draws what it echoes, so any needle longer
/// than a few characters is unreliable. Conversely a resend only happens while
/// the pane is unchanged — the same rule [`dismiss_launch_dialog`] uses, and for
/// the same reason: a redraw means the first one landed, and resending would
/// double the message.
///
/// Returns whether the message was seen to land. Callers submit either way,
/// because a false negative (a pane that happened not to redraw) must not
/// swallow the task.
/// Lines at the bottom of a pane treated as the composer.
///
/// Sized from the worst real layout, not from the composer alone: while the
/// command picker is open the agent draws its suggestions *below* the composer,
/// and cursor's footer wraps the worktree path over three more lines. That puts
/// the text being submitted eight or more lines off the bottom — a snugger
/// window reads it as already gone and stops pressing Enter after one.
///
/// The cost of erring wide is one extra Enter into a composer that already
/// submitted, which is inert; the cost of erring narrow is a command parked
/// forever.
const COMPOSER_TAIL_LINES: usize = 14;

/// Whether the message is still sitting in the composer rather than submitted.
///
/// Only the bottom of the pane is examined: after a submit the text moves up into
/// the scrollback, and finding it *there* is proof it went, not that it stayed.
fn composer_holds(pane: &str, needle: &str) -> bool {
    // Trailing blanks first. `capture-pane -p` emits one line per pane *row*, not
    // per rendered line — verified against tmux 3.5a: a 20-row pane holding one
    // word comes back as 20 lines, 19 of them empty. Anchoring the window to the
    // raw end would put it entirely inside that padding whenever the agent's
    // output has not yet filled the pane, find nothing, and stop pressing Enter
    // after one — the very park this exists to catch.
    let mut lines: Vec<&str> = pane.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(COMPOSER_TAIL_LINES);
    pane_shows(&lines[start..].join("\n"), needle)
}

/// Press Enter until the message actually leaves the composer.
///
/// The check is "the text is gone from the composer", not "the pane changed".
/// A repaint is not a submit: a **bare skill command** — one with no prompt after
/// it, which is what a phase whose command carries no `{task}`/`{task_id}` sends —
/// exactly matches a skill name, so the composer's command picker opens *on the
/// paste*. Enter is then consumed by the picker ("Press enter to insert"), which
/// inserts the command and repaints. The old change-detector read that repaint as
/// success and returned, leaving the command parked in the composer forever.
///
/// Measured against codex-cli 0.144.5 and cursor-agent 2026.08.25: both open the
/// picker on a pasted bare command, both need the second Enter, and both run the
/// skill once it arrives.
///
/// Falls back to the change-detector when the text is too short to track, and is
/// bounded either way — an agent that never submits costs `SUBMIT_ATTEMPTS`
/// keypresses, not an unbounded stream.
///
/// Known cost: agents echo the submitted message into the transcript just above
/// the composer, so a *successful* submit can leave the needle inside the window
/// and spend the remaining attempts. Those Enters land in an empty composer,
/// which is inert — except against a dialog that renders mid-submit, where a bare
/// Enter picks the highlighted option. `answer_session_dialogs` is what answers
/// those, and it runs on the refresh loop rather than here.
fn submit_message(tmux_ops: &Arc<dyn TmuxOperations>, target: &str, text: &str) {
    let needle = delivery_needle(text);
    for _ in 0..SUBMIT_ATTEMPTS {
        let before = tmux_ops.capture_pane(target).unwrap_or_default();
        let _ = tmux_ops.send_key(target, "Enter");
        for _ in 0..SUBMIT_CONFIRM_POLLS {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Ok(now) = tmux_ops.capture_pane(target) else {
                continue;
            };
            match needle.as_deref() {
                Some(n) if !composer_holds(&now, n) => return,
                Some(_) => {}
                // Nothing distinctive enough to look for; the pane moving is all
                // there is to go on.
                None if now != before => return,
                None => {}
            }
        }
    }
}

fn deliver_message(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    text: &str,
    paste: bool,
) -> bool {
    let needle = delivery_needle(text);
    for attempt in 0..DELIVERY_ATTEMPTS {
        // Settle first, for two reasons. A composer that is still rendering the
        // previous turn may not take the message at all — a phase advance fires
        // as soon as the artifact appears, which can be while the agent is still
        // writing its closing lines. And a pane that is changing on its own makes
        // change-detection meaningless, because the change it sees would be the
        // agent's output rather than the echo of what was sent.
        let settled = wait_for_pane_settled(tmux_ops, target);

        // Clear the composer before a *resend* only. A resend happens because
        // nothing was seen to land, but "seen" is not "did not" — if the first
        // send did land and merely rendered late, appending a second copy gives
        // the agent the message twice, concatenated. Ctrl+U is best-effort: an
        // agent that does not map it to kill-line is no worse off than before
        // this guard, and the first send is never preceded by one, so a fresh
        // composer is never touched.
        if attempt > 0 {
            let _ = tmux_ops.send_key(target, "C-u");
            std::thread::sleep(std::time::Duration::from_millis(150));
        }

        let before = tmux_ops.capture_pane(target).unwrap_or_default();
        let _ = if paste {
            tmux_ops.paste_text(target, text)
        } else {
            tmux_ops.send_text(target, text)
        };
        for _ in 0..DELIVERY_CONFIRM_POLLS {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Ok(now) = tmux_ops.capture_pane(target) else {
                continue;
            };
            // On a pane that went quiet, any change is the echo. On one that
            // never did, a change proves nothing — it is the agent still
            // writing — so the text itself has to be found. Without that
            // distinction a busy pane confirms on its first poll every time,
            // which is precisely the case the settle step exists for.
            let landed = match (settled, needle.as_deref()) {
                (true, _) => now != before,
                (false, Some(n)) => pane_shows(&now, n),
                (false, None) => now != before,
            };
            if landed {
                return true;
            }
        }
    }
    false
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
    clear_context: bool,
) {
    // How this agent's composer takes a message. Read once: it decides both the
    // clear-context send below and the skill+prompt send after it.
    let strategy =
        agent::spec(agent_name).map_or(agent::SendStrategy::Generic, |s| s.send_strategy);

    // Opt-in context clear on phase advance. Agents with no known clear command
    // (`clear_context_command: None`, tbd per issue #46) fall through to a normal
    // send unchanged.
    let clear_cmd = agent::spec(agent_name).and_then(|s| s.clear_context_command);
    if let (true, Some(cmd)) = (clear_context, clear_cmd) {
        // An Ink-class composer (`SendStrategy::Combined`) drops a combined
        // text+Enter `send-keys`: the Enter fires before the TUI has rendered the
        // input, so the command is left parked. That is worse than not clearing —
        // the skill+prompt below is then pasted *onto the end* of the parked text
        // and the whole thing submits as one message, so the phase command never
        // resolves. Deliver it the way a message is delivered and confirm it left
        // the composer. Verified against pi 0.84.3, which is why pi is `Combined`.
        if strategy == agent::SendStrategy::Combined {
            deliver_message(tmux_ops, target, cmd, true);
            submit_message(tmux_ops, target, cmd);
        } else {
            let _ = tmux_ops.send_keys(target, cmd);
        }
        // Wait for the agent to clear its buffer and return to idle prompt.
        // Pattern mirrors the stability-poll loops used elsewhere in this
        // function: poll until pane content stabilises (no changes for ~1s),
        // capped at ~5s total.
        let mut last_content = String::new();
        let mut stable_ticks = 0u32;
        for _ in 0..25 {
            std::thread::sleep(std::time::Duration::from_millis(200));
            if let Ok(content) = tmux_ops.capture_pane(target) {
                if content != last_content {
                    last_content = content;
                    stable_ticks = 0;
                } else {
                    stable_ticks += 1;
                    if stable_ticks >= 5 {
                        break;
                    }
                }
            }
        }
    }

    // OpenCode command picker handles args differently: when a command has arguments
    // (e.g. `/agtx-plan abc123`), typing the full string and pressing Enter causes the
    // picker to confirm/insert only the command name — stripping the args. Commands
    // without args (e.g. `/agtx-review`) work fine with a single Enter confirm + submit.
    //
    // Fix: send just the command name, wait for picker, Enter to confirm (inserts cmd),
    // then send the args (picker dismissed, input now has just the command), then Enter.
    if strategy == agent::SendStrategy::OpenCodePicker {
        // Build the full message: skill command (if any) + prompt (if any)
        let full_text = if let Some(cmd) = skill_cmd {
            if !prompt.is_empty() {
                format!("{}\n\n{}", cmd, prompt)
            } else {
                cmd.clone()
            }
        } else if !prompt.is_empty() {
            prompt.to_string()
        } else {
            let oneline = task_content
                .lines()
                .map(|l| l.trim())
                .filter(|l| !l.is_empty())
                .collect::<Vec<_>>()
                .join(" ");
            oneline
        };

        if !full_text.is_empty() {
            // Check if the first token looks like a slash command (starts with /)
            let first_line = full_text.lines().next().unwrap_or(&full_text);
            if let Some(space_pos) = first_line.find(' ') {
                let cmd_name = &first_line[..space_pos];
                let cmd_args = &first_line[space_pos..]; // includes leading space
                let rest = &full_text[first_line.len()..]; // rest of the message after first line

                if cmd_name.starts_with('/') {
                    // Send just the command name to trigger the picker. Typed,
                    // not pasted — the picker opens on typing — so this is the
                    // step most exposed to a TUI that is not reading stdin yet,
                    // and losing it loses the whole message: the Enter below then
                    // confirms nothing and the args are typed into an empty
                    // composer. `deliver_message` resends while the pane is
                    // unchanged.
                    deliver_message(tmux_ops, target, cmd_name, false);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    // Enter confirms/inserts the command from picker
                    let _ = tmux_ops.send_key(target, "Enter");
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    // Now send the args + any remaining prompt text
                    let remaining = format!("{}{}", cmd_args, rest);
                    deliver_message(tmux_ops, target, &remaining, false);
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    let _ = tmux_ops.send_key(target, "Enter");
                    return;
                }
            }

            // No args (or no slash command): one send, confirmed, then Enter.
            deliver_message(tmux_ops, target, &full_text, false);
            std::thread::sleep(std::time::Duration::from_millis(200));
            // Enter to confirm picker (if any), then second Enter to submit
            let _ = tmux_ops.send_key(target, "Enter");
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = tmux_ops.send_key(target, "Enter");
        }
        return;
    }

    // Gemini, Codex, cursor & antigravity: always combine skill+prompt into a
    // single message.
    // Gemini: sending separately causes it to execute the skill and queue the
    //   prompt, which gets lost or arrives too late.
    // Codex: skill mentions ($skill-name) are inline references that must be
    //   part of a message — sending just "$skill" standalone does nothing.
    // Antigravity: descends from the Gemini CLI lineage (settings live under
    //   ~/.gemini/), so it is treated as Ink-class and gets the same
    //   wait-for-echo send. Not yet confirmed against a live session.
    if strategy == agent::SendStrategy::Combined {
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
            // Bracketed paste rather than typed keystrokes. Two reasons, both of
            // which the echo-poll this replaces was only working around:
            //
            // 1. It is atomic. `send-keys` streams characters into a TUI that may
            //    not have attached its stdin reader yet, so the old code typed the
            //    text and then polled `capture_pane` for up to 4s waiting for it to
            //    render before daring to press Enter. A paste arrives as one write
            //    wrapped in \x1b[200~ … \x1b[201~, so there is no window to lose
            //    keystrokes in and nothing to poll for.
            // 2. It keeps newlines as newlines. A skill command and its prompt are
            //    joined by "\n\n", and `send-keys` delivers those as real Enter
            //    presses — an Ink composer submits on the first one and the rest of
            //    the message arrives as a second, truncated turn. Inside a bracketed
            //    paste they are literal text.
            //
            // Atomic is not the same as delivered, though: an agent that has
            // not attached its stdin reader discards the paste whole, which is
            // how every antigravity task reached its composer empty. So the
            // paste goes through `deliver_message`, which resends while the pane
            // is unchanged, and only then is it submitted.
            deliver_message(tmux_ops, target, &text, true);
            std::thread::sleep(std::time::Duration::from_millis(150));
            // How many Enters this takes is not fixed. A message with a prompt
            // after the command submits on the first; a bare command opens the
            // command picker on the paste and needs a second. `submit_message`
            // decides by watching the composer rather than counting keypresses.
            submit_message(tmux_ops, target, &text);
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

            // Verify the command was received: check that pane content changes within
            // ~3s (agent picked it up). If nothing changed, the agent wasn't ready yet —
            // wait 1s and resend once.
            let baseline = tmux_ops.capture_pane(target).unwrap_or_default();
            let mut received = false;
            for _ in 0..15 {
                // up to 3s
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Ok(content) = tmux_ops.capture_pane(target) {
                    if content != baseline {
                        received = true;
                        break;
                    }
                }
            }
            if !received {
                std::thread::sleep(std::time::Duration::from_secs(1));
                let _ = tmux_ops.send_keys(target, cmd);
            }
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
                    // Task-derived text: must not go through key-name lookup.
                    let _ = tmux_ops.send_text(target, &oneline);
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
                        tracing::info!(
                            target = target,
                            patterns = ?rule.detect,
                            response = %rule.response,
                            "Auto-dismiss rule triggered"
                        );
                        for key in rule.response.split('\n') {
                            let _ = tmux_ops.send_key(target, key);
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
    // Empty task.agent means agent not yet assigned (SetupResult pending) — no switch needed
    let switch = !task.agent.is_empty() && task.agent != target;
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
static AGENT_COMMANDS: std::sync::LazyLock<Vec<&'static str>> = std::sync::LazyLock::new(|| {
    agent::AGENT_SPECS
        .iter()
        .flat_map(|s| s.process_names.iter().copied())
        // Not agent binaries: a pane running a Python entry point (the swebench
        // harness) must not read as "back at the shell".
        .chain(["python3", "python"])
        .collect()
});

/// Strings in pane content that indicate an agent TUI is active and ready.
/// Used by `is_agent_active` to detect agents like Gemini, Cursor and Grok that
/// run inside bash/node and don't change `pane_current_command` to their own name.
/// (Grok is a native binary, but the npm package launches it through a wrapper,
/// so the pane still reports `bash`.)
/// Also used by `wait_for_agent_ready` (Check 2) to detect readiness for these agents.
/// Flattened across all agents deliberately: matching is done against any pane
/// regardless of which agent runs there. Attributing each string to its agent
/// would be strictly more correct — "Ask anything" matching in a Claude pane is a
/// false positive today — but that *changes behaviour*, so it belongs in its own
/// change with its own test rather than riding inside a refactor that is meant to
/// preserve it.
static AGENT_ACTIVE_INDICATORS: std::sync::LazyLock<Vec<&'static str>> =
    std::sync::LazyLock::new(|| {
        agent::AGENT_SPECS
            .iter()
            .flat_map(|s| s.active_indicators.iter().copied())
            .collect()
    });

/// Check if the pane is running a shell (i.e. the agent has exited).
/// Returns true when `pane_current_command` reports a shell (bash, zsh, sh, fish)
/// rather than an agent process.
fn is_pane_at_shell(tmux_ops: &dyn TmuxOperations, target: &str) -> bool {
    if let Some(cmd) = tmux_ops.pane_current_command(target) {
        // Compared whole, not by substring. `pane_current_command` reports the
        // command *name*, so equality is what the field means — and a substring
        // test lets short names swallow unrelated processes: `pi` matches `pip`,
        // `pipx`, `pipenv` and `pinentry`, and `agent` (cursor) matches anything
        // ending in it. Every one of those would read as "an agent is running"
        // for *every* task, since this list is flattened across all agents.
        let cmd = cmd.trim();
        !AGENT_COMMANDS.iter().any(|a| cmd == *a)
    } else {
        false
    }
}

/// Orchestrator is live iff its tmux window exists (no pane-command peeking).
fn is_orchestrator_live(tmux_ops: &dyn TmuxOperations, target: &str) -> bool {
    tmux_ops.window_exists(target).unwrap_or(false)
}

/// Startup reattach: returns the target if a window survives, replaying catch-up.
fn detect_existing_orchestrator(
    experimental: bool,
    tmux_ops: &dyn TmuxOperations,
    tmux_project_name: &str,
    db: Option<&Database>,
    tasks: &[Task],
    project_path: Option<&Path>,
) -> Option<String> {
    if !experimental {
        return None;
    }
    let target = format!("{}:orchestrator", tmux_project_name);
    if !tmux_ops.window_exists(&target).unwrap_or(false) {
        return None;
    }
    if let Some(db) = db {
        run_orchestrator_catchup(db, tasks, project_path);
    }
    Some(target)
}

/// Kill all windows matching `target` (tmux allows duplicates); false if the 16-iter cap is hit.
fn kill_windows_by_name(tmux_ops: &dyn TmuxOperations, target: &str) -> bool {
    for _ in 0..16 {
        if !tmux_ops.window_exists(target).unwrap_or(false) {
            return true;
        }
        let _ = tmux_ops.kill_window(target);
    }
    !tmux_ops.window_exists(target).unwrap_or(false)
}

/// Replay "completed phase" notifications for tasks whose artifact is on disk.
fn run_orchestrator_catchup(db: &Database, tasks: &[Task], project_path: Option<&Path>) {
    let existing: HashSet<String> = db
        .peek_notifications()
        .unwrap_or_default()
        .into_iter()
        .map(|n| n.message)
        .collect();

    for task in tasks {
        if !matches!(task.status, TaskStatus::Planning | TaskStatus::Running) {
            continue;
        }
        let plugin = match &task.plugin {
            Some(name) => WorkflowPlugin::load(name, project_path).ok(),
            None => skills::load_bundled_plugin("agtx"),
        };
        let Some(ref wt) = task.worktree_path else {
            continue;
        };
        if !phase_artifact_exists(wt, task.status, &plugin, task.cycle) {
            continue;
        }
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
        if existing.contains(&message) {
            continue;
        }
        let _ = db.create_notification(&crate::db::Notification::new(message));
    }
}

/// Check if an agent is actively running in the pane.
/// Uses both `pane_current_command` (works for Claude, Codex, Copilot) and
/// pane content indicators (works for Gemini which runs inside bash).
fn is_agent_active(tmux_ops: &dyn TmuxOperations, target: &str, agent_name: Option<&str>) -> bool {
    // Check 1: agent process visible in pane_current_command
    if !is_pane_at_shell(tmux_ops, target) {
        return true;
    }
    // Check 2: check the bottom of the visible pane for agent UI indicators.
    // Only the last few lines are checked to avoid false positives from
    // indicator strings appearing in conversation output higher up.
    if let Ok(content) = tmux_ops.capture_pane(target) {
        let tail_text = pane_tail(&content, PANE_TAIL_LINES);
        if active_indicators_for(agent_name)
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
    task_id: &str,
) {
    if !tmux_ops.window_exists(target).unwrap_or(true) {
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
        let _ = tmux_ops.create_window(
            session,
            window,
            wt_path,
            Some(resume_cmd),
            true,
            &agtx_task_env(task_id, wt_path),
        );
    }
}

/// Gracefully switch the agent running in a tmux window.
/// Terminates the current agent, waits for the shell prompt,
/// then starts the new agent.
///
/// The exit command comes from `AgentSpec::exit_command`; `None` (codex, cursor)
/// means Ctrl+C is the only way out, and Ctrl+C + Ctrl+D is the last resort when
/// the command does not take. How it is *delivered* is decided by
/// `AgentSpec::send_strategy` — see the comment on the send below.
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
    // `None` means Ctrl+C is the only way out (codex, cursor). An agent agtx does
    // not know keeps the historical default of /exit.
    let exit_cmd = agent::spec(current_agent).map_or(Some("/exit"), |s| s.exit_command);

    if let Some(cmd) = exit_cmd {
        // An Ink-class composer (`SendStrategy::Combined`) loses a combined
        // text+Enter `send-keys`: Enter fires before the TUI has rendered the
        // input, and the exit command is lost. Send the text, wait for it to
        // echo, then Enter — the same pattern as `send_skill_and_prompt`. Found
        // with gemini; pi's spec records the identical measurement, and
        // antigravity is the same class of TUI. Without this the graceful path is
        // dead code for those agents: the command sits unsent, the 3s poll times
        // out, and every switch away from them kills the agent with Ctrl+C
        // mid-turn — after which the unsent text lands in the bare shell.
        let split_enter = agent::spec(current_agent)
            .is_some_and(|s| s.send_strategy == agent::SendStrategy::Combined);
        if split_enter {
            let _ = tmux_ops.send_text(target, cmd);
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(200));
                if let Ok(content) = tmux_ops.capture_pane(target) {
                    if content.contains(cmd) {
                        break;
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(200));
            let _ = tmux_ops.send_key(target, "Enter");
        } else {
            let _ = tmux_ops.send_keys(target, cmd);
        }
    } else {
        let _ = tmux_ops.send_key(target, "C-c");
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
        if !is_agent_active(tmux_ops, target, Some(current_agent)) {
            found_shell = true;
            break;
        }
    }

    // 3. If still running, the agent was likely busy. Ctrl+C to cancel, then retry exit.
    if !found_shell {
        let _ = tmux_ops.send_key(target, "C-c");
        std::thread::sleep(std::time::Duration::from_millis(1000));

        if let Some(cmd) = exit_cmd {
            if current_agent == "gemini" {
                let _ = tmux_ops.send_text(target, cmd);
                for _ in 0..20 {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Ok(content) = tmux_ops.capture_pane(target) {
                        if content.contains(cmd) {
                            break;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                let _ = tmux_ops.send_key(target, "Enter");
            } else {
                let _ = tmux_ops.send_keys(target, cmd);
            }
        }

        // Wait for agent exit after retry
        for _ in 0..50 {
            // 5s
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !is_agent_active(tmux_ops, target, Some(current_agent)) {
                found_shell = true;
                break;
            }
        }
    }

    // 4. Last resort: Ctrl+D to force exit
    if !found_shell {
        let _ = tmux_ops.send_key(target, "C-d");
        for _ in 0..20 {
            // 2s
            std::thread::sleep(std::time::Duration::from_millis(100));
            if !is_agent_active(tmux_ops, target, Some(current_agent)) {
                break;
            }
        }
    }

    // 4. Let the shell fully initialize before sending the new agent command
    std::thread::sleep(std::time::Duration::from_millis(2000));

    // 5. Start the new agent
    // Wrap with env -u to clear Claude Code's nesting-detection vars — the
    // persistent shell inherited them at window creation time, so they must
    // be stripped explicitly here (unlike create_window which uses env -u
    // on the initial command only).
    let cmd = format!(
        "env -u CLAUDECODE -u CLAUDE_CODE_ENTRYPOINT {}",
        new_agent_cmd
    );
    // **One layer of quoting here, not two.** `create_window` nests its command
    // inside `sh -c '…'` (`wrap_launch_command`), so a prompt going that way is
    // quoted twice. This path types a command line into the window's
    // *already-running* interactive shell, which parses it once — the quoting
    // `compose_command` already applied is exactly right, and adding
    // `single_quote` again would deliver visible backslashes into the composer.
    if cmd.contains('\n') {
        // A launch-injected prompt keeps its paragraphs, and typing a newline at a
        // shell prompt submits the line. Bracketed paste puts the whole thing in
        // the line editor as literal text, so one Enter runs it intact. If the
        // shell has bracketed paste off this degrades to the typed behaviour —
        // never worse than `send_keys`, which is why the no-newline path is left
        // exactly as it was.
        let _ = tmux_ops.paste_text(target, &cmd);
        std::thread::sleep(std::time::Duration::from_millis(300));
        let _ = tmux_ops.send_key(target, "Enter");
    } else {
        let _ = tmux_ops.send_keys(target, &cmd);
    }

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

/// Known first-launch dialogs that block an agent before it accepts any input,
/// paired with the keystroke that answers them.
///
/// A worktree is a brand-new directory every time, and for agents whose trust is
/// per-directory and not inherited (antigravity, cursor) these fire on most task
/// starts — the normal case, not an edge case. Claude is the exception: it
/// honours a trusted ancestor, so with the default in-project `worktree_dir` its
/// trust arm only fires where the worktree has no trusted parent (containers,
/// an out-of-project `worktree_dir`, the smoke runner). See the per-agent notes
/// on `AgentSpec::dialogs`.
/// Every agent's `Launch`-scope dialog, flattened.
///
/// Flat because `wait_for_agent_ready` is handed only a tmux target and does not
/// know which agent runs there. Attributing these at match time is strictly more
/// correct but *changes behaviour*, so it gets its own change. `Session`-scope
/// dialogs are excluded: they are matched against their own agent only, by the
/// refresh loop.
static LAUNCH_DIALOGS: std::sync::LazyLock<Vec<&'static agent::AgentDialog>> =
    std::sync::LazyLock::new(|| {
        agent::AGENT_SPECS
            .iter()
            .flat_map(|s| s.dialogs.iter())
            .filter(|d| d.scope == agent::DialogScope::Launch)
            .collect()
    });

/// The launch dialogs to match in a pane running `agent_name`.
///
/// Attributed when the agent is known, so one agent's prompt is never answered in
/// another's pane — answering sends a menu digit, and a stray "2" typed into a
/// live composer is a real corruption. Falls back to every agent's dialogs for an
/// agent agtx has no spec for, which is the historical behaviour and better than
/// leaving such a pane blocked forever.
fn launch_dialogs_for(agent_name: Option<&str>) -> Vec<&'static agent::AgentDialog> {
    match agent_name.and_then(agent::spec) {
        Some(spec) => spec
            .dialogs
            .iter()
            .filter(|d| d.scope == agent::DialogScope::Launch)
            .collect(),
        None => LAUNCH_DIALOGS.clone(),
    }
}

/// The readiness indicators to look for in a pane running `agent_name`.
///
/// Same reasoning: "Ask anything" appearing in a Claude pane used to read as
/// OpenCode being ready. Unknown agents keep the flat list.
fn active_indicators_for(agent_name: Option<&str>) -> Vec<&'static str> {
    match agent_name.and_then(agent::spec) {
        // `scoped_indicators` are added only here, where the pane's agent is
        // known. They are deliberately absent from AGENT_ACTIVE_INDICATORS: pi's
        // `%/` occurs in ordinary output, so in the flat list it would report an
        // exited claude or codex as still running.
        Some(spec) => spec
            .active_indicators
            .iter()
            .chain(spec.scoped_indicators.iter())
            .copied()
            .collect(),
        None => AGENT_ACTIVE_INDICATORS.clone(),
    }
}

/// The indicators distinctive enough to match anywhere in a pane.
///
/// The counterpart of [`scoped_indicators_for`]: the split exists because the two
/// halves need different match windows. Only [`is_agent_active`] uses the merged
/// list, and it looks at the tail either way.
fn flat_indicators_for(agent_name: Option<&str>) -> Vec<&'static str> {
    match agent_name.and_then(agent::spec) {
        Some(spec) => spec.active_indicators.to_vec(),
        None => AGENT_ACTIVE_INDICATORS.clone(),
    }
}

/// Indicators that are only meaningful at the *bottom* of the pane.
///
/// pi's `%/` is one field of its footer and also occurs in ordinary output
/// (`Coverage: 85%/90%`), so matching it against a whole capture — scrollback
/// included — finds an earlier turn's text rather than a live footer. On the
/// agent-switch path that scrollback belongs to the *previous* agent, so a stale
/// percentage would end the readiness wait before the new agent had execed.
fn scoped_indicators_for(agent_name: Option<&str>) -> &'static [&'static str] {
    agent_name
        .and_then(agent::spec)
        .map_or(&[][..], |spec| spec.scoped_indicators)
}

/// How many lines from the bottom of a capture count as "live now".
const PANE_TAIL_LINES: usize = 5;

/// The bottom `n` lines of a captured pane, trailing blank rows dropped first.
///
/// `capture-pane -p` emits one line per pane *row*, so the raw end of a capture
/// is padding whenever the agent's output has not filled the pane; anchoring the
/// window there would look at nothing. Same reasoning as `composer_holds`.
fn pane_tail(content: &str, n: usize) -> String {
    let mut lines: Vec<&str> = content.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(n);
    lines[start..].join("\n")
}

/// Per-launch state for [`dismiss_launch_dialog`].
#[derive(Default)]
struct LaunchDialogState {
    /// Answers sent per dialog, and a hash of the pane at the last attempt.
    /// Indexed by position in [`LAUNCH_DIALOGS`], so it grows with the table
    /// rather than needing a hand-maintained size.
    attempts: Vec<(u8, u64)>,
}

/// An agent TUI that has not yet started reading stdin drops the keystrokes, so
/// a single attempt is not enough on a slow machine.
///
/// Sized to span the whole readiness budget (30 polls in step 1 + 30 in the
/// settle loop, one per second), because that is how long the agent has to
/// become ready. An earlier value of 5 exhausted itself in the first five
/// seconds of an emulated swebench container — every answer was dropped, and
/// Claude sat on the bypass warning for the rest of the run. The cap is only a
/// backstop against a pattern that matches something which is not a dialog;
/// the real guard is that a retry requires an unchanged pane.
const LAUNCH_DIALOG_MAX_ATTEMPTS: u8 = 60;

fn hash_of(content: &str) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    content.hash(&mut h);
    h.finish()
}

/// Answer any `Session`-scope dialog this agent shows, visible in `content`.
///
/// Unlike the launch dialogs these are matched **only** against their own agent,
/// because the caller knows which agent occupies the pane and these prompts can
/// appear at any point in a session rather than once at startup.
fn answer_session_dialogs(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    agent_name: &str,
    content: &str,
) {
    let Some(spec) = agent::spec(agent_name) else {
        return;
    };
    for dialog in spec
        .dialogs
        .iter()
        .filter(|d| d.scope == agent::DialogScope::Session)
    {
        if dialog.matches(content) {
            for key in dialog.answer {
                let _ = tmux_ops.send_key(target, key);
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        }
    }
}

/// Answer any known first-launch dialog visible in `content`.
///
/// Retries only while **nothing has changed** since the last attempt: an
/// unchanged pane means the keystrokes were dropped, whereas any redraw means
/// they landed and resending would type a stray "2" into the agent's live
/// composer. Capped at [`LAUNCH_DIALOG_MAX_ATTEMPTS`] so a pattern that matches
/// something which is not really a dialog cannot hammer the pane forever.
///
/// Returns true when an answer was sent.
/// The security-decision dialog visible in `content`, if any.
///
/// Detection is deliberately separate from answering: with `auto_trust` off agtx
/// still needs to *know* a trust prompt is up — that is what turns the task card
/// `Blocked` — it just does not answer it.
fn visible_security_dialog(agent_name: Option<&str>, content: &str) -> Option<&'static str> {
    launch_dialogs_for(agent_name)
        .into_iter()
        .find(|d| d.security && d.matches(content))
        .and_then(|d| d.patterns.first().copied())
}

fn dismiss_launch_dialog(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    agent_name: Option<&str>,
    content: &str,
    state: &mut LaunchDialogState,
    auto_trust: bool,
) -> bool {
    let dialogs = launch_dialogs_for(agent_name);
    let content_hash = hash_of(content);
    if state.attempts.len() < dialogs.len() {
        state.attempts.resize(dialogs.len(), (0, 0));
    }
    for (i, dialog) in dialogs.iter().enumerate() {
        if !dialog.matches(content) {
            continue;
        }
        // Vouching for a directory, or accepting unattended tool execution, is the
        // user's call. Left alone the agent simply waits, and the task shows
        // `Blocked` with the reason — nothing is lost, because a prompt handed to
        // the process in argv is queued behind the dialog rather than eaten by it.
        if dialog.security && !auto_trust {
            continue;
        }
        let (attempts, last_hash) = state.attempts[i];
        if attempts >= LAUNCH_DIALOG_MAX_ATTEMPTS {
            continue;
        }
        // The pane redrew since the last answer — it landed; the lingering text
        // is just the previous frame. Do not send into a live composer.
        if attempts > 0 && last_hash != content_hash {
            continue;
        }
        for key in dialog.answer {
            let _ = tmux_ops.send_key(target, key);
            std::thread::sleep(std::time::Duration::from_millis(200));
        }
        state.attempts[i] = (attempts + 1, content_hash);
        return true;
    }
    false
}

fn wait_for_agent_ready(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    agent_name: Option<&str>,
    auto_trust: bool,
) -> Option<String> {
    // Step 1: detect the ready signal (up to 30s).
    // Three detection methods, whichever fires first:
    //   1. Agent process detected via pane_current_command (Claude, Codex, Copilot)
    //   2. Known ready indicator in pane content (Gemini's "Type your message")
    //   3. Content stabilization: pane unchanged for 3s after >=3 changes (universal fallback)
    let mut last_content = String::new();
    let mut stable_ticks: u32 = 0;
    let mut change_count: u32 = 0;
    // Shared with the settle loop below so attempt counts carry across both.
    let mut dialog_state = LaunchDialogState::default();

    for _ in 0..30 {
        // 30s (30 * 1s)
        std::thread::sleep(std::time::Duration::from_secs(1));

        // A blocking dialog is checked BEFORE anything that can break out of this
        // loop. A native-binary agent (claude, grok) changes
        // `pane_current_command` to its own name the moment it execs — before it
        // has rendered anything — so Check 1 used to win this race and leave the
        // dialog standing, with the task prompt then typed into the menu.
        let content = tmux_ops.capture_pane(target).ok();
        if let Some(ref c) = content {
            if !auto_trust && visible_security_dialog(agent_name, c).is_some() {
                // Parked awaiting a human. Returning `None` is what stops the
                // caller typing the task into a menu that ignores text — the
                // failure this whole path exists to prevent. Agents on the argv
                // lane never reach here: their prompt is already queued in the
                // process and runs the moment the user answers.
                return None;
            }
            if dismiss_launch_dialog(
                tmux_ops,
                target,
                agent_name,
                c,
                &mut dialog_state,
                auto_trust,
            ) {
                // Keep looping rather than breaking: the answer may have been
                // dropped by a TUI that was not reading stdin yet, and the
                // readiness checks below would otherwise run against a pane
                // that is still showing the dialog.
                last_content = String::new();
                stable_ticks = 0;
                change_count = 0;
                continue;
            }
        }

        // Check 1: agent process detected via pane_current_command
        if !is_pane_at_shell(tmux_ops.as_ref(), target) {
            break;
        }

        // Check 2 & 3: pane content checks
        if let Some(content) = content {
            // Check 2: known ready indicator in pane content. Flat indicators
            // are distinctive enough to match anywhere; `scoped_indicators` are
            // not, so they get the same tail window `is_agent_active` uses —
            // otherwise the previous agent's scrollback answers for the new one.
            let scoped_hit = {
                let tail = pane_tail(&content, PANE_TAIL_LINES);
                scoped_indicators_for(agent_name)
                    .iter()
                    .any(|s| tail.contains(s))
            };
            if scoped_hit
                || flat_indicators_for(agent_name)
                    .iter()
                    .any(|s| content.contains(s))
            {
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
            // The dialog can render *after* the process is detectable, so it must
            // be watched for here too — this is the case that actually bit in a
            // swebench container: claude exec'd, Check 1 broke out of step 1, and
            // the warning appeared only once we were already in this loop.
            if !auto_trust && visible_security_dialog(agent_name, &content).is_some() {
                return None;
            }
            if dismiss_launch_dialog(
                tmux_ops,
                target,
                agent_name,
                &content,
                &mut dialog_state,
                auto_trust,
            ) {
                last_content = String::new();
                stable_ticks = 0;
                continue;
            }
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

    // Fixed 2s grace period after stability is detected. There is a small window
    // where the agent's prompt indicator is visible but the input buffer is not
    // yet accepting keystrokes (e.g. Claude finishing async tool registration).
    std::thread::sleep(std::time::Duration::from_secs(2));

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

/// Home directory for agent-global config that agtx writes as a trust
/// side-effect — today only Codex's `~/.codex/config.toml`.
///
/// `AGTX_AGENT_HOME` overrides `$HOME` so tests can exercise that write without
/// appending temp-dir trust entries to the real user's config, which is
/// otherwise exactly what running the suite does.
fn agent_trust_home() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("AGTX_AGENT_HOME") {
        if !dir.is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    std::env::var("HOME").ok().map(PathBuf::from)
}

/// Write skill files to a worktree's .agtx/skills/ directory and agent-native discovery paths.
/// `agent_names` determines which native paths to use (e.g. `.claude/commands/agtx/` for Claude).
/// When multiple agents are configured for different phases, skills are deployed for all of them.
/// The `agtx hook` invocation registered for one agent.
///
/// Task-agnostic: the hook reads AGTX_TASK_ID / AGTX_WORKTREE from the window
/// env. Baking the task id in breaks `skip_worktree`, where every task shares one
/// config file.
fn hook_command(agtx_bin: &str, agent: &str) -> String {
    format!("{} hook --env {}", agtx_bin, agent)
}

/// Build the `hooks` block for a worktree's `.claude/settings.local.json`.
///
/// Event names verified against claude 2.1.247. Unregistered names are ignored,
/// so listing one an older build does not know is harmless.
#[cfg(test)]
fn claude_hook_settings(agtx_bin: &str) -> serde_json::Value {
    claude_shaped_hooks(agtx_bin, "claude", agent::HookConfigKind::ClaudeSettings)
}

/// The `{Event: [{matcher, hooks: [{type, command}]}]}` shape, which four of the
/// six agents share. Where the object goes and what the events are called still
/// differ — that is `write_hook_config` and [`hook_events`].
fn claude_shaped_hooks(
    agtx_bin: &str,
    agent_name: &str,
    kind: agent::HookConfigKind,
) -> serde_json::Value {
    let base = hook_command(agtx_bin, agent_name);
    let argv_event = agent::spec(agent_name)
        .is_some_and(|s| s.hook_event_source == agent::HookEventSource::Argv);
    let mut out = serde_json::Map::new();
    for (event, matcher) in hook_status::hook_events(kind) {
        let command = if argv_event {
            format!("{} --event {}", base, event)
        } else {
            base.clone()
        };
        let mut group = serde_json::json!({
            "hooks": [{ "type": "command", "command": command }]
        });
        if let (Some(m), Some(obj)) = (matcher, group.as_object_mut()) {
            obj.insert("matcher".to_string(), serde_json::json!(m));
        }
        out.insert((*event).to_string(), serde_json::Value::Array(vec![group]));
    }
    serde_json::Value::Object(out)
}

/// Re-deploy agent configs for worktrees that were set up by a different agtx
/// binary.
///
/// The hook command and every MCP config embed an absolute path from
/// `current_exe()`. After `cargo install`, a Homebrew upgrade, or a `cargo clean`
/// following a debug-build session, those paths dangle: hooks stop reporting (the
/// board silently falls back to the pane heuristic) and the task's MCP server
/// stops resolving. Neither failure is visible.
///
/// Runs on a background thread at startup and rewrites only worktrees whose
/// marker disagrees with the running binary.
fn refresh_stale_worktree_configs(
    stale_candidates: Vec<(String, Option<String>)>,
    project_path: PathBuf,
    agent_names: Vec<String>,
    agent_hooks: bool,
) {
    let Ok(current_bin) = std::env::current_exe() else {
        return;
    };
    let current_bin = current_bin.to_string_lossy().to_string();

    std::thread::spawn(move || {
        let mut plugin_cache: HashMap<Option<String>, Option<WorkflowPlugin>> = HashMap::new();
        let refs: Vec<&str> = agent_names.iter().map(|s| s.as_str()).collect();

        for (worktree, plugin_name) in stale_candidates {
            let wt = Path::new(&worktree);
            if !wt.exists() {
                continue;
            }
            // A missing marker means the worktree predates the marker itself, so
            // its paths cannot be verified — redeploy to be sure.
            if read_deploy_marker(wt).as_deref() == Some(current_bin.as_str()) {
                continue;
            }
            let plugin = plugin_cache
                .entry(plugin_name.clone())
                .or_insert_with(|| match &plugin_name {
                    Some(name) => WorkflowPlugin::load(name, Some(project_path.as_path()))
                        .ok()
                        .or_else(|| skills::load_bundled_plugin(name)),
                    None => skills::load_bundled_plugin("agtx"),
                })
                .clone();

            tracing::info!(
                worktree = %worktree,
                "Re-deploying agent configs: worktree was set up by a different agtx binary"
            );
            write_skills_to_worktree(&worktree, &project_path, &plugin, &refs, agent_hooks);
        }
    });
}

/// True when a hook command is one agtx wrote, regardless of where the binary
/// lived at the time. See `merge_claude_hooks`.
fn is_agtx_hook_command(command: &str) -> bool {
    command.contains(" hook --env")
}

/// Records which agtx binary deployed a worktree's agent configs.
///
/// The absolute path from `current_exe()` is baked into the hook command and
/// every MCP config, so moving or reinstalling agtx silently breaks both for
/// worktrees deployed earlier. This marker makes the mismatch detectable in O(1)
/// per task instead of parsing seven different config formats.
const DEPLOY_MARKER: &str = ".agtx/deployed-by";

fn read_deploy_marker(worktree: &Path) -> Option<String> {
    std::fs::read_to_string(worktree.join(DEPLOY_MARKER))
        .ok()
        .map(|s| s.trim().to_string())
}

/// Compose the opening message handed to an agent at launch: the phase skill
/// command followed by the task prompt.
///
/// Same text `send_skill_and_prompt` builds for the combined-send agents, but
/// given to the process instead of typed at it.
fn compose_launch_text(skill_cmd: Option<&str>, prompt: &str) -> String {
    match (skill_cmd, prompt.trim().is_empty()) {
        (Some(cmd), false) => format!("{}\n\n{}", cmd, prompt),
        (Some(cmd), true) => cmd.to_string(),
        (None, false) => prompt.to_string(),
        (None, true) => String::new(),
    }
}

/// Environment identifying which task a tmux window belongs to.
///
/// Set on the window (tmux `-e`) so the agent — and any hook it spawns —
/// inherits it. Agent hooks are registered once with a task-agnostic command and
/// read these to know what they are reporting about, which is what lets several
/// tasks share one `.claude/settings.local.json` under `skip_worktree`.
fn agtx_task_env(task_id: &str, worktree: &str) -> Vec<(String, String)> {
    vec![
        ("AGTX_TASK_ID".to_string(), task_id.to_string()),
        ("AGTX_WORKTREE".to_string(), worktree.to_string()),
    ]
}

/// Merge agtx's hook entries into an existing `hooks` object, preserving the
/// user's own entries on the same events.
///
/// agtx's previous entries (identified by the agtx binary path in the command,
/// the same way orca matches on its managed script filename) are dropped first,
/// so re-running against an existing worktree replaces them rather than
/// accumulating duplicates that would fire the hook N times per event.
fn merge_claude_hooks(
    settings: &mut serde_json::Map<String, serde_json::Value>,
    ours: serde_json::Value,
) {
    let mut hooks = settings
        .get("hooks")
        .and_then(|h| h.as_object())
        .cloned()
        .unwrap_or_default();
    merge_hook_events(&mut hooks, ours);
    // Pruning to nothing removes the key rather than leaving `"hooks": {}`, so a
    // worktree with hooks turned off is byte-identical to one that never had them.
    if hooks.is_empty() {
        settings.remove("hooks");
    } else {
        settings.insert("hooks".to_string(), serde_json::Value::Object(hooks));
    }
}

/// True when a handler entry is one agtx wrote, in either shape agents accept:
/// the Claude wrapper `{hooks: [{command}]}` or cursor's flat `{command}`.
///
/// Matched on the invocation, not the binary path — the path changes when agtx is
/// moved or reinstalled, and a prefix match would then stop recognising our own
/// entries, leaving the stale one behind and appending a duplicate.
fn is_agtx_hook_entry(def: &serde_json::Value) -> bool {
    if def["command"].as_str().is_some_and(is_agtx_hook_command) {
        return true;
    }
    def["hooks"].as_array().is_some_and(|hooks| {
        hooks
            .iter()
            .any(|h| h["command"].as_str().is_some_and(is_agtx_hook_command))
    })
}

/// Replace agtx's handlers under each event in `ours`, leaving the user's own
/// entries on those events untouched and every other event alone.
///
/// An event whose `ours` value is an empty array is *pruned*: agtx's handlers are
/// stripped and nothing replaces them, and the key is dropped entirely when
/// nothing else was registered under it. That is what turning `agent_hooks` off
/// uses to unregister from a worktree that already has hooks deployed.
fn merge_hook_events(
    existing: &mut serde_json::Map<String, serde_json::Value>,
    ours: serde_json::Value,
) {
    let Some(ours) = ours.as_object() else {
        return;
    };
    for (event, our_defs) in ours {
        let mut defs: Vec<serde_json::Value> = existing
            .get(event)
            .and_then(|d| d.as_array())
            .map(|a| {
                a.iter()
                    .filter(|d| !is_agtx_hook_entry(d))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        if let Some(arr) = our_defs.as_array() {
            defs.extend(arr.iter().cloned());
        }
        if defs.is_empty() {
            existing.remove(event);
        } else {
            existing.insert(event.clone(), serde_json::Value::Array(defs));
        }
    }
}

fn write_skills_to_worktree(
    worktree_path: &str,
    project_path: &Path,
    plugin: &Option<WorkflowPlugin>,
    agent_names: &[&str],
    agent_hooks: bool,
) {
    // Replay the project's existing trust onto this worktree, for the one agent
    // that needs it. Antigravity matches trusted paths **exactly** — no ancestor
    // inheritance at any depth — so a user who already trusted the project would
    // otherwise face a fresh prompt for every task.
    //
    // This is the right call site for both paths: worktree creation *and* an agent
    // switch land here, and with a multi-agent config the switched-in agent sees
    // the worktree for the first time at switch time, not at creation.
    //
    // It never grants trust that does not already exist — `seed_from_project` is a
    // no-op unless the project root is in the agent's own store. See
    // `agent::trust`.
    if let Some(home) = agent_trust_home() {
        for agent_name in agent_names {
            if !agent::trust::needs_seeding(agent_name) {
                continue;
            }
            let _ = agent::trust::seed_from_project(
                agent_name,
                project_path,
                Path::new(worktree_path),
                &home,
            );
        }
    }

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

    // Write project-scoped MCP server config for each configured agent.
    // Use the project root path (not the worktree path) so the MCP server opens
    // the correct project DB where tasks are stored.
    let agtx_bin = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from("agtx"))
        .to_string_lossy()
        .to_string();
    let project_path_str = project_path.to_string_lossy().to_string();
    // Marker for the startup drift check; see DEPLOY_MARKER.
    let _ = std::fs::write(Path::new(worktree_path).join(DEPLOY_MARKER), &agtx_bin);
    for agent_name in agent_names {
        if let Some(spec) = agent::spec(agent_name) {
            write_mcp_config(spec, worktree_path, &project_path_str, &agtx_bin);
            // After the MCP writer, never before: two agents keep their hooks in
            // the same file as their MCP config, and both writers
            // read-modify-write it. Called even when hooks are off, so a worktree
            // deployed with them on gets them removed rather than left firing.
            write_hook_config(spec, worktree_path, &agtx_bin, agent_hooks);
        }
    }

    // Write to agent-native discovery paths (e.g. .claude/commands/agtx/)
    // Deploy for all configured agents so skills are available across phase transitions
    for agent_name in agent_names {
        let Some(spec) = agent::spec(agent_name) else {
            continue;
        };
        if let Some((base_dir, namespace)) = spec.skill_dir {
            let native_dir = if namespace.is_empty() {
                Path::new(worktree_path).join(base_dir)
            } else {
                Path::new(worktree_path).join(base_dir).join(namespace)
            };
            let _ = std::fs::create_dir_all(&native_dir);

            for (skill_dir_name, default_content) in skills::BUILTIN_SKILLS {
                let content =
                    resolve_skill_content(plugin, skill_dir_name, project_path, default_content);
                write_skill_file(spec, skill_dir_name, &content, &native_dir);
            }
        }
    }
}

/// Write one agent's lifecycle-hook config into its worktree.
///
/// Selected by [`HookConfigKind`](agent::HookConfigKind); `None` means the agent
/// reports nothing and the pane-hash heuristic runs unchanged.
///
/// Every path here is inside the worktree — all six agents accept a project-local
/// hook config, so agtx never writes into `~/.codex`, `~/.gemini` or `~/.cursor`
/// and removing the worktree removes the registration.
///
/// The writers whose file is shared with an MCP config, or may be committed,
/// merge; the rest own their file and are written outright.
fn write_hook_config(spec: &agent::AgentSpec, worktree_path: &str, agtx_bin: &str, enabled: bool) {
    let Some(kind) = spec.hook_config else {
        return;
    };
    let wt = Path::new(worktree_path);
    // When hooks are off this is an *empty* set of handlers under the same event
    // names, which `merge_hook_events` reads as "strip agtx's and add nothing".
    // Skipping the call instead would leave a worktree deployed while hooks were
    // on firing `agtx hook` forever after they were turned off.
    let ours = if enabled {
        claude_shaped_hooks(agtx_bin, spec.name, kind)
    } else {
        empty_hook_events(kind)
    };

    match kind {
        // Shares `.claude/settings.local.json` with the MCP preflight keys
        // `write_mcp_config` just wrote.
        agent::HookConfigKind::ClaudeSettings => {
            merge_hooks_into_json_settings(&wt.join(".claude").join("settings.local.json"), ours);
        }
        // Same shape; `.gemini/settings.json` also carries `mcpServers` and
        // `trust: true`.
        agent::HookConfigKind::GeminiSettings => {
            merge_hooks_into_json_settings(&wt.join(".gemini").join("settings.json"), ours);
        }
        // `.codex/hooks.json` is auto-discovered — no pointer in
        // `.codex/config.toml`, whose `hooks` key is a table, not a path;
        // assigning a string to it makes codex reject the entire config with
        // "invalid type: string, expected struct HooksToml" and lose its MCP
        // server with it. The events sit under a `hooks` object beside an
        // optional `description`. codex-cli 0.144.5.
        agent::HookConfigKind::CodexHooksJson => {
            let dir = wt.join(".codex");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("hooks.json");
            let mut root = read_json_object(&path);
            let mut hooks = root
                .get("hooks")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            merge_hook_events(&mut hooks, ours);
            if hooks.is_empty() {
                root.remove("hooks");
            } else {
                root.entry("description".to_string())
                    .or_insert_with(|| serde_json::json!("agtx phase status"));
                root.insert("hooks".to_string(), serde_json::Value::Object(hooks));
            }
            write_json(&path, &serde_json::Value::Object(root));
        }
        // `{version, hooks: {event: [{command}]}}`. The one agent that does not
        // take the Claude-shaped `{hooks: [{type, command}]}` wrapper — its
        // handlers are a flat list of `{command}`, and written the other way the
        // file parses, loads, and fires nothing. cursor-agent 2026.08.25.
        agent::HookConfigKind::CursorHooksJson => {
            let command = hook_command(agtx_bin, spec.name);
            let mut ours = serde_json::Map::new();
            for (event, _) in hook_status::hook_events(kind) {
                let defs = if enabled {
                    serde_json::json!([{ "command": command }])
                } else {
                    serde_json::json!([])
                };
                ours.insert((*event).to_string(), defs);
            }
            // Merges, like every other writer whose file the project may ship. A
            // worktree is a full checkout, so a repo that tracks
            // `.cursor/hooks.json` has it here — and clobbering it would both
            // destroy the user's hooks and leave a modified tracked file on the
            // task branch for the agent to commit. `.cursor` is not in
            // AGENT_CONFIG_DIRS either, so the diff view would not hide it.
            let dir = wt.join(".cursor");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("hooks.json");
            let mut root = read_json_object(&path);
            let mut hooks = root
                .get("hooks")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            merge_hook_events(&mut hooks, serde_json::Value::Object(ours));
            if hooks.is_empty() {
                root.remove("hooks");
            } else {
                root.insert("version".to_string(), serde_json::json!(1));
                root.insert("hooks".to_string(), serde_json::Value::Object(hooks));
            }
            write_json(&path, &serde_json::Value::Object(root));
        }
        // Grok scans the directory, so agtx owns one file in it and never merges.
        agent::HookConfigKind::GrokHooksJson => {
            let dir = wt.join(".grok").join("hooks");
            let _ = std::fs::create_dir_all(&dir);
            write_json(
                &dir.join("agtx.json"),
                &serde_json::json!({ "hooks": ours }),
            );
        }
        // Keyed by hook *name* first, event second; every handler carries its own
        // `--event` because the payload has no event name. `.agents/` is
        // vendor-neutral and may be committed, so this merges.
        agent::HookConfigKind::AntigravityHooksJson => {
            let dir = wt.join(".agents");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("hooks.json");
            let mut root = read_json_object(&path);
            // Keyed by hook name, so agtx owns one key and pruning is a removal
            // rather than a per-event filter.
            if enabled {
                root.insert(
                    AGTX_HOOK_NAME.to_string(),
                    antigravity_hook_spec(agtx_bin, spec, kind),
                );
            } else {
                root.remove(AGTX_HOOK_NAME);
            }
            write_json(&path, &serde_json::Value::Object(root));
        }
    }
}

/// The same event keys with no handlers, which `merge_hook_events` prunes.
fn empty_hook_events(kind: agent::HookConfigKind) -> serde_json::Value {
    let mut out = serde_json::Map::new();
    for (event, _) in hook_status::hook_events(kind) {
        out.insert((*event).to_string(), serde_json::json!([]));
    }
    serde_json::Value::Object(out)
}

/// The name agtx registers its hooks under, where the format is keyed by name.
const AGTX_HOOK_NAME: &str = "agtx";

/// Antigravity's per-event handlers, each carrying its own `--event`.
///
/// Its two event shapes are not interchangeable: tool events are grouped under a
/// `matcher`, the rest are a flat list of handlers, and the wrong shape is
/// silently ignored.
fn antigravity_hook_spec(
    agtx_bin: &str,
    spec: &agent::AgentSpec,
    kind: agent::HookConfigKind,
) -> serde_json::Value {
    let handler = |event: &str| {
        let base = hook_command(agtx_bin, spec.name);
        // `--event` only where the agent's payload carries no event name. Read
        // from the spec rather than assumed, so an agent that later needs the
        // same treatment gets it by setting one field.
        let command = match spec.hook_event_source {
            agent::HookEventSource::Argv => format!("{} --event {}", base, event),
            agent::HookEventSource::Payload => base,
        };
        serde_json::json!({ "type": "command", "command": command })
    };
    let mut out = serde_json::Map::new();
    for (event, matcher) in hook_status::hook_events(kind) {
        let entry = match matcher {
            Some(m) => serde_json::json!([{ "matcher": m, "hooks": [handler(event)] }]),
            None => serde_json::json!([handler(event)]),
        };
        out.insert((*event).to_string(), entry);
    }
    serde_json::Value::Object(out)
}

/// Read a JSON file as an object, or an empty one if it is missing or not an
/// object. Never an error: a config the user can fix is not a reason to refuse to
/// set up their worktree.
fn read_json_object(path: &Path) -> serde_json::Map<String, serde_json::Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_json(path: &Path, value: &serde_json::Value) {
    let _ = std::fs::write(
        path,
        serde_json::to_string_pretty(value).unwrap_or_default(),
    );
}

/// Merge agtx's hooks into a settings file it shares with other keys.
fn merge_hooks_into_json_settings(path: &Path, ours: serde_json::Value) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut settings = read_json_object(path);
    merge_claude_hooks(&mut settings, ours);
    write_json(path, &serde_json::Value::Object(settings));
}

/// Insert agtx's entry into a `mcpServers` JSON file without disturbing the rest.
///
/// Shared by the two `…Merge` JSON kinds (antigravity, pi), which differ only in
/// directory and filename: their file is vendor-neutral or written back by the
/// agent's own tooling, so the project's other servers — and any sibling
/// top-level keys — have to survive. A missing or unparseable file starts from an
/// empty object rather than failing, on the same best-effort footing as the rest
/// of worktree setup.
fn merge_mcp_servers_json(dir: &Path, filename: &str, agtx_bin: &str, project_path_str: &str) {
    let _ = std::fs::create_dir_all(dir);
    let path = dir.join(filename);
    let mut root = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    if !root["mcpServers"].is_object() {
        root["mcpServers"] = serde_json::json!({});
    }
    root["mcpServers"]["agtx"] = serde_json::json!({
        "command": agtx_bin,
        "args": ["mcp-serve", project_path_str]
    });
    let _ = std::fs::write(
        &path,
        serde_json::to_string_pretty(&root).unwrap_or_default(),
    );
}

/// Write the project-scoped MCP server config for one agent into its worktree.
///
/// Selected by [`McpConfigKind`](agent::McpConfigKind) rather than agent name.
/// The variants are genuinely seven, not one parameterised writer: the formats
/// differ (JSON vs TOML, `mcpServers` vs `mcp_servers` vs `mcp`), two must
/// **merge** rather than overwrite because their file may already be tracked in
/// the repo, and two carry a side-effect beyond the config file itself — so the
/// `…Merge` names mark where clobbering a user's file is the failure mode.
///
/// `project_path_str` is the *project root*, not the worktree, so the server
/// opens the project DB where tasks actually live.
fn write_mcp_config(
    spec: &agent::AgentSpec,
    worktree_path: &str,
    project_path_str: &str,
    agtx_bin: &str,
) {
    let Some(kind) = spec.mcp_config else {
        return;
    };
    match kind {
        agent::McpConfigKind::ClaudeJson => {
            let cfg = serde_json::json!({
                "mcpServers": {
                    "agtx": { "command": agtx_bin, "args": ["mcp-serve", &project_path_str] }
                }
            });
            let _ = std::fs::write(
                Path::new(worktree_path).join(".mcp.json"),
                serde_json::to_string_pretty(&cfg).unwrap_or_default(),
            );
            // Merge into any existing settings rather than replacing them:
            // `.claude` is in AGENT_CONFIG_DIRS, so a project that ships its own
            // settings.local.json has it copied into every worktree, and a plain
            // write would silently drop the user's permissions/env/hooks.
            // Same merge-don't-overwrite rule the grok and antigravity writers follow.
            let claude_dir = Path::new(worktree_path).join(".claude");
            let _ = std::fs::create_dir_all(&claude_dir);
            let settings_path = claude_dir.join("settings.local.json");
            let mut settings = std::fs::read_to_string(&settings_path)
                .ok()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
                .filter(|v| v.is_object())
                .unwrap_or_else(|| serde_json::json!({}));

            if let Some(obj) = settings.as_object_mut() {
                // Pre-trust the agtx MCP server so Claude doesn't show an interactive
                // trust dialog when the agent window opens for the first time.
                obj.insert(
                    "enableAllProjectMcpServers".to_string(),
                    serde_json::json!(true),
                );
                // agtx always launches Claude with --dangerously-skip-permissions, and
                // since Claude Code ~2.1 that mode is gated behind an interactive
                // "Yes, I accept" dialog. A worktree is a brand-new directory every
                // time, so without this the agent parks on that dialog and the task
                // prompt is typed into the menu. (IS_SANDBOX=1 covers only the
                // separate root-user check, not this one.)
                //
                // Note this is a *settings* key, so unlike `hasTrustDialogAccepted`
                // (a ~/.claude.json project record) a worktree-local file is enough.
                obj.insert(
                    "skipDangerousModePermissionPrompt".to_string(),
                    serde_json::json!(true),
                );
            }
            let _ = std::fs::write(
                &settings_path,
                serde_json::to_string_pretty(&settings).unwrap_or_default(),
            );
        }
        agent::McpConfigKind::CodexToml => {
            let toml = format!(
                "[mcp_servers.agtx]\ncommand = \"{}\"\nargs = [\"mcp-serve\", \"{}\"]\n",
                agtx_bin, project_path_str
            );
            let dir = Path::new(worktree_path).join(".codex");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(dir.join("config.toml"), toml);

            // agtx used to append `[projects."<worktree>"] trust_level = "trusted"`
            // to the user's global ~/.codex/config.toml here, on the belief that
            // codex would otherwise ignore the project-local config written above
            // and lose its MCP server.
            //
            // Measured against codex 0.144.5 and removed. Codex resolves trust to
            // the **git repository root** — its own dialog says so: "You're in a
            // subdirectory of a Git project. Trusting will apply to the repository
            // root." With only the root trusted, a worktree beneath it showed no
            // dialog *and* loaded its own .codex/config.toml: `/mcp` listed agtx
            // either way, with or without a per-worktree entry. The entry bought
            // nothing, and one had accumulated per worktree in the user's config.
            //
            // What remains is the case where the user never trusted the project at
            // all. That is a decision for them, surfaced rather than made here —
            // see `agent_trust`.
        }
        agent::McpConfigKind::GeminiJson => {
            // Merge, don't overwrite. `.gemini` is in AGENT_CONFIG_DIRS, so a
            // project shipping its own settings.json has it copied into every
            // worktree, and a plain write drops the user's theme, model and any
            // `mcpServers` besides agtx. Same rule as the Claude writer above.
            let dir = Path::new(worktree_path).join(".gemini");
            let _ = std::fs::create_dir_all(&dir);
            let path = dir.join("settings.json");
            let mut settings = read_json_object(&path);
            let mut servers = settings
                .get("mcpServers")
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default();
            servers.insert(
                "agtx".to_string(),
                serde_json::json!({
                    "command": agtx_bin,
                    "args": ["mcp-serve", &project_path_str],
                    "trust": true
                }),
            );
            settings.insert("mcpServers".to_string(), serde_json::Value::Object(servers));
            write_json(&path, &serde_json::Value::Object(settings));
        }
        agent::McpConfigKind::CursorJson => {
            let cfg = serde_json::json!({
                "mcpServers": {
                    "agtx": { "command": agtx_bin, "args": ["mcp-serve", &project_path_str] }
                }
            });
            let dir = Path::new(worktree_path).join(".cursor");
            let _ = std::fs::create_dir_all(&dir);
            let _ = std::fs::write(
                dir.join("mcp.json"),
                serde_json::to_string_pretty(&cfg).unwrap_or_default(),
            );
        }
        agent::McpConfigKind::GrokTomlMerge => {
            // Grok reads project-scoped MCP servers from .grok/config.toml (TOML, not JSON),
            // walking from the worktree up to the git root.
            let esc = |v: &str| v.replace('\\', "\\\\").replace('"', "\\\"");
            let cfg = format!(
                "[mcp_servers.agtx]\ncommand = \"{}\"\nargs = [\"mcp-serve\", \"{}\"]\n",
                esc(&agtx_bin),
                esc(&project_path_str)
            );
            let dir = Path::new(worktree_path).join(".grok");
            let _ = std::fs::create_dir_all(&dir);
            // A repo may already ship a .grok/config.toml — append the agtx table
            // instead of clobbering the project's own settings.
            let path = dir.join("config.toml");
            let existing = std::fs::read_to_string(&path).unwrap_or_default();
            if !existing.contains("[mcp_servers.agtx]") {
                let merged = if existing.trim().is_empty() {
                    cfg
                } else {
                    format!("{}\n\n{}", existing.trim_end(), cfg)
                };
                let _ = std::fs::write(&path, merged);
            }
        }
        agent::McpConfigKind::AntigravityJsonMerge => {
            // Antigravity reads workspace MCP servers from .agents/mcp_config.json
            // (JSON, `mcpServers`). `.agents/` is vendor-neutral and may already be
            // tracked in the repo, so merge the agtx entry into any existing file
            // instead of clobbering the project's own servers.
            merge_mcp_servers_json(
                &Path::new(worktree_path).join(".agents"),
                "mcp_config.json",
                &agtx_bin,
                &project_path_str,
            );
        }
        agent::McpConfigKind::PiJsonMerge => {
            // pi itself has no MCP client; the `pi-mcp-adapter` package provides
            // one and reads `.pi/mcp.json` as its highest-precedence project
            // layer, in the standard `mcpServers` shape. Merged rather than
            // overwritten because the adapter also persists its own per-server
            // `disabled` flags into this file.
            merge_mcp_servers_json(
                &Path::new(worktree_path).join(".pi"),
                "mcp.json",
                &agtx_bin,
                &project_path_str,
            );
        }
        agent::McpConfigKind::OpenCode => {
            let cfg = serde_json::json!({
                "mcp": {
                    "agtx": {
                        "type": "local",
                        "command": [&agtx_bin, "mcp-serve", &project_path_str]
                    }
                }
            });
            let _ = std::fs::write(
                Path::new(worktree_path).join("opencode.json"),
                serde_json::to_string_pretty(&cfg).unwrap_or_default(),
            );
        }
    }
}

/// Write one skill into an agent's native discovery directory, in that agent's
/// layout.
///
/// `native_dir` is the already-created base directory (plus namespace subdir if
/// the agent uses one). Selected by [`SkillLayout`] rather than by agent name, so
/// an agent reusing an existing layout needs no code here.
///
/// This is the single implementation behind both [`deploy_skill`] and
/// [`write_skills_to_worktree`], which each carried their own copy of this branch
/// and had already drifted: the latter treated Claude's format as its `_`
/// fallback, so a future agent with a skill dir but no arm would silently get
/// Claude's `.md` layout from one and nothing from the other.
fn write_skill_file(spec: &agent::AgentSpec, skill_name: &str, content: &str, native_dir: &Path) {
    match spec.skill_layout {
        agent::SkillLayout::CommandFile => {
            let transformed = transform_skill_frontmatter(content);
            let filename = skills::skill_dir_to_filename(skill_name, spec.name);
            let _ = std::fs::write(native_dir.join(&filename), transformed);
        }
        agent::SkillLayout::GeminiToml => {
            let description = skills::extract_description(content)
                .unwrap_or_else(|| format!("agtx {} skill", skill_name));
            let toml_content = skills::skill_to_gemini_toml(&description, content);
            let filename = skills::skill_dir_to_filename(skill_name, spec.name);
            let _ = std::fs::write(native_dir.join(&filename), toml_content);
        }
        agent::SkillLayout::SkillDir => {
            let skill_subdir = native_dir.join(skill_name);
            let _ = std::fs::create_dir_all(&skill_subdir);
            let _ = std::fs::write(skill_subdir.join("SKILL.md"), content);
        }
        agent::SkillLayout::OpenCodeFlat => {
            let oc_content = transform_skill_for_opencode(content);
            let filename = skills::skill_dir_to_filename(skill_name, spec.name);
            let _ = std::fs::write(native_dir.join(&filename), oc_content);
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
    let Some(spec) = agent::spec(agent_name) else {
        return;
    };
    if let Some((base_dir, namespace)) = spec.skill_dir {
        let native_dir = if namespace.is_empty() {
            target_dir.join(base_dir)
        } else {
            target_dir.join(base_dir).join(namespace)
        };
        let _ = std::fs::create_dir_all(&native_dir);
        write_skill_file(spec, skill_name, content, &native_dir);
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

/// The fields a trust prompt is asking permission for, with their values.
///
/// These are exactly the three `App::new` strips from an untrusted project, and
/// the prompt shows them verbatim: consenting to a script you cannot see is not
/// consent. Order is fixed so the dialog does not reshuffle between launches.
fn dangerous_fields(config: &ProjectConfig) -> Vec<(&'static str, String)> {
    [
        ("init_script", config.init_script.as_ref()),
        ("cleanup_script", config.cleanup_script.as_ref()),
        ("copy_files", config.copy_files.as_ref()),
    ]
    .into_iter()
    .filter_map(|(name, value)| value.map(|v| (name, v.clone())))
    .collect()
}

/// Draw the config editor: sections down the left, the selected section's
/// fields on the right, and the selected field's help line at the bottom.
///
/// The help line is where a setting that only affects *new* worktrees says so,
/// rather than leaving the user to discover that the change looked like it
/// applied and did not.
fn draw_config_editor(
    state: &AppState,
    editor: &super::config_editor::ConfigEditor,
    frame: &mut Frame,
    area: Rect,
) {
    // Size to the content rather than to a percentage of the screen: a fixed
    // 72x78% box left everything huddled in the top-left corner of a mostly
    // empty dialog. Measured across *all* sections so switching tabs does not
    // resize the box under the cursor.
    let popup_area = config_editor_area(editor, area);
    frame.render_widget(Clear, popup_area);

    let selected = hex_to_color(&state.config.theme.color_selected);
    let text_color = hex_to_color(&state.config.theme.color_text);
    let dimmed = hex_to_color(&state.config.theme.color_dimmed);
    let accent = hex_to_color(&state.config.theme.color_accent);
    let desc = hex_to_color(&state.config.theme.color_description);

    let dirty_marker = if editor.dirty { " ●" } else { "" };
    let block = Block::default()
        .title(format!(" Configuration{dirty_marker} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // section tabs
            Constraint::Min(3),    // fields
            Constraint::Length(2), // help
            Constraint::Length(1), // footer
        ])
        .split(inner);

    // --- section tabs ---
    let mut tab_spans: Vec<Span<'static>> = vec![Span::raw(" ".to_string())];
    for (i, section) in editor.sections.iter().enumerate() {
        let style = if i == editor.section {
            Style::default().fg(selected).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(dimmed)
        };
        tab_spans.push(Span::styled(section.title.to_string(), style));
        if i + 1 < editor.sections.len() {
            tab_spans.push(Span::styled(
                "  ·  ".to_string(),
                Style::default().fg(dimmed),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(tab_spans)), rows[0]);

    // --- fields ---
    let label_width = editor
        .current_section()
        .fields
        .iter()
        .map(|f| f.label.len())
        .max()
        .unwrap_or(12)
        .max(12);

    let mut lines: Vec<Line<'static>> = Vec::new();
    for (i, field) in editor.current_section().fields.iter().enumerate() {
        let is_selected = i == editor.field;
        let marker = if is_selected { " > " } else { "   " };
        let label_style = if is_selected {
            Style::default().fg(selected).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(text_color)
        };

        let mut spans = vec![
            Span::styled(marker.to_string(), label_style),
            Span::styled(format!("{:<label_width$}  ", field.label), label_style),
        ];

        let editing_here = is_selected.then_some(editor.editing.as_ref()).flatten();
        match (&field.kind, editing_here) {
            // A text or colour field open for typing shows a caret.
            (_, Some(super::config_editor::EditState::Text(input))) => {
                spans.push(Span::styled(
                    format!("{}▏", input.as_str()),
                    Style::default().fg(accent).add_modifier(Modifier::BOLD),
                ));
            }
            (FieldKind::Toggle, _) => {
                let on = editor.value(field.id).as_bool();
                spans.push(Span::styled(
                    if on { "[x] on" } else { "[ ] off" }.to_string(),
                    Style::default().fg(if on { accent } else { dimmed }),
                ));
            }
            (FieldKind::Color, _) => {
                let value = editor.value(field.id);
                let hex = value.as_text().to_string();
                // The swatch is the point: a hex string is not a colour anyone
                // can read off the page.
                spans.push(Span::styled(
                    "███ ".to_string(),
                    Style::default().fg(hex_to_color(&hex)),
                ));
                spans.push(Span::styled(hex, Style::default().fg(text_color)));
            }
            (kind, _) => {
                let value = editor.value(field.id);
                let raw = value.as_text();
                let shown = match kind {
                    FieldKind::Choice(choices) => choices
                        .iter()
                        .find(|c| c.value == raw)
                        .map(|c| c.label.clone())
                        .unwrap_or_else(|| raw.to_string()),
                    _ if raw.is_empty() => "(unset)".to_string(),
                    _ => raw.to_string(),
                };
                let style = if raw.is_empty() {
                    Style::default().fg(dimmed)
                } else {
                    Style::default().fg(text_color)
                };
                spans.push(Span::styled(shown, style));
            }
        }
        lines.push(Line::from(spans));
    }

    // Keep the selected row on screen without a scrollbar's worth of machinery:
    // the field list is short and the cursor moves one row at a time.
    let viewport = rows[1].height as usize;
    let offset = editor.field.saturating_sub(viewport.saturating_sub(1));
    frame.render_widget(
        Paragraph::new(Text::from(lines)).scroll((offset as u16, 0)),
        rows[1],
    );

    // --- help / status ---
    let help = editor
        .current_field()
        .map(|f| f.help.to_string())
        .unwrap_or_default();
    let mut help_lines = vec![Line::from(Span::styled(
        format!(" {help}"),
        Style::default().fg(desc),
    ))];
    if let Some(ref status) = editor.status {
        help_lines.push(Line::from(Span::styled(
            format!(" {status}"),
            Style::default().fg(Color::Yellow),
        )));
    }
    frame.render_widget(Paragraph::new(Text::from(help_lines)), rows[2]);

    // --- footer ---
    let footer = if editor.confirming_discard {
        " Unsaved changes. [y] discard  [s] save  [any] back ".to_string()
    } else if let Some(super::config_editor::EditState::Choice { .. }) = editor.editing {
        " [j/k] choose  [Enter] pick  [Esc] cancel ".to_string()
    } else if editor.editing.is_some() {
        " [Enter] accept  [Esc] cancel ".to_string()
    } else {
        " [h/l] section  [j/k] field  [Enter] edit  [C-s] save  [Esc] close ".to_string()
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            footer,
            Style::default().fg(dimmed),
        ))),
        rows[3],
    );

    // --- open choice list, drawn over the form ---
    if let Some(super::config_editor::EditState::Choice { selected: pick }) = editor.editing {
        if let Some(FieldKind::Choice(choices)) = editor.current_field().map(|f| &f.kind) {
            let height = (choices.len() as u16 + 2).min(rows[1].height.max(3));
            // Anchor under the *value* column and one row below the field, so
            // the list reads as a dropdown from the thing it is replacing
            // rather than covering the labels beside it.
            let value_column = 3 + label_width as u16 + 2;
            let row = (editor.field.saturating_sub(offset) as u16).saturating_add(1);
            let list_area = Rect {
                x: rows[1].x + value_column,
                y: rows[1].y + row.min(rows[1].height),
                width: rows[1]
                    .width
                    .saturating_sub(value_column + 2)
                    .min(40)
                    .max(12),
                height,
            };
            let list_area = if list_area.y + list_area.height > inner.y + inner.height {
                Rect {
                    y: (inner.y + inner.height).saturating_sub(list_area.height),
                    ..list_area
                }
            } else {
                list_area
            };
            frame.render_widget(Clear, list_area);
            let items: Vec<ListItem> = choices
                .iter()
                .enumerate()
                .map(|(i, choice)| {
                    let style = if i == pick {
                        Style::default().bg(selected).fg(Color::Black)
                    } else {
                        Style::default().fg(text_color)
                    };
                    ListItem::new(format!(" {} ", choice.label)).style(style)
                })
                .collect();
            frame.render_widget(
                List::new(items).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::default().fg(accent)),
                ),
                list_area,
            );
        }
    }
}

/// How wide the value column has to be for one field, in cells.
fn config_field_value_width(
    editor: &super::config_editor::ConfigEditor,
    field: &super::config_editor::Field,
) -> usize {
    use super::config_editor::FieldKind;
    match &field.kind {
        FieldKind::Toggle => "[ ] off".len(),
        // The stored value is a swatch plus the hex it names.
        FieldKind::Color => "███ #ffffff".chars().count(),
        // A choice field must fit its longest label, not just the current one:
        // the value changes as the user picks, and a box that grew mid-edit
        // would be worse than one that was always big enough.
        FieldKind::Choice(choices) => choices
            .iter()
            .map(|c| c.label.chars().count())
            .max()
            .unwrap_or(0),
        FieldKind::Text => editor
            .value(field.id)
            .as_text()
            .chars()
            .count()
            .max("(unset)".len()),
    }
}

/// The config editor's box: as small as its content allows, centred, and
/// clamped to the terminal.
fn config_editor_area(editor: &super::config_editor::ConfigEditor, area: Rect) -> Rect {
    const MARKER: usize = 3; // " > "
    const GAP: usize = 2; // between label and value
    const BORDERS: u16 = 2;
    const CHROME_ROWS: u16 = 1 /* tabs */ + 2 /* help + status */ + 1 /* footer */;

    let mut content_width = 0usize;
    let mut rows = 0usize;
    for section in &editor.sections {
        let label_width = section
            .fields
            .iter()
            .map(|f| f.label.chars().count())
            .max()
            .unwrap_or(12)
            .max(12);
        for field in &section.fields {
            let width = MARKER + label_width + GAP + config_field_value_width(editor, field);
            content_width = content_width.max(width);
            // The help line sits inside the same box, so it is part of how wide
            // the box has to be.
            content_width = content_width.max(field.help.chars().count() + 1);
        }
        rows = rows.max(section.fields.len());
    }

    // The tab strip and the footer are as long as they are, whatever the fields
    // measure.
    let tabs: usize = editor
        .sections
        .iter()
        .map(|s| s.title.chars().count())
        .sum::<usize>()
        + editor.sections.len().saturating_sub(1) * 5
        + 1;
    content_width = content_width.max(tabs);
    content_width = content_width
        .max(" [h/l] section  [j/k] field  [Enter] edit  [C-s] save  [Esc] close ".len());

    let width = (content_width as u16 + BORDERS).min(area.width.saturating_sub(4));
    let height = (rows as u16 + CHROME_ROWS + BORDERS).min(area.height.saturating_sub(2));

    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

/// Draw the `?` overlay: every binding, grouped, scrollable.
///
/// Two columns where the terminal is wide enough, which roughly halves the
/// height — in one column the whole reference is a tall thin strip that has to
/// be scrolled even in a large window. Falls back to one column on a narrow
/// terminal, where two would not fit side by side at all.
fn draw_help(state: &AppState, scroll: usize, frame: &mut Frame, area: Rect) {
    const PADDING: u16 = 2;
    const GUTTER: u16 = 3;

    let column_width = help::content_width() as u16;
    let chrome = 2 + PADDING * 2; // borders + padding
    let two_columns = area.width.saturating_sub(4) >= column_width * 2 + GUTTER + chrome;
    let columns = help::columns(if two_columns { 2 } else { 1 });

    let tallest = columns.iter().map(|c| c.len()).max().unwrap_or(0);
    let inner_width = if two_columns {
        column_width * 2 + GUTTER
    } else {
        column_width
    };
    let width = (inner_width + chrome).min(area.width.saturating_sub(4));
    // +2 borders, +1 footer.
    let height = (tallest as u16 + 3).min(area.height.saturating_sub(2));
    let popup_area = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    };
    frame.render_widget(Clear, popup_area);

    let title_color = hex_to_color(&state.config.theme.color_selected);
    let key_color = hex_to_color(&state.config.theme.color_accent);
    let text_color = hex_to_color(&state.config.theme.color_text);
    let dimmed = hex_to_color(&state.config.theme.color_dimmed);

    let block = Block::default()
        .title(" Keys ")
        .borders(Borders::ALL)
        .padding(Padding::horizontal(PADDING))
        .border_style(Style::default().fg(hex_to_color(&state.config.theme.color_popup_border)));
    let inner = block.inner(popup_area);
    frame.render_widget(block, popup_area);

    let body = Rect {
        height: inner.height.saturating_sub(1),
        ..inner
    };
    let visible = body.height as usize;
    // Never scroll past the last screenful — an overlay that can be scrolled
    // into empty space looks broken. Paced by the tallest column so the two
    // stay in step.
    let max_scroll = tallest.saturating_sub(visible);
    state.help_max_scroll.set(max_scroll);
    let scroll = scroll.min(max_scroll);

    let areas: Vec<Rect> = if two_columns {
        Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Length(column_width),
                Constraint::Length(GUTTER),
                Constraint::Min(0),
            ])
            .split(body)
            .to_vec()
    } else {
        vec![body]
    };

    for (index, column) in columns.iter().enumerate() {
        // The gutter is chunk 1, so the second column is chunk 2.
        let Some(target) = areas.get(if index == 0 { 0 } else { 2 }) else {
            continue;
        };
        let lines: Vec<Line<'static>> = column
            .iter()
            .skip(scroll)
            .take(visible)
            .map(|(indent, text)| {
                if !*indent {
                    return Line::from(Span::styled(
                        text.clone(),
                        Style::default()
                            .fg(title_color)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
                // The key column is fixed-width in `help::rows`, so splitting on
                // it keeps the two colours aligned without measuring anything.
                let split = text.char_indices().nth(18).map_or(text.len(), |(i, _)| i);
                let (keys, action) = text.split_at(split);
                Line::from(vec![
                    Span::raw("  ".to_string()),
                    Span::styled(keys.to_string(), Style::default().fg(key_color)),
                    Span::styled(action.to_string(), Style::default().fg(text_color)),
                ])
            })
            .collect();
        frame.render_widget(Paragraph::new(Text::from(lines)), *target);
    }

    let more = if max_scroll > 0 {
        format!("  ({}/{})", scroll + 1, max_scroll + 1)
    } else {
        String::new()
    };
    let hint = if max_scroll > 0 {
        format!("[C-d/u] page  [j/k] line  [Esc] close{more}")
    } else {
        "[Esc] close".to_string()
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(hint, Style::default().fg(dimmed)))),
        Rect {
            y: inner.y + inner.height.saturating_sub(1),
            height: 1,
            ..inner
        },
    );
}

/// Draw the task wizard./// Draw the task wizard.
///
/// A real `Layout` — breadcrumb, the steps already behind you, the active step,
/// a validation line — rather than one flat `Paragraph` of pre-wrapped lines.
/// The old shape meant the plugin list scrolled by hand (bottom-anchored, no
/// scrollbar) and the prompt ran edge to edge with no frame around it.
fn draw_wizard(
    state: &AppState,
    wizard: &super::wizard::WizardState,
    frame: &mut Frame,
    area: Rect,
) {
    frame.render_widget(Clear, area);

    let selected = hex_to_color(&state.config.theme.color_selected);
    let text_color = hex_to_color(&state.config.theme.color_text);
    let dimmed = hex_to_color(&state.config.theme.color_dimmed);
    let accent = hex_to_color(&state.config.theme.color_accent);
    let desc_color = hex_to_color(&state.config.theme.color_description);

    let block = Block::default()
        .title(if wizard.is_editing() {
            " Edit Task "
        } else {
            " New Task "
        })
        .borders(Borders::ALL)
        .padding(Padding::horizontal(WIZARD_PADDING))
        .border_style(Style::default().fg(selected));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    // Rows behind the cursor are read-only context; the prompt is never one of
    // them, so it is excluded rather than special-cased below.
    let steps = wizard.steps();
    let index = wizard.step_index();
    let context: Vec<(&str, String)> = steps
        .iter()
        .take(index)
        .filter_map(|step| match step {
            WizardStep::Title => Some(("Title", wizard.title.as_str().to_string())),
            WizardStep::Agent => Some((
                "Agent",
                wizard.agent_name().unwrap_or("default").to_string(),
            )),
            WizardStep::Plugin => Some(("Plugin", wizard.plugin_label().to_string())),
            WizardStep::Prompt => None,
        })
        .collect();

    // A title is one line; a prompt and a list want the room. The trailing
    // slack is what stops a short body stretching to fill the popup.
    let body_height = match wizard.step() {
        WizardStep::Title => Constraint::Length(3),
        _ => Constraint::Min(3),
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                    // breadcrumb
            Constraint::Length(1),                    // rule
            Constraint::Length(context.len() as u16), // completed steps
            Constraint::Length(if context.is_empty() { 0 } else { 1 }),
            body_height,           // the active step
            Constraint::Length(1), // validation
            Constraint::Min(0),    // slack
        ])
        .split(inner);

    // --- breadcrumb ---
    let mut crumbs: Vec<Span<'static>> = Vec::new();
    for (i, step) in steps.iter().enumerate() {
        let style = if i == index {
            Style::default().fg(selected).add_modifier(Modifier::BOLD)
        } else if i < index {
            Style::default().fg(accent)
        } else {
            Style::default().fg(dimmed)
        };
        crumbs.push(Span::styled(step.label().to_string(), style));
        if i + 1 < steps.len() {
            crumbs.push(Span::styled(
                "  ›  ".to_string(),
                Style::default().fg(dimmed),
            ));
        }
    }
    frame.render_widget(Paragraph::new(Line::from(crumbs)), rows[0]);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            "─".repeat(rows[1].width as usize),
            Style::default().fg(dimmed),
        ))),
        rows[1],
    );

    // --- completed steps ---
    let context_lines: Vec<Line<'static>> = context
        .into_iter()
        .map(|(label, value)| {
            Line::from(vec![
                Span::styled(format!("{label}: "), Style::default().fg(dimmed)),
                Span::styled(value, Style::default().fg(text_color)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(Text::from(context_lines)), rows[2]);

    // --- the active step ---
    let body = rows[4];
    match wizard.step() {
        WizardStep::Title => draw_wizard_text_field(
            "Title",
            &wizard.title,
            state,
            frame,
            body,
            "What should this task be called?",
            &HashSet::new(),
        ),
        WizardStep::Prompt => draw_wizard_text_field(
            "Prompt",
            &wizard.prompt,
            state,
            frame,
            body,
            "# files   / skills   ! tasks",
            &wizard.highlighted_references,
        ),
        WizardStep::Agent | WizardStep::Plugin => {
            if let Some(list) = wizard.current_list() {
                draw_wizard_list(wizard.step().label(), list, state, frame, body);
            }
        }
    }

    // --- validation ---
    //
    // Drawn inside the wizard rather than in the footer because the footer is
    // also the background-warning channel, and an unrelated warning must not be
    // able to replace this while the user is reading it.
    if let Some(ref message) = wizard.validation {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                message.clone(),
                Style::default().fg(Color::Yellow),
            ))),
            rows[5],
        );
    } else if wizard.step() == WizardStep::Prompt {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Leave empty to let the plugin's own prompt stand.".to_string(),
                Style::default().fg(desc_color),
            ))),
            rows[5],
        );
    }
}

/// A bordered text area with the caret anchored in it.
///
/// The caret is a real terminal cursor rather than a drawn block so the OS can
/// render IME composition (Korean, Japanese, Chinese) inline where the text
/// will land.
fn draw_wizard_text_field(
    title: &str,
    input: &TextInput,
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
    hint: &str,
    highlights: &HashSet<String>,
) {
    let text_color = hex_to_color(&state.config.theme.color_text);
    let accent = hex_to_color(&state.config.theme.color_accent);
    let dimmed = hex_to_color(&state.config.theme.color_dimmed);

    let block = Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(Style::default().fg(accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    if input.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                hint.to_string(),
                Style::default().fg(dimmed),
            ))),
            inner,
        );
    } else {
        let wrap_width = inner.width as usize;
        let mut lines: Vec<Line<'static>> = Vec::new();
        for part in input.as_str().split('\n') {
            let spans: Vec<Span<'static>> = if highlights.is_empty() {
                vec![Span::styled(
                    part.to_string(),
                    Style::default().fg(text_color),
                )]
            } else {
                build_highlighted_text(part, highlights, text_color, accent)
                    .lines
                    .into_iter()
                    .flat_map(|l| l.spans)
                    .map(|s| Span::styled(s.content.into_owned(), s.style))
                    .collect()
            };
            lines.extend(wrap_spans(spans, wrap_width));
        }
        frame.render_widget(Paragraph::new(Text::from(lines)), inner);
    }

    // No prefix any more: the field's own border is the label, so the caret
    // starts at column 0 of the inner area.
    let (col, row) = wrapped_cursor_pos(input.as_str(), input.cursor, 0, inner.width as usize);
    let x = inner.x.saturating_add(col as u16);
    let y = inner.y.saturating_add(row as u16);
    if x < inner.x + inner.width && y < inner.y + inner.height {
        frame.set_cursor_position((x, y));
    }
}

/// A pick-one list with a scrollbar and, when filtering, the pattern in its
/// border.
///
/// Ratatui's own `List` does the scrolling, so the selection stays in view and
/// the scrollbar shows how much more there is.
fn draw_wizard_list(
    title: &str,
    list: &super::wizard::ListPick,
    state: &AppState,
    frame: &mut Frame,
    area: Rect,
) {
    let selected = hex_to_color(&state.config.theme.color_selected);
    let text_color = hex_to_color(&state.config.theme.color_text);
    let dimmed = hex_to_color(&state.config.theme.color_description);
    let accent = hex_to_color(&state.config.theme.color_accent);

    let block_title = match list.filter.as_ref() {
        Some(filter) => format!(" {title}  /{}▏", filter.as_str()),
        None => format!(" {title} "),
    };
    let block = Block::default()
        .title(block_title)
        .borders(Borders::ALL)
        .padding(Padding::horizontal(1))
        .border_style(Style::default().fg(accent));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let visible = list.matching();
    if visible.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "Nothing matches that.".to_string(),
                Style::default().fg(dimmed),
            ))),
            inner,
        );
        return;
    }

    let items: Vec<ListItem> = visible
        .iter()
        .map(|i| {
            let option = &list.options[*i];
            let is_selected = *i == list.selected;
            let name_style = if is_selected {
                Style::default().fg(selected).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(text_color)
            };
            let mut spans = vec![
                Span::styled(
                    if is_selected { "› " } else { "  " }.to_string(),
                    name_style,
                ),
                Span::styled(
                    format!("{:<14}", truncate_str(&option.label, 14)),
                    name_style,
                ),
            ];
            if option.active {
                spans.push(Span::styled(
                    "✓ ".to_string(),
                    Style::default().fg(Color::Green),
                ));
            } else {
                spans.push(Span::raw("  ".to_string()));
            }
            spans.push(Span::styled(
                option.description.clone(),
                Style::default().fg(dimmed),
            ));
            ListItem::new(Line::from(spans))
        })
        .collect();

    let mut list_state = ListState::default().with_selected(Some(list.position()));
    frame.render_stateful_widget(List::new(items), inner, &mut list_state);

    if visible.len() > inner.height as usize {
        let mut scrollbar_state = ScrollbarState::new(visible.len()).position(list.position());
        // Inset to the rows the list actually occupies. Handed the whole `area`
        // the track starts on the block's top border and eats its corners.
        let track = Rect {
            y: area.y + 1,
            height: area.height.saturating_sub(2),
            ..area
        };
        frame.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight)
                .begin_symbol(None)
                .end_symbol(None)
                .style(Style::default().fg(dimmed)),
            track,
            &mut scrollbar_state,
        );
    }
}
