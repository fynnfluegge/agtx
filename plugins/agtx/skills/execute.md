---
name: agtx-execute
description: Execute an approved implementation plan. Implement the changes, then write a summary to .agtx/execute.md and stop.
---

# Execution Phase

You are in the **execution phase** of an agtx-managed task.

## Input

- **Task description** — provided inline with this command (when entering directly from Backlog)
- **`.agtx/plan.md`** — approved implementation plan (when planning was completed first)

## Instructions

1. If `.agtx/plan.md` exists, read it for the approved plan
2. Read and understand the task description
3. Implement the changes
4. Run relevant tests to verify your changes
5. Fix any issues found during testing

## Output

When implementation is complete, write a summary to `.agtx/execute.md` with these sections:

## Changes
What files were modified/created and what was changed in each.

## Testing
How you verified the changes — tests run, results, manual checks.

## CRITICAL: Stop After Writing

After writing `.agtx/execute.md`:
- Do NOT start new work beyond the plan
- Say: "Implementation complete. Summary written to `.agtx/execute.md`."
- Wait for further instructions
