//! Linux ExecutionEnv contract. The OS backend is feature-gated; ledger views
//! exist in every build so a default host still blocks and explains v7 rows.
#![cfg_attr(
    not(all(feature = "linux-execution", target_os = "linux")),
    allow(dead_code)
)]
use crate::{Error, Result, RuntimeBinding, operation_id, payload_hash};
use serde_json::{Value, json};

pub const READ_SCOPE: &str = "linux.file.read.v1";
pub const RANGE_SCOPE: &str = "linux.file.range_read.v1";
pub const LIST_SCOPE: &str = "linux.dir.list.v1";
pub const WRITE_SCOPE: &str = "linux.file.write.v1";
pub const EDIT_SCOPE: &str = "linux.file.edit.v1";
pub const EXEC_START_SCOPE: &str = "linux.exec.start.v1";
pub const EXEC_STATUS_SCOPE: &str = "linux.exec.status.v1";
pub const EXEC_CANCEL_SCOPE: &str = "linux.exec.cancel.v1";

pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const EXEC_OUTPUT_CAP: usize = 64 * 1024;
/// Verifier stdout and stderr tail. Matches the frozen document's output cap.
pub const VERIFIER_OUTPUT_CAP: usize = 1024 * 1024;
pub const MAX_DEADLINE_MS: i64 = 60_000;
/// Reviewed Bubblewrap package. The helper, the host, and receipts share this pin.
pub const BUBBLEWRAP_VERSION: &str = "0.9.0";
pub const BUBBLEWRAP_BACKEND: &str = "linux-bubblewrap-0.9.0";
/// Verifier `cpu.max`: two CPUs. Writers stay at half a CPU.
pub const VERIFIER_CPU_MAX: &str = "200000 100000";
pub const VERIFIER_MEMORY_MAX: &str = "4G";
pub const VERIFIER_PIDS_MAX: &str = "2048";
/// Stored stdout and stderr tails. The hash still covers the full stream.
pub const VERIFIER_TAIL_BYTES: usize = 64 * 1024;

/// Worst case is two 64 KiB tails stored as JSON text (about 6 bytes per byte)
/// plus the envelope. Base64 keeps the common case smaller. 1 MiB covers both.
pub fn verifier_result_cap() -> usize {
    1024 * 1024
}

/// `oom_kill` from a cgroup `memory.events` body.
pub fn memory_events_oom_kill(text: &str) -> u64 {
    text.lines()
        .find_map(|line| {
            let (name, value) = line.split_once(' ')?;
            (name == "oom_kill").then(|| value.parse::<u64>().ok())?
        })
        .unwrap_or(0)
}

pub(crate) fn b64_encode(bytes: &[u8]) -> String {
    const ALPHA: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    let mut index = 0;
    while index + 3 <= bytes.len() {
        let n = u32::from(bytes[index]) << 16
            | u32::from(bytes[index + 1]) << 8
            | u32::from(bytes[index + 2]);
        out.push(char::from(ALPHA[((n >> 18) & 63) as usize]));
        out.push(char::from(ALPHA[((n >> 12) & 63) as usize]));
        out.push(char::from(ALPHA[((n >> 6) & 63) as usize]));
        out.push(char::from(ALPHA[(n & 63) as usize]));
        index += 3;
    }
    if bytes.len() - index == 1 {
        let n = u32::from(bytes[index]) << 16;
        out.push(char::from(ALPHA[((n >> 18) & 63) as usize]));
        out.push(char::from(ALPHA[((n >> 12) & 63) as usize]));
        out.push('=');
        out.push('=');
    } else if bytes.len() - index == 2 {
        let n = u32::from(bytes[index]) << 16 | u32::from(bytes[index + 1]) << 8;
        out.push(char::from(ALPHA[((n >> 18) & 63) as usize]));
        out.push(char::from(ALPHA[((n >> 12) & 63) as usize]));
        out.push(char::from(ALPHA[((n >> 6) & 63) as usize]));
        out.push('=');
    }
    out
}

pub(crate) fn b64_decode(text: &str) -> Option<Vec<u8>> {
    fn val(byte: u8) -> Option<u8> {
        match byte {
            b'A'..=b'Z' => Some(byte - b'A'),
            b'a'..=b'z' => Some(byte - b'a' + 26),
            b'0'..=b'9' => Some(byte - b'0' + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes = text.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::new();
    let mut index = 0;
    while index < bytes.len() {
        let pad = (bytes[index + 2] == b'=') as usize + (bytes[index + 3] == b'=') as usize;
        let a = val(bytes[index])?;
        let b = val(bytes[index + 1])?;
        let c = if pad == 2 { 0 } else { val(bytes[index + 2])? };
        let d = if pad >= 1 { 0 } else { val(bytes[index + 3])? };
        let n = (u32::from(a) << 18) | (u32::from(b) << 12) | (u32::from(c) << 6) | u32::from(d);
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
        index += 4;
    }
    Some(out)
}

#[cfg(test)]
mod memory_events_tests {
    use super::{b64_decode, b64_encode, memory_events_oom_kill};

    #[test]
    fn memory_events_oom_kill_is_counted() {
        assert_eq!(memory_events_oom_kill("low 0\noom_kill 3\n"), 3);
        assert_eq!(memory_events_oom_kill("oom 1\n"), 0);
        assert_eq!(memory_events_oom_kill(""), 0);
    }

    #[test]
    fn base64_round_trip_keeps_control_bytes() {
        let bytes: Vec<u8> = (0..256).map(|value| value as u8).collect();
        let encoded = b64_encode(&bytes);
        assert!(
            encoded
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
        );
        assert_eq!(b64_decode(&encoded).as_deref(), Some(bytes.as_slice()));
    }
}

pub fn bubblewrap_version_line() -> String {
    format!("bubblewrap {BUBBLEWRAP_VERSION}\n")
}

/// `channel` from a rust-toolchain.toml body. A comment line is ignored.
/// A `channel` key is accepted even when it is not under `[toolchain]`.
pub fn toolchain_channel(text: &str) -> Option<&str> {
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some(rest) = line.strip_prefix("channel") else {
            continue;
        };
        let Some(rest) = rest.trim().strip_prefix('=') else {
            continue;
        };
        let rest = rest.trim().trim_matches('"').trim_matches('\'');
        if !rest.is_empty() {
            return Some(rest);
        }
    }
    None
}

#[cfg(test)]
mod toolchain_channel_tests {
    use super::toolchain_channel;

    #[test]
    fn toolchain_channel_skips_the_table_header_comments_and_crlf() {
        let text = "[toolchain]\nchannel = \"1.99.1\"\ncomponents = [\"rustfmt\", \"clippy\"]\n";
        assert_eq!(toolchain_channel(text), Some("1.99.1"));
        assert_eq!(toolchain_channel("channel = 'stable'\n"), Some("stable"));
        assert_eq!(toolchain_channel("[toolchain]\n"), None);
        assert_eq!(
            toolchain_channel("# channel = \"ignore\"\nchannel = \"1.99.1\"\n"),
            Some("1.99.1")
        );
        assert_eq!(
            toolchain_channel("channel = \"1.99.1\"\r\n"),
            Some("1.99.1")
        );
        assert_eq!(toolchain_channel("# only a comment\n[toolchain]\n"), None);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinuxOperation {
    ReadFile {
        path: String,
    },
    ReadFileRange {
        path: String,
        offset: u64,
        length: u64,
    },
    ListDirectory {
        path: String,
    },
    WriteFile {
        path: String,
        content: String,
    },
    EditFile {
        path: String,
        old: String,
        new: String,
    },
    ExecStart {
        argv: Vec<String>,
    },
    ExecStatus {
        start_operation_id: String,
        start_payload_hash: String,
    },
    ExecCancel {
        start_operation_id: String,
        start_payload_hash: String,
    },
}

impl LinuxOperation {
    pub fn scope(&self) -> &'static str {
        match self {
            Self::ReadFile { .. } => READ_SCOPE,
            Self::ReadFileRange { .. } => RANGE_SCOPE,
            Self::ListDirectory { .. } => LIST_SCOPE,
            Self::WriteFile { .. } => WRITE_SCOPE,
            Self::EditFile { .. } => EDIT_SCOPE,
            Self::ExecStart { .. } => EXEC_START_SCOPE,
            Self::ExecStatus { .. } => EXEC_STATUS_SCOPE,
            Self::ExecCancel { .. } => EXEC_CANCEL_SCOPE,
        }
    }

    pub fn method(&self) -> &'static str {
        match self {
            Self::ReadFile { .. } => "read_file",
            Self::ReadFileRange { .. } => "read_file_range",
            Self::ListDirectory { .. } => "list_directory",
            Self::WriteFile { .. } => "write_file",
            Self::EditFile { .. } => "edit_file",
            Self::ExecStart { .. } => "exec_start",
            Self::ExecStatus { .. } => "exec_status",
            Self::ExecCancel { .. } => "exec_cancel",
        }
    }

    pub(crate) fn to_json(&self) -> Value {
        match self {
            Self::ReadFile { path } => json!({"method":"read_file","path":path}),
            Self::ReadFileRange {
                path,
                offset,
                length,
            } => {
                json!({"method":"read_file_range","path":path,"offset":offset,"length":length})
            }
            Self::ListDirectory { path } => json!({"method":"list_directory","path":path}),
            Self::WriteFile { path, content } => {
                json!({"method":"write_file","path":path,"content":content})
            }
            Self::EditFile { path, old, new } => {
                json!({"method":"edit_file","path":path,"old":old,"new":new})
            }
            Self::ExecStart { argv } => json!({"method":"exec_start","argv":argv}),
            Self::ExecStatus {
                start_operation_id,
                start_payload_hash,
            } => json!({
                "method":"exec_status","startOperationId":start_operation_id,
                "startPayloadHash":start_payload_hash
            }),
            Self::ExecCancel {
                start_operation_id,
                start_payload_hash,
            } => json!({
                "method":"exec_cancel","startOperationId":start_operation_id,
                "startPayloadHash":start_payload_hash
            }),
        }
    }
}

/// One confined operation. Status and cancel are their own op-v1 identities;
/// they reference a start operation instead of reusing its request.
#[derive(Clone, Debug)]
pub struct LinuxExecutionRequest {
    pub binding: RuntimeBinding,
    pub tool_task_id: String,
    pub effect_key: String,
    pub capability_scope: String,
    /// Absolute Unix milliseconds. A replay cannot extend it. The deadline
    /// must be at most 60 seconds ahead of the call that starts the effect.
    pub deadline_ms: i64,
    pub operation: LinuxOperation,
}

impl LinuxExecutionRequest {
    pub fn operation_id(&self) -> Result<String> {
        operation_id(
            &self.binding.attempt_id,
            &self.binding.session_id,
            &self.tool_task_id,
            &self.effect_key,
        )
    }

    pub fn payload_hash(&self) -> Result<String> {
        payload_hash(&json!({
            "version": 1,
            "operation": self.operation.to_json(),
            "scope": self.capability_scope,
            "deadlineMs": self.deadline_ms
        }))
    }

    pub(crate) fn validate(&self) -> Result<()> {
        if self.tool_task_id.is_empty()
            || self.effect_key.is_empty()
            || self.tool_task_id.len() > 1024
            || self.effect_key.len() > 1024
        {
            return Err(Error::guard("invalid execution task/effect identity"));
        }
        if self.capability_scope != self.operation.scope() {
            return Err(Error::guard(
                "linux execution capability scope does not match the operation",
            ));
        }
        match &self.operation {
            LinuxOperation::ReadFile { path } | LinuxOperation::ListDirectory { path } => {
                validate_workspace_path(
                    path,
                    matches!(self.operation, LinuxOperation::ListDirectory { .. }),
                )?;
            }
            LinuxOperation::ReadFileRange {
                path,
                offset,
                length,
            } => {
                validate_workspace_path(path, false)?;
                if *length == 0 || *length > MAX_FILE_BYTES as u64 {
                    return Err(Error::guard("range read length is empty or above 64 KiB"));
                }
                if offset.checked_add(*length).is_none() {
                    return Err(Error::guard("range read offset overflows"));
                }
            }
            LinuxOperation::WriteFile { path, content } => {
                validate_workspace_path(path, false)?;
                if content.len() > MAX_FILE_BYTES {
                    return Err(Error::guard("write content exceeds 64 KiB"));
                }
            }
            LinuxOperation::EditFile { path, old, new } => {
                validate_workspace_path(path, false)?;
                if old.is_empty() || old.len() > MAX_FILE_BYTES || new.len() > MAX_FILE_BYTES {
                    return Err(Error::guard("edit strings are empty or exceed 64 KiB"));
                }
            }
            LinuxOperation::ExecStart { argv } => validate_argv(argv)?,
            LinuxOperation::ExecStatus {
                start_operation_id,
                start_payload_hash,
            }
            | LinuxOperation::ExecCancel {
                start_operation_id,
                start_payload_hash,
            } => {
                if !start_operation_id.starts_with("op-v1:")
                    || start_payload_hash.len() < 8
                    || start_operation_id.len() > 128
                    || start_payload_hash.len() > 128
                {
                    return Err(Error::guard(
                        "status/cancel requires the original start identity",
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn validate_workspace_path(path: &str, allow_dot: bool) -> Result<()> {
    if path.is_empty()
        || path.len() > 4096
        || path.contains('\0')
        || path.starts_with('/')
        || path.starts_with('\\')
    {
        return Err(Error::guard("linux path must be workspace-relative"));
    }
    if allow_dot && path == "." {
        return Ok(());
    }
    if path
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == ".." || part == ".git")
    {
        return Err(Error::guard(
            "linux path contains an empty, traversal, or git administrative component",
        ));
    }
    Ok(())
}

fn validate_argv(argv: &[String]) -> Result<()> {
    if argv.is_empty() || argv.len() > 32 {
        return Err(Error::guard("exec argv must contain 1 to 32 arguments"));
    }
    if argv
        .iter()
        .any(|arg| arg.is_empty() || arg.len() > 4096 || arg.contains('\0'))
    {
        return Err(Error::guard(
            "exec argument is empty, oversized, or contains NUL",
        ));
    }
    Ok(())
}

/// Explicit test barrier. Production callers leave this unset. When the stage
/// matches, the host prints `AT-{stage}` and waits to be killed.
#[derive(Clone, Debug)]
pub struct LinuxExecutionConfig {
    pub cgroup_root: std::path::PathBuf,
    pub bubblewrap: std::path::PathBuf,
    pub helper: std::path::PathBuf,
    /// Prints `AT-{stage}` and waits to be killed. Production leaves this unset.
    pub crash_at: Option<String>,
    /// JSON body for the acceptance workload `["/helper", "probe"]`.
    pub probe_request: Option<serde_json::Value>,
    /// Test-only. Bubblewrap is invoked with an unknown flag and exits before
    /// it writes `child-pid`. Production leaves this false.
    pub fail_launch: bool,
}

#[derive(Clone, Debug)]
pub struct LinuxExecutionOutcome {
    pub operation_id: String,
    pub payload_hash: String,
    pub state: String,
    pub receipt: Option<Value>,
}

/// Read-only Linux evidence. `entered` stays true after a later uncertain
/// quarantine so a missing receipt is not mistaken for "never started".
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinuxExecutionView {
    pub operation_id: String,
    pub state: String,
    pub payload_hash: String,
    pub started_generation: u64,
    pub scope: String,
    pub method: String,
    pub entered: bool,
    /// None when inspecting a pre-v8 ledger that has no release evidence.
    pub released: Option<bool>,
    pub receipt_retained: bool,
    pub quiescence: String,
    /// An abandonment supersession exists. The state stays `uncertain`.
    pub superseded: bool,
}

impl LinuxExecutionView {
    pub fn to_json(&self) -> Value {
        json!({
            "operationId": self.operation_id,
            "state": self.state,
            "payloadHash": self.payload_hash,
            "startedGeneration": self.started_generation,
            "scope": self.scope,
            "method": self.method,
            "entered": self.entered,
            "released": self.released,
            "receiptRetained": self.receipt_retained,
            "quiescence": self.quiescence,
            "superseded": self.superseded,
            "simulated": false,
        })
    }
}

#[cfg(all(feature = "linux-execution", target_os = "linux"))]
pub(crate) mod cgroup;
#[cfg(all(feature = "linux-execution", target_os = "linux"))]
mod helper;
#[cfg(all(feature = "linux-execution", target_os = "linux"))]
pub use helper::helper_main;
#[cfg(all(feature = "linux-execution", target_os = "linux"))]
pub(crate) use helper::{VERIFIER_ETC_ALLOWLIST, present_etc_binds, unresolved_sandbox_cc};
