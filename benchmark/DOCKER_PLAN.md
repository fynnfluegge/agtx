# Plan: Run benchmark inside SWE-bench Docker containers

## Context

The benchmark currently clones repos onto the host and runs agents there. The repos are not
installable on the host (wrong Python version, missing C extensions, etc.), so agents fail
when they try to run tests. The SWE-bench project provides pre-built Docker images
(`swebench/sweb.eval.x86_64.{instance_id}:latest`) with the repo pre-installed at the correct
commit in a working conda environment (`testbed`). Running the agent inside these containers
gives it a fully working `pytest` and all dependencies.

## Approach

Add a `--docker` flag to `benchmark.py`. When set, the infrastructure layer is swapped out:
host-side `setup_repo` / `start_tui_in_tmux` / `McpClient` / `kill_tmux_session` are replaced
with Docker equivalents. The MCP phase logic and result collection stay unchanged.

### Architecture

```
benchmark.py (--docker)
  ├── pull image: swebench/sweb.eval.x86_64.{instance_1776_id}:latest
  ├── start container (long-running, --platform linux/amd64 for Rosetta on Apple Silicon)
  │     volumes: agtx_bin, agtx DB dir, agent credentials
  ├── exec: apt-get install tmux + node + npm + claude (once per container)
  ├── exec: agtx TUI inside container's tmux session
  ├── McpClient: spawns "docker exec <cid> agtx mcp-serve /testbed" over stdio
  ├── agent works in /testbed (conda testbed env, tests work)
  ├── patch: "docker exec <cid> git diff <base_commit> -- . :!.agtx/" in /testbed
  └── stop + remove container
```

## File to modify

**`benchmark/swebench/benchmark.py`** — only file changed.

## Implementation steps

### 1. Add Docker helper functions (new, after `kill_tmux_session`)

**`docker_image_name(instance_id) -> str`**
- Converts `astropy__astropy-12907` → `swebench/sweb.eval.x86_64.astropy_1776_astropy-12907:latest`
- Rule: replace `__` with `_1776_`

**`start_docker_container(instance_id, slug, agtx_bin, verbose) -> str`**
- Pulls image (progress if verbose)
- `docker run -d --platform linux/amd64 \`
  `-v {agtx_bin}:/usr/local/bin/agtx \`
  `-v {agtx_db_dir}:{agtx_db_dir} \`   ← ~/Library/Application Support/agtx on macOS
  `-v ~/.config/claude:~/.config/claude \`
  `-v ~/.claude:~/.claude \`
  `--name swebench-{slug} {image} sleep infinity`
- Returns container ID

**`setup_container(container_id, config_path, base_commit, verbose)`**
- Writes `.agtx/config.toml` into container at `/testbed/.agtx/config.toml`
- Installs tmux + Node.js + Claude Code inside container:
  `apt-get install -y tmux nodejs npm && npm install -g @anthropic-ai/claude-code`
- Cleans stale agtx state (`.agtx/worktrees`, project DB) same as current `_write_agtx_config`

**`start_tui_in_container(slug, container_id, verbose)`**
- `docker exec -d {container_id} tmux new-session -d -s {slug} "agtx /testbed"`
- Replaces `start_tui_in_tmux()`

**`stop_docker_container(container_id, verbose)`**
- `docker stop {container_id} && docker rm {container_id}`
- Replaces `kill_tmux_session()`

### 2. Docker-aware McpClient

Add `container_id: str | None = None` parameter to `McpClient.__init__`.

When `container_id` is set, spawn MCP server as:
```
docker exec -i {container_id} /bin/bash -c \
  "source /opt/miniconda3/bin/activate && conda activate testbed && agtx mcp-serve /testbed"
```
instead of `[agtx_bin, "mcp-serve", repo_path]`.

### 3. Docker-aware patch collection

In `TaskRunner._collect_patch()`, add `container_id` path:
```python
if self.container_id:
    result = subprocess.run(
        ["docker", "exec", self.container_id,
         "git", "-C", "/testbed", "diff", base_commit,
         "--", ".", ":!.agtx/"],
        ...
    )
```

### 4. Wire `--docker` through orchestrator

- Add `docker: bool = False` to `TaskRunner.__init__` and `BenchmarkOrchestrator.__init__`
- In `BenchmarkOrchestrator._run_one()`:
  - If `--docker`: call `start_docker_container()` + `setup_container()` + `start_tui_in_container()`
  - Else: existing `setup_repo()` + `start_tui_in_tmux()`
  - Pass `container_id` to `TaskRunner`
  - Cleanup: `stop_docker_container()` vs `kill_tmux_session()`
- In `TaskRunner.run()`: pass `container_id` to `McpClient` and `_collect_patch()`

### 5. argparse

```
--docker    Run each task inside its SWE-bench Docker image
            (swebench/sweb.eval.x86_64.*). Gives agents a working test
            environment. Requires Docker with Rosetta on Apple Silicon.
```

### 6. README update

Add Docker section under Running with prerequisites and example command.

## Volumes mounted into container

| Host path | Container path | Purpose |
|-----------|---------------|---------|
| `{agtx_bin}` | `/usr/local/bin/agtx` | agtx binary |
| `~/Library/Application Support/agtx/` | same path | agtx DB (project index + task DBs) |
| `~/.config/claude/` | same path | Claude credentials |
| `~/.claude/` | same path | Claude session data |

DB path must be identical inside and outside the container so the MCP server and TUI
(both running inside) find the same SQLite files.

## Notes

- `--platform linux/amd64` enables Rosetta on Apple Silicon — no ARM build needed
- Agent install runs once per container; containers are fresh per run so it always runs
- `sleep infinity` keeps the container alive while agtx TUI + agent work
- Container named `swebench-{slug}` enables manual cleanup if benchmark crashes:
  `docker rm -f swebench-{slug}`
- Existing non-docker path fully preserved — `--docker` is purely additive
- Future optimisation: `--docker-base-image` to use a pre-baked image with agent pre-installed

## Verification

```bash
# 1. Single instance smoke test
python benchmark.py \
  --config configs/claude-agtx.toml \
  --instance-ids astropy__astropy-12907 \
  --docker --verbose \
  --agtx ../../target/release/agtx

# 2. Check container is running
docker ps | grep swebench

# 3. Attach to agent session inside container
docker exec -it swebench-astropy__astropy-12907 tmux attach

# 4. Confirm tests run without errors
# 5. Check results.json has patch and status=success
# 6. Run report.py to confirm output
```
