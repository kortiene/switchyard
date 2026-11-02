//! S3-B2: the only Pi-to-Linux operation currently admitted is a file write.
//! The caller owns bridge confinement and supplies the authenticated binding
//! and absolute deadline. Neither is taken from model-controlled parameters.
use super::*;
use crate::PiEffectOutcome;
use crate::linux_execution::{
    BUBBLEWRAP_BACKEND, LinuxExecutionRequest, LinuxOperation, WRITE_SCOPE,
};

impl Host {
    /// Forward one validated Pi effect into the real Linux journal. This is a
    /// non-default integration API, not a product Pi runtime/launcher. Call only
    /// from a separately confined bridge's `call_with_effects` callback, passing
    /// that bridge's binding and a host-selected deadline (at most 60s away).
    /// Once intent exists, its stored deadline wins over a new caller deadline.
    pub fn execute_pi_linux_write(
        &mut self,
        binding: &RuntimeBinding,
        params: &Value,
        deadline_ms: i64,
    ) -> PiEffectOutcome {
        self.pi_linux_write(binding, params, deadline_ms)
            .unwrap_or(PiEffectOutcome::Uncertain)
    }

    fn pi_linux_write(
        &mut self,
        binding: &RuntimeBinding,
        params: &Value,
        deadline_ms: i64,
    ) -> Result<PiEffectOutcome> {
        if let Err(error) = self.accept_runtime_binding(binding, &binding.mission_id) {
            return match error {
                Error::BindingMismatch { .. } | Error::StaleGeneration { .. } => {
                    Ok(PiEffectOutcome::Rejected)
                }
                _ => Err(error),
            };
        }
        let mut request = match parse_write(binding, params, deadline_ms) {
            Ok(request) => request,
            Err(_) => return Ok(PiEffectOutcome::Rejected),
        };
        let id = request.operation_id()?;
        if let Some(effect) = self.store.linux_effect(&id)? {
            if effect.mission_id != binding.mission_id
                || effect.attempt_id != binding.attempt_id
                || effect.session_id != binding.session_id
            {
                return Ok(PiEffectOutcome::Rejected);
            }
            // The existing Linux intent already freezes the deadline. Reading
            // it avoids a second mapping journal or any cross-store atomicity
            // assumption. Never derive a fresh deadline on replay.
            let stored: Value =
                serde_json::from_str(&effect.request_json).map_err(Error::storage)?;
            request.deadline_ms = stored["deadlineMs"]
                .as_i64()
                .ok_or_else(|| Error::guard("Linux intent has no frozen deadline"))?;
            if effect.payload_hash != request.payload_hash()? {
                return Ok(PiEffectOutcome::Conflict);
            }
        } else {
            if let Some(effect) = self.store.execution_effect(&id)? {
                return Ok(
                    if effect.mission_id == binding.mission_id
                        && effect.attempt_id == binding.attempt_id
                        && effect.session_id == binding.session_id
                    {
                        PiEffectOutcome::Conflict
                    } else {
                        PiEffectOutcome::Rejected
                    },
                );
            }
            if self.linux.is_none() || self.runtime.fake_mut().is_none() {
                return Ok(PiEffectOutcome::Rejected);
            }
            // This check precedes entry into the backend, so semantic refusal
            // proves this invocation admitted no work. Failed evidence reads
            // remain uncertain. The backend repeats authorization at admission.
            if let Err(error) = self.authorize_new(&request) {
                return if error.is_transient() {
                    Err(error)
                } else {
                    Ok(PiEffectOutcome::Rejected)
                };
            }
        }
        // After entering the backend, a generic error is never a rejection.
        // Only this backend's durable evidence may settle the outcome.
        let _result = self.execute_linux_operation(&request);
        self.pi_linux_write_outcome(&request)
    }

    fn pi_linux_write_outcome(&self, request: &LinuxExecutionRequest) -> Result<PiEffectOutcome> {
        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
        let id = request.operation_id()?;
        let hash = request.payload_hash()?;
        let Some(effect) = self.store.linux_effect(&id)? else {
            // Absence after backend entry is not evidence of definite refusal.
            return Ok(PiEffectOutcome::Uncertain);
        };
        if effect.mission_id != request.binding.mission_id
            || effect.attempt_id != request.binding.attempt_id
            || effect.session_id != request.binding.session_id
        {
            return Ok(PiEffectOutcome::Rejected);
        }
        if effect.payload_hash != hash {
            return Ok(PiEffectOutcome::Conflict);
        }
        if self.store.operation_superseded(&id)? {
            return Ok(PiEffectOutcome::Uncertain);
        }
        match effect.state.as_str() {
            "completed" => {
                let receipt: Value = serde_json::from_str(
                    effect
                        .receipt_json
                        .as_deref()
                        .ok_or_else(|| Error::guard("missing Linux receipt"))?,
                )
                .map_err(Error::storage)?;
                if receipt["operationId"] != id
                    || receipt["payloadHash"] != hash
                    || receipt["backend"] != BUBBLEWRAP_BACKEND
                    || receipt["simulated"] != false
                {
                    return Ok(PiEffectOutcome::Uncertain);
                }
                Ok(PiEffectOutcome::Completed(receipt))
            }
            "not_applied" => Ok(PiEffectOutcome::Rejected),
            _ => Ok(PiEffectOutcome::Uncertain),
        }
    }
}

fn keys(value: &Value, expected: &[&str]) -> Result<()> {
    let object = value
        .as_object()
        .ok_or_else(|| Error::guard("expected an object"))?;
    if object.len() != expected.len() || expected.iter().any(|key| !object.contains_key(*key)) {
        return Err(Error::guard("unexpected Pi write fields"));
    }
    Ok(())
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| Error::guard("expected a Pi write string"))
}

fn parse_write(
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
    keys(
        payload,
        &["method", "path", "content", "capability", "deadlinePolicy"],
    )?;
    if payload["method"] != "file.write"
        || payload["capability"] != "workspace-write"
        || payload["deadlinePolicy"] != "attempt-bound"
    {
        return Err(Error::guard("unsupported Pi Linux operation or policy"));
    }
    let request = LinuxExecutionRequest {
        binding: binding.clone(),
        tool_task_id: text(params, "taskId")?.into(),
        effect_key: text(params, "effectKey")?.into(),
        capability_scope: WRITE_SCOPE.into(),
        deadline_ms,
        operation: LinuxOperation::WriteFile {
            path: text(payload, "path")?.into(),
            content: text(payload, "content")?.into(),
        },
    };
    request.validate()?;
    if text(params, "operationId")? != request.operation_id()?
        || text(params, "payloadHash")? != payload_hash(payload)?
    {
        return Err(Error::guard("Pi write identity or payload hash mismatch"));
    }
    Ok(request)
}

#[cfg(test)]
mod tests;
