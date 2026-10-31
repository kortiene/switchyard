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
            if existing.state == "not_applied" {
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
        self.store.prepare_execution(request)?;
        checkpoint("prepared")?;
        self.accept_runtime_binding(&request.binding, &mission.id)?;
        if request.deadline_ms <= now() {
            // We have not called the backend: retain a definite, nonblocking outcome.
            if let Err(error) = self.store.execution_not_applied(request) {
                return Err(self.execution_failure(request, error));
            }
            return Err(Error::ExecutionDeadlineExceeded { operation_id: id });
        }
        // Reborrow after the Host/Store operations. Both checks use the typed
        // adapter accessor; no adapter-name string or panic establishes authority.
        let applied = self
            .runtime
            .fake_mut()
            .ok_or_else(|| Error::guard("simulated execution requires FakeRuntime"))
            .and_then(|runtime| runtime.apply(request));
        let receipt = match applied {
            Ok(receipt) => receipt,
            Err(error) => return Err(self.execution_failure(request, error)),
        };
        checkpoint("effect")?;
        self.accept_runtime_binding(&request.binding, &mission.id)?;
        if let Err(error) = self.store.complete_execution(request, &receipt) {
            return Err(self.execution_failure(request, error));
        }
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
