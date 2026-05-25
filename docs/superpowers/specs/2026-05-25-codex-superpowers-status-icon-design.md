# Codex Superpowers Status Icon Design

Date: 2026-05-25

## Problem
When using the Superpowers plugin with Codex in agtx, the task card status icon shows a green tick immediately in planning, even before the agent has produced a new plan for the task.

## Observed Behavior
- Phase readiness is computed from artifact existence in the task worktree.
- Superpowers planning artifact pattern is:
  - `docs/superpowers/plans/*.md`
- `main` currently contains:
  - `docs/superpowers/plans/2026-05-25-superpowers-codex-support.md`
- New worktrees inherit this file from `main`, so the wildcard matches immediately and planning is marked `Ready`.

## Root Cause
A stale plan artifact exists on `main` at a path that matches the planning artifact wildcard. Because worktrees are created from `main`, the inherited file satisfies the completion check before new planning output is generated.

## Design Decision
Use a minimal, non-behavioral fix:
- Remove stale plan artifact(s) from `main` that should not be versioned there.
- Keep the current status computation and artifact path logic unchanged.

Rationale:
- The check already targets each task worktree, which is correct.
- The issue is repository state (unexpected committed artifact), not runtime detection logic.
- This avoids unnecessary plugin or code changes.

## Scope
In scope:
- Repository cleanup: remove the stale Superpowers planning file from tracked `main` content.
- Verify that a fresh worktree without a new plan file does not show `Ready`.

Out of scope:
- Changing plugin artifact patterns.
- Changing phase status algorithms.
- Adding new metadata or task-specific artifact naming.

## Validation
Success criteria:
1. New task worktree created from updated `main` has no matching `docs/superpowers/plans/*.md` file by default.
2. Planning phase remains non-ready (spinner/idle), not green tick, until a new plan file is created in that worktree.
3. Existing behavior for true completion remains unchanged.

## Risks
- If future plan artifacts are accidentally committed to `main` again, the issue can recur.

## Follow-up (Optional)
- Add team hygiene guidance to avoid committing ephemeral planning artifacts to `main`.
