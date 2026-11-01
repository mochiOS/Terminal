//! Bounded VT screen state. Escape sequences cannot execute host commands or access the clipboard.
use std::num::NonZeroU16;
#[derive(Default)]
pub(crate) struct Events {
    pub title: String,
    pub bell: bool,
    replies: Vec<u8>,
}
impl vt100::Callbacks for Events {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = String::from_utf8_lossy(title)
            .chars()
            .filter(|c| !c.is_control())
            .take(128)
            .collect();
    }
    fn audible_bell(&mut self, _: &mut vt100::Screen) {
        self.bell = true;
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        first: Option<u8>,
        _: Option<u8>,
        params: &[&[u16]],
        code: char,
    ) {
        let value = params.first().and_then(|p| p.first()).copied().unwrap_or(0);
        let reply = match (first, code, value) {
            (None, 'n', 5) => "\x1b[0n".to_owned(),
            (None, 'n', 6) => {
                let (row, col) = screen.cursor_position();
                format!("\x1b[{};{}R", row + 1, col + 1)
            }
            (None, 'c', 0) => "\x1b[?1;2c".to_owned(),
            (Some(b'>'), 'c', 0) => "\x1b[>0;1;0c".to_owned(),
            _ => return,
        };
        if self.replies.len() + reply.len() <= 4096 {
            self.replies.extend_from_slice(reply.as_bytes());
        }
    }
    // OSC 52 and application-requested resizing deliberately retain the no-op defaults.
}
pub(crate) struct TerminalBuffer {
    parser: vt100::Parser<Events>,
    last_find: Option<(String, (isize, u16))>,
}
pub(crate) fn dimension(value: usize, limit: usize) -> NonZeroU16 {
    NonZeroU16::new(value.clamp(1, limit) as u16).unwrap()
}
impl Default for TerminalBuffer {
    fn default() -> Self {
        Self::new(2000)
    }
}
impl TerminalBuffer {
    pub fn new(scrollback: usize) -> Self {
        Self {
            last_find: None,
            parser: vt100::Parser::new_with_callbacks(
                dimension(24, 256),
                dimension(80, 512),
                scrollback.min(10_000),
                Events::default(),
            ),
        }
    }
    pub fn push_bytes(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }
    pub fn push_message(&mut self, text: &str) {
        self.push_bytes(text.as_bytes());
        self.push_bytes(b"\r\n");
    }
    pub fn resize(&mut self, columns: usize, rows: usize) {
        self.parser
            .set_size(dimension(rows, 256), dimension(columns, 512));
    }
    pub fn screen(&self) -> &vt100::Screen {
        self.parser.screen()
    }
    pub fn scroll(&mut self, rows: i32) -> bool {
        let old = self.screen().scrollback();
        let new = if rows > 0 {
            old.saturating_add(rows as usize)
        } else {
            old.saturating_sub(rows.unsigned_abs() as usize)
        };
        self.parser.screen_mut().set_scrollback(new);
        old != self.screen().scrollback()
    }
    pub fn live(&mut self) {
        self.parser.screen_mut().set_scrollback(0);
    }
    pub fn title(&self) -> &str {
        &self.parser.callbacks().title
    }
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.parser.callbacks_mut().bell)
    }
    pub fn replies(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.parser.callbacks_mut().replies)
    }
    pub fn clear(&mut self) {
        self.push_bytes(b"\x1b[2J\x1b[3J\x1b[H");
    }
    #[cfg(mochios_pty_launch_probe_310)]
    pub fn visible_text(&self, _: usize, _: usize) -> String {
        self.screen().contents()
    }
    pub fn selected_text(&self, start: (u16, u16), end: (u16, u16)) -> String {
        let (start, end) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.screen()
            .contents_between(start.0, start.1, end.0, end.1.saturating_add(1))
    }
    pub fn find(&mut self, query: &str, previous: bool) -> Option<(u16, u16, u16)> {
        if query.is_empty() || query.len() > 256 {
            self.last_find = None;
            return None;
        }
        let (rows, cols) = self.screen().size();
        let current = self.screen().scrollback();
        let maximum = self.screen().scrollback_len();
        let last = self
            .last_find
            .as_ref()
            .filter(|(q, _)| q == query)
            .map(|(_, key)| *key);
        // Compare stable history coordinates, retaining only the nearest match and wrap target.
        type Hit = ((isize, u16), usize, u16, u16);
        let mut nearest: Option<Hit> = None;
        let mut wrap: Option<Hit> = None;
        let mut offsets: Vec<usize> = (0..=maximum).step_by(rows.get() as usize).collect();
        if offsets.last().copied() != Some(maximum) {
            offsets.push(maximum);
        }
        for offset in offsets {
            self.parser.screen_mut().set_scrollback(offset);
            for row in 0..rows.get() {
                let mut line = String::new();
                let mut columns = Vec::new();
                for col in 0..cols.get() {
                    let cell = self.screen().cell(row, col).unwrap();
                    if cell.is_wide_continuation() {
                        continue;
                    }
                    let text = if cell.contents().is_empty() {
                        " "
                    } else {
                        cell.contents()
                    };
                    columns.extend(std::iter::repeat_n(col, text.len()));
                    line.push_str(text);
                }
                for (index, _) in line.match_indices(query) {
                    let key = (row as isize - offset as isize, columns[index]);
                    let hit = (key, offset, row, columns[index + query.len() - 1]);
                    let better = |a: &Hit, b: &Hit| if previous { a.0 > b.0 } else { a.0 < b.0 };
                    if wrap.as_ref().is_none_or(|old| better(&hit, old)) {
                        wrap = Some(hit);
                    }
                    if last.is_none_or(|old| if previous { key < old } else { key > old })
                        && nearest.as_ref().is_none_or(|old| better(&hit, old))
                    {
                        nearest = Some(hit);
                    }
                }
            }
        }
        if let Some((key, offset, row, end)) = nearest.or(wrap) {
            self.parser.screen_mut().set_scrollback(offset);
            self.last_find = Some((query.to_owned(), key));
            Some((row, key.1, end))
        } else {
            self.parser.screen_mut().set_scrollback(current);
            None
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn cursor_erase_colors_and_split_utf8() {
        let mut b = TerminalBuffer::default();
        b.push_bytes(b"hello\rOK\x1b[K\r\n\x1b[31mred\x1b[0m");
        assert_eq!(b.screen().cell(0, 0).unwrap().contents(), "O");
        assert_eq!(b.screen().cell(0, 2).unwrap().contents(), "");
        assert_eq!(
            b.screen().cell(1, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        b.push_bytes(&"日本".as_bytes()[..2]);
        b.push_bytes(&"日本".as_bytes()[2..]);
        assert_eq!(b.screen().cell(1, 3).unwrap().contents(), "日");
        assert!(b.screen().cell(1, 4).unwrap().is_wide_continuation());
    }
    #[test]
    fn alternate_screen_restores_primary() {
        let mut b = TerminalBuffer::default();
        b.push_bytes(b"shell\x1b[?1049hTUI\x1b[?2004h");
        assert!(b.screen().alternate_screen());
        assert!(b.screen().bracketed_paste());
        b.push_bytes(b"\x1b[?1049l");
        assert!(b.screen().contents().contains("shell"));
    }
    #[test]
    fn replies_and_titles_do_not_enable_osc52() {
        let mut b = TerminalBuffer::default();
        b.push_bytes(b"\x1b[3;4H\x1b[6n\x1b]2;cargo test\x07\x1b]52;c;secret\x07");
        assert_eq!(b.replies(), b"\x1b[3;4R");
        assert_eq!(b.title(), "cargo test");
    }
    #[test]
    fn find_visits_each_match_forward_backward_and_wraps() {
        let mut b = TerminalBuffer::default();
        b.push_message("needle x needle");
        b.push_message("needle");
        assert_eq!(b.find("needle", false), Some((0, 0, 5)));
        assert_eq!(b.find("needle", false), Some((0, 9, 14)));
        assert_eq!(b.find("needle", false), Some((1, 0, 5)));
        assert_eq!(b.find("needle", false), Some((0, 0, 5)));
        assert_eq!(b.find("needle", true), Some((1, 0, 5)));
        assert_eq!(b.find("absent", false), None);
    }
    #[test]
    fn bounded_history_search() {
        let mut b = TerminalBuffer::new(100);
        b.resize(80, 10);
        for n in 0..300 {
            b.push_message(&format!("line {n}"));
        }
        assert!(b.screen().scrollback_len() <= 100);
        assert!(b.find("line 250", false).is_some());
        b.live();
        assert!(b.screen().contents().contains("line 299"));
    }
}
