//! Changed-path gate for the host verifier. Does not execute `argv`.
use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use crate::error::{Error, Result};
use crate::execution::host_git;

pub(crate) struct Gate {
    allowed: Vec<String>,
    denied: Vec<String>,
    max_added_lines: u64,
}

impl Gate {
    pub(crate) fn parse(doc: &Value) -> Result<Self> {
        if doc.get("id").and_then(Value::as_str) != Some("switchyard.verifier.rust-verify.v1") {
            return Err(Error::VerifierGateUnavailable);
        }
        Ok(Self {
            allowed: strings(doc, "allowedPaths")?,
            denied: strings(doc, "deniedPaths")?,
            max_added_lines: doc
                .get("maxAddedLines")
                .and_then(Value::as_u64)
                .ok_or_else(|| Error::guard("maxAddedLines is required"))?,
        })
    }
}

pub(crate) struct Report {
    pub violations: Vec<String>,
    pub added_lines: u64,
    /// First 4096 changed paths, in path order.
    pub paths: Vec<String>,
    /// Every changed path, including those past the stored list.
    pub path_count: usize,
}

const MAX_CHANGED_PATHS: usize = 4096;
/// `line_count` stops once a file passes this many bytes.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// Changed paths against `base`: committed, unstaged, untracked, and ignored.
///
/// Renames are two paths (`--no-renames`). Line counts ignore attributes
/// (`--text`) and are read only for allowed regular files, stopping at the cap.
pub(crate) fn assess(repo: &Path, base: &str, gate: &Gate) -> Result<Report> {
    refuse_hidden_index(repo)?;
    let mut files = BTreeMap::<String, Entry>::new();
    // Running total of allowed lines. Once it passes the cap, later files are
    // not read. Re-walking the map for every path would be quadratic.
    let mut tallied = 0u64;
    let diff = host_git(
        repo,
        &[
            "diff",
            "--no-renames",
            "--no-ext-diff",
            "--no-textconv",
            "--text",
            "--numstat",
            "-z",
            base,
        ],
    )?;
    for record in diff.split('\0').filter(|record| !record.is_empty()) {
        let mut parts = record.splitn(3, '\t');
        let added = parts.next().unwrap_or("");
        let _deleted = parts.next();
        let Some(path) = parts.next().filter(|path| !path.is_empty()) else {
            continue;
        };
        // `--text` does not override an unset `diff` attribute on Git 2.43, so
        // a `-` stat is counted from disk for allowed paths instead of as zero.
        let entry = if added == "-" {
            disk_entry(repo, gate, path, &mut tallied)?
        } else {
            Entry {
                lines: added
                    .parse()
                    .map_err(|_| Error::guard("numstat was not a line count"))?,
                special: false,
                max_bytes: false,
            }
        };
        remember(&mut files, gate, path, entry, &mut tallied);
    }
    for args in [
        &["ls-files", "-z", "-o", "--exclude-standard"][..],
        &["ls-files", "-z", "-o", "-i", "--exclude-standard"][..],
    ] {
        for path in host_git(repo, args)?
            .split('\0')
            .filter(|path| !path.is_empty())
        {
            if files.contains_key(path) {
                continue;
            }
            let entry = disk_entry(repo, gate, path, &mut tallied)?;
            remember(&mut files, gate, path, entry, &mut tallied);
        }
    }
    let mut violations = Vec::new();
    let mut added_lines = 0u64;
    for (path, entry) in &files {
        if entry.max_bytes {
            violations.push(format!("maxBytes:{path}"));
            continue;
        }
        if entry.special {
            violations.push(format!("special:{path}"));
            continue;
        }
        if gate.denied.iter().any(|pattern| glob_match(pattern, path)) {
            violations.push(format!("denied:{path}"));
        } else if !path_allowed(gate, path) {
            violations.push(format!("outside:{path}"));
        } else {
            added_lines = added_lines.saturating_add(entry.lines);
        }
    }
    if added_lines > gate.max_added_lines {
        violations.push(format!("maxAddedLines:{added_lines}"));
    }
    let path_count = files.len();
    if path_count > MAX_CHANGED_PATHS {
        violations.push(format!("maxChangedPaths:{path_count}"));
    }
    Ok(Report {
        paths: files.into_keys().take(MAX_CHANGED_PATHS).collect(),
        violations,
        added_lines,
        path_count,
    })
}

struct Entry {
    lines: u64,
    special: bool,
    max_bytes: bool,
}

fn path_allowed(gate: &Gate, path: &str) -> bool {
    !gate.denied.iter().any(|pattern| glob_match(pattern, path))
        && gate.allowed.iter().any(|pattern| glob_match(pattern, path))
}

fn remember(
    files: &mut BTreeMap<String, Entry>,
    gate: &Gate,
    path: &str,
    entry: Entry,
    tallied: &mut u64,
) {
    if !entry.special && !entry.max_bytes && path_allowed(gate, path) {
        *tallied = tallied.saturating_add(entry.lines);
    }
    files.insert(path.to_string(), entry);
}

/// Allowed paths are counted from disk until the cap is passed. After that,
/// further files are not read.
fn disk_entry(repo: &Path, gate: &Gate, path: &str, tallied: &mut u64) -> Result<Entry> {
    if !path_allowed(gate, path) {
        return Ok(Entry {
            lines: 0,
            special: false,
            max_bytes: false,
        });
    }
    if *tallied > gate.max_added_lines {
        return Ok(Entry {
            lines: 0,
            special: false,
            max_bytes: false,
        });
    }
    untracked_entry(repo, path, gate.max_added_lines.saturating_sub(*tallied))
}

/// From-disk line count when Git will not diff the path.
///
/// This deliberately overcounts: it counts every line in the file, not the
/// added lines of a real diff. Do not replace it with a diff.
fn untracked_entry(repo: &Path, path: &str, room: u64) -> Result<Entry> {
    if parent_is_symlink(repo, path)? {
        return Ok(Entry {
            lines: 0,
            special: true,
            max_bytes: false,
        });
    }
    let full = repo.join(path);
    let meta = match std::fs::symlink_metadata(&full) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Entry {
                lines: 0,
                special: false,
                max_bytes: false,
            });
        }
        Err(error) => return Err(Error::storage(error)),
    };
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Ok(Entry {
            lines: 0,
            special: true,
            max_bytes: false,
        });
    }
    // Already at the cap: do not read. A non-empty file still exceeds it.
    if room == 0 {
        return Ok(Entry {
            lines: u64::from(meta.len() > 0),
            special: false,
            max_bytes: false,
        });
    }
    // A file that is already larger than the byte cap is `maxBytes` without a read.
    if meta.len() > MAX_FILE_BYTES {
        return Ok(Entry {
            lines: 0,
            special: false,
            max_bytes: true,
        });
    }
    Ok(match line_count(&full, room)? {
        LineRead::Lines(lines) => Entry {
            lines,
            special: false,
            max_bytes: false,
        },
        LineRead::Special => Entry {
            lines: 0,
            special: true,
            max_bytes: false,
        },
        LineRead::MaxBytes => Entry {
            lines: 0,
            special: false,
            max_bytes: true,
        },
    })
}

/// `S` is skip-worktree. A lowercase tag is assume-unchanged. Both hide edits.
fn refuse_hidden_index(repo: &Path) -> Result<()> {
    let raw = host_git(repo, &["ls-files", "-v", "-z"])?;
    for record in raw.split('\0').filter(|record| !record.is_empty()) {
        let tag = record.chars().next().unwrap_or(' ');
        if tag == 'S' || tag.is_ascii_lowercase() {
            return Err(Error::VerifierIndexFlagsSet);
        }
    }
    Ok(())
}

fn strings(doc: &Value, key: &str) -> Result<Vec<String>> {
    doc.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| Error::guard(format!("{key} is required")))?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| Error::guard(format!("{key} must be strings")))
        })
        .collect()
}

/// A parent component that is a symlink would make `lstat` of the leaf look
/// outside the worktree. The leaf itself is classified by the caller.
fn parent_is_symlink(repo: &Path, path: &str) -> Result<bool> {
    let mut cursor = repo.to_path_buf();
    let mut parts = path.split('/').filter(|part| !part.is_empty()).peekable();
    while let Some(part) = parts.next() {
        if parts.peek().is_none() {
            break;
        }
        cursor.push(part);
        match std::fs::symlink_metadata(&cursor) {
            Ok(meta) if meta.file_type().is_symlink() => return Ok(true),
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(Error::storage(error)),
        }
    }
    Ok(false)
}

enum LineRead {
    Lines(u64),
    /// Symlink or other non-regular file.
    Special,
    /// The file passed the 1 MiB byte cap.
    MaxBytes,
}

/// Regular files stop once `room` newlines are counted, or once the byte cap
/// is passed, so a huge allowed file is not read in full.
fn line_count(path: &Path, room: u64) -> Result<LineRead> {
    let mut file = match crate::regular_file::open_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(LineRead::Lines(0));
        }
        Err(error) if error.raw_os_error() == Some(libc::ELOOP) => return Ok(LineRead::Special),
        Err(error) => return Err(Error::storage(error)),
    };
    // fstat the open handle. A FIFO swapped in after the earlier lstat must not be read.
    let meta = file.metadata().map_err(Error::storage)?;
    if !meta.file_type().is_file() {
        return Ok(LineRead::Special);
    }
    let mut buf = [0u8; 8192];
    let mut lines = 0u64;
    let mut bytes = 0u64;
    let mut last_newline = true;
    let mut saw_byte = false;
    loop {
        let read = std::io::Read::read(&mut file, &mut buf).map_err(Error::storage)?;
        if read == 0 {
            break;
        }
        bytes = bytes.saturating_add(u64::try_from(read).unwrap_or(u64::MAX));
        if bytes > MAX_FILE_BYTES {
            return Ok(LineRead::MaxBytes);
        }
        saw_byte = true;
        for byte in &buf[..read] {
            if *byte == b'\n' {
                lines += 1;
                last_newline = true;
                if lines > room {
                    return Ok(LineRead::Lines(lines));
                }
            } else {
                last_newline = false;
            }
        }
    }
    if saw_byte && !last_newline {
        lines += 1;
    }
    Ok(LineRead::Lines(lines))
}

fn glob_match(pattern: &str, path: &str) -> bool {
    fn rec(pattern: &[&str], path: &[&str]) -> bool {
        match (pattern.split_first(), path.split_first()) {
            (None, None) => true,
            (Some((star, rest)), _) if *star == "**" => {
                rec(rest, path)
                    || path
                        .split_first()
                        .is_some_and(|(_, tail)| rec(pattern, tail))
            }
            (Some((segment, rest)), Some((name, tail))) if *segment == "*" || segment == name => {
                rec(rest, tail)
            }
            _ => false,
        }
    }
    let pattern: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    let path: Vec<&str> = path.split('/').filter(|part| !part.is_empty()).collect();
    rec(&pattern, &path)
}

#[cfg(test)]
mod line_count_tests {
    use super::{LineRead, MAX_FILE_BYTES, line_count};

    #[test]
    fn line_count_stops_when_the_stream_passes_one_mebibyte() {
        let dir =
            std::env::temp_dir().join(format!("switchyard-line-count-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("wide.rs");
        let mut body = vec![b'x'; usize::try_from(MAX_FILE_BYTES).unwrap() + 8];
        body.push(b'\n');
        std::fs::write(&path, body).unwrap();
        assert!(matches!(line_count(&path, 10).unwrap(), LineRead::MaxBytes));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
