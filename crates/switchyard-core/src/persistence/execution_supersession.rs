use super::execution::resolution_command_conflict;
use super::*;
use crate::ExecutionSupersessionRequest;

impl Store {
    pub(crate) fn execution_supersessions(&self, mission: &str) -> Result<Vec<Value>> {
        if user_version(&self.conn)? < 9 {
            return Ok(Vec::new());
        }
        let mut stmt = self
            .conn
            .prepare(
                "SELECT audit_json FROM execution_supersessions s
                 WHERE (
                    s.journal = 'simulated' AND EXISTS (
                        SELECT 1 FROM execution_effects e
                        WHERE e.operation_id = s.operation_id AND e.mission_id = ?1
                    )
                 ) OR (
                    s.journal = 'linux' AND EXISTS (
                        SELECT 1 FROM linux_execution_effects e
                        WHERE e.operation_id = s.operation_id AND e.mission_id = ?1
                    )
                 )
                 ORDER BY s.rowid",
            )
            .map_err(Error::storage)?;
        stmt.query_map([mission], |row| row.get::<_, String>(0))
            .map_err(Error::storage)?
            .map(|row| serde_json::from_str(&row.map_err(Error::storage)?).map_err(Error::storage))
            .collect()
    }

    pub(crate) fn abandonment_audit(
        &self,
        journal: &str,
        command_id: &str,
    ) -> Result<Option<(String, String, Value)>> {
        let version = user_version(&self.conn)?;
        let table = match journal {
            "simulated" if version >= 6 => "execution_resolutions",
            "linux" if version >= 7 => "linux_execution_resolutions",
            _ => return Ok(None),
        };
        let row: Option<(String, String, String)> = self
            .conn
            .query_row(
                &format!(
                    "SELECT operation_id, request_hash, audit_json FROM {table} WHERE command_id=?1"
                ),
                [command_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(Error::storage)?;
        row.map(|(operation, hash, raw)| {
            Ok((
                operation,
                hash,
                serde_json::from_str(&raw).map_err(Error::storage)?,
            ))
        })
        .transpose()
    }

    pub(crate) fn operation_superseded(&self, operation: &str) -> Result<bool> {
        if user_version(&self.conn)? < 9 {
            return Ok(false);
        }
        self.conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM execution_supersessions WHERE operation_id=?1)",
                [operation],
                |row| row.get(0),
            )
            .map_err(Error::storage)
    }

    pub(crate) fn replay_execution_supersession(
        &self,
        request: &ExecutionSupersessionRequest,
    ) -> Result<Option<Value>> {
        if user_version(&self.conn)? < 9 {
            return Ok(None);
        }
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT request_hash, audit_json FROM execution_supersessions WHERE command_id=?1",
                [&request.command_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Error::storage)?;
        if row.is_none() {
            let used: bool = self
                .conn
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM execution_resolutions WHERE command_id=?1)
                     OR EXISTS(SELECT 1 FROM linux_execution_resolutions WHERE command_id=?1)",
                    [&request.command_id],
                    |row| row.get(0),
                )
                .map_err(Error::storage)?;
            if used {
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

    /// Revision increment and the supersession audit commit together. The
    /// effect row and the abandonment audit are not rewritten.
    pub(crate) fn commit_supersession(
        &mut self,
        request: &ExecutionSupersessionRequest,
        journal: &str,
        evidence: &Value,
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
            "evidence": evidence
        });
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE missions SET revision=?1, updated_at=?2
                 WHERE id=?3 AND revision=?4 AND phase='needs_attention'
                   AND EXISTS(
                        SELECT 1 FROM attempts
                        WHERE id=?5 AND mission_id=?3 AND session_id=?6 AND active=1
                   )",
                params![
                    revision,
                    now_ms(),
                    request.binding.mission_id,
                    request.expected_revision,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard(
                "supersession requires the expected revision and an active needs_attention attempt",
            ));
        }
        let effect_table = if journal == "linux" {
            "linux_execution_effects"
        } else {
            "execution_effects"
        };
        let resolution_table = if journal == "linux" {
            "linux_execution_resolutions"
        } else {
            "execution_resolutions"
        };
        let matched: i64 = tx
            .query_row(
                &format!(
                    "SELECT COUNT(*) FROM {effect_table}
                     WHERE operation_id=?1 AND payload_hash=?2 AND state='uncertain'
                       AND mission_id=?3 AND attempt_id=?4 AND session_id=?5"
                ),
                params![
                    request.operation_id,
                    request.payload_hash,
                    request.binding.mission_id,
                    request.binding.attempt_id,
                    request.binding.session_id
                ],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        if matched != 1 {
            return Err(Error::guard(
                "supersession requires the original uncertain effect row",
            ));
        }
        let abandonment: Option<String> = tx
            .query_row(
                &format!(
                    "SELECT request_hash FROM {resolution_table}
                     WHERE command_id=?1 AND operation_id=?2 AND mission_id=?3"
                ),
                params![
                    request.abandonment_command_id,
                    request.operation_id,
                    request.binding.mission_id
                ],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::storage)?;
        if abandonment.as_deref() != Some(request.abandonment_request_hash.as_str()) {
            return Err(Error::guard(
                "supersession requires the original abandonment audit",
            ));
        }
        tx.execute(
            "INSERT INTO execution_supersessions(
                command_id, operation_id, journal, abandonment_command_id, request_hash, audit_json
             ) VALUES (?1,?2,?3,?4,?5,?6)",
            params![
                request.command_id,
                request.operation_id,
                journal,
                request.abandonment_command_id,
                request.hash()?,
                canonicalize(&audit)?
            ],
        )
        .map_err(|error| map_supersession_insert(&request.command_id, error))?;
        tx.commit().map_err(Error::storage)?;
        Ok(audit)
    }
}

fn map_supersession_insert(command_id: &str, error: rusqlite::Error) -> Error {
    let text = error.to_string();
    if text.contains("UNIQUE constraint failed")
        && text.contains("execution_supersessions.operation_id")
    {
        return Error::guard("effect already has an immutable supersession; replay its command ID");
    }
    if text.contains("UNIQUE constraint failed")
        && text.contains("execution_supersessions.abandonment_command_id")
    {
        return Error::guard("abandonment already has an immutable supersession");
    }
    resolution_command_conflict(command_id, error)
}
