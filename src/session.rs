use crate::{
    input::{self, Special},
    model::TerminalBuffer,
    settings::Profile,
};
use viewkit::platform::{Key, KeyModifiers};
pub(crate) struct TerminalSession {
    pub buffer: TerminalBuffer,
    dimensions: (usize, usize),
    dirty: bool,
    transport: Option<crate::transport::Transport>,
    attempted: bool,
    pub profile: Profile,
    pub failed: bool,
    protected_input: bool,
}
impl TerminalSession {
    #[cfg(mochios_pty_launch_probe_310)]
    pub fn start() -> Self {
        Self::with_profile(Profile::default(), 2000)
    }
    pub fn with_profile(profile: Profile, scrollback: usize) -> Self {
        Self {
            buffer: TerminalBuffer::new(scrollback),
            dimensions: (80, 24),
            dirty: true,
            transport: None,
            attempted: false,
            profile,
            failed: false,
            protected_input: false,
        }
    }
    pub fn running(&self) -> bool {
        !self.failed
            && self
                .transport
                .as_ref()
                .is_none_or(|transport| !transport.exited)
    }
    pub fn needs_poll(&self) -> bool {
        !self.failed
            && self
                .transport
                .as_ref()
                .is_none_or(|t| !t.exited || !t.output_closed)
    }
    #[cfg(target_os = "mochios")]
    pub fn consent_master(&self) -> std::io::Result<Option<u32>> {
        self.transport
            .as_ref()
            .map_or(Ok(None), |transport| transport.consent_master())
    }
    #[cfg(target_os = "mochios")]
    pub fn protect_input(&mut self, active: bool) {
        self.protected_input = active;
        if let Some(transport) = &mut self.transport {
            transport.protect_input(active);
        }
    }
    #[cfg(test)]
    pub fn suppress_launch(&mut self) {
        self.attempted = true;
    }
    pub fn title(&self) -> String {
        if self.buffer.title().is_empty() {
            self.profile.name.clone()
        } else {
            self.buffer.title().to_owned()
        }
    }
    pub fn resize(&mut self, columns: usize, rows: usize) {
        let size = (columns.clamp(1, 512), rows.clamp(1, 256));
        if size == self.dimensions {
            return;
        }
        self.dimensions = size;
        self.buffer.resize(size.0, size.1);
        self.dirty = true;
        if let Some(transport) = &self.transport {
            if let Err(error) = transport.resize(size.0, size.1) {
                self.buffer
                    .push_message(&format!("terminal: resize failed: {error}"));
            }
        }
    }
    pub fn poll(&mut self) -> bool {
        let mut changed = std::mem::take(&mut self.dirty);
        if !self.attempted {
            self.attempted = true;
            match crate::transport::Transport::start(
                self.dimensions.0,
                self.dimensions.1,
                &self.profile,
            ) {
                Ok(transport) => self.transport = Some(transport),
                Err(error) => {
                    self.failed = true;
                    self.buffer
                        .push_message(&format!("Unable to start {}: {error}", self.profile.name));
                    return true;
                }
            }
        }
        if let Some(transport) = self.transport.as_mut() {
            changed |= transport.poll(&mut self.buffer);
            let replies = self.buffer.replies();
            if !replies.is_empty() {
                let _ = transport.send(&replies);
            }
            if transport.exited && transport.output_closed && !transport.exit_reported {
                let status = transport
                    .exit_status
                    .map(|s| {
                        if s & 0x7f == 0 {
                            format!("exit code {}", (s >> 8) & 255)
                        } else {
                            format!("signal {}", s & 0x7f)
                        }
                    })
                    .unwrap_or_else(|| "status unavailable".into());
                self.buffer.push_message(&format!(
                    "\r\n[Process completed: {status} — close this pane or start a new session]"
                ));
                transport.exit_reported = true;
                changed = true;
            }
        }
        changed
    }
    #[cfg(mochios_pty_launch_probe_310)]
    pub fn visible_text(&self, columns: usize, rows: usize) -> String {
        self.buffer.visible_text(columns, rows)
    }
    pub fn scroll(&mut self, rows: i32) -> bool {
        self.buffer.scroll(rows)
    }
    pub fn send_text(&mut self, text: &str) -> bool {
        self.send(&input::text(text))
    }
    pub fn paste(&mut self, text: &str) -> bool {
        if text.len() > 60_000 {
            return false;
        }
        let Some(bytes) = paste_bytes(text, self.buffer.screen().bracketed_paste()) else {
            return false;
        };
        self.send(&bytes)
    }
    pub fn send_backspace(&mut self) -> bool {
        self.send(&[127])
    }
    pub fn send_key(&mut self, key: Key, modifiers: KeyModifiers) -> bool {
        if let Key::Character(c) = key {
            if modifiers.control() || modifiers.alt() {
                return input::character(c, modifiers.control(), modifiers.alt())
                    .is_some_and(|bytes| self.send(&bytes));
            }
        }
        if let Key::Function(n) = key {
            return input::function(n, modifiers.shift(), modifiers.alt(), modifiers.control())
                .is_some_and(|b| self.send(&b));
        }
        if key == Key::Insert {
            return self.send(&input::insert(
                modifiers.shift(),
                modifiers.alt(),
                modifiers.control(),
            ));
        }
        if key == Key::Enter {
            return self.send(if modifiers.alt() { b"\x1b\r" } else { b"\r" });
        }
        if key == Key::Tab && !modifiers.shift() {
            return self.send(b"\t");
        }
        if key == Key::Space && modifiers.control() {
            return self.send(&[0]);
        }
        let special = match key {
            Key::Escape => Special::Escape,
            Key::Delete => Special::Delete,
            Key::Home => Special::Home,
            Key::End => Special::End,
            Key::ArrowLeft => Special::Left,
            Key::ArrowRight => Special::Right,
            Key::ArrowUp => Special::Up,
            Key::ArrowDown => Special::Down,
            Key::PageUp => Special::PageUp,
            Key::PageDown => Special::PageDown,
            Key::Tab if modifiers.shift() => Special::BackTab,
            _ => return false,
        };
        if self.buffer.screen().application_cursor()
            && !modifiers.shift()
            && !modifiers.alt()
            && !modifiers.control()
        {
            let suffix = match key {
                Key::ArrowUp => Some(b'A'),
                Key::ArrowDown => Some(b'B'),
                Key::ArrowRight => Some(b'C'),
                Key::ArrowLeft => Some(b'D'),
                Key::Home => Some(b'H'),
                Key::End => Some(b'F'),
                _ => None,
            };
            if let Some(suffix) = suffix {
                return self.send(&[27, b'O', suffix]);
            }
        }
        self.send(&input::special(
            special,
            modifiers.shift(),
            modifiers.alt(),
            modifiers.control(),
        ))
    }
    pub fn send(&mut self, bytes: &[u8]) -> bool {
        if self.protected_input {
            return false;
        }
        self.buffer.live();
        self.dirty = true;
        if let Some(transport) = self.transport.as_mut() {
            match transport.send(bytes) {
                Ok(()) => return true,
                Err(error) => self
                    .buffer
                    .push_message(&format!("Input was not accepted: {error}")),
            }
        }
        false
    }
}

fn paste_bytes(text: &str, bracketed: bool) -> Option<Vec<u8>> {
    if text.len() > 60_000 {
        return None;
    }
    // Do not allow a paste to inject a bracketed-paste terminator or arbitrary ESC controls.
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let safe: String = normalized
        .chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect();
    let bytes = if bracketed {
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        safe.replace('\n', "\r").into_bytes()
    };

    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paste_normalizes_lines_strips_escape_injection_and_never_truncates() {
        assert_eq!(paste_bytes("a\r\nb\rc", false).unwrap(), b"a\rb\rc");
        assert_eq!(
            paste_bytes("x\x1b[201~y", true).unwrap(),
            b"\x1b[200~x[201~y\x1b[201~"
        );
        assert!(paste_bytes(&"界".repeat(20001), true).is_none());
    }
}
