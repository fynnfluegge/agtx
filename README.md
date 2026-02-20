# agtx

#### A terminal-native kanban board for managing coding agent sessions.

![Xnapper-2026-02-14-09 36 33 (1)](https://github.com/user-attachments/assets/fce21a9c-2fe1-4b14-8f24-55e058531370)

## Features

- **Kanban workflow**: Backlog → Planning → Running → Review → Done
- **Git worktree and tmux isolation**: Each task gets its own worktree and tmux window, keeping work separated
- **Claude Code integration**: Automatic session management with resume capability
- **PR workflow**: Generate descriptions with AI, create PRs directly from the TUI
- **Multi-project dashboard**: Manage tasks across all your projects
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
- **claude** - Claude Code CLI

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
| `↩` | Open task (view Claude session) |
| `m` | Move task forward in workflow |
| `r` | Resume task (Review → Running) |
| `d` | Show git diff |
| `x` | Delete task |
| `/` | Search tasks |
| `e` | Toggle project sidebar |
| `q` | Quit |

### Task Workflow

1. **Create a task** (`o`): Enter title and description
2. **Move to Planning** (`m`): Creates worktree, starts Claude in planning mode
3. **Move to Running** (`m`): Claude implements the plan
4. **Move to Review** (`m`): Opens PR with AI-generated description
5. **Move to Done** (`m`): Cleans up after PR is merged

### Claude Session Features

- Sessions automatically resume when moving Review → Running
- Full conversation context is preserved across the task lifecycle
- View live Claude output in the task popup

## Configuration

Config file location: `~/.config/agtx/config.toml`

```toml
# Default agent for new tasks
default_agent = "claude"

[worktree]
enabled = true
auto_cleanup = true
base_branch = "main"

[theme]
color_selected = "#FFFF99"
color_normal = "#00FFFF"
color_dimmed = "#666666"
color_text = "#FFFFFF"
color_accent = "#00FFFF"
color_description = "#E8909C"
```

### Project Configuration

Per-project settings can be placed in `.agtx/config.toml` at the project root:

```toml
# Files to copy from project root into each new worktree (comma-separated)
# Paths are relative and preserve directory structure
copy_files = ".env, .env.local, web/.env.local"

# Shell command to run inside the worktree after creation and file copying
init_script = "scripts/init_worktree.sh"
```

Both options run during the Backlog → Planning transition, after `git worktree add`
and before the agent session starts.

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

- **Database**: `~/.config/agtx/` (stores task metadata per project)
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
