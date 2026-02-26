# AGTX - Terminal Kanban for Coding Agents

A terminal-native kanban board for managing multiple coding agent sessions (Claude Code, Codex, Gemini, Copilot, OpenCode) with isolated git worktrees.

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
├── skills.rs         # Skill constants, agent-native paths, plugin command translation
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
    └── mod.rs        # GlobalConfig, ProjectConfig, ThemeConfig, WorkflowPlugin

skills/                # Built-in skill files (embedded at compile time)
├── plan.md            # Planning phase instructions
├── execute.md         # Execution phase instructions
├── review.md          # Review phase instructions
└── research.md        # Research phase instructions

plugins/               # Bundled plugin configs (embedded at compile time)
├── gsd/plugin.toml    # Get Shit Done workflow
├── spec-kit/plugin.toml # GitHub spec-kit workflow
└── void/plugin.toml   # Plain agent session, no prompting

tests/
├── db_tests.rs        # Database and model tests
├── config_tests.rs    # Configuration tests
├── board_tests.rs     # Board navigation tests
├── git_tests.rs       # Git worktree tests
├── agent_tests.rs     # Agent detection and spawn args tests
├── mock_infrastructure_tests.rs # Mock infrastructure tests
└── shell_popup_tests.rs         # Shell popup logic tests
```

## Key Concepts

### Task Workflow
```
Backlog → Planning → Running → Review → Done
            ↓           ↓         ↓        ↓
         worktree    agent      optional  cleanup
         + agent     working    PR        (keep
         planning              (resume)   branch)
```

- **Backlog**: Task ideas, not started
- **Planning**: Creates git worktree at `.agtx/worktrees/{slug}`, copies configured files, runs init script, deploys skills, starts agent in planning mode
- **Running**: Agent is implementing (sends execute command/prompt)
- **Review**: Optionally create PR. Tmux window stays open. Can resume to address feedback
- **Done**: Cleanup worktree + tmux window (branch kept locally)

### Workflow Plugins
Plugins customize the task lifecycle per phase. A plugin is a TOML file (`plugin.toml`) that defines:
- **commands**: Slash commands sent to the agent at each phase (auto-translated per agent)
- **prompts**: Task content templates with `{task}` and `{task_id}` placeholders
- **artifacts**: File paths that signal phase completion (supports `*` wildcards)
- **prompt_triggers**: Text patterns to wait for in tmux before sending prompts
- **init_script**: Shell command run in worktree before agent starts (`{agent}` placeholder)
- **copy_dirs**: Extra directories to copy from project root into worktrees
- **supported_agents**: Agent whitelist (empty = all supported)

Plugin resolution: project-local `.agtx/plugins/{name}/` → global `~/.config/agtx/plugins/{name}/` → bundled.

Each task stores its plugin name in the database at creation time. Switching the project plugin only affects new tasks.

### Skill System
Skills are markdown files with YAML frontmatter deployed to agent-native discovery paths in worktrees:
- Claude: `.claude/commands/agtx/plan.md`
- Gemini: `.gemini/commands/agtx/plan.toml` (converted to TOML format)
- Codex: `.codex/skills/agtx-plan/SKILL.md`
- OpenCode: `.config/opencode/command/agtx-plan.md` (frontmatter stripped)
- Copilot: `.github/agents/agtx/plan.md`

Canonical copy always at `.agtx/skills/agtx-plan/SKILL.md`.

Commands are written once in canonical format (`/ns:command`) and auto-translated:
- Claude/Gemini: `/ns:command` (unchanged)
- OpenCode: `/ns-command` (colon → hyphen)
- Codex: `$ns-command` (slash → dollar, colon → hyphen)
- Copilot: no interactive skill invocation (falls back to file-path reference in prompt)

### Session Persistence
- Tmux window stays open when moving Running → Review
- Resume from Review simply changes status back to Running (window already exists)
- No special resume logic needed - the session just stays alive in tmux

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
| `P` | Select workflow plugin |
| `e` | Toggle project sidebar |
| `q` | Quit |

### Task Popup (tmux view)
| Key | Action |
|-----|--------|
| `Ctrl+j/k` or `Ctrl+n/p` | Scroll up/down |
| `Ctrl+d/u` | Page down/up |
| `Ctrl+g` | Jump to bottom |
| `Ctrl+q` or `Esc` | Close popup |
| Other keys | Forwarded to tmux/agent |

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

### Phase Status Polling
- `refresh_sessions()` runs every 100ms, checks artifact files with 2-second cache TTL
- Three states: Working (spinner), Ready (checkmark), Exited (no window)
- Phase artifact paths come from the task's plugin or agtx defaults
- Plugin instances cached per task in `HashMap<Option<String>, Option<WorkflowPlugin>>` to avoid repeated disk reads

### Agent Integration
- Agents spawned via `build_interactive_command()` in `src/agent/mod.rs`
- Each agent has its own flags: Claude (`--dangerously-skip-permissions`), Codex (`--approval-mode full-auto`), Gemini (`--approval-mode yolo`), Copilot (`--allow-all-tools`)
- Skills deployed to agent-native paths via `write_skills_to_worktree()` in app.rs
- Commands and prompts resolved per-task via `resolve_skill_command()` and `resolve_prompt()`

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
2. Add `build_interactive_command()` match arm in `src/agent/mod.rs`
3. Add agent-native skill dir in `agent_native_skill_dir()` in `src/skills.rs`
4. Add skill invocation format in `skill_invocation_command()` in `src/skills.rs`
5. Add plugin command transform in `transform_plugin_command()` in `src/skills.rs`

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

### Adding a new bundled plugin
1. Create `plugins/<name>/plugin.toml` with commands, prompts, artifacts
2. Add entry to `BUNDLED_PLUGINS` in `src/skills.rs`
3. Optionally add `supported_agents` to restrict agent compatibility

### Adding custom skills to a plugin
1. Create `plugins/<name>/skills/agtx-{phase}/SKILL.md` files
2. Skills use YAML frontmatter: `name: agtx-{phase}`, `description: ...`
3. Skills are auto-deployed to agent-native paths during worktree setup

## Supported Agents

Detected automatically via `known_agents()` in order of preference:
1. **claude** - Anthropic's Claude Code CLI
2. **codex** - OpenAI's Codex CLI
3. **copilot** - GitHub Copilot CLI
4. **gemini** - Google Gemini CLI
5. **opencode** - AI-powered coding assistant

## Future Enhancements
- Reopen Done tasks (recreate worktree from preserved branch)
- Notification when agent finishes work
