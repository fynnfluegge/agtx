# Review

## Findings

### Fixed during review

1. **Lock icon width**: Changed from `🔒` (U+1F512, wide emoji that takes 2 cells in most terminals) to `⊘` (U+2298, narrow unicode symbol) — consistent with existing indicators (`✓`, `✗`, `⏸`, `⚠`) which are all single-width.

2. **Dep filter scope in `allowed_actions()`**: The initial implementation filtered `move_forward` from *all* statuses when deps were unsatisfied. This would incorrectly block Planning→Running and Running→Review transitions for tasks that already started. Fixed to only filter when `task.status == TaskStatus::Backlog`. The TUI-side checks were already correctly scoped to Backlog.

### What looks good

- **`deps_satisfied()` helper**: Clean, handles empty refs and missing tasks gracefully. Missing refs default to satisfied (deleted tasks shouldn't block).
- **Four enforcement points**: TUI (`move_task_right`, `move_backlog_to_running_by_id`), orchestrator (`execute_transition_request`), MCP (`allowed_actions`), and MCP eager validation (`move_task` handler). Complete coverage.
- **Warning message**: Uses existing `warning_message` pattern with auto-clear — no new UI state needed.
- **Deps cache**: Refreshed only on `refresh_tasks()`, avoids DB queries per render frame.
- **`base_branch` for stacked PRs**: Clean trait extension with `Option<String>` (avoids mockall lifetime issues with `Option<&str>`). The `--base` flag is only added when `base_branch` is Some.
- **MCP/TUI parity**: Full parity achieved — `deps_satisfied` in both `list_tasks` and `get_task`, `blocking_tasks` array with blocker details in `get_task`, `escalation_note` exposed, `research` action dep-gated on both sides, eager dep validation in `move_task` handler.
- **Tests**: 5 new dep satisfaction tests covering all cases (no refs, all satisfied, partial block, missing ref, planning block). 607 total tests pass.

### Concerns (acceptable)

- **Circular deps**: Two tasks referencing each other will both be permanently blocked. Not worth adding cycle detection — this is a user error and easily visible on the board.
- **`_plugin` variable**: Renamed from `plugin` to suppress unused warning in `allowed_actions()`. The plugin loading was pre-existing dead code in this method — left as-is since removing it is out of scope.

## Status

READY
