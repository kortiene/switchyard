use super::*;
use crate::execution_env::ExecutionEnv;
use crate::{ExecutionReceipt, ExecutionRequest};

impl Host {
    pub(super) fn reconcile_execution(&mut self, mission_id: &str) -> Result<()> {
        self.store.assert_workspace_owner()?;
        if self.store.execution_blocked(mission_id)? {
            self.store.quarantine_pending_execution(mission_id)?;
            let mission = self.store.load_mission(mission_id)?;
            if !mission.phase.is_terminal() {
                self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
            }
        }
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

    fn execute_fake_at(
        &mut self,
        request: &ExecutionRequest,
        mut checkpoint: impl FnMut(&str) -> Result<()>,
    ) -> Result<ExecutionReceipt> {
        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
        if self.runtime.name() != "fake" {
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
        if mission.phase != Phase::Implementing {
            return Err(Error::guard(
                "new execution requires an implementing mission",
            ));
        }
        if self.store.ops_blocked(&mission.id)? {
            return Err(Error::ExecutionUncertain { operation_id: id });
        }
        if request.deadline_ms <= crate::domain::now_ms() {
            return Err(Error::guard("execution deadline expired"));
        }
        let workspace = self
            .recover_workspace(&mission.id)?
            .ok_or_else(|| Error::guard("execution requires an owned workspace"))?;
        self.store.require_workspace_ready(&workspace.attempt_id)?;
        self.store.prepare_execution(request)?;
        checkpoint("prepared")?;
        self.accept_runtime_binding(&request.binding, &mission.id)?;
        if request.deadline_ms <= crate::domain::now_ms() {
            self.store.quarantine_execution(&id)?;
            self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
            return Err(Error::ExecutionUncertain { operation_id: id });
        }
        let receipt = self
            .runtime
            .fake_mut()
            .expect("fake adapter checked")
            .apply(request)?;
        checkpoint("effect")?;
        self.accept_runtime_binding(&request.binding, &mission.id)?;
        self.store.complete_execution(request, &receipt)?;
        checkpoint("completed")?;
        Ok(receipt)
    }
}

#[cfg(test)]
mod tests;
