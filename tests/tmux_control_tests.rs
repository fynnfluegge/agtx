//! Real-tmux tests for the pane input path.
//!
//! The unit tests in `src/tmux/input.rs` and `src/tmux/control.rs` prove the
//! broker's ordering and the encoder's escaping against recording doubles. They
//! cannot prove the one thing that matters most here: that **tmux agrees**. Every
//! bug this path can have is silent — a mis-encoded argument is still a valid
//! command, a control client that joins the size calculation shrinks a pane, and
//! a key that tmux resolves as a name never arrives as a character.
//!
//! So these run against a real `tmux` server on a throwaway socket, assert on
//! the **bytes a program in the pane received** rather than on rendered output,
//! and are opt-in:
//!
//! ```bash
//! AGTX_TMUX_IT=1 cargo test --test tmux_control_tests -- --nocapture
//! ```
//!
//! They are opt-in because they start processes, take seconds rather than
//! milliseconds, and depend on a tmux whose version the machine chooses. When
//! they are skipped they say so by name rather than passing quietly.

use agtx::tmux::input::{spawn, InputConfig, PaneInput, PaneInputSink};
use agtx::tmux::{CaptureSpec, TmuxOperations};
use anyhow::Result;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Skip guard. Returns the reason, so a skipped test names itself.
fn skip_reason() -> Option<&'static str> {
    if std::env::var("AGTX_TMUX_IT").ok().as_deref() != Some("1") {
        return Some("AGTX_TMUX_IT=1 not set (opt-in: starts real tmux servers)");
    }
    if which::which("tmux").is_err() {
        return Some("tmux is not installed");
    }
    None
}

macro_rules! guard {
    () => {
        if let Some(reason) = skip_reason() {
            eprintln!("SKIP {}: {}", module_path!(), reason);
            return;
        }
    };
}

/// A tmux server on its own socket, killed when the test ends — including when
/// it panics, which is when a leaked server would be most confusing.
struct Server {
    name: String,
    dir: tempfile::TempDir,
}

impl Server {
    fn start(tag: &str) -> Self {
        let name = format!(
            "agtx-it-{tag}-{}",
            std::process::id() as u64 * 1_000 + rand_suffix()
        );
        let server = Self {
            name,
            dir: tempfile::tempdir().expect("tempdir"),
        };
        server
            .tmux(&["new-session", "-d", "-s", "it", "-x", "200", "-y", "50"])
            .expect("start a session");
        server
    }

    fn tmux(&self, args: &[&str]) -> Result<String> {
        let out = Command::new("tmux")
            .args(["-L", &self.name])
            .args(args)
            .output()?;
        if !out.status.success() {
            anyhow::bail!(
                "tmux {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// A window whose pane writes everything it is sent into `name.txt`, so a
    /// test can assert on exact bytes instead of on what a terminal drew.
    fn recording_window(&self, name: &str) -> String {
        let path = self.dir.path().join(format!("{name}.txt"));
        self.tmux(&[
            "new-window",
            "-d",
            "-t",
            "it:",
            "-n",
            name,
            "sh",
            "-c",
            &format!("cat > {}", path.display()),
        ])
        .expect("create the recording window");
        // The shell has to be running before anything is typed at it.
        std::thread::sleep(Duration::from_millis(300));
        format!("it:{name}")
    }

    fn recorded(&self, name: &str) -> Vec<u8> {
        std::fs::read(self.dir.path().join(format!("{name}.txt"))).unwrap_or_default()
    }

    /// Cheap enough (a stat, tens of microseconds) to poll in a latency loop.
    fn recorded_len(&self, name: &str) -> u64 {
        std::fs::metadata(self.dir.path().join(format!("{name}.txt")))
            .map(|m| m.len())
            .unwrap_or(0)
    }

    fn client_count(&self) -> usize {
        self.tmux(&["list-clients", "-F", "#{client_name}"])
            .map(|s| s.lines().filter(|l| !l.trim().is_empty()).count())
            .unwrap_or(0)
    }

    fn pane_size(&self, target: &str) -> String {
        self.tmux(&[
            "display",
            "-p",
            "-t",
            target,
            "#{pane_width}x#{pane_height}",
        ])
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.tmux(&["kill-server"]);
    }
}

fn rand_suffix() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(0)
}

/// `TmuxOperations` bound to a test server.
///
/// `RealTmuxOps` hard-codes the `agtx` socket, which is the production server a
/// developer's own tasks are running on. Only the three send primitives the
/// broker's fallback uses are implemented; anything else would be a silent way
/// for a test to reach the wrong server.
struct TestTmuxOps {
    server: String,
}

impl TestTmuxOps {
    fn run(&self, args: &[&str]) {
        let _ = Command::new("tmux")
            .args(["-L", &self.server])
            .args(args)
            .output();
    }
}

impl TmuxOperations for TestTmuxOps {
    fn create_window(
        &self,
        _session: &str,
        _window_name: &str,
        _working_dir: &str,
        _command: Option<String>,
        _keep_shell_on_exit: bool,
        _env: &[(String, String)],
    ) -> Result<()> {
        unimplemented!("not used by the input broker")
    }
    fn kill_window(&self, _target: &str) -> Result<()> {
        unimplemented!("not used by the input broker")
    }
    fn pane_id(&self, target: &str) -> Option<String> {
        let out = Command::new("tmux")
            .args(["-L", &self.server])
            .args(["display", "-p", "-t", target, "#{pane_id}"])
            .output()
            .ok()?;
        let id = String::from_utf8_lossy(&out.stdout).trim().to_string();
        (!id.is_empty()).then_some(id)
    }
    fn list_window_targets(&self) -> Result<Vec<String>> {
        let out = Command::new("tmux")
            .args(["-L", &self.server])
            .args(["list-windows", "-a", "-F", "#{session_name}:#{window_name}"])
            .output()?;
        Ok(String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect())
    }
    fn window_exists(&self, target: &str) -> Result<bool> {
        Ok(Command::new("tmux")
            .args(["-L", &self.server])
            .args(["list-windows", "-t", target])
            .output()?
            .status
            .success())
    }
    fn send_keys(&self, target: &str, keys: &str) -> Result<()> {
        self.run(&["send-keys", "-t", target, keys]);
        self.run(&["send-keys", "-t", target, "Enter"]);
        Ok(())
    }
    fn send_key(&self, target: &str, keys: &str) -> Result<()> {
        self.run(&["send-keys", "-t", target, keys]);
        Ok(())
    }
    fn send_text(&self, target: &str, text: &str) -> Result<()> {
        self.run(&["send-keys", "-t", target, "-l", "--", text]);
        Ok(())
    }
    fn paste_text(&self, target: &str, text: &str) -> Result<()> {
        use std::io::Write;
        let mut child = Command::new("tmux")
            .args(["-L", &self.server, "load-buffer", "-"])
            .stdin(std::process::Stdio::piped())
            .spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(text.as_bytes())?;
        }
        child.wait()?;
        self.run(&["paste-buffer", "-p", "-t", target]);
        Ok(())
    }
    fn capture_pane(&self, target: &str) -> Result<String> {
        let out = Command::new("tmux")
            .args(["-L", &self.server])
            .args(["capture-pane", "-t", target, "-p"])
            .output()?;
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }
    /// The subprocess capture path, verbatim from `RealTmuxOps` but on the
    /// test server's socket. It is the reference the control capture is
    /// compared against, so it must not be a simplification of it.
    fn capture_pane_with_history(&self, target: &str, history_lines: i32) -> Vec<u8> {
        Command::new("tmux")
            .args(["-L", &self.server])
            .args(["capture-pane", "-t", target, "-p", "-e"])
            .args(["-S", &format!("-{history_lines}")])
            .output()
            .map(|o| o.stdout)
            .unwrap_or_default()
    }
    fn pane_metrics(&self, target: &str) -> Option<agtx::tmux::PaneMetrics> {
        let out = Command::new("tmux")
            .args(["-L", &self.server])
            .args([
                "display",
                "-p",
                "-t",
                target,
                agtx::tmux::PANE_METRICS_FORMAT,
            ])
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        agtx::tmux::parse_pane_metrics(&String::from_utf8_lossy(&out.stdout))
    }
    fn resize_window(&self, _target: &str, _width: u16, _height: u16) -> Result<()> {
        unimplemented!("not used by the input broker")
    }
    fn pane_current_command(&self, _target: &str) -> Option<String> {
        None
    }
    fn has_session(&self, _session: &str) -> bool {
        true
    }
    fn create_session(&self, _session: &str, _working_dir: &str) -> Result<()> {
        unimplemented!("not used by the input broker")
    }
}

fn sink_for(server: &Server, control_mode: bool) -> Arc<dyn PaneInputSink> {
    let mut config = InputConfig::new(server.name.clone(), "it");
    config.control_mode = control_mode;
    spawn(
        config,
        Arc::new(TestTmuxOps {
            server: server.name.clone(),
        }),
    )
}

/// Wait for the pane's recorded bytes to settle, then return them.
fn settle(server: &Server, window: &str) -> Vec<u8> {
    std::thread::sleep(Duration::from_millis(400));
    server.recorded(window)
}

// --- ordering and encoding ---

#[test]
fn a_thousand_characters_and_a_cursor_move_arrive_in_order() {
    guard!();
    let server = Server::start("order");
    let target = server.recording_window("order");
    let sink = sink_for(&server, true);

    for _ in 0..1000 {
        sink.text(&target, "a").unwrap();
    }
    sink.key(&target, "Left").unwrap();
    sink.text(&target, "X").unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    // The tty is in canonical mode, so an arrow key is not interpreted: it lands
    // in the line buffer as the escape sequence tmux sent, between the `a`s and
    // the `X`. That is exactly the ordering claim, in bytes.
    let mut expected = vec![b'a'; 1000];
    expected.extend_from_slice(b"\x1b[D");
    expected.push(b'X');
    expected.push(b'\n');
    assert_eq!(settle(&server, "order"), expected);
}

#[test]
fn text_that_spells_a_tmux_key_name_stays_text() {
    guard!();
    let server = Server::start("keyname");
    let target = server.recording_window("keyname");
    let sink = sink_for(&server, true);

    // Without `-l` tmux resolves each of these as a key: `Space` would arrive as
    // 0x20, `Up` as an escape sequence, and the words themselves would be gone.
    for word in ["Space", "Enter", "Up", "C-c", "--", "-x"] {
        sink.text(&target, word).unwrap();
        sink.text(&target, " ").unwrap();
    }
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    assert_eq!(
        settle(&server, "keyname"),
        b"Space Enter Up C-c -- -x \n".to_vec()
    );
}

#[test]
fn every_character_the_encoder_escapes_survives_the_round_trip() {
    guard!();
    let server = Server::start("escape");
    let target = server.recording_window("escape");
    let sink = sink_for(&server, true);

    // One string per replacement tmux performs inside double quotes, plus the
    // punctuation that would end the argument or start a new command.
    let payload = r##"$HOME #{pane_id} ~root "quoted" back\slash semi;colon 'single' %pct"##;
    sink.text(&target, payload).unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    assert_eq!(
        String::from_utf8_lossy(&settle(&server, "escape")),
        format!("{payload}\n")
    );
}

#[test]
fn unicode_survives_the_control_connection() {
    guard!();
    let server = Server::start("unicode");
    let target = server.recording_window("unicode");
    let sink = sink_for(&server, true);

    let payload = "한국어 naïve 😀 ok";
    for ch in payload.chars() {
        sink.text(&target, &ch.to_string()).unwrap();
    }
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    assert_eq!(
        String::from_utf8_lossy(&settle(&server, "unicode")),
        format!("{payload}\n")
    );
}

#[test]
fn a_multiline_paste_arrives_as_one_block_in_order() {
    guard!();
    let server = Server::start("paste");
    let target = server.recording_window("paste");
    let sink = sink_for(&server, true);

    sink.text(&target, "before ").unwrap();
    sink.paste(&target, "one\ntwo").unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    // A paste travels the buffer, not the control connection; the barrier is
    // what keeps it behind the text typed before it. Both lines arrive as one
    // block, in order, after the text typed first.
    //
    // No `\e[200~` markers here: tmux emits the bracketed-paste wrapper only for
    // a pane that asked for it (DECSET 2004), and `cat` does not — which is the
    // point of using `paste-buffer -p` rather than writing the markers by hand.
    assert_eq!(
        String::from_utf8_lossy(&settle(&server, "paste")),
        "before one\ntwo\n"
    );
}

// --- the properties that made control mode risky ---

#[test]
fn the_control_client_never_changes_the_pane_size() {
    guard!();
    let server = Server::start("size");
    let target = server.recording_window("size");
    let before = server.pane_size(&target);
    assert_eq!(
        before, "200x50",
        "the fixture window should start at 200x50"
    );

    let sink = sink_for(&server, true);
    sink.text(&target, "x").unwrap();
    sink.flush().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    let during = server.pane_size(&target);

    sink.shutdown();
    std::thread::sleep(Duration::from_millis(300));
    let after = server.pane_size(&target);

    // `-f ignore-size` is what guarantees this. A control client that took part
    // in the size calculation would resize the very pane the popup sized by hand.
    assert_eq!(
        (before.as_str(), during.as_str(), after.as_str()),
        ("200x50", "200x50", "200x50")
    );
}

#[test]
fn a_missing_target_window_does_not_wedge_the_broker() {
    guard!();
    let server = Server::start("missing");
    let target = server.recording_window("live");
    let sink = sink_for(&server, true);

    // tmux answers with `%error` and keeps the connection: the next request must
    // still land.
    sink.text("it:no-such-window", "lost").unwrap();
    sink.flush().unwrap();
    sink.text(&target, "found").unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    assert_eq!(settle(&server, "live"), b"found\n".to_vec());
}

#[test]
fn a_target_naming_another_session_is_delivered_there() {
    guard!();
    // The regression guard for the bug this file could not see: every test here
    // used an `it:`-qualified target while the popup passed a bare window name,
    // which tmux resolves *inside the attached session*. A second session with a
    // window the attached one does not have is the shape that failed —
    // `%error can't find pane`, keystroke dropped, `write_command` still `Ok`.
    let server = Server::start("cross");
    let target = server.recording_window("here");
    let sink = sink_for(&server, true);

    let path = server.dir.path().join("there.txt");
    server
        .tmux(&[
            "new-session",
            "-d",
            "-s",
            "other",
            "-n",
            "there",
            "sh",
            "-c",
            &format!("cat > {}", path.display()),
        ])
        .expect("create the second session");
    std::thread::sleep(Duration::from_millis(300));

    // The client is attached to `it`; this names `other` and must land there.
    sink.text("other:there", "elsewhere").unwrap();
    sink.key("other:there", "Enter").unwrap();
    // ...without disturbing the session it is attached to.
    sink.text(&target, "here").unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        String::from_utf8_lossy(&std::fs::read(&path).unwrap_or_default()),
        "elsewhere\n"
    );
    assert_eq!(settle(&server, "here"), b"here\n".to_vec());
}

#[test]
fn a_restarted_server_is_reconnected_to() {
    guard!();
    let server = Server::start("restart");
    let target = server.recording_window("first");
    let sink = sink_for(&server, true);
    sink.text(&target, "before").unwrap();
    sink.key(&target, "Enter").unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let _ = server.tmux(&["kill-server"]);
    std::thread::sleep(Duration::from_millis(300));
    server
        .tmux(&["new-session", "-d", "-s", "it", "-x", "200", "-y", "50"])
        .expect("restart the session");
    let target = server.recording_window("second");
    // Past the first reconnect backoff.
    std::thread::sleep(Duration::from_millis(600));

    sink.text(&target, "after").unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    assert_eq!(settle(&server, "second"), b"after\n".to_vec());
}

#[test]
fn a_noisy_pane_cannot_stall_the_writer() {
    guard!();
    let server = Server::start("noisy");
    let target = server.recording_window("quiet");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "noise", "yes"])
        .expect("create the noisy window");
    let sink = sink_for(&server, true);

    // `-f no-output` is why this is not a deadlock: without it every byte `yes`
    // prints would be mirrored down our stdout, and a reader that fell behind
    // would block the server writing to it.
    let started = Instant::now();
    for _ in 0..200 {
        sink.text(&target, "z").unwrap();
    }
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "the writer stalled behind pane output"
    );
    assert_eq!(
        settle(&server, "quiet"),
        [vec![b'z'; 200], vec![b'\n']].concat()
    );
}

#[test]
fn the_subprocess_backend_delivers_the_same_bytes() {
    guard!();
    let server = Server::start("subproc");
    let target = server.recording_window("subproc");
    // control_mode off: the fallback path, unchanged behaviour, same assertions.
    let sink = sink_for(&server, false);

    sink.text(&target, "abc").unwrap();
    sink.key(&target, "Left").unwrap();
    sink.text(&target, "X").unwrap();
    sink.key(&target, "Enter").unwrap();
    sink.shutdown();

    assert_eq!(settle(&server, "subproc"), b"abc\x1b[DX\n".to_vec());
}

// --- reading the pane back over the same connection ---

/// A pane painted with known text, captured both ways, must come back
/// **byte-identical**.
///
/// The two paths parse different things — one reads a process's stdout, the
/// other reassembles `%begin`/`%end` payload lines — and the popup renders the
/// result through an ANSI parser that will happily draw a subtly wrong frame
/// without failing. Only a byte comparison catches a dropped trailing newline,
/// a mangled escape or a lost blank row.
#[test]
fn a_control_capture_is_byte_identical_to_the_subprocess_one() {
    guard!();
    let server = Server::start("capture");
    // A plain shell, not a recording window: this test needs a pane with
    // *content on screen*, which is what a capture reads.
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "cap", "cat"])
        .expect("create the capture window");
    std::thread::sleep(Duration::from_millis(300));
    let target = "it:cap";
    // Colour, a blank line, a trailing space and a line that looks like a
    // control-mode tag — the payload shapes that could each break framing.
    server
        .tmux(&[
            "send-keys",
            "-t",
            target,
            "-l",
            "plain\n\x1b[31mred\x1b[0m\n\n%end 1 7 0\ntrailing \n",
        ])
        .expect("paint the pane");
    std::thread::sleep(Duration::from_millis(300));

    let ops = TestTmuxOps {
        server: server.name.clone(),
    };
    let expected_content = ops.capture_pane_with_history(target, 500);
    let expected_metrics = ops.pane_metrics(target);

    let sink = sink_for(&server, true);
    let snapshot = sink
        .capture(target, CaptureSpec::popup(500))
        .expect("the control connection served the capture");
    sink.shutdown();

    assert_eq!(
        String::from_utf8_lossy(&snapshot.content),
        String::from_utf8_lossy(&expected_content),
        "the two capture paths disagree about the pane"
    );
    assert_eq!(
        snapshot.metrics, expected_metrics,
        "the two metrics paths disagree about the pane geometry"
    );
    assert!(
        snapshot.content.windows(10).any(|w| w == b"%end 1 7 0"),
        "the tag-shaped line was lost, so framing closed the block early"
    );
}

/// A `CaptureSpec::text()` capture must be **byte-identical** to
/// `TmuxOperations::capture_pane`.
///
/// The status refresh matches trust dialogs and hashes pane content against
/// these bytes. An `-e` left on would embed SGR escapes through the middle of a
/// dialog's wording, so the match would fail and the task would sit at "working"
/// forever with nobody told a decision was waiting — silent, and exactly the
/// class of bug the popup's byte-identity test was written for.
#[test]
fn a_text_capture_matches_the_subprocess_capture_pane() {
    guard!();
    let server = Server::start("captext");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "cap", "cat"])
        .expect("create the capture window");
    std::thread::sleep(Duration::from_millis(300));
    let target = "it:cap";
    server
        .tmux(&[
            "send-keys",
            "-t",
            target,
            "-l",
            "plain\n\x1b[31mDo you trust the files in this folder?\x1b[0m\n",
        ])
        .expect("paint the pane");
    std::thread::sleep(Duration::from_millis(300));

    let ops = TestTmuxOps {
        server: server.name.clone(),
    };
    let expected = ops.capture_pane(target).expect("subprocess capture");

    let sink = sink_for(&server, true);
    let snapshot = sink
        .capture(target, CaptureSpec::text())
        .expect("the control connection served the capture");
    sink.shutdown();

    let got = String::from_utf8_lossy(&snapshot.content).into_owned();
    assert_eq!(got, expected, "the two text-capture paths disagree");
    assert!(
        !got.contains('\u{1b}'),
        "a text capture must carry no escapes: {got:?}"
    );
    assert!(
        got.contains("Do you trust the files in this folder?"),
        "the dialog wording must survive intact for the matcher"
    );
    assert_eq!(
        snapshot.metrics, None,
        "a text capture skips the second command"
    );
}

/// One listing answers `window_exists` for every task on the board.
#[test]
fn the_window_listing_names_every_window_as_session_colon_window() {
    guard!();
    let server = Server::start("winlist");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "alpha", "cat"])
        .expect("create a window");
    std::thread::sleep(Duration::from_millis(200));
    let ops = TestTmuxOps {
        server: server.name.clone(),
    };
    let targets = ops.list_window_targets().expect("listing");
    assert!(
        targets.iter().any(|t| t == "it:alpha"),
        "the listing must use the same `session:window` form task.session_name holds: {targets:?}"
    );
    // What the refresh actually asks of it.
    assert!(ops.window_exists("it:alpha").unwrap_or(false));
    assert!(!targets.iter().any(|t| t == "it:missing"));
}

/// With control mode off there is no connection to serve a capture, and the
/// broker must say so rather than running the subprocess itself — the caller
/// owns that path, and paying for it here would block the next keystroke.
#[test]
fn a_capture_is_declined_when_control_mode_is_off() {
    guard!();
    let server = Server::start("nocapture");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "cap", "cat"])
        .expect("create the capture window");
    std::thread::sleep(Duration::from_millis(300));
    let sink = sink_for(&server, false);
    assert!(sink.capture("it:cap", CaptureSpec::popup(500)).is_none());
    sink.shutdown();
}

/// The number this change was made for: a capture over the input connection
/// against the two `tmux` processes it replaces.
#[test]
fn a_control_capture_beats_two_tmux_processes() {
    guard!();
    let server = Server::start("capbench");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "cap", "cat"])
        .expect("create the capture window");
    std::thread::sleep(Duration::from_millis(300));
    let target = "it:cap";
    let samples = 50;

    let ops = TestTmuxOps {
        server: server.name.clone(),
    };
    let mut subprocess = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let _ = ops.capture_pane_with_history(target, 500);
        let _ = ops.pane_metrics(target);
        subprocess.push(started.elapsed());
    }

    let sink = sink_for(&server, true);
    // One warm-up: the first capture may pay for the connect.
    let _ = sink.capture(target, CaptureSpec::popup(500));
    let mut control = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let snapshot = sink.capture(target, CaptureSpec::popup(500));
        control.push(started.elapsed());
        assert!(snapshot.is_some(), "the control capture stopped working");
    }
    sink.shutdown();

    let (sub_p95, ctl_p95) = (p95(subprocess), p95(control));
    eprintln!(
        "capture p95 — two tmux processes {:?}, control connection {:?} ({})",
        sub_p95,
        ctl_p95,
        agtx_tmux_version()
    );
    // Deliberately loose: this asserts the order of magnitude the popup's
    // refresh rate depends on, not a number a loaded CI box has to hit.
    assert!(
        ctl_p95 * 5 < sub_p95,
        "a control capture ({ctl_p95:?}) should be far cheaper than two processes ({sub_p95:?})"
    );
}

/// tmux must tell the **input** connection that a window closed, even though it
/// is attached with `no-output`.
///
/// This is what turns an exited agent into an `Exited` card promptly rather than
/// on the status refresh's next tick, and it costs nothing: the notification
/// arrives on a connection agtx already holds open for keystrokes.
#[test]
fn a_closing_window_is_reported_on_the_input_connection() {
    guard!();
    let server = Server::start("winclose");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "doomed", "cat"])
        .expect("create the window");
    std::thread::sleep(Duration::from_millis(300));

    let flag = Arc::new(AtomicBool::new(false));
    let client =
        agtx::tmux::ControlClient::connect_with(&server.name, "it", 1, Some(Arc::clone(&flag)))
            .expect("attach the control client");
    std::thread::sleep(Duration::from_millis(400));
    // The attach itself must not look like a window closing.
    assert!(!flag.load(Ordering::Relaxed), "attaching raised the flag");

    server
        .tmux(&["kill-window", "-t", "it:doomed"])
        .expect("kill the window");
    let deadline = Instant::now() + Duration::from_secs(3);
    while !flag.load(Ordering::Relaxed) {
        assert!(
            Instant::now() < deadline,
            "a closing window was never reported on the input connection"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    client.shutdown();
}

// --- %output push: does tmux actually tell us a pane painted? ---

/// The scope assumption the push design rests on, pinned so a tmux upgrade
/// cannot change it in silence: `%output` covers every pane in the **attached**
/// session and no pane outside it.
#[test]
fn output_notifications_cover_the_attached_session_only() {
    guard!();
    let server = Server::start("outscope");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "mine", "cat"])
        .expect("window in the attached session");
    server
        .tmux(&["new-session", "-d", "-s", "other", "-n", "theirs", "cat"])
        .expect("window in another session");
    std::thread::sleep(Duration::from_millis(300));

    let ops = TestTmuxOps {
        server: server.name.clone(),
    };
    let mine = ops.pane_id("it:mine").expect("pane id");
    let theirs = ops.pane_id("other:theirs").expect("pane id");
    assert_ne!(mine, theirs);

    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let sink = Arc::clone(&seen);
    let watch = agtx::tmux::OutputWatch::connect(&server.name, "it", move |id| {
        sink.lock().unwrap().push(id.to_string());
    })
    .expect("attach the output watch");
    std::thread::sleep(Duration::from_millis(400));

    server
        .tmux(&["send-keys", "-t", "it:mine", "-l", "painted\n"])
        .expect("paint the watched pane");
    server
        .tmux(&["send-keys", "-t", "other:theirs", "-l", "painted\n"])
        .expect("paint the other session's pane");
    std::thread::sleep(Duration::from_millis(600));

    let ids = seen.lock().unwrap().clone();
    drop(watch);
    assert!(
        ids.iter().any(|id| *id == mine),
        "the attached session's pane must notify: {ids:?}"
    );
    assert!(
        !ids.iter().any(|id| *id == theirs),
        "a pane outside the attached session must not: {ids:?}"
    );
}

/// A pane that paints notifies **promptly** — this is what replaces the timer,
/// so it has to beat one.
#[test]
fn a_paint_notifies_faster_than_the_poll_interval() {
    guard!();
    let server = Server::start("outlat");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "p", "cat"])
        .expect("create the window");
    std::thread::sleep(Duration::from_millis(300));
    let ops = TestTmuxOps {
        server: server.name.clone(),
    };
    let pane = ops.pane_id("it:p").expect("pane id");

    let (tx, rx) = std::sync::mpsc::channel();
    let want = pane.clone();
    let watch = agtx::tmux::OutputWatch::connect(&server.name, "it", move |id| {
        if id == want {
            let _ = tx.send(Instant::now());
        }
    })
    .expect("attach the output watch");
    std::thread::sleep(Duration::from_millis(400));
    while rx.try_recv().is_ok() {}

    let mut samples = Vec::new();
    for i in 0..10 {
        // Drain *per iteration*: one paint produces several `%output` frames, and
        // reading a leftover from the previous round would time an event that
        // happened before the send — which saturates to zero and makes the whole
        // measurement look perfect.
        while rx.try_recv().is_ok() {}
        let started = Instant::now();
        server
            .tmux(&["send-keys", "-t", "it:p", "-l", &format!("x{i}\n")])
            .expect("paint");
        let at = rx
            .recv_timeout(Duration::from_secs(2))
            .expect("a paint must notify");
        samples.push(at.duration_since(started));
        std::thread::sleep(Duration::from_millis(60));
    }
    drop(watch);
    let p95 = p95(samples);
    eprintln!("paint -> %output p95 {p95:?} ({})", agtx_tmux_version());
    // Loose on purpose: the `send-keys` process in front of it costs more than
    // the notification does. It only has to be in a different class from a poll.
    assert!(
        p95 < Duration::from_millis(100),
        "a paint notification should be prompt, got {p95:?}"
    );
}

/// An idle pane must produce **nothing**. This is the property that makes an
/// open popup free, and the one a timer can never have.
#[test]
fn an_idle_pane_notifies_nothing() {
    guard!();
    let server = Server::start("outidle");
    server
        .tmux(&["new-window", "-d", "-t", "it:", "-n", "quiet", "cat"])
        .expect("create the window");
    std::thread::sleep(Duration::from_millis(400));

    let count = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&count);
    let watch = agtx::tmux::OutputWatch::connect(&server.name, "it", move |_| {
        seen.fetch_add(1, Ordering::Relaxed);
    })
    .expect("attach the output watch");
    // Let the attach settle: a client attaching makes tmux repaint.
    std::thread::sleep(Duration::from_millis(700));
    count.store(0, Ordering::Relaxed);
    std::thread::sleep(Duration::from_secs(2));
    let n = count.load(Ordering::Relaxed);
    drop(watch);
    assert_eq!(n, 0, "an idle pane produced {n} notifications");
}

/// Dropping the watch must close the tmux client. Left running it would mirror
/// the whole session's output for the life of the server, which is the cost the
/// dedicated-client design exists to avoid paying while no popup is open.
#[test]
fn dropping_the_watch_closes_its_client() {
    guard!();
    let server = Server::start("outdrop");
    std::thread::sleep(Duration::from_millis(200));
    let before = server.client_count();
    let watch = agtx::tmux::OutputWatch::connect(&server.name, "it", |_| {})
        .expect("attach the output watch");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(
        server.client_count(),
        before + 1,
        "the watch did not attach"
    );
    drop(watch);
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(
        server.client_count(),
        before,
        "the watch client outlived the watch"
    );
}

// --- the measurement the whole change exists for ---

#[test]
fn control_mode_is_faster_than_a_process_per_key() {
    guard!();
    let server = Server::start("bench");
    let samples = 200;

    // (1) The baseline this whole change exists to remove: one `tmux` process
    //     per key, timed the way an input thread would pay for it.
    let bench_target = server.recording_window("bench");
    let mut subprocess_spawn = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        let _ = Command::new("tmux")
            .args([
                "-L",
                &server.name,
                "send-keys",
                "-t",
                &bench_target,
                "-l",
                "--",
                "a",
            ])
            .output();
        subprocess_spawn.push(started.elapsed());
    }

    // (2) What the TUI thread pays now: an enqueue and nothing else.
    let control = sink_for(&server, true);
    control.text(&bench_target, "warmup").unwrap();
    control.flush().unwrap();
    std::thread::sleep(Duration::from_millis(200));
    let mut enqueue = Vec::with_capacity(samples);
    for _ in 0..samples {
        let started = Instant::now();
        control.text(&bench_target, "b").unwrap();
        enqueue.push(started.elapsed());
    }

    // (3) End to end: enqueue until the byte has reached a program in the pane.
    //     Measured for both backends in the same run, on the same machine, under
    //     whatever load this test suite is putting on it — the only comparison
    //     that means anything.
    let control_target = server.recording_window("ctl");
    let control_delivery = delivery_samples(&server, "ctl", &control_target, control.as_ref(), 60);
    control.shutdown();

    let sub_target = server.recording_window("sub");
    let subprocess = sink_for(&server, false);
    let subprocess_delivery =
        delivery_samples(&server, "sub", &sub_target, subprocess.as_ref(), 60);
    subprocess.shutdown();

    let enqueue_p95 = p95(enqueue);
    let spawn_p95 = p95(subprocess_spawn);
    let control_p95 = p95(control_delivery);
    let subprocess_p95 = p95(subprocess_delivery);
    eprintln!(
        "{}: per-key subprocess spawn p95 {spawn_p95:?} | enqueue p95 {enqueue_p95:?} | \
         delivery p95 control {control_p95:?} vs subprocess {subprocess_p95:?}",
        agtx_tmux_version()
    );

    // The plan's success criteria, as assertions rather than as prose.
    assert!(
        enqueue_p95 < Duration::from_millis(1),
        "the TUI thread must not wait: enqueue p95 was {enqueue_p95:?}"
    );
    assert!(
        enqueue_p95 * 10 < spawn_p95,
        "enqueue {enqueue_p95:?} vs a process spawn {spawn_p95:?}"
    );
    assert!(
        control_p95 < subprocess_p95,
        "control delivery {control_p95:?} was not faster than subprocess {subprocess_p95:?}"
    );
}

/// Time from enqueue until a program in the pane has actually read the byte.
fn delivery_samples(
    server: &Server,
    window: &str,
    target: &str,
    sink: &dyn PaneInputSink,
    samples: usize,
) -> Vec<Duration> {
    let mut out = Vec::with_capacity(samples);
    for _ in 0..samples {
        let before = server.recorded_len(window);
        let started = Instant::now();
        // The Enter is what makes the line discipline hand the line to `cat`;
        // it is part of what is being measured, for both backends alike.
        sink.text(target, "a").unwrap();
        sink.key(target, "Enter").unwrap();
        while server.recorded_len(window) == before {
            if started.elapsed() > Duration::from_secs(2) {
                break;
            }
            std::thread::sleep(Duration::from_micros(100));
        }
        out.push(started.elapsed());
    }
    out
}

fn p95(mut samples: Vec<Duration>) -> Duration {
    samples.sort();
    samples[((samples.len() as f64 * 0.95) as usize).saturating_sub(1)]
}

fn agtx_tmux_version() -> String {
    Command::new("tmux")
        .arg("-V")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "tmux ?".to_string())
}

/// Exists so the file still compiles the imports when every test skips.
#[test]
fn a_pane_input_names_its_target() {
    assert_eq!(
        PaneInput::Text {
            target: "s:w".into(),
            text: "a".into()
        }
        .target(),
        Some("s:w")
    );
}
