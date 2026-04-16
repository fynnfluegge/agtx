# AGENTS.md

This file provides guidance to AI coding agents (Claude Code, Codex, Gemini, Copilot, OpenCode, Cursor, etc.) when working with the agtx codebase.

## What is agtx?

A terminal-native kanban board for managing multiple coding agent sessions in parallel. Each task gets its own git worktree, tmux window, and agent session. Written in Rust.

## Build & Test

```bash
cargo build --release
cargo test --features test-mocks
```

## Key Architecture

```
src/
├── main.rs        # CLI entry point, AppMode routing
├── lib.rs         # Module exports
├── tui/app.rs     # Main TUI app (largest file — board, popups, key handling)
├── db/schema.rs   # SQLite operations (tasks, projects, notifications)
├── db/models.rs   # Task, Project, TaskStatus structs
├── mcp/server.rs  # MCP server (JSON-RPC over stdio)
├── tmux/          # Tmux session/window management
├── git/           # Worktree creation, conflict detection
├── agent/         # Agent detection, spawn commands
├── config/        # GlobalConfig, ProjectConfig, WorkflowPlugin
└── skills.rs      # Skill deployment to agent-native paths
```

See [CLAUDE.md](CLAUDE.md) for the full architecture reference including common task patterns, code conventions, and testing approach.

## Core Concepts

- **Tasks** move through: `Backlog → Planning → Running → Review → Done`
- **Each task** gets a git worktree + tmux window + agent session
- **Plugins** (TOML) customize the task lifecycle per phase
- **MCP server** (`agtx mcp-serve`) exposes the board as tools over stdio
- **Global MCP** (`agtx mcp-serve`, no args) serves all projects via `~/.config/agtx/index.db`
- **Project MCP** (`agtx mcp-serve <path>`) serves a single project (used by the orchestrator)

## Sweep Skill

If you want to sweep this conversation into agtx tasks, use the sweep skill:

```
@skills/agtx/sweep/SKILL.md
```

Follow the Sweep workflow in that file.

## Rules

- Edit `src/` for behavior changes — never edit generated or compiled artifacts
- Run `cargo test --features test-mocks` before considering any change done
- Database migrations use `ALTER TABLE ... ADD COLUMN` (errors ignored if column exists)
- Mock-dependent tests use `#[cfg(feature = "test-mocks")]`
- See [CLAUDE.md](CLAUDE.md) for adding new fields, agents, plugins, shortcuts
