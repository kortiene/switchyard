//! Feature-gated S1 outbox adapter for the credential-free Pi probe.
//! Inspect first; never resume a recovered submission merely to learn its receipt.
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::pi_bridge::{adapter_fingerprint, safe_json};
use super::{PiBridge, RuntimeBinding, RuntimeReceipt, RuntimeRequest};
use crate::canonical::{canonicalize, payload_hash, sha256_hex};
use crate::error::{Error, Result};

pub(crate) struct PiRuntime {
    node: PathBuf,
    directory: PathBuf,
    bridge: Option<PiBridge>,
    pub(crate) needs_resume: bool,
    pub(crate) fixture: &'static str,
}

impl PiRuntime {
    pub(crate) fn new(ledger: &Path, node: &Path) -> Result<Self> {
        let node = node.canonicalize().map_err(Error::storage)?;
        let mut directory = ledger.as_os_str().to_os_string();
        directory.push(".pi-sessions");
        Ok(Self {
            node,
            directory: directory.into(),
            bridge: None,
            needs_resume: false,
            fixture: "answer",
        })
    }

    pub(crate) fn configuration(&self) -> Value {
        let pins: Value =
            serde_json::from_str(include_str!("../../../../runtime/pi-source/pins.json"))
                .expect("committed pins");
        json!({"adapter":"pi-probe-v1", "credentials":"none", "source":pins["commit"],
            "fingerprint":adapter_fingerprint(), "node":pins["node"]})
    }

    pub(crate) fn database(&self, binding: &RuntimeBinding) -> PathBuf {
        self.directory.join(format!(
            "{}.sqlite",
            sha256_hex(binding.session_id.as_bytes())
        ))
    }

    pub(crate) fn open(&mut self, binding: &RuntimeBinding, require_existing: bool) -> Result<()> {
        if let Some(bridge) = &self.bridge {
            if bridge.binding() != binding {
                return Err(Error::guard(
                    "Pi probe already bound to another session/generation",
                ));
            }
            return Ok(());
        }
        let database = self.database(binding);
        if require_existing && !database.is_file() {
            return Err(Error::guard(
                "original Pi storage is missing; recovery blocked",
            ));
        }
        let launcher = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/pi-source/test/bridge-child.ts");
        let mut bridge = PiBridge::spawn(
            &self.node,
            &launcher,
            &database,
            binding.clone(),
            self.fixture,
        )?;
        bridge.hello()?;
        let opened = bridge.call(
            "runtime.open",
            None,
            json!({"requireExisting":require_existing}),
            &mut no_effects,
        )?;
        if opened["opened"] != true || opened["scheduling"] != "paused" {
            return Err(Error::guard("Pi open did not return an inert session"));
        }
        self.bridge = Some(bridge);
        self.needs_resume = require_existing;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn inspect(&mut self, request: &RuntimeRequest) -> Result<Value> {
        self.call(
            request,
            "runtime.inspect",
            None,
            json!({"commandIds":[request.request_id]}),
        )
    }

    pub(crate) fn validate_command(request: &RuntimeRequest, payload: &Value) -> Result<()> {
        safe_json(payload)?;
        if payload_hash(payload)? != request.payload_hash
            || payload["missionId"] != request.binding.mission_id
            || payload["attemptId"] != request.binding.attempt_id
        {
            return Err(Error::guard("Pi command payload/hash/binding mismatch"));
        }
        if request.request_id.is_empty()
            || request.request_id.len() > 256
            || request.binding.generation == 0
            || request.binding.generation > 9_007_199_254_740_991
        {
            return Err(Error::guard("Pi command identity outside protocol bounds"));
        }
        let (method, params) = mapped(request, payload)?;
        // Reserve the maximum wire sequence width, so admission cannot create a
        // permanently oversized pending command. The bridge checks the real frame.
        let pins: Value =
            serde_json::from_str(include_str!("../../../../runtime/pi-source/pins.json"))
                .expect("committed pins");
        let frame = json!({"protocol":pins["bridge"]["protocol"],"kind":"request","id":"host:18446744073709551615",
            "method":method,"commandId":request.request_id,"params":params,
            "binding":{"missionId":request.binding.mission_id,"attemptId":request.binding.attempt_id,
                "sessionId":request.binding.session_id,"generation":9_007_199_254_740_991_u64}});
        if serde_json::to_vec(&frame).map_err(Error::storage)?.len() + 1
            > pins["bridge"]["maxFrame"].as_u64().expect("frame pin") as usize
        {
            return Err(Error::guard(
                "Pi command exceeds frame limit before admission",
            ));
        }
        Ok(())
    }

    fn call(
        &mut self,
        request: &RuntimeRequest,
        method: &str,
        command: Option<&str>,
        params: Value,
    ) -> Result<Value> {
        let bridge = self
            .bridge
            .as_mut()
            .ok_or_else(|| Error::guard("Pi session is not open"))?;
        if bridge.binding() != &request.binding {
            return Err(Error::guard("Pi request has stale/mismatched binding"));
        }
        bridge.call(method, command, params, &mut no_effects)
    }

    pub(crate) fn reconcile(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
    ) -> Result<Option<RuntimeReceipt>> {
        Self::validate_command(request, payload)?;
        let (method, params) = mapped(request, payload)?;
        let snapshot = self.call(
            request,
            "runtime.inspect",
            None,
            json!({"commandIds":[request.request_id]}),
        )?;
        let rows = snapshot["commands"]
            .as_array()
            .ok_or_else(|| Error::guard("missing Pi command snapshot"))?;
        if rows.len() != 1 || rows[0]["commandId"] != request.request_id {
            return Err(Error::guard("Pi inspection returned a different command"));
        }
        let record = &rows[0]["receipt"];
        if record.is_null() {
            return Ok(None);
        }
        if record["hash"] != payload_hash(&json!({"method":method,"params":params}))?
            || record["method"] != method
        {
            return Err(Error::CommandConflict {
                command_id: request.request_id.clone(),
            });
        }
        // An incomplete abort must be explicitly replayed to settle, never ACKed.
        if record["settled"] == false && method == "runtime.abort" {
            return Ok(None);
        }
        if record["settled"] != true {
            return Err(Error::guard("uncertain Pi command record"));
        }
        let receipt = record["receipt"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::guard("Pi record has no receipt"))?;
        if method != "runtime.abort" && !rows[0]["status"].is_string() {
            return Err(Error::guard(
                "Pi receipt has no corresponding durable submission",
            ));
        }
        Ok(Some(receipt_for(request, receipt)))
    }

    pub(crate) fn require_acknowledged(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
        recorded: Option<&str>,
    ) -> Result<RuntimeReceipt> {
        let original = self.reconcile(request, payload)?.ok_or_else(|| {
            Error::guard("acknowledged Pi submission is missing; recovery blocked")
        })?;
        let recorded: Value = serde_json::from_str(
            recorded.ok_or_else(|| Error::guard("acknowledged command has no Rust receipt"))?,
        )
        .map_err(Error::storage)?;
        if recorded["receiptId"] != original.receipt_id
            || recorded["requestId"] != original.request_id
            || recorded["payloadHash"] != original.payload_hash
            || recorded["adapter"] != "pi-probe-v1"
        {
            return Err(Error::guard(
                "Rust/Pi receipt disagreement; recovery blocked",
            ));
        }
        Ok(original)
    }

    pub(crate) fn dispatch(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
    ) -> Result<RuntimeReceipt> {
        Self::validate_command(request, payload)?;
        let (method, params) = mapped(request, payload)?;
        let wire_hash = payload_hash(&json!({"method":method,"params":params}))?;
        let result = self.call(request, method, Some(&request.request_id), params)?;
        if result["commandId"] != request.request_id {
            return Err(Error::guard("Pi receipt command mismatch"));
        }
        if method == "runtime.abort" {
            if result["accepted"] != true || result["runtimeSettled"] != true {
                return Err(Error::guard("Pi abort is not settled"));
            }
            // This receipt only acknowledges runtime abort, never OS quiescence.
            return Ok(receipt_for(request, &request.request_id));
        }
        if result["payloadHash"] != wire_hash
            || result["admitted"] != true
            || result["requestId"]
                != canonicalize(&json!([request.binding.attempt_id, request.request_id]))?
        {
            return Err(Error::guard("Pi admission identity mismatch"));
        }
        let receipt = result["receiptId"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| Error::guard("Pi admission has no receipt"))?;
        self.needs_resume = false;
        Ok(receipt_for(request, receipt))
    }
}

fn receipt_for(request: &RuntimeRequest, receipt: &str) -> RuntimeReceipt {
    RuntimeReceipt {
        receipt_id: receipt.into(),
        request_id: request.request_id.clone(),
        payload_hash: request.payload_hash.clone(),
    }
}

fn no_effects(_: &Value) -> Result<Value> {
    // No ExecutionEnv in S2-A. The bridge retains conservative uncertain-effect
    // handling; S3 supplies the actual operation ledger and execution boundary.
    Err(Error::guard(
        "workspace effects unavailable in the S2-A probe",
    ))
}

fn mapped(request: &RuntimeRequest, payload: &Value) -> Result<(&'static str, Value)> {
    let kind = payload["kind"]
        .as_str()
        .ok_or_else(|| Error::guard("missing command kind"))?;
    let mut params = match kind {
        "pause" | "cancel" if request.method == "abort" => json!({}),
        "continue" if request.method == "submit" => json!({"type":"continue"}),
        "steer" if request.method == "steer" => {
            let content = payload["body"]["content"]
                .as_str()
                .ok_or_else(|| Error::guard("missing steering content"))?;
            json!({"type":"input","content":content})
        }
        "start_planning"
        | "request_revised_plan"
        | "approve_plan"
        | "authorize_bounded_repair"
        | "request_changes"
            if request.method == "submit" =>
        {
            json!({"type":"input","content":canonicalize(payload)?})
        }
        _ => return Err(Error::guard("command has no S2-A runtime mapping")),
    };
    params["hostCommand"] = json!({"payload":payload,"payloadHash":request.payload_hash});
    let method = match request.method {
        "abort" => "runtime.abort",
        "steer" => "runtime.steer",
        _ => "runtime.submit",
    };
    Ok((method, params))
}
