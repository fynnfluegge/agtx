//! Tests for agent-reported hook status (`src/agent/hook_status.rs`).
//!
//! Event names and payload shapes here were captured from a live
//! `claude -p` run (Claude Code 2.1.241), not assumed.

use agtx::agent::hook_status::{
    map_claude_event, merge_event, read_status, status_path, write_status, AgentHookStatus,
    HookState, BLOCKED_GUARD_SECS, HOOK_STALE_SECS,
};
use std::path::Path;
use tempfile::TempDir;

fn status(state: HookState, ts: i64) -> AgentHookStatus {
    AgentHookStatus {
        ts,
        state,
        session_id: None,
        transcript_path: None,
        message: None,
        tool: None,
        agent: "claude".to_string(),
    }
}

// ── event mapping ───────────────────────────────────────────────────

#[test]
fn maps_every_registered_claude_event() {
    assert_eq!(map_claude_event("SessionStart"), Some(HookState::Working));
    assert_eq!(
        map_claude_event("UserPromptSubmit"),
        Some(HookState::Working)
    );
    assert_eq!(map_claude_event("PreToolUse"), Some(HookState::Working));
    assert_eq!(
        map_claude_event("PermissionRequest"),
        Some(HookState::Blocked)
    );
    assert_eq!(map_claude_event("Notification"), Some(HookState::Blocked));
    assert_eq!(map_claude_event("Stop"), Some(HookState::Waiting));
    assert_eq!(map_claude_event("StopFailure"), Some(HookState::Waiting));
    assert_eq!(map_claude_event("SessionEnd"), Some(HookState::Ended));
}

#[test]
fn unknown_event_carries_no_transition() {
    // Unregistered events must leave the stored record alone rather than
    // resetting it — agents emit events we do not subscribe to.
    assert_eq!(map_claude_event("PostToolUse"), None);
    assert_eq!(map_claude_event("PreCompact"), None);
    assert_eq!(map_claude_event(""), None);
}

/// The exact sequence a real `claude -p` turn emits, verified against
/// Claude Code 2.1.241.
#[test]
fn real_turn_sequence_walks_working_to_ended() {
    let states: Vec<_> = [
        "SessionStart",
        "UserPromptSubmit",
        "PreToolUse",
        "Stop",
        "SessionEnd",
    ]
    .iter()
    .map(|e| map_claude_event(e).unwrap())
    .collect();

    assert_eq!(
        states,
        vec![
            HookState::Working,
            HookState::Working,
            HookState::Working,
            HookState::Waiting,
            HookState::Ended,
        ]
    );
}

// ── persistence ─────────────────────────────────────────────────────

#[test]
fn write_then_read_round_trips() {
    let dir = TempDir::new().unwrap();
    let mut s = status(HookState::Working, 1_000);
    s.session_id = Some("abc-123".into());
    s.tool = Some("Write".into());

    write_status(dir.path(), "task1", &s).unwrap();
    let back = read_status(dir.path(), "task1", 1_000).unwrap();

    assert_eq!(back, s);
}

#[test]
fn missing_file_reads_as_none() {
    let dir = TempDir::new().unwrap();
    assert!(read_status(dir.path(), "nope", 0).is_none());
}

#[test]
fn corrupt_json_reads_as_none_without_panicking() {
    let dir = TempDir::new().unwrap();
    let path = status_path(dir.path(), "task1");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ this is not json").unwrap();

    assert!(read_status(dir.path(), "task1", 0).is_none());
}

#[test]
fn write_leaves_no_temp_file_behind() {
    let dir = TempDir::new().unwrap();
    write_status(dir.path(), "task1", &status(HookState::Working, 1)).unwrap();

    let entries: Vec<_> = std::fs::read_dir(dir.path().join(".agtx/status"))
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();

    assert_eq!(entries, vec!["task1.json"]);
}

#[test]
fn status_path_is_scoped_by_task_id() {
    // Under skip_worktree every task shares one directory, so the filename
    // is the only thing keeping their records apart.
    let a = status_path(Path::new("/wt"), "task-a");
    let b = status_path(Path::new("/wt"), "task-b");
    assert_ne!(a, b);
    assert!(a.ends_with(".agtx/status/task-a.json"));
}

// ── staleness ───────────────────────────────────────────────────────

#[test]
fn stale_working_record_is_distrusted() {
    let dir = TempDir::new().unwrap();
    write_status(dir.path(), "task1", &status(HookState::Working, 1_000)).unwrap();

    let just_inside = 1_000 + HOOK_STALE_SECS;
    let past_it = just_inside + 1;

    assert!(read_status(dir.path(), "task1", just_inside).is_some());
    assert!(
        read_status(dir.path(), "task1", past_it).is_none(),
        "a Working record older than HOOK_STALE_SECS must fall back to the pane heuristic"
    );
}

#[test]
fn only_working_decays() {
    // A blocked agent stays blocked indefinitely; so does a finished one.
    // Decaying these would silently resurrect the heuristic they replaced.
    let dir = TempDir::new().unwrap();
    let ancient = 1_000 + HOOK_STALE_SECS * 10;

    for state in [HookState::Blocked, HookState::Waiting, HookState::Ended] {
        write_status(dir.path(), "task1", &status(state, 1_000)).unwrap();
        assert!(
            read_status(dir.path(), "task1", ancient).is_some(),
            "{:?} must not decay",
            state
        );
    }
}

// ── merge rules ─────────────────────────────────────────────────────

#[test]
fn blocked_survives_an_immediate_pretooluse() {
    // Claude emits PreToolUse for the very tool it is requesting permission
    // for. Without this guard the block clears itself instantly.
    let blocked = status(HookState::Blocked, 1_000);
    let merged = merge_event(
        Some(&blocked),
        "PreToolUse",
        1_000 + BLOCKED_GUARD_SECS,
        "claude",
        None,
        None,
        None,
        Some("Bash".into()),
    );
    assert!(merged.is_none(), "Working must not clobber a fresh Blocked");
}

#[test]
fn blocked_yields_to_work_once_the_guard_expires() {
    let blocked = status(HookState::Blocked, 1_000);
    let merged = merge_event(
        Some(&blocked),
        "PreToolUse",
        1_000 + BLOCKED_GUARD_SECS + 1,
        "claude",
        None,
        None,
        None,
        None,
    );
    assert_eq!(merged.unwrap().state, HookState::Working);
}

#[test]
fn stop_does_not_clobber_a_blocked_record() {
    // Only Working is guarded — a real Stop after a permission denial is a
    // genuine transition and must land.
    let blocked = status(HookState::Blocked, 1_000);
    let merged = merge_event(
        Some(&blocked),
        "Stop",
        1_000,
        "claude",
        None,
        None,
        None,
        None,
    );
    assert_eq!(merged.unwrap().state, HookState::Waiting);
}

#[test]
fn session_identity_carries_across_events() {
    // SessionStart reports the id; a later event that omits it must not
    // erase it, or resume-by-id has nothing to resume.
    let start = merge_event(
        None,
        "SessionStart",
        1_000,
        "claude",
        Some("sess-1".into()),
        Some("/t.jsonl".into()),
        None,
        None,
    )
    .unwrap();

    let stop = merge_event(
        Some(&start),
        "Stop",
        1_010,
        "claude",
        None,
        None,
        None,
        None,
    )
    .unwrap();

    assert_eq!(stop.session_id.as_deref(), Some("sess-1"));
    assert_eq!(stop.transcript_path.as_deref(), Some("/t.jsonl"));
}

#[test]
fn blocked_reason_is_kept_and_cleared_with_the_block() {
    let blocked = merge_event(
        None,
        "PermissionRequest",
        1_000,
        "claude",
        None,
        None,
        Some("Allow Bash(rm -rf /)?".into()),
        None,
    )
    .unwrap();
    assert_eq!(blocked.message.as_deref(), Some("Allow Bash(rm -rf /)?"));

    // Once the agent moves on the stale reason must not linger on the card.
    let working = merge_event(
        Some(&blocked),
        "UserPromptSubmit",
        1_100,
        "claude",
        None,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(working.message, None);
}

#[test]
fn untrusted_message_text_is_bounded_and_stripped() {
    // Hook payloads are untrusted input that reaches the TUI; control
    // characters would corrupt the rendered card.
    let hostile = format!("bad\u{1b}[31mescape{}", "x".repeat(5_000));
    let merged = merge_event(
        None,
        "Notification",
        1,
        "claude",
        None,
        None,
        Some(hostile),
        None,
    )
    .unwrap();

    let msg = merged.message.unwrap();
    assert!(msg.chars().count() <= 512);
    assert!(!msg.contains('\u{1b}'));
}

#[test]
fn unknown_event_produces_no_record() {
    assert!(merge_event(None, "PostToolUse", 1, "claude", None, None, None, None).is_none());
}

// ── env routing (agtx hook --env) ────────────────────────────────────

/// The hook must be inert when the env vars are absent, so a future
/// user-global registration cannot hijack the user's own agent sessions.
#[test]
fn env_mode_is_a_noop_without_the_task_env() {
    std::env::remove_var("AGTX_TASK_ID");
    std::env::remove_var("AGTX_WORKTREE");
    // Returns Ok and writes nothing; a failing hook must never break a turn.
    assert!(agtx::agent::hook_status::run_hook_cli(&["--env".to_string()]).is_ok());
}

/// A backgrounded agent task inherits the pane's env, so it would otherwise
/// report status against the task that spawned it.
#[test]
fn env_mode_ignores_background_workers() {
    let dir = TempDir::new().unwrap();
    std::env::set_var("AGTX_TASK_ID", "t-bg");
    std::env::set_var("AGTX_WORKTREE", dir.path());
    std::env::set_var("CLAUDE_JOB_DIR", "/tmp/job");

    let _ = agtx::agent::hook_status::run_hook_cli(&["--env".to_string()]);

    std::env::remove_var("CLAUDE_JOB_DIR");
    std::env::remove_var("AGTX_TASK_ID");
    std::env::remove_var("AGTX_WORKTREE");
    assert!(
        read_status(dir.path(), "t-bg", 0).is_none(),
        "a background worker must not write status for its parent task"
    );
}
