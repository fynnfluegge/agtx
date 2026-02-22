# agtx

#### A terminal-native kanban board for managing coding agent sessions.

![Xnapper-2026-02-14-09 36 33 (1)](https://github.com/user-attachments/assets/fce21a9c-2fe1-4b14-8f24-55e058531370)

## Features

- **Kanban workflow**: Backlog → Explore → Planning → Running → Review → Done
- **Git worktree and tmux isolation**: Each task gets its own worktree and tmux window, keeping work separated
- **Multi-agent support**: Claude Code, Aider, Codex, GitHub Copilot, OpenCode, Cline, Amazon Q
- **Auto-respawn**: Dead agent sessions are detected and respawned when you open a task
- **PR workflow**: Generate descriptions with AI, create PRs directly from the TUI
- **Multi-project dashboard**: Manage tasks across all your projects
- **Configurable hooks**: Custom scripts for review and done transitions
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
| `↩` | Open task (view agent session) / edit task (backlog) |
| `m` | Move task forward in workflow |
| `M` | Skip Explore, move Backlog → Planning directly |
| `r` | Move task backward (Review → Running, Running → Planning, etc.) |
| `d` | Show git diff |
| `x` | Delete task |
| `/` | Search tasks |
| `e` | Toggle project sidebar |
| `q` | Quit |

### Task Workflow

1. **Create a task** (`o`): Enter title and description (use `#` or `@` to fuzzy-find files)
2. **Move to Explore** (`m`): Creates worktree, starts agent in exploration mode (research, no changes)
3. **Move to Planning** (`m`): Agent creates an implementation plan
4. **Move to Running** (`m`): Agent implements the plan
5. **Move to Review** (`m`): Optionally create a PR (or run a custom hook)
6. **Move to Done** (`m`): Cleanup worktree and tmux window (branch kept locally)

> Use `M` from Backlog to skip Explore and go straight to Planning.

### Agent Session Features

- Dead sessions are auto-detected and respawned when you open a task
- Claude sessions resume with `--resume` to preserve full conversation context
- Tmux window stays open when moving Running → Review, so you can resume work
- View live agent output in the task popup

## Configuration

Config file location: `~/.config/agtx/config.toml`

```toml
# Default agent for new tasks
default_agent = "claude"

# Per-agent CLI flags (no defaults — users must opt-in)
[agent_flags]
claude = ["--dangerously-skip-permissions"]
aider = ["--no-auto-commits"]

[worktree]
enabled = true
auto_cleanup = true
base_branch = "main"

[theme]
color_selected = "#ead49a"
color_normal = "#5cfff7"
color_dimmed = "#9C9991"
color_text = "#f2ece6"
color_accent = "#5cfff7"
color_description = "#C4B0AC"
color_column_header = "#a0d2fa"
color_popup_border = "#9ffcf8"
color_popup_header = "#69fae7"
```

### Project Configuration

Per-project settings can be placed in `.agtx/config.toml` at the project root:

```toml
# Files to copy from project root into each new worktree (comma-separated)
copy_files = ".env, .env.local, web/.env.local"

# Shell command to run inside the worktree after creation and file copying
init_script = "scripts/init_worktree.sh"

# Customize workflow transitions
# Options: omit for default behavior, "skip" to skip, or a shell command to run
on_review = "skip"                    # Skip PR prompt, just move the task
on_done = "scripts/post-done.sh"      # Run custom script

# Override global agent flags for this project
[agent_flags]
claude = []  # Use Claude's default permission system here
```

Worktree setup (file copying, init script) runs when a task first leaves Backlog
(either to Explore or Planning), after `git worktree add` and before the agent starts.

Hook commands receive these environment variables: `AGTX_TASK_ID`, `AGTX_TASK_TITLE`,
`AGTX_BRANCH_NAME`, `AGTX_WORKTREE_PATH`, `AGTX_PROJECT_PATH`.

## How It Works

### Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                            agtx TUI                                  │
├──────────────────────────────────────────────────────────────────────┤
│  Backlog │ Explore │ Planning │  Running  │  Review  │     Done      │
│  ┌─────┐ │ ┌─────┐ │ ┌─────┐  │  ┌─────┐  │  ┌─────┐ │              │
│  │Task1│ │ │Task2│ │ │Task3│  │  │Task4│  │  │Task5│ │              │
│  └─────┘ │ └─────┘ │ └─────┘  │  └─────┘  │  └─────┘ │              │
└──────────────────────────────────────────────────────────────────────┘
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

- **Database**: `~/Library/Application Support/agtx/` (macOS) or `~/.config/agtx/` (Linux)
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
