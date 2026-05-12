---
name: agtx-plan
description: Plan a task implementation. Analyze the codebase, create a detailed plan, write it to .agtx/plan.md, then stop and wait for user approval before making any changes.
---

# Planning Phase

You are in the **planning phase** of an agtx-managed task.

## Input

The argument to this command is a task ID. Fetch the task description using the agtx MCP tool:
```
mcp__agtx__get_task(task_id: "<the id passed to this command>")
```
Use the `description` field as the task to work on. Also check for `.agtx/research.md` if a research phase was completed first.

## Instructions

1. Fetch the task description via `get_task`
2. If `.agtx/research.md` exists, read it for prior analysis
3. Explore the codebase to understand relevant files, patterns, and architecture
4. Identify all files that need to be created or modified
5. Create a detailed implementation plan

## Output

Write your plan to `.agtx/plan.md` in the **current working directory** (do NOT navigate up — write directly to `.agtx/plan.md` relative to where you are now) with these sections:

## Analysis
What you found in the codebase — relevant files, patterns, dependencies.

## Plan
Step-by-step implementation plan — files to modify, approach, order of changes.

## Risks
What could go wrong — edge cases, breaking changes, areas needing extra care.

## CRITICAL: Stop After Writing

After writing `.agtx/plan.md` (in the current working directory):
- Do NOT start implementing
- Do NOT modify any source files
- Say: "Plan written to `.agtx/plan.md`. Waiting for approval."
- Wait for explicit instructions to proceed

## Output Style

Terse. No pleasantries. Fragments OK. Short synonyms. Code exact.
Status updates: one line. Pattern: [what] [why]. Done.
