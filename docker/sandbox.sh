#!/bin/bash
set -e

# agtx sandbox
# Usage: ./docker/sandbox.sh [path/to/project]  (defaults to current directory)

DOCKER_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

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

# Rootless podman remaps UIDs; keep-id preserves host ownership on bind mounts
USERNS=""; [ "$CR" = "podman" ] && [ "$(id -u)" -ne 0 ] && USERNS="--userns=keep-id"

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

if [ ! -d "$PROJECT/.git" ]; then
    warn "$PROJECT is not a git repository"
    read -rp "  Continue anyway? [y/N] " ans
    [[ "$ans" =~ ^[Yy]$ ]] || exit 0
    echo ""
fi

# Build image with host UID/GID so files created in the container are owned correctly
info "Building image..."
"$CR" build -q \
    --build-arg UID="$(id -u)" \
    --build-arg GID="$(id -g)" \
    -t agtx-sandbox \
    "$DOCKER_DIR"

success "Image ready"
echo ""

# `~/.claude.json` carries onboarding state (theme choice, `hasCompletedOnboarding`)
# and the per-directory trust map — none of which live in `~/.claude`. Without it a
# fresh sandbox opens on Claude's theme picker and never reaches the prompt. The
# benchmark has always copied it; the sandbox did not.
CLAUDE_JSON_MOUNT=""
if [ -f "${HOME}/.claude.json" ]; then
    CLAUDE_JSON_MOUNT="-v ${HOME}/.claude.json:/claude-host.json:ro"
fi

# The agtx-data volume holds the sandbox's state (trust store, project DBs)
# across runs. Host settings are staged read-only below and copied in at
# startup, so the volume never sees your host's state files.
AGTX_CONFIG_MOUNT="-v agtx-data:/home/sandbox/.config/agtx"
[ -f "${HOME}/.config/agtx/config.toml" ] && AGTX_CONFIG_MOUNT="$AGTX_CONFIG_MOUNT -v ${HOME}/.config/agtx/config.toml:/agtx-host-config.toml:ro"
[ -d "${HOME}/.config/agtx/plugins" ] && AGTX_CONFIG_MOUNT="$AGTX_CONFIG_MOUNT -v ${HOME}/.config/agtx/plugins:/agtx-host-plugins:ro"

# On macOS the Claude OAuth token lives in the Keychain, not in `~/.claude`, so
# copying that directory leaves the container on "Not logged in · Please run
# /login". Materialise the same `.credentials.json` Claude Code writes on Linux.
#
# The secret is piped through stdin: never written to a host temp file, and never
# placed in argv where `ps` would expose it. Same approach the benchmark uses.
CLAUDE_CREDS=""
if [ "$(uname -s)" = "Darwin" ] && [ ! -f "${HOME}/.claude/.credentials.json" ]; then
    CLAUDE_CREDS="$(security find-generic-password -s 'Claude Code-credentials' -w 2>/dev/null || true)"
    if [ -z "$CLAUDE_CREDS" ]; then
        info "No Claude credentials found — the Keychain lookup failed (you may need"
        info "to allow access when prompted). Set ANTHROPIC_API_KEY, or the agent"
        info "inside the sandbox will report 'Not logged in'."
    fi
fi

# Started detached so the credential can be planted before any agent launches,
# then attached so the TUI behaves exactly as before. `--rm` still cleans up on
# exit, and `attach` returns the container's exit code.
CID=$("$CR" run -d -it --rm \
    $USERNS \
    --security-opt no-new-privileges:true \
    --cap-drop ALL \
    --cap-add CHOWN \
    --cap-add DAC_OVERRIDE \
    --cap-add SETUID \
    --cap-add SETGID \
    -v "${PROJECT}:/home/sandbox/workspace" \
    ${AGTX_CONFIG_MOUNT} \
    -v "${HOME}/.claude:/claude-host:ro" \
    ${CLAUDE_JSON_MOUNT} \
    -w /home/sandbox/workspace \
    agtx-sandbox \
    agtx /home/sandbox/workspace)

if [ -n "$CLAUDE_CREDS" ]; then
    printf '%s' "$CLAUDE_CREDS" | "$CR" exec -i "$CID" \
        /bin/bash -c 'umask 077 && cat > /home/sandbox/.claude/.credentials.json'
    unset CLAUDE_CREDS
fi

exec "$CR" attach "$CID"
