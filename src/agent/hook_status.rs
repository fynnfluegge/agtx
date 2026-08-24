//! Agent-reported lifecycle status, written by agent hooks and read by the TUI.
//!
//! Agents that support lifecycle hooks (currently Claude Code) are configured to
//! invoke `agtx hook <task-id> <worktree>` on session/turn/tool events. That
//! subcommand parses the hook's JSON payload from stdin and writes
//! `.agtx/status/{task_id}.json` inside the worktree. The board's session-refresh
//! thread reads that file instead of guessing liveness from `tmux capture-pane`
//! output.
//!
//! This module is deliberately free of tmux, database and TUI types so the event
//! mapping and staleness rules can be unit-tested in isolation — the same
//! precedent as `src/tui/dep_graph.rs`.

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::{Path, PathBuf};

/// Directory (relative to a worktree) holding per-task status files.
pub const STATUS_DIR: &str = ".agtx/status";

/// A `Working` record older than this is distrusted and the caller falls back to
/// the pane-hash heuristic. Covers an agent killed without firing `SessionEnd`
/// and an agent build that dropped an event. Only `Working` decays — a blocked
/// agent stays blocked indefinitely.
pub const HOOK_STALE_SECS: i64 = 300;

/// How long a `Blocked` record is protected from being overwritten by a
/// `Working` event. Claude can emit a `PreToolUse` for the very tool it is
/// asking permission for, which would otherwise clear the block immediately.
pub const BLOCKED_GUARD_SECS: i64 = 2;

/// Maximum stdin payload accepted from a hook invocation.
pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

/// Longest `message` / `tool` string retained; hook payloads are untrusted input
/// and this text reaches the TUI and orchestrator notifications.
const MAX_FIELD_CHARS: usize = 512;

/// What the agent last told us about itself.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AgentHookStatus {
    /// Unix seconds when the event was recorded.
    pub ts: i64,
    pub state: HookState,
    /// Agent-reported session id, for future resume-by-id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Path to the agent's own transcript, when reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_path: Option<String>,
    /// The permission prompt or question text, when the agent is Blocked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// Tool name from the last PreToolUse — heartbeat detail.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    /// Which agent wrote this record.
    pub agent: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum HookState {
    /// Turn in progress, or a tool is running.
    Working,
    /// Stopped, awaiting a permission decision or an answer from the user.
    Blocked,
    /// Turn ended; the composer is idle and accepting input.
    Waiting,
    /// Session terminated.
    Ended,
}

/// Map a Claude Code `hook_event_name` to a state.
///
/// `None` means the event carries no state transition and any stored record
/// should be left alone rather than overwritten.
pub fn map_claude_event(event: &str) -> Option<HookState> {
    match event {
        "SessionStart" | "UserPromptSubmit" | "PreToolUse" => Some(HookState::Working),
        "PermissionRequest" | "Notification" => Some(HookState::Blocked),
        "Stop" | "StopFailure" => Some(HookState::Waiting),
        "SessionEnd" => Some(HookState::Ended),
        _ => None,
    }
}

/// Path to a task's status file inside its worktree.
pub fn status_path(worktree: &Path, task_id: &str) -> PathBuf {
    worktree.join(STATUS_DIR).join(format!("{}.json", task_id))
}

/// Read and parse a task's status record.
///
/// Returns `None` when the file is missing, unparseable, or holds a `Working`
/// record older than [`HOOK_STALE_SECS`] — in every one of those cases the
/// caller should fall back to the pane-hash heuristic.
pub fn read_status(worktree: &Path, task_id: &str, now: i64) -> Option<AgentHookStatus> {
    let raw = std::fs::read_to_string(status_path(worktree, task_id)).ok()?;
    let status: AgentHookStatus = serde_json::from_str(&raw).ok()?;
    if status.state == HookState::Working && now.saturating_sub(status.ts) > HOOK_STALE_SECS {
        return None;
    }
    Some(status)
}

/// Write a status record atomically: temp file in the same directory, then rename.
///
/// The rename is what makes a concurrent reader see either the old record or the
/// new one, never a half-written file.
pub fn write_status(worktree: &Path, task_id: &str, status: &AgentHookStatus) -> Result<()> {
    let path = status_path(worktree, task_id);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Include the pid so two hooks racing on the same task can't truncate each
    // other's temp file before either rename lands.
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, serde_json::to_string(status)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// Decide the record to persist given an incoming event and whatever is stored.
///
/// Returns `None` when the event should not change the stored record. Two rules:
/// an event with no state mapping is ignored, and a `Working` event is dropped
/// while a fresh `Blocked` record stands — Claude emits `PreToolUse` for the very
/// tool it is requesting permission for, which would otherwise clear the block.
pub fn merge_event(
    previous: Option<&AgentHookStatus>,
    event: &str,
    now: i64,
    agent: &str,
    session_id: Option<String>,
    transcript_path: Option<String>,
    message: Option<String>,
    tool: Option<String>,
) -> Option<AgentHookStatus> {
    let state = map_claude_event(event)?;

    if state == HookState::Working {
        if let Some(prev) = previous {
            if prev.state == HookState::Blocked && now.saturating_sub(prev.ts) <= BLOCKED_GUARD_SECS
            {
                return None;
            }
        }
    }

    // Carry forward identity fields the current event didn't report, so a `Stop`
    // payload doesn't erase the session id learned at `SessionStart`.
    let prev_session = previous.and_then(|p| p.session_id.clone());
    let prev_transcript = previous.and_then(|p| p.transcript_path.clone());

    Some(AgentHookStatus {
        ts: now,
        state,
        session_id: session_id.or(prev_session),
        transcript_path: transcript_path.or(prev_transcript),
        // `message` is only meaningful for the event that produced it.
        message: if state == HookState::Blocked {
            message.map(|m| truncate(&m))
        } else {
            None
        },
        tool: tool.map(|t| truncate(&t)),
        agent: agent.to_string(),
    })
}

fn truncate(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.chars().count() <= MAX_FIELD_CHARS {
        return trimmed.to_string();
    }
    trimmed.chars().take(MAX_FIELD_CHARS).collect()
}

/// Fields lifted from a hook's JSON payload. Every field is optional — agents
/// disagree about which they send, and a missing field must never be fatal.
#[derive(Debug, Default, Deserialize)]
struct HookPayload {
    hook_event_name: Option<String>,
    session_id: Option<String>,
    transcript_path: Option<String>,
    message: Option<String>,
    tool_name: Option<String>,
}

/// Environment variables identifying the task a hook is reporting about, set on
/// the tmux window by `create_window`.
pub const ENV_TASK_ID: &str = "AGTX_TASK_ID";
pub const ENV_WORKTREE: &str = "AGTX_WORKTREE";

/// Entry point for `agtx hook --env` (preferred) or `agtx hook <task-id> <worktree> [agent]`.
///
/// `--env` reads the task from the process environment instead of argv, so the
/// registered hook command is identical for every task. That matters because a
/// single config file can serve several tasks — under `skip_worktree` they all
/// share the project's `.claude/settings.local.json` — and because a
/// task-agnostic command is what a future user-global install would need.
///
/// If the variables are absent the hook exits silently: a globally registered
/// hook must be inert in the user's own sessions outside agtx.
///
/// Always succeeds. A hook that reports an error can break the agent's turn, and
/// no status update is worth that — every failure path here is a silent no-op
/// after the mandatory `{}` on stdout.
pub fn run_hook_cli(args: &[String]) -> Result<()> {
    // Claude-compatible permission hooks fail closed on empty stdout, so this
    // must be printed before any early return.
    println!("{{}}");

    let from_env = args.first().map(String::as_str) == Some("--env");
    let (task_id, worktree, agent);
    if from_env {
        // A backgrounded agent task runs in a worker that inherited the pane's
        // env, so these would name a task it is not running in.
        if std::env::var_os("CLAUDE_JOB_DIR").is_some() {
            return Ok(());
        }
        let (Ok(t), Ok(w)) = (std::env::var(ENV_TASK_ID), std::env::var(ENV_WORKTREE)) else {
            return Ok(());
        };
        if t.is_empty() || w.is_empty() {
            return Ok(());
        }
        task_id = t;
        worktree = w;
        agent = args.get(1).cloned().unwrap_or_else(|| "claude".to_string());
    } else {
        let (Some(t), Some(w)) = (args.first(), args.get(1)) else {
            return Ok(());
        };
        task_id = t.clone();
        worktree = w.clone();
        agent = args.get(2).cloned().unwrap_or_else(|| "claude".to_string());
    }
    let (task_id, worktree, agent) = (task_id.as_str(), worktree.as_str(), agent.as_str());

    let mut buf = Vec::new();
    if std::io::stdin()
        .take(MAX_PAYLOAD_BYTES as u64)
        .read_to_end(&mut buf)
        .is_err()
    {
        return Ok(());
    }
    let payload: HookPayload = serde_json::from_slice(&buf).unwrap_or_default();
    let Some(event) = payload.hook_event_name.as_deref() else {
        return Ok(());
    };

    let worktree = Path::new(worktree);
    let now = chrono::Utc::now().timestamp();
    let previous = read_status(worktree, task_id, now);

    if let Some(next) = merge_event(
        previous.as_ref(),
        event,
        now,
        agent,
        payload.session_id,
        payload.transcript_path,
        payload.message,
        payload.tool_name,
    ) {
        let _ = write_status(worktree, task_id, &next);
    }
    Ok(())
}
