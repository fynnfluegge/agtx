# agtx benchmarks 📊

## SWE-bench Lite

Runs AI coding agent workflows against [SWE-bench Lite](https://github.com/princeton-nlp/SWE-bench)
(300 real GitHub bug-fix tasks). Uses agtx as the agent runner, drives it via its MCP server,
collects git diff patches, and outputs SWE-bench-compatible results.

> **All commands below assume you are in the `benchmark/` directory.**
> ```bash
> cd benchmark
> ```

### Sandbox Mode (Docker)

In sandbox mode (`--sandbox`), each task runs inside its official SWE-bench Docker image.
The repo is pre-installed in a working conda environment (`testbed`) so agents can run
`pytest` and all dependencies without any setup.

**When to use:** Always recommended. Agents fail on the host because SWE-bench repos require
specific Python versions and C extensions that aren't available outside the containers.

**One-time setup:**
```bash
cd swebench

# Build the tools image (tmux + Node.js + Claude Code + Grok)
python prebake_images.py --verbose

# Build the Linux agtx binary (Ubuntu 22.04 / glibc 2.35)
bash build_linux_binary.sh
```

The tools image is the base for the shared Docker named volume `agtx-swebench-tools`.
On the **first benchmark run**, this volume is created automatically and populated from the
tools image — tmux, Node.js, Claude Code, Grok, and their shared libraries are copied in once and
then mounted read-only into every instance container. Subsequent runs skip this step.

To force a refresh (e.g. after updating Claude Code or Grok):
```bash
docker volume rm agtx-swebench-tools
python prebake_images.py --force --verbose
```

**Agent credentials** are copied into each container as snapshots (host files are never modified):
Claude reads `~/.claude/` + `~/.claude.json`, Grok reads `~/.grok/auth.json` + `~/.grok/config.toml`
(or `XAI_API_KEY` from the host environment, passed through to the container's tmux env).

**Run in sandbox mode** (Linux binary required — agents run inside Ubuntu 22.04 containers):
```bash
python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --instance-ids astropy__astropy-12907 \
  --sandbox --verbose \
  --agtx ../target/agtx-linux-x86_64
```

**Attach to a running container** to watch the agent:
```bash
docker exec -it swebench-astropy-astropy-12907 tmux -L agtx attach -t testbed:1
# Ctrl+b 0 → agtx board   Ctrl+b 1 → agent session   Ctrl+b d → detach
```

**Cleanup** if the benchmark crashes:
```bash
docker rm -f swebench-astropy-astropy-12907
docker volume rm agtx-swebench-tools  # only if tools need refreshing
```

---

### Prerequisites

**Docker** (required for sandbox mode):
```bash
# macOS — install Docker Desktop from https://docs.docker.com/desktop/mac/install/
# Ubuntu/Debian
apt install docker.io
```

**agtx** (built from repo root):
```bash
cargo build --release
```

**uv** (Python package manager):
```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**tokscale** (token/cost tracking — optional but recommended):
```bash
npm install -g tokscale
```
If not installed, `cost_usd` and token fields will be `null` in results. tokscale filters by agent
for Claude, Codex, Gemini, Copilot and OpenCode; for agents it has no filter for (Cursor, Grok) the
fields are `null` in sandbox runs and unfiltered — summed across all agents — in host-mode runs.

**tmux** (required — agtx runs agent sessions inside tmux):
```bash
# macOS
brew install tmux

# Ubuntu/Debian
apt install tmux
```

At least one coding agent CLI must be installed and authenticated:
- [Claude Code](https://docs.anthropic.com/en/docs/claude-code) — `npm install -g @anthropic-ai/claude-code`
- [Gemini CLI](https://github.com/google-gemini/gemini-cli) — `npm install -g @google/gemini-cli`
- [Codex CLI](https://github.com/openai/codex) — `npm install -g @openai/codex`
- [Grok Build](https://docs.x.ai/build/overview) — `npm install -g @xai-official/grok`, then `grok login`

---

### Setup

**1. Initialize the Python environment:**
```bash
cd swebench
uv sync
cd ..
```

This creates a `.venv` inside `swebench/` and installs all dependencies (including `swebench`,
so evaluation can run via `uv run` without a separate swebench install).
Only needed once (or after updating `pyproject.toml`).

**2. Create a config file** for your benchmark run.

Config files live in `swebench/configs/`. Each file is a standard agtx
`ProjectConfig` TOML that gets written to `.agtx/config.toml` in every cloned repo.
It controls which agent and plugin are used.

Minimal example — `configs/claude-void.toml`:
```toml
default_agent = "claude"
workflow_plugin = "void"
```

Structured workflow — `configs/claude-agtx.toml`:
```toml
default_agent = "claude"
workflow_plugin = "agtx"
worktree_dir = ".agtx/worktrees"
```

Sandbox-optimised (agent works directly in `/testbed`, no worktree) — `configs/claude-agtx.toml` with `skip_worktree`:
```toml
default_agent = "claude"
workflow_plugin = "agtx"
worktree_dir = ".agtx/worktrees"
skip_worktree = true   # recommended for sandbox: agent works in /testbed directly
```

Mixed agents (different agent per phase) — `configs/gemini-claude-codex-agtx.toml`:
```toml
default_agent = "claude"
workflow_plugin = "agtx"

[agents]
planning = "gemini"
running  = "claude"
review   = "codex"
```

Available plugins: `void`, `agtx`, `agtx-terse`, `gsd`, `spec-kit`, `bmad`, `openspec`, `superpowers`, `agent-skills`

> [!WARNING]
> `void` is for interactive use, not benchmark runs. With no phase command or prompt, agtx prefills
> the task in the agent's input box and waits for a human to press Enter, so an automated run stalls
> with "Pane stable but no finish marker". Use a plugin with artifacts (`agtx`, `agtx-terse`, …)
> for benchmarks.

Pre-built configs for common single-agent and multi-agent combinations are in [`swebench/configs/`](swebench/configs/).

#### Sandbox-specific config keys

These keys are only used in sandbox mode (`--sandbox`) and are ignored otherwise:

| Key | Description |
|-----|-------------|
| `skip_worktree = true` | Agent works directly in `/testbed` instead of a git worktree. Recommended for sandbox runs. |
| `sandbox_init = [...]` | List of shell commands run inside the container (as `/home/bench`) after tools are wired but before the TUI starts. Use for installing per-config tooling (e.g. rtk, caveman). |

Example with `sandbox_init`:
```toml
default_agent = "claude"
workflow_plugin = "agtx"
skip_worktree = true

sandbox_init = [
    # Install rtk token-compression hook
    "curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh",
    "export PATH=$HOME/.local/bin:$PATH && rtk init -g",
]
```

`sandbox_init` commands run with `HOME=/home/bench` and have `PATH` including `/home/bench/.local/bin`.
Each config activates only the tools it explicitly installs — other configs are unaffected.

---

### Running

> **Note:** The examples below use `--agtx ../target/release/agtx` (the host binary). For sandbox
> mode (`--sandbox`), the Linux x86_64 binary is required — use `--agtx ../target/agtx-linux-x86_64`
> instead (built via `bash swebench/build_linux_binary.sh`).

**Single task:**
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-void.toml \
  --instances 1 --verbose \
  --agtx ../target/release/agtx
```

**Specific instance IDs:**
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-void.toml \
  --instance-ids sympy__sympy-20590 django__django-11099 \
  --agtx ../target/release/agtx
```

**Full 300-task run:**
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --agtx ../target/release/agtx
```

**Parallel tasks:**
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --concurrency 4 \
  --agtx ../target/release/agtx
```

**Resume an interrupted run** (pass the same `--output-dir`):
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --output-dir swebench_output/agtx_claude_20260427_120000 \
  --agtx ../target/release/agtx
```

**Hard mode** (prose only — no code blocks or stack traces):
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --hard \
  --agtx ../target/release/agtx
```

#### All options

| Flag | Default | Description |
|------|---------|-------------|
| `--config PATH` | *(required)* | agtx config.toml for this run |
| `--instances N` | all 300 | Run first N tasks |
| `--instance-ids ID...` | — | Run specific instance IDs |
| `--concurrency N` | 1 | Parallel tasks |
| `--sandbox` | off | Run each task inside its SWE-bench Docker image (recommended) |
| `--output-dir PATH` | `./swebench_output/{config-name}_{ts}/` | Output directory |
| `--workdir PATH` | `/tmp/swebench_repos` | Repo clone directory (non-sandbox only) |
| `--agtx PATH` | `./target/release/agtx` | agtx binary — must be a Linux x86_64 binary for sandbox mode (use `../target/agtx-linux-x86_64`) |
| `--phase-timeout SECS` | 1200 | Per-phase max seconds (20 min) |
| `--model-name STRING` | `{config-stem}` (e.g. `claude-agtx`) | Label in predictions.jsonl |
| `--split STRING` | `test` | HuggingFace dataset split |
| `--verbose` / `-v` | off | Print step-by-step progress to stderr (good for debugging) |
| `--hard` | off | Strip fenced code blocks and stack traces from the problem statement, keeping prose and inline code. The agent must find and fix the bug from first principles. |

---

### Output

Results are written to `./swebench_output/{config-name}_{timestamp}/`:

**`predictions.jsonl`** — SWE-bench format, one line per task:
```json
{"instance_id": "sympy__sympy-20590", "model_name_or_path": "agtx-agtx-claude", "model_patch": "diff --git ..."}
```

**`results.json`** — detailed results with timing and cost:
```json
[{
  "instance_id": "sympy__sympy-20590",
  "status": "success",
  "duration_seconds": 342.1,
  "cost_usd": 0.23,
  "cost_tokens": 54000,
  "model_patch": "diff --git ...",
  "error": null
}]
```

Status values: `success`, `timeout`, `error`, `setup_error`

**Check results:**
```bash
cat swebench_output/*/results.json | \
  python3 -c "import json,sys; r=json.load(sys.stdin); print(f'{sum(1 for x in r if x[\"status\"]==\"success\")}/{len(r)} success')"
```

---

### Capturing the agent's session transcript

The `predictions.jsonl` / `results.json` only capture the **git diff and metrics** — not what the
agent actually said or did. To inspect the full turn-by-turn conversation (tool calls, reasoning,
and — importantly — whether an injected skill/plugin actually influenced behavior), read the Claude
Code **session transcript**.

**Where Claude Code writes transcripts.** For every session, Claude Code appends a JSONL file:

```
~/.claude/projects/<encoded-cwd>/<session-id>.jsonl
```

`<encoded-cwd>` is the working directory with every `/` and `.` replaced by `-`. Each line is one
event (`type` = `user` / `assistant` / …) carrying the full message content, tool uses, and a `cwd`
field. On the host you can find your own sessions there; the same layout exists **inside the
container** for the agent Claude Code runs.

**In sandbox mode the transcript lives inside the container and is destroyed when the container
stops.** The agent runs as user `bench` with `HOME=/home/bench`, working in `/testbed` (when
`skip_worktree = true`) or in its worktree under `/testbed/.agtx/worktrees/…`. So the transcript is at:

```
# skip_worktree = true  (agent works in /testbed)
/home/bench/.claude/projects/-testbed/<session-id>.jsonl
```

Containers are removed automatically at the end of each task, so you **must copy the transcript out
while the container is still alive**. A single `docker cp` "before the run finishes" is unreliable —
the container often stops before you catch it. Instead, run a **polling loop that re-copies every few
seconds** in a separate shell (start it right after launching the benchmark):

```bash
C=swebench-astropy-astropy-12907
DEST=/tmp/transcript_capture

# Poll while the container is alive, re-copying the projects tree each pass.
for i in $(seq 1 180); do
  docker ps --filter name=$C -q | grep -q . || { echo "container gone"; break; }
  docker cp $C:/home/bench/.claude/projects "$DEST" 2>/dev/null
  sleep 5
done

# List what landed
find "$DEST" -name '*.jsonl'
```

> **`skip_worktree = true` → one session per phase.** Each agtx phase (Planning / Running / Review)
> is a **separate** Claude Code session, so you get one `<session-id>.jsonl` per phase, not one file
> for the whole task. `void` runs as a single session.

#### Analysing the transcript (do NOT use a naive grep)

⚠️ **A raw `grep` across the JSONL gives false positives.** SessionStart-hook plugins (focus,
ponytail, caveman) inject their entire `SKILL.md` into the transcript as an `attachment` event — and
that injected text *documents* the very tags you're grepping for. So
`grep -c 'context-commit' *.jsonl` counts the **injected instructions**, not what the model actually
emitted. In one run a raw grep reported 34 CSP-tag hits while the model emitted **zero** — all 34
were inside the injected skill.

To measure real behavior you must parse the JSONL, keep only `type == "assistant"` events, and count
tags in their `text` content blocks:

```bash
python3 - "$DEST"/**/-testbed/*.jsonl << 'PY'
import json, sys
TAGS = ["context-commit","context-scope","context-checkpoint","context-transient","context-revoke"]
for path in sys.argv[1:]:
    injected = emitted = turns = 0
    counts = {}
    for line in open(path):
        try: d = json.loads(line)
        except Exception: continue
        raw = json.dumps(d)
        # SessionStart-injected SKILL.md lands as an attachment — exclude it
        if d.get("type") == "attachment" and "Always-on context discipline" in raw:
            injected += 1
        if d.get("type") != "assistant":
            continue
        turns += 1
        content = d.get("message", {}).get("content", [])
        text = "".join(c.get("text","") for c in content
                        if isinstance(c, dict) and c.get("type") == "text") \
               if isinstance(content, list) else (content if isinstance(content, str) else "")
        for t in TAGS:
            n = text.count("<" + t)
            if n: counts[t] = counts.get(t, 0) + n
    print(f"{path}")
    print(f"  SKILL.md injected (attachment): {injected}")
    print(f"  assistant turns: {turns}")
    print(f"  tags EMITTED BY MODEL: {counts or 'NONE'}")
PY
```

> **Note:** the CSP tags are HTML-style (`<context-commit>`) so the Claude Code TUI **hides them in
> the rendered pane** — `tmux capture-pane` will never show them. You must read the JSONL to see
> whether the model actually emitted them.

> **`void` quirk:** with the `void` plugin the phase prompt is *pasted* into the input box and never
> submitted — agtx prefills it and waits for a human Enter — which shows as "Pane stable but no
> finish marker". To unstick a run you are capturing from:
> `docker exec $C tmux -L agtx send-keys -t testbed:1 Enter`.

---

### Cleanup

#### Non-sandbox cleanup

After an interrupted or completed run, stale state (worktrees, tmux sessions, SQLite DBs) can be
cleaned up with the included script.

**Clean all instances:**
```bash
./swebench/cleanup.sh
```

**Clean a specific instance:**
```bash
./swebench/cleanup.sh astropy__astropy-12907
```

**Clean multiple specific instances:**
```bash
./swebench/cleanup.sh astropy__astropy-12907 sympy__sympy-20590
```

The script removes `.agtx/` dirs, tmux sessions, and central SQLite project DBs.
Repo clones under `/tmp/swebench_repos/` are preserved so the next run skips re-cloning.

Override the repo clone directory with `SWEBENCH_WORKDIR`:
```bash
SWEBENCH_WORKDIR=/my/custom/path ./swebench/cleanup.sh
```

#### Sandbox cleanup

Containers are stopped and removed automatically when a run completes or fails. If the process
is killed mid-run, containers may be left running:

```bash
# Stop and remove a specific container
docker rm -f swebench-astropy-astropy-12907

# Stop and remove all swebench containers
docker ps -a --filter name=swebench- -q | xargs docker rm -f
```

The tools volume is persistent across runs. Remove it only if you want to repopulate it
(e.g. after rebuilding the tools image):
```bash
docker volume rm agtx-swebench-tools
```

---

### Evaluation

After the run, the benchmark prints exact commands to copy-paste. Evaluate patches against the
SWE-bench test harness (requires Docker running):

```bash
uv run python -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Lite \
  --predictions_path swebench_output/claude-agtx_20260427_120000/predictions.jsonl \
  --run_id claude-agtx-1746345600
```

The harness runs tests in Docker containers — each task gets a fresh repo checkout
and the patch is applied and tested in isolation.

### Report

After evaluation, print a summary table with resolved status, duration, cost, and token usage:

```bash
uv run python swebench/report.py \
  --results swebench_output/claude-agtx_20260427_120000/results.json \
  --logs logs/run_evaluation/claude-agtx-1746345600/
```

Example output:
```
┌───────────────────────┬─────────────┬──────────┬───────┬────────┐
│ Instance              │ Status      │ Duration │ Cost  │ Tokens │
├───────────────────────┼─────────────┼──────────┼───────┼────────┤
│ astropy/astropy-12907 │ ✅ resolved │ 4m 57s   │ $0.95 │ 1.9M   │
├───────────────────────┼─────────────┼──────────┼───────┼────────┤
│ astropy/astropy-14182 │ ❌ failed   │ 5m 57s   │ $1.66 │ 5.3M   │
└───────────────────────┴─────────────┴──────────┴───────┴────────┘

1/2 resolved  ·  10m 54s total  ·  $2.60 total  ·  7.3M tokens
```

The exact commands (with correct paths and run_id) are printed at the end of every benchmark run.

---

### How it works

```
benchmark.py
  ├── [sandbox] Pulls SWE-bench Docker image, starts container with tools volume mounted
  ├── [sandbox] Copies credentials, wires tools (tmux/node/claude via symlinks), runs sandbox_init
  ├── [non-sandbox] Clones each repo at base_commit → /tmp/swebench_repos/{instance_id}/
  ├── Writes .agtx/config.toml (your config file) into each repo/container
  ├── Starts agtx TUI per task in detached tmux (tmux -L agtx, inside container in sandbox mode)
  ├── Spawns agtx mcp-serve as subprocess (JSON-RPC 2.0 over stdio)
  ├── Drives task via MCP:
  │     create_task → move_forward (Planning)
  │     → poll planning artifact → move_forward (Running)
  │     → poll running artifact  → move_forward (Review)
  │     → poll review artifact   → git diff HEAD...{branch} → move_to_done
  ├── Snapshots tokscale before/after running phase for token counts
  └── Appends to predictions.jsonl + rewrites results.json atomically
```

Phase completion detection (per phase, in priority order):
1. **Artifact file** — if the plugin defines an artifact for that phase (e.g. `.agtx/plan.md`,
   `.agtx/execute.md`, `.agtx/review.md` for the `agtx`/`agtx-terse` plugins), polls for its
   existence every 5 seconds
2. **Claude finish marker** — detects `✻ [Word] for Xs` followed by a `❯` prompt in the pane;
   confirmed within 5 seconds of the marker appearing
3. **Pane stability** — fallback for plugins without artifacts when no finish marker is seen: pane
   content identical for 2 consecutive 5-second checks (10s stable). If stability is reached
   without a finish marker, a warning is emitted — the agent may be stuck, errored, or waiting
   for manual approval
