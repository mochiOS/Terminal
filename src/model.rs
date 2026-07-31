use std::collections::VecDeque;

const MAX_SCROLLBACK_LINES: usize = 2_000;
const MAX_LINE_CHARACTERS: usize = 4_096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum EscapeState {
    #[default]
    Ground,
    Escape,
    Csi,
    Osc,
    OscEscape,
}

pub(crate) struct TerminalBuffer {
    lines: VecDeque<String>,
    cursor: usize,
    utf8_pending: Vec<u8>,
    escape_state: EscapeState,
    scroll_rows: usize,
}

impl Default for TerminalBuffer {
    fn default() -> Self {
        let mut lines = VecDeque::new();
        lines.push_back(String::new());
        Self {
            lines,
            cursor: 0,
            utf8_pending: Vec::new(),
            escape_state: EscapeState::Ground,
            scroll_rows: 0,
        }
    }
}

impl TerminalBuffer {
    pub(crate) fn push_bytes(&mut self, bytes: &[u8]) {
        self.utf8_pending.extend_from_slice(bytes);
        self.decode_pending_utf8();
    }

    pub(crate) fn push_message(&mut self, message: &str) {
        self.push_bytes(message.as_bytes());
        self.push_bytes(b"\n");
    }

    pub(crate) fn scroll(&mut self, rows: i32) -> bool {
        let previous = self.scroll_rows;
        if rows > 0 {
            self.scroll_rows = self.scroll_rows.saturating_add(rows as usize);
        } else {
            self.scroll_rows = self
                .scroll_rows
                .saturating_sub(rows.unsigned_abs() as usize);
        }
        self.scroll_rows = self.scroll_rows.min(self.lines.len().saturating_sub(1));
        self.scroll_rows != previous
    }

    pub(crate) fn visible_text(&self, columns: usize, rows: usize) -> String {
        let columns = columns.max(1);
        let rows = rows.max(1);
        let mut wrapped = Vec::new();
        let last_index = self.lines.len().saturating_sub(1);

        for (index, line) in self.lines.iter().enumerate() {
            let mut characters: Vec<char> = line.chars().collect();
            if index == last_index && self.scroll_rows == 0 {
                let cursor = self.cursor.min(characters.len());
                characters.insert(cursor, '\u{258c}');
            }
            if characters.is_empty() {
                wrapped.push(String::new());
                continue;
            }
            for chunk in characters.chunks(columns) {
                wrapped.push(chunk.iter().collect::<String>());
            }
        }

        let end = wrapped.len().saturating_sub(self.scroll_rows).max(1);
        let start = end.saturating_sub(rows);
        wrapped[start..end].join("\n")
    }

    fn decode_pending_utf8(&mut self) {
        loop {
            match std::str::from_utf8(&self.utf8_pending) {
                Ok(text) => {
                    let owned = text.to_owned();
                    self.utf8_pending.clear();
                    self.push_text(&owned);
                    break;
                }
                Err(error) if error.valid_up_to() > 0 => {
                    let valid = self.utf8_pending[..error.valid_up_to()].to_vec();
                    self.utf8_pending.drain(..error.valid_up_to());
                    if let Ok(text) = std::str::from_utf8(&valid) {
                        self.push_text(text);
                    }
                }
                Err(error) => {
                    let Some(length) = error.error_len() else {
                        break;
                    };
                    self.utf8_pending
                        .drain(..length.min(self.utf8_pending.len()));
                    self.push_character('\u{fffd}');
                }
            }
        }
    }

    fn push_text(&mut self, text: &str) {
        for character in text.chars() {
            self.push_character(character);
        }
    }

    fn push_character(&mut self, character: char) {
        match self.escape_state {
            EscapeState::Ground => match character {
                '\u{1b}' => self.escape_state = EscapeState::Escape,
                '\n' => self.new_line(),
                '\r' => self.cursor = 0,
                '\u{8}' => self.cursor = self.cursor.saturating_sub(1),
                '\t' => {
                    let spaces = 4 - self.cursor % 4;
                    for _ in 0..spaces {
                        self.write_printable(' ');
                    }
                }
                character if !character.is_control() => self.write_printable(character),
                _ => {}
            },
            EscapeState::Escape => {
                self.escape_state = match character {
                    '[' => EscapeState::Csi,
                    ']' => EscapeState::Osc,
                    _ => EscapeState::Ground,
                };
            }
            EscapeState::Csi => {
                if ('@'..='~').contains(&character) {
                    self.escape_state = EscapeState::Ground;
                }
            }
            EscapeState::Osc => {
                if character == '\u{7}' {
                    self.escape_state = EscapeState::Ground;
                } else if character == '\u{1b}' {
                    self.escape_state = EscapeState::OscEscape;
                }
            }
            EscapeState::OscEscape => {
                self.escape_state = if character == '\\' {
                    EscapeState::Ground
                } else {
                    EscapeState::Osc
                };
            }
        }
    }

    fn write_printable(&mut self, character: char) {
        let Some(line) = self.lines.back_mut() else {
            return;
        };
        let mut characters: Vec<char> = line.chars().collect();
        let cursor = self.cursor.min(characters.len());
        if cursor < characters.len() {
            characters[cursor] = character;
        } else if characters.len() < MAX_LINE_CHARACTERS {
            characters.push(character);
        }
        *line = characters.into_iter().collect();
        self.cursor = (cursor + 1).min(MAX_LINE_CHARACTERS);
        self.scroll_rows = 0;
    }

    fn new_line(&mut self) {
        self.lines.push_back(String::new());
        if self.lines.len() > MAX_SCROLLBACK_LINES {
            self.lines.pop_front();
        }
        self.cursor = 0;
        self.scroll_rows = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::TerminalBuffer;

    #[test]
    fn strips_ansi_sequences_and_preserves_utf8_across_chunks() {
        let mut buffer = TerminalBuffer::default();
        let text = "ready 日本語".as_bytes();
        let split = text.len() - 2;
        buffer.push_bytes(b"\x1b[32m");
        buffer.push_bytes(&text[..split]);
        buffer.push_bytes(&text[split..]);
        buffer.push_bytes(b"\x1b[0m");

        assert_eq!(buffer.visible_text(80, 5), "ready 日本語\u{258c}");
    }

    #[test]
    fn carriage_return_overwrites_the_current_line() {
        let mut buffer = TerminalBuffer::default();
        buffer.push_bytes(b"hello\rOK");

        assert_eq!(buffer.visible_text(80, 5), "OK\u{258c}llo");
    }

    #[test]
    fn wraps_and_scrolls_visible_rows() {
        let mut buffer = TerminalBuffer::default();
        buffer.push_bytes(b"abcdefgh\nsecond\nthird");

        assert_eq!(buffer.visible_text(4, 3), "nd\nthir\nd\u{258c}");
        assert!(buffer.scroll(2));
        assert_eq!(buffer.visible_text(4, 3), "efgh\nseco\nnd");
    }
}
