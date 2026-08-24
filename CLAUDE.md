# AGTX - Terminal Kanban for Coding Agents

A terminal-native kanban board for managing multiple coding agent sessions (Claude Code, Codex, Gemini, Copilot, OpenCode, Cursor, Grok, Antigravity) with isolated git worktrees.

## Quick Start

```bash
# Build
cargo build --release

# Run in a git project directory
./target/release/agtx

# Or run in dashboard mode (no git project required)
./target/release/agtx -g

# Enable experimental features (orchestrator agent)
./target/release/agtx --experimental

# Never run init_script / cleanup_script from project or plugin config
./target/release/agtx --no-init-scripts

# Trust the current project's config (enables its scripts and copy_files)
./target/release/agtx trust

# Run the MCP server (global mode, or project-scoped with a path)
./target/release/agtx mcp-serve [path]

# Record an agent lifecycle event (invoked by agent hooks, not by hand)
./target/release/agtx hook <task-id> <worktree> [agent] < payload.json
```

Logs are written as JSON to `~/.config/agtx/logs/agtx.log` (daily rotation, `RUST_LOG` respected).

## Architecture

```
src/
├── main.rs           # Entry point, CLI arg parsing, AppMode enum, FeatureFlags
├── lib.rs            # Module exports, AppMode, FeatureFlags
├── skills.rs         # Skill constants, agent-native paths, plugin command translation
├── tui/
│   ├── mod.rs        # Re-exports
│   ├── app.rs        # Main App struct, event loop, rendering (largest file)
│   ├── app_tests.rs  # Unit tests for app.rs (included via #[path])
│   ├── board.rs      # BoardState - kanban column/row navigation
│   ├── dep_graph.rs  # Pure dependency-graph model (topological levels, unblocked nodes)
│   ├── input.rs      # InputMode enum for UI states
│   └── shell_popup.rs # Shell popup state, rendering, content trimming
├── db/
│   ├── mod.rs        # Re-exports
│   ├── schema.rs     # Database struct, SQLite operations
│   └── models.rs     # Task, Project, TaskStatus, PhaseStatus, TransitionRequest,
│                     # Notification, RunningAgent, AgentStatus
├── tmux/
│   ├── mod.rs        # Tmux server "agtx", session management
│   └── operations.rs # TmuxOperations trait (mockable for testing)
├── git/
│   ├── mod.rs        # is_git_repo, repo_root, current_branch, diff_stat/diff_full,
│                     # merge_branch, check_merge_conflicts, delete_branch
│   ├── worktree.rs   # Git worktree create/remove/list
│   ├── operations.rs # GitOperations trait (mockable for testing)
│   └── provider.rs   # GitProviderOperations trait (GitHub PR ops)
├── agent/
│   ├── mod.rs        # Agent struct, detection, command builders (all derived from spec.rs)
│   ├── spec.rs       # AgentSpec table — one declarative record per agent + kind enums
│   └── operations.rs # AgentOperations/CodingAgent traits (mockable)
├── mcp/
│   ├── mod.rs        # Re-exports
│   └── server.rs     # MCP server (JSON-RPC over stdio) — global and project-scoped modes
└── config/
    └── mod.rs        # GlobalConfig, ProjectConfig, MergedConfig, PhaseAgentsConfig,
                      # WorktreeConfig, ThemeConfig, WorkflowPlugin, TrustStore

skills/                # Plugin skill files — auto-discovered as /agtx:* (Claude) or @agtx:* (Codex)
├── sweep/SKILL.md     # Sweep skill — push any conversation to the board (/agtx:sweep)
└── brainstorm/SKILL.md # Brainstorm skill — free-form exploration (/agtx:brainstorm)

.claude-plugin/        # Claude Code plugin manifest
├── plugin.json        # Plugin metadata + MCP server registration
└── marketplace.json   # Makes repo discoverable via /plugin marketplace add

.codex-plugin/         # Codex plugin manifest
└── plugin.json        # Plugin metadata (skills + MCP via .mcp.json)

.mcp.json              # Shared MCP server config (used by Codex plugin)

plugins/               # Bundled plugin configs (embedded at compile time)
├── agtx/
│   ├── plugin.toml    # Default workflow with skills and prompts
│   └── skills/        # Builtin skills embedded via include_str! in src/skills.rs:
│       ├── research.md, plan.md, execute.md, review.md
│       ├── orchestrate.md      # Orchestrator agent skill (experimental)
│       └── merge-conflicts.md  # Auto merge-conflict resolution skill
├── agtx-terse/
│   ├── plugin.toml    # Token-efficient variant of agtx workflow
│   └── skills/        # Terse skill overrides with brevity directive
├── gsd/plugin.toml    # Get Shit Done workflow
├── spec-kit/plugin.toml # GitHub spec-kit workflow
├── openspec/plugin.toml # OpenSpec specification framework
├── bmad/plugin.toml   # BMAD Method - AI-driven agile development
├── superpowers/plugin.toml # Superpowers - brainstorming, plans, TDD, subagent-driven dev
├── oh-my-claudecode/plugin.toml # oh-my-claudecode - multi-agent orchestration
├── agent-skills/plugin.toml # Agent Skills - spec-to-ship engineering skills
└── void/plugin.toml   # Plain agent session, no prompting

tests/
├── db_tests.rs        # Database, model, and dependency-graph tests
├── config_tests.rs    # Configuration tests
├── board_tests.rs     # Board navigation tests
├── git_tests.rs       # Git worktree tests
├── agent_tests.rs     # Agent detection and spawn args tests
├── agent_parity_tests.rs # Per-agent behaviour lock (launch/resume/skill paths/syntax)
├── hook_status_tests.rs  # Agent lifecycle-hook status reporting
├── mcp_tests.rs       # MCP server tests
├── mock_infrastructure_tests.rs # Mock infrastructure tests
└── shell_popup_tests.rs         # Shell popup logic tests

benchmark/             # SWE-bench harness
docker/                # Container images for sandboxed runs
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

- **Backlog**: Task ideas, not started. Also hosts the optional **Research** phase (`R`) — the research session runs in place, so a Backlog task can already have a worktree and tmux window. There is no separate `Research` status: `TaskStatus` is `Backlog | Planning | Running | Review | Done`, and Backlog's display name is `backlog/research`.
- **Planning**: Creates git worktree at `{worktree_dir}/{slug}` (default `.agtx/worktrees/{slug}`, configurable via `worktree_dir`), copies configured files, runs init script, deploys skills, starts agent in planning mode
- **Running**: Agent is implementing (sends execute command/prompt)
- **Review**: Optionally create PR. Tmux window stays open. Can resume to address feedback
- **Done**: Cleanup worktree + tmux window (branch kept locally). Runs the project `cleanup_script` before removal

Backlog tasks can also skip straight to Running (`M`), and Running can be sent back to Planning (`r`).

### Workflow Plugins
Plugins customize the task lifecycle per phase. A plugin is a TOML file (`plugin.toml`) that defines:
- **commands**: Slash commands sent to the agent at each phase (auto-translated per agent). Supports `preresearch` (one-time setup) and `research` (default research command).
- **prompts**: Task content templates with `{task}`, `{task_id}`, and `{phase}` placeholders
- **artifacts**: File paths that signal phase completion (supports `*` wildcards and `{phase}` placeholder)
- **prompt_triggers**: Text patterns to wait for in tmux before sending prompts
- **init_script**: Shell command run in worktree before agent starts (`{agent}` placeholder)
- **copy_dirs**: Extra directories to copy from project root into worktrees
- **copy_files**: Individual files to copy from project root into worktrees (merged with project-level `copy_files`)
- **copy_back**: Files/dirs to copy from worktree back to project root when a phase completes
- **cyclic**: When true, enables Review → Planning transition with incrementing phase counter (`p` on the board)
- **clear_context_on_advance**: When true, send an agent-specific clear-context command before the phase skill/prompt on transitions (only Claude Code's `/clear` is honored today)
- **supported_agents**: Agent whitelist (empty = all supported)
- **auto_dismiss**: Rules to auto-dismiss interactive prompts before sending the task prompt

Phase gating is derived from the config: if a phase's command or prompt contains `{task}`, the phase can be entered directly from Backlog. Otherwise, it requires a prior phase artifact. If a phase has no command AND no prompt (e.g. void plugin), it is ungated and can be entered freely. This replaces the old `research_required` flag — all behavior is now inferred from the plugin TOML.

Plugin resolution: project-local `.agtx/plugins/{name}/` → global `~/.config/agtx/plugins/{name}/` → bundled. `load_task_plugin` falls back to bundled plugins when disk load fails, so tasks always resolve their plugin correctly even if the on-disk copy is missing.

Plugin discovery for pickers: `discover_custom_plugins` (in `src/skills.rs`) scans the global then project-local plugins directories and surfaces on-disk plugins alongside `BUNDLED_PLUGINS` in both the board selector (`P`) and the task creation wizard. Project-local plugins shadow global ones by name; names colliding with a bundled plugin are skipped (the bundled entry already represents them, and `load` resolves the on-disk copy). Both pickers filter discovered plugins by `supported_agents` against the default agent.

Each task stores its plugin name explicitly in the database at creation time (e.g. `Some("agtx")`, `Some("gsd")`). Switching the project plugin only affects new tasks.

### Skill System
Skills are markdown files with YAML frontmatter deployed to agent-native discovery paths in worktrees:
- Claude: `.claude/commands/agtx/plan.md`
- Gemini: `.gemini/commands/agtx/plan.toml` (converted to TOML format)
- Codex: `.codex/skills/agtx-plan/SKILL.md`
- Cursor: `.cursor/skills/agtx-plan/SKILL.md`
- Grok: `.grok/skills/agtx-plan/SKILL.md`
- Antigravity: `.agents/skills/agtx-plan/SKILL.md` (vendor-neutral tree, not an agent dotdir)
- OpenCode: `.opencode/command/agtx-plan.md` (frontmatter stripped)
- Copilot: `.github/agents/agtx/plan.md`

Canonical copy always at `.agtx/skills/agtx-plan/SKILL.md`.

`write_skills_to_worktree()` also drops a **per-agent MCP config** into the worktree so the task's agent can reach the project-scoped MCP server (`agtx mcp-serve <project>`). Both the filename and the format vary:

| Agent | File | Format |
|-------|------|--------|
| claude | `.mcp.json` | JSON, `mcpServers` |
| codex | `.codex/config.toml` | TOML, `[mcp_servers.agtx]` |
| gemini | `.gemini/settings.json` | JSON, `mcpServers` + `trust: true` |
| cursor | `.cursor/mcp.json` | JSON, `mcpServers` |
| grok | `.grok/config.toml` | TOML, `[mcp_servers.agtx]` |
| antigravity | `.agents/mcp_config.json` | JSON, `mcpServers` |
| opencode | opencode config | JSON, `mcp` |

Two writers **merge** instead of overwriting, because their file may already be tracked in the repo: grok appends `[mcp_servers.agtx]` to any existing `.grok/config.toml`, and antigravity parses `.agents/mcp_config.json`, inserts `mcpServers.agtx`, and writes it back — preserving other servers and top-level sibling keys. (`.agents/` is vendor-neutral, so a project is more likely to ship one.)

Two agents need an extra trust side-effect to avoid an interactive dialog on first open: Claude gets `.claude/settings.local.json` with `enableAllProjectMcpServers: true`, and Codex gets a `[projects."<worktree>"] trust_level = "trusted"` entry appended to the user's global `~/.codex/config.toml`. Both writes resolve the home directory through `agent_trust_home()`, which honours `AGTX_AGENT_HOME` so the test suite does not append temp-dir trust entries to the real user's config.

### MCP pre-handshake filter
`agtx mcp-serve` does not hand raw stdio to rmcp. Antigravity probes every stdio server with a custom `server/discover` request *before* `initialize`, and rmcp treats a non-`initialize` first message as fatal (`ExpectedInitializeRequest`) and exits — so the follow-up `initialize` hits a closed pipe. `src/mcp/prehandshake.rs` answers pre-handshake requests with JSON-RPC `-32601` and keeps the connection open; once `initialize` is forwarded it is a pass-through. Two background tasks pump real stdin/stdout through in-memory duplex pipes (`filtered_stdio()` in `src/mcp/server.rs`).

Commands are written once in canonical format (`/ns:command`) and auto-translated:
- Claude/Gemini: `/ns:command` (unchanged)
- OpenCode/Cursor/Grok/Antigravity: `/ns-command` (colon → hyphen, slash kept)
- Codex: `$ns-command` (slash → dollar, colon → hyphen)
- Copilot: no interactive skill invocation (prompt only, no commands sent)

### Sending Skills & Prompts to Agents
Prompt delivery has **two lanes**. Getting this wrong is the usual cause of "the agent started but never got the task".

**Launch lane — the first message of a task's life.** The skill command and prompt are composed by `compose_launch_text()` and handed to the process in **argv**, so the agent starts with the task already in hand: no readiness polling, no window in which a keystroke can be dropped. Gated by `spec::can_launch_with_prompt()`, which requires both an agent whose launch form is *verified* (`AgentSpec::launch_prompt_verified` — Claude only today) and a prompt under `MAX_LAUNCH_PROMPT_BYTES` (128 KiB; `execve` counts bytes). Anything else falls through to the mid-session lane. `setup_task_worktree` returns `(target, launched_with_prompt)` and its callers skip the send entirely when true.

Two consequences worth knowing: `resolve_skill_command(collapse: false)` is used here, so `{task}` keeps its paragraphs and lists (the typed path must flatten them); and `spec::normalize_prompt()` strips NUL, `\r` and other control characters that cannot survive argv, keeping `\n` and `\t`.

**Mid-session lane — phase advances into an already-running agent.** `send_skill_and_prompt()`, three paths, because agent TUIs disagree about how a slash command plus arguments must arrive:

1. **opencode** — its picker strips arguments if the whole string is typed at once. So: send the bare command name → wait for the picker → Enter (inserts it) → send the args → Enter. Still on the typed path: it is the one flow where the text has to arrive in two pieces by design
2. **gemini / codex / cursor / antigravity** — skill + prompt combined into a *single* message delivered by **bracketed paste** (`paste_text`), then one Enter. The paste is atomic, so the old poll-until-it-renders step is gone, and the `\n\n` joining command to prompt stays literal text instead of arriving as a real Enter that submits the message half-written. Gemini executes-and-loses a separately sent prompt; Codex's `$skill` mentions are inline references that do nothing when sent standalone. **No second Enter for Codex**: its command picker opens on *typing*, not on a paste (verified against codex-cli 0.144.5), so there is nothing to dismiss
3. **everything else** (claude, copilot, grok) — the generic `match (skill_cmd, prompt_trigger)` path using `send_keys`, waiting on `prompt_triggers` between the command and the prompt when configured

`clear_context_on_advance` is applied before all three, and only for Claude (`/clear`, then poll until the pane stabilises).

**tmux send primitives** — pick by what you are sending, they are not interchangeable:

| Method | tmux | Use for |
|---|---|---|
| `paste_text` | `load-buffer` + `paste-buffer -p` | a whole message; bracketed, atomic, newlines stay literal |
| `send_text` | `send-keys -l --` | literal text; no key-name lookup |
| `send_keys_literal` | `send-keys` (no `-l`) | **key names** — `Enter`, `C-c`, dialog answers |
| `send_keys` | `send-keys` + `Enter` | text plus a submit, generic path |

Without `-l`, tmux resolves an argument that matches a key name *as that key*: `"Space"` arrives as `0x20`, `"Escape"` as `ESC`, `"Up"` as `\033[A` (tmux 3.5a). So `send_keys_literal` — despite its name — must never carry task-derived text.

### First-Launch Dialogs
Agents gate a brand-new directory behind an interactive dialog, and every task gets a brand-new
worktree — so these fire on the *first* launch of nearly every task, not just in containers.
`LAUNCH_DIALOGS` (`src/tui/app.rs`) is the table of `(patterns, answer)`; `dismiss_launch_dialog`
answers any that is visible.

| Dialog | Match | Answer | Scope |
|---|---|---|---|
| Claude workspace trust | `Yes, I trust this folder` | `1` | per directory — `projects."<dir>".hasTrustDialogAccepted` in `~/.claude.json` |
| Claude bypass-permissions warning | `Yes, I accept` / `I accept the risk` | `2` | not per-project; the `bypassPermissionsModeAccepted` preflight does **not** reliably suppress it |
| Gemini folder trust | `Do you trust the files in this folder?` | `1` | answering it restarts the process |

It runs in **both** `wait_for_agent_ready` loops *and* the session-refresh loop: the readiness
budget expires after ~60s and a slow agent can render its dialog later than that. A retry only
happens while the pane is **unchanged** — a redraw means the answer landed, and resending would
type a stray digit into the agent's live composer.

Missing an arm is silent and total: the task's prompt is delivered but never read, because the
agent never reaches its composer. Antigravity's trust dialog is deliberately *not* handled — see
the note in the agent's own section.

### Session Persistence
- Tmux window stays open when moving Running → Review
- Resume from Review simply changes status back to Running (window already exists)
- No special resume logic needed - the session just stays alive in tmux

### Database Storage
All databases stored centrally (not in project directories), in the platform data dir (`GlobalConfig::data_dir`, via the `directories` crate):
- macOS: `~/Library/Application Support/agtx/`
- Linux: `~/.local/share/agtx/`

Config paths are split across two roots — watch out when adding new files:
- `GlobalConfig::config_path()` / `WorkflowPlugin::global_plugins_dir()` build from `$HOME` directly, so `config.toml`, `plugins/`, and `logs/` are always at `$HOME/.config/agtx/` on **every** platform
- `TrustStore::path()` uses `directories`' `config_dir()`, so `trusted_projects.toml` lands in `~/Library/Application Support/agtx/` on macOS and `~/.config/agtx/` on Linux

On first run, a `config.toml` at the old `directories`-derived location is migrated to `$HOME/.config/agtx/` automatically.

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

### Orchestrator Agent (Experimental)
A dedicated Claude Code agent that autonomously manages the kanban board. Enabled with `--experimental`, toggled with `O`.

```
┌─────────────┐     MCP (stdio)     ┌──────────────┐     SQLite     ┌─────┐
│ Orchestrator │ ←──────────────────→ │  MCP Server  │ ←────────────→ │ DB  │
│ (Claude Code)│                     │(agtx mcp-serve)│             └──┬──┘
└──────┬───────┘                     └──────────────┘                  │
       │  send_keys (push-when-idle)                                   │
┌──────┴───────┐                                                       │
│   TUI (agtx) │ ←────────────────────────────────────────────────────┘
└──────────────┘
```

- **Orchestrator → TUI**: `transition_requests` DB table (commands like "move task X forward")
- **TUI → Orchestrator**: `notifications` DB table, pushed via `send_keys` when orchestrator is idle
- MCP registered per-session via `claude mcp add-json --scope local`, cleaned up on exit. It registers as **`agtx-orchestrator`**, not `agtx`, so it can't collide with an `agtx` server defined in another scope (the Claude Code plugin, `~/.claude.json`, or the repo's `.mcp.json`) — a same-named server with a different endpoint makes Claude Code report a config conflict and refuse to connect. A stale registration is removed first so `add-json` can't fail with "already exists" and short-circuit the `&&`
- Only Claude implements `build_orchestrator_command()`; every other agent falls through to a plain interactive session with no MCP wiring
- Orchestrator only manages Planning and Running phases; the user triages Backlog/Research manually and handles merging in Review/Done
- Orchestrator is a coordinator, not a reviewer — it moves tasks forward immediately when phases complete, without inspecting output
- Only "completed phase" notifications are sent (no "entered phase" notifications)
- On startup, if an orchestrator tmux session already exists, it is detected and reconnected; catch-up notifications are created for tasks that completed phases while the TUI was down (deduplicated via `peek_notifications`)

**MCP tools**:
- Discovery: `list_projects` (global mode only)
- Read: `list_tasks`, `get_task` (includes `allowed_actions`), `get_transition_status`, `check_conflicts`, `get_notifications`, `read_pane_content`
- Write: `move_task` (queues a transition request; actions `research`, `move_forward`, `move_to_planning`, `move_to_running`, `move_to_review`, `move_to_done`, `resume`, `escalate_to_user`), `send_to_task` (Planning/Running only, 4096-byte cap)
- CRUD (Backlog only for update/delete): `create_task`, `create_tasks_batch` (max 50, index-based `depends_on` wiring), `update_task`, `delete_task`

### MCP Server Modes

Two modes, selected by whether a path argument is passed to `agtx mcp-serve`:

| Mode | Command | Used by |
|------|---------|---------|
| **Project-scoped** | `agtx mcp-serve <path>` | Orchestrator (bound to one project) |
| **Global** | `agtx mcp-serve` | Sweep skill, any ad-hoc session |

In global mode all CRUD tools (`list_tasks`, `create_task`, etc.) require a `project_id` parameter. The agent calls `list_projects` first to resolve it. In project-scoped mode `project_id` is ignored — the path is fixed at startup.

`ServerMode` enum in `src/mcp/server.rs`. Path resolution via `resolve_project_path(project_id)` helper.

### General Configuration
Global config lives at `~/.config/agtx/config.toml` (`GlobalConfig`):
```toml
default_agent = "claude"
fullscreen_on_enter = false  # When true, Enter on a task attaches to tmux directly instead of opening the in-TUI popup
agent_hooks = true           # Write agent lifecycle-hook configs into worktrees (see Hook-Based Phase Status)

[agents]                     # Per-phase agent overrides (PhaseAgentsConfig)
research = "claude"
planning = "claude"
running = "codex"
review = "claude"

[worktree]                   # WorktreeConfig
enabled = true
auto_cleanup = true
base_branch = ""             # empty = auto-detect main/master
worktree_dir = ".agtx/worktrees"
branch_prefix = "task"       # "task" → task/{slug}
```

### Project Configuration
Per-project overrides live in `{project}/.agtx/config.toml` (`ProjectConfig`) and are merged over the global config via `MergedConfig::merge`:

| Field | Purpose |
|-------|---------|
| `default_agent`, `agents` | Agent / per-phase agent overrides |
| `base_branch` | Branch worktrees are cut from |
| `github_url` | Repo URL for PR operations |
| `worktree_dir` | Where worktrees are created |
| `copy_files` | Comma-separated files copied from project root into worktrees |
| `init_script` | Shell command run in the worktree after creation |
| `cleanup_script` | Shell command run in the worktree before removal |
| `workflow_plugin` | Active plugin for new tasks |
| `branch_prefix` | Branch name prefix |
| `skip_worktree` | Work directly in the project root (e.g. already-isolated Docker repos) |

### Project Trust
Project config can execute shell commands (`init_script`, `cleanup_script`) and copy files, so those three fields are stripped from an untrusted project's config at startup (`App::new`), with a warning banner.

- Trust is a **canonical path → SHA-256 of `.agtx/config.toml`** map in `TrustStore` (`trusted_projects.toml`, see path note above). Editing the project config invalidates trust and re-prompts
- A project with no `.agtx/config.toml` is trusted by default (nothing to distrust)
- An untrusted project also forces `flags.no_init_scripts = true`, which additionally suppresses **plugin** `init_script`s
- Approve via the in-TUI trust confirmation popup (any key) or `agtx trust` in the project directory
- `--no-init-scripts` suppresses `init_script` and `cleanup_script` execution regardless of trust

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
| `Ctrl+f` | Fullscreen attach to task's tmux session |
| `d` | Show git diff for task |
| `D` | Open dependency-graph overlay |
| `m` | Move task forward (advance workflow) |
| `M` | Move Backlog task straight to Running |
| `R` | Start research for a Backlog task (in place, no column change) |
| `r` | Resume: Review → Running, or Running → Planning |
| `p` | Cyclic plugins only: Review → Planning (next phase) |
| `/` | Search tasks (jumps to and opens task) |
| `P` | Select workflow plugin |
| `O` | Toggle orchestrator agent (experimental) |
| `e` | Toggle project sidebar (`h`/`Left` from the Backlog column focuses it) |
| `q` | Quit |

### Dashboard Mode (`agtx -g`, or launched outside a git repo)
| Key | Action |
|-----|--------|
| `p` | Open the indexed-project list |
| `n` | Adopt the current directory as a project (must be a git repo) and open it |
| `j/k` or arrows | Navigate the project list |
| `Enter` | Open the selected project |
| `Esc` | Close the project list |
| `q` | Quit |

### Dependency Graph Overlay (`D`)
| Key | Action |
|-----|--------|
| `h/j/k/l` or arrows | Move between nodes / levels |
| `Space` | Toggle mark on an unblocked node |
| `a` | Mark all unblocked nodes |
| `c` | Clear marks |
| `Enter` | Batch-move marked (or selected) unblocked tasks forward |
| `Esc` or `q` | Close |

### Task Popup (tmux view)
| Key | Action |
|-----|--------|
| `Ctrl+j/k` or `Ctrl+n/p` | Scroll up/down |
| `Ctrl+d/u` or `PageDown/PageUp` | Page down/up |
| `Ctrl+g` | Jump to bottom |
| `Ctrl+f` | Fullscreen attach to tmux session |
| `Ctrl+q` | Close popup |
| Other keys | Forwarded to tmux/agent (including `Esc`) |

When an escalation note is present, the first keypress only dismisses the banner and is not forwarded.

### PR Creation Popup
| Key | Action |
|-----|--------|
| `Tab` | Switch between title/description |
| `Enter` | In title: move to description. In description: newline |
| `Ctrl+s` | Create PR and move to Review (ignored while the description is still generating) |
| `Esc` | Cancel |

### Task Creation Wizard
The wizard flow is: **Title → Plugin → Prompt** (plugin step auto-skipped if ≤1 option or no agents detected).

| Key | Action |
|-----|--------|
| `j/k` or arrows | Navigate plugin list |
| `Tab` | Cycle through options |
| `Enter` | Advance to next step / save |
| `Esc` | Cancel wizard |

Agent is determined by `config.default_agent` (set via config file), not selected per-task.
Plugin defaults to the project's active plugin (set via `P` on the board).

### Task Edit (Description)
| Key | Action |
|-----|--------|
| `#` or `@` | Start file search (fuzzy find) |
| `/` | Start skill search (at start of line or after space) |
| `!` | Start task reference search (at start of line or after space) |
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
- Phase status polling runs in background thread (`maybe_spawn_session_refresh`)
- Uses `mpsc` channels to communicate results back to main thread via `try_recv()` (non-blocking)
- Loading spinners shown during async operations

### Phase Status Polling
- `maybe_spawn_session_refresh()` spawns a background thread with 2-second cache TTL per task, covering Planning/Running/Review tasks plus Backlog tasks with an active research session
- Overlap guard: only one refresh thread runs at a time (`session_refresh_rx.is_some()`)
- Thread does all expensive work: plugin TOML loading, artifact file checks, `tmux capture-pane`, copy-back side effects
- `apply_session_refresh()` applies results on main thread (non-blocking `try_recv`)
- Idle detection (Working → Idle) handled on main thread using `pane_content_hashes` timestamps — **only for tasks with no hook report**, see below
- Five states (`PhaseStatus` in `src/db/models.rs`): Working (spinner), Blocked (bold `?`, agent-reported only), Idle (pause icon, 15s no output), Ready (checkmark), Exited (no window)
- Phase artifact paths come from the task's plugin or agtx defaults
- Plugin instances cached per task in `HashMap<Option<String>, Option<WorkflowPlugin>>` to avoid repeated disk reads

### Hook-Based Phase Status
Agents that support lifecycle hooks report their own state instead of agtx guessing it from pane
output. Design notes: `docs/planning/hook-based-phase-status.md`.

```
Claude Code ──hook──► agtx hook <task-id> <worktree> ──► {worktree}/.agtx/status/{task_id}.json
                                                                   │
                                        maybe_spawn_session_refresh ┘──► apply_session_refresh
```

- **Writing the config**: `write_skills_to_worktree()` merges a `hooks` block into the worktree's
  `.claude/settings.local.json` (the same file that carries `enableAllProjectMcpServers`). Built by
  `claude_hook_settings()`. Gated on `config.agent_hooks`; Claude-only today
- **The Claude settings writer merges, it does not overwrite.** `.claude` is in
  `AGENT_CONFIG_DIRS`, so a project shipping its own `settings.local.json` has it copied into every
  worktree — a plain write would drop the user's `permissions`/`env`/`hooks`. `merge_claude_hooks()`
  keeps user entries on the same events and replaces only agtx's own (matched by the agtx binary
  path in the command), so redeploying is idempotent instead of accumulating duplicate hooks. Same
  merge-don't-overwrite rule as the grok and antigravity MCP writers
- **The hook command is task-agnostic**: `agtx hook --env claude`. The task is read from
  `AGTX_TASK_ID` / `AGTX_WORKTREE`, set on the tmux **window** by `create_window` (tmux `-e`, so a
  resumed or switched agent inherits them). Baking the task id into the command breaks
  `skip_worktree`, where every task shares the project's `.claude/settings.local.json` and the last
  deploy would re-point every other task's agent at its own status file. The hook exits silently
  when the variables are absent, so a future user-global registration stays inert outside agtx, and
  when `CLAUDE_JOB_DIR` is set, since a backgrounded task inherits its parent's env
- **Registered events** (verified against Claude Code 2.1.241): `SessionStart`, `UserPromptSubmit`,
  `PreToolUse` (heartbeat) → `working`; `PermissionRequest`, `Notification` → `blocked`; `Stop`,
  `StopFailure` → `waiting`; `SessionEnd` → `ended`. Unregistered names are ignored by the agent
- **`src/agent/hook_status.rs`** is pure (no tmux/DB/TUI types): event mapping, atomic
  write-then-rename, staleness, and `merge_event`'s guard preventing a late `PreToolUse` from
  clearing a fresh `Blocked`
- **Precedence** in the refresh thread: artifact → `Ready` > window gone → `Exited` > hook status >
  pane-hash heuristic. Only *liveness* is replaced; artifact detection is untouched
- **Purely additive**: no status file means the pre-existing 15s pane-hash heuristic runs unchanged.
  A `working` record older than `HOOK_STALE_SECS` (300s) is distrusted and also falls back
- The pane is **not** captured when a hook report exists — one fewer `capture-pane` per task per
  refresh. Codex's MCP approval auto-dismiss runs outside that branch so it fires either way
- **Consumers**: `Blocked` fires the orchestrator stuck-task notification *immediately* (with the
  reason text from `blocked_reasons`) rather than after 60s of `Idle`; the merge-conflict trigger
  skips `Blocked` entirely. MCP `get_task` exposes `agent_state` + `blocked_reason`, read straight
  from the file — it works cross-process precisely because it is a file, not TUI state
- **Binary-path drift**: the absolute `agtx` path from `current_exe()` is baked into the hook command
  *and* every MCP config a worktree gets, so moving or reinstalling agtx would silently break both
  for worktrees created earlier. `write_skills_to_worktree` records the deploying binary in
  `.agtx/deployed-by`; `refresh_stale_worktree_configs` re-deploys mismatched worktrees on a
  background thread at startup. `is_agtx_hook_command` matches on the `" hook --env"` invocation
  rather than the binary path so the merge still recognises its own entries across a move
- Not yet wired for Codex/Gemini/Cursor: those read hooks from user-global paths
  (`~/.codex/hooks.json`, `~/.gemini/settings.json`, `~/.cursor/hooks.json`) and need an
  install/uninstall lifecycle plus `cwd`-based routing

### Task References & Dependencies
- In description input, type `!` (at start of line or after space) to search existing tasks
- Selecting a task inserts `![task-title]` and tracks the reference ID
- Referenced task IDs stored as comma-separated string in `task.referenced_tasks`
- References double as **dependencies**: `Database::deps_satisfied` returns true only when every referenced task is in Review or Done. Starting research or moving a Backlog task forward is blocked until then (a warning is shown instead)
- `src/tui/dep_graph.rs` builds a topologically-leveled `DepGraph` from `referenced_tasks` — level 0 = no in-graph deps, and a node is `unblocked` when it is in Backlog with satisfied deps. The `D` overlay renders it and can batch-move unblocked tasks. The module is free of ratatui/DB types (the caller passes a `deps_satisfied` closure), so it is unit-testable in isolation
- MCP `create_tasks_batch` wires the same dependencies via 0-based `depends_on` indices
- At worktree setup, referenced tasks' artifacts are copied to `.agtx/references/`:
  - Git diffs (`{slug}.diff`) from `git diff main..{branch}`
  - Worktree files (`.agtx/skills/`, `.planning/`) if the referenced worktree still exists

### Auto Merge-Conflict Resolution
- During `apply_session_refresh`, Review tasks are checked for merge conflicts with the default branch (main/master)
- Uses `git merge-tree --write-tree` (Git 2.38+) for a non-destructive virtual merge check — does not modify the worktree
- Triggers when a Review task becomes **newly Ready** or has been **Idle for 30+ seconds**
- If conflicts detected, sends the `/agtx:merge-conflicts` skill + prompt to the agent's tmux session
- One-shot per task: `merge_conflict_checked: HashSet<String>` guard ensures each task is only checked once
- Works with all plugins — the merge-conflicts skill is a builtin skill deployed to every worktree
- The skill instructs the agent to: commit current work → merge origin/main → resolve conflicts → review only conflicted files against both parents → run tests

### Agent Integration
- Every per-agent value below is one field of that agent's `AgentSpec` in `src/agent/spec.rs`; the
  functions here read the table rather than matching on the agent's name
- Agents spawned via `build_interactive_command()` in `src/agent/mod.rs`
- Each agent has its own flags: Claude (`--dangerously-skip-permissions`), Codex (`--sandbox workspace-write`), Gemini (`GEMINI_TRUST_WORKSPACE=true` + `--approval-mode yolo`), Copilot (`--allow-all-tools`), Cursor (`agent --yolo`), Grok (`--yolo --trust`, where `--trust` also ungates the repo-local `.grok/config.toml` MCP server and suppresses the directory-trust dialog), Antigravity (`agy --dangerously-skip-permissions --mode accept-edits` — two orthogonal controls: the flag governs shell/MCP/URL approvals, `--mode` governs the file-edit diff review, and both are needed to run unattended)
- `build_resume_command()` is the recovery variant used after a tmux/server restart — mostly `--continue` appended to the launch flags (`ResumeArgs::Append`), but Gemini uses `--resume` and Codex's `resume --last` *replaces* them (`ResumeArgs::Replace`: `codex resume` rejects `--sandbox`)
- `tests/agent_parity_tests.rs` pins every one of these strings per agent. It is written against
  behaviour, not derived from the table, so a diff there means something actually changed
- Skills deployed to agent-native paths via `write_skills_to_worktree()` in app.rs
- Commands resolved per-task via `resolve_skill_command()` (plugin command + agent transform)
- Prompts resolved per-task via `resolve_prompt()` (pure template substitution, agent-agnostic)

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
- A recent stable Rust (CI builds on `stable`; no MSRV is pinned in `Cargo.toml`)
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
Half of this is now one table entry; the other half is still per-agent match arms in `app.rs`
(migrating those is stage E of `docs/planning/agent-spec-table.md`).

**`src/agent/spec.rs`** — the declarative half
1. Add one `AgentSpec` entry to `AGENT_SPECS`: identity (name, binary, description, git co-author),
   launching (`env`, `base_args`, `prompt_form`, `resume`, `headless_args`), and skills
   (`skill_dir`, `skill_layout`, `skill_scan_dir`, `command_syntax`). Everything in
   `src/agent/mod.rs`, `src/agent/operations.rs` and `src/skills.rs` is derived from it —
   `known_agents`, both command builders, the headless invocation, the skill paths, the
   command syntax, and skill discovery
2. Declare `prompt_form` (how the CLI takes a prompt: `Argv` or `Flag("-i")`) but leave
   `launch_prompt_verified: false` until that form is checked against the real binary. A
   `-p`-style flag that runs headless and exits swallows the task text silently; flipping the bool
   is what opts the agent into the launch lane
3. If no existing `SkillLayout` / `CommandSyntax` variant fits, add **one** variant plus the arm in
   the single function that reads it — never a new match on the agent's name

**`tests/agent_parity_tests.rs`**
4. Extend every parity table with the new agent (`parity_covers_every_known_agent` fails until you
   do). These are literals on purpose: a diff there means behaviour changed

**`src/tui/app.rs`** — still per-agent, until stage E
5. Add the binary to `AGENT_COMMANDS` (pane process detection)
6. Add an activity indicator to `AGENT_ACTIVE_INDICATORS` if the agent is an Ink/Node TUI (runs inside bash)
7. Add exit command handling in `switch_agent_in_tmux()` (graceful exit cmd or Ctrl+C)
8. Add the skill-deploy branch in **both** `write_skills_to_worktree()` and `deploy_skill()` (e.g. `"codex" | "cursor" | "grok"` for SKILL.md subdirectories)
9. Add the per-agent MCP config writer in `write_skills_to_worktree()` — note the format varies (JSON vs TOML, `mcpServers` vs `mcp_servers`)
9b. If the agent supports lifecycle hooks, register them so it reports its own phase status (see *Hook-Based Phase Status*); otherwise it falls back to the pane-hash heuristic
10. Add an agent label color in the task-card footer `match task.agent.as_str()`
11. If Ink/Node TUI: add to the combined-send branch `matches!(agent_name, "gemini" | "codex" | ...)` in `send_skill_and_prompt()`; add double-Enter handling if the agent has a command picker popup

**Plugins**
12. Add the agent to `supported_agents` in any `plugins/*/plugin.toml` that whitelists agents

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
6. **cursor** - Cursor Agent CLI (binary is `agent`)
7. **grok** - xAI's Grok Build CLI
8. **antigravity** - Google's Antigravity CLI (binary is `agy`)

## Future Enhancements
- Reopen Done tasks (recreate worktree from preserved branch)
- Orchestrator: support non-Claude agents as orchestrator
- Orchestrator: task deletion notifications
- Orchestrator: multi-project support

Design notes for in-flight work live in `docs/planning/` (untracked).
