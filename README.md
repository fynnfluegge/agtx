<div align="center">

<img src="https://github.com/user-attachments/assets/f8d3f9b6-712b-49d0-a84e-2a2c9400bacc" width="680" />

### **Terminal-native kanban board for managing spec-driven AI coding agent sessions.**

Let different coding agents collaborate on the same task. Plug in any existing spec-driven development framework or specify your own workflow as a custom plugin with per-phase skills, prompts, artifact tracking and autonomous execution.

<img width="960" height="645" alt="Screenshot 2026-02-28 at 18 32 06" src="https://github.com/user-attachments/assets/442965f4-d2c4-435f-a065-df56dc483ab7" />

</div>

[//]: <![Xnapper-2026-02-14-09 36 33 (1)](https://github.com/user-attachments/assets/fce21a9c-2fe1-4b14-8f24-55e058531370)>

## Features

- **Kanban workflow**: Backlog/Research → Planning → Running → Review → Done
- **Git worktree and tmux isolation**: Each task gets its own worktree and tmux window, keeping work separated
- **Coding agent integrations**: Automatic session management for Claude Code, Codex, Gemini, Copilot and OpenCode
- **Multi-agent per task**: Configure different agents per workflow phase — e.g. Gemini for planning, Claude for implementation, Codex for review — with automatic agent switching in the same tmux window
- **Spec-driven development plugins**: Plug in any spec-driven development framework or select from a predefined set of plugins like GSD or Spec-kit — or define custom skills, prompts and artifact tracking - with automatic execution and tracking at each phase
- **Multi-project dashboard**: Manage tasks across all your projects
- **PR workflow**: Generate descriptions with AI, create PRs directly from the TUI
- **Customizable themes**: Configure colors via config file

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
- Supported coding agents: [Claude Code](https://github.com/anthropics/claude-code), [Codex](https://github.com/openai/codex), [Gemini](https://github.com/google-gemini/gemini-cli), [Copilot](https://github.com/github/copilot-cli), [OpenCode](https://github.com/sst/opencode)

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
| `r` | Resume task (Review → Running) |
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

**Skill references** (`!`) discover commands from your active agent's native command directory (e.g., `.claude/commands/` for Claude, `.codex/skills/` for Codex). The dropdown shows all available slash commands with descriptions, and inserts them in the agent's native invocation format:

```
/agtx:research the authentication module, then /agtx:plan a fix for the session timeout bug
```

This includes agtx built-in skills, plugin commands, and any custom user-defined commands.

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
research = "claude"
planning = "claude"
running = "codex"
review = "claude"
```

```toml
# .agtx/config.toml (project override — takes precedence over global)
[agents]
running = "gemini"
```

When a task moves to a phase with a different agent configured, the current agent session is terminated and the new agent starts automatically in the same tmux window. The worktree, git state, and all file changes are preserved across the switch.

Phases without an explicit agent override keep whatever agent is currently running — no unnecessary switching occurs.

## Spec-driven Development Plugins

agtx ships with a plugin system that lets any spec-driven development framework hook into the task lifecycle. A plugin is a single TOML file that defines what happens at each phase transition — the commands sent to the agent, the prompts, the artifact files that signal completion, and optional setup scripts. Write a command once in canonical format and agtx translates it automatically for every supported agent.

Press `P` to select a plugin for the current project. The active plugin is shown in the header bar.

| Plugin | Description |
|--------|-------------|
| **void** | Plain agent session - no prompting or skills, task description prefilled in input |
| **agtx** (default) | Built-in workflow with skills and prompts for each phase |
| **gsd** | [Get Shit Done](https://github.com/fynnfluegge/get-shit-done-cc) - structured spec-driven development with interactive planning |
| **spec-kit** | [Spec-Driven Development](https://github.com/github/spec-kit) by GitHub - specifications become executable artifacts |

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
running = ""
review = ""
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

# Artifact files that signal phase completion.
# When detected, the task shows a checkmark instead of the spinner.
# Supports * wildcard for one directory level (e.g. "specs/*/plan.md").
# Omitted phases fall back to agtx defaults (.agtx/plan.md, .agtx/execute.md, .agtx/review.md).
[artifacts]
research = ".my-plugin/research.md"
planning = ".my-plugin/plan.md"
running = ".my-plugin/summary.md"
review = ".my-plugin/review.md"

# Slash commands sent to the agent via tmux for each phase.
# Written in canonical format (Claude/Gemini style): /namespace:command
# Automatically transformed per agent:
#   Claude/Gemini: /my-plugin:plan (unchanged)
#   OpenCode:      /my-plugin-plan (colon -> hyphen)
#   Codex:         $my-plugin-plan (slash -> dollar, colon -> hyphen)
# Set to "" to skip sending a command for that phase.
[commands]
research = "/my-plugin:research {task}"
planning = "/my-plugin:plan {task}"
running = "/my-plugin:execute"
review = "/my-plugin:review"

# Prompt templates sent as task content after the command.
# {task} = task title + description, {task_id} = unique task ID.
# Set to "" to skip sending a prompt for that phase.
[prompts]
research = "Task: {task}"
planning = ""
running = ""
review = ""

# Text patterns to wait for in the tmux pane before sending the prompt.
# Useful when a command triggers an interactive prompt that must appear first.
# Polls every 500ms, times out after 5 minutes.
[prompt_triggers]
research = "What do you want to build?"
```

**What happens at each phase transition:**

1. The **command** is sent to the agent via tmux (e.g., `/my-plugin:plan`)
2. If a **prompt_trigger** is set, agtx waits for that prompt trigger to appear in the tmux pane
3. The **prompt** is sent with `{task}` and `{task_id}` replaced
4. agtx polls for the **artifact** file — when found, the spinner becomes a checkmark to indicate task phase completion

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
