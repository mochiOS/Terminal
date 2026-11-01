//! A real Linux PTY child. No shell command string is evaluated by Terminal.
use crate::settings::Profile;
use std::{
    fs::File,
    io,
    os::unix::process::CommandExt,
    process::{Command, Stdio},
};
pub(crate) fn spawn(profile: &Profile, slave: &File) -> io::Result<libc::pid_t> {
    let mut command = Command::new(&profile.program);
    command
        .args(&profile.arguments)
        .env("TERM", "xterm-256color")
        .env("COLORTERM", "truecolor")
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave.try_clone()?));
    if !profile.directory.is_empty() {
        command.current_dir(&profile.directory);
    }
    // Only async-signal-safe operations run in the post-fork child.
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn()?;
    Ok(child.id() as libc::pid_t)
}
