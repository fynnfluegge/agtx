---
name: agtx-execute
description: Execute an approved implementation plan. Implement the changes, then write a summary to .agtx/execute.md and stop.
---

# Execution Phase

You are in the **execution phase** of an agtx-managed task.

## Input

The argument to this command is a task ID. Fetch the task description using the agtx MCP tool:
```
mcp__agtx__get_task(task_id: "<the id passed to this command>")
```
Use the `description` field as the task to work on. Also check for `.agtx/plan.md` if a planning phase was completed first.

## Instructions

1. Fetch the task description via `get_task`
2. If `.agtx/plan.md` exists, read it for the approved plan
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
