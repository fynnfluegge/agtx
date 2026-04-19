use crate::tmux::safe_session_name;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Task status in the kanban board
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TaskStatus {
    Backlog,
    Planning,
    Running,
    Review,
    Done,
}

impl TaskStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TaskStatus::Backlog => "backlog",
            TaskStatus::Planning => "planning",
            TaskStatus::Running => "running",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            TaskStatus::Backlog => "backlog/research",
            TaskStatus::Planning => "planning",
            TaskStatus::Running => "running",
            TaskStatus::Review => "review",
            TaskStatus::Done => "done",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "backlog" => Some(TaskStatus::Backlog),
            "planning" => Some(TaskStatus::Planning),
            "running" => Some(TaskStatus::Running),
            "review" => Some(TaskStatus::Review),
            "done" => Some(TaskStatus::Done),
            _ => None,
        }
    }

    pub fn columns() -> &'static [TaskStatus] {
        &[
            TaskStatus::Backlog,
            TaskStatus::Planning,
            TaskStatus::Running,
            TaskStatus::Review,
            TaskStatus::Done,
        ]
    }
}

/// A task on the kanban board
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub description: Option<String>,
    pub status: TaskStatus,
    pub agent: String,
    pub project_id: String,
    pub session_name: Option<String>,
    pub worktree_path: Option<String>,
    pub branch_name: Option<String>,
    pub pr_number: Option<i32>,
    pub pr_url: Option<String>,
    pub plugin: Option<String>,
    pub cycle: i32,
    pub referenced_tasks: Option<String>,
    pub escalation_note: Option<String>,
    pub base_branch: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl Task {
    pub fn new(
        title: impl Into<String>,
        agent: impl Into<String>,
        project_id: impl Into<String>,
    ) -> Self {
        let id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now();
        Self {
            id,
            title: title.into(),
            description: None,
            status: TaskStatus::Backlog,
            agent: agent.into(),
            project_id: project_id.into(),
            session_name: None,
            worktree_path: None,
            branch_name: None,
            pr_number: None,
            pr_url: None,
            plugin: None,
            cycle: 1,
            referenced_tasks: None,
            escalation_note: None,
            base_branch: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Returns the task description if present, otherwise the title.
    pub fn content_text(&self) -> String {
        self.description
            .as_deref()
            .unwrap_or(&self.title)
            .to_string()
    }

    /// Generate tmux session name: task-{id}--{project}--{slug}
    pub fn generate_session_name(&self, project_name: &str) -> String {
        let project_name = safe_session_name(project_name);
        let slug = self
            .title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>();
        let slug = slug.trim_matches('-');
        // Truncate slug to keep session name reasonable
        let slug: String = slug.chars().take(20).collect();
        format!("task-{}--{}--{}", &self.id[..8], project_name, slug)
    }
}

/// A project tracked by agtx
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Project {
    pub id: String,
    pub name: String,
    pub path: String,
    pub github_url: Option<String>,
    pub default_agent: Option<String>,
    pub last_opened: DateTime<Utc>,
}

impl Project {
    pub fn new(name: impl Into<String>, path: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            path: path.into(),
            github_url: None,
            default_agent: None,
            last_opened: Utc::now(),
        }
    }
}

/// A queued request for a task state transition (used by MCP server).
/// The TUI polls this table and executes transitions with full side effects.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransitionRequest {
    pub id: String,
    pub task_id: String,
    pub action: String,
    pub reason: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub processed_at: Option<DateTime<Utc>>,
    pub error: Option<String>,
}

impl TransitionRequest {
    pub fn new(task_id: impl Into<String>, action: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            task_id: task_id.into(),
            action: action.into(),
            reason: None,
            requested_at: Utc::now(),
            processed_at: None,
            error: None,
        }
    }
}

/// Represents a running agent session
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunningAgent {
    pub session_name: String,
    pub project_id: String,
    pub task_id: String,
    pub agent_name: String,
    pub started_at: DateTime<Utc>,
    pub status: AgentStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AgentStatus {
    Running,
    Waiting,
    Completed,
}

impl AgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            AgentStatus::Running => "running",
            AgentStatus::Waiting => "waiting",
            AgentStatus::Completed => "completed",
        }
    }
}

/// A notification for the orchestrator agent (pull-based).
/// Events are written to the DB by the TUI and fetched by the orchestrator via MCP.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: String,
    pub message: String,
    pub created_at: DateTime<Utc>,
}

impl Notification {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.into(),
            created_at: Utc::now(),
        }
    }
}

/// Phase completion status (runtime-only, not persisted to DB)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStatus {
    /// Agent is still working, no artifact yet
    Working,
    /// Agent output hasn't changed for 15s — may need user input
    Idle,
    /// Phase artifact detected, ready to advance
    Ready,
    /// Tmux window gone (process exited)
    Exited,
}
