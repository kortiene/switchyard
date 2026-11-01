#![cfg_attr(
    not(all(feature = "linux-execution", target_os = "linux")),
    allow(dead_code, clippy::too_many_arguments)
)]
use super::*;
use crate::linux_execution::{LinuxExecutionRequest, LinuxExecutionView};
use crate::persistence::execution::{identity_conflict, resolution_command_conflict};
use crate::{ExecutionResolutionDecision, ExecutionResolutionRequest};

pub(crate) struct LinuxEffectRecord {
    pub mission_id: String,
    pub attempt_id: String,
    pub session_id: String,
    pub payload_hash: String,
    pub state: String,
    pub started_generation: u64,
    pub cgroup_path: Option<String>,
    pub cgroup_dev: Option<i64>,
    pub cgroup_ino: Option<i64>,
    pub boot_id: Option<String>,
    pub entered_at: Option<i64>,
    pub receipt_json: Option<String>,
    pub quiescence_json: Option<String>,
}

pub(crate) struct StoredCgroup {
    pub path: String,
    pub dev: i64,
    pub ino: i64,
    pub boot_id: String,
}

const BLOCKING_STATES: &str = "('prepared','entered','running','uncertain')";

impl Store {
    pub(crate) fn linux_execution_blocked(&self, mission: &str) -> Result<bool> {
        if user_version(&self.conn)? < 7 {
            return Ok(false);
        }
        self.conn
            .query_row(
                &format!(
                    "SELECT EXISTS(SELECT 1 FROM linux_execution_effects WHERE mission_id=?1 AND state IN {BLOCKING_STATES})"
                ),
                [mission],
                |row| row.get(0),
            )
            .map_err(Error::storage)
    }

    pub(crate) fn linux_views(&self, mission: &str) -> Result<Vec<LinuxExecutionView>> {
        if user_version(&self.conn)? < 7 {
            return Ok(Vec::new());
        }
        let mut stmt = self.conn.prepare(
            "SELECT operation_id, state, payload_hash, started_generation, request_json, entered_at, receipt_json, quiescence_json
             FROM linux_execution_effects WHERE mission_id=?1 ORDER BY created_at, operation_id",
        ).map_err(Error::storage)?;
        let rows = stmt
            .query_map([mission], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u64>(3)?,
                    row.get::<_, String>(4)?,
                    row.get::<_, Option<i64>>(5)?,
                    row.get::<_, Option<String>>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            })
            .map_err(Error::storage)?;
        rows.map(|row| {
            let (
                operation_id,
                state,
                payload_hash,
                started_generation,
                request,
                entered,
                receipt,
                quiescence,
            ) = row.map_err(Error::storage)?;
            let request: Value = serde_json::from_str(&request).unwrap_or(Value::Null);
            Ok(LinuxExecutionView {
                operation_id,
                state,
                payload_hash,
                started_generation,
                scope: request
                    .get("scope")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                method: request
                    .pointer("/operation/method")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown")
                    .to_string(),
                entered: entered.is_some(),
                receipt_retained: receipt.is_some(),
                quiescence: quiescence_summary(quiescence.as_deref()),
            })
        })
        .collect()
    }

    pub(crate) fn linux_resolutions(&self, mission: &str) -> Result<Vec<Value>> {
        if user_version(&self.conn)? < 7 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT audit_json FROM linux_execution_resolutions WHERE mission_id=?1 ORDER BY rowid",
            )
            .map_err(Error::storage)?;
        stmt.query_map([mission], |row| row.get::<_, String>(0))
            .map_err(Error::storage)?
            .map(|row| serde_json::from_str(&row.map_err(Error::storage)?).map_err(Error::storage))
            .collect()
    }

    pub(crate) fn linux_effect(&self, id: &str) -> Result<Option<LinuxEffectRecord>> {
        if user_version(&self.conn)? < 7 {
            return Ok(None);
        }
        self.conn
            .query_row(
                "SELECT mission_id, attempt_id, session_id, payload_hash, state, started_generation,
                        cgroup_path, cgroup_dev, cgroup_ino, boot_id, entered_at, receipt_json, quiescence_json
                 FROM linux_execution_effects WHERE operation_id=?1",
                [id],
                |row| {
                    Ok(LinuxEffectRecord {
                        mission_id: row.get(0)?,
                        attempt_id: row.get(1)?,
                        session_id: row.get(2)?,
                        payload_hash: row.get(3)?,
                        state: row.get(4)?,
                        started_generation: row.get(5)?,
                        cgroup_path: row.get(6)?,
                        cgroup_dev: row.get(7)?,
                        cgroup_ino: row.get(8)?,
                        boot_id: row.get(9)?,
                        entered_at: row.get(10)?,
                        receipt_json: row.get(11)?,
                        quiescence_json: row.get(12)?,
                    })
                },
            )
            .optional()
            .map_err(Error::storage)
    }

    pub(crate) fn prepare_linux(
        &mut self,
        request: &LinuxExecutionRequest,
        cgroup: Option<&StoredCgroup>,
    ) -> Result<()> {
        self.require_linux_generation(request)?;
        let id = request.operation_id()?;
        let hash = request.payload_hash()?;
        let payload = canonicalize(&json!({
            "operation": request.operation.to_json(),
            "scope": request.capability_scope,
            "deadlineMs": request.deadline_ms,
            "toolTaskId": request.tool_task_id,
            "effectKey": request.effect_key
        }))?;
        let tx = self.immediate()?;
        tx.execute(
            "INSERT INTO linux_execution_effects(
                operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id, created_at
             ) VALUES (?1,?2,?3,?4,?5,?6,?7,'prepared',?8,?9,?10,?11,?12)",
            params![
                id,
                request.binding.mission_id,
                request.binding.attempt_id,
                request.binding.session_id,
                hash,
                payload,
                request.binding.generation,
                cgroup.map(|c| c.path.as_str()),
                cgroup.map(|c| c.dev),
                cgroup.map(|c| c.ino),
                cgroup.map(|c| c.boot_id.as_str()),
                now_ms()
            ],
        )
        .map_err(|error| identity_conflict(&id, error))?;
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn mark_linux_entered(
        &mut self,
        request: &LinuxExecutionRequest,
        launcher_pid: u32,
        launcher_start: &str,
    ) -> Result<()> {
        self.require_linux_generation(request)?;
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE linux_execution_effects
                 SET state='entered', entered_at=?1, launcher_pid=?2, launcher_start=?3
                 WHERE operation_id=?4 AND payload_hash=?5 AND started_generation=?6
                   AND state='prepared' AND entered_at IS NULL
                   AND mission_id=?7 AND attempt_id=?8 AND session_id=?9",
                params![
                    now_ms(),
                    launcher_pid,
                    launcher_start,
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
                "linux entry requires the original prepared effect",
            ));
        }
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn mark_linux_running(&mut self, request: &LinuxExecutionRequest) -> Result<()> {
        self.require_linux_generation(request)?;
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE linux_execution_effects SET state='running'
                 WHERE operation_id=?1 AND payload_hash=?2 AND started_generation=?3
                   AND state='entered' AND entered_at IS NOT NULL
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
                "linux running state requires a recorded entry",
            ));
        }
        tx.commit().map_err(Error::storage)
    }

    /// Persist the original captured result without settling the effect.
    pub(crate) fn save_linux_receipt(
        &mut self,
        request: &LinuxExecutionRequest,
        receipt: &Value,
    ) -> Result<()> {
        self.require_linux_generation(request)?;
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE linux_execution_effects SET receipt_json=?1
                 WHERE operation_id=?2 AND payload_hash=?3
                   AND state IN ('entered','running','uncertain') AND receipt_json IS NULL
                   AND mission_id=?4 AND attempt_id=?5 AND session_id=?6",
                params![
                    serde_json::to_string(receipt).map_err(Error::storage)?,
                    request.operation_id()?,
                    request.payload_hash()?,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "linux receipt capture requires the entered effect and no prior receipt",
            ));
        }
        tx.commit().map_err(Error::storage)
    }

    /// Settle a captured start effect without rebuilding its original request.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn finish_linux_identity(
        &mut self,
        operation_id: &str,
        payload_hash: &str,
        generation: u64,
        mission: &str,
        attempt: &str,
        session: &str,
        receipt: &Value,
        quiescence: &Value,
    ) -> Result<()> {
        let expected = self.generation()?;
        if generation != expected || self.owner_generation != Some(expected) {
            return Err(Error::StaleGeneration {
                expected,
                actual: generation,
            });
        }
        if receipt.get("operationId").and_then(Value::as_str) != Some(operation_id)
            || receipt.get("payloadHash").and_then(Value::as_str) != Some(payload_hash)
            || receipt.get("simulated").and_then(Value::as_bool) != Some(false)
        {
            return Err(Error::guard("linux receipt does not match its intent"));
        }
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE linux_execution_effects
                 SET state='completed', receipt_json=?1, quiescence_json=?2
                 WHERE operation_id=?3 AND payload_hash=?4 AND started_generation=?5
                   AND state IN ('entered','running','uncertain')
                   AND mission_id=?6 AND attempt_id=?7 AND session_id=?8",
                params![
                    serde_json::to_string(receipt).map_err(Error::storage)?,
                    serde_json::to_string(quiescence).map_err(Error::storage)?,
                    operation_id,
                    payload_hash,
                    generation,
                    mission,
                    attempt,
                    session
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "linux identity settlement requires the original unresolved effect",
            ));
        }
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn remember_linux_receipt(
        &mut self,
        id: &str,
        payload_hash: &str,
        receipt: &Value,
    ) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute(
            "UPDATE linux_execution_effects SET receipt_json=?1
             WHERE operation_id=?2 AND payload_hash=?3 AND receipt_json IS NULL
               AND state IN ('entered','running','uncertain')",
            params![
                serde_json::to_string(receipt).map_err(Error::storage)?,
                id,
                payload_hash
            ],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn save_linux_quiescence(&mut self, id: &str, quiescence: &Value) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute(
            "UPDATE linux_execution_effects SET quiescence_json=?1
             WHERE operation_id=?2 AND state IN ('prepared','entered','running','uncertain')",
            params![
                serde_json::to_string(quiescence).map_err(Error::storage)?,
                id
            ],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn complete_linux(
        &mut self,
        request: &LinuxExecutionRequest,
        receipt: &Value,
        quiescence: &Value,
    ) -> Result<()> {
        self.require_linux_generation(request)?;
        if receipt.get("operationId").and_then(Value::as_str) != Some(&request.operation_id()?)
            || receipt.get("payloadHash").and_then(Value::as_str) != Some(&request.payload_hash()?)
            || receipt.get("simulated").and_then(Value::as_bool) != Some(false)
        {
            return Err(Error::guard("linux receipt does not match its intent"));
        }
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE linux_execution_effects
                 SET state='completed', receipt_json=?1, quiescence_json=?2
                 WHERE operation_id=?3 AND payload_hash=?4
                   AND state IN ('prepared','entered','running','uncertain')
                   AND mission_id=?5 AND attempt_id=?6 AND session_id=?7",
                params![
                    serde_json::to_string(receipt).map_err(Error::storage)?,
                    serde_json::to_string(quiescence).map_err(Error::storage)?,
                    request.operation_id()?,
                    request.payload_hash()?,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "linux completion requires the original unresolved effect",
            ));
        }
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn linux_not_applied(
        &mut self,
        request: &LinuxExecutionRequest,
        quiescence: &Value,
    ) -> Result<()> {
        self.require_linux_generation(request)?;
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE linux_execution_effects
                 SET state='not_applied', quiescence_json=?1
                 WHERE operation_id=?2 AND payload_hash=?3
                   AND state IN ('prepared','uncertain') AND entered_at IS NULL
                   AND receipt_json IS NULL
                   AND mission_id=?4 AND attempt_id=?5 AND session_id=?6",
                params![
                    serde_json::to_string(quiescence).map_err(Error::storage)?,
                    request.operation_id()?,
                    request.payload_hash()?,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "not-applied requires an unentered linux intent and matching quiescence",
            ));
        }
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn quarantine_linux(&mut self, id: &str) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute(
            "UPDATE linux_execution_effects SET state='uncertain'
             WHERE operation_id=?1 AND state IN ('prepared','entered','running')",
            [id],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn linux_resolution_for(&self, operation: &str) -> Result<Option<Value>> {
        if user_version(&self.conn)? < 7 {
            return Ok(None);
        }
        let raw: Option<String> = self
            .conn
            .query_row(
                "SELECT audit_json FROM linux_execution_resolutions WHERE operation_id=?1",
                [operation],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::storage)?;
        raw.map(|text| serde_json::from_str(&text).map_err(Error::storage))
            .transpose()
    }

    pub(crate) fn replay_linux_resolution(
        &self,
        request: &ExecutionResolutionRequest,
    ) -> Result<Option<Value>> {
        if user_version(&self.conn)? < 7 {
            return Ok(None);
        }
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT request_hash, audit_json FROM linux_execution_resolutions WHERE command_id=?1",
                [&request.command_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Error::storage)?;
        if row.is_none() {
            let simulated: bool = self
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM execution_resolutions WHERE command_id=?1)",
                    [&request.command_id],
                    |row| row.get(0),
                )
                .map_err(Error::storage)?;
            if simulated {
                return Err(Error::CommandConflict {
                    command_id: request.command_id.clone(),
                });
            }
        }
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

    pub(crate) fn resolve_linux(
        &mut self,
        request: &ExecutionResolutionRequest,
        receipt: Option<&Value>,
        quiescence: &Value,
    ) -> Result<Value> {
        let revision = request
            .expected_revision
            .checked_add(1)
            .ok_or_else(|| Error::guard("mission revision exhausted"))?;
        let audit = json!({
            "commandId": request.command_id,
            "request": request.payload(),
            "requestHash": request.hash()?,
            "generation": request.binding.generation,
            "resultingRevision": revision,
            "createdAt": now_ms(),
            "evidence": if request.decision == ExecutionResolutionDecision::Abandon {
                json!({"source":"operator_abandonment","outcomeKnown":false,"blockRetained":true,
                    "quiescence": quiescence, "simulated": false})
            } else {
                json!({"source":"host_observed_linux_execution","simulated":false,
                    "receipt": receipt, "quiescence": quiescence})
            }
        });
        let tx = self.immediate()?;
        let changed = tx.execute(
            "UPDATE missions SET revision=?1, updated_at=?2 WHERE id=?3 AND revision=?4 AND phase='needs_attention'
             AND EXISTS(SELECT 1 FROM attempts WHERE id=?5 AND mission_id=?3 AND session_id=?6 AND active=1)",
            params![revision, now_ms(), request.binding.mission_id, request.expected_revision,
                request.binding.attempt_id, request.binding.session_id],
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
        let receipt_json = receipt
            .map(|value| serde_json::to_string(value).map_err(Error::storage))
            .transpose()?;
        let extra = if request.decision == ExecutionResolutionDecision::NotApplied {
            "AND entered_at IS NULL AND receipt_json IS NULL"
        } else if request.decision == ExecutionResolutionDecision::Completed {
            "AND receipt_json IS NOT NULL"
        } else {
            ""
        };
        let changed = tx
            .execute(
                &format!(
                    "UPDATE linux_execution_effects SET state=?1, receipt_json=COALESCE(?2, receipt_json), quiescence_json=?3
                     WHERE operation_id=?4 AND payload_hash=?5 AND mission_id=?6 AND attempt_id=?7 AND session_id=?8
                       AND state IN ('prepared','entered','running','uncertain') {extra}"
                ),
                params![
                    state,
                    receipt_json,
                    serde_json::to_string(quiescence).map_err(Error::storage)?,
                    request.operation_id,
                    request.payload_hash,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "linux resolution requires a matching unresolved effect and its evidence",
            ));
        }
        tx.execute(
            "INSERT INTO linux_execution_resolutions(command_id,operation_id,mission_id,request_hash,audit_json) VALUES (?1,?2,?3,?4,?5)",
            params![request.command_id, request.operation_id, request.binding.mission_id, request.hash()?, canonicalize(&audit)?],
        ).map_err(|error| resolution_command_conflict(&request.command_id, error))?;
        tx.commit().map_err(Error::storage)?;
        Ok(audit)
    }

    fn require_linux_generation(&self, request: &LinuxExecutionRequest) -> Result<()> {
        let expected = self.generation()?;
        if request.binding.generation != expected || self.owner_generation != Some(expected) {
            return Err(Error::StaleGeneration {
                expected,
                actual: request.binding.generation,
            });
        }
        Ok(())
    }
}

fn quiescence_summary(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return "unproven".into();
    };
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return "unproven".into();
    };
    if value.get("proven").and_then(Value::as_bool) == Some(true)
        && value.get("populated").and_then(Value::as_u64) == Some(0)
    {
        "populated 0".into()
    } else {
        value
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unproven")
            .to_string()
    }
}

pub(crate) fn quiescence_document(
    proven: bool,
    reason: &str,
    populated: Option<u64>,
    events: &str,
    identity: Option<(&str, i64, i64, &str)>,
) -> Value {
    let mut value = json!({
        "proven": proven,
        "reason": reason,
        "populated": populated,
        "events": events,
    });
    if let Some((path, dev, ino, boot)) = identity {
        value["path"] = json!(path);
        value["device"] = json!(dev);
        value["inode"] = json!(ino);
        value["bootId"] = json!(boot);
    }
    value
}
