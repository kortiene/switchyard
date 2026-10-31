use super::*;
use crate::execution_env::ExecutionEnv;
use std::collections::BTreeMap;

/// The only Host path to apply: absence proof cannot outlive backend entry.
/// Keep the map private so callers cannot independently insert an absence proof.
#[derive(Default)]
pub(in crate::service) struct ExecutionEvidence {
    observations: BTreeMap<String, Option<ExecutionReceipt>>,
}

impl ExecutionEvidence {
    pub(in crate::service) fn prepare(
        &mut self,
        store: &mut Store,
        request: &ExecutionRequest,
    ) -> Result<()> {
        let id = request.operation_id()?;
        store.prepare_execution(request)?;
        self.observations.insert(id, None);
        Ok(())
    }

    pub(in crate::service) fn apply(
        &mut self,
        runtime: &mut RuntimeAdapter,
        request: &ExecutionRequest,
    ) -> Result<ExecutionReceipt> {
        let id = request.operation_id()?;
        // Remove even stale absence evidence before any backend call. An error
        // after this point remains unknown; it never restores absence proof.
        self.observations.remove(&id);
        let receipt = runtime
            .fake_mut()
            .ok_or_else(|| Error::guard("simulated execution requires FakeRuntime"))?
            .apply(request)?;
        self.observations.insert(id, Some(receipt.clone()));
        Ok(receipt)
    }

    pub(in crate::service) fn get(&self, id: &str) -> Option<&Option<ExecutionReceipt>> {
        self.observations.get(id)
    }

    pub(in crate::service) fn remove(&mut self, id: &str) {
        self.observations.remove(id);
    }
}
