---
name: agtx-plan
description: Analyze the codebase, write a plan, then decide whether to decompose into parallel subtasks.
---

# Planning Phase

You are in the **planning phase** of an agtx-managed task.

## Input

- **Task description** — provided inline with this command (when entering directly from Backlog)
- **`.agtx/research.md`** — prior analysis from research phase (when research was completed first)

## Instructions

1. If `.agtx/research.md` exists, read it for prior analysis
2. Read and understand the task description
3. Explore the codebase to understand relevant files, patterns, and architecture
4. Identify all files that need to be created or modified
5. Create a detailed implementation plan

## Output

Write your plan to `.agtx/plan.md` with these sections:

## Analysis
What you found in the codebase — relevant files, patterns, dependencies.

## Plan
Step-by-step implementation plan — files to modify, approach, order of changes.

## Risks
What could go wrong — edge cases, breaking changes, areas needing extra care.

## CRITICAL: Stop After Writing

Once you have written `.agtx/plan.md`, apply the decomposition heuristic below — do NOT
start implementing.

## Decomposition Heuristic

**Split into subtasks if 2+ of these signals are present:**
- Changes span 3+ distinct top-level directories (e.g. `src/db/`, `src/mcp/`, `src/tui/`)
- Plan has clearly independent layers (schema, API, UI, tests) with minimal cross-dependency
- 5+ implementation steps that could proceed in parallel or strict sequence
- Different subtasks would never need to edit the same file

**Proceed as single task if:**
- Change is focused within 1-2 modules
- Steps are tightly coupled — each step depends on the previous output
- 3 or fewer files need to be modified

## Path A: Decompose (complex)

Before calling `create_subtask`, write a focused plan file for each subtask at
`.agtx/subtasks/{slug}/plan.md`. Each file must be self-contained — no references to the
global plan or other subtasks.

```markdown
# {title}

## Scope
{comma-separated files/dirs this subtask exclusively owns}

## What to implement
{focused implementation steps — only what this subtask does}

## Context
{minimal background needed — data structures, interfaces, conventions this subtask depends on}
```

Then call `create_subtask` via MCP for each subtask:
- `parent_task_id`: the current task's ID (available from your context as the agtx task ID)
- `slug`: "subtask-1", "subtask-2", etc. — must match the directory name used above
- `title`: 3-5 words, imperative, describes the deliverable (e.g. "Add DB schema migrations")
- `description`: 1-3 sentences max — what to implement and which files to own
- `scope`: comma-separated files/dirs this subtask exclusively owns
- `depends_on`: sibling slugs that must complete before this starts ([] if none)

Rules:
- Each file must appear in exactly one subtask's scope
- Subtasks with no dependencies can run in parallel — list first
- Tests/docs subtask always depends_on all implementation subtasks
- Maximum 5 subtasks

After all `create_subtask` calls:
1. Write `.agtx/planning.done`
2. Say: "Decomposition complete — N subtasks created."
**Stop.**

## Path B: No decomposition (simple)

1. Write `.agtx/planning.done`
2. Say: "No decomposition needed. Planning complete."
**Stop.**
