#!/usr/bin/env python3
"""
Build the shared agtx/swebench-tools:latest image with tmux, Node.js, and Claude Code.

This single image is used by all benchmark sandbox runs — the benchmark runner copies
the installed binaries from a running tools container into each SWE-bench instance
container at startup, saving ~3-5 min per instance compared to installing from scratch.

The tools image is based on Ubuntu 22.04 (matching SWE-bench images) and contains only
the agent tooling layer. SWE-bench instance images are used as-is from Docker Hub.

Usage:
    # Build the tools image (run once, or after updating Claude Code)
    python prebake_images.py

    # Force rebuild even if image already exists
    python prebake_images.py --force

    # Show docker build output
    python prebake_images.py --verbose
"""

from __future__ import annotations

import argparse
import subprocess
import sys

TOOLS_IMAGE = "agtx/swebench-tools:latest"

DOCKERFILE = """\
FROM --platform=linux/amd64 ubuntu:22.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update -qq \\
    && apt-get install -y -qq tmux curl ca-certificates \\
    && curl -fsSL https://deb.nodesource.com/setup_22.x | bash - \\
    && apt-get install -y -qq nodejs \\
    && npm install -g @anthropic-ai/claude-code \\
    && apt-get clean \\
    && rm -rf /var/lib/apt/lists/*
"""


def tools_image_exists() -> bool:
    result = subprocess.run(
        ["docker", "image", "inspect", TOOLS_IMAGE],
        capture_output=True,
    )
    return result.returncode == 0


def build_tools_image(force: bool = False, verbose: bool = False) -> None:
    if not force and tools_image_exists():
        print(f"Tools image already exists: {TOOLS_IMAGE}")
        print("Use --force to rebuild.")
        return

    print(f"Building {TOOLS_IMAGE}...")
    capture = not verbose
    subprocess.run(
        ["docker", "build", "--platform", "linux/amd64", "-t", TOOLS_IMAGE, "-"],
        input=DOCKERFILE,
        text=True,
        capture_output=capture,
        check=True,
    )
    print(f"Done: {TOOLS_IMAGE}")


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Build the shared agtx/swebench-tools image for sandbox benchmark runs.",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog=__doc__,
    )
    parser.add_argument(
        "--force",
        action="store_true",
        help="Rebuild even if the tools image already exists",
    )
    parser.add_argument(
        "--verbose", "-v",
        action="store_true",
        help="Show docker build output",
    )
    args = parser.parse_args()
    build_tools_image(force=args.force, verbose=args.verbose)


if __name__ == "__main__":
    main()
