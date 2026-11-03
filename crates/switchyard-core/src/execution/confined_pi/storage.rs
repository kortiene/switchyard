//! Host-only control files and no-follow inspection of bridge-writable state.
use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::unix::fs::MetadataExt;
use std::path::{Component, Path, PathBuf};

use rustix::fs::{Mode, OFlags, open, openat};
use serde_json::Value;

use crate::{Error, Result};

pub(super) struct Directory {
    pub path: PathBuf,
    file: File,
}

impl Directory {
    pub fn root(path: &Path) -> Result<Self> {
        if !path.is_absolute() {
            return Err(Error::guard("bridge root must be absolute"));
        }
        let flags = OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC;
        let mut file = File::from(open("/", flags, Mode::empty()).map_err(Error::storage)?);
        for part in path.components() {
            match part {
                Component::RootDir => {}
                Component::Normal(name) => {
                    file = File::from(openat(&file, name, flags, Mode::empty()).map_err(Error::storage)?);
                }
                _ => return Err(Error::guard("bridge root contains a non-normal component")),
            }
        }
        private(&file)?;
        Ok(Self { path: path.into(), file })
    }

    pub fn child(&self, name: &str) -> Result<Self> {
        segment(name)?;
        match rustix::fs::mkdirat(&self.file, name, Mode::from_raw_mode(0o700)) {
            Ok(()) => self.file.sync_all().map_err(Error::storage)?,
            Err(rustix::io::Errno::EXIST) => {}
            Err(error) => return Err(Error::storage(error)),
        }
        let file = File::from(openat(&self.file, name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::empty()).map_err(Error::storage)?);
        private(&file)?;
        Ok(Self { path: self.path.join(name), file })
    }

    pub fn open_file(&self, name: &str, create: bool) -> Result<File> {
        segment(name)?;
        let flags = OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC | OFlags::NONBLOCK
            | if create { OFlags::CREATE } else { OFlags::empty() };
        let file = File::from(openat(&self.file, name, flags, Mode::from_raw_mode(0o600))
            .map_err(Error::storage)?);
        regular(&file)?;
        if create {
            private(&file)?;
            file.sync_all().map_err(Error::storage)?;
            self.file.sync_all().map_err(Error::storage)?;
        }
        Ok(file)
    }

    pub fn read(&self, name: &str) -> Result<Option<Value>> {
        if !self.exists(name)? { return Ok(None); }
        let mut file = self.open_file(name, false)?;
        private(&file)?;
        if file.metadata().map_err(Error::storage)?.len() > 64 * 1024 {
            return Err(Error::guard("oversized bridge control record"));
        }
        let mut text = String::new();
        file.read_to_string(&mut text).map_err(Error::storage)?;
        Ok(Some(serde_json::from_str(&text).map_err(Error::storage)?))
    }

    pub fn exists(&self, name: &str) -> Result<bool> {
        segment(name)?;
        match rustix::fs::statat(&self.file, name, rustix::fs::AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => Ok(true),
            Err(rustix::io::Errno::NOENT) => Ok(false),
            Err(error) => Err(Error::storage(error)),
        }
    }

    pub fn write(&self, name: &str, value: &Value) -> Result<()> {
        segment(name)?;
        if self.exists(name)? { private(&self.open_file(name, false)?)?; }
        let temporary = format!("pending-{}", uuid::Uuid::new_v4());
        let mut file = File::from(openat(&self.file, temporary.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600)).map_err(Error::storage)?);
        file.write_all(&serde_json::to_vec(value).map_err(Error::storage)?).map_err(Error::storage)?;
        file.sync_all().map_err(Error::storage)?;
        rustix::fs::renameat(&self.file, temporary.as_str(), &self.file, name).map_err(Error::storage)?;
        self.file.sync_all().map_err(Error::storage)
    }

    /// Called only after the old cgroup is proven empty. SQLite may write its
    /// own sidecars; the host never follows or removes a bridge-planted link.
    pub fn inspect_state(&self, require_database: bool) -> Result<()> {
        for entry in fs::read_dir(&self.path).map_err(Error::storage)? {
            let entry = entry.map_err(Error::storage)?;
            let name = entry.file_name();
            let name = name.to_str().ok_or_else(|| Error::guard("non-UTF8 bridge state entry"))?;
            regular(&self.open_file(name, false)?)?;
        }
        if require_database && !self.exists("session.sqlite")? {
            return Err(Error::guard("original Pi database is missing; recovery blocked"));
        }
        Ok(())
    }
}

fn segment(name: &str) -> Result<()> {
    if name.is_empty() || name == "." || name == ".." || name.contains('/') || name.contains('\0') {
        return Err(Error::guard("invalid bridge storage component"));
    }
    Ok(())
}

fn private(file: &File) -> Result<()> {
    let meta = file.metadata().map_err(Error::storage)?;
    if meta.uid() != rustix::process::geteuid().as_raw() || meta.mode() & 0o077 != 0 {
        return Err(Error::guard("bridge control storage must be owner-only"));
    }
    Ok(())
}

fn regular(file: &File) -> Result<()> {
    let meta = file.metadata().map_err(Error::storage)?;
    if !meta.is_file() || meta.nlink() != 1 || meta.uid() != rustix::process::geteuid().as_raw() {
        return Err(Error::guard("bridge storage entry must be an owned regular file with exactly one link"));
    }
    Ok(())
}
