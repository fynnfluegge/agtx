//! What a task is allowed to do next, and who is asking.
//!
//! Shared by the MCP server and the web API so the phone and the orchestrator
//! cannot drift apart on what a task permits.

use crate::db::{Task, TaskStatus};

/// Who is asking what a task may do.
///
/// The answer genuinely differs, and collapsing the two is a bug in both
/// directions. The orchestrator does not manage Backlog — a human triages it —
/// so offering it `research` would have it starting work the user has not
/// looked at. A person opening the board has the opposite need: Backlog triage
/// is most of what they came to do, and a Backlog card with no actions is a
/// dead end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    /// The orchestrator agent, driving the board unattended.
    Orchestrator,
    /// A person, through a UI.
    Human,
}

/// Every `move_task` verb, in one place so the MCP tool and the web API cannot
/// disagree about what a valid action is even called.
pub const ACTIONS: &[&str] = &[
    "research",
    "move_forward",
    "move_to_planning",
    "move_to_running",
    "move_to_review",
    "move_to_done",
    "resume",
    "escalate_to_user",
];

/// The longest a task title may be. Mirrors the wizard's own cap, so a task
/// created from a phone cannot be one the desktop refuses to edit.
pub const MAX_TASK_TITLE_CHARS: usize = 120;

/// The actions `task` currently permits, named as the MCP `move_task` verbs.
///
/// `deps_satisfied` comes from [`crate::db::Database::deps_satisfied`]: a task
/// whose referenced tasks are not yet in Review or Done cannot leave Backlog.
pub fn allowed_actions(task: &Task, deps_satisfied: bool, caller: CallerKind) -> Vec<String> {
    let mut actions: Vec<String> = Vec::new();

    match task.status {
        TaskStatus::Backlog => {
            // The orchestrator is a coordinator, not a triager: it moves work
            // that a person has already decided to start. Everything a human
            // can do from Backlog is what the board's `R`, `m` and `M` keys do.
            if caller == CallerKind::Human {
                actions.push("research".to_string());
                actions.push("move_to_planning".to_string());
                actions.push("move_to_running".to_string());
            }
        }
        TaskStatus::Planning | TaskStatus::Running => {
            actions.push("move_forward".to_string());
            // `escalate_to_user` means "stop and ask the person". Offering it to
            // the person is incoherent — they are already looking at the task —
            // so it belongs to the orchestrator alone.
            if caller == CallerKind::Orchestrator {
                actions.push("escalate_to_user".to_string());
            }
        }
        TaskStatus::Review => {
            actions.push("move_to_done".to_string());
            actions.push("resume".to_string());
        }
        TaskStatus::Done => {}
    }

    // Block forward transitions out of Backlog when dependencies are not
    // satisfied. Reachable only for `Human`, since the orchestrator is offered
    // nothing from Backlog to retain — but it belongs here rather than inside
    // the arm above, because the rule is about the transition, not the caller.
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

/// Why an action was refused.
///
/// The two cases are different kinds of wrong and a caller should treat them
/// differently: an unknown verb is a malformed request that will never work,
/// while a verb the task does not currently permit may succeed once the board
/// moves. Collapsing them tells a client to give up on something it should
/// retry, or to retry something it should fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionRefusal {
    /// Not a verb agtx knows.
    UnknownAction(String),
    /// A real verb, not available in this task's current state.
    NotPermitted(String),
}

impl ActionRefusal {
    pub fn message(&self) -> &str {
        match self {
            ActionRefusal::UnknownAction(m) | ActionRefusal::NotPermitted(m) => m,
        }
    }
}

/// Whether `caller` may ask for `action` on `task` right now.
pub fn validate_action(
    task: &Task,
    deps_satisfied: bool,
    caller: CallerKind,
    action: &str,
) -> Result<(), ActionRefusal> {
    if !ACTIONS.contains(&action) {
        return Err(ActionRefusal::UnknownAction(format!(
            "unknown action {action:?}; valid actions are {}",
            ACTIONS.join(", ")
        )));
    }

    let allowed = allowed_actions(task, deps_satisfied, caller);
    if allowed.iter().any(|a| a == action) {
        return Ok(());
    }

    // Separate "not yet" from "not ever": a dependency clears on its own, and
    // telling someone their tap was invalid when it was merely early sends them
    // looking for a bug in the wrong place.
    if !deps_satisfied
        && task.status == TaskStatus::Backlog
        && matches!(
            action,
            "move_forward" | "move_to_planning" | "move_to_running"
        )
    {
        return Err(ActionRefusal::NotPermitted(format!(
            "{action} is blocked: this task's dependencies are not all in Review or Done yet"
        )));
    }

    Err(ActionRefusal::NotPermitted(format!(
        "{action} is not available for a task in {}{}",
        task.status.as_str(),
        if allowed.is_empty() {
            String::new()
        } else {
            format!("; try one of {}", allowed.join(", "))
        }
    )))
}
