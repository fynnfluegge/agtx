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
./target/release/agtx hook --env <agent> [--event <Name>] < payload.json

# Version, and self-update to the latest GitHub release
./target/release/agtx --version
./target/release/agtx update [--check]
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
│   ├── operations.rs # TmuxOperations trait (mockable for testing)
│   ├── input.rs      # PaneInput/PaneInputSink, the input broker, both backends
│   ├── input_tests.rs # Coalescing, ordering, queue policy (included via #[path])
│   ├── control.rs    # Persistent `tmux -C` client, frame parser, tmux command encoder
│   └── control_tests.rs # Encoder fixtures + parser tests (included via #[path])
├── git/
│   ├── mod.rs        # is_git_repo, repo_root, current_branch, diff_stat/diff_full,
│                     # merge_branch, check_merge_conflicts, delete_branch
│   ├── worktree.rs   # Git worktree create/remove/list
│   ├── operations.rs # GitOperations trait (mockable for testing)
│   └── provider.rs   # GitProviderOperations trait (GitHub PR ops)
├── agent/
│   ├── mod.rs        # Agent struct, detection, command builders (all derived from spec.rs)
│   ├── spec.rs       # AgentSpec table — one declarative record per agent + kind enums
│   ├── hook_status.rs # Agent-reported liveness: per-agent event vocabularies,
│                     # what agtx registers, atomic status writes, staleness
│   ├── trust.rs      # Reads each agent's own workspace-trust store; seeds antigravity's
│   ├── trust_tests.rs # Unit tests for trust.rs (included via #[path])
│   └── operations.rs # AgentOperations/CodingAgent traits (mockable)
├── mcp/
│   ├── mod.rs        # Re-exports
│   └── server.rs     # MCP server (JSON-RPC over stdio) — global and project-scoped modes
├── update/
│   ├── mod.rs        # check_for_update() — the whole background check
│   ├── version.rs    # Semver-lite parse/compare + the prerelease policy (pure)
│   ├── check.rs      # Cache TTL, should_check, available (pure but for the file)
│   ├── github.rs     # releases/latest over curl
│   ├── release.rs    # Repo slug + archive naming, shared with install.sh/release.yml
│   └── install.rs    # Download, verify sha256, atomic in-place binary swap
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
├── update_tests.rs    # Version policy, cache TTL, artifact naming parity, binary swap
├── mock_infrastructure_tests.rs # Mock infrastructure tests
├── shell_popup_tests.rs         # Shell popup logic tests
├── tmux_control_tests.rs        # Real-tmux pane input: ordering, escaping, sizing,
│                                # reconnect, latency — opt-in via AGTX_TMUX_IT=1
└── smoke/             # Per-agent smoke tests — real binaries, no mocks, opt-in
    ├── agent_smoke.py      # The runner: scratch repo → TUI in tmux → phases over MCP
    ├── test_agent_smoke.py # Deterministic tests for the harness itself (no tmux/auth)
    ├── agent_matrix.rs     # Dumps AGENT_SPECS + BUNDLED_PLUGINS as JSON for the runner
    │                       # (a [[example]] in Cargo.toml, so `cargo test` compiles it)
    └── README.md           # What it asserts, its outcomes, and what it has found

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
- **clear_context_on_advance**: When true, send an agent-specific clear-context command before the phase skill/prompt on transitions. Honored for the agents whose `clear_context_command` is verified — Claude's `/clear` and pi's `/new` — and a no-op for the rest (issue #46). Delivery follows the agent's `send_strategy`: an Ink-class composer drops a combined text+Enter `send_keys`, and the parked text would then be concatenated with the skill+prompt into one unusable message, so those agents get the same paste-and-confirm path the message itself takes
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
| pi | `.pi/mcp.json` | JSON, `mcpServers` |

Four writers **merge** instead of overwriting, because their file may already exist in the worktree — either tracked in the repo, or (for `.gemini`, `.grok` and `.agents`) copied in from the project root by `AGENT_CONFIG_DIRS`; `.pi` is not in that list, so only the tracked-in-the-repo half applies to it. Grok appends `[mcp_servers.agtx]` to any existing `.grok/config.toml`; antigravity parses `.agents/mcp_config.json` and inserts `mcpServers.agtx`, preserving other servers and top-level sibling keys (`.agents/` is vendor-neutral, so a project is more likely to ship one); gemini inserts into `.gemini/settings.json`, which otherwise loses the user's theme, model and any other `mcpServers`; pi's `.pi/mcp.json` is where the `pi-mcp-adapter` package persists its own per-server `disabled` flags, so clobbering it re-enables servers the user switched off. pi has no MCP client of its own — without that package the file is inert, not harmful. Claude's `settings.local.json` side-effect below merges for the same reason.

Claude needs an extra side-effect to avoid an interactive dialog on first open: `.claude/settings.local.json` gets `enableAllProjectMcpServers: true` plus `skipDangerousModePermissionPrompt: true`, which is what actually preflights the bypass-permissions warning (see the dialog table).

agtx used to append a `[projects."<worktree>"] trust_level = "trusted"` entry to the user's global `~/.codex/config.toml` here too. **Removed, measured:** codex resolves trust to the *git repository root*, and a worktree under a trusted root both skips the dialog and loads its own `.codex/config.toml` — `/mcp` listed agtx with and without the entry. It bought nothing and accumulated one entry per worktree.

`write_skills_to_worktree` also seeds antigravity's `trustedWorkspaces` for the new worktree, when the project root is already trusted there (`agent::trust`). It is the right call site for both worktree creation *and* an agent switch: with a per-phase agent config, the switched-in agent sees the worktree for the first time at switch time. Home-directory lookups go through `agent_trust_home()`, which honours `AGTX_AGENT_HOME` so the test suite never touches the real user's config.

### MCP pre-handshake filter
`agtx mcp-serve` does not hand raw stdio to rmcp. Antigravity probes every stdio server with a custom `server/discover` request *before* `initialize`, and rmcp treats a non-`initialize` first message as fatal (`ExpectedInitializeRequest`) and exits — so the follow-up `initialize` hits a closed pipe. `src/mcp/prehandshake.rs` answers pre-handshake requests with JSON-RPC `-32601` and keeps the connection open; once `initialize` is forwarded it is a pass-through. Two background tasks pump real stdin/stdout through in-memory duplex pipes (`filtered_stdio()` in `src/mcp/server.rs`).

Commands are written once in canonical format (`/ns:command`) and auto-translated:
- Claude/Gemini: `/ns:command` (unchanged)
- OpenCode/Cursor/Grok/Antigravity: `/ns-command` (colon → hyphen, slash kept)
- Codex: `$ns-command` (slash → dollar, colon → hyphen)
- Copilot: no interactive skill invocation (prompt only, no commands sent)

### Sending Skills & Prompts to Agents
Prompt delivery has **two lanes**. Getting this wrong is the usual cause of "the agent started but never got the task".

**Launch lane — the first message of a task's life.** The skill command and prompt are composed by `compose_launch_text()` and handed to the process in **argv**, so the agent starts with the task already in hand: no readiness polling, no window in which a keystroke can be dropped. Gated by `spec::can_launch_with_prompt()`, which requires both an agent whose launch form is *verified* (`AgentSpec::launch_prompt_verified` — all but copilot, which is unmeasured) and a prompt under `MAX_LAUNCH_PROMPT_BYTES` (128 KiB; `execve` counts bytes). Anything else falls through to the mid-session lane.

**An agent switch takes this lane too.** The rule is *whenever agtx starts a new agent process and has a prompt for it, the prompt goes in argv* — a switch is the same act as a first launch, just into an existing window. `spawn_send_to_agent` and the cyclic Review→Planning path compose the launch text and skip the send when it lands; the two resume-style switches carry no prompt. A **same-agent** advance cannot use it: that process is already running, so the typed lane is correct there.

One trap: `create_window` nests its command inside `sh -c '…'`, so a prompt going that way is quoted **twice** (`wrap_launch_command` + `compose_command`). The switch path types a command line into the window's *already-running* shell — **one** level. Adding `single_quote` again there would deliver visible backslashes into the composer. A multi-line launch text is sent with `paste_text` + `Enter` rather than `send_keys`, because a newline typed at a shell prompt submits the line. `setup_task_worktree` returns `(target, launched_with_prompt)` and its callers skip the send entirely when true.

Two consequences worth knowing: `resolve_skill_command(collapse: false)` is used here, so `{task}` keeps its paragraphs and lists (the typed path must flatten them); and `spec::normalize_prompt()` strips NUL, `\r` and other control characters that cannot survive argv, keeping `\n` and `\t`.

**The prompt is quoted twice, and both layers must escape.** `compose_command` single-quotes the prompt, then `create_window` nests the whole command inside `sh -c …`. Interpolating it raw ends the outer word at the first inner quote and the shell parses the prompt as code — verified against tmux 3.5a, claude received argv `["--dangerously-skip-permissions", "/agtx:plan"]` with the task id and entire task silently gone, and codex's `$agtx-plan` lost `$agtx` to expansion. `single_quote()` in `src/tmux/operations.rs` is the second layer; its tests run the wrapper through a real `sh` and assert the delivered argv, because a string-equality test would have passed on the broken version.

**Mid-session lane — phase advances into an already-running agent.** `send_skill_and_prompt()`, three paths, because agent TUIs disagree about how a slash command plus arguments must arrive:

1. **opencode** — its picker strips arguments if the whole string is typed at once. So: send the bare command name → wait for the picker → Enter (inserts it) → send the args → Enter. Still on the typed path: it is the one flow where the text has to arrive in two pieces by design
2. **gemini / codex / cursor / antigravity / pi** — skill + prompt combined into a *single* message delivered by **bracketed paste** (`paste_text`), then one Enter. The paste is atomic, so the old poll-until-it-renders step is gone, and the `\n\n` joining command to prompt stays literal text instead of arriving as a real Enter that submits the message half-written. Gemini executes-and-loses a separately sent prompt; Codex's `$skill` mentions are inline references that do nothing when sent standalone; pi's composer takes a paste as literal text but leaves a combined text+Enter `send_keys` sitting unsent. **How many Enters this takes is not fixed** — see *Submitting is its own delivery problem* below
3. **everything else** (claude, copilot, grok) — the generic `match (skill_cmd, prompt_trigger)` path using `send_keys`, waiting on `prompt_triggers` between the command and the prompt when configured

**Submitting is its own delivery problem.** A message with a prompt after the command submits on the first Enter. A **bare skill command** — what a phase whose command carries no `{task}`/`{task_id}` sends, which is `review` — exactly matches a skill name, so the composer's command picker opens *on the paste*. That Enter is then consumed by the picker ("Press enter to insert"), which inserts the command and repaints, leaving it parked. Measured against codex-cli 0.144.5 and cursor-agent 2026.08.25: both open the picker on a pasted bare command, and both submit on the second Enter.

So `submit_message()` counts nothing and watches the composer instead: it presses Enter until the text is **gone from the bottom `COMPOSER_TAIL_LINES` of the pane**, bounded by `SUBMIT_ATTEMPTS`. A repaint is not a submit — that was the old test, and a picker opening satisfies it. The window is 14 lines because the picker draws its suggestions *below* the composer and cursor's footer wraps the worktree path, putting the text eight or more lines off the bottom; sizing it from the tidy pane left behind *after* a failure is how a too-narrow window looks correct. Erring wide costs one inert Enter into a submitted composer; erring narrow parks the command forever.

**Atomic is not the same as delivered.** An agent TUI that has not attached its stdin reader yet discards what it is sent — bracketed paste included, because the discard happens in the application, not in the pty — and `wait_for_agent_ready` cannot prove otherwise. So paths 1 and 2 go through `deliver_message()`, which resends **while the pane is unchanged** (three attempts, 2s each) and stops the moment it redraws, on the same reasoning `dismiss_launch_dialog` uses: a redraw means it landed, and resending would double the message. Landing is judged by the pane changing rather than by finding the text, because a composer wraps, re-indents and box-draws what it echoes.

`clear_context_on_advance` is applied before all three, and only for Claude (`/clear`, then poll until the pane stabilises).

**tmux send primitives** — pick by what you are sending, they are not interchangeable:

| Method | tmux | Use for |
|---|---|---|
| `paste_text` | `load-buffer` + `paste-buffer -p` | a whole message; bracketed, atomic, newlines stay literal |
| `send_text` | `send-keys -l --` | literal text; no key-name lookup |
| `send_key` | `send-keys` (no `-l`) | **key names** — `Enter`, `C-c`, dialog answers |
| `send_keys` | `send-keys` + `Enter` | text plus a submit, generic path |

Without `-l`, tmux resolves an argument that matches a key name *as that key*: `"Space"` arrives as `0x20`, `"Escape"` as `ESC`, `"Up"` as `\033[A` (tmux 3.5a). So `send_key` must never carry task-derived text — that is what `send_text` is for.

### Typing into a Task Pane
The two lanes above deliver a *task*. This is the third lane, and it is the only one a **human** is
waiting on: the keys forwarded from an open task popup.

```text
crossterm key event
        │  popup_key_input()          — Char → Text, everything else → Key
        ▼
 PaneInputSink::send                  — enqueue, ~1 µs, never waits for tmux
   bounded channel (1024)
        ▼
   one broker thread                  — the single ordering authority
        ├─ coalesces adjacent Text for the same target (2 ms window, 4 KiB cap)
        ├─ flushes before every Key, Paste, target change, popup close, shutdown
        ├─► control backend  `tmux -C attach-session`   (persistent, opt-in)
        └─► subprocess backend  `tmux send-keys`        (default, and the fallback)
```

**Every key used to be a process.** `send_key` starts a `tmux` client and waits for it: **~20 ms
p95** idle, 40–50 ms under load, on the input thread, per keystroke. That is the whole reason this
lane exists. Enqueueing costs ~1 µs, and end-to-end delivery over the control connection is **0.32 ms
p95** against 38 ms for the subprocess path (tmux 3.5a, macOS; the measurement lives in
`tests/tmux_control_tests.rs`, and `docs/planning/tmux-control-mode-input.md` has the full table).

- **`PaneInput` is typed, not a formatted command**, because the broker must be able to tell literal
  text from a key name: text goes out with `send-keys -l` (no key-name lookup), a key without it.
  An unmodified character is therefore **`Text`**, where it used to be a key — a fix, not a
  reclassification: `send-keys -t x ";"` never arrived at all, because a standalone semicolon is how
  tmux separates commands.
- **Batching never delays a key.** Enter, Escape, arrows, and anything Ctrl/Alt-modified flush the
  buffer first and go immediately. A delayed Enter is a visibly broken editor.
- **A target change flushes.** Queued characters belong to the pane they were typed into; the popup
  close, popup open and fullscreen-toggle paths all flush so nothing can follow the target.
- **The control connection is on, and there is no config field for it.** A connect that fails, and a
  connection lost later, both fall back to the subprocess backend on their own (`maybe_connect` /
  `drop_control`), so a persisted setting could only hold a staler copy of a decision the broker
  already makes at runtime. `AGTX_TMUX_CONTROL=0` turns it off for one run, which is what a bug
  report needs to bisect the two lanes. The non-blocking broker is *not* conditional either way —
  the backend choice only decides what the broker thread writes through, so the subprocess path gets
  the same ordering and the same responsive input thread.
- **`tmux -C` is attached with `-f ignore-size,no-output`.** `ignore-size` keeps it out of tmux's
  client-size calculation, so it cannot resize the pane the popup sized by hand; `no-output` stops
  every byte an agent paints from being mirrored down our stdout, which the popup does not read
  (it captures panes instead). The session is an **attach point**, but only for targets that name
  their own session: `send-keys -t "orchestrator"` is resolved *inside the attached session*, so one
  client drives every window on the server **only** if every target is `session:window`. That is
  what `pane_target` guarantees, and it is not a nicety — `orchestrator` is a window name every
  project session has, so a bare target after a project switch delivered keystrokes to the previous
  project's agent, while a bare `task-<slug>` drew `%error can't find pane` and was dropped in
  silence. `set_session` re-points the *next* connect, for when the old session is killed.
- **`tmux_quote` is not `single_quote`.** Control mode parses tmux's syntax, not the shell's: inside
  double quotes tmux replaces `$VAR`, `#{format}`, a leading `~`, and backslash escapes, so all of
  `\ " $ # ~` are escaped, LF/CR/tab become `\n`/`\r`/`\t`, and the rest of C0 becomes `\ooo`. A
  raw newline is the one thing that cannot be sent: commands are newline-terminated, so it splits
  the command and the tail is parsed as a second one.
- **A failed control write is not replayed.** A write error is ambiguous — part of it may have
  reached tmux — and a duplicated Enter is worse than a dropped one. The request is dropped, the
  connection is marked dead, and the *next* one goes to the subprocess backend. A backend found
  *dead before writing* is different: nothing was sent, so that request moves to the fallback in
  place, keeping its order.
- **A full queue warns rather than reordering.** Sending synchronously would put that key ahead of
  everything already queued, so agtx keeps the prefix and tells the user.
- **A paste stays on `load-buffer`** (it needs a pipe, not a command argument) and is issued behind a
  `ControlClient::barrier` — the two travel different sockets, and only the barrier keeps the paste
  behind the text typed before it.
- **A pane with no tmux scrollback delegates its scroll keys to the agent.** A
  full-screen agent UI lives in the terminal's *alternate screen*, which
  accumulates no scrollback: `history_size` is 0, `capture-pane -S -500` returns
  exactly the visible rows, and the session's history belongs to the agent.
  Scrolling agtx's one-screen buffer would move nothing while the footer printed
  a line number to match — which is what made an empty buffer look like a broken
  scrollbar. So `ShellPopup::has_scrollback()` (from `PaneMetrics::history_size`,
  free in the `display -p` the refresh already runs) switches those keys over to
  `handle_popup_scroll`; the footer shows only the available actions. Unknown
  metrics count as *has* scrollback, so a failed query changes nothing.
  Scroll chords are translated rather than passed through: `C-n/p` use the same
  Page Down/Up translation, while `C-g` uses End. Measured against
  Claude Code 2.1.251: in its `ctrl+o`
  transcript view all four of `Up`/`Down`/`PageUp`/`PageDown` scroll, but in the
  main view `Up` recalls a previous prompt **into the composer** — overwriting
  what the user was typing — while `PageUp` is inert. A raw `C-d` would be an
  EOF that ends the session and `C-u` would kill the composer line, which is why
  nothing is forwarded verbatim. Claude takes the alternate screen shortly after startup and never
  gives it back, so this is the normal case for a task pane, not an edge one.
  `C-g` maps to `End`, measured the same way: it returns the transcript view to
  the bottom, and in the main view it only moves the composer cursor to the end
  of the line without altering the text. `C-n/p` use that same Page Up/Down
  translation; `C-d/u` remain explicit page navigation.
  Unrelated but worth knowing when a key "does nothing": a user's **own** tmux
  can eat it before agtx sees it — `vim-tmux-navigator` binds `C-h/C-j/C-k/C-l`
  in the root table and only forwards them to vim-like panes.
- Logs carry lengths, targets, key categories and timings. **Pane input is never logged.**
- The broker's ordering authority covers **popup input**. agtx's own writes to a pane — a dialog
  answer from `dismiss_launch_dialog`, a phase advance from `send_skill_and_prompt` — still go
  through `TmuxOperations` on their own threads, unordered with respect to it. That was equally true
  when both were subprocesses, and they do not overlap in practice: a pane parked on a dialog is not
  one the user is typing into.

### First-Launch Dialogs
Agents gate a directory they have not seen behind an interactive dialog. `LAUNCH_DIALOGS`
(`src/tui/app.rs`) is the table, derived from `AgentSpec::dialogs`; `dismiss_launch_dialog` answers
what it is allowed to.

**These mostly do not fire any more, and by default agtx does not answer the ones that do.**
Measured per agent (see `src/agent/trust.rs` for the table and the versions):

- **Trust is inherited from the project root** for claude, codex and gemini — the default
  `worktree_dir` is inside the project, so a user who opened the agent there once never sees the
  prompt again. cursor and grok are launched with `--trust`, and pi with `--approve`; opencode has no trust gate. pi's flag does double duty: it is also what lets it load the `.pi/skills/` agtx wrote into the worktree, since pi gates project-local skills on the same trust decision.
- **antigravity is the exception**: it matches trusted paths *exactly*, at any depth. agtx seeds
  each new worktree into its `trustedWorkspaces` — but only when the project root is already there,
  so it replays a consent the user gave rather than creating one.
- **`AgentDialog::security` splits the table.** Trust prompts and the bypass-permissions warning are
  the user's decision: with `auto_trust = false` (the default) agtx *detects* them — that is what
  turns the card `Blocked`, with the reason and the fix — and leaves them unanswered. Prompts that
  decide nothing about safety (codex's update prompt, its MCP tool approval) are answered either
  way, since leaving them up only wedges the pane.
- **Nothing is lost by waiting.** An argv-delivered prompt is *queued behind* a dialog, not eaten by
  it — verified for claude, cursor, gemini and antigravity. `wait_for_agent_ready` returns `None`
  when parked on an unanswered security dialog, so the typed fallback never sends into a menu.
- `auto_trust = true` restores the historical behaviour, and is set by `docker/entrypoint.sh` and
  the benchmark, where the container is disposable and nobody is at the board.

Dialogs are declared per agent on `AgentSpec::dialogs` and **matched against the running agent's own entries** — a stray digit typed into another agent's live composer is real corruption. An agent with no spec falls back to matching every known dialog, which beats leaving its pane blocked forever.

`answer` is a **key sequence**, not one key, because menus differ in kind: a numbered menu needs the digit and then an Enter to confirm, while an arrow-navigated menu whose safe option is already highlighted needs only the Enter — and sending it a digit first would type a stray character into the composer it opens.

| Agent | Dialog | Match | Answer | Scope |
|---|---|---|---|---|
| claude | workspace trust | `Yes, I trust this folder` | `1` `Enter` | Launch — `projects."<dir>".hasTrustDialogAccepted` in `~/.claude.json`, **inherited from a trusted ancestor**, so with the default in-project `worktree_dir` this does not fire once the project is trusted |
| claude | bypass-permissions warning | `Yes, I accept` / `I accept the risk` | `2` `Enter` | Launch — suppressed by the `skipDangerousModePermissionPrompt` preflight; backstop only. Options are inverted (`1. No, exit`), so never a lone Enter |
| codex | directory trust | `Do you trust the contents of this directory?` | `1` `Enter` | Launch — per directory. Worded unlike Claude's *and* Gemini's, so neither pattern catches it |
| codex | update prompt | `Update now (runs` | `2` `Enter` (Skip) | Launch — never "Update now": agtx must not upgrade an agent binary behind the user's back |
| codex | hook review | `Hooks need review` | `3` `Enter` (Continue without trusting) | Launch — fires when the project ships a `.codex/hooks.json` codex has not seen. Answered because option 3 *declines*; option 2 would trust every hook in the repo |
| codex | MCP tool approval | `Allow the` + `MCP server to run tool` + `Always allow` | `3` `Enter` | **Session** — mid-session, matched only against a codex pane |
| gemini | folder trust | `Do you trust the files in this folder?` | `1` `Enter` | Launch — answering it restarts the process |
| cursor | workspace trust | `Workspace Trust Required` | `a` alone | Launch — its question line is *identical* to codex's, so it is matched on the heading. Answered with the access key the dialog advertises, which survives an option being added above the highlighted row |
| antigravity | project trust | `Do you trust the contents of this project?` | `Enter` alone | Launch — arrow-navigated with "Yes, I trust this folder" preselected, so a digit would land in the composer. Its own wording, not Claude's; codex's differs by one word (`directory`) |

`require_all` distinguishes alternatives (several wordings of one prompt) from conjunctions (a prompt identified only by a combination of phrases). `security` marks the rows agtx will not answer unless `auto_trust` is on.

It runs in **both** `wait_for_agent_ready` loops *and* the session-refresh loop: the readiness
budget expires after ~60s and a slow agent can render its dialog later than that. A retry only
happens while the pane is **unchanged** — a redraw means the answer landed, and resending would
type a stray digit into the agent's live composer.

Missing an arm is silent and total: the task's prompt is delivered but never read, because the
agent never reaches its composer. Antigravity and cursor are the worked examples — same bug, two
agents, found within a day of each other by the same smoke run. Antigravity's is the fuller story. It was left unhandled on the
reasoning that its wording matched Claude's and the choice belonged to the user — but "unhandled"
was never neutral: agtx pasted the task into a menu that ignores text, and its follow-up Enter
confirmed the dialog, so **every** antigravity task reached its composer empty with the prompt gone.
Per-agent scoping is what made answering it safe, and the per-agent smoke run
(`tests/smoke/agent_smoke.py`) is what found it. Cursor's was the same shape, and shows why the
scoping matters: its question line is *character-identical* to codex's, whose answer (`1`) is not
even an option in cursor's menu.

Some prompts must stay unanswered, and that is a different thing from being undeclared. Gemini's
first-run `Opening authentication page in your browser. Do you want to continue?` has `1. Yes`
preselected — an Enter would start an OAuth flow and open a browser, which is not a side effect a
phase transition gets to have. Same reasoning as never choosing codex's "Update now".

### Session Persistence
- Tmux window stays open when moving Running → Review
- Resume from Review simply changes status back to Running (window already exists)
- No special resume logic needed - the session just stays alive in tmux

### Self-Update
agtx tells the user when a newer release exists and replaces its own binary on request.

```
  startup ──► background thread ──► curl api.github.com/…/releases/latest
                     │                        │
                     │                  cache 24h  →  ~/.config/agtx/update.json
                     ▼
              mpsc::Receiver  ──► event loop try_recv ──► header: "⬆ 0.2.8 [u]"
                                                                     │
                                                             [u] popup ──► install_release()
```

- **The binary must know its own version.** `env!("CARGO_PKG_VERSION")` is the only source, and
  `release.yml`'s *Tag matches Cargo.toml* step fails the build when the pushed tag disagrees with
  the manifest. Before this existed the tags had reached `v0.2.7` while `Cargo.toml` still said
  `0.1.0` — a released binary could not answer the question every part of this feature compares
  against
- `--version` / `-V` / `version` and `update` are handled in the **early fast path** in `main.rs`,
  beside the `hook` arm, for two reasons: neither wants a daily log appender built for it, and the
  `mode` match below filters out every `--`-prefixed argument, so `--version` would otherwise fall
  through and open the current directory as a project
- **The cache is not an optimisation.** Unauthenticated GitHub allows 60 requests/hour *per IP* and
  agtx is launched dozens of times a day across project directories. 24h TTL; a stale cache is still
  served when the network is down, because a week-old "0.2.8 is out" is still true
- **Cache path gotcha:** built from `GlobalConfig::config_path()`'s parent, **not** `directories`'
  `config_dir()` — see the config-path split under *Database Storage*. The latter would put it in
  `~/Library/Application Support/` on macOS, away from the `config.toml` and `logs/` it belongs with
- **`curl`, not an HTTP crate.** One GET per day does not justify adding a TLS stack to a binary
  that has none; `curl` is already required by `install.sh`, and it honours `HTTPS_PROXY`/`NO_PROXY`
  and the system CA store, which is what corporate networks need. `src/update/github.rs` is the only
  file that would change if that stops being true
- **Failure is always a missing notice**, never an error: no network, no `curl`, a rate-limit body,
  an unparseable tag — all yield "no update". A version check must not be able to make the TUI shout
- **The swap** is `rename(target, target.old)` → `rename(new, target)` → unlink, staged inside the
  *target's own directory* so the rename is same-filesystem (`/tmp` often is not). Renaming over a
  running binary is legal on Unix — `ETXTBSY` applies to writing into the busy inode, not to
  replacing the directory entry. The `.old` step means a failure between the two renames leaves a
  recoverable file rather than no `agtx` at all
- **Replacing in place is what keeps worktrees valid.** The absolute `agtx` path is baked into every
  worktree's hook command and MCP configs (*Binary-path drift* below); an in-place swap keeps that
  path identical, so nothing needs re-deploying. Installing to a new location would invalidate every
  existing worktree
- **A package-managed binary is refused**, not overwritten: `/nix/store/…` and Homebrew prefixes get
  the right command for that manager instead. Silently replacing a file a manager believes it owns
  breaks the machine in a way that is hard to diagnose later
- **Never automatic.** agtx already refuses to answer codex's "Update now" dialog on the principle
  that it must not upgrade an agent binary behind the user's back; swapping its own would be
  incoherent. Two opt-outs for the *check*: `update_check = false` and `AGTX_NO_UPDATE_CHECK=1`
  (set in `docker/Dockerfile`, and what CI and the smoke runner should use)
- **Three files must agree on artifact naming** — `src/update/release.rs`, `install.sh` and
  `release.yml` — or `agtx update` 404s. `tests/update_tests.rs` greps the other two and asserts the
  match, because the alternative is finding the drift in a user's failed update
- `release.yml` publishes `<archive>.sha256` alongside each tarball. `install.sh` had fetched and
  verified them since it was written, but none were ever published, so its verification had never
  once run

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
fullscreen_on_enter = false  # When true, Enter opens the task's tmux pane fullscreen inside agtx
agent_hooks = true           # Write agent lifecycle-hook configs into worktrees (see Hook-Based Phase Status)
auto_trust = false           # Answer agents' trust / bypass-permission prompts by reading the pane (see First-Launch Dialogs)
update_check = true          # Daily GitHub release check + header notice (see Self-Update)

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
| `Ctrl+f` | Open the task popup fullscreen; press again to return to windowed mode |
| `d` | Show git diff for task |
| `D` | Open dependency-graph overlay |
| `m` | Move task forward (advance workflow) |
| `M` | Move Backlog task straight to Running |
| `R` | Start research for a Backlog task (in place, no column change) |
| `r` | Resume: Review → Running, or Running → Planning |
| `p` | Cyclic plugins only: Review → Planning (next phase) |
| `/` | Search tasks (jumps to and opens task) |
| `P` | Select workflow plugin |
| `u` | Update agtx (only bound when a newer release was found) |
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
| `Ctrl+d/u` or `PageDown/PageUp` | Page down/up — the pair the footer advertises |
| `Ctrl+n/p` or `Ctrl+Down/Up` | Scroll down/up five lines — bound, but not named in the footer |
| `Ctrl+g` | Jump to bottom |
| `Ctrl+f` | Toggle the task popup between windowed and fullscreen modes |
| `Ctrl+q` | Close popup |
| Other keys | Forwarded to tmux/agent (including `Esc`) |

**All three scroll rows** are delegated to the agent when the pane has no tmux scrollback — see
*Typing into a Task Pane*. There they are translated to `PageUp`/`PageDown`/`End` rather than passed
through, so the five-line and twenty-line distinction disappears and both pairs page.

The footer names only `C-d/u` because it has room for one pair, **not** because the other is
unbound. Do not reconcile the two by unbinding `C-n/p`.

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
output.

```
agent ──hook──► agtx hook --env <agent> ──► {worktree}/.agtx/status/{task_id}.json
                                                       │
                            maybe_spawn_session_refresh ┘──► apply_session_refresh
```

**Five of eight agents report their own state.** Every one of them takes a **project-local** hook
config, so agtx writes into the worktree and never into the user's global agent config; removing the
worktree removes the registration, and there is nothing to uninstall. `write_hook_config()` is the
writer, selected by `AgentSpec::hook_config` (`HookConfigKind`). `None` — no hooks, keep the
pane-hash heuristic — is a supported state, not a degraded one.

| Agent | Config file | Handler shape | Payload event key | Can report `Blocked` |
|---|---|---|---|---|
| claude | `.claude/settings.local.json` | `{hooks:[{type,command}]}` | `hook_event_name`, PascalCase | yes — `PermissionRequest`, `Notification` |
| gemini | `.gemini/settings.json` | same | `hook_event_name`, PascalCase | yes — `Notification` |
| cursor | `.cursor/hooks.json` | **flat `{command}`** | `hook_event_name`, camelCase | no |
| grok | `.grok/hooks/agtx.json` | `{hooks:[{type,command}]}` | `hookEventName`, **snake_case value** | yes — `Notification` scoped to `permission_prompt` |
| antigravity | `.agents/hooks.json` | keyed by hook *name*, then event | **none** — passed as `--event` | no |
| codex | `.codex/hooks.json` | `{description, hooks:{…}}` | `hook_event_name`, PascalCase | mapped, **not deployed** |
| opencode, copilot, pi | — | — | — | no hooks; see `HookConfigKind` |

The formats are **not interchangeable**, and every mismatch below fails silently — a valid-looking
config in the worktree and no status file ever written. This is why `tests/smoke/agent_smoke.py` is
the gate: the unit suite cannot see "the agent ignored it".

- **cursor** takes a flat `[{command}]` list; with Claude's `{hooks:[{type,command}]}` wrapper the
  file parses, loads, and fires nothing
- **grok** registers `PreToolUse` and reports `pre_tool_use`, so both spellings must map — that is
  `squash()`. It also sends `hookEventName`/`sessionId`/`transcriptPath` in camelCase, hence the
  serde aliases on `HookPayload`
- **antigravity's `PreToolUse` is not subscribable.** Its `decision` output is *required* and
  anything else reads as a refusal, so registering it blocks every tool call. Answering `"allow"`
  would make a liveness reporter into a permission granter — the line agtx declines to cross with
  trust dialogs. `PostToolUse` carries the heartbeat instead, and does include the tool name despite
  its docs. Its two event shapes also differ: tool events group under a `matcher`, the rest are a
  flat handler list, and the wrong shape is ignored
- **codex's `hooks` key in `.codex/config.toml` is a table, not a path.** `hooks.json` is
  auto-discovered; assigning a string to that key makes codex reject the whole config
  (`invalid type: string, expected struct HooksToml`) and lose its MCP server with it

**Codex is off, deliberately.** Its hooks work and speak Claude's schema, but a project-local
`.codex/hooks.json` triggers a startup review — *"N hooks are new or changed. Hooks can run outside
the sandbox after you trust them."* — whose only enabling answer trusts **every** hook the repo
ships, as does `--dangerously-bypass-hook-trust`. Not agtx's decision, on the same reasoning as
never choosing codex's "Update now". The vocabulary is mapped and the writer exists, so enabling it
is one field if codex gains per-hook trust. The dialog is in `AgentSpec::dialogs` regardless
(answered `3`, *Continue without trusting*, `security: false` because it declines rather than
grants) — a project shipping its own hooks file parks every codex task there whether or not agtx
writes one.

- **Writers merge where the file is shared.** Claude's and Gemini's hooks live in the same file as
  their MCP config, and `.claude`/`.gemini`/`.codex` are in `AGENT_CONFIG_DIRS`, so a project
  shipping its own settings has them copied into every worktree. `merge_claude_hooks()` keeps user
  entries on the same events and replaces only agtx's own, matched on the `" hook --env"` invocation
  rather than the binary path, so redeploying is idempotent and a moved agtx still recognises its own
  work. `write_hook_config` runs **after** `write_mcp_config` because both read-modify-write those
  two files
- **The hook command is task-agnostic**: `agtx hook --env <agent>`. The task comes from
  `AGTX_TASK_ID` / `AGTX_WORKTREE`, set on the tmux **window** by `create_window` (tmux `-e`, so a
  resumed or switched agent inherits them). Baking the task id in breaks `skip_worktree`, where every
  task shares one settings file. The hook exits silently when those are absent, so a registration
  stays inert outside agtx, and when `CLAUDE_JOB_DIR` is set, since a backgrounded task inherits its
  parent's env
- **`--event <Name>`** supplies the event for an agent whose payload carries none
  (`HookEventSource::Argv`, antigravity only). The payload wins when it has one
- **`hook_events(kind)` and `map_hook_event(kind, …)` are the two halves of one contract** and live
  in the same file so a test can guard the drift. An event registered but unmapped fires and reports
  nothing; an event mapped but unregistered pays a process spawn to decide nothing. Both directions
  are asserted. The kind is a *parameter*, not a fallback chain: `Stop` means "turn over" to four
  agents while Gemini's equivalent is `AfterAgent` and Cursor's is lowercase `stop`, so one shared
  table would let a registration typo succeed against the wrong agent's arm
- **Grok also scans `.claude/settings*.json` and `.cursor/hooks.json`** for vendor compatibility, so
  in a multi-phase-agent worktree it fires agtx's Claude- and cursor-registered hooks too. Mostly
  inert: the vocabulary is chosen by the agent named in the command agtx wrote, and grok's snake_case
  payloads do not resolve against Claude's PascalCase arm. **Cursor's arm is the exception** — it is
  lowercase and contains `stop`, which is exactly what grok reports, so a grok turn can write a
  record labelled `agent: "cursor"`. The *state* is right either way (both map `stop` to `Waiting`)
  and nothing reads that label, so this is a wrong name on a correct record rather than a wrong
  status
- **Claude's registered events** (verified against Claude Code 2.1.247): `SessionStart`,
  `UserPromptSubmit`, `PreToolUse` (heartbeat) → `working`; `PermissionRequest`, `Notification` →
  `blocked`; `Stop`, `StopFailure` → `waiting`; `SessionEnd` → `ended`. Unregistered names are
  ignored by the agent
- **`src/agent/hook_status.rs`** is pure (no tmux/DB/TUI types): event mapping, atomic
  write-then-rename, staleness, and `merge_event`'s guard preventing a late `PreToolUse` from
  clearing a fresh `Blocked`
- **Precedence** in the refresh thread: artifact → `Ready` > window gone → `Exited` > hook status >
  pane-hash heuristic. Only *liveness* is replaced; artifact detection is untouched
- **Purely additive**: no status file means the pre-existing 15s pane-hash heuristic runs unchanged.
  A `working` record older than `HOOK_STALE_SECS` (300s) is distrusted and also falls back
- The pane capture is skipped only on a **fresh `Working`** report — one fewer `capture-pane` per
  task per refresh, and proof the pane is past whatever gated startup. A `waiting`/`ended` record
  never expires, so letting one suppress the capture would hide a dialog rendered after a relaunch,
  which is exactly the resume path. Codex's MCP approval auto-dismiss runs outside that branch so it
  fires either way
- **Consumers**: an agent-reported `Blocked` fires the orchestrator stuck-task notification
  *immediately* (with the reason text from `blocked_reasons`), where `Idle` keeps its 60s settle
  because it is still a guess. A **trust-blocked** task is excluded from that notification
  altogether: the orchestrator's only remedies are a nudge — which would be typed into the dialog —
  and escalation, and only the user can answer it. The merge-conflict trigger fires on `Ready` and
  `Idle` only, so it never sends into a `Blocked` pane. MCP `get_task` exposes `agent_state` +
  `blocked_reason`, read straight from the file — it works cross-process precisely because it is a
  file, not TUI state
- **Binary-path drift**: the absolute `agtx` path from `current_exe()` is baked into the hook command
  *and* every MCP config a worktree gets, so moving or reinstalling agtx would silently break both
  for worktrees created earlier. `write_skills_to_worktree` records the deploying binary in
  `.agtx/deployed-by`; `refresh_stale_worktree_configs` re-deploys mismatched worktrees on a
  background thread at startup. `is_agtx_hook_command` matches on the `" hook --env"` invocation
  rather than the binary path so the merge still recognises its own entries across a move
- **Not wired**: codex (hook-trust review, above); opencode, whose lifecycle callbacks are a
  TypeScript plugin API rather than shell commands, so it would need agtx to ship a JS shim; and
  copilot, whose hook support is *unknown* rather than absent — it is not installed on any machine
  this was measured on, and that distinction is recorded in its spec so it is not filled in from
  documentation

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

# Per-agent smoke tests — real binaries, real auth, real tokens (opt-in, never CI)
tests/smoke/agent_smoke.py                    # installed agents x the agtx plugin
python3 tests/smoke/test_agent_smoke.py       # tests for the harness itself

# Real-tmux pane input: ordering, escaping, pane sizing, reconnect, latency (opt-in)
AGTX_TMUX_IT=1 cargo test --test tmux_control_tests -- --nocapture
```

`tmux_control_tests.rs` starts throwaway tmux servers and asserts on the **bytes a program in the
pane received**, not on rendered output — a redraw hides a reordering that a byte comparison
catches. Skipped tests name their reason instead of passing quietly. It has only been run on macOS
(tmux 3.5a); run it on Linux too.

The smoke runner answers the one question the Rust suite cannot: **does a real agent binary actually
receive its work?** Unit tests mock `TmuxOperations` and `agent_parity_tests.rs` pins the strings
agtx builds, so neither can see "the agent ignored it" — the gap where every silent-hang bug so far
has lived. Per phase it asserts the command was *submitted* (not parked in a composer), that a marker
file carries the **task id** that was passed in, that the artifact appeared and the phase advanced,
and that the session is still usable (process alive, no dialog on screen). Dialogs are never
pre-answered — they are the thing under test. Details: `tests/smoke/README.md`.

`cargo run --example agent_matrix` (source in `tests/smoke/`) dumps `AGENT_SPECS` +
`BUNDLED_PLUGINS` as JSON. The smoke runner reads it instead of keeping its own copy of the agent
table, and `cargo test` compiles it, so a new spec field breaks the build rather than drifting.

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
Half of this is now one table entry; the other half is still per-agent match arms in `app.rs`.

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
9b. Set `hook_config` / `hook_event_source` on the spec if the agent supports lifecycle hooks, add
   its `HookConfigKind` arm to `write_hook_config`, `hook_events` and `map_hook_event`, and extend
   `vocabulary()` in `tests/hook_status_tests.rs`. Leave `hook_config: None` until a real run writes
   a `.agtx/status/*.json`: a wrong handler shape or event spelling fails silently (see
   *Hook-Based Phase Status*), and `None` is a supported state, not a failure
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
