//! Pre-handshake stdin filter for the MCP server.
//!
//! rmcp refuses to start if the first client message is anything other than
//! `initialize` — it returns `ExpectedInitializeRequest` and the process exits.
//! Antigravity CLI (`agy`) probes every stdio server with a custom
//! `server/discover` request *before* `initialize`, so an unfiltered rmcp server
//! dies during discovery and `agy`'s follow-up `initialize` hits a closed pipe
//! ("connection closed: calling \"initialize\": client is closing: EOF").
//!
//! JSON-RPC already has the right answer for an unknown method: reply with
//! -32601 and keep the connection open. This module does exactly that for
//! requests arriving before `initialize`, and forwards everything else
//! untouched. Once `initialize` has been forwarded, the filter is a pass-through
//! — rmcp handles unknown methods itself from then on.

use serde_json::Value;

/// JSON-RPC "method not found".
const METHOD_NOT_FOUND: i32 = -32601;

/// What to do with a client message that arrived before `initialize`.
#[derive(Debug, PartialEq, Eq)]
pub enum PreInitAction {
    /// Pass the line through to rmcp untouched.
    Forward,
    /// Answer with a -32601 error and do not forward. Carries the serialized
    /// response line.
    Reject(String),
    /// Drop silently — a notification we cannot answer and rmcp would reject.
    Drop,
}

/// Classify a single client line seen before the handshake completes.
///
/// Anything we cannot parse is forwarded: rmcp's own error handling is a better
/// place to deal with malformed input than a guess here.
pub fn classify_pre_init(line: &str) -> PreInitAction {
    let value: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(_) => return PreInitAction::Forward,
    };

    let method = match value.get("method").and_then(Value::as_str) {
        Some(m) => m,
        // A response or malformed object — not ours to interpret.
        None => return PreInitAction::Forward,
    };

    if method == "initialize" {
        return PreInitAction::Forward;
    }

    match value.get("id") {
        // A request: answer it so the client can move on to `initialize`.
        Some(id) if !id.is_null() => {
            let response = serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {
                    "code": METHOD_NOT_FOUND,
                    "message": format!("Method not found: {method}"),
                },
            });
            PreInitAction::Reject(response.to_string())
        }
        // A notification: nothing to answer, and forwarding it would abort the
        // handshake.
        _ => PreInitAction::Drop,
    }
}

/// Tracks whether `initialize` has been forwarded yet.
#[derive(Debug, Default)]
pub struct HandshakeFilter {
    initialized: bool,
}

impl HandshakeFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// Classify the next client line. After `initialize` has gone through, every
    /// line is forwarded — rmcp answers unknown methods correctly once running.
    pub fn next(&mut self, line: &str) -> PreInitAction {
        if self.initialized {
            return PreInitAction::Forward;
        }
        let action = classify_pre_init(line);
        if action == PreInitAction::Forward && line.contains("\"initialize\"") {
            // Only flip once the message we forwarded really was the handshake;
            // `classify_pre_init` forwards unparseable lines too.
            if let Ok(v) = serde_json::from_str::<Value>(line) {
                if v.get("method").and_then(Value::as_str) == Some("initialize") {
                    self.initialized = true;
                }
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_line() -> String {
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "capabilities": {}}
        })
        .to_string()
    }

    #[test]
    fn initialize_is_forwarded() {
        assert_eq!(classify_pre_init(&init_line()), PreInitAction::Forward);
    }

    #[test]
    fn agy_server_discover_is_rejected_not_forwarded() {
        // The exact probe Antigravity CLI 1.1.19 sends before `initialize`.
        let line = r#"{"jsonrpc":"2.0","id":1,"method":"server/discover","params":{}}"#;
        match classify_pre_init(line) {
            PreInitAction::Reject(response) => {
                let v: Value = serde_json::from_str(&response).unwrap();
                assert_eq!(v["id"], 1);
                assert_eq!(v["error"]["code"], METHOD_NOT_FOUND);
                assert!(
                    v["error"]["message"]
                        .as_str()
                        .unwrap()
                        .contains("server/discover"),
                    "message should name the method: {response}"
                );
                // A rejection must never look like a success to the client.
                assert!(v.get("result").is_none());
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn pre_init_notification_is_dropped() {
        let line = r#"{"jsonrpc":"2.0","method":"notifications/cancelled"}"#;
        assert_eq!(classify_pre_init(line), PreInitAction::Drop);
    }

    #[test]
    fn null_id_counts_as_notification() {
        let line = r#"{"jsonrpc":"2.0","id":null,"method":"server/discover"}"#;
        assert_eq!(classify_pre_init(line), PreInitAction::Drop);
    }

    #[test]
    fn malformed_input_is_left_to_rmcp() {
        assert_eq!(classify_pre_init("not json"), PreInitAction::Forward);
        assert_eq!(classify_pre_init(""), PreInitAction::Forward);
    }

    #[test]
    fn string_ids_are_echoed_verbatim() {
        let line = r#"{"jsonrpc":"2.0","id":"abc-1","method":"server/discover"}"#;
        match classify_pre_init(line) {
            PreInitAction::Reject(response) => {
                let v: Value = serde_json::from_str(&response).unwrap();
                assert_eq!(v["id"], "abc-1");
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn filter_passes_everything_through_after_initialize() {
        let mut filter = HandshakeFilter::new();
        // The agy probe first...
        assert!(matches!(
            filter.next(r#"{"jsonrpc":"2.0","id":1,"method":"server/discover"}"#),
            PreInitAction::Reject(_)
        ));
        // ...then the real handshake...
        assert_eq!(filter.next(&init_line()), PreInitAction::Forward);
        // ...after which rmcp owns error handling, including for the same probe.
        assert_eq!(
            filter.next(r#"{"jsonrpc":"2.0","id":2,"method":"server/discover"}"#),
            PreInitAction::Forward
        );
        assert_eq!(
            filter.next(r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#),
            PreInitAction::Forward
        );
    }

    #[test]
    fn unparseable_line_does_not_flip_the_initialized_flag() {
        let mut filter = HandshakeFilter::new();
        // Contains the word but is not a handshake — must not open the gate.
        assert_eq!(
            filter.next(r#"garbage "initialize" garbage"#),
            PreInitAction::Forward
        );
        assert!(matches!(
            filter.next(r#"{"jsonrpc":"2.0","id":9,"method":"server/discover"}"#),
            PreInitAction::Reject(_)
        ));
    }
}
