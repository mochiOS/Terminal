//! A PTY master owned by this window; only its slave is delegated to msh.
use crate::{model::TerminalBuffer, pending_input::PendingInput};
use std::{
    fs::File,
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd},
    time::{Duration, Instant},
};
pub(crate) struct Transport {
    pid: libc::pid_t,
    master: Option<File>,
    pending: PendingInput,
    pub(crate) exited: bool,
    pub(crate) exit_reported: bool,
    input_failed: bool,
    output_closed: bool,
}
fn window(columns: usize, rows: usize) -> libc::winsize {
    libc::winsize {
        ws_col: columns.clamp(1, u16::MAX as usize) as u16,
        ws_row: rows.clamp(1, u16::MAX as usize) as u16,
        ws_xpixel: 0,
        ws_ypixel: 0,
    }
}
impl Transport {
    pub(crate) fn start(columns: usize, rows: usize) -> io::Result<Self> {
        let (mut master, mut slave) = (-1, -1);
        let size = window(columns, rows);
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                core::ptr::null_mut(),
                core::ptr::null(),
                &size,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        for fd in [master.as_raw_fd(), slave.as_raw_fd()] {
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } < 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let flags = unsafe { libc::fcntl(master.as_raw_fd(), libc::F_GETFL) };
        if flags < 0
            || unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) }
                < 0
        {
            return Err(io::Error::last_os_error());
        }
        let pid = crate::native_launch::spawn(
            "/system/bin/msh",
            &["/system/bin/msh", "--login-tty"],
            [slave.as_raw_fd(); 3],
        )?;
        Ok(Self {
            pid,
            master: Some(master),
            pending: PendingInput::default(),
            exited: false,
            exit_reported: false,
            input_failed: false,
            output_closed: false,
        })
    }
    pub(crate) fn resize(&self, columns: usize, rows: usize) -> io::Result<()> {
        if self.exited {
            return Ok(());
        }
        let size = window(columns, rows);
        if unsafe { libc::tcsetwinsize(self.master.as_ref().unwrap().as_raw_fd(), &size) } < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
    pub(crate) fn send(&mut self, bytes: &[u8]) -> io::Result<()> {
        if self.exited || self.input_failed {
            return Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "terminal input closed",
            ));
        }
        self.pending.enqueue(bytes)
    }
    pub(crate) fn poll(&mut self, buffer: &mut TerminalBuffer) -> bool {
        let mut changed = false;
        if !self.exited {
            let result = unsafe { libc::waitpid(self.pid, core::ptr::null_mut(), libc::WNOHANG) };
            if result == self.pid {
                self.exited = true;
            } else if result < 0 {
                let error = io::Error::last_os_error();
                if error.kind() != io::ErrorKind::Interrupted {
                    buffer.push_message(&format!("terminal: wait failed: {error}"));
                    self.exited = true;
                    changed = true;
                }
            }
        }
        let master = self.master.as_mut().unwrap();
        if !self.exited
            && !self.input_failed
            && let Err(error) = self.pending.flush(master)
        {
            self.input_failed = true;
            buffer.push_message(&format!("terminal: input failed: {error}"));
            changed = true;
        }
        if !self.output_closed {
            let mut bytes = [0; 4096];
            for _ in 0..16 {
                match master.read(&mut bytes) {
                    Ok(0) => {
                        self.output_closed = true;
                        break;
                    }
                    Ok(length) => {
                        buffer.push_bytes(&bytes[..length]);
                        changed = true;
                    }
                    Err(error) if self.exited && error.raw_os_error() == Some(libc::EIO) => {
                        self.output_closed = true;
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(error) => {
                        buffer.push_message(&format!("terminal: output failed: {error}"));
                        self.output_closed = true;
                        changed = true;
                        break;
                    }
                }
            }
        }
        changed
    }
}
impl Drop for Transport {
    fn drop(&mut self) {
        // Closing the master first hangs up the actual controlling terminal.
        drop(self.master.take());
        if self.exited {
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            let result = unsafe { libc::waitpid(self.pid, core::ptr::null_mut(), libc::WNOHANG) };
            if result == self.pid {
                return;
            }
            if result < 0 && io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD) {
                return;
            }
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        unsafe {
            libc::kill(self.pid, libc::SIGKILL);
            libc::waitpid(self.pid, core::ptr::null_mut(), 0);
        }
    }
}
