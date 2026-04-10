<div align="center">

[//]: <img src="https://github.com/user-attachments/assets/54ac039b-085e-490b-aacc-36c8e244e313" width="428" />

# agtx

<div align="left">
    
> **An AI agent that manages other coding agents in a terminal kanban board** - Add tasks. Press one key. An orchestrator agent picks it up, plans, and delegates to multiple coding agents running in parallel. Come back to changes ready to merge.
> 
> **Let different AI coding agents collaborate** autonomously on the same task with automatic session switching and context awareness -  
> e.g. **Gemini** → research | **Claude** → implement | **Codex** → review

</div>

[![CI](https://github.com/fynnfluegge/agtx/actions/workflows/ci.yml/badge.svg)](https://github.com/fynnfluegge/agtx/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/fynnfluegge/agtx)](https://github.com/fynnfluegge/agtx/releases)
[![PRs Welcome](https://img.shields.io/badge/PRs-welcome-brightgreen.svg)](CONTRIBUTING.md)
[![License: Apache-2.0](https://img.shields.io/badge/License-Apache_2.0-blue.svg)](https://opensource.org/licenses/Apache-2.0)

---

<img width="840" src="https://github.com/user-attachments/assets/45858e09-ab61-422b-b708-db060c73a900" />

[//]:  <img width="840" src="https://github.com/user-attachments/assets/42f71a6c-424c-4cc4-80fc-dc9bb8ba1467" />

<br/>

[//]: <img width="1486" height="680" src="https://github.com/user-attachments/assets/45858e09-ab61-422b-b708-db060c73a900" />

[//]: <![Xnapper-2026-02-14-09 36 33 (1)](https://github.com/user-attachments/assets/fce21a9c-2fe1-4b14-8f24-55e058531370)>

</div>

## Why agtx?

AI coding tools give you one agent, one task, one terminal. agtx gives you a **kanban board where multiple coding agents work in parallel** — each in its own git worktree, each in its own tmux window, running autonomously through a spec-driven workflow managed by an orchestrator agent.

With the orchestrator, you don't even manage the board yourself. **An AI agent picks up tasks, delegates work, and ensures getting things done** through planning, implementation, review and resolving conflicts — while you focus on what matters: research, defining tasks, and merging changes.

> [!TIP]
> Check out the [Contributing](#contributing) section or have a look at [`good first issues`](https://github.com/fynnfluegge/agtx/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) to get involved and become a contributor ⭐️ 

## Features

- **Orchestrator agent**: A dedicated AI agent that autonomously manages your kanban board via [MCP](https://modelcontextprotocol.io) — delegates to coding agents, advances phases, checks for merge conflicts ([experimental](#orchestrator-agent-experimental))
- **Multi-agent task lifecycle**: Configure different agents per workflow phase — e.g. Gemini for research, Claude for implementation, Codex for review — with automatic agent switching
- **Parallel execution**: Every task gets its own git worktree and tmux window — run as many agents as needed, simultaneously
- **Spec-driven plugins**: Plug in [GSD](https://github.com/fynnfluegge/get-shit-done-cc), [Spec-kit](https://github.com/github/spec-kit), [OpenSpec](https://github.com/Fission-AI/OpenSpec), [BMAD](https://github.com/bmad-code-org/BMAD-METHOD), [Superpowers](https://github.com/obra/superpowers) — or define your own with a single TOML file
- **Multi-project dashboard**: Manage agent sessions across all projects via a single TUI
- **Works with**: [Claude Code](https://github.com/anthropics/claude-code) | [Codex](https://github.com/openai/codex) | [Gemini CLI](https://github.com/google-gemini/gemini-cli) | [OpenCode](https://github.com/sst/opencode)  | [Cursor Agent](https://cursor.com/docs/cli/overview) | [Copilot](https://github.com/github/copilot-cli)

> [!NOTE]
> Just need a plain coding agent session manager with **full human-in-the-loop control** and **no automatic spec-driven skill execution and orchestration** on advancing tasks?
>
> Choose the **`void` plugin** and enjoy the kanban coding agent board - with full human control.

## Quick Start

```bash
# Install
curl -fsSL https://raw.githubusercontent.com/fynnfluegge/agtx/main/install.sh | bash

# Run in any git repository
cd your-project && agtx
```

```bash
# Dashboard mode — manage all projects
agtx -g

# Orchestrator mode — let an AI manage the board for you
agtx --experimental
```

> [!NOTE]
> Add `.agtx/` to your project's `.gitignore` to avoid committing worktrees and local task data.

```bash
# Install from source
cargo build --release
cp target/release/agtx ~/.local/bin/
```
</details>

### Requirements

- **tmux** — agent sessions run in a dedicated tmux server
- **gh** (optional) — GitHub CLI for PR operations

## Usage

### Keyboard Shortcuts

| Key | Action |
|-----|--------|
| `h/l` or `←/→` | Move between columns |
| `j/k` or `↑/↓` | Move between tasks |
| `o` | Create new task |
| `R` | Enter research mode |
| `↩` | Open task (view agent session) |
| `m` | Move task forward in workflow |
| `r` | Resume task (Review → Running) / Move back (Running → Planning) |
| `p` | Next phase (Review → Planning, cyclic plugins only) |
| `d` | Show git diff |
| `x` | Delete task |
| `/` | Search tasks |
| `P` | Select spec-driven workflow plugin |
| `O` | Toggle orchestrator agent (`--experimental`) |
| `e` | Toggle project sidebar |
| `q` | Quit |

### Task Creation Wizard

Press `o` to create a new task. The wizard guides you through:
1. **Title** — enter a short task name
2. **Plugin** — select a workflow plugin (auto-skipped if only one option)
3. **Prompt** — write a detailed task description with inline references

The agent is configured at the project level via `config.toml` (not per-task).

### Task Description Editor

When writing a task description, you can reference files, skills, and other tasks inline:

| Key | Action |
|-----|--------|
| `#` or `@` | Fuzzy search and insert a file path |
| `/` | Fuzzy search and insert an agent skill/command (at line start or after space) |
| `!` | Fuzzy search and insert a task reference (at line start or after space) |


### Agent Session Features

- Sessions automatically resume when moving Review → Running
- Full conversation context is preserved across the task lifecycle
- View live agent output in the task popup
- **Auto merge-conflict resolution**: When a Review task becomes idle, agtx checks for merge conflicts with the default branch. If conflicts are detected, the agent is automatically instructed to resolve them.

## Configuration

Config file location: `~/.config/agtx/config.toml`

### Worktree Base Branch

agtx creates a new git worktree for each task. By default it auto-detects the base branch in this
order: `main`, then `master`, then the current branch. You can override this to force a specific
base branch (for example `dev` or `develop`).

Global worktree defaults can be set here:

```toml
# ~/.config/agtx/config.toml
[worktree]
base_branch = "dev"
worktree_dir = ".worktrees"  # default: ".agtx/worktrees"
```

`worktree_dir` is the directory (relative to project root) where task worktrees are created. Defaults
to `.agtx/worktrees` if not set.

### Project Configuration

Per-project settings can be placed in `.agtx/config.toml` at the project root:

```toml
# Base branch used when creating new task worktrees (optional)
base_branch = "dev"

# Directory where worktrees are created (optional, default: ".agtx/worktrees")
worktree_dir = ".worktrees"

# Files to copy from project root into each new worktree (comma-separated)
# Paths are relative and preserve directory structure
copy_files = ".env, .env.local, web/.env.local"

# Shell command to run inside the worktree after creation and file copying
init_script = "scripts/init_worktree.sh"

# Shell command to run inside the worktree before removal
cleanup_script = "scripts/cleanup_worktree.sh"
```

`base_branch` controls which branch new task worktrees are created from. If omitted or empty, agtx
auto-detects `main`, `master`, or falls back to the current branch.

### Per-Phase Agent Configuration

By default, all phases use `default_agent`. You can override the agent for specific phases globally or per project:

```toml
# ~/.config/agtx/config.toml
default_agent = "claude"

[agents]
research = "gemini"
planning = "claude"
running = "claude"
review = "codex"
```

```toml
# .agtx/config.toml (project override — takes precedence over global)
[agents]
running = "codex"
```

## Plugins

Plug any spec-driven framework into the task lifecycle. Define commands, prompts, and artifacts — agtx handles phase gating, artifact polling, worktree sync, agent switching, and autonomous execution.

Press `P` to switch plugins. Ships with 7 built-in:

| Plugin | Description |
|--------|-------------|
| **void** | Plain agent session - no prompting or skills, task description prefilled in input |
| **agtx** (default) | Built-in workflow with skills and prompts for each phase |
| **gsd** | [Get Shit Done](https://github.com/fynnfluegge/get-shit-done-cc) - structured spec-driven development with interactive planning |
| **spec-kit** | [Spec-Driven Development](https://github.com/github/spec-kit) by GitHub - specifications become executable artifacts |
| **openspec** | [OpenSpec](https://github.com/Fission-AI/OpenSpec) - lightweight AI-guided specification framework |
| **bmad** | [BMAD Method](https://github.com/bmad-code-org/BMAD-METHOD) - AI-driven agile development with structured phases |
| **superpowers** | [Superpowers](https://github.com/obra/superpowers) - brainstorming, plans, TDD, subagent-driven development |

### Agent Compatibility

Commands are written once in canonical format and automatically translated per agent:

| Canonical (plugin.toml) | Claude / Gemini | Codex | OpenCode | Cursor |
|--------------------------|-----------------|-------|----------|--------|
| `/agtx:plan` | `/agtx:plan` | `$agtx-plan` | `/agtx-plan` | `/agtx-plan` |

|  | Claude | Codex | Gemini | OpenCode | Cursor | Copilot |
|--|:------:|:-----:|:------:|:--------:|:------:|:-------:|
| **agtx** | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 |
| **gsd** | ✅ | ✅ | ✅ | ✅ | ✅ | ❌ |
| **spec-kit** | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 |
| **openspec** | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 |
| **bmad** | ✅ | ✅ | ✅ | ✅ | ✅ | 🟡 |
| **superpowers** | ✅ | ❌ | ❌ | ❌ | ❌ | ❌ |
| **void** | ✅ | ✅ | ✅ | ✅ | ✅ | ✅ |

✅ Skills, commands, and prompts fully supported · 🟡 Prompt only, no interactive skill support · ❌ Not supported

<details>
<summary><b>Creating a Plugin</b></summary>

Place your plugin at `.agtx/plugins/<name>/plugin.toml` in your project root (or `~/.config/agtx/plugins/<name>/plugin.toml` for global use). It will appear in the plugin selector automatically.

**Minimal example** — a plugin that uses custom slash commands:

```toml
name = "my-plugin"
description = "My custom workflow"

[commands]
research = "/my-plugin:research {task}"
planning = "/my-plugin:plan"
running = "/my-plugin:execute"
review = "/my-plugin:review"

[prompts]
planning = "Task: {task}"
```

**Full reference** with all available fields:

```toml
name = "my-plugin"
description = "My custom workflow"

# Shell command to run in the worktree after creation, before the agent starts.
# {agent} is replaced with the agent name (claude, codex, gemini, etc.)
init_script = "npm install --prefix .my-plugin --{agent}"

# Restrict to specific agents (empty or omitted = all agents supported)
supported_agents = ["claude", "codex", "gemini", "opencode"]

# Extra directories to copy from project root into each worktree.
# Agent config dirs (.claude, .gemini, .codex, .github/agents, .config/opencode)
# are always copied automatically.
copy_dirs = [".my-plugin"]

# Individual files to copy from project root into each worktree.
# Merged with project-level copy_files from .agtx/config.toml.
copy_files = ["PROJECT.md", "REQUIREMENTS.md"]

# When true, enables Review → Planning transition via the `p` key.
# Each cycle increments the phase counter ({phase} placeholder).
# Use this for multi-milestone workflows (e.g. plan → execute → review → next milestone).
cyclic = false

# Artifact files that signal phase completion.
# When detected, the task shows a checkmark instead of the spinner.
# Supports * wildcard for one directory level (e.g. "specs/*/plan.md").
# Use {phase} for cycle-aware paths (replaced with the current cycle number).
# Omitted phases show no completion indicator.
[artifacts]
research = ".my-plugin/research.md"
planning = ".my-plugin/{phase}/plan.md"
running = ".my-plugin/{phase}/summary.md"
review = ".my-plugin/{phase}/review.md"

# Slash commands sent to the agent via tmux for each phase.
# Written in canonical format (Claude/Gemini style): /namespace:command
# Automatically transformed per agent:
#   Claude/Gemini: /my-plugin:plan (unchanged)
#   OpenCode:      /my-plugin-plan (colon -> hyphen)
#   Codex:         $my-plugin-plan (slash -> dollar, colon -> hyphen)
# Omitted phases fall back to agent-native agtx skill invocation
# (e.g. /agtx:plan for Claude, $agtx-plan for Codex).
# Set to "" to skip sending a command for that phase.
# Use {phase} for cycle-aware commands (replaced with the current cycle number).
# Use {task} to inline the task description.
[commands]
preresearch = "/my-plugin:research {task}"  # Used only when no research artifacts exist yet
research = "/my-plugin:discuss {phase}"
planning = "/my-plugin:plan {phase}"
running = "/my-plugin:execute {phase}"
review = "/my-plugin:review {phase}"

# Prompt templates sent as task content after the command.
# {task} = task title + description, {task_id} = unique task ID, {phase} = cycle number.
# Omitted phases send no prompt (the skill/command handles instructions).
[prompts]
research = "Task: {task}"

# Text patterns to wait for in the tmux pane before sending the prompt.
# Useful when a command triggers an interactive prompt that must appear first.
# Polls every 500ms, times out after 5 minutes.
[prompt_triggers]
research = "What do you want to build?"

# Files/dirs to copy from worktree back to project root after a phase completes.
# Triggered automatically when the phase artifact is detected (spinner → checkmark).
# Useful for sharing research artifacts (specs, plans) across worktrees.
[copy_back]
research = ["PROJECT.md", "REQUIREMENTS.md", ".my-plugin"]

# Auto-dismiss interactive prompts that appear before the prompt trigger.
# Each rule fires when ALL detect patterns are present and the pane is stable.
# Response is newline-separated keystrokes (e.g. "2\nEnter" sends "2" then Enter).
[[auto_dismiss]]
detect = ["Map codebase", "Skip mapping", "Enter to select"]
response = "2\nEnter"
```

**What happens at each phase transition:**

1. The **command** is sent to the agent via tmux (e.g., `/my-plugin:plan`)
2. If a **prompt_trigger** is set, agtx waits for that prompt trigger to appear in the tmux pane
3. The **prompt** is sent with `{task}`, `{task_id}`, and `{phase}` replaced
4. agtx polls for the **artifact** file — when found, the spinner becomes a checkmark
5. If **copy_back** is configured, artifacts are copied from worktree to project root on completion
6. If the agent appears idle (no output for 15s), the spinner becomes a pause icon

**Phase gating:** Whether a phase can be entered directly from Backlog is derived from the plugin config. If a phase's command or prompt contains `{task}`, it can receive task context and is accessible from Backlog. If neither has `{task}`, the phase depends on a prior phase and is blocked until that artifact exists. For example, OpenSpec's `/opsx:propose {task}` allows direct Backlog → Planning, but `/opsx:apply` (no `{task}`) blocks Backlog → Running until proposal artifacts exist.

**Preresearch fallback:** When pressing `R` on a task, if `preresearch` is configured and no research artifacts from `copy_back` exist in the project root yet, the `preresearch` command is used instead of `research`. This lets plugins run a one-time project setup (e.g. `/gsd:new-project`) before switching to the regular research command for subsequent tasks. If the plugin has no research command at all (e.g. OpenSpec), pressing `R` shows a warning.

**Cyclic workflows:** When `cyclic = true`, pressing `p` in Review moves the task back to Planning with an incremented phase counter. This enables multi-milestone workflows where each cycle (plan → execute → review) produces artifacts in a separate `{phase}` directory.

**Custom skills:** If your plugin provides its own skill files, place them in the plugin directory:

```
.agtx/plugins/my-plugin/
├── plugin.toml
└── skills/
    ├── agtx-plan/SKILL.md
    ├── agtx-execute/SKILL.md
    └── agtx-review/SKILL.md
```

These override the built-in agtx skills and are automatically deployed to each agent's native discovery path (`.claude/commands/`, `.codex/skills/`, `.gemini/commands/`, etc.) in every worktree.

</details>

## How It Works

### Architecture

```
┌─────────────────────────────────────────────────────────┐
│                      agtx TUI                           │
├─────────────────────────────────────────────────────────┤
│  Backlog  │  Planning  │  Running  │  Review  │  Done   │
│  ┌─────┐  │  ┌─────┐   │  ┌─────┐  │  ┌─────┐ │         │
│  │Task1│  │  │Task2│   │  │Task3│  │  │Task4│ │         │
│  └─────┘  │  └─────┘   │  └─────┘  │  └─────┘ │         │
└─────────────────────────────────────────────────────────┘
                    │           │
                    ▼           ▼
┌─────────────────────────────────────────────────────────┐
│                 tmux server "agtx"                      │
│  ┌────────────────────────────────────────────────────┐ │
│  │ Session: "my-project"                              │ │
│  │  ┌────────┐  ┌────────┐  ┌────────┐                │ │
│  │  │Window: │  │Window: │  │Window: │                │ │
│  │  │task2   │  │task3   │  │task4   │                │ │
│  │  │(Claude)│  │(Claude)│  │(Claude)│                │ │
│  │  └────────┘  └────────┘  └────────┘                │ │
│  └────────────────────────────────────────────────────┘ │
│  ┌────────────────────────────────────────────────────┐ │
│  │ Session: "other-project"                           │ │
│  │  ┌───────────────────┐                             │ │
│  │  │ Window:           │                             │ │
│  │  │ some_other_task   │                             │ │
│  │  └───────────────────┘                             │ │
│  └────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────┘
                    │           │
                    ▼           ▼
            ┌───────────────────────────┐
            │   Git Worktrees           │
            │  .agtx/worktrees/task2/   │
            │  .agtx/worktrees/task3/   │
            │  .agtx/worktrees/task4/   │
            └───────────────────────────┘
```

### Tmux Structure

- **Server**: All sessions run on a dedicated tmux server named `agtx`
- **Sessions**: Each project gets its own tmux session (named after the project)
- **Windows**: Each task gets its own window within the project's session

```bash
# List all sessions
tmux -L agtx list-sessions

# List all windows across sessions
tmux -L agtx list-windows -a

# Attach to the agtx server
tmux -L agtx attach
```

### Data Storage

- **Database**: `~/Library/Application Support/agtx/` (macOS) or `~/.local/share/agtx/` (Linux)
- Config: `~/.config/agtx/config.toml`
- **Worktrees**: `.agtx/worktrees/` in each project
- **Tmux**: Dedicated server `agtx` with per-project sessions

## Orchestrator Agent (Experimental)

> Press `O` and walk away. Come back to changes ready to merge.

The orchestrator is an AI agent that **drives other AI agents to completion**. You triage tasks into Planning or Running — the orchestrator takes over from there, advancing each task through its phases until it lands in Review, ready for you to merge.

```bash
agtx --experimental   # then press O
```

**What it does:**
- Monitors tasks in Planning and Running
- Advances tasks automatically as phases complete (Planning → Running → Review)
- Respects plugin phase rules — checks `allowed_actions` before each transition
- Detects stuck tasks (idle for 1+ minute without a phase artifact) and reads the agent pane to diagnose the cause
- Nudges stuck agents, answers CLI prompts automatically, or escalates to you with a reason when human input is needed

**You triage. It executes.** Move tasks from Backlog into Planning or Running — the orchestrator handles the rest. Merging is your call.

### MCP Integration

The orchestrator communicates with agtx through the [Model Context Protocol (MCP)](https://modelcontextprotocol.io). agtx ships with a built-in MCP server (`agtx serve`) that exposes the kanban board as a set of tools over JSON-RPC via stdio.

```
┌─────────────-┐     MCP (stdio)     ┌──────────────┐     SQLite     ┌─────┐
│ Orchestrator │ ←─────────────────→ │  MCP Server  │ ←────────────→ │ DB  │
│ (Claude Code)│                     │ (agtx serve) │                └──┬──┘
└──────┬───────┘                     └──────────────┘                   │
       │  push-when-idle notifications                                  │
┌──────┴───────┐                                                        │
│   TUI (agtx) │ ←───────────────────────────────────────────────────--─┘
└──────────────┘
```

**MCP tools available to the orchestrator:**

| Tool | Description |
|------|-------------|
| `list_tasks` | List all tasks, optionally filtered by status |
| `get_task` | Get task details including `allowed_actions` for valid transitions |
| `move_task` | Queue a state transition (the TUI executes it with full side effects) |
| `get_transition_status` | Check if a queued transition completed or errored |
| `check_conflicts` | Non-destructive merge conflict detection against the default branch |
| `get_notifications` | Manually fetch pending notifications (backup — usually pushed automatically) |
| `read_pane_content` | Read the last N lines of a task's agent tmux pane |
| `send_to_task` | Send a message to a task's agent pane (Planning/Running only) |

**How it works:**
1. When you press `O`, the TUI registers the MCP server with the orchestrator agent via `claude mcp add-json --scope local`
2. The orchestrator receives phase completion notifications pushed to its tmux pane when idle
3. It reacts by calling `get_task` to check `allowed_actions`, then `move_task` to advance the task
4. The TUI processes the transition request, executes all side effects (agent switching, skill deployment, prompt sending), and updates the database
5. If a task has been idle for 1+ minute without a phase artifact, the orchestrator is notified — it reads the pane with `read_pane_content`, then either nudges the agent with `send_to_task` or calls `move_task` with `escalate_to_user` to flag it for your attention
6. Escalated tasks show a `⚠` badge on the kanban board; opening the task popup shows the reason and dismisses the flag
7. MCP registration is cleaned up when the orchestrator is stopped

## Contributing

Contributions are welcome! Whether it's a bug fix, new plugin, agent integration, or documentation improvement.

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full guide. Here's the short version:

```bash
# Fork & clone
git clone https://github.com/<you>/agtx && cd agtx

# Build & test
cargo build && cargo test --features test-mocks
```

### Good First Contributions

Not sure where to start? Here are some ideas:

- **Write a plugin** — A single `plugin.toml` is all you need. See [Creating a Plugin](#plugins) for the full reference
- **Add a new agent** — Integrate your favorite AI coding CLI. See the [architecture docs](CLAUDE.md) for how agents are structured
- **Improve documentation** — Found something unclear? Help others by improving it
- **Report bugs** — Open an [issue](https://github.com/fynnfluegge/agtx/issues). Reproduction steps are always appreciated
- **Browse open issues** — Check the [`good first issue`](https://github.com/fynnfluegge/agtx/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) label for beginner-friendly tasks

## Development

See [CLAUDE.md](CLAUDE.md) for full architecture docs and development patterns.

```bash
# Build
cargo build

# Run tests (includes mock-based tests)
cargo test --features test-mocks

# Build release
cargo build --release
```
