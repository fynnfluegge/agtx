# Per-agent smoke tests

Does a real agent binary actually receive its work?

Nothing else in the tree answers that. The unit suite mocks `TmuxOperations`, so it pins what agtx
*sends* and by construction cannot catch "the agent ignored it". `tests/agent_parity_tests.rs` pins
launch strings as literals, which proves agtx builds the string it used to build, not that any binary
accepts it. Three silent-hang bugs found by hand in a single session lived in exactly that gap — all
of the same shape: **the prompt was delivered and then never read**.

```sh
cargo build --release

tests/smoke/agent_smoke.py                                  # installed agents x the agtx plugin
tests/smoke/agent_smoke.py --agents claude,codex
tests/smoke/agent_smoke.py --agents claude --plugins all --force-unsupported
tests/smoke/agent_smoke.py --phases planning,running,review --json out.json
```

Opt-in, never CI: it needs real agent auth and spends real tokens. Run it before a release, or after
touching the send path. A full pass is ~60-70s per (agent, plugin) case.

## What each phase asserts

| # | Check | Passes when |
|---|---|---|
| 1 | `submitted` | the resolved skill command reached the agent as a *submitted message*, not text parked in a composer |
| 2 | `skill-ran` | the marker file contains the **task id** that was passed in — argument substitution, not merely "a skill fired" |
| 3 | `artifact` | the phase artifact appeared, and agtx moved the task to the next status |
| 4 | `usable` | the agent process is alive and no dialog is on screen |

Check 4 is the one that matters most. Antigravity's skill *ran* and wrote its file while the session
sat blocked on an unanswered trust dialog: a harness that only asserted output would have certified
it green.

Check 1 reports `?` when the command text is not visible in the pane — several TUIs redraw their
scrollback, and check 2 catches a genuinely undelivered command anyway. It reports `–` for copilot,
which has no interactive slash-command syntax at all.

## Outcomes

| Outcome | Meaning |
|---|---|
| `PASS` | every applicable check passed for every phase |
| `FAIL` | a check failed; the report prints which, plus the last 20 pane lines and the scratch repo path |
| `SKIP` | the case was **not tested** — agent not installed, plugin needs project setup, agent not in the plugin's `supported_agents` |
| `PARK` | delivered its work and *then* parked on a dialog agtx does not answer (see below) |
| `QUOTA` | the agent hit a usage limit — a known park, not a send-path bug |
| `AUTH` | the case failed and the pane shows a login prompt |
| `ERROR` | the harness itself could not complete the case (MCP timeout, tmux failure) |

Skips are printed in a block of their own at the end, listing every untested case with its reason. A
green run that quietly tested two of eight agents is worse than no run, because it invites the
assumption that the other six work.

Exit code is 1 if anything is `FAIL` or `ERROR`, 0 otherwise.

## How it works

```
scratch git repo (one commit)  ──►  agtx TUI in tmux -L agtx-smoke
        │                                    │
        │  .agtx/config.toml pins the agent   │ spawns the agent on tmux -L agtx
        ▼                                    ▼
  agtx mcp-serve <repo>  ◄── create_task / move_task / get_task ── the runner
```

Phases are driven through **agtx's own MCP server**, exactly as `benchmark/swebench/benchmark.py`
does. Reimplementing skill deployment or phase transitions in the runner would test the runner.

The agent × plugin matrix is not duplicated here: `cargo run --example agent_matrix` dumps
`AGENT_SPECS` and `BUNDLED_PLUGINS` as JSON — binaries, dialogs, liveness signals, per-agent command
syntax (through the real `transform_plugin_command`), artifacts. The runner reads that, so it cannot
drift from what agtx actually does. Its source is `agent_matrix.rs` in this directory, declared as an
`[[example]]` with an explicit path: an example rather than a bin so `cargo install` never ships it,
and still compiled by `cargo test` so a new spec field breaks the build instead of the runner.

### Dialogs are never pre-answered

All three motivating bugs were dialog bugs. A harness that made itself reliable by writing
`hasTrustDialogAccepted`, passing `--trust`, or copying `~/.claude.json` would have been green
through every one of them. The scratch repo gets `agtx trust` — that is agtx's *project config*
trust, which gates plugin init scripts — and nothing else. No agent trust state is seeded.

### The fixture

The task is "write this one file and stop", costing a few hundred tokens per phase. The marker
carries the task id rather than fixed text, because a skill that runs but drops its argument is a
real failure mode.

Each phase artifact is written with one line pointing at `.agtx/smoke/INSTRUCTIONS.md`, which the
runner drops in the worktree. This is not decoration: agtx's `execute` skill says, in as many words,
*"if `.agtx/plan.md` exists, read it — do NOT call `get_task`"*, and the agtx plugin clears the
agent's context between phases. An artifact containing `smoke` strands the running phase with no idea
what it was asked for. Pointing at a file carries the fixture forward through the same mechanism the
plugin uses to carry a plan forward.

## Testing the harness

```sh
python3 tests/smoke/test_agent_smoke.py
```

Deterministic — no tmux, no agent binaries, no auth. Covers the two paths whose failure would make
the harness lie: an uninstalled agent must report `skipped` and never `pass`, and a case that never
delivers must report a failure carrying pane output rather than hanging.

One test needs `cargo`, because it cross-checks the runner's list of dialogs *agtx does not answer*
against the ones `AGENT_SPECS` declares — a pattern must not be on both lists, and it was, once. It
skips rather than fails without a toolchain.

## Design decisions

1. **"Session still usable" is defined as** the agent process alive **and** no dialog matching. The
   tempting third option — the agent's own indicator visible — does not generalise, because copilot
   declares empty `active_indicators`. (Antigravity did too, until this runner found it had no
   readiness signal at all and one was added.)

   Liveness is read from the pane's **process tree**, not from `pane_current_command`. The latter
   reports the pane leader, and every agent is launched as `sh -c '<agent>'` with `exec $SHELL`
   behind it: verified against tmux 3.5a on macOS, a live Claude pane reports `bash`, and so does a
   dead one. The hook-reported `agent_state` is folded in only as a tiebreaker when nothing landed,
   because agtx maps Claude's `Notification` event to `blocked` and that event also means "has been
   waiting for your input" — every completed phase would otherwise read as unusable.

2. **It lives in `tests/smoke/`, as a script rather than a `#[ignore]`d integration test.** Auth and
   quota make it fundamentally not a `cargo test`; cargo ignores the directory because it has no
   `main.rs`, so it sits alongside the Rust suite without joining it. Python rather than shell,
   because the MCP transport is JSON-RPC over stdio — stdlib only, no dependencies. The one piece
   that *must* stay near the code, the agent table, is read from `agent_matrix.rs` in this same
   directory rather than copied.

3. **Quota is its own outcome**, matched on patterns that describe a limit actually being *hit*. The
   first draft matched a bare `usage limit` and turned Codex's startup tip ("You have 1 usage limit
   reset available") into a QUOTA row, hiding a real delivery failure underneath it.

4. **Per-agent expected outcomes are encoded, narrowly — and `EXPECTED_PARK_AGENTS` is now empty.**
   It held `antigravity` for exactly one run. Once agtx learned to answer that trust dialog the agent
   passed cleanly, and the harness reported its own entry as stale — an expectation that encodes
   today's behaviour has to announce itself the moment today's behaviour improves, or it quietly
   becomes the specification. The mechanism stays, with its
   two guards — a lost prompt is never excused as a park (only a check-4-only failure qualifies), and
   a clean pass prints the staleness note.

5. **Agent switching is not covered.** `switch_agent_in_tmux` and its per-agent exit commands are
   the obvious next thing to add.

## What it has found

| Finding | Fix |
|---|---|
| The launch-lane prompt was mangled by double shell quoting — claude received `["--dangerously-skip-permissions", "/agtx:plan"]`, codex `["--sandbox", "workspace-write", "-plan"]` | `single_quote()` in `src/tmux/operations.rs` |
| Antigravity parked on an undeclared trust dialog, and agtx's own Enter confirmed it *after* pasting the task into the menu — every task reached an empty composer | its dialog declared with a bare-Enter answer, plus a real readiness indicator |
| Cursor did the same, one day later: `Workspace Trust Required` undeclared, dismissed by accident by the send's own Enter | its dialog declared, answered with the access key it advertises (`a`) |
| A dropped paste or keystroke was silent and unrecoverable in both mid-session paths | `deliver_message()` resends while the pane is unchanged |
| A bare `usage limit` pattern reported a healthy Codex session as QUOTA | narrowed to limits actually *hit* |
| An opencode provider-credential error, gemini's OAuth prompt, and gemini's `API key not valid` read as undelivered prompts | `AUTH` outcome, short-circuited |
| Grok's "You hit your free usage limit." read as a send failure | quota pattern widened to cover both wordings |

## Known limits

- The matrix runs cases sequentially. Parallelism would need one tmux server and one project DB per
  case, which is doable but not worth it at ~60s per case.
- `--plugins all` reaches plugins that need `npx`, a framework install, or a `.specify`/`openspec`
  directory in the project root. Those are skipped by default with a reason derived from the plugin's
  own TOML; `--force-unsupported` runs them anyway, with the network cost that implies.
- The `void` plugin sends the agent nothing by design, so it is always skipped: there is no delivery
  to observe.
