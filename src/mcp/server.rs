use std::path::PathBuf;
use std::process::Command;

use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars,
    tool, tool_handler, tool_router,
    transport::io::stdio,
};
use serde::{Deserialize, Serialize};

use crate::db::{Database, Task, TaskStatus, TransitionRequest};

// === Parameter types ===

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListProjectsParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListTasksParams {
    /// Filter by status: "backlog", "planning", "running", "review", "done". Omit for all tasks.
    #[schemars(description = "Filter by status: backlog, planning, running, review, done")]
    pub status: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTaskParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
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
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetTransitionStatusParams {
    /// The transition request ID returned by move_task
    #[schemars(description = "The transition request ID returned by move_task")]
    pub request_id: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CheckConflictsParams {
    /// Optional task ID. If omitted, checks all tasks in Review status.
    #[schemars(description = "Optional task ID. If omitted, checks all tasks in Review status.")]
    pub task_id: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GetNotificationsParams {}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadPaneParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
    /// Number of lines to read from the end of the pane (default 50)
    #[schemars(description = "Number of lines to read from the end of the pane (default 50)")]
    pub lines: Option<i32>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SendToTaskParams {
    /// The task ID (UUID)
    #[schemars(description = "The task ID (UUID)")]
    pub task_id: String,
    /// Message to send to the task's agent pane (followed by Enter)
    #[schemars(description = "Message to send to the task's agent pane (followed by Enter)")]
    pub message: String,
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
    created_at: String,
    updated_at: String,
    /// Actions the orchestrator can take on this task given its current status and plugin rules.
    allowed_actions: Vec<String>,
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

// === MCP Server ===

#[derive(Debug, Clone)]
pub struct AgtxMcpServer {
    project_path: PathBuf,
    tool_router: ToolRouter<Self>,
}

impl AgtxMcpServer {
    fn new(project_path: PathBuf) -> Self {
        Self {
            project_path,
            tool_router: Self::tool_router(),
        }
    }

    fn open_project_db(&self) -> Result<Database, String> {
        Database::open_project(&self.project_path).map_err(|e| format!("Failed to open project database: {}", e))
    }

    fn open_global_db(&self) -> Result<Database, String> {
        Database::open_global().map_err(|e| format!("Failed to open global database: {}", e))
    }

    /// Compute which move_task actions are valid for a task given its status and plugin rules.
    fn allowed_actions(&self, task: &Task) -> Vec<String> {
        let mut actions = Vec::new();

        let plugin = match &task.plugin {
            Some(name) => crate::config::WorkflowPlugin::load(name, Some(&self.project_path))
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
                    serde_json::to_string_pretty(&summaries).unwrap_or_else(|e| format!("Error serializing: {}", e))
                }
                Err(e) => format!("Error listing projects: {}", e),
            },
            Err(e) => e,
        }
    }

    #[tool(description = "List tasks for the current project, optionally filtered by status (backlog, planning, running, review, done)")]
    fn list_tasks(&self, Parameters(params): Parameters<ListTasksParams>) -> String {
        match self.open_project_db() {
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
                            .map(|t| TaskSummary {
                                id: t.id,
                                title: t.title,
                                description: t.description,
                                status: t.status.as_str().to_string(),
                                agent: t.agent,
                                branch_name: t.branch_name,
                                pr_url: t.pr_url,
                                plugin: t.plugin,
                            })
                            .collect();
                        serde_json::to_string_pretty(&summaries).unwrap_or_else(|e| format!("Error serializing: {}", e))
                    }
                    Err(e) => format!("Error listing tasks: {}", e),
                }
            }
            Err(e) => e,
        }
    }

    #[tool(description = "Get full details of a specific task by its ID. Includes allowed_actions based on the task's current status and plugin rules.")]
    fn get_task(&self, Parameters(params): Parameters<GetTaskParams>) -> String {
        match self.open_project_db() {
            Ok(db) => match db.get_task(&params.task_id) {
                Ok(Some(t)) => {
                    let allowed = self.allowed_actions(&t);
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
                        created_at: t.created_at.to_rfc3339(),
                        updated_at: t.updated_at.to_rfc3339(),
                        allowed_actions: allowed,
                    };
                    serde_json::to_string_pretty(&detail).unwrap_or_else(|e| format!("Error serializing: {}", e))
                }
                Ok(None) => format!("Task not found: {}", params.task_id),
                Err(e) => format!("Error getting task: {}", e),
            },
            Err(e) => e,
        }
    }

    #[tool(description = "Queue a task state transition. The agtx TUI will process it and execute all side effects (worktree creation, agent spawning, etc). Use get_transition_status to check completion. Actions: research (start research phase for backlog task), move_forward, move_to_planning, move_to_running, move_to_review, move_to_done, resume, escalate_to_user (flag task for user attention with an optional reason)")]
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

        match self.open_project_db() {
            Ok(db) => {
                // Verify task exists
                match db.get_task(&params.task_id) {
                    Ok(Some(_)) => {}
                    Ok(None) => return format!("Task not found: {}", params.task_id),
                    Err(e) => return format!("Error checking task: {}", e),
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

    #[tool(description = "Check the status of a queued transition request. Returns pending, completed, or error with details.")]
    fn get_transition_status(
        &self,
        Parameters(params): Parameters<GetTransitionStatusParams>,
    ) -> String {
        match self.open_project_db() {
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

    #[tool(description = "Check if task branches have merge conflicts with the main branch. Pass a task_id to check one task, or omit it to check all Review tasks. Uses a read-only git check — no files are modified.")]
    fn check_conflicts(&self, Parameters(params): Parameters<CheckConflictsParams>) -> String {
        let main_branch = match crate::git::detect_main_branch(&self.project_path) {
            Ok(b) => b,
            Err(e) => return format!("Failed to detect main branch: {}", e),
        };

        let tasks = match self.open_project_db() {
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

                match crate::git::check_merge_conflicts(&self.project_path, &main_branch, &branch) {
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
        serde_json::to_string_pretty(&response).unwrap_or_else(|e| format!("Error serializing: {}", e))
    }

    #[tool(description = "Fetch and consume pending notifications. Returns new events (task created, phase completed, etc.) and removes them from the queue. Note: notifications are also pushed to your input automatically when you are idle, so you usually don't need to call this manually.")]
    fn get_notifications(&self, _params: Parameters<GetNotificationsParams>) -> String {
        match self.open_project_db() {
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

    #[tool(description = "Read the last N lines of a task's agent tmux pane. Use this to understand what the agent is showing — e.g., when a task has been idle for a while. Returns pane content as text.")]
    fn read_pane_content(&self, Parameters(params): Parameters<ReadPaneParams>) -> String {
        let db = match self.open_project_db() {
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
            .args(["-L", "agtx", "capture-pane", "-t", &session_name, "-p", "-S", &lines_arg])
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

    #[tool(description = "Send a message to a task's agent pane (followed by Enter). Only works for tasks in Planning or Running status. Use this to nudge a stuck agent, answer a CLI prompt (e.g. 'y' for yes), or provide guidance.")]
    fn send_to_task(&self, Parameters(params): Parameters<SendToTaskParams>) -> String {
        let db = match self.open_project_db() {
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
            .args(["-L", "agtx", "send-keys", "-t", &session_name, &params.message])
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
}

#[tool_handler]
impl ServerHandler for AgtxMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            instructions: Some(
                "agtx MCP server — control the terminal kanban board for coding agents. \
                 Use list_tasks to see current tasks, move_task to transition tasks between phases, \
                 and get_transition_status to check if a transition completed."
                    .into(),
            ),
            capabilities: ServerCapabilities::builder().enable_tools().build(),
            ..Default::default()
        }
    }
}

pub async fn serve(project_path: PathBuf) -> anyhow::Result<()> {
    // Validate project DB can be opened
    Database::open_project(&project_path)?;

    let server = AgtxMcpServer::new(project_path);
    let service = server.serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
