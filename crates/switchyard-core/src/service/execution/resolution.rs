use super::*;
use crate::{ExecutionResolutionDecision, ExecutionResolutionRequest};

impl Host {
    /// Record an operator decision using Host-owned simulation evidence only.
    /// This never calls the backend, dispatches work, or changes mission phase.
    pub fn resolve_fake_execution(
        &mut self,
        request: &ExecutionResolutionRequest,
    ) -> Result<Value> {
        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
        if self.runtime.fake_mut().is_none() {
            return Err(Error::guard("execution resolution requires FakeRuntime"));
        }
        for (value, limit) in [
            (&request.command_id, 1024),
            (&request.actor, 1024),
            (&request.rationale, 8192),
        ] {
            if value.trim().is_empty() || value.len() > limit {
                return Err(Error::guard(
                    "resolution requires bounded nonempty command ID, actor and rationale",
                ));
            }
        }
        if let Some(audit) = self.store.replay_execution_resolution(request)? {
            return Ok(audit);
        }
        let mission = self.store.load_mission(&request.binding.mission_id)?;
        if mission.phase.is_terminal() {
            return Err(Error::TerminalImmutable {
                phase: mission.phase.to_string(),
            });
        }
        if request.expected_revision != mission.revision {
            return Err(Error::StaleRevision {
                expected: request.expected_revision,
                actual: mission.revision,
            });
        }
        if mission.phase != Phase::NeedsAttention {
            return Err(Error::guard(
                "inspect and reconcile the mission into needs_attention before resolving an effect",
            ));
        }
        if self.store.linux_effect(&request.operation_id)?.is_some() {
            return Err(Error::guard(
                "this command only records simulated abandonment; a linux effect stays blocked until a host with the linux backend resolves it from durable cgroup evidence",
            ));
        }
        let effect = self
            .store
            .execution_effect(&request.operation_id)?
            .ok_or(Error::NotFound)?;
        if effect.mission_id != request.binding.mission_id
            || effect.attempt_id != request.binding.attempt_id
            || effect.session_id != request.binding.session_id
        {
            return Err(Error::BindingMismatch {
                detail: "resolution does not match effect ownership".into(),
            });
        }
        if effect.payload_hash != request.payload_hash {
            return Err(Error::OperationConflict {
                operation_id: request.operation_id.clone(),
            });
        }
        if self
            .store
            .execution_resolution_for(&request.operation_id)?
            .is_some()
        {
            return Err(Error::guard(
                "effect already has an immutable operator decision; replay its command ID",
            ));
        }
        let receipt = match (
            request.decision,
            self.execution_evidence.get(&request.operation_id),
        ) {
            (ExecutionResolutionDecision::Completed, Some(Some(receipt)))
                if receipt.operation_id == request.operation_id
                    && receipt.payload_hash == request.payload_hash =>
            {
                Some(receipt.clone())
            }
            (ExecutionResolutionDecision::NotApplied, Some(None)) => None,
            (ExecutionResolutionDecision::Abandon, None) => None,
            (ExecutionResolutionDecision::Abandon, Some(_)) => {
                return Err(Error::guard(
                    "Host-observed evidence is available; resolve the proven outcome instead of abandoning it",
                ));
            }
            _ => {
                return Err(Error::guard(
                    "matching Host-observed evidence is unavailable; outcome remains uncertain",
                ));
            }
        };
        let audit = self.store.resolve_execution(request, receipt.as_ref())?;
        self.execution_evidence.remove(&request.operation_id);
        Ok(audit)
    }
}
