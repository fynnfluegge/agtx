# Orchestrator Non-Claude Support Design

## Summary

Fix orchestrator startup so it works with whatever agent is selected by `default_agent`, not only Claude. The current TUI path already selects `default_agent`, but orchestrator MCP registration/cleanup logic is only implemented for Claude in `build_orchestrator_command()`. This design adds equivalent orchestrator command wrappers for other supported agents while keeping existing selection/config behavior unchanged.

## Problem Statement

Today:
- `toggle_orchestrator()` selects `self.state.config.default_agent`.
- It builds project-scoped MCP JSON and asks the selected agent to build an orchestrator command.
- In `src/agent/operations.rs`, only the `"claude"` arm performs MCP register/launch/cleanup.
- Non-Claude agents fall back to plain interactive launch, so orchestrator runs without MCP and cannot control the board as intended.

Result: orchestrator is effectively Claude-only despite multi-agent support elsewhere.

## Goals

- Make orchestrator MCP lifecycle work for non-Claude default agents.
- Preserve existing orchestrator selection model: `default_agent` is authoritative.
- Keep integration points and user flow unchanged in TUI.
- Maintain backward compatibility for unknown/custom agents.

## Non-Goals

- Adding a separate `orchestrator_agent` config key.
- Refactoring agent metadata schema or plugin format.
- Changing orchestrator UX, keybindings, or notification flow.

## Architecture

- Keep TUI ownership of agent selection (`default_agent`).
- Keep `AgentOperations` ownership of launch command construction.
- Extend `build_orchestrator_command(mcp_json, agtx_bin)` match arms for supported non-Claude agents so each can:
  1. pre-remove stale `agtx` MCP registration,
  2. register project-scoped MCP server,
  3. launch interactive agent,
  4. remove MCP registration on exit.

Unknown agents continue using default fallback `build_interactive_command("")`.

## Component-Level Changes

### `src/agent/operations.rs`

Primary change location.

- Update `CodingAgent::build_orchestrator_command()`:
  - Keep existing Claude behavior unchanged.
  - Add arms for supported agents (`codex`, `gemini`, `opencode`, `cursor`, `copilot`) using each CLI’s MCP command conventions.
  - Preserve fallback behavior for unrecognized agent names.

### `src/tui/app.rs`

No structural changes expected.

- `toggle_orchestrator()` already:
  - picks `default_agent`,
  - builds MCP JSON,
  - invokes `build_orchestrator_command()`,
  - creates orchestrator tmux window,
  - deploys `agtx-orchestrate`,
  - sends orchestration command after readiness.

This remains the correct integration sequence.

### Tests

- Add/update tests covering orchestrator command composition in agent operations and orchestrator spawn path expectations in TUI tests.

## Runtime Data Flow

1. User presses `O`.
2. App resolves project mode and `default_agent`.
3. App constructs project-scoped MCP JSON: `agtx mcp-serve <project_path>`.
4. App asks selected agent ops for orchestrator command.
5. Agent-specific command performs register -> launch -> cleanup lifecycle.
6. TUI starts/attaches orchestrator window and sends `/agtx:orchestrate` when ready.

Invariant: orchestrator always has MCP configured for the selected `default_agent` when supported.

## Error Handling and Compatibility

- Registration failure blocks launch (fail fast), consistent with current Claude semantics.
- Pre-clean stale registration remains best-effort.
- Post-exit cleanup remains best-effort and should not destabilize TUI flow.
- Unknown/custom agents keep plain interactive fallback to avoid hard failures.
- No migration or config changes required.

## Testing Strategy

- Unit tests for `build_orchestrator_command()` per supported agent:
  - assert stale remove step exists,
  - assert MCP add step exists and targets `agtx`,
  - assert interactive launch is present,
  - assert cleanup remove step exists.
- Fallback test for unknown agent returns plain interactive launch.
- TUI regression check: non-Claude `default_agent` path still reaches orchestrator window creation using returned command.
- Run full test suite per repo rule:
  - `cargo test --features test-mocks`

## Risks and Mitigations

- Risk: MCP subcommand syntax differences across CLIs.
  - Mitigation: explicit per-agent match arms and targeted tests on generated command strings.
- Risk: shell quoting errors for embedded JSON.
  - Mitigation: keep existing JSON escaping pipeline unchanged; only consume `mcp_json` as pre-escaped input.
- Risk: partial support across installed agent versions.
  - Mitigation: unknown/unsupported behavior remains non-panicking fallback.

## Acceptance Criteria

- With `default_agent=claude`, orchestrator behavior remains unchanged.
- With `default_agent` set to a supported non-Claude agent, orchestrator starts and can use MCP tools.
- No regressions in orchestrator toggle, popup opening, readiness wait, or notification delivery flow.
- Test suite passes with `--features test-mocks`.
