#!/usr/bin/env python3
"""Per-agent smoke tests: does a real agent binary actually receive its work?

Rationale, decisions and findings: see README.md next to this file. The short
version: three silent-hang bugs were found by hand in one session, all of the same shape
— the prompt was delivered and then never read. None was catchable by the unit
suite, which mocks tmux, or by the parity tests, which pin the strings agtx
builds. They live in the gap between agtx and a real agent binary.

Per phase this asserts, in order:

  1. command submitted   the resolved skill command reached the agent as a
                         submitted message, not as text parked in a composer
  2. skill ran           a marker file contains the task id that was passed in,
                         proving argument substitution rather than "a skill fired"
  3. artifact + advance  the phase artifact appeared and agtx moved the task on
  4. session usable      the agent process is alive and no dialog is on screen

Check 4 is the one that matters most: antigravity's skill *ran* and wrote its
file while the session sat blocked on an unanswered trust dialog. A harness that
only asserts output would have certified it green.

Never pre-answers a dialog. It *does* trust the scratch repo **root** for the agent
first (`setup_agent_trust`), which is what a user does once before using agtx — and
that is what gives Check 4 its teeth. Every scratch repo is a fresh mkdtemp path, so
without it every agent opens on its trust dialog and "parked on a dialog" is the
expected state: exactly the noise two undeclared-dialog bugs hid in. With the root
trusted, a dialog during a *task* is a real defect.

It also makes the run a test of the inheritance property itself. claude, codex and
gemini inherit the root's trust into their worktrees, so a dialog there means that
stopped being true. antigravity matches paths exactly and never inherits, so agtx
must seed each worktree from that consent — if the seeding breaks, its cases park
and the run says so.

Usage:
    tests/smoke/agent_smoke.py                      # installed agents x agtx plugin
    tests/smoke/agent_smoke.py --agents claude,codex
    tests/smoke/agent_smoke.py --agents claude --plugins all
    tests/smoke/agent_smoke.py --phases planning,running,review --json out.json

Requires: a built agtx binary (cargo build --release), tmux, git, cargo, and
real agent auth. Spends real tokens — opt-in, never CI.
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import re
import shlex
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass, field
from glob import glob
from pathlib import Path
from typing import Any, Iterable

REPO_ROOT = Path(__file__).resolve().parents[2]

# The tmux server the runner starts TUIs on. agtx spawns agent windows on its
# own `agtx` server; keeping the TUIs elsewhere means killing ours never takes
# an agent pane with it.
TUI_SERVER = "agtx-smoke"
AGENT_SERVER = "agtx"

# Phases the runner can drive, in order. Research is deliberately absent: it runs
# in place on a Backlog task and is a different transition shape.
ORDERED_PHASES = ["planning", "running", "review"]
PHASE_STATUS = {"planning": "planning", "running": "running", "review": "review"}

# Dialogs agtx does **not** answer, so they never appear in AGENT_SPECS. Check 4
# exists to catch exactly this class: a session that took the prompt and then
# parked, or one where agtx's own Enter answered a menu by accident.
#
# Antigravity's "Do you trust the contents of this project?" used to head this
# list, with a note that agtx deliberately left it to the user. This runner is
# what proved that reasoning wrong — the session did not wait for the user, it
# swallowed the paste and took agtx's Enter as the answer — so the dialog is
# declared now and comes from AGENT_SPECS like the rest. Anything left here is a
# prompt no agent claims; add to it rather than assuming a blocked pane is fine.
UNHANDLED_DIALOG_PATTERNS = [
    "Press Enter to continue",
    "Press any key to continue",
    "[y/N]",
    "[Y/n]",
]

# Agents known to park on a dialog agtx does not answer. A case that delivers its
# work and *then* parks is reported as `expected-park`, not `pass` and not
# `fail`: the delivery worked, the session is unusable, and both facts matter.
#
# **Empty, and that is the point.** It held `antigravity` for exactly one run.
# The harness then reported the entry as stale — the agent passed cleanly once
# agtx learned to answer its trust dialog — which is the guard the plan asked for
# (open question 4) doing its job: an expectation that encodes today's behaviour
# must announce itself the moment today's behaviour improves, or it silently
# becomes the specification.
EXPECTED_PARK_AGENTS: set[str] = set()

# A usage-limit park is a known failure mode of long agent runs and is not a bug
# in the send path. Reported distinctly so it never shows up as a red column.
#
# Narrow on purpose. A bare "usage limit" matched Codex's startup tip ("You have
# 1 usage limit reset available"), turning a live session into a QUOTA row and
# hiding a real delivery failure underneath it. Every pattern here must describe
# a limit that has actually been *hit*.
QUOTA_PATTERNS = [
    # Covers Claude's "You've hit your session limit" and Grok's "You hit your
    # free usage limit." — same sentence, different agents, different wording.
    r"You(?:'ve)? hit your [\w ]*limit",
    r"usage limit (reached|exceeded)",
    r"rate limit (reached|exceeded)",
    r"quota exceeded",
    r"Press .* to continue after reset",
    r"upgrade to increase your usage limit",
]

# Only consulted to explain an already-failing case — an unauthenticated agent
# looks exactly like an undelivered prompt from the outside.
AUTH_PATTERNS = [
    r"Sign in to",
    r"Please log in",
    r"/login",
    r"not authenticated",
    r"(No|Invalid|missing) API key",
    # Gemini's wording, and the machine-readable reason next to it. Both are
    # unambiguous credential rejections — unlike its own "This request failed"
    # summary line, which is generic and would launder real failures if matched.
    r"API key not valid",
    r"API_KEY_INVALID",
    # A provider misconfiguration reads exactly like an undelivered prompt from
    # the outside: the command was submitted, the model call failed, no artifact
    # appeared. opencode on Amazon Bedrock with no AWS credentials produced this
    # and the case was reported as a send-path FAIL until the pattern was added.
    r"requires AWS credentials",
    r"access key ID setting is missing",
    r"authentication (failed|error)",
    # Gemini's first-run OAuth prompt. agtx deliberately does not answer it — an
    # Enter would confirm "Yes" and open a browser to start an OAuth flow, which
    # is not a side effect a task transition gets to have.
    r"Opening authentication page",
    r"credentials (are )?(not )?(set|missing|required)",
]

# Interpreters an agent may be shipped as. Finding one in the pane's process
# tree means something other than a shell is running there.
RUNTIME_NAMES = {"node", "bun", "deno", "python", "python3", "ruby", "java"}

PASS, FAIL, SKIP = "pass", "fail", "skipped"
QUOTA, AUTH, PARK, ERROR = "blocked-on-quota", "blocked-on-auth", "expected-park", "error"


# ---------------------------------------------------------------------------
# Matrix — read from the agent spec table rather than duplicated here
# ---------------------------------------------------------------------------

def load_matrix(cargo: str = "cargo") -> dict:
    """Run the `agent_matrix` example and parse its JSON.

    Everything per-agent (binary name, dialogs, command syntax, liveness
    signals) and per-plugin (artifacts, commands, prompts) comes from the real
    tables in src/, so the runner cannot drift from what agtx actually does.
    """
    proc = subprocess.run(
        [cargo, "run", "--quiet", "--example", "agent_matrix"],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        raise RuntimeError(
            f"cargo run --example agent_matrix failed:\n{proc.stderr.strip()}"
        )
    return json.loads(proc.stdout)


# ---------------------------------------------------------------------------
# MCP client (JSON-RPC 2.0 over the agtx MCP server's stdio)
# ---------------------------------------------------------------------------

class McpError(Exception):
    pass


class McpClient:
    """Minimal MCP client. Mirrors benchmark/swebench/benchmark.py's client.

    `_recv` has a hard timeout because a bare `readline()` on a live-but-silent
    server blocks forever, and no phase timeout can rescue it — the block is
    inside the MCP call rather than inside the poll the timeout governs.
    """

    RECV_TIMEOUT_SECONDS = 120

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
        # Lines are read on a thread rather than by selecting on the fd. A
        # `readline()` on a TextIOWrapper can pull several lines into Python's
        # buffer at once; selecting on the raw descriptor afterwards then reports
        # nothing readable and the next `_recv` times out on a response that is
        # already in hand. Reachable whenever the server flushes a notification
        # and a reply together, which `_request`'s skip-until-matching-id loop
        # does not otherwise mind.
        self._lines: queue.Queue = queue.Queue()
        self._reader = threading.Thread(target=self._pump, daemon=True)
        self._reader.start()
        self._handshake()

    def _pump(self) -> None:
        for line in self._proc.stdout:
            self._lines.put(line)
        self._lines.put(None)  # EOF sentinel

    def _next_id(self) -> int:
        with self._lock:
            self._seq += 1
            return self._seq

    def _send(self, msg: dict) -> None:
        self._proc.stdin.write(json.dumps(msg) + "\n")
        self._proc.stdin.flush()

    def _recv(self) -> dict:
        try:
            line = self._lines.get(timeout=self.RECV_TIMEOUT_SECONDS)
        except queue.Empty:
            raise McpError(f"MCP server silent for {self.RECV_TIMEOUT_SECONDS}s")
        if line is None:
            raise McpError("MCP server closed connection")
        return json.loads(line)

    def _request(self, method: str, params: dict) -> Any:
        req_id = self._next_id()
        self._send({"jsonrpc": "2.0", "id": req_id, "method": method, "params": params})
        while True:
            msg = self._recv()
            if msg.get("id") != req_id:
                continue  # notification
            if "error" in msg:
                raise McpError(f"MCP error: {msg['error']}")
            result = msg.get("result", {})
            content = result.get("content", [])
            if content and content[0].get("type") == "text":
                text = content[0]["text"]
                try:
                    return json.loads(text)
                except json.JSONDecodeError:
                    # Tools report failures as plain text, not JSON-RPC errors.
                    raise McpError(text.strip())
            if result.get("isError"):
                raise McpError(f"Tool returned error: {content}")
            return result

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
                "clientInfo": {"name": "agtx-smoke", "version": "1.0"},
            },
        })
        while True:
            if self._recv().get("id") == req_id:
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
# Case planning
# ---------------------------------------------------------------------------

@dataclass
class Case:
    agent: str
    plugin: str
    skip_reason: str | None = None


def plugin_prereq(plugin: dict) -> str | None:
    """Why this plugin cannot run against a scratch repo, or None.

    Derived from the plugin's own TOML rather than a hand-kept list:

    - an `init_script` installs a framework (npx, `claude plugin install`), which
      needs the network and a real toolchain;
    - `copy_dirs` names a directory the framework creates in the project root
      (`.specify`, `openspec`), and its slash commands only exist once it is there;
    - a plugin with no command and no prompt for any phase (void) sends the agent
      nothing at all, so there is no delivery to observe.
    """
    if plugin.get("init_script"):
        return "plugin init_script needs network/toolchain setup"
    if plugin.get("copy_dirs"):
        dirs = ", ".join(plugin["copy_dirs"])
        return f"plugin needs {dirs} in the project root"
    if not plugin.get("commands") and not plugin.get("prompts"):
        return "plugin sends the agent nothing by design"
    return None


def phase_is_assertable(plugin: dict, phase: str) -> bool:
    """Whether the task text can reach the agent in this phase at all.

    A phase whose command and prompt carry neither `{task}` nor `{task_id}`
    (agtx's `review`, which sends a bare `/agtx:review`) gives the agent no way
    to learn what the smoke task asked for — especially under
    `clear_context_on_advance`. Such a phase is still driven, but only checks 3
    and 4 apply to it.
    """
    text = (plugin.get("commands", {}).get(phase) or "") + (
        plugin.get("prompts", {}).get(phase) or ""
    )
    return "{task}" in text or "{task_id}" in text


def plan_cases(
    matrix: dict,
    agents_arg: str,
    plugins_arg: str,
    force: bool = False,
) -> list[Case]:
    """Build the case list, marking every excluded case with a visible reason.

    A silently narrowed matrix is worse than no run: it invites the assumption
    that the agents it never touched work. Every exclusion becomes a `skipped`
    row with a reason, never an omission.
    """
    by_agent = {a["name"]: a for a in matrix["agents"]}
    by_plugin = {p["name"]: p for p in matrix["plugins"]}

    if agents_arg in ("installed", ""):
        agent_names = [n for n, a in by_agent.items() if shutil.which(a["binary"])]
        if not agent_names:
            agent_names = list(by_agent)
    elif agents_arg == "all":
        agent_names = list(by_agent)
    else:
        agent_names = [n.strip() for n in agents_arg.split(",") if n.strip()]

    if plugins_arg == "all":
        plugin_names = list(by_plugin)
    else:
        plugin_names = [p.strip() for p in plugins_arg.split(",") if p.strip()]

    cases: list[Case] = []
    for plugin_name in plugin_names:
        plugin = by_plugin.get(plugin_name)
        for agent_name in agent_names:
            agent = by_agent.get(agent_name)
            if agent is None:
                cases.append(Case(agent_name, plugin_name, "unknown agent"))
                continue
            if plugin is None:
                cases.append(Case(agent_name, plugin_name, "unknown plugin"))
                continue
            if not shutil.which(agent["binary"]):
                cases.append(
                    Case(agent_name, plugin_name, f"{agent['binary']} not installed")
                )
                continue
            supported = plugin.get("supported_agents") or []
            if supported and agent_name not in supported:
                cases.append(
                    Case(agent_name, plugin_name, "agent not in supported_agents")
                )
                continue
            prereq = plugin_prereq(plugin)
            if prereq and not force:
                cases.append(Case(agent_name, plugin_name, prereq))
                continue
            cases.append(Case(agent_name, plugin_name))
    return cases


# ---------------------------------------------------------------------------
# Scratch repo
# ---------------------------------------------------------------------------

def run(cmd: list[str], cwd: Path | None = None, check: bool = True) -> subprocess.CompletedProcess:
    proc = subprocess.run(cmd, cwd=cwd, capture_output=True, text=True)
    if check and proc.returncode != 0:
        raise RuntimeError(
            f"{' '.join(cmd)} failed ({proc.returncode}): {proc.stderr.strip()}"
        )
    return proc


def make_scratch_repo(workdir: Path, slug: str) -> Path:
    """A one-commit git repo — the smallest thing agtx will open as a project."""
    repo = workdir / slug
    repo.mkdir(parents=True, exist_ok=True)
    run(["git", "init", "-b", "main"], cwd=repo)
    run(["git", "config", "user.email", "smoke@agtx.local"], cwd=repo)
    run(["git", "config", "user.name", "agtx smoke"], cwd=repo)
    run(["git", "config", "commit.gpgsign", "false"], cwd=repo)
    (repo / "README.md").write_text(
        "# agtx smoke test scratch repo\n\nDisposable. Created by tests/smoke/agent_smoke.py.\n"
    )
    (repo / "main.py").write_text('def main():\n    print("hello")\n')
    run(["git", "add", "-A"], cwd=repo)
    run(["git", "commit", "-m", "initial commit"], cwd=repo)
    return repo


def write_project_config(repo: Path, agent: str, plugin: str) -> None:
    agtx_dir = repo / ".agtx"
    agtx_dir.mkdir(exist_ok=True)
    (agtx_dir / "config.toml").write_text(
        "\n".join(
            [
                f'default_agent = "{agent}"',
                'base_branch = "main"',
                'worktree_dir = ".agtx/worktrees"',
                'branch_prefix = "task"',
                f'workflow_plugin = "{plugin}"',
                "",
            ]
        )
    )
    exclude = repo / ".git" / "info" / "exclude"
    exclude.parent.mkdir(parents=True, exist_ok=True)
    with exclude.open("a") as f:
        f.write("\n# agtx smoke artifacts\n.agtx/\n")


def trust_project(agtx_bin: str, repo: Path) -> None:
    """Trust the scratch repo's own config.

    This trusts *agtx's* project config, which is what gates plugin init scripts
    and copy_files. Agent trust is a separate thing — see `setup_agent_trust`.
    """
    run([agtx_bin, "trust"], cwd=repo)


# How each agent records that a directory is trusted, and whether a trusted
# ancestor covers what lies beneath it. Mirrors `src/agent/trust.rs`; the runner
# writes these directly rather than driving each agent's dialog, because this is
# **setup**, not the thing under test.
AGENT_TRUST_STORES = {
    "claude": "claude",
    "codex": "codex",
    "gemini": "gemini",
    "antigravity": "antigravity",
}


def setup_agent_trust(agent: str, repo: Path) -> bool:
    """Trust the scratch repo *root* for `agent`, as a user would before using agtx.

    This is the step that makes Check 4 mean something. Every scratch repo is a
    fresh `mkdtemp` path, so without it every agent opens on its first-launch
    trust dialog and "parked on a dialog" is the expected state — which is exactly
    the noise that let two undeclared-dialog bugs hide. With the root trusted, a
    dialog appearing during a *task* is a real defect.

    It also turns the run into a test of the inheritance property itself: claude,
    codex and gemini trust the root and their worktrees inherit it, so a dialog in
    a worktree means that stopped being true. antigravity matches exactly and never
    inherits, so agtx must seed each worktree from this consent — if that seeding
    breaks, its cases park and the run says so.

    Returns whether anything was written. cursor and grok pass `--trust` at launch,
    opencode has no trust gate, and copilot is unmeasured — nothing to set up, and
    nothing to guess at.
    """
    kind = AGENT_TRUST_STORES.get(agent)
    if kind is None:
        return False
    home = Path.home()
    root = str(repo.resolve())
    if kind == "claude":
        path = home / ".claude.json"
        data = json.loads(path.read_text()) if path.exists() else {}
        data.setdefault("projects", {}).setdefault(root, {})["hasTrustDialogAccepted"] = True
        atomic_write_json(path, data)
    elif kind == "codex":
        path = home / ".codex" / "config.toml"
        path.parent.mkdir(parents=True, exist_ok=True)
        body = path.read_text() if path.exists() else ""
        header = f'[projects."{root}"]'
        if header not in body:
            path.write_text(body + f'\n{header}\ntrust_level = "trusted"\n')
    elif kind == "gemini":
        path = home / ".gemini" / "trustedFolders.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        data = json.loads(path.read_text()) if path.exists() else {}
        # gemini lowercases what it stores.
        data[root.lower()] = "TRUST_FOLDER"
        atomic_write_json(path, data)
    elif kind == "antigravity":
        path = home / ".gemini" / "antigravity-cli" / "settings.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        data = json.loads(path.read_text()) if path.exists() else {}
        entries = data.setdefault("trustedWorkspaces", [])
        if root not in entries:
            entries.append(root)
        atomic_write_json(path, data)
    return True


def trust_store_lists(agent: str, path: Path) -> bool | None:
    """Whether `agent`'s trust store still lists `path` exactly.

    `None` for agents with no store agtx seeds. Only the exact-match store
    (antigravity) is meaningful here: the others record an ancestor, which agtx
    neither wrote nor prunes.
    """
    if AGENT_TRUST_STORES.get(agent) != "antigravity":
        return None
    store = Path.home() / ".gemini" / "antigravity-cli" / "settings.json"
    if not store.exists():
        return False
    try:
        entries = json.loads(store.read_text()).get("trustedWorkspaces", [])
    except (json.JSONDecodeError, OSError):
        return False
    return str(path.resolve()) in entries or str(path) in entries


def atomic_write_json(path: Path, data) -> None:
    """Write JSON via a temp file and a rename.

    These are live config files the agents rewrite wholesale; a partial write is
    visible to them.
    """
    tmp = path.with_suffix(path.suffix + f".agtx-smoke{os.getpid()}")
    tmp.write_text(json.dumps(data, indent=2))
    tmp.replace(path)


# ---------------------------------------------------------------------------
# tmux
# ---------------------------------------------------------------------------

def tmux(server: str, *args: str, check: bool = False) -> subprocess.CompletedProcess:
    return run(["tmux", "-L", server, *args], check=check)


def start_tui(session: str, repo: Path, agtx_bin: str) -> None:
    tmux(TUI_SERVER, "kill-session", "-t", session)
    tmux(AGENT_SERVER, "kill-session", "-t", repo.name)
    # tmux runs this through a shell, so both words are quoted: a --workdir
    # containing a space would otherwise make agtx open the wrong path, and the
    # failure would surface much later as "no worktree" with no obvious cause.
    proc = tmux(
        TUI_SERVER, "new-session", "-d", "-s", session, "-x", "200", "-y", "50",
        f"{shlex.quote(str(agtx_bin))} {shlex.quote(str(repo))}",
    )
    if proc.returncode != 0:
        raise RuntimeError(f"tmux new-session failed: {proc.stderr.strip()}")


def stop_tui(session: str, repo: Path) -> None:
    tmux(TUI_SERVER, "kill-session", "-t", session)
    tmux(AGENT_SERVER, "kill-session", "-t", repo.name)


def pane_command(target: str) -> str | None:
    proc = tmux(
        AGENT_SERVER, "display-message", "-p", "-t", target, "#{pane_current_command}"
    )
    if proc.returncode != 0:
        return None
    return proc.stdout.strip() or None


def pane_pid(target: str) -> int | None:
    proc = tmux(AGENT_SERVER, "display-message", "-p", "-t", target, "#{pane_pid}")
    if proc.returncode != 0:
        return None
    try:
        return int(proc.stdout.strip())
    except ValueError:
        return None


# ---------------------------------------------------------------------------
# Task description — the fixture
# ---------------------------------------------------------------------------

def concrete_artifact(template: str, cycle: int = 1) -> str:
    """A writable path for an artifact template.

    `{phase}` becomes the cycle number and `*` becomes a literal `smoke`, so the
    agent is told one exact path while `find_artifact` still accepts anything the
    glob matches — which is what agtx itself checks.
    """
    return template.replace("{phase}", str(cycle)).replace("*", "smoke")


def find_artifact(worktree: Path, template: str, cycle: int = 1) -> bool:
    """Mirror of agtx's `artifact_path_exists`: both {phase} spellings, glob-aware."""
    for phase_str in (f"{cycle:02d}", str(cycle)):
        rel = template.replace("{phase}", phase_str)
        full = str(worktree / rel)
        if "*" in rel:
            if glob(full):
                return True
        elif Path(full).exists():
            return True
    return False


def marker_rel(phase: str) -> str:
    return f".agtx/smoke/{phase}.txt"


INSTRUCTIONS_REL = ".agtx/smoke/INSTRUCTIONS.md"

# Every phase artifact is written with this one line, and it is the whole reason
# the fixture survives a phase advance.
#
# A later phase does not necessarily see the task description again: agtx's
# `execute` skill says, in as many words, "if .agtx/plan.md exists, read it — do
# NOT call get_task", and the agtx plugin clears the agent's context between
# phases. So an artifact holding "smoke" strands the running phase with no idea
# what it was asked for. Pointing at a file the runner drops in the worktree
# carries the fixture forward through exactly the mechanism the plugin uses to
# carry a plan forward.
ARTIFACT_POINTER = f"SMOKE TEST — follow the instructions in {INSTRUCTIONS_REL}"


def phase_instructions(task_id: str, artifacts: dict[str, str], phases: list[str]) -> str:
    """The per-phase file list, shared by the task description and the fixture."""
    lines = []
    for phase in phases:
        lines.append(f"{phase.upper()} phase — write these files, then stop:")
        lines.append(f"  1. {marker_rel(phase)} containing one line: DELIVERED {task_id}")
        if artifacts.get(phase):
            lines.append(f"  2. {artifacts[phase]} containing one line: {ARTIFACT_POINTER}")
        lines.append("")
    return "\n".join(lines)


SMOKE_RULES = [
    "SMOKE TEST — this is not a coding task.",
    "These instructions override anything the phase skill tells you to do.",
    "",
    "Rules: do not read, create, or modify any source file. Do not explore the",
    "codebase. Do not run tests, builds, installs, or git commands. Do not spawn",
    "subagents. Do not write a summary.",
    "",
    "Do the work for the phase you are currently in, then stop and wait. Ignore the",
    "other phases' entries.",
    "",
]


def build_description(task_id: str, artifacts: dict[str, str], phases: list[str]) -> str:
    """The smallest task that still proves delivery.

    The marker carries the **task id** rather than fixed text: a skill that runs
    but drops its argument is a real failure mode, and a fixed-content marker
    cannot see it. For the agtx plugin the id only reaches the agent as the
    argument to `/agtx:plan <id>`, and the description itself is only readable by
    calling `get_task` with it — so a correct marker proves the whole chain.
    """
    return "\n".join(
        SMOKE_RULES
        + [phase_instructions(task_id, artifacts, phases)]
        + [
            f"The id to write into the marker is exactly: {task_id}",
            f"If a phase skill tells you not to fetch the task description, read",
            f"{INSTRUCTIONS_REL} instead — it holds this same list.",
            "Paths are relative to the current working directory; create parent dirs as needed.",
            "Write nothing else and stop as soon as the files exist.",
        ]
    )


def write_fixture(worktree: Path, task_id: str, artifacts: dict[str, str],
                  phases: list[str]) -> None:
    """Drop the instruction fixture into the worktree.

    Written by the runner rather than by the agent so that a phase which never
    sees the description again still has somewhere to look — see
    [`ARTIFACT_POINTER`].
    """
    path = worktree / INSTRUCTIONS_REL
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "\n".join(SMOKE_RULES + [phase_instructions(task_id, artifacts, phases)])
    )


# ---------------------------------------------------------------------------
# Assertions
# ---------------------------------------------------------------------------

@dataclass
class Check:
    name: str
    state: str  # pass | fail | unknown | n/a
    detail: str = ""

    SYMBOLS = {"pass": "✓", "fail": "✗", "unknown": "?", "n/a": "–"}

    @property
    def symbol(self) -> str:
        return self.SYMBOLS.get(self.state, "?")


def tail(text: str, n: int) -> str:
    lines = [l for l in text.splitlines()]
    return "\n".join(lines[-n:])


def match_any(patterns: Iterable[str], text: str) -> str | None:
    for pat in patterns:
        m = re.search(pat, text, re.IGNORECASE)
        if m:
            return m.group(0)
    return None


def visible_dialog(pane: str, all_dialogs: list[dict]) -> str | None:
    """A dialog on screen right now, whether or not agtx knows how to answer it.

    Scans only the pane tail: a dialog is modal, so if it is still blocking it is
    on the visible screen. Matching further back would re-find dialogs agtx
    already answered, which scroll away but stay in history.
    """
    screen = tail(pane, 30)
    for dialog in all_dialogs:
        patterns = dialog["patterns"]
        hit = (
            all(p in screen for p in patterns)
            if dialog.get("require_all")
            else any(p in screen for p in patterns)
        )
        if hit:
            owner = dialog.get("owner", "?")
            return f"{owner}: {patterns[0]}"
    for pat in UNHANDLED_DIALOG_PATTERNS:
        if pat in screen:
            return f"unhandled: {pat}"
    return None


def process_tree(pid: int) -> list[str]:
    """Command names of `pid` and every descendant.

    One `ps` call, then a downward walk — portable across macOS and Linux.
    """
    proc = run(["ps", "-eo", "pid=,ppid=,comm="], check=False)
    if proc.returncode != 0:
        return []
    children: dict[int, list[int]] = {}
    names: dict[int, str] = {}
    for line in proc.stdout.splitlines():
        parts = line.split(None, 2)
        if len(parts) < 3:
            continue
        try:
            child, parent = int(parts[0]), int(parts[1])
        except ValueError:
            continue
        names[child] = Path(parts[2].strip().lstrip("-")).name
        children.setdefault(parent, []).append(child)
    seen, stack, out = set(), [pid], []
    while stack:
        current = stack.pop()
        if current in seen:
            continue
        seen.add(current)
        if current in names:
            out.append(names[current])
        stack.extend(children.get(current, []))
    return out


def agent_is_live(target: str, pane: str, agent: dict) -> bool:
    """Is the agent process still running in this pane?

    Deliberately stronger than agtx's own `is_agent_active`, which asks tmux for
    `pane_current_command`. That reports the **pane leader**, and every agent is
    launched as `sh -c '<agent>'` with `exec $SHELL` behind it, so a live Claude
    pane reports `bash` and a dead one reports `bash` too. Verified against
    tmux 3.5a on macOS: `pane_current_command` was `bash` while `claude` ran as
    its child.

    So the pane's process **tree** is what is checked: an agent shipped as a node
    script shows up as `node`, one shipped as a binary under its own name, and a
    pane whose agent has exited has nothing but a shell left. `active_indicators`
    alone does not generalise — copilot and antigravity declare none.
    """
    cmd = pane_command(target)
    if cmd and any(name in cmd for name in agent["process_names"]):
        return True
    pid = pane_pid(target)
    if pid:
        tree = process_tree(pid)
        if any(name in tree for name in agent["process_names"]):
            return True
        if any(name in RUNTIME_NAMES for name in tree):
            return True
    indicators = agent.get("active_indicators") or []
    if indicators and any(i in tail(pane, 6) for i in indicators):
        return True
    return False


def check_command_submitted(pane: str, command: str | None, work_landed: bool) -> Check:
    """Check 1: the command was submitted, not left sitting in a composer.

    This is the check that separates "agtx typed something" from "the agent
    understood it". A submitted message has the agent's response rendered under
    it; a command parked in a composer is the last thing on screen with nothing
    below.

    Two deliberate softenings, both because the pane is a rendered TUI rather
    than a log:

    - the command not appearing at all is `unknown`, not a failure — several
      TUIs redraw their scrollback, and check 2 catches a genuinely undelivered
      command anyway;
    - the command at the bottom of the screen only fails when nothing landed.
      Claude Code renders a dim *suggestion* for the next phase's command in its
      empty composer (verified against 2.1.245: the line carries SGR 2, which
      `capture-pane -p` strips), so the bottom line is not proof of a park.
    """
    if not command:
        return Check("submitted", "n/a", "agent has no interactive command syntax")
    needle = command.strip()
    lines = [l.rstrip() for l in pane.splitlines()]
    hits = [i for i, l in enumerate(lines) if needle in l]
    if not hits:
        # Wrapped panes break long commands across lines; fall back to the head.
        head = needle.split()[0]
        hits = [i for i, l in enumerate(lines) if head in l]
        if not hits:
            return Check("submitted", "unknown", "command text not visible in pane")
    non_empty = [i for i, l in enumerate(lines) if l.strip()]
    last = non_empty[-1] if non_empty else 0
    if last - hits[-1] <= 1 and not work_landed:
        return Check(
            "submitted", "fail", "command is the last thing on screen and nothing ran"
        )
    return Check("submitted", "pass")


def check_marker(worktree: Path, phase: str, task_id: str, assertable: bool) -> Check:
    """Check 2: the skill body ran, with the argument it was given."""
    if not assertable:
        return Check("skill-ran", "n/a", "phase carries no task text")
    path = worktree / marker_rel(phase)
    if not path.exists():
        return Check("skill-ran", "fail", f"{marker_rel(phase)} never written")
    content = path.read_text(errors="replace").strip()
    if task_id not in content:
        return Check(
            "skill-ran", "fail", f"marker lacks the task id: {content[:120]!r}"
        )
    return Check("skill-ran", "pass")


def check_artifact(worktree: Path, template: str | None, cycle: int) -> Check:
    if not template:
        return Check("artifact", "n/a", "plugin declares no artifact for this phase")
    if find_artifact(worktree, template, cycle):
        return Check("artifact", "pass")
    return Check("artifact", "fail", f"{template} never appeared")


def check_usable(target: str, pane: str, agent: dict, all_dialogs: list[dict],
                 agent_state: str | None, work_landed: bool = False) -> Check:
    """Check 4: is this session still usable by a human?

    The honest floor (plan open question 1): the agent process is alive **and**
    no dialog matches. `active_indicators` alone does not generalise — copilot
    and antigravity declare none, so "the agent's own indicator is visible" is
    not a definition that covers the matrix.

    The agent's hook-reported state is folded in, but only as a tiebreaker when
    nothing landed. agtx maps Claude's `Notification` event to `blocked`, and
    that event covers both "waiting on a permission prompt" and "has been
    waiting for your input" — a phase that finished and went idle reports
    `blocked` too. Failing on it unconditionally would mark every completed
    phase unusable.
    """
    dialog = visible_dialog(pane, all_dialogs)
    if dialog:
        return Check("usable", "fail", f"dialog on screen — {dialog}")
    if not agent_is_live(target, pane, agent):
        return Check("usable", "fail", "agent process is gone (pane at shell)")
    if agent_state == "blocked" and not work_landed:
        return Check("usable", "fail", "agent reports itself blocked and nothing ran")
    return Check("usable", "pass")


# ---------------------------------------------------------------------------
# Driving one case
# ---------------------------------------------------------------------------

@dataclass
class PhaseResult:
    phase: str
    checks: list[Check] = field(default_factory=list)
    outcome: str = PASS
    pane_tail: str = ""

    def failed(self) -> bool:
        return any(c.state == "fail" for c in self.checks)


@dataclass
class CaseResult:
    agent: str
    plugin: str
    outcome: str
    reason: str = ""
    phases: list[PhaseResult] = field(default_factory=list)
    seconds: float = 0.0
    workdir: str = ""
    # Set when retiring the task to Done went wrong — reported, but never allowed
    # to overwrite the phase outcome the case is actually about.
    cleanup_note: str = ""

    @property
    def phase_reached(self) -> str:
        return self.phases[-1].phase if self.phases else "-"


class CaseRunner:
    def __init__(self, case: Case, matrix: dict, opts: argparse.Namespace, workdir: Path):
        self.case = case
        self.opts = opts
        self.agent = next(a for a in matrix["agents"] if a["name"] == case.agent)
        self.plugin = next(p for p in matrix["plugins"] if p["name"] == case.plugin)
        # Every agent's declared dialogs, tagged with their owner. Check 4 wants
        # any dialog on screen, including one belonging to another agent — that
        # is itself a bug worth seeing.
        self.all_dialogs = [
            dict(d, owner=a["name"]) for a in matrix["agents"] for d in a["dialogs"]
        ]
        self.workdir = workdir
        self.repo: Path | None = None
        self.mcp: McpClient | None = None
        self.session = f"smoke-{case.agent}-{case.plugin}"

    # -- helpers ---------------------------------------------------------
    def log(self, msg: str) -> None:
        if self.opts.verbose:
            print(f"  [{self.case.agent}/{self.case.plugin}] {msg}", file=sys.stderr)

    def pane(self, task_id: str, lines: int = 200) -> str:
        try:
            return self.mcp.call("read_pane_content", task_id=task_id, lines=lines).get(
                "content", ""
            )
        except McpError:
            return ""

    def poll_transition(self, request_id: str, timeout: int) -> None:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            res = self.mcp.call("get_transition_status", request_id=request_id)
            if res.get("status") == "completed":
                return
            if res.get("status") == "error":
                raise RuntimeError(f"transition failed: {res.get('error')}")
            time.sleep(2)
        raise TimeoutError(f"transition {request_id} timed out after {timeout}s")

    def move_forward(self, task_id: str) -> None:
        res = self.mcp.call("move_task", task_id=task_id, action="move_forward")
        self.poll_transition(res["request_id"], self.opts.transition_timeout)

    def wait_for(self, task_id: str, predicate, timeout: int, what: str) -> dict:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            task = self.mcp.call("get_task", task_id=task_id)
            if predicate(task):
                return task
            time.sleep(3)
        raise TimeoutError(f"{what} not reached within {timeout}s")

    # -- phases ----------------------------------------------------------
    def drive_phase(self, task_id: str, phase: str, worktree: Path, cycle: int) -> PhaseResult:
        """Wait for the phase to land, then run the four checks.

        The wait ends early on a dialog or a quota park: both mean nothing more
        is coming, and burning the full timeout only delays the diagnosis.
        """
        assertable = phase_is_assertable(self.plugin, phase)
        artifact = self.plugin.get("artifacts", {}).get(phase)
        marker = worktree / marker_rel(phase)
        deadline = time.monotonic() + self.opts.phase_timeout
        pane = ""
        early = None

        while time.monotonic() < deadline:
            pane = self.pane(task_id)
            quota = match_any(QUOTA_PATTERNS, tail(pane, 40))
            if quota:
                early = (QUOTA, f"agent parked on a usage limit: {quota!r}")
                break
            # A credential error is terminal for the phase — the model was never
            # reached — so there is nothing to gain from waiting out the timeout.
            auth = match_any(AUTH_PATTERNS, tail(pane, 40))
            if auth:
                early = (AUTH, f"provider rejected the request: {auth!r}")
                break
            done_marker = (not assertable) or marker.exists()
            done_artifact = (not artifact) or find_artifact(worktree, artifact, cycle)
            if done_marker and done_artifact:
                # Let the agent settle so check 1 sees the response rendered
                # under the command rather than mid-stream.
                time.sleep(5)
                pane = self.pane(task_id)
                break
            dialog = visible_dialog(pane, self.all_dialogs)
            if dialog:
                # Give agtx its dismissal window before calling it stuck: the
                # readiness loop and the session-refresh loop both answer known
                # dialogs, the latter only after the pane stops changing.
                time.sleep(20)
                pane = self.pane(task_id)
                if visible_dialog(pane, self.all_dialogs):
                    early = (FAIL, f"session parked on a dialog: {dialog}")
                    break
            time.sleep(5)
        else:
            early = (FAIL, f"phase did not complete within {self.opts.phase_timeout}s")

        task = self.mcp.call("get_task", task_id=task_id)
        target = task.get("session_name") or ""
        command = self.resolved_command(phase, task_id, cycle)

        # Check 2 and 3 first: check 1 needs to know whether anything landed
        # before it can read a bottom-of-screen command as a parked composer.
        marker_check = check_marker(worktree, phase, task_id, assertable)
        artifact_check = check_artifact(worktree, artifact, cycle)
        work_landed = marker_check.state == "pass" or artifact_check.state == "pass"

        result = PhaseResult(phase=phase, pane_tail=tail(pane, 20))
        result.checks = [
            check_command_submitted(pane, command, work_landed),
            marker_check,
            artifact_check,
            check_usable(
                target, pane, self.agent, self.all_dialogs, task.get("agent_state"),
                work_landed,
            ),
        ]

        if early and early[0] in (QUOTA, AUTH):
            result.outcome = early[0]
            result.checks.append(Check("note", "unknown", early[1]))
            return result

        # Also consulted at the end: an auth prompt that only appears once the
        # phase has already failed still explains the failure.
        auth = match_any(AUTH_PATTERNS, tail(pane, 40))
        if result.failed() and auth:
            result.outcome = AUTH
            result.checks.append(Check("note", "unknown", f"auth prompt: {auth!r}"))
            return result

        # The checks are the verdict, not the reason the wait ended: an
        # artifact landing between the last poll and the capture is a pass, not a
        # timeout.
        result.outcome = FAIL if result.failed() else PASS
        return result

    def resolved_command(self, phase: str, task_id: str, cycle: int) -> str | None:
        template = self.agent["commands"].get(self.case.plugin, {}).get(phase)
        if not template:
            return None
        return (
            template.replace("{task_id}", task_id)
            .replace("{phase}", str(cycle))
            .split("{task}")[0]
            .strip()
        )

    # -- lifecycle -------------------------------------------------------
    def run(self) -> CaseResult:
        started = time.monotonic()
        res = CaseResult(self.case.agent, self.case.plugin, PASS)
        # Bound before the try so teardown can still retire a task created just
        # before something threw.
        task_id: str | None = None
        worktree: Path | None = None
        try:
            slug = f"{self.case.agent}-{self.case.plugin}"
            self.repo = make_scratch_repo(self.workdir, slug)
            res.workdir = str(self.repo)
            write_project_config(self.repo, self.case.agent, self.case.plugin)
            trust_project(self.opts.agtx_bin, self.repo)
            if setup_agent_trust(self.case.agent, self.repo):
                self.log(f"trusted repo root for {self.case.agent} (setup, not under test)")
            self.log(f"scratch repo at {self.repo}")

            start_tui(self.session, self.repo, self.opts.agtx_bin)
            time.sleep(3)  # let the TUI open the project DB
            self.mcp = McpClient(self.opts.agtx_bin, str(self.repo))

            created = self.mcp.call(
                "create_task",
                title=f"smoke {self.case.agent} {self.case.plugin}",
                description="placeholder",
                plugin=self.case.plugin,
            )
            task_id = created["id"]
            artifacts = {
                p: concrete_artifact(t)
                for p, t in (self.plugin.get("artifacts") or {}).items()
            }
            self.mcp.call(
                "update_task",
                task_id=task_id,
                description=build_description(task_id, artifacts, self.opts.phases),
            )
            self.log(f"task {task_id}")

            self.move_forward(task_id)
            task = self.wait_for(
                task_id,
                lambda t: t.get("status") == "planning" and t.get("worktree_path"),
                self.opts.worktree_timeout,
                "planning + worktree",
            )
            worktree = Path(task["worktree_path"])
            write_fixture(worktree, task_id, artifacts, self.opts.phases)
            self.log(f"worktree {worktree}")

            for index, phase in enumerate(self.opts.phases):
                phase_res = self.drive_phase(task_id, phase, worktree, task.get("cycle", 1))
                res.phases.append(phase_res)
                if phase_res.outcome != PASS:
                    res.outcome = phase_res.outcome
                    res.reason = next(
                        (c.detail for c in phase_res.checks if c.state == "fail"),
                        next((c.detail for c in phase_res.checks if c.detail), ""),
                    )
                    break
                if index + 1 < len(self.opts.phases):
                    self.move_forward(task_id)
                    nxt = self.opts.phases[index + 1]
                    task = self.wait_for(
                        task_id,
                        lambda t, s=PHASE_STATUS[nxt]: t.get("status") == s,
                        self.opts.transition_timeout,
                        f"status {nxt}",
                    )
            else:
                res.outcome = PASS

            res = self.apply_park_expectation(res)
        except (McpError, TimeoutError, RuntimeError, OSError) as exc:
            res.outcome = ERROR
            res.reason = f"{type(exc).__name__}: {exc}"
        finally:
            # Retire before tearing anything down: the TUI has to be alive to
            # process the transition, and the MCP client is how it is asked.
            if self.mcp and task_id and not self.opts.keep_sessions:
                note = self.retire_task(task_id, worktree)
                if note:
                    self.log(f"cleanup: {note}")
                    res.cleanup_note = note
            if self.mcp:
                self.mcp.close()
            if not self.opts.keep_sessions and self.repo:
                stop_tui(self.session, self.repo)
            res.seconds = time.monotonic() - started
        return res

    def retire_task(self, task_id: str, worktree: Path | None) -> str | None:
        """Drive the task to Done, so the real cleanup path runs.

        Deleting the scratch repo is not the same as finishing a task: worktree
        removal, `cleanup_script` and the agent-trust prune all hang off
        `cleanup_task_for_done`. Without this the runner exercised none of them,
        and every run left the seeded worktree behind in antigravity's
        `trustedWorkspaces` — which is how the omission was noticed.

        Returns a note when something looks wrong, otherwise `None`. Never raises:
        a problem here is worth reporting but must not overwrite the outcome of
        the phases, which are what the case is actually about.
        """
        seeded_before = worktree is not None and trust_store_lists(self.case.agent, worktree)
        try:
            for _ in range(len(PHASE_STATUS) + 1):
                task = self.mcp.call("get_task", task_id=task_id)
                if task.get("status") == "done":
                    break
                self.move_forward(task_id)
            else:
                return "task never reached done"
        except (McpError, TimeoutError, RuntimeError, OSError) as exc:
            return f"retire failed: {type(exc).__name__}: {exc}"

        # The prune only means something where agtx seeded in the first place.
        if seeded_before and trust_store_lists(self.case.agent, worktree):
            return (
                f"{self.case.agent} trust entry for the worktree survived Done — "
                "agent::trust::forget did not prune it"
            )
        return None

    def apply_park_expectation(self, res: CaseResult) -> CaseResult:
        """Re-label the antigravity-shaped outcome: delivered, then parked."""
        if self.case.agent not in EXPECTED_PARK_AGENTS:
            return res
        if res.outcome == PASS:
            res.reason = (
                "expectation is stale: this agent was expected to park on an "
                "unanswered dialog but completed cleanly — drop it from "
                "EXPECTED_PARK_AGENTS"
            )
            return res
        if res.outcome != FAIL or not res.phases:
            return res
        last = res.phases[-1]
        usable = next((c for c in last.checks if c.name == "usable"), None)
        others = [c for c in last.checks if c.name != "usable"]
        if usable and usable.state == "fail" and not any(c.state == "fail" for c in others):
            res.outcome = PARK
            res.reason = f"delivered, then parked — {usable.detail}"
        return res


# ---------------------------------------------------------------------------
# Reporting
# ---------------------------------------------------------------------------

OUTCOME_STYLE = {
    PASS: "PASS",
    FAIL: "FAIL",
    SKIP: "SKIP",
    QUOTA: "QUOTA",
    AUTH: "AUTH",
    PARK: "PARK",
    ERROR: "ERROR",
}


def report(results: list[CaseResult]) -> None:
    width_agent = max([len(r.agent) for r in results] + [5])
    width_plugin = max([len(r.plugin) for r in results] + [6])
    header = (
        f"{'agent':<{width_agent}}  {'plugin':<{width_plugin}}  "
        f"{'phase':<8}  {'checks':<11}  {'outcome':<6}  {'time':>7}"
    )
    print()
    print(header)
    print("-" * len(header))
    for r in results:
        checks = "".join(c.symbol for c in r.phases[-1].checks[:4]) if r.phases else "----"
        secs = f"{r.seconds:.0f}s" if r.seconds else "-"
        print(
            f"{r.agent:<{width_agent}}  {r.plugin:<{width_plugin}}  "
            f"{r.phase_reached:<8}  {checks:<11}  {OUTCOME_STYLE[r.outcome]:<6}  {secs:>7}"
        )
        if r.reason:
            print(f"    {r.reason}")
        if r.cleanup_note:
            # Separate from the outcome on purpose: the phases passed, but
            # finishing the task did not do what it should.
            print(f"    cleanup: {r.cleanup_note}")
    print()
    print("checks: 1 submitted  2 skill-ran  3 artifact  4 usable   "
          "(✓ pass  ✗ fail  ? unknown  – n/a)")

    counts: dict[str, int] = {}
    for r in results:
        counts[r.outcome] = counts.get(r.outcome, 0) + 1
    summary = "  ".join(f"{OUTCOME_STYLE[k]}={v}" for k, v in sorted(counts.items()))
    print(f"\n{summary}")

    skipped = [r for r in results if r.outcome == SKIP]
    if skipped:
        # Loud on purpose: a green run that quietly tested two of eight agents is
        # worse than no run at all.
        print(f"\n{len(skipped)} of {len(results)} cases were NOT tested:")
        for r in skipped:
            print(f"  - {r.agent}/{r.plugin}: {r.reason}")

    for r in results:
        if r.outcome in (PASS, SKIP):
            continue
        print(f"\n--- {r.agent}/{r.plugin} [{OUTCOME_STYLE[r.outcome]}] {r.reason}")
        for p in r.phases:
            details = "  ".join(
                f"{c.name}={c.symbol}" + (f" ({c.detail})" if c.detail else "")
                for c in p.checks
            )
            print(f"  {p.phase}: {details}")
        if r.phases and r.phases[-1].pane_tail:
            print("  pane tail:")
            for line in r.phases[-1].pane_tail.splitlines():
                print(f"    | {line}")
        if r.workdir:
            print(f"  scratch repo: {r.workdir}")


def to_json(results: list[CaseResult]) -> list[dict]:
    return [
        {
            "agent": r.agent,
            "plugin": r.plugin,
            "outcome": r.outcome,
            "reason": r.reason,
            "seconds": round(r.seconds, 1),
            "workdir": r.workdir,
            "phases": [
                {
                    "phase": p.phase,
                    "outcome": p.outcome,
                    "checks": [
                        {"name": c.name, "state": c.state, "detail": c.detail}
                        for c in p.checks
                    ],
                    "pane_tail": p.pane_tail,
                }
                for p in r.phases
            ],
        }
        for r in results
    ]


# ---------------------------------------------------------------------------
# main
# ---------------------------------------------------------------------------

def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    p.add_argument(
        "--agents",
        default="installed",
        help="comma-separated agent names, 'installed' (default), or 'all'",
    )
    p.add_argument(
        "--plugins",
        default="agtx",
        help="comma-separated plugin names (default: agtx), or 'all'",
    )
    p.add_argument(
        "--phases",
        default="planning,running",
        help="phases to drive, in order (default: planning,running)",
    )
    p.add_argument("--agtx-bin", default=str(REPO_ROOT / "target/release/agtx"))
    p.add_argument("--cargo", default="cargo")
    p.add_argument("--workdir", default=None, help="where scratch repos are created")
    p.add_argument("--phase-timeout", type=int, default=300)
    p.add_argument("--worktree-timeout", type=int, default=240)
    p.add_argument("--transition-timeout", type=int, default=180)
    p.add_argument("--keep-sessions", action="store_true", help="leave tmux sessions running")
    p.add_argument("--clean", action="store_true", help="delete scratch repos on success")
    p.add_argument(
        "--force-unsupported",
        action="store_true",
        help="run plugins that need project setup (init scripts, framework dirs)",
    )
    p.add_argument("--json", dest="json_out", default=None)
    p.add_argument("-v", "--verbose", action="store_true")
    opts = p.parse_args(argv)
    opts.phases = [p_ for p_ in opts.phases.split(",") if p_.strip()]
    bad = [p_ for p_ in opts.phases if p_ not in ORDERED_PHASES]
    if bad:
        p.error(f"unknown phase(s): {', '.join(bad)}")
    if opts.phases != [p_ for p_ in ORDERED_PHASES if p_ in opts.phases]:
        p.error("--phases must be in workflow order: planning,running,review")
    return opts


def main(argv: list[str] | None = None) -> int:
    opts = parse_args(argv)

    if not Path(opts.agtx_bin).exists():
        print(f"agtx binary not found: {opts.agtx_bin}", file=sys.stderr)
        print("build it first: cargo build --release", file=sys.stderr)
        return 2
    for tool in ("tmux", "git"):
        if not shutil.which(tool):
            print(f"required tool not found: {tool}", file=sys.stderr)
            return 2

    matrix = load_matrix(opts.cargo)
    cases = plan_cases(matrix, opts.agents, opts.plugins, opts.force_unsupported)
    if not cases:
        print("no cases to run", file=sys.stderr)
        return 2

    workdir = Path(opts.workdir) if opts.workdir else Path(
        tempfile.mkdtemp(prefix="agtx-smoke-")
    )
    workdir.mkdir(parents=True, exist_ok=True)
    runnable = [c for c in cases if not c.skip_reason]
    print(f"agtx smoke: {len(runnable)} case(s) to run, {len(cases) - len(runnable)} skipped")
    print(f"scratch: {workdir}")

    results: list[CaseResult] = []
    for case in cases:
        if case.skip_reason:
            results.append(CaseResult(case.agent, case.plugin, SKIP, case.skip_reason))
            continue
        print(f"→ {case.agent} / {case.plugin} …", flush=True)
        results.append(CaseRunner(case, matrix, opts, workdir).run())
        print(f"  {OUTCOME_STYLE[results[-1].outcome]}", flush=True)

    report(results)

    if opts.json_out:
        Path(opts.json_out).write_text(json.dumps(to_json(results), indent=2))
        print(f"\nwrote {opts.json_out}")

    failed = [r for r in results if r.outcome in (FAIL, ERROR)]
    if opts.clean and not failed:
        shutil.rmtree(workdir, ignore_errors=True)
    return 1 if failed else 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("\ninterrupted", file=sys.stderr)
        sys.exit(130)
