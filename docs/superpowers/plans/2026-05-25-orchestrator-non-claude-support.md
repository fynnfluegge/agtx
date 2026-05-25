# Orchestrator Non-Claude Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make orchestrator MCP registration/cleanup work for supported non-Claude agents while keeping `default_agent` as the orchestrator selector.

**Architecture:** Keep TUI orchestration flow unchanged and implement agent-specific orchestrator launch behavior in `AgentOperations::build_orchestrator_command()`. Add focused tests at the agent-operations layer for command composition and one TUI regression to ensure non-Claude default agent still routes through orchestrator command creation.

**Tech Stack:** Rust, cargo test harness, mockall (`test-mocks` feature), tmux abstractions, existing agent registry/operations.

---

## File Structure Map

- Modify: `src/agent/operations.rs`
  - Add non-Claude match arms in `build_orchestrator_command()` for MCP register/launch/cleanup.
- Modify: `src/agent/operations_tests.rs` (or nearest existing tests module for `operations.rs`)
  - Add/extend unit tests for orchestrator command construction by agent.
- Modify: `src/tui/app_tests.rs`
  - Add one regression test asserting non-Claude `default_agent` orchestrator toggle path uses returned orchestrator command and still spawns/open popup as expected.

If `src/agent/operations_tests.rs` does not exist, place tests in the current test location used for `src/agent/operations.rs` (likely inline `#[cfg(test)]` module in `operations.rs`) and update paths accordingly.

### Task 1: Add Failing Tests for Orchestrator Command Composition

**Files:**
- Modify: `src/agent/operations_tests.rs` or `src/agent/operations.rs` test module
- Test: same file/module

- [ ] **Step 1: Write failing tests for non-Claude orchestrator command wrappers**

```rust
#[test]
fn test_build_orchestrator_command_codex_includes_mcp_lifecycle() {
    let agent = Agent::new("codex", "codex", "OpenAI's Codex CLI", "Codex <noreply@openai.com>");
    let ops = CodingAgent::new(agent);
    let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

    assert!(cmd.contains("mcp"), "expected MCP command in orchestrator wrapper");
    assert!(cmd.contains("remove") && cmd.contains("agtx"));
    assert!(cmd.contains("add") || cmd.contains("add-json"));
    assert!(cmd.contains("codex --dangerously-bypass-approvals-and-sandbox"));
}

#[test]
fn test_build_orchestrator_command_gemini_includes_mcp_lifecycle() {
    let agent = Agent::new("gemini", "gemini", "Google Gemini CLI", "Gemini <noreply@google.com>");
    let ops = CodingAgent::new(agent);
    let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

    assert!(cmd.contains("mcp"));
    assert!(cmd.contains("remove") && cmd.contains("agtx"));
    assert!(cmd.contains("add") || cmd.contains("add-json"));
    assert!(cmd.contains("gemini --approval-mode yolo"));
}

#[test]
fn test_build_orchestrator_command_unknown_agent_falls_back_to_interactive() {
    let agent = Agent::new("custom", "custom-agent", "Custom", "Custom <noreply@example.com>");
    let ops = CodingAgent::new(agent);
    let cmd = ops.build_orchestrator_command("{}", "agtx");

    assert_eq!(cmd, "custom-agent");
}
```

- [ ] **Step 2: Add analogous tests for `opencode`, `cursor`, and `copilot`**

```rust
#[test]
fn test_build_orchestrator_command_opencode_includes_mcp_lifecycle() {
    let agent = Agent::new("opencode", "opencode", "AI-powered coding assistant", "OpenCode <noreply@opencode.ai>");
    let ops = CodingAgent::new(agent);
    let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

    assert!(cmd.contains("mcp"));
    assert!(cmd.contains("remove") && cmd.contains("agtx"));
    assert!(cmd.contains("add") || cmd.contains("add-json"));
    assert!(cmd.contains("opencode"));
}

#[test]
fn test_build_orchestrator_command_cursor_includes_mcp_lifecycle() {
    let agent = Agent::new("cursor", "agent", "Cursor Agent CLI", "Cursor Agent <noreply@cursor.com>");
    let ops = CodingAgent::new(agent);
    let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

    assert!(cmd.contains("mcp"));
    assert!(cmd.contains("remove") && cmd.contains("agtx"));
    assert!(cmd.contains("add") || cmd.contains("add-json"));
    assert!(cmd.contains("agent --yolo"));
}

#[test]
fn test_build_orchestrator_command_copilot_includes_mcp_lifecycle() {
    let agent = Agent::new("copilot", "copilot", "GitHub Copilot CLI", "GitHub Copilot <noreply@github.com>");
    let ops = CodingAgent::new(agent);
    let cmd = ops.build_orchestrator_command("{\\\"type\\\":\\\"stdio\\\"}", "agtx");

    assert!(cmd.contains("mcp"));
    assert!(cmd.contains("remove") && cmd.contains("agtx"));
    assert!(cmd.contains("add") || cmd.contains("add-json"));
    assert!(cmd.contains("copilot --allow-all-tools"));
}
```

- [ ] **Step 3: Run targeted test command to verify RED**

Run:
```bash
rtk cargo test --features test-mocks build_orchestrator_command -- --nocapture
```

Expected:
- FAIL for new non-Claude tests because current implementation falls back to plain interactive command.

- [ ] **Step 4: Commit failing tests (optional if team requires strict red commit)**

```bash
rtk git add src/agent/operations.rs src/agent/operations_tests.rs src/tui/app_tests.rs
rtk git commit -m "test: cover orchestrator MCP command lifecycle for non-claude agents"
```

If your team does not commit red-state checkpoints, skip commit and continue directly to Task 2.

### Task 2: Implement Non-Claude Orchestrator Command Lifecycle

**Files:**
- Modify: `src/agent/operations.rs`
- Test: `src/agent/operations_tests.rs` or inline tests

- [ ] **Step 1: Implement agent-specific orchestrator match arms**

```rust
fn build_orchestrator_command(&self, mcp_json: &str, _agtx_bin: &str) -> String {
    match self.agent.name.as_str() {
        "claude" => format!(
            "claude mcp remove agtx --scope local 2>/dev/null || true; \
             claude mcp add-json agtx '{}' --scope local && {}; \
             claude mcp remove agtx --scope local",
            mcp_json,
            self.build_interactive_command("")
        ),
        "codex" => format!(
            "codex mcp remove agtx --scope local 2>/dev/null || true; \
             codex mcp add-json agtx '{}' --scope local && {}; \
             codex mcp remove agtx --scope local",
            mcp_json,
            self.build_interactive_command("")
        ),
        "gemini" => format!(
            "gemini mcp remove agtx --scope local 2>/dev/null || true; \
             gemini mcp add-json agtx '{}' --scope local && {}; \
             gemini mcp remove agtx --scope local",
            mcp_json,
            self.build_interactive_command("")
        ),
        "opencode" => format!(
            "opencode mcp remove agtx --scope local 2>/dev/null || true; \
             opencode mcp add-json agtx '{}' --scope local && {}; \
             opencode mcp remove agtx --scope local",
            mcp_json,
            self.build_interactive_command("")
        ),
        "cursor" => format!(
            "agent mcp remove agtx --scope local 2>/dev/null || true; \
             agent mcp add-json agtx '{}' --scope local && {}; \
             agent mcp remove agtx --scope local",
            mcp_json,
            self.build_interactive_command("")
        ),
        "copilot" => format!(
            "copilot mcp remove agtx --scope local 2>/dev/null || true; \
             copilot mcp add-json agtx '{}' --scope local && {}; \
             copilot mcp remove agtx --scope local",
            mcp_json,
            self.build_interactive_command("")
        ),
        _ => self.build_interactive_command(""),
    }
}
```

Adjust exact MCP subcommands per CLI support discovered in current code/tests (e.g., if an agent uses `mcp add` instead of `add-json`). Keep the 4-stage lifecycle invariant unchanged.

- [ ] **Step 2: Keep fallback and Claude semantics unchanged**

Verify in code review:
- Claude arm remains functionally identical.
- Unknown agents still return `build_interactive_command("")`.
- No behavior changes in `generate_text`, resume, or interactive command builders.

- [ ] **Step 3: Run targeted test command to verify GREEN**

Run:
```bash
rtk cargo test --features test-mocks build_orchestrator_command -- --nocapture
```

Expected:
- PASS for all new and existing `build_orchestrator_command` tests.

- [ ] **Step 4: Commit implementation**

```bash
rtk git add src/agent/operations.rs src/agent/operations_tests.rs
rtk git commit -m "feat: add orchestrator MCP lifecycle for non-claude agents"
```

### Task 3: TUI Regression Coverage for Non-Claude Default Agent Path

**Files:**
- Modify: `src/tui/app_tests.rs`
- Test: `src/tui/app_tests.rs`

- [ ] **Step 1: Add test that toggling orchestrator with non-Claude default agent still spawns window using orchestrator command**

```rust
#[test]
fn test_toggle_orchestrator_spawns_new_session_for_non_claude_default_agent() {
    // Arrange app with default_agent="codex"
    // Mock agent registry get("codex")
    // Mock build_orchestrator_command(...) -> returns sentinel command
    // Expect tmux create_window called with sentinel command
    // Act: app.toggle_orchestrator()
    // Assert: orchestrator_session is set and popup is opened
}
```

Implementation details in test:
- Reuse existing `test_toggle_orchestrator_spawns_new_session` scaffolding.
- Set merged config default agent to `codex`.
- Assert `expect_get().with(eq("codex"))` is hit.
- Assert `create_window(..., Some("<sentinel>") ...)` uses sentinel returned by mocked `build_orchestrator_command`.

- [ ] **Step 2: Run targeted toggle tests**

Run:
```bash
rtk cargo test --features test-mocks toggle_orchestrator -- --nocapture
```

Expected:
- PASS including new non-Claude regression test.

- [ ] **Step 3: Commit test updates**

```bash
rtk git add src/tui/app_tests.rs
rtk git commit -m "test: cover orchestrator toggle path with non-claude default agent"
```

### Task 4: Full Verification and Final Cleanup

**Files:**
- Modify: none (unless fixes needed)
- Test: repository-wide

- [ ] **Step 1: Run full required test suite**

Run:
```bash
rtk cargo test --features test-mocks
```

Expected:
- PASS all tests.

- [ ] **Step 2: Optional lint/build smoke check**

Run:
```bash
rtk cargo build --release
```

Expected:
- Successful build.

- [ ] **Step 3: If failures occur, fix in smallest possible scope and re-run**

Failure loop:
```bash
rtk cargo test --features test-mocks <failing_test_name> -- --nocapture
# apply minimal fix
rtk cargo test --features test-mocks
```

Expected:
- All regressions resolved with no unrelated refactors.

- [ ] **Step 4: Final commit (if any verification fixes were needed)**

```bash
rtk git add src/agent/operations.rs src/tui/app_tests.rs src/agent/operations_tests.rs
rtk git commit -m "fix: resolve orchestrator non-claude verification regressions"
```

## Self-Review Checklist

- Spec coverage:
  - `default_agent` remains orchestrator selector: covered in Tasks 2 and 3.
  - Non-Claude MCP lifecycle support: covered in Tasks 1 and 2.
  - Backward-compatible fallback for unknown agents: covered in Task 1 tests + Task 2 implementation.
  - No TUI flow redesign: covered by minimal-change approach and Task 3 regression.
- Placeholder scan:
  - No `TBD`, `TODO`, or vague “handle appropriately” directives remain.
- Type/signature consistency:
  - Uses existing `build_orchestrator_command(&self, mcp_json: &str, agtx_bin: &str) -> String` contract and current `toggle_orchestrator()` flow.
