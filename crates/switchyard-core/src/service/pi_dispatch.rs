//! B5 composes the outbox, confined session and Linux effect ledger. Public
//! runtime observations never acquire mission/verification/quiescence authority.
//! Rust cgroup proofs are recorded as `CommandOrigin::HostSupervisor`.
use super::*;
use crate::execution::confined_pi::BRIDGE_WALL;
use crate::execution::confined_runtime::ConfinedRuntime;
use crate::execution::pi_runtime::{mapped, receipt_from_row};
use crate::linux_execution::MAX_DEADLINE_MS;
use crate::{ConfinedPiConfig, ConfinedPiSession, LinuxExecutionConfig, PiEffectOutcome};

/// Absolute budget for one forwarded effect, measured from the callback.
/// Half of [`BRIDGE_WALL`] (the 60-second bridge watchdog and policy
/// `wallSeconds`) so the tool can settle inside that wall instead of racing
/// it. The Linux ledger rejects anything above [`MAX_DEADLINE_MS`], which is
/// the same 60 seconds.
const CONFINED_EFFECT_DEADLINE_MS: i64 = BRIDGE_WALL.as_secs() as i64 * 1_000 / 2;

const _: () = assert!(CONFINED_EFFECT_DEADLINE_MS > 0);
const _: () = assert!(CONFINED_EFFECT_DEADLINE_MS <= MAX_DEADLINE_MS);
const _: () = assert!(CONFINED_EFFECT_DEADLINE_MS * 2 == BRIDGE_WALL.as_secs() as i64 * 1_000);

impl Host {
    /// Explicit credential-free conformance adapter. `fixture` is trusted Host
    /// configuration, never a command/model parameter. All egress stays denied.
    /// The product CLI continues to use FakeRuntime.
    pub fn open_confined_pi(
        db: &Path,
        config: ConfinedPiConfig,
        linux: LinuxExecutionConfig,
        fixture: &str,
    ) -> Result<Self> {
        let db = canonical_db_path(db)?;
        let mut host = Self::open_with_runtime(&db, |_| {
            Ok(RuntimeAdapter::Confined(Box::new(ConfinedRuntime::new(
                config, fixture,
            )?)))
        })?;
        host.enable_linux_execution(linux)?;
        // No bridge is opened until a mission binding is available. Each reopen
        // proves the old bridge empty before workspace recovery or dispatch.
        Ok(host)
    }

    /// Reconcile only an existing intent: reconstruct its frozen request and
    /// settle a retained Host receipt (or proven non-release) without scheduling Pi.
    pub(super) fn reconcile_confined_effect(
        &mut self,
        id: &str,
        effect: &crate::persistence::linux_execution::LinuxEffectRecord,
        proof: &Value,
    ) -> Result<bool> {
        use super::pi_linux_write::text;
        use crate::{LinuxExecutionRequest, LinuxOperation};
        if proof["proven"] != true || self.store.operation_superseded(id)? {
            return Ok(false);
        }
        let absent = effect.released_at.is_none()
            && effect.entered_at.is_none()
            && effect.receipt_json.is_none();
        if effect.receipt_json.is_none() && !absent {
            return Ok(false);
        }
        let stored: Value = serde_json::from_str(&effect.request_json).map_err(Error::storage)?;
        let op = &stored["operation"];
        let operation = match op["method"].as_str() {
            Some("write_file") => LinuxOperation::WriteFile {
                path: text(op, "path")?.into(),
                content: text(op, "content")?.into(),
            },
            Some("exec_start") => LinuxOperation::ExecStart {
                argv: op["argv"]
                    .as_array()
                    .ok_or_else(|| Error::guard("invalid frozen argv"))?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| Error::guard("invalid frozen argument"))
                    })
                    .collect::<Result<Vec<_>>>()?,
            },
            Some("exec_status") => LinuxOperation::ExecStatus {
                start_operation_id: text(op, "startOperationId")?.into(),
                start_payload_hash: text(op, "startPayloadHash")?.into(),
            },
            Some("exec_cancel") => LinuxOperation::ExecCancel {
                start_operation_id: text(op, "startOperationId")?.into(),
                start_payload_hash: text(op, "startPayloadHash")?.into(),
            },
            _ => return Ok(false),
        };
        let binding = RuntimeBinding {
            mission_id: effect.mission_id.clone(),
            attempt_id: effect.attempt_id.clone(),
            session_id: effect.session_id.clone(),
            generation: self.generation(),
        };
        self.accept_runtime_binding(&binding, &effect.mission_id)?;
        let request = LinuxExecutionRequest {
            binding,
            operation,
            tool_task_id: text(&stored, "toolTaskId")?.into(),
            effect_key: text(&stored, "effectKey")?.into(),
            capability_scope: text(&stored, "scope")?.into(),
            deadline_ms: stored["deadlineMs"]
                .as_i64()
                .ok_or_else(|| Error::guard("missing frozen deadline"))?,
        };
        request.validate()?;
        if request.operation_id()? != id || request.payload_hash()? != effect.payload_hash {
            return Err(Error::guard("frozen Linux intent identity changed"));
        }
        if let Some(raw) = &effect.receipt_json {
            let receipt: Value = serde_json::from_str(raw).map_err(Error::storage)?;
            if receipt["backend"] != crate::linux_execution::BUBBLEWRAP_BACKEND {
                return Err(Error::guard("invalid retained Linux backend"));
            }
            self.store.complete_linux(&request, &receipt, proof)?;
        } else {
            self.store.linux_not_applied(&request, proof)?;
        }
        Ok(true)
    }

    fn confined_mut(&mut self) -> Result<&mut ConfinedRuntime> {
        match &mut self.runtime {
            RuntimeAdapter::Confined(runtime) => Ok(runtime),
            _ => Err(Error::guard("Host was not opened with confined Pi")),
        }
    }

    pub(super) fn ensure_confined_session(&mut self, mission_id: &str) -> Result<()> {
        let mission = self.store.load_mission(mission_id)?;
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        if self.confined_mut()?.is_released(&binding.session_id) {
            return Ok(());
        }
        if let Some(session) = self.confined_mut()?.sessions.get(&binding.session_id) {
            if session.binding() != &binding {
                return Err(Error::guard("confined session binding mismatch"));
            }
            return Ok(());
        }
        let initialized = self.store.runtime_initialized(&binding.attempt_id)?;
        let runtime = self.confined_mut()?;
        let opened = ConfinedPiSession::open_required(
            &runtime.config,
            binding.clone(),
            &runtime.fixture,
            initialized,
        );
        let session = match opened {
            Ok(session) => session,
            Err(error) => {
                if !mission.phase.is_terminal() {
                    self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
                }
                return Err(error);
            }
        };
        self.store.save_pi_host(
            &binding.attempt_id,
            binding.generation,
            &session.lifecycle(),
            &Value::Null,
        )?;
        self.store.mark_runtime_initialized(&binding.attempt_id)?;
        self.confined_mut()?
            .sessions
            .insert(binding.session_id, session);
        Ok(())
    }

    fn confined_call(
        &mut self,
        binding: &RuntimeBinding,
        method: &str,
        command: Option<&str>,
        params: Value,
    ) -> Result<Value> {
        self.accept_runtime_binding(binding, &binding.mission_id)?;
        // Move only the session, not the adapter identity, out while the callback
        // borrows Host. On unwind the local session's Drop terminates the bridge;
        // its durable record prevents a same-generation replacement.
        let mut session = self
            .confined_mut()?
            .sessions
            .remove(&binding.session_id)
            .ok_or_else(|| Error::guard("confined session is unavailable"))?;
        self.barrier("bridge-live");
        let result = session.call(binding, method, command, params, &mut |effect| {
            // PiBridge checks the wire binding; each mapper repeats Host fencing.
            if self
                .accept_runtime_binding(binding, &binding.mission_id)
                .is_err()
            {
                return PiEffectOutcome::Rejected;
            }
            let deadline = now_ms().saturating_add(CONFINED_EFFECT_DEADLINE_MS);
            match confined_effect_route(effect["payload"]["method"].as_str()) {
                ConfinedEffectRoute::Write => {
                    self.execute_pi_linux_write(binding, effect, deadline)
                }
                ConfinedEffectRoute::Process => {
                    self.execute_pi_linux_process(binding, effect, deadline)
                }
                ConfinedEffectRoute::Rejected => PiEffectOutcome::Rejected,
            }
        });
        let lifecycle = session.lifecycle();
        self.confined_mut()?
            .sessions
            .insert(binding.session_id.clone(), session);
        let observed = if method == "runtime.inspect" {
            result.as_ref().ok().cloned()
        } else {
            None
        };
        let previous = self.store.pi_host_view(&binding.attempt_id)?;
        let observation = observed.unwrap_or_else(|| {
            previous
                .map(|v| v["observation"].clone())
                .unwrap_or(Value::Null)
        });
        self.store.save_pi_host(
            &binding.attempt_id,
            binding.generation,
            &lifecycle,
            &observation,
        )?;
        if let Err(error) = &result
            && lifecycle["phase"] == "stopped"
        {
            self.terminate_linux_writers(&binding.mission_id)?;
            let mission = self.store.load_mission(&binding.mission_id)?;
            if !mission.phase.is_terminal()
                && !matches!(mission.phase, Phase::Pausing | Phase::Cancelling)
                && !error.is_transient()
            {
                self.uncertainty(&mission, AttentionReason::ExecutionUncertain)?;
            }
        }
        result
    }

    fn confined_inspect(&mut self, binding: &RuntimeBinding, ids: &[&str]) -> Result<Value> {
        self.confined_call(binding, "runtime.inspect", None, json!({"commandIds":ids}))
    }

    fn confined_receipt(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
    ) -> Result<Option<RuntimeReceipt>> {
        crate::execution::PiRuntime::validate_command(request, payload)?;
        let (method, params) = mapped(request, payload)?;
        let snapshot = self.confined_inspect(&request.binding, &[&request.request_id])?;
        let rows = snapshot["commands"]
            .as_array()
            .filter(|r| r.len() == 1)
            .ok_or_else(|| Error::guard("invalid confined command snapshot"))?;
        receipt_from_row(request, method, &params, &rows[0])
    }

    fn confined_send(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
    ) -> Result<RuntimeReceipt> {
        crate::execution::PiRuntime::validate_command(request, payload)?;
        let (method, params) = mapped(request, payload)?;
        self.confined_call(&request.binding, method, Some(&request.request_id), params)?;
        self.barrier("pi-before-ack");
        // Read the public durable receipt, including hash/binding checks, rather
        // than trusting a successful transport return as an admission fact.
        self.confined_receipt(request, payload)?
            .ok_or_else(|| Error::guard("confined command has no settled receipt"))
    }

    fn confined_history(&mut self, mission: &MissionRecord) -> Result<()> {
        let result = self.check_confined_history(mission);
        if let Err(error) = &result
            && !error.is_transient()
            && !mission.phase.is_terminal()
        {
            self.uncertainty(mission, AttentionReason::ExecutionUncertain)?;
        }
        result
    }

    fn check_confined_history(&mut self, mission: &MissionRecord) -> Result<()> {
        let binding = self.binding(mission);
        for command in self
            .store
            .list_commands(&mission.id)?
            .iter()
            .filter(|c| c.state == CommandState::Acknowledged && c.kind.is_runtime_bound())
        {
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id.clone(),
                payload_hash: command.payload_hash.clone(),
                method: runtime_method(command.kind),
            };
            let payload =
                serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
            let receipt = self
                .confined_receipt(&request, &payload)?
                .ok_or_else(|| Error::guard("acknowledged confined command is missing"))?;
            let stored: Value = serde_json::from_str(
                command
                    .runtime_receipt_json
                    .as_deref()
                    .ok_or_else(|| Error::guard("missing Host receipt"))?,
            )
            .map_err(Error::storage)?;
            if stored
                != serde_json::from_str::<Value>(&receipt_json(&receipt, "pi-confined-v1"))
                    .map_err(Error::storage)?
            {
                return Err(Error::guard("Host/Pi receipt disagreement"));
            }
        }
        Ok(())
    }

    pub(super) fn dispatch_confined_work(&mut self, mission_id: &str) -> Result<usize> {
        let count = self.dispatch_confined_work_inner(mission_id)?;
        // Terminal and superseded bridges are proven stopped here. Pause keeps
        // its stopped session so this generation cannot open a replacement.
        self.release_finished_sessions()?;
        Ok(count)
    }

    fn dispatch_confined_work_inner(&mut self, mission_id: &str) -> Result<usize> {
        let mut count = self.dispatch_confined_stops(mission_id)?;
        let mission = self.store.load_mission(mission_id)?;
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        self.store.require_workspace_ready(&binding.attempt_id)?;
        if mission.phase.is_terminal() || mission.phase == Phase::Paused {
            for command in self.store.list_commands(mission_id)? {
                if command.state == CommandState::Pending && is_runtime_work(command.kind) {
                    self.store.supersede_pending(
                        &command.id,
                        "admission closed before runtime accepted this command",
                    )?;
                }
            }
            return Ok(count);
        }
        self.ensure_confined_session(mission_id)?;
        self.confined_history(&mission)?;
        for command in self
            .store
            .list_commands(mission_id)?
            .iter()
            .filter(|c| c.state == CommandState::Pending && c.kind.is_runtime_bound())
        {
            if command.attempt_id != binding.attempt_id {
                return Err(Error::guard("foreign confined outbox command"));
            }
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id.clone(),
                payload_hash: command.payload_hash.clone(),
                method: runtime_method(command.kind),
            };
            let payload =
                serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
            let receipt = match self.confined_receipt(&request, &payload)? {
                Some(receipt) => receipt,
                None => {
                    let current = self.store.load_mission(mission_id)?;
                    if !matches!(current.phase, Phase::Planning | Phase::Implementing) {
                        self.store.supersede_pending(
                            &command.id,
                            "admission closed before runtime accepted this command",
                        )?;
                        continue;
                    }
                    if self.store.ops_blocked(mission_id)? {
                        return Err(Error::guard("unresolved effects block confined dispatch"));
                    }
                    match self.confined_send(&request, &payload) {
                        Ok(receipt) => receipt,
                        Err(error) => {
                            let Some(category) = error.rejected_admission().map(str::to_owned)
                            else {
                                return Err(error);
                            };
                            match self.confined_receipt(&request, &payload)? {
                                Some(receipt) => receipt,
                                None => {
                                    self.store.reject_pending(
                                        &command.id,
                                        &rejection_json(&category, "pi-confined-v1"),
                                    )?;
                                    continue;
                                }
                            }
                        }
                    }
                }
            };
            self.accept_runtime_binding(&binding, mission_id)?;
            self.store
                .acknowledge(&command.id, &receipt_json(&receipt, "pi-confined-v1"))?;
            count += 1;
        }
        // Reopened Pi is inert. Resume only the most recent acknowledged work
        // after the last abort, once the entire outbox and effects are reconciled.
        if !matches!(
            self.store.load_mission(mission_id)?.phase,
            Phase::Planning | Phase::Implementing
        ) {
            return Ok(count);
        }
        let snapshot = self.confined_inspect(&binding, &[])?;
        if snapshot["scheduling"] == "paused" && !self.store.ops_blocked(mission_id)? {
            let commands = self.store.list_commands(mission_id)?;
            if !commands.iter().any(|c| c.state == CommandState::Pending)
                && let Some(command) = commands
                    .iter()
                    .rev()
                    .take_while(|c| {
                        !(c.state == CommandState::Acknowledged
                            && matches!(c.kind, CommandKind::Pause | CommandKind::Cancel))
                    })
                    .find(|c| c.state == CommandState::Acknowledged && is_runtime_work(c.kind))
            {
                let request = RuntimeRequest {
                    binding,
                    request_id: command.request_id.clone(),
                    payload_hash: command.payload_hash.clone(),
                    method: runtime_method(command.kind),
                };
                let payload =
                    serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
                let original = self.confined_receipt(&request, &payload)?;
                if Some(self.confined_send(&request, &payload)?) != original {
                    return Err(Error::guard("confined replay changed receipt"));
                }
            }
        }
        Ok(count)
    }

    pub(super) fn dispatch_confined_stops(&mut self, mission_id: &str) -> Result<usize> {
        let mission = self.store.load_mission(mission_id)?;
        let stops: Vec<_> = self
            .store
            .list_commands(mission_id)?
            .into_iter()
            .filter(|c| {
                c.state == CommandState::Pending
                    && matches!(c.kind, CommandKind::Pause | CommandKind::Cancel)
            })
            .collect();
        if stops.is_empty() {
            return Ok(0);
        }
        self.ensure_confined_session(mission_id)?;
        let binding = self.binding(&mission);
        if self.confined_mut()?.is_released(&binding.session_id) {
            return Err(Error::guard(
                "confined bridge was already released for this generation",
            ));
        }
        self.stop_confined_linux_writers(mission_id)?;
        self.reconcile_execution(mission_id)?;
        self.confined_history(&mission)?;
        let mut count = 0;
        for command in stops {
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id,
                payload_hash: command.payload_hash,
                method: "abort",
            };
            let payload =
                serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
            let receipt = match self.confined_receipt(&request, &payload)? {
                Some(r) => r,
                None => self.confined_send(&request, &payload)?,
            };
            self.store
                .acknowledge(&command.id, &receipt_json(&receipt, "pi-confined-v1"))?;
            count += 1;
        }
        // An abort may follow work whose Pi acknowledgement was lost. Preserve
        // that original receipt before closing the transport; only absent work
        // may be superseded. Never resubmit it merely to discover its receipt.
        for command in self.store.list_commands(mission_id)? {
            if command.state != CommandState::Pending || !is_runtime_work(command.kind) {
                continue;
            }
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id.clone(),
                payload_hash: command.payload_hash.clone(),
                method: runtime_method(command.kind),
            };
            let payload =
                serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
            if let Some(receipt) = self.confined_receipt(&request, &payload)? {
                self.store
                    .acknowledge(&command.id, &receipt_json(&receipt, "pi-confined-v1"))?;
            } else {
                self.store.supersede_pending(
                    &command.id,
                    "admission closed before runtime accepted this command",
                )?;
            }
        }
        let proof = self
            .confined_mut()?
            .sessions
            .get_mut(&binding.session_id)
            .ok_or(Error::NotFound)?
            .stop()?;
        let lifecycle = self
            .confined_mut()?
            .sessions
            .get(&binding.session_id)
            .ok_or(Error::NotFound)?
            .lifecycle();
        self.store.save_pi_host(
            &binding.attempt_id,
            binding.generation,
            &lifecycle,
            &Value::Null,
        )?;
        self.reconcile_execution(mission_id)?;
        if proof["proven"] == true && !self.store.ops_blocked(mission_id)? {
            let current = self.store.load_mission(mission_id)?;
            let body = match current.phase {
                Phase::Pausing => Some(CommandBody::ConfirmStopped),
                Phase::Cancelling => Some(CommandBody::ConfirmCancelled),
                _ => None,
            };
            if let Some(body) = body {
                self.confined_host_fact(&current, body)?;
            }
        }
        // Cancel is terminal, so this drops the proven-stopped bridge. Pause is
        // not, and its stopped session stays in this generation.
        self.release_finished_sessions()?;
        Ok(count)
    }

    pub(super) fn poll_confined(&mut self, mission_id: &str) -> Result<Vec<NotifyOutcome>> {
        let mission = self.store.load_mission(mission_id)?;
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        if !self.store.runtime_initialized(&binding.attempt_id)? {
            return Ok(Vec::new());
        }
        self.ensure_confined_session(mission_id)?;
        if self.confined_mut()?.is_released(&binding.session_id)
            || self
                .confined_mut()?
                .sessions
                .get(&binding.session_id)
                .is_some_and(|session| session.lifecycle()["phase"] == "stopped")
        {
            return Ok(vec![unchanged(&mission)]);
        }
        // Snapshot resync is diagnostic evidence only: never infer verification,
        // plan acceptance or OS quiescence from runtimeIdle or status=done.
        let ids: Vec<String> = self
            .store
            .list_commands(mission_id)?
            .iter()
            .filter(|c| c.kind.is_runtime_bound())
            .map(|c| c.request_id.clone())
            .collect();
        if ids.is_empty() {
            self.confined_inspect(&binding, &[])?;
        }
        for chunk in ids.chunks(32) {
            self.confined_inspect(
                &binding,
                &chunk.iter().map(String::as_str).collect::<Vec<_>>(),
            )?;
        }
        self.reconcile_execution(mission_id)?;
        Ok(vec![unchanged(&self.store.load_mission(mission_id)?)])
    }

    pub(super) fn confined_host_fact(
        &mut self,
        mission: &MissionRecord,
        body: CommandBody,
    ) -> Result<AdmitResult> {
        if !matches!(
            body,
            CommandBody::DetectUncertainty { .. }
                | CommandBody::ConfirmStopped
                | CommandBody::ConfirmCancelled
        ) {
            return Err(Error::guard("unsupported Host-owned fact"));
        }
        self.admit_checked(
            AdmitRequest {
                command_id: None,
                mission_id: mission.id.clone(),
                attempt_id: Some(mission.attempt.id.clone()),
                expected_revision: mission.revision,
                actor: "rust-confined-supervisor".into(),
                origin: CommandOrigin::HostSupervisor,
                body,
            },
            None,
            true,
        )
    }
}

enum ConfinedEffectRoute {
    Write,
    Process,
    Rejected,
}

fn confined_effect_route(method: Option<&str>) -> ConfinedEffectRoute {
    match method {
        Some("file.write") => ConfinedEffectRoute::Write,
        Some("process.start" | "process.status" | "process.cancel") => ConfinedEffectRoute::Process,
        _ => ConfinedEffectRoute::Rejected,
    }
}

#[cfg(test)]
mod tests {
    use super::ConfinedEffectRoute;
    use super::confined_effect_route;

    #[test]
    fn pi_linux_confined_effect_routes_known_methods_and_rejects_the_rest() {
        assert!(matches!(
            confined_effect_route(Some("file.write")),
            ConfinedEffectRoute::Write
        ));
        for method in ["process.start", "process.status", "process.cancel"] {
            assert!(
                matches!(
                    confined_effect_route(Some(method)),
                    ConfinedEffectRoute::Process
                ),
                "{method}"
            );
        }
        for method in [None, Some("file.read"), Some("process.kill"), Some("")] {
            assert!(
                matches!(confined_effect_route(method), ConfinedEffectRoute::Rejected),
                "{method:?}"
            );
        }
    }
}
