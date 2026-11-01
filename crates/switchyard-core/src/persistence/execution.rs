use super::*;
use crate::{ExecutionReceipt, ExecutionRequest};

pub(crate) fn identity_conflict(operation_id: &str, error: rusqlite::Error) -> Error {
    if error
        .to_string()
        .contains("operation identity already used")
    {
        Error::OperationConflict {
            operation_id: operation_id.to_string(),
        }
    } else {
        Error::storage(error)
    }
}

pub(crate) fn resolution_command_conflict(command_id: &str, error: rusqlite::Error) -> Error {
    let text = error.to_string();
    if text.contains("resolution command identity already used")
        || text.contains("supersession command identity already used")
    {
        Error::CommandConflict {
            command_id: command_id.to_string(),
        }
    } else {
        Error::storage(error)
    }
}

pub(crate) struct EffectRecord {
    pub mission_id: String,
    pub attempt_id: String,
    pub session_id: String,
    pub payload_hash: String,
    pub state: String,
    pub receipt: Option<ExecutionReceipt>,
}
impl Store {
    fn require_execution_generation(&self, request: &ExecutionRequest) -> Result<()> {
        let expected = self.generation()?;
        if request.binding.generation != expected || self.owner_generation != Some(expected) {
            return Err(Error::StaleGeneration {
                expected,
                actual: request.binding.generation,
            });
        }
        Ok(())
    }
    pub(crate) fn execution_views(&self, mission: &str) -> Result<Vec<crate::ExecutionView>> {
        if user_version(&self.conn)? < 4 {
            return Ok(Vec::new());
        }
        let superseded = if user_version(&self.conn)? >= 9 {
            "EXISTS(SELECT 1 FROM execution_supersessions s WHERE s.operation_id = execution_effects.operation_id AND s.journal = 'simulated')"
        } else {
            "0"
        };
        let mut stmt=self.conn.prepare(&format!("SELECT operation_id, state, payload_hash, started_generation, {superseded} FROM execution_effects WHERE mission_id=?1 ORDER BY created_at, operation_id")).map_err(Error::storage)?;
        stmt.query_map([mission], |r| {
            Ok(crate::ExecutionView {
                operation_id: r.get(0)?,
                state: r.get(1)?,
                payload_hash: r.get(2)?,
                started_generation: r.get(3)?,
                superseded: r.get(4)?,
            })
        })
        .map_err(Error::storage)?
        .map(|r| r.map_err(Error::storage))
        .collect()
    }

    pub(crate) fn execution_effect(&self, id: &str) -> Result<Option<EffectRecord>> {
        if user_version(&self.conn)? < 4 {
            return Ok(None);
        }
        let row = self.conn.query_row(
            "SELECT mission_id, attempt_id, session_id, payload_hash, receipt_json, state FROM execution_effects WHERE operation_id=?1",
            [id], |r| Ok((r.get::<_,String>(0)?, r.get::<_,String>(1)?, r.get::<_,String>(2)?, r.get::<_,String>(3)?, r.get::<_,Option<String>>(4)?, r.get::<_,String>(5)?)))
            .optional().map_err(Error::storage)?;
        row.map(
            |(mission_id, attempt_id, session_id, payload_hash, receipt, state)| {
                Ok(EffectRecord {
                    mission_id,
                    attempt_id,
                    session_id,
                    payload_hash,
                    state,
                    receipt: receipt
                        .map(|s| {
                            ExecutionReceipt::from_json(
                                &serde_json::from_str::<Value>(&s).map_err(Error::storage)?,
                            )
                        })
                        .transpose()?,
                })
            },
        )
        .transpose()
    }
    pub(crate) fn prepare_execution(&mut self, request: &ExecutionRequest) -> Result<()> {
        self.require_execution_generation(request)?;
        let id = request.operation_id()?;
        let hash = request.payload_hash()?;
        let payload = canonicalize(
            &json!({"operation":request.operation.to_json(), "scope":request.capability_scope,
            "deadlineMs":request.deadline_ms, "toolTaskId":request.tool_task_id, "effectKey":request.effect_key}),
        )?;
        let tx = self.immediate()?;
        tx.execute("INSERT INTO execution_effects(operation_id,mission_id,attempt_id,session_id,payload_hash,request_json,started_generation,state,created_at)
            VALUES (?1,?2,?3,?4,?5,?6,?7,'prepared',?8)",
            params![id, request.binding.mission_id, request.binding.attempt_id, request.binding.session_id, hash, payload, request.binding.generation, now_ms()]).map_err(|error| identity_conflict(&id, error))?;
        tx.commit().map_err(Error::storage)
    }
    pub(crate) fn complete_execution(
        &mut self,
        request: &ExecutionRequest,
        receipt: &ExecutionReceipt,
    ) -> Result<()> {
        self.require_execution_generation(request)?;
        if receipt.operation_id != request.operation_id()?
            || receipt.payload_hash != request.payload_hash()?
            || !receipt.simulated
        {
            return Err(Error::guard("execution receipt does not match its intent"));
        }
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE execution_effects SET state='completed', receipt_json=?1
            WHERE operation_id=?2 AND payload_hash=?3 AND started_generation=?4 AND state='prepared'
            AND mission_id=?5 AND attempt_id=?6 AND session_id=?7",
                params![
                    serde_json::to_string(&receipt.to_json()).map_err(Error::storage)?,
                    request.operation_id()?,
                    request.payload_hash()?,
                    request.binding.generation,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "execution settlement requires the original prepared generation",
            ));
        }
        tx.commit().map_err(Error::storage)
    }
    /// Only the still-owning synchronous caller, before apply, can certify this.
    pub(crate) fn execution_not_applied(&mut self, request: &ExecutionRequest) -> Result<()> {
        self.require_execution_generation(request)?;
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE execution_effects SET state='not_applied'
            WHERE operation_id=?1 AND payload_hash=?2 AND started_generation=?3 AND state='prepared'
            AND mission_id=?4 AND attempt_id=?5 AND session_id=?6",
                params![
                    request.operation_id()?,
                    request.payload_hash()?,
                    request.binding.generation,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "not-applied settlement requires the original prepared generation",
            ));
        }
        tx.commit().map_err(Error::storage)
    }
    pub(crate) fn quarantine_execution(&mut self, id: &str) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute("UPDATE execution_effects SET state='uncertain' WHERE operation_id=?1 AND state='prepared'", [id]).map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }
    pub(crate) fn quarantine_pending_execution(&mut self, mission: &str) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute("UPDATE execution_effects SET state='uncertain' WHERE mission_id=?1 AND state='prepared'", [mission]).map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }
    pub(crate) fn execution_blocked(&self, mission: &str) -> Result<bool> {
        if user_version(&self.conn)? < 4 {
            return Ok(false);
        }
        let simulated: bool = if user_version(&self.conn)? >= 9 {
            self.conn.query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM execution_effects e
                    WHERE e.mission_id=?1 AND e.state IN ('prepared', 'uncertain')
                      AND NOT EXISTS (
                        SELECT 1 FROM execution_supersessions s
                        WHERE s.operation_id = e.operation_id AND s.journal = 'simulated'
                      )
                 )",
                [mission],
                |row| row.get(0),
            )
        } else {
            self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM execution_effects WHERE mission_id=?1 AND state IN ('prepared', 'uncertain'))",
                [mission],
                |row| row.get(0),
            )
        }
        .map_err(Error::storage)?;
        Ok(simulated || self.linux_execution_blocked(mission)?)
    }
}
