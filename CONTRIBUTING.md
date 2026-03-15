# Contributing

Thanks for your interest in contributing to agtx! This guide will help you get started.

New here? Issues labeled [`good first issue`](https://github.com/fynnfluegge/agtx/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) are a great place to start — they're scoped, well-described, and a perfect way to get familiar with the codebase.

## Getting Started

### Prerequisites

- **Rust** 1.70+ ([install](https://rustup.rs))
- **tmux** (runtime dependency for agent sessions)
- **git** (runtime dependency for worktrees)
- **gh** CLI (optional, for PR operations)

### Setup

```bash
git clone https://github.com/fynnfluegge/agtx
cd agtx
cargo build
cargo test --features test-mocks
```

### Running Locally

```bash
# Run in any git repository
cd agtx && cargo run --manifest-path /path/to/agtx/Cargo.toml

# Or build and copy to PATH
cargo build --release
cp target/release/agtx ~/.local/bin/
```

## Ways to Contribute

### Bug Reports

Open an [issue](https://github.com/fynnfluegge/agtx/issues) with:
- What you expected vs. what happened
- Steps to reproduce
- OS, terminal emulator, and agent CLI versions

### Write a Plugin

Plugins are the easiest way to contribute. A single `plugin.toml` defines an entire workflow. See the [plugin reference](README.md#plugins) in the README.

Place your plugin in `plugins/<name>/plugin.toml` and open a PR. Check the existing plugins for examples:
- `plugins/void/plugin.toml` — simplest possible plugin
- `plugins/agtx/plugin.toml` — full-featured with skills and prompts
- `plugins/gsd/plugin.toml` — cyclic workflow with preresearch

### Add Agent Support

To add a new AI coding CLI:

1. Add to `known_agents()` in `src/agent/mod.rs`
2. Add `build_interactive_command()` match arm in `src/agent/mod.rs`
3. Add agent-native skill directory in `agent_native_skill_dir()` in `src/skills.rs`
4. Add plugin command transform in `transform_plugin_command()` in `src/skills.rs`

### Code Changes

1. Fork the repository
2. Create a feature branch: `git checkout -b my-feature`
3. Make your changes
4. Run tests: `cargo test --features test-mocks`
5. Open a pull request against `main`

## Architecture Overview

See [CLAUDE.md](CLAUDE.md) for the full architecture documentation, including:
- Module structure and key files
- Task workflow lifecycle
- Plugin system internals
- Agent integration patterns
- Testing patterns (mock traits, feature flags)

### Key Directories

```
src/tui/     — TUI rendering and input handling
src/db/      — SQLite database (tasks, projects)
src/tmux/    — Tmux server management
src/git/     — Git worktree operations
src/agent/   — Agent detection, spawning, switching
src/mcp/     — MCP server for orchestrator
src/config/  — Configuration and plugin loading
plugins/     — Bundled workflow plugins
skills/      — Built-in skill files (embedded at compile time)
```

### Testing

```bash
# Run all tests
cargo test --features test-mocks

# Run a specific test
cargo test --features test-mocks test_name
```

Tests use mock traits (`TmuxOperations`, `GitOperations`, `AgentOperations`) behind the `test-mocks` feature flag. Pure function tests don't need the feature flag.

## Code Style

- Follow existing patterns in the codebase
- Use `anyhow::Result` for fallible functions with `.context()` for error messages
- Keep UI state in `AppState`, drawing functions are static
- Add tests for new functionality

## License

By contributing, you agree that your contributions will be licensed under the [Apache License 2.0](LICENSE).
