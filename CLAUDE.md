# AGTX - Terminal Kanban for Coding Agents

A terminal-native kanban board for managing multiple coding agent sessions (Claude Code, Codex, etc.) with isolated git worktrees.

## Quick Start

```bash
# Build
cargo build --release

# Run in a git project directory
./target/release/agtx

# Or run in dashboard mode (no git project required)
./target/release/agtx -g
```

## Architecture

```
src/
├── main.rs           # Entry point, CLI arg parsing, AppMode enum
├── lib.rs            # Module exports for integration tests
├── tui/
│   ├── mod.rs        # Re-exports
│   ├── app.rs        # Main App struct, event loop, rendering (largest file)
│   ├── board.rs      # BoardState - kanban column/row navigation
│   └── input.rs      # InputMode enum for UI states
├── db/
│   ├── mod.rs        # Re-exports
│   ├── schema.rs     # Database struct, SQLite operations
│   └── models.rs     # Task, Project, TaskStatus enums
├── tmux/
│   └── mod.rs        # Tmux server "agtx", session management
├── git/
│   ├── mod.rs        # is_git_repo helper
│   └── worktree.rs   # Git worktree create/remove/list
├── agent/
│   └── mod.rs        # Agent definitions, detection, spawn args
├── config/
│   └── mod.rs        # GlobalConfig, ProjectConfig, ThemeConfig
└── operations.rs     # Traits for tmux/git operations (for testing)

tests/
├── db_tests.rs       # Database and model tests
├── config_tests.rs   # Configuration tests
├── board_tests.rs    # Board navigation tests
├── git_tests.rs      # Git worktree tests
└── workflow_tests.rs # Task workflow tests
```

## Key Concepts

### Task Workflow
```
Backlog → Planning → Running → Review → Done
            ↓           ↓         ↓        ↓
         worktree    Claude    optional  cleanup
         + Claude    working   PR        (keep
         planning             (resume)   branch)
```

- **Backlog**: Task ideas, not started
- **Planning**: Creates git worktree at `.agtx/worktrees/{slug}`, copies configured files, runs init script, starts Claude Code in planning mode
- **Running**: Claude is implementing (sends "proceed with implementation")
- **Review**: Optionally create PR. Tmux window stays open. Can resume to address feedback
- **Done**: Cleanup worktree + tmux window (branch kept locally)

### Session Persistence
- Tmux window stays open when moving Running → Review
- Resume from Review simply changes status back to Running (window already exists)
- No special Claude resume logic needed - the session just stays alive in tmux

### Database Storage
All databases stored centrally (not in project directories):
- macOS: `~/Library/Application Support/agtx/`
- Linux: `~/.config/agtx/`

Structure:
- `index.db` - Global project index
- `projects/{hash}.db` - Per-project task database (hash of project path)

### Tmux Architecture
```
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
```

- **Server**: Dedicated tmux server named `agtx` (`tmux -L agtx`)
- **Sessions**: Each project gets its own session (named after project)
- **Windows**: Each task gets its own window within the project's session
- Separate from user's regular tmux sessions
- View sessions: `tmux -L agtx list-windows -a`
- Attach: `tmux -L agtx attach`

### Theme Configuration
Colors configurable via `~/.config/agtx/config.toml`:
```toml
[theme]
color_selected = "#FFFF99"      # Selected elements (light yellow)
color_normal = "#00FFFF"        # Normal borders (cyan)
color_dimmed = "#666666"        # Inactive elements (gray)
color_text = "#FFFFFF"          # Text (white)
color_accent = "#00FFFF"        # Accents (cyan)
color_description = "#E8909C"   # Task descriptions (rose)
color_popup_border = "#00FF00"  # Popup borders (green)
color_popup_header = "#00FFFF"  # Popup headers (cyan)
```

## Keyboard Shortcuts

### Board Mode
| Key | Action |
|-----|--------|
| `h/l` or arrows | Move between columns |
| `j/k` or arrows | Move between tasks |
| `o` | Create new task |
| `Enter` | Open task popup (tmux view) / Edit task (backlog) |
| `x` | Delete task (with confirmation) |
| `d` | Show git diff for task |
| `m` | Move task forward (advance workflow) |
| `r` | Resume task (Review → Running) |
| `/` | Search tasks (jumps to and opens task) |
| `e` | Toggle project sidebar |
| `q` | Quit |

### Task Popup (tmux view)
| Key | Action |
|-----|--------|
| `Ctrl+j/k` or `Ctrl+n/p` | Scroll up/down |
| `Ctrl+d/u` | Page down/up |
| `Ctrl+g` | Jump to bottom |
| `Ctrl+q` or `Esc` | Close popup |
| Other keys | Forwarded to tmux/Claude |

### PR Creation Popup
| Key | Action |
|-----|--------|
| `Tab` | Switch between title/description |
| `Ctrl+s` | Create PR and move to Review |
| `Esc` | Cancel |

### Task Edit (Description)
| Key | Action |
|-----|--------|
| `#` | Start file search (fuzzy find) |
| `\` + Enter | Line continuation (multi-line) |
| Arrow keys | Move cursor |
| `Home/End` | Jump to start/end |

## Code Patterns

### Ratatui TUI
- Uses `crossterm` backend
- State separated from terminal for borrow checker: `App { terminal, state: AppState }`
- Drawing functions are static: `fn draw_*(state: &AppState, frame: &mut Frame, area: Rect)`
- Theme colors accessed via `state.config.theme.color_*`

### Error Handling
- Use `anyhow::Result` for all fallible functions
- Use `.context()` for adding context to errors
- Gracefully handle missing tmux sessions/worktrees

### Database
- SQLite via `rusqlite` with `bundled` feature
- Migrations via `ALTER TABLE ... ADD COLUMN` (ignores errors if column exists)
- DateTime stored as RFC3339 strings

### Background Operations
- PR description generation runs in background thread
- PR creation runs in background thread
- Uses `mpsc` channels to communicate results back to main thread
- Loading spinners shown during async operations

### Claude Integration
- Uses `--dangerously-skip-permissions` flag
- Polls tmux pane for "Yes, I accept" prompt before sending acceptance

## Building & Testing

```bash
# Build
cargo build --release

# Run tests
cargo test

# Run tests with mock support
cargo test --features test-mocks
```

Dependencies require:
- Rust 1.70+
- SQLite (bundled via rusqlite)
- tmux (runtime dependency)
- git (runtime dependency)
- gh CLI (for PR operations)
- claude CLI (Claude Code)

## Common Tasks

### Adding a new task field
1. Add field to `Task` struct in `src/db/models.rs`
2. Add column to schema and migration in `src/db/schema.rs`
3. Update `create_task`, `update_task`, `task_from_row` in schema.rs
4. Update UI rendering in `src/tui/app.rs`

### Adding a new theme color
1. Add field to `ThemeConfig` in `src/config/mod.rs`
2. Add default function and update `Default` impl
3. Use `hex_to_color(&state.config.theme.color_*)` in app.rs

### Adding a new agent
1. Add to `known_agents()` in `src/agent/mod.rs`
2. Add spawn args handling in `build_spawn_args()`
3. Add resume args if supported in `build_resume_args()`

### Adding a keyboard shortcut
1. Find the appropriate `handle_*_key` function in `src/tui/app.rs`
2. Add match arm for the new key
3. Update help/footer text if visible to user

### Adding a new popup
1. Add state struct (e.g., `MyPopup`) in app.rs
2. Add `Option<MyPopup>` field to `AppState`
3. Initialize to `None` in `App::new()`
4. Add rendering in `draw_board()` function
5. Add key handler function `handle_my_popup_key()`
6. Add check in `handle_key()` to route to handler

## Planning Docs

See `docs/planning/` for future enhancements:
- `codex-cli-integration.md` - Plan for Codex CLI agent support
- `dependency-injection-testing.md` - Plan for improved test coverage
- `tmux-popup-focus-bug.md` - Known issue with tmux attachment

## Future Enhancements
- Auto-detect Claude idle status (show spinner when working)
- Reopen Done tasks (recreate worktree from preserved branch)
- Support for additional agents (Aider, Codex, GitHub Copilot CLI)
- Notification when Claude finishes work
