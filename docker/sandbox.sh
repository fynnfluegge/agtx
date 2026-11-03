#!/bin/bash
set -e

# agtx sandbox
#
# Launches the container defined in compose.yaml (in this directory) via the
# Compose tool for the available container runtime — docker when present, else
# podman (override with AGTX_CONTAINER_RUNTIME). The container definition lives
# in compose.yaml; this script only does what cannot be a compose value:
#
#   * pick the runtime (docker / podman),
#   * select the optional compose.<asset>.yaml fragments whose host assets exist,
#   * plant the macOS Keychain credential (Darwin only) before the agent starts,
#   * capture the container's exit code and tear the container down.
#
# Usage: ./docker/sandbox.sh [path/to/project]  (defaults to current directory)

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

RAW_PROJECT="${1:-$(pwd)}"

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
"$CR" compose -f compose.yaml build
success "Image ready"
echo ""

# Select the optional fragments. Each is included only when the host asset it
# mounts exists, so an absent path is never bind-mounted (which would create a
# root-owned directory on the host).
F="-f compose.yaml"
[ -f  "${HOME}/.claude.json" ]            && F="$F -f compose.claude-json.yaml"
[ -f  "${HOME}/.config/agtx/config.toml" ] && F="$F -f compose.agtx-config.yaml"
[ -d  "${HOME}/.config/agtx/plugins" ]     && F="$F -f compose.agtx-plugins.yaml"
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

# Best-effort: drop any stale container from a previous run (keeps the named
# volume, so state persists) so the fixed container_name never conflicts.
"$CR" compose $F down >/dev/null 2>&1 || true

# Launch and capture the container's exit code. A non-zero code means agtx
# crashed, so it must not trip `set -e` — capture it explicitly.
EXIT=0
if [ -n "$CLAUDE_CREDS" ] && [ "$CR" = "docker" ]; then
    "$CR" compose $F up -d
    printf '%s' "$CLAUDE_CREDS" | "$CR" compose $F exec -T sandbox \
        /bin/bash -c 'umask 077 && cat > /home/sandbox/.claude/.credentials.json'
    unset CLAUDE_CREDS
    "$CR" compose $F attach sandbox || EXIT=$?
else
    "$CR" compose $F up || EXIT=$?
    # podman-compose reports 0 from `up`; recover the real exit code.
    [ "$CR" = "podman" ] && EXIT=$("$CR" inspect --format '{{.State.ExitCode}}' agtx-sandbox 2>/dev/null || echo "$EXIT")
fi

"$CR" compose $F down >/dev/null 2>&1 || true
exit "$EXIT"
