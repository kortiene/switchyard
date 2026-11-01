use super::*;
use crate::ExecutionSupersessionRequest;
use crate::execution::workspace as fs_workspace;
use std::path::Path;

impl Host {
    /// Append one supersession for an abandoned effect.
    ///
    /// The effect row, its intent, and the abandonment audit stay unchanged.
    /// The outcome stays unknown. Linux rows require the delegated cgroup
    /// backend; this method refuses them with guidance when that backend is
    /// not enabled. The simulated CLI uses this same entry point and therefore
    /// cannot supersede a Linux row.
    pub fn supersede_execution(&mut self, request: &ExecutionSupersessionRequest) -> Result<Value> {
        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
        if self.runtime.fake_mut().is_none() {
            return Err(Error::guard("execution supersession requires FakeRuntime"));
        }
        for (value, limit) in [
            (&request.command_id, 1024usize),
            (&request.actor, 1024),
            (&request.rationale, 8192),
            (&request.abandonment_command_id, 1024),
            (&request.abandonment_request_hash, 1024),
        ] {
            if value.trim().is_empty() || value.len() > limit {
                return Err(Error::guard(
                    "supersession requires bounded nonempty command ID, abandonment ID, actor and rationale",
                ));
            }
        }
        if let Some(audit) = self.store.replay_execution_supersession(request)? {
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
                "inspect and reconcile the mission into needs_attention before superseding an effect",
            ));
        }
        let journal = self.supersession_journal(request)?;
        let evidence_quiescence = self.fresh_linux_quiescence(
            &request.binding.mission_id,
            journal == "linux",
            &request.operation_id,
        )?;
        let (digest, workspace_path) = self.supersession_workspace(&request.binding.attempt_id)?;
        let simulated = journal == "simulated";
        let evidence = json!({
            "source": "operator_supersession",
            "outcomeKnown": false,
            "effectBlockLifted": true,
            "journal": journal,
            "abandonmentCommandId": request.abandonment_command_id,
            "abandonmentRequestHash": request.abandonment_request_hash,
            "quiescence": evidence_quiescence,
            "workspaceDigest": digest,
            "digestKind": "git-tree",
            "digestIncludesIgnored": true,
            "workspacePath": workspace_path,
            "simulated": simulated
        });
        self.store.commit_supersession(request, journal, &evidence)
    }

    fn supersession_journal(&self, request: &ExecutionSupersessionRequest) -> Result<&'static str> {
        let linux = self.store.linux_effect(&request.operation_id)?;
        let simulated = self.store.execution_effect(&request.operation_id)?;
        let journal = match (linux.is_some(), simulated.is_some()) {
            (true, false) => "linux",
            (false, true) => "simulated",
            _ => return Err(Error::NotFound),
        };
        if journal == "linux" {
            self.linux_supervisor_blocks(&request.binding.mission_id)?;
        }
        if let Some(effect) = linux.as_ref() {
            self.require_effect_binding(
                request,
                &effect.mission_id,
                &effect.attempt_id,
                &effect.session_id,
                &effect.payload_hash,
                &effect.state,
            )?;
        }
        if let Some(effect) = simulated.as_ref() {
            self.require_effect_binding(
                request,
                &effect.mission_id,
                &effect.attempt_id,
                &effect.session_id,
                &effect.payload_hash,
                &effect.state,
            )?;
        }
        self.require_abandonment(request, journal)?;
        if self.store.operation_superseded(&request.operation_id)? {
            return Err(Error::guard(
                "effect already has an immutable supersession; replay its command ID",
            ));
        }
        Ok(journal)
    }

    fn require_effect_binding(
        &self,
        request: &ExecutionSupersessionRequest,
        mission_id: &str,
        attempt_id: &str,
        session_id: &str,
        payload_hash: &str,
        state: &str,
    ) -> Result<()> {
        if mission_id != request.binding.mission_id
            || attempt_id != request.binding.attempt_id
            || session_id != request.binding.session_id
        {
            return Err(Error::BindingMismatch {
                detail: "supersession does not match effect ownership".into(),
            });
        }
        if payload_hash != request.payload_hash {
            return Err(Error::OperationConflict {
                operation_id: request.operation_id.clone(),
            });
        }
        if state != "uncertain" {
            return Err(Error::guard(
                "supersession requires an abandoned uncertain effect; completed and not_applied outcomes cannot be superseded",
            ));
        }
        Ok(())
    }

    fn require_abandonment(
        &self,
        request: &ExecutionSupersessionRequest,
        journal: &str,
    ) -> Result<()> {
        let Some((operation_id, request_hash, audit)) = self
            .store
            .abandonment_audit(journal, &request.abandonment_command_id)?
        else {
            return Err(Error::guard(
                "supersession requires a committed abandonment of this effect",
            ));
        };
        if operation_id != request.operation_id || request_hash != request.abandonment_request_hash
        {
            return Err(Error::guard(
                "abandonment command ID or request hash does not match the stored decision",
            ));
        }
        let decision = audit.pointer("/request/decision").and_then(Value::as_str);
        if decision != Some("abandoned") {
            return Err(Error::guard(
                "supersession requires an abandonment decision; completed and not_applied outcomes cannot be superseded",
            ));
        }
        Ok(())
    }

    fn supersession_workspace(&self, attempt_id: &str) -> Result<(String, String)> {
        let workspace = self
            .store
            .load_workspace(attempt_id)?
            .ok_or_else(|| Error::guard("supersession requires the attempt's owned workspace"))?;
        if workspace.view.state != "ready" {
            return Err(Error::guard("supersession requires a ready workspace"));
        }
        let _filters = fs_workspace::FilterCacheScope::enter();
        fs_workspace::verify(&workspace)?;
        let digest = fs_workspace::content_digest(Path::new(&workspace.view.path))?;
        Ok((digest, workspace.view.path))
    }
}
