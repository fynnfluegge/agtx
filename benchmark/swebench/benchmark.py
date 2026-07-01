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
    python benchmark.py --config my_config.toml --instance-ids astropy__astropy-12907 --sandbox --verbose --agtx ../../target/release/agtx

Example config.toml:
    default_agent = "claude"
    workflow_plugin = "agtx"
    worktree_dir = ".agtx/worktrees"
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
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

    def __init__(self, agtx_bin: str, repo_path: str, container_id: str | None = None):
        self._seq = 0
        self._lock = threading.Lock()
        if container_id:
            cmd = [
                "docker", "exec", "-i", container_id,
                "/bin/bash", "-c",
                "export HOME=/home/bench"
                " && source /opt/miniconda3/bin/activate && conda activate testbed"
                " && agtx mcp-serve /testbed",
            ]
        else:
            cmd = [agtx_bin, "mcp-serve", repo_path]
        self._proc = subprocess.Popen(
            cmd,
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

def setup_repo(instance: dict, workdir: str, config_path: Path, verbose: bool = False, smoke_test: bool = False) -> Path:
    """
    Clone the repo at base_commit and write .agtx/config.toml.
    Returns repo path. Safe to call again on an existing clone (resumable).
    In smoke test mode, initializes an empty git repo instead of cloning.
    """
    instance_id = instance["instance_id"]

    if smoke_test:
        repo_path = Path(workdir) / instance_id
        if not repo_path.exists():
            repo_path.mkdir(parents=True)
            subprocess.run(["git", "init"], cwd=repo_path, capture_output=True, check=True)
            subprocess.run(
                ["git", "commit", "--allow-empty", "-m", "init"],
                cwd=repo_path,
                capture_output=True,
                check=True,
                env={**__import__("os").environ, "GIT_AUTHOR_NAME": "smoke", "GIT_AUTHOR_EMAIL": "smoke@test", "GIT_COMMITTER_NAME": "smoke", "GIT_COMMITTER_EMAIL": "smoke@test"},
            )
            if verbose:
                print(f"  [setup] Initialized empty git repo at {repo_path}", file=sys.stderr)
        _write_agtx_config(repo_path, config_path, "HEAD")
        return repo_path

    repo_url = f"https://github.com/{instance['repo']}.git"
    base_commit = instance["base_commit"]

    repo_path = Path(workdir) / instance_id
    if repo_path.exists():
        # Check if the repo is clean: HEAD must be at base_commit and no task/* branches.
        head_result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=repo_path,
            capture_output=True,
            text=True,
        )
        branch_result = subprocess.run(
            ["git", "branch", "--list", "task/*"],
            cwd=repo_path,
            capture_output=True,
            text=True,
        )
        head_ok = head_result.returncode == 0 and head_result.stdout.strip() == base_commit
        no_task_branches = branch_result.returncode != 0 or not branch_result.stdout.strip()

        if not head_ok:
            # Wrong commit — need a clean clone
            if verbose:
                print(
                    f"  [setup] Stale clone (wrong commit), removing {repo_path}",
                    file=sys.stderr,
                )
            subprocess.run(["rm", "-rf", str(repo_path)], check=True)
        else:
            if not no_task_branches:
                # Just delete leftover task branches, no need to re-clone
                for branch in branch_result.stdout.strip().splitlines():
                    branch = branch.strip()
                    if branch:
                        subprocess.run(
                            ["git", "branch", "-D", branch],
                            cwd=repo_path,
                            capture_output=True,
                        )
                if verbose:
                    print(f"  [setup] Cleaned stale task branches, reusing {repo_path}", file=sys.stderr)
            else:
                if verbose:
                    print(f"  [setup] Using cached repo at {repo_path}", file=sys.stderr)
            _write_agtx_config(repo_path, config_path, base_commit)
            return repo_path

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
    _write_agtx_config(repo_path, config_path, base_commit)
    return repo_path


def _write_agtx_config(repo_path: Path, config_path: Path, base_commit: str) -> None:
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
    # Insert base_branch = "<commit>" before any [section] headers so it stays
    # a top-level key. Appending at the end would put it inside the last section
    # (e.g. [agents]) and TOML would parse it as agents.base_branch instead.
    content = config_path.read_text()
    base_branch_line = f'base_branch = "{base_commit}"\n'
    # Remove any existing base_branch line first (idempotent)
    lines = [l for l in content.splitlines(keepends=True) if not l.startswith("base_branch =")]
    # Find the first [section] line and insert before it; if none, append at end
    insert_at = next((i for i, l in enumerate(lines) if l.startswith("[")), len(lines))
    lines.insert(insert_at, base_branch_line)
    dest.write_text("".join(lines))

    # Prune stale git worktree registrations and delete old task branches.
    # git worktree prune won't remove registrations whose gitdir is gone until the
    # grace period expires (default 3 months). Delete .git/worktrees/ directly so
    # the task/* branches become deletable immediately.
    git_worktrees_meta = repo_path / ".git" / "worktrees"
    if git_worktrees_meta.exists():
        subprocess.run(["rm", "-rf", str(git_worktrees_meta)], check=True)
    subprocess.run(["git", "worktree", "prune"], cwd=repo_path, capture_output=True)
    result = subprocess.run(
        ["git", "branch", "--list", "task/*"],
        cwd=repo_path, capture_output=True, text=True,
    )
    for branch in result.stdout.splitlines():
        branch = branch.strip().lstrip("*").strip()
        if branch:
            subprocess.run(
                ["git", "branch", "-D", branch],
                cwd=repo_path, capture_output=True,
            )

    # Ensure .agtx/ is ignored locally without touching .gitignore (which is a tracked file).
    # .git/info/exclude is a local-only ignore file — changes never appear in git diff.
    exclude_file = repo_path / ".git" / "info" / "exclude"
    marker = ".agtx/"
    existing = exclude_file.read_text() if exclude_file.exists() else ""
    if marker not in existing:
        with exclude_file.open("a") as f:
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
# Docker helpers
# ---------------------------------------------------------------------------

def docker_image_name(instance_id: str) -> str:
    """Convert an instance_id to its SWE-bench Docker image name.

    e.g. astropy__astropy-12907 → swebench/sweb.eval.x86_64.astropy_1776_astropy-12907:latest
    """
    image_slug = instance_id.replace("__", "_1776_")
    return f"swebench/sweb.eval.x86_64.{image_slug}:latest"


TOOLS_IMAGE = "agtx/swebench-tools:latest"
TOOLS_CONTAINER = "agtx-swebench-tools"
TOOLS_VOLUME = "agtx-swebench-tools"


def _tools_image_exists() -> bool:
    """Return True if the shared tools image has been built by prebake_images.py."""
    result = subprocess.run(
        ["docker", "image", "inspect", TOOLS_IMAGE],
        capture_output=True,
    )
    return result.returncode == 0


def _tools_volume_exists() -> bool:
    result = subprocess.run(
        ["docker", "volume", "inspect", TOOLS_VOLUME],
        capture_output=True,
    )
    return result.returncode == 0


def ensure_tools_volume(verbose: bool = False) -> None:
    """Create and populate the tools volume from the tools image if it doesn't exist.

    The volume contains tmux + libutempter (so SWE-bench containers don't need apt),
    node, node_modules, and rtk. Mounting read-only at /tools avoids copying the large
    Node stack into instance containers' writable layers.
    """
    if _tools_volume_exists():
        if verbose:
            print(f"  [docker] Tools volume '{TOOLS_VOLUME}' already exists.", file=sys.stderr)
        return
    if verbose:
        print(f"  [docker] Creating tools volume '{TOOLS_VOLUME}'...", file=sys.stderr)
    subprocess.run(["docker", "volume", "create", TOOLS_VOLUME], check=True, capture_output=True)
    # Populate volume by running a one-shot container that copies from the image into the volume.
    # libutempter is copied alongside tmux because SWE-bench containers don't have it.
    subprocess.run(
        [
            "docker", "run", "--rm", "--platform", "linux/amd64",
            "-v", f"{TOOLS_VOLUME}:/tools",
            TOOLS_IMAGE,
            "/bin/bash", "-c",
            "cp -a /usr/bin/tmux /usr/bin/node /usr/lib/node_modules /tools/"
            " && mkdir -p /tools/lib/x86_64-linux-gnu"
            " && cp /lib/x86_64-linux-gnu/libutempter.so.* /tools/lib/x86_64-linux-gnu/"
            " && cp /lib/x86_64-linux-gnu/libevent_core-2.1.so.* /tools/lib/x86_64-linux-gnu/",
        ],
        check=True, capture_output=not verbose,
    )
    if verbose:
        print(f"  [docker] Tools volume ready.", file=sys.stderr)


def start_tools_container(verbose: bool = False) -> str:
    """Start the shared tools container and return its container ID.

    The tools container is a long-running instance of agtx/swebench-tools:latest
    used as a source to copy tmux/Node/Claude Code binaries into benchmark containers.
    Starting it once and sharing it across all concurrent runs avoids redundant installs.
    """
    # Kill any stale tools container
    subprocess.run(["docker", "rm", "-f", TOOLS_CONTAINER], capture_output=True)
    result = subprocess.run(
        ["docker", "run", "-d", "--platform", "linux/amd64",
         "--name", TOOLS_CONTAINER, TOOLS_IMAGE, "sleep", "infinity"],
        check=True, capture_output=True, text=True,
    )
    container_id = result.stdout.strip()
    if verbose:
        print(f"  [docker] Started tools container {container_id[:12]}", file=sys.stderr)
    return container_id


def stop_tools_container(verbose: bool = False) -> None:
    subprocess.run(["docker", "rm", "-f", TOOLS_CONTAINER], capture_output=True)


def _agtx_db_dir() -> str:
    """Return the host path to the agtx database directory."""
    if sys.platform == "darwin":
        return str(Path.home() / "Library" / "Application Support" / "agtx")
    return str(Path.home() / ".config" / "agtx")


def _docker_env_overrides() -> list[str]:
    """Build -e flags for docker run to fix localhost URLs in Claude env settings.

    Reads ~/.claude/settings.json env overrides (read-only, never modified) and
    replaces http://localhost: with http://host.docker.internal: so proxies running
    on the macOS host are reachable from inside the container.
    Returns a flat list of ["-e", "KEY=VALUE", ...] suitable for subprocess args.
    """
    settings_path = Path.home() / ".claude" / "settings.json"
    if not settings_path.exists():
        return []
    try:
        data = json.loads(settings_path.read_text())
        env_overrides = data.get("env", {})
    except Exception:
        return []
    result = []
    for key, val in env_overrides.items():
        if isinstance(val, str) and "localhost" in val:
            val = val.replace("http://localhost:", "http://host.docker.internal:")
            val = val.replace("https://localhost:", "https://host.docker.internal:")
            result += ["-e", f"{key}={val}"]
    return result


def start_docker_container(
    instance_id: str, slug: str, agtx_bin: str, verbose: bool = False
) -> str:
    """Pull the SWE-bench image and start a long-running container. Returns container_id."""
    container_name = f"swebench-{slug}"
    image = docker_image_name(instance_id)

    # Kill any stale container with the same name
    subprocess.run(["docker", "rm", "-f", container_name], capture_output=True)

    if verbose:
        print(f"  [docker] Pulling {image}...", file=sys.stderr)
    subprocess.run(
        ["docker", "pull", "--platform", "linux/amd64", image],
        check=True,
        capture_output=not verbose,
    )

    if verbose:
        print(f"  [docker] Starting container {container_name}...", file=sys.stderr)

    # Mount the tools volume if available — provides tmux, node, node_modules, rtk
    # without consuming writable layer space in the instance container.
    extra_mounts = []
    if _tools_volume_exists():
        extra_mounts = ["-v", f"{TOOLS_VOLUME}:/tools:ro"]
        if verbose:
            print(f"  [docker] Mounting tools volume.", file=sys.stderr)

    run_result = subprocess.run(
        [
            "docker", "run", "-d",
            "--platform", "linux/amd64",
            "--name", container_name,
            "-v", f"{agtx_bin}:/usr/local/bin/agtx",
        ] + extra_mounts + [
            image,
            "sleep", "infinity",
        ],
        capture_output=True,
        text=True,
        check=True,
    )
    container_id = run_result.stdout.strip()
    if verbose:
        print(f"  [docker] Container id: {container_id[:12]}", file=sys.stderr)
    return container_id


def setup_container(
    container_id: str, config_path: Path, base_commit: str, verbose: bool = False,
    tools_container_id: str | None = None,
    extra_dirs: list[tuple[Path, str]] | None = None,
    sandbox_init: list[str] | None = None,
) -> None:
    """Write agtx config into the container and install tmux + Node + Claude Code.

    If tools_container_id is provided, binaries are copied from the shared tools
    container instead of being installed from the network (~3-5 min saved).

    extra_dirs is an optional list of (host_path, container_dest) pairs for copying
    additional files into the container (e.g. third-party plugin skill directories).

    sandbox_init is an optional list of shell commands to run inside the container
    after setup (e.g. ["rtk init -g"] to activate rtk hooks).
    """
    # Parse default_agent from the benchmark config so we can write a global agtx config
    # inside the container. Without it agtx shows the agent-selection wizard on startup.
    bench_config = load_config_toml(config_path)
    default_agent = bench_config.get("default_agent", "claude")

    # Copy Claude credentials into the container as snapshots (not mounts).
    # This avoids writing back to host files and lets us patch localhost URLs.
    # We only copy the credential/config files — NOT ~/.claude/projects/ which can be
    # gigabytes of conversation history and would exhaust the container's disk space.
    home = Path.home()
    subprocess.run(
        ["docker", "exec", container_id, "/bin/bash", "-c",
         "mkdir -p /home/bench/.config/claude /home/bench/.claude"],
        check=True, capture_output=not verbose,
    )
    # ~/.config/claude/ — API key and auth tokens
    config_claude = home / ".config" / "claude"
    if config_claude.exists():
        subprocess.run(
            ["docker", "cp", f"{config_claude}/.", f"{container_id}:/home/bench/.config/claude"],
            check=True, capture_output=not verbose,
        )
    # ~/.claude/ — copy only top-level files (settings.json, credentials), skip subdirs
    # (projects/, worktrees/, etc.) which contain large conversation history
    claude_dir = home / ".claude"
    if claude_dir.exists():
        for item in claude_dir.iterdir():
            if item.is_file():
                subprocess.run(
                    ["docker", "cp", str(item), f"{container_id}:/home/bench/.claude/{item.name}"],
                    check=True, capture_output=not verbose,
                )

    # Copy ~/.claude.json as a snapshot so Claude skips the first-launch welcome screen.
    claude_json = home / ".claude.json"
    if claude_json.exists():
        subprocess.run(
            ["docker", "cp", str(claude_json), f"{container_id}:/home/bench/.claude.json"],
            check=True, capture_output=not verbose,
        )

    # Copy extra directories into the container (e.g. third-party plugin skill dirs).
    for src, dst in (extra_dirs or []):
        if not src.exists():
            print(f"  [docker] Warning: extra_dir source not found, skipping: {src}", file=sys.stderr)
            continue
        if verbose:
            print(f"  [docker] Copying {src} → container:{dst}", file=sys.stderr)
        subprocess.run(
            ["docker", "exec", container_id, "/bin/bash", "-c", f"mkdir -p {dst}"],
            capture_output=True,
        )
        subprocess.run(
            ["docker", "cp", f"{src}/.", f"{container_id}:{dst}"],
            check=True, capture_output=not verbose,
        )

    # Patch localhost → host.docker.internal in settings.json inside the container.
    # Claude reads ANTHROPIC_BASE_URL from settings.json["env"] which overrides process env,
    # so we must fix it in the file itself (container-local copy only, host file untouched).
    subprocess.run(
        ["docker", "exec", container_id, "/bin/bash", "-c",
         r"sed -i 's|http://localhost:|http://host.docker.internal:|g'"
         r" /home/bench/.claude/settings.json 2>/dev/null || true"],
        capture_output=True,
    )

    # Write global agtx config for bench user so the TUI skips the agent-selection wizard
    global_config_content = f'default_agent = "{default_agent}"\n'
    if verbose:
        print(f"  [docker] Writing global agtx config (default_agent={default_agent})...", file=sys.stderr)
    subprocess.run(
        ["docker", "exec", container_id,
         "/bin/bash", "-c",
         f"mkdir -p /home/bench/.config/agtx && cat > /home/bench/.config/agtx/config.toml << 'ENDOFCONFIG'\n{global_config_content}\nENDOFCONFIG"],
        check=True,
        capture_output=not verbose,
    )

    # Write .agtx/config.toml into /testbed inside the container
    # Build the config content (with base_branch injected) in memory
    content = config_path.read_text()
    base_branch_line = f'base_branch = "{base_commit}"\n'
    lines = [l for l in content.splitlines(keepends=True) if not l.startswith("base_branch =")]
    insert_at = next((i for i, l in enumerate(lines) if l.startswith("[")), len(lines))
    lines.insert(insert_at, base_branch_line)
    config_content = "".join(lines)

    if verbose:
        print(f"  [docker] Writing .agtx/config.toml into container...", file=sys.stderr)
    subprocess.run(
        ["docker", "exec", container_id,
         "/bin/bash", "-c",
         f"mkdir -p /testbed/.agtx && cat > /testbed/.agtx/config.toml << 'ENDOFCONFIG'\n{config_content}\nENDOFCONFIG"],
        check=True,
        capture_output=not verbose,
    )

    # Clean stale agtx state (worktrees, project DB) — same as _write_agtx_config
    subprocess.run(
        ["docker", "exec", container_id,
         "/bin/bash", "-c",
         "rm -rf /testbed/.agtx/worktrees /testbed/.agtx/db"],
        capture_output=True,
    )

    # Activate the conda 'testbed' environment for all new shells.
    # SWE-bench images ship with repo dependencies pre-installed in conda env 'testbed'.
    # Without this, Claude uses the system python/pip and tries to reinstall everything.
    subprocess.run(
        ["docker", "exec", container_id, "/bin/bash", "-c",
         "echo 'source /opt/miniconda3/etc/profile.d/conda.sh && conda activate testbed'"
         " >> /root/.bashrc"],
        check=True, capture_output=not verbose,
    )

    if tools_container_id:
        # Tools volume is mounted at /tools (read-only). Symlink everything from there —
        # zero writable-layer usage. libutempter is included in the volume because
        # SWE-bench containers don't have it (tmux needs it).
        if verbose:
            print(f"  [docker] Wiring tools from volume...", file=sys.stderr)
        subprocess.run(
            ["docker", "exec", container_id, "/bin/bash", "-c",
             "ln -sf /tools/tmux /usr/bin/tmux"
             " && ln -sf /tools/node /usr/bin/node"
             " && ln -sf /tools/node_modules/npm/bin/npm-cli.js /usr/bin/npm"
             " && ln -sf /tools/node_modules/npm/bin/npx-cli.js /usr/bin/npx"
             " && ln -sf /tools/node_modules/@anthropic-ai/claude-code/bin/claude.exe /usr/bin/claude"
             " && mkdir -p /usr/lib/node_modules"
             " && ln -sf /tools/node_modules/@anthropic-ai /usr/lib/node_modules/@anthropic-ai"
             " && ln -sf /tools/node_modules/npm /usr/lib/node_modules/npm"
             " && test -f /tools/node_modules/.bin/caveman && ln -sf /tools/node_modules/.bin/caveman /usr/bin/caveman || true"
             " && mkdir -p /usr/lib/x86_64-linux-gnu"
             " && ln -sf /tools/lib/x86_64-linux-gnu/libutempter.so.0 /usr/lib/x86_64-linux-gnu/libutempter.so.0"
             " && ln -sf /tools/lib/x86_64-linux-gnu/libutempter.so.1.2.1 /usr/lib/x86_64-linux-gnu/libutempter.so.1.2.1"
             " && ln -sf /tools/lib/x86_64-linux-gnu/libevent_core-2.1.so.7 /usr/lib/x86_64-linux-gnu/libevent_core-2.1.so.7"],
            check=True, capture_output=not verbose,
        )
    else:
        if verbose:
            print(f"  [docker] Installing tmux, Node.js 22, Claude Code...", file=sys.stderr)
        subprocess.run(
            ["docker", "exec", container_id,
             "/bin/bash", "-c",
             "apt-get update -qq && apt-get install -y -qq tmux curl ca-certificates "
             "&& curl -fsSL https://deb.nodesource.com/setup_22.x | bash - "
             "&& apt-get install -y -qq nodejs "
             "&& npm install -g @anthropic-ai/claude-code"],
            check=True,
            capture_output=not verbose,
        )

    # Run optional sandbox init commands (e.g. "rtk init -g").
    # These run after credentials are in place so tools can write into ~/.claude/.
    # Prepend ~/.local/bin to PATH so tools installed there (e.g. rtk) are found.
    for cmd in (sandbox_init or []):
        if verbose:
            print(f"  [docker] sandbox_init: {cmd}", file=sys.stderr)
        subprocess.run(
            ["docker", "exec", container_id, "/bin/bash", "-c",
             f"HOME=/home/bench && export HOME PATH=$HOME/.local/bin:$PATH && {cmd}"],
            check=True, capture_output=not verbose,
        )


def start_tui_in_container(slug: str, container_id: str, verbose: bool = False) -> None:
    """Start the agtx TUI inside the container's tmux session."""
    if verbose:
        print(f"  [docker] Starting agtx TUI in container tmux session '{slug}'...", file=sys.stderr)
    # Launch agtx with all required env vars baked into the command so every tmux
    # window it spawns (agent sessions) inherits them from the start — no race.
    #   HOME=/home/bench   — agtx reads ~/.config/agtx/config.toml
    #   IS_SANDBOX=1       — bypasses Claude's root check (--dangerously-skip-permissions)
    #   PATH/CONDA vars    — activate conda 'testbed' env so pre-installed deps are used
    testbed_bin = "/opt/miniconda3/envs/testbed/bin"
    env_prefix = (
        f"HOME=/home/bench "
        f"IS_SANDBOX=1 "
        f"CONDA_DEFAULT_ENV=testbed "
        f"CONDA_PREFIX=/opt/miniconda3/envs/testbed "
        f"PATH={testbed_bin}:/opt/miniconda3/bin:/home/bench/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"
    )
    subprocess.run(
        ["docker", "exec", "-d", container_id,
         "tmux", "new-session", "-d", "-s", slug,
         f"env {env_prefix} agtx /testbed"],
        check=True,
        capture_output=not verbose,
    )
    # Also set them in the agtx tmux server global env so windows created later
    # (e.g. after a phase transition) also inherit the correct PATH.
    time.sleep(3)
    for key, val in [
        ("HOME", "/home/bench"),
        ("IS_SANDBOX", "1"),
        ("CONDA_DEFAULT_ENV", "testbed"),
        ("CONDA_PREFIX", "/opt/miniconda3/envs/testbed"),
        ("PATH", f"{testbed_bin}:/opt/miniconda3/bin:/home/bench/.local/bin:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin"),
    ]:
        subprocess.run(
            ["docker", "exec", container_id,
             "tmux", "-L", "agtx", "set-environment", "-g", key, val],
            capture_output=True,
        )
    # Give the TUI time to fully start before the caller proceeds
    time.sleep(5)


def stop_docker_container(container_id: str, verbose: bool = False) -> None:
    """Stop and remove the container."""
    if verbose:
        print(f"  [docker] Stopping container {container_id[:12]}...", file=sys.stderr)
    subprocess.run(["docker", "stop", container_id], capture_output=True)
    subprocess.run(["docker", "rm", container_id], capture_output=True)


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
        return {"cost_usd": None, "cost_tokens": None}

    inp  = max(0, after["input"]   - before["input"])
    out  = max(0, after["output"]  - before["output"])
    cost = max(0.0, after["cost_usd"] - before["cost_usd"])
    total = inp + out + max(0, after["cache_read"]  - before["cache_read"]) + \
                        max(0, after["cache_write"] - before["cache_write"]) + \
                        max(0, after["reasoning"]   - before["reasoning"])

    return {
        "cost_usd":    round(cost, 6) if cost > 0 else None,
        "cost_tokens": total if total > 0 else None,
    }


def tokscale_from_container(container_id: str, agent: str) -> dict:
    """
    Read token/cost usage from a sandbox container by copying /home/bench/.claude
    to a temp dir and running tokscale --home against it.
    Returns the standard cost_data dict {cost_usd, cost_tokens}, or nulls on failure.
    """
    tokscale = _find_tokscale()
    if not tokscale:
        return {"cost_usd": None, "cost_tokens": None}

    flag = _TOKSCALE_FLAG.get(agent)
    if not flag:
        return {"cost_usd": None, "cost_tokens": None}

    try:
        with tempfile.TemporaryDirectory() as tmpdir:
            # Copy /home/bench/.claude out of the container
            result = subprocess.run(
                ["docker", "cp", f"{container_id}:/home/bench/.claude", tmpdir],
                capture_output=True,
            )
            if result.returncode != 0:
                return {"cost_usd": None, "cost_tokens": None}

            # tokscale --home expects the home dir (parent of .claude)
            result = subprocess.run(
                [tokscale, "--home", tmpdir, "--json", flag],
                capture_output=True,
                text=True,
                timeout=15,
            )
            if result.returncode != 0 or not result.stdout.strip():
                return {"cost_usd": None, "cost_tokens": None}

            data = json.loads(result.stdout)
            entries = data.get("entries", [])

            inp = out = cr = cw = reasoning = 0
            cost = 0.0
            for entry in entries:
                inp       += entry.get("input", 0)
                out       += entry.get("output", 0)
                cr        += entry.get("cacheRead", 0)
                cw        += entry.get("cacheWrite", 0)
                reasoning += entry.get("reasoning", 0)
                cost      += entry.get("cost", 0.0)

            total = inp + out + cr + cw + reasoning
            return {
                "cost_usd":    round(cost, 6) if cost > 0 else None,
                "cost_tokens": total if total > 0 else None,
            }
    except Exception:
        return {"cost_usd": None, "cost_tokens": None}




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
        error: str | None = None,
    ) -> None:
        with self._lock:
            result = {
                "instance_id": instance_id,
                "status": status,
                "duration_seconds": round(duration_seconds, 1),
                "cost_usd": cost_usd,
                "cost_tokens": cost_tokens,
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
    "agent-skills":     None,
    "superpowers":      "docs/superpowers/plans/*.md",
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
    "openspec":         None,
    "agent-skills":     None,
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
    "agent-skills":     None,
    "superpowers":      None,
    "oh-my-claudecode": None,
    "void":             None,
}


def _artifact_exists(worktree: Path, pattern: str, container_id: str | None = None) -> bool:
    """Return True if the artifact path (possibly with * or {phase} placeholders) exists.

    In Docker mode (container_id provided), checks inside the container via docker exec
    instead of the local filesystem (the worktree lives inside the container).
    """
    if "{phase}" in pattern:
        for n in range(1, 21):
            for fmt in (f"{n:02d}", str(n)):
                if _artifact_exists(worktree, pattern.replace("{phase}", fmt), container_id):
                    return True
        return False
    if container_id:
        # In Docker: use shell glob via docker exec so * patterns work
        path = str(worktree / pattern)
        result = subprocess.run(
            ["docker", "exec", container_id, "/bin/bash", "-c", f"ls {path} 2>/dev/null | head -1"],
            capture_output=True, text=True,
        )
        return bool(result.stdout.strip())
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

    SMOKE_TEST_DESCRIPTION = (
        "This is a smoke test run. Do not modify any source files.\n\n"
        "Planning phase: write the following content to `.agtx/plan.md` in the current working directory:\n"
        "```\n"
        "# Smoke Test Plan\n"
        "This is a smoke test. In the running phase: write an empty `.agtx/execute.md`. "
        "In the review phase: write an empty `.agtx/review.md`. Do not modify any source files.\n"
        "```\n"
        "Then stop and wait."
    )

    NOTE = (
        "IMPORTANT CONSTRAINTS — these apply to all phases including review:\n"
        "- NEVER install packages, dependencies, or build the project.\n"
        "- NEVER run tests under any circumstances.\n"
        "- Do not run any git commands (no fetch, pull, merge, or commit).\n"
        "- The repo may not be installable in this environment — do not attempt it.\n"
        "- Read the source code, understand the bug, and fix it by editing the relevant files directly."
    )

    @staticmethod
    def _strip_code(problem_statement: str) -> str:
        """Remove fenced code blocks and inline code, keep prose."""
        # Remove fenced code blocks (``` ... ```)
        text = re.sub(r"```.*?```", "", problem_statement, flags=re.DOTALL)
        # Collapse runs of blank lines left behind
        text = re.sub(r"\n{3,}", "\n\n", text)
        return text.strip()

    def _build_description(self, problem_statement: str) -> str:
        if self.hard:
            return self._strip_code(problem_statement) + "\n\n---\n" + self.NOTE
        return problem_statement + "\n\n---\n" + self.NOTE

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
        smoke_test: bool = False,
        hard: bool = False,
        container_id: str | None = None,
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
        self.smoke_test = smoke_test
        self.hard = hard
        self.container_id = container_id
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
            if _artifact_exists(worktree, artifact_pattern, self.container_id):
                return True
            time.sleep(5)
        return False

    # Patterns that indicate a shell command run by the agent is waiting for input,
    # blocking Claude from finishing. Only used in the stability fallback path.
    _INTERACTIVE_PROMPT_PATTERNS = [
        r"\[y/N\]",
        r"\[Y/n\]",
        r"\[y/n\]",
        r"\[Y/N\]",
        r"Press Enter to continue",
        r"Press any key",
    ]

    def _check_interactive_prompt(self, content: str) -> str | None:
        """Return the matching line if pane content looks like an interactive prompt, else None."""
        for line in content.splitlines()[-20:]:
            for pattern in self._INTERACTIVE_PROMPT_PATTERNS:
                if re.search(pattern, line, re.IGNORECASE):
                    return line.strip()
        return None

    # Claude always prints one of these lines when it finishes a response.
    # Detecting this (plus the ❯ prompt) is more reliable than pure pane stability.
    _CLAUDE_FINISHED_RE = re.compile(r"✻\s+[A-Z][a-z]+ for \d+s")

    def _is_claude_idle(self, content: str) -> bool:
        """Return True when Claude has finished its last response and shows the ❯ prompt."""
        lines = content.splitlines()
        has_finish_marker = any(self._CLAUDE_FINISHED_RE.search(l) for l in lines)
        has_prompt = any(l.strip() == "❯" or l.strip().startswith("❯ ") for l in lines[-10:])
        return has_finish_marker and has_prompt

    def _wait_for_pane_stable(self, task_id: str, timeout: int) -> None:
        """Wait until Claude has finished its last response (no artifact available).

        Primary signal: Claude prints '✻ Cooked/Worked/Crunched for Xs' + ❯ prompt.
        Fallback: pane content identical for 2 consecutive 10s checks.

        If an interactive prompt is detected while stable, prints a warning so the user
        can attach to the session and answer it.
        """
        prev_content: str | None = None
        stable_count = 0
        prompt_warned = False
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            time.sleep(5)
            try:
                result = self.mcp.call("read_pane_content", task_id=task_id, lines=80)
                content = result.get("content", "")
            except McpError:
                content = ""
            # Primary: Claude idle signal
            if self._is_claude_idle(content):
                return
            # Interactive prompt check (even when not yet stable)
            if content == prev_content:
                stable_count += 1
                prompt_line = self._check_interactive_prompt(content)
                if prompt_line:
                    if not prompt_warned:
                        print(f"\n[{self.instance_id}] ⚠ Agent is waiting for input: \"{prompt_line}\"", file=sys.stderr)
                        slug = self.instance_id.replace("__", "-").replace("_", "-")
                        print(f"  Attach to answer: docker exec -it swebench-{slug} tmux -L agtx attach -t testbed:1", file=sys.stderr)
                        prompt_warned = True
                    stable_count = 0  # Reset — not actually done
                elif stable_count >= 2:
                    # Pane stable but no Claude finish marker — needs attention.
                    # Possible causes: agent error, waiting for user approval (e.g. superpowers brainstorm),
                    # or genuinely stuck. Warn once and keep polling; do NOT advance.
                    if not prompt_warned:
                        slug = self.instance_id.replace("__", "-").replace("_", "-")
                        print(f"\n[{self.instance_id}] ⚠ Pane stable but no finish marker — agent may be stuck, hit an error, or requires manual approval.", file=sys.stderr)
                        print(f"  Attach to inspect or interact: docker exec -it swebench-{slug} tmux -L agtx attach -t testbed:1", file=sys.stderr)
                        prompt_warned = True
                    stable_count = 0  # Keep polling, don't advance
            else:
                stable_count = 0
                prompt_warned = False
                prev_content = content

    def run(self) -> dict:
        start = time.monotonic()
        if self.verbose:
            print(f"  [{self.instance_id}] Starting MCP handshake...", file=sys.stderr)
        self.mcp = McpClient(self.agtx_bin, str(self.repo_path), container_id=self.container_id)
        task_id = None

        try:
            if self.verbose:
                print(f"  [{self.instance_id}] Creating task...", file=sys.stderr)
            problem = self.instance.get("problem_statement", "")
            if self.smoke_test:
                description = self.SMOKE_TEST_DESCRIPTION
            else:
                description = self._build_description(problem)
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

            # Snapshot tokscale before agent starts working (non-sandbox only)
            cost_before = _tokscale_snapshot(self.running_agent) if not self.container_id else None

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

            # Collect token/cost usage:
            # - sandbox: read directly from container's /home/bench/.claude via tokscale --home
            # - non-sandbox: diff host tokscale snapshots taken before/after running phase
            if self.container_id:
                cost_data = tokscale_from_container(self.container_id, self.running_agent)
            else:
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
        base_commit = self.instance["base_commit"]

        if self.container_id:
            # In Docker mode, diff /testbed directly inside the container
            result = subprocess.run(
                [
                    "docker", "exec", self.container_id,
                    "git", "-C", "/testbed", "diff", base_commit,
                    "--", ".", ":!.agtx/",
                ],
                capture_output=True,
            )
            if result.returncode != 0:
                return ""
            return result.stdout.decode("utf-8", errors="replace")

        worktree_path = task.get("worktree_path")
        if not worktree_path:
            return ""
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
        smoke_test: bool = False,
        hard: bool = False,
        docker: bool = False,
        extra_dirs: list[tuple[Path, str]] | None = None,
        sandbox_init: list[str] | None = None,
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
        self.smoke_test = smoke_test
        self.hard = hard
        self.docker = docker
        self.extra_dirs = extra_dirs or []
        self.sandbox_init = sandbox_init or []
        self.tools_container_id: str | None = None  # set in run() when --sandbox is active

    def _run_one(self, instance: dict, progress: tqdm) -> None:
        instance_id = instance["instance_id"]

        if self.store.is_done(instance_id):
            progress.update(1)
            return

        slug = re.sub(r"[^a-z0-9]+", "-", instance_id.lower()).strip("-")[:50]
        start = time.monotonic()

        container_id: str | None = None
        repo_path: Path | None = None

        if self.docker:
            if self.verbose:
                print(f"\n[{instance_id}] Starting Docker container...", file=sys.stderr)
            try:
                container_id = start_docker_container(
                    instance_id, slug, self.agtx_bin, verbose=self.verbose
                )
                setup_container(
                    container_id, self.config_path, instance["base_commit"],
                    verbose=self.verbose, tools_container_id=self.tools_container_id,
                    extra_dirs=self.extra_dirs or None,
                    sandbox_init=self.sandbox_init or None,
                )
            except Exception as e:
                if container_id:
                    stop_docker_container(container_id, verbose=self.verbose)
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
                print(f"[{instance_id}] Starting TUI inside container...", file=sys.stderr)
                print(f"  [docker] To watch the agent:", file=sys.stderr)
                print(f"    docker exec -it swebench-{slug} tmux -L agtx attach -t testbed:1", file=sys.stderr)
            try:
                start_tui_in_container(slug, container_id, verbose=self.verbose)
                # start_tui_in_container already waits for TUI startup internally
            except Exception as e:
                stop_docker_container(container_id, verbose=self.verbose)
                self.store.save_result(
                    instance_id=instance_id,
                    status="setup_error",
                    duration_seconds=time.monotonic() - start,
                    model_name=self.model_name,
                    error=f"TUI startup failed: {e}",
                )
                progress.update(1)
                return

            # In Docker mode, repo_path is unused by McpClient/patch collection
            # but TaskRunner still needs a Path object (used as identifier).
            repo_path = Path("/testbed")

        else:
            if self.verbose:
                print(f"\n[{instance_id}] Setting up repo...", file=sys.stderr)
            try:
                repo_path = setup_repo(
                    instance, self.workdir, self.config_path,
                    verbose=self.verbose, smoke_test=self.smoke_test,
                )
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
                time.sleep(8)  # Wait for TUI startup + project registration
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
            smoke_test=self.smoke_test,
            hard=self.hard,
            container_id=container_id,
        )
        result = runner.run()

        if self.docker:
            stop_docker_container(container_id, verbose=self.verbose)
        else:
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

        if self.docker and _tools_image_exists():
            if self.verbose:
                print("[docker] Ensuring tools volume is ready...", file=sys.stderr)
            ensure_tools_volume(verbose=self.verbose)
            self.tools_container_id = "volume"  # sentinel: volume is mounted, no container needed

        try:
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
        finally:
            if self.tools_container_id and self.tools_container_id != "volume":
                stop_tools_container(verbose=self.verbose)

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
        help="Path to agtx binary (default: ../target/release/agtx relative to benchmark/)",
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
    parser.add_argument(
        "--smoke-test",
        action="store_true",
        dest="smoke_test",
        help="Replace task description with a trivial prompt (create artifact files and stop). "
             "Use to verify the full pipeline works without spending tokens on real coding work.",
    )
    parser.add_argument(
        "--hard",
        action="store_true",
        dest="hard",
        help="Strip code blocks and stack traces from the problem statement, keeping only the prose. "
             "The agent must find and fix the bug from first principles.",
    )
    parser.add_argument(
        "--sandbox",
        action="store_true",
        dest="docker",
        help=(
            "Run each task inside its SWE-bench Docker image "
            "(swebench/sweb.eval.x86_64.*). Gives agents a working test "
            "environment with pre-installed dependencies. "
            "Requires Docker with Rosetta on Apple Silicon."
        ),
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

    # Resolve sandbox_copy_dirs — paths relative to the config file's directory
    extra_dirs: list[tuple[Path, str]] = []
    for entry in config.get("sandbox_copy_dirs", []):
        src = (config_path.parent / entry["src"]).resolve()
        dst = entry["dst"]
        extra_dirs.append((src, dst))

    # Resolve sandbox_init — shell commands to run inside the container after setup
    sandbox_init: list[str] = config.get("sandbox_init", [])

    # Resolve agtx binary
    agtx_bin = str(Path(args.agtx).resolve())
    if not Path(agtx_bin).exists():
        print(f"agtx binary not found: {agtx_bin}", file=sys.stderr)
        print("Build with: cargo build --release", file=sys.stderr)
        sys.exit(1)

    model_name = args.model_name or config_path.stem

    if args.output_dir is None:
        ts = datetime.now().strftime("%Y%m%d_%H%M%S")
        config_stem = config_path.stem  # e.g. "claude-rtk-caveman-ponytail-agtx"
        output_dir = Path(f"./swebench_output/{config_stem}_{ts}").resolve()
    else:
        output_dir = Path(args.output_dir).resolve()

    # In smoke test mode with no explicit instance selection, skip loading the dataset
    # entirely and use a single synthetic instance.
    if args.smoke_test and args.instance_ids is None and args.instances is None:
        instances = [{"instance_id": "smoke-test", "repo": "", "base_commit": "HEAD", "problem_statement": ""}]
    else:
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
        smoke_test=args.smoke_test,
        hard=args.hard,
        docker=args.docker,
        extra_dirs=extra_dirs or None,
        sandbox_init=sandbox_init or None,
    )
    orchestrator.run()

    print(f"\nPredictions: {orchestrator.store.predictions_path}")
    print(f"Results:     {orchestrator.store.results_path}")
    run_id = f"{config_path.stem}-{int(time.time())}"
    print("\nTo evaluate:")
    print(f"  uv run python -m swebench.harness.run_evaluation \\")
    print(f"    --dataset_name princeton-nlp/SWE-bench_Lite \\")
    print(f"    --predictions_path {orchestrator.store.predictions_path} \\")
    print(f"    --run_id {run_id}")
    print("\nTo report (after evaluation):")
    print(f"  uv run python swebench/report.py \\")
    print(f"    --results {orchestrator.store.results_path} \\")
    print(f"    --logs logs/run_evaluation/{run_id}/")


if __name__ == "__main__":
    main()
