use super::*;
mod evidence;
mod resolution;
mod supersede;
use crate::{ExecutionReceipt, ExecutionRequest};
pub(super) use evidence::ExecutionEvidence;

impl Host {
    pub(super) fn reconcile_execution(&mut self, mission_id: &str) -> Result<()> {
        self.store.assert_workspace_owner()?;
        if self.store.execution_blocked(mission_id)? {
            self.store.quarantine_pending_execution(mission_id)?;
            let mission = self.store.load_mission(mission_id)?;
            // Preserve an admitted stop while retaining every execution block.
            // A delivered abort cannot authorize ConfirmStopped/ConfirmCancelled.
            let stopping_fake = matches!(self.runtime, RuntimeAdapter::Fake(_))
                && matches!(mission.phase, Phase::Pausing | Phase::Cancelling);
            if !mission.phase.is_terminal() && !stopping_fake {
                self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
            }
        }
        self.reconcile_linux(mission_id)?;
        Ok(())
    }

    /// Simulate one authorized effect. No real file/process backend or Pi wiring.
    /// An interrupted prepared effect is quarantined, never automatically repeated.
    pub fn execute_fake_operation(
        &mut self,
        request: &ExecutionRequest,
    ) -> Result<ExecutionReceipt> {
        self.execute_fake_at(request, |_| Ok(()))
    }

    /// S2 probe adapter for the existing simulated execution journal. The
    /// product runtime and Linux backend are not wired to Pi by this method.
    /// A generic error is never by itself evidence of non-admission.
    #[cfg(feature = "pi-contract")]
    pub fn execute_fake_operation_outcome(
        &mut self,
        request: &ExecutionRequest,
    ) -> crate::PiEffectOutcome {
        use crate::PiEffectOutcome;
        if let Err(error) =
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)
        {
            return match error {
                Error::BindingMismatch { .. } | Error::StaleGeneration { .. } => {
                    PiEffectOutcome::Rejected
                }
                _ => PiEffectOutcome::Uncertain,
            };
        }
        if self.runtime.fake_mut().is_none() {
            return PiEffectOutcome::Rejected;
        }
        let result = self.execute_fake_operation(request);
        self.fake_effect_outcome(request, result)
            .unwrap_or(PiEffectOutcome::Uncertain)
    }

    #[cfg(feature = "pi-contract")]
    fn fake_effect_outcome(
        &self,
        request: &ExecutionRequest,
        result: Result<ExecutionReceipt>,
    ) -> Result<crate::PiEffectOutcome> {
        use crate::PiEffectOutcome;
        // A stale owner cannot read receipts or turn a post-effect error into
        // a definite rejection. Failed evidence reads also remain uncertain.
        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
        let id = request.operation_id()?;
        let hash = request.payload_hash()?;
        if let Some(effect) = self.store.execution_effect(&id)? {
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
            if effect.state == "completed" {
                return Ok(effect
                    .receipt
                    .map(|r| PiEffectOutcome::Completed(r.to_json()))
                    .unwrap_or(PiEffectOutcome::Uncertain));
            }
            return Ok(if effect.state == "not_applied" {
                PiEffectOutcome::Rejected
            } else {
                PiEffectOutcome::Uncertain
            });
        }
        if self.store.linux_effect(&id)?.is_some() {
            return Ok(PiEffectOutcome::Conflict);
        }
        // The simulator cannot apply before its intent commits. Only absence
        // plus a completed semantic refusal establishes non-admission here;
        // storage/transport errors are deliberately still conservative.
        Ok(match result {
            Err(error) if !error.is_transient() => PiEffectOutcome::Rejected,
            _ => PiEffectOutcome::Uncertain,
        })
    }

    fn execute_fake_at(
        &mut self,
        request: &ExecutionRequest,
        checkpoint: impl FnMut(&str) -> Result<()>,
    ) -> Result<ExecutionReceipt> {
        self.execute_fake_with_clock(request, checkpoint, crate::domain::now_ms)
    }

    fn execute_fake_with_clock(
        &mut self,
        request: &ExecutionRequest,
        mut checkpoint: impl FnMut(&str) -> Result<()>,
        mut now: impl FnMut() -> i64,
    ) -> Result<ExecutionReceipt> {
        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
        if self.runtime.fake_mut().is_none() {
            return Err(Error::guard("simulated execution requires FakeRuntime"));
        }
        let id = request.operation_id()?;
        let hash = request.payload_hash()?;
        let mission = self.store.load_mission(&request.binding.mission_id)?;
        // Authorization precedes even read-only receipt lookup. Recovery may read
        // a completed result after pause, deadline expiry or terminal transition.
        if let Some(existing) = self.store.execution_effect(&id)? {
            if existing.mission_id != request.binding.mission_id
                || existing.attempt_id != request.binding.attempt_id
                || existing.session_id != request.binding.session_id
            {
                return Err(Error::BindingMismatch {
                    detail: "effect binding mismatch".into(),
                });
            }
            if existing.payload_hash != hash {
                return Err(Error::OperationConflict { operation_id: id });
            }
            if self.store.operation_superseded(&id)? {
                return Err(Error::ExecutionUncertain { operation_id: id });
            }
            if existing.state == "not_applied" {
                if self.store.execution_resolution_for(&id)?.is_some() {
                    return Err(Error::ExecutionNotApplied { operation_id: id });
                }
                return Err(Error::ExecutionDeadlineExceeded { operation_id: id });
            }
            if let Some(receipt) = existing.receipt {
                return Ok(receipt);
            }
            self.store.quarantine_execution(&id)?;
            if !mission.phase.is_terminal() {
                self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
            }
            return Err(Error::ExecutionUncertain { operation_id: id });
        }
        request.validate()?;
        if self.store.execution_blocked(&mission.id)? {
            return Err(Error::ExecutionBlocked {
                mission_id: mission.id,
                detail: "unresolved execution effects; this request was not attempted".into(),
            });
        }
        if self.store.ops_blocked(&mission.id)? {
            return Err(Error::ExecutionBlocked {
                mission_id: mission.id,
                detail: "unresolved generic S1 operations; this request was not attempted".into(),
            });
        }
        if mission.phase != Phase::Implementing {
            return Err(Error::guard(
                "new execution requires an implementing mission",
            ));
        }
        if request.deadline_ms <= now() {
            return Err(Error::ExecutionDeadlineExceeded { operation_id: id });
        }
        let workspace = self
            .recover_workspace(&mission.id)?
            .ok_or_else(|| Error::guard("execution requires an owned workspace"))?;
        self.store.require_workspace_ready(&workspace.attempt_id)?;
        self.execution_evidence.prepare(&mut self.store, request)?;
        checkpoint("prepared")?;
        self.accept_runtime_binding(&request.binding, &mission.id)?;
        if request.deadline_ms <= now() {
            // We have not called the backend: retain a definite, nonblocking outcome.
            if let Err(error) = self.store.execution_not_applied(request) {
                return Err(self.execution_failure(request, error));
            }
            self.execution_evidence.remove(&id);
            return Err(Error::ExecutionDeadlineExceeded { operation_id: id });
        }
        let receipt = match self.execution_evidence.apply(&mut self.runtime, request) {
            Ok(receipt) => receipt,
            Err(error) => return Err(self.execution_failure(request, error)),
        };
        checkpoint("effect")?;
        self.accept_runtime_binding(&request.binding, &mission.id)?;
        if let Err(error) = self.store.complete_execution(request, &receipt) {
            return Err(self.execution_failure(request, error));
        }
        self.execution_evidence.remove(&id);
        checkpoint("completed")?;
        Ok(receipt)
    }
    /// Best-effort immediate reconciliation of an in-process failure. Preserve
    /// the original error; if storage cannot record uncertainty, report that too.
    /// Never let a stale owner mutate the mission, or quarantine a committed result.
    fn execution_failure(&mut self, request: &ExecutionRequest, original: Error) -> Error {
        let reconcile = (|| -> Result<()> {
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            let id = request.operation_id()?;
            let existing = self.store.execution_effect(&id)?.ok_or(Error::NotFound)?;
            if existing.receipt.is_some() || existing.state == "not_applied" {
                return Ok(());
            }
            self.store.quarantine_execution(&id)?;
            let mission = self.store.load_mission(&request.binding.mission_id)?;
            if !mission.phase.is_terminal() {
                self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
            }
            Ok(())
        })();
        match reconcile {
            Ok(()) => original,
            Err(error @ Error::StaleGeneration { .. }) => {
                error.with_context(format!("original execution failure: {original}"))
            }
            Err(error) => {
                original.with_context(format!("could not record execution uncertainty: {error}"))
            }
        }
    }
}

#[cfg(test)]
mod tests;
