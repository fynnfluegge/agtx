//! Agent-reported lifecycle status, written by agent hooks and read by the TUI.
//!
//! Agents that support lifecycle hooks are configured to invoke `agtx hook --env
//! <agent>` on session/turn/tool events. That subcommand parses the hook's JSON
//! payload from stdin and writes `.agtx/status/{task_id}.json` inside the
//! worktree. The board's session-refresh thread reads that file instead of
//! guessing liveness from `tmux capture-pane` output.
//!
//! Which agents, and where their config goes, is [`HookConfigKind`]; every one
//! of them is project-local, so registration lives and dies with the worktree.
//!
//! Two things here are per-agent: [`map_hook_event`], because the vocabularies
//! differ, and where the event name comes from ([`super::spec::HookEventSource`]).
//! Merging, staleness, the `Blocked` guard and the atomic write are shared.
//!
//! This module is deliberately free of tmux, database and TUI types so the event
//! mapping and staleness rules can be unit-tested in isolation — the same
//! precedent as `src/tui/dep_graph.rs`.

use crate::agent::spec::HookConfigKind;
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
/// `Working` event. An agent can emit its tool heartbeat for the very tool it is
/// requesting permission for, which would otherwise clear the block instantly.
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

/// Map one agent's lifecycle event name to a state.
///
/// `None` means the event carries no transition and the stored record is left
/// alone. An event from another agent's vocabulary also returns `None`: the kind
/// is a parameter, not a fallback chain, so a registration typo cannot resolve
/// against the wrong agent's arm. `Stop` means "turn over" to four agents while
/// Gemini's equivalent is `AfterAgent` and Cursor's is lowercase `stop`.
///
/// Each arm names the version it was read from.
pub fn map_hook_event(kind: HookConfigKind, event: &str) -> Option<HookState> {
    use HookConfigKind::*;
    match kind {
        // claude 2.1.247.
        ClaudeSettings => match event {
            "SessionStart" | "UserPromptSubmit" | "PreToolUse" => Some(HookState::Working),
            "PermissionRequest" | "Notification" => Some(HookState::Blocked),
            "Stop" | "StopFailure" => Some(HookState::Waiting),
            "SessionEnd" => Some(HookState::Ended),
            _ => None,
        },
        // codex-cli 0.144.5. Claude's schema minus `SessionEnd` (window-gone
        // detection covers Exited) and `Notification`.
        CodexHooksJson => match event {
            "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse" => {
                Some(HookState::Working)
            }
            "PermissionRequest" => Some(HookState::Blocked),
            "Stop" => Some(HookState::Waiting),
            _ => None,
        },
        // gemini-cli 0.46.0. Its own vocabulary: the turn boundary is
        // `BeforeAgent`/`AfterAgent` and the tool heartbeat is `BeforeTool`.
        // `Notification` fires on a ToolPermission alert and carries `message`.
        GeminiSettings => match event {
            "SessionStart" | "BeforeAgent" | "BeforeTool" | "AfterTool" => {
                Some(HookState::Working)
            }
            "Notification" => Some(HookState::Blocked),
            "AfterAgent" => Some(HookState::Waiting),
            "SessionEnd" => Some(HookState::Ended),
            _ => None,
        },
        // cursor-agent 2026.08.25. camelCase, and no event for a human-blocking
        // prompt: the `beforeShellExecution` family are policy gates that decide,
        // not prompts that wait. Cursor never reports Blocked.
        CursorHooksJson => match event {
            "sessionStart" | "beforeSubmitPrompt" | "preToolUse" | "postToolUse" => {
                Some(HookState::Working)
            }
            "stop" => Some(HookState::Waiting),
            "sessionEnd" => Some(HookState::Ended),
            _ => None,
        },
        // grok 1.0.5. Config and payload disagree about spelling: hooks are
        // registered under `PreToolUse` and arrive as `"pre_tool_use"`, so both
        // must map — hence `squash`.
        //
        // `notification` counts as Blocked only because the registration pins
        // `matcher: "permission_prompt"`; unscoped it also fires for
        // `idle_prompt` and `task_complete`.
        GrokHooksJson => match squash(event).as_str() {
            "sessionstart" | "userpromptsubmit" | "pretooluse" => Some(HookState::Working),
            "notification" => Some(HookState::Blocked),
            "stop" | "stopfailure" | "stopcancelled" => Some(HookState::Waiting),
            "sessionend" => Some(HookState::Ended),
            _ => None,
        },
        // agy 1.1.21. Five events, no session lifecycle and no permission event.
        // `PreInvocation` fires before each model call — its nearest thing to a
        // turn start; `Stop` ends the execution loop, not the session.
        AntigravityHooksJson => match event {
            "PreInvocation" | "PostInvocation" | "PostToolUse" => Some(HookState::Working),
            "Stop" => Some(HookState::Waiting),
            _ => None,
        },
    }
}

/// Lowercase and drop underscores, so one arm accepts both spellings grok uses
/// for an event: `PreToolUse` when registered, `pre_tool_use` when reported.
fn squash(event: &str) -> String {
    event.chars().filter(|c| *c != '_').flat_map(char::to_lowercase).collect()
}

/// The events agtx registers for one agent.
///
/// The write half of the pair whose read half is [`map_hook_event`]. A name
/// listed here but not mapped there is a hook that fires and reports nothing, so
/// `tests/hook_status_tests.rs` asserts the two agree in both directions.
///
/// The second element is the event's tool matcher, or `None` when the event is
/// not tool-scoped.
pub fn hook_events(kind: HookConfigKind) -> &'static [(&'static str, Option<&'static str>)] {
    use HookConfigKind::*;
    match kind {
        ClaudeSettings => &[
            // Liveness: a turn started, or a tool is about to run (heartbeat).
            ("SessionStart", None),
            ("UserPromptSubmit", None),
            ("PreToolUse", Some("*")),
            // Blocked: the agent is stopped waiting on a human.
            ("PermissionRequest", Some("*")),
            ("Notification", None),
            // Turn over / session over.
            ("Stop", None),
            ("StopFailure", None),
            ("SessionEnd", None),
        ],
        CodexHooksJson => &[
            ("SessionStart", None),
            ("UserPromptSubmit", None),
            ("PreToolUse", Some("*")),
            ("PostToolUse", Some("*")),
            ("PermissionRequest", Some("*")),
            ("Stop", None),
        ],
        GeminiSettings => &[
            ("SessionStart", Some("*")),
            ("BeforeAgent", Some("*")),
            ("BeforeTool", Some("*")),
            ("AfterTool", Some("*")),
            ("Notification", Some("*")),
            ("AfterAgent", Some("*")),
            ("SessionEnd", Some("*")),
        ],
        CursorHooksJson => &[
            ("sessionStart", None),
            ("beforeSubmitPrompt", None),
            ("preToolUse", None),
            ("postToolUse", None),
            ("stop", None),
            ("sessionEnd", None),
        ],
        // Registered in PascalCase, reported in snake_case; see the grok arm of
        // `map_hook_event`.
        GrokHooksJson => &[
            ("SessionStart", None),
            ("UserPromptSubmit", None),
            ("PreToolUse", Some("*")),
            // Scoped: unmatched, this also fires on idle and task-complete.
            ("Notification", Some("permission_prompt")),
            ("Stop", None),
            ("StopFailure", None),
            ("StopCancelled", None),
            ("SessionEnd", None),
        ],
        // No `PreToolUse`, deliberately: antigravity makes that hook's
        // `decision` output *required* and reads anything else as a refusal, so
        // subscribing blocks every tool call. Answering `"allow"` would make a
        // liveness reporter into a permission granter. `PostToolUse` carries the
        // heartbeat instead and wants exactly the `{}` this hook prints.
        AntigravityHooksJson => &[
            ("PreInvocation", None),
            ("PostInvocation", None),
            ("PostToolUse", Some("*")),
            ("Stop", None),
        ],
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
/// an event with no mapping *for this agent* is ignored, and a `Working` event is
/// dropped while a fresh `Blocked` record stands (see [`BLOCKED_GUARD_SECS`]).
pub fn merge_event(
    previous: Option<&AgentHookStatus>,
    kind: HookConfigKind,
    event: &str,
    now: i64,
    agent: &str,
    session_id: Option<String>,
    transcript_path: Option<String>,
    message: Option<String>,
    tool: Option<String>,
) -> Option<AgentHookStatus> {
    let state = map_hook_event(kind, event)?;

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

/// Fields lifted from a hook's JSON payload. Every field is optional: agents
/// disagree about which they send, and a missing field must never be fatal.
///
/// The aliases carry the two agents that do not send `snake_case`.
#[derive(Debug, Default, Deserialize)]
struct HookPayload {
    /// Grok spells this `hookEventName`, with a snake_case value.
    #[serde(alias = "hookEventName")]
    hook_event_name: Option<String>,
    #[serde(alias = "sessionId")]
    session_id: Option<String>,
    /// `conversationId` is antigravity's session identifier.
    #[serde(alias = "conversationId")]
    conversation_id: Option<String>,
    #[serde(alias = "transcriptPath")]
    transcript_path: Option<String>,
    message: Option<String>,
    #[serde(alias = "toolName")]
    tool_name: Option<String>,
    /// Antigravity's `PreToolUse` payload nests the tool under `toolCall.name`.
    #[serde(alias = "toolCall")]
    tool_call: Option<ToolCall>,
}

#[derive(Debug, Default, Deserialize)]
struct ToolCall {
    name: Option<String>,
}

/// Environment variables identifying the task a hook is reporting about, set on
/// the tmux window by `create_window`.
pub const ENV_TASK_ID: &str = "AGTX_TASK_ID";
pub const ENV_WORKTREE: &str = "AGTX_WORKTREE";

/// Entry point for `agtx hook --env <agent> [--event <Name>]` (preferred) or the
/// legacy `agtx hook <task-id> <worktree> [agent]`.
///
/// `--env` reads the task from the process environment, so one registered command
/// serves every task — necessary under `skip_worktree`, where all of them share
/// the project's agent config.
///
/// `--event` names the firing event for an agent whose payload does not
/// ([`super::spec::HookEventSource::Argv`]). The payload wins when it has one, so
/// an agent that starts sending an event name is not shadowed by a stale argv
/// value.
///
/// If the task variables are absent the hook exits silently, so a registration
/// stays inert outside agtx.
///
/// Always succeeds. A hook that reports an error can break the agent's turn, so
/// every failure path is a silent no-op after the mandatory `{}` on stdout.
pub fn run_hook_cli(args: &[String]) -> Result<()> {
    // Claude-compatible permission hooks fail closed on empty stdout, so this
    // must be printed before any early return.
    println!("{{}}");

    // Lifted out first so the positional parsing below is unaffected.
    let mut argv_event: Option<String> = None;
    let mut args: Vec<String> = args.to_vec();
    if let Some(i) = args.iter().position(|a| a == "--event") {
        argv_event = args.get(i + 1).cloned();
        args.drain(i..=(i + 1).min(args.len() - 1));
    }
    let args = &args[..];

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
    record_hook_event(task_id, worktree, agent, argv_event.as_deref(), &buf);
    Ok(())
}

/// Everything after the argv and stdin plumbing: decide what the payload means
/// and persist it. Split out so tests can supply a payload without real stdin.
///
/// `argv_event` names the firing event for an agent whose payload does not carry
/// one; it is consulted only when the payload has no `hook_event_name`.
pub fn record_hook_event(
    task_id: &str,
    worktree: &str,
    agent: &str,
    argv_event: Option<&str>,
    payload: &[u8],
) {
    // An unknown agent name — a typo, or a spec removed under a live worktree —
    // must not borrow another agent's vocabulary. See `map_hook_event`.
    let Some(kind) = crate::agent::spec(agent).and_then(|s| s.hook_config) else {
        return;
    };

    let payload: HookPayload = serde_json::from_slice(payload).unwrap_or_default();
    // Payload first: an agent that sends an event name outranks the registration.
    let Some(event) = payload
        .hook_event_name
        .as_deref()
        .or(argv_event)
        .filter(|e| !e.is_empty())
    else {
        return;
    };

    let worktree = Path::new(worktree);
    let now = chrono::Utc::now().timestamp();
    let previous = read_status(worktree, task_id, now);

    if let Some(next) = merge_event(
        previous.as_ref(),
        kind,
        event,
        now,
        agent,
        payload.session_id.or(payload.conversation_id),
        payload.transcript_path,
        payload.message,
        payload.tool_name.or(payload.tool_call.and_then(|t| t.name)),
    ) {
        let _ = write_status(worktree, task_id, &next);
    }
}
