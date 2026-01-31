#[derive(Debug, Clone, Default)]
pub struct State {
    pub input: String,
    pub cursor: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    Submit,
    InsertChar(char),
    Backspace,
    Delete,
    MoveCursorLeft,
    MoveCursorRight,
    MoveCursorHome,
    MoveCursorEnd,
}

impl State {
    pub fn insert_char(&mut self, ch: char) {
        let idx = self.byte_index(self.cursor);
        self.input.insert(idx, ch);
        self.cursor = self.cursor.saturating_add(1);
    }

    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let start = self.byte_index(self.cursor - 1);
        let end = self.byte_index(self.cursor);
        if start < end {
            self.input.replace_range(start..end, "");
            self.cursor = self.cursor.saturating_sub(1);
        }
    }

    pub fn delete(&mut self) {
        let len = self.len_chars();
        if self.cursor >= len {
            return;
        }
        let start = self.byte_index(self.cursor);
        let end = self.byte_index(self.cursor + 1);
        if start < end {
            self.input.replace_range(start..end, "");
        }
    }

    pub fn move_left(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    pub fn move_right(&mut self) {
        let len = self.len_chars();
        if self.cursor < len {
            self.cursor += 1;
        }
    }

    pub fn move_home(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.len_chars();
    }

    pub fn clear(&mut self) {
        self.input.clear();
        self.cursor = 0;
    }

    pub fn clamp_cursor(&mut self) {
        let len = self.len_chars();
        if self.cursor > len {
            self.cursor = len;
        }
    }

    fn len_chars(&self) -> usize {
        self.input.chars().count()
    }

    fn byte_index(&self, char_index: usize) -> usize {
        if char_index == 0 {
            return 0;
        }
        self.input
            .char_indices()
            .nth(char_index)
            .map(|(idx, _)| idx)
            .unwrap_or_else(|| self.input.len())
    }
}
