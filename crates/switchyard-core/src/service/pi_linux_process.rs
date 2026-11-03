//! S3-B3 process requests. A start handle is not a process exit receipt.
use super::pi_linux_write::{keys, text};
use super::*;
use crate::PiEffectOutcome;
use crate::linux_execution::{LinuxExecutionRequest, LinuxOperation};

impl Host {
    /// Non-default, credential-free integration seam, with the same trusted
    /// caller/binding/deadline requirements as `execute_pi_linux_write`.
    /// Start returns a deterministic handle to durable intent; status/cancel
    /// return their original durable observation/termination receipts. A fresh
    /// status needs a fresh effect identity; replay is a historical observation.
    pub fn execute_pi_linux_process(
        &mut self,
        binding: &RuntimeBinding,
        params: &Value,
        deadline_ms: i64,
    ) -> PiEffectOutcome {
        self.pi_linux_effect(binding, parse_process(binding, params, deadline_ms))
            .unwrap_or(PiEffectOutcome::Uncertain)
    }
}

fn parse_process(
    binding: &RuntimeBinding,
    params: &Value,
    deadline_ms: i64,
) -> Result<LinuxExecutionRequest> {
    keys(
        params,
        &[
            "operationId",
            "taskId",
            "effectKey",
            "payloadHash",
            "payload",
        ],
    )?;
    let payload = &params["payload"];
    if payload["capability"] != "workspace-process" || payload["deadlinePolicy"] != "attempt-bound"
    {
        return Err(Error::guard("unsupported Pi process policy"));
    }
    let operation = match text(payload, "method")? {
        "process.start" => {
            keys(payload, &["method", "argv", "capability", "deadlinePolicy"])?;
            let argv = payload["argv"]
                .as_array()
                .ok_or_else(|| Error::guard("process argv must be an array"))?
                .iter()
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .ok_or_else(|| Error::guard("process argv must contain strings"))
                })
                .collect::<Result<Vec<_>>>()?;
            LinuxOperation::ExecStart { argv }
        }
        method @ ("process.status" | "process.cancel") => {
            keys(
                payload,
                &[
                    "method",
                    "startOperationId",
                    "startPayloadHash",
                    "capability",
                    "deadlinePolicy",
                ],
            )?;
            let start_operation_id = text(payload, "startOperationId")?.to_owned();
            let start_payload_hash = text(payload, "startPayloadHash")?.to_owned();
            if method == "process.status" {
                LinuxOperation::ExecStatus {
                    start_operation_id,
                    start_payload_hash,
                }
            } else {
                LinuxOperation::ExecCancel {
                    start_operation_id,
                    start_payload_hash,
                }
            }
        }
        _ => return Err(Error::guard("unsupported Pi process method")),
    };
    let request = LinuxExecutionRequest {
        binding: binding.clone(),
        tool_task_id: text(params, "taskId")?.into(),
        effect_key: text(params, "effectKey")?.into(),
        capability_scope: operation.scope().into(),
        deadline_ms,
        operation,
    };
    request.validate()?;
    if text(params, "operationId")? != request.operation_id()?
        || text(params, "payloadHash")? != payload_hash(payload)?
    {
        return Err(Error::guard("Pi process identity or payload hash mismatch"));
    }
    Ok(request)
}

#[cfg(test)]
mod tests;
