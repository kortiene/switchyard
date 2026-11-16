//! `fcntl(fd, F_SETFD, FD_CLOEXEC)` for a descriptor this process inherited.
//!
//! `BorrowedFd::borrow_raw` is unsafe, and `switchyard-core` forbids unsafe, so
//! the one borrow lives here. The caller keeps the descriptor open.
#![deny(unsafe_code)]

use std::io;
use std::os::fd::{BorrowedFd, RawFd};

use rustix::io::FdFlags;

/// Set close-on-exec on `fd`. The descriptor stays open in this process.
pub fn set_fd_cloexec(fd: RawFd) -> io::Result<()> {
    let borrowed = borrow(fd)?;
    rustix::io::fcntl_setfd(borrowed, FdFlags::CLOEXEC)?;
    let flags = rustix::io::fcntl_getfd(borrowed)?;
    if !flags.contains(FdFlags::CLOEXEC) {
        return Err(io::Error::other("FD_CLOEXEC was not set"));
    }
    Ok(())
}

/// Write `bytes` to an inherited descriptor. Short writes are retried.
pub fn write_all(fd: RawFd, mut bytes: &[u8]) -> io::Result<()> {
    while !bytes.is_empty() {
        let borrowed = borrow(fd)?;
        match rustix::io::write(borrowed, bytes) {
            Ok(0) => return Err(io::Error::new(io::ErrorKind::WriteZero, "write closed")),
            Ok(n) => bytes = &bytes[n..],
            Err(rustix::io::Errno::INTR) => continue,
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

/// `fd` must be open for the returned borrow and must not be -1.
#[allow(unsafe_code)]
fn borrow(fd: RawFd) -> io::Result<BorrowedFd<'static>> {
    if fd < 0 {
        return Err(io::Error::from_raw_os_error(libc_ebadf()));
    }
    // SAFETY: the caller keeps `fd` open through this borrow, and it is not -1.
    Ok(unsafe { BorrowedFd::borrow_raw(fd) })
}

fn libc_ebadf() -> i32 {
    9
}

#[cfg(test)]
mod tests {
    use std::fs::File;
    use std::os::fd::AsRawFd;

    use rustix::io::FdFlags;

    #[test]
    fn set_fd_cloexec_marks_the_inherited_number() {
        let file = File::open("/dev/null").unwrap();
        let fd = file.as_raw_fd();
        super::set_fd_cloexec(fd).unwrap();
        let flags = rustix::io::fcntl_getfd(&file).unwrap();
        assert!(flags.contains(FdFlags::CLOEXEC));
    }
}
