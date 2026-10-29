use std::fs::{File, OpenOptions};
use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Exclusive single-host owner lock.
///
/// The lock is an `flock` on a sibling of the mission database. It is released
/// when this value is dropped, including after a crash. A second live process
/// fails with [`Error::SecondActiveOwner`]. The database generation is advanced
/// separately by the store after the lock is held; generation checks reject
/// stale callers, and the flock stops a second writer on this host.
pub struct OwnerLock {
    file: File,
    pub generation: u64,
    pub token: String,
    pub host_id: String,
    pub pid: u32,
}

impl Drop for OwnerLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

pub fn lock_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push(".owner.lock");
    PathBuf::from(name)
}

pub fn try_acquire(db_path: &Path) -> Result<OwnerLock> {
    let path = lock_path(db_path);
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(Error::storage)?;
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(Error::storage)?;
    match file.try_lock() {
        Ok(()) => Ok(OwnerLock {
            file,
            generation: 0,
            token: String::new(),
            host_id: host_id(),
            pid: std::process::id(),
        }),
        Err(std::fs::TryLockError::WouldBlock) => Err(Error::SecondActiveOwner),
        Err(std::fs::TryLockError::Error(error)) => Err(Error::storage(error)),
    }
}

fn host_id() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "localhost".to_string())
}
