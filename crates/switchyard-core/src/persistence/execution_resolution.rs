use super::*;
use crate::{ExecutionReceipt, ExecutionResolutionDecision, ExecutionResolutionRequest};

impl Store {
    pub(crate) fn execution_resolutions(&self, mission: &str) -> Result<Vec<Value>> {
        if user_version(&self.conn)? < 6 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT audit_json FROM execution_resolutions WHERE mission_id=?1 ORDER BY rowid",
            )
            .map_err(Error::storage)?;
        stmt.query_map([mission], |r| r.get::<_, String>(0))
            .map_err(Error::storage)?
            .map(|r| serde_json::from_str(&r.map_err(Error::storage)?).map_err(Error::storage))
            .collect()
    }

    pub(crate) fn execution_resolution_for(&self, operation: &str) -> Result<Option<Value>> {
        if user_version(&self.conn)? < 6 {
            return Ok(None);
        }
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT audit_json FROM execution_resolutions WHERE operation_id=?1",
                [operation],
                |r| r.get(0),
            )
            .optional()
            .map_err(Error::storage)?;
        raw.map(|s| serde_json::from_str(&s).map_err(Error::storage))
            .transpose()
    }

    pub(crate) fn replay_execution_resolution(
        &self,
        request: &ExecutionResolutionRequest,
    ) -> Result<Option<Value>> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT request_hash, audit_json FROM execution_resolutions WHERE command_id=?1",
                [&request.command_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
            .map_err(Error::storage)?;
        match row {
            Some((hash, raw)) if hash == request.hash()? => {
                Ok(Some(serde_json::from_str(&raw).map_err(Error::storage)?))
            }
            Some(_) => Err(Error::CommandConflict {
                command_id: request.command_id.clone(),
            }),
            None => Ok(None),
        }
    }

    /// Effect, audit decision and revision commit together under the owner fence.
    pub(crate) fn resolve_execution(
        &mut self,
        request: &ExecutionResolutionRequest,
        receipt: Option<&ExecutionReceipt>,
    ) -> Result<Value> {
        let revision = request
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::guard("mission revision exhausted"))?;
        let audit = json!({"commandId":request.command_id,"request":request.payload(),
        "requestHash":request.hash()?,"generation":request.binding.generation,
        "resultingRevision":revision,"createdAt":now_ms(),
        "evidence": if request.decision == ExecutionResolutionDecision::Abandon {
            json!({"source":"operator_abandonment", "outcomeKnown":false, "blockRetained":true})
        } else {
            json!({"source":"host_observed_fake_execution", "quiescence":"synchronous_in_process_no_workers",
                "receipt":receipt.map(ExecutionReceipt::to_json), "simulated":true})
        }});
        let tx = self.immediate()?;
        let changed = tx.execute(
            "UPDATE missions SET revision=?1, updated_at=?2 WHERE id=?3 AND revision=?4 AND phase='needs_attention'
             AND EXISTS(SELECT 1 FROM attempts WHERE id=?5 AND mission_id=?3 AND session_id=?6 AND active=1)",
            params![revision,now_ms(),request.binding.mission_id,request.expected_revision,
                request.binding.attempt_id,request.binding.session_id],
        ).map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "resolution requires the expected revision and an active needs_attention attempt",
            ));
        }
        let state = match request.decision {
            ExecutionResolutionDecision::Completed => "completed",
            ExecutionResolutionDecision::NotApplied => "not_applied",
            ExecutionResolutionDecision::Abandon => "uncertain",
        };
        let receipt_json = receipt.map(|r| canonicalize(&r.to_json())).transpose()?;
        let changed = tx.execute(
            "UPDATE execution_effects SET state=?1, receipt_json=?2 WHERE operation_id=?3 AND payload_hash=?4
             AND mission_id=?5 AND attempt_id=?6 AND session_id=?7 AND state IN ('prepared','uncertain')",
            params![state,receipt_json,request.operation_id,request.payload_hash,request.binding.mission_id,
                request.binding.attempt_id,request.binding.session_id],
        ).map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "resolution requires a matching unresolved execution intent",
            ));
        }
        tx.execute(
            "INSERT INTO execution_resolutions(command_id,operation_id,mission_id,request_hash,audit_json) VALUES (?1,?2,?3,?4,?5)",
            params![request.command_id,request.operation_id,request.binding.mission_id,request.hash()?,canonicalize(&audit)?],
        ).map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)?;
        Ok(audit)
    }
}
