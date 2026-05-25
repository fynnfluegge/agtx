# Superpowers Codex Support

Enable the `superpowers` workflow plugin for Codex CLI tasks in agtx.

## Motivation

`plugins/superpowers/plugin.toml` currently has `supported_agents = ["claude"]` and an init_script that runs `claude plugin install`, making Superpowers unavailable when Codex is the active agent. Superpowers skills need to be deployed to Codex-native paths (`.codex/skills/superpowers-{name}/SKILL.md`) and phases need explicit skill invocations, since Codex does not have Claude's session-start auto-loading hook.

## Constraints

- No silent failures: init scripts must fail loudly so errors surface to the user
- No per-agent conditional logic in a shared init_script (fragile, hard to read)
- Backward compatible: existing Claude behavior unchanged, other plugins unaffected
- Minimal scope: Codex CLI only (not Gemini, OpenCode, Copilot, Cursor)

## Design

### 1. Per-agent init_scripts table in WorkflowPlugin

Add `init_scripts: HashMap<String, String>` to the `WorkflowPlugin` struct. This is a TOML inline table `[init_scripts]` where keys are agent names and values are shell commands.

**Resolution order when selecting the init script for a worktree:**
1. `plugin.init_scripts[agent_name]` — agent-specific script (takes precedence)
2. `config.init_script` — existing project-level or plugin-level generic fallback

If neither is present, no init script runs (existing behavior).

### 2. Worktree setup changes in app.rs

In all three places where `init_script` is selected before spawning a worktree (initial creation, re-planning, resume), replace:

```rust
let init_script = if self.state.flags.no_init_scripts {
    None
} else {
    self.state.config.init_script.clone()
};
```

With:

```rust
let init_script = if self.state.flags.no_init_scripts {
    None
} else {
    plugin.as_ref()
        .and_then(|p| p.init_scripts.get(agent_name).cloned())
        .or_else(|| self.state.config.init_script.clone())
};
```

The `agent_name` is already available at these call sites from the task's agent configuration.

### 3. superpowers/plugin.toml changes

#### supported_agents
```toml
supported_agents = ["claude", "codex"]
```

#### init_scripts (replaces init_script)
```toml
[init_scripts]
claude = "claude plugin install superpowers@claude-plugins-official --scope local"
codex = """
CACHE_BASE="$HOME/.claude/plugins/cache/superpowers-marketplace/superpowers"
LATEST_VER=$(ls "$CACHE_BASE" | sort -V | tail -1)
SKILLS_SRC="$CACHE_BASE/$LATEST_VER/skills"
for skill_dir in "$SKILLS_SRC"/*/; do
  skill_name=$(basename "$skill_dir")
  mkdir -p ".codex/skills/superpowers-$skill_name"
  cp "$skill_dir/SKILL.md" ".codex/skills/superpowers-$skill_name/SKILL.md"
done
"""
```

The Codex script copies skill files from the marketplace cache to Codex-native skill directories. If the cache is absent (superpowers never installed for Claude), the script fails with a clear error — no `|| true` suppression.

#### commands (new section)
```toml
[commands]
planning = "/superpowers:brainstorming"
running = "/superpowers:executing-plans"
review = "/superpowers:requesting-code-review"
```

agtx's existing `transform_plugin_command` translates these per-agent:
- Claude: `/superpowers:brainstorming` (unchanged)
- Codex: `$superpowers-brainstorming`

Commands are sent before the task prompt at each phase transition, giving both agents an explicit skill invocation rather than relying on auto-loading.

## Files Changed

| File | Change |
|---|---|
| `src/config/mod.rs` | Add `init_scripts: HashMap<String, String>` to `WorkflowPlugin` |
| `src/tui/app.rs` | Update init_script selection at 3 worktree setup sites |
| `plugins/superpowers/plugin.toml` | supported_agents, init_scripts table, commands table |

## Testing

- Build passes (`cargo build --release`)
- Existing tests pass (`cargo test`)
- Claude path: create a superpowers task with Claude agent → `claude plugin install` runs, skill files appear in `.claude/commands/superpowers/`, commands section sends `/superpowers:brainstorming` at planning phase
- Codex path: create a superpowers task with Codex agent → skill files copied from cache to `.codex/skills/superpowers-*/`, `$superpowers-brainstorming` sent at planning phase
- Error surfacing: if cache is missing for Codex, worktree setup fails visibly (not silently)
