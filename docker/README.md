# agtx container

Run agtx in a container (Docker or Podman, including rootless Podman). One
launcher — `sandbox.sh` — with two modes, and the container itself defined in
`compose.yaml` / `compose.runtime.yaml`.

> All commands below assume you are in the repository root.

## Sandbox (default)

Isolated: agtx's state lives in a named volume and your host settings are staged
read-only, so nothing the agent does can reach the host.

```bash
# On a project — or from inside the project directory
./docker/sandbox.sh path/to/your-project
```

## Runtime (no sandbox)

Not a jail — a way to *run* agtx. Your real `~/.config/agtx`, `~/.claude`, and
`~/.claude.json` are mounted **writable**, so agtx writes config, status, and
updates straight to the host, like a local install. Same least-privilege caps.

```bash
# On a project, writing config/state back to the host
AGTX_RUNTIME=1 ./docker/sandbox.sh path/to/your-project
```

## Details

- **Runtime:** docker when installed, else podman. Set `AGTX_CONTAINER_RUNTIME=docker|podman` to force one. The image builds on first run, then is cached.
- **Launch a definition directly** (skip `sandbox.sh`):
  ```bash
  PROJECT=/path/to/project docker compose -f docker/compose.yaml up            # sandbox
  PROJECT=/path/to/project docker compose -f docker/compose.runtime.yaml up    # runtime
  ```
- **Files:** `compose.yaml` (sandbox) + `compose.<asset>.yaml` fragments (optional host-asset mounts, Podman `keep-id`); `compose.runtime.yaml` (runtime); `sandbox.sh` (launcher); `entrypoint.sh` (startup config); `Dockerfile`.
