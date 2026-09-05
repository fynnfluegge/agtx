//! Tests for agent-reported hook status (`src/agent/hook_status.rs`).
//!
//! Event names and payload shapes here were captured from a live
//! `claude -p` run (Claude Code 2.1.241), not assumed.

use agtx::agent::hook_status::{
    map_hook_event, read_status, status_path, write_status, AgentHookStatus, HookState,
    BLOCKED_GUARD_SECS, HOOK_STALE_SECS,
};
use agtx::agent::HookConfigKind;
use std::path::Path;
use tempfile::TempDir;

/// Claude's arm of the kind-keyed mapper.
///
/// These two shims keep every assertion below independent of the extra parameter
/// the functions take, so a change to Claude's behaviour shows up as a failing
/// assertion rather than a diff. Do not inline them.
fn map_claude_event(event: &str) -> Option<HookState> {
    map_hook_event(HookConfigKind::ClaudeSettings, event)
}

#[allow(clippy::too_many_arguments)]
fn merge_event(
    previous: Option<&AgentHookStatus>,
    event: &str,
    now: i64,
    agent: &str,
    session_id: Option<String>,
    transcript_path: Option<String>,
    message: Option<String>,
    tool: Option<String>,
) -> Option<AgentHookStatus> {
    agtx::agent::hook_status::merge_event(
        previous,
        HookConfigKind::ClaudeSettings,
        event,
        now,
        agent,
        session_id,
        transcript_path,
        message,
        tool,
    )
}

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

// ── every agent, not just Claude ─────────────────────────────────────
//
// The vocabularies below were read off each agent's binary or bundled docs at the
// versions named in `src/agent/spec.rs`. Literals on purpose, like the launch
// strings in `agent_parity_tests.rs`: a diff here means an agent's contract
// changed.

/// Guards a failure that is invisible in production: the agent accepts a
/// registration for an event nothing maps, fires it, and the board goes on
/// guessing from pane hashes. Nothing logs, nothing fails.
#[test]
fn every_registered_event_has_a_state_mapping() {
    for kind in ALL_KINDS {
        for (event, _) in agtx::agent::hook_status::hook_events(*kind) {
            assert!(
                map_hook_event(*kind, event).is_some(),
                "{:?} registers {event:?} but map_hook_event does not map it — \
                 the hook would fire and report nothing",
                kind
            );
        }
    }
}

const ALL_KINDS: &[HookConfigKind] = &[
    HookConfigKind::ClaudeSettings,
    HookConfigKind::CodexHooksJson,
    HookConfigKind::GeminiSettings,
    HookConfigKind::CursorHooksJson,
    HookConfigKind::GrokHooksJson,
    HookConfigKind::AntigravityHooksJson,
];

/// Registering an event agtx cannot act on is dead weight in the agent's turn:
/// every fire pays a process spawn to decide nothing.
#[test]
fn every_mapped_event_is_actually_registered() {
    for kind in ALL_KINDS {
        let registered: Vec<&str> = agtx::agent::hook_status::hook_events(*kind)
            .iter()
            .map(|(e, _)| *e)
            .collect();
        for event in vocabulary(*kind) {
            if map_hook_event(*kind, event).is_some() {
                assert!(
                    registered.contains(event),
                    "{:?} maps {event:?} but never registers it",
                    kind
                );
            }
        }
    }
}

/// Each agent's full published event vocabulary, mapped or not — the input to
/// the coverage checks above.
fn vocabulary(kind: HookConfigKind) -> &'static [&'static str] {
    match kind {
        // Claude Code 2.1.247.
        HookConfigKind::ClaudeSettings => &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "Notification",
            "Stop",
            "StopFailure",
            "SubagentStop",
            "PreCompact",
            "PostCompact",
            "SessionEnd",
        ],
        // codex-cli 0.144.5, from the hook schema embedded in the binary.
        HookConfigKind::CodexHooksJson => &[
            "SessionStart",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PermissionRequest",
            "PreCompact",
            "PostCompact",
            "Stop",
            "SubagentStart",
            "SubagentStop",
        ],
        // gemini-cli 0.46.0, from bundled docs/hooks/index.md.
        HookConfigKind::GeminiSettings => &[
            "SessionStart",
            "SessionEnd",
            "BeforeAgent",
            "AfterAgent",
            "BeforeModel",
            "AfterModel",
            "BeforeToolSelection",
            "BeforeTool",
            "AfterTool",
            "PreCompress",
            "Notification",
        ],
        // cursor-agent 2026.08.25.
        HookConfigKind::CursorHooksJson => &[
            "sessionStart",
            "sessionEnd",
            "beforeSubmitPrompt",
            "preToolUse",
            "postToolUse",
            "postToolUseFailure",
            "stop",
            "subagentStart",
            "subagentStop",
            "preCompact",
            "workspaceOpen",
            "beforeShellExecution",
            "beforeMCPExecution",
            "beforeReadFile",
            "afterFileEdit",
        ],
        // grok 1.0.5. Registration spelling; its payloads report the snake_case
        // form of the same names, which `squash` reconciles.
        HookConfigKind::GrokHooksJson => &[
            "SessionStart",
            "SessionEnd",
            "UserPromptSubmit",
            "PreToolUse",
            "PostToolUse",
            "PostToolUseFailure",
            "PermissionDenied",
            "Notification",
            "Stop",
            "StopFailure",
            "StopCancelled",
            "SubagentStart",
            "SubagentStop",
            "PreCompact",
            "PostCompact",
        ],
        // agy 1.1.21 — the whole vocabulary is five events. `PreToolUse` is one
        // of them and is deliberately neither mapped nor registered; see the
        // antigravity arm of `hook_events`.
        HookConfigKind::AntigravityHooksJson => &[
            "PreInvocation",
            "PostInvocation",
            "PostToolUse",
            "Stop",
        ],
    }
}

/// One agent's event name must not resolve against another's arm: `Stop` means
/// "turn over" to four agents, while Gemini's equivalent is `AfterAgent` and
/// Cursor's is lowercase `stop`. A typo matching the wrong arm would report a
/// plausible-looking wrong state.
#[test]
fn vocabularies_do_not_leak_across_agents() {
    assert_eq!(map_hook_event(HookConfigKind::GeminiSettings, "Stop"), None);
    assert_eq!(map_hook_event(HookConfigKind::CursorHooksJson, "Stop"), None);
    assert_eq!(
        map_hook_event(HookConfigKind::ClaudeSettings, "stop"),
        None,
        "Cursor's lowercase stop must not resolve against Claude"
    );
    assert_eq!(
        map_hook_event(HookConfigKind::ClaudeSettings, "AfterAgent"),
        None
    );
    assert_eq!(
        map_hook_event(HookConfigKind::AntigravityHooksJson, "SessionEnd"),
        None,
        "antigravity has no session lifecycle event at all"
    );
}

/// The two states that drive a feature rather than an icon: `Blocked` fires the
/// orchestrator's immediate stuck-task notification, and `Waiting` is what makes
/// a finished turn distinguishable from a stalled one.
#[test]
fn turn_end_is_expressible_for_every_agent() {
    for kind in ALL_KINDS {
        let ends: Vec<&str> = agtx::agent::hook_status::hook_events(*kind)
            .iter()
            .map(|(e, _)| *e)
            .filter(|e| map_hook_event(*kind, e) == Some(HookState::Waiting))
            .collect();
        assert!(
            !ends.is_empty(),
            "{:?} can never report Waiting — every turn would look like work in \
             progress until the staleness timer expired",
            kind
        );
    }
}

/// Cursor and Antigravity have no human-blocking event to subscribe to. Pinned
/// so the gap is not closed later with a wrong mapping.
#[test]
fn only_agents_with_a_permission_event_report_blocked() {
    let can_block: Vec<HookConfigKind> = ALL_KINDS
        .iter()
        .copied()
        .filter(|k| {
            agtx::agent::hook_status::hook_events(*k)
                .iter()
                .any(|(e, _)| map_hook_event(*k, e) == Some(HookState::Blocked))
        })
        .collect();
    assert_eq!(
        can_block,
        vec![
            HookConfigKind::ClaudeSettings,
            HookConfigKind::CodexHooksJson,
            HookConfigKind::GeminiSettings,
            HookConfigKind::GrokHooksJson,
        ],
        "Cursor exposes only policy gates that decide (beforeShellExecution), and \
         Antigravity's five events contain no permission prompt"
    );
}

/// Antigravity's payload is protojson camelCase with no event name and no
/// `session_id`: the event arrives in argv, and the identity fields resolve by
/// alias or not at all.
#[test]
fn antigravity_payload_round_trips_through_the_cli() {
    let dir = TempDir::new().unwrap();
    std::env::set_var("AGTX_TASK_ID", "t-agy");
    std::env::set_var("AGTX_WORKTREE", dir.path());

    // A real `PostToolUse` payload: protojson camelCase, `stepIdx`, and no
    // `hook_event_name` anywhere in it.
    let payload = serde_json::json!({
        "conversationId": "ec33ebf9-0cba-4100-8142-c61503f6c587",
        "workspacePaths": ["/tmp/wt"],
        "transcriptPath": "/tmp/wt/.gemini/antigravity-cli/transcript.jsonl",
        "artifactDirectoryPath": "/tmp/wt/.gemini/antigravity-cli/artifacts",
        "modelName": "auto",
        "stepIdx": 19,
    })
    .to_string();

    agtx::agent::hook_status::record_hook_event(
        "t-agy",
        &dir.path().to_string_lossy(),
        "antigravity",
        Some("PostToolUse"),
        payload.as_bytes(),
    );

    let got = read_status(dir.path(), "t-agy", 0).expect("argv event must be honoured");
    std::env::remove_var("AGTX_TASK_ID");
    std::env::remove_var("AGTX_WORKTREE");

    assert_eq!(got.state, HookState::Working);
    assert_eq!(got.agent, "antigravity");
    assert_eq!(
        got.session_id.as_deref(),
        Some("ec33ebf9-0cba-4100-8142-c61503f6c587"),
        "conversationId is antigravity's session id"
    );
    assert_eq!(
        got.transcript_path.as_deref(),
        Some("/tmp/wt/.gemini/antigravity-cli/transcript.jsonl")
    );
}

/// An agent with `hook_config: None` writes nothing even when the CLI is invoked
/// in its name, so a registration left behind by an older agtx cannot keep
/// reporting under a vocabulary that no longer exists.
#[test]
fn an_agent_without_hooks_writes_nothing() {
    let dir = TempDir::new().unwrap();
    std::env::set_var("AGTX_TASK_ID", "t-oc");
    std::env::set_var("AGTX_WORKTREE", dir.path());

    let wt = dir.path().to_string_lossy().to_string();
    let payload = br#"{"hook_event_name":"SessionStart"}"#;

    agtx::agent::hook_status::record_hook_event("t-oc", &wt, "opencode", None, payload);
    let a = read_status(dir.path(), "t-oc", 0);

    agtx::agent::hook_status::record_hook_event("t-oc", &wt, "not-an-agent", None, payload);
    let b = read_status(dir.path(), "t-oc", 0);

    std::env::remove_var("AGTX_TASK_ID");
    std::env::remove_var("AGTX_WORKTREE");
    assert!(a.is_none(), "opencode has no hook support");
    assert!(b.is_none(), "an unknown agent must not borrow a vocabulary");
}

/// Grok registers `PreToolUse` and reports `pre_tool_use`. Matching only the
/// registered spelling is a silent no-op: the hook fires, the payload parses, and
/// no state is written.
#[test]
fn grok_answers_to_both_of_its_spellings() {
    for (registered, reported) in [
        ("SessionStart", "session_start"),
        ("UserPromptSubmit", "user_prompt_submit"),
        ("PreToolUse", "pre_tool_use"),
        ("Stop", "stop"),
        ("StopCancelled", "stop_cancelled"),
        ("SessionEnd", "session_end"),
    ] {
        let a = map_hook_event(HookConfigKind::GrokHooksJson, registered);
        let b = map_hook_event(HookConfigKind::GrokHooksJson, reported);
        assert!(a.is_some(), "{registered} unmapped");
        assert_eq!(a, b, "{registered} and {reported} are the same event");
    }
}

/// A status hook must not decide whether a tool runs. Antigravity's `PreToolUse`
/// makes `decision` required and reads anything else as a refusal, so subscribing
/// blocks every tool call — and the only safe subscription would answer
/// `"allow"`, a permission decision a liveness reporter has no business making.
#[test]
fn antigravity_never_subscribes_to_its_gating_hook() {
    let registered: Vec<&str> =
        agtx::agent::hook_status::hook_events(HookConfigKind::AntigravityHooksJson)
            .iter()
            .map(|(e, _)| *e)
            .collect();
    assert!(
        !registered.contains(&"PreToolUse"),
        "antigravity's PreToolUse requires a decision field; subscribing blocks every tool call"
    );
    assert_eq!(
        map_hook_event(HookConfigKind::AntigravityHooksJson, "PreToolUse"),
        None
    );
    assert_eq!(
        map_hook_event(HookConfigKind::AntigravityHooksJson, "PostToolUse"),
        Some(HookState::Working),
        "PostToolUse carries the heartbeat instead, and its contract wants the empty \
         JSON object this hook already prints"
    );
}

/// Grok scans `.claude/settings*.json` and `.cursor/hooks.json` for vendor
/// compatibility, so in a worktree configured for several phase agents it fires
/// agtx's Claude- and cursor-registered hooks with *its own* payloads.
///
/// Claude's arm rejects them outright. Cursor's does not: it is lowercase and
/// contains `stop`, which is exactly what grok reports. That is a real overlap,
/// pinned here rather than assumed away — the state agrees (both mean the turn
/// ended), so the cost is a record carrying a neighbouring agent's name, and
/// nothing reads that name.
#[test]
fn a_grok_payload_reaching_a_neighbours_registration() {
    for event in ["session_start", "user_prompt_submit", "pre_tool_use"] {
        assert_eq!(
            map_hook_event(HookConfigKind::ClaudeSettings, event),
            None,
            "grok's {event} must not resolve against Claude's PascalCase arm"
        );
        assert_eq!(map_hook_event(HookConfigKind::CursorHooksJson, event), None);
    }
    // The one that does overlap, and agrees.
    assert_eq!(
        map_hook_event(HookConfigKind::CursorHooksJson, "stop"),
        map_hook_event(HookConfigKind::GrokHooksJson, "stop"),
        "grok's `stop` resolves against cursor's arm; it must at least mean the same"
    );
    assert_eq!(map_hook_event(HookConfigKind::ClaudeSettings, "stop"), None);
}
