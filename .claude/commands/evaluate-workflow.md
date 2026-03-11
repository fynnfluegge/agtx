# Evaluate Spec-Driven Workflow for agtx Plugin Integration

You are evaluating a spec-driven coding agent workflow for integration into the **agtx plugin framework**. The user will provide a GitHub repository URL. Your job is to analyze the workflow and produce both a structured comparison and a draft `plugin.toml`.

## Input

GitHub repository URL: `$ARGUMENTS`

If no URL is provided, ask the user for one before proceeding.

## Step 1: Fetch and Analyze the Repository

Use web fetch and GitHub CLI to gather information:

1. **README and docs** — Fetch the repo's README for an overview of the workflow, phases, and installation
2. **Command/skill files** — Look for slash commands, skill definitions, or agent instructions (check directories like `commands/`, `skills/`, `.claude/`, `.gemini/`, `src/`)
3. **Installation method** — How is it installed? (npm/npx, git clone, manual copy, etc.)
4. **Artifact files** — What files does the workflow produce at each phase? (specs, plans, summaries, reviews)
5. **Agent support** — Which coding agents does it target? (Claude, Codex, Gemini, OpenCode, Copilot)
6. **Phase structure** — What phases does the workflow define? Map them to agtx's: preresearch, research, planning, running, review

## Step 2: Map to agtx Plugin Schema

For each `plugin.toml` field, determine the best mapping from the analyzed workflow. Here is the complete schema reference:

### Top-level fields

| Field | Type | Description |
|---|---|---|
| `name` | `String` | Plugin identifier (lowercase, hyphenated slug) |
| `description` | `Option<String>` | One-line human-readable description |
| `init_script` | `Option<String>` | Shell command run in worktree before agent starts. Supports `{agent}` placeholder (replaced with agent name: claude, codex, gemini, opencode, copilot) |
| `supported_agents` | `Vec<String>` | Agent whitelist. Empty array = all agents supported. Valid values: `claude`, `codex`, `gemini`, `opencode`, `copilot` |
| `cyclic` | `bool` | When `true`, enables Review -> Planning transition with incrementing phase counter (for multi-phase/iterative workflows) |
| `copy_dirs` | `Vec<String>` | Directories to copy from project root into worktrees (e.g., `[".specify", "openspec"]`) |
| `copy_files` | `Vec<String>` | Individual files to copy from project root into worktrees |

### `[artifacts]` — Files that signal phase completion

Polled every 100ms. Supports `*` wildcards (single directory level) and `{phase}` placeholder (substituted with phase number, tries zero-padded `01` then `1`).

| Field | Type | Description |
|---|---|---|
| `preresearch` | `Vec<String>` | Multiple files that must ALL exist to complete preresearch |
| `research` | `Option<String>` | Single artifact file path |
| `planning` | `Option<String>` | Single artifact file path |
| `running` | `Option<String>` | Single artifact file path |
| `review` | `Option<String>` | Single artifact file path |

### `[commands]` — Slash commands sent to the agent at each phase

Written in canonical form: `/namespace:command args`. Auto-translated per agent at runtime:
- Claude/Gemini: unchanged (`/ns:command`)
- OpenCode: colon to hyphen (`/ns-command`)
- Codex: slash to dollar + colon to hyphen (`$ns-command`)
- Copilot: prompt-only (no interactive commands)

| Field | Type | Description |
|---|---|---|
| `preresearch` | `Option<String>` | One-time setup command (runs only on first phase entry) |
| `research` | `Option<String>` | Research/discovery command |
| `planning` | `Option<String>` | Planning command |
| `running` | `Option<String>` | Execution command |
| `review` | `Option<String>` | Review/verification command |

### `[prompts]` — Task content templates sent after commands

Supports placeholders: `{task}` (task description), `{task_id}` (task ID), `{phase}` (phase number).

**Phase gating rule**: If a phase's command or prompt contains `{task}`, it can be entered directly from Backlog. Otherwise, it requires a prior phase's artifact to exist first.

| Field | Type | Description |
|---|---|---|
| `research` | `Option<String>` | Prompt for research phase |
| `planning` | `Option<String>` | Prompt when entering planning directly from Backlog |
| `planning_with_research` | `Option<String>` | Prompt when entering planning after research (usually empty — research provides context) |
| `running` | `Option<String>` | Prompt when entering running directly from Backlog |
| `running_with_research_or_planning` | `Option<String>` | Prompt when entering running after a prior phase (usually empty) |
| `review` | `Option<String>` | Prompt for review phase |

### `[prompt_triggers]` — Text patterns to wait for before sending prompts

Useful for interactive commands that ask a question before accepting input.

| Field | Type | Description |
|---|---|---|
| `research` | `Option<String>` | Pattern to wait for during research |
| `planning` | `Option<String>` | Pattern to wait for during planning |
| `running` | `Option<String>` | Pattern to wait for during running |
| `review` | `Option<String>` | Pattern to wait for during review |

### `[copy_back]` — Files/directories to copy from worktree back to project root

Keyed by phase name. Triggered when a phase's artifact is detected.

```toml
[copy_back]
preresearch = ["dir1", "file1"]
planning = ["output_dir"]
running = ["results_dir"]
```

### `[[auto_dismiss]]` — Rules to auto-dismiss interactive prompts

Each rule fires when ALL `detect` patterns are found in tmux pane content, then sends `response` keystrokes (newline-separated).

```toml
[[auto_dismiss]]
detect = ["Map codebase", "Skip mapping"]
response = "2\nEnter"
```

## Step 3: Produce Comparison Analysis

Output a structured analysis with these sections:

### Workflow Overview
Brief description of the workflow, its philosophy, and target use cases.

### Phase Mapping Table

| agtx Phase | Workflow Equivalent | Command | Artifact | Notes |
|---|---|---|---|---|
| preresearch | ... | ... | ... | ... |
| research | ... | ... | ... | ... |
| planning | ... | ... | ... | ... |
| running | ... | ... | ... | ... |
| review | ... | ... | ... | ... |

### Integration Assessment

For each dimension, rate as Full / Partial / None / N/A:

| Dimension | Rating | Notes |
|---|---|---|
| Phase coverage | | Which agtx phases are covered? |
| Artifact detection | | Does it produce clear, predictable output files? |
| Command structure | | Does it use slash commands or similar invocation? |
| Agent compatibility | | Which agents can it work with? |
| Install automation | | Can setup be scripted via `init_script`? |
| Cyclic support | | Does it support iterative phase loops? |
| File propagation | | Does it need `copy_dirs`/`copy_files`/`copy_back`? |
| Interactive prompts | | Does it have prompts needing `prompt_triggers` or `auto_dismiss`? |

### Gaps and Considerations
- What doesn't map cleanly?
- What would require custom skills (`.md` files in `plugins/{name}/skills/`)?
- Are there agent-specific limitations?

## Step 4: Generate Draft plugin.toml

Produce a complete, valid TOML file that can be saved to `plugins/{name}/plugin.toml`. Include comments explaining non-obvious choices. Omit sections that don't apply (don't include empty tables).

Reference these existing integrations as style guides:
- **Simple** (spec-kit): commands + artifacts + copy_dirs, no prompts needed when commands take `{task}` inline
- **Medium** (bmad): init_script + commands + prompts + copy_back + copy_dirs
- **Complex** (gsd): preresearch + cyclic + prompt_triggers + auto_dismiss + copy_files + copy_back

## Output Format

Present your findings as:

1. **Workflow Overview** (2-3 sentences)
2. **Phase Mapping Table**
3. **Integration Assessment Table**
4. **Gaps and Considerations** (bullet list)
5. **Draft `plugin.toml`** (in a TOML code block, ready to copy)
6. **Next Steps** (what the user should do to test/refine the integration)
