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
    pub paths: Vec<String>,
}

/// Changed paths against `base`: committed, unstaged, untracked, and ignored.
///
/// Renames are two paths (`--no-renames`). Line counts ignore attributes
/// (`--text`) and are read only for allowed regular files, stopping at the cap.
pub(crate) fn assess(repo: &Path, base: &str, gate: &Gate) -> Result<Report> {
    refuse_hidden_index(repo)?;
    let mut files = BTreeMap::<String, Entry>::new();
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
            if path_allowed(gate, path) {
                untracked_entry(
                    repo,
                    path,
                    gate.max_added_lines.saturating_sub(counted(&files, gate)),
                )?
            } else {
                Entry {
                    lines: 0,
                    special: false,
                }
            }
        } else {
            Entry {
                lines: added
                    .parse()
                    .map_err(|_| Error::guard("numstat was not a line count"))?,
                special: false,
            }
        };
        files.insert(path.to_string(), entry);
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
            let entry = if path_allowed(gate, path) {
                let room = gate.max_added_lines.saturating_sub(counted(&files, gate));
                untracked_entry(repo, path, room)?
            } else {
                Entry {
                    lines: 0,
                    special: false,
                }
            };
            files.insert(path.to_string(), entry);
        }
    }
    let mut violations = Vec::new();
    let mut added_lines = 0u64;
    for (path, entry) in &files {
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
    Ok(Report {
        paths: files.into_keys().collect(),
        violations,
        added_lines,
    })
}

struct Entry {
    lines: u64,
    special: bool,
}

fn path_allowed(gate: &Gate, path: &str) -> bool {
    !gate.denied.iter().any(|pattern| glob_match(pattern, path))
        && gate.allowed.iter().any(|pattern| glob_match(pattern, path))
}

fn counted(files: &BTreeMap<String, Entry>, gate: &Gate) -> u64 {
    files
        .iter()
        .filter(|(path, entry)| !entry.special && path_allowed(gate, path))
        .fold(0, |sum, (_, entry)| sum.saturating_add(entry.lines))
}

fn untracked_entry(repo: &Path, path: &str, room: u64) -> Result<Entry> {
    let full = repo.join(path);
    let meta = std::fs::symlink_metadata(&full).map_err(Error::storage)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Ok(Entry {
            lines: 0,
            special: true,
        });
    }
    // Already at the cap: do not read. A non-empty file still exceeds it.
    if room == 0 {
        return Ok(Entry {
            lines: u64::from(meta.len() > 0),
            special: false,
        });
    }
    Ok(Entry {
        lines: line_count(&full, room)?.unwrap_or(0),
        special: false,
    })
}

/// `S` is skip-worktree. A lowercase tag is assume-unchanged. Both hide edits.
fn refuse_hidden_index(repo: &Path) -> Result<()> {
    let raw = host_git(repo, &["ls-files", "-v", "-z"])?;
    for record in raw.split('\0').filter(|record| !record.is_empty()) {
        let tag = record.chars().next().unwrap_or(' ');
        if tag == 'S' || tag.is_ascii_lowercase() {
            return Err(Error::guard(
                "assume-unchanged or skip-worktree hides verifier paths",
            ));
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

/// `None` is a symlink or special file. Regular files stop once `room` newlines
/// are counted, so a huge allowed file is not read in full.
fn line_count(path: &Path, room: u64) -> Result<Option<u64>> {
    let meta = std::fs::symlink_metadata(path).map_err(Error::storage)?;
    if meta.file_type().is_symlink() || !meta.file_type().is_file() {
        return Ok(None);
    }
    let mut file = std::fs::File::open(path).map_err(Error::storage)?;
    let mut buf = [0u8; 8192];
    let mut lines = 0u64;
    let mut last_newline = true;
    let mut saw_byte = false;
    loop {
        let read = std::io::Read::read(&mut file, &mut buf).map_err(Error::storage)?;
        if read == 0 {
            break;
        }
        saw_byte = true;
        for byte in &buf[..read] {
            if *byte == b'\n' {
                lines += 1;
                last_newline = true;
                if lines > room {
                    return Ok(Some(lines));
                }
            } else {
                last_newline = false;
            }
        }
    }
    if saw_byte && !last_newline {
        lines += 1;
    }
    Ok(Some(lines))
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
