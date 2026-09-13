use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router, ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};

use super::board_watch::{BoardView, TaskMark};
use crate::agent::hook_status::{AgentHookStatus, HookState};
use crate::config::{GlobalConfig, ProjectConfig};
use crate::core::actions::CallerKind;
use crate::db::{Database, Task, TaskStatus, TransitionRequest};

/// Whether the MCP server is bound to a specific project or serves all projects globally.
#[derive(Debug, Clone)]
pub enum ServerMode {
    /// Serve a single project (legacy / orchestrator mode — path is fixed at startup).
    Project(PathBuf),
    /// Serve all projects indexed in the global DB.
    /// CRUD tools require a `project_id` parameter.
    Global,
}

// === Parameter types ===

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListProjectsParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetConfigParams {
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    /// Filter by status: "backlog", "planning", "running", "review", "done". Omit for all tasks.
    #[schemars(description = "Filter by status: backlog, planning, running, review, done")]
    pub status: Option<String>,
    /// Include each task's description. Off by default: a description is most
    /// of a listing's bytes and a polling caller wrote them itself.
    #[schemars(
        description = "Include each task's full description (default false). get_task always includes it."
    )]
    pub include_description: Option<bool>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct WaitForBoardChangeParams {
    /// How long to wait for something to need you before answering anyway.
    #[schemars(
        description = "Seconds to wait before answering anyway (default 300, max 900). A timeout is not an error: it means nothing needed you."
    )]
    pub timeout_secs: Option<u64>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTaskParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct MoveTaskParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
    /// Action: "research", "move_forward", "move_to_planning", "move_to_running", "move_to_review", "move_to_done", "resume", "escalate_to_user"
    #[schemars(
        description = "Action: research (start research for backlog task), move_forward, move_to_planning, move_to_running, move_to_review, move_to_done, resume, escalate_to_user"
    )]
    pub action: String,
    /// Optional reason (used with escalate_to_user action)
    #[schemars(description = "Optional reason, used with escalate_to_user action")]
    pub reason: Option<String>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTransitionStatusParams {
    /// The transition request ID returned by move_task
    #[schemars(description = "The transition request ID returned by move_task")]
    pub request_id: String,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckConflictsParams {
    /// Optional task ID. If omitted, checks all tasks in Review status.
    #[schemars(description = "Optional task ID. If omitted, checks all tasks in Review status.")]
    pub task_id: Option<String>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetNotificationsParams {
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadPaneParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
    /// Number of lines to read from the end of the pane (default 50)
    #[schemars(description = "Number of lines to read from the end of the pane (default 50)")]
    pub lines: Option<i32>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendToTaskParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
    /// Message to send to the task's agent pane (followed by Enter). Max 4096 bytes, no null bytes.
    #[schemars(
        description = "Message to send to the task's agent pane (followed by Enter). Max 4096 bytes."
    )]
    pub message: String,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTaskParams {
    /// Task title
    #[schemars(description = "Task title")]
    pub title: String,
    /// Task description (what to implement, context, approach hints)
    #[schemars(description = "Task description (what to implement, context, approach hints)")]
    pub description: Option<String>,
    /// Workflow plugin name (defaults to project's active plugin)
    #[schemars(description = "Workflow plugin name (defaults to project's active plugin)")]
    pub plugin: Option<String>,
    /// Comma-separated task IDs that this task depends on
    #[schemars(
        description = "Comma-separated task IDs that this task depends on (must complete before this task starts)"
    )]
    pub referenced_tasks: Option<String>,
    /// Base branch to create worktree from (defaults to project's main branch)
    #[schemars(
        description = "Base branch to create the worktree from (e.g. another task's branch for stacked PRs). Defaults to project's main branch."
    )]
    pub base_branch: Option<String>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BatchTask {
    /// Task title
    #[schemars(description = "Task title")]
    pub title: String,
    /// Task description
    #[schemars(description = "Task description (what to implement, context, approach hints)")]
    pub description: Option<String>,
    /// Workflow plugin name (defaults to project's active plugin)
    #[schemars(description = "Workflow plugin name (defaults to project's active plugin)")]
    pub plugin: Option<String>,
    /// Indices (0-based) into the tasks array that this task depends on
    #[schemars(
        description = "Indices (0-based) into the tasks array that this task depends on. Referenced tasks must have a lower index (no forward references)."
    )]
    pub depends_on: Option<Vec<usize>>,
    /// Base branch to create worktree from (defaults to project's main branch)
    #[schemars(
        description = "Base branch to create the worktree from (e.g. another task's branch for stacked PRs). Defaults to project's main branch."
    )]
    pub base_branch: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateTasksBatchParams {
    /// Array of tasks to create, with index-based dependency wiring
    #[schemars(
        description = "Array of tasks to create. Use depends_on with 0-based indices to wire dependencies between them."
    )]
    pub tasks: Vec<BatchTask>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateTaskParams {
    /// The task ID (UUID) to update
    #[schemars(description = "The task ID (UUID) to update. Only backlog tasks can be updated.")]
    pub task_id: String,
    /// New title (if provided)
    #[schemars(description = "New task title")]
    pub title: Option<String>,
    /// New description (if provided)
    #[schemars(description = "New task description")]
    pub description: Option<String>,
    /// New plugin (if provided)
    #[schemars(description = "New workflow plugin name")]
    pub plugin: Option<String>,
    /// New referenced tasks (if provided, replaces existing)
    #[schemars(
        description = "Comma-separated task IDs that this task depends on (replaces existing dependencies)"
    )]
    pub referenced_tasks: Option<String>,
    /// New base branch (if provided)
    #[schemars(
        description = "Base branch to create the worktree from (e.g. another task's branch for stacked PRs)"
    )]
    pub base_branch: Option<String>,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DeleteTaskParams {
    /// The task ID (UUID) to delete
    #[schemars(description = "The task ID (UUID) to delete. Only backlog tasks can be deleted.")]
    pub task_id: String,
    /// Project ID (required in global mode — call list_projects first to get IDs).
    #[schemars(
        description = "Project ID. Required in global mode. Call list_projects first to get project IDs."
    )]
    pub project_id: Option<String>,
}

// === Response types ===

#[derive(Serialize)]
struct ProjectSummary {
    id: String,
    name: String,
    path: String,
}

/// One card, as a polling caller sees it.
///
/// Every optional field is omitted when empty, and the description only on
/// request: this is the response a supervising session receives most, and it
/// re-reads each one on every later turn. Measured on a 14-task run,
/// descriptions were 80% of a listing's bytes — text the caller wrote itself
/// when it created the tasks.
#[derive(Serialize)]
struct TaskSummary {
    id: String,
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    status: String,
    agent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pr_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    plugin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    referenced_tasks: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    base_branch: Option<String>,
    /// Why the task was flagged for a person — a merge conflict, an
    /// escalation. Present only when set.
    #[serde(skip_serializing_if = "Option::is_none")]
    escalation_note: Option<String>,
    deps_satisfied: bool,
    /// The TUI's published phase status, or `None` when nothing has been
    /// observed for this task. Weigh it against `phase_age_secs` rather than
    /// treating it as live — with no TUI running, nothing refreshes it.
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_status: Option<String>,
    /// How many seconds ago that status was observed. The refresh republishes
    /// every live task on every pass, so a small age means "seen just now" and
    /// a large one means nothing is watching this board — which is a different
    /// problem from a task that is genuinely idle.
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_age_secs: Option<i64>,
}

/// The settings that decide how a run behaves, already merged.
///
/// Only the fields a caller acts on. The theme is not here — nothing driving
/// the board makes a decision from it — and neither is anything secret.
#[derive(Serialize)]
struct EffectiveConfig {
    /// Where these values came from, so a caller can say *which* file to edit
    /// rather than guess at `~/.config/agtx/config.toml`.
    global_config_path: String,
    project_config_path: String,
    project_config_exists: bool,
    /// Whether agtx answers agents' trust and bypass-permission dialogs. When
    /// false, an unattended run parks every task as `blocked` on its first
    /// dialog with nobody to answer it. Global-only: a project config cannot
    /// set this, because a repository must not be able to grant itself trust.
    auto_trust: bool,
    default_agent: String,
    /// Per-phase overrides. A phase absent here uses `default_agent`.
    phase_agents: serde_json::Value,
    worktree_enabled: bool,
    skip_worktree: bool,
    auto_cleanup: bool,
    /// Empty means auto-detect (main, then master).
    base_branch: String,
    worktree_dir: String,
    branch_prefix: String,
    workflow_plugin: Option<String>,
    agent_hooks: bool,
    github_url: Option<String>,
}

/// The board, plus the one fact that says whether any of it is live.
///
/// `tui_connected` belongs at board level rather than on each card: it is one
/// answer for the whole project, and repeating it per task would imply it could
/// differ between them. It is wrapped around the task list rather than offered
/// as a separate tool because a caller needs it on *every* poll — a separate
/// call is one a polling loop will skip, and skipping it means reading frozen
/// rows as live state.
#[derive(Serialize)]
struct BoardListing {
    /// Whether a TUI has beaten recently. When false, nothing is executing
    /// queued transitions and every `phase_status` below is frozen at whatever
    /// was last observed — stale, not current.
    tui_connected: bool,
    tasks: Vec<TaskSummary>,
}

/// The answer to `wait_for_board_change`: only what differs from what this
/// session was last shown.
#[derive(Serialize)]
struct BoardChanges {
    tui_connected: bool,
    /// True when the wait ran out with nothing needing the caller. `tasks` can
    /// still be non-empty — moves into states that ask nothing of it, such as
    /// a task starting to work, are reported but do not wake a wait.
    timed_out: bool,
    tasks: Vec<TaskSummary>,
    /// Ids of tasks this session was shown that no longer exist.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    removed: Vec<String>,
    /// Outcomes of `move_task` requests this session queued, since it was last
    /// told. What makes polling `get_transition_status` unnecessary.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    transitions: Vec<ResolvedTransition>,
}

#[derive(Debug, Serialize, Clone)]
struct ResolvedTransition {
    request_id: String,
    task_id: String,
    action: String,
    /// `completed` or `error`.
    status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<String>,
}

/// A `move_task` request this session queued and has not been told the
/// outcome of.
#[derive(Debug, Clone)]
struct QueuedMove {
    request_id: String,
    task_id: String,
    action: String,
}

/// One MCP session's view of one board, for `wait_for_board_change`.
///
/// Kept in the server because the server *is* the session: each client spawns
/// its own `agtx mcp-serve` over stdio, so this map lives exactly as long as
/// the caller it describes.
#[derive(Debug, Default)]
struct SessionBoard {
    view: BoardView,
    queued: Vec<QueuedMove>,
    /// Outcomes found by a poll but not yet returned to the caller.
    resolved: Vec<ResolvedTransition>,
}

/// One pass of a wait, read and folded into the session's view.
struct BoardPoll {
    snapshot: Vec<(TaskSummary, TaskMark)>,
    tui_connected: bool,
    wake: bool,
}

/// How long a wait lasts when the caller does not say.
const DEFAULT_WAIT_SECS: u64 = 300;
/// The longest wait a caller can ask for. A timeout is cheap — one turn — and
/// a cap keeps a stuck call from outliving a client's own tool-call limit.
const MAX_WAIT_SECS: u64 = 900;
/// How often a wait re-reads the board. The TUI refreshes phase status every
/// two seconds, so reading faster than this finds nothing new.
const WAIT_POLL_INTERVAL: Duration = Duration::from_secs(1);

#[derive(Serialize)]
struct TaskDetail {
    id: String,
    title: String,
    description: Option<String>,
    status: String,
    agent: String,
    project_id: String,
    session_name: Option<String>,
    worktree_path: Option<String>,
    branch_name: Option<String>,
    pr_number: Option<i32>,
    pr_url: Option<String>,
    plugin: Option<String>,
    cycle: i32,
    referenced_tasks: Option<String>,
    base_branch: Option<String>,
    escalation_note: Option<String>,
    created_at: String,
    updated_at: String,
    /// Whether all referenced_tasks (dependencies) are in Review or Done.
    deps_satisfied: bool,
    /// Dependencies that are not yet in Review or Done status.
    blocking_tasks: Vec<BlockingTask>,
    /// Actions the orchestrator can take on this task given its current status and plugin rules.
    allowed_actions: Vec<String>,
    /// The agent's own report of what it is doing: "working", "blocked",
    /// "waiting" or "ended". Absent when the agent reports nothing (hooks
    /// disabled, or an agent that does not support them).
    #[serde(skip_serializing_if = "Option::is_none")]
    agent_state: Option<String>,
    /// What the agent is waiting for, when `agent_state` is "blocked". This is
    /// the permission prompt or question text the agent itself reported.
    #[serde(skip_serializing_if = "Option::is_none")]
    blocked_reason: Option<String>,
    /// The TUI's own verdict on the phase: working, blocked, idle, ready or
    /// exited. Distinct from `agent_state`, which is what the agent reports
    /// about itself — this one also covers agents with no hooks, and is the
    /// only signal that can say `ready` (the phase artifact exists) or
    /// `exited` (the window is gone).
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_status: Option<String>,
    /// How many seconds ago `phase_status` was observed. See `TaskSummary`.
    #[serde(skip_serializing_if = "Option::is_none")]
    phase_age_secs: Option<i64>,
    /// Whether a TUI is running for this project. When false, `phase_status` is
    /// frozen rather than current, and no queued transition will execute.
    tui_connected: bool,
}

#[derive(Serialize)]
struct BlockingTask {
    id: String,
    title: String,
    status: String,
}

#[derive(Serialize)]
struct MoveTaskResult {
    request_id: String,
    message: String,
}

#[derive(Serialize)]
struct TransitionStatusResult {
    request_id: String,
    status: String,
    error: Option<String>,
}

#[derive(Serialize)]
struct ConflictCheckResult {
    task_id: String,
    title: String,
    branch_name: Option<String>,
    has_conflicts: bool,
    conflicting_files: Vec<String>,
    error: Option<String>,
}

#[derive(Serialize)]
struct CheckConflictsResponse {
    main_branch: String,
    results: Vec<ConflictCheckResult>,
}

#[derive(Serialize)]
struct NotificationItem {
    message: String,
    created_at: String,
}

#[derive(Serialize)]
struct GetNotificationsResponse {
    notifications: Vec<NotificationItem>,
}

#[derive(Serialize)]
struct ReadPaneResponse {
    task_id: String,
    session_name: String,
    content: String,
    lines_requested: i32,
}

#[derive(Serialize)]
struct SendToTaskResponse {
    task_id: String,
    session_name: String,
    success: bool,
    message: String,
}

#[derive(Serialize)]
struct CreateTaskResponse {
    id: String,
    title: String,
    status: String,
}

#[derive(Serialize)]
struct BatchTaskResponse {
    index: usize,
    id: String,
    title: String,
}

#[derive(Serialize)]
struct CreateTasksBatchResponse {
    created: Vec<BatchTaskResponse>,
    count: usize,
}

#[derive(Serialize)]
struct UpdateTaskResponse {
    id: String,
    title: String,
    updated_fields: Vec<String>,
}

#[derive(Serialize)]
struct DeleteTaskResponse {
    id: String,
    title: String,
    message: String,
}

/// How long a TUI heartbeat stays trusted. Three beats of the TUI's
/// `TRANSITION_POLL_INTERVAL`, so one missed tick is not read as a disconnect —
/// the same window the web API applies, since the two answer the same question.
const TUI_HEARTBEAT_STALE_AFTER: chrono::Duration = chrono::Duration::seconds(6);

// === MCP Server ===

#[derive(Debug, Clone)]
pub struct AgtxMcpServer {
    mode: ServerMode,
    /// When each project's `board_watch` row was last marked, so a read burst
    /// costs one write rather than one per tool call. Shared across clones
    /// because rmcp clones the server per request.
    watched: Arc<Mutex<HashMap<String, Instant>>>,
    /// What this session has been shown of each project's board, keyed by
    /// project path. Shared across clones for the same reason as `watched`.
    boards: Arc<Mutex<HashMap<String, SessionBoard>>>,
    tool_router: ToolRouter<Self>,
}

impl AgtxMcpServer {
    fn new(mode: ServerMode) -> Self {
        Self {
            mode,
            watched: Arc::new(Mutex::new(HashMap::new())),
            boards: Arc::new(Mutex::new(HashMap::new())),
            tool_router: Self::tool_router(),
        }
    }

    fn project_key(&self, project_id: Option<&str>) -> Result<String, String> {
        self.resolve_project_path(project_id)
            .map(|p| p.to_string_lossy().to_string())
    }

    /// Run `f` on this session's view of a project's board.
    fn with_board<R>(&self, key: &str, f: impl FnOnce(&mut SessionBoard) -> R) -> R {
        let mut boards = self.boards.lock().unwrap_or_else(|e| e.into_inner());
        f(boards.entry(key.to_string()).or_default())
    }

    /// When the agent's hook last reported a turn boundary — see
    /// [`TaskMark::turn_ts`].
    fn turn_ts(hook: Option<&AgentHookStatus>) -> Option<i64> {
        hook.filter(|h| h.state != HookState::Working).map(|h| h.ts)
    }

    /// Summaries of `tasks`, each with the mark a wait compares.
    ///
    /// One runtime query for the whole board rather than one per task: this is
    /// what a polling caller runs most, and the one that grows with the project.
    fn board_snapshot(
        db: &Database,
        tasks: Vec<Task>,
        include_description: bool,
    ) -> Vec<(TaskSummary, TaskMark)> {
        let runtime = db.list_task_runtime().unwrap_or_default();
        let now = chrono::Utc::now().timestamp();
        tasks
            .into_iter()
            .map(|t| {
                let deps_satisfied = db.deps_satisfied(&t);
                let (phase_status, phase_age_secs) =
                    Self::runtime_fields(runtime.iter().find(|r| r.task_id == t.id), t.status);
                let hook = t.worktree_path.as_ref().and_then(|wt| {
                    crate::agent::hook_status::read_status(std::path::Path::new(wt), &t.id, now)
                });
                let mark = TaskMark {
                    status: t.status,
                    phase_status: phase_status.clone(),
                    turn_ts: Self::turn_ts(hook.as_ref()),
                    deps_satisfied,
                    escalation_note: t.escalation_note.clone(),
                };
                let summary = TaskSummary {
                    phase_status,
                    phase_age_secs,
                    id: t.id,
                    title: t.title,
                    description: t.description.filter(|_| include_description),
                    status: t.status.as_str().to_string(),
                    agent: t.agent,
                    branch_name: t.branch_name,
                    pr_url: t.pr_url,
                    plugin: t.plugin,
                    referenced_tasks: t.referenced_tasks,
                    base_branch: t.base_branch,
                    escalation_note: t.escalation_note,
                    deps_satisfied,
                };
                (summary, mark)
            })
            .collect()
    }

    /// Record what a read tool just showed the caller, so the next wait does
    /// not wake to show it again.
    fn remember_seen<'a>(
        &self,
        project_id: Option<&str>,
        seen: impl IntoIterator<Item = (&'a str, &'a TaskMark)>,
        tui_connected: bool,
    ) {
        if let Ok(key) = self.project_key(project_id) {
            self.with_board(&key, |b| b.view.mark_seen(seen, tui_connected));
        }
    }

    /// One pass of a wait. Synchronous on purpose: the database handle and the
    /// lock both stay inside it, so nothing is held across the wait's sleep.
    fn poll_board(&self, project_id: Option<&str>, key: &str) -> Result<BoardPoll, String> {
        // A wait is a reader for its whole length, not just when it starts: the
        // TUI stops publishing phase status once nobody has read it for a while.
        self.note_board_watched(project_id);
        let db = self.open_project_db_for(project_id)?;
        let tasks = db
            .get_all_tasks()
            .map_err(|e| format!("Error listing tasks: {}", e))?;
        let snapshot = Self::board_snapshot(&db, tasks, false);
        let tui_connected = self.tui_connected(project_id);
        let marks: Vec<(String, TaskMark)> = snapshot
            .iter()
            .map(|(s, m)| (s.id.clone(), m.clone()))
            .collect();

        let wake = self.with_board(key, |board| {
            // A failed move wakes: nothing on the board shows it, so it is the
            // one outcome the caller cannot learn any other way. A completed
            // one does not — the status it changed is already on the board.
            let mut failed = false;
            for q in std::mem::take(&mut board.queued) {
                match db.get_transition_request(&q.request_id) {
                    Ok(Some(req)) if req.processed_at.is_some() => {
                        failed |= req.error.is_some();
                        board.resolved.push(ResolvedTransition {
                            request_id: q.request_id,
                            task_id: q.task_id,
                            action: q.action,
                            status: if req.error.is_some() {
                                "error"
                            } else {
                                "completed"
                            }
                            .to_string(),
                            error: req.error,
                        });
                    }
                    // Processed requests are deleted after an hour; one gone
                    // before a wait saw it has nothing left to report.
                    Ok(None) => {}
                    _ => board.queued.push(q),
                }
            }
            board.view.poll(&marks, tui_connected) || failed
        });
        Ok(BoardPoll {
            snapshot,
            tui_connected,
            wake,
        })
    }

    /// Answer a wait with what changed, and record that the caller has seen it.
    fn answer_wait(&self, key: &str, poll: BoardPoll, timed_out: bool) -> String {
        let marks: Vec<(String, TaskMark)> = poll
            .snapshot
            .iter()
            .map(|(s, m)| (s.id.clone(), m.clone()))
            .collect();
        let (diff, transitions) = self.with_board(key, |board| {
            (
                board.view.report(&marks, poll.tui_connected),
                std::mem::take(&mut board.resolved),
            )
        });
        let changed: std::collections::HashSet<String> = diff.changed.into_iter().collect();
        let changes = BoardChanges {
            tui_connected: poll.tui_connected,
            timed_out,
            tasks: poll
                .snapshot
                .into_iter()
                .filter(|(s, _)| changed.contains(&s.id))
                .map(|(s, _)| s)
                .collect(),
            removed: diff.removed,
            transitions,
        };
        serde_json::to_string_pretty(&changes)
            .unwrap_or_else(|e| format!("Error serializing: {}", e))
    }

    /// Tell the TUI someone is reading this board, so it starts publishing
    /// `task_runtime`.
    ///
    /// Publishing is gated on a recent reader because a board nobody reads
    /// should cost no writes. An MCP client is such a reader — without this the
    /// orchestrator and any oneshot session poll a table that is never written,
    /// and every task reports no phase status forever.
    ///
    /// Throttled on the same reasoning as the web server's copy: the question
    /// it answers resolves in minutes, and `get_task` is called once per task
    /// in a polling loop.
    fn note_board_watched(&self, project_id: Option<&str>) {
        const NOTE_INTERVAL: Duration = Duration::from_secs(30);
        let Ok(path) = self.resolve_project_path(project_id) else {
            return;
        };
        let key = path.to_string_lossy().to_string();
        {
            let mut watched = self.watched.lock().unwrap_or_else(|e| e.into_inner());
            if watched.get(&key).is_some_and(|at| at.elapsed() < NOTE_INTERVAL) {
                return;
            }
            watched.insert(key.clone(), Instant::now());
        }
        if let Ok(db) = Database::open_global() {
            let _ = db.note_board_watched(&key);
        }
    }

    /// Whether a TUI has beaten for this project recently enough to be draining
    /// the queue and refreshing phase status.
    ///
    /// Without this a caller can only *infer* a dead board from `phase_age_secs`
    /// climbing across every task at once — and until it does, a frozen row
    /// reads as live state. A task showing `blocked` while its agent works
    /// normally is what that looks like, and the answer was already in
    /// `tui_heartbeat` the whole time.
    fn tui_connected(&self, project_id: Option<&str>) -> bool {
        let Ok(path) = self.resolve_project_path(project_id) else {
            return false;
        };
        let Ok(db) = Database::open_global() else {
            return false;
        };
        db.tui_is_live(&path.to_string_lossy(), TUI_HEARTBEAT_STALE_AFTER)
            .unwrap_or(false)
    }

    /// The published phase status for a task, as `(status, age_secs)`.
    ///
    /// A row computed for a status the task has since left is withheld: it
    /// describes the previous phase, and its `updated_at` is fresh, so age alone
    /// cannot catch it. Returning nothing reads as "not yet observed", which is
    /// what it is.
    fn runtime_fields(
        rt: Option<&crate::db::TaskRuntime>,
        current: TaskStatus,
    ) -> (Option<String>, Option<i64>) {
        let rt = rt.filter(|r| r.status.map_or(true, |s| s == current));
        (
            rt.map(|r| r.phase_status.as_str().to_string()),
            rt.map(|r| (chrono::Utc::now() - r.updated_at).num_seconds()),
        )
    }

    /// Resolve a project path from an optional `project_id`.
    ///
    /// - In `Project` mode the fixed path is always returned; `project_id` is ignored.
    /// - In `Global` mode `project_id` is required and looked up in the global DB.
    fn resolve_project_path(&self, project_id: Option<&str>) -> Result<PathBuf, String> {
        match &self.mode {
            ServerMode::Project(path) => Ok(path.clone()),
            ServerMode::Global => {
                let pid = project_id.ok_or_else(|| {
                    "project_id is required in global mode. Call list_projects first to get project IDs.".to_string()
                })?;
                let global_db = Database::open_global()
                    .map_err(|e| format!("Failed to open global database: {}", e))?;
                match global_db.get_project_by_id(pid) {
                    Ok(Some(p)) => Ok(PathBuf::from(p.path)),
                    Ok(None) => Err(format!("Project not found: {}", pid)),
                    Err(e) => Err(format!("Failed to look up project: {}", e)),
                }
            }
        }
    }

    /// Open a project DB, resolving the path via `resolve_project_path`.
    fn open_project_db_for(&self, project_id: Option<&str>) -> Result<Database, String> {
        let path = self.resolve_project_path(project_id)?;
        Database::open_project(&path).map_err(|e| format!("Failed to open project database: {}", e))
    }

    fn open_project_db(&self) -> Result<Database, String> {
        self.open_project_db_for(None)
    }

    fn open_global_db(&self) -> Result<Database, String> {
        Database::open_global().map_err(|e| format!("Failed to open global database: {}", e))
    }

    /// Get the project name from the project path.
    fn project_name_for(&self, project_id: Option<&str>) -> String {
        match self.resolve_project_path(project_id) {
            Ok(path) => path
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| "unknown".to_string()),
            Err(_) => "unknown".to_string(),
        }
    }

    /// Get the default agent and plugin from merged config.
    fn config_defaults_for(&self, project_id: Option<&str>) -> (String, Option<String>) {
        let global = GlobalConfig::load().unwrap_or_default();
        match self.resolve_project_path(project_id) {
            Ok(path) => {
                let project = ProjectConfig::load(&path).unwrap_or_default();
                let agent = project
                    .default_agent
                    .unwrap_or_else(|| global.default_agent.clone());
                let plugin = project.workflow_plugin.clone();
                (agent, plugin)
            }
            Err(_) => (global.default_agent.clone(), None),
        }
    }

    /// Which `move_task` actions a task permits, as the orchestrator.
    ///
    /// The web API asks the same question as [`CallerKind::Human`] and gets a
    /// different answer for Backlog — see [`crate::core::actions`].
    fn allowed_actions(&self, task: &Task, deps_satisfied: bool) -> Vec<String> {
        crate::core::actions::allowed_actions(task, deps_satisfied, CallerKind::Orchestrator)
    }
}

#[tool_router]
impl AgtxMcpServer {
    #[tool(description = "List all projects indexed by agtx")]
    fn list_projects(&self, _params: Parameters<ListProjectsParams>) -> String {
        tracing::info!(tool = "list_projects", "MCP tool called");
        match self.open_global_db() {
            Ok(db) => match db.get_all_projects() {
                Ok(projects) => {
                    let summaries: Vec<ProjectSummary> = projects
                        .into_iter()
                        .map(|p| ProjectSummary {
                            id: p.id,
                            name: p.name,
                            path: p.path,
                        })
                        .collect();
                    serde_json::to_string_pretty(&summaries)
                        .unwrap_or_else(|e| format!("Error serializing: {}", e))
                }
                Err(e) => format!("Error listing projects: {}", e),
            },
            Err(e) => e,
        }
    }

    #[tool(
        description = "Read the effective agtx configuration for a project — the global config merged with the project's own, which is what actually governs a run. Use this instead of reading ~/.config/agtx/config.toml: that path is not authoritative (AGTX_CONFIG_DIR relocates it), and a project config overrides much of it. Returns auto_trust (whether agents' trust dialogs are answered automatically — an unattended run needs this true), default_agent, per-phase agents, worktree settings, the active workflow plugin, and the paths both files live at."
    )]
    fn get_config(&self, Parameters(params): Parameters<GetConfigParams>) -> String {
        tracing::info!(tool = "get_config", project_id = ?params.project_id, "MCP tool called");
        let project_path = match self.resolve_project_path(params.project_id.as_deref()) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let global = GlobalConfig::load().unwrap_or_default();
        let project = ProjectConfig::load(&project_path).unwrap_or_default();
        let merged = crate::config::MergedConfig::merge(&global, &project);

        let project_config_path = project_path.join(".agtx").join("config.toml");
        let cfg = EffectiveConfig {
            global_config_path: GlobalConfig::config_path()
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| "<unresolved>".to_string()),
            project_config_exists: project_config_path.exists(),
            project_config_path: project_config_path.to_string_lossy().to_string(),
            auto_trust: merged.auto_trust,
            default_agent: merged.default_agent,
            phase_agents: serde_json::json!({
                "research": merged.phase_agents.research,
                "planning": merged.phase_agents.planning,
                "running": merged.phase_agents.running,
                "review": merged.phase_agents.review,
            }),
            worktree_enabled: merged.worktree_enabled,
            skip_worktree: merged.skip_worktree,
            auto_cleanup: merged.auto_cleanup,
            base_branch: merged.base_branch,
            worktree_dir: merged.worktree_dir,
            branch_prefix: merged.branch_prefix,
            workflow_plugin: merged.workflow_plugin,
            agent_hooks: merged.agent_hooks,
            github_url: merged.github_url,
        };
        serde_json::to_string_pretty(&cfg)
            .unwrap_or_else(|e| format!("Error serializing: {}", e))
    }

    #[tool(
        description = "List tasks for a project, optionally filtered by status (backlog, planning, running, review, done). Descriptions are left out unless include_description is true — get_task has the full task. To watch the board, call wait_for_board_change instead of calling this in a loop. In global mode, project_id is required — call list_projects first."
    )]
    fn list_tasks(&self, Parameters(params): Parameters<ListTasksParams>) -> String {
        tracing::info!(tool = "list_tasks", status = ?params.status, project_id = ?params.project_id, "MCP tool called");
        self.note_board_watched(params.project_id.as_deref());
        match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => {
                let tasks_result = if let Some(status_str) = &params.status {
                    match TaskStatus::from_str(status_str) {
                        Some(status) => db.get_tasks_by_status(status),
                        None => return format!("Invalid status: '{}'. Valid values: backlog, planning, running, review, done", status_str),
                    }
                } else {
                    db.get_all_tasks()
                };
                match tasks_result {
                    Ok(tasks) => {
                        let snapshot = Self::board_snapshot(
                            &db,
                            tasks,
                            params.include_description.unwrap_or(false),
                        );
                        let tui_connected = self.tui_connected(params.project_id.as_deref());
                        self.remember_seen(
                            params.project_id.as_deref(),
                            snapshot.iter().map(|(s, m)| (s.id.as_str(), m)),
                            tui_connected,
                        );
                        let listing = BoardListing {
                            tui_connected,
                            tasks: snapshot.into_iter().map(|(s, _)| s).collect(),
                        };
                        serde_json::to_string_pretty(&listing)
                            .unwrap_or_else(|e| format!("Error serializing: {}", e))
                    }
                    Err(e) => format!("Error listing tasks: {}", e),
                }
            }
            Err(e) => e,
        }
    }

    #[tool(
        description = "Get full details of a specific task by its ID. Includes allowed_actions based on the task's current status and plugin rules, and agent_state (working/blocked/waiting/ended) as reported by the agent itself — when it is \"blocked\", blocked_reason names what the agent is waiting for. In global mode, project_id is required — call list_projects first."
    )]
    fn get_task(&self, Parameters(params): Parameters<GetTaskParams>) -> String {
        tracing::info!(tool = "get_task", task_id = %params.task_id, "MCP tool called");
        self.note_board_watched(params.project_id.as_deref());
        match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => match db.get_task(&params.task_id) {
                Ok(Some(t)) => {
                    let deps_ok = db.deps_satisfied(&t);
                    let allowed = self.allowed_actions(&t, deps_ok);
                    let blocking = match &t.referenced_tasks {
                        Some(refs) if !refs.is_empty() => refs
                            .split(',')
                            .filter(|s| !s.is_empty())
                            .filter_map(|ref_id| {
                                db.get_task(ref_id)
                                    .ok()
                                    .flatten()
                                    .filter(|dep| {
                                        !matches!(dep.status, TaskStatus::Review | TaskStatus::Done)
                                    })
                                    .map(|dep| BlockingTask {
                                        id: dep.id,
                                        title: dep.title,
                                        status: dep.status.as_str().to_string(),
                                    })
                            })
                            .collect(),
                        _ => Vec::new(),
                    };
                    // Read the agent's own status file, written by its hooks.
                    // Works cross-process precisely because it is a file rather
                    // than TUI state.
                    let hook = t.worktree_path.as_ref().and_then(|wt| {
                        crate::agent::hook_status::read_status(
                            std::path::Path::new(wt),
                            &t.id,
                            chrono::Utc::now().timestamp(),
                        )
                    });
                    let agent_state = hook.as_ref().map(|h| {
                        match h.state {
                            crate::agent::hook_status::HookState::Working => "working",
                            crate::agent::hook_status::HookState::Blocked => "blocked",
                            crate::agent::hook_status::HookState::Waiting => "waiting",
                            crate::agent::hook_status::HookState::Ended => "ended",
                        }
                        .to_string()
                    });
                    let blocked_reason = hook.as_ref().and_then(|h| h.message.clone());
                    let runtime = db.get_task_runtime(&params.task_id).ok().flatten();
                    let (phase_status, phase_age_secs) = Self::runtime_fields(runtime.as_ref(), t.status);
                    let tui_connected = self.tui_connected(params.project_id.as_deref());
                    let mark = TaskMark {
                        status: t.status,
                        phase_status: phase_status.clone(),
                        turn_ts: Self::turn_ts(hook.as_ref()),
                        deps_satisfied: deps_ok,
                        escalation_note: t.escalation_note.clone(),
                    };
                    self.remember_seen(
                        params.project_id.as_deref(),
                        [(t.id.as_str(), &mark)],
                        tui_connected,
                    );

                    let detail = TaskDetail {
                        phase_status,
                        phase_age_secs,
                        tui_connected,
                        id: t.id,
                        title: t.title,
                        description: t.description,
                        status: t.status.as_str().to_string(),
                        agent: t.agent,
                        project_id: t.project_id,
                        session_name: t.session_name,
                        worktree_path: t.worktree_path,
                        branch_name: t.branch_name,
                        pr_number: t.pr_number,
                        pr_url: t.pr_url,
                        plugin: t.plugin,
                        cycle: t.cycle,
                        referenced_tasks: t.referenced_tasks,
                        base_branch: t.base_branch,
                        escalation_note: t.escalation_note,
                        created_at: t.created_at.to_rfc3339(),
                        updated_at: t.updated_at.to_rfc3339(),
                        deps_satisfied: deps_ok,
                        blocking_tasks: blocking,
                        allowed_actions: allowed,
                        agent_state,
                        blocked_reason,
                    };
                    serde_json::to_string_pretty(&detail)
                        .unwrap_or_else(|e| format!("Error serializing: {}", e))
                }
                Ok(None) => format!("Task not found: {}", params.task_id),
                Err(e) => format!("Error getting task: {}", e),
            },
            Err(e) => e,
        }
    }

    #[tool(
        description = "Block until something on the board needs you, then return only what changed since you last looked (via this tool, list_tasks or get_task). Wakes when a task reaches ready, idle, blocked or exited, reaches Done, becomes startable in Backlog, is escalated, appears or is deleted; when one of your move_task requests fails; or when the TUI connects or disconnects. A task merely starting to work does not wake it. The first call returns the whole board. Also reports the outcome of every move_task you queued, so get_transition_status is not needed. Use this as the polling loop instead of sleeping and calling list_tasks. timed_out: true means nothing needed you. In global mode, project_id is required — call list_projects first."
    )]
    async fn wait_for_board_change(
        &self,
        Parameters(params): Parameters<WaitForBoardChangeParams>,
    ) -> String {
        tracing::info!(tool = "wait_for_board_change", timeout_secs = ?params.timeout_secs, "MCP tool called");
        let timeout = Duration::from_secs(
            params
                .timeout_secs
                .unwrap_or(DEFAULT_WAIT_SECS)
                .clamp(1, MAX_WAIT_SECS),
        );
        let project_id = params.project_id.as_deref();
        let key = match self.project_key(project_id) {
            Ok(k) => k,
            Err(e) => return e,
        };
        let started = Instant::now();
        loop {
            let poll = match self.poll_board(project_id, &key) {
                Ok(p) => p,
                Err(e) => return e,
            };
            if poll.wake {
                return self.answer_wait(&key, poll, false);
            }
            if started.elapsed() >= timeout {
                return self.answer_wait(&key, poll, true);
            }
            tokio::time::sleep(WAIT_POLL_INTERVAL).await;
        }
    }

    #[tool(
        description = "Queue a task state transition. The agtx TUI will process it and execute all side effects (worktree creation, agent spawning, etc). Use get_transition_status to check completion — a transition out of Backlog waits for the single worktree-setup slot, so it stays 'pending' until its turn. Actions: research (start research phase for backlog task), move_forward, move_to_planning, move_to_running, move_to_review, move_to_done, move_to_done_and_merge (merge the task branch into its base branch in the project checkout first, and stay in Review if it conflicts — for unattended callers that integrate locally instead of through a PR), resume, escalate_to_user (flag task for user attention with an optional reason)"
    )]
    fn move_task(&self, Parameters(params): Parameters<MoveTaskParams>) -> String {
        tracing::info!(tool = "move_task", task_id = %params.task_id, action = %params.action, "MCP tool called");
        let valid_actions = crate::core::actions::ACTIONS;
        if !valid_actions.contains(&params.action.as_str()) {
            return format!(
                "Invalid action: '{}'. Valid actions: {}",
                params.action,
                valid_actions.join(", ")
            );
        }

        match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => {
                // Verify task exists
                let task = match db.get_task(&params.task_id) {
                    Ok(Some(t)) => t,
                    Ok(None) => return format!("Task not found: {}", params.task_id),
                    Err(e) => return format!("Error checking task: {}", e),
                };

                // Eagerly check dependency gates for forward transitions from Backlog
                let forward_actions = [
                    "move_forward",
                    "move_to_planning",
                    "move_to_running",
                    "research",
                ];
                if forward_actions.contains(&params.action.as_str())
                    && task.status == TaskStatus::Backlog
                    && !db.deps_satisfied(&task)
                {
                    return "Cannot advance task: dependencies not in Review/Done. Use get_task to see blocking_tasks.".to_string();
                }

                let mut req = TransitionRequest::new(&params.task_id, &params.action);
                req.reason = params.reason.clone();
                let request_id = req.id.clone();

                match db.create_transition_request(&req) {
                    Ok(()) => {
                        if let Ok(key) = self.project_key(params.project_id.as_deref()) {
                            self.with_board(&key, |b| {
                                b.queued.push(QueuedMove {
                                    request_id: request_id.clone(),
                                    task_id: params.task_id.clone(),
                                    action: params.action.clone(),
                                })
                            });
                        }
                        let result = MoveTaskResult {
                            request_id,
                            message: format!(
                                "Transition '{}' queued for task {}. The agtx TUI will process it shortly.",
                                params.action, params.task_id
                            ),
                        };
                        serde_json::to_string_pretty(&result)
                            .unwrap_or_else(|e| format!("Error serializing: {}", e))
                    }
                    Err(e) => format!("Error creating transition request: {}", e),
                }
            }
            Err(e) => e,
        }
    }

    #[tool(
        description = "Check the status of a queued transition request. Returns pending, completed, or error with details."
    )]
    fn get_transition_status(
        &self,
        Parameters(params): Parameters<GetTransitionStatusParams>,
    ) -> String {
        tracing::info!(tool = "get_transition_status", request_id = %params.request_id, "MCP tool called");
        match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => match db.get_transition_request(&params.request_id) {
                Ok(Some(req)) => {
                    let status = if req.processed_at.is_some() {
                        if req.error.is_some() {
                            "error"
                        } else {
                            "completed"
                        }
                    } else {
                        "pending"
                    };
                    let result = TransitionStatusResult {
                        request_id: req.id,
                        status: status.to_string(),
                        error: req.error,
                    };
                    serde_json::to_string_pretty(&result)
                        .unwrap_or_else(|e| format!("Error serializing: {}", e))
                }
                Ok(None) => format!("Transition request not found: {}", params.request_id),
                Err(e) => format!("Error getting transition status: {}", e),
            },
            Err(e) => e,
        }
    }

    #[tool(
        description = "Check if task branches have merge conflicts with the main branch. Pass a task_id to check one task, or omit it to check all Review tasks. Uses a read-only git check — no files are modified."
    )]
    fn check_conflicts(&self, Parameters(params): Parameters<CheckConflictsParams>) -> String {
        tracing::info!(tool = "check_conflicts", task_id = ?params.task_id, "MCP tool called");
        let project_path = match self.resolve_project_path(params.project_id.as_deref()) {
            Ok(p) => p,
            Err(e) => return e,
        };
        let main_branch = match crate::git::detect_main_branch(&project_path) {
            Ok(b) => b,
            Err(e) => return format!("Failed to detect main branch: {}", e),
        };

        let tasks = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => {
                if let Some(task_id) = &params.task_id {
                    match db.get_task(task_id) {
                        Ok(Some(t)) => vec![t],
                        Ok(None) => return format!("Task not found: {}", task_id),
                        Err(e) => return format!("Error getting task: {}", e),
                    }
                } else {
                    match db.get_tasks_by_status(TaskStatus::Review) {
                        Ok(tasks) => tasks,
                        Err(e) => return format!("Error listing review tasks: {}", e),
                    }
                }
            }
            Err(e) => return e,
        };

        let results: Vec<ConflictCheckResult> = tasks
            .into_iter()
            .map(|t| {
                let branch = match &t.branch_name {
                    Some(b) => b.clone(),
                    None => {
                        return ConflictCheckResult {
                            task_id: t.id,
                            title: t.title,
                            branch_name: None,
                            has_conflicts: false,
                            conflicting_files: vec![],
                            error: Some("No branch name set for this task".to_string()),
                        };
                    }
                };

                match crate::git::check_merge_conflicts(&project_path, &main_branch, &branch) {
                    Ok((has_conflicts, files)) => ConflictCheckResult {
                        task_id: t.id,
                        title: t.title,
                        branch_name: Some(branch),
                        has_conflicts,
                        conflicting_files: files,
                        error: None,
                    },
                    Err(e) => ConflictCheckResult {
                        task_id: t.id,
                        title: t.title,
                        branch_name: Some(branch),
                        has_conflicts: false,
                        conflicting_files: vec![],
                        error: Some(format!("{}", e)),
                    },
                }
            })
            .collect();

        let response = CheckConflictsResponse {
            main_branch,
            results,
        };
        serde_json::to_string_pretty(&response)
            .unwrap_or_else(|e| format!("Error serializing: {}", e))
    }

    #[tool(
        description = "Fetch and consume pending notifications. Returns new events (task created, phase completed, etc.) and removes them from the queue. Note: notifications are also pushed to your input automatically when you are idle, so you usually don't need to call this manually."
    )]
    fn get_notifications(&self, Parameters(params): Parameters<GetNotificationsParams>) -> String {
        tracing::info!(tool = "get_notifications", "MCP tool called");
        match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => match db.consume_notifications() {
                Ok(notifs) => {
                    let items: Vec<NotificationItem> = notifs
                        .into_iter()
                        .map(|n| NotificationItem {
                            message: n.message,
                            created_at: n.created_at.to_rfc3339(),
                        })
                        .collect();
                    let response = GetNotificationsResponse {
                        notifications: items,
                    };
                    serde_json::to_string_pretty(&response)
                        .unwrap_or_else(|e| format!("Error serializing: {}", e))
                }
                Err(e) => format!("Error fetching notifications: {}", e),
            },
            Err(e) => e,
        }
    }

    #[tool(
        description = "Read the last N lines of what a task's agent pane shows (default 50), blank rows at the bottom dropped. Use this to understand what the agent is showing — e.g., when a task has been idle for a while. An agent that draws full-screen, like Claude Code, keeps no history outside its visible screen, so nothing older than that screen can be read this way. Ask for only as many lines as you need: the answer stays in your context."
    )]
    fn read_pane_content(&self, Parameters(params): Parameters<ReadPaneParams>) -> String {
        tracing::info!(tool = "read_pane_content", task_id = %params.task_id, "MCP tool called");
        let db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let task = match db.get_task(&params.task_id) {
            Ok(Some(t)) => t,
            Ok(None) => return format!("Task not found: {}", params.task_id),
            Err(e) => return format!("Error getting task: {}", e),
        };

        let session_name = match task.session_name {
            Some(ref s) => s.clone(),
            None => return format!("Task {} has no active session", params.task_id),
        };

        let lines = params.lines.unwrap_or(50).max(1).min(10000);
        let lines_arg = format!("-{}", lines);

        let output = Command::new("tmux")
            .args([
                "-L",
                "agtx",
                "capture-pane",
                "-t",
                &session_name,
                "-p",
                "-S",
                &lines_arg,
            ])
            .output();

        match output {
            Ok(out) => {
                let content = pane_tail(&String::from_utf8_lossy(&out.stdout), lines as usize);
                let response = ReadPaneResponse {
                    task_id: params.task_id,
                    session_name,
                    content,
                    lines_requested: lines,
                };
                serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing: {}", e))
            }
            Err(e) => format!("Error reading pane content: {}", e),
        }
    }

    #[tool(
        description = "Send a message to a task's agent pane (followed by Enter). Works for tasks in Planning, Running or Review. Use this to nudge a stuck agent, answer a CLI prompt (e.g. 'y' for yes), or provide guidance. In Review it is how to give the reviewer a small fix to make in place, without moving the task back to Running — resume is for significant rework."
    )]
    fn send_to_task(&self, Parameters(params): Parameters<SendToTaskParams>) -> String {
        tracing::info!(tool = "send_to_task", task_id = %params.task_id, "MCP tool called");

        // Input validation: limit message length and reject null bytes
        const MAX_MESSAGE_LENGTH: usize = 4096;
        if params.message.len() > MAX_MESSAGE_LENGTH {
            return format!(
                "Error: message too long ({} bytes, max {})",
                params.message.len(),
                MAX_MESSAGE_LENGTH
            );
        }
        if params.message.contains('\x00') {
            return "Error: message contains null bytes".to_string();
        }

        let db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let task = match db.get_task(&params.task_id) {
            Ok(Some(t)) => t,
            Ok(None) => return format!("Task not found: {}", params.task_id),
            Err(e) => return format!("Error getting task: {}", e),
        };

        // Review is included so a reviewer can be given a small fix in place;
        // see `accepts_task_input`.
        if !crate::core::actions::accepts_task_input(task.status) {
            return format!(
                "Error: task has no agent to receive input (current: {}). send_to_task works for Planning, Running and Review tasks.",
                task.status.as_str()
            );
        }

        let session_name = match task.session_name {
            Some(ref s) => s.clone(),
            None => return format!("Task {} has no active session", params.task_id),
        };

        // A bracketed paste, then a watched submit: the path agtx uses for every
        // other whole message, via `core::input::send_user_text`.
        //
        // A raw `send-keys` of the text is what this replaces, and it lost the
        // head of every long message. Measured against Claude Code 2.1.268: a
        // 1644-byte message typed that way arrived as its last 622 bytes, the first
        // 1022 silently dropped, while the same bytes as a bracketed paste arrived
        // whole. tmux and the pty are not the cause — a raw-mode `cat` received
        // every byte by both methods — the agent's input handling discards the
        // start of a large typed burst. The agent then acts on the tail of an
        // instruction with its premise gone, which is worse than receiving nothing.
        let tmux_ops: Arc<dyn crate::tmux::TmuxOperations> = Arc::new(crate::tmux::RealTmuxOps);
        if !crate::core::input::send_user_text(&tmux_ops, &session_name, &params.message, true) {
            return format!("Error sending message to {}", session_name);
        }
        let response = SendToTaskResponse {
            task_id: params.task_id,
            session_name,
            success: true,
            message: format!("Message sent: {}", params.message),
        };
        serde_json::to_string_pretty(&response)
            .unwrap_or_else(|e| format!("Error serializing: {}", e))
    }

    #[tool(
        description = "Create a new task in the Backlog column. Returns the created task's ID. Use create_tasks_batch for multiple tasks with dependencies. In global mode, project_id is required — call list_projects first."
    )]
    fn create_task(&self, Parameters(params): Parameters<CreateTaskParams>) -> String {
        tracing::info!(tool = "create_task", title = %params.title, "MCP tool called");
        let db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let (default_agent, default_plugin) =
            self.config_defaults_for(params.project_id.as_deref());
        let project_name = self.project_name_for(params.project_id.as_deref());

        // Validate referenced task IDs exist
        if let Some(ref refs) = params.referenced_tasks {
            for ref_id in refs.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                match db.get_task(ref_id) {
                    Ok(Some(_)) => {}
                    Ok(None) => return format!("Error: referenced task not found: {}", ref_id),
                    Err(e) => return format!("Error checking referenced task: {}", e),
                }
            }
        }

        let mut task = Task::new(&params.title, &default_agent, &project_name);
        task.description = params.description;
        task.plugin = params.plugin.or(default_plugin);
        task.referenced_tasks = params.referenced_tasks;
        task.base_branch = params.base_branch;

        match db.create_task(&task) {
            Ok(()) => {
                let response = CreateTaskResponse {
                    id: task.id,
                    title: task.title,
                    status: "backlog".to_string(),
                };
                serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing: {}", e))
            }
            Err(e) => format!("Error creating task: {}", e),
        }
    }

    #[tool(
        description = "Create multiple tasks at once with index-based dependency wiring. Each task's depends_on field uses 0-based indices into the tasks array (no forward references). Returns all created task IDs. In global mode, project_id is required — call list_projects first."
    )]
    fn create_tasks_batch(&self, Parameters(params): Parameters<CreateTasksBatchParams>) -> String {
        tracing::info!(
            tool = "create_tasks_batch",
            count = params.tasks.len(),
            "MCP tool called"
        );
        if params.tasks.is_empty() {
            return "Error: tasks array is empty".to_string();
        }
        if params.tasks.len() > 50 {
            return "Error: maximum 50 tasks per batch".to_string();
        }

        // Pass 1: Validate index-based dependencies
        for (i, batch_task) in params.tasks.iter().enumerate() {
            if let Some(ref deps) = batch_task.depends_on {
                let mut seen = std::collections::HashSet::new();
                for &dep_idx in deps {
                    if dep_idx >= i {
                        return format!(
                            "Error: task[{}] '{}' has depends_on index {} which is >= its own index {}. Only backward references allowed.",
                            i, batch_task.title, dep_idx, i
                        );
                    }
                    if !seen.insert(dep_idx) {
                        return format!(
                            "Error: task[{}] '{}' has duplicate depends_on index {}.",
                            i, batch_task.title, dep_idx
                        );
                    }
                }
            }
        }

        let mut db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let (default_agent, default_plugin) =
            self.config_defaults_for(params.project_id.as_deref());
        let project_name = self.project_name_for(params.project_id.as_deref());

        // Pass 2: Create all tasks, collect IDs
        let mut created_tasks: Vec<Task> = Vec::with_capacity(params.tasks.len());
        for batch_task in &params.tasks {
            let mut task = Task::new(&batch_task.title, &default_agent, &project_name);
            task.description = batch_task.description.clone();
            task.plugin = batch_task.plugin.clone().or_else(|| default_plugin.clone());
            task.base_branch = batch_task.base_branch.clone();
            created_tasks.push(task);
        }

        // Pass 3: Resolve index-based deps to real task IDs
        for (i, batch_task) in params.tasks.iter().enumerate() {
            if let Some(ref deps) = batch_task.depends_on {
                let dep_ids: Vec<String> = deps
                    .iter()
                    .map(|&idx| created_tasks[idx].id.clone())
                    .collect();
                created_tasks[i].referenced_tasks = Some(dep_ids.join(","));
            }
        }

        // Insert all tasks atomically — on any failure none are committed
        if let Err(e) = db.create_tasks_batch(&created_tasks) {
            return format!("Error creating tasks: {}", e);
        }

        let results: Vec<BatchTaskResponse> = created_tasks
            .iter()
            .enumerate()
            .map(|(i, task)| BatchTaskResponse {
                index: i,
                id: task.id.clone(),
                title: task.title.clone(),
            })
            .collect();

        let response = CreateTasksBatchResponse {
            count: results.len(),
            created: results,
        };
        serde_json::to_string_pretty(&response)
            .unwrap_or_else(|e| format!("Error serializing: {}", e))
    }

    #[tool(
        description = "Update a backlog task's fields. Only tasks in Backlog status can be updated. All fields are optional — only provided fields are changed. In global mode, project_id is required — call list_projects first."
    )]
    fn update_task(&self, Parameters(params): Parameters<UpdateTaskParams>) -> String {
        tracing::info!(tool = "update_task", task_id = %params.task_id, "MCP tool called");
        let db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let mut task = match db.get_task(&params.task_id) {
            Ok(Some(t)) => t,
            Ok(None) => return format!("Task not found: {}", params.task_id),
            Err(e) => return format!("Error getting task: {}", e),
        };

        if task.status != TaskStatus::Backlog {
            return format!(
                "Error: can only update Backlog tasks. Task '{}' is in {} status.",
                task.title,
                task.status.as_str()
            );
        }

        let mut updated_fields = Vec::new();

        if let Some(title) = params.title {
            task.title = title;
            updated_fields.push("title".to_string());
        }
        if let Some(description) = params.description {
            task.description = Some(description);
            updated_fields.push("description".to_string());
        }
        if let Some(plugin) = params.plugin {
            task.plugin = Some(plugin);
            updated_fields.push("plugin".to_string());
        }
        if let Some(ref refs) = params.referenced_tasks {
            // Validate referenced task IDs exist
            for ref_id in refs.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                match db.get_task(ref_id) {
                    Ok(Some(_)) => {}
                    Ok(None) => return format!("Error: referenced task not found: {}", ref_id),
                    Err(e) => return format!("Error checking referenced task: {}", e),
                }
            }
            task.referenced_tasks = Some(refs.clone());
            updated_fields.push("referenced_tasks".to_string());
        }
        if let Some(base_branch) = params.base_branch {
            task.base_branch = Some(base_branch);
            updated_fields.push("base_branch".to_string());
        }

        if updated_fields.is_empty() {
            return "No fields to update".to_string();
        }

        match db.update_task(&task) {
            Ok(()) => {
                let response = UpdateTaskResponse {
                    id: task.id,
                    title: task.title,
                    updated_fields,
                };
                serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing: {}", e))
            }
            Err(e) => format!("Error updating task: {}", e),
        }
    }

    #[tool(
        description = "Delete a task. Only tasks in Backlog status can be deleted. In global mode, project_id is required — call list_projects first."
    )]
    fn delete_task(&self, Parameters(params): Parameters<DeleteTaskParams>) -> String {
        tracing::info!(tool = "delete_task", task_id = %params.task_id, "MCP tool called");
        let db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let task = match db.get_task(&params.task_id) {
            Ok(Some(t)) => t,
            Ok(None) => return format!("Task not found: {}", params.task_id),
            Err(e) => return format!("Error getting task: {}", e),
        };

        if task.status != TaskStatus::Backlog {
            return format!(
                "Error: can only delete Backlog tasks. Task '{}' is in {} status.",
                task.title,
                task.status.as_str()
            );
        }

        match db.delete_task(&params.task_id) {
            Ok(()) => {
                let response = DeleteTaskResponse {
                    id: task.id,
                    title: task.title,
                    message: "Task deleted".to_string(),
                };
                serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing: {}", e))
            }
            Err(e) => format!("Error deleting task: {}", e),
        }
    }
}

#[tool_handler]
impl ServerHandler for AgtxMcpServer {
    fn get_info(&self) -> ServerInfo {
        let instructions = match &self.mode {
            ServerMode::Global =>
                "agtx MCP server (global mode) — control the terminal kanban board for coding agents. \
                 IMPORTANT: always call list_projects first to get the project_id for your target project, \
                 then pass it to every other tool call. \
                 Use list_tasks to see tasks, create_task or create_tasks_batch to add new tasks \
                 (with optional dependency wiring via referenced_tasks), update_task to modify backlog \
                 task fields, move_task to transition tasks between phases, get_transition_status to \
                 check if a transition completed, and delete_task to remove backlog tasks. \
                 To supervise the board, call wait_for_board_change in a loop rather than sleeping \
                 and re-listing: it returns only what changed.",
            ServerMode::Project(_) =>
                "agtx MCP server — control the terminal kanban board for coding agents. \
                 Use list_tasks to see current tasks, create_task or create_tasks_batch to add new tasks \
                 (with optional dependency wiring via referenced_tasks), update_task to modify backlog \
                 task fields, move_task to transition tasks between phases, get_transition_status to \
                 check if a transition completed, and delete_task to remove backlog tasks. \
                 To supervise the board, call wait_for_board_change in a loop rather than sleeping \
                 and re-listing: it returns only what changed.",
        };
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(instructions)
    }
}

pub async fn serve(project_path: Option<PathBuf>) -> anyhow::Result<()> {
    let mode = match project_path {
        Some(path) => {
            // Validate project DB can be opened
            Database::open_project(&path)?;
            ServerMode::Project(path)
        }
        None => {
            // Global mode — validate global DB can be opened
            Database::open_global()?;
            ServerMode::Global
        }
    };

    let server = AgtxMcpServer::new(mode);
    let service = server.serve(filtered_stdio()).await?;
    service.waiting().await?;
    Ok(())
}

/// The last `n` lines of a pane capture, after dropping the blank rows below
/// the last line of output.
///
/// `capture-pane -S -N` is not a tail: it starts N lines back in the
/// scrollback and runs to the bottom of the *visible* screen. An agent that
/// draws full-screen keeps no tmux scrollback, so the capture is the whole
/// screen whatever N is — measured, a request for 15 lines returned 49, and a
/// caller that asked for less to save context got the full screen anyway.
/// Trailing blank rows go first, so N counts lines of output rather than the
/// empty rows under a short one.
fn pane_tail(content: &str, n: usize) -> String {
    let lines: Vec<&str> = content.lines().collect();
    let end = lines
        .iter()
        .rposition(|l| !l.trim().is_empty())
        .map_or(0, |i| i + 1);
    lines[end.saturating_sub(n)..end].join("\n")
}

/// Buffer size for the in-memory pipes bridging real stdio to rmcp.
const PIPE_BUF: usize = 64 * 1024;

/// stdio for rmcp, with pre-handshake requests answered instead of fatal.
///
/// rmcp aborts if the first message is not `initialize`, which Antigravity CLI
/// triggers by probing with `server/discover` first. See `prehandshake` for the
/// full story. Returns a (reader, writer) pair rmcp can serve on; two background
/// tasks pump real stdin/stdout through it.
fn filtered_stdio() -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
    use crate::mcp::prehandshake::{HandshakeFilter, PreInitAction};
    use std::sync::Arc;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::sync::Mutex;

    // client → us → rmcp
    let (mut to_rmcp, rmcp_reader) = tokio::io::duplex(PIPE_BUF);
    // rmcp → us → client
    let (rmcp_writer, mut from_rmcp) = tokio::io::duplex(PIPE_BUF);

    // Both the filter (error replies) and rmcp (real responses) write to stdout.
    let stdout = Arc::new(Mutex::new(tokio::io::stdout()));

    // Pump 1: real stdin → filter → rmcp.
    let filter_stdout = stdout.clone();
    tokio::spawn(async move {
        let mut lines = BufReader::new(tokio::io::stdin()).lines();
        let mut filter = HandshakeFilter::new();
        while let Ok(Some(line)) = lines.next_line().await {
            match filter.next(&line) {
                PreInitAction::Forward => {
                    if to_rmcp.write_all(line.as_bytes()).await.is_err()
                        || to_rmcp.write_all(b"\n").await.is_err()
                        || to_rmcp.flush().await.is_err()
                    {
                        break;
                    }
                }
                PreInitAction::Reject(response) => {
                    let mut out = filter_stdout.lock().await;
                    if out.write_all(response.as_bytes()).await.is_err()
                        || out.write_all(b"\n").await.is_err()
                        || out.flush().await.is_err()
                    {
                        break;
                    }
                    tracing::debug!(
                        "answered pre-handshake request with method-not-found: {}",
                        line
                    );
                }
                PreInitAction::Drop => {
                    tracing::debug!("dropped pre-handshake notification: {}", line);
                }
            }
        }
        // EOF on stdin — closing our end shuts rmcp down cleanly.
        drop(to_rmcp);
    });

    // Pump 2: rmcp → real stdout.
    tokio::spawn(async move {
        let mut buf = vec![0u8; 8192];
        loop {
            let n = match tokio::io::AsyncReadExt::read(&mut from_rmcp, &mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => n,
            };
            let mut out = stdout.lock().await;
            if out.write_all(&buf[..n]).await.is_err() || out.flush().await.is_err() {
                break;
            }
        }
    });

    (rmcp_reader, rmcp_writer)
}

#[cfg(test)]
mod tests {
    use super::pane_tail;

    #[test]
    fn a_pane_read_returns_only_the_lines_asked_for() {
        let screen: String = (1..=49).map(|i| format!("line {i}\n")).collect();
        let tail = pane_tail(&screen, 15);
        assert_eq!(tail.lines().count(), 15);
        assert!(tail.starts_with("line 35"));
        assert!(tail.ends_with("line 49"));
    }

    #[test]
    fn blank_rows_below_the_output_do_not_count() {
        let screen = "prompt\n❯ working\n\n   \n\n";
        assert_eq!(pane_tail(screen, 1), "❯ working");
    }

    #[test]
    fn asking_for_more_than_there_is_returns_all_of_it() {
        assert_eq!(pane_tail("a\nb\n", 50), "a\nb");
        assert_eq!(pane_tail("\n\n", 5), "");
    }
}
