use super::*;

#[allow(dead_code)]
pub(super) enum VerifierLaunch {
    /// No verdict. `detail` is `toolchain_unavailable` or `dependencies_unavailable`.
    Unavailable {
        detail: Option<String>,
    },
    Ran(VerifierRun),
}

pub(super) struct VerifierRun {
    pub exit_code: Option<i32>,
    pub duration_ms: u64,
    pub stdout: String,
    pub stderr: String,
    pub stdout_sha256: String,
    pub stderr_sha256: String,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
    pub timed_out: bool,
    pub toolchain_identity: Option<String>,
    pub signaled: bool,
    pub oom_killed: bool,
    pub limit_killed: bool,
    pub result_unreadable: bool,
    pub script_status_missing: bool,
    /// Allowlisted `/etc` paths that existed on the host before the sandbox spawn.
    pub etc_binds: Vec<String>,
    /// Sandbox path `cc` could not resolve. The profile is not widened to cover it.
    pub linker_missing: Option<String>,
}

impl Host {
    pub(super) fn reconcile_linux_mode(
        &mut self,
        mission_id: &str,
        settle_verifier: bool,
    ) -> Result<()> {
        self.hold_unknown_execution_kinds(mission_id)?;
        #[cfg(all(feature = "linux-execution", target_os = "linux"))]
        if self.linux.is_some() {
            return self.reconcile_linux_os(mission_id, settle_verifier);
        }
        let _ = settle_verifier;
        self.note_unproven_linux(mission_id)
    }

    /// Unknown `kind` values are quarantined and never settled or skipped.
    fn hold_unknown_execution_kinds(&mut self, mission_id: &str) -> Result<()> {
        use crate::persistence::linux_execution::{
            ExecutionKind, blocking_linux_state, execution_kind,
        };
        let effects = self.store.linux_mission_effects(mission_id)?;
        for (id, effect) in effects {
            if !blocking_linux_state(&effect.state) {
                continue;
            }
            if execution_kind(&effect.request_json) == ExecutionKind::Unknown
                && effect.state != "uncertain"
            {
                self.store.quarantine_linux(&id)?;
            }
        }
        Ok(())
    }

    pub(super) fn run_host_verifier(
        &mut self,
        mission: &crate::domain::MissionRecord,
        tree: &std::path::Path,
        verifier: &serde_json::Value,
    ) -> Result<VerifierLaunch> {
        #[cfg(all(feature = "linux-execution", target_os = "linux"))]
        if self.linux.is_some() {
            return self.run_host_verifier_os(mission, tree, verifier);
        }
        let _ = (mission, tree, verifier);
        Ok(VerifierLaunch::Unavailable { detail: None })
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
        let preserving_stop = self.runtime.linux_ledger().preserves_admitted_stop
            && matches!(mission.phase, Phase::Pausing | Phase::Cancelling);
        if !mission.phase.is_terminal() && !preserving_stop {
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

    #[cfg(feature = "pi-contract")]
    fn cancelled_writer_receipt(id: &str, payload_hash: &str) -> serde_json::Value {
        json!({
            "operationId": id,
            "payloadHash": payload_hash,
            "simulated": false,
            "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
            "disposition": "cancelled",
            "result": {"disposition": "cancelled"},
        })
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
            if !self.runtime.linux_ledger().admits_effects {
                return Err(Error::guard(
                    "linux execution requires an adapter that admits Linux effects",
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

        #[cfg(feature = "pi-contract")]
        pub(in crate::service) fn linux_operation_live(&self, id: &str) -> bool {
            self.linux
                .as_ref()
                .is_some_and(|s| s.active.contains_key(id))
        }

        /// Every blocking row is a writer this generation still supervises.
        /// An uncertain or unsupervised row makes this false.
        #[cfg(feature = "pi-contract")]
        pub(in crate::service) fn confined_supervised_writers_only(
            &self,
            mission_id: &str,
        ) -> Result<bool> {
            let views = self.store.linux_views(mission_id)?;
            let blocking: Vec<_> = views
                .iter()
                .filter(|view| {
                    !view.superseded
                        && matches!(
                            view.state.as_str(),
                            "prepared" | "entered" | "running" | "uncertain"
                        )
                })
                .collect();
            if blocking.is_empty() {
                return Ok(false);
            }
            Ok(blocking.iter().all(|view| {
                view.method == "exec_start"
                    && view.entered
                    && matches!(view.state.as_str(), "entered" | "running")
                    && view.started_generation == self.generation()
                    && self.linux_operation_live(&view.operation_id)
            }))
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

        fn settle_supervised_effect(
            &mut self,
            id: &str,
            effect: &crate::persistence::linux_execution::LinuxEffectRecord,
            proof: &Value,
        ) -> Result<bool> {
            #[cfg(feature = "pi-contract")]
            {
                self.reconcile_confined_effect(id, effect, proof)
            }
            #[cfg(not(feature = "pi-contract"))]
            {
                // Unreachable today. Only Confined supervises live effects, and
                // that variant exists only with pi-contract.
                let _ = (id, effect, proof);
                Err(Error::guard(
                    "linux ledger profile requires a confined session",
                ))
            }
        }

        pub(super) fn reconcile_linux_os(
            &mut self,
            mission_id: &str,
            settle_verifier: bool,
        ) -> Result<()> {
            let views = self.store.linux_views(mission_id)?;
            for view in &views {
                if self.runtime.linux_ledger().supervises_live_effects {
                    self.poll_active(&view.operation_id)?;
                }
            }
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
                match crate::persistence::linux_execution::execution_kind(&effect.request_json) {
                    crate::persistence::linux_execution::ExecutionKind::Unknown => {
                        if effect.state != "uncertain" {
                            self.store.quarantine_linux(&view.operation_id)?;
                        }
                        continue;
                    }
                    crate::persistence::linux_execution::ExecutionKind::Verifier => {
                        reconcile_verifier_effect(
                            self,
                            &view.operation_id,
                            &effect,
                            settle_verifier,
                        )?;
                        continue;
                    }
                    crate::persistence::linux_execution::ExecutionKind::Writer => {}
                }
                if let Ok(quiescence) = self.prove_effect(&effect) {
                    self.store
                        .save_linux_quiescence(&view.operation_id, &quiescence)?;
                    if self.runtime.linux_ledger().supervises_live_effects
                        && self.settle_supervised_effect(
                            &view.operation_id,
                            &effect,
                            &quiescence,
                        )?
                    {
                        continue;
                    }
                }
                if orphaned_without_receipt(&effect) {
                    self.store.quarantine_linux(&view.operation_id)?;
                }
            }
            let unsettled = self.store.linux_views(mission_id)?;
            let abandoned = unsettled.iter().any(|v| {
                blocking(&v.state)
                    && !v.superseded
                    && !self
                        .linux
                        .as_ref()
                        .is_some_and(|s| s.active.contains_key(&v.operation_id))
            });
            if self.store.linux_execution_blocked(mission_id)?
                && (!self.runtime.linux_ledger().supervises_live_effects || abandoned)
            {
                let mission = self.store.load_mission(mission_id)?;
                self.flag_linux_attention(&mission)?;
            }
            Ok(())
        }

        #[cfg(feature = "pi-contract")]
        pub(in crate::service) fn stop_confined_linux_writers(
            &mut self,
            mission_id: &str,
        ) -> Result<()> {
            let ids: Vec<String> = self
                .linux
                .as_ref()
                .map(|s| s.active.keys().cloned().collect())
                .unwrap_or_default();
            for id in ids {
                let effect = self.store.linux_effect(&id)?.ok_or(Error::NotFound)?;
                if effect.mission_id != mission_id {
                    continue;
                }
                if effect.started_generation != self.generation() {
                    return Err(Error::guard("stale supervised process"));
                }
                self.linux
                    .as_mut()
                    .expect("active Linux session")
                    .active
                    .remove(&id);
                let proof = self.prove_effect(&effect)?;
                self.store.save_linux_quiescence(&id, &proof)?;
                if proof["proven"] == true
                    && effect.entered_at.is_some()
                    && effect.receipt_json.is_none()
                {
                    let receipt = cancelled_writer_receipt(&id, &effect.payload_hash);
                    self.complete_start_cancelled(&id, &effect.payload_hash, &receipt, &proof)?;
                }
            }
            // A host killed between admitting a stop and delivering it is not
            // in `active`. The writer can still be populated. Prove that leaf
            // here, before confirmation, so the pending Continue is not resumed
            // and Pause/Cancel is not confirmed over a live descendant.
            self.settle_unsupervised_writer(mission_id)?;
            Ok(())
        }

        /// Record one cancellation for an entered `exec_start` this host is not
        /// supervising. The current generation can settle immediately. A previous
        /// owner's `started_generation` is rejected by `finish_linux_identity`;
        /// the receipt is retained so the reconcile that follows this stop can
        /// complete it without launching the command again.
        #[cfg(feature = "pi-contract")]
        fn settle_unsupervised_writer(&mut self, mission_id: &str) -> Result<()> {
            let views = self.store.linux_views(mission_id)?;
            for view in views {
                // Uncertain rows were already quarantined. Killing them again
                // during stop delivery would treat a previous owner's orphan as
                // a fresh cancellation. Only entered or running rows are settled.
                if view.method != "exec_start"
                    || view.receipt_retained
                    || view.superseded
                    || !view.entered
                    || !matches!(view.state.as_str(), "entered" | "running")
                {
                    continue;
                }
                if self
                    .linux
                    .as_ref()
                    .is_some_and(|session| session.active.contains_key(&view.operation_id))
                {
                    continue;
                }
                let effect = self
                    .store
                    .linux_effect(&view.operation_id)?
                    .ok_or(Error::NotFound)?;
                if effect.mission_id != mission_id
                    || effect.entered_at.is_none()
                    || effect.receipt_json.is_some()
                {
                    continue;
                }
                let proof = self.prove_effect(&effect)?;
                if proof["proven"] != true {
                    // Missing or mismatched identity never reaches cgroup.kill.
                    // Quarantine instead of failing Pause/Cancel permanently.
                    self.store.quarantine_linux(&view.operation_id)?;
                    let mission = self.store.load_mission(mission_id)?;
                    self.uncertainty(&mission, crate::domain::AttentionReason::ExecutionUncertain)?;
                    continue;
                }
                let receipt = cancelled_writer_receipt(&view.operation_id, &effect.payload_hash);
                self.store
                    .save_linux_quiescence(&view.operation_id, &proof)?;
                if effect.started_generation == self.generation() {
                    self.complete_start_cancelled(
                        &view.operation_id,
                        &effect.payload_hash,
                        &receipt,
                        &proof,
                    )?;
                } else {
                    self.store.remember_linux_receipt(
                        &view.operation_id,
                        &effect.payload_hash,
                        &receipt,
                    )?;
                }
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

        fn planning_tool_effect(operation: &LinuxOperation) -> crate::execution::ToolEffect {
            match operation {
                LinuxOperation::ReadFile { .. }
                | LinuxOperation::ReadFileRange { .. }
                | LinuxOperation::ListDirectory { .. } => crate::execution::ToolEffect::Read,
                LinuxOperation::WriteFile { .. }
                | LinuxOperation::EditFile { .. }
                | LinuxOperation::ExecStart { .. }
                // Status and cancel return before this function. ExecCancel is
                // a write-class effect, so a direct call does not treat it as a read.
                | LinuxOperation::ExecCancel { .. } => crate::execution::ToolEffect::Write,
                LinuxOperation::ExecStatus { .. } => crate::execution::ToolEffect::Read,
            }
        }

        pub(in crate::service) fn authorize_new(
            &self,
            request: &LinuxExecutionRequest,
        ) -> Result<()> {
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
            } else if mission.phase == Phase::Planning {
                self.runtime
                    .admit_planning_tool(Self::planning_tool_effect(&request.operation))?;
                return Err(Error::guard(
                    "new linux execution requires an implementing mission",
                ));
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
            let mut start = self.store.linux_effect(start_id)?.unwrap_or(start);
            // No supervisor is tracked for this start: the Host restarted, or
            // poll_active already dropped a finished child whose result.json
            // was missing or unparseable and wrote no receipt. Record that
            // uncertainty. Status only observes; it must not kill.
            let supervised = self
                .linux
                .as_ref()
                .is_some_and(|session| session.active.contains_key(start_id));
            if !supervised && blocking(&start.state) && orphaned_without_receipt(&start) {
                self.store.quarantine_linux(start_id)?;
                let mission = self.store.load_mission(&start.mission_id)?;
                self.flag_linux_attention(&mission)?;
                start = self.store.linux_effect(start_id)?.ok_or(Error::NotFound)?;
            }
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
            self.barrier("result");
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
            let intent: Value =
                serde_json::from_str(&start.request_json).map_err(Error::storage)?;
            if intent["operation"]["method"] != "exec_start"
                || self.store.operation_superseded(start_id)?
            {
                return Err(Error::guard(
                    "status/cancel requires a non-superseded process start",
                ));
            }
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

        pub(in crate::service) fn barrier(&self, stage: &str) {
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

        pub(super) fn run_host_verifier_os(
            &mut self,
            mission: &crate::domain::MissionRecord,
            tree: &Path,
            verifier: &serde_json::Value,
        ) -> Result<super::VerifierLaunch> {
            drive_host_verifier(self, mission, tree, verifier)
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

    /// Entered or released, and still missing a final receipt. Callers also
    /// require a blocking state and no live supervisor.
    fn orphaned_without_receipt(
        effect: &crate::persistence::linux_execution::LinuxEffectRecord,
    ) -> bool {
        (effect.released_at.is_some() || effect.entered_at.is_some())
            && effect.receipt_json.is_none()
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
            "startResult": start.receipt_json.as_deref().and_then(process_result),
            "quiescence": quiescence,
        })
    }

    // Keep an observation safely below the 64 KiB bridge frame even when
    // both streams contain control characters that expand during JSON encoding.
    fn process_result(text: &str) -> Option<Value> {
        let receipt: Value = serde_json::from_str(text).ok()?;
        let result = &receipt["result"];
        let bounded = |key: &str| {
            let value = result[key].as_str().unwrap_or("");
            let mut end = value.len().min(2048);
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            (value[..end].to_string(), end < value.len())
        };
        let (stdout, stdout_cut) = bounded("stdout");
        let (stderr, stderr_cut) = bounded("stderr");
        Some(json!({"receiptHash": payload_hash(&receipt).ok()?,
            "disposition":receipt["disposition"], "exitCode":result["exitCode"],
            "stdout":stdout, "stderr":stderr,
            "stdoutTruncated":stdout_cut || result["stdoutTruncated"] == true,
            "stderrTruncated":stderr_cut || result["stderrTruncated"] == true}))
    }

    #[test]
    fn pi_linux_process_observation_bounds_utf8_and_json_expansion() {
        let receipt = json!({"disposition":"exited","result":{"exitCode":3,
            "stdout": "é".repeat(4000), "stderr": "\u{0}".repeat(4000)}});
        let result = process_result(&receipt.to_string()).unwrap();
        assert_eq!(result["stdout"].as_str().unwrap().len(), 2048);
        assert_eq!(result["stderr"].as_str().unwrap().len(), 2048);
        assert_eq!(result["stdoutTruncated"], true);
        assert_eq!(result["stderrTruncated"], true);
        assert_eq!(result["receiptHash"], payload_hash(&receipt).unwrap());
        assert!(serde_json::to_vec(&result).unwrap().len() < 32 * 1024);
    }

    #[test]
    fn pi_linux_status_refuses_a_file_write_start() {
        assert_status_or_cancel_refused(false, false);
    }

    #[test]
    fn pi_linux_cancel_refuses_a_file_write_start() {
        assert_status_or_cancel_refused(true, false);
    }

    #[test]
    fn pi_linux_status_refuses_a_superseded_start() {
        assert_status_or_cancel_refused(false, true);
    }

    #[test]
    fn pi_linux_cancel_refuses_a_superseded_start() {
        assert_status_or_cancel_refused(true, true);
    }

    /// Status and cancel must refuse a file write or a superseded start before
    /// any cgroup kill or a new effect row. The leaf is populated so a cancel
    /// that skipped the `matching_start` guard would write `cgroup.kill`.
    #[cfg(test)]
    fn assert_status_or_cancel_refused(cancel: bool, superseded: bool) {
        use std::os::unix::fs::MetadataExt;

        let dir = tempfile::tempdir().unwrap();
        let repo = dir.path().join("repo");
        fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "--initial-branch=main"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
            &["commit", "--allow-empty", "-m", "base"],
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let db = dir.path().join("mission.sqlite");
        let mut host = Host::open(&db).unwrap();
        let created = host
            .create_mission(CreateMission {
                repo,
                issue_number: 97,
                issue_title: "process guard".into(),
                objective: "refuse".into(),
                acceptance: "no kill".into(),
                actor: "operator".into(),
                max_repairs: 1,
                max_plan_revisions: 1,
            })
            .unwrap();
        let workspaces = dir.path().join("workspaces");
        fs::create_dir(&workspaces).unwrap();
        host.prepare_workspace(&created.mission_id, &created.attempt_id, &workspaces)
            .unwrap();
        let admit = |host: &mut Host, body: crate::CommandBody| {
            let revision = host.snapshot(&created.mission_id).unwrap().revision;
            host.admit(AdmitRequest {
                command_id: None,
                mission_id: created.mission_id.clone(),
                attempt_id: None,
                expected_revision: revision,
                actor: "operator".into(),
                origin: CommandOrigin::Operator,
                body,
            })
            .unwrap();
        };
        admit(&mut host, crate::CommandBody::StartPlanning);
        host.dispatch_pending(&created.mission_id).unwrap();
        host.apply_notification(
            &created.mission_id,
            RuntimeNotification::PlanCompleted {
                plan: "plan".into(),
                quiescent: true,
            },
        )
        .unwrap();
        let digest = host
            .snapshot(&created.mission_id)
            .unwrap()
            .plan_digest
            .unwrap();
        admit(
            &mut host,
            crate::CommandBody::ApprovePlan {
                plan_digest: digest,
            },
        );
        let mission = host.store.load_mission(&created.mission_id).unwrap();
        assert_eq!(mission.phase, Phase::Implementing);
        let binding = host.binding(&mission);

        let root = dir.path().join("cgroup");
        let leaf = root.join("leaf");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("cgroup.events"), "populated 1\n").unwrap();
        let leaf = fs::canonicalize(&leaf).unwrap();
        let meta = fs::metadata(&leaf).unwrap();
        let boot_id = fs::read_to_string("/proc/sys/kernel/random/boot_id").unwrap();
        let start_id = "op-v1:guard-target";
        let start_hash = "sha256:guard-target";
        let operation = if superseded {
            json!({"method":"exec_start","argv":["/bin/echo","hello"]})
        } else {
            json!({"method":"write_file","path":"out.txt","content":"once"})
        };
        let scope = if superseded {
            crate::linux_execution::EXEC_START_SCOPE
        } else {
            crate::linux_execution::WRITE_SCOPE
        };
        let request_json = json!({
            "operation": operation,
            "scope": scope,
            "deadlineMs": 1,
            "toolTaskId": "task",
            "effectKey": "target"
        });
        let inserted = rusqlite::Connection::open(&db)
            .unwrap()
            .execute(
                "INSERT INTO linux_execution_effects(
                    operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                    started_generation, state, cgroup_path, cgroup_dev, cgroup_ino, boot_id,
                    entered_at, created_at
                 ) VALUES (?1,?2,?3,?4,?5,?6,?7,'running',?8,?9,?10,?11,1,1)",
                rusqlite::params![
                    start_id,
                    binding.mission_id,
                    binding.attempt_id,
                    binding.session_id,
                    start_hash,
                    request_json.to_string(),
                    i64::try_from(binding.generation).unwrap(),
                    leaf.to_str().unwrap(),
                    i64::try_from(meta.dev()).unwrap(),
                    i64::try_from(meta.ino()).unwrap(),
                    boot_id.trim(),
                ],
            )
            .unwrap();
        assert_eq!(inserted, 1);
        if superseded {
            let inserted = rusqlite::Connection::open(&db)
                .unwrap()
                .execute(
                    "INSERT INTO execution_supersessions(
                        command_id, operation_id, journal, abandonment_command_id,
                        request_hash, audit_json
                     ) VALUES ('supersede-target', ?1, 'linux', 'abandon-target', 'sha256:supersede', '{}')",
                    [start_id],
                )
                .unwrap();
            assert_eq!(inserted, 1);
        }
        assert_eq!(
            host.store.operation_superseded(start_id).unwrap(),
            superseded
        );
        let recorded = host
            .store
            .linux_views(&binding.mission_id)
            .unwrap()
            .into_iter()
            .find(|view| view.operation_id == start_id)
            .unwrap();
        assert_eq!(
            recorded.method,
            if superseded {
                "exec_start"
            } else {
                "write_file"
            }
        );
        assert_eq!(recorded.superseded, superseded);
        assert_eq!(recorded.state, "running");

        // The guard returns before this root is used. It is installed so a
        // removed guard would kill this populated leaf instead of refusing.
        host.linux = Some(LinuxSession::new(LinuxExecutionConfig {
            cgroup_root: fs::canonicalize(&root).unwrap(),
            bubblewrap: PathBuf::from("/nonexistent-bwrap"),
            helper: PathBuf::from("/nonexistent-helper"),
            crash_at: None,
            probe_request: None,
            fail_launch: false,
        }));
        let probe = if cancel {
            LinuxOperation::ExecCancel {
                start_operation_id: start_id.to_string(),
                start_payload_hash: start_hash.to_string(),
            }
        } else {
            LinuxOperation::ExecStatus {
                start_operation_id: start_id.to_string(),
                start_payload_hash: start_hash.to_string(),
            }
        };
        let request = LinuxExecutionRequest {
            binding,
            tool_task_id: "task".into(),
            effect_key: if cancel { "cancel" } else { "status" }.into(),
            capability_scope: probe.scope().into(),
            deadline_ms: crate::domain::now_ms() + 5_000,
            operation: probe,
        };
        let probe_id = request.operation_id().unwrap();
        let error = host.execute_linux_operation(&request).unwrap_err();
        assert_eq!(error.category(), "guard_failed", "{error}");
        assert!(
            error
                .to_string()
                .contains("status/cancel requires a non-superseded process start"),
            "{error}"
        );
        assert!(host.store.linux_effect(&probe_id).unwrap().is_none());
        let start = host.store.linux_effect(start_id).unwrap().unwrap();
        assert_eq!(start.state, "running");
        assert!(start.receipt_json.is_none());
        assert!(start.quiescence_json.is_none());
        assert!(!leaf.join("cgroup.kill").exists());
        assert_eq!(
            fs::read_to_string(leaf.join("cgroup.events")).unwrap(),
            "populated 1\n"
        );
        let mut names = fs::read_dir(&leaf)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, vec!["cgroup.events".to_string()]);
    }

    #[cfg(feature = "pi-contract")]
    #[test]
    fn pi_linux_fresh_status_quarantines_orphan_without_rewriting_history() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mission.sqlite");
        let (mut host, binding) = implementing_process_host(dir.path());

        let operation = LinuxOperation::ExecStart {
            argv: vec!["/bin/true".into()],
        };
        let start = LinuxExecutionRequest {
            binding: binding.clone(),
            tool_task_id: "start".into(),
            effect_key: "start".into(),
            capability_scope: operation.scope().into(),
            deadline_ms: crate::domain::now_ms() + 5000,
            operation,
        };
        let start_id = start.operation_id().unwrap();
        // Ledger transition only: release and entry are committed without a
        // cgroup identity. The confined suite covers the live cgroup path.
        record_running_without_cgroup(&mut host, &start);
        let operation = LinuxOperation::ExecStatus {
            start_operation_id: start_id.clone(),
            start_payload_hash: start.payload_hash().unwrap(),
        };
        let historical = LinuxExecutionRequest {
            binding: binding.clone(),
            tool_task_id: "observer".into(),
            effect_key: "before".into(),
            capability_scope: operation.scope().into(),
            deadline_ms: start.deadline_ms,
            operation,
        };
        let row = host.store.linux_effect(&start_id).unwrap().unwrap();
        let receipt = observation_receipt(&historical, &row, &json!({"proven":false}));
        host.store.prepare_linux(&historical, None).unwrap();
        host.store
            .complete_linux(&historical, &receipt, &json!({"proven":true}))
            .unwrap();
        drop(host);

        let mut host = Host::open(&db).unwrap();
        let mut binding = binding;
        binding.generation = host.generation();
        // Deliberately unusable executables: status must never relaunch or kill.
        host.linux = Some(LinuxSession::new(LinuxExecutionConfig {
            cgroup_root: dir.path().into(),
            bubblewrap: "/nonexistent-bwrap".into(),
            helper: "/nonexistent-helper".into(),
            crash_at: None,
            probe_request: None,
            fail_launch: false,
        }));
        let payload = json!({"method":"process.status","startOperationId":start_id,
            "startPayloadHash":start.payload_hash().unwrap(),"capability":"workspace-process","deadlinePolicy":"attempt-bound"});
        let frame = |key: &str| {
            json!({"operationId":operation_id(&binding.attempt_id,&binding.session_id,"observer",key).unwrap(),
            "taskId":"observer","effectKey":key,"payloadHash":payload_hash(&payload).unwrap(),"payload":payload})
        };
        // Historical replay alone must neither reconcile the target nor change its receipt.
        assert_eq!(
            host.execute_pi_linux_process(&binding, &frame("before"), 0),
            crate::PiEffectOutcome::Completed(receipt.clone())
        );
        assert_eq!(
            host.store.linux_effect(&start_id).unwrap().unwrap().state,
            "running"
        );
        for key in ["after", "later"] {
            let out = host.execute_pi_linux_process(
                &binding,
                &frame(key),
                crate::domain::now_ms() + 5000,
            );
            let crate::PiEffectOutcome::Completed(observed) = out else {
                panic!("{out:?}")
            };
            assert_eq!(observed["startState"], "uncertain");
            assert_eq!(observed["startReceiptRetained"], false);
            assert_eq!(observed["startResult"], Value::Null);
            assert_eq!(observed["quiescence"]["proven"], false);
        }
        let snapshot = host.snapshot(&binding.mission_id).unwrap();
        assert_eq!(snapshot.phase, Phase::NeedsAttention);
        assert!(snapshot.execution_blocked);
        assert_eq!(
            host.execute_pi_linux_process(&binding, &frame("before"), 0),
            crate::PiEffectOutcome::Completed(receipt)
        );
    }

    #[cfg(all(test, feature = "pi-contract"))]
    #[test]
    fn pi_linux_same_host_status_quarantines_finished_child_without_result() {
        for case in ["missing", "unparseable"] {
            let dir = tempfile::tempdir().unwrap();
            let (mut host, binding) = implementing_process_host(dir.path());
            let operation = LinuxOperation::ExecStart {
                argv: vec!["/bin/true".into()],
            };
            let start = LinuxExecutionRequest {
                binding: binding.clone(),
                tool_task_id: "start".into(),
                effect_key: case.into(),
                capability_scope: operation.scope().into(),
                deadline_ms: crate::domain::now_ms() + 5000,
                operation,
            };
            let start_id = start.operation_id().unwrap();
            // Ledger transition only, same as the restart test. This case keeps
            // the Host and drops a finished child with no readable result.
            record_running_without_cgroup(&mut host, &start);
            host.linux = Some(LinuxSession::new(LinuxExecutionConfig {
                cgroup_root: dir.path().into(),
                bubblewrap: "/nonexistent-bwrap".into(),
                helper: "/nonexistent-helper".into(),
                crash_at: None,
                probe_request: None,
                fail_launch: false,
            }));
            let scratch = dir.path().join("scratch");
            fs::create_dir(&scratch).unwrap();
            if case == "unparseable" {
                fs::write(scratch.join("result.json"), b"{").unwrap();
            }
            host.linux
                .as_mut()
                .unwrap()
                .active
                .insert(start_id.clone(), exited_active(scratch));
            assert!(host.linux_operation_live(&start_id), "{case}");
            let payload = json!({"method":"process.status","startOperationId":start_id,
                "startPayloadHash":start.payload_hash().unwrap(),"capability":"workspace-process","deadlinePolicy":"attempt-bound"});
            let frame = |key: &str| {
                json!({"operationId":operation_id(&binding.attempt_id,&binding.session_id,"observer",key).unwrap(),
                "taskId":"observer","effectKey":key,"payloadHash":payload_hash(&payload).unwrap(),"payload":payload})
            };
            for key in ["after-exit", "later"] {
                let out = host.execute_pi_linux_process(
                    &binding,
                    &frame(key),
                    crate::domain::now_ms() + 5000,
                );
                let crate::PiEffectOutcome::Completed(observed) = out else {
                    panic!("{case}: {out:?}")
                };
                assert_eq!(observed["startState"], "uncertain", "{case}");
                assert_eq!(observed["startReceiptRetained"], false, "{case}");
                assert_eq!(observed["startResult"], Value::Null, "{case}");
            }
            let effect = host.store.linux_effect(&start_id).unwrap().unwrap();
            assert_eq!(effect.state, "uncertain", "{case}");
            assert!(effect.receipt_json.is_none(), "{case}");
            assert!(!host.linux_operation_live(&start_id), "{case}");
            let snapshot = host.snapshot(&binding.mission_id).unwrap();
            assert_eq!(snapshot.phase, Phase::NeedsAttention, "{case}");
            assert!(snapshot.execution_blocked, "{case}");
        }
    }

    /// Release and entry without a cgroup. Callers cover the ledger transition;
    /// the confined suite covers the live cgroup path.
    #[cfg(all(test, feature = "pi-contract"))]
    fn record_running_without_cgroup(host: &mut Host, start: &LinuxExecutionRequest) {
        host.store.prepare_linux(start, None).unwrap();
        host.store.mark_linux_released(start, 1, "1").unwrap();
        host.store.mark_linux_entered(start, 1, "1").unwrap();
        host.store.mark_linux_running(start).unwrap();
    }

    #[cfg(all(test, feature = "pi-contract"))]
    fn implementing_process_host(dir: &Path) -> (Host, RuntimeBinding) {
        let repo = dir.join("repo");
        fs::create_dir(&repo).unwrap();
        for args in [
            &["init", "--initial-branch=main"][..],
            &["config", "user.email", "test@example.com"],
            &["config", "user.name", "Test"],
            &["commit", "--allow-empty", "-m", "base"],
        ] {
            let output = std::process::Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let mut host = Host::open(&dir.join("mission.sqlite")).unwrap();
        let created = host
            .create_mission(CreateMission {
                repo,
                issue_number: 97,
                issue_title: "process guard".into(),
                objective: "refuse".into(),
                acceptance: "no kill".into(),
                actor: "operator".into(),
                max_repairs: 1,
                max_plan_revisions: 1,
            })
            .unwrap();
        let workspaces = dir.join("workspaces");
        fs::create_dir(&workspaces).unwrap();
        host.prepare_workspace(&created.mission_id, &created.attempt_id, &workspaces)
            .unwrap();
        let admit = |host: &mut Host, body: crate::CommandBody| {
            let revision = host.snapshot(&created.mission_id).unwrap().revision;
            host.admit(AdmitRequest {
                command_id: None,
                mission_id: created.mission_id.clone(),
                attempt_id: None,
                expected_revision: revision,
                actor: "operator".into(),
                origin: CommandOrigin::Operator,
                body,
            })
            .unwrap();
        };
        admit(&mut host, crate::CommandBody::StartPlanning);
        host.dispatch_pending(&created.mission_id).unwrap();
        host.apply_notification(
            &created.mission_id,
            RuntimeNotification::PlanCompleted {
                plan: "plan".into(),
                quiescent: true,
            },
        )
        .unwrap();
        let digest = host
            .snapshot(&created.mission_id)
            .unwrap()
            .plan_digest
            .unwrap();
        admit(
            &mut host,
            crate::CommandBody::ApprovePlan {
                plan_digest: digest,
            },
        );
        let mission = host.store.load_mission(&created.mission_id).unwrap();
        assert_eq!(mission.phase, Phase::Implementing);
        let binding = host.binding(&mission);
        (host, binding)
    }

    #[cfg(all(test, feature = "pi-contract"))]
    fn exited_active(scratch: PathBuf) -> ActiveExec {
        let mut child = Command::new("/bin/true")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let raw = i32::try_from(child.id()).unwrap();
        let pid = Pid::from_raw(raw).unwrap();
        let pidfd = pidfd_open(pid, PidfdFlags::empty()).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            let zombie = fs::read_to_string(format!("/proc/{}/stat", child.id()))
                .ok()
                .and_then(|stat| {
                    stat.rsplit_once(") ")
                        .map(|(_, rest)| rest.starts_with('Z'))
                })
                .unwrap_or(false);
            if zombie {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let (tx, lines) = mpsc::channel();
        tx.send("DONE".to_string()).unwrap();
        let stdin = child.stdin.take();
        drop(child.stdout.take());
        ActiveExec {
            child,
            stdin,
            lines,
            pidfd,
            scratch: scratch.clone(),
            cgroup: CgroupIdentity {
                path: scratch.join("missing-leaf"),
                dev: 1,
                ino: 1,
                boot_id: "test-boot".into(),
            },
        }
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

    fn reconcile_verifier_effect(
        host: &mut Host,
        id: &str,
        effect: &crate::persistence::linux_execution::LinuxEffectRecord,
        settle_verifier: bool,
    ) -> Result<()> {
        // An abandoned in-flight run is marked uncertain on the first empty
        // proof so the next `mission verify` stays `execution_blocked`.
        // `mission recover` settles it once that proof is empty. A run that
        // never entered is `not_applied` on the first empty proof.
        let proof = match host.prove_effect(effect) {
            Ok(proof) => proof,
            Err(_) => {
                if effect.state != "uncertain" {
                    host.store.quarantine_linux(id)?;
                }
                return Ok(());
            }
        };
        host.store.save_linux_quiescence(id, &proof)?;
        let proven = proof.get("proven").and_then(serde_json::Value::as_bool) == Some(true);
        if !proven {
            if effect.state != "uncertain" {
                host.store.quarantine_linux(id)?;
            }
            return Ok(());
        }
        if effect.entered_at.is_none() && effect.receipt_json.is_none() {
            host.store.verifier_not_applied(id, &proof)?;
            return Ok(());
        }
        if effect.state != "uncertain" && effect.receipt_json.is_none() {
            host.store.quarantine_linux(id)?;
            if !settle_verifier {
                return Ok(());
            }
        }
        let receipt = if let Some(text) = &effect.receipt_json {
            serde_json::from_str(text).map_err(Error::storage)?
        } else {
            json!({
                "operationId": id,
                "payloadHash": effect.payload_hash,
                "simulated": false,
                "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
                "disposition": "cancelled",
                "kind": "verifier",
            })
        };
        host.store.complete_verifier_effect(id, &receipt, &proof)
    }

    fn drive_host_verifier(
        host: &mut Host,
        mission: &crate::domain::MissionRecord,
        tree: &Path,
        verifier: &serde_json::Value,
    ) -> Result<super::VerifierLaunch> {
        let config = host
            .linux
            .as_ref()
            .ok_or_else(|| Error::guard("linux execution backend is not enabled on this host"))?
            .config
            .clone();
        if require_bwrap(&config.bubblewrap).is_err()
            || cgroup::validate_delegation(&config.cgroup_root).is_err()
        {
            return Ok(super::VerifierLaunch::Unavailable { detail: None });
        }
        let rustup_override = test_rustup_home(host);
        let layout = match toolchain_layout(tree, rustup_override.as_deref()) {
            Ok(layout) => layout,
            Err(error) if error.category() == "verifier_confinement_unavailable" => {
                return Ok(super::VerifierLaunch::Unavailable {
                    detail: Some("toolchain_unavailable".into()),
                });
            }
            Err(error) => return Err(error),
        };
        if cgroup::git_file_is_regular(tree).is_err() {
            return Ok(super::VerifierLaunch::Unavailable { detail: None });
        }
        let window_ms = verifier_window_ms(verifier, test_deadline_ms(host));
        let deadline_ms = crate::domain::now_ms().saturating_add(window_ms);
        let cpu_seconds = test_cpu_seconds(host)
            .unwrap_or_else(|| u64::try_from((window_ms.max(1) + 999) / 1000).unwrap_or(1));
        let effect_key = format!("verify-{}-{}", mission.revision, crate::domain::now_ms());
        let operation_id = crate::execution::operation_id(
            &mission.attempt.id,
            &mission.attempt.session_id,
            "switchyard-host-verifier",
            &effect_key,
        )?;
        let request = json!({
            "argv": ["./scripts/rust-verify.sh"],
            "deadlineMs": deadline_ms,
            "kind": "verifier",
        });
        let request_json = crate::canonical::canonicalize(&request)?;
        let payload_hash = crate::canonical::payload_hash(&request)?;
        let identity = crate::persistence::linux_execution::LinuxEffectIdentity {
            operation_id: &operation_id,
            payload_hash: &payload_hash,
            generation: host.generation(),
            mission_id: &mission.id,
            attempt_id: &mission.attempt.id,
            session_id: &mission.attempt.session_id,
        };
        let leaf =
            cgroup::create_verifier_leaf(&config.cgroup_root, &cgroup::leaf_name(&operation_id)?)?;
        if !cgroup::within_root(&config.cgroup_root, &leaf.path) {
            discard_leaf(&leaf);
            return Err(Error::guard("cgroup leaf escaped the delegated root"));
        }
        let stored = StoredCgroup {
            path: leaf.path.display().to_string(),
            dev: i64::try_from(leaf.dev)
                .map_err(|_| Error::guard("cgroup device id does not fit"))?,
            ino: i64::try_from(leaf.ino).map_err(|_| Error::guard("cgroup inode does not fit"))?,
            boot_id: leaf.boot_id.clone(),
        };
        let prepared = if fail_prepare_requested(host) {
            Err(Error::guard("injected prepare failure"))
        } else {
            host.store
                .prepare_verifier_effect(&identity, &request_json, &stored)
        };
        if let Err(error) = prepared {
            discard_leaf(&leaf);
            return Err(error);
        }
        let toolchain_identity = match host_toolchain_identity(&layout.toolchain) {
            Ok(identity_hash) => identity_hash,
            Err(_) => {
                let _ = fail_verifier_launch(host, &operation_id);
                return Ok(super::VerifierLaunch::Unavailable {
                    detail: Some("toolchain_unavailable".into()),
                });
            }
        };
        if let Some(missing) = linker_gap(host, &layout.toolchain) {
            let _ = fail_verifier_launch(host, &operation_id);
            return Ok(super::VerifierLaunch::Unavailable {
                detail: Some(format!("toolchain_unavailable: {missing}")),
            });
        }
        let keep_target = test_target_dir(host).is_some();
        let target = match verifier_target(tree, test_target_dir(host).as_deref()) {
            Ok(target) => target,
            Err(error) => {
                fail_verifier_launch(host, &operation_id)?;
                return Err(error);
            }
        };
        let scratch = match scratch_dir() {
            Ok(scratch) => scratch,
            Err(error) => {
                fail_verifier_launch(host, &operation_id)?;
                if !keep_target {
                    let _ = fs::remove_dir_all(&target);
                }
                return Err(error);
            }
        };
        if let Err(error) = prepare_verifier_scratch(&scratch) {
            let _ = fail_verifier_launch(host, &operation_id);
            release_verifier_dirs(&target, &scratch, keep_target);
            return Err(error);
        }
        if let Err(reason) = run_preflight(
            host,
            &config,
            &layout,
            &leaf,
            tree,
            &scratch,
            &target,
            &operation_id,
        ) {
            let _ = fail_verifier_launch(host, &operation_id);
            release_verifier_dirs(&target, &scratch, keep_target);
            return Ok(super::VerifierLaunch::Unavailable {
                detail: Some(reason),
            });
        }
        let started = std::time::Instant::now();
        let driven = drive_verifier(
            host,
            &config,
            &layout,
            &leaf,
            &identity,
            tree,
            &scratch,
            &target,
            deadline_ms,
            cpu_seconds,
            started,
            &toolchain_identity,
        );
        release_verifier_dirs(&target, &scratch, keep_target);
        driven
    }

    fn release_verifier_dirs(target: &Path, scratch: &Path, keep_target: bool) {
        if !keep_target {
            let _ = fs::remove_dir_all(target);
        }
        let _ = fs::remove_dir_all(scratch);
    }

    fn fail_verifier_launch(host: &mut Host, operation_id: &str) -> Result<()> {
        let Some(effect) = host.store.linux_effect(operation_id)? else {
            return Ok(());
        };
        reconcile_verifier_effect(host, operation_id, &effect, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn drive_verifier(
        host: &mut Host,
        config: &LinuxExecutionConfig,
        layout: &ToolchainLayout,
        leaf: &CgroupIdentity,
        identity: &crate::persistence::linux_execution::LinuxEffectIdentity,
        tree: &Path,
        scratch: &Path,
        target: &Path,
        deadline_ms: i64,
        cpu_seconds: u64,
        started: std::time::Instant,
        toolchain_identity: &str,
    ) -> Result<super::VerifierLaunch> {
        let etc_present = crate::linux_execution::present_etc_binds(Path::new("/"));
        let etc_binds = CheckedEtcBinds {
            linker_missing: crate::linux_execution::unresolved_sandbox_cc(
                Path::new("/"),
                &layout.toolchain,
                &etc_present,
            ),
            present: etc_present,
        };
        let prelude = "exec ./scripts/rust-verify.sh\n";
        let mut proc = match spawn_verifier_supervisor(
            config,
            layout,
            leaf,
            tree,
            scratch,
            target,
            deadline_ms,
            cpu_seconds,
            prelude,
            omit_linker(host),
        ) {
            Ok(proc) => proc,
            Err(error) => {
                fail_verifier_launch(host, identity.operation_id)?;
                return Err(error);
            }
        };
        let ready = match wait_line(&proc.lines, Duration::from_secs(10)) {
            Ok(line) => line,
            Err(error) => {
                drop(proc);
                fail_verifier_launch(host, identity.operation_id)?;
                return Err(error);
            }
        };
        if ready != "READY" {
            drop(proc);
            fail_verifier_launch(host, identity.operation_id)?;
            return Err(Error::guard(format!(
                "verifier supervisor was not ready: {ready}"
            )));
        }
        let start = match start_time(proc.child.id()) {
            Ok(start) => start,
            Err(error) => {
                drop(proc);
                fail_verifier_launch(host, identity.operation_id)?;
                return Err(error);
            }
        };
        if let Err(error) = host
            .store
            .mark_verifier_released(identity, proc.child.id(), &start)
        {
            drop(proc);
            fail_verifier_launch(host, identity.operation_id)?;
            return Err(error);
        }
        if let Some(stdin) = proc.stdin.as_mut() {
            if let Err(error) = writeln!(stdin, "GO").and_then(|()| stdin.flush()) {
                drop(proc);
                fail_verifier_launch(host, identity.operation_id)?;
                return Err(Error::storage(error));
            }
        } else {
            drop(proc);
            fail_verifier_launch(host, identity.operation_id)?;
            return Err(Error::guard("missing verifier supervisor stdin"));
        }
        let mut saw_entered = false;
        let wait = Duration::from_millis(
            u64::try_from((deadline_ms - crate::domain::now_ms()).max(1) + 30_000)
                .unwrap_or(30_000),
        );
        loop {
            let line = match wait_line(&proc.lines, wait) {
                Ok(line) => line,
                Err(_) => {
                    drop(proc);
                    return host.finish_verifier_timeout(
                        identity,
                        target,
                        started,
                        &etc_binds,
                        toolchain_identity,
                    );
                }
            };
            match line.as_str() {
                "ENTERED" if !saw_entered => {
                    let start = start_time(proc.child.id()).unwrap_or_else(|_| start.clone());
                    if let Err(error) = host
                        .store
                        .mark_verifier_entered(identity, proc.child.id(), &start)
                        .and_then(|()| host.store.mark_verifier_running(identity))
                    {
                        drop(proc);
                        let _ = fail_verifier_launch(host, identity.operation_id);
                        return Err(Error::ExecutionBlocked {
                            mission_id: identity.mission_id.to_string(),
                            detail: format!(
                                "verifier entry could not be recorded ({error}); mission recover"
                            ),
                        });
                    }
                    saw_entered = true;
                    host.barrier("verifier-running");
                }
                "DONE" => {
                    drop(proc);
                    return host.finish_verifier_result(
                        identity,
                        leaf,
                        scratch,
                        target,
                        started,
                        saw_entered,
                        &etc_binds,
                        toolchain_identity,
                    );
                }
                "ENTERED" => {}
                other => {
                    drop(proc);
                    fail_verifier_launch(host, identity.operation_id)?;
                    return Err(Error::guard(format!(
                        "verifier supervisor reported {other}"
                    )));
                }
            }
        }
    }

    struct ToolchainLayout {
        cargo_home: PathBuf,
        rustup_home: PathBuf,
        toolchain: PathBuf,
    }

    fn toolchain_layout(
        workspace: &Path,
        rustup_override: Option<&Path>,
    ) -> Result<ToolchainLayout> {
        let text = crate::regular_file::read_regular_nofollow(
            &workspace.join("rust-toolchain.toml"),
            64 * 1024,
        )
        .map_err(|_| Error::VerifierConfinementUnavailable)?;
        let text = String::from_utf8(text).map_err(|_| Error::VerifierConfinementUnavailable)?;
        let channel = crate::linux_execution::toolchain_channel(&text)
            .ok_or(Error::VerifierConfinementUnavailable)?;
        let rustup_home = match rustup_override {
            Some(path) if path.is_dir() => path.to_path_buf(),
            Some(_) => return Err(Error::VerifierConfinementUnavailable),
            None => {
                env_dir("RUSTUP_HOME", ".rustup").ok_or(Error::VerifierConfinementUnavailable)?
            }
        };
        let cargo_home =
            env_dir("CARGO_HOME", ".cargo").ok_or(Error::VerifierConfinementUnavailable)?;
        let toolchain =
            find_toolchain(&rustup_home, channel).ok_or(Error::VerifierConfinementUnavailable)?;
        Ok(ToolchainLayout {
            cargo_home,
            rustup_home,
            toolchain,
        })
    }

    fn env_dir(key: &str, fallback: &str) -> Option<PathBuf> {
        if let Some(value) = std::env::var_os(key) {
            let path = PathBuf::from(value);
            return path.is_dir().then_some(path);
        }
        let home = std::env::var_os("HOME")?;
        let path = PathBuf::from(home).join(fallback);
        path.is_dir().then_some(path)
    }

    fn find_toolchain(rustup_home: &Path, channel: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(rustup_home.join("toolchains")).ok()?;
        for entry in entries {
            let Ok(entry) = entry else {
                continue;
            };
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let matches = name == channel
                || name
                    .strip_prefix(channel)
                    .is_some_and(|rest| rest.starts_with('-'));
            if !matches {
                continue;
            }
            let path = entry.path();
            if fs::File::open(path.join("bin/rustc")).is_ok()
                && fs::File::open(path.join("bin/cargo")).is_ok()
            {
                return Some(path);
            }
        }
        None
    }

    fn verifier_window_ms(verifier: &serde_json::Value, override_ms: Option<i64>) -> i64 {
        if let Some(ms) = override_ms.filter(|ms| *ms > 0) {
            return ms;
        }
        let seconds = verifier
            .get("timeoutSeconds")
            .and_then(serde_json::Value::as_i64)
            .filter(|seconds| *seconds > 0)
            .unwrap_or(1800);
        seconds.saturating_mul(1000)
    }

    fn verifier_target(tree: &Path, forced: Option<&Path>) -> Result<PathBuf> {
        let path = if let Some(path) = forced {
            fs::create_dir_all(path).map_err(Error::storage)?;
            path.to_path_buf()
        } else {
            let path = std::env::temp_dir().join(format!(
                "switchyard-verify-target-{}",
                crate::domain::new_id("t")
            ));
            fs::create_dir(&path).map_err(Error::storage)?;
            path
        };
        let tree = fs::canonicalize(tree).map_err(Error::storage)?;
        let dir = fs::canonicalize(&path).map_err(Error::storage)?;
        if dir.starts_with(&tree) {
            return Err(Error::VerifierConfinementUnavailable);
        }
        Ok(dir)
    }

    fn test_cpu_seconds(host: &Host) -> Option<u64> {
        #[cfg(test)]
        {
            host.verifier_cpu_seconds
        }
        #[cfg(not(test))]
        {
            let _ = host;
            None
        }
    }

    fn test_deadline_ms(host: &Host) -> Option<i64> {
        #[cfg(test)]
        {
            host.verifier_deadline_ms
        }
        #[cfg(not(test))]
        {
            let _ = host;
            None
        }
    }

    fn test_rustup_home(host: &Host) -> Option<PathBuf> {
        #[cfg(test)]
        {
            host.verifier_rustup_home.clone()
        }
        #[cfg(not(test))]
        {
            let _ = host;
            None
        }
    }

    fn test_target_dir(host: &Host) -> Option<PathBuf> {
        #[cfg(test)]
        {
            host.verifier_target_dir.clone()
        }
        #[cfg(not(test))]
        {
            let _ = host;
            None
        }
    }

    struct VerifierProc {
        child: Child,
        stdin: Option<ChildStdin>,
        lines: Receiver<String>,
        pidfd: OwnedFd,
        cgroup: CgroupIdentity,
    }

    impl Drop for VerifierProc {
        fn drop(&mut self) {
            self.stdin.take();
            let started = std::time::Instant::now();
            loop {
                match self.child.try_wait() {
                    Ok(Some(_)) | Err(_) => break,
                    Ok(None) if started.elapsed() > Duration::from_secs(2) => {
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

    #[allow(clippy::too_many_arguments)]
    fn spawn_verifier_supervisor(
        config: &LinuxExecutionConfig,
        layout: &ToolchainLayout,
        leaf: &CgroupIdentity,
        workspace: &Path,
        scratch: &Path,
        target: &Path,
        deadline_ms: i64,
        cpu_seconds: u64,
        prelude: &str,
        omit_linker: bool,
    ) -> Result<VerifierProc> {
        let mut command = Command::new(&config.helper);
        if omit_linker {
            command.env("SWITCHYARD_OMIT_LINKER", "1");
        } else {
            command.env_remove("SWITCHYARD_OMIT_LINKER");
        }
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
            .arg("verifier")
            .arg("--deadline-ms")
            .arg(deadline_ms.to_string())
            .arg("--helper")
            .arg(&config.helper)
            .arg("--request")
            .arg(scratch.join("result.json"))
            .arg("--result")
            .arg(scratch.join("result.json"))
            .arg("--cargo-home")
            .arg(&layout.cargo_home)
            .arg("--rustup-home")
            .arg(&layout.rustup_home)
            .arg("--toolchain")
            .arg(&layout.toolchain)
            .arg("--target-dir")
            .arg(target)
            .arg("--cpu-seconds")
            .arg(cpu_seconds.to_string())
            .arg("--")
            .arg("/bin/sh")
            .arg("-c")
            .arg(prelude);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| Error::guard(format!("cannot spawn verifier supervisor: {error}")))?;
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
        Ok(VerifierProc {
            child,
            stdin,
            lines,
            pidfd,
            cgroup: CgroupIdentity {
                path: leaf.path.clone(),
                dev: leaf.dev,
                ino: leaf.ino,
                boot_id: leaf.boot_id.clone(),
            },
        })
    }

    impl Host {
        fn finish_verifier_timeout(
            &mut self,
            identity: &crate::persistence::linux_execution::LinuxEffectIdentity,
            _target: &Path,
            started: std::time::Instant,
            etc_binds: &CheckedEtcBinds,
            toolchain_identity: &str,
        ) -> Result<super::VerifierLaunch> {
            let effect = self
                .store
                .linux_effect(identity.operation_id)?
                .ok_or(Error::NotFound)?;
            let proof = self.prove_effect(&effect)?;
            let proven = proof.get("proven").and_then(serde_json::Value::as_bool) == Some(true);
            if !proven || effect.entered_at.is_none() {
                if effect.state != "uncertain" {
                    self.store.quarantine_linux(identity.operation_id)?;
                }
                if !proven {
                    return Err(Error::ExecutionBlocked {
                        mission_id: identity.mission_id.to_string(),
                        detail: "verifier cgroup was not proven empty".into(),
                    });
                }
                self.store
                    .verifier_not_applied(identity.operation_id, &proof)?;
                return Ok(super::VerifierLaunch::Unavailable { detail: None });
            }
            self.complete_verifier_capture(
                identity,
                &proof,
                "deadline",
                None,
                "",
                "",
                "",
                "",
                started,
                etc_binds,
                &RunFlags {
                    timed_out: true,
                    stdout_truncated: false,
                    stderr_truncated: false,
                    signaled: false,
                    oom_killed: false,
                    limit_killed: false,
                    result_unreadable: false,
                    script_status_missing: false,
                    toolchain_identity: Some(toolchain_identity.to_string()),
                },
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn finish_verifier_result(
            &mut self,
            identity: &crate::persistence::linux_execution::LinuxEffectIdentity,
            leaf: &CgroupIdentity,
            scratch: &Path,
            _target: &Path,
            started: std::time::Instant,
            saw_entered: bool,
            etc_binds: &CheckedEtcBinds,
            toolchain_identity: &str,
        ) -> Result<super::VerifierLaunch> {
            #[cfg(test)]
            if let Some(plant) = self.verifier_result_plant.take() {
                plant_result(&scratch.join("result.json"), plant);
            }
            let parsed = read_verifier_result(&scratch.join("result.json"));
            let proved = cgroup::kill_and_prove(leaf, Duration::from_secs(5))?;
            let proof = quiescence_value(&proved, leaf);
            self.store
                .save_linux_quiescence(identity.operation_id, &proof)?;
            if !proved.proven {
                self.store.quarantine_linux(identity.operation_id)?;
                return Err(Error::ExecutionBlocked {
                    mission_id: identity.mission_id.to_string(),
                    detail: "verifier cgroup was not proven empty".into(),
                });
            }
            if !saw_entered {
                self.store
                    .verifier_not_applied(identity.operation_id, &proof)?;
                return Ok(super::VerifierLaunch::Unavailable { detail: None });
            }
            let Ok(result) = parsed else {
                return self.complete_verifier_capture(
                    identity,
                    &proof,
                    "unreadable",
                    None,
                    "",
                    "",
                    "",
                    "",
                    started,
                    etc_binds,
                    &RunFlags {
                        timed_out: false,
                        stdout_truncated: false,
                        stderr_truncated: false,
                        signaled: false,
                        oom_killed: false,
                        limit_killed: false,
                        result_unreadable: true,
                        script_status_missing: true,
                        toolchain_identity: Some(toolchain_identity.to_string()),
                    },
                );
            };
            let disposition = result
                .get("disposition")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("runtime_failure");
            let script_status = result
                .get("scriptStatus")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            let script_signaled = result
                .get("scriptSignaled")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            let exit_code = if script_signaled {
                None
            } else {
                result.get("scriptExit").and_then(serde_json::Value::as_i64)
            };
            let (stdout, stderr) = match (
                decode_tail(&result, "stdoutB64"),
                decode_tail(&result, "stderrB64"),
            ) {
                (Ok(stdout), Ok(stderr)) => (stdout, stderr),
                _ => {
                    return self.complete_verifier_capture(
                        identity,
                        &proof,
                        "unreadable",
                        None,
                        "",
                        "",
                        "",
                        "",
                        started,
                        etc_binds,
                        &RunFlags {
                            timed_out: disposition == "deadline",
                            stdout_truncated: false,
                            stderr_truncated: false,
                            signaled: false,
                            oom_killed: false,
                            limit_killed: false,
                            result_unreadable: true,
                            script_status_missing: true,
                            toolchain_identity: Some(toolchain_identity.to_string()),
                        },
                    );
                }
            };
            let stdout_sha = result
                .get("stdoutSha256")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let stderr_sha = result
                .get("stderrSha256")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let stdout_truncated = result
                .get("stdoutTruncated")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            let stderr_truncated = result
                .get("stderrTruncated")
                .and_then(serde_json::Value::as_bool)
                == Some(true);
            let binds = binds_from_result(&result, etc_binds);
            let oom_killed = result
                .get("oomKills")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|count| count > 0);
            let limit_killed = result
                .get("pidsMaxEvents")
                .and_then(serde_json::Value::as_u64)
                .is_some_and(|count| count > 0)
                || disposition == "limit";
            let signaled = script_signaled;
            self.complete_verifier_capture(
                identity,
                &proof,
                disposition,
                exit_code,
                &stdout,
                &stderr,
                stdout_sha,
                stderr_sha,
                started,
                &binds,
                &RunFlags {
                    timed_out: disposition == "deadline",
                    stdout_truncated,
                    stderr_truncated,
                    signaled,
                    oom_killed,
                    limit_killed,
                    result_unreadable: false,
                    script_status_missing: !script_status,
                    toolchain_identity: Some(toolchain_identity.to_string()),
                },
            )
        }

        #[allow(clippy::too_many_arguments)]
        fn complete_verifier_capture(
            &mut self,
            identity: &crate::persistence::linux_execution::LinuxEffectIdentity,
            proof: &serde_json::Value,
            disposition: &str,
            exit_code: Option<i64>,
            stdout: &str,
            stderr: &str,
            stdout_sha256: &str,
            stderr_sha256: &str,
            started: std::time::Instant,
            etc_binds: &CheckedEtcBinds,
            flags: &RunFlags,
        ) -> Result<super::VerifierLaunch> {
            let receipt = json!({
                "operationId": identity.operation_id,
                "payloadHash": identity.payload_hash,
                "simulated": false,
                "backend": crate::linux_execution::BUBBLEWRAP_BACKEND,
                "disposition": disposition,
                "kind": "verifier",
                "exitCode": exit_code,
            });
            self.store
                .complete_verifier_effect(identity.operation_id, &receipt, proof)?;
            if flags.toolchain_identity.as_deref().unwrap_or("").is_empty() {
                return Ok(super::VerifierLaunch::Unavailable {
                    detail: Some("toolchain_unavailable".into()),
                });
            }
            let duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
            Ok(super::VerifierLaunch::Ran(super::VerifierRun {
                exit_code: exit_code.and_then(|code| i32::try_from(code).ok()),
                duration_ms,
                stdout: stdout.to_string(),
                stderr: stderr.to_string(),
                stdout_sha256: stdout_sha256.to_string(),
                stderr_sha256: stderr_sha256.to_string(),
                stdout_truncated: flags.stdout_truncated,
                stderr_truncated: flags.stderr_truncated,
                timed_out: flags.timed_out,
                toolchain_identity: flags.toolchain_identity.clone(),
                signaled: flags.signaled,
                oom_killed: flags.oom_killed,
                limit_killed: flags.limit_killed,
                result_unreadable: flags.result_unreadable,
                script_status_missing: flags.script_status_missing,
                etc_binds: etc_binds.present.clone(),
                linker_missing: etc_binds.linker_missing.clone(),
            }))
        }
    }

    struct RunFlags {
        timed_out: bool,
        stdout_truncated: bool,
        stderr_truncated: bool,
        signaled: bool,
        oom_killed: bool,
        limit_killed: bool,
        result_unreadable: bool,
        script_status_missing: bool,
        toolchain_identity: Option<String>,
    }

    struct CheckedEtcBinds {
        present: Vec<String>,
        linker_missing: Option<String>,
    }

    fn binds_from_result(
        result: &serde_json::Value,
        fallback: &CheckedEtcBinds,
    ) -> CheckedEtcBinds {
        let Some(items) = result.get("etcBinds").and_then(|value| value.as_array()) else {
            return CheckedEtcBinds {
                present: allowlisted_etc(fallback.present.iter().map(String::as_str)),
                linker_missing: fallback.linker_missing.clone(),
            };
        };
        CheckedEtcBinds {
            present: allowlisted_etc(items.iter().filter_map(|item| item.as_str())),
            linker_missing: result
                .get("linkerMissing")
                .and_then(|value| value.as_str())
                .map(str::to_string),
        }
    }

    fn allowlisted_etc<'a>(paths: impl IntoIterator<Item = &'a str>) -> Vec<String> {
        paths
            .into_iter()
            .filter(|path| crate::linux_execution::VERIFIER_ETC_ALLOWLIST.contains(path))
            .map(str::to_string)
            .collect()
    }

    fn fail_prepare_requested(host: &Host) -> bool {
        #[cfg(test)]
        {
            host.fail_prepare_verifier
        }
        #[cfg(not(test))]
        {
            let _ = host;
            false
        }
    }

    fn discard_leaf(leaf: &CgroupIdentity) {
        let _ = cgroup::kill_and_prove(leaf, Duration::from_secs(2));
        let _ = fs::remove_dir(&leaf.path);
    }

    fn host_toolchain_identity(toolchain: &Path) -> Result<String> {
        let rustc = Command::new(toolchain.join("bin/rustc"))
            .arg("-Vv")
            .output()
            .map_err(|_| {
                Error::VerifierConfinementUnavailable.with_context("toolchain_unavailable")
            })?;
        let cargo = Command::new(toolchain.join("bin/cargo"))
            .arg("-V")
            .output()
            .map_err(|_| {
                Error::VerifierConfinementUnavailable.with_context("toolchain_unavailable")
            })?;
        if !rustc.status.success() || !cargo.status.success() {
            return Err(Error::VerifierConfinementUnavailable.with_context("toolchain_unavailable"));
        }
        let rustc = String::from_utf8(rustc.stdout).map_err(|_| {
            Error::VerifierConfinementUnavailable.with_context("toolchain_unavailable")
        })?;
        let cargo = String::from_utf8(cargo.stdout).map_err(|_| {
            Error::VerifierConfinementUnavailable.with_context("toolchain_unavailable")
        })?;
        if rustc.trim().is_empty() || cargo.trim().is_empty() {
            return Err(Error::VerifierConfinementUnavailable.with_context("toolchain_unavailable"));
        }
        Ok(toolchain_identity_hash(&rustc, &cargo))
    }

    fn linker_gap(host: &Host, toolchain: &Path) -> Option<String> {
        let mut present = crate::linux_execution::present_etc_binds(Path::new("/"));
        if omit_linker(host) {
            present.retain(|path| path != "/etc/alternatives");
        }
        crate::linux_execution::unresolved_sandbox_cc(Path::new("/"), toolchain, &present)
    }

    fn prepare_verifier_scratch(scratch: &Path) -> Result<()> {
        fs::write(
            scratch.join("marker"),
            "Git administration is unavailable in this sandbox\n",
        )
        .map_err(Error::storage)?;
        fs::write(scratch.join("status.json"), b"").map_err(Error::storage)?;
        fs::write(scratch.join("result.json"), b"").map_err(Error::storage)?;
        Ok(())
    }

    fn result_cap() -> usize {
        crate::linux_execution::verifier_result_cap()
    }

    fn read_verifier_result(path: &Path) -> Result<serde_json::Value> {
        let bytes = crate::regular_file::read_regular_nofollow(path, result_cap() + 1)?;
        if bytes.len() > result_cap() {
            return Err(Error::guard("verifier result exceeds the read cap"));
        }
        let text =
            String::from_utf8(bytes).map_err(|_| Error::guard("verifier result is not utf-8"))?;
        if text.trim().is_empty() {
            return Err(Error::guard("verifier result is empty"));
        }
        serde_json::from_str(&text).map_err(|_| Error::guard("verifier result is not json"))
    }

    #[cfg(test)]
    fn plant_result(path: &Path, plant: super::super::VerifierResultPlant) {
        let _ = fs::remove_file(path);
        match plant {
            super::super::VerifierResultPlant::Symlink => {
                let secret = path.with_file_name("result-secret");
                let _ = fs::write(&secret, "SECRET-RESULT");
                let _ = std::os::unix::fs::symlink(&secret, path);
            }
            super::super::VerifierResultPlant::Fifo => {
                let _ = Command::new("mkfifo").arg(path).status();
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn run_preflight(
        host: &mut Host,
        config: &LinuxExecutionConfig,
        layout: &ToolchainLayout,
        leaf: &CgroupIdentity,
        tree: &Path,
        scratch: &Path,
        target: &Path,
        operation_id: &str,
    ) -> std::result::Result<(), String> {
        if let Err(error) = write_probe(target, break_probe(host)) {
            let _ = fail_verifier_launch(host, operation_id);
            return Err(error);
        }
        let probe = "cd /cargo-target/probe && CARGO_TARGET_DIR=/tmp/switchyard-probe-target cargo build --offline --locked\n";
        if !stage_ok(config, layout, leaf, tree, scratch, target, probe, host) {
            let _ = fail_verifier_launch(host, operation_id);
            return Err("toolchain_unavailable".into());
        }
        let metadata = "cargo metadata --locked --offline --format-version 1 --manifest-path /workspace/Cargo.toml >/tmp/switchyard-metadata.json\n";
        if !stage_ok(config, layout, leaf, tree, scratch, target, metadata, host) {
            let _ = fail_verifier_launch(host, operation_id);
            return Err("dependencies_unavailable".into());
        }
        Ok(())
    }

    fn write_probe(target: &Path, broken: bool) -> std::result::Result<(), String> {
        let probe = target.join("probe");
        fs::create_dir_all(probe.join("src")).map_err(|error| error.to_string())?;
        fs::write(
            probe.join("Cargo.toml"),
            "[package]\nname = \"switchyard-verifier-probe\"\nversion = \"0.0.0\"\nedition = \"2021\"\n",
        )
        .map_err(|error| error.to_string())?;
        let source = if broken {
            "fn main( {\n"
        } else {
            "fn main() {}\n"
        };
        fs::write(probe.join("src/main.rs"), source).map_err(|error| error.to_string())?;
        fs::write(
            probe.join("Cargo.lock"),
            "# This file is automatically @generated by Cargo.\n# It is not intended for manual editing.\nversion = 4\n\n[[package]]\nname = \"switchyard-verifier-probe\"\nversion = \"0.0.0\"\n",
        )
        .map_err(|error| error.to_string())?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn stage_ok(
        config: &LinuxExecutionConfig,
        layout: &ToolchainLayout,
        leaf: &CgroupIdentity,
        tree: &Path,
        scratch: &Path,
        target: &Path,
        script: &str,
        host: &Host,
    ) -> bool {
        let _ = fs::write(scratch.join("status.json"), b"");
        let _ = fs::write(scratch.join("result.json"), b"");
        let deadline = crate::domain::now_ms().saturating_add(120_000);
        let Ok(mut proc) = spawn_verifier_supervisor(
            config,
            layout,
            leaf,
            tree,
            scratch,
            target,
            deadline,
            120,
            script,
            omit_linker(host),
        ) else {
            return false;
        };
        if wait_line(&proc.lines, Duration::from_secs(10))
            .ok()
            .as_deref()
            != Some("READY")
        {
            return false;
        }
        let Some(stdin) = proc.stdin.as_mut() else {
            return false;
        };
        if writeln!(stdin, "GO").and_then(|()| stdin.flush()).is_err() {
            return false;
        }
        let mut done = false;
        let wait_until = std::time::Instant::now() + Duration::from_secs(120);
        while std::time::Instant::now() < wait_until {
            let remaining = wait_until.saturating_duration_since(std::time::Instant::now());
            match wait_line(&proc.lines, remaining).ok() {
                Some(line) if line == "DONE" => {
                    done = true;
                    break;
                }
                Some(_) => {}
                None => break,
            }
        }
        drop(proc);
        if !done {
            return false;
        }
        let Ok(result) = read_verifier_result(&scratch.join("result.json")) else {
            return false;
        };
        result
            .get("scriptStatus")
            .and_then(serde_json::Value::as_bool)
            == Some(true)
            && result
                .get("scriptSignaled")
                .and_then(serde_json::Value::as_bool)
                != Some(true)
            && result.get("scriptExit").and_then(serde_json::Value::as_i64) == Some(0)
    }

    fn decode_tail(result: &serde_json::Value, key: &str) -> Result<String> {
        let Some(text) = result.get(key).and_then(serde_json::Value::as_str) else {
            return Err(Error::guard("verifier tail is missing"));
        };
        let bytes = crate::linux_execution::b64_decode(text)
            .ok_or_else(|| Error::guard("verifier tail is not base64"))?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    fn break_probe(host: &Host) -> bool {
        #[cfg(test)]
        {
            host.verifier_break_probe
        }
        #[cfg(not(test))]
        {
            let _ = host;
            false
        }
    }

    fn omit_linker(host: &Host) -> bool {
        #[cfg(test)]
        {
            host.verifier_omit_linker
        }
        #[cfg(not(test))]
        {
            let _ = host;
            false
        }
    }

    fn toolchain_identity_hash(rustc: &str, cargo: &str) -> String {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(rustc.as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(cargo.as_bytes());
        format!("sha256:{}", crate::canonical::sha256_hex(&bytes))
    }
} // mod os

#[cfg(all(feature = "linux-execution", target_os = "linux"))]
pub(super) use os::LinuxSession;
