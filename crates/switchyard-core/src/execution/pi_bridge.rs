//! Credential-free S2 process transport. No product CLI or execution backend enables it.
use std::collections::VecDeque;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use serde_json::{Value, json};

use super::{OwnerLock, RuntimeBinding, operation_id, try_acquire};
use crate::canonical::{canonicalize, payload_hash};
use crate::error::{Error, Result};

const STDERR_LIMIT: usize = 4096;

struct Pins {
    commit: String,
    node: String,
    bridge: ProtocolPins,
}

struct ProtocolPins {
    protocol: u64,
    max_frame: usize,
    max_pending: usize,
    deadline_ms: u64,
    response_timeout_ms: u64,
    max_effects_per_call: usize,
    capabilities: Vec<String>,
}

fn pins() -> &'static Pins {
    static PINS: OnceLock<Pins> = OnceLock::new();
    PINS.get_or_init(|| {
        let value: Value =
            serde_json::from_str(include_str!("../../../../runtime/pi-source/pins.json"))
                .expect("valid committed bridge pins");
        let bridge = &value["bridge"];
        let number = |key: &str| bridge[key].as_u64().expect("integer bridge pin");
        Pins {
            commit: value["commit"].as_str().expect("source pin").into(),
            node: value["node"].as_str().expect("Node pin").into(),
            bridge: ProtocolPins {
                protocol: number("protocol"),
                max_frame: number("maxFrame").try_into().expect("frame limit"),
                max_pending: number("maxPending").try_into().expect("pending limit"),
                deadline_ms: number("deadlineMs"),
                response_timeout_ms: number("responseTimeoutMs"),
                max_effects_per_call: number("maxEffectsPerCall")
                    .try_into()
                    .expect("effect limit"),
                capabilities: serde_json::from_value(bridge["capabilities"].clone())
                    .expect("capabilities"),
            },
        }
    })
}

pub fn adapter_fingerprint() -> String {
    payload_hash(&json!([
        include_str!("../../../../runtime/pi-source/src/protocol.ts"),
        include_str!("../../../../runtime/pi-source/src/transport.ts"),
        include_str!("../../../../runtime/pi-source/src/runtime.ts"),
        include_str!("../../../../runtime/pi-source/src/forwarders.ts"),
        include_str!("../../../../runtime/pi-source/test/bridge-child.ts"),
        include_str!("../../../../runtime/pi-source/pins.json"),
        include_str!("../../../../runtime/pi-source/package-lock.json")
    ]))
    .expect("fingerprint contains only strings")
}

pub struct PiBridge {
    child: Child,
    writer: SyncSender<Vec<u8>>,
    reader: Receiver<std::result::Result<Value, String>>,
    binding: RuntimeBinding,
    next_id: u64,
    last_effect_id: u64,
    deadline: Duration,
    stderr: Arc<Mutex<VecDeque<u8>>>,
    stderr_done: Receiver<()>,
    _owner: OwnerLock,
}

impl PiBridge {
    /// Spawn the trusted conformance launcher with an empty environment and a
    /// canonical storage lock. S3 must add OS confinement before live use.
    pub fn spawn(
        node: &Path,
        launcher: &Path,
        database: &Path,
        binding: RuntimeBinding,
        fixture: &str,
    ) -> Result<Self> {
        if !matches!(
            fixture,
            "answer" | "tools" | "recover" | "crash-before-admission" | "crash-after-admission"
        ) || binding.generation == 0
            || binding.generation > 9_007_199_254_740_991
            || [
                &binding.mission_id,
                &binding.attempt_id,
                &binding.session_id,
            ]
            .iter()
            .any(|s| s.is_empty() || s.len() > 256)
        {
            return Err(Error::guard("invalid bridge configuration"));
        }
        let parent = database
            .parent()
            .ok_or_else(|| Error::guard("missing storage parent"))?;
        fs::create_dir_all(parent).map_err(Error::storage)?;
        let database = if database.exists() {
            database.canonicalize().map_err(Error::storage)?
        } else {
            parent.canonicalize().map_err(Error::storage)?.join(
                database
                    .file_name()
                    .ok_or_else(|| Error::guard("missing storage name"))?,
            )
        };
        let owner = try_acquire(&database)?;
        let mut child = Command::new(node)
            .arg("--experimental-strip-types")
            .arg(launcher)
            .arg(&database)
            .arg(binding_json(&binding).to_string())
            .arg(fixture)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::storage)?;
        let mut input = child.stdin.take().expect("piped stdin");
        let output = child.stdout.take().expect("piped stdout");
        let mut diagnostics = child.stderr.take().expect("piped stderr");
        let max_frame = pins().bridge.max_frame;
        let (writer, writes) = sync_channel::<Vec<u8>>(pins().bridge.max_pending);
        std::thread::spawn(move || {
            while let Ok(bytes) = writes.recv() {
                if input
                    .write_all(&bytes)
                    .and_then(|()| input.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        let (frames, reader) = sync_channel(pins().bridge.max_pending);
        std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let result = (&mut output)
                    .take((max_frame + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                let frame = match result {
                    Ok(n) if n > 0 && n <= max_frame && bytes.last() == Some(&b'\n') => {
                        serde_json::from_slice::<Value>(&bytes)
                            .map_err(|_| "malformed bridge frame".to_string())
                    }
                    _ => Err("interrupted or oversized bridge frame".to_string()),
                };
                let failed = frame.is_err();
                if frames.send(frame).is_err() || failed {
                    break;
                }
            }
        });
        let stderr = Arc::new(Mutex::new(VecDeque::with_capacity(STDERR_LIMIT)));
        let tail = Arc::clone(&stderr);
        let (finished, stderr_done) = sync_channel(1);
        std::thread::spawn(move || {
            let mut bytes = [0_u8; STDERR_LIMIT];
            while let Ok(n) = diagnostics.read(&mut bytes) {
                if n == 0 {
                    break;
                }
                let mut tail = tail.lock().expect("stderr mutex");
                let overflow = (tail.len() + n).saturating_sub(STDERR_LIMIT);
                tail.drain(..overflow);
                tail.extend(&bytes[..n]);
            }
            let _ = finished.send(());
        });
        Ok(Self {
            child,
            writer,
            reader,
            binding,
            next_id: 0,
            last_effect_id: 0,
            deadline: Duration::from_millis(pins().bridge.response_timeout_ms),
            stderr,
            stderr_done,
            _owner: owner,
        })
    }

    pub fn binding(&self) -> &RuntimeBinding {
        &self.binding
    }

    pub fn process_id(&self) -> u32 {
        self.child.id()
    }

    pub fn hello(&mut self) -> Result<Value> {
        let result = self.call(
            "runtime.hello",
            None,
            json!({"source": pins().commit, "adapter": adapter_fingerprint(), "maxFrame": pins().bridge.max_frame,
            "maxPending": pins().bridge.max_pending, "capabilities": pins().bridge.capabilities}),
            &mut |_| Err(Error::guard("unexpected effect")),
        )?;
        if result["source"] != pins().commit
            || result["adapter"] != adapter_fingerprint()
            || result["node"] != pins().node
            || result["protocol"] != pins().bridge.protocol
            || result["maxFrame"] != pins().bridge.max_frame
            || result["maxPending"] != pins().bridge.max_pending
            || result["storage"] != "sqlite-full"
            || result["deadlineMs"] != pins().bridge.deadline_ms
            || result["capabilities"] != json!(pins().bridge.capabilities)
        {
            return Err(self.failure(Error::guard("incompatible bridge handshake")));
        }
        Ok(result)
    }

    /// Calls never replay after a transport failure. The owner reconciles the
    /// original durable command/operation identity before deciding what to send.
    /// The response timeout restarts after each effect receipt; trusted callback
    /// execution time is excluded. The peer retains its separate effect timeout.
    pub fn call(
        &mut self,
        method: &str,
        command_id: Option<&str>,
        params: Value,
        effect: &mut dyn FnMut(&Value) -> Result<Value>,
    ) -> Result<Value> {
        self.call_inner(method, command_id, params, effect)
            .map_err(|error| self.failure(error))
    }

    fn call_inner(
        &mut self,
        method: &str,
        command_id: Option<&str>,
        params: Value,
        effect: &mut dyn FnMut(&Value) -> Result<Value>,
    ) -> Result<Value> {
        safe_json(&params)?;
        self.next_id += 1;
        let id = format!("host:{}", self.next_id);
        let mut frame = json!({"protocol": pins().bridge.protocol, "kind": "request", "id": id, "method": method,
            "binding": binding_json(&self.binding), "params": params});
        if let Some(command_id) = command_id {
            frame["commandId"] = json!(command_id);
        }
        self.send(&frame)?;
        let mut limit = std::time::Instant::now() + self.deadline;
        let mut effects = 0;
        loop {
            let remaining = limit.saturating_duration_since(std::time::Instant::now());
            let frame = self
                .reader
                .recv_timeout(remaining)
                .map_err(|_| Error::guard("bridge interrupted or deadline exceeded"))?
                .map_err(Error::guard)?;
            if frame["protocol"] != pins().bridge.protocol
                || frame["binding"] != binding_json(&self.binding)
            {
                return Err(Error::guard("invalid bridge protocol or binding"));
            }
            if frame["kind"] == "request" {
                effects += 1;
                if effects > pins().bridge.max_effects_per_call {
                    return Err(Error::guard("bridge effect limit exceeded"));
                }
                let wire_id = text(&frame, "id")?;
                let sequence = wire_id
                    .strip_prefix("bridge:")
                    .and_then(|s| s.parse::<u64>().ok())
                    .ok_or_else(|| Error::guard("invalid effect wire id"))?;
                if sequence <= self.last_effect_id || frame["method"] != "execution.request" {
                    return Err(Error::guard("unknown bridge request"));
                }
                self.last_effect_id = sequence;
                let params = &frame["params"];
                safe_json(params)?;
                let task = text(params, "taskId")?;
                let key = text(params, "effectKey")?;
                if text(params, "operationId")?
                    != operation_id(
                        &self.binding.attempt_id,
                        &self.binding.session_id,
                        task,
                        key,
                    )?
                    || text(params, "payloadHash")? != payload_hash(&params["payload"])?
                {
                    return Err(Error::guard("invalid effect identity"));
                }
                let reply = match effect(params) {
                    Ok(result) => json!({"result": result}),
                    Err(_) => {
                        // TODO(#96, s2-bridge-probe.md "Remaining S2 acceptance"):
                        // replace this generic callback result with typed definite
                        // rejection/conflict vs uncertain execution receipts. An S1
                        // Error alone does not prove whether an effect was admitted.
                        json!({"error": {"category": "execution_uncertain", "admission": "uncertain"}})
                    }
                };
                let mut response = json!({"protocol": pins().bridge.protocol, "kind": "response", "id": wire_id,
                    "binding": binding_json(&self.binding)});
                response
                    .as_object_mut()
                    .expect("object")
                    .extend(reply.as_object().expect("object").clone());
                self.send(&response)?;
                // A committed effect receipt must not consume the runtime's next
                // reply window. This does not extend the effect's own deadline.
                limit = std::time::Instant::now() + self.deadline;
                continue;
            }
            if frame["kind"] != "response"
                || frame["id"] != id
                || frame.get("result").is_some() == frame.get("error").is_some()
            {
                return Err(Error::guard("unexpected bridge response"));
            }
            if let Some(error) = frame.get("error") {
                return Err(Error::guard(format!(
                    "bridge: {}",
                    text(error, "category")?
                )));
            }
            let result = frame["result"].clone();
            safe_json(&result)?;
            // Runtime frames are never passed to S1's fake-only observation parser.
            reject_authority(&result)?;
            return Ok(result);
        }
    }

    fn send(&self, frame: &Value) -> Result<()> {
        let mut bytes = serde_json::to_vec(frame).map_err(Error::storage)?;
        bytes.push(b'\n');
        if bytes.len() > pins().bridge.max_frame {
            return Err(Error::guard("oversized host frame"));
        }
        self.writer
            .try_send(bytes)
            .map_err(|_| Error::guard("bridge write queue unavailable"))
    }

    pub fn terminate(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    /// Exit status once the peer has been reaped. `Ok(None)` while it is still running.
    pub fn exit_status(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    /// Untrusted fixture diagnostics, capped at 4 KiB with terminal controls escaped.
    pub fn stderr_tail(&self) -> String {
        let bytes: Vec<u8> = self
            .stderr
            .lock()
            .expect("stderr mutex")
            .iter()
            .copied()
            .collect();
        let rendered: String = String::from_utf8_lossy(&bytes)
            .chars()
            .map(|c| {
                if c.is_control() && c != '\n' && c != '\t' {
                    c.escape_default().to_string()
                } else {
                    c.to_string()
                }
            })
            .collect();
        let mut start = rendered.len().saturating_sub(STDERR_LIMIT);
        while !rendered.is_char_boundary(start) {
            start += 1;
        }
        rendered[start..].to_string()
    }

    fn failure(&mut self, mut error: Error) -> Error {
        // Retain storage and fence the peer. Descendants holding stderr cannot
        // make diagnostic collection wait indefinitely; S3 owns process trees.
        self.terminate();
        let _ = self.stderr_done.recv_timeout(Duration::from_millis(100));
        let tail = self.stderr_tail();
        if !tail.is_empty() {
            match &mut error {
                Error::GuardFailed { detail }
                | Error::Storage { detail }
                | Error::Canonical { detail } => {
                    detail.push_str("\nbridge stderr (last 4 KiB, untrusted):\n");
                    detail.push_str(&tail);
                }
                _ => {}
            }
        }
        error
    }
}

impl Drop for PiBridge {
    fn drop(&mut self) {
        self.terminate();
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::guard(format!("missing bridge {key}")))
}

fn binding_json(binding: &RuntimeBinding) -> Value {
    json!({"missionId": binding.mission_id, "attemptId": binding.attempt_id,
        "sessionId": binding.session_id, "generation": binding.generation})
}

pub(super) fn safe_json(value: &Value) -> Result<()> {
    match value {
        Value::Number(n) => {
            let signed = n
                .as_i64()
                .is_some_and(|v| (-9_007_199_254_740_991..=9_007_199_254_740_991).contains(&v));
            if !signed {
                return Err(Error::guard("bridge requires safe integer JSON"));
            }
        }
        Value::Array(a) => {
            for v in a {
                safe_json(v)?;
            }
        }
        Value::Object(o) => {
            for v in o.values() {
                safe_json(v)?;
            }
        }
        _ => {}
    }
    canonicalize(value)?;
    Ok(())
}

fn reject_authority(value: &Value) -> Result<()> {
    if let Some(fields) = value.as_object() {
        for key in [
            "verified",
            "approved",
            "published",
            "quiescent",
            "missionStatus",
            "verification",
            "publication",
        ] {
            if fields.contains_key(key) {
                return Err(Error::guard("runtime cannot supply mission authority"));
            }
        }
        for nested in fields.values() {
            reject_authority(nested)?;
        }
    } else if let Some(items) = value.as_array() {
        for nested in items {
            reject_authority(nested)?;
        }
    }
    Ok(())
}
