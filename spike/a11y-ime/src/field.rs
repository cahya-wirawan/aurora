//! A text field, built from nothing.
//!
//! This is the point of the spike: with a platform toolkit you inherit all of
//! this. Rendering our own UI means cursor movement, editing, and IME
//! composition state are ours, and every one of them has to be reflected to the
//! accessibility API separately.

use accesskit::ActionRequest;

#[derive(Default)]
pub struct TextField {
    pub text: String,
    /// Byte offset. Kept on a char boundary by every mutation below.
    pub cursor: usize,
    /// Uncommitted IME composition. Not part of `text` until Commit arrives.
    pub preedit: String,
}

impl TextField {
    pub fn insert(&mut self, s: &str) {
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        // Char-wise, not byte-wise: deleting a byte from a multi-byte character
        // corrupts the string. The kind of bug a toolkit would have handled.
        let prev = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
        self.text.replace_range(prev..self.cursor, "");
        self.cursor = prev;
    }

    pub fn move_left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .char_indices()
            .next_back()
            .map_or(0, |(i, _)| i);
    }

    pub fn move_right(&mut self) {
        if let Some(c) = self.text[self.cursor..].chars().next() {
            self.cursor += c.len_utf8();
        }
    }

    pub fn set_preedit(&mut self, s: String) {
        self.preedit = s;
    }

    pub fn commit(&mut self, s: &str) {
        self.preedit.clear();
        self.insert(s);
    }

    /// Field contents with the composition shown in place.
    pub fn display_text(&self) -> String {
        let mut s = self.text.clone();
        if self.preedit.is_empty() {
            s.insert(self.cursor, '|');
        } else {
            s.insert_str(self.cursor, &format!("[{}]", self.preedit));
        }
        format!("> {s}")
    }

    /// A screen reader can act on the field, not just read it. Handling this is
    /// what separates "announced" from "usable".
    pub fn handle_action(&mut self, req: &ActionRequest) {
        use accesskit::{Action, ActionData};
        match req.action {
            Action::SetValue => {
                if let Some(ActionData::Value(v)) = &req.data {
                    self.text = v.to_string();
                    self.cursor = self.text.len();
                }
            }
            Action::Focus => {}
            _ => {}
        }
    }
}
