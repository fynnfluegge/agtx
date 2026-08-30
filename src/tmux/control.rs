//! Persistent tmux **control-mode** client.
//!
//! Every `send-keys` agtx runs through [`RealTmuxOps`](super::RealTmuxOps) starts
//! a `tmux` process and waits for it. Measured against tmux 3.5a on macOS that
//! is **~25 ms per key** — the dominant term in the delay between pressing a key
//! in a task popup and seeing it echoed. A control-mode client is one long-lived
//! process that takes commands as lines on stdin, so the same `send-keys` costs
//! ~0.05 ms round-trip and ~0.001 ms if nothing waits for the reply.
//!
//! ```text
//! tmux -L agtx -C attach-session -t <session> -f ignore-size,no-output
//!      │ stdin: one tmux command per line
//!      │ stdout: %begin/%end/%error blocks + %notifications
//! ```
//!
//! Three things about that invocation are load-bearing, and all three were
//! measured before this file existed (see `docs/planning/tmux-control-mode-input.md`):
//!
//! - **`ignore-size`.** A control client can otherwise take part in tmux's client
//!   size calculation and shrink the very panes agtx sizes by hand for the popup.
//!   On 3.5a a control client with no size set turned out to be size-neutral
//!   already, but the flag makes that a guarantee rather than an observation.
//! - **`no-output`.** Without it every byte an agent paints is mirrored down our
//!   stdout as `%output`. The popup gets its content from `capture-pane`, so that
//!   is pure cost — a busy agent would push megabytes through the reader thread.
//! - **The session is only an attach point.** Commands carry their own
//!   `session:window` target and are executed server-wide, so one client drives
//!   every window on the `agtx` server. Verified: a client attached to session
//!   `one` typed into `two`.
//!
//! Commands are *not* shell words. tmux parses them itself, so the quoting is
//! tmux's, not POSIX's — see [`tmux_quote`].

use anyhow::{Context, Result};
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// Payload line the handshake command echoes back, proving the connection round
/// trips before agtx routes a keystroke through it.
const READY_SENTINEL: &str = "agtx-control-ready";

/// How long to wait for that sentinel before giving up on the connection.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

/// Wrap `text` as one tmux **double-quoted** argument.
///
/// This is not [`single_quote`](super::single_quote): that one quotes a word for
/// `sh`, and control mode never involves a shell. tmux applies its own
/// replacements *inside* double quotes (tmux(1), PARSING SYNTAX), and every one
/// of them has to be defused:
///
/// | In the text | Encoded as | Why |
/// |---|---|---|
/// | `\` | `\\` | escape lead-in, and a trailing one is a line continuation |
/// | `"` | `\"` | ends the argument |
/// | `$` | `\$` | `$VAR` is replaced from the global environment |
/// | `#` | `\#` | starts a comment, and `#{…}` is a format |
/// | `~` | `\~` | a leading `~` expands to a home directory |
/// | LF | `\n` | commands are newline-terminated; a raw LF splits the command |
/// | CR, tab | `\r`, `\t` | same class, and clearer than an octal escape |
/// | other C0, DEL | `\ooo` | three-digit octal, as tmux documents |
///
/// Single quotes are the tempting alternative — tmux performs no replacements
/// inside them — but they cannot contain a `'`, and task panes are typed into by
/// humans writing English. Double quotes can encode everything.
///
/// Everything else, UTF-8 included, is passed through untouched: verified that
/// `한국어` and `😀` arrive byte-identical.
pub fn tmux_quote(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            '\\' | '"' | '$' | '#' | '~' => {
                out.push('\\');
                out.push(ch);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\{:03o}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One unit of control-mode output.
///
/// tmux frames every command's result between `%begin` and `%end`/`%error`, and
/// emits unsolicited `%notifications` outside those blocks. The distinction
/// matters: a payload line may itself start with `%`, so "starts with a percent
/// sign" is not what tells a notification from output — being outside a block is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// `%begin <time> <cmd> <flags>`
    Begin { cmd: u64 },
    /// `%end <time> <cmd> <flags>` — the command succeeded.
    End { cmd: u64 },
    /// `%error <time> <cmd> <flags>` — the command failed; the payload says why.
    Error { cmd: u64 },
    /// A line inside a `%begin`/`%end` block.
    Payload(String),
    /// An unsolicited notification (`%exit`, `%window-add @2`, …).
    Notify(String),
}

/// Incremental parser for the control-mode stream.
///
/// A read from a pipe is not a line and not a frame: it may split one line
/// across two reads or carry twenty. The parser buffers bytes and yields whole
/// frames, so the reader thread never has to care where the read boundaries fell.
#[derive(Default)]
pub struct FrameParser {
    buf: Vec<u8>,
    in_block: bool,
}

impl FrameParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed bytes straight from a `read` call.
    pub fn push(&mut self, bytes: &[u8]) {
        self.buf.extend_from_slice(bytes);
    }

    /// Pop the next complete frame, or `None` while the buffer holds only a
    /// partial line.
    pub fn next_frame(&mut self) -> Option<Frame> {
        let idx = self.buf.iter().position(|b| *b == b'\n')?;
        let line: Vec<u8> = self.buf.drain(..=idx).collect();
        let line = String::from_utf8_lossy(&line[..line.len() - 1])
            .trim_end_matches('\r')
            .to_string();
        Some(self.classify(line))
    }

    fn classify(&mut self, line: String) -> Frame {
        // A `%begin` block's payload is arbitrary text, so it is classified by
        // position, never by a leading `%`.
        if let Some(cmd) = frame_cmd(&line, "%begin") {
            self.in_block = true;
            return Frame::Begin { cmd };
        }
        if self.in_block {
            if let Some(cmd) = frame_cmd(&line, "%end") {
                self.in_block = false;
                return Frame::End { cmd };
            }
            if let Some(cmd) = frame_cmd(&line, "%error") {
                self.in_block = false;
                return Frame::Error { cmd };
            }
            return Frame::Payload(line);
        }
        Frame::Notify(line)
    }
}

/// `%begin 1788074601 521 0` → `Some(521)`. A malformed or missing command
/// number still frames the block; it is only used for logging.
fn frame_cmd(line: &str, tag: &str) -> Option<u64> {
    let rest = line.strip_prefix(tag)?;
    if !rest.is_empty() && !rest.starts_with(' ') {
        return None; // `%endsomething` is a different notification
    }
    Some(
        rest.split_whitespace()
            .nth(1)
            .and_then(|n| n.parse().ok())
            .unwrap_or(0),
    )
}

/// State the reader thread publishes and the writer waits on.
#[derive(Debug, Default)]
struct ClientState {
    /// Commands whose `%end`/`%error` has been seen. Commands complete in the
    /// order they were issued, so this doubles as a barrier counter.
    completed: u64,
    /// The handshake sentinel came back.
    ready: bool,
    /// `%exit` or EOF — the tmux server or this client is gone.
    alive: bool,
    /// Last `%error` payload, for logs. Never contains pane input: tmux errors
    /// name the target, not the keys.
    last_error: Option<String>,
    errors: u64,
}

#[derive(Debug, Default)]
struct Shared {
    state: Mutex<ClientState>,
    cv: Condvar,
}

impl Shared {
    fn signal(&self, f: impl FnOnce(&mut ClientState)) {
        if let Ok(mut st) = self.state.lock() {
            f(&mut st);
        }
        self.cv.notify_all();
    }
}

/// A live `tmux -C` connection.
///
/// Owned exclusively by the input broker thread: `stdin` is a single writer by
/// construction, which is what makes command ordering a property of the type
/// rather than a convention.
pub struct ControlClient {
    child: Child,
    stdin: ChildStdin,
    shared: Arc<Shared>,
    /// Commands this client has written.
    issued: u64,
    /// `completed - issued` at the end of the handshake. Attaching produces one
    /// framed block of its own before we send anything, so a raw
    /// `completed >= issued` comparison would let a barrier pass one command
    /// early.
    offset: u64,
    generation: u64,
}

impl ControlClient {
    /// Attach a control client to `session` on the `-L server` tmux server.
    ///
    /// Returns only once the connection has round-tripped a command, so a
    /// caller that gets a `ControlClient` has one that works.
    pub fn connect(server: &str, session: &str, generation: u64) -> Result<Self> {
        let mut child = Command::new("tmux")
            .args(["-L", server, "-C", "attach-session", "-t", session])
            // See the module docs: size-neutral, and no pane output mirrored at us.
            .args(["-f", "ignore-size,no-output"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .context("failed to start tmux control-mode client")?;

        let stdin = child
            .stdin
            .take()
            .context("tmux control client has no stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("tmux control client has no stdout")?;

        let shared = Arc::new(Shared {
            state: Mutex::new(ClientState {
                alive: true,
                ..Default::default()
            }),
            cv: Condvar::new(),
        });

        let reader_shared = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(format!("agtx-tmux-control-{generation}"))
            .spawn(move || read_loop(stdout, reader_shared))
            .context("failed to start the control-mode reader thread")?;

        let mut client = Self {
            child,
            stdin,
            shared,
            issued: 0,
            offset: 0,
            generation,
        };

        client.handshake().inspect_err(|_| {
            client.kill();
        })?;
        Ok(client)
    }

    /// Prove the connection works, and learn how many framed blocks tmux emitted
    /// on its own before we said anything.
    fn handshake(&mut self) -> Result<()> {
        self.write_command(&format!(
            "display-message -p {}",
            tmux_quote(READY_SENTINEL)
        ))?;
        let deadline = Instant::now() + HANDSHAKE_TIMEOUT;
        let mut st = self
            .shared
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("control-mode state poisoned"))?;
        while !st.ready && st.alive {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                anyhow::bail!("tmux control client did not answer within {HANDSHAKE_TIMEOUT:?}");
            }
            let (next, _) = self
                .shared
                .cv
                .wait_timeout(st, remaining)
                .map_err(|_| anyhow::anyhow!("control-mode state poisoned"))?;
            st = next;
        }
        if !st.ready {
            anyhow::bail!("tmux control client exited during handshake");
        }
        self.offset = st.completed.saturating_sub(self.issued);
        Ok(())
    }

    /// Write one command. Returns as soon as the bytes are in the pipe — the
    /// reply is not waited for, because the connection is FIFO and ordering,
    /// not acknowledgement, is what callers need.
    ///
    /// An `Err` here is **ambiguous**: a partial write may have reached tmux.
    /// Callers must not retry the command on another backend; see the broker.
    pub fn write_command(&mut self, cmd: &str) -> Result<()> {
        self.stdin.write_all(cmd.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        self.issued += 1;
        Ok(())
    }

    /// Block until every command written so far has been executed by the server.
    ///
    /// Used before handing work to the subprocess path, which travels a
    /// *different* socket and could otherwise overtake commands still queued
    /// here. Costs one round trip (~0.05 ms measured), not one per command.
    pub fn barrier(&mut self, timeout: Duration) -> bool {
        let want = self.issued + self.offset;
        let deadline = Instant::now() + timeout;
        let Ok(mut st) = self.shared.state.lock() else {
            return false;
        };
        while st.completed < want && st.alive {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            match self.shared.cv.wait_timeout(st, remaining) {
                Ok((next, _)) => st = next,
                Err(_) => return false,
            }
        }
        st.completed >= want
    }

    pub fn alive(&self) -> bool {
        self.shared.state.lock().map(|st| st.alive).unwrap_or(false)
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Number of `%error` replies and the most recent message, for logging.
    pub fn error_report(&self) -> (u64, Option<String>) {
        self.shared
            .state
            .lock()
            .map(|st| (st.errors, st.last_error.clone()))
            .unwrap_or((0, None))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Close stdin and reap the client. tmux exits on EOF, so this is the
    /// graceful path; the kill is only for a client that ignores it.
    pub fn shutdown(self) {
        // Destructured rather than `Option`-wrapped: dropping stdin is what asks
        // tmux to exit, and making the field optional would put an unwrap on
        // every write instead.
        let ControlClient {
            mut child, stdin, ..
        } = self;
        drop(stdin);
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => break,
            }
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

fn read_loop(mut stdout: std::process::ChildStdout, shared: Arc<Shared>) {
    let mut parser = FrameParser::new();
    let mut buf = [0u8; 8192];
    let mut block_payload: Vec<String> = Vec::new();
    loop {
        let n = match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        parser.push(&buf[..n]);
        while let Some(frame) = parser.next_frame() {
            match frame {
                Frame::Begin { .. } => block_payload.clear(),
                Frame::End { .. } => {
                    let ready = block_payload.iter().any(|l| l == READY_SENTINEL);
                    shared.signal(|st| {
                        st.completed += 1;
                        if ready {
                            st.ready = true;
                        }
                    });
                    block_payload.clear();
                }
                Frame::Error { cmd } => {
                    let msg = block_payload.join("; ");
                    tracing::debug!(cmd, error = %msg, "tmux control command failed");
                    shared.signal(|st| {
                        st.completed += 1;
                        st.errors += 1;
                        st.last_error = Some(msg);
                    });
                    block_payload.clear();
                }
                Frame::Payload(line) => block_payload.push(line),
                Frame::Notify(line) => {
                    if line.starts_with("%exit") {
                        shared.signal(|st| st.alive = false);
                    }
                }
            }
        }
    }
    shared.signal(|st| st.alive = false);
}

#[cfg(test)]
#[path = "control_tests.rs"]
mod control_tests;
