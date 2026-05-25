# Superpowers Codex Support Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Enable the `superpowers` workflow plugin for Codex CLI tasks by adding per-agent init_scripts support to agtx and updating the superpowers plugin.toml.

**Architecture:** Add `init_scripts: HashMap<String, String>` to `WorkflowPlugin` so plugins can specify agent-specific setup scripts. At the existing plugin-init execution point in `setup_task_worktree`, prefer the agent-specific plugin script over the legacy plugin `init_script` fallback, while continuing to run any project `init_script` independently. Update `plugins/superpowers/plugin.toml` to deploy Superpowers skills to Codex-native paths and send explicit phase commands.

**Implementation correction:** The original Tasks 2-4 below selected an agent-specific plugin script in the project-init argument passed to `initialize_worktree`. That would replace a configured project init script instead of running both existing setup layers. Those instructions are superseded: retain project init selection at the three call sites and perform per-agent selection inside the plugin-init block in `setup_task_worktree`.

**Tech Stack:** Rust, TOML (serde), Ratatui TUI, SQLite

---

## File Map

| File | Change |
|---|---|
| `src/config/mod.rs` | Add `init_scripts` field to `WorkflowPlugin` struct |
| `src/tui/app.rs` | Select agent-specific plugin setup in `setup_task_worktree`, preserving project setup |
| `plugins/superpowers/plugin.toml` | Update supported_agents, replace init_script with [init_scripts], add [commands] |
| `tests/config_tests.rs` | Add deserialization test for [init_scripts] |
| `src/tui/app_tests.rs` | Add regression coverage for project + agent-specific plugin initialization |

---

### Task 1: Add `init_scripts` field to `WorkflowPlugin`

**Files:**
- Modify: `src/config/mod.rs` (around line 432 — after `init_script` field)
- Modify: `tests/config_tests.rs` (end of file)

- [ ] **Step 1: Write failing test for [init_scripts] deserialization**

Add to the end of `tests/config_tests.rs`:

```rust
#[test]
fn test_workflow_plugin_init_scripts_deserialization() {
    let toml = r#"
name = "myplugin"

[init_scripts]
claude = "claude plugin install foo --scope local"
codex = "cp ~/.cache/skills/* .codex/skills/"
"#;
    let plugin: WorkflowPlugin = toml::from_str(toml).unwrap();
    assert_eq!(
        plugin.init_scripts.get("claude").map(|s| s.as_str()),
        Some("claude plugin install foo --scope local")
    );
    assert_eq!(
        plugin.init_scripts.get("codex").map(|s| s.as_str()),
        Some("cp ~/.cache/skills/* .codex/skills/")
    );
    assert!(plugin.init_scripts.get("gemini").is_none());
}

#[test]
fn test_workflow_plugin_init_scripts_empty_when_absent() {
    let toml = r#"
name = "myplugin"
init_script = "echo hello"
"#;
    let plugin: WorkflowPlugin = toml::from_str(toml).unwrap();
    assert!(plugin.init_scripts.is_empty());
    assert_eq!(plugin.init_script, Some("echo hello".to_string()));
}
```

- [ ] **Step 2: Run tests to confirm they fail**

```bash
cd /home/fullmetal/projects/agtx && cargo test test_workflow_plugin_init_scripts 2>&1 | tail -20
```

Expected: compile error — field `init_scripts` does not exist on `WorkflowPlugin`

- [ ] **Step 3: Add `init_scripts` field to `WorkflowPlugin` in `src/config/mod.rs`**

Find the `init_script` field (line ~432):
```rust
    pub init_script: Option<String>,
```

Add the new field immediately after it:
```rust
    pub init_script: Option<String>,
    /// Per-agent init scripts. Keys are agent names (e.g. "claude", "codex").
    /// When present, the agent-specific script takes precedence over `init_script`.
    #[serde(default)]
    pub init_scripts: std::collections::HashMap<String, String>,
```

- [ ] **Step 4: Run tests to verify they pass**

```bash
cd /home/fullmetal/projects/agtx && cargo test test_workflow_plugin_init_scripts 2>&1 | tail -10
```

Expected: both tests PASS

- [ ] **Step 5: Run full test suite to verify no regressions**

```bash
cd /home/fullmetal/projects/agtx && cargo test --quiet 2>&1 | tail -10
```

Expected: all tests pass, 0 failures

- [ ] **Step 6: Commit**

```bash
cd /home/fullmetal/projects/agtx && git add src/config/mod.rs tests/config_tests.rs && git commit -m "feat: add init_scripts per-agent table to WorkflowPlugin"
```

---

### Task 2: Update init_script selection at the planning phase site

**Files:**
- Modify: `src/tui/app.rs` (~line 4598)

- [ ] **Step 1: Locate the exact line**

```bash
grep -n 'let init_script = if self.state.flags.no_init_scripts' /home/fullmetal/projects/agtx/src/tui/app.rs
```

Expected: 3 line numbers printed. Note the **first** one (planning phase — appears after `planning_agent` and `resolve_skill_command` usages).

- [ ] **Step 2: Replace the init_script selection block at site 1**

Find this exact block (at the first occurrence, planning phase):
```rust
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.init_script.clone()
        };
```

Replace with:
```rust
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            plugin.as_ref()
                .and_then(|p| p.init_scripts.get(&planning_agent).cloned())
                .or_else(|| self.state.config.init_script.clone())
        };
```

- [ ] **Step 3: Build to verify it compiles**

```bash
cd /home/fullmetal/projects/agtx && cargo build 2>&1 | grep -E '^error' | head -20
```

Expected: no error lines

- [ ] **Step 4: Run tests**

```bash
cd /home/fullmetal/projects/agtx && cargo test --quiet 2>&1 | tail -10
```

Expected: all tests pass

- [ ] **Step 5: Commit**

```bash
cd /home/fullmetal/projects/agtx && git add src/tui/app.rs && git commit -m "feat: prefer per-agent init_scripts at planning phase worktree setup"
```

---

### Task 3: Update init_script selection at the research phase site

**Files:**
- Modify: `src/tui/app.rs` (~line 4967 — second occurrence)

- [ ] **Step 1: Locate site 2**

```bash
grep -n 'let init_script = if self.state.flags.no_init_scripts' /home/fullmetal/projects/agtx/src/tui/app.rs
```

Note the **second** line number (research phase — appears after `let agent_name = self.state.config.agent_for_phase("research").to_string()`).

- [ ] **Step 2: Replace the init_script selection block at site 2**

Find:
```rust
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.init_script.clone()
        };
```

(at the second occurrence, which is followed by `let skip_init_scripts = self.state.flags.no_init_scripts;` and then `let tmux_ops = Arc::clone(&self.state.tmux_ops);` with no `planning_agent` binding nearby)

Replace with:
```rust
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            plugin.as_ref()
                .and_then(|p| p.init_scripts.get(&agent_name).cloned())
                .or_else(|| self.state.config.init_script.clone())
        };
```

- [ ] **Step 3: Build and test**

```bash
cd /home/fullmetal/projects/agtx && cargo build 2>&1 | grep -E '^error' | head -20 && cargo test --quiet 2>&1 | tail -10
```

Expected: no errors, all tests pass

- [ ] **Step 4: Commit**

```bash
cd /home/fullmetal/projects/agtx && git add src/tui/app.rs && git commit -m "feat: prefer per-agent init_scripts at research phase worktree setup"
```

---

### Task 4: Update init_script selection at the running phase site

**Files:**
- Modify: `src/tui/app.rs` (~line 5230 — third occurrence)

- [ ] **Step 1: Locate site 3**

```bash
grep -n 'let init_script = if self.state.flags.no_init_scripts' /home/fullmetal/projects/agtx/src/tui/app.rs
```

Note the **third** line number (running phase — the remaining occurrence after Tasks 2 and 3 are done; `running_agent` is the agent variable, visible in the nearby `self.state.agent_registry.get(&running_agent)` call).

- [ ] **Step 2: Replace the init_script selection block at site 3**

Find:
```rust
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            self.state.config.init_script.clone()
        };
```

Replace with:
```rust
        let init_script = if self.state.flags.no_init_scripts {
            None
        } else {
            plugin.as_ref()
                .and_then(|p| p.init_scripts.get(&running_agent).cloned())
                .or_else(|| self.state.config.init_script.clone())
        };
```

- [ ] **Step 3: Build and test**

```bash
cd /home/fullmetal/projects/agtx && cargo build 2>&1 | grep -E '^error' | head -20 && cargo test --quiet 2>&1 | tail -10
```

Expected: no errors, all tests pass

- [ ] **Step 4: Commit**

```bash
cd /home/fullmetal/projects/agtx && git add src/tui/app.rs && git commit -m "feat: prefer per-agent init_scripts at running phase worktree setup"
```

---

### Task 5: Update superpowers/plugin.toml for Codex

**Files:**
- Modify: `plugins/superpowers/plugin.toml`

- [ ] **Step 1: Replace the entire plugin.toml content**

Write `plugins/superpowers/plugin.toml` with this exact content:

```toml
name = "superpowers"
description = "Superpowers by @obra - brainstorming, plans, TDD, subagent-driven development"
supported_agents = ["claude", "codex"]

[init_scripts]
claude = "claude plugin install superpowers@claude-plugins-official --scope local"
codex = '''
CACHE_BASE="$HOME/.claude/plugins/cache/superpowers-marketplace/superpowers"
LATEST_VER=$(ls "$CACHE_BASE" | sort -V | tail -1)
SKILLS_SRC="$CACHE_BASE/$LATEST_VER/skills"
for skill_dir in "$SKILLS_SRC"/*/; do
  skill_name=$(basename "$skill_dir")
  mkdir -p ".codex/skills/superpowers-$skill_name"
  cp "$skill_dir/SKILL.md" ".codex/skills/superpowers-$skill_name/SKILL.md"
done
'''

[artifacts]
planning = "docs/superpowers/plans/*.md"

[commands]
planning = "/superpowers:brainstorming"
running = "/superpowers:executing-plans"
review = "/superpowers:requesting-code-review"

[prompts]
planning = "You are already in an isolated git worktree managed by agtx. Do NOT create additional worktrees or branches. Work directly in this directory.\n\nUse the brainstorming skill to explore and design this task. When the design is approved, use the writing-plans skill to produce an implementation plan.\n\nTask: {task}"
running = "Use the executing-plans skill (or subagent-driven-development if subagents are available) to implement the plan in docs/superpowers/plans/."
review = "Use the requesting-code-review skill to review the completed work."

[copy_back]
planning = ["docs/superpowers"]
```

Key changes from the original:
- `supported_agents`: added `"codex"`
- Removed top-level `init_script = "..."` 
- Added `[init_scripts]` table with `claude` and `codex` entries
- The Codex script uses TOML literal multiline strings (`'''...'''`) — no escape sequences
- Added `[commands]` section with per-phase skill invocations

- [ ] **Step 2: Verify the file parses as valid TOML**

```bash
cd /home/fullmetal/projects/agtx && cargo build 2>&1 | grep -E '^error' | head -20
```

Expected: no errors (the `include_str!` in `src/skills.rs` would fail at compile time if the TOML is malformed)

- [ ] **Step 3: Verify the bundled plugin loads correctly**

```bash
cd /home/fullmetal/projects/agtx && cargo test --quiet 2>&1 | tail -10
```

Expected: all tests pass

- [ ] **Step 4: Smoke-check the new plugin.toml parses with the right fields**

Add a temporary test (or just run this inline check):

```bash
cd /home/fullmetal/projects/agtx && cargo run --example check_plugin 2>/dev/null || \
  cargo test -q --test config_tests 2>&1 | tail -5
```

Expected: config tests still pass

- [ ] **Step 5: Commit**

```bash
cd /home/fullmetal/projects/agtx && git add plugins/superpowers/plugin.toml && git commit -m "feat: enable Superpowers for Codex — add init_scripts and phase commands"
```

---

### Task 6: Final build verification

- [ ] **Step 1: Full clean build**

```bash
cd /home/fullmetal/projects/agtx && cargo build --release 2>&1 | grep -E '^error' | head -20
```

Expected: no errors

- [ ] **Step 2: Full test suite**

```bash
cd /home/fullmetal/projects/agtx && cargo test 2>&1 | tail -15
```

Expected: all tests pass, 0 failures

- [ ] **Step 3: Verify command translation for Codex**

The existing `transform_plugin_command` function in `src/skills.rs` is unchanged, but verify it handles the new commands correctly:

```bash
cd /home/fullmetal/projects/agtx && grep -A 20 'fn transform_plugin_command' src/skills.rs | head -25
```

Verify the codex arm transforms `/superpowers:brainstorming` → `$superpowers-brainstorming` (slash→dollar, colon→hyphen).

- [ ] **Step 4: Verify `supported_agents` check gates Codex correctly**

The `supports_agent` method in `WorkflowPlugin` already handles this correctly (`supported_agents.is_empty() || supported_agents.contains(agent)`). No change needed; Codex is now in the list.

- [ ] **Step 5: Final commit if needed**

If any cleanup was needed:
```bash
cd /home/fullmetal/projects/agtx && git add -p && git commit -m "chore: final cleanup for Superpowers Codex support"
```
