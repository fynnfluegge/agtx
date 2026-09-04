//! Live pane frames over a WebSocket.
//!
//! **Snapshot frames, not a PTY.** Each frame is a whole `capture-pane -p -e`
//! of the pane, so nothing has to be replayed in order and a dropped frame
//! costs one refresh rather than desynchronising a terminal emulator. Measured:
//! `capture-pane -e` emits *only* `ESC[…m` — no cursor motion, no scroll
//! regions — because it is a snapshot of an already-rendered grid. That is why
//! the client renders it with a small SGR-to-HTML pass rather than a terminal
//! emulator; there is no terminal control in the stream to emulate.
//!
//! **One capture loop per socket, and only while subscribed.** A capture makes
//! the tmux server format an entire pane, so the cost is real and it is paid
//! per subscriber. Nothing is captured for a task nobody is looking at.

use std::sync::Arc;
use std::time::Duration;

use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::Response,
};
use serde::{Deserialize, Serialize};
use tokio::time::interval;

use crate::tmux::{RealTmuxOps, TmuxOperations};

use super::state::ServerState;

/// How often a subscribed pane is captured.
///
/// Matches the TUI watcher's own ceiling for agent output rather than its
/// typing-window floor: a phone is watching an agent work, not echoing its own
/// keystrokes, and the floor exists for local echo latency that does not apply
/// over a tunnel.
const FRAME_INTERVAL: Duration = Duration::from_millis(250);

/// Rows captured per frame. Enough to fill a phone screen with room to scroll,
/// far short of the popup's 500-line scrollback spec — this is a live view, not
/// a history browser.
const FRAME_LINES: i32 = 200;

/// What a client may say. Public because it *is* the wire contract: the
/// browser has a hand-written encoder for it, and a rename here that nothing
/// pins would break the live view silently — the socket would connect, the
/// subscribe would be rejected as a bad message, and the pane would sit empty.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Start streaming a task's pane. Replaces any previous subscription: a
    /// socket watches one pane, because a phone shows one screen.
    Subscribe {
        project_id: String,
        task_id: String,
    },
    Unsubscribe,
}

/// What the server sends back. Public for the same reason as [`ClientMessage`].
#[derive(Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage<'a> {
    Frame {
        task_id: &'a str,
        content: &'a str,
    },
    /// The pane went away — the agent exited, or the window was killed. Sent
    /// once rather than every tick, so a client can say so and stop waiting.
    Gone {
        task_id: &'a str,
    },
    Error {
        message: String,
    },
}

pub async fn handler(
    ws: WebSocketUpgrade,
    State(state): State<Arc<ServerState>>,
    headers: axum::http::HeaderMap,
) -> Response {
    // Authentication already happened in the router's auth layer, which reads
    // the same subprotocol — it has to run *before* `WebSocketUpgrade` accepts
    // the upgrade, and an extractor cannot.
    //
    // What is left here is echoing the subprotocol back: a browser that offered
    // one and receives nothing fails the handshake itself, which surfaces as a
    // network error rather than as anything to do with tokens.
    let offered = headers
        .get(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| {
            v.split(',')
                .map(str::trim)
                .find(|p| p.starts_with(super::auth::WS_TOKEN_PREFIX))
                .map(str::to_string)
        });

    let upgrade = match offered {
        Some(proto) => ws.protocols([proto]),
        None => ws,
    };
    upgrade.on_upgrade(move |socket| run(socket, state))
}

async fn run(mut socket: WebSocket, state: Arc<ServerState>) {
    let mut sub: Option<Subscription> = None;
    let mut ticker = interval(FRAME_INTERVAL);
    let mut last_sent: Option<String> = None;

    loop {
        tokio::select! {
            // A client message. `None` is a closed socket.
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Subscribe { project_id, task_id }) => {
                                last_sent = None;
                                match Subscription::open(&state, &project_id, &task_id) {
                                    Ok(s) => sub = Some(s),
                                    Err(message) => {
                                        let _ = send(&mut socket, &ServerMessage::Error { message }).await;
                                        sub = None;
                                    }
                                }
                            }
                            Ok(ClientMessage::Unsubscribe) => {
                                // Stops the captures immediately rather than at
                                // the next tick: this is the message a client
                                // sends when its screen closed.
                                sub = None;
                                last_sent = None;
                            }
                            Err(e) => {
                                let _ = send(
                                    &mut socket,
                                    &ServerMessage::Error { message: format!("bad message: {e}") },
                                )
                                .await;
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    // Ping/pong are handled by axum; other frames are ignored.
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }

            _ = ticker.tick() => {
                let Some(ref s) = sub else { continue };
                match s.capture() {
                    Some(content) => {
                        // Only send what changed. An idle agent is the common
                        // case, and re-sending an identical pane every 250ms
                        // over a tunnel is the cost this avoids.
                        if last_sent.as_deref() != Some(content.as_str()) {
                            let msg = ServerMessage::Frame { task_id: &s.task_id, content: &content };
                            if send(&mut socket, &msg).await.is_err() {
                                break;
                            }
                            last_sent = Some(content);
                        }
                    }
                    None => {
                        let msg = ServerMessage::Gone { task_id: &s.task_id };
                        let _ = send(&mut socket, &msg).await;
                        // Stop capturing a pane that is gone; the client
                        // resubscribes if the task comes back.
                        sub = None;
                        last_sent = None;
                    }
                }
            }
        }
    }
}

async fn send(socket: &mut WebSocket, msg: &ServerMessage<'_>) -> Result<(), axum::Error> {
    let text = serde_json::to_string(msg).unwrap_or_else(|_| "{}".to_string());
    socket.send(Message::Text(text.into())).await
}

/// What a socket needs to capture one pane, resolved once at subscribe time
/// rather than per frame — the task's session name does not change while it is
/// being watched, and looking it up four times a second would put a SQLite read
/// on the frame path.
pub struct Subscription {
    task_id: String,
    session_name: String,
    /// Captures go through `TmuxOperations` rather than a hand-rolled `tmux`
    /// invocation. Two reasons, and the second is the one that bites: the
    /// socket name lives in exactly one place (`tmux::AGENT_SERVER`), and a
    /// second spelling of `capture-pane` here would be free to drift from the
    /// flags the popup uses — `-p -e` with wrapped rows kept separate — which
    /// is what the client's renderer is written against.
    tmux: Arc<dyn TmuxOperations>,
}

impl Subscription {
    pub fn open(state: &ServerState, project_id: &str, task_id: &str) -> Result<Self, String> {
        let db = state
            .project_db(project_id)
            .map_err(|_| format!("no project {project_id}"))?;
        let task = db
            .get_task(task_id)
            .map_err(|e| format!("reading task: {e}"))?
            .ok_or_else(|| format!("no task {task_id}"))?;
        let session_name = task
            .session_name
            .ok_or_else(|| format!("task {task_id} has no active session"))?;
        Ok(Self {
            task_id: task_id.to_string(),
            session_name,
            tmux: Arc::new(RealTmuxOps),
        })
    }

    /// `None` means the pane is gone, which is distinct from an empty pane —
    /// a window whose agent exited returns no bytes *and* no window, and only
    /// the first of those is worth telling the client about.
    fn capture(&self) -> Option<String> {
        if !self.tmux.window_exists(&self.session_name).unwrap_or(false) {
            return None;
        }
        let bytes = self
            .tmux
            .capture_pane_with_history(&self.session_name, FRAME_LINES);
        Some(String::from_utf8_lossy(&bytes).to_string())
    }
}
