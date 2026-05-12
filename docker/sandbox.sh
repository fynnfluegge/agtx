#!/bin/bash
set -e

# agtx Docker sandbox
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

# Checks
if ! command -v docker &>/dev/null; then
    error "docker not found — install Docker Desktop (macOS/Windows) or Docker Engine (Linux)"
fi

RAW_PROJECT="${1:-$(pwd)}"

if [ ! -d "$RAW_PROJECT" ]; then
    error "Directory not found: $RAW_PROJECT"
fi

PROJECT="$(resolve_path "$RAW_PROJECT")"

echo ""
echo "  ╭──────────────────────────────────────────╮"
echo "  │           agtx docker sandbox            │"
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
docker build -q \
    --build-arg UID="$(id -u)" \
    --build-arg GID="$(id -g)" \
    -t agtx-sandbox \
    "$DOCKER_DIR"

success "Image ready"
echo ""

# agtx state lives in named volumes — isolated from the host, persists across runs
exec docker run --rm -it \
    --security-opt no-new-privileges:true \
    --cap-drop ALL \
    --cap-add CHOWN \
    --cap-add DAC_OVERRIDE \
    --cap-add SETUID \
    --cap-add SETGID \
    -v "${PROJECT}:/home/sandbox/workspace" \
    -v agtx-data:/home/sandbox/.local/share/agtx \
    -v agtx-config:/home/sandbox/.config/agtx \
    -v "${HOME}/.claude:/claude-host:ro" \
    -w /home/sandbox/workspace \
    agtx-sandbox \
    agtx /home/sandbox/workspace
