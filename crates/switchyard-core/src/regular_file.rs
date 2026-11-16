//! No-follow open of a regular file. Operator write reads a capped body.
//! The host verifier uses the same open and keeps its own line cap.
//!
//! The file is opened once with `O_NOFOLLOW|O_NONBLOCK`, then `fstat`ed.
//! At most `limit` bytes are read.

use std::fs::OpenOptions;
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use crate::error::{Error, Result};

/// Open for reading with `O_NOFOLLOW|O_NONBLOCK`. The caller maps the `io::Error`.
pub(crate) fn open_nofollow(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.read(true);
    options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    options.open(path)
}

/// Read up to `limit` bytes from a regular file without following a symlink.
///
/// A missing path, a symlink, a FIFO, or any other non-regular file is
/// `guard_failed`. The read stops at `limit`, so a caller that passes
/// `max + 1` can tell an oversized file from a full one.
pub(crate) fn read_regular_nofollow(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let mut file = match open_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::guard("source file is missing"));
        }
        Err(error) if is_not_regular(&error) => {
            return Err(Error::guard("source is not a regular file"));
        }
        Err(error) => return Err(Error::storage(error)),
    };
    let meta = file.metadata().map_err(Error::storage)?;
    if !meta.is_file() {
        return Err(Error::guard("source is not a regular file"));
    }
    let mut buf = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        if buf.len() >= limit {
            break;
        }
        let want = (limit - buf.len()).min(chunk.len());
        match file.read(&mut chunk[..want]) {
            Ok(0) => break,
            Ok(read) => buf.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) if is_not_regular(&error) => {
                return Err(Error::guard("source is not a regular file"));
            }
            Err(error) => return Err(Error::storage(error)),
        }
    }
    Ok(buf)
}

fn is_not_regular(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(code) if code == libc::ELOOP || code == libc::ENXIO || code == libc::EAGAIN
    )
}

#[cfg(all(test, unix))]
mod tests {
    use super::read_regular_nofollow;
    use std::os::unix::fs::symlink;
    use std::time::{Duration, Instant};

    #[test]
    fn a_symlink_or_fifo_is_refused_without_following_or_blocking() {
        let dir = std::env::temp_dir().join(format!("switchyard-nofollow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let secret = dir.join("secret");
        std::fs::write(&secret, "SECRET-RESULT").unwrap();
        let link = dir.join("result.json");
        symlink(&secret, &link).unwrap();
        let started = Instant::now();
        let error = read_regular_nofollow(&link, 64).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2), "{error}");
        assert!(!error.to_string().contains("SECRET-RESULT"), "{error}");
        let _ = std::fs::remove_file(&link);
        assert!(
            std::process::Command::new("mkfifo")
                .arg(&link)
                .status()
                .unwrap()
                .success()
        );
        let started = Instant::now();
        let error = read_regular_nofollow(&link, 64).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(2), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
