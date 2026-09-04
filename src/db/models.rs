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
    /// The task this is about. `None` only for rows written before the column
    /// existed — every producer sets it.
    pub task_id: Option<String>,
    /// What kind of event this is. Separate from `message` because a consumer
    /// outside the TUI filters on the event, not on prose: a chat bridge that
    /// wakes someone for [`TaskStuck`](NotificationKind::TaskStuck) but not for
    /// [`PhaseCompleted`](NotificationKind::PhaseCompleted) cannot express that
    /// against a formatted string.
    pub kind: Option<NotificationKind>,
}

/// Why a [`Notification`] was raised.
///
/// The serde spellings match [`as_str`](Self::as_str) so the stored value and
/// the wire value cannot drift — a consumer reading the DB directly and one
/// reading serialised output must agree on the name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NotificationKind {
    /// A task's phase artifact appeared — it can move forward.
    PhaseCompleted,
    /// A task has stopped making progress and may need a human.
    TaskStuck,
}

impl NotificationKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            NotificationKind::PhaseCompleted => "phase_completed",
            NotificationKind::TaskStuck => "task_stuck",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "phase_completed" => Some(NotificationKind::PhaseCompleted),
            "task_stuck" => Some(NotificationKind::TaskStuck),
            _ => None,
        }
    }
}

impl Notification {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            message: message.into(),
            created_at: Utc::now(),
            task_id: None,
            kind: None,
        }
    }

    /// The form every producer should use: a notification is always *about* a
    /// task, and a consumer outside this process needs both halves to route it.
    pub fn for_task(
        kind: NotificationKind,
        task_id: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            task_id: Some(task_id.into()),
            kind: Some(kind),
            ..Self::new(message)
        }
    }
}

/// Phase completion status.
///
/// The TUI computes this on its own thread and reads its in-memory copy; the
/// `task_runtime` table is a *published* mirror for readers in other processes,
/// which cannot recompute it without duplicating the artifact-check and
/// pane-hash pipeline. A reader must treat a row as a snapshot and check
/// `updated_at`: with no TUI running nothing refreshes it, and a frozen
/// [`Working`](Self::Working) must not be presented as live.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhaseStatus {
    /// Agent is still working, no artifact yet
    Working,
    /// Agent is stopped waiting on a permission prompt or a question.
    ///
    /// Set from an agent-reported hook event, or from a match against that
    /// agent's own `security` dialogs in the pane (`visible_security_dialog`,
    /// which runs only while `auto_trust` is off). Never from silence: "no output
    /// for 15s" is [`Idle`](Self::Idle), which guesses, where this names what the
    /// task is waiting on.
    Blocked,
    /// Agent output hasn't changed for 15s — may need user input
    Idle,
    /// Phase artifact detected, ready to advance
    Ready,
    /// Tmux window gone (process exited)
    Exited,
}

impl PhaseStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            PhaseStatus::Working => "working",
            PhaseStatus::Blocked => "blocked",
            PhaseStatus::Idle => "idle",
            PhaseStatus::Ready => "ready",
            PhaseStatus::Exited => "exited",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "working" => Some(PhaseStatus::Working),
            "blocked" => Some(PhaseStatus::Blocked),
            "idle" => Some(PhaseStatus::Idle),
            "ready" => Some(PhaseStatus::Ready),
            "exited" => Some(PhaseStatus::Exited),
            _ => None,
        }
    }
}

/// A published snapshot of one task's runtime state, for readers outside the
/// TUI process. Written by the session refresh; never read back by the TUI,
/// which has the live values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskRuntime {
    pub task_id: String,
    pub phase_status: PhaseStatus,
    /// Hash of the last pane capture, and when it last changed. Carried so a
    /// reader can distinguish "idle because the agent is quiet" from "idle
    /// because nothing has refreshed this row".
    pub pane_hash: Option<String>,
    pub pane_changed_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

/// A phone (or anything else) paired with `agtx serve`.
///
/// One row per device rather than one shared secret, so a lost phone can be
/// revoked without re-pairing everything else — which is the whole reason this
/// exists rather than the single token it replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileDevice {
    pub id: String,
    /// What the user calls it. Free text from the pairing request, so it is
    /// shown but never trusted or matched on.
    pub label: String,
    /// SHA-256 of the token, hex. The token itself is shown once, at pairing,
    /// and never stored.
    pub token_hash: String,
    pub created_at: DateTime<Utc>,
    pub last_seen: Option<DateTime<Utc>>,
}

impl MobileDevice {
    pub fn new(label: impl Into<String>, token_hash: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            label: label.into(),
            token_hash: token_hash.into(),
            created_at: Utc::now(),
            last_seen: None,
        }
    }
}
