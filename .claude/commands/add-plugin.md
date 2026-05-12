---
description: Integrate a spec-driven/skill framework as a bundled agtx plugin. Pass a GitHub repo URL to auto-generate the plugin.toml, or run without arguments to write one from scratch.
---

# Add agtx Plugin

Integrate a spec-driven development framework or skill library as a bundled agtx plugin.

## Input

Optionally, a GitHub repository URL is provided as the argument (e.g. `https://github.com/org/repo`).
If no URL is given, ask the user which framework they want to integrate and gather the necessary details interactively.

## Steps

### 1. Gather information about the framework

**If a GitHub URL was provided**, use the GitHub CLI to inspect the repo:

```bash
gh repo view <owner>/<repo> --json name,description
gh api repos/<owner>/<repo>/git/trees/HEAD?recursive=1 | jq '[.tree[].path]'
gh api repos/<owner>/<repo>/contents/README.md | jq -r '.content' | base64 -d
```

**If no URL was provided**, ask the user:
- Framework name and slash command namespace (e.g. `gsd`, `opsx`)
- Which phases it covers and what commands it uses
- How it is installed (npm, git clone, shell script)
- Which agents it supports

In either case, look for:
- The slash command namespace and per-phase commands
- Installation method: npm package, clone-and-run library, or shell installer
- Agent compatibility: Claude-only (hooks/skills), multi-agent (npm flags), or prompt-only (all agents)
- Artifact files that signal phase completion
- Directories/files that need to be propagated to each worktree (`copy_dirs`, `copy_files`, `copy_back`)
- A one-time project setup step (`preresearch` + artifact)
- Interactive CLI prompts at startup that need `auto_dismiss` or `prompt_triggers`
- Multi-milestone iteration (`cyclic = true`)

### 2. Study existing bundled plugins as reference

Read `plugins/*/plugin.toml` and pick the closest analog to use as a starting point.

Key patterns:
- `init_script` with `--{agent}` placeholder when the tool has per-agent flags
- `copy_back` so artifacts produced in one worktree are visible to others
- `{task}` in a command or prompt → phase is accessible directly from Backlog; omit `{task}` → phase requires a prior artifact
- `cyclic = true` only when the framework supports multi-milestone iteration (Review → Planning loop)

### 3. Determine agent compatibility

| Signal | Compatible agents |
|--------|------------------|
| Only `.claude/commands/` skills or `.claude/hooks/` | `["claude"]` |
| npm package with `--codex` / `--gemini` flags | `["claude", "codex", "gemini", "opencode"]` |
| Pure prompt-based, no interactive skills | all agents (omit `supported_agents`) |

### 4. Write the plugin

Create `plugins/<name>/plugin.toml`. The full field reference is in the "Creating a Plugin" section of `README.md` — use it as the authoritative guide for every available field and its semantics.

Omit sections and fields that don't apply to this framework. Start minimal and add only what is needed.

### 5. Register it in `src/skills.rs`

Add an entry to `BUNDLED_PLUGINS`:

```rust
(
    "<name>",
    "<one-line description>",
    include_str!("../plugins/<name>/plugin.toml"),
),
```

### 6. Add agent compatibility row to README

In the plugin compatibility table in `README.md`, add a row for the new plugin showing which agents are supported (✅ / 🟡 / ❌).

Also add a row to the plugin description table:

```markdown
| **<name>** | [<Framework Name>](<repo-url>) - <short description> |
```

### 7. Verify

```bash
cargo build 2>&1 | grep -E "error|warning"
```

Fix any compile errors (usually a missing comma in `BUNDLED_PLUGINS`).

## Output

Report what was created:
- Path to the generated `plugin.toml`
- The `src/skills.rs` entry added
- The README rows added
- Any ambiguities or fields left as TODOs that need manual review
