<div align="center">

[//]: <img src="https://github.com/user-attachments/assets/54ac039b-085e-490b-aacc-36c8e244e313" width="428" />

<img width="1486" height="680" src="https://github.com/user-attachments/assets/45858e09-ab61-422b-b708-db060c73a900" />

## Run and manage multi-agent AI coding workflows from a single terminal board
Let different AI coding agents collaborate on the same task. **Gemini → research → Claude → implement → Codex → review.**  
Plug in any existing spec-driven development framework or specify your own workflow as a custom plugin with per-phase skills, prompts, artifact tracking and autonomous execution.


[//]: <img width="1486" height="680" src="https://github.com/user-attachments/assets/45858e09-ab61-422b-b708-db060c73a900" />

</div>

[//]: <![Xnapper-2026-02-14-09 36 33 (1)](https://github.com/user-attachments/assets/fce21a9c-2fe1-4b14-8f24-55e058531370)>

## Features

- **Multi-project dashboard**: Manage and orchestrate coding agent sessions across all your projects
- **Multi-agent task lifecycle**: Configure different agents per workflow phase — e.g. Gemini for planning, Claude for implementation, Codex for review — with automatic agent switching in the same tmux window
- **Git worktree and tmux isolation**: Each task gets its own worktree and tmux window, keeping work separated
- **Spec-driven development plugins**: Plug in any spec-driven development framework or select from a predefined set of plugins like GSD, Spec-kit or OpenSpec — or define custom skills, prompts and artifact tracking - with automatic execution and tracking at each phase
- **Multi-agent per task**: Configure different agents per workflow phase — e.g. Gemini for planning, Claude for implementation, Codex for review — with automatic agent switching in the same tmux window

## Installation

### Quick Install

```bash
curl -fsSL https://raw.githubusercontent.com/fynnfluegge/agtx/main/install.sh | bash
```

### From Source

```bash
cargo build --release
cp target/release/agtx ~/.local/bin/
```

### Requirements

- **tmux** - Agent sessions run in a dedicated tmux server
- **gh** - GitHub CLI for PR operations
- Supported coding agents: [Claude Code](https://github.com/anthropics/claude-code), [Codex](https://github.com/openai/codex), [Gemini](https://github.com/google-gemini/gemini-cli), [OpenCode](https://github.com/sst/opencode), [Copilot](https://github.com/github/copilot-cli)

## Quick Start

```bash
# Run in any git repository
cd your-project
agtx

# Or run in dashboard mode (manage all projects)
agtx -g
```

> [!NOTE]
> Add `.agtx/` to your project's `.gitignore` to avoid committing worktrees and local task data.

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
| `e` | Toggle project sidebar |
| `q` | Quit |

### Task Description Editor

When writing a task description, you can reference files and agent skills inline:

| Key | Action |
|-----|--------|
| `#` or `@` | Fuzzy search and insert a file path |
| `!` | Fuzzy search and insert an agent skill/command |


### Agent Session Features

- Sessions automatically resume when moving Review → Running
- Full conversation context is preserved across the task lifecycle
- View live agent output in the task popup

## Configuration

Config file location: `~/.config/agtx/config.toml`

### Project Configuration

Per-project settings can be placed in `.agtx/config.toml` at the project root:

```toml
# Files to copy from project root into each new worktree (comma-separated)
# Paths are relative and preserve directory structure
copy_files = ".env, .env.local, web/.env.local"

# Shell command to run inside the worktree after creation and file copying
init_script = "scripts/init_worktree.sh"
```

Both options run during the Backlog → Research/Planning/Running transition, after worktree creation
and before the agent session starts.

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

## Spec-driven Development Plugins

agtx ships with a fully declarative plugin framework — a single TOML file is all it takes to integrate any spec-driven development framework into the task lifecycle. 

A plugin defines commands, prompts and artifacts — agtx handles the rest: phase gating, artifact polling, worktree sync, agent switching, and autonomous execution.

Press `P` to activate a plugin for the current project. The active plugin is shown in the header bar.

| Plugin | Description |
|--------|-------------|
| **void** | Plain agent session - no prompting or skills, task description prefilled in input |
| **agtx** (default) | Built-in workflow with skills and prompts for each phase |
| **gsd** | [Get Shit Done](https://github.com/fynnfluegge/get-shit-done-cc) - structured spec-driven development with interactive planning |
| **spec-kit** | [Spec-Driven Development](https://github.com/github/spec-kit) by GitHub - specifications become executable artifacts |
| **openspec** | [OpenSpec](https://github.com/Fission-AI/OpenSpec) - lightweight AI-guided specification framework |

### Agent Compatibility

Commands are written once in canonical format and automatically translated per agent:

| Canonical (plugin.toml) | Claude / Gemini | Codex | OpenCode |
|--------------------------|-----------------|-------|----------|
| `/agtx:plan` | `/agtx:plan` | `$agtx-plan` | `/agtx-plan` |

|  | Claude | Codex | Gemini | Copilot | OpenCode |
|--|:------:|:-----:|:------:|:-------:|:--------:|
| **agtx** | ✅ | ✅ | ✅ | 🟡 | ✅ |
| **gsd** | ✅ | ✅ | ✅ | ❌ | ✅ |
| **spec-kit** | ✅ | ✅ | ✅ | 🟡 | ✅ |
| **openspec** | ✅ | ✅ | ✅ | 🟡 | ✅ |
| **void** | ✅ | ✅ | ✅ | ✅ | ✅ |

✅ Skills, commands, and prompts fully supported · 🟡 Prompt only, no interactive skill support · ❌ Not supported

### Creating a Plugin

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

## Development

See [CLAUDE.md](CLAUDE.md) for development documentation.

```bash
# Build
cargo build

# Run tests (includes mock-based tests)
cargo test --features test-mocks

# Build release
cargo build --release
```
