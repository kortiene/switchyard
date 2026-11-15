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
            return Err(Error::guard("verifier has no host path gate"));
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

/// Violations and added lines. Committed, unstaged, untracked, and ignored paths.
pub(crate) fn assess(repo: &Path, base: &str, gate: &Gate) -> Result<(Vec<String>, u64)> {
    let mut files = BTreeMap::new();
    let diff = host_git(repo, &["diff", "--numstat", "-z", base])?;
    for record in diff.split('\0').filter(|record| !record.is_empty()) {
        let mut parts = record.splitn(3, '\t');
        let added = parts.next().unwrap_or("");
        let _deleted = parts.next();
        let Some(path) = parts.next().filter(|path| !path.is_empty()) else {
            continue;
        };
        let added = if added == "-" {
            0
        } else {
            added
                .parse()
                .map_err(|_| Error::guard("numstat was not a line count"))?
        };
        files.insert(path.to_string(), added);
    }
    for args in [
        &["ls-files", "-z", "-o", "--exclude-standard"][..],
        &["ls-files", "-z", "-o", "-i", "--exclude-standard"][..],
    ] {
        for path in host_git(repo, args)?
            .split('\0')
            .filter(|path| !path.is_empty())
        {
            let lines = line_count(&repo.join(path))?;
            files.entry(path.to_string()).or_insert(lines);
        }
    }
    let mut violations = Vec::new();
    let mut added_lines = 0u64;
    for (path, lines) in &files {
        added_lines = added_lines.saturating_add(*lines);
        if gate.denied.iter().any(|pattern| glob_match(pattern, path)) {
            violations.push(format!("denied:{path}"));
        } else if !gate.allowed.iter().any(|pattern| glob_match(pattern, path)) {
            violations.push(format!("outside:{path}"));
        }
    }
    if added_lines > gate.max_added_lines {
        violations.push(format!("maxAddedLines:{added_lines}"));
    }
    Ok((violations, added_lines))
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

fn line_count(path: &Path) -> Result<u64> {
    let bytes = std::fs::read(path).map_err(Error::storage)?;
    if bytes.is_empty() {
        return Ok(0);
    }
    let newlines = bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
    Ok(newlines + u64::from(!bytes.ends_with(b"\n")))
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
