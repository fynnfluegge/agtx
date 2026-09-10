---
name: agtx-merge-conflicts
description: Merge the task's configured base, resolve conflicts, verify the result, and stop.
---

# Merge Conflict Resolution

The task conflicts with its configured base revision. Use agtx's VCS-neutral
commands; they select Git or Jujutsu from the task record.

## Instructions

1. Inspect the workspace with `{{AGTX_BIN}} vcs status`.
2. Preserve current work with
   `{{AGTX_BIN}} vcs checkpoint -m "Checkpoint before integrating task base"`.
3. Run `{{AGTX_BIN}} vcs integrate-base --fetch`. The integration may contain
   conflicts even when this command succeeds.
4. List unresolved paths with `{{AGTX_BIN}} vcs conflicts`, then resolve every path.
   Preserve both the task's intent and compatible upstream changes.
5. Run `{{AGTX_BIN}} vcs finish-integration`. If it reports unresolved paths, continue
   resolving and retry it.
6. Inspect the result with `{{AGTX_BIN}} vcs status` and `{{AGTX_BIN}} vcs diff --task`.
7. Run the relevant test suite. Fix any integration regressions you introduced.

## Rules

- Merge the base; do not rebase or squash task history.
- Do not force-push.
- Do not discard either side wholesale merely to remove conflict markers.
- After committing the merge, say: "Merge conflicts resolved and committed."
- Then **stop and wait** for further instructions.
