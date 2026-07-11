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

`sandbox_init` commands run as `/home/bench` with `PATH` including `/home/bench/.local/bin`.

Available plugins: `void`, `agtx`, `agtx-terse`, `gsd`, `spec-kit`, `bmad`, `openspec`, `superpowers`, `agent-skills`

Pre-built configs for common combinations are in `swebench/configs/`.

---

## Running

> **Sandbox mode requires a Linux x86_64 binary.** Use `--agtx ../target/agtx-linux-x86_64` (not `../target/release/agtx`).

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
