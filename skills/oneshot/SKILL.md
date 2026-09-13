---
name: agtx-oneshot
description: "One-shot a whole project on an agtx board: decompose the goal, run every task unattended, unblock the workers, and merge each one. Use when the user wants a long autonomous run rather than a single session."
disable-model-invocation: true
---

# agtx — One-Shotting a Project

You are **one-shotting** a project on an agtx kanban board: a goal too large for one
session, run to completion unattended.
Ordinarily a person sits at the board and does this. Here, you are that person.

This is not the built-in orchestrator (`O`), which only advances Planning → Running →
Review and refuses to touch Backlog. **You own all five columns**: you decompose the
goal into tasks, decide what starts, watch the workers, unblock them, judge Review, and
merge. The workers do the coding; you never write feature code yourself.

```
Backlog → Planning → Running → Review → Done
^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
              all of it is yours
```

## Before anything else: verify the board is live

**Nothing you do executes unless the agtx TUI is running.** `move_task` writes a row to
a queue; the TUI drains it and performs the real work — worktree creation, agent spawn,
skill deployment. With no TUI, every transition sits pending forever and the board looks
frozen for no visible reason.

1. Call `list_tasks`. If the MCP server does not answer, tell the user to register it and stop:
   ```bash
   claude mcp add-json agtx '{"type":"stdio","command":"<abs path to agtx>","args":["mcp-serve","<abs path to project>"]}' --scope local
   ```
2. Check `tui_connected` in that response. If it is `false`, no TUI is draining the
   queue — ask the user to open `agtx` in another terminal on this project, and do not
   queue anything into a dead board.

Also confirm before a long unattended run:

- **Call `get_config` and check `auto_trust`.** It must be `true`, or agents' trust and
  bypass-permission dialogs are detected but left unanswered and every task parks as
  Blocked waiting for a human who is not there. Use the tool — do **not** read
  `~/.config/agtx/config.toml` yourself: `AGTX_CONFIG_DIR` relocates that file, so the path
  is not authoritative and you will get a stale answer. `get_config` also tells you which
  file to edit. `auto_trust` is global-only, so a project config cannot turn it on.
- **Open the coding agent once in the project root and accept its trust prompt** before
  starting. Your own session parks on that dialog in a directory the agent has not seen,
  and an unattended run has nobody to answer it.
- The project has a **base branch** and a clean tree.

## Three things that differ from a normal agtx session

**1. `allowed_actions` is not written for you.** `get_task` computes that field for the
built-in orchestrator, which is forbidden from triaging Backlog — so it comes back empty
for every Backlog task. **Ignore it.** `move_task` itself does not enforce it: `research`,
`move_to_planning`, `move_to_running`, `move_forward`, `move_to_review`, `move_to_done`
and `resume` all work from here. The only real gate is dependencies (below).

**2. No notifications reach you.** `get_notifications` only fills when the built-in
orchestrator is running. Assume it returns nothing. `wait_for_board_change` is your whole
feedback loop (see *The loop*).

**3. Read `phase_status` against `phase_age_secs`, never alone.** `list_tasks` and
`get_task` both carry the board's own verdict on each task:

| `phase_status` | Meaning |
|---|---|
| `working` | The agent is producing output. Leave it alone. |
| `ready` | The phase artifact exists — the phase is **complete**. Advance it. |
| `blocked` | The agent reported it is waiting on a human. |
| `idle` | No output for 15s. A guess, not a report. |
| `exited` | The tmux window is gone. |

**`tui_connected` decides whether any of that is current.** `list_tasks` and
`wait_for_board_change` both carry it. When it is `false`, nothing is executing transitions and
every `phase_status` is frozen at whatever was last observed — **stop and tell the user**;
do not read those rows as task state. A live run once showed a task `blocked` for minutes
while its agent worked normally, purely because the TUI had exited.

`phase_age_secs` is the corroborating detail: the board republishes every live task on
every pass, so a small age means "seen just now". Check `tui_connected` first — it is the
direct answer; age is the symptom.

Two other signals, for when you need more than the verdict:

| Signal | How | Use |
|---|---|---|
| `agent_state` | `get_task`, hook-reported | `blocked_reason` names the exact prompt the agent is waiting on |
| `read_pane_content` | last N lines of the pane | Ground truth, but costs context — diagnose with it, never poll with it |

Artifact files are the fallback if `phase_status` is absent. For the default `agtx`
plugin, relative to `worktree_path`: `.agtx/research.md`, `.agtx/plan.md`,
`.agtx/execute.md`, `.agtx/review.md`. Other plugins declare their own under
`[artifacts]` in `plugins/<name>/plugin.toml`.

**4. Starting tasks is serialized, and that is fine.** Worktree setup runs one at a
time. Queue as many `move_to_planning` calls as you like in one pass — they line up and
drain in order. A start waiting for the slot has simply not resolved yet. You do not need
to poll `get_transition_status`: `wait_for_board_change` reports the outcome of every
move you queued in its `transitions` list, and wakes you if one fails. Only an `error`
means it will not happen.

## Decomposition

A goal this size cannot be enumerated up front, and trying wastes the run. Work in
**waves**.

1. Write a milestone spine first — 4–8 milestones, coarse, in `oneshot-state.md` (below).
   Nothing goes on the board yet.
2. Turn **only the current milestone** into tasks. One task = one reviewable,
   independently mergeable PR. `create_tasks_batch` takes up to 50 and wires
   dependencies by 0-based index into the same array (no forward references).
3. When a milestone is mostly in Review/Done, write the next wave — informed by what the
   workers actually built, which is the point of not planning it all on day one.

Task quality decides whether the run survives:

- **Title**: imperative, ≤ 8 words, ≤ 120 characters (hard cap — creation fails above it).
- **Description**: 3–6 sentences. The worker agent has **zero** context from this
  conversation. Name the files, the interfaces it must match, the constraints, and what
  "done" looks like. A vague description is the single most common cause of a task that
  burns an hour and produces nothing.
- **Dependencies are a real gate.** A Backlog task whose referenced tasks are not yet in
  Review or Done cannot be advanced at all — the move is refused. Use this deliberately
  to sequence, and don't over-wire it: a false dependency stalls a whole branch of the DAG.

```json
create_tasks_batch({
  "tasks": [
    { "title": "Add entity component store", "description": "..." },
    { "title": "Add physics step over the store", "description": "...", "depends_on": [0] },
    { "title": "Add render pass over the store", "description": "...", "depends_on": [0] }
  ]
})
```

Keep 3–6 tasks running at once. More than that and you cannot actually supervise them,
and the machine starts thrashing on parallel agent sessions.

## The loop

**Call `wait_for_board_change`, act on what it returns, and call it again.** It blocks
inside the server until something needs you, then returns only the tasks that changed
since you last looked. The first call returns the whole board.

Do not `sleep` and re-list instead. Every poll is a turn, and every turn re-reads your
entire context — so a run's cost is set by how many turns it takes far more than by what
any one call returns. A task starting to work does not wake the wait; you hear about a
task when it needs you.

Each time it returns:

1. `tui_connected: false` → the TUI is gone and every `phase_status` is frozen. Stop and
   say so rather than acting on it.
2. `transitions` → the outcome of each move you queued. An `error` is the one failure
   nothing else on the board shows; read it and act on it (see *Review and merge* for
   merge refusals and conflicts).
3. For each task in `tasks`:
   - `ready` in Planning or Running → `move_task` with `move_forward`.
   - `ready` in Review → judge it (below).
   - `blocked` → `get_task` for `blocked_reason`. A permission or trust prompt is a
     config problem (`auto_trust`), not something to answer by typing into the pane —
     surface it to the user. A question: see *Answering questions* below.
   - `idle` → `read_pane_content`, then act on what you see. For an agent without hooks
     `idle` is a guess from pane output, so confirm before nudging.
   - `exited` → the session died. `resume` it, or investigate before restarting.
   - `done` → record it; its dependents may now start.
   - Backlog with `deps_satisfied` → start it if it fits the concurrency budget —
     `move_to_planning`. Starts serialize on their own.
4. Update `oneshot-state.md`, then wait again.

`timed_out: true` means nothing needed you — wait again. It is not a reason to inspect
every task.

**Keep the board in `oneshot-state.md`, not in your head.** The wait tells you what
changed; the state file is what you apply it to. When your context has been compacted,
or you are picking up a run, call `list_tasks` once to re-sync — it leaves descriptions
out, and `get_task` has the full task when you need one. A session that has memorised a
stale board makes confident wrong moves.

### Keeping durable state

Maintain `oneshot-state.md` in the project root. It is what a fresh session reads to pick
up the run:

```markdown
# Oneshot state — <goal>
Updated: <timestamp>

## Milestones
- [x] M1 core loop
- [ ] M2 world streaming   ← current wave
- [ ] M3 ...

## Board
| id | title | status | notes |
|----|-------|--------|-------|
| a1b2c3 | Add entity component store | Done | merged |
| d4e5f6 | Add physics step | Running | nudged once, was idle |

## Decisions
- Physics runs fixed-step at 60Hz; renderer interpolates. (M1, task a1b2c3)

## Open questions for the user
- (none)
```

Rewrite the Board table each pass; **append** to Decisions and Open questions.
Decisions are what stop later tasks contradicting earlier ones.

## Review and merge

Review is where an unattended run quietly produces garbage, so spend your judgment here
rather than on scheduling.

1. Read the diff (`git -C <worktree_path> diff <base>...HEAD`). Ask only: does this do
   what the task said, and does it break the milestone spine? You are not doing a
   line-level code review — the Review phase agent does that.
2. **If it is wrong, decide how wrong before acting.** Small fixes go to the reviewer in
   place; `resume` is only for real rework.
   - **A small, contained change** — a wrong sentence, a missing guard, a rename, a test
     to add — send it with `send_to_task` **while the task stays in Review**. Say exactly
     what to change, and to commit and update `.agtx/review.md` when done. The reviewer
     makes it where it is: no transition, no second execute cycle. Its turn restarts, so
     `phase_status` reads `working` until it has finished; wait for `ready`, re-read the
     diff, then merge.
   - **Significant rework** — the approach is wrong, a requirement was missed, the work
     needs replanning — `resume` the task, then `send_to_task` the correction. **`resume`
     puts the task back in Running**, so when the agent is done you must `move_forward` to
     Review again before you can merge; merging straight after a resume is refused.

   Either way, act only once the agent has stopped. `ready` means that for an agent with
   hooks; for one without, read the pane first. An agent writes `review.md` and keeps
   working after it, and a message sent into a turn that has not ended lands in a busy
   pane. Vague feedback produces another wrong attempt, in either lane.
3. **Land it with `move_to_done_and_merge`.** This merges the branch into its base in the
   project checkout and then moves the task to Done. Plain `move_to_done` keeps the branch
   and merges nothing — use it only if you are integrating some other way, otherwise you
   finish the run with N parallel branches and an empty base branch.
4. **On conflict the task stays in Review** and its own agent is sent
   `/agtx:merge-conflicts` automatically, with the conflicting files named. The
   transition reports an error saying so. Do not retry immediately and do not try to
   resolve it yourself — wait for that task to go `ready` again, then call
   `move_to_done_and_merge` a second time.
5. **A refusal is about the project checkout, not the branch.** "on X, not the base
   branch" or "has uncommitted changes" means someone left the project root dirty or on
   another branch. Nothing was changed. Tell the user — you must not switch their branch
   or stash their work to get around it.
6. **`move_to_done` is refused while the *worktree* has uncommitted changes.** Different
   error, different fix: `send_to_task` telling the agent to commit, then retry. Do not
   queue it repeatedly; it will keep failing.

Merges are serialized with everything else the board does, so several tasks reaching
Done in one pass is safe.

## Answering questions

When a worker asks something, classify before acting:

- **A tool's yes/no prompt** — the uppercase letter is the default; `send_to_task` it.
- **A numbered menu with a marked default** — send that number.
- **A question about the goal that your milestone spine or Decisions already answers** —
  answer it. That is why you keep those.
- **A question that changes the shape of the product**, or a menu with no default, or a
  trust/permission prompt — **stop and ask the user.** Record it under *Open questions*
  and keep the rest of the board moving while you wait. Do not invent product decisions
  the user would want to make; a wrong answer here propagates into every later task.

If the same task goes idle twice after you nudged it, stop nudging. Escalate to the user
or delete and rewrite the task with a better description.

## Rules

- You never write feature code. You write tasks, and `oneshot-state.md`.
- Drive the run with `wait_for_board_change`, never with `sleep`. Use `list_tasks` to
  re-sync after a gap, never to poll.
- Ignore `allowed_actions`; respect dependency refusals.
- One task = one mergeable PR. Split anything with "and" in its title.
- Cap concurrency at what you can actually supervise (3–6).
- Nothing executes without the TUI. If transitions stop completing, or `phase_age_secs`
  climbs across the whole board, say so and stop.
- Never touch the user's checkout to get a merge through — no branch switch, no stash.
- Surface product decisions to the user rather than deciding them.
- Report honestly. A milestone with three failed tasks is a result; say it plainly.
