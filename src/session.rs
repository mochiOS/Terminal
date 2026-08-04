use crate::model::TerminalBuffer;
use viewkit::platform::{Key, KeyModifiers};

const KEY_ESCAPE: u16 = 1;
const KEY_BACKSPACE: u16 = 2;
const KEY_TAB: u16 = 3;
const KEY_ENTER: u16 = 4;
const KEY_SPACE: u16 = 5;
const KEY_DELETE: u16 = 79;
const KEY_HOME: u16 = 80;
const KEY_END: u16 = 81;
const KEY_LEFT: u16 = 82;
const KEY_RIGHT: u16 = 83;
const KEY_UP: u16 = 84;
const KEY_DOWN: u16 = 85;
const KEY_PAGE_UP: u16 = 86;
const KEY_PAGE_DOWN: u16 = 87;
const INPUT_KIND_KEY: u16 = 1;
const INPUT_FLAG_PRESS: u16 = 1;
const INPUT_MOD_SHIFT: u32 = 1 << 0;
const INPUT_MOD_CONTROL: u32 = 1 << 1;
const INPUT_MOD_ALT: u32 = 1 << 2;

#[derive(Clone, Copy)]
struct KeyPacket {
    keycode: u16,
    codepoint: u32,
    modifiers: u32,
}

pub(crate) struct TerminalSession {
    buffer: TerminalBuffer,
    #[cfg(target_os = "mochios")]
    transport: Option<MochiOsTransport>,
    #[cfg(target_os = "mochios")]
    launch_attempted: bool,
}

impl TerminalSession {
    pub(crate) fn start() -> Self {
        let buffer = TerminalBuffer::default();
        #[cfg(target_os = "mochios")]
        {
            Self {
                buffer,
                transport: None,
                launch_attempted: false,
            }
        }
        #[cfg(not(target_os = "mochios"))]
        {
            let mut session = Self { buffer };
            session
                .buffer
                .push_message("Terminal session is available on mochiOS.");
            session
        }
    }

    pub(crate) fn poll(&mut self) -> bool {
        #[cfg(target_os = "mochios")]
        {
            if self.transport.is_none() && !self.launch_attempted {
                self.launch_attempted = true;
                match MochiOsTransport::start() {
                    Ok(transport) => self.transport = Some(transport),
                    Err(error) => {
                        self.buffer
                            .push_message(&format!("terminal: failed to start msh: {error}"));
                        return true;
                    }
                }
            }
            let Some(transport) = self.transport.as_mut() else {
                return false;
            };
            let mut changed = transport.poll(&mut self.buffer);
            if transport.exited && !transport.exit_reported {
                self.buffer.push_message("[Process completed]");
                transport.exit_reported = true;
                changed = true;
            }
            changed
        }
        #[cfg(not(target_os = "mochios"))]
        {
            false
        }
    }

    pub(crate) fn visible_text(&self, columns: usize, rows: usize) -> String {
        self.buffer.visible_text(columns, rows)
    }

    pub(crate) fn scroll(&mut self, rows: i32) -> bool {
        self.buffer.scroll(rows)
    }

    pub(crate) fn send_text(&mut self, text: &str) -> bool {
        let mut sent = false;
        for character in text.chars() {
            let packet = match character {
                '\n' | '\r' => KeyPacket::new(KEY_ENTER, 0, KeyModifiers::default()),
                '\t' => KeyPacket::new(KEY_TAB, 0, KeyModifiers::default()),
                ' ' => KeyPacket::new(KEY_SPACE, ' ' as u32, KeyModifiers::default()),
                character if !character.is_control() => {
                    KeyPacket::new(0, character as u32, KeyModifiers::default())
                }
                _ => continue,
            };
            sent |= self.send_packet(packet);
        }
        sent
    }

    pub(crate) fn send_key(&mut self, key: Key, modifiers: KeyModifiers) -> bool {
        let keycode = match key {
            Key::Escape => KEY_ESCAPE,
            Key::Delete => KEY_DELETE,
            Key::ArrowLeft => KEY_LEFT,
            Key::ArrowRight => KEY_RIGHT,
            Key::ArrowUp => KEY_UP,
            Key::ArrowDown => KEY_DOWN,
            Key::Home => KEY_HOME,
            Key::End => KEY_END,
            Key::PageUp => KEY_PAGE_UP,
            Key::PageDown => KEY_PAGE_DOWN,
            Key::Backspace | Key::Tab | Key::Enter | Key::Space | Key::Character(_) => {
                return false;
            }
        };
        self.send_packet(KeyPacket::new(keycode, 0, modifiers))
    }

    pub(crate) fn send_backspace(&mut self) -> bool {
        self.send_packet(KeyPacket::new(KEY_BACKSPACE, 0, KeyModifiers::default()))
    }

    fn send_packet(&mut self, packet: KeyPacket) -> bool {
        #[cfg(target_os = "mochios")]
        {
            let Some(transport) = self.transport.as_mut() else {
                return false;
            };
            transport.send(packet)
        }
        #[cfg(not(target_os = "mochios"))]
        {
            let _ = packet;
            false
        }
    }
}

impl KeyPacket {
    fn new(keycode: u16, codepoint: u32, modifiers: KeyModifiers) -> Self {
        let mut wire_modifiers = 0;
        if modifiers.shift() {
            wire_modifiers |= INPUT_MOD_SHIFT;
        }
        if modifiers.control() {
            wire_modifiers |= INPUT_MOD_CONTROL;
        }
        if modifiers.alt() {
            wire_modifiers |= INPUT_MOD_ALT;
        }
        Self {
            keycode,
            codepoint,
            modifiers: wire_modifiers,
        }
    }

    fn encode(self) -> [u8; 32] {
        let mut event = [0u8; 32];
        event[0..2].copy_from_slice(&INPUT_KIND_KEY.to_le_bytes());
        event[2..4].copy_from_slice(&INPUT_FLAG_PRESS.to_le_bytes());
        event[4..6].copy_from_slice(&self.keycode.to_le_bytes());
        event[8..12].copy_from_slice(&self.codepoint.to_le_bytes());
        event[24..28].copy_from_slice(&self.modifiers.to_le_bytes());
        event
    }
}

#[cfg(target_os = "mochios")]
struct MochiOsTransport {
    pid: libc::pid_t,
    stdin: std::fs::File,
    stdout: std::fs::File,
    stderr: std::fs::File,
    exited: bool,
    exit_reported: bool,
}

#[cfg(target_os = "mochios")]
impl MochiOsTransport {
    fn start() -> std::io::Result<Self> {
        let (pid, stdin, stdout, stderr) = spawn_msh()?;
        set_nonblocking(&stdout)?;
        set_nonblocking(&stderr)?;

        Ok(Self {
            pid,
            stdin,
            stdout,
            stderr,
            exited: false,
            exit_reported: false,
        })
    }

    fn poll(&mut self, buffer: &mut TerminalBuffer) -> bool {
        let mut changed = false;
        changed |= drain_reader(&mut self.stdout, buffer);
        changed |= drain_reader(&mut self.stderr, buffer);
        if !self.exited {
            let mut status = 0;
            let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if result == self.pid {
                self.exited = true;
            } else if result < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(10) {
                    buffer.push_message(&format!("terminal: failed to inspect msh: {error}"));
                    self.exited = true;
                    changed = true;
                }
            }
        }
        changed
    }

    fn send(&mut self, packet: KeyPacket) -> bool {
        use std::io::Write;

        self.stdin.write_all(&packet.encode()).is_ok()
    }
}

#[cfg(target_os = "mochios")]
impl Drop for MochiOsTransport {
    fn drop(&mut self) {
        if !self.exited {
            unsafe {
                libc::kill(self.pid, libc::SIGKILL);
                let mut status = 0;
                libc::waitpid(self.pid, &mut status, 0);
            }
        }
    }
}

#[cfg(target_os = "mochios")]
fn spawn_msh() -> std::io::Result<(libc::pid_t, std::fs::File, std::fs::File, std::fs::File)> {
    use std::ffi::CString;
    use std::os::fd::FromRawFd;

    let stdin_pipe = create_pipe()?;
    let stdout_pipe = match create_pipe() {
        Ok(pipe) => pipe,
        Err(error) => {
            close_pipe(stdin_pipe);
            return Err(error);
        }
    };
    let stderr_pipe = match create_pipe() {
        Ok(pipe) => pipe,
        Err(error) => {
            close_pipe(stdin_pipe);
            close_pipe(stdout_pipe);
            return Err(error);
        }
    };
    let path = CString::new("/bin/msh")
        .map_err(|_| std::io::Error::other("invalid msh executable path"))?;
    let mode = CString::new("--stdio-input")
        .map_err(|_| std::io::Error::other("invalid msh input mode"))?;

    let mut actions: libc::posix_spawn_file_actions_t = core::ptr::null_mut();
    let init_status = unsafe { libc::posix_spawn_file_actions_init(&mut actions) };
    if init_status != 0 {
        close_pipe(stdin_pipe);
        close_pipe(stdout_pipe);
        close_pipe(stderr_pipe);
        return Err(std::io::Error::from_raw_os_error(init_status));
    }
    let actions_status =
        configure_spawn_file_actions(&mut actions, stdin_pipe, stdout_pipe, stderr_pipe);
    if let Err(error) = actions_status {
        unsafe {
            libc::posix_spawn_file_actions_destroy(&mut actions);
        }
        close_pipe(stdin_pipe);
        close_pipe(stdout_pipe);
        close_pipe(stderr_pipe);
        return Err(error);
    }

    let argv = [
        path.as_ptr().cast_mut(),
        mode.as_ptr().cast_mut(),
        core::ptr::null_mut(),
    ];
    let mut pid = 0;
    let spawn_status = unsafe {
        libc::posix_spawn(
            &mut pid,
            path.as_ptr(),
            &actions,
            core::ptr::null(),
            argv.as_ptr(),
            core::ptr::null(),
        )
    };
    unsafe {
        libc::posix_spawn_file_actions_destroy(&mut actions);
    }
    if spawn_status != 0 {
        close_pipe(stdin_pipe);
        close_pipe(stdout_pipe);
        close_pipe(stderr_pipe);
        return Err(std::io::Error::from_raw_os_error(spawn_status));
    }

    unsafe {
        libc::close(stdin_pipe.0);
        libc::close(stdout_pipe.1);
        libc::close(stderr_pipe.1);
    }
    let stdin = unsafe { std::fs::File::from_raw_fd(stdin_pipe.1) };
    let stdout = unsafe { std::fs::File::from_raw_fd(stdout_pipe.0) };
    let stderr = unsafe { std::fs::File::from_raw_fd(stderr_pipe.0) };
    Ok((pid, stdin, stdout, stderr))
}

#[cfg(target_os = "mochios")]
fn configure_spawn_file_actions(
    actions: &mut libc::posix_spawn_file_actions_t,
    stdin_pipe: (libc::c_int, libc::c_int),
    stdout_pipe: (libc::c_int, libc::c_int),
    stderr_pipe: (libc::c_int, libc::c_int),
) -> std::io::Result<()> {
    let operations = [
        unsafe { libc::posix_spawn_file_actions_adddup2(actions, stdin_pipe.0, 0) },
        unsafe { libc::posix_spawn_file_actions_adddup2(actions, stdout_pipe.1, 1) },
        unsafe { libc::posix_spawn_file_actions_adddup2(actions, stderr_pipe.1, 2) },
        unsafe { libc::posix_spawn_file_actions_addclose(actions, stdin_pipe.0) },
        unsafe { libc::posix_spawn_file_actions_addclose(actions, stdin_pipe.1) },
        unsafe { libc::posix_spawn_file_actions_addclose(actions, stdout_pipe.0) },
        unsafe { libc::posix_spawn_file_actions_addclose(actions, stdout_pipe.1) },
        unsafe { libc::posix_spawn_file_actions_addclose(actions, stderr_pipe.0) },
        unsafe { libc::posix_spawn_file_actions_addclose(actions, stderr_pipe.1) },
    ];
    for status in operations {
        if status != 0 {
            return Err(std::io::Error::from_raw_os_error(status));
        }
    }
    Ok(())
}

#[cfg(target_os = "mochios")]
fn create_pipe() -> std::io::Result<(libc::c_int, libc::c_int)> {
    let mut fds = [0; 2];
    if unsafe { libc::pipe(fds.as_mut_ptr()) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok((fds[0], fds[1]))
}

#[cfg(target_os = "mochios")]
fn close_pipe(pipe: (libc::c_int, libc::c_int)) {
    unsafe {
        libc::close(pipe.0);
        libc::close(pipe.1);
    }
}

#[cfg(target_os = "mochios")]
fn set_nonblocking(stream: &impl std::os::fd::AsRawFd) -> std::io::Result<()> {
    let fd = stream.as_raw_fd();
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(target_os = "mochios")]
fn drain_reader(reader: &mut impl std::io::Read, buffer: &mut TerminalBuffer) -> bool {
    let mut changed = false;
    let mut bytes = [0u8; 4096];
    loop {
        match reader.read(&mut bytes) {
            Ok(0) => break,
            Ok(length) => {
                buffer.push_bytes(&bytes[..length]);
                changed = true;
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::WouldBlock
                    || error.raw_os_error() == Some(11) =>
            {
                break;
            }
            Err(error) => {
                buffer.push_message(&format!("terminal: output read failed: {error}"));
                changed = true;
                break;
            }
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::{INPUT_MOD_ALT, INPUT_MOD_CONTROL, INPUT_MOD_SHIFT, KeyPacket};
    use viewkit::platform::KeyModifiers;

    #[test]
    fn key_packet_encoding_matches_msh_input_event_layout() {
        let modifiers = KeyModifiers::from_bits(
            KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SUPER,
        );
        let encoded = KeyPacket::new(4, 'x' as u32, modifiers).encode();

        assert_eq!(&encoded[0..2], &1u16.to_le_bytes());
        assert_eq!(&encoded[2..4], &1u16.to_le_bytes());
        assert_eq!(&encoded[4..6], &4u16.to_le_bytes());
        assert_eq!(&encoded[8..12], &('x' as u32).to_le_bytes());
        assert_eq!(
            &encoded[24..28],
            &(INPUT_MOD_SHIFT | INPUT_MOD_CONTROL | INPUT_MOD_ALT).to_le_bytes()
        );
        assert_eq!(&encoded[28..32], &[0; 4]);
    }
}
