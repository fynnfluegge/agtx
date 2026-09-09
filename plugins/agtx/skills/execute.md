---
name: agtx-execute
description: Execute an approved implementation plan. Implement the changes, then write a summary to .agtx/execute.md and stop.
---

# Execution Phase

You are in the **execution phase** of an agtx-managed task.

## Input

The argument to this command is a task ID.

If `.agtx/plan.md` exists in the current working directory, read it — it already contains the approved plan and all needed context. Do NOT call `get_task`.

If `.agtx/plan.md` does not exist, fetch the task description:
```
mcp__agtx__get_task(task_id: "<the id passed to this command>")
```

## Instructions

1. Read `.agtx/plan.md` if it exists (skip `get_task`), otherwise fetch via `get_task`
2. Implement the changes
3. Run relevant tests to verify your changes
4. Fix any issues found during testing
5. **Commit your work.** `git add -A`, then commit with a short imperative message
   describing what you built. Verify with `git status --short` that the tree is clean.

   This is not optional and not someone else's job. Your worktree is deleted when the
   task completes, so **anything you have not committed is destroyed** — a task cannot
   be merged, and the phase cannot complete, until its work is in a commit. agtx's own
   files (`.agtx/`, agent configs) are already excluded from git, so `git add -A` picks
   up your work and nothing else.

## Output

When implementation is complete, write a summary to `.agtx/execute.md` in the **current working directory** with these sections:

## Changes
What files were modified/created and what was changed in each.

## Testing
How you verified the changes — tests run, results, manual checks.

## CRITICAL: Stop After Writing

After writing `.agtx/execute.md` (in the current working directory):
- Do NOT start new work beyond the plan
- Say: "Implementation complete. Summary written to `.agtx/execute.md`."
- Wait for further instructions
