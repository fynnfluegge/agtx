# Execution Summary: Dependency Enforcement & Stacked PRs

## Changes

### `src/db/schema.rs`
- Added `deps_satisfied(&self, task: &Task) -> bool` method on `Database`. Resolves `referenced_tasks` comma-separated IDs, returns `true` only if all referenced tasks are in Review or Done (or if no deps exist). Missing refs treated as satisfied.

### `src/mcp/server.rs`
- Added `deps_satisfied: bool` field to both `TaskDetail` and `TaskSummary` structs.
- Added `BlockingTask` struct (`id`, `title`, `status`) and `blocking_tasks: Vec<BlockingTask>` to `TaskDetail` — lists deps not yet in Review/Done.
- Added `escalation_note: Option<String>` to `TaskDetail`.
- Added `referenced_tasks`, `base_branch` to both `TaskDetail` and `TaskSummary`.
- Changed `allowed_actions()` to accept `deps_satisfied: bool` parameter. When false and task is in Backlog, filters out `move_forward`, `move_to_planning`, `move_to_running`.
- Updated `get_task` handler to compute `deps_satisfied`, `blocking_tasks`, and pass them to `TaskDetail` and `allowed_actions()`.
- Updated `list_tasks` handler to compute and include `deps_satisfied` per task.
- Added eager dep validation in `move_task` handler — rejects `move_forward`, `move_to_planning`, `move_to_running`, `research` for Backlog tasks with unsatisfied deps before queuing the transition.

### `src/tui/app.rs`
- **`move_task_right()`**: Added dep check before Backlog transitions — shows warning message and returns early if deps unsatisfied.
- **`move_backlog_to_running_by_id()`**: Same dep check added.
- **`execute_transition_request()`**: Added dep check for orchestrator forward transitions (`move_forward`, `move_to_planning`, `move_to_running`, `research`) — bails with error when deps unsatisfied.
- **`AppState`**: Added `deps_satisfied_cache: HashMap<String, bool>` field.
- **`refresh_tasks()`**: Populates deps cache for all tasks with `referenced_tasks`.
- **`draw_task_card()`**: Added `deps_blocked` parameter. Shows `⊘` indicator in dimmed style for blocked tasks in Backlog.
- **`create_pr_with_content()`**: Passes `task.base_branch.clone()` to `create_pr()`.

### `src/git/provider.rs`
- Extended `GitProviderOperations::create_pr()` trait with `base_branch: Option<String>` parameter.
- Updated `RealGitHubOps::create_pr()` to add `--base <branch>` args when `base_branch` is Some.

### `src/tui/app_tests.rs`
- Updated 3 mock `expect_create_pr()` expectations to include the new 5th `base_branch` parameter.

### `tests/db_tests.rs`
- Added 5 tests: `test_deps_satisfied_no_refs`, `test_deps_satisfied_all_review_or_done`, `test_deps_not_satisfied_dep_in_backlog`, `test_deps_satisfied_missing_ref_treated_as_ok`, `test_deps_not_satisfied_dep_in_planning`.

## Testing

- `cargo build` — compiles cleanly (warnings are pre-existing, unrelated)
- `cargo test --features test-mocks` — **607 tests pass**, 0 failures
  - 5 new dependency satisfaction tests in db_tests
  - 3 existing PR creation tests updated for new signature
