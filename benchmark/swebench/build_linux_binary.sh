#!/usr/bin/env bash
# Build the agtx Linux x86_64 binary inside a Ubuntu 22.04 container.
# Output: ../../target/agtx-linux-x86_64
#
# Must be run from the repo root or benchmark/swebench/ directory.
# Requires Docker.

set -euo pipefail

# Resolve repo root (two levels up from this script)
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUTPUT="$REPO_ROOT/target/agtx-linux-x86_64"

echo "Building agtx Linux x86_64 binary..."
echo "Repo root: $REPO_ROOT"
echo "Output:    $OUTPUT"

# Clean up any stale builder container
docker rm -f agtx-builder 2>/dev/null || true

# Start Ubuntu 22.04 builder (matches glibc 2.35 of SWE-bench images)
docker run -d --name agtx-builder ubuntu:22.04 sleep infinity

# Install build dependencies
docker exec agtx-builder apt-get update -qq
docker exec agtx-builder apt-get install -y curl gcc pkg-config libssl-dev

# Install Rust
docker exec agtx-builder bash -c "curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal"

# Copy source into container
docker exec agtx-builder mkdir -p /build
docker cp "$REPO_ROOT/src"        agtx-builder:/build/src
docker cp "$REPO_ROOT/Cargo.toml" agtx-builder:/build/Cargo.toml
docker cp "$REPO_ROOT/Cargo.lock" agtx-builder:/build/Cargo.lock
docker cp "$REPO_ROOT/plugins"    agtx-builder:/build/plugins
docker cp "$REPO_ROOT/skills"     agtx-builder:/build/skills

# Build
docker exec agtx-builder bash -c "source /root/.cargo/env && cd /build && cargo build --release"

# Copy binary out
mkdir -p "$REPO_ROOT/target"
docker cp agtx-builder:/build/target/release/agtx "$OUTPUT"

# Clean up
docker rm -f agtx-builder

echo ""
echo "Done: $OUTPUT"
file "$OUTPUT"
