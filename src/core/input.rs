//! Getting text and keystrokes into a running agent's pane.
//!
//! Shared by the TUI's phase delivery and the web API's `/input`, because the
//! hard-won part is not the sending — it is knowing that an agent's composer
//! may swallow the first Enter, and that a bare `send-keys` resolves anything
//! matching a tmux key name *as that key*. A second implementation of this
//! would rediscover both the slow way.
//!
//! Nothing here decides *what* to send; that belongs to the caller.

use std::sync::Arc;

use crate::tmux::TmuxOperations;

/// Attempts and per-attempt budget for [`submit_message`].
///
/// Smaller than the delivery budget on purpose: by the time this runs the text is
/// known to be in the composer, so this is only absorbing a composer that is still
/// mid-render, not a session that never attached its stdin.
pub const SUBMIT_ATTEMPTS: u32 = 3;

pub const SUBMIT_CONFIRM_POLLS: u32 = 5; // x 200ms = 1s

/// Longest prefix of a message used to confirm it landed on a pane that never
/// went quiet. Short on purpose: a composer wraps and re-indents what it echoes,
/// so a long needle straddles a line break and reads as absent.
pub const DELIVERY_NEEDLE_CHARS: usize = 16;

/// Whitespace-collapsed prefix of `text`, or `None` when there is nothing
/// distinctive enough to look for.
pub fn delivery_needle(text: &str) -> Option<String> {
    let flat: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let needle: String = flat.chars().take(DELIVERY_NEEDLE_CHARS).collect();
    (needle.chars().count() >= 4).then_some(needle)
}

/// Whether `needle` is visible in `pane`, comparing both whitespace-collapsed so
/// a wrap or re-indent in the composer does not hide it.
pub fn pane_shows(pane: &str, needle: &str) -> bool {
    let flat: String = pane.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.contains(needle)
}

/// Lines at the bottom of a pane treated as the composer.
///
/// Sized from the worst real layout, not from the composer alone: while the
/// command picker is open the agent draws its suggestions *below* the composer,
/// and cursor's footer wraps the worktree path over three more lines. That puts
/// the text being submitted eight or more lines off the bottom — a snugger
/// window reads it as already gone and stops pressing Enter after one.
///
/// The cost of erring wide is one extra Enter into a composer that already
/// submitted, which is inert; the cost of erring narrow is a command parked
/// forever.
pub const COMPOSER_TAIL_LINES: usize = 14;

/// Whether the message is still sitting in the composer rather than submitted.
///
/// Only the bottom of the pane is examined: after a submit the text moves up into
/// the scrollback, and finding it *there* is proof it went, not that it stayed.
pub fn composer_holds(pane: &str, needle: &str) -> bool {
    // Trailing blanks first. `capture-pane -p` emits one line per pane *row*, not
    // per rendered line — verified against tmux 3.5a: a 20-row pane holding one
    // word comes back as 20 lines, 19 of them empty. Anchoring the window to the
    // raw end would put it entirely inside that padding whenever the agent's
    // output has not yet filled the pane, find nothing, and stop pressing Enter
    // after one — the very park this exists to catch.
    let mut lines: Vec<&str> = pane.lines().collect();
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(COMPOSER_TAIL_LINES);
    pane_shows(&lines[start..].join("\n"), needle)
}

/// Press Enter until the message actually leaves the composer.
///
/// The check is "the text is gone from the composer", not "the pane changed".
/// A repaint is not a submit: a **bare skill command** — one with no prompt after
/// it, which is what a phase whose command carries no `{task}`/`{task_id}` sends —
/// exactly matches a skill name, so the composer's command picker opens *on the
/// paste*. Enter is then consumed by the picker ("Press enter to insert"), which
/// inserts the command and repaints. The old change-detector read that repaint as
/// success and returned, leaving the command parked in the composer forever.
///
/// Measured against codex-cli 0.144.5 and cursor-agent 2026.08.25: both open the
/// picker on a pasted bare command, both need the second Enter, and both run the
/// skill once it arrives.
///
/// Falls back to the change-detector when the text is too short to track, and is
/// bounded either way — an agent that never submits costs `SUBMIT_ATTEMPTS`
/// keypresses, not an unbounded stream.
///
/// Known cost: agents echo the submitted message into the transcript just above
/// the composer, so a *successful* submit can leave the needle inside the window
/// and spend the remaining attempts. Those Enters land in an empty composer,
/// which is inert — except against a dialog that renders mid-submit, where a bare
/// Enter picks the highlighted option. `answer_session_dialogs` is what answers
/// those, and it runs on the refresh loop rather than here.
pub fn submit_message(tmux_ops: &Arc<dyn TmuxOperations>, target: &str, text: &str) {
    let needle = delivery_needle(text);
    for _ in 0..SUBMIT_ATTEMPTS {
        let before = tmux_ops.capture_pane(target).unwrap_or_default();
        let _ = tmux_ops.send_key(target, "Enter");
        for _ in 0..SUBMIT_CONFIRM_POLLS {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let Ok(now) = tmux_ops.capture_pane(target) else {
                continue;
            };
            match needle.as_deref() {
                Some(n) if !composer_holds(&now, n) => return,
                Some(_) => {}
                // Nothing distinctive enough to look for; the pane moving is all
                // there is to go on.
                None if now != before => return,
                None => {}
            }
        }
    }
}

// ── user input ──────────────────────────────────────────────────────────

/// A keystroke a person asked for, as opposed to text they typed.
///
/// The distinction is not cosmetic. `send-keys` without `-l` resolves an
/// argument matching a key name *as that key*: a message of `"Space"` arrives
/// as `0x20` and a leading `;` separates tmux commands. So text must go out
/// literally and keys must not — which is why the wire format is
/// `{text?, key?}` and never one field a server has to guess about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneKey {
    Enter,
    Escape,
    CtrlC,
    Up,
    Down,
    Tab,
    /// A single character sent as a key rather than as text — the digits and
    /// letters an agent's dialogs ask for (`1`, `2`, `y`, `n`).
    Char(char),
}

impl PaneKey {
    /// Parse the name a client may send. Deliberately a closed set: forwarding
    /// arbitrary key names would let a request send `C-d` (an EOF that ends the
    /// session) or `C-u` (which kills the composer line).
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "Enter" => PaneKey::Enter,
            "Escape" | "Esc" => PaneKey::Escape,
            "C-c" | "CtrlC" => PaneKey::CtrlC,
            "Up" => PaneKey::Up,
            "Down" => PaneKey::Down,
            "Tab" => PaneKey::Tab,
            other => {
                let mut chars = other.chars();
                match (chars.next(), chars.next()) {
                    (Some(c), None) if c.is_ascii_alphanumeric() => PaneKey::Char(c),
                    _ => return None,
                }
            }
        })
    }

    /// The tmux key name to send.
    pub fn tmux_name(&self) -> String {
        match self {
            PaneKey::Enter => "Enter".to_string(),
            PaneKey::Escape => "Escape".to_string(),
            PaneKey::CtrlC => "C-c".to_string(),
            PaneKey::Up => "Up".to_string(),
            PaneKey::Down => "Down".to_string(),
            PaneKey::Tab => "Tab".to_string(),
            PaneKey::Char(c) => c.to_string(),
        }
    }

    /// Whether this is a bare character, which must go out with `-l` so tmux
    /// does not resolve it as a key name.
    pub fn is_literal_char(&self) -> bool {
        matches!(self, PaneKey::Char(_))
    }
}

/// Send one keystroke.
///
/// A `Char` goes out literally; everything else is a real key name. Sending a
/// digit without `-l` mostly works and then does not: tmux resolves `0`..`9` as
/// themselves, but the same code path with a letter like `n` is one rename away
/// from colliding with a key name, and the literal form is correct for both.
pub fn send_user_key(tmux_ops: &Arc<dyn TmuxOperations>, target: &str, key: PaneKey) -> bool {
    let name = key.tmux_name();
    let sent = if key.is_literal_char() {
        tmux_ops.send_text(target, &name)
    } else {
        tmux_ops.send_key(target, &name)
    };
    sent.is_ok()
}

/// Put a person's message into the agent's composer and submit it.
///
/// `paste` selects the delivery lane. An Ink-class composer (gemini, codex,
/// cursor, antigravity, pi) takes a bracketed paste as literal text but leaves
/// a combined text+Enter `send_keys` sitting unsent, so those agents must paste
/// and then submit separately. The rest take the generic path.
///
/// Submission is [`submit_message`] rather than one Enter, for the reason that
/// function documents: a composer's picker can eat the first one.
pub fn send_user_text(
    tmux_ops: &Arc<dyn TmuxOperations>,
    target: &str,
    text: &str,
    paste: bool,
) -> bool {
    if text.is_empty() {
        return false;
    }
    let delivered = if paste {
        tmux_ops.paste_text(target, text).is_ok()
    } else {
        tmux_ops.send_text(target, text).is_ok()
    };
    if !delivered {
        return false;
    }
    submit_message(tmux_ops, target, text);
    true
}

/// Whether `agent` needs the paste-then-submit lane rather than a plain send.
///
/// The same list `send_skill_and_prompt` uses for its combined-send branch;
/// see the agent table in CLAUDE.md for what was measured on each.
pub fn agent_needs_paste(agent: &str) -> bool {
    matches!(agent, "gemini" | "codex" | "cursor" | "antigravity" | "pi")
}
