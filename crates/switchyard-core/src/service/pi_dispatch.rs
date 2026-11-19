//! B5 composes the outbox, confined session and Linux effect ledger. Public
//! runtime observations never acquire mission/verification/quiescence authority.
//! Rust cgroup proofs are recorded as `CommandOrigin::HostSupervisor`.
use std::path::Path;
use std::time::Duration;

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
    /// configuration, never a command/model parameter. Egress stays denied
    /// unless `config.model_endpoint` names one operator endpoint. The product
    /// CLI stays on FakeRuntime unless `--runtime confined-pi` is passed.
    /// That path sets an endpoint only from `--model-endpoint`, and only when
    /// the value is a loopback IP literal pinned before launch. Omitting the
    /// flag keeps egress denied.
    pub fn open_confined_pi(
        db: &Path,
        config: ConfinedPiConfig,
        linux: LinuxExecutionConfig,
        fixture: &str,
    ) -> Result<Self> {
        Self::open_confined(db, config, linux, fixture, false)
    }

    /// Same as [`Self::open_confined_pi`], plus the `busy-once` stop-admission
    /// fixture. Compiled only with the `test-fixtures` feature, which the
    /// confinement probe enables from its dev-dependency. `cfg(test)` is not
    /// set on this library when that probe builds it.
    #[cfg(feature = "test-fixtures")]
    pub fn open_confined_test_fixture(
        db: &Path,
        config: ConfinedPiConfig,
        linux: LinuxExecutionConfig,
        fixture: &str,
    ) -> Result<Self> {
        Self::open_confined(db, config, linux, fixture, true)
    }

    fn open_confined(
        db: &Path,
        config: ConfinedPiConfig,
        linux: LinuxExecutionConfig,
        fixture: &str,
        test_fixtures: bool,
    ) -> Result<Self> {
        let db = canonical_db_path(db)?;
        let mut host = Self::open_with_runtime(&db, |_| {
            let runtime = if !test_fixtures {
                ConfinedRuntime::new(config, fixture)?
            } else {
                #[cfg(any(test, feature = "test-fixtures"))]
                {
                    ConfinedRuntime::new_for_harness(config, fixture)?
                }
                #[cfg(not(any(test, feature = "test-fixtures")))]
                {
                    // Unreachable. Only `open_confined_test_fixture` passes true,
                    // and that method exists only with `test-fixtures`.
                    let _ = (config, fixture);
                    return Err(Error::guard("busy-once is not a production fixture"));
                }
            };
            Ok(RuntimeAdapter::Confined(Box::new(runtime)))
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

    pub(super) fn confined_mut(&mut self) -> Result<&mut ConfinedRuntime> {
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
        // Frozen generation wall when the attempt has one. Direct session
        // opens keep the 60s conformance watchdog. No product-mode flag.
        // TODO(#217): the product fixture should keep this wall. Do not add a
        // product-mode flag here; the freeze is the gate.
        // Cancel and pause do not parse budgets. A pre-#218 freeze has no
        // budget keys; absence is the old wall, not an error.
        let wall = if matches!(mission.phase, Phase::Cancelling | Phase::Pausing) {
            BRIDGE_WALL
        } else {
            self.frozen_budgets(mission_id)
                .ok()
                .flatten()
                .and_then(|budgets| budgets.bridge_wall)
                .unwrap_or(BRIDGE_WALL)
        };
        let model_request_cap = crate::budget::MAX_MODEL_REQUESTS;
        let digest = self.confined_mut()?.runtime_digest.clone();
        let runtime = self.confined_mut()?;
        let test_fixtures = runtime.allow_test_fixtures();
        let opened = ConfinedPiSession::open_required(
            &runtime.config,
            binding.clone(),
            &runtime.fixture,
            initialized,
            Some(&digest),
            test_fixtures,
            crate::execution::confined_pi::OpenLimits {
                wall,
                model_request_cap,
            },
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
        self.note_bridge_up(&binding.attempt_id)?;
        let mut lifecycle = session.lifecycle();
        self.stamp_attempt_clock(&mut lifecycle);
        self.store.save_pi_host(
            &binding.attempt_id,
            binding.generation,
            &lifecycle,
            &Value::Null,
        )?;
        self.store.mark_runtime_initialized(&binding.attempt_id)?;
        self.confined_mut()?
            .sessions
            .insert(binding.session_id.clone(), session);
        // The previous generation was just proved dead by `rekill_and_prove`
        // inside `open`. An unsettled submission still in its sqlite belongs
        // to that generation. This process has not submitted yet, so the
        // count is not this generation's own turn and it is not a timeout.
        self.note_unsettled_dead_generation(mission_id)?;
        Ok(())
    }

    /// `recovered.proven` is the fresh cgroup proof of the generation that
    /// just died. A submission still open there is `unsettled_turn`. The way
    /// out is cancel, then `confirm_cancelled`, on that same stop path.
    fn note_unsettled_dead_generation(&mut self, mission_id: &str) -> Result<()> {
        let mission = self.store.load_mission(mission_id)?;
        if mission.phase.is_terminal()
            || matches!(
                mission.phase,
                Phase::Cancelling | Phase::Pausing | Phase::NeedsAttention
            )
        {
            return Ok(());
        }
        let binding = self.binding(&mission);
        if !self.recovered_dead_generation(&binding)? {
            return Ok(());
        }
        let ids: Vec<String> = self
            .store
            .list_commands(mission_id)?
            .into_iter()
            .filter(|command| command.kind.is_runtime_bound())
            .map(|command| command.request_id)
            .take(32)
            .collect();
        let id_refs: Vec<&str> = ids.iter().map(String::as_str).collect();
        // One signal: `pi.live.run` on the pi.live document. Inspect failure
        // is fail-closed in this process; a later ensure would not re-check.
        let snapshot = match self.confined_inspect(&binding, &id_refs) {
            Ok(snapshot) => snapshot,
            Err(_) => {
                let mission = self.store.load_mission(mission_id)?;
                self.admit_unsettled_turn(&mission, "unreadable")?;
                return Ok(());
            }
        };
        let Some(turn) = snapshot["liveRun"].as_str().filter(|turn| !turn.is_empty()) else {
            return Ok(());
        };
        // `pi.live.run` survives SIGKILL because endRun never ran. Crash
        // recovery still has to replay the admitted submission and finish a
        // tool that already captured its receipt; flagging here blocks that
        // and the mission never goes idle. The stuck case is a tool that is
        // still entered, running, or uncertain and has no receipt: that run
        // cannot finish, and the next submit would be protocol `busy`.
        let stuck_tool = self.store.linux_views(mission_id)?.iter().any(|view| {
            !view.superseded
                && !view.receipt_retained
                && matches!(view.state.as_str(), "entered" | "running" | "uncertain")
        });
        if !stuck_tool {
            return Ok(());
        }
        let mission = self.store.load_mission(mission_id)?;
        self.admit_unsettled_turn(&mission, turn)?;
        Ok(())
    }

    /// The open proved a previous generation's cgroup empty. That generation
    /// is dead. `pi.live.run` from it is not cleared here.
    fn recovered_dead_generation(&mut self, binding: &RuntimeBinding) -> Result<bool> {
        let recovered = self
            .confined_mut()?
            .sessions
            .get(&binding.session_id)
            .map(|session| session.lifecycle()["recovered"].clone())
            .unwrap_or(Value::Null);
        Ok(recovered["proven"] == true && recovered["generation"].as_u64().is_some())
    }

    fn admit_unsettled_turn(
        &mut self,
        mission: &crate::domain::MissionRecord,
        turn: &str,
    ) -> Result<()> {
        if mission.phase == Phase::NeedsAttention || mission.phase.is_terminal() {
            return Ok(());
        }
        self.admit_host_fact(
            mission,
            CommandBody::DetectUncertainty {
                reason: AttentionReason::UnsettledTurn,
                content_digest: Some(turn.to_string()),
            },
        )?;
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
        let wall_hit = session.wall_exhausted();
        let mut lifecycle = session.lifecycle();
        self.stamp_attempt_clock(&mut lifecycle);
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
        if wall_hit {
            let mission = self.store.load_mission(&binding.mission_id)?;
            if !mission.phase.is_terminal()
                && !matches!(
                    mission.phase,
                    Phase::NeedsAttention | Phase::Pausing | Phase::Cancelling
                )
            {
                self.exhaust_runtime_budget(&binding.mission_id)?;
            }
        } else if let Err(error) = &result
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

    /// Bridge wall, attempt wall, and bridge CPU from the freeze. The frozen
    /// 40 is not counted here; the splice allowance enforces that cap.
    fn enforce_observed(&mut self, binding: &RuntimeBinding) -> Result<()> {
        let (bridge, verifier) = self.attempt_clock.components();
        let (generation, cpu, wall_hit) = self.confined_generation_usage(&binding.session_id);
        let usage = crate::budget::BudgetUsage {
            bridge_generation: if wall_hit {
                Duration::from_secs(crate::BRIDGE_WALL_SECONDS.saturating_add(1))
            } else {
                generation
            },
            bridge_outside_verify: bridge,
            verifier,
            bridge_cpu: cpu,
        };
        self.enforce_usage(&binding.mission_id, &usage)
    }

    fn confined_generation_usage(&mut self, session_id: &str) -> (Duration, Duration, bool) {
        let Ok(runtime) = self.confined_mut() else {
            return (Duration::ZERO, Duration::ZERO, false);
        };
        let Some(session) = runtime.sessions.get(session_id) else {
            return (Duration::ZERO, Duration::ZERO, false);
        };
        let cpu = session
            .lifecycle()
            .get("cgroup")
            .and_then(|cgroup| cgroup.get("path"))
            .and_then(Value::as_str)
            .and_then(|path| crate::linux_execution::cgroup::cpu_usage(Path::new(path)))
            .unwrap_or(Duration::ZERO);
        (session.generation_elapsed(), cpu, session.wall_exhausted())
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

    fn session_is_stopped(&mut self, binding: &RuntimeBinding) -> Result<bool> {
        Ok(self
            .confined_mut()?
            .sessions
            .get(&binding.session_id)
            .is_some_and(|session| session.is_stopped()))
    }

    /// Fresh `reprove` for a bridge settle already stopped, then store it.
    /// The session goes back in the map before any error returns. The attempt
    /// clock is stamped on the stored lifecycle.
    fn reprove_and_store(&mut self, binding: &RuntimeBinding) -> Result<Value> {
        let session_id = binding.session_id.clone();
        let mut session = self
            .confined_mut()?
            .sessions
            .remove(&session_id)
            .ok_or_else(|| Error::guard("confined session is unavailable"))?;
        self.note_bridge_down();
        let proof = match session.reprove() {
            Ok(evidence) => evidence,
            Err(error) => {
                self.confined_mut()?.sessions.insert(session_id, session);
                return Err(error);
            }
        };
        let mut lifecycle = session.lifecycle();
        self.stamp_attempt_clock(&mut lifecycle);
        let observation = match self.store.pi_host_view(&binding.attempt_id) {
            Ok(view) => view
                .map(|view| view["observation"].clone())
                .unwrap_or(Value::Null),
            Err(error) => {
                self.confined_mut()?.sessions.insert(session_id, session);
                return Err(error);
            }
        };
        self.confined_mut()?.sessions.insert(session_id, session);
        self.store.save_pi_host(
            &binding.attempt_id,
            binding.generation,
            &lifecycle,
            &observation,
        )?;
        Ok(proof)
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
                method: RuntimeAdapter::runtime_method(command.kind),
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
        if matches!(mission.phase, Phase::Pausing | Phase::Cancelling) {
            // The stop was not confirmed. Leave runtime work pending.
            return Err(Error::SettleNotProven);
        }
        self.ensure_confined_session(mission_id)?;
        if self.store.load_mission(mission_id)?.phase == Phase::NeedsAttention {
            return Ok(count);
        }
        if self.session_is_stopped(&binding)? {
            // Do not send on a stopped bridge. A fresh proof that is still
            // unproven leaves the work pending. A proven stop keeps a repair
            // pending and supersedes other runtime work.
            let proof = self.reprove_and_store(&binding)?;
            if proof.get("proven").and_then(Value::as_bool) != Some(true) {
                return Err(Error::SettleNotProven);
            }
            for command in self.store.list_commands(mission_id)? {
                if command.state == CommandState::Pending && is_runtime_work(command.kind) {
                    if is_post_settle_repair(command.kind) {
                        return Err(Error::RepairAfterSettle);
                    }
                    self.store.supersede_pending(
                        &command.id,
                        "admission closed before runtime accepted this command",
                    )?;
                }
            }
            return Ok(count);
        }
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
                method: RuntimeAdapter::runtime_method(command.kind),
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
                            // A dead generation left `pi.live.run` set. Do not
                            // clear it and do not retry the submit. Cancel is
                            // the way out.
                            if self.recovered_dead_generation(&request.binding)?
                                && error.category() == "busy"
                            {
                                let current = self.store.load_mission(mission_id)?;
                                self.admit_unsettled_turn(&current, &command.request_id)?;
                                return Ok(count);
                            }
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
                    method: RuntimeAdapter::runtime_method(command.kind),
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
        // One branch for a bridge that is already stopped. `reprove` always
        // runs `rekill_and_prove`. Ack the host marker only when that fresh
        // proof is `proven`. An unproven result leaves the stop pending.
        if self.session_is_stopped(&binding)? {
            let proof = self.reprove_and_store(&binding)?;
            self.reconcile_execution(mission_id)?;
            if proof.get("proven").and_then(Value::as_bool) == Some(true)
                && !self.store.ops_blocked(mission_id)?
            {
                let mut count = 0;
                for command in &stops {
                    self.store.acknowledge(
                        &command.id,
                        &host_settle_acknowledgement(&command.request_id),
                    )?;
                    count += 1;
                }
                for command in self.store.list_commands(mission_id)? {
                    if command.state == CommandState::Pending && is_runtime_work(command.kind) {
                        self.store.supersede_pending(
                            &command.id,
                            "admission closed before runtime accepted this command",
                        )?;
                    }
                }
                let current = self.store.load_mission(mission_id)?;
                let body = match current.phase {
                    Phase::Pausing => Some(CommandBody::ConfirmStopped),
                    Phase::Cancelling => Some(CommandBody::ConfirmCancelled),
                    _ => None,
                };
                if let Some(body) = body {
                    let admitted = self.admit_host_fact(&current, body)?;
                    self.store
                        .record_supervisor_receipt(&admitted.command_id, &proof.to_string())?;
                }
                self.release_finished_sessions()?;
                return Ok(count);
            }
            return Ok(0);
        }
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
                method: RuntimeAdapter::runtime_method(command.kind),
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
        // Fold bridge-alive time before `stop`. Proof stays `stop` then
        // `rekill_and_prove`, then reconcile and the ops_blocked check.
        self.note_bridge_down();
        let proof = self
            .confined_mut()?
            .sessions
            .get_mut(&binding.session_id)
            .ok_or(Error::NotFound)?
            .stop()?;
        // TODO(d1b-splice-timeout-connection-cap): after #229 is on main, if
        // `modelEgress` says the proxy was exhausted, call
        // `exhaust_runtime_budget`. Do not admit `request_verification`.
        // The allowance is the frozen cap minus recorded connections across
        // prior generations, saturating at 0 (`try_from`, 0 on overflow).
        // No frozen budget, or a prior generation with egress and no recorded
        // count, is allowance 0. Persist that generation's `modelEgress`
        // before the next open. Freeze `maxRetries` 0. No second counter.
        let mut lifecycle = self
            .confined_mut()?
            .sessions
            .get(&binding.session_id)
            .ok_or(Error::NotFound)?
            .lifecycle();
        self.stamp_attempt_clock(&mut lifecycle);
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
                let admitted = self.admit_host_fact(&current, body)?;
                self.store
                    .record_supervisor_receipt(&admitted.command_id, &proof.to_string())?;
            }
        }
        // Cancel is terminal, so this drops the proven-stopped bridge. Pause is
        // not, and its stopped session stays in this generation.
        self.release_finished_sessions()?;
        Ok(count)
    }

    /// Stop the live bridge and leave the proven session in memory.
    ///
    /// The confinement probe uses this to dispatch a repair after settle
    /// without admitting cancel. `cfg(test)` is not set when that probe
    /// builds this crate.
    #[cfg(feature = "test-fixtures")]
    pub fn settle_unproven_bridge_for_test(&mut self, mission_id: &str) -> Result<Value> {
        self.ensure_confined_session(mission_id)?;
        let mission = self.store.load_mission(mission_id)?;
        let session_id = mission.attempt.session_id;
        let session = self
            .confined_mut()?
            .sessions
            .get_mut(&session_id)
            .ok_or(Error::NotFound)?;
        session.settle_unproven_for_test()?;
        Ok(session.lifecycle()["termination"].clone())
    }

    #[cfg(feature = "test-fixtures")]
    pub fn miss_bridge_leaf_for_test(&mut self, mission_id: &str) -> Result<()> {
        let mission = self.store.load_mission(mission_id)?;
        let session_id = mission.attempt.session_id;
        self.confined_mut()?
            .sessions
            .get_mut(&session_id)
            .ok_or(Error::NotFound)?
            .miss_leaf_for_test();
        Ok(())
    }

    #[cfg(feature = "test-fixtures")]
    pub fn stop_live_bridge_for_test(&mut self, mission_id: &str) -> Result<Value> {
        self.ensure_confined_session(mission_id)?;
        let mission = self.store.load_mission(mission_id)?;
        let session_id = mission.attempt.session_id;
        self.confined_mut()?
            .sessions
            .get_mut(&session_id)
            .ok_or(Error::NotFound)?
            .stop()
    }

    pub(super) fn poll_confined(&mut self, mission_id: &str) -> Result<Vec<NotifyOutcome>> {
        let mission = self.store.load_mission(mission_id)?;
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        if !self.store.runtime_initialized(&binding.attempt_id)? {
            return Ok(Vec::new());
        }
        self.ensure_confined_session(mission_id)?;
        if !matches!(mission.phase, Phase::Cancelling | Phase::Pausing)
            && let Err(error) = self.enforce_observed(&binding)
        {
            if error.category() == "runtime_budget_exhausted" {
                return Ok(vec![unchanged(&self.store.load_mission(mission_id)?)]);
            }
            return Err(error);
        }
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
}

/// Stop acknowledgement used when the bridge is already stopped. Not a Pi
/// receipt. Written only after a fresh proof with `proven: true`.
fn host_settle_acknowledgement(request_id: &str) -> String {
    json!({
        "adapter": "host-settle-proof",
        "delivered": false,
        "requestId": request_id,
    })
    .to_string()
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
