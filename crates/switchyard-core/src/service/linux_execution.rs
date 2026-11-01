use super::*;

impl Host {
    pub(super) fn reconcile_linux(&mut self, mission_id: &str) -> Result<()> {
        #[cfg(all(feature = "linux-execution", target_os = "linux"))]
        if self.linux.is_some() {
            return self.reconcile_linux_os(mission_id);
        }
        self.note_unproven_linux(mission_id)
    }

    pub(super) fn terminate_linux_writers(&mut self, mission_id: &str) -> Result<()> {
        #[cfg(all(feature = "linux-execution", target_os = "linux"))]
        {
            self.terminate_linux_writers_os(mission_id)?;
        }
        let _ = mission_id;
        Ok(())
    }

    fn note_unproven_linux(&mut self, mission_id: &str) -> Result<()> {
        if !self.store.linux_execution_blocked(mission_id)? {
            return Ok(());
        }
        let mission = self.store.load_mission(mission_id)?;
        self.flag_linux_attention(&mission)
    }

    fn flag_linux_attention(&mut self, mission: &crate::domain::MissionRecord) -> Result<()> {
        let stopping_fake = matches!(self.runtime, RuntimeAdapter::Fake(_))
            && matches!(mission.phase, Phase::Pausing | Phase::Cancelling);
        if !mission.phase.is_terminal() && !stopping_fake {
            self.uncertainty(mission, crate::domain::AttentionReason::ExecutionUncertain)?;
        }
        Ok(())
    }

    /// Mission-wide Linux quiescence for a supersession. Settled and already
    /// superseded effects are not killed again. Every other Linux effect,
    /// including the one being superseded, is re-proven empty. A default build
    /// cannot do that and refuses.
    ///
    /// Re-killing a sibling is not a cross-host kill. [`crate::execution::OwnerLock`]
    /// holds an exclusive `flock` on `{db}.owner.lock` for this Host's whole
    /// lifetime (`execution/owner.rs`). `Host::open` fails with
    /// `second_active_owner` while that lock is held, so no other live process
    /// can own a supervisor for this ledger. `mission_supervisor_live` still
    /// refuses when *this* Host has the supervisor in memory. A sibling left
    /// `entered` or `running` in the ledger belongs to a dead owner; proving
    /// that leaf empty is the quiescence check.
    pub(super) fn fresh_linux_quiescence(
        &self,
        mission_id: &str,
        target_is_linux: bool,
        target_id: &str,
    ) -> Result<Value> {
        let effects = self.store.linux_mission_effects(mission_id)?;
        let mut pending = Vec::new();
        for (id, effect) in effects {
            let settled = matches!(effect.state.as_str(), "completed" | "not_applied");
            let already = self.store.operation_superseded(&id)?;
            let target = target_is_linux && id == target_id;
            if target || (!settled && !already) {
                pending.push((id, effect));
            }
        }
        if pending.is_empty() {
            return Ok(json!({
                "source": "synchronous_in_process_no_workers",
                "exclusiveHost": true,
                "worktreeMutatedBySimulatedEffect": false,
                "linux": []
            }));
        }
        self.prove_pending_linux(mission_id, target_is_linux, &pending)
    }

    pub(super) fn linux_supervisor_blocks(&self, mission_id: &str) -> Result<()> {
        #[cfg(all(feature = "linux-execution", target_os = "linux"))]
        if self.linux.is_some() && self.mission_supervisor_live(mission_id)? {
            return Err(Error::guard(
                "linux supersession refused: this host still has a live supervisor for the mission",
            ));
        }
        let _ = mission_id;
        Ok(())
    }

    fn prove_pending_linux(
        &self,
        mission_id: &str,
        target_is_linux: bool,
        pending: &[(
            String,
            crate::persistence::linux_execution::LinuxEffectRecord,
        )],
    ) -> Result<Value> {
        #[cfg(all(feature = "linux-execution", target_os = "linux"))]
        if self.linux.is_some() {
            return self.prove_effects_for_supersede(mission_id, pending);
        }
        let _ = mission_id;
        if !target_is_linux {
            let ids = pending
                .iter()
                .map(|(id, _)| id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(Error::guard(format!(
                "simulated supersession refused: unsettled linux effect {ids} must be settled or re-proven empty, and this default build cannot prove that cgroup; the simulated effect stays blocked"
            )));
        }
        Err(Error::guard(LINUX_SUPERSEDE_GUIDANCE))
    }
}

pub(super) const LINUX_SUPERSEDE_GUIDANCE: &str = "linux supersession requires the delegated cgroup backend to prove fresh quiescence; the default FakeRuntime build and mission supersede-execution cannot supply that proof, so the linux effect stays uncertain and blocked";

#[cfg(all(feature = "linux-execution", target_os = "linux"))]
mod os {
    use super::*;
    use crate::linux_execution::cgroup::{self, CgroupIdentity, Quiescence};
    use crate::linux_execution::{
        LinuxExecutionConfig, LinuxExecutionOutcome, LinuxExecutionRequest, LinuxOperation,
        MAX_DEADLINE_MS,
    };
    use crate::persistence::linux_execution::{StoredCgroup, quiescence_document};
    use crate::{ExecutionResolutionDecision, ExecutionResolutionRequest};
    use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
    use std::fs;
    use std::io::{BufRead, BufReader, Write};
    use std::os::fd::OwnedFd;
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::mpsc::{self, Receiver};
    use std::time::Duration;

    pub(in crate::service) struct LinuxSession {
        config: LinuxExecutionConfig,
        active: std::collections::BTreeMap<String, ActiveExec>,
    }

    struct ActiveExec {
        child: Child,
        stdin: Option<ChildStdin>,
        lines: Receiver<String>,
        pidfd: OwnedFd,
        scratch: PathBuf,
        cgroup: CgroupIdentity,
    }

    impl Drop for ActiveExec {
        fn drop(&mut self) {
            // EOF makes the supervisor reap the launcher before it proves the
            // leaf empty. Proving first would accept `populated 0` while the
            // launcher can still join.
            self.stdin.take();
            let started = std::time::Instant::now();
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if started.elapsed() > Duration::from_secs(2) => {
                        // The pidfd was opened before the pid could be recycled.
                        let _ = pidfd_send_signal(&self.pidfd, Signal::KILL);
                        let _ = self.child.try_wait();
                        break;
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(20)),
                }
            }
            let _ = cgroup::kill_and_prove(&self.cgroup, Duration::from_secs(2));
        }
    }

    impl LinuxSession {
        fn new(config: LinuxExecutionConfig) -> Self {
            Self {
                config,
                active: std::collections::BTreeMap::new(),
            }
        }
    }

    impl Host {
        pub fn enable_linux_execution(&mut self, config: LinuxExecutionConfig) -> Result<()> {
            if self.runtime.fake_mut().is_none() {
                return Err(Error::guard(
                    "linux execution is a non-default test backend and requires FakeRuntime",
                ));
            }
            require_bwrap(&config.bubblewrap)?;
            if !config.helper.is_file() {
                return Err(Error::guard(
                    "linux execution helper is missing; refusing an unconfined fallback",
                ));
            }
            let root = cgroup::validate_delegation(&config.cgroup_root)?;
            let mut config = config;
            config.cgroup_root = root;
            self.linux = Some(LinuxSession::new(config));
            Ok(())
        }

        pub fn execute_linux_operation(
            &mut self,
            request: &LinuxExecutionRequest,
        ) -> Result<LinuxExecutionOutcome> {
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            let id = request.operation_id()?;
            let hash = request.payload_hash()?;
            if let Some(existing) = self.store.linux_effect(&id)? {
                return self.replay_linux(request, existing);
            }
            request.validate()?;
            self.authorize_new(request)?;
            if let Err(error) = self.launch_linux(request) {
                return Err(self.fail_linux(request, error));
            }
            let effect = self.store.linux_effect(&id)?.ok_or(Error::NotFound)?;
            Ok(outcome(&id, &hash, &effect))
        }

        pub fn resolve_linux_execution(
            &mut self,
            request: &ExecutionResolutionRequest,
        ) -> Result<serde_json::Value> {
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            if self.linux.is_none() {
                return Err(Error::guard(
                    "linux resolution requires the delegated cgroup backend to be enabled",
                ));
            }
            for (value, limit) in [
                (&request.command_id, 1024usize),
                (&request.actor, 1024),
                (&request.rationale, 8192),
            ] {
                if value.trim().is_empty() || value.len() > limit {
                    return Err(Error::guard(
                        "resolution requires bounded nonempty command ID, actor and rationale",
                    ));
                }
            }
            if let Some(audit) = self.store.replay_linux_resolution(request)? {
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
            let effect = self
                .store
                .linux_effect(&request.operation_id)?
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
                .linux_resolution_for(&request.operation_id)?
                .is_some()
                || self
                    .store
                    .execution_resolution_for(&request.operation_id)?
                    .is_some()
            {
                return Err(Error::guard(
                    "effect already has an immutable operator decision; replay its command ID",
                ));
            }
            if self
                .linux
                .as_ref()
                .is_some_and(|session| session.active.contains_key(&request.operation_id))
            {
                return Err(Error::guard(
                    "linux effect still has a live supervisor; cancel it and prove the cgroup is empty first",
                ));
            }
            let quiescence = self.inspect_effect(&effect, false)?;
            let proven = quiescence
                .get("proven")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            let receipt = effect
                .receipt_json
                .as_ref()
                .and_then(|text| serde_json::from_str(text).ok());
            let no_entry =
                effect.released_at.is_none() && effect.entered_at.is_none() && receipt.is_none();
            match request.decision {
                ExecutionResolutionDecision::NotApplied if no_entry && proven => {}
                ExecutionResolutionDecision::Completed
                    if receipt.is_some()
                        && proven
                        && receipt.as_ref().is_some_and(|value: &serde_json::Value| {
                            value.get("operationId").and_then(serde_json::Value::as_str)
                                == Some(request.operation_id.as_str())
                                && value.get("payloadHash").and_then(serde_json::Value::as_str)
                                    == Some(request.payload_hash.as_str())
                        }) => {}
                ExecutionResolutionDecision::Abandon
                    if proven && !(no_entry || receipt.is_some()) => {}
                ExecutionResolutionDecision::Abandon
                    if proven && effect.entered_at.is_some() && receipt.is_none() => {}
                ExecutionResolutionDecision::Abandon if no_entry && proven => {
                    return Err(Error::guard(
                        "Host-observed evidence shows the effect was not applied; resolve that outcome instead of abandoning it",
                    ));
                }
                ExecutionResolutionDecision::Abandon if receipt.is_some() && proven => {
                    return Err(Error::guard(
                        "Host-observed evidence is available; resolve the proven outcome instead of abandoning it",
                    ));
                }
                _ => {
                    return Err(Error::guard(
                        "matching durable linux evidence is unavailable or the cgroup is not quiescent; outcome remains uncertain",
                    ));
                }
            }
            self.store
                .resolve_linux(request, receipt.as_ref(), &quiescence)
        }

        pub(super) fn reconcile_linux_os(&mut self, mission_id: &str) -> Result<()> {
            let views = self.store.linux_views(mission_id)?;
            for view in views {
                if !blocking(&view.state) {
                    continue;
                }
                if self
                    .linux
                    .as_ref()
                    .is_some_and(|session| session.active.contains_key(&view.operation_id))
                {
                    continue;
                }
                let Some(effect) = self.store.linux_effect(&view.operation_id)? else {
                    continue;
                };
                if let Ok(quiescence) = self.prove_effect(&effect) {
                    self.store
                        .save_linux_quiescence(&view.operation_id, &quiescence)?;
                }
                if (effect.released_at.is_some() || effect.entered_at.is_some())
                    && effect.receipt_json.is_none()
                {
                    self.store.quarantine_linux(&view.operation_id)?;
                }
            }
            if self.store.linux_execution_blocked(mission_id)? {
                let mission = self.store.load_mission(mission_id)?;
                self.flag_linux_attention(&mission)?;
            }
            Ok(())
        }

        pub(super) fn terminate_linux_writers_os(&mut self, mission_id: &str) -> Result<()> {
            let ids: Vec<String> = self
                .linux
                .as_ref()
                .map(|session| session.active.keys().cloned().collect())
                .unwrap_or_default();
            let mut doomed = Vec::new();
            for id in ids {
                if self
                    .store
                    .linux_effect(&id)?
                    .is_some_and(|effect| effect.mission_id == mission_id)
                {
                    doomed.push(id);
                }
            }
            if let Some(session) = self.linux.as_mut() {
                for id in doomed {
                    session.active.remove(&id);
                }
            }
            Ok(())
        }

        fn replay_linux(
            &mut self,
            request: &LinuxExecutionRequest,
            effect: crate::persistence::linux_execution::LinuxEffectRecord,
        ) -> Result<LinuxExecutionOutcome> {
            if effect.mission_id != request.binding.mission_id
                || effect.attempt_id != request.binding.attempt_id
                || effect.session_id != request.binding.session_id
            {
                return Err(Error::BindingMismatch {
                    detail: "effect binding mismatch".into(),
                });
            }
            let hash = request.payload_hash()?;
            if effect.payload_hash != hash {
                return Err(Error::OperationConflict {
                    operation_id: request.operation_id()?,
                });
            }
            let id = request.operation_id()?;
            if self.store.operation_superseded(&id)? {
                return Err(Error::ExecutionUncertain { operation_id: id });
            }
            if effect.state == "completed" {
                return Ok(outcome(&id, &hash, &effect));
            }
            if effect.state == "not_applied" {
                return Err(not_applied_error(&effect, &id));
            }
            if self
                .linux
                .as_ref()
                .is_some_and(|session| session.active.contains_key(&id))
            {
                self.poll_active(&id)?;
                let effect = self.store.linux_effect(&id)?.ok_or(Error::NotFound)?;
                if effect.state == "running" || effect.state == "entered" {
                    return Ok(outcome(&id, &hash, &effect));
                }
                if effect.state == "completed" {
                    return Ok(outcome(&id, &hash, &effect));
                }
            }
            // A restarted or finished intent is never launched again.
            let quiescence = self.prove_effect(&effect)?;
            let proven = quiescence
                .get("proven")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            if effect.released_at.is_none()
                && effect.entered_at.is_none()
                && effect.receipt_json.is_none()
                && proven
            {
                self.store.linux_not_applied(request, &quiescence)?;
                return Err(Error::ExecutionNotApplied { operation_id: id });
            }
            if let Some(text) = effect.receipt_json.clone() {
                let receipt: serde_json::Value =
                    serde_json::from_str(&text).map_err(Error::storage)?;
                if proven {
                    self.store.complete_linux(request, &receipt, &quiescence)?;
                    let effect = self.store.linux_effect(&id)?.ok_or(Error::NotFound)?;
                    return Ok(outcome(&id, &hash, &effect));
                }
            }
            let _ = self.store.save_linux_quiescence(&id, &quiescence);
            self.store.quarantine_linux(&id)?;
            let mission = self.store.load_mission(&request.binding.mission_id)?;
            if !mission.phase.is_terminal() {
                self.uncertainty(&mission, crate::domain::AttentionReason::ExecutionUncertain)?;
            }
            Err(Error::ExecutionUncertain { operation_id: id })
        }

        fn authorize_new(&self, request: &LinuxExecutionRequest) -> Result<()> {
            let mission = self.store.load_mission(&request.binding.mission_id)?;
            let except = match &request.operation {
                LinuxOperation::ExecStatus {
                    start_operation_id, ..
                }
                | LinuxOperation::ExecCancel {
                    start_operation_id, ..
                } => Some(start_operation_id.as_str()),
                _ => None,
            };
            if self.blocked_by_others(&mission.id, except)? {
                return Err(Error::ExecutionBlocked {
                    mission_id: mission.id,
                    detail: "unresolved execution effects; this request was not attempted".into(),
                });
            }
            if self.store.generic_ops_blocked(&mission.id)? {
                return Err(Error::ExecutionBlocked {
                    mission_id: mission.id,
                    detail: "unresolved generic S1 operations; this request was not attempted"
                        .into(),
                });
            }
            let status_or_cancel = matches!(
                request.operation,
                LinuxOperation::ExecStatus { .. } | LinuxOperation::ExecCancel { .. }
            );
            if status_or_cancel {
                if !matches!(mission.phase, Phase::Implementing | Phase::NeedsAttention) {
                    return Err(Error::guard(
                        "linux status and cancel require an implementing or needs_attention mission",
                    ));
                }
            } else if mission.phase != Phase::Implementing {
                return Err(Error::guard(
                    "new linux execution requires an implementing mission",
                ));
            }
            self.ensure_deadline(request)?;
            Ok(())
        }

        fn blocked_by_others(&self, mission: &str, except: Option<&str>) -> Result<bool> {
            let simulated = self
                .store
                .execution_views(mission)?
                .into_iter()
                .any(|view| {
                    matches!(view.state.as_str(), "prepared" | "uncertain") && !view.superseded
                });
            let linux = self.store.linux_views(mission)?.into_iter().any(|view| {
                blocking(&view.state)
                    && !view.superseded
                    && Some(view.operation_id.as_str()) != except
            });
            Ok(simulated || linux)
        }

        fn launch_linux(&mut self, request: &LinuxExecutionRequest) -> Result<()> {
            match &request.operation {
                LinuxOperation::ExecStatus {
                    start_operation_id,
                    start_payload_hash,
                } => self.linux_status(request, start_operation_id, start_payload_hash),
                LinuxOperation::ExecCancel {
                    start_operation_id,
                    start_payload_hash,
                } => self.linux_cancel(request, start_operation_id, start_payload_hash),
                _ => self.launch_workload(request),
            }
        }

        fn linux_status(
            &mut self,
            request: &LinuxExecutionRequest,
            start_id: &str,
            start_hash: &str,
        ) -> Result<()> {
            let start = self.matching_start(request, start_id, start_hash)?;
            self.poll_active(start_id)?;
            let start = self.store.linux_effect(start_id)?.unwrap_or(start);
            let quiescence = self
                .inspect_effect(&start, false)
                .unwrap_or_else(|_| quiescence_document(false, "unproven", None, "", None));
            self.store.prepare_linux(request, None)?;
            self.barrier("prepared");
            let receipt = observation_receipt(request, &start, &quiescence);
            let observation =
                quiescence_document(true, "no_workload", Some(0), "status observation", None);
            self.store.complete_linux(request, &receipt, &observation)?;
            self.barrier("completed");
            Ok(())
        }

        fn linux_cancel(
            &mut self,
            request: &LinuxExecutionRequest,
            start_id: &str,
            start_hash: &str,
        ) -> Result<()> {
            let start = self.matching_start(request, start_id, start_hash)?;
            if self.store.linux_resolution_for(start_id)?.is_some() {
                return Err(Error::guard(
                    "abandoned linux effect is final; cancel cannot complete it",
                ));
            }
            self.store.prepare_linux(request, None)?;
            self.barrier("prepared");
            if let Some(session) = self.linux.as_mut() {
                session.active.remove(start_id);
            }
            let quiescence = self.prove_effect(&start)?;
            if quiescence
                .get("proven")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            {
                return Err(Error::guard(
                    "cancel could not prove the start cgroup is empty; the effect stays uncertain",
                ));
            }
            let start = self.store.linux_effect(start_id)?.ok_or(Error::NotFound)?;
            if start.receipt_json.is_none() && start.entered_at.is_some() {
                let receipt = json!({
                    "operationId": start_id,
                    "payloadHash": start_hash,
                    "simulated": false,
                    "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
                    "disposition": "cancelled",
                    "result": {"disposition": "cancelled"}
                });
                // The start request is not this cancel request. Settle it by id.
                self.store.save_linux_quiescence(start_id, &quiescence)?;
                self.complete_start_cancelled(start_id, start_hash, &receipt, &quiescence)?;
            }
            let receipt = json!({
                "operationId": request.operation_id()?,
                "payloadHash": request.payload_hash()?,
                "simulated": false,
                "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
                "disposition": "cancelled",
                "startOperationId": start_id,
            });
            let observation = quiescence_document(true, "populated 0", Some(0), "cancel", None);
            self.store.complete_linux(request, &receipt, &observation)?;
            self.barrier("completed");
            Ok(())
        }

        fn complete_start_cancelled(
            &mut self,
            start_id: &str,
            start_hash: &str,
            receipt: &serde_json::Value,
            quiescence: &serde_json::Value,
        ) -> Result<()> {
            let effect = self.store.linux_effect(start_id)?.ok_or(Error::NotFound)?;
            if effect.payload_hash != start_hash {
                return Err(Error::OperationConflict {
                    operation_id: start_id.to_string(),
                });
            }
            if effect.state == "completed" {
                return Ok(());
            }
            self.store.finish_linux_identity(
                start_id,
                start_hash,
                effect.started_generation,
                &effect.mission_id,
                &effect.attempt_id,
                &effect.session_id,
                receipt,
                quiescence,
            )
        }

        fn launch_workload(&mut self, request: &LinuxExecutionRequest) -> Result<()> {
            let session = self.linux.as_ref().ok_or_else(|| {
                Error::guard("linux execution backend is not enabled on this host")
            })?;
            let config = session.config.clone();
            // A new owner must re-verify the worktree before the first launch.
            // This is refused earlier when a real writer is still unresolved.
            self.recover_workspace(&request.binding.mission_id)?;
            let workspace = self.ready_workspace_path(&request.binding.mission_id)?;
            cgroup::git_file_is_regular(&workspace)?;
            let id = request.operation_id()?;
            let leaf = cgroup::create_leaf(&config.cgroup_root, &cgroup::leaf_name(&id)?)?;
            if !cgroup::within_root(&config.cgroup_root, &leaf.path) {
                return Err(Error::guard("cgroup leaf escaped the delegated root"));
            }
            let stored = StoredCgroup {
                path: leaf.path.display().to_string(),
                dev: i64::try_from(leaf.dev)
                    .map_err(|_| Error::guard("cgroup device id does not fit"))?,
                ino: i64::try_from(leaf.ino)
                    .map_err(|_| Error::guard("cgroup inode does not fit"))?,
                boot_id: leaf.boot_id.clone(),
            };
            self.store.prepare_linux(request, Some(&stored))?;
            self.barrier("prepared");
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            if request.deadline_ms <= crate::domain::now_ms() {
                let quiescence = prove_identity(&leaf)?;
                if quiescence
                    .get("proven")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true)
                {
                    self.store.linux_not_applied(request, &quiescence)?;
                }
                return Err(Error::ExecutionDeadlineExceeded { operation_id: id });
            }
            let scratch = scratch_dir()?;
            write_launch_files(&scratch, &config, request)?;
            cgroup::git_file_is_regular(&workspace)?;
            let mut active = spawn_supervisor(&config, &leaf, &workspace, &scratch, request)?;
            let ready = wait_line(&active.lines, Duration::from_secs(10))?;
            if ready != "READY" {
                return Err(Error::guard(format!(
                    "linux supervisor was not ready: {ready}"
                )));
            }
            self.barrier("ready");
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            let start = start_time(active.child.id())?;
            self.store
                .mark_linux_released(request, active.child.id(), &start)?;
            self.barrier("released");
            let stdin = active
                .stdin
                .as_mut()
                .ok_or_else(|| Error::guard("missing supervisor stdin"))?;
            writeln!(stdin, "GO").map_err(Error::storage)?;
            stdin.flush().map_err(Error::storage)?;
            self.barrier("go");
            let exec = matches!(request.operation, LinuxOperation::ExecStart { .. });
            let mut saw_entered = false;
            loop {
                let line = wait_line(&active.lines, remaining(request)?)?;
                match line.as_str() {
                    "ENTERED" if !saw_entered => {
                        let start = start_time(active.child.id())?;
                        self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
                        self.store
                            .mark_linux_entered(request, active.child.id(), &start)?;
                        self.barrier("entered");
                        saw_entered = true;
                    }
                    "STARTED" if exec => {
                        if !saw_entered {
                            return Err(Error::guard(
                                "linux workload reported STARTED before entry",
                            ));
                        }
                        // A fast command can finish in the same breath as STARTED.
                        if active
                            .lines
                            .recv_timeout(Duration::from_millis(500))
                            .ok()
                            .as_deref()
                            == Some("DONE")
                        {
                            self.note_late_entry(request, &active, &scratch, saw_entered)?;
                            drop(active);
                            self.finish_sync(request, &leaf, &scratch)?;
                            return Ok(());
                        }
                        self.store.mark_linux_running(request)?;
                        self.barrier("running");
                        self.linux
                            .as_mut()
                            .ok_or_else(|| Error::guard("linux backend disappeared"))?
                            .active
                            .insert(id, active);
                        return Ok(());
                    }
                    "DONE" => {
                        self.note_late_entry(request, &active, &scratch, saw_entered)?;
                        drop(active);
                        self.finish_sync(request, &leaf, &scratch)?;
                        return Ok(());
                    }
                    other => {
                        return Err(Error::guard(format!("linux supervisor reported {other}")));
                    }
                }
            }
        }

        fn finish_sync(
            &mut self,
            request: &LinuxExecutionRequest,
            leaf: &CgroupIdentity,
            scratch: &Path,
        ) -> Result<()> {
            let result: serde_json::Value = serde_json::from_str(
                &fs::read_to_string(scratch.join("result.json")).map_err(Error::storage)?,
            )
            .map_err(Error::storage)?;
            let proved = cgroup::kill_and_prove(leaf, Duration::from_secs(5))?;
            let quiescence = quiescence_value(&proved, leaf);
            let id = request.operation_id()?;
            let effect = self.store.linux_effect(&id)?.ok_or(Error::NotFound)?;
            let workload_entered = result
                .get("workloadEntered")
                .and_then(serde_json::Value::as_bool)
                .ok_or_else(|| Error::guard("linux result is missing explicit entry evidence"))?;
            // No Bubblewrap child-pid and an empty leaf: the workload never started.
            if effect.entered_at.is_none() && !workload_entered {
                if proved.proven && effect.receipt_json.is_none() {
                    self.store.linux_launch_not_applied(request, &quiescence)?;
                    return Err(Error::ExecutionNotApplied { operation_id: id });
                }
                self.store.quarantine_linux(&id)?;
                let mission = self.store.load_mission(&request.binding.mission_id)?;
                if !mission.phase.is_terminal() {
                    self.uncertainty(&mission, crate::domain::AttentionReason::ExecutionUncertain)?;
                }
                return Err(Error::guard(
                    "linux launch failed without an empty-cgroup proof; outcome remains uncertain",
                ));
            }
            if effect.entered_at.is_none() && workload_entered {
                self.store.quarantine_linux(&id)?;
                return Err(Error::guard(
                    "linux workload entered but the entry marker was not recorded",
                ));
            }
            // A failed read/list still has a conclusive, non-mutating outcome.
            // Keep its error/truncation in the receipt without quarantining
            // the mission. Missing results still fail closed above.
            let read_only = matches!(
                request.operation,
                LinuxOperation::ReadFile { .. }
                    | LinuxOperation::ReadFileRange { .. }
                    | LinuxOperation::ListDirectory { .. }
            );
            let settle = read_only
                || result.get("settle").and_then(serde_json::Value::as_bool) == Some(true);
            if !settle {
                self.store.quarantine_linux(&id)?;
                let mission = self.store.load_mission(&request.binding.mission_id)?;
                if !mission.phase.is_terminal() {
                    self.uncertainty(&mission, crate::domain::AttentionReason::ExecutionUncertain)?;
                }
                return Err(Error::guard(
                    "linux helper reported failure; the effect stays uncertain and was not completed",
                ));
            }
            let receipt = host_receipt(request, &result, leaf);
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            self.store.save_linux_receipt(request, &receipt)?;
            self.barrier("receipt");
            if !proved.proven {
                self.store.save_linux_quiescence(&id, &quiescence)?;
                return Err(Error::guard(format!(
                    "workload result was captured but cgroup quiescence is {}",
                    proved.reason
                )));
            }
            self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
            self.store.complete_linux(request, &receipt, &quiescence)?;
            let _ = fs::remove_dir_all(scratch);
            self.barrier("completed");
            Ok(())
        }

        fn note_late_entry(
            &mut self,
            request: &LinuxExecutionRequest,
            active: &ActiveExec,
            scratch: &Path,
            saw_entered: bool,
        ) -> Result<()> {
            if saw_entered {
                return Ok(());
            }
            let entered = fs::read_to_string(scratch.join("result.json"))
                .ok()
                .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
                .and_then(|value| value.get("workloadEntered").and_then(|bit| bit.as_bool()))
                == Some(true);
            if !entered {
                return Ok(());
            }
            let start = start_time(active.child.id())?;
            self.store
                .mark_linux_entered(request, active.child.id(), &start)?;
            self.barrier("entered");
            Ok(())
        }

        fn matching_start(
            &self,
            request: &LinuxExecutionRequest,
            start_id: &str,
            start_hash: &str,
        ) -> Result<crate::persistence::linux_execution::LinuxEffectRecord> {
            let start = self.store.linux_effect(start_id)?.ok_or(Error::NotFound)?;
            if start.mission_id != request.binding.mission_id
                || start.attempt_id != request.binding.attempt_id
                || start.session_id != request.binding.session_id
                || start.payload_hash != start_hash
            {
                return Err(Error::BindingMismatch {
                    detail: "status/cancel does not match the start effect".into(),
                });
            }
            Ok(start)
        }

        fn ready_workspace_path(&mut self, mission_id: &str) -> Result<PathBuf> {
            let mission = self.store.load_mission(mission_id)?;
            self.store.require_workspace_ready(&mission.attempt.id)?;
            let workspace = self
                .store
                .load_workspace(&mission.attempt.id)?
                .ok_or_else(|| Error::guard("linux execution requires an owned workspace"))?;
            if workspace.view.state != "ready" {
                return Err(Error::guard("linux execution requires a ready workspace"));
            }
            let path = PathBuf::from(&workspace.view.path);
            cgroup::git_file_is_regular(&path)?;
            fs::canonicalize(&path).map_err(Error::storage)
        }

        fn prove_effect(
            &self,
            effect: &crate::persistence::linux_execution::LinuxEffectRecord,
        ) -> Result<serde_json::Value> {
            self.inspect_effect(effect, true)
        }

        fn inspect_effect(
            &self,
            effect: &crate::persistence::linux_execution::LinuxEffectRecord,
            kill: bool,
        ) -> Result<serde_json::Value> {
            let Some(path) = effect.cgroup_path.as_deref() else {
                return Ok(quiescence_document(
                    effect.entered_at.is_none(),
                    if effect.entered_at.is_none() {
                        "no_workload"
                    } else {
                        "missing"
                    },
                    if effect.entered_at.is_none() {
                        Some(0)
                    } else {
                        None
                    },
                    "",
                    None,
                ));
            };
            let (dev, ino, boot) = match (
                effect.cgroup_dev,
                effect.cgroup_ino,
                effect.boot_id.as_deref(),
            ) {
                (Some(dev), Some(ino), Some(boot)) => (dev, ino, boot),
                _ => return Ok(quiescence_document(false, "missing", None, "", None)),
            };
            let root = self
                .linux
                .as_ref()
                .ok_or_else(|| Error::guard("linux backend is not enabled"))?
                .config
                .cgroup_root
                .clone();
            let leaf = PathBuf::from(path);
            if !cgroup::within_root(&root, &leaf) {
                return Ok(quiescence_document(false, "mismatched", None, "", None));
            }
            let identity = CgroupIdentity {
                path: leaf,
                dev: u64::try_from(dev)
                    .map_err(|_| Error::guard("cgroup device id is negative"))?,
                ino: u64::try_from(ino).map_err(|_| Error::guard("cgroup inode is negative"))?,
                boot_id: boot.to_string(),
            };
            let proved = if kill {
                cgroup::kill_and_prove(&identity, Duration::from_secs(5))?
            } else {
                cgroup::observe(&identity)?
            };
            Ok(quiescence_value(&proved, &identity))
        }

        fn poll_active(&mut self, id: &str) -> Result<()> {
            let polled = {
                let Some(session) = self.linux.as_mut() else {
                    return Ok(());
                };
                let Some(active) = session.active.get_mut(id) else {
                    return Ok(());
                };
                let mut done = active.child.try_wait().map_err(Error::storage)?.is_some();
                while let Ok(line) = active.lines.try_recv() {
                    if line == "DONE" {
                        done = true;
                    }
                }
                if !done {
                    return Ok(());
                }
                let scratch = active.scratch.clone();
                let cgroup = CgroupIdentity {
                    path: active.cgroup.path.clone(),
                    dev: active.cgroup.dev,
                    ino: active.cgroup.ino,
                    boot_id: active.cgroup.boot_id.clone(),
                };
                session.active.remove(id);
                (scratch, cgroup)
            };
            let Ok(text) = fs::read_to_string(polled.0.join("result.json")) else {
                return Ok(());
            };
            let Ok(result) = serde_json::from_str::<serde_json::Value>(&text) else {
                return Ok(());
            };
            let Some(effect) = self.store.linux_effect(id)? else {
                return Ok(());
            };
            if effect.receipt_json.is_some() || effect.state == "completed" {
                return Ok(());
            }
            let receipt = captured_receipt(id, &effect.payload_hash, &result, &polled.1);
            let quiescence = quiescence_value(
                &cgroup::observe(&polled.1).unwrap_or(Quiescence {
                    proven: false,
                    reason: "unproven".into(),
                    populated: None,
                    events: String::new(),
                    pids_max: 0,
                    oom_kill: 0,
                }),
                &polled.1,
            );
            if quiescence
                .get("proven")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
            {
                self.store.finish_linux_identity(
                    id,
                    &effect.payload_hash,
                    effect.started_generation,
                    &effect.mission_id,
                    &effect.attempt_id,
                    &effect.session_id,
                    &receipt,
                    &quiescence,
                )?;
            } else {
                self.store
                    .remember_linux_receipt(id, &effect.payload_hash, &receipt)?;
                self.store.save_linux_quiescence(id, &quiescence)?;
            }
            let _ = fs::remove_dir_all(&polled.0);
            Ok(())
        }

        fn ensure_deadline(&self, request: &LinuxExecutionRequest) -> Result<()> {
            let left = request.deadline_ms.saturating_sub(crate::domain::now_ms());
            if left <= 0 {
                return Err(Error::ExecutionDeadlineExceeded {
                    operation_id: request.operation_id()?,
                });
            }
            if left > MAX_DEADLINE_MS {
                return Err(Error::guard("linux operation deadline exceeds 60 seconds"));
            }
            Ok(())
        }

        fn fail_linux(&mut self, request: &LinuxExecutionRequest, original: Error) -> Error {
            let reconcile = (|| -> Result<()> {
                self.accept_runtime_binding(&request.binding, &request.binding.mission_id)?;
                let id = request.operation_id()?;
                let Some(effect) = self.store.linux_effect(&id)? else {
                    return Ok(());
                };
                if effect.state == "completed" || effect.state == "not_applied" {
                    return Ok(());
                }
                if effect.released_at.is_none()
                    && effect.entered_at.is_none()
                    && let Ok(quiescence) = self.prove_effect(&effect)
                    && quiescence
                        .get("proven")
                        .and_then(serde_json::Value::as_bool)
                        == Some(true)
                    && effect.receipt_json.is_none()
                {
                    self.store.linux_not_applied(request, &quiescence)?;
                    return Ok(());
                }
                self.store.quarantine_linux(&id)?;
                let mission = self.store.load_mission(&request.binding.mission_id)?;
                if !mission.phase.is_terminal() {
                    self.uncertainty(&mission, crate::domain::AttentionReason::ExecutionUncertain)?;
                }
                Ok(())
            })();
            match reconcile {
                Ok(()) => original,
                Err(error) => {
                    original.with_context(format!("could not record linux uncertainty: {error}"))
                }
            }
        }

        fn barrier(&self, stage: &str) {
            let crash = self
                .linux
                .as_ref()
                .and_then(|session| session.config.crash_at.as_deref());
            if crash == Some(stage) {
                println!("AT-{stage}");
                let _ = std::io::Write::flush(&mut std::io::stdout());
                loop {
                    std::thread::sleep(Duration::from_secs(30));
                }
            }
        }

        pub(super) fn prove_effects_for_supersede(
            &self,
            mission_id: &str,
            pending: &[(
                String,
                crate::persistence::linux_execution::LinuxEffectRecord,
            )],
        ) -> Result<serde_json::Value> {
            if self.mission_supervisor_live(mission_id)? {
                return Err(Error::guard(
                    "linux supersession refused: this host still has a live supervisor for the mission",
                ));
            }
            // OwnerLock excludes a second live Host; see fresh_linux_quiescence.
            let mut proofs = Vec::new();
            for (id, effect) in pending {
                let doc = self
                    .fresh_leaf(effect)
                    .map_err(|error| attach_kill_evidence(error, &proofs))?;
                if !crate::persistence::linux_execution::fresh_quiescence_is_empty(&doc) {
                    let reason = doc
                        .get("reason")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("unproven");
                    let refusal = if reason == "outside_root" {
                        format!(
                            "linux supersession refused: cgroup path for {id} is outside the delegated root"
                        )
                    } else {
                        format!(
                            "linux supersession refused: cgroup for {id} is {reason} and not populated 0; stored abandonment quiescence is not reused"
                        )
                    };
                    let error = Error::guard(format!("{refusal}; kill evidence {doc}"));
                    return Err(attach_kill_evidence(error, &proofs));
                }
                proofs.push(json!({"operationId": id, "quiescence": doc}));
            }
            Ok(json!({
                "source": "fresh_cgroup_kill",
                "reusedAbandonmentQuiescence": false,
                "effects": proofs
            }))
        }

        pub(super) fn mission_supervisor_live(&self, mission_id: &str) -> Result<bool> {
            let Some(session) = self.linux.as_ref() else {
                return Ok(false);
            };
            for id in session.active.keys() {
                match self.store.linux_effect(id)? {
                    Some(effect) if effect.mission_id == mission_id => return Ok(true),
                    Some(_) => {}
                    None => return Ok(true),
                }
            }
            Ok(false)
        }

        fn fresh_leaf(
            &self,
            effect: &crate::persistence::linux_execution::LinuxEffectRecord,
        ) -> Result<serde_json::Value> {
            let Some(path) = effect.cgroup_path.as_deref() else {
                return Ok(quiescence_document(
                    effect.entered_at.is_none(),
                    if effect.entered_at.is_none() {
                        "no_workload"
                    } else {
                        "missing"
                    },
                    if effect.entered_at.is_none() {
                        Some(0)
                    } else {
                        None
                    },
                    "",
                    None,
                ));
            };
            let (dev, ino, boot) = match (
                effect.cgroup_dev,
                effect.cgroup_ino,
                effect.boot_id.as_deref(),
            ) {
                (Some(dev), Some(ino), Some(boot)) => (dev, ino, boot),
                _ => return Ok(quiescence_document(false, "missing", None, "", None)),
            };
            let root = self
                .linux
                .as_ref()
                .ok_or_else(|| Error::guard("linux backend is not enabled"))?
                .config
                .cgroup_root
                .clone();
            let leaf = PathBuf::from(path);
            if leaf.exists() && !cgroup::within_root(&root, &leaf) {
                return Ok(quiescence_document(false, "outside_root", None, "", None));
            }
            let identity = CgroupIdentity {
                path: leaf,
                dev: u64::try_from(dev)
                    .map_err(|_| Error::guard("cgroup device id is negative"))?,
                ino: u64::try_from(ino).map_err(|_| Error::guard("cgroup inode is negative"))?,
                boot_id: boot.to_string(),
            };
            let proved = cgroup::rekill_and_prove(&identity, Duration::from_secs(5))?;
            Ok(quiescence_value(&proved, &identity))
        }
    }

    fn attach_kill_evidence(error: Error, proofs: &[serde_json::Value]) -> Error {
        if proofs.is_empty() {
            return error;
        }
        error.with_context(format!(
            "supersession already re-killed other leaves: {}",
            serde_json::Value::Array(proofs.to_vec())
        ))
    }

    fn outcome(
        id: &str,
        hash: &str,
        effect: &crate::persistence::linux_execution::LinuxEffectRecord,
    ) -> LinuxExecutionOutcome {
        LinuxExecutionOutcome {
            operation_id: id.to_string(),
            payload_hash: hash.to_string(),
            state: effect.state.clone(),
            receipt: effect
                .receipt_json
                .as_deref()
                .and_then(|text| serde_json::from_str(text).ok()),
        }
    }

    fn not_applied_error(
        effect: &crate::persistence::linux_execution::LinuxEffectRecord,
        id: &str,
    ) -> Error {
        if effect
            .quiescence_json
            .as_deref()
            .is_some_and(|text| text.contains("deadline"))
        {
            Error::ExecutionDeadlineExceeded {
                operation_id: id.to_string(),
            }
        } else {
            Error::ExecutionNotApplied {
                operation_id: id.to_string(),
            }
        }
    }

    fn blocking(state: &str) -> bool {
        matches!(state, "prepared" | "entered" | "running" | "uncertain")
    }

    fn prove_identity(leaf: &CgroupIdentity) -> Result<serde_json::Value> {
        let proved = cgroup::kill_and_prove(leaf, Duration::from_secs(2))?;
        Ok(quiescence_value(&proved, leaf))
    }

    fn quiescence_value(proved: &Quiescence, leaf: &CgroupIdentity) -> serde_json::Value {
        quiescence_document(
            proved.proven,
            &proved.reason,
            proved.populated,
            &proved.events,
            Some((
                leaf.path.to_str().unwrap_or(""),
                i64::try_from(leaf.dev).unwrap_or(0),
                i64::try_from(leaf.ino).unwrap_or(0),
                leaf.boot_id.as_str(),
            )),
        )
    }

    fn host_receipt(
        request: &LinuxExecutionRequest,
        result: &serde_json::Value,
        leaf: &CgroupIdentity,
    ) -> serde_json::Value {
        host_receipt_parts(request, result, leaf)
    }

    fn host_receipt_parts(
        request: &LinuxExecutionRequest,
        result: &serde_json::Value,
        leaf: &CgroupIdentity,
    ) -> serde_json::Value {
        json!({
            "operationId": request.operation_id().ok(),
            "payloadHash": request.payload_hash().ok(),
            "simulated": false,
            "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
            "disposition": result.get("disposition").and_then(serde_json::Value::as_str).unwrap_or("runtime_failure"),
            "result": result,
            "cgroup": {
                "path": leaf.path,
                "device": leaf.dev,
                "inode": leaf.ino,
                "bootId": leaf.boot_id,
            }
        })
    }

    fn observation_receipt(
        request: &LinuxExecutionRequest,
        start: &crate::persistence::linux_execution::LinuxEffectRecord,
        quiescence: &serde_json::Value,
    ) -> serde_json::Value {
        json!({
            "operationId": request.operation_id().ok(),
            "payloadHash": request.payload_hash().ok(),
            "simulated": false,
            "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
            "disposition": "observation",
            "startOperationId": match &request.operation {
                LinuxOperation::ExecStatus { start_operation_id, .. }
                | LinuxOperation::ExecCancel { start_operation_id, .. } => start_operation_id,
                _ => "",
            },
            "startState": start.state,
            "startEntered": start.entered_at.is_some(),
            "startReceiptRetained": start.receipt_json.is_some(),
            "quiescence": quiescence,
        })
    }

    fn captured_receipt(
        id: &str,
        hash: &str,
        result: &serde_json::Value,
        leaf: &CgroupIdentity,
    ) -> serde_json::Value {
        json!({
            "operationId": id,
            "payloadHash": hash,
            "simulated": false,
            "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
            "disposition": result.get("disposition").and_then(serde_json::Value::as_str).unwrap_or("runtime_failure"),
            "result": result,
            "cgroup": {
                "path": leaf.path,
                "device": leaf.dev,
                "inode": leaf.ino,
                "bootId": leaf.boot_id,
            }
        })
    }

    fn scratch_dir() -> Result<PathBuf> {
        let dir =
            std::env::temp_dir().join(format!("switchyard-linux-{}", crate::domain::new_id("run")));
        fs::create_dir(&dir).map_err(Error::storage)?;
        Ok(dir)
    }

    fn write_launch_files(
        scratch: &Path,
        config: &LinuxExecutionConfig,
        request: &LinuxExecutionRequest,
    ) -> Result<()> {
        fs::write(
            scratch.join("marker"),
            "Git administration is unavailable in this sandbox\n",
        )
        .map_err(Error::storage)?;
        let body = match &request.operation {
            LinuxOperation::ReadFile { path } => json!({"op":"read","path":path}),
            LinuxOperation::ReadFileRange {
                path,
                offset,
                length,
            } => {
                json!({"op":"range","path":path,"offset":offset,"length":length})
            }
            LinuxOperation::ListDirectory { path } => json!({"op":"list","path":path}),
            LinuxOperation::WriteFile { path, content } => {
                json!({"op":"write","path":path,"content":content})
            }
            LinuxOperation::EditFile { path, old, new } => {
                json!({"op":"edit","path":path,"old":old,"new":new})
            }
            LinuxOperation::ExecStart { argv } if argv.as_slice() == ["/helper", "probe"] => {
                config.probe_request.clone().unwrap_or_else(|| json!({}))
            }
            _ => json!({}),
        };
        fs::write(
            scratch.join("request.json"),
            serde_json::to_string(&body).map_err(Error::storage)?,
        )
        .map_err(Error::storage)?;
        fs::write(scratch.join("status.json"), b"").map_err(Error::storage)?;
        Ok(())
    }

    fn spawn_supervisor(
        config: &LinuxExecutionConfig,
        leaf: &CgroupIdentity,
        workspace: &Path,
        scratch: &Path,
        request: &LinuxExecutionRequest,
    ) -> Result<ActiveExec> {
        let mode = match &request.operation {
            LinuxOperation::ExecStart { argv } if argv.as_slice() == ["/helper", "probe"] => {
                "probe"
            }
            LinuxOperation::ExecStart { .. } => "exec",
            _ => "file",
        };
        let mut command = Command::new(&config.helper);
        command
            .arg("supervise")
            .arg("--cgroup")
            .arg(&leaf.path)
            .arg("--bwrap")
            .arg(&config.bubblewrap)
            .arg("--workspace")
            .arg(workspace)
            .arg("--marker")
            .arg(scratch.join("marker"))
            .arg("--status")
            .arg(scratch.join("status.json"))
            .arg("--mode")
            .arg(mode)
            .arg("--deadline-ms")
            .arg(request.deadline_ms.to_string())
            .arg("--helper")
            .arg(&config.helper)
            .arg("--request")
            .arg(scratch.join("request.json"))
            .arg("--result")
            .arg(scratch.join("result.json"));
        if config.fail_launch {
            command.arg("--fail-launch").arg("1");
        }
        if let LinuxOperation::ExecStart { argv } = &request.operation
            && mode == "exec"
        {
            command.arg("--");
            command.args(argv);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Error::guard(format!("cannot spawn linux supervisor: {error}")))?;
        let pid = Pid::from_raw(
            i32::try_from(child.id()).map_err(|_| Error::guard("supervisor pid does not fit"))?,
        )
        .ok_or_else(|| Error::guard("supervisor pid is zero"))?;
        let pidfd = pidfd_open(pid, PidfdFlags::empty()).map_err(|error| {
            Error::guard(format!(
                "cannot obtain a supervisor pidfd before the pid can be reused: {error}"
            ))
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::guard("supervisor stdout is missing"))?;
        let (tx, lines) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        let stdin = child.stdin.take();
        Ok(ActiveExec {
            child,
            stdin,
            lines,
            pidfd,
            scratch: scratch.to_path_buf(),
            cgroup: CgroupIdentity {
                path: leaf.path.clone(),
                dev: leaf.dev,
                ino: leaf.ino,
                boot_id: leaf.boot_id.clone(),
            },
        })
    }

    fn wait_line(lines: &Receiver<String>, timeout: Duration) -> Result<String> {
        lines
            .recv_timeout(timeout)
            .map_err(|_| Error::guard("linux supervisor timed out before reporting its next state"))
    }

    fn remaining(request: &LinuxExecutionRequest) -> Result<Duration> {
        let left = request.deadline_ms.saturating_sub(crate::domain::now_ms());
        if left <= 0 {
            return Err(Error::ExecutionDeadlineExceeded {
                operation_id: request.operation_id()?,
            });
        }
        Ok(Duration::from_millis(u64::try_from(left).unwrap_or(0)))
    }

    fn start_time(pid: u32) -> Result<String> {
        let raw = fs::read_to_string(format!("/proc/{pid}/stat")).map_err(Error::storage)?;
        let (_, rest) = raw
            .rsplit_once(") ")
            .ok_or_else(|| Error::guard("supervisor stat is unreadable"))?;
        let fields: Vec<_> = rest.split_whitespace().collect();
        fields
            .get(19)
            .map(|value| (*value).to_string())
            .ok_or_else(|| Error::guard("supervisor start time is missing"))
    }

    fn require_bwrap(path: &Path) -> Result<()> {
        let output = Command::new(path)
            .arg("--version")
            .env_clear()
            .output()
            .map_err(|error| Error::guard(format!("bubblewrap is unavailable: {error}")))?;
        if !output.status.success()
            || output.stdout != crate::linux_execution::bubblewrap_version_line().as_bytes()
        {
            return Err(Error::guard(format!(
                "reviewed linux backend requires bubblewrap {}; refusing a different supervisor",
                crate::linux_execution::BUBBLEWRAP_VERSION
            )));
        }
        Ok(())
    }
}

#[cfg(all(feature = "linux-execution", target_os = "linux"))]
pub(super) use os::LinuxSession;
