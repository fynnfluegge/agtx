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

# Serve the board to a phone (feature = "serve"; loopback needs no pairing)
./target/release/agtx serve [path] [--host 0.0.0.0] [--port 8787] [--tunnel private|public]
./target/release/agtx serve --devices          # paired devices
./target/release/agtx serve --revoke <id>      # or --revoke-all

# Record an agent lifecycle event (invoked by agent hooks, not by hand)
./target/release/agtx hook --env <agent> [--event <Name>] < payload.json

# Version, and self-update to the latest GitHub release
./target/release/agtx --version
./target/release/agtx update [--check]
```

Logs are JSON at `~/.config/agtx/logs/agtx.log` (daily rotation, `RUST_LOG` respected).

## Architecture

```
src/
├── main.rs           # Entry point, CLI arg parsing, AppMode, FeatureFlags
├── lib.rs            # Module exports
├── skills.rs         # Skill constants, agent-native paths, plugin command translation
├── tui/
│   ├── app.rs        # Main App struct, event loop, rendering (largest file)
│   ├── board.rs      # BoardState - kanban column/row navigation
│   ├── config_editor.rs # In-TUI config form: declared fields, two matches
│   ├── dep_graph.rs  # Pure dependency-graph model (topological levels, unblocked nodes)
│   ├── help.rs       # The `?` overlay's binding table (declared)
│   ├── input.rs      # InputMode enum
│   ├── serve_control.rs # The `W` overlay: run `agtx serve` as a child, manage devices
│   ├── shell_popup.rs # Shell popup state, rendering, content trimming
│   ├── text_input.rs # Shared line editor: buffer + byte caret, motion, deletion
│   ├── wizard.rs     # Task create/edit wizard state
│   └── *_tests.rs    # Unit tests included via #[path]
├── db/{schema.rs (SQLite ops), models.rs (Task, Project, TaskStatus, PhaseStatus, ...)}
├── tmux/
│   ├── mod.rs        # Tmux server "agtx", session management
│   ├── operations.rs # TmuxOperations trait (mockable)
│   ├── input.rs      # PaneInput/PaneInputSink, the input broker, both backends
│   └── control.rs    # Persistent `tmux -C` client, frame parser, command encoder
├── git/{mod.rs (repo/branch/diff/merge/conflicts), worktree.rs, operations.rs, provider.rs}
├── agent/
│   ├── mod.rs        # Agent struct, detection, command builders (derived from spec.rs)
│   ├── spec.rs       # AgentSpec table — one declarative record per agent + kind enums
│   ├── hook_status.rs # Agent-reported liveness: event vocabularies, atomic status writes
│   ├── trust.rs      # Reads each agent's own workspace-trust store
│   └── operations.rs # AgentOperations/CodingAgent traits (mockable)
├── core/{actions.rs (allowed_actions + CallerKind), input.rs (submit_message, parking)}
├── mcp/server.rs     # MCP server (JSON-RPC over stdio) — global and project-scoped modes
├── web/              # `agtx serve` — the board over HTTP (feature = "serve")
│   └── mod.rs, routes.rs, writes.rs, ws.rs, state.rs, auth.rs, assets.rs, qr.rs, tunnel.rs
├── update/{mod.rs, version.rs, check.rs, github.rs, release.rs, install.rs}
└── config/mod.rs     # GlobalConfig, ProjectConfig, MergedConfig, WorkflowPlugin, TrustStore

web/                   # The mobile PWA — plain ES modules, no build step
└── index.html, app.js, api.js, ansi.js, app.css, sw.js, manifest.webmanifest, icons

skills/                # Plugin skill files — /agtx:* (Claude) or @agtx:* (Codex)
└── sweep/, brainstorm/, oneshot/ SKILL.md

.claude-plugin/, .codex-plugin/, .mcp.json   # Plugin manifests + shared MCP config

plugins/               # Bundled plugin configs (embedded at compile time)
├── agtx/ (plugin.toml + skills: research, plan, execute, review, orchestrate, merge-conflicts)
└── agtx-terse/, gsd/, spec-kit/, openspec/, bmad/, superpowers/,
    oh-my-claudecode/, agent-skills/, void/  (each plugin.toml)

tests/
├── db_, config_, board_, git_, agent_ tests.rs
├── agent_parity_tests.rs  # Per-agent behaviour lock (launch/resume/skill paths/syntax)
├── hook_status_, mcp_, update_, mock_infrastructure_, shell_popup_ tests.rs
├── tmux_control_tests.rs  # Real-tmux pane input — opt-in via AGTX_TMUX_IT=1
└── smoke/             # Per-agent smoke tests — real binaries, no mocks, opt-in
    └── agent_smoke.py, test_agent_smoke.py, agent_matrix.rs, README.md

benchmark/ (SWE-bench harness), docker/ (sandbox images)
```

## Key Concepts

### Task Workflow
```
Backlog → Planning → Running → Review → Done
```

- **Backlog**: Task ideas. Also hosts the optional **Research** phase (`R`), which runs in place. No separate `Research` status — `TaskStatus` is `Backlog | Planning | Running | Review | Done`; Backlog's display name is `backlog/research`.
- **Planning**: Creates git worktree at `{worktree_dir}/{slug}` (default `.agtx/worktrees/{slug}`), copies files, runs init script, deploys skills, starts agent.
- **Running**: Agent implementing. **Review**: optionally create PR; tmux window stays open; can resume. **Done**: cleanup worktree + tmux window (branch kept locally), runs `cleanup_script` first.
- Backlog can skip to Running (`M`); Running can go back to Planning (`r`).

**Removing a worktree** — `RealGitOps::remove_worktree` (`git/operations.rs`); all cleanup paths (Review→Done, force-move, delete `x`) go through it on a background thread. Refuses a main working tree (no-op `Ok`); `is_main_working_tree` checks the toplevel (`--show-toplevel`), not just the git dir. A failed removal prunes (`git worktree prune`) and returns `Err`. `delete_task_resources` removes the worktree independently of a branch; `delete_branch` (`git branch -D`) stays paired with having had a worktree.

**Reaping spawned processes** — `kill-window` misses processes backgrounded into their own group. `reap_task_processes` finds them by environment (`AGTX_TASK_ID` set on the window by `create_window`, inherited by children — catches daemons) and by descent from the pane (backstop, runs before `kill_window`). `processes_for_task` reads `/proc/<pid>/environ` else `ps`. A root pid (0/1) or agtx's own pid is refused. Children signalled before parents (TERM then KILL) via the `kill` command.

**agtx's own files excluded from git** — `exclude_agtx_files_from_git` writes `AGTX_WRITTEN_PATHS` into `$GIT_COMMON_DIR/info/exclude` at worktree setup (per-worktree `info/exclude` is ignored by git). Exclude patterns affect untracked files only. Makes `git add -A` safe in the phase skills.

**The phase skills commit** — `execute.md` and `review.md` end by committing (Done requires a clean tree; merging requires commits).

**A repository with no commits** — `has_no_commits` → `create_initial_commit` (`git commit --allow-empty -m init`), since a worktree must be cut from a commit. `detect_main_branch` checks the exit status of its `git rev-parse --abbrev-ref HEAD` fallback.

### Workflow Plugins
Plugins customize the lifecycle per phase. A plugin is `plugin.toml` defining: **commands** (slash commands sent per phase, auto-translated; supports `preresearch` + `research`), **prompts** (`{task}`/`{task_id}`/`{phase}` templates), **artifacts** (completion paths, `*` wildcards), **prompt_triggers** (text to wait for), **init_script** (`{agent}` placeholder), **copy_dirs**/**copy_files**/**copy_back**, **cyclic** (Review→Planning with incrementing phase, `p`), **clear_context_on_advance** (send a clear-context command before the phase skill; Claude `/clear`, pi `/new`, no-op for the rest), **supported_agents** (whitelist, empty = all), **auto_dismiss**.

Phase gating is derived from config: a phase whose command/prompt contains `{task}` can be entered directly from Backlog; otherwise it needs a prior phase artifact. A phase with no command AND no prompt (void) is ungated. No `research_required` flag.

Resolution: project-local `.agtx/plugins/{name}/` → global `~/.config/agtx/plugins/{name}/` → bundled (`load_task_plugin` falls back to bundled). `discover_custom_plugins` (`src/skills.rs`) surfaces on-disk plugins alongside `BUNDLED_PLUGINS` in the board selector (`P`) and wizard (project-local shadows global; bundled-name collisions skipped; filtered by `supported_agents`). Each task stores its plugin name explicitly; switching the project plugin only affects new tasks.

### Skill System
Skills are markdown with YAML frontmatter deployed to agent-native paths in worktrees. Canonical copy always at `.agtx/skills/agtx-plan/SKILL.md`.

| Agent | Skill path |
|-------|-----------|
| claude | `.claude/commands/agtx/plan.md` |
| gemini | `.gemini/commands/agtx/plan.toml` (TOML) |
| codex | `.codex/skills/agtx-plan/SKILL.md` |
| cursor | `.cursor/skills/agtx-plan/SKILL.md` |
| grok | `.grok/skills/agtx-plan/SKILL.md` |
| antigravity | `.agents/skills/agtx-plan/SKILL.md` (vendor-neutral) |
| opencode | `.opencode/command/agtx-plan.md` (frontmatter stripped) |
| copilot | `.github/agents/agtx/plan.md` |

`write_skills_to_worktree()` also drops a **per-agent MCP config** pointing at `agtx mcp-serve <project>`:

| Agent | File | Format |
|-------|------|--------|
| claude | `.mcp.json` | JSON, `mcpServers` |
| codex | `.codex/config.toml` | TOML, `[mcp_servers.agtx]` |
| gemini | `.gemini/settings.json` | JSON, `mcpServers` + `trust: true` |
| cursor | `.cursor/mcp.json` | JSON, `mcpServers` |
| grok | `.grok/config.toml` | TOML, `[mcp_servers.agtx]` |
| antigravity | `.agents/mcp_config.json` | JSON, `mcpServers` |
| opencode | opencode config | JSON, `mcp` |
| pi | `.pi/mcp.json` | JSON, `mcpServers` |

Four writers (grok, antigravity, gemini, pi) **merge** instead of overwriting (their file may already exist — tracked, or copied from the project root by `AGENT_CONFIG_DIRS`). Claude's `settings.local.json` side-effect merges too and gets `enableAllProjectMcpServers: true` + `skipDangerousModePermissionPrompt: true` (avoids a first-open dialog). `write_skills_to_worktree` also seeds antigravity's `trustedWorkspaces` when the project root is trusted (`agent::trust`; home lookups via `agent_trust_home()` honouring `AGTX_AGENT_HOME`).

Commands are written once canonical (`/ns:command`) and auto-translated: Claude/Gemini `/ns:command`; OpenCode/Cursor/Grok/Antigravity `/ns-command`; Codex `$ns-command`; Copilot prompt-only.

**MCP pre-handshake filter** — Antigravity probes stdio servers with `server/discover` before `initialize`, which rmcp treats as fatal. `src/mcp/prehandshake.rs` answers pre-handshake requests with JSON-RPC `-32601` and keeps the connection open; once `initialize` is forwarded it is a pass-through (`filtered_stdio()` in `src/mcp/server.rs`).

### Sending Skills & Prompts to Agents
Two lanes.

**Launch lane** (first message of a task's life, and **an agent switch**): skill + prompt composed by `compose_launch_text()` and handed to the process in **argv**. Gated by `spec::can_launch_with_prompt()`: `AgentSpec::launch_prompt_verified` (all but copilot) and a prompt under `MAX_LAUNCH_PROMPT_BYTES` (128 KiB). A **same-agent** advance cannot use it (process already running → typed lane). `create_window` nests inside `sh -c '…'` so a prompt there is quoted twice (`single_quote()` in `src/tmux/operations.rs` is the second layer; the switch path types into a running shell — one level). `resolve_skill_command(collapse: false)` keeps `{task}` paragraphs; `spec::normalize_prompt()` strips control chars. `setup_task_worktree` returns `(target, launched_with_prompt)`; callers skip the send when true.

**Mid-session lane** — `send_skill_and_prompt()`, three paths:
1. **opencode** — its picker strips arguments typed all at once. Send bare command name → wait for picker → Enter → send args → Enter.
2. **gemini / codex / cursor / antigravity / pi** — skill + prompt combined into a single message via **bracketed paste** (`paste_text`) + one Enter.
3. **everything else** (claude, copilot, grok) — generic `match (skill_cmd, prompt_trigger)` using `send_keys`, waiting on `prompt_triggers`.

**Submitting** — a bare skill command (a phase with no `{task}`, i.e. `review`) opens the composer's picker on the paste, consuming the first Enter. `submit_message()` watches the composer (presses Enter until the text is gone from the bottom `COMPOSER_TAIL_LINES`, window 14, bounded by `SUBMIT_ATTEMPTS`). Paths 1 and 2 go through `deliver_message()` (resends while the pane is unchanged, 3 attempts × 2s, stops on redraw). `clear_context_on_advance` applies before all three, only for Claude.

**tmux send primitives** (pick by what you send, not interchangeable):

| Method | tmux | Use for |
|---|---|---|
| `paste_text` | `load-buffer` + `paste-buffer -p` | a whole message; bracketed, atomic, newlines literal |
| `send_text` | `send-keys -l --` | literal text; no key-name lookup |
| `send_key` | `send-keys` (no `-l`) | key names — `Enter`, `C-c`, dialog answers |
| `send_keys` | `send-keys` + `Enter` | text plus a submit, generic path |

Without `-l`, tmux resolves an argument matching a key name as that key. `send_key` must never carry task-derived text.

### Typing into a Task Pane
The third lane — keys forwarded from an open task popup (the only lane a human waits on).

```text
crossterm key event → popup_key_input() (Char → Text, else → Key)
  → PaneInputSink::send (enqueue only, never waits for tmux; bounded channel 1024)
  → one broker thread (the single ordering authority)
      ├─ coalesces adjacent Text for the same target (only while more is queued)
      ├─ flushes before every Key, Paste, target change, popup close, shutdown
      ├─► control backend  `tmux -C attach-session`   (persistent, opt-in)
      └─► subprocess backend  `tmux send-keys`         (default, and fallback)
  popup refresh thread → PaneInputSink::capture (same queue: flush, then capture-pane + display -p)
  → ShellPopup.cached_content (drawn next frame)
```

- **No key is a process** — enqueueing is free; delivery rides a persistent control connection. **`PaneInput` is typed** — text goes out with `send-keys -l`, a key without it. **Batching never delays a key** — Enter/Escape/arrows/modified flush and go immediately; buffered text flushes as soon as the queue is empty (`DEFAULT_BATCH_WINDOW` bounds a genuine backlog). A target change flushes.
- **The control connection is on, no config field.** A failed connect / lost connection falls back to subprocess (`maybe_connect`/`drop_control`); `AGTX_TMUX_CONTROL=0` / `AGTX_TMUX_PUSH=0` turn off control / capture-push for one run. `tmux -C` attaches with `-f ignore-size,no-output`; targets must name their own session — `pane_target` guarantees `session:window`, or a bare target after a project switch hits the wrong session.
- **`tmux_quote` is not `single_quote`** — control mode parses tmux syntax (inside double quotes tmux replaces `$VAR`, `#{format}`, leading `~`, backslashes; `\ " $ # ~` escaped; a raw newline cannot be sent). **A failed control write is not replayed** (ambiguous) — dropped, connection marked dead, next request → subprocess. A full queue warns rather than reordering.
- **The popup's pane capture rides the same connection** (a `Capture` request, so it shows keys typed before it). The broker declines with no control connection (`capture_pane_for_popup` is the fallback); a failed capture keeps a healthy connection. Both commands in one round trip; `trim_content_to_cursor` returns the cursor's line index carried by `ShellPopup::cursor_line`; `FrameParser` closes a block only on a matching `%end` command id.
- **A pane with no tmux scrollback delegates scroll keys to the agent** (full-screen agents live on the alternate screen, `history_size` 0). `ShellPopup::has_scrollback()` switches to `handle_popup_scroll`; chords translated (`C-n/p` → PageUp/Down, `C-g` → End). Claude takes the alternate screen shortly after startup.
- **Pane input is never logged.** The broker's ordering authority covers popup input only; agtx's own writes go through `TmuxOperations` on their own threads.

### First-Launch Dialogs
Agents gate an unseen directory behind a dialog. `LAUNCH_DIALOGS` (`src/tui/app.rs`, from `AgentSpec::dialogs`); `dismiss_launch_dialog` answers what it is allowed to.

Mostly these do not fire (see `src/agent/trust.rs`): trust is inherited from the project root for claude/codex/gemini; cursor/grok launch with `--trust`, pi with `--approve`; **antigravity** matches trusted paths exactly (agtx seeds each worktree into `trustedWorkspaces` when the root is trusted). **`AgentDialog::security` splits the table** — trust/bypass prompts are the user's decision: with `auto_trust = false` (default) agtx detects them (card → `Blocked` with reason + fix) and leaves them unanswered; non-safety prompts are answered. An argv prompt is queued behind a dialog, not eaten; `wait_for_agent_ready` returns `None` when parked on an unanswered security dialog. `auto_trust = true` restores historical behaviour (docker/benchmark). Dialogs are matched against the running agent's own entries; `answer` is a key *sequence*.

| Agent | Dialog | Match | Answer | Scope |
|---|---|---|---|---|
| claude | workspace trust | `Yes, I trust this folder` | `1` `Enter` | Launch |
| claude | bypass-permissions | `Yes, I accept` / `I accept the risk` | `2` `Enter` | Launch — backstop |
| codex | directory trust | `Do you trust the contents of this directory?` | `1` `Enter` | Launch |
| codex | update prompt | `Update now (runs` | `2` `Enter` (Skip) | Launch |
| codex | hook review | `Hooks need review` | `3` `Enter` (Continue) | Launch |
| codex | MCP tool approval | `Allow the` + `MCP server to run tool` + `Always allow` | `3` `Enter` | Session |
| gemini | folder trust | `Do you trust the files in this folder?` | `1` `Enter` | Launch — answering restarts |
| cursor | workspace trust | `Workspace Trust Required` | `a` alone | Launch — matched on heading |
| antigravity | project trust | `Do you trust the contents of this project?` | `Enter` alone | Launch — preselected |

`require_all` distinguishes alternatives (wordings of one prompt) from conjunctions (a combination). `security` marks rows agtx will not answer unless `auto_trust`. Runs in both `wait_for_agent_ready` loops and the session-refresh loop; a retry only happens while the pane is unchanged. Some prompts must stay unanswered but declared (gemini's OAuth continue).

### Session Persistence
Tmux window stays open when moving Running → Review. Resume from Review changes status back to Running (window already exists) — no special resume logic.

### Self-Update
agtx tells the user when a newer release exists and replaces its own binary on request.

```
startup → background thread → curl api.github.com/…/releases/latest
  → cache 24h → ~/.config/agtx/update.json
  → mpsc → event loop try_recv → header "⬆ 0.2.8 [u]" → [u] popup → install_release()
```

- The binary knows its own version via `env!("CARGO_PKG_VERSION")`; `release.yml`'s *Tag matches Cargo.toml* step fails the build on drift. `--version`/`-V`/`version` and `update` are in the early fast path in `main.rs`. Cache: 24h TTL, still served offline; path from `GlobalConfig::config_path()`'s parent. `curl`, not an HTTP crate (`src/update/github.rs`). Failure is always a missing notice, never an error.
- **The swap**: `rename(target, target.old)` → `rename(new, target)` → unlink, staged inside the target's own directory (same-filesystem; renaming over a running binary is legal on Unix). Replacing in place keeps worktrees valid (the absolute `agtx` path is baked into every hook command + MCP config). A package-managed binary (`/nix/store/…`, Homebrew) is refused. Never automatic — opt out with `update_check = false` or `AGTX_NO_UPDATE_CHECK=1`.
- **Three files must agree on artifact naming** — `src/update/release.rs`, `install.sh`, `release.yml`; `tests/update_tests.rs` greps and asserts. `release.yml` publishes `<archive>.sha256`; `install.sh` verifies it.

### Serving the Board to a Phone
`agtx serve` (feature = `serve`) is the MCP server re-exposed over HTTP with a PWA — talks to SQLite/tmux/git, never to `App`. `W` runs it as a child and shows the pairing QR.

- **Actions queue; they do not execute.** A tap writes to `transition_requests`; only a running TUI drains it. Every action response says `queued`, carries `tui_connected`, phone shows a banner when nothing drains.
- **Loopback needs no credential; anything wider does** (off-loopback, including a tunnel, needs a paired device — `ServeOptions::is_loopback` reads the tunnel too). **Per-device tokens, hashed at rest** in `mobile_devices`; `--revoke` is immediate + cross-process.
- **Serving is per-session; the pairing is not.** A pairing outlives the server (device row in global `index.db`, token in the phone's `localStorage`); revocation is manual. Secrets reach the phone in the URL fragment; `/ws` uses `Sec-WebSocket-Protocol`. Auth in the router middleware, not the socket handler.
- **No bundler** (plain ES modules via `include_bytes!` — a missing file is a compile error). **No xterm.js** (`capture-pane -p -e` emits only `ESC[…m`; `web/ansi.js` builds a DOM fragment so `<script>` is text). **ratatui does not interpret ANSI** — the `W` overlay uses `qr::grid` + styled spans, `qr::render` is the CLI banner. **The board does not poll** — fetched when something happened; the live pane keeps its socket, sends only changed frames.

### Database Storage
Databases stored centrally in the platform data dir (`GlobalConfig::data_dir`, via `directories`): macOS `~/Library/Application Support/agtx/`, Linux `~/.local/share/agtx/`. Config paths split across two roots: `GlobalConfig::config_path()` builds from `$HOME`, so `config.toml`/`plugins/`/`logs/` are always at `$HOME/.config/agtx/`; `TrustStore::path()` uses `directories`' `config_dir()`, so `trusted_projects.toml` follows the platform. On first run a `config.toml` at the old location is migrated. Structure: `index.db` (global project index), `projects/{hash}.db` (per-project, hash of project path).

### Tmux Architecture
Dedicated tmux server named `agtx` (`tmux -L agtx`). Each project gets its own session (named after project); each task gets its own window in that session. Separate from the user's regular tmux. View: `tmux -L agtx list-windows -a`; attach: `tmux -L agtx attach`.

### Orchestrator Agent (Experimental)
A dedicated Claude Code agent that autonomously manages the board. Enabled with `--experimental`, toggled with `O`.

```
Orchestrator (Claude Code) ←MCP stdio→ MCP Server (agtx mcp-serve) ←SQLite→ DB
Orchestrator → TUI: transition_requests table.  TUI → Orchestrator: notifications (send_keys when idle)
```

- MCP registered per-session via `claude mcp add-json --scope local` as **`agtx-orchestrator`** (not `agtx`, avoids config conflict), cleaned up on exit. Only Claude implements `build_orchestrator_command()`. Manages Planning/Running; the user triages Backlog/Research and handles merging. A coordinator, not a reviewer. Only "completed phase" notifications sent; on startup an existing session is reconnected with catch-up notifications (deduped via `peek_notifications`).

**MCP tools**:
- Discovery: `list_projects` (global mode only), `get_config` (global + project merged — the only way a caller learns `auto_trust`; carries both file paths).
- Read: `list_tasks`, `get_task` (includes `allowed_actions`), `wait_for_board_change`, `get_transition_status`, `check_conflicts`, `get_notifications`, `read_pane_content`. `list_tasks`/`get_task` carry `phase_status` + `phase_age_secs` + `tui_connected`. `list_tasks` returns `{tui_connected, tasks: [...]}`, omits descriptions unless `include_description` is set. `read_pane_content` returns a real tail (`pane_tail`) via `capture-pane -S -N`.
- Write: `move_task` (queues a transition request; actions `research`, `move_forward`, `move_to_planning`, `move_to_running`, `move_to_review`, `move_to_done`, `move_to_done_and_merge`, `resume`, `escalate_to_user`), `send_to_task` (Planning/Running/Review, 4096-byte cap; bracketed paste + watched submit). `resume` runs the task round a whole execute cycle.
- CRUD (Backlog only for update/delete): `create_task`, `create_tasks_batch` (max 50, index-based `depends_on`), `update_task`, `delete_task`.

**Phase status (`task_runtime`)** — how a non-TUI process learns a phase's status; published only while someone is reading (`board_watch`, 10-min window), marked by the web server on a board request and the MCP server on `list_tasks`/`get_task` (throttled 30s). Not gated on `serve`. Read `phase_status` against `phase_age_secs`, never alone (large age = nothing watching). `phase_status` is the TUI's verdict (all agents, the only signal that can say `ready`/`exited`); `agent_state` is the agent's own report (hooks, 5 of 8). `tui_connected` reads `tui_heartbeat` via `Database::tui_is_live` (6s window) — without it a frozen row reads as live.

**`wait_for_board_change`** replaces a poll loop: blocks inside the server, re-reading every `WAIT_POLL_INTERVAL`, answers once with only the tasks that differ from what this session was last shown, and carries the outcome of every queued `move_task`. Timeout defaults `DEFAULT_WAIT_SECS`, capped `MAX_WAIT_SECS` (a timeout is an answer). What wakes it is narrower than what changed — a state that needs the caller (`ready`/`idle`/`blocked`/`exited`, Done, a startable Backlog task, an escalation), appears/vanishes, a failed transition, a `tui_connected` flip (`TaskMark::needs_attention`); `working` and an absent phase status do not. Per session (`AgtxMcpServer::boards`; pure half `src/mcp/board_watch.rs`). Two maps (last reported + last poll); `turn_ts` tells two `idle`s apart. `async`.

**When a phase counts as done** (`ready` = safe to advance/merge/resume):
- Artifact written during this phase — `Task::phase_entered_at` stamped by `Database::update_task` on any status change; `phase_artifact_fresh` counts an artifact only if mtime ≥ that stamp. `phase_artifact_exists` stays for gating callers.
- A verdict applies only to the status it was computed for — `apply_session_refresh` drops one describing the previous phase; `TaskRuntime::status` records what each row was computed for.
- The turn must be over — `gate_ready_on_turn` holds a fresh artifact at `working` while the hook reports `working`/`blocked` (a silent `working` stops being trusted after `HOOK_STALE_SECS`).

`send_to_task` goes through `core::input::send_user_text` (bracketed paste + watched submit), not raw `send-keys` (a large typed burst has its head silently dropped by the agent).

**Serialized worktree setup** — one at a time (`setup_rx` is a single slot). Everything starting a Backlog task goes through `setup_queue` carrying a `SetupIntent` (the dependency overlay lets each plugin choose research vs planning via `PluginDefault`; an MCP request names one transition). A queued MCP request stays claimed + unprocessed (`get_transition_status` → `pending`) until the drain starts it; every way the drain declines resolves it with an error. Claims are reclaimed after `RECLAIM_CLAIMS_AFTER` (5 min, `Database::reclaim_stale_transition_requests`); `cleanup_old_transition_requests` deletes abandoned ones after an hour.

**`move_to_done_and_merge`** — merges the branch into its base in the project's own checkout, then Done (`allowed_actions` offers it to `Orchestrator`, not `Human`). `git::merge_task_branch` merges only when the checkout is on the base branch with no *tracked* modifications, else `Refused`. **Every route to Done must appear in the uncommitted-changes guard** (Done deletes the worktree; `has_changes` counts untracked; `every_route_to_done_refuses_a_worktree_with_uncommitted_work` iterates all three). An empty branch is refused (`commits_ahead` checked first; `MergeOutcome::NothingToMerge`). A conflict leaves the task in Review, sets `escalation_note`, sends `/agtx:merge-conflicts` (the virtual merge `check_merge_conflicts` runs first; `merge_branch` runs `git merge --abort` on failure).

**Incremental re-review** — the `resume` path writes HEAD to `.agtx/reviewed-at` (`mark_reviewed_point`). The review skill: present → `<marker>..HEAD` plus prior `.agtx/review.md`; absent → whole branch. The marker is a commit, so the skill pairs the range with **`git status --short`** (not `git diff HEAD`, which omits untracked files).

### MCP Server Modes
| Mode | Command | Used by |
|------|---------|---------|
| Project-scoped | `agtx mcp-serve <path>` | Orchestrator (bound to one project) |
| Global | `agtx mcp-serve` | Sweep and oneshot skills, ad-hoc sessions |

In global mode all CRUD tools require a `project_id` (call `list_projects` first); project-scoped ignores it. `ServerMode` enum in `src/mcp/server.rs`; resolution via `resolve_project_path(project_id)`.

### General Configuration
Global config at `~/.config/agtx/config.toml` (`GlobalConfig`):
```toml
default_agent = "claude"
fullscreen_on_enter = false  # Enter opens the task's tmux pane fullscreen inside agtx
agent_hooks = true           # Write agent lifecycle-hook configs into worktrees
auto_trust = false           # Answer agents' trust / bypass-permission prompts by reading the pane
update_check = true          # Daily GitHub release check + header notice

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
Per-project overrides at `{project}/.agtx/config.toml` (`ProjectConfig`), merged over global via `MergedConfig::merge`:

| Field | Purpose |
|-------|---------|
| `default_agent`, `agents` | Agent / per-phase agent overrides |
| `base_branch` | Branch worktrees are cut from |
| `github_url` | Repo URL for PR operations |
| `worktree_dir` | Where worktrees are created |
| `copy_files` | Comma-separated files copied into worktrees |
| `init_script` / `cleanup_script` | Shell commands run after creation / before removal |
| `workflow_plugin` | Active plugin for new tasks |
| `branch_prefix` | Branch name prefix |
| `skip_worktree` | Work directly in the project root |

### Project Trust
`init_script`, `cleanup_script`, `copy_files` are stripped from an untrusted project's config at startup (`App::new`) with a warning banner. Trust is a **canonical path → SHA-256 of `.agtx/config.toml`** map in `TrustStore` (`trusted_projects.toml`); editing the config invalidates trust. A project with no `.agtx/config.toml` is trusted by default. An untrusted project also forces `flags.no_init_scripts = true`. Approve via the in-TUI popup (any key) or `agtx trust`.

**agtx's own writes re-record trust** — any write to `.agtx/config.toml` (`P`, config editor save) changes the hash. `TrustStore::retrust_after_agtx_write` restores the *prior* decision, gated on the trust state read **before** the write. `TrustStore::path()`/`GlobalConfig::config_path()` honour `AGTX_CONFIG_DIR`; `Database::data_root` honours `AGTX_DATA_DIR`. `--no-init-scripts` suppresses scripts regardless of trust.

### Help Overlay
`?` opens `tui::help`'s table: every binding, grouped, scrollable, closed with `?`/`Esc`/`q`. **`HELP` is the complete list**; `build_footer_text` shows only column-specific actions plus `[?] help`/`[q] quit` (one line; `every_footer_fits_a_narrow_terminal` caps every variant at 120 chars). Scrolls with the same chords as a task pane (`scroll_action_for` table). Two columns where wide enough (`help::columns(n)` balances but never splits a section). `help_max_scroll` is a `Cell` the renderer writes.

### First Run
No separate flow. `main.rs` writes the default config, sets `FeatureFlags::first_run`; the TUI opens the **config editor** focused on `Default agent` (`open_first_run_editor`). `new_for_test_with_flags` calls the same path.

### Config Editor
`,` opens `~/.config/agtx/config.toml` and the project's `.agtx/config.toml` as a form (`src/tui/config_editor.rs`). Sections across the top (General / Agents / Worktree / Theme, plus Project), fields below. `config_editor_area` sizes the box to its content across all sections, clamped to the terminal.

| Key | Action |
|-----|--------|
| `h/l` or `Tab`/`S-Tab` | Move between sections |
| `j/k` or arrows | Move between fields |
| `Enter` / `Space` | Toggle, or open the field for editing |
| `Enter` / `Esc` in an open field | Accept / abandon that field only |
| `C-s` | Save |
| `Esc` | Close (asks first if there are unsaved changes) |

**Fields are declared, not open-coded** — one `FieldId`, exactly two matches (`read`/`write`); `every_field_round_trips_through_read_and_write` walks the form. Opens on the files, not on `state.config` (a merged view would bake global defaults into the project file). `dirty` tracks what is stored. A save re-records trust (read state before the write). A bad colour is refused and the field stays open; theme edits preview live (global-only), closing without saving reloads from disk.

### Config Writes Preserve Comments
`GlobalConfig::save` / `ProjectConfig::save` go through `write_toml_preserving` (`src/config/mod.rs`), not `toml::to_string_pretty` (which drops comments and unrecognised keys). Rule: *agtx rewrites the values of keys it knows, never deletes a key it does not recognise, removes a key it manages when unset.* `expand_inline_tables` normalises serde's inline `worktree = { .. }` to a `[worktree]` section; `remove_key_keeping_comments` moves a removed first key's comment to the next key; `GLOBAL_MANAGED`/`PROJECT_MANAGED` name the keys a save may delete (`managed_keys_tests` uses an exhaustive struct literal). An unparseable file is replaced rather than erroring.

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
| `Ctrl+f` | Open the task popup fullscreen; press again to return |
| `d` | Show git diff for task |
| `D` | Open dependency-graph overlay |
| `m` | Move task forward (advance workflow) |
| `M` | Move Backlog task straight to Running |
| `R` | Start research for a Backlog task (in place) |
| `r` | Resume: Review → Running, or Running → Planning |
| `p` | Cyclic plugins only: Review → Planning (next phase) |
| `/` | Search tasks (jumps to and opens task) |
| `P` | Select workflow plugin |
| `,` | Open the config editor |
| `?` | Show every binding (help overlay) |
| `u` | Update agtx (only bound when a newer release was found) |
| `W` | Serve the board to a phone: QR, pairing, paired devices |
| `O` | Toggle orchestrator agent (experimental) |
| `e` | Toggle project sidebar |
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
| `Ctrl+d/u` or `PageDown/PageUp` | Page down/up — the pair the footer advertises |
| `Ctrl+n/p` or `Ctrl+Down/Up` | Scroll down/up five lines — bound, not named in the footer |
| `Ctrl+g` | Jump to bottom |
| `Ctrl+f` | Toggle windowed / fullscreen |
| `Ctrl+q` | Close popup |
| Other keys | Forwarded to tmux/agent (including `Esc`) |

All three scroll rows are delegated to the agent when the pane has no tmux scrollback (translated to `PageUp`/`PageDown`/`End`). When an escalation note is present, the first keypress only dismisses the banner.

### PR Creation Popup
| Key | Action |
|-----|--------|
| `Tab` | Switch between title/description |
| `Enter` | In title: move to description. In description: newline |
| `Ctrl+s` | Create PR and move to Review (ignored while generating) |
| `Esc` | Cancel |

### Task Creation Wizard
Flow: **Title → Agent → Plugin → Prompt**. Both middle steps are optional and drop out when there is nothing to choose (one installed agent, or one compatible plugin), so `steps()` is derived.

| Key | Action |
|-----|--------|
| `j/k` or arrows | Navigate a list step |
| `/` | Filter the list |
| `Tab` | Cycle through options |
| `Enter` | Advance to next step / save |
| `Esc` | Cancel the wizard, from any step |
| `S-Tab` / `C-b` | Step back one step |
| `C-s` | Save from any step |
| `\` + Enter (or `C-j`, `Alt+Enter`) | Newline in the prompt |

`WizardState` (`src/tui/wizard.rs`) is the whole flow as one value (step, both text fields, both list picks, edited task id) — holding every field makes back-navigation work.

- **The agent step writes `Task::base_agent`**, which stands in for `default_agent` when a phase's agent is resolved (`phase_agent`: a phase's own `[agents]` entry wins, else the task's pick, else the default). It is a column of its own, not `Task::agent` (which names what runs now). A `NULL` row resolves to the configured default; the migration backfills only for Backlog tasks with no session. The plugin list is filtered by the settled agent (`reseed_plugins_for_agent`); `try_save_wizard` rebuilds too.
- `ListPick` is one type for both list steps; `selected` indexes `options`, not the filtered view; `settle()` moves it only when the filter excludes it. A filter belongs to the visit, not the step. Each step seeds itself once (`take_seed`/`take_prompt_seed`). `steps()` is derived; `step_index` is a position in *this* flow.
- **`Esc` cancels outright**; stepping back is `Shift+Tab`/`Ctrl+B`. Two things own `Esc` first — a prompt-step dropdown, and an open list filter.
- **`Enter` saves; a newline needs an escape**: `\`+Enter (documented), `Ctrl+J` (crossterm parses `0x0A` as `Ctrl+J` in raw mode), `Alt+Enter`. **`Shift+Enter` is deliberately not bound and agtx does not request the Kitty protocol** (a terminal sends bare CR for both unless negotiated; `supports_keyboard_enhancement()` blocks up to 2s). `Ctrl+J` overlaps the pickers' down-arrow and collides with `vim-tmux-navigator`.
- **A chord is not text** — `TextInput::handle_edit_key` and all three dropdowns guard `Char(c)` on `!ctrl && !alt`. Under the Kitty protocol `Shift+Tab` arrives as `Tab`+`SHIFT`, not `BackTab` (both accepted).
- `AppState.wizard: Option<WizardState>` **is** the input mode; `wizard_step()` is the one accessor. A refusal is never silent — `title_problem()` is the single source for `Enter` on the title step and `C-s` (empty / too long / duplicate); a failure walks back to the title step and renders the reason in the wizard body. The renderer is a real `Layout` (`draw_wizard`). The prompt step's `#`/`@`, `/`, `!` dropdowns are each their own handler (`handle_file_search_key`/`handle_skill_search_key`/`handle_task_ref_search_key`), each takes its state out, all commit/cancel through `splice_search_region`.

### Task Edit (Description)
| Key | Action |
|-----|--------|
| `#` or `@` | Start file search (fuzzy find) |
| `/` | Start skill search (at start of line or after space) |
| `!` | Start task reference search (at start of line or after space) |
| `\` + Enter (or `C-j`, `Alt+Enter`) | Newline (multi-line) |
| Arrow keys | Move cursor |
| `Alt+Left/Right` or `Alt+b/f` | Word-by-word navigation |
| `Home/End` | Jump to start/end |

## Code Patterns

### Comments and Docs
**Describe the code as it is: keep the reasoning and the measurements, drop the chronology.** Say what a guard prevents, not what once went wrong; name the alternative, not the predecessor. `used to`/`no longer`/`previously`/`the old X` usually signal a lapse (but each has a legitimate present-tense sense — read the line).

### Ratatui TUI
- `crossterm` backend. State separated from terminal for the borrow checker: `App { terminal, state: AppState }`. Drawing functions are static: `fn draw_*(state: &AppState, frame: &mut Frame, area: Rect)`. Theme colors via `state.config.theme.color_*`.
- **Every text field uses `tui::text_input::TextInput`** (buffer + caret as one value; motion/deletion/word-jumps behind `handle_edit_key`, which *reports* whether it consumed the key rather than swallowing it). `Enter`/`Esc` are not handled there. The caret is a **byte** offset — every motion goes through the module's boundary helpers. `wrapped_cursor_pos` stays beside `wrap_spans` in `app.rs` (shared wrap rule).

### Error Handling
`anyhow::Result` for all fallible functions; `.context()` for context. Gracefully handle missing tmux sessions/worktrees.

### Database
SQLite via `rusqlite` (`bundled`). Migrations via `ALTER TABLE ... ADD COLUMN` (ignores errors if column exists). DateTime as RFC3339 strings.

### The Event Loop
`App::run` **blocks** on one `mpsc` channel two threads feed, and draws only when something changed.

```text
agtx-terminal-input ─┐   blocking event::read()
                     ├─► mpsc<Wake> ─► run(): recv_timeout(HOUSEKEEPING_TICK)
agtx-pane-watch ─────┘        ├─ draw, if anything set `dirty`
   captures the open popup's  └─ housekeeping, on its own tick
   pane, sends only when CHANGED
```

- **Echo latency** is the pane watcher's cadence. It learns a pane painted two ways: **push** — a second control client attached without `no-output` (`OutputWatch`), open only while a popup is, whose `%output` says which pane painted (`SHELL_REFRESH_INTERVAL` becomes a rate limit; `PANE_OUTPUT_MIN_INTERVAL` paces agent output, `PANE_TYPING_WINDOW` keeps the fast cadence after a keystroke); **poll** — the timer when push is unavailable. Either way the watcher compares and wakes the loop only on a difference. Capture depth follows the scroll position — `SHELL_POPUP_TAIL_LINES` (100) at the bottom, `SHELL_POPUP_CAPTURE_LINES` (500) once scrolled up (`popup_capture_depth`). `PANE_PUSH_BACKSTOP` is the net.
- **Housekeeping** (`maybe_spawn_session_refresh`, expiring warnings, the MCP transition queue) runs on `HOUSEKEEPING_TICK`; its SQLite query has its own `TRANSITION_POLL_INTERVAL` (phase status is only as fresh as `PHASE_STATUS_CACHE_TTL`). **Drawing** happens when `dirty` was set; `REDRAW_BACKSTOP` is a safety net, not the mechanism. **Nothing on the board animates** (`Working` is a static `▶`; `nothing_on_the_board_animates` fails if a spinner appears). A closing window is pushed, not polled (`%window-close` → `InputConfig::window_events`). The poll fallback backs off to `PANE_IDLE_INTERVAL` after `PANE_IDLE_ROUNDS`; a keystroke pokes it back.

### Background Operations
PR description generation, PR creation, and phase status polling run in background threads. Results come back over `mpsc`, collected by `pump_background_results()` on each wake-up (non-blocking `try_recv`).

### Phase Status Polling
`maybe_spawn_session_refresh()` spawns a background thread (2s cache TTL per task) covering Planning/Running/Review plus Backlog tasks with an active research session. **Its tmux work is per pass, not per task** — one `list-windows -a` answers `window_exists` for every task (`live_window_targets`); each pane captured once over the control connection (`capture_pane_text` → `CaptureSpec::text()` — no `-e`, so no SGR escapes in a dialog's wording), shared by the dialog scan and content hash. `window_is_gone` fails safe (unreadable listing = unknown); overlap guard: one refresh thread at a time. `apply_session_refresh()` applies on the main thread; idle detection (Working → Idle) via `pane_content_hashes` timestamps, only for tasks with no hook report. Five states (`PhaseStatus`): Working, Blocked (agent-reported only), Idle (15s no output), Ready, Exited.

### Hook-Based Phase Status
Agents that support lifecycle hooks report their own state instead of agtx guessing from pane output. `agtx hook --env <agent>` writes `{worktree}/.agtx/status/{task_id}.json`, read by the refresh thread.

**Five of eight agents report their own state.** Every one takes a **project-local** hook config (removing the worktree removes the registration). `write_hook_config()` is the writer, selected by `AgentSpec::hook_config` (`HookConfigKind`); `None` (keep the pane-hash heuristic) is a supported state.

| Agent | Config file | Handler shape | Payload event key | Can report `Blocked` |
|---|---|---|---|---|
| claude | `.claude/settings.local.json` | `{hooks:[{type,command}]}` | `hook_event_name`, PascalCase | yes — `PermissionRequest`, `Notification` |
| gemini | `.gemini/settings.json` | same | `hook_event_name`, PascalCase | yes — `Notification` |
| cursor | `.cursor/hooks.json` | flat `{command}` | `hook_event_name`, camelCase | no |
| grok | `.grok/hooks/agtx.json` | `{hooks:[{type,command}]}` | `hookEventName`, snake_case value | yes — `Notification` scoped to `permission_prompt` |
| antigravity | `.agents/hooks.json` | keyed by hook *name*, then event | none — passed as `--event` | no |
| codex | `.codex/hooks.json` | `{description, hooks:{…}}` | `hook_event_name`, PascalCase | mapped, **not deployed** |
| opencode, copilot, pi | — | — | — | no hooks |

The formats are **not interchangeable**, and every mismatch fails silently (a valid-looking config, no status file written) — this is why `tests/smoke/agent_smoke.py` is the gate. Notes: cursor takes a flat list; grok reports `pre_tool_use` for `PreToolUse` (both map via `squash()`; camelCase keys need serde aliases); antigravity's `PreToolUse` is not subscribable (`PostToolUse` carries the heartbeat); codex's `hooks` key in `.codex/config.toml` is a table, not a path. **Codex is off, deliberately** — a project-local `.codex/hooks.json` triggers a startup review that would trust every hook the repo ships (the vocabulary is mapped and the writer exists — one field to enable).

- **Writers merge where the file is shared** — `merge_claude_hooks()` keeps user entries, replaces only agtx's own (matched on `" hook --env"`); `write_hook_config` runs **after** `write_mcp_config`. The hook command is task-agnostic; the task comes from `AGTX_TASK_ID`/`AGTX_WORKTREE` set on the tmux **window** by `create_window` (tmux `-e`); exits silently when absent. `--event <Name>` supplies the event for an agent whose payload carries none (antigravity).
- **`hook_events(kind)` and `map_hook_event(kind, …)` are two halves of one contract** (same file, both directions asserted). `src/agent/hook_status.rs` is pure (event mapping, atomic write-then-rename, staleness, `merge_event`'s guard against a late `PreToolUse` clearing a fresh `Blocked`). Claude's `Notification` is scoped to `permission_prompt` (unscoped a finished turn would report Blocked immediately).
- **Precedence** in the refresh thread: fresh artifact with the turn over → `Ready` > window gone → `Exited` > hook status > pane-hash heuristic. **Purely additive** — no status file → the 15s pane-hash heuristic runs unchanged; a `working` record older than `HOOK_STALE_SECS` (300s) is distrusted. **Consumers**: an agent-reported `Blocked` fires the orchestrator stuck-task notification *immediately*, `Idle` keeps its 60s settle; a trust-blocked task is excluded. The merge-conflict trigger fires on `Ready`/`Idle` only.
- **Binary-path drift**: the absolute `agtx` path is baked into the hook command + every MCP config. `write_skills_to_worktree` records the deploying binary in `.agtx/deployed-by`; `refresh_stale_worktree_configs` re-deploys mismatched worktrees at startup. **Not wired**: codex, opencode (TypeScript plugin API), copilot (support *unknown*).

### Task References & Dependencies
- Type `!` in description input (start of line or after space) to search tasks; selecting inserts `![task-title]` and tracks the ID (stored comma-separated in `task.referenced_tasks`). References double as **dependencies** — `Database::deps_satisfied` returns true only when every referenced task is in Review or Done; starting research or moving a Backlog task forward is blocked until then.
- `src/tui/dep_graph.rs` builds a topologically-leveled `DepGraph` (level 0 = no in-graph deps; a node is `unblocked` when in Backlog with satisfied deps). The `D` overlay renders it and can batch-move; free of ratatui/DB types (caller passes a `deps_satisfied` closure). MCP `create_tasks_batch` wires the same deps via 0-based `depends_on` indices.
- At worktree setup, referenced tasks' artifacts are copied to `.agtx/references/` (git diffs `{slug}.diff`, worktree files `.agtx/skills/`/`.planning/` if the referenced worktree still exists).

### Auto Merge-Conflict Resolution
During `apply_session_refresh`, Review tasks are checked against the default branch using `git merge-tree --write-tree` (Git 2.38+, non-destructive). Triggers when a Review task becomes **newly Ready** or has been **Idle 30+ seconds**; sends `/agtx:merge-conflicts` skill + prompt to the agent. One-shot per task (`merge_conflict_checked: HashSet<String>`). Works with all plugins (the skill is a builtin deployed to every worktree). The skill: commit current work → merge origin/main → resolve conflicts → review only conflicted files against both parents → run tests.

### Agent Integration
- Every per-agent value is one field of that agent's `AgentSpec` in `src/agent/spec.rs`; functions read the table rather than matching on the agent's name. Agents spawned via `build_interactive_command()` in `src/agent/mod.rs`. Flags: Claude `--dangerously-skip-permissions`, Codex `--sandbox workspace-write`, Gemini `GEMINI_TRUST_WORKSPACE=true` + `--approval-mode yolo`, Copilot `--allow-all-tools`, Cursor `agent --yolo`, Grok `--yolo --trust`, Antigravity `agy --dangerously-skip-permissions --mode accept-edits`.
- `build_resume_command()` is the recovery variant after a restart — mostly `--continue` appended (`ResumeArgs::Append`), but Gemini uses `--resume` and Codex's `resume --last` *replaces* the flags (`ResumeArgs::Replace`).
- `tests/agent_parity_tests.rs` pins every string per agent (written against behaviour). Commands resolved per-task via `resolve_skill_command()`; prompts via `resolve_prompt()` (pure template substitution).

## Building & Testing

```bash
cargo build --release
cargo test
cargo test --features test-mocks

# Per-agent smoke tests — real binaries, real auth (opt-in, never CI)
tests/smoke/agent_smoke.py
python3 tests/smoke/test_agent_smoke.py

# Real-tmux pane input: ordering, escaping, pane sizing, reconnect, latency (opt-in)
AGTX_TMUX_IT=1 cargo test --test tmux_control_tests -- --nocapture
```

`tmux_control_tests.rs` starts throwaway tmux servers and asserts on the **bytes a program in the pane received**, not rendered output. Run on Linux too (only macOS / tmux 3.5a so far).

The smoke runner answers the one question the Rust suite cannot: **does a real agent binary actually receive its work?** Per phase it asserts the command was *submitted* (not parked in a composer), a marker file carries the **task id** passed in, the artifact appeared and the phase advanced, and the session is still usable. Dialogs are never pre-answered. See `tests/smoke/README.md`.

`cargo run --example agent_matrix` dumps `AGENT_SPECS` + `BUNDLED_PLUGINS` as JSON; the smoke runner reads it and `cargo test` compiles it (a new spec field breaks the build rather than drifting).

Dependencies: recent stable Rust (CI on `stable`, no MSRV pinned), SQLite (bundled), tmux, git, gh CLI (PR operations).

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
**`src/agent/spec.rs`** — the declarative half
1. Add one `AgentSpec` entry to `AGENT_SPECS`: identity (name, binary, description, git co-author), launching (`env`, `base_args`, `prompt_form`, `resume`, `headless_args`), skills (`skill_dir`, `skill_layout`, `skill_scan_dir`, `command_syntax`). Everything in `mod.rs`, `operations.rs`, `skills.rs` is derived from it.
2. Declare `prompt_form` (`Argv` or `Flag("-i")`) but leave `launch_prompt_verified: false` until checked against the real binary.
3. If no existing `SkillLayout`/`CommandSyntax` variant fits, add **one** variant plus the arm in the single function that reads it — never a new match on the agent's name.

**`tests/agent_parity_tests.rs`**
4. Extend every parity table (`parity_covers_every_known_agent` fails until you do).

**`src/tui/app.rs`** — still per-agent
5. Add the binary to `AGENT_COMMANDS` (pane process detection)
6. Add an activity indicator to `AGENT_ACTIVE_INDICATORS` if it is an Ink/Node TUI (runs inside bash)
7. Add exit command handling in `switch_agent_in_tmux()` (graceful exit cmd or Ctrl+C)
8. Add the skill-deploy branch in **both** `write_skills_to_worktree()` and `deploy_skill()`
9. Add the per-agent MCP config writer in `write_skills_to_worktree()` (format varies)
9b. Set `hook_config`/`hook_event_source` on the spec if it supports hooks; add its `HookConfigKind` arm to `write_hook_config`, `hook_events`, `map_hook_event`, and extend `vocabulary()` in `tests/hook_status_tests.rs`. Leave `hook_config: None` until a real run writes a `.agtx/status/*.json`.
10. Add an agent label color in the task-card footer `match task.agent.as_str()`
11. If Ink/Node TUI: add to the combined-send branch in `send_skill_and_prompt()`; add double-Enter handling if it has a command picker popup

**Plugins**
12. Add the agent to `supported_agents` in any `plugins/*/plugin.toml` that whitelists agents

### Adding a keyboard shortcut
1. Find the appropriate `handle_*_key` function in `src/tui/app.rs`
2. Add match arm for the new key
3. Add it to the `HELP` table in `src/tui/help.rs` (that overlay is the complete list)
4. Only add it to `build_footer_text` if it belongs to the five things you reach for constantly

### Adding a new popup
1. Add state struct (e.g. `MyPopup`) in app.rs
2. Add `Option<MyPopup>` field to `AppState`, initialize to `None` in `App::new()`
3. Add rendering in `draw_board()`
4. Add key handler function `handle_my_popup_key()`
5. Add check in `handle_key()` to route to handler

### Adding a new bundled plugin
1. Create `plugins/<name>/plugin.toml` with commands, prompts, artifacts
2. Add entry to `BUNDLED_PLUGINS` in `src/skills.rs`
3. Optionally add `supported_agents` to restrict agent compatibility

### Adding custom skills to a plugin
1. Create `plugins/<name>/skills/agtx-{phase}/SKILL.md` files (YAML frontmatter: `name: agtx-{phase}`, `description: ...`)
2. Auto-deployed to agent-native paths during worktree setup

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
- Orchestrator: support non-Claude agents; task deletion notifications; multi-project support

Design notes for in-flight work live in `docs/planning/` (untracked).
