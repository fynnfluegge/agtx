use std::path::PathBuf;
use std::process::Command;

use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
    transport::io::stdio,
    ServerHandler, ServiceExt,
};
use serde::{Deserialize, Serialize};

use crate::config::{GlobalConfig, ProjectConfig};
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
pub struct ListTasksParams {
    /// Filter by status: "backlog", "planning", "running", "review", "done". Omit for all tasks.
    #[schemars(description = "Filter by status: backlog, planning, running, review, done")]
    pub status: Option<String>,
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
    /// Message to send to the task's agent pane (followed by Enter)
    #[schemars(description = "Message to send to the task's agent pane (followed by Enter)")]
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
    #[schemars(description = "Array of tasks to create. Use depends_on with 0-based indices to wire dependencies between them.")]
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

#[derive(Serialize)]
struct TaskSummary {
    id: String,
    title: String,
    description: Option<String>,
    status: String,
    agent: String,
    branch_name: Option<String>,
    pr_url: Option<String>,
    plugin: Option<String>,
    referenced_tasks: Option<String>,
    base_branch: Option<String>,
    deps_satisfied: bool,
}

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

// === MCP Server ===

#[derive(Debug, Clone)]
pub struct AgtxMcpServer {
    mode: ServerMode,
    tool_router: ToolRouter<Self>,
}

impl AgtxMcpServer {
    fn new(mode: ServerMode) -> Self {
        Self {
            mode,
            tool_router: Self::tool_router(),
        }
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
        Database::open_project(&path)
            .map_err(|e| format!("Failed to open project database: {}", e))
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

    /// Compute which move_task actions are valid for a task given its status and plugin rules.
    fn allowed_actions(&self, task: &Task, deps_satisfied: bool) -> Vec<String> {
        let project_path = self.resolve_project_path(None).ok();
        let mut actions = Vec::new();

        let _plugin = match &task.plugin {
            Some(name) => crate::config::WorkflowPlugin::load(
                name,
                project_path.as_deref(),
            )
            .ok()
            .or_else(|| crate::skills::load_bundled_plugin(name)),
            None => crate::skills::load_bundled_plugin("agtx"),
        };

        match task.status {
            TaskStatus::Backlog => {
                // Orchestrator does not manage Backlog — user triages manually
            }
            TaskStatus::Planning => {
                actions.push("move_forward".to_string());
                actions.push("escalate_to_user".to_string());
            }
            TaskStatus::Running => {
                actions.push("move_forward".to_string());
                actions.push("escalate_to_user".to_string());
            }
            TaskStatus::Review => {
                actions.push("move_to_done".to_string());
                actions.push("resume".to_string());
            }
            TaskStatus::Done => {}
        }

        // Block forward transitions out of Backlog when dependencies are not satisfied
        if !deps_satisfied && task.status == TaskStatus::Backlog {
            actions.retain(|a| {
                !matches!(
                    a.as_str(),
                    "move_forward" | "move_to_planning" | "move_to_running"
                )
            });
        }

        actions
    }
}

#[tool_router]
impl AgtxMcpServer {
    #[tool(description = "List all projects indexed by agtx")]
    fn list_projects(&self, _params: Parameters<ListProjectsParams>) -> String {
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
        description = "List tasks for a project, optionally filtered by status (backlog, planning, running, review, done). In global mode, project_id is required — call list_projects first."
    )]
    fn list_tasks(&self, Parameters(params): Parameters<ListTasksParams>) -> String {
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
                        let summaries: Vec<TaskSummary> = tasks
                            .into_iter()
                            .map(|t| {
                                let deps_satisfied = db.deps_satisfied(&t);
                                TaskSummary {
                                    id: t.id,
                                    title: t.title,
                                    description: t.description,
                                    status: t.status.as_str().to_string(),
                                    agent: t.agent,
                                    branch_name: t.branch_name,
                                    pr_url: t.pr_url,
                                    plugin: t.plugin,
                                    referenced_tasks: t.referenced_tasks,
                                    base_branch: t.base_branch,
                                    deps_satisfied,
                                }
                            })
                            .collect();
                        serde_json::to_string_pretty(&summaries)
                            .unwrap_or_else(|e| format!("Error serializing: {}", e))
                    }
                    Err(e) => format!("Error listing tasks: {}", e),
                }
            }
            Err(e) => e,
        }
    }

    #[tool(
        description = "Get full details of a specific task by its ID. Includes allowed_actions based on the task's current status and plugin rules. In global mode, project_id is required — call list_projects first."
    )]
    fn get_task(&self, Parameters(params): Parameters<GetTaskParams>) -> String {
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
                                        !matches!(
                                            dep.status,
                                            TaskStatus::Review | TaskStatus::Done
                                        )
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
                    let detail = TaskDetail {
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
        description = "Queue a task state transition. The agtx TUI will process it and execute all side effects (worktree creation, agent spawning, etc). Use get_transition_status to check completion. Actions: research (start research phase for backlog task), move_forward, move_to_planning, move_to_running, move_to_review, move_to_done, resume, escalate_to_user (flag task for user attention with an optional reason)"
    )]
    fn move_task(&self, Parameters(params): Parameters<MoveTaskParams>) -> String {
        let valid_actions = [
            "research",
            "move_forward",
            "move_to_planning",
            "move_to_running",
            "move_to_review",
            "move_to_done",
            "resume",
            "escalate_to_user",
        ];
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
        description = "Read the last N lines of a task's agent tmux pane. Use this to understand what the agent is showing — e.g., when a task has been idle for a while. Returns pane content as text."
    )]
    fn read_pane_content(&self, Parameters(params): Parameters<ReadPaneParams>) -> String {
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

        let lines = params.lines.unwrap_or(50);
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
                let content = String::from_utf8_lossy(&out.stdout).to_string();
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
        description = "Send a message to a task's agent pane (followed by Enter). Only works for tasks in Planning or Running status. Use this to nudge a stuck agent, answer a CLI prompt (e.g. 'y' for yes), or provide guidance."
    )]
    fn send_to_task(&self, Parameters(params): Parameters<SendToTaskParams>) -> String {
        let db = match self.open_project_db_for(params.project_id.as_deref()) {
            Ok(db) => db,
            Err(e) => return e,
        };

        let task = match db.get_task(&params.task_id) {
            Ok(Some(t)) => t,
            Ok(None) => return format!("Task not found: {}", params.task_id),
            Err(e) => return format!("Error getting task: {}", e),
        };

        // Only allow sending to active phases
        if !matches!(task.status, TaskStatus::Planning | TaskStatus::Running) {
            return format!(
                "Error: task is not in an active phase (current: {}). send_to_task only works for Planning or Running tasks.",
                task.status.as_str()
            );
        }

        let session_name = match task.session_name {
            Some(ref s) => s.clone(),
            None => return format!("Task {} has no active session", params.task_id),
        };

        // Send the message text
        let send_text = Command::new("tmux")
            .args([
                "-L",
                "agtx",
                "send-keys",
                "-t",
                &session_name,
                &params.message,
            ])
            .output();

        if let Err(e) = send_text {
            return format!("Error sending message: {}", e);
        }

        // Send Enter
        let send_enter = Command::new("tmux")
            .args(["-L", "agtx", "send-keys", "-t", &session_name, "Enter"])
            .output();

        match send_enter {
            Ok(_) => {
                let response = SendToTaskResponse {
                    task_id: params.task_id,
                    session_name,
                    success: true,
                    message: format!("Message sent: {}", params.message),
                };
                serde_json::to_string_pretty(&response)
                    .unwrap_or_else(|e| format!("Error serializing: {}", e))
            }
            Err(e) => format!("Error sending Enter: {}", e),
        }
    }

    #[tool(
        description = "Create a new task in the Backlog column. Returns the created task's ID. Use create_tasks_batch for multiple tasks with dependencies. In global mode, project_id is required — call list_projects first."
    )]
    fn create_task(&self, Parameters(params): Parameters<CreateTaskParams>) -> String {
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
    fn create_tasks_batch(
        &self,
        Parameters(params): Parameters<CreateTasksBatchParams>,
    ) -> String {
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
                 check if a transition completed, and delete_task to remove backlog tasks.",
            ServerMode::Project(_) =>
                "agtx MCP server — control the terminal kanban board for coding agents. \
                 Use list_tasks to see current tasks, create_task or create_tasks_batch to add new tasks \
                 (with optional dependency wiring via referenced_tasks), update_task to modify backlog \
                 task fields, move_task to transition tasks between phases, get_transition_status to \
                 check if a transition completed, and delete_task to remove backlog tasks.",
        };
        ServerInfo {
            instructions: Some(instructions.into()),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
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
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
