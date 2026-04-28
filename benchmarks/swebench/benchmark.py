#!/usr/bin/env python3
"""
SWE-bench Lite benchmark runner for agtx.

Drives agtx via its MCP server (JSON-RPC 2.0 over stdio) to run coding agent
workflows against SWE-bench Lite (300 tasks), collects git diff patches when
tasks reach Review, uses tokscale for token/cost metrics (all agents), and
writes SWE-bench-compatible predictions.jsonl + results.json.

A config.toml is required. It is written to .agtx/config.toml in each repo
before the TUI starts — this is how agent, plugin, worktree_dir, etc. are
configured. The script reads plugin and default_agent back from the config to
drive artifact polling.

Token/cost metrics use tokscale (https://github.com/junhoyeo/tokscale) if
available. Supports claude, codex, gemini, and 20+ other agents. Install with:
    npm install -g tokscale
If tokscale is not installed, cost fields are null.

Usage:
    python benchmark.py --config my_config.toml --instances 1
    python benchmark.py --config my_config.toml --concurrency 2 --instances 10
    python benchmark.py --config my_config.toml --instance-ids sympy__sympy-20590

Example config.toml:
    default_agent = "claude"
    workflow_plugin = "agtx"
    worktree_dir = ".agtx/worktrees"
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import threading
import time
from concurrent.futures import ThreadPoolExecutor, as_completed
from datetime import datetime
from pathlib import Path
from typing import Any

try:
    from datasets import load_dataset
    from tqdm import tqdm
except ImportError:
    print("Missing dependencies. Run: pip install -r requirements.txt", file=sys.stderr)
    sys.exit(1)

try:
    import tomllib  # Python 3.11+
except ImportError:
    try:
        import tomli as tomllib  # fallback
    except ImportError:
        # Minimal TOML parser for the subset we need (key = "value" lines only)
        tomllib = None  # type: ignore


# ---------------------------------------------------------------------------
# Config loading
# ---------------------------------------------------------------------------

def load_config_toml(path: Path) -> dict:
    """Load a TOML config file, returning a dict of its keys."""
    content = path.read_text()
    if tomllib is not None:
        return tomllib.loads(content)
    # Fallback: parse simple key = "value" lines and [section] tables
    result: dict = {}
    current_section: dict | None = None
    current_section_name: str | None = None
    for line in content.splitlines():
        line = line.strip()
        if not line or line.startswith("#"):
            continue
        if line.startswith("[") and line.endswith("]"):
            current_section_name = line[1:-1].strip()
            current_section = {}
            result[current_section_name] = current_section
            continue
        if "=" in line:
            k, _, v = line.partition("=")
            k = k.strip()
            v = v.strip().strip('"').strip("'")
            if current_section is not None:
                current_section[k] = v
            else:
                result[k] = v
    return result


def running_agent(config: dict) -> str:
    """Return the agent that will handle the running phase."""
    return (
        config.get("agents", {}).get("running")
        or config.get("default_agent")
        or "claude"
    )


# ---------------------------------------------------------------------------
# MCP Client
# ---------------------------------------------------------------------------

class McpError(Exception):
    pass


class McpClient:
    """Raw JSON-RPC 2.0 MCP client over subprocess stdin/stdout."""

    def __init__(self, agtx_bin: str, repo_path: str):
        self._seq = 0
        self._lock = threading.Lock()
        self._proc = subprocess.Popen(
            [agtx_bin, "mcp-serve", repo_path],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            bufsize=1,
        )
        self._handshake()

    def _next_id(self) -> int:
        with self._lock:
            self._seq += 1
            return self._seq

    def _send(self, msg: dict) -> None:
        line = json.dumps(msg) + "\n"
        self._proc.stdin.write(line)
        self._proc.stdin.flush()

    def _recv(self) -> dict:
        line = self._proc.stdout.readline()
        if not line:
            raise McpError("MCP server closed connection")
        return json.loads(line)

    def _request(self, method: str, params: dict) -> Any:
        req_id = self._next_id()
        self._send({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params})
        while True:
            msg = self._recv()
            if msg.get("id") == req_id:
                if "error" in msg:
                    raise McpError(f"MCP error: {msg['error']}")
                result = msg.get("result", {})
                content = result.get("content", [])
                if content and content[0].get("type") == "text":
                    return json.loads(content[0]["text"])
                if result.get("isError"):
                    raise McpError(f"Tool returned error: {content}")
                return result
            # ignore notifications

    def _notify(self, method: str, params: dict) -> None:
        self._send({"jsonrpc": "2.0", "method": method, "params": params})

    def _handshake(self) -> None:
        req_id = self._next_id()
        self._send({
            "jsonrpc": "2.0",
            "id": req_id,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "swebench-runner", "version": "1.0"},
            },
        })
        while True:
            msg = self._recv()
            if msg.get("id") == req_id:
                break
        self._notify("notifications/initialized", {})

    def call(self, tool: str, **kwargs) -> Any:
        params = {k: v for k, v in kwargs.items() if v is not None}
        return self._request("tools/call", {"name": tool, "arguments": params})

    def close(self) -> None:
        try:
            self._proc.stdin.close()
            self._proc.wait(timeout=5)
        except Exception:
            self._proc.kill()


# ---------------------------------------------------------------------------
# Repo Setup
# ---------------------------------------------------------------------------

def setup_repo(instance: dict, workdir: str, config_path: Path, verbose: bool = False) -> Path:
    """
    Clone the repo at base_commit and write .agtx/config.toml.
    Returns repo path. Safe to call again on an existing clone (resumable).
    """
    instance_id = instance["instance_id"]
    repo_url = f"https://github.com/{instance['repo']}.git"
    base_commit = instance["base_commit"]

    repo_path = Path(workdir) / instance_id
    if repo_path.exists():
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repo_path,
            capture_output=True,
            text=True,
        )
        if result.returncode == 0 and result.stdout.strip() == base_commit:
            if verbose:
                print(f"  [setup] Using cached repo at {repo_path}", file=sys.stderr)
            _write_agtx_config(repo_path, config_path)
            return repo_path
        if verbose:
            print(f"  [setup] Stale clone, removing {repo_path}", file=sys.stderr)
        subprocess.run(["rm", "-rf", str(repo_path)], check=True)

    if verbose:
        print(f"  [setup] Cloning {repo_url} → {repo_path}", file=sys.stderr)
    repo_path.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["git", "clone", "--quiet", "--depth=1", repo_url, str(repo_path)],
        check=True,
        capture_output=True,
        timeout=300,
    )
    if verbose:
        print(f"  [setup] Checking out {base_commit}", file=sys.stderr)
    fetch_result = subprocess.run(
        ["git", "fetch", "--depth=1", "origin", base_commit],
        cwd=repo_path,
        capture_output=True,
        text=True,
        timeout=120,
    )
    if fetch_result.returncode != 0:
        subprocess.run(
            ["git", "fetch", "--unshallow"],
            cwd=repo_path,
            capture_output=True,
            timeout=600,
        )
    subprocess.run(
        ["git", "checkout", base_commit],
        cwd=repo_path,
        check=True,
        capture_output=True,
        timeout=60,
    )
    _write_agtx_config(repo_path, config_path)
    return repo_path


def _write_agtx_config(repo_path: Path, config_path: Path) -> None:
    """
    Write config.toml and wipe any stale agtx state (old worktrees, task DB, old artifacts).
    This ensures each benchmark run starts clean even if the repo clone is reused.
    """
    agtx_dir = repo_path / ".agtx"
    # Remove stale worktrees and the project DB so the TUI starts fresh
    for stale in ["worktrees", "db"]:
        stale_path = agtx_dir / stale
        if stale_path.exists():
            subprocess.run(["rm", "-rf", str(stale_path)], check=True)
    agtx_dir.mkdir(exist_ok=True)
    dest = agtx_dir / "config.toml"
    dest.write_text(config_path.read_text())

    # Ensure .agtx/ is gitignored
    gitignore = repo_path / ".gitignore"
    marker = ".agtx/"
    existing = gitignore.read_text() if gitignore.exists() else ""
    if marker not in existing:
        with gitignore.open("a") as f:
            f.write(f"\n# agtx benchmark artifacts\n{marker}\n")


def start_tui_in_tmux(slug: str, repo_path: Path, agtx_bin: str, verbose: bool = False) -> None:
    """Start an agtx TUI instance in a detached tmux session on the swebench server."""
    # Kill any stale session with the same slug on the swebench server
    subprocess.run(
        ["tmux", "-L", "swebench", "kill-session", "-t", slug],
        capture_output=True,
    )
    # Also kill the agtx-server session for this repo (named after the repo dir)
    # so the new TUI doesn't inherit stale task windows from a previous run
    repo_session = repo_path.name  # e.g. "astropy__astropy-12907"
    subprocess.run(
        ["tmux", "-L", "agtx", "kill-session", "-t", repo_session],
        capture_output=True,
    )
    if verbose:
        print(f"  [tmux] Starting TUI session '{slug}' for {repo_path}", file=sys.stderr)
    result = subprocess.run(
        [
            "tmux", "-L", "swebench",
            "new-session", "-d", "-s", slug,
            f"{agtx_bin} {repo_path}",
        ],
        capture_output=True,
        text=True,
    )
    if result.returncode != 0:
        raise RuntimeError(f"tmux new-session failed: {result.stderr.strip()}")


def kill_tmux_session(slug: str, repo_path: Path | None = None) -> None:
    subprocess.run(
        ["tmux", "-L", "swebench", "kill-session", "-t", slug],
        capture_output=True,
    )
    if repo_path is not None:
        subprocess.run(
            ["tmux", "-L", "agtx", "kill-session", "-t", repo_path.name],
            capture_output=True,
        )


# ---------------------------------------------------------------------------
# Token/cost tracking via tokscale
# ---------------------------------------------------------------------------

# CLI flag that filters tokscale output to a specific agent
_TOKSCALE_FLAG: dict[str, str] = {
    "claude":   "--claude",
    "codex":    "--codex",
    "gemini":   "--gemini",
    "copilot":  "--copilot",
    "opencode": "--opencode",
}

# Cached result of tokscale availability check
_tokscale_bin: str | None | bool = False  # False = not yet checked


def _find_tokscale() -> str | None:
    """Return path to tokscale binary, or None if not installed."""
    global _tokscale_bin
    if _tokscale_bin is not False:
        return _tokscale_bin  # type: ignore
    result = subprocess.run(["which", "tokscale"], capture_output=True, text=True)
    _tokscale_bin = result.stdout.strip() or None
    return _tokscale_bin  # type: ignore


def _tokscale_snapshot(agent: str) -> dict:
    """
    Run `tokscale --json --today --{agent}` and return aggregated totals across all models:
        {input, output, cache_read, cache_write, reasoning, cost_usd}
    Returns zeros if tokscale is unavailable or the agent has no records today.
    """
    tokscale = _find_tokscale()
    if not tokscale:
        return {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "reasoning": 0, "cost_usd": 0.0}

    flag = _TOKSCALE_FLAG.get(agent)
    cmd = [tokscale, "--json", "--today"]
    if flag:
        cmd.append(flag)

    try:
        result = subprocess.run(cmd, capture_output=True, text=True, timeout=15)
        if result.returncode != 0 or not result.stdout.strip():
            return {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "reasoning": 0, "cost_usd": 0.0}

        data = json.loads(result.stdout)
        entries = data.get("entries", [])

        totals = {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "reasoning": 0, "cost_usd": 0.0}
        for entry in entries:
            totals["input"]       += entry.get("input", 0)
            totals["output"]      += entry.get("output", 0)
            totals["cache_read"]  += entry.get("cacheRead", 0)
            totals["cache_write"] += entry.get("cacheWrite", 0)
            totals["reasoning"]   += entry.get("reasoning", 0)
            totals["cost_usd"]    += entry.get("cost", 0.0)
        return totals

    except Exception:
        return {"input": 0, "output": 0, "cache_read": 0, "cache_write": 0, "reasoning": 0, "cost_usd": 0.0}


def tokscale_diff(before: dict, after: dict) -> dict:
    """
    Subtract before-snapshot from after-snapshot to get this task's usage.
    Returns the standard cost_data dict used throughout the script.
    """
    if not _find_tokscale():
        return {"cost_usd": None, "input_tokens": None, "output_tokens": None, "cost_tokens": None}

    inp  = max(0, after["input"]   - before["input"])
    out  = max(0, after["output"]  - before["output"])
    cost = max(0.0, after["cost_usd"] - before["cost_usd"])
    total = inp + out + max(0, after["cache_read"]  - before["cache_read"]) + \
                        max(0, after["cache_write"] - before["cache_write"]) + \
                        max(0, after["reasoning"]   - before["reasoning"])

    return {
        "cost_usd":      round(cost, 6) if cost > 0 else None,
        "input_tokens":  inp   if (inp or out) else None,
        "output_tokens": out   if (inp or out) else None,
        "cost_tokens":   total if total > 0    else None,
    }


# ---------------------------------------------------------------------------
# Results store
# ---------------------------------------------------------------------------

class ResultsStore:
    """Thread-safe, resumable persistence for benchmark results."""

    def __init__(self, output_dir: Path):
        self.output_dir = output_dir
        output_dir.mkdir(parents=True, exist_ok=True)
        self.predictions_path = output_dir / "predictions.jsonl"
        self.results_path = output_dir / "results.json"
        self._lock = threading.Lock()
        self._results: list[dict] = []
        self._done_ids: set[str] = set()
        self._load_existing()

    def _load_existing(self) -> None:
        if self.results_path.exists():
            try:
                data = json.loads(self.results_path.read_text())
                self._results = data if isinstance(data, list) else []
                self._done_ids = {r["instance_id"] for r in self._results}
                print(f"Resuming: {len(self._done_ids)} tasks already completed.")
            except Exception:
                pass

    def is_done(self, instance_id: str) -> bool:
        return instance_id in self._done_ids

    def save_result(
        self,
        instance_id: str,
        status: str,
        duration_seconds: float,
        model_name: str,
        model_patch: str = "",
        cost_usd: float | None = None,
        cost_tokens: int | None = None,
        input_tokens: int | None = None,
        output_tokens: int | None = None,
        error: str | None = None,
    ) -> None:
        with self._lock:
            result = {
                "instance_id": instance_id,
                "status": status,
                "duration_seconds": round(duration_seconds, 1),
                "cost_usd": cost_usd,
                "cost_tokens": cost_tokens,
                "input_tokens": input_tokens,
                "output_tokens": output_tokens,
                "model_patch": model_patch,
                "error": error,
            }
            self._results.append(result)
            self._done_ids.add(instance_id)

            # Append prediction (SWE-bench format)
            with self.predictions_path.open("a") as f:
                pred = {
                    "instance_id": instance_id,
                    "model_name_or_path": model_name,
                    "model_patch": model_patch,
                }
                f.write(json.dumps(pred) + "\n")

            # Rewrite results atomically
            tmp = self.results_path.with_suffix(".json.tmp")
            tmp.write_text(json.dumps(self._results, indent=2))
            tmp.replace(self.results_path)


# ---------------------------------------------------------------------------
# Phase artifact paths per bundled plugin
# ---------------------------------------------------------------------------

# Artifact path for the "planning" phase (relative to worktree root).
# None means the plugin has no planning artifact → fall back to pane stability.
PLUGIN_PLANNING_ARTIFACTS: dict[str, str | None] = {
    "agtx":             ".agtx/plan.md",
    "agtx-terse":       ".agtx/plan.md",
    "gsd":              ".planning/phases/*/{phase}-CONTEXT.md",
    "spec-kit":         None,
    "bmad":             None,
    "openspec":         None,
    "superpowers":      None,
    "oh-my-claudecode": None,
    "void":             None,
}

# Artifact path for the "running" phase (relative to worktree root).
# None means the plugin has no running artifact → fall back to pane stability.
PLUGIN_RUNNING_ARTIFACTS: dict[str, str | None] = {
    "agtx":             ".agtx/execute.md",
    "agtx-terse":       ".agtx/execute.md",
    "gsd":              ".planning/phases/*/{phase}-SUMMARY.md",
    "spec-kit":         None,
    "bmad":             "_bmad-output/implementation-artifacts/*.md",
    "openspec":         "openspec/changes/*/tasks.md",
    "superpowers":      None,
    "oh-my-claudecode": None,
    "void":             None,
}

# Artifact path for the "review" phase (relative to worktree root).
# None means no review artifact → collect patch immediately after entering Review.
PLUGIN_REVIEW_ARTIFACTS: dict[str, str | None] = {
    "agtx":             ".agtx/review.md",
    "agtx-terse":       ".agtx/review.md",
    "gsd":              None,
    "spec-kit":         None,
    "bmad":             None,
    "openspec":         None,
    "superpowers":      None,
    "oh-my-claudecode": None,
    "void":             None,
}


def _artifact_exists(worktree: Path, pattern: str) -> bool:
    """Return True if the artifact path (possibly with * or {phase} placeholders) exists."""
    if "{phase}" in pattern:
        for n in range(1, 21):
            for fmt in (f"{n:02d}", str(n)):
                if _artifact_exists(worktree, pattern.replace("{phase}", fmt)):
                    return True
        return False
    if "*" not in pattern:
        return (worktree / pattern).exists()
    parts = Path(pattern).parts
    base = worktree
    for i, part in enumerate(parts):
        if "*" in part:
            remaining = str(Path(*parts[i:]))
            return any(True for _ in base.glob(remaining))
        base = base / part
    return False


# ---------------------------------------------------------------------------
# Task runner
# ---------------------------------------------------------------------------

class TaskRunner:
    """Runs a single SWE-bench instance through the full agtx lifecycle."""

    def __init__(
        self,
        instance: dict,
        repo_path: Path,
        agtx_bin: str,
        plugin: str,
        agent: str,
        running_agent: str,
        model_name: str,
        phase_timeout: int,
        verbose: bool = False,
    ):
        self.instance = instance
        self.instance_id = instance["instance_id"]
        self.repo_path = repo_path
        self.agtx_bin = agtx_bin
        self.plugin = plugin
        self.agent = agent
        self.running_agent = running_agent
        self.model_name = model_name
        self.phase_timeout = phase_timeout
        self.verbose = verbose
        self.mcp: McpClient | None = None

    def _poll_transition(self, request_id: str, timeout: int = 120) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            result = self.mcp.call("get_transition_status", request_id=request_id)
            status = result.get("status", "pending")
            if status == "completed":
                return
            if status == "error":
                raise McpError(f"Transition failed: {result.get('error')}")
            time.sleep(2)
        raise TimeoutError(f"Transition {request_id} timed out after {timeout}s")

    def _move_task(self, task_id: str, action: str) -> None:
        result = self.mcp.call("move_task", task_id=task_id, action=action)
        self._poll_transition(result["request_id"])

    def _wait_for_status(self, task_id: str, target_status: str, timeout: int = 120) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            task = self.mcp.call("get_task", task_id=task_id)
            if task.get("status") == target_status:
                return task
            time.sleep(3)
        raise TimeoutError(f"Task never reached {target_status} within {timeout}s")

    def _wait_for_worktree(self, task_id: str, timeout: int = 180) -> dict:
        """
        Wait until the task has a worktree_path set in the DB.
        This is populated asynchronously by the TUI's background setup thread
        after transition_to_planning/running completes — the transition request
        being marked 'completed' only means the TUI accepted it, not that
        worktree setup is done.
        """
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            task = self.mcp.call("get_task", task_id=task_id)
            if task.get("worktree_path"):
                return task
            time.sleep(3)
        raise TimeoutError(f"Worktree never populated for task {task_id} within {timeout}s")

    def _wait_for_artifact(self, task_id: str, worktree_path: str, artifact_pattern: str, timeout: int) -> bool:
        """Poll for an artifact file in the worktree. Returns True if found, False on timeout."""
        worktree = Path(worktree_path)
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if _artifact_exists(worktree, artifact_pattern):
                return True
            time.sleep(5)
        return False

    def _wait_for_pane_stable(self, task_id: str, timeout: int) -> None:
        """Wait for pane content to be stable for 2 consecutive 30s checks (no artifact available)."""
        prev_content: str | None = None
        stable_count = 0
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            time.sleep(30)
            try:
                result = self.mcp.call("read_pane_content", task_id=task_id, lines=80)
                content = result.get("content", "")
            except McpError:
                content = ""
            if content == prev_content:
                stable_count += 1
                if stable_count >= 2:
                    return
            else:
                stable_count = 0
                prev_content = content

    def run(self) -> dict:
        start = time.monotonic()
        if self.verbose:
            print(f"  [{self.instance_id}] Starting MCP handshake...", file=sys.stderr)
        self.mcp = McpClient(self.agtx_bin, str(self.repo_path))
        task_id = None

        try:
            if self.verbose:
                print(f"  [{self.instance_id}] Creating task...", file=sys.stderr)
            problem = self.instance.get("problem_statement", "")
            description = (
                problem
                + "\n\n---\n"
                + "Note: the repo may not be fully installable in this environment. "
                + "Do not attempt to build, install, or run tests. "
                + "Do not run any git commands (no fetch, pull, merge, or commit). "
                + "Read the source code, understand the bug, and fix it by editing the relevant files directly."
            )
            task_resp = self.mcp.call(
                "create_task",
                title=self.instance_id,
                description=description,
                plugin=self.plugin,
            )
            task_id = task_resp["id"]

            if self.verbose:
                print(f"  [{self.instance_id}] task_id={task_id}, moving to Planning...", file=sys.stderr)
            # Backlog → Planning
            self._move_task(task_id, "move_forward")
            self._wait_for_status(task_id, "planning", timeout=120)

            # Wait for worktree to be created (set by TUI background thread)
            if self.verbose:
                print(f"  [{self.instance_id}] Waiting for worktree...", file=sys.stderr)
            planning_task = self._wait_for_worktree(task_id, timeout=180)
            worktree_path = planning_task.get("worktree_path", "")

            # Snapshot tokscale before agent starts working
            cost_before = _tokscale_snapshot(self.running_agent)

            # Wait for planning phase artifact (.agtx/plan.md for agtx/agtx-terse plugins)
            planning_artifact = PLUGIN_PLANNING_ARTIFACTS.get(self.plugin)
            if planning_artifact and worktree_path:
                if self.verbose:
                    print(f"  [{self.instance_id}] Waiting for planning artifact ({planning_artifact})...", file=sys.stderr)
                found = self._wait_for_artifact(task_id, worktree_path, planning_artifact, self.phase_timeout)
                if not found:
                    raise TimeoutError(f"Planning artifact not found within {self.phase_timeout}s")
            else:
                if self.verbose:
                    print(f"  [{self.instance_id}] Waiting for planning phase (pane stability)...", file=sys.stderr)
                self._wait_for_pane_stable(task_id, self.phase_timeout)

            if self.verbose:
                print(f"  [{self.instance_id}] Planning done, moving to Running...", file=sys.stderr)
            # Small delay: let the agent finish any final output after writing the artifact
            # before the TUI sends /exit (clear_context_on_advance) and restarts it.
            time.sleep(5)
            # Planning → Running
            self._move_task(task_id, "move_forward")
            running_task = self._wait_for_status(task_id, "running", timeout=60)
            worktree_path = running_task.get("worktree_path") or worktree_path

            # Wait for running phase artifact (.agtx/execute.md for agtx/agtx-terse plugins)
            running_artifact = PLUGIN_RUNNING_ARTIFACTS.get(self.plugin)
            if running_artifact and worktree_path:
                if self.verbose:
                    print(f"  [{self.instance_id}] Waiting for running artifact ({running_artifact})...", file=sys.stderr)
                found = self._wait_for_artifact(task_id, worktree_path, running_artifact, self.phase_timeout)
                if not found:
                    raise TimeoutError(f"Running artifact not found within {self.phase_timeout}s")
            else:
                if self.verbose:
                    print(f"  [{self.instance_id}] Waiting for running phase (pane stability)...", file=sys.stderr)
                self._wait_for_pane_stable(task_id, self.phase_timeout)

            # Snapshot again and diff to get this task's usage
            cost_data = tokscale_diff(cost_before, _tokscale_snapshot(self.running_agent))

            if self.verbose:
                print(f"  [{self.instance_id}] Running done, moving to Review...", file=sys.stderr)
            # Running → Review
            self._move_task(task_id, "move_forward")
            review_task = self._wait_for_status(task_id, "review", timeout=60)
            worktree_path = review_task.get("worktree_path") or worktree_path

            # Wait for review phase artifact (.agtx/review.md for agtx/agtx-terse plugins)
            review_artifact = PLUGIN_REVIEW_ARTIFACTS.get(self.plugin)
            if review_artifact and worktree_path:
                if self.verbose:
                    print(f"  [{self.instance_id}] Waiting for review artifact ({review_artifact})...", file=sys.stderr)
                found = self._wait_for_artifact(task_id, worktree_path, review_artifact, self.phase_timeout)
                if not found:
                    raise TimeoutError(f"Review artifact not found within {self.phase_timeout}s")
            else:
                if self.verbose:
                    print(f"  [{self.instance_id}] Waiting for review phase (pane stability)...", file=sys.stderr)
                self._wait_for_pane_stable(task_id, self.phase_timeout)

            model_patch = self._collect_patch(review_task)

            if self.verbose:
                print(f"  [{self.instance_id}] Done. patch_len={len(model_patch)}", file=sys.stderr)
            # Review → Done (cleanup worktree)
            try:
                self._move_task(task_id, "move_to_done")
            except Exception:
                pass

            return {
                "status": "success",
                "duration_seconds": time.monotonic() - start,
                "model_patch": model_patch,
                **cost_data,
                "error": None,
            }

        except TimeoutError as e:
            self._cleanup(task_id)
            return self._error_result("timeout", time.monotonic() - start, str(e))
        except Exception as e:
            self._cleanup(task_id)
            return self._error_result("error", time.monotonic() - start, str(e))
        finally:
            if self.mcp:
                self.mcp.close()

    def _collect_patch(self, task: dict) -> str:
        worktree_path = task.get("worktree_path")
        if not worktree_path:
            return ""
        base_commit = self.instance["base_commit"]
        # Diff the worktree's actual files against the base commit.
        # Using the worktree path directly avoids picking up any extra commits
        # the agent may have merged/fetched onto the branch — we only care about
        # file-level changes, not git history.
        result = subprocess.run(
            ["git", "diff", base_commit,
             "--", ".", ":!.agtx/"],
            cwd=worktree_path,
            capture_output=True,
        )
        if result.returncode != 0:
            return ""
        # Decode as UTF-8, replacing undecodable bytes so binary files don't crash the run
        return result.stdout.decode("utf-8", errors="replace")

    def _cleanup(self, task_id: str | None) -> None:
        if task_id:
            try:
                self.mcp.call("move_task", task_id=task_id, action="move_to_done")
            except Exception:
                pass

    def _error_result(self, status: str, duration: float, error: str) -> dict:
        return {
            "status": status,
            "duration_seconds": duration,
            "model_patch": "",
            "cost_usd": None,
            "cost_tokens": None,
            "input_tokens": None,
            "output_tokens": None,
            "error": error,
        }


# ---------------------------------------------------------------------------
# Orchestrator
# ---------------------------------------------------------------------------

class BenchmarkOrchestrator:
    """Drives N tasks, manages concurrency."""

    def __init__(
        self,
        instances: list[dict],
        agtx_bin: str,
        config_path: Path,
        plugin: str,
        agent: str,
        running_agent: str,
        model_name: str,
        phase_timeout: int,
        workdir: str,
        output_dir: Path,
        concurrency: int,
        verbose: bool = False,
    ):
        self.instances = instances
        self.agtx_bin = agtx_bin
        self.config_path = config_path
        self.plugin = plugin
        self.agent = agent
        self.running_agent = running_agent
        self.model_name = model_name
        self.phase_timeout = phase_timeout
        self.workdir = workdir
        self.store = ResultsStore(output_dir)
        self.concurrency = concurrency
        self.verbose = verbose

    def _run_one(self, instance: dict, progress: tqdm) -> None:
        instance_id = instance["instance_id"]

        if self.store.is_done(instance_id):
            progress.update(1)
            return

        slug = re.sub(r"[^a-z0-9]+", "-", instance_id.lower()).strip("-")[:50]
        start = time.monotonic()

        if self.verbose:
            print(f"\n[{instance_id}] Setting up repo...", file=sys.stderr)
        try:
            repo_path = setup_repo(instance, self.workdir, self.config_path, verbose=self.verbose)
        except Exception as e:
            self.store.save_result(
                instance_id=instance_id,
                status="setup_error",
                duration_seconds=time.monotonic() - start,
                model_name=self.model_name,
                error=str(e),
            )
            progress.update(1)
            return

        if self.verbose:
            print(f"[{instance_id}] Starting TUI...", file=sys.stderr)
        try:
            start_tui_in_tmux(slug, repo_path, self.agtx_bin, verbose=self.verbose)
            time.sleep(3)  # Wait for TUI startup + project registration
        except Exception as e:
            self.store.save_result(
                instance_id=instance_id,
                status="setup_error",
                duration_seconds=time.monotonic() - start,
                model_name=self.model_name,
                error=f"TUI startup failed: {e}",
            )
            progress.update(1)
            return

        if self.verbose:
            print(f"[{instance_id}] Connecting MCP client...", file=sys.stderr)
        runner = TaskRunner(
            instance=instance,
            repo_path=repo_path,
            agtx_bin=self.agtx_bin,
            plugin=self.plugin,
            agent=self.agent,
            running_agent=self.running_agent,
            model_name=self.model_name,
            phase_timeout=self.phase_timeout,
            verbose=self.verbose,
        )
        result = runner.run()
        kill_tmux_session(slug, repo_path)

        self.store.save_result(
            instance_id=instance_id,
            model_name=self.model_name,
            **result,
        )
        progress.update(1)
        progress.set_postfix_str(f"{result['status']} {instance_id}")

    def run(self) -> None:
        pending = [i for i in self.instances if not self.store.is_done(i["instance_id"])]
        total = len(self.instances)
        already_done = total - len(pending)

        print(f"Agent: {self.agent} | Plugin: {self.plugin} | Tasks: {len(pending)} | Concurrency: {self.concurrency}")
        print(f"Output: {self.store.output_dir}")

        with tqdm(total=total, initial=already_done, unit="task") as progress:
            if self.concurrency == 1:
                for instance in pending:
                    self._run_one(instance, progress)
            else:
                with ThreadPoolExecutor(max_workers=self.concurrency) as pool:
                    futures = {
                        pool.submit(self._run_one, instance, progress): instance
                        for instance in pending
                    }
                    for future in as_completed(futures):
                        exc = future.exception()
                        if exc:
                            inst = futures[future]
                            print(f"\nUnhandled error for {inst['instance_id']}: {exc}", file=sys.stderr)

        statuses: dict[str, int] = {}
        for r in self.store._results:
            statuses[r["status"]] = statuses.get(r["status"], 0) + 1
        print(f"\nDone. {total} tasks total.")
        for status, count in sorted(statuses.items()):
            print(f"  {status}: {count}")


# ---------------------------------------------------------------------------
# Dataset loader
# ---------------------------------------------------------------------------

def load_swebench(split: str = "test") -> list[dict]:
    print(f"Loading SWE-bench Lite ({split} split)...")
    dataset = load_dataset("princeton-nlp/SWE-bench_Lite", split=split)
    return list(dataset)


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main() -> None:
    parser = argparse.ArgumentParser(
        description="Run SWE-bench Lite benchmark using agtx as the agent runner.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
The --config file is an agtx ProjectConfig TOML written to .agtx/config.toml
in each cloned repo before the TUI starts. It controls agent, plugin, and all
other agtx project settings. Example:

    default_agent = "claude"
    workflow_plugin = "agtx"
    worktree_dir = ".agtx/worktrees"
""",
    )
    parser.add_argument(
        "--config",
        required=True,
        metavar="PATH",
        help="Path to agtx config.toml (written to .agtx/config.toml in each repo)",
    )
    parser.add_argument(
        "--instances",
        type=int,
        default=None,
        metavar="N",
        help="Run first N tasks (default: all 300)",
    )
    parser.add_argument(
        "--instance-ids",
        nargs="+",
        metavar="ID",
        dest="instance_ids",
        help="Run specific instance IDs",
    )
    parser.add_argument(
        "--concurrency",
        type=int,
        default=1,
        help="Parallel tasks (default: 1)",
    )
    parser.add_argument(
        "--output-dir",
        default=None,
        dest="output_dir",
        help="Output directory (default: ./swebench_output/{plugin}_{agent}_{timestamp}/)",
    )
    parser.add_argument(
        "--workdir",
        default="/tmp/swebench_repos",
        help="Repo clone directory (default: /tmp/swebench_repos)",
    )
    parser.add_argument(
        "--agtx",
        default="./target/release/agtx",
        help="Path to agtx binary (default: ./target/release/agtx)",
    )
    parser.add_argument(
        "--phase-timeout",
        type=int,
        default=1200,
        dest="phase_timeout",
        help="Per-phase max seconds (default: 1200)",
    )
    parser.add_argument(
        "--model-name",
        default=None,
        dest="model_name",
        help="Label in predictions.jsonl (default: agtx-{plugin}-{agent})",
    )
    parser.add_argument(
        "--split",
        default="test",
        help="HuggingFace dataset split (default: test)",
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Print step-by-step progress to stderr",
    )
    args = parser.parse_args()

    # Load and validate config
    config_path = Path(args.config).resolve()
    if not config_path.exists():
        print(f"Config not found: {config_path}", file=sys.stderr)
        sys.exit(1)
    config = load_config_toml(config_path)
    plugin = config.get("workflow_plugin", "void")
    agent = config.get("default_agent", "claude")
    run_agent = running_agent(config)

    # Resolve agtx binary
    agtx_bin = str(Path(args.agtx).resolve())
    if not Path(agtx_bin).exists():
        print(f"agtx binary not found: {agtx_bin}", file=sys.stderr)
        print("Build with: cargo build --release", file=sys.stderr)
        sys.exit(1)

    model_name = args.model_name or f"agtx-{plugin}-{agent}"

    if args.output_dir is None:
        ts = datetime.now().strftime("%Y%m%d_%H%M%S")
        output_dir = Path(f"./swebench_output/{plugin}_{agent}_{ts}").resolve()
    else:
        output_dir = Path(args.output_dir).resolve()

    instances = load_swebench(args.split)

    if args.instance_ids:
        id_set = set(args.instance_ids)
        instances = [i for i in instances if i["instance_id"] in id_set]
        if not instances:
            print("No matching instance IDs found.", file=sys.stderr)
            sys.exit(1)
    elif args.instances is not None:
        instances = instances[: args.instances]

    orchestrator = BenchmarkOrchestrator(
        instances=instances,
        agtx_bin=agtx_bin,
        config_path=config_path,
        plugin=plugin,
        agent=agent,
        running_agent=run_agent,
        model_name=model_name,
        phase_timeout=args.phase_timeout,
        workdir=args.workdir,
        output_dir=output_dir,
        concurrency=args.concurrency,
        verbose=args.verbose,
    )
    orchestrator.run()

    print(f"\nPredictions: {orchestrator.store.predictions_path}")
    print(f"Results:     {orchestrator.store.results_path}")
    print("\nTo evaluate:")
    print(f"  python -m swebench.harness.run_evaluation \\")
    print(f"    --dataset_name princeton-nlp/SWE-bench_Lite \\")
    print(f"    --predictions_path {orchestrator.store.predictions_path} \\")
    print(f"    --run_id {plugin}-{agent}-$(date +%s)")


if __name__ == "__main__":
    main()
