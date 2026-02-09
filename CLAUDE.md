# AGTX - Terminal Kanban for Coding Agents

A terminal-native kanban board for managing multiple coding agent sessions (Claude Code, Aider, etc.) with isolated git worktrees.

## Quick Start

```bash
# Build
cargo build --release

# Run in a git project directory
./target/release/agtx

# Or run in dashboard mode
./target/release/agtx -g
```

## Architecture

```
src/
├── main.rs           # Entry point, CLI arg parsing, AppMode enum
├── tui/
│   ├── mod.rs        # Re-exports
│   ├── app.rs        # Main App struct, event loop, rendering (largest file)
│   ├── board.rs      # BoardState - kanban column/row navigation
│   └── input.rs      # InputMode enum for UI states
├── db/
│   ├── mod.rs        # Re-exports
│   ├── schema.rs     # Database struct, SQLite operations
│   └── models.rs     # Task, Project, TaskStatus, AgentStatus enums
├── tmux/
│   └── mod.rs        # Tmux server "agents", session management
├── git/
│   ├── mod.rs        # is_git_repo helper
│   └── worktree.rs   # Git worktree create/remove/list
├── agent/
│   └── mod.rs        # Agent definitions, detection, spawn args
└── config/
    └── mod.rs        # GlobalConfig, ProjectConfig, MergedConfig
```

## Key Concepts

### Task Workflow
```
Backlog → Planning → Running → Review → Done
           ↓           ↓         ↓
        worktree    Claude    PR opens
        created     starts
```

- **Planning**: Creates git worktree at `.agtx/worktrees/{slug}`, starts Claude Code session
- **Running**: Claude is implementing (tmux session active)
- **Review**: PR is opened, awaiting review. Can move back to Running for changes
- **Done**: PR merged, cleanup worktree/branch/tmux session

### Database Storage
All databases stored centrally (not in project directories):
- macOS: `~/Library/Application Support/agtx/`
- Linux: `~/.config/agtx/`

Structure:
- `index.db` - Global project index
- `projects/{hash}.db` - Per-project task database (hash of project path)

### Tmux Architecture
- All agent sessions run in tmux server named `agents` (`tmux -L agents`)
- Session naming: `task-{id}--{project}--{slug}`
- Separate from user's regular tmux sessions

## Keyboard Shortcuts (Board Mode)

| Key | Action |
|-----|--------|
| `h/l` | Move between columns |
| `j/k` | Move between tasks |
| `o` | Create new task |
| `Enter` | Open task shell popup |
| `i` | Edit task |
| `d` | Delete task |
| `m` | Move task right (advance workflow) |
| `/` | Search tasks |
| `e` | Toggle project sidebar |
| `q` | Quit |

## Code Patterns

### Ratatui TUI
- Uses `crossterm` backend
- State separated from terminal for borrow checker: `App { terminal, state: AppState }`
- Drawing functions are static: `fn draw_*(state: &AppState, frame: &mut Frame, area: Rect)`

### Error Handling
- Use `anyhow::Result` for all fallible functions
- Use `.context()` for adding context to errors
- Gracefully handle missing tmux sessions/worktrees

### Database
- SQLite via `rusqlite` with `bundled` feature
- Migrations via `ALTER TABLE ... ADD COLUMN` (ignores errors if column exists)
- DateTime stored as RFC3339 strings

## Building

```bash
cargo build --release
```

Dependencies require:
- Rust 1.70+
- SQLite (bundled via rusqlite)
- tmux (runtime dependency)
- git (runtime dependency)

## Testing

```bash
cargo test
```

## Common Tasks

### Adding a new task field
1. Add field to `Task` struct in `src/db/models.rs`
2. Add column to schema and migration in `src/db/schema.rs`
3. Update `create_task`, `update_task`, `task_from_row` in schema.rs
4. Update UI rendering in `src/tui/app.rs`

### Adding a new agent
1. Add to `known_agents()` in `src/agent/mod.rs`
2. Add spawn args handling in `build_spawn_args()`
3. Add resume args if supported in `build_resume_args()`

### Adding a keyboard shortcut
1. Find the appropriate `handle_*_key` function in `src/tui/app.rs`
2. Add match arm for the new key
3. Update help/footer text if visible to user
