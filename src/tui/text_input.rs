//! A single-line-buffer editor shared by every text field in the TUI.
//!
//! The wizard's title and prompt steps had a copy each of the same ~110 lines
//! of motion and deletion handling, so an editing fix had to be made twice and
//! the two drifted. This is that logic, once.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Text plus the caret, as one value.
///
/// The cursor is a **byte** offset rather than a char index because callers
/// slice the buffer directly — the `#`/`/`/`!` dropdowns splice at byte ranges
/// they computed themselves. That is also why every motion goes through the
/// boundary helpers below: `String` indexing panics mid-codepoint, so a cursor
/// left inside a multi-byte char is a crash waiting for the next keystroke.
///
/// The fields are public on purpose. Those dropdowns do surgery this type has
/// no business modelling, and hiding the buffer behind accessors would only
/// have them call `.buffer_mut()` for the same effect with more ceremony.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextInput {
    pub buffer: String,
    pub cursor: usize,
}

impl TextInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Replace the contents, putting the caret at the end — what every caller
    /// that loads an existing value into a field wants.
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.buffer = text.into();
        self.cursor = self.buffer.len();
    }

    pub fn clear(&mut self) {
        self.buffer.clear();
        self.cursor = 0;
    }

    pub fn is_empty(&self) -> bool {
        self.buffer.is_empty()
    }

    pub fn as_str(&self) -> &str {
        &self.buffer
    }

    pub fn insert_char(&mut self, c: char) {
        self.buffer.insert(self.cursor, c);
        self.cursor += c.len_utf8();
    }

    pub fn backspace(&mut self) {
        if self.cursor > 0 {
            let start = prev_char_boundary(&self.buffer, self.cursor);
            self.buffer.drain(start..self.cursor);
            self.cursor = start;
        }
    }

    pub fn delete_forward(&mut self) {
        if self.cursor < self.buffer.len() {
            let end = next_char_boundary(&self.buffer, self.cursor);
            self.buffer.drain(self.cursor..end);
        }
    }

    pub fn delete_word_left(&mut self) {
        let start = word_boundary_left(&self.buffer, self.cursor);
        self.buffer.drain(start..self.cursor);
        self.cursor = start;
    }

    pub fn left(&mut self) {
        self.cursor = prev_char_boundary(&self.buffer, self.cursor);
    }

    pub fn right(&mut self) {
        self.cursor = next_char_boundary(&self.buffer, self.cursor);
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.buffer.len();
    }

    pub fn word_left(&mut self) {
        self.cursor = word_boundary_left(&self.buffer, self.cursor);
    }

    pub fn word_right(&mut self) {
        self.cursor = word_boundary_right(&self.buffer, self.cursor);
    }

    /// Apply one editing key, reporting whether it was consumed.
    ///
    /// Callers match their **own** keys first and delegate the rest here, so a
    /// field that gives `/` a special meaning still gets `/` inserted verbatim
    /// when its own guard declines. Returning `false` rather than swallowing
    /// the key is what lets a caller keep a fallback arm.
    ///
    /// `Enter` and `Esc` are deliberately absent: they mean submit, newline,
    /// step-back or cancel depending on the field, and none of those is this
    /// type's decision.
    pub fn handle_edit_key(&mut self, key: KeyEvent) -> bool {
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            // macOS sends Option+Left/Right as Alt+b / Alt+f, so both spellings
            // of word motion have to be here.
            KeyCode::Left if alt => self.word_left(),
            KeyCode::Right if alt => self.word_right(),
            KeyCode::Char('b') if alt => self.word_left(),
            KeyCode::Char('f') if alt => self.word_right(),
            KeyCode::Backspace if alt => self.delete_word_left(),
            KeyCode::Left => self.left(),
            KeyCode::Right => self.right(),
            KeyCode::Home => self.home(),
            KeyCode::End => self.end(),
            KeyCode::Backspace => self.backspace(),
            KeyCode::Delete => self.delete_forward(),
            // A chord is not text. Without this guard `Ctrl+X` typed a literal
            // "x" into every field in the TUI, and a caller that gives a chord
            // its own meaning could never get the key back — `handle_edit_key`
            // would have swallowed it.
            KeyCode::Char(c) if !ctrl && !alt => self.insert_char(c),
            _ => return false,
        }
        true
    }
}

impl From<String> for TextInput {
    fn from(text: String) -> Self {
        let cursor = text.len();
        Self {
            buffer: text,
            cursor,
        }
    }
}

/// Snap `pos` back to the nearest UTF-8 char boundary at or before it.
/// Cursor arithmetic tracks bytes, but String indexing panics mid-codepoint —
/// callers use this to stay valid after moving across multi-byte chars.
pub fn prev_char_boundary(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut new_pos = pos - 1;
    while new_pos > 0 && !s.is_char_boundary(new_pos) {
        new_pos -= 1;
    }
    new_pos
}

/// Snap `pos` forward to the next UTF-8 char boundary (or `s.len()` if none).
/// See `prev_char_boundary` for why byte-indexed cursors need this.
pub fn next_char_boundary(s: &str, pos: usize) -> usize {
    let len = s.len();
    if pos >= len {
        return len;
    }
    let mut new_pos = pos + 1;
    while new_pos < len && !s.is_char_boundary(new_pos) {
        new_pos += 1;
    }
    new_pos
}

/// Find the previous word boundary (for Option+Left)
pub fn word_boundary_left(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let bytes = s.as_bytes();
    let mut i = pos - 1;
    // Skip whitespace/punctuation
    while i > 0 && !bytes[i].is_ascii_alphanumeric() {
        i -= 1;
    }
    // Skip word characters
    while i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
        i -= 1;
    }
    i
}

/// Find the next word boundary (for Option+Right)
pub fn word_boundary_right(s: &str, pos: usize) -> usize {
    let len = s.len();
    if pos >= len {
        return len;
    }
    let bytes = s.as_bytes();
    let mut i = pos;
    // Skip current word characters
    while i < len && bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    // Skip whitespace/punctuation
    while i < len && !bytes[i].is_ascii_alphanumeric() {
        i += 1;
    }
    i
}

#[cfg(test)]
#[path = "text_input_tests.rs"]
mod tests;
