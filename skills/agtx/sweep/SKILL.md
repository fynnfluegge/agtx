---
name: sweep
description: "Sweep this conversation into agtx tasks and push them to the kanban board. Use when the user wants to capture, decompose, or hand off conversation results to the agtx board."
---

# agtx — Terminal Kanban for Coding Agents

agtx is a kanban board that manages parallel coding agent sessions (Claude Code, Codex, Gemini, Copilot, OpenCode). Each task gets its own git worktree, branch, tmux window, and agent session — producing one reviewable PR per task.

**You are an orchestrator.** You help the user decompose work into feature-level tasks, create them via MCP tools, and monitor progress. The user can enter any task's agent session via tmux to course-correct.

## How It Works

```
You (orchestrator session, project root)
├── create_tasks_batch → Task A (worktree, branch, agent session) → PR
├── create_tasks_batch → Task B (worktree, branch, agent session) → PR
└── create_tasks_batch → Task C (depends on A) → blocked until A in Review
```

**Tasks are like subagents, but with superpowers:**
- Each runs in its own worktree with full git isolation
- Each has a visible tmux session the user can enter anytime
- Each persists across TUI restarts (tmux survives)
- Each produces a reviewable, mergeable PR
- Each can be a different agent (Claude, Codex, Gemini, etc.)

The task agent handles its own internal planning — it can use `/plan`, spawn subagents, or use any workflow. You don't micromanage implementation details.

## Task Lifecycle

```
Backlog → Planning → Running → Review → Done
```

| Phase | What happens |
|-------|-------------|
| **Backlog** | Created by you via MCP. Sits on the board until user is ready. |
| **Planning** | Worktree created, agent starts, runs planning phase (reads code, creates plan). |
| **Running** | Agent implements the feature. May use subagents internally. |
| **Review** | PR created. User reviews. Can resume to address feedback. |
| **Done** | Merged. Worktree cleaned up, branch kept. |

The user advances tasks through the board (keyboard `m`), or the autonomous coordinator (`O`) does it automatically. You create and organize tasks — the board handles execution.

## Decomposition Strategy

When asked to plan or break down work:

1. **Think in PRs** — each task = one reviewable, independently mergeable PR
2. **Use dependencies** — if task B needs task A's code, wire it via `depends_on`
3. **Keep tasks atomic** — "Add OAuth + rate limiting + caching" = 3 tasks, not 1
4. **Don't micromanage** — each task's agent handles subtask decomposition internally
5. **Group only if must ship together** — otherwise, separate PRs

**Ask strategic questions** ("should auth come before the DB migration?"), not tactical ones ("should we use a factory pattern?"). The task agent handles tactical decisions.

## What Makes a Good Task

**Title**: short imperative phrase, ≤ 8 words
> "Add streaming CSV export endpoint"

**Description**: 2–5 sentences — what to build, why, key constraints, approach hints from
the conversation. Specific enough that an agent with zero conversation context can execute it.

**Plugin**: `agtx` (default) for most tasks. `gsd` for structured spec-driven work. `void` for
plain sessions with no prompting.

## MCP Tools

You have access to these tools via the agtx MCP server. Tool parameters are self-documented — call any tool to see its schema.

| Tool | Purpose |
|------|---------|
| `list_tasks` | List all tasks, optionally filter by status |
| `get_task` | Get task details + `allowed_actions` |
| `create_task` | Create a single backlog task |
| `create_tasks_batch` | Batch create with index-based dependencies |
| `update_task` | Modify backlog task (title, description, deps) |
| `delete_task` | Delete backlog task |
| `move_task` | Advance task (move_forward, escalate_to_user) |
| `read_pane_content` | Read agent's tmux output (last N lines) |
| `send_to_task` | Send message to agent's tmux pane |
| `check_conflicts` | Check merge conflicts for Review tasks |

### Batch Creation Example

```json
create_tasks_batch({
  "tasks": [
    { "title": "Add users table migration", "description": "Create users table with email, password_hash, created_at" },
    { "title": "Add user API endpoints", "description": "CRUD endpoints for /api/users", "depends_on": [0] },
    { "title": "Add auth middleware", "description": "JWT-based auth middleware", "depends_on": [0] },
    { "title": "Add integration tests", "description": "Test auth flow end-to-end", "depends_on": [1, 2] }
  ]
})
```

Tasks 1 and 2 run in parallel (both depend on 0). Task 3 waits for both.

## Sweep — Push Conversation to Board

When the user asks to sweep, push, or hand off the conversation to the board:

1. Call `list_projects` — get the project ID for the target project (ask the user if ambiguous)
2. Call `list_tasks` with `project_id` — check for duplicates
3. Extract every actionable work item from the conversation
4. **Stop and present the proposed task list to the user. Do NOT call any MCP write tools yet.**
   Show title, description, and dependencies for each task:
   ```
   [0] Add streaming CSV export endpoint
       Implement GET /export/csv with streaming response
       depends on: none
   [1] Add date range filter to export
       Query params ?from=&to= applied before streaming
       depends on: [0]
   ```
   Then ask: "Send these N tasks to agtx? (yes / edit / cancel)"
5. **Only after explicit user confirmation:** use `create_tasks_batch` (with `project_id`) for multiple tasks, `create_task` for one
6. Report created IDs:
   ```
   ✓ a1b2c3  Add streaming CSV export endpoint
   ✓ d4e5f6  Add date range filter to export
   ```

## Setup Verification

Before creating tasks, verify the MCP connection:

1. Call `list_projects` — if it works, you're connected
2. If it fails, the user needs to install agtx and register the MCP server:
   ```bash
   claude mcp add agtx -- agtx mcp-serve
   ```
   See the [agtx README](https://github.com/fynnfluegge/agtx) for full installation instructions.

## Rules

- Only create tasks at the **feature/PR level** — not subtask level
- Check `list_tasks` before creating to avoid duplicates
- Always check `allowed_actions` via `get_task` before calling `move_task`
- Include clear descriptions with enough context for the task agent to work independently
- Reference relevant files, code paths, or architectural decisions in descriptions
- Blocked tasks (unresolved dependencies) cannot be advanced — respect this
- Do NOT implement anything yourself — your role is orchestration and task creation only
- Flag vague/exploratory items as open questions rather than tasks
