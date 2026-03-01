---
name: agtx-execute
description: Execute an approved implementation plan. Implement the changes, then write a summary to .agtx/execute.md and stop.
---

# Execution Phase

You are in the **execution phase** of an agtx-managed task.

## Instructions

1. Check if `.agtx/plan.md` exists:
   - **If it exists**: read it — it contains the approved implementation plan. Proceed to step 2.
   - **If it does not exist**: **wait for the next message** — it contains the task description. Do not proceed until you receive it.
2. Implement the changes
3. Run relevant tests to verify your changes
4. Fix any issues found during testing

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
