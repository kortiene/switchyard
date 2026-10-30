//! Credential-free S2 process transport. No product CLI or execution backend enables it.
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::time::Duration;

use serde_json::{Value, json};

use super::{OwnerLock, RuntimeBinding, operation_id, try_acquire};
use crate::canonical::{canonicalize, payload_hash};
use crate::error::{Error, Result};

const MAX_FRAME: usize = 65_536;
const MAX_PENDING: usize = 16;
const SOURCE: &str = "e65b3b88c08412bb8ffbd21fb5dc5756064a6e5a";

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
        if !matches!(fixture, "answer" | "tools" | "recover")
            || binding.generation == 0
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
        let (writer, writes) = sync_channel::<Vec<u8>>(MAX_PENDING);
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
        let (frames, reader) = sync_channel(MAX_PENDING);
        std::thread::spawn(move || {
            let mut output = BufReader::new(output);
            loop {
                let mut bytes = Vec::new();
                let result = (&mut output)
                    .take((MAX_FRAME + 1) as u64)
                    .read_until(b'\n', &mut bytes);
                let frame = match result {
                    Ok(n) if n > 0 && n <= MAX_FRAME && bytes.last() == Some(&b'\n') => {
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
        std::thread::spawn(move || {
            let mut bytes = [0_u8; 4096];
            while matches!(diagnostics.read(&mut bytes), Ok(n) if n > 0) {}
        });
        Ok(Self {
            child,
            writer,
            reader,
            binding,
            next_id: 0,
            last_effect_id: 0,
            deadline: Duration::from_secs(8),
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
            json!({"source": SOURCE, "adapter": adapter_fingerprint(), "maxFrame": MAX_FRAME,
            "maxPending": MAX_PENDING, "capabilities": ["durable-admission", "payload-conflicts",
            "snapshot-resync", "cooperative-abort", "forwarded-effects"]}),
            &mut |_| Err(Error::guard("unexpected effect")),
        )?;
        if result["source"] != SOURCE
            || result["adapter"] != adapter_fingerprint()
            || result["node"] != "22.23.4"
            || result["protocol"] != 0
            || result["maxFrame"] != MAX_FRAME
            || result["maxPending"] != MAX_PENDING
            || result["storage"] != "sqlite-full"
            || result["deadlineMs"] != 5000
            || result["capabilities"]
                != json!([
                    "durable-admission",
                    "payload-conflicts",
                    "snapshot-resync",
                    "cooperative-abort",
                    "forwarded-effects"
                ])
        {
            self.terminate();
            return Err(Error::guard("incompatible bridge handshake"));
        }
        Ok(result)
    }

    /// Calls never replay after a transport failure. The owner reconciles the
    /// original durable command/operation identity before deciding what to send.
    pub fn call(
        &mut self,
        method: &str,
        command_id: Option<&str>,
        params: Value,
        effect: &mut dyn FnMut(&Value) -> Result<Value>,
    ) -> Result<Value> {
        let outcome = self.call_inner(method, command_id, params, effect);
        if outcome.is_err() {
            // A peer error is conservatively fenced too. Retain its storage for inspection.
            self.terminate();
        }
        outcome
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
        let mut frame = json!({"protocol": 0, "kind": "request", "id": id, "method": method,
            "binding": binding_json(&self.binding), "params": params});
        if let Some(command_id) = command_id {
            frame["commandId"] = json!(command_id);
        }
        self.send(&frame)?;
        let limit = std::time::Instant::now() + self.deadline;
        loop {
            let remaining = limit.saturating_duration_since(std::time::Instant::now());
            let frame = self
                .reader
                .recv_timeout(remaining)
                .map_err(|_| Error::guard("bridge interrupted or deadline exceeded"))?
                .map_err(Error::guard)?;
            if frame["protocol"] != 0 || frame["binding"] != binding_json(&self.binding) {
                return Err(Error::guard("invalid bridge protocol or binding"));
            }
            if frame["kind"] == "request" {
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
                        json!({"error": {"category": "execution_uncertain", "admission": "uncertain"}})
                    }
                };
                let mut response = json!({"protocol": 0, "kind": "response", "id": wire_id,
                    "binding": binding_json(&self.binding)});
                response
                    .as_object_mut()
                    .expect("object")
                    .extend(reply.as_object().expect("object").clone());
                self.send(&response)?;
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
        if bytes.len() > MAX_FRAME {
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

fn safe_json(value: &Value) -> Result<()> {
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
    }
    Ok(())
}
