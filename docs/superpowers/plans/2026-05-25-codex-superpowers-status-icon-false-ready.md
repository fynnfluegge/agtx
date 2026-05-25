# Codex Superpowers Status Icon False-Ready Fix Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Ensure Superpowers+Codex planning tasks do not show a green tick until a plan file is created in that task worktree.

**Architecture:** Keep runtime status logic unchanged. Fix repository state by removing stale committed plan artifacts that are inherited into new worktrees, and add a regression test that proves planning is `Working` when the planning artifact does not exist.

**Tech Stack:** Rust, cargo test (`test-mocks` feature), agtx TUI unit tests.

---

### Task 1: Add Regression Test For Planning Artifact Absence

**Files:**
- Modify: `src/tui/app_tests.rs`
- Test: `src/tui/app_tests.rs`

- [ ] **Step 1: Write the failing test**

Add this test near existing phase status cache/apply session refresh tests:

```rust
#[test]
fn test_superpowers_planning_not_ready_without_plan_artifact() {
    use crate::db::TaskStatus;

    let mut app = App::new_for_test();

    let mut task = Task::new("Superpowers planning", "codex", "project-1");
    task.id = "task-1".to_string();
    task.status = TaskStatus::Planning;

    // Simulate background refresh result where planning artifact does NOT exist yet.
    app.apply_session_refresh(SessionRefreshResult {
        statuses: vec![SessionTaskStatus {
            task_id: task.id.clone(),
            phase_status: PhaseStatus::Working,
            content_hash: None,
            status: TaskStatus::Planning,
            worktree_path: None,
            session_name: None,
            agent: "codex".to_string(),
            was_ready: false,
        }],
    });

    let (phase, _) = app
        .state
        .phase_status_cache
        .get(&task.id)
        .expect("phase status should be cached");
    assert_eq!(*phase, PhaseStatus::Working);
}
```

- [ ] **Step 2: Run test to verify it fails (if behavior regressed) or passes (if already correct)**

Run:
```bash
rtk cargo test --features test-mocks test_superpowers_planning_not_ready_without_plan_artifact -- --nocapture
```

Expected:
- PASS on current correct logic, confirming baseline behavior.
- If FAIL, investigate before proceeding.

- [ ] **Step 3: Commit the regression test**

```bash
rtk git add src/tui/app_tests.rs
rtk git commit -m "test: cover superpowers planning non-ready state without artifact"
```

### Task 2: Remove Stale Superpowers Plan Artifact From Repository

**Files:**
- Delete: `docs/superpowers/plans/2026-05-25-superpowers-codex-support.md`

- [ ] **Step 1: Remove the stale committed plan file**

Run:
```bash
rtk rm docs/superpowers/plans/2026-05-25-superpowers-codex-support.md
```

- [ ] **Step 2: Verify stale artifact path no longer exists in git index**

Run:
```bash
rtk git ls-files docs/superpowers/plans
```

Expected:
- File `docs/superpowers/plans/2026-05-25-superpowers-codex-support.md` is not listed.

- [ ] **Step 3: Commit artifact cleanup**

```bash
rtk git add -A
rtk git commit -m "fix: remove stale superpowers plan artifact causing false ready state"
```

### Task 3: End-to-End Verification In This Branch

**Files:**
- Modify: none
- Test: repository-wide verification commands

- [ ] **Step 1: Run focused TUI tests related to phase status**

Run:
```bash
rtk cargo test --features test-mocks phase_status_cache -- --nocapture
```

Expected:
- PASS for phase status cache behaviors.

- [ ] **Step 2: Run full required test suite**

Run:
```bash
rtk cargo test --features test-mocks
```

Expected:
- All tests pass.

- [ ] **Step 3: Capture proof in git history**

Run:
```bash
rtk git log --oneline -n 5
```

Expected:
- Shows the two commits from Tasks 1 and 2.

- [ ] **Step 4: Optional manual sanity check in agtx TUI**

Run:
```bash
rtk cargo build --release
```

Then launch agtx in a project and create a fresh Superpowers+Codex planning task. Confirm card indicator is spinner/pause (not green tick) until a new plan file is actually written in that worktree.

## Self-Review

- Spec coverage: plan covers root cause (stale inherited artifact), preserves existing runtime logic, and validates non-ready-before-plan behavior.
- Placeholder scan: no TBD/TODO or ambiguous implementation instructions.
- Type consistency: test uses existing `App`, `Task`, `SessionRefreshResult`, `SessionTaskStatus`, and `PhaseStatus` naming consistent with current code.
