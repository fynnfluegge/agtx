//! Unit tests for the input broker.
//!
//! These assert **byte and key order**, not final visible text: a batching bug
//! that swaps a cursor key past the characters around it produces the same pane
//! content in a test and the wrong text in a real editor.

use super::*;
use std::sync::mpsc::sync_channel;

/// What a backend was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Op {
    Text(String, String),
    Key(String, String),
    Paste(String, String),
    Barrier,
    Capture(String),
    OutsideTmuxOperation,
}

#[derive(Clone, Default)]
struct Recorder {
    ops: Arc<Mutex<Vec<Op>>>,
}

impl Recorder {
    fn ops(&self) -> Vec<Op> {
        self.ops.lock().unwrap().clone()
    }
    fn push(&self, op: Op) {
        self.ops.lock().unwrap().push(op);
    }
}

struct RecordingBackend {
    label: &'static str,
    recorder: Recorder,
    /// Fail every write, as a broken control connection does.
    failing: bool,
    /// Report itself as dead, as a `%exit`ed connection does.
    dead: Arc<std::sync::atomic::AtomicBool>,
    /// What a capture returns. `None` leaves the trait default, which is the
    /// subprocess backend's real behaviour: it does not support captures.
    capture: Option<PaneSnapshot>,
}

impl RecordingBackend {
    fn new(label: &'static str, recorder: Recorder) -> Self {
        Self {
            label,
            recorder,
            failing: false,
            dead: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            capture: None,
        }
    }
}

impl PaneBackend for RecordingBackend {
    fn text(&mut self, target: &str, text: &str) -> Result<()> {
        if self.failing {
            anyhow::bail!("write failed");
        }
        self.recorder.push(Op::Text(
            format!("{}:{target}", self.label),
            text.to_string(),
        ));
        Ok(())
    }
    fn key(&mut self, target: &str, key: &str) -> Result<()> {
        if self.failing {
            anyhow::bail!("write failed");
        }
        self.recorder
            .push(Op::Key(format!("{}:{target}", self.label), key.to_string()));
        Ok(())
    }
    fn paste(&mut self, target: &str, text: &str) -> Result<()> {
        if self.failing {
            anyhow::bail!("write failed");
        }
        self.recorder.push(Op::Paste(
            format!("{}:{target}", self.label),
            text.to_string(),
        ));
        Ok(())
    }
    fn barrier(&mut self) -> bool {
        self.recorder.push(Op::Barrier);
        true
    }
    fn capture(&mut self, target: &str, _spec: CaptureSpec) -> Result<PaneSnapshot> {
        self.recorder
            .push(Op::Capture(format!("{}:{target}", self.label)));
        match self.capture.clone() {
            Some(snapshot) => Ok(snapshot),
            None => anyhow::bail!("capture is not supported on the {} backend", self.label),
        }
    }
    fn healthy(&self) -> bool {
        !self.dead.load(std::sync::atomic::Ordering::Relaxed)
    }
    fn label(&self) -> &'static str {
        self.label
    }
}

/// A batching window long enough that only an explicit flush can end it, so a
/// test asserting coalescing is not really asserting how fast the machine is.
const NEVER: Duration = Duration::from_secs(3600);

struct Harness {
    tx: SyncSender<PaneInput>,
    depth: Arc<AtomicUsize>,
    join: std::thread::JoinHandle<()>,
    recorder: Recorder,
}

impl Harness {
    fn send(&self, input: PaneInput) {
        self.depth.fetch_add(1, Ordering::Relaxed);
        self.tx.send(input).expect("broker is running");
    }
    fn text(&self, target: &str, text: &str) {
        self.send(PaneInput::Text {
            target: target.to_string(),
            text: text.to_string(),
        });
    }
    fn key(&self, target: &str, key: &str) {
        self.send(PaneInput::Key {
            target: target.to_string(),
            key: key.to_string(),
        });
    }
    fn flush_sync(&self) {
        let (ack, done) = std::sync::mpsc::channel();
        self.send(PaneInput::Barrier { ack });
        assert_eq!(done.recv_timeout(Duration::from_secs(1)), Ok(true));
    }
    /// Stop the broker and return everything the backends were asked to do.
    fn finish(self) -> Vec<Op> {
        self.send(PaneInput::Shutdown);
        self.join.join().expect("broker thread");
        self.recorder.ops()
    }
}

fn harness(batch_window: Duration) -> Harness {
    harness_with(batch_window, Recorder::default(), None)
}

/// A harness whose queue is **already full of `inputs`** when the broker starts.
///
/// Coalescing is opportunistic by design: buffered text is flushed as soon as
/// the queue runs dry, because between two keystrokes a human types it always
/// does and waiting out the batch window there would tax every character. So a
/// test that sends three characters to a running broker is asserting that it
/// lost the race to the broker's own drain, which is timing, not behaviour.
/// Queueing first makes the backlog real and the outcome deterministic.
fn harness_queued(batch_window: Duration, inputs: Vec<PaneInput>) -> Harness {
    let (tx, rx) = sync_channel(64);
    let depth = Arc::new(AtomicUsize::new(0));
    for input in inputs {
        depth.fetch_add(1, Ordering::Relaxed);
        tx.send(input).expect("queue has room");
    }
    let recorder = Recorder::default();
    let broker = Broker {
        rx,
        depth: Arc::clone(&depth),
        batch_window,
        fallback: Box::new(RecordingBackend::new("sub", recorder.clone())),
        control: None,
        control_factory: None,
        generation: 0,
        next_attempt: Some(Instant::now()),
        backoff: Duration::from_millis(1),
        pending: None,
        deadline: None,
        fallbacks: 0,
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let join = std::thread::spawn(move || broker.run());
    Harness {
        tx,
        depth,
        join,
        recorder,
    }
}

fn text_input(target: &str, text: &str) -> PaneInput {
    PaneInput::Text {
        target: target.to_string(),
        text: text.to_string(),
    }
}

/// Both backends record into the **same** log, so a test can assert not just
/// what was delivered but which backend delivered it, in one order.
fn harness_with(
    batch_window: Duration,
    recorder: Recorder,
    control_factory: Option<ControlFactory>,
) -> Harness {
    let (tx, rx) = sync_channel(64);
    let depth = Arc::new(AtomicUsize::new(0));
    let broker = Broker {
        rx,
        depth: Arc::clone(&depth),
        batch_window,
        fallback: Box::new(RecordingBackend::new("sub", recorder.clone())),
        control: None,
        control_factory,
        generation: 0,
        next_attempt: Some(Instant::now()),
        backoff: Duration::from_millis(1),
        pending: None,
        deadline: None,
        fallbacks: 0,
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let join = std::thread::spawn(move || broker.run());
    Harness {
        tx,
        depth,
        join,
        recorder,
    }
}

#[test]
fn adjacent_text_for_one_target_is_coalesced() {
    let h = harness_queued(
        NEVER,
        vec![
            text_input("s:w", "a"),
            text_input("s:w", "b"),
            text_input("s:w", "c"),
        ],
    );
    assert_eq!(h.finish(), vec![Op::Text("sub:s:w".into(), "abc".into())]);
}

#[test]
fn text_is_not_held_when_nothing_else_is_queued() {
    // The other half of the policy, and the one a user feels: with an idle
    // queue the character goes out now, not when the batch window expires.
    // NEVER as the window is what makes this an assertion rather than a race —
    // under a timer-only broker nothing could arrive before the shutdown flush.
    let h = harness(NEVER);
    h.text("s:w", "a");
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.recorder.ops().is_empty() {
        assert!(
            Instant::now() < deadline,
            "buffered text was never flushed without a key, a flush or a shutdown"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        h.recorder.ops(),
        vec![Op::Text("sub:s:w".into(), "a".into())]
    );
    h.finish();
}

#[test]
fn text_is_never_coalesced_across_targets() {
    // Queued characters belong to the pane they were typed into. Merging them
    // across a target change would type one task's input into another's agent.
    let h = harness(NEVER);
    h.text("s:one", "ab");
    h.text("s:two", "cd");
    assert_eq!(
        h.finish(),
        vec![
            Op::Text("sub:s:one".into(), "ab".into()),
            Op::Text("sub:s:two".into(), "cd".into()),
        ]
    );
}

#[test]
fn a_key_flushes_buffered_text_first_and_is_never_delayed() {
    let h = harness(NEVER);
    h.text("s:w", "abc");
    h.key("s:w", "Left");
    h.text("s:w", "X");
    h.key("s:w", "Enter");
    assert_eq!(
        h.finish(),
        vec![
            Op::Text("sub:s:w".into(), "abc".into()),
            Op::Key("sub:s:w".into(), "Left".into()),
            Op::Text("sub:s:w".into(), "X".into()),
            Op::Key("sub:s:w".into(), "Enter".into()),
        ]
    );
}

#[test]
fn an_explicit_flush_delivers_the_prefix() {
    // What a popup close does: whatever was typed lands in the pane it was typed
    // into, before the target can change.
    let h = harness(NEVER);
    h.text("s:w", "half");
    h.send(PaneInput::Flush);
    h.text("s:w", "rest");
    assert_eq!(
        h.finish(),
        vec![
            Op::Text("sub:s:w".into(), "half".into()),
            Op::Text("sub:s:w".into(), "rest".into()),
        ]
    );
}

#[test]
fn an_acknowledged_flush_prevents_an_external_tmux_operation_from_overtaking_input() {
    let h = harness(NEVER);
    h.text("s:w", "typed-before-resize");

    // This models Ctrl+F: the resize is issued synchronously by the TUI after
    // the broker boundary returns. A plain `Flush` enqueue can return before
    // the text is delivered; the acknowledged barrier may not.
    h.flush_sync();
    h.recorder.push(Op::OutsideTmuxOperation);

    assert_eq!(
        h.finish(),
        vec![
            Op::Text("sub:s:w".into(), "typed-before-resize".into()),
            Op::OutsideTmuxOperation,
        ]
    );
}

/// A backend whose every command costs what a `tmux` subprocess costs.
struct SlowBackend {
    recorder: Recorder,
    per_command: Duration,
}

impl PaneBackend for SlowBackend {
    fn text(&mut self, target: &str, text: &str) -> Result<()> {
        std::thread::sleep(self.per_command);
        self.recorder
            .push(Op::Text(format!("slow:{target}"), text.to_string()));
        Ok(())
    }
    fn key(&mut self, target: &str, key: &str) -> Result<()> {
        std::thread::sleep(self.per_command);
        self.recorder
            .push(Op::Key(format!("slow:{target}"), key.to_string()));
        Ok(())
    }
    fn paste(&mut self, _target: &str, _text: &str) -> Result<()> {
        Ok(())
    }
    fn label(&self) -> &'static str {
        "slow"
    }
}

#[test]
fn an_acknowledged_flush_waits_out_a_slow_drain() {
    // The barrier sits *behind* the queue, so the wait is dominated by draining
    // it — not by the barrier round trip. Sized as a barrier round trip instead,
    // it expired mid-drain and handed back a guarantee that had not been kept:
    // with a queue of keys still in flight it reported failure, and the caller
    // went on to resize the pane anyway.
    //
    // The drain is spread over a few long commands rather than many short ones.
    // Its length is what the test needs, but the jitter a loaded machine adds is
    // paid per `sleep`: the same nominal drain split twenty ways lands four
    // times closer to the budget on a runner that stretches every wake-up, which
    // is a timeout that says nothing about the code under test.
    const COMMANDS: usize = 4;
    const PER_COMMAND: Duration = Duration::from_millis(100);
    let drain = PER_COMMAND * COMMANDS as u32;
    assert!(
        drain > BARRIER_TIMEOUT,
        "a drain of {drain:?} would fit inside a barrier-sized budget, so the \
         regression this locks would pass"
    );
    assert!(
        drain * 4 < FLUSH_SYNC_TIMEOUT,
        "a fourfold slowdown of a {drain:?} drain must still fit inside \
         {FLUSH_SYNC_TIMEOUT:?}"
    );

    let recorder = Recorder::default();
    let (tx, rx) = sync_channel(64);
    let depth = Arc::new(AtomicUsize::new(0));
    let broker = Broker {
        rx,
        depth: Arc::clone(&depth),
        batch_window: NEVER,
        fallback: Box::new(SlowBackend {
            recorder: recorder.clone(),
            per_command: PER_COMMAND,
        }),
        control: None,
        control_factory: None,
        generation: 0,
        next_attempt: None,
        backoff: Duration::from_millis(1),
        pending: None,
        deadline: None,
        fallbacks: 0,
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    std::thread::spawn(move || broker.run());
    let sink = BrokerSink {
        tx,
        depth,
        high_water: Arc::new(AtomicUsize::new(0)),
        session: Arc::new(Mutex::new("it".to_string())),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        join: Mutex::new(None),
    };

    // Non-text keys, because adjacent text would coalesce into one command.
    for _ in 0..COMMANDS {
        sink.key("s:w", "BSpace").unwrap();
    }
    let started = Instant::now();
    assert_eq!(
        sink.flush_sync(),
        Ok(()),
        "the flush gave up after {:?} of a {drain:?} drain",
        started.elapsed()
    );
    assert_eq!(
        recorder.ops().len(),
        COMMANDS,
        "every queued key must have been delivered before the flush returned"
    );
}

#[test]
fn the_batching_window_bounds_how_long_a_backlog_is_held() {
    // The window is the burst case's bound: with a backlog queued and a window
    // that expires, the buffer must go out on the timer even though no key,
    // flush or shutdown followed it.
    let h = harness_queued(
        Duration::from_millis(1),
        vec![text_input("s:w", "a"), text_input("s:w", "b")],
    );
    let deadline = Instant::now() + Duration::from_secs(2);
    while h.recorder.ops().is_empty() {
        assert!(
            Instant::now() < deadline,
            "the batching window never expired"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        h.recorder.ops(),
        vec![Op::Text("sub:s:w".into(), "ab".into())]
    );
    h.finish();
}

#[test]
fn shutdown_delivers_what_is_still_buffered() {
    let h = harness(NEVER);
    h.text("s:w", "unsent");
    assert_eq!(
        h.finish(),
        vec![Op::Text("sub:s:w".into(), "unsent".into())]
    );
}

#[test]
fn dropping_every_sender_still_delivers_the_last_word() {
    let recorder = Recorder::default();
    let (tx, rx) = sync_channel(8);
    let broker = Broker {
        rx,
        depth: Arc::new(AtomicUsize::new(0)),
        batch_window: NEVER,
        fallback: Box::new(RecordingBackend::new("sub", recorder.clone())),
        control: None,
        control_factory: None,
        generation: 0,
        next_attempt: None,
        backoff: Duration::from_millis(1),
        pending: None,
        deadline: None,
        fallbacks: 0,
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let join = std::thread::spawn(move || broker.run());
    tx.send(PaneInput::Text {
        target: "s:w".into(),
        text: "tail".into(),
    })
    .unwrap();
    drop(tx);
    join.join().unwrap();
    assert_eq!(
        recorder.ops(),
        vec![Op::Text("sub:s:w".into(), "tail".into())]
    );
}

#[test]
fn unicode_survives_coalescing() {
    let h = harness_queued(
        NEVER,
        "한국어😀"
            .chars()
            .map(|ch| text_input("s:w", &ch.to_string()))
            .collect(),
    );
    assert_eq!(
        h.finish(),
        vec![Op::Text("sub:s:w".into(), "한국어😀".into())]
    );
}

#[test]
fn a_batch_is_capped_in_bytes() {
    // The cap bounds how much one failed write can lose, and keeps a command
    // line at a size tmux is comfortable with.
    let h = harness(NEVER);
    let chunk = "x".repeat(1000);
    for _ in 0..10 {
        h.text("s:w", &chunk);
    }
    let ops = h.finish();
    assert!(
        ops.len() > 1,
        "10 KiB must not go out as one write: {ops:?}"
    );
    let mut total = 0;
    for op in &ops {
        match op {
            Op::Text(_, text) => {
                assert!(
                    text.len() <= MAX_BATCH_BYTES + chunk.len(),
                    "a batch overshot the cap by more than one request"
                );
                total += text.len();
            }
            other => panic!("expected text writes, got {other:?}"),
        }
    }
    assert_eq!(total, 10_000, "no character may be lost to the cap");
}

#[test]
fn a_paste_flushes_first_and_stays_atomic() {
    let h = harness(NEVER);
    h.text("s:w", "before");
    h.send(PaneInput::Paste {
        target: "s:w".into(),
        text: "multi\nline".into(),
    });
    h.text("s:w", "after");
    assert_eq!(
        h.finish(),
        vec![
            Op::Text("sub:s:w".into(), "before".into()),
            Op::Paste("sub:s:w".into(), "multi\nline".into()),
            Op::Text("sub:s:w".into(), "after".into()),
        ]
    );
}

// --- pane capture over the input connection ---

fn canned_snapshot() -> PaneSnapshot {
    PaneSnapshot {
        content: b"hello\n".to_vec(),
        metrics: Some(super::super::PaneMetrics {
            cursor_x: 1,
            cursor_y: 2,
            pane_height: 34,
            history_size: 0,
        }),
    }
}

fn capture(h: &Harness, target: &str) -> Option<PaneSnapshot> {
    let (ack, done) = std::sync::mpsc::channel();
    h.send(PaneInput::Capture {
        target: target.to_string(),
        spec: CaptureSpec::popup(500),
        ack,
    });
    done.recv_timeout(Duration::from_secs(2))
        .expect("the broker answered the capture")
}

#[test]
fn a_capture_is_served_by_the_control_connection() {
    let recorder = Recorder::default();
    let rec = recorder.clone();
    let factory: ControlFactory = Box::new(move |_| {
        let mut backend = RecordingBackend::new("ctl", rec.clone());
        backend.capture = Some(canned_snapshot());
        Ok(Box::new(backend))
    });
    let h = harness_with(NEVER, recorder, Some(factory));
    assert_eq!(capture(&h, "s:w"), Some(canned_snapshot()));
    assert_eq!(h.finish(), vec![Op::Capture("ctl:s:w".into())]);
}

#[test]
fn a_capture_shows_the_keys_typed_before_it() {
    // The reason a read belongs in an input queue at all. Buffered text must
    // reach the pane *before* the capture reads it back, or the popup renders a
    // frame that is missing characters the user has already typed — which is
    // precisely the staleness this path exists to remove.
    let recorder = Recorder::default();
    let rec = recorder.clone();
    let factory: ControlFactory = Box::new(move |_| {
        let mut backend = RecordingBackend::new("ctl", rec.clone());
        backend.capture = Some(canned_snapshot());
        Ok(Box::new(backend))
    });
    let h = harness_with(NEVER, recorder, Some(factory));
    h.text("s:w", "typed");
    assert!(capture(&h, "s:w").is_some());
    assert_eq!(
        h.finish(),
        vec![
            Op::Text("ctl:s:w".into(), "typed".into()),
            Op::Capture("ctl:s:w".into()),
        ]
    );
}

#[test]
fn a_capture_without_a_control_connection_is_declined_not_run_on_the_fallback() {
    // The subprocess path costs the caller the same two processes either way,
    // and running them here would block the next keystroke behind that process
    // startup. So the broker says no and the caller captures itself.
    let h = harness(NEVER);
    assert_eq!(capture(&h, "s:w"), None);
    assert_eq!(
        h.finish(),
        vec![],
        "nothing may reach the subprocess backend"
    );
}

#[test]
fn a_failed_capture_keeps_a_healthy_control_connection() {
    // A read that fails is not the ambiguous *write* the broker tears the
    // connection down for. Demoting every later keystroke to the subprocess path
    // over one failed read would trade the fix for the bug.
    let recorder = Recorder::default();
    let rec = recorder.clone();
    let connects = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&connects);
    let factory: ControlFactory = Box::new(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
        // `capture: None` makes the backend decline, as a query timeout does.
        Ok(Box::new(RecordingBackend::new("ctl", rec.clone())))
    });
    let h = harness_with(NEVER, recorder, Some(factory));
    assert_eq!(capture(&h, "s:w"), None);
    h.key("s:w", "Enter");
    assert_eq!(
        h.finish(),
        vec![
            Op::Capture("ctl:s:w".into()),
            Op::Key("ctl:s:w".into(), "Enter".into()),
        ],
        "the key must still go out on the control backend"
    );
    assert_eq!(
        connects.load(Ordering::Relaxed),
        1,
        "the connection was torn down and rebuilt over a failed read"
    );
}

// --- backend selection, failure and reconnect ---

#[test]
fn the_control_backend_is_preferred_when_it_connects() {
    let recorder = Recorder::default();
    let rec = recorder.clone();
    let factory: ControlFactory =
        Box::new(move |_| Ok(Box::new(RecordingBackend::new("ctl", rec.clone()))));
    let h = harness_with(NEVER, recorder, Some(factory));
    h.text("s:w", "hi");
    h.key("s:w", "Enter");
    assert_eq!(
        h.finish(),
        vec![
            Op::Text("ctl:s:w".into(), "hi".into()),
            Op::Key("ctl:s:w".into(), "Enter".into()),
        ]
    );
}

#[test]
fn an_ambiguous_control_write_is_not_replayed_on_the_fallback() {
    // A failed write may have landed partially. Replaying it could repeat an
    // Enter, which is worse than dropping one: the plan says so explicitly.
    let recorder = Recorder::default();
    let rec = recorder.clone();
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&attempts);
    let factory: ControlFactory = Box::new(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
        let mut backend = RecordingBackend::new("ctl", rec.clone());
        backend.failing = true;
        Ok(Box::new(backend))
    });
    let h = harness_with(NEVER, recorder, Some(factory));
    h.key("s:w", "Enter");
    assert_eq!(
        h.finish(),
        vec![],
        "the dropped key must not reappear on the subprocess backend"
    );
    assert_eq!(attempts.load(Ordering::Relaxed), 1);
}

#[test]
fn a_dead_control_connection_falls_back_without_reordering() {
    let recorder = Recorder::default();
    let rec = recorder.clone();
    let dead = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = Arc::clone(&dead);
    let factory: ControlFactory = Box::new(move |_| {
        let mut backend = RecordingBackend::new("ctl", rec.clone());
        backend.dead = Arc::clone(&flag);
        Ok(Box::new(backend))
    });
    let h = harness_with(NEVER, recorder, Some(factory));
    h.key("s:w", "a");
    std::thread::sleep(Duration::from_millis(20));
    dead.store(true, std::sync::atomic::Ordering::Relaxed);
    h.key("s:w", "b");
    h.key("s:w", "c");
    // A backend found dead has written nothing, so its request is *not*
    // ambiguous: it moves to the fallback in place, keeping the order.
    assert_eq!(
        h.finish(),
        vec![
            Op::Key("ctl:s:w".into(), "a".into()),
            Op::Key("sub:s:w".into(), "b".into()),
            Op::Key("sub:s:w".into(), "c".into()),
        ]
    );
}

#[test]
fn a_control_connection_that_never_comes_up_is_retried_with_backoff() {
    let recorder = Recorder::default();
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&attempts);
    let factory: ControlFactory = Box::new(move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
        anyhow::bail!("no tmux here")
    });
    let h = harness_with(NEVER, recorder, Some(factory));
    for _ in 0..3 {
        h.key("s:w", "x");
        std::thread::sleep(Duration::from_millis(5));
    }
    // Nothing is lost while the control client is unavailable: every key lands
    // on the subprocess backend.
    assert_eq!(
        h.finish(),
        vec![
            Op::Key("sub:s:w".into(), "x".into()),
            Op::Key("sub:s:w".into(), "x".into()),
            Op::Key("sub:s:w".into(), "x".into()),
        ]
    );
    assert!(attempts.load(Ordering::Relaxed) >= 1, "it must keep trying");
}

// --- the sink's queue policy ---

#[test]
fn a_full_queue_reports_queue_full_rather_than_blocking() {
    // The TUI thread must never wait on tmux. A full queue is a named error the
    // caller can surface, not a stall.
    let (tx, rx) = sync_channel(2);
    let sink = BrokerSink {
        tx,
        depth: Arc::new(AtomicUsize::new(0)),
        high_water: Arc::new(AtomicUsize::new(0)),
        session: Arc::new(Mutex::new("it".to_string())),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        join: Mutex::new(None),
    };
    assert!(sink.key("s:w", "a").is_ok());
    assert!(sink.key("s:w", "b").is_ok());
    assert_eq!(sink.key("s:w", "c"), Err(InputError::QueueFull));
    let (depth, high_water) = sink.depth();
    assert_eq!(
        depth, 2,
        "the rejected request must not be counted as queued"
    );
    // The high-water mark may include the rejected attempt: the counter is
    // incremented before the send and rolled back after, which is the only order
    // that cannot underflow. It is a diagnostic, and over-counting is the
    // harmless direction.
    assert!(high_water >= 2);
    drop(rx);
    assert_eq!(sink.key("s:w", "d"), Err(InputError::Disconnected));
}

#[test]
fn changing_project_repoints_the_next_connection() {
    // agtx switches project in place. An open connection is deliberately left
    // alone — it still delivers, because every target is `session:window`
    // (`pane_target`) and is therefore resolved server-wide rather than inside
    // the attached session. What the switch does change is where a *reconnect*
    // attaches, since the previous project's session may be killed.
    let (tx, _rx) = sync_channel(2);
    let sink = BrokerSink {
        tx,
        depth: Arc::new(AtomicUsize::new(0)),
        high_water: Arc::new(AtomicUsize::new(0)),
        session: Arc::new(Mutex::new("old".to_string())),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        join: Mutex::new(None),
    };
    sink.set_session("new");
    assert_eq!(sink.session.lock().unwrap().as_str(), "new");
}

#[test]
fn the_queue_depth_counter_cannot_go_negative() {
    // The broker decrements this from another thread. Counting after a
    // successful send let it decrement first, taking a `usize` to `usize::MAX`
    // and panicking on the next increment — found only under real load, never
    // by a deterministic test, which is why it is pinned here.
    let (tx, rx) = sync_channel(64);
    let depth = Arc::new(AtomicUsize::new(0));
    let sink = Arc::new(BrokerSink {
        tx,
        depth: Arc::clone(&depth),
        high_water: Arc::new(AtomicUsize::new(0)),
        session: Arc::new(Mutex::new("it".to_string())),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        join: Mutex::new(None),
    });
    let drain = std::thread::spawn(move || {
        let mut seen = 0;
        while rx.recv().is_ok() {
            depth.fetch_sub(1, Ordering::Relaxed);
            seen += 1;
        }
        seen
    });
    for _ in 0..2000 {
        let _ = sink.key("s:w", "a");
    }
    drop(sink);
    assert!(drain.join().unwrap() > 0);
}

#[test]
fn quitting_cannot_hang_on_a_wedged_broker() {
    // The broker can be blocked inside a `tmux` subprocess that is never coming
    // back. Quitting agtx must still take a bounded amount of time, so both the
    // enqueue and the wait for the final flush give up rather than waiting for a
    // thread that will not answer.
    let (tx, rx) = sync_channel(1);
    let wedged = std::thread::spawn(move || {
        // Holds the receiver — and therefore the channel — without ever draining.
        std::thread::sleep(Duration::from_secs(30));
        drop(rx);
    });
    let sink = BrokerSink {
        tx,
        depth: Arc::new(AtomicUsize::new(0)),
        high_water: Arc::new(AtomicUsize::new(0)),
        session: Arc::new(Mutex::new("it".to_string())),
        finished: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        join: Mutex::new(None),
    };
    // Fill the one slot, so the stop request cannot be enqueued either.
    sink.key("s:w", "a").unwrap();

    let started = Instant::now();
    sink.shutdown();
    assert!(
        started.elapsed() < SHUTDOWN_ENQUEUE_WAIT + SHUTDOWN_DRAIN_WAIT + Duration::from_secs(1),
        "shutdown took {:?}",
        started.elapsed()
    );
    drop(sink);
    drop(wedged);
}

#[test]
fn the_recording_sink_captures_what_the_ui_enqueued() {
    let sink = RecordingSink::new();
    sink.text("s:w", "a").unwrap();
    sink.key("s:w", "Enter").unwrap();
    sink.flush().unwrap();
    assert_eq!(
        sink.taken(),
        vec![
            PaneInput::Text {
                target: "s:w".into(),
                text: "a".into()
            },
            PaneInput::Key {
                target: "s:w".into(),
                key: "Enter".into()
            },
            PaneInput::Flush,
        ]
    );
}

#[test]
fn every_request_names_the_pane_it_is_for() {
    // The broker keeps no "currently selected task": a stale one is exactly how
    // queued text would reach the wrong agent.
    assert_eq!(
        PaneInput::Text {
            target: "s:w".into(),
            text: "a".into()
        }
        .target(),
        Some("s:w")
    );
    assert_eq!(PaneInput::Flush.target(), None);
}
