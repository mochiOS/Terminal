use crate::{
    input::{self, Special},
    model::TerminalBuffer,
};
use viewkit::platform::{Key, KeyModifiers};

pub(crate) struct TerminalSession {
    buffer: TerminalBuffer,
    dimensions: (usize, usize),
    dirty: bool,
    #[cfg(target_os = "mochios")]
    transport: Option<crate::transport::Transport>,
    #[cfg(target_os = "mochios")]
    launch_attempted: bool,
}
impl TerminalSession {
    pub(crate) fn start() -> Self {
        let mut session = Self {
            buffer: TerminalBuffer::default(),
            dimensions: (80, 24),
            dirty: false,
            #[cfg(target_os = "mochios")]
            transport: None,
            #[cfg(target_os = "mochios")]
            launch_attempted: false,
        };
        #[cfg(not(target_os = "mochios"))]
        session
            .buffer
            .push_message("Terminal session is available on mochiOS.");
        session
    }
    pub(crate) fn resize(&mut self, columns: usize, rows: usize) {
        let size = (columns.max(1), rows.max(1));
        if self.dimensions == size {
            return;
        }
        self.dimensions = size;
        #[cfg(target_os = "mochios")]
        if let Some(transport) = self.transport.as_ref()
            && let Err(error) = transport.resize(size.0, size.1)
        {
            self.buffer
                .push_message(&format!("terminal: resize failed: {error}"));
            self.dirty = true;
        }
    }
    pub(crate) fn poll(&mut self) -> bool {
        let mut changed = std::mem::take(&mut self.dirty);
        #[cfg(target_os = "mochios")]
        {
            if self.transport.is_none() && !self.launch_attempted {
                self.launch_attempted = true;
                match crate::transport::Transport::start(self.dimensions.0, self.dimensions.1) {
                    Ok(transport) => self.transport = Some(transport),
                    Err(error) => {
                        self.buffer
                            .push_message(&format!("terminal: failed to start msh: {error}"));
                        return true;
                    }
                }
            }
            if let Some(transport) = self.transport.as_mut() {
                changed |= transport.poll(&mut self.buffer);
                if transport.exited && !transport.exit_reported {
                    self.buffer.push_message("[Process completed]");
                    transport.exit_reported = true;
                    changed = true;
                }
            }
        }
        changed
    }
    pub(crate) fn visible_text(&self, columns: usize, rows: usize) -> String {
        self.buffer.visible_text(columns, rows)
    }
    pub(crate) fn scroll(&mut self, rows: i32) -> bool {
        self.buffer.scroll(rows)
    }
    pub(crate) fn send_text(&mut self, text: &str) -> bool {
        self.send(&input::text(text))
    }
    pub(crate) fn send_backspace(&mut self) -> bool {
        self.send(&[127])
    }
    pub(crate) fn send_key(&mut self, key: Key, modifiers: KeyModifiers) -> bool {
        if let Key::Character(value) = key
            && (modifiers.control() || modifiers.alt())
        {
            return input::character(value, modifiers.control(), modifiers.alt())
                .is_some_and(|bytes| self.send(&bytes));
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
        self.send(&input::special(
            special,
            modifiers.shift(),
            modifiers.alt(),
            modifiers.control(),
        ))
    }
    fn send(&mut self, bytes: &[u8]) -> bool {
        #[cfg(target_os = "mochios")]
        if let Some(transport) = self.transport.as_mut() {
            match transport.send(bytes) {
                Ok(()) => return true,
                Err(error) => {
                    self.buffer.push_message(&format!("terminal: {error}"));
                    self.dirty = true;
                }
            }
        }
        #[cfg(not(target_os = "mochios"))]
        let _ = bytes;
        false
    }
}
