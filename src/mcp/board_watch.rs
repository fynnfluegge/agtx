//! "What changed since I last looked" — the state behind `wait_for_board_change`.
//!
//! A caller supervising the board unattended pays for every turn it takes, and
//! the price of a turn is its whole context, re-read. Polling is two turns per
//! pass — a sleep, then a full listing — and most passes find nothing to do.
//! Measured on a 14-task run: 73 sleeps and 96 listings, with the listings alone
//! the largest share of every tool result the session re-read afterwards.
//!
//! So the wait blocks inside the server and answers once, when something needs
//! the caller, with only the tasks that differ from what it was last shown.
//!
//! Pure: no database, no tmux, no clock. The server feeds it one [`TaskMark`]
//! per task per poll.

use std::collections::{HashMap, HashSet};

use crate::db::TaskStatus;

/// The parts of a task a caller acts on. Two equal marks mean "nothing for the
/// caller here", so what goes in decides what can wake it.
///
/// `phase_age_secs` is deliberately absent: it changes on every refresh, and a
/// mark that always differs wakes nothing but reports everything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TaskMark {
    pub status: TaskStatus,
    /// The TUI's verdict, already withheld when it describes a previous status.
    pub phase_status: Option<String>,
    /// When the agent's hook last reported a turn boundary — `waiting`,
    /// `ended` or `blocked` — and `None` while it reports `working`.
    ///
    /// This is what tells two `idle`s apart. A caller nudges an idle agent, the
    /// agent works and stops again, and the board reads `idle` before and after:
    /// equal marks, and a caller that is never told its nudge was answered. The
    /// second `Stop` carries a new timestamp. `None` while working keeps a busy
    /// agent's heartbeat out of the mark, since every tool call bumps it.
    pub turn_ts: Option<i64>,
    pub deps_satisfied: bool,
    pub escalation_note: Option<String>,
}

impl TaskMark {
    /// Whether a task in this state is waiting on the caller.
    ///
    /// `working` and an absent status are not: they are what a task looks like
    /// between the caller's move and the outcome of it, and waking on them costs
    /// a turn per transition to learn nothing. A status change alone is not
    /// either — the caller asked for it — except into Done, which is where a
    /// merge landed and dependents may have come free.
    pub fn needs_attention(&self) -> bool {
        if self.escalation_note.is_some() {
            return true;
        }
        match self.status {
            TaskStatus::Done => true,
            // A research session runs in Backlog, so it has a phase of its own;
            // otherwise a Backlog task wants the caller once it can be started.
            TaskStatus::Backlog => self.deps_satisfied || self.phase_needs_attention(),
            TaskStatus::Planning | TaskStatus::Running | TaskStatus::Review => {
                self.phase_needs_attention()
            }
        }
    }

    fn phase_needs_attention(&self) -> bool {
        matches!(
            self.phase_status.as_deref(),
            Some("ready" | "idle" | "blocked" | "exited")
        )
    }
}

/// What one caller has been shown of one board.
///
/// Two maps, because "changed since the caller last saw it" and "changed since
/// the last poll" are different questions, and each misses a case the other
/// catches. Against `reported` alone, an agent nudged out of `idle` that works
/// and goes idle again inside one wait looks unchanged. Against `observed`
/// alone, a task that went `ready` while the caller was busy between two waits
/// is already `ready` at the first poll and looks unchanged too.
#[derive(Debug, Default)]
pub struct BoardView {
    /// The mark each task had when the caller was last shown it.
    reported: HashMap<String, TaskMark>,
    /// The mark each task had at the last poll.
    observed: HashMap<String, TaskMark>,
    /// Tasks that woke a wait while matching `reported` — see [`Self::poll`].
    woken: HashSet<String>,
    /// What the caller was last told about the TUI. `None` until it has been
    /// told anything, which is not a change to wake for.
    tui_connected: Option<bool>,
}

/// The answer to a wait: which tasks to show the caller.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BoardDiff {
    /// Tasks whose mark differs from what the caller was last shown, or that
    /// woke this wait. In board order.
    pub changed: Vec<String>,
    /// Tasks the caller was shown that no longer exist.
    pub removed: Vec<String>,
}

impl BoardView {
    /// Record that the caller was just shown these tasks — by `list_tasks` or
    /// `get_task` — so a wait does not wake to repeat them.
    ///
    /// A partial listing (one status, one task) records only what it showed;
    /// the rest of the board stays unseen, as it is. A task never shown wakes
    /// the next wait at once, which is also what makes a caller's first wait a
    /// baseline of the whole board.
    pub fn mark_seen<'a>(
        &mut self,
        tasks: impl IntoIterator<Item = (&'a str, &'a TaskMark)>,
        tui_connected: bool,
    ) {
        for (id, mark) in tasks {
            self.reported.insert(id.to_string(), mark.clone());
            self.observed.insert(id.to_string(), mark.clone());
            self.woken.remove(id);
        }
        self.tui_connected = Some(tui_connected);
    }

    /// One poll of the whole board. True when the caller should be woken.
    ///
    /// Wakes when a task is in a state that needs the caller *and* the caller
    /// has not seen it there — either it differs from what was reported, or it
    /// moved since the last poll. The second half is what catches a return to
    /// the same state; such a task matches `reported`, so it is remembered in
    /// `woken` to be included in the answer regardless.
    ///
    /// A task already reported in a state that needs attention does not wake
    /// again while it stays there. A caller that chooses to leave a `blocked`
    /// task alone must not be woken for it on every poll.
    pub fn poll(&mut self, board: &[(String, TaskMark)], tui_connected: bool) -> bool {
        let mut wake = false;
        if self.tui_connected.is_some_and(|t| t != tui_connected) {
            wake = true;
        }
        for (id, mark) in board {
            let unseen = self.reported.get(id) != Some(mark);
            let moved = self.observed.get(id) != Some(mark);
            if mark.needs_attention() && (unseen || moved) {
                wake = true;
                if !unseen {
                    self.woken.insert(id.clone());
                }
            }
            // A task the caller has never been shown is news in any state.
            if !self.reported.contains_key(id) {
                wake = true;
            }
            self.observed.insert(id.clone(), mark.clone());
        }
        let present: HashSet<&str> = board.iter().map(|(id, _)| id.as_str()).collect();
        if self
            .reported
            .keys()
            .any(|id| !present.contains(id.as_str()))
        {
            wake = true;
        }
        self.observed.retain(|id, _| present.contains(id.as_str()));
        wake
    }

    /// What to show the caller now, and record that it has been shown.
    ///
    /// Called when a wait answers, whether it woke or timed out: a timeout
    /// still reports the moves into non-actionable states it accumulated, so
    /// the next wait starts from what the caller actually knows.
    pub fn report(&mut self, board: &[(String, TaskMark)], tui_connected: bool) -> BoardDiff {
        let present: HashSet<&str> = board.iter().map(|(id, _)| id.as_str()).collect();
        let mut removed: Vec<String> = self
            .reported
            .keys()
            .filter(|id| !present.contains(id.as_str()))
            .cloned()
            .collect();
        removed.sort();
        for id in &removed {
            self.reported.remove(id);
        }

        let mut changed = Vec::new();
        for (id, mark) in board {
            if self.reported.get(id) != Some(mark) || self.woken.contains(id) {
                changed.push(id.clone());
                self.reported.insert(id.clone(), mark.clone());
            }
        }
        self.woken.clear();
        self.tui_connected = Some(tui_connected);
        BoardDiff { changed, removed }
    }
}
