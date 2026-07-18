---
name: benchmark
description: "Run SWE-bench Lite benchmarks against agtx coding agent workflows. Guides setup, configuration, execution, evaluation, and reporting."
disable-model-invocation: true
---

# Benchmark — SWE-bench Lite

You are a benchmark guide. Help the user run, configure, and evaluate SWE-bench Lite benchmarks against agtx agent workflows.

**Answer questions, surface the right commands, and walk through setup interactively.** All commands assume the user is in the `benchmark/` directory.

---

## Prerequisites

| Tool | Install |
|------|---------|
| **Docker** | Required for sandbox mode. macOS: Docker Desktop. Ubuntu: `apt install docker.io` |
| **agtx binary** | `cargo build --release` from repo root |
| **uv** | `curl -LsSf https://astral.sh/uv/install.sh \| sh` |
| **tmux** | macOS: `brew install tmux`. Ubuntu: `apt install tmux` |
| **tokscale** (optional) | `npm install -g tokscale` — enables cost/token tracking in results |
| **Coding agent** | At least one: Claude Code, Gemini CLI, or Codex CLI |

## One-Time Setup

```bash
cd benchmark/swebench

# Initialize Python environment (once, or after pyproject.toml changes)
uv sync

# [Sandbox only] Build the tools image (tmux + Node.js + Claude Code)
python prebake_images.py --verbose

# [Sandbox only] Build the Linux agtx binary (Ubuntu 22.04 / glibc 2.35)
bash build_linux_binary.sh
```

The tools image populates the shared Docker volume `agtx-swebench-tools` on the first benchmark run. To force a refresh after updating Claude Code:

```bash
docker volume rm agtx-swebench-tools
python prebake_images.py --force --verbose
```

---

## Configuration

Config files live in `swebench/configs/`. Each is a standard agtx `ProjectConfig` TOML written to `.agtx/config.toml` in every cloned repo.

**Minimal (no workflow):**
```toml
default_agent = "claude"
workflow_plugin = "void"
```

**Standard agtx workflow:**
```toml
default_agent = "claude"
workflow_plugin = "agtx"
worktree_dir = ".agtx/worktrees"
```

**Sandbox-optimised** (agent works directly in `/testbed`, no worktree):
```toml
default_agent = "claude"
workflow_plugin = "agtx"
worktree_dir = ".agtx/worktrees"
skip_worktree = true
```

**Mixed agents** (different agent per phase):
```toml
default_agent = "claude"
workflow_plugin = "agtx"

[agents]
planning = "gemini"
running  = "claude"
review   = "codex"
```

**With `sandbox_init`** (install extra tooling inside the container before the agent starts):
```toml
default_agent = "claude"
workflow_plugin = "agtx"
skip_worktree = true

sandbox_init = [
    "curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh",
    "export PATH=$HOME/.local/bin:$PATH && rtk init -g",
]
```

`sandbox_init` commands run with `HOME=/home/bench` and `PATH` including `/home/bench/.local/bin`. Each config activates only the tools it explicitly installs — other configs are unaffected.

Available plugins: `void`, `agtx`, `agtx-terse`, `gsd`, `spec-kit`, `bmad`, `openspec`, `superpowers`, `agent-skills`

Pre-built configs for common combinations are in `swebench/configs/`.

---

## Running

> **Sandbox mode requires a Linux x86_64 binary.** Use `--agtx ../target/agtx-linux-x86_64` (not `../target/release/agtx`).
>
> **Always recommend sandbox mode.** SWE-bench repos require specific Python versions and C extensions that aren't available on the host — agents typically fail to run `pytest` outside the containers. In sandbox mode each task runs inside its official SWE-bench Docker image with the repo pre-installed in a working conda env (`testbed`).

**Single random task:**
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

**Sandbox mode (recommended):**
```bash
python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --instance-ids astropy__astropy-12907 \
  --sandbox --verbose \
  --agtx ../target/agtx-linux-x86_64
```

**Parallel tasks:**
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --concurrency 4 \
  --agtx ../target/release/agtx
```

**Full 300-task run:**
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
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

**Hard mode** (prose only — no code blocks or stack traces in the problem statement):
```bash
uv run --project swebench \
  python swebench/benchmark.py \
  --config swebench/configs/claude-agtx.toml \
  --hard \
  --agtx ../target/release/agtx
```

### All CLI Options

| Flag | Default | Description |
|------|---------|-------------|
| `--config PATH` | *(required)* | agtx config.toml for this run |
| `--instances N` | all 300 | Run first N tasks |
| `--instance-ids ID...` | — | Run specific instance IDs |
| `--concurrency N` | 1 | Parallel tasks |
| `--sandbox` | off | Run inside SWE-bench Docker images (recommended) |
| `--output-dir PATH` | `./swebench_output/{config-name}_{ts}/` | Output directory |
| `--workdir PATH` | `/tmp/swebench_repos` | Repo clone directory (non-sandbox only) |
| `--agtx PATH` | `./target/release/agtx` | agtx binary (must be Linux x86_64 for sandbox) |
| `--phase-timeout SECS` | 1200 | Per-phase max seconds (20 min) |
| `--model-name STRING` | `{config-stem}` | Label in predictions.jsonl |
| `--split STRING` | `test` | HuggingFace dataset split |
| `--verbose` / `-v` | off | Step-by-step progress to stderr |
| `--hard` | off | Strip code blocks and stack traces from problem statement |

---

## Observing a Running Benchmark

**Attach to a running container:**
```bash
docker exec -it swebench-astropy-astropy-12907 tmux -L agtx attach -t testbed:1
# Ctrl+b 0 → agtx board   Ctrl+b 1 → agent session   Ctrl+b d → detach
```

---

## Output

Results are written to `./swebench_output/{config-name}_{timestamp}/`.

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

**Quick check:**
```bash
cat swebench_output/*/results.json | \
  python3 -c "import json,sys; r=json.load(sys.stdin); print(f'{sum(1 for x in r if x[\"status\"]==\"success\")}/{len(r)} success')"
```

---

## Capturing the Agent Session Transcript

`predictions.jsonl` / `results.json` capture only the git diff and metrics — NOT what the agent said or did. To verify *behavior* (e.g. whether an injected skill/plugin actually changed the agent's output), read the Claude Code session transcript.

**Where Claude Code writes it:** `~/.claude/projects/{encoded-cwd}/{session-id}.jsonl`, where `{encoded-cwd}` is the working directory with every `/` and `.` replaced by `-`. Each line is one event (`type` = `user`/`assistant`/…) with full message content, tool uses, and a `cwd` field.

**In sandbox mode the transcript lives inside the container** (agent runs as `bench`, `HOME=/home/bench`, cwd `/testbed` when `skip_worktree = true`) and is **destroyed when the container stops**:
```
/home/bench/.claude/projects/-testbed/{session-id}.jsonl
```

Containers are removed at task end, and a single `docker cp` "before the run finishes" is unreliable — the container often stops first. Run a **polling loop that re-copies every few seconds** while the container is alive (start it right after launching the benchmark):
```bash
C=swebench-astropy-astropy-12907
DEST=/tmp/transcript_capture
for i in $(seq 1 180); do
  docker ps --filter name=$C -q | grep -q . || { echo "container gone"; break; }
  docker cp $C:/home/bench/.claude/projects "$DEST" 2>/dev/null
  sleep 5
done
find "$DEST" -name '*.jsonl'
```

With `skip_worktree = true`, each agtx phase (Planning / Running / Review) is a **separate** Claude Code session → one `.jsonl` per phase. `void` runs as a single session.

**Analysing the transcript — do NOT use a naive grep.** SessionStart-hook plugins (focus, ponytail, caveman) inject their whole `SKILL.md` into the transcript as an `attachment` event, and that injected text *documents* the very tags you'd grep for. So `grep -c 'context-commit' *.jsonl` counts the **injected instructions**, not model output. In one run a raw grep reported 34 CSP-tag hits while the model emitted **zero** — all 34 were inside the injected skill. Parse the JSONL, keep only `type == "assistant"` events, exclude the injected `attachment`, and count tags in the `text` blocks:
```bash
python3 - "$DEST"/**/-testbed/*.jsonl << 'PY'
import json, sys
TAGS = ["context-commit","context-scope","context-checkpoint","context-transient","context-revoke"]
for path in sys.argv[1:]:
    injected = turns = 0; counts = {}
    for line in open(path):
        try: d = json.loads(line)
        except Exception: continue
        if d.get("type") == "attachment" and "Always-on context discipline" in json.dumps(d):
            injected += 1
        if d.get("type") != "assistant": continue
        turns += 1
        content = d.get("message", {}).get("content", [])
        text = "".join(c.get("text","") for c in content
                        if isinstance(c, dict) and c.get("type") == "text") \
               if isinstance(content, list) else (content if isinstance(content, str) else "")
        for t in TAGS:
            n = text.count("<" + t)
            if n: counts[t] = counts.get(t, 0) + n
    print(f"{path}\n  injected SKILL.md: {injected}  assistant turns: {turns}  tags EMITTED BY MODEL: {counts or 'NONE'}")
PY
```

**Gotchas:**
- CSP tags are HTML-style (`<context-commit>`) → the Claude Code TUI **hides them in the rendered pane**; `tmux capture-pane` never shows them. Read the JSONL.
- **`void` quirk:** the phase prompt is *pasted* but not always submitted (no artifact/finish-marker), showing as "Pane stable but no finish marker". Submit manually: `docker exec $C tmux -L agtx send-keys -t testbed:1 Enter`.

---

## Evaluation

After the run, copy-paste the printed commands or run manually:

```bash
uv run python -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Lite \
  --predictions_path swebench_output/claude-agtx_20260427_120000/predictions.jsonl \
  --run_id claude-agtx-1746345600
```

The harness runs tests in Docker containers — each task gets a fresh repo checkout with the patch applied and tested in isolation.

---

## Report

Print a summary table with resolved status, duration, cost, and token usage:

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

---

## Cleanup

**Non-sandbox** (stale worktrees, tmux sessions, SQLite DBs):
```bash
# All instances
./swebench/cleanup.sh

# Specific instance
./swebench/cleanup.sh astropy__astropy-12907

# Custom workdir
SWEBENCH_WORKDIR=/my/custom/path ./swebench/cleanup.sh
```

**Sandbox** (containers left running after a killed process):
```bash
# Specific container
docker rm -f swebench-astropy-astropy-12907

# All swebench containers
docker ps -a --filter name=swebench- -q | xargs docker rm -f

# Remove tools volume (only if you need to repopulate it)
docker volume rm agtx-swebench-tools
```

---

## How It Works

Use this to diagnose stuck or slow runs.

```
benchmark.py
  ├── [sandbox] Pulls SWE-bench Docker image, starts container with tools volume mounted
  ├── [sandbox] Copies credentials, wires tools (tmux/node/claude via symlinks), runs sandbox_init
  ├── [non-sandbox] Clones each repo at base_commit → /tmp/swebench_repos/{instance_id}/
  ├── Writes .agtx/config.toml into each repo/container
  ├── Starts agtx TUI per task in detached tmux (tmux -L agtx)
  ├── Spawns agtx mcp-serve as subprocess (JSON-RPC 2.0 over stdio)
  ├── Drives task via MCP:
  │     create_task → move_forward (Planning)
  │     → poll planning artifact → move_forward (Running)
  │     → poll running artifact  → move_forward (Review)
  │     → poll review artifact   → git diff HEAD...{branch} → move_to_done
  ├── Snapshots tokscale before/after running phase for token counts
  └── Appends to predictions.jsonl + rewrites results.json atomically
```

**Phase completion detection** (per phase, in priority order):
1. **Artifact file** — if the plugin defines an artifact for that phase (e.g. `.agtx/plan.md`, `.agtx/execute.md`, `.agtx/review.md` for `agtx`/`agtx-terse`), polls for its existence every 5 seconds.
2. **Claude finish marker** — detects `✻ [Word] for Xs` followed by a `❯` prompt in the pane; confirmed within 5 seconds of the marker appearing.
3. **Pane stability** — fallback for plugins without artifacts and no finish marker: pane content identical for 2 consecutive 5-second checks (10s stable). If stability is reached without a finish marker, a warning is emitted — **the agent may be stuck, errored, or waiting for manual approval.**

When troubleshooting a phase that won't advance: attach to the container's tmux (see *Observing a Running Benchmark*), check whether the expected artifact file exists, and look for the finish marker or a pending approval prompt in the agent pane.
