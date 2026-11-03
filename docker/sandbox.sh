#!/bin/bash
set -e

# agtx sandbox
#
# Launches the container defined in docker/compose*.yaml (in this directory) via
# the Compose tool for the available container runtime — docker when present,
# else podman (override with AGTX_CONTAINER_RUNTIME). Two modes share the
# launcher:
#
#   sandbox (default)  compose.yaml — agtx's state is isolated in a named volume
#                    and your host settings are staged read-only; nothing the
#                    agent does can reach the host.
#   runtime (AGTX_RUNTIME=1)  compose.runtime.yaml — your host's real config and
#                    state are mounted writable, so agtx writes config/status/
#                    updates straight to the host. The in-container equivalent of
#                    installing agtx: another way to run it, not a jail.
#
# The container definition lives in the compose file; this script only does what
# cannot be a compose value:
#
#   * pick the runtime (docker / podman),
#   * pick the mode (sandbox vs runtime) and its base compose file,
#   * select the optional compose.<asset>.yaml fragments whose host assets exist,
#   * plant the macOS Keychain credential (Darwin only) before the agent starts,
#   * attach the caller's terminal — compose `up` attaches logs, not stdin,
#   * capture the container's exit code and tear the container down.
#
# Usage:
#   ./docker/sandbox.sh [path/to/project]            # sandbox mode (default)
#   AGTX_RUNTIME=1 ./docker/sandbox.sh [path/to/project]   # runtime mode
#   (defaults to current directory)

# Remember where the script was invoked: the project argument (and the default
# when none is given) is relative to the caller, not to this script's directory.
CALLER_PWD="$(pwd)"
DOCKER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$DOCKER_DIR"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m'

info()    { echo -e "${BLUE}==>${NC} $1"; }
success() { echo -e "${GREEN}==>${NC} $1"; }
warn()    { echo -e "${YELLOW}==>${NC} $1"; }
error()   { echo -e "${RED}==>${NC} $1"; exit 1; }

# Resolve project path portably (realpath not available on all macOS versions)
resolve_path() {
    cd "$1" && pwd -P
}

# Container runtime: docker when installed, else podman (AGTX_CONTAINER_RUNTIME overrides)
CR="${AGTX_CONTAINER_RUNTIME:-docker}"
command -v "$CR" &>/dev/null || CR=podman
command -v "$CR" &>/dev/null || error "no container runtime found — install docker or podman"

# Mode + the base compose file it uses. AGTX_RUNTIME=1 selects runtime mode
# (host config/state mounted writable); anything else is sandbox mode (default).
if [ "${AGTX_RUNTIME:-0}" = "1" ]; then
    MODE=runtime; BASE=compose.runtime.yaml
else
    MODE=sandbox; BASE=compose.yaml
fi

RAW_PROJECT="${1:-$CALLER_PWD}"

# We are already in docker/ for compose, so resolve a relative project path
# against the directory the caller invoked us from.
case "$RAW_PROJECT" in
    /*) ;;
    *)  RAW_PROJECT="$CALLER_PWD/$RAW_PROJECT" ;;
esac

if [ ! -d "$RAW_PROJECT" ]; then
    error "Directory not found: $RAW_PROJECT"
fi

PROJECT="$(resolve_path "$RAW_PROJECT")"

echo ""
echo "  ╭──────────────────────────────────────────╮"
echo "  │               agtx sandbox               │"
echo "  ╰──────────────────────────────────────────╯"
echo ""

info "Project : $PROJECT"
info "Mode    : $MODE"
info "User    : sandbox (non-root, uid=$(id -u))"
info "Runtime : $CR (compose)"

if [ ! -d "$PROJECT/.git" ]; then
    warn "$PROJECT is not a git repository"
    read -rp "  Continue anyway? [y/N] " ans
    [[ "$ans" =~ ^[Yy]$ ]] || exit 0
    echo ""
fi

# Expose the host identity to compose: the image build uses the UID/GID, and the
# base + fragment mounts reference the resolved project and home paths.
export PROJECT
export AGTX_BUILD_UID="$(id -u)"
export AGTX_BUILD_GID="$(id -g)"

# Build image with host UID/GID so files created in the container are owned correctly
info "Building image..."
"$CR" compose -f "$BASE" build
success "Image ready"
echo ""

# Select the base file and the fragments that apply.
#   sandbox : the host-asset fragments, each included only when the asset it
#             mounts exists, so an absent path is never bind-mounted (which
#             would create a root-owned directory on the host).
#   runtime : your real config/state are already mounted writable in the base
#             file, so there are no staging assets to select.
if [ "$MODE" = runtime ]; then
    F="-f compose.runtime.yaml"
else
    F="-f compose.yaml"
    [ -f  "${HOME}/.claude.json" ]            && F="$F -f compose.claude-json.yaml"
    [ -f  "${HOME}/.config/agtx/config.toml" ] && F="$F -f compose.agtx-config.yaml"
    [ -d  "${HOME}/.config/agtx/plugins" ]     && F="$F -f compose.agtx-plugins.yaml"
fi
# Rootless Podman remaps UIDs; keep-id preserves host ownership on bind mounts.
[ "$CR" = "podman" ] && [ "$(id -u)" -ne 0 ] && F="$F -f compose.keepid.yaml"

# On macOS the Claude OAuth token lives in the Keychain, not in `~/.claude`, so
# copying that directory leaves the container on "Not logged in · Please run
# /login". Materialise the same `.credentials.json` Claude Code writes on Linux.
#
# The secret is piped through stdin: never written to a host temp file, and never
# placed in argv where `ps` would expose it. It is planted after the container is
# up (detached) but before we attach, so no agent starts before it lands.
#
# This needs `compose attach`, which Docker Compose has but podman-compose does
# not — and the Keychain case is Darwin-only, where the runtime is Docker. On
# Podman (Linux) there is no Keychain credential to plant, so it never reaches
# this branch.
CLAUDE_CREDS=""
if [ "$(uname -s)" = "Darwin" ] && [ ! -f "${HOME}/.claude/.credentials.json" ]; then
    CLAUDE_CREDS="$(security find-generic-password -s 'Claude Code-credentials' -w 2>/dev/null || true)"
    if [ -z "$CLAUDE_CREDS" ]; then
        info "No Claude credentials found — the Keychain lookup failed (you may need"
        info "to allow access when prompted). Set ANTHROPIC_API_KEY, or the agent"
        info "inside the sandbox will report 'Not logged in'."
    fi
fi

# Best-effort: drop any stale container from a previous run so the fixed
# container_name never conflicts. In sandbox mode this keeps the named state
# volume; in runtime mode there is no volume (state lives on the host).
"$CR" compose $F down >/dev/null 2>&1 || true

# Start detached, then attach this terminal. `compose up` in the foreground
# attaches stdout/logs but not stdin, and the agtx TUI needs both — plain `up`
# leaves it running with no keyboard input. Detaching first also lets the macOS
# Keychain credential land before any agent starts.
#
# A non-zero code means agtx crashed, so failures must not trip `set -e` —
# capture them explicitly.
EXIT=0
"$CR" compose $F up -d || EXIT=$?

if [ "$EXIT" -eq 0 ] && [ -n "$CLAUDE_CREDS" ] && [ "$CR" = "docker" ]; then
    printf '%s' "$CLAUDE_CREDS" | "$CR" compose $F exec -T sandbox \
        /bin/bash -c 'umask 077 && cat > /home/sandbox/.claude/.credentials.json'
    unset CLAUDE_CREDS
fi

ATTACHED=0
if [ "$EXIT" -eq 0 ]; then
    if [ ! -t 0 ]; then
        warn "stdin is not a TTY — the agtx TUI needs an interactive terminal"
    fi
    if [ "$CR" = "docker" ]; then
        "$CR" compose $F attach sandbox || ATTACHED=$?
    else
        # podman-compose has no `attach`; the fixed container_name lets the
        # runtime-native command do it.
        "$CR" attach agtx-sandbox || ATTACHED=$?
    fi

    # Prefer the container's own exit status: attach can fail because the
    # container already stopped, which says nothing about why agtx exited. If
    # the container is still running (e.g. the user detached), keep attach's
    # status instead of the running container's placeholder 0.
    container_code=$("$CR" inspect --format '{{.State.ExitCode}}' agtx-sandbox 2>/dev/null || true)
    if [ -n "$container_code" ]; then
        container_running=$("$CR" inspect --format '{{.State.Running}}' agtx-sandbox 2>/dev/null || true)
        if [ "$container_running" = "true" ]; then
            EXIT=$ATTACHED
        else
            EXIT=$container_code
        fi
    else
        EXIT=$ATTACHED
    fi
fi

"$CR" compose $F down >/dev/null 2>&1 || true
exit "$EXIT"
