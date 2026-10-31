//! Preserve the access mode of explicitly supplied stdio descriptors.
use mochi_user_platform::handle::{HANDLE_RIGHT_READ, HANDLE_RIGHT_WRITE};
use std::io;

pub(crate) fn data_rights(flags: i32, required: u64) -> io::Result<u64> {
    let rights = match flags & libc::O_ACCMODE {
        libc::O_RDONLY => HANDLE_RIGHT_READ,
        libc::O_WRONLY => HANDLE_RIGHT_WRITE,
        libc::O_RDWR => HANDLE_RIGHT_READ | HANDLE_RIGHT_WRITE,
        _ => return Err(io::Error::from_raw_os_error(libc::EINVAL)),
    };
    if rights & required != required {
        return Err(io::Error::from_raw_os_error(libc::EACCES));
    }
    Ok(rights)
}
