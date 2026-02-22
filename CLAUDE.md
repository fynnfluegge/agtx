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
│   ├── app_tests.rs  # Unit tests for app.rs (included via #[path])
│   ├── board.rs      # BoardState - kanban column/row navigation
│   ├── input.rs      # InputMode enum for UI states
│   └── shell_popup.rs # Shell popup state, rendering, content trimming
├── db/
│   ├── mod.rs        # Re-exports
│   ├── schema.rs     # Database struct, SQLite operations
│   └── models.rs     # Task, Project, TaskStatus enums
├── tmux/
│   ├── mod.rs        # Tmux server "agtx", session management
│   └── operations.rs # TmuxOperations trait (mockable for testing)
├── git/
│   ├── mod.rs        # is_git_repo helper
│   ├── worktree.rs   # Git worktree create/remove/list
│   ├── operations.rs # GitOperations trait (mockable for testing)
│   └── provider.rs   # GitProviderOperations trait (GitHub PR ops)
├── agent/
│   ├── mod.rs        # Agent definitions, detection, spawn args
│   └── operations.rs # AgentOperations/CodingAgent traits (mockable)
└── config/
    └── mod.rs        # GlobalConfig, ProjectConfig, ThemeConfig

tests/
├── db_tests.rs       # Database and model tests
├── config_tests.rs   # Configuration tests
├── board_tests.rs    # Board navigation tests
├── git_tests.rs      # Git worktree tests
├── mock_infrastructure_tests.rs # Mock infrastructure tests
└── shell_popup_tests.rs         # Shell popup logic tests
```

## Key Concepts

### Task Workflow
```
Backlog → Explore → Planning → Running → Review → Done
            ↓          ↓           ↓         ↓        ↓
         worktree   worktree    Claude    optional  cleanup
         + agent    + agent     working   PR/hook   (keep
         exploring  planning             (resume)   branch)
```

- **Backlog**: Task ideas, not started
- **Explore**: Creates git worktree, starts agent in exploration mode (codebase research, no changes). Use `m` to move here, `M` to skip to Planning
- **Planning**: Agent creates implementation plan. Sends planning prompt to existing session
- **Running**: Agent is implementing (sends "proceed with implementation")
- **Review**: Optionally create PR (or run `on_review` hook). Tmux window stays open. Can resume to address feedback
- **Done**: Cleanup worktree + tmux window, or run `on_done` hook (branch kept locally)

### Session Persistence
- Tmux window stays open when moving Running → Review
- Resume from Review simply changes status back to Running (window already exists)
- Dead sessions are auto-detected on task open and respawned (Claude resumes named session, other agents get fresh prompt)

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
color_selected = "#ead49a"      # Selected elements (yellow)
color_normal = "#5cfff7"        # Normal borders (cyan)
color_dimmed = "#9C9991"        # Inactive elements (dark gray)
color_text = "#f2ece6"          # Text (light rose)
color_accent = "#5cfff7"        # Accents (cyan)
color_description = "#C4B0AC"   # Task descriptions (dimmed rose)
color_column_header = "#a0d2fa" # Column headers (light blue gray)
color_popup_border = "#9ffcf8"  # Popup borders (light cyan)
color_popup_header = "#69fae7"  # Popup headers (light cyan)
```

### Per-Agent Flags
Configure CLI flags per agent in global or project config:
```toml
# ~/.config/agtx/config.toml (global)
[agent_flags]
claude = ["--dangerously-skip-permissions"]
aider = ["--no-auto-commits"]

# .agtx/config.toml (project override)
[agent_flags]
claude = []  # Use Claude's default permission system for this project
```
No default flags for any agent — users must explicitly opt-in.

### Review/Done Hooks
Customize workflow transitions in project config:
```toml
# .agtx/config.toml
on_review = "skip"                    # Skip PR prompt entirely
on_done = "scripts/post-done.sh"      # Run custom script
```
- `None` (field absent): default built-in behavior
- `"skip"`: skip built-in behavior, just move the task
- Any other string: run as shell command (with env vars AGTX_TASK_ID, AGTX_TASK_TITLE, AGTX_BRANCH_NAME, AGTX_WORKTREE_PATH, AGTX_PROJECT_PATH)

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
| `M` | Skip Explore, move Backlog → Planning directly |
| `r` | Move task backward (Review → Running, Planning → Explore) |
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
| `#` or `@` | Start file search (fuzzy find) |
| `\` + Enter | Line continuation (multi-line) |
| Arrow keys | Move cursor |
| `Alt+Left/Right` or `Alt+b/f` | Word-by-word navigation |
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
- Permission flags configured via `agent_flags` in config (no longer hardcoded)
- When `--dangerously-skip-permissions` is in agent flags, polls tmux pane for "Yes, I accept" prompt before sending acceptance
- Dead sessions auto-respawn using `claude --resume {task_id}` for Claude, fresh prompt for other agents

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

## Supported Agents

Detected automatically via `known_agents()` in order of preference:
1. **claude** - Anthropic's Claude Code CLI
2. **aider** - AI pair programming in your terminal
3. **codex** - OpenAI's Codex CLI
4. **gh-copilot** - GitHub Copilot CLI
5. **opencode** - AI-powered coding assistant
6. **cline** - AI coding assistant for VS Code
7. **q** - Amazon Q Developer CLI

## Future Enhancements
- Auto-detect Claude idle status (show spinner when working)
- Reopen Done tasks (recreate worktree from preserved branch)
- Notification when Claude finishes work
