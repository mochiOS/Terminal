//! A PTY master owned by one pane; only its slave is delegated to the child.
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
    pub(crate) output_closed: bool,
    pub(crate) exit_status: Option<i32>,
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
    pub(crate) fn start(
        columns: usize,
        rows: usize,
        profile: &crate::settings::Profile,
    ) -> io::Result<Self> {
        profile
            .validate()
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
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
        #[cfg(target_os = "mochios")]
        let pid = {
            let arguments: Vec<&str> = std::iter::once(profile.program.as_str())
                .chain(profile.arguments.iter().map(String::as_str))
                .collect();
            crate::native_launch::spawn(&profile.program, &arguments, [slave.as_raw_fd(); 3])?
        };
        #[cfg(target_os = "linux")]
        let pid = crate::linux_launch::spawn(profile, &slave)?;
        Ok(Self {
            pid,
            master: Some(master),
            pending: PendingInput::default(),
            exited: false,
            exit_reported: false,
            input_failed: false,
            output_closed: false,
            exit_status: None,
        })
    }
    pub(crate) fn resize(&self, columns: usize, rows: usize) -> io::Result<()> {
        if self.exited {
            return Ok(());
        }
        let size = window(columns, rows);
        #[cfg(target_os = "mochios")]
        let result =
            unsafe { libc::tcsetwinsize(self.master.as_ref().unwrap().as_raw_fd(), &size) };
        #[cfg(target_os = "linux")]
        let result = unsafe {
            libc::ioctl(
                self.master.as_ref().unwrap().as_raw_fd(),
                libc::TIOCSWINSZ,
                &size,
            )
        };
        if result < 0 {
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
        // Input and VT query replies must not wait for another rendered frame.
        // The nonblocking queue retains any bytes the PTY cannot accept yet.
        self.pending.send(bytes, self.master.as_mut().unwrap())
    }
    pub(crate) fn poll(&mut self, buffer: &mut TerminalBuffer) -> bool {
        let mut changed = false;
        if !self.exited {
            let mut status = 0;
            let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
            if result == self.pid {
                self.exit_status = Some(status);
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
                    Err(error) if error.raw_os_error() == Some(libc::EIO) => {
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
            let mut status = 0;
            let result = unsafe { libc::waitpid(self.pid, &mut status, libc::WNOHANG) };
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
            #[cfg(target_os = "linux")]
            libc::kill(-self.pid, libc::SIGKILL);
            libc::kill(self.pid, libc::SIGKILL);
            libc::waitpid(self.pid, core::ptr::null_mut(), 0);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use crate::settings::Profile;
    fn until(
        t: &mut Transport,
        b: &mut TerminalBuffer,
        predicate: impl Fn(&Transport, &TerminalBuffer) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            t.poll(b);
            if predicate(t, b) {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "PTY timed out: {}",
                b.screen().contents()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    #[test]
    fn real_pty_passes_literal_arguments_cwd_colors_resize_and_exit_status() {
        let p=Profile{name:"test".into(),program:"/bin/sh".into(),arguments:vec!["-c".into(),"printf '\\033[31mREADY\\033[0m:%s:' \"$1\"; pwd; read x; stty size; printf 'DONE:%s' \"$x\"; exit 7".into(),"sh".into(),"$(literal argument)".into()],directory:"/tmp".into()};
        let mut t = Transport::start(80, 24, &p).unwrap();
        let mut b = TerminalBuffer::default();
        until(&mut t, &mut b, |_, b| {
            b.screen().contents().contains("/tmp")
        });
        assert!(b.screen().contents().contains("$(literal argument)"));
        assert_eq!(
            b.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
        t.resize(91, 31).unwrap();
        t.send(b"hello\r").unwrap();
        until(&mut t, &mut b, |t, _| t.exited && t.output_closed);
        assert!(b.screen().contents().contains("31 91"));
        assert!(b.screen().contents().contains("DONE:hello"));
        assert_eq!(t.exit_status.unwrap() >> 8, 7);
    }
    #[test]
    fn interrupt_reaches_the_foreground_pty_and_exited_output_is_fully_drained() {
        let p = Profile {
            program: "/bin/sh".into(),
            arguments: vec![
                "-c".into(),
                "trap 'printf INTERRUPTED; exit 0' INT; printf READY; while :; do read x; done"
                    .into(),
            ],
            ..Profile::default()
        };
        let mut t = Transport::start(80, 24, &p).unwrap();
        let mut b = TerminalBuffer::default();
        until(&mut t, &mut b, |_, b| {
            b.screen().contents().contains("READY")
        });
        t.send(&[3]).unwrap();
        until(&mut t, &mut b, |t, _| t.exited && t.output_closed);
        assert!(b.screen().contents().contains("INTERRUPTED"));
        let p=Profile{program:"/bin/sh".into(),arguments:vec!["-c".into(),"i=0; while [ $i -lt 3000 ]; do printf 'abcdefghijklmnopqrstuvwxyz0123456789\\n'; i=$((i+1)); done; printf FINAL_OUTPUT_MARKER".into()],..Profile::default()};
        let mut t = Transport::start(80, 24, &p).unwrap();
        let mut b = TerminalBuffer::new(100);
        until(&mut t, &mut b, |t, _| t.exited && t.output_closed);
        assert!(b.screen().contents().contains("FINAL_OUTPUT_MARKER"));
    }
    #[test]
    fn closing_live_pane_reaps_the_real_child() {
        let p = Profile {
            program: "/bin/sh".into(),
            arguments: vec!["-c".into(), "sleep 60".into()],
            ..Profile::default()
        };
        let t = Transport::start(80, 24, &p).unwrap();
        let pid = t.pid;
        drop(t);
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
    }
}
