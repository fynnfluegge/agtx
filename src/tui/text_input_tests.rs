use super::*;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn alt(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::ALT)
}

fn typed(text: &str) -> TextInput {
    let mut input = TextInput::new();
    for c in text.chars() {
        input.insert_char(c);
    }
    input
}

#[test]
fn typing_advances_the_caret() {
    let input = typed("hello");
    assert_eq!(input.as_str(), "hello");
    assert_eq!(input.cursor, 5);
}

#[test]
fn insert_happens_at_the_caret_not_the_end() {
    let mut input = typed("helo");
    input.left();
    input.insert_char('l');
    assert_eq!(input.as_str(), "hello");
    assert_eq!(input.cursor, 4);
}

#[test]
fn backspace_deletes_before_the_caret() {
    let mut input = typed("hello");
    input.backspace();
    assert_eq!(input.as_str(), "hell");
    assert_eq!(input.cursor, 4);
}

#[test]
fn backspace_at_the_start_is_a_no_op() {
    let mut input = typed("hi");
    input.home();
    input.backspace();
    assert_eq!(input.as_str(), "hi");
    assert_eq!(input.cursor, 0);
}

#[test]
fn delete_forward_removes_the_char_under_the_caret() {
    let mut input = typed("hello");
    input.home();
    input.delete_forward();
    assert_eq!(input.as_str(), "ello");
    assert_eq!(input.cursor, 0);
}

#[test]
fn delete_forward_at_the_end_is_a_no_op() {
    let mut input = typed("hi");
    input.delete_forward();
    assert_eq!(input.as_str(), "hi");
}

/// The reason the cursor is snapped rather than incremented: a byte-stepping
/// caret lands inside a multi-byte char, and the next `drain` panics.
#[test]
fn motion_lands_on_char_boundaries_for_multibyte_text() {
    let mut input = typed("héllo");
    input.home();
    input.right();
    assert_eq!(input.cursor, 1);
    input.right();
    assert_eq!(input.cursor, 3, "é is two bytes, so the caret skips both");
    input.left();
    assert_eq!(input.cursor, 1);
}

#[test]
fn backspace_removes_a_whole_multibyte_char() {
    let mut input = typed("aé");
    input.backspace();
    assert_eq!(input.as_str(), "a");
    assert_eq!(input.cursor, 1);
}

#[test]
fn backspace_removes_a_whole_emoji() {
    let mut input = typed("ok🚀");
    input.backspace();
    assert_eq!(input.as_str(), "ok");
    assert_eq!(input.cursor, 2);
}

#[test]
fn word_motion_crosses_words_and_stops_at_the_ends() {
    let mut input = typed("hello world");
    input.word_left();
    assert_eq!(input.cursor, 6);
    input.word_left();
    assert_eq!(input.cursor, 0);
    input.word_left();
    assert_eq!(input.cursor, 0, "already at the start");
    input.word_right();
    assert_eq!(input.cursor, 6);
    input.word_right();
    assert_eq!(input.cursor, 11);
    input.word_right();
    assert_eq!(input.cursor, 11, "already at the end");
}

#[test]
fn delete_word_left_removes_the_preceding_word() {
    let mut input = typed("hello world");
    input.delete_word_left();
    assert_eq!(input.as_str(), "hello ");
    assert_eq!(input.cursor, 6);
}

#[test]
fn set_text_puts_the_caret_at_the_end() {
    let mut input = typed("old");
    input.home();
    input.set_text("a longer value");
    assert_eq!(input.as_str(), "a longer value");
    assert_eq!(input.cursor, 14);
}

#[test]
fn clear_resets_both_halves() {
    let mut input = typed("something");
    input.clear();
    assert!(input.is_empty());
    assert_eq!(input.cursor, 0);
}

// --- handle_edit_key ---

#[test]
fn edit_keys_are_consumed_and_applied() {
    let mut input = typed("hello");
    assert!(input.handle_edit_key(key(KeyCode::Home)));
    assert_eq!(input.cursor, 0);
    assert!(input.handle_edit_key(key(KeyCode::End)));
    assert_eq!(input.cursor, 5);
    assert!(input.handle_edit_key(key(KeyCode::Backspace)));
    assert_eq!(input.as_str(), "hell");
    assert!(input.handle_edit_key(key(KeyCode::Char('o'))));
    assert_eq!(input.as_str(), "hello");
}

/// macOS sends Option+Left/Right as Alt+b / Alt+f, so both spellings have to
/// move by a word rather than typing a literal `b` or `f`.
#[test]
fn alt_b_and_alt_f_move_by_word_instead_of_typing() {
    let mut input = typed("hello world");
    input.handle_edit_key(alt(KeyCode::Char('b')));
    assert_eq!(input.cursor, 6);
    assert_eq!(input.as_str(), "hello world", "no literal b was inserted");
    input.handle_edit_key(alt(KeyCode::Char('f')));
    assert_eq!(input.cursor, 11);
    assert_eq!(input.as_str(), "hello world");
}

#[test]
fn alt_arrows_move_by_word() {
    let mut input = typed("hello world");
    input.handle_edit_key(alt(KeyCode::Left));
    assert_eq!(input.cursor, 6);
    input.handle_edit_key(alt(KeyCode::Right));
    assert_eq!(input.cursor, 11);
}

#[test]
fn alt_backspace_deletes_a_word() {
    let mut input = typed("hello world");
    input.handle_edit_key(alt(KeyCode::Backspace));
    assert_eq!(input.as_str(), "hello ");
}

/// The caller keeps a fallback arm, so an unhandled key must be reported
/// rather than swallowed — and Enter/Esc are the caller's to interpret.
#[test]
fn unhandled_keys_are_declined() {
    let mut input = typed("x");
    assert!(!input.handle_edit_key(key(KeyCode::Enter)));
    assert!(!input.handle_edit_key(key(KeyCode::Esc)));
    assert!(!input.handle_edit_key(key(KeyCode::Tab)));
    assert!(!input.handle_edit_key(key(KeyCode::Up)));
    assert_eq!(input.as_str(), "x", "declined keys change nothing");
}

/// A field that gives `/` its own meaning declines it in its own match and
/// falls through to here, where it must arrive as ordinary text.
#[test]
fn trigger_characters_insert_when_the_caller_declines_them() {
    let mut input = TextInput::new();
    for c in ['#', '/', '!', '@'] {
        assert!(input.handle_edit_key(key(KeyCode::Char(c))));
    }
    assert_eq!(input.as_str(), "#/!@");
}

// --- word boundaries ---
//
// These moved here from `app_tests.rs` with the functions themselves, where
// they had accumulated two overlapping copies. One home for the code, one for
// its tests.

#[test]
fn word_boundary_left_from_the_end_of_the_string() {
    assert_eq!(word_boundary_left("hello world", 11), 6);
}

#[test]
fn word_boundary_left_from_a_word_start_crosses_to_the_previous_one() {
    assert_eq!(word_boundary_left("hello world", 6), 0);
}

#[test]
fn word_boundary_left_from_the_end_of_the_first_word() {
    assert_eq!(word_boundary_left("hello world", 5), 0);
}

#[test]
fn word_boundary_left_from_mid_word() {
    assert_eq!(word_boundary_left("hello world", 8), 6);
}

#[test]
fn word_boundary_left_at_the_start_stays() {
    assert_eq!(word_boundary_left("hello", 0), 0);
    assert_eq!(word_boundary_left("hello world", 0), 0);
}

#[test]
fn word_boundary_left_skips_a_run_of_spaces() {
    assert_eq!(word_boundary_left("hello   world", 13), 8);
}

/// Punctuation is not a word character, so a path walks segment by segment.
#[test]
fn word_boundary_left_stops_inside_a_path() {
    assert_eq!(word_boundary_left("src/main.rs", 11), 9);
}

#[test]
fn word_boundary_right_from_the_start() {
    assert_eq!(word_boundary_right("hello world", 0), 6);
}

#[test]
fn word_boundary_right_from_a_space() {
    assert_eq!(word_boundary_right("hello world", 5), 6);
}

#[test]
fn word_boundary_right_from_mid_word() {
    assert_eq!(word_boundary_right("hello world", 3), 6);
    assert_eq!(word_boundary_right("hello world", 2), 6);
}

#[test]
fn word_boundary_right_at_the_end_stays() {
    assert_eq!(word_boundary_right("hello", 5), 5);
}

#[test]
fn word_boundary_right_skips_a_run_of_spaces() {
    assert_eq!(word_boundary_right("hello   world", 0), 8);
}

#[test]
fn word_boundary_right_stops_inside_a_path() {
    assert_eq!(word_boundary_right("src/main.rs", 0), 4);
}

#[test]
fn word_boundaries_handle_the_empty_string() {
    assert_eq!(word_boundary_left("", 0), 0);
    assert_eq!(word_boundary_right("", 0), 0);
}

#[test]
fn word_boundary_round_trip_returns_to_the_start() {
    let s = "hello world foo";
    let pos = word_boundary_right(s, 0); // -> 6 (start of "world")
    let pos = word_boundary_right(s, pos); // -> 12 (start of "foo")
    let pos = word_boundary_left(s, pos); // -> 6 (start of "world")
    let pos = word_boundary_left(s, pos); // -> 0 (start of "hello")
    assert_eq!(pos, 0);
}

// --- char boundaries ---

#[test]
fn char_boundaries_clamp_at_the_ends() {
    assert_eq!(prev_char_boundary("abc", 0), 0);
    assert_eq!(next_char_boundary("abc", 3), 3);
    assert_eq!(next_char_boundary("abc", 99), 3);
}

/// The whole reason these exist: a byte offset landing mid-codepoint would
/// panic the next time anything sliced the buffer.
#[test]
fn char_boundaries_never_land_mid_codepoint() {
    let s = "héllo🚀!";
    for pos in 0..=s.len() {
        assert!(s.is_char_boundary(prev_char_boundary(s, pos)));
        assert!(s.is_char_boundary(next_char_boundary(s, pos)));
    }
}

/// A chord is not text. `Ctrl+X` used to type a literal "x" into every field in
/// the TUI, and a caller that gives a chord its own meaning could never get the
/// key back.
#[test]
fn modified_characters_are_declined_rather_than_typed() {
    let mut input = typed("abc");
    for modifiers in [KeyModifiers::CONTROL, KeyModifiers::ALT] {
        assert!(!input.handle_edit_key(KeyEvent::new(KeyCode::Char('x'), modifiers)));
    }
    assert_eq!(input.as_str(), "abc");
}

/// The two exceptions, which are word motion rather than text: macOS sends
/// Option+Left/Right as Alt+b / Alt+f.
#[test]
fn alt_b_and_alt_f_remain_the_exception() {
    let mut input = typed("hello world");
    assert!(input.handle_edit_key(alt(KeyCode::Char('b'))));
    assert!(input.handle_edit_key(alt(KeyCode::Char('f'))));
    assert_eq!(input.as_str(), "hello world");
}
