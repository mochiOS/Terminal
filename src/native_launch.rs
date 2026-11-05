//! Adapt this application's local FDs to the shared verified launcher.
use mochi_user_platform as p;
use mochios_posix_client::{Client, ClientError, SyscallEndpointTransport, newlib_errno};
use mochios_posix_protocol::{self as protocol, ExportedFdHandle};
use std::io;

unsafe extern "C" {
    fn mochi_fd_export(fd: libc::c_int, rights: u64, output: *mut ExportedFdHandle) -> libc::c_int;
}

struct Reference(u32);
pub(crate) fn master_reference(fd: libc::c_int) -> io::Result<u32> {
    let rights = p::handle::HANDLE_RIGHT_CONTROL
        | p::handle::HANDLE_RIGHT_TRANSFER
        | p::handle::HANDLE_RIGHT_DUPLICATE;
    let mut output = ExportedFdHandle::default();
    // This FD API also encodes inherited data-access metadata. Capture through
    // its readable descriptor contract, then discard data authority locally;
    // neither the broker nor POSIX observations receive READ/WRITE rights.
    if unsafe { mochi_fd_export(fd, rights | p::handle::HANDLE_RIGHT_READ, &mut output) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let narrowed = p::handle::duplicate(output.handle, rights).map_err(native);
    let closed = p::handle::close(output.handle).map_err(native);
    match (narrowed, closed) {
        (Ok(handle), Ok(())) => Ok(handle),
        (Ok(handle), Err(error)) => {
            let _ = p::handle::close(handle);
            Err(error)
        }
        (Err(error), _) => Err(error),
    }
}
impl Drop for Reference {
    fn drop(&mut self) {
        let _ = p::handle::close(self.0);
    }
}
struct Exported(ExportedFdHandle);
impl Exported {
    fn new(fd: libc::c_int, direction: u64) -> io::Result<Self> {
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags < 0 {
            return Err(io::Error::last_os_error());
        }
        // A PTY slave is normally O_RDWR on all three slots. Its input FD
        // must remain writable when login_tty duplicates it onto stdout.
        // mochi_fd_export still enforces the source Handle's rights ceiling.
        let direction = crate::stdio_access::data_rights(flags, direction)?;
        let rights = direction
            | p::handle::HANDLE_RIGHT_WAIT
            | p::handle::HANDLE_RIGHT_CONTROL
            | p::handle::HANDLE_RIGHT_DUPLICATE
            | p::handle::HANDLE_RIGHT_TRANSFER;
        let mut output = ExportedFdHandle::default();
        if unsafe { mochi_fd_export(fd, rights, &mut output) } != 0 {
            let error = io::Error::last_os_error();
            return Err(io::Error::new(
                error.kind(),
                format!("export stdio fd {fd}: {error}"),
            ));
        }
        Ok(Self(output))
    }
    fn descriptor(&self) -> p::application::Descriptor {
        p::application::Descriptor {
            handle: self.0.handle,
            rights: self.0.rights,
            metadata: self.0.metadata,
        }
    }
}
impl Drop for Exported {
    fn drop(&mut self) {
        let _ = p::handle::close(self.0.handle);
    }
}
fn native(error: p::syscall::SysError) -> io::Error {
    io::Error::from_raw_os_error(newlib_errno::from_status(error.raw()))
}
fn remote(error: ClientError<p::syscall::SysError>) -> io::Error {
    match error {
        ClientError::Transport(error) => native(error),
        ClientError::Remote(status) => {
            io::Error::from_raw_os_error(newlib_errno::from_status(i64::from(status)))
        }
        _ => io::Error::other("invalid POSIX launch-scope reply"),
    }
}

pub(crate) fn spawn(
    path: &str,
    arguments: &[&str],
    stdio: [libc::c_int; 3],
) -> io::Result<libc::pid_t> {
    let exported = [
        Exported::new(stdio[0], p::handle::HANDLE_RIGHT_READ)?,
        Exported::new(stdio[1], p::handle::HANDLE_RIGHT_WRITE)?,
        Exported::new(stdio[2], p::handle::HANDLE_RIGHT_WRITE)?,
    ];
    let posix = p::handle::inherited(protocol::CONTROL_HANDLE_KEY)
        .map_err(|error| contextual("POSIX endpoint", error))?;
    let client = Client::new(SyscallEndpointTransport::new(posix));
    let scope = client.create_read_scope().map_err(|error| {
        let error = remote(error);
        io::Error::new(error.kind(), format!("capture launch scope: {error}"))
    })?;
    let reference = Reference(scope.handle);
    let destination = p::process::find_by_name("capability.service").map_err(native)?;
    let destination = u32::try_from(destination)
        .ok()
        .filter(|value| *value != 0)
        .ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "capability.service is unavailable")
        })?;
    let mut environment: Vec<String> = [
        "HOME",
        "USER",
        "LOGNAME",
        "PATH",
        "XDG_RUNTIME_DIR",
        "MOCHI_SESSION_ID",
    ]
    .into_iter()
    .filter_map(|key| {
        std::env::var(key)
            .ok()
            .map(|value| format!("{key}={value}"))
    })
    .collect();
    // The shared VT screen handles ANSI colors, cursor addressing and alternate screens.
    environment.push(String::from("TERM=xterm-256color"));
    environment.push(String::from("COLORTERM=truecolor"));
    let environment: Vec<&str> = environment.iter().map(String::as_str).collect();
    let process = p::application::spawn_with_stdio(
        destination,
        path,
        arguments,
        &environment,
        p::ipc::IpcObjectHandle {
            handle: reference.0,
            reserved: 0,
            rights: protocol::read_scope::ATTACH_RIGHTS,
        },
        &exported.each_ref().map(Exported::descriptor),
    )
    .map_err(|error| contextual("verified application launch", error))?;
    libc::pid_t::try_from(process)
        .map_err(|_| io::Error::from_raw_os_error(newlib_errno::EOVERFLOW))
}

fn contextual(stage: &str, error: p::syscall::SysError) -> io::Error {
    let cause = native(error);
    io::Error::new(
        cause.kind(),
        format!("{stage}: {cause} (status {})", error.raw()),
    )
}
