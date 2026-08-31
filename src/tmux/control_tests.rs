//! Unit tests for the control-mode encoder and frame parser.
//!
//! Both are pure, and both are places where a wrong answer is silent: a
//! mis-encoded argument reaches tmux as a *different valid command*, and a
//! mis-framed stream desynchronises every later reply. The real-tmux half of the
//! contract lives in `tests/tmux_control_tests.rs`.

use super::*;

// --- encoder ---

#[test]
fn plain_text_is_just_quoted() {
    assert_eq!(tmux_quote("hello"), "\"hello\"");
    assert_eq!(tmux_quote(""), "\"\"");
    assert_eq!(tmux_quote("has space"), "\"has space\"");
}

#[test]
fn the_five_replacement_characters_are_escaped() {
    // Each of these is replaced by tmux inside double quotes, so each must be
    // defused. Verified against tmux 3.5a: unescaped, `$HOME` became the home
    // directory and `#{…}` a format expansion.
    assert_eq!(tmux_quote(r#"a"b"#), r#""a\"b""#);
    assert_eq!(tmux_quote(r"a\b"), r#""a\\b""#);
    assert_eq!(tmux_quote("$HOME"), r#""\$HOME""#);
    assert_eq!(tmux_quote("#{pane_id}"), r#""\#{pane_id}""#);
    assert_eq!(tmux_quote("~/src"), r#""\~/src""#);
}

#[test]
fn a_trailing_backslash_cannot_continue_the_line() {
    // An unescaped trailing `\` joins the next line — the next *command* — into
    // this one's argument.
    assert_eq!(tmux_quote("end\\"), "\"end\\\\\"");
}

#[test]
fn whitespace_controls_become_escapes_not_raw_bytes() {
    // A raw newline would end the command: commands are newline-terminated.
    assert_eq!(tmux_quote("a\nb"), r#""a\nb""#);
    assert_eq!(tmux_quote("a\rb"), r#""a\rb""#);
    assert_eq!(tmux_quote("a\tb"), r#""a\tb""#);
}

#[test]
fn other_control_characters_become_octal() {
    assert_eq!(tmux_quote("\u{1}x"), "\"\\001x\"");
    assert_eq!(tmux_quote("\u{1b}"), "\"\\033\"");
    assert_eq!(tmux_quote("\u{7f}"), "\"\\177\"");
}

#[test]
fn unicode_passes_through_untouched() {
    assert_eq!(tmux_quote("한국어"), "\"한국어\"");
    assert_eq!(tmux_quote("😀"), "\"😀\"");
    assert_eq!(tmux_quote("naïve"), "\"naïve\"");
}

#[test]
fn text_that_spells_a_key_name_is_still_text() {
    // The encoder does not care — `-l` is what keeps these literal — but the
    // quoting must not mangle them either.
    for name in ["Space", "Enter", "Up", "C-c", "-x", "--"] {
        assert_eq!(tmux_quote(name), format!("\"{name}\""));
    }
}

#[test]
fn a_semicolon_does_not_start_a_second_command() {
    // Inside double quotes a `;` is literal, which is why it needs no escape —
    // asserted so a future "escape everything" rewrite has to justify itself.
    assert_eq!(tmux_quote("a;b"), "\"a;b\"");
}

// --- frame parser ---

fn frames(chunks: &[&str]) -> Vec<Frame> {
    let mut parser = FrameParser::new();
    let mut out = Vec::new();
    for chunk in chunks {
        parser.push(chunk.as_bytes());
        while let Some(frame) = parser.next_frame() {
            out.push(frame);
        }
    }
    out
}

#[test]
fn a_command_block_is_framed() {
    assert_eq!(
        frames(&["%begin 1788074601 521 0\nhello\n%end 1788074601 521 0\n"]),
        vec![
            Frame::Begin { cmd: 521 },
            Frame::Payload("hello".to_string()),
            Frame::End { cmd: 521 },
        ]
    );
}

#[test]
fn an_error_block_carries_its_message() {
    assert_eq!(
        frames(&["%begin 1 7 1\ncan't find window: w2\n%error 1 7 1\n"]),
        vec![
            Frame::Begin { cmd: 7 },
            Frame::Payload("can't find window: w2".to_string()),
            Frame::Error { cmd: 7 },
        ]
    );
}

#[test]
fn reads_may_split_a_line_anywhere() {
    // A pipe read is not a line. Splitting mid-token must not lose or duplicate
    // a frame.
    assert_eq!(
        frames(&["%beg", "in 1 3 0\npay", "load\n%end 1 ", "3 0\n"]),
        vec![
            Frame::Begin { cmd: 3 },
            Frame::Payload("payload".to_string()),
            Frame::End { cmd: 3 },
        ]
    );
}

#[test]
fn one_read_may_carry_many_frames() {
    let out = frames(&["%begin 1 1 0\n%end 1 1 0\n%window-add @2\n%begin 1 2 0\n%end 1 2 0\n"]);
    assert_eq!(out.len(), 5);
    assert_eq!(out[2], Frame::Notify("%window-add @2".to_string()));
}

#[test]
fn a_partial_trailing_line_yields_nothing_yet() {
    let mut parser = FrameParser::new();
    parser.push(b"%begin 1 1 0\n%en");
    assert_eq!(parser.next_frame(), Some(Frame::Begin { cmd: 1 }));
    assert_eq!(parser.next_frame(), None);
    parser.push(b"d 1 1 0\n");
    assert_eq!(parser.next_frame(), Some(Frame::End { cmd: 1 }));
}

#[test]
fn notifications_outside_a_block_are_notifications() {
    assert_eq!(
        frames(&["%session-changed $0 s\n%exit\n"]),
        vec![
            Frame::Notify("%session-changed $0 s".to_string()),
            Frame::Notify("%exit".to_string()),
        ]
    );
}

#[test]
fn a_payload_line_starting_with_percent_is_payload() {
    // `display-message -p` can print anything, `%exit` included. Position in the
    // stream decides, not the leading character — otherwise a pane full of
    // percent signs would look like the server going away.
    assert_eq!(
        frames(&["%begin 1 1 0\n%exit\n%end 1 1 0\n"]),
        vec![
            Frame::Begin { cmd: 1 },
            Frame::Payload("%exit".to_string()),
            Frame::End { cmd: 1 },
        ]
    );
}

#[test]
fn an_output_notification_yields_its_pane_id() {
    assert_eq!(output_pane_id("%output %7 hello"), Some("%7"));
    assert_eq!(output_pane_id("%output %12 "), Some("%12"));
    // The payload is arbitrary bytes, escaped by tmux; only the id is taken.
    assert_eq!(
        output_pane_id("%output %3 \\033[31mred\\033[0m %output %9 not-an-id"),
        Some("%3")
    );
}

#[test]
fn a_notification_that_is_not_an_output_yields_no_pane_id() {
    // Guessing an id from a notification we do not understand would signal the
    // wrong pane, which reads as "some other task painted" and captures for it.
    for line in [
        "%exit",
        "%window-add @2",
        "%output",
        "%output notapane data",
        "%output % data",
        "%output %x1 data",
        "%outputs %1 data",
    ] {
        assert_eq!(
            output_pane_id(line),
            None,
            "{line:?} must not parse as a pane id"
        );
    }
}

#[test]
fn a_payload_line_that_looks_like_an_output_notification_is_payload() {
    // A capture of a pane that is itself showing control-mode output. Inside a
    // block it is that command's reply, not a pane painting — position decides,
    // as it does for every other tag.
    assert_eq!(
        frames(&["%begin 1 4 0\n%output %9 painted\n%end 1 4 0\n"]),
        vec![
            Frame::Begin { cmd: 4 },
            Frame::Payload("%output %9 painted".to_string()),
            Frame::End { cmd: 4 },
        ]
    );
}

#[test]
fn a_payload_line_that_looks_like_an_end_tag_does_not_close_the_block() {
    // A block's payload is now a whole pane capture, so its lines are whatever
    // an agent chose to paint — `%end 1 7 0` included. Closing on the tag alone
    // would end the block early, hand the caller half a capture, and leave
    // `completed` permanently ahead of the commands that were actually run,
    // which every barrier and query counts on. tmux pairs each `%end` with its
    // `%begin`'s id, so the id is what decides.
    assert_eq!(
        frames(&["%begin 1 4 0\n%end 1 7 0\nreal content\n%end 1 4 0\n"]),
        vec![
            Frame::Begin { cmd: 4 },
            Frame::Payload("%end 1 7 0".to_string()),
            Frame::Payload("real content".to_string()),
            Frame::End { cmd: 4 },
        ]
    );
}

#[test]
fn a_payload_line_that_looks_like_an_error_tag_does_not_fail_the_block() {
    assert_eq!(
        frames(&["%begin 1 4 0\n%error 1 9 0\n%end 1 4 0\n"]),
        vec![
            Frame::Begin { cmd: 4 },
            Frame::Payload("%error 1 9 0".to_string()),
            Frame::End { cmd: 4 },
        ]
    );
}

#[test]
fn a_malformed_begin_still_frames_the_block() {
    // Losing the command number costs a log field. Losing the framing would
    // desynchronise every later reply, so the parser is lenient here.
    assert_eq!(
        frames(&["%begin\npayload\n%end\n"]),
        vec![
            Frame::Begin { cmd: 0 },
            Frame::Payload("payload".to_string()),
            Frame::End { cmd: 0 },
        ]
    );
}

#[test]
fn a_notification_that_merely_starts_like_a_tag_is_not_one() {
    assert_eq!(
        frames(&["%begin 1 1 0\n%end 1 1 0\n%exit-something-else\n"])
            .last()
            .cloned(),
        Some(Frame::Notify("%exit-something-else".to_string()))
    );
}

#[test]
fn crlf_line_endings_are_tolerated() {
    assert_eq!(
        frames(&["%begin 1 1 0\r\nline\r\n%end 1 1 0\r\n"]),
        vec![
            Frame::Begin { cmd: 1 },
            Frame::Payload("line".to_string()),
            Frame::End { cmd: 1 },
        ]
    );
}

#[test]
fn invalid_utf8_does_not_stall_the_parser() {
    let mut parser = FrameParser::new();
    parser.push(b"%begin 1 1 0\n\xff\xfe\n%end 1 1 0\n");
    assert_eq!(parser.next_frame(), Some(Frame::Begin { cmd: 1 }));
    assert!(matches!(parser.next_frame(), Some(Frame::Payload(_))));
    assert_eq!(parser.next_frame(), Some(Frame::End { cmd: 1 }));
}
