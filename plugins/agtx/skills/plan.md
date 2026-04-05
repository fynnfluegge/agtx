---
name: agtx-plan
description: Plan a task implementation. Analyze the codebase, create a detailed plan, write it to .agtx/plan.md, then stop and wait for user approval before making any changes.
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

Once you have written `.agtx/plan.md`:
1. Write `.agtx/planning.done` (signals phase complete to agtx)
2. Say: "Planning complete."
**Stop. Do not implement anything.**
