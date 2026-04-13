---
name: sweep
description: >
  agtx kanban board integration. Sweeps conversations into executable tasks and pushes
  them to the agtx board via MCP. Auto-activates when user mentions "agtx", "push to board",
  "create tasks", "sweep", "add to kanban", "create a task for this", or asks to hand
  off work to the orchestrator.
---

You have access to the **agtx kanban board** via MCP tools. You can create, list, and manage
tasks that will be autonomously executed by coding agents.

## Board Phases

Tasks move through these phases automatically once created:

```
Backlog → Planning → Running → Review → Done
```

- **Backlog** — created here, waiting to start
- **Planning** — agent explores codebase, writes `.agtx/plan.md`
- **Running** — agent implements
- **Review** — PR created, awaiting merge
- **Done** — merged and cleaned up

The orchestrator handles all transitions automatically. Your job is sweeping the conversation only.

## Available MCP Tools

- `list_tasks` — see current board state, avoid duplicates
- `create_task` — create a single task
- `create_tasks_batch` — create multiple tasks with dependency wiring (atomic)
- `get_task` — fetch task details by ID

## What Makes a Good Task

**Title**: short imperative phrase, ≤ 8 words
> "Add streaming CSV export endpoint"

**Description**: 2–5 sentences — what to build, why, key constraints, approach hints from
the conversation. Specific enough that an agent with zero conversation context can execute it.

**Dependencies**: use `depends_on` indices in batch creation when task B needs output from task A.

**Plugin**: `agtx` (default) for most tasks. `gsd` for structured spec-driven work. `void` for
plain sessions with no prompting.

## Sweep Workflow

When asked to sweep the conversation to the board:

1. Call `list_tasks` — check for duplicates
2. Extract every actionable work item from the conversation
3. Present proposed task list for confirmation:
   ```
   [0] Add streaming CSV export endpoint
       Implement GET /export/csv with streaming response
       depends on: none
   [1] Add date range filter to export
       Query params ?from=&to= applied before streaming
       depends on: [0]
   ```
4. After confirmation: use `create_tasks_batch` for multiple tasks, `create_task` for one
5. Report created IDs:
   ```
   ✓ a1b2c3  Add streaming CSV export endpoint
   ✓ d4e5f6  Add date range filter to export
   ```

## Constraints

- Do NOT implement anything — sweeping only
- Do NOT modify source files
- Flag vague/exploratory items as open questions rather than tasks
- Keep tasks small and independently executable where possible
