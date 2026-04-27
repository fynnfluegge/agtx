# agtx Benchmarks

## SWE-bench Lite

Runs AI coding agent workflows against [SWE-bench Lite](https://github.com/princeton-nlp/SWE-bench)
(300 real GitHub bug-fix tasks). Uses agtx as the agent runner, drives it via its MCP server,
collects git diff patches, and outputs SWE-bench-compatible results.

### Prerequisites

**agtx** (built from source):
```bash
cargo build --release
```

**uv** (Python package manager):
```bash
curl -LsSf https://astral.sh/uv/install.sh | sh
```

**tokscale** (token/cost tracking — optional but recommended):
```bash
cargo install tokscale
```
If not installed, `cost_usd` and token fields will be `null` in results.

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

---

### Setup

**1. Create a config file** for your benchmark run.

Config files live in `benchmarks/swebench/configs/`. Each file is a standard agtx
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

Mixed agents (different agent per phase) — `configs/gemini-claude-codex-agtx.toml`:
```toml
default_agent = "claude"
workflow_plugin = "agtx"

[agents]
planning = "gemini"
running  = "claude"
review   = "codex"
```

Available plugins: `void`, `agtx`, `agtx-terse`, `gsd`, `spec-kit`, `bmad`, `openspec`, `superpowers`

**2. Install Python dependencies:**
```bash
uv sync --project benchmarks/swebench
```

Or without a project file:
```bash
uv pip install -r benchmarks/swebench/requirements.txt
```

---

### Running

All commands are run from the repo root.

**Single task (smoke test):**
```bash
uv run --project benchmarks/swebench \
  python benchmarks/swebench/benchmark.py \
  --config benchmarks/swebench/configs/claude-void.toml \
  --instances 1
```

**Specific instance IDs:**
```bash
uv run --project benchmarks/swebench \
  python benchmarks/swebench/benchmark.py \
  --config benchmarks/swebench/configs/claude-void.toml \
  --instance-ids sympy__sympy-20590 django__django-11099
```

**Full 300-task run:**
```bash
uv run --project benchmarks/swebench \
  python benchmarks/swebench/benchmark.py \
  --config benchmarks/swebench/configs/claude-agtx.toml
```

**Parallel tasks:**
```bash
uv run --project benchmarks/swebench \
  python benchmarks/swebench/benchmark.py \
  --config benchmarks/swebench/configs/claude-agtx.toml \
  --concurrency 4
```

**Resume an interrupted run** (pass the same `--output-dir`):
```bash
uv run --project benchmarks/swebench \
  python benchmarks/swebench/benchmark.py \
  --config benchmarks/swebench/configs/claude-agtx.toml \
  --output-dir swebench_output/agtx_claude_20260427_120000
```

#### All options

| Flag | Default | Description |
|------|---------|-------------|
| `--config PATH` | *(required)* | agtx config.toml for this run |
| `--instances N` | all 300 | Run first N tasks |
| `--instance-ids ID...` | — | Run specific instance IDs |
| `--concurrency N` | 1 | Parallel tasks |
| `--output-dir PATH` | `./swebench_output/{plugin}_{agent}_{ts}/` | Output directory |
| `--workdir PATH` | `/tmp/swebench_repos` | Repo clone directory |
| `--agtx PATH` | `./target/release/agtx` | agtx binary |
| `--phase-timeout SECS` | 1200 | Per-phase max seconds (20 min) |
| `--model-name STRING` | `agtx-{plugin}-{agent}` | Label in predictions.jsonl |
| `--split STRING` | `test` | HuggingFace dataset split |

---

### Output

Results are written to `./swebench_output/{plugin}_{agent}_{timestamp}/`:

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
  "input_tokens": 45000,
  "output_tokens": 9000,
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

### Evaluation

After the run, evaluate patches against the SWE-bench test harness
(requires [SWE-bench](https://github.com/princeton-nlp/SWE-bench) installed):

```bash
python -m swebench.harness.run_evaluation \
  --dataset_name princeton-nlp/SWE-bench_Lite \
  --predictions_path swebench_output/agtx_claude_20260427_120000/predictions.jsonl \
  --run_id agtx-claude-1
```

The harness runs tests in Docker containers — each task gets a fresh repo checkout
and the patch is applied and tested in isolation. The benchmark script's output
directory is not used during evaluation.

---

### How it works

```
benchmark.py
  ├── Clones each repo at base_commit → /tmp/swebench_repos/{instance_id}/
  ├── Writes .agtx/config.toml (your config file) into each clone
  ├── Starts agtx TUI per task in detached tmux (tmux -L swebench)
  ├── Spawns agtx mcp-serve as subprocess (JSON-RPC 2.0 over stdio)
  ├── Drives task via MCP:
  │     create_task → move_forward (Planning) → move_forward (Running)
  │     → poll artifact file or pane stability → move_forward (Review)
  │     → git diff HEAD...{branch} → move_to_done
  ├── Snapshots tokscale before/after running phase for token counts
  └── Appends to predictions.jsonl + rewrites results.json atomically
```

Phase completion detection (in priority order):
1. **Artifact file** — if the plugin defines a `running` artifact (e.g. `.agtx/execute.md`
   for the `agtx` plugin), polls for its existence every 5 seconds
2. **Pane stability** — fallback for plugins without artifacts (e.g. `void`): two
   consecutive unchanged pane reads at 30-second intervals (≥60s stable)
