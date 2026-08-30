//! Non-blocking pane input: the path a keystroke takes from the popup to tmux.
//!
//! ```text
//! crossterm key event
//!         │
//!         ▼
//!  PaneInputSink::send      (TUI thread — enqueue only, never waits for tmux)
//!         │
//!   bounded channel
//!         │
//!         ▼
//!   one broker thread ── coalesces adjacent text, flushes before every key
//!         │
//!         ├─► control-mode client   (persistent process, ~0.05 ms/command)
//!         └─► subprocess fallback   (`tmux send-keys`, ~25 ms/command)
//! ```
//!
//! The broker is the **single ordering authority** for popup input. Nothing else
//! may write keys to a task pane while a popup is open: `Text("abc")`,
//! `Key("Left")`, `Text("X")`, `Key("Enter")` must reach tmux in that order, and
//! they do because one thread executes them in receive order — across batching,
//! across a backend switch, and across a reconnect.
//!
//! Coalescing is what makes fast typing cheap: adjacent characters for the same
//! target become one `send-keys -l`. It is bounded by a few milliseconds and by
//! a byte cap, and **never** delays a key that is not literal text, because a
//! delayed Enter is a visibly broken editor.

use anyhow::Result;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::control::{tmux_quote, ControlClient};
use super::TmuxOperations;

/// One request to put something into a pane.
///
/// Typed rather than preformatted tmux commands: the broker has to be able to
/// tell literal text from a key name to decide what may be batched, and a
/// formatted `send-keys` string has already thrown that distinction away.
#[derive(Debug, Clone)]
pub enum PaneInput {
    /// Literal text (`send-keys -l`). Never goes through tmux's key-name lookup,
    /// so text that spells `Space` or `Up` stays text.
    Text { target: String, text: String },
    /// A tmux **key name** (`Enter`, `Escape`, `C-c`, `M-b`, `F5`).
    Key { target: String, key: String },
    /// A whole block of text, bracketed-paste semantics.
    Paste { target: String, text: String },
    /// Deliver everything buffered now. Sent when a popup closes or changes
    /// target, so queued text can never land in a different task's pane.
    Flush,
    /// Flush and acknowledge only after the active backend has executed every
    /// preceding command. This is the ownership boundary used before the TUI
    /// performs a synchronous tmux operation outside the broker.
    Barrier { ack: std::sync::mpsc::Sender<bool> },
    /// Flush, then stop the broker.
    Shutdown,
}

impl PaneInput {
    /// The pane this request is for, if it names one.
    pub fn target(&self) -> Option<&str> {
        match self {
            PaneInput::Text { target, .. }
            | PaneInput::Key { target, .. }
            | PaneInput::Paste { target, .. } => Some(target),
            PaneInput::Flush | PaneInput::Barrier { .. } | PaneInput::Shutdown => None,
        }
    }
}

impl PartialEq for PaneInput {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Text { target: a, text: b }, Self::Text { target: c, text: d }) => {
                a == c && b == d
            }
            (Self::Key { target: a, key: b }, Self::Key { target: c, key: d }) => a == c && b == d,
            (Self::Paste { target: a, text: b }, Self::Paste { target: c, text: d }) => {
                a == c && b == d
            }
            (Self::Flush, Self::Flush)
            | (Self::Barrier { .. }, Self::Barrier { .. })
            | (Self::Shutdown, Self::Shutdown) => true,
            _ => false,
        }
    }
}

impl Eq for PaneInput {}

/// Why an enqueue failed. Named, because the caller's recovery differs per case
/// and "the input was dropped" must never be silent.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum InputError {
    /// The broker is behind and the bounded queue is full.
    #[error("pane input queue is full")]
    QueueFull,
    /// The broker thread is gone.
    #[error("pane input broker is not running")]
    Disconnected,
    /// An acknowledged flush was not answered in time. The queued prefix is
    /// still being delivered — it is *late*, not lost, which is why this is not
    /// `Disconnected`: that variant tells the user to restart agtx, and here
    /// there is nothing wrong to restart.
    #[error("pane input did not drain in time")]
    Timeout,
}

/// Where popup input is handed off. Implementations must not block the caller.
pub trait PaneInputSink: Send + Sync {
    fn send(&self, input: PaneInput) -> std::result::Result<(), InputError>;

    fn text(&self, target: &str, text: &str) -> std::result::Result<(), InputError> {
        self.send(PaneInput::Text {
            target: target.to_string(),
            text: text.to_string(),
        })
    }

    fn key(&self, target: &str, key: &str) -> std::result::Result<(), InputError> {
        self.send(PaneInput::Key {
            target: target.to_string(),
            key: key.to_string(),
        })
    }

    fn paste(&self, target: &str, text: &str) -> std::result::Result<(), InputError> {
        self.send(PaneInput::Paste {
            target: target.to_string(),
            text: text.to_string(),
        })
    }

    fn flush(&self) -> std::result::Result<(), InputError> {
        self.send(PaneInput::Flush)
    }

    /// Flush and wait until tmux has executed the queued prefix.
    ///
    /// The default keeps test/embedding sinks simple. The production broker
    /// overrides this with an acknowledged barrier.
    fn flush_sync(&self) -> std::result::Result<(), InputError> {
        self.flush()
    }

    /// Deliver what is queued and stop. Called when agtx exits, so the last
    /// characters typed into a pane are not lost with the process.
    fn shutdown(&self) {
        let _ = self.flush();
    }

    /// Point future control-mode connections at a different session.
    ///
    /// agtx switches project in place, and a session that no longer exists is
    /// one the control client cannot attach to. An **already open** connection
    /// is left alone on purpose: commands carry their own target and are
    /// executed server-wide, so a client attached to the previous project still
    /// delivers correctly. This only matters for the next connect.
    fn set_session(&self, _session: &str) {}
}

/// What the broker writes through. Two implementations ship: the persistent
/// control client and the one-process-per-command fallback.
pub(crate) trait PaneBackend: Send {
    fn text(&mut self, target: &str, text: &str) -> Result<()>;
    fn key(&mut self, target: &str, key: &str) -> Result<()>;
    fn paste(&mut self, target: &str, text: &str) -> Result<()>;
    /// Block until everything written so far has actually been executed.
    /// A no-op for a backend that is synchronous anyway.
    fn barrier(&mut self) -> bool {
        true
    }
    fn healthy(&self) -> bool {
        true
    }
    fn label(&self) -> &'static str;
}

/// The historical path: one `tmux` process per command, via [`TmuxOperations`]
/// so test mocks keep working unchanged.
pub(crate) struct SubprocessBackend {
    ops: Arc<dyn TmuxOperations>,
}

impl SubprocessBackend {
    pub(crate) fn new(ops: Arc<dyn TmuxOperations>) -> Self {
        Self { ops }
    }
}

impl PaneBackend for SubprocessBackend {
    fn text(&mut self, target: &str, text: &str) -> Result<()> {
        self.ops.send_text(target, text)
    }
    fn key(&mut self, target: &str, key: &str) -> Result<()> {
        self.ops.send_key(target, key)
    }
    fn paste(&mut self, target: &str, text: &str) -> Result<()> {
        self.ops.paste_text(target, text)
    }
    fn label(&self) -> &'static str {
        "subprocess"
    }
}

/// The persistent path. Keys and text go down the control connection; a paste
/// still goes through `load-buffer`, which needs a pipe rather than a command
/// argument, so it is delegated — behind a barrier, because the two travel
/// different sockets and only the barrier keeps them ordered.
pub(crate) struct ControlBackend {
    /// `Option` only so [`Drop`] can move the client out and close it. A dropped
    /// `ControlClient` would otherwise leave an attached `tmux -C` process
    /// running for the life of the server — one per reconnect.
    client: Option<ControlClient>,
    paste_via: SubprocessBackend,
}

impl ControlBackend {
    pub(crate) fn new(client: ControlClient, ops: Arc<dyn TmuxOperations>) -> Self {
        Self {
            client: Some(client),
            paste_via: SubprocessBackend::new(ops),
        }
    }

    fn client(&mut self) -> Result<&mut ControlClient> {
        self.client
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("control client already closed"))
    }
}

impl Drop for ControlBackend {
    fn drop(&mut self) {
        if let Some(client) = self.client.take() {
            client.shutdown();
        }
    }
}

/// How long a barrier waits before giving up and letting the paste race. Chosen
/// well above the measured ~0.05 ms round trip: reaching it means tmux is wedged,
/// and blocking the broker further would not help.
const BARRIER_TIMEOUT: Duration = Duration::from_millis(250);

impl PaneBackend for ControlBackend {
    fn text(&mut self, target: &str, text: &str) -> Result<()> {
        let cmd = format!(
            "send-keys -t {} -l -- {}",
            tmux_quote(target),
            tmux_quote(text)
        );
        self.client()?.write_command(&cmd)
    }
    fn key(&mut self, target: &str, key: &str) -> Result<()> {
        // No `-l`: this is exactly the key-name lookup `send_key` documents, and
        // the reason a key and literal text cannot share one request type.
        let cmd = format!("send-keys -t {} -- {}", tmux_quote(target), tmux_quote(key));
        self.client()?.write_command(&cmd)
    }
    fn paste(&mut self, target: &str, text: &str) -> Result<()> {
        // `load-buffer` needs a pipe, not a command argument, so a paste stays on
        // the subprocess path. It travels a *different* socket, so without the
        // barrier it could overtake text still queued on this connection.
        let _ = self.barrier();
        // Reported, never propagated. The broker reads an `Err` from this
        // backend as an *ambiguous control write* and tears the connection
        // down — but nothing was written to it here, and the request already
        // ran on the fallback it would be moved to. Propagating would kill a
        // healthy connection over an unrelated failure.
        if let Err(e) = self.paste_via.paste(target, text) {
            tracing::warn!(error = %e, "pane paste failed on the subprocess backend");
        }
        Ok(())
    }
    fn barrier(&mut self) -> bool {
        if let Some(client) = self.client.as_mut() {
            let completed = client.barrier(BARRIER_TIMEOUT);
            if !completed {
                tracing::debug!("tmux control barrier timed out");
            }
            completed
        } else {
            false
        }
    }
    fn healthy(&self) -> bool {
        self.client.as_ref().map(|c| c.alive()).unwrap_or(false)
    }
    fn label(&self) -> &'static str {
        "control"
    }
}

/// Why the broker decided to deliver its buffered text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FlushReason {
    /// The batching window elapsed.
    Window,
    /// A non-text request came next.
    BeforeKey,
    /// The next text is for a different pane.
    TargetChange,
    /// The buffer hit its byte cap.
    Full,
    /// A `Flush` request.
    Explicit,
    /// The broker is stopping.
    Shutdown,
}

/// How long adjacent characters may wait to be combined.
///
/// Two milliseconds is short enough to be invisible next to the ~16 ms a typical
/// key repeat takes and the 50 ms pane refresh, and long enough to sweep up a
/// burst of pasted-as-keystrokes characters. It is the plan's starting value;
/// raising it trades echo latency for fewer commands, which only pays on the
/// subprocess backend.
pub const DEFAULT_BATCH_WINDOW: Duration = Duration::from_millis(2);

/// Byte cap on one coalesced `send-keys`. Well under any command-length limit,
/// and it bounds how much text a single failed write can lose.
const MAX_BATCH_BYTES: usize = 4096;

/// Bounded queue depth. Deep enough that a human cannot fill it (it is ~40
/// seconds of typing at 25 keys/s) and shallow enough to bound memory if tmux
/// stops consuming.
pub const DEFAULT_QUEUE_CAPACITY: usize = 1024;

/// Reconnect backoff bounds for the control client.
const RECONNECT_MIN: Duration = Duration::from_millis(250);
const RECONNECT_MAX: Duration = Duration::from_secs(5);

/// How long an acknowledged flush waits for the broker to answer.
///
/// **Not** a second copy of [`BARRIER_TIMEOUT`]. The barrier sits *behind the
/// queue*, so this wait is dominated by draining whatever is already in it — and
/// on the subprocess backend every queued command is a ~25 ms process, so a
/// barrier-sized budget expires mid-drain and hands back a guarantee that was
/// not kept. Measured: 20 queued keys at 25 ms each returned an error after
/// 300 ms with half of them still in flight.
///
/// So this is the "the broker is wedged" guard instead. The work it waits on is
/// the user's own keystrokes, so in practice it returns in microseconds; the cap
/// only exists so a hung tmux cannot freeze the TUI thread forever.
const FLUSH_SYNC_TIMEOUT: Duration = Duration::from_secs(2);

/// Bounds on quitting: how long to keep trying to enqueue the stop request, and
/// how long to wait for the broker's final flush afterwards.
const SHUTDOWN_ENQUEUE_WAIT: Duration = Duration::from_millis(200);
const SHUTDOWN_DRAIN_WAIT: Duration = Duration::from_millis(500);

/// How to build the input pipeline.
#[derive(Debug, Clone)]
pub struct InputConfig {
    /// tmux server name (`-L`).
    pub server: String,
    /// Session the control client attaches to. Commands still carry their own
    /// targets; this is only an attach point.
    pub session: String,
    /// Try the persistent control-mode backend. On by default; a failure to
    /// connect, or a connection lost later, falls back to the subprocess
    /// backend on its own. See `control_mode_enabled` for the escape hatch.
    pub control_mode: bool,
    pub batch_window: Duration,
    pub capacity: usize,
}

impl InputConfig {
    pub fn new(server: impl Into<String>, session: impl Into<String>) -> Self {
        Self {
            server: server.into(),
            session: session.into(),
            control_mode: true,
            batch_window: DEFAULT_BATCH_WINDOW,
            capacity: DEFAULT_QUEUE_CAPACITY,
        }
    }
}

/// Handle held by the TUI. `send` is an enqueue and nothing else.
pub struct BrokerSink {
    tx: SyncSender<PaneInput>,
    depth: Arc<AtomicUsize>,
    high_water: Arc<AtomicUsize>,
    /// Read by the control factory at every connect, so a project switch is
    /// picked up without restarting the broker.
    session: Arc<Mutex<String>>,
    /// Set by the broker as the last thing it does. Lets shutdown wait for the
    /// final flush *without* an unbounded `join` — see below.
    finished: Arc<AtomicBool>,
    join: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl BrokerSink {
    /// Queue depth right now, and the deepest it has ever been. Diagnostics only.
    pub fn depth(&self) -> (usize, usize) {
        (
            self.depth.load(Ordering::Relaxed),
            self.high_water.load(Ordering::Relaxed),
        )
    }
}

impl PaneInputSink for BrokerSink {
    fn set_session(&self, session: &str) {
        if let Ok(mut current) = self.session.lock() {
            if *current != session {
                tracing::debug!(session, "control-mode attach point changed");
                *current = session.to_string();
            }
        }
    }

    /// Flush, stop the broker thread, and wait for it — but never forever.
    ///
    /// Quitting must not be able to hang. The broker can be blocked inside a
    /// `tmux` subprocess that is not coming back, and both a blocking `send` and
    /// a plain `join` would then wait for it with no bound. So both steps are
    /// bounded, and if the broker misses its window the thread is simply left to
    /// the exiting process. Safe to call twice.
    fn shutdown(&self) {
        let deadline = Instant::now() + SHUTDOWN_ENQUEUE_WAIT;
        let mut queued = false;
        while Instant::now() < deadline {
            self.depth.fetch_add(1, Ordering::Relaxed);
            match self.tx.try_send(PaneInput::Shutdown) {
                Ok(()) => {
                    queued = true;
                    break;
                }
                // Backed up: give the broker a moment to drain, since what is
                // queued is the user's last keystrokes.
                Err(TrySendError::Full(_)) => {
                    self.depth.fetch_sub(1, Ordering::Relaxed);
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.depth.fetch_sub(1, Ordering::Relaxed);
                    queued = true;
                    break;
                }
            }
        }
        if !queued {
            tracing::warn!("pane input broker did not accept shutdown; leaving it to exit");
            return;
        }
        let deadline = Instant::now() + SHUTDOWN_DRAIN_WAIT;
        while !self.finished.load(Ordering::Relaxed) {
            if Instant::now() >= deadline {
                tracing::warn!("pane input broker did not finish draining in time");
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if let Ok(mut join) = self.join.lock() {
            if let Some(handle) = join.take() {
                let _ = handle.join();
            }
        }
    }

    fn flush_sync(&self) -> std::result::Result<(), InputError> {
        let (ack, done) = std::sync::mpsc::channel();
        self.send(PaneInput::Barrier { ack })?;
        match done.recv_timeout(FLUSH_SYNC_TIMEOUT) {
            Ok(true) => Ok(()),
            // The control barrier gave up; the broker is alive and answered.
            Ok(false) => Err(InputError::Timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Err(InputError::Timeout),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(InputError::Disconnected),
        }
    }

    fn send(&self, input: PaneInput) -> std::result::Result<(), InputError> {
        // Counted *before* the send, and rolled back if it fails. The other
        // order is a race: the broker can receive and decrement between the
        // successful `try_send` and this `fetch_add`, taking the counter below
        // zero — which on a `usize` is not a small error but `usize::MAX`.
        // Over-counting for a few nanoseconds is the harmless direction.
        let depth = self.depth.fetch_add(1, Ordering::Relaxed) + 1;
        self.high_water.fetch_max(depth, Ordering::Relaxed);
        match self.tx.try_send(input) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                self.depth.fetch_sub(1, Ordering::Relaxed);
                Err(InputError::QueueFull)
            }
            Err(TrySendError::Disconnected(_)) => {
                self.depth.fetch_sub(1, Ordering::Relaxed);
                Err(InputError::Disconnected)
            }
        }
    }
}

/// Start the broker thread and return the sink the TUI enqueues into.
pub fn spawn(config: InputConfig, ops: Arc<dyn TmuxOperations>) -> Arc<BrokerSink> {
    let (tx, rx) = std::sync::mpsc::sync_channel(config.capacity);
    let depth = Arc::new(AtomicUsize::new(0));
    let high_water = Arc::new(AtomicUsize::new(0));
    let session = Arc::new(Mutex::new(config.session.clone()));
    let finished = Arc::new(AtomicBool::new(false));

    let control_factory: Option<ControlFactory> = if config.control_mode {
        let server = config.server.clone();
        let session = Arc::clone(&session);
        let ops = Arc::clone(&ops);
        Some(Box::new(move |generation| {
            let attach = session
                .lock()
                .map(|s| s.clone())
                .map_err(|_| anyhow::anyhow!("attach session poisoned"))?;
            let client = ControlClient::connect(&server, &attach, generation)?;
            Ok(Box::new(ControlBackend::new(client, Arc::clone(&ops))) as Box<dyn PaneBackend>)
        }))
    } else {
        None
    };

    let broker = Broker {
        rx,
        depth: Arc::clone(&depth),
        batch_window: config.batch_window,
        fallback: Box::new(SubprocessBackend::new(ops)),
        control: None,
        control_factory,
        generation: 0,
        next_attempt: Some(Instant::now()),
        backoff: RECONNECT_MIN,
        pending: None,
        deadline: None,
        fallbacks: 0,
        finished: Arc::clone(&finished),
    };

    let join = std::thread::Builder::new()
        .name("agtx-pane-input".to_string())
        .spawn(move || broker.run())
        .ok();
    if join.is_none() {
        // Nothing will ever set `finished`, and `shutdown` would then wait out
        // its whole drain budget on every quit. Mark it done now; the sink's
        // sends fail with `Disconnected`, which the TUI already surfaces.
        tracing::error!("failed to start the pane input broker thread");
        finished.store(true, Ordering::Relaxed);
    }

    Arc::new(BrokerSink {
        tx,
        depth,
        high_water,
        session,
        finished,
        join: Mutex::new(join),
    })
}

type ControlFactory = Box<dyn FnMut(u64) -> Result<Box<dyn PaneBackend>> + Send>;

pub(crate) struct Broker {
    rx: Receiver<PaneInput>,
    depth: Arc<AtomicUsize>,
    batch_window: Duration,
    fallback: Box<dyn PaneBackend>,
    control: Option<Box<dyn PaneBackend>>,
    control_factory: Option<ControlFactory>,
    generation: u64,
    next_attempt: Option<Instant>,
    backoff: Duration,
    pending: Option<(String, String)>,
    deadline: Option<Instant>,
    fallbacks: u64,
    finished: Arc<AtomicBool>,
}

impl Broker {
    pub(crate) fn run(mut self) {
        loop {
            let next = match self.deadline {
                Some(deadline) => {
                    let wait = deadline.saturating_duration_since(Instant::now());
                    match self.rx.recv_timeout(wait) {
                        Ok(input) => Some(input),
                        Err(RecvTimeoutError::Timeout) => {
                            self.flush(FlushReason::Window);
                            continue;
                        }
                        Err(RecvTimeoutError::Disconnected) => None,
                    }
                }
                None => self.rx.recv().ok(),
            };

            let Some(input) = next else {
                // Every sink handle was dropped: deliver what is buffered rather
                // than losing the last word the user typed.
                self.flush(FlushReason::Shutdown);
                break;
            };
            self.depth.fetch_sub(1, Ordering::Relaxed);

            match input {
                PaneInput::Text { target, text } => self.push_text(target, text),
                PaneInput::Key { target, key } => {
                    self.flush(FlushReason::BeforeKey);
                    self.dispatch(|b| b.key(&target, &key), "key");
                }
                PaneInput::Paste { target, text } => {
                    self.flush(FlushReason::BeforeKey);
                    let len = text.len();
                    self.dispatch(|b| b.paste(&target, &text), "paste");
                    tracing::debug!(bytes = len, "pane paste delivered");
                }
                PaneInput::Flush => self.flush(FlushReason::Explicit),
                PaneInput::Barrier { ack } => {
                    self.flush(FlushReason::Explicit);
                    let completed = self.barrier();
                    let _ = ack.send(completed);
                }
                PaneInput::Shutdown => {
                    self.flush(FlushReason::Shutdown);
                    break;
                }
            }
        }
        if let Some(control) = self.control.take() {
            drop(control);
        }
        self.finished.store(true, Ordering::Relaxed);
    }

    fn push_text(&mut self, target: String, text: String) {
        match &mut self.pending {
            Some((pending_target, buf)) if *pending_target == target => {
                buf.push_str(&text);
                if buf.len() >= MAX_BATCH_BYTES {
                    self.flush(FlushReason::Full);
                }
            }
            Some(_) => {
                // Different pane: everything buffered belongs to the old one and
                // must land there first.
                self.flush(FlushReason::TargetChange);
                self.pending = Some((target, text));
                self.deadline = Some(Instant::now() + self.batch_window);
            }
            None => {
                self.pending = Some((target, text));
                self.deadline = Some(Instant::now() + self.batch_window);
            }
        }
    }

    fn flush(&mut self, reason: FlushReason) {
        self.deadline = None;
        let Some((target, text)) = self.pending.take() else {
            return;
        };
        let chars = text.chars().count();
        self.dispatch(|b| b.text(&target, &text), "text");
        // Lengths and reasons only: pane input is never logged.
        tracing::trace!(?reason, chars, "pane text flushed");
    }

    fn barrier(&mut self) -> bool {
        match self.control.as_mut() {
            Some(control) if control.healthy() => control.barrier(),
            // The subprocess backend is synchronous. If control died after the
            // preceding dispatch, that dispatch was deliberately treated as
            // ambiguous and cannot be made safer by waiting here.
            _ => true,
        }
    }

    /// Run one operation on the best available backend.
    ///
    /// A control-mode write error is **ambiguous** — a partial write may already
    /// have reached tmux — so the request is dropped rather than replayed on the
    /// fallback. Repeating an Enter is worse than losing one, and the *next*
    /// request goes to the fallback anyway because the failure marks the
    /// connection dead.
    fn dispatch(&mut self, op: impl FnOnce(&mut dyn PaneBackend) -> Result<()>, kind: &str) {
        self.maybe_connect();
        let started = Instant::now();
        if let Some(control) = self.control.as_mut() {
            if control.healthy() {
                match op(control.as_mut()) {
                    Ok(()) => {
                        tracing::trace!(
                            kind,
                            backend = "control",
                            micros = started.elapsed().as_micros() as u64,
                            "pane input delivered"
                        );
                        return;
                    }
                    Err(e) => {
                        self.drop_control(&format!("write failed: {e}"));
                        tracing::warn!(kind, "pane input dropped after an ambiguous control write");
                        return;
                    }
                }
            }
            self.drop_control("connection closed");
        }
        self.fallbacks += 1;
        if let Err(e) = op(self.fallback.as_mut()) {
            tracing::warn!(kind, error = %e, "pane input failed on the subprocess backend");
        } else {
            tracing::trace!(
                kind,
                backend = "subprocess",
                micros = started.elapsed().as_micros() as u64,
                "pane input delivered"
            );
        }
    }

    fn drop_control(&mut self, reason: &str) {
        if self.control.take().is_some() {
            tracing::info!(
                generation = self.generation,
                reason,
                "tmux control connection lost; falling back to subprocess"
            );
        }
        self.next_attempt = Some(Instant::now() + self.backoff);
        self.backoff = (self.backoff * 2).min(RECONNECT_MAX);
    }

    fn maybe_connect(&mut self) {
        if self.control.is_some() || self.control_factory.is_none() {
            return;
        }
        match self.next_attempt {
            Some(at) if Instant::now() < at => return,
            None => return,
            _ => {}
        }
        self.next_attempt = None;
        self.generation += 1;
        let generation = self.generation;
        let factory = self.control_factory.as_mut().expect("checked above");
        match factory(generation) {
            Ok(backend) => {
                tracing::info!(
                    generation,
                    backend = backend.label(),
                    "pane input backend ready"
                );
                self.control = Some(backend);
                self.backoff = RECONNECT_MIN;
            }
            Err(e) => {
                tracing::info!(generation, error = %e, "tmux control connect failed; using subprocess");
                self.next_attempt = Some(Instant::now() + self.backoff);
                self.backoff = (self.backoff * 2).min(RECONNECT_MAX);
            }
        }
    }
}

/// A sink that records requests instead of sending them. For tests that assert
/// what the UI enqueued without needing a tmux server.
#[cfg(any(test, feature = "test-mocks"))]
#[derive(Default)]
pub struct RecordingSink {
    sent: Mutex<Vec<PaneInput>>,
    /// When set, every `send` fails with it — for exercising `QueueFull`.
    fail_with: Mutex<Option<InputError>>,
}

#[cfg(any(test, feature = "test-mocks"))]
impl RecordingSink {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn taken(&self) -> Vec<PaneInput> {
        self.sent.lock().map(|s| s.clone()).unwrap_or_default()
    }

    pub fn fail_with(&self, err: InputError) {
        if let Ok(mut slot) = self.fail_with.lock() {
            *slot = Some(err);
        }
    }
}

#[cfg(any(test, feature = "test-mocks"))]
impl PaneInputSink for RecordingSink {
    /// Records the barrier rather than degrading to a plain `Flush`, so a test
    /// can tell the two apart. Without this the default turns `flush_sync` into
    /// `flush` and nothing pins which one a call site asked for.
    fn flush_sync(&self) -> std::result::Result<(), InputError> {
        let (ack, _done) = std::sync::mpsc::channel();
        self.send(PaneInput::Barrier { ack })
    }

    fn send(&self, input: PaneInput) -> std::result::Result<(), InputError> {
        if let Ok(slot) = self.fail_with.lock() {
            if let Some(err) = slot.as_ref() {
                return Err(err.clone());
            }
        }
        if let Ok(mut sent) = self.sent.lock() {
            sent.push(input);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "input_tests.rs"]
mod input_tests;
