use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::canonical::{canonicalize, payload_hash};
use crate::delivery::{publication_operation_id, publication_payload_hash};
use crate::domain::{
    AttentionReason, CommandBody, CommandKind, CommandOrigin, CommandState, Completeness,
    MissionRecord, OperationState, Phase, PublicationProof, SourceIssue, VerificationFact,
    VerificationOutcome, now_ms,
};
use crate::error::{Error, Result};
use crate::execution::{
    FakeRuntime, OwnerLock, RuntimeAdapter, RuntimeBinding, RuntimeNotification, RuntimeReceipt,
    RuntimeRequest, inspect_repo, operation_id, parse_notification, try_acquire,
};
use crate::persistence::{CreatedMission, NewCommand, Store};
use crate::policy::{self, Facts};
use crate::transitions::target_spec;
use crate::verification::NewVerification;

mod execution;
mod linux_execution;
mod workspace;

/// Open the host, take the single-host lock, and reuse the active attempt.
///
/// Opening does not dispatch pending commands and does not create an attempt.
/// Call [`Host::dispatch_pending`] or [`Host::recover`] to deliver work that was
/// admitted before a restart.
pub struct Host {
    store: Store,
    runtime: RuntimeAdapter,
    lock: OwnerLock,
    ledger_path: PathBuf,
    // Process-local, Host-observed proof; never reconstructed from operator claims.
    execution_evidence: execution::ExecutionEvidence,
    #[cfg(all(feature = "linux-execution", target_os = "linux"))]
    linux: Option<linux_execution::LinuxSession>,
}

pub struct CreateMission {
    pub repo: PathBuf,
    pub issue_number: u64,
    pub issue_title: String,
    pub objective: String,
    pub acceptance: String,
    pub actor: String,
    pub max_repairs: u32,
    pub max_plan_revisions: u32,
}

pub struct AdmitRequest {
    pub command_id: Option<String>,
    pub mission_id: String,
    pub attempt_id: Option<String>,
    pub expected_revision: u64,
    pub actor: String,
    pub origin: CommandOrigin,
    pub body: CommandBody,
}

#[derive(Clone, Debug)]
pub struct AdmitResult {
    pub command_id: String,
    pub request_id: String,
    pub payload_hash: String,
    pub state: CommandState,
    pub revision: u64,
    pub phase: Phase,
    pub attempt_id: String,
    pub mission_id: String,
    pub idempotent: bool,
}

#[derive(Clone, Debug)]
pub struct CommandView {
    pub id: String,
    pub kind: String,
    pub state: String,
    pub payload_hash: String,
    pub request_id: String,
    pub expected_revision: u64,
    pub resulting_revision: Option<u64>,
    pub attempt_id: String,
    pub origin: String,
    pub receipt_id: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DeliveryView {
    pub id: String,
    pub mission_id: String,
    pub attempt_id: String,
    pub target_branch: String,
    pub base_branch: String,
    pub approved_digest: String,
    pub commit_sha: Option<String>,
    pub tree_sha: Option<String>,
    pub pr_identity: Option<String>,
    pub publication_state: String,
}

#[derive(Clone, Debug)]
pub struct MissionSnapshot {
    pub mission_id: String,
    pub revision: u64,
    pub phase: Phase,
    pub objective: String,
    pub attempt_id: String,
    pub task_id: String,
    pub session_id: String,
    pub base_sha: String,
    pub repair_count: u32,
    pub max_repairs: u32,
    pub plan_revision_count: u32,
    pub max_plan_revisions: u32,
    pub content_digest: String,
    pub plan_digest: Option<String>,
    pub plan_body: Option<String>,
    pub reviewed_digest: Option<String>,
    pub resume_target: Option<Phase>,
    pub prior_phase: Option<Phase>,
    pub attention_reason: Option<String>,
    pub permitted_recovery_target: Option<Phase>,
    pub reconcile_only: bool,
    pub recorded_owner_generation: u64,
    pub pending_commands: Vec<CommandView>,
    pub commands: Vec<CommandView>,
    pub successor_of_mission_id: Option<String>,
    pub head_sha: String,
    pub runtime_adapter: String,
    pub verifier_json: String,
    pub delivery: Option<DeliveryView>,
    pub workspace: Option<crate::WorkspaceView>,
    pub execution_effects: Vec<crate::ExecutionView>,
    pub execution_resolutions: Vec<Value>,
    pub linux_execution_effects: Vec<crate::LinuxExecutionView>,
    pub linux_execution_resolutions: Vec<Value>,
    pub execution_blocked: bool,
    pub quarantined: Vec<(i64, String, String, String)>,
}

impl MissionSnapshot {
    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "missionId": self.mission_id,
            "revision": self.revision,
            "phase": self.phase.as_str(),
            "objective": self.objective,
            "attemptId": self.attempt_id,
            "taskId": self.task_id,
            "sessionId": self.session_id,
            "baseSha": self.base_sha,
            "repairCount": self.repair_count,
            "maxRepairs": self.max_repairs,
            "planRevisionCount": self.plan_revision_count,
            "maxPlanRevisions": self.max_plan_revisions,
            "contentDigest": self.content_digest,
            "planDigest": self.plan_digest,
            "reviewedDigest": self.reviewed_digest,
            "resumeTarget": self.resume_target.map(|phase| phase.as_str()),
            "priorPhase": self.prior_phase.map(|phase| phase.as_str()),
            "attentionReason": self.attention_reason,
            "permittedRecoveryTarget": self.permitted_recovery_target.map(|phase| phase.as_str()),
            "reconcileOnly": self.reconcile_only,
            "recordedOwnerGeneration": self.recorded_owner_generation,
            "successorOfMissionId": self.successor_of_mission_id,
            "headSha": self.head_sha,
            "runtimeAdapter": self.runtime_adapter,
            "workspace": self.workspace.as_ref().map(crate::WorkspaceView::to_json),
            "executionEffects": self.execution_effects.iter().map(crate::ExecutionView::to_json).collect::<Vec<_>>(),
            "verifier": serde_json::from_str::<Value>(&self.verifier_json).ok(),
            "pendingCommands": self.pending_commands.iter().map(command_json).collect::<Vec<_>>(),
            "commands": self.commands.iter().map(command_json).collect::<Vec<_>>(),
            "delivery": self.delivery.as_ref().map(|delivery| json!({
                "id": delivery.id,
                "missionId": delivery.mission_id,
                "attemptId": delivery.attempt_id,
                "targetBranch": delivery.target_branch,
                "baseBranch": delivery.base_branch,
                "approvedDigest": delivery.approved_digest,
                "commitSha": delivery.commit_sha,
                "treeSha": delivery.tree_sha,
                "prId": delivery.pr_identity,
                "publicationState": delivery.publication_state,
            })),
            "quarantine": self.quarantined.iter().map(|(id, category, reason, frame)| json!({
                "id": id,
                "category": category,
                "reason": reason,
                "frame": frame,
            })).collect::<Vec<_>>(),
        });
        value["linuxExecutionEffects"] = json!(
            self.linux_execution_effects
                .iter()
                .map(crate::LinuxExecutionView::to_json)
                .collect::<Vec<_>>()
        );
        value["executionResolutions"] = json!(self.execution_resolutions);
        value["linuxExecutionResolutions"] = json!(self.linux_execution_resolutions);
        value["executionBlocked"] = json!(self.execution_blocked);
        value["nextSteps"] = json!(self.next_steps());
        value
    }

    fn next_steps(&self) -> Vec<String> {
        let mut steps = Vec::new();
        if matches!(
            self.phase,
            Phase::Pausing | Phase::Paused | Phase::Cancelling | Phase::NeedsAttention
        ) {
            for stop in self
                .commands
                .iter()
                .filter(|c| matches!(c.kind.as_str(), "pause" | "cancel"))
            {
                match stop.state.as_str() {
                    "pending" => steps.push(format!("{} abort {} is pending. Retry recovery with the same command identity to deliver it.", stop.kind, stop.id)),
                    "acknowledged" => steps.push(format!("{} abort {} was delivered (receipt {}). Delivery does not prove writer quiescence or complete the stop.", stop.kind, stop.id, stop.receipt_id.as_deref().unwrap_or("unavailable"))),
                    _ => {}
                }
            }
        }
        if let Some(workspace) = &self.workspace {
            if workspace.state != "ready" {
                steps.push(format!("Workspace remains {}. Preserve its ownership evidence; do not reuse it or dispatch work until explicit reconciliation succeeds.", workspace.state));
            } else if workspace.verified_generation != Some(self.recorded_owner_generation) {
                steps.push("Workspace is not verified for the current owner. Explicit recovery is required before work dispatch.".into());
            }
            steps.push("Status is read-only and does not probe the workspace lease or Git; recorded readiness is not a current filesystem or quiescence check.".into());
        }
        if self.linux_execution_effects.iter().any(|effect| {
            matches!(
                effect.state.as_str(),
                "prepared" | "entered" | "running" | "uncertain"
            )
        }) {
            steps.push("Linux execution evidence is under linuxExecutionEffects. not_applied requires a committed intent with no backend-entered marker and a matching empty cgroup (same path, device, inode and boot id, populated 0). completed requires the original Host-captured receipt plus that same quiescence. Anything else stays uncertain. Abandonment is final, keeps the block, and does not authorize another writer, workspace reuse, verification or publication. A superseded generation cannot start, settle or read the result. There is no CLI execution command and the simulated abandon-execution command cannot supply this proof. Missing, replaced or nonempty cgroups fail closed. Reboot recovery is out of scope.".into());
        }
        if self.execution_blocked {
            steps.push("Unresolved execution effects still block pause/cancel completion, recovery and workspace reuse. Inspect each block and its evidence protocol (#125 for execution effects); an abandoned effect remains blocking.".into());
            if self.runtime_adapter == "fake"
                && matches!(self.phase, Phase::Pausing | Phase::Cancelling)
            {
                steps.push(format!("To enter needs_attention for #125 resolution, admit DetectUncertainty as the operator: switchyard mission command --db '<DB_PATH>' --mission {} --attempt {} --command-id '<STABLE_COMMAND_ID>' --kind detect_uncertainty --payload '{{\"reason\":\"execution_uncertain\"}}' --expected-revision {} --persist-only. Replace the quoted placeholders and reuse the command ID on retry. This records uncertainty without clearing any effect. Resolve with matching evidence, then explicitly recover the recorded stop target; abandonment remains blocking.", self.mission_id, self.attempt_id, self.revision));
            }
        }
        steps
    }
}

#[derive(Clone, Debug)]
pub enum NotifyOutcome {
    Unchanged { phase: Phase, revision: u64 },
    Transition(AdmitResult),
    Recorded { phase: Phase, revision: u64 },
}

impl NotifyOutcome {
    pub fn phase(&self) -> Phase {
        match self {
            Self::Unchanged { phase, .. } | Self::Recorded { phase, .. } => *phase,
            Self::Transition(result) => result.phase,
        }
    }

    pub fn revision(&self) -> u64 {
        match self {
            Self::Unchanged { revision, .. } | Self::Recorded { revision, .. } => *revision,
            Self::Transition(result) => result.revision,
        }
    }
}

pub fn journal_path(db_path: &Path) -> PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push(".fake-runtime.json");
    PathBuf::from(name)
}

/// Resolve the database file, including a symlink, before lock and journal paths.
pub fn canonical_db_path(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(Error::storage);
    }
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| Error::storage("database path has no file name"))?;
    if !parent.exists() {
        std::fs::create_dir_all(parent).map_err(Error::storage)?;
    }
    Ok(std::fs::canonicalize(parent)
        .map_err(Error::storage)?
        .join(name))
}

impl Host {
    pub fn open(db_path: &Path) -> Result<Self> {
        let db_path = canonical_db_path(db_path)?;
        Self::open_with_runtime(&db_path, |db_path| {
            Ok(RuntimeAdapter::Fake(FakeRuntime::open(&journal_path(
                db_path,
            ))?))
        })
    }

    /// Explicit, credential-free S2-A probe. Uses the committed faux launcher;
    /// it enables neither model calls nor workspace effects. FakeRuntime remains
    /// the CLI/default adapter. A ledger cannot be switched between adapters.
    #[cfg(feature = "pi-contract")]
    pub fn open_pi_probe(db_path: &Path, node: &Path) -> Result<Self> {
        let db_path = canonical_db_path(db_path)?;
        let node = node.to_path_buf();
        Self::open_with_runtime(&db_path, |db_path| {
            Ok(RuntimeAdapter::Pi(Box::new(
                crate::execution::PiRuntime::new(db_path, &node)?,
            )))
        })
    }

    /// Acquire the owner lock before constructing the runtime. `FakeRuntime::open`
    /// loads the journal into memory, and `persist` rewrites that whole file.
    /// Loading first would let a handoff keep a stale snapshot.
    fn open_with_runtime(
        db_path: &Path,
        open_runtime: impl FnOnce(&Path) -> Result<RuntimeAdapter>,
    ) -> Result<Self> {
        let mut lock = try_acquire(db_path)?;
        let runtime = open_runtime(db_path)?;
        let mut store = Store::open_writer(db_path)?;
        store.migrate()?;
        store.require_runtime_config(&runtime.configuration())?;
        store.bump_generation(&mut lock)?;
        store.bind_owner_generation(lock.generation);
        Ok(Self {
            store,
            runtime,
            lock,
            ledger_path: db_path.to_path_buf(),
            execution_evidence: Default::default(),
            #[cfg(all(feature = "linux-execution", target_os = "linux"))]
            linux: None,
        })
    }

    pub fn generation(&self) -> u64 {
        self.lock.generation
    }

    /// Fake-runtime simulation seam for hosts opened with [`Host::open`].
    /// A Pi probe host returns `None` and cannot accept fake observations.
    pub fn runtime_mut(&mut self) -> Option<&mut FakeRuntime> {
        self.runtime.fake_mut()
    }

    pub fn durability(&self) -> Result<(String, i64)> {
        self.store.durability()
    }

    /// Read-only inspection. Does not take the owner lock, migrate, or dispatch.
    pub fn inspect(db_path: &Path, mission_id: Option<&str>) -> Result<Vec<MissionSnapshot>> {
        let db_path = canonical_db_path(db_path)?;
        let store = Store::open_reader(&db_path)?;
        let generation = store.generation()?;
        let ids = match mission_id {
            Some(id) => vec![id.to_string()],
            None => store.list_mission_ids()?,
        };
        if ids.is_empty() && mission_id.is_some() {
            return Err(Error::NotFound);
        }
        let mut snapshots = Vec::new();
        for id in ids {
            snapshots.push(snapshot_from(&store, &id, generation)?);
        }
        Ok(snapshots)
    }

    pub fn create_mission(&mut self, request: CreateMission) -> Result<MissionSnapshot> {
        let repo = inspect_repo(&request.repo)?;
        let created = CreatedMission {
            runtime_config: canonicalize(&self.runtime.configuration())?,
            id: crate::domain::new_id("m"),
            task_id: crate::domain::new_id("t"),
            attempt_id: crate::domain::new_id("a"),
            session_id: crate::domain::new_id("s"),
            objective: request.objective,
            acceptance: request.acceptance,
            content_digest: repo.head_sha.clone(),
            repo,
            issue: SourceIssue {
                provider: "github".into(),
                number: request.issue_number,
                title: request.issue_title,
                snapshot_at: now_ms(),
            },
            actor: request.actor,
            max_repairs: request.max_repairs,
            max_plan_revisions: request.max_plan_revisions,
            successor_of_mission_id: None,
            successor_of_attempt_id: None,
        };
        let id = created.id.clone();
        self.store.create_mission(created)?;
        self.snapshot(&id)
    }

    pub fn create_successor(
        &mut self,
        source_mission_id: &str,
        actor: &str,
    ) -> Result<MissionSnapshot> {
        let source = self.store.load_mission(source_mission_id)?;
        if !source.phase.is_terminal() {
            return Err(Error::guard(
                "a successor attempt requires a terminal mission and a new identity",
            ));
        }
        let before_revision = source.revision;
        let before_phase = source.phase;
        let repo = inspect_repo(Path::new(&source.repo.toplevel))?;
        let created = CreatedMission {
            runtime_config: canonicalize(&self.runtime.configuration())?,
            id: crate::domain::new_id("m"),
            task_id: crate::domain::new_id("t"),
            attempt_id: crate::domain::new_id("a"),
            session_id: crate::domain::new_id("s"),
            objective: source.objective.clone(),
            acceptance: source.acceptance_criteria.clone(),
            content_digest: repo.head_sha.clone(),
            repo,
            issue: source.source_issue.clone(),
            actor: actor.to_string(),
            max_repairs: source.attempt.max_repairs,
            max_plan_revisions: source.attempt.max_plan_revisions,
            successor_of_mission_id: Some(source.id.clone()),
            successor_of_attempt_id: Some(source.attempt.id.clone()),
        };
        let id = created.id.clone();
        self.store.create_mission(created)?;
        let unchanged = self.store.load_mission(source_mission_id)?;
        if unchanged.revision != before_revision || unchanged.phase != before_phase {
            return Err(Error::storage(
                "successor creation modified the terminal mission",
            ));
        }
        #[cfg(feature = "pi-contract")]
        self.release_finished_sessions()?;
        self.snapshot(&id)
    }

    pub fn admit(&mut self, request: AdmitRequest) -> Result<AdmitResult> {
        self.admit_attached(request, None)
    }

    fn admit_attached(
        &mut self,
        request: AdmitRequest,
        verification: Option<&NewVerification>,
    ) -> Result<AdmitResult> {
        let mission = self.store.load_mission(&request.mission_id)?;
        let attempt_id = request
            .attempt_id
            .clone()
            .unwrap_or_else(|| mission.attempt.id.clone());
        if attempt_id != mission.attempt.id {
            return Err(Error::BindingMismatch {
                detail: "attempt does not match the mission's active attempt".into(),
            });
        }
        let payload = request.body.payload(&mission.id, &attempt_id);
        let canonical = canonicalize(&payload)?;
        let hash = payload_hash(&payload)?;
        let command_id = request
            .command_id
            .clone()
            .unwrap_or_else(|| crate::domain::new_id("c"));
        self.runtime
            .validate_admission(&request.body, request.origin)?;
        if request.body.kind().is_runtime_bound() {
            let runtime_request = RuntimeRequest {
                binding: self.binding(&mission),
                request_id: format!("{attempt_id}:{command_id}"),
                payload_hash: hash.clone(),
                method: runtime_method(request.body.kind()),
            };
            self.runtime.validate_command(&runtime_request, &payload)?;
        }
        if let Some(existing) = self.store.find_command(&command_id)? {
            if existing.payload_hash != hash {
                return Err(Error::CommandConflict {
                    command_id: existing.id,
                });
            }
            if existing.mission_id != mission.id || existing.attempt_id != attempt_id {
                return Err(Error::BindingMismatch {
                    detail: "command id is already bound to a different mission or attempt".into(),
                });
            }
            return Ok(AdmitResult {
                command_id: existing.id,
                request_id: existing.request_id,
                payload_hash: existing.payload_hash,
                state: existing.state,
                revision: mission.revision,
                phase: mission.phase,
                attempt_id,
                mission_id: mission.id,
                idempotent: true,
            });
        }
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
        if is_runtime_observation(request.body.kind())
            && request.origin != CommandOrigin::RuntimeObservation
        {
            return Err(Error::guard(
                "plan, verification, and publication observations must come from the runtime",
            ));
        }
        if matches!(
            request.body.kind(),
            CommandKind::Steer | CommandKind::Continue
        ) {
            let facts = self.facts(&mission)?;
            policy::authorize(&mission, &request.body, &facts)?;
            let command = new_command(
                &request,
                &mission,
                &attempt_id,
                CommandDraft {
                    command_id,
                    canonical,
                    hash,
                    resulting_revision: mission.revision,
                    state: CommandState::Pending,
                },
            );
            self.store.insert_steer(&command)?;
            return Ok(result_of(&command, mission.phase, false));
        }
        let spec = target_spec(mission.phase, request.body.kind())?;
        let mut facts = self.facts(&mission)?;
        if let Some(verification) = verification {
            facts.verification = Some(VerificationFact {
                outcome: verification.outcome,
                content_digest: verification.content_digest.clone(),
                complete: verification.completeness == Completeness::Complete,
            });
        }
        policy::authorize(&mission, &request.body, &facts)?;
        let plan = policy::project(&mission, spec, &request.body, &facts)?;
        let mut command = new_command(
            &request,
            &mission,
            &attempt_id,
            CommandDraft {
                command_id,
                canonical,
                hash,
                resulting_revision: mission.revision.saturating_add(1),
                state: initial_state(&request),
            },
        );
        command.decision_digest = plan.decision_digest.clone();
        let continuation = self.continuation_command(&mission, &request, plan.next)?;
        self.store
            .apply_transition(&plan, &command, verification, continuation.as_ref())?;
        Ok(result_of(&command, plan.next, false))
    }

    /// Deliver pending commands. A recorded workspace is reconciled in this
    /// owner generation before the readiness gate, so a CLI command opened
    /// after preparation can dispatch without a separate recover.
    ///
    /// Reconciles once. A caller that admits in the same invocation should
    /// use [`Self::dispatch_reconciled`] for any later dispatch so the
    /// workspace is not verified again.
    pub fn dispatch_pending(&mut self, mission_id: &str) -> Result<usize> {
        self.reconcile_execution(mission_id)?;
        if self.store.linux_execution_blocked(mission_id)? {
            // Stop delivery still runs. Git verification waits until the
            // recorded cgroup is empty and the effect is settled or abandoned.
            self.dispatch_stops(mission_id)?;
            return Err(Error::ExecutionBlocked {
                mission_id: mission_id.to_string(),
                detail: "unresolved linux execution effects; workspace was not verified or reused"
                    .into(),
            });
        }
        match self.recover_workspace(mission_id) {
            Ok(_) => self.dispatch_reconciled(mission_id),
            Err(error) if error.is_isolated_workspace_failure() => {
                self.dispatch_stops(mission_id).map_err(|stop_error| {
                    stop_error
                        .with_context(format!("workspace reconciliation also failed: {error}"))
                })?;
                Err(error)
            }
            Err(error) => Err(error),
        }
    }

    /// Deliver only already-admitted FakeRuntime Pause/Cancel commands. This
    /// does not inspect or mutate a workspace, apply effects, or complete a stop.
    /// Pi stop delivery remains behind its existing S2/Track B contract.
    pub fn dispatch_stops(&mut self, mission_id: &str) -> Result<usize> {
        self.terminate_linux_writers(mission_id)?;
        self.reconcile_execution(mission_id)?;
        if !matches!(self.runtime, RuntimeAdapter::Fake(_)) {
            return Ok(0);
        }
        let mission = self.store.load_mission(mission_id)?;
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        let pending = self.store.list_commands(mission_id)?;
        let mut dispatched = 0;
        for command in pending.iter().filter(|command| {
            command.state == CommandState::Pending
                && matches!(command.kind, CommandKind::Pause | CommandKind::Cancel)
        }) {
            self.accept_runtime_binding(&binding, mission_id)?;
            // Successors have a new mission ID; list_commands is mission-scoped.
            // A different attempt here is an integrity error, not ordinary stale
            // work to silently discard. Keep its evidence and fail closed.
            if command.attempt_id != binding.attempt_id {
                return Err(Error::BindingMismatch {
                    detail: "stop command does not belong to the active attempt".into(),
                });
            }
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id.clone(),
                payload_hash: command.payload_hash.clone(),
                method: "abort",
            };
            let payload =
                serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
            // FakeRuntime's idempotent abort validates stored identity, method
            // and payload on replay, returning the original durable receipt.
            let receipt = self.runtime.dispatch(&request, &payload)?;
            self.store
                .acknowledge(&command.id, &receipt_json(&receipt, self.runtime.name()))?;
            dispatched += 1;
        }
        Ok(dispatched)
    }

    /// Dispatch pending commands without verifying the workspace again.
    /// The caller must already have reconciled it in this owner generation.
    pub fn dispatch_reconciled(&mut self, mission_id: &str) -> Result<usize> {
        let stops_dispatched = self.dispatch_stops(mission_id)?;
        let mission = self.store.load_mission(mission_id)?;
        self.store.require_workspace_ready(&mission.attempt.id)?;
        let recorded = self.store.list_commands(mission_id)?;
        let mut admitted_runtime_work = recorded.iter().any(|command| {
            command.state == CommandState::Acknowledged && is_runtime_work(command.kind)
        });
        let pending: Vec<_> = recorded
            .iter()
            .filter(|command| command.state == CommandState::Pending)
            .cloned()
            .collect();
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        #[cfg(feature = "pi-contract")]
        let pi_closed = matches!(
            &self.runtime,
            RuntimeAdapter::Pi(runtime) if runtime.was_closed(&binding)
        );
        #[cfg(feature = "pi-contract")]
        let mut pending = pending;
        #[cfg(feature = "pi-contract")]
        if pi_closed {
            // Supersede rows the closed phase would not send, then fail only if
            // a runtime command would still have to reach the evicted child.
            let mut kept = Vec::new();
            let mut must_send = false;
            for command in pending {
                if command.kind.is_runtime_bound()
                    && suppress_pending(mission.phase, command.kind, admitted_runtime_work)
                {
                    self.store.supersede_pending(
                        &command.id,
                        "admission closed before the runtime accepted this command",
                    )?;
                    continue;
                }
                if command.kind.is_runtime_bound() {
                    must_send = true;
                }
                kept.push(command);
            }
            if must_send {
                return Err(Error::guard(
                    "Pi session was closed for this host generation; restart to dispatch it again",
                ));
            }
            pending = kept;
        } else if let RuntimeAdapter::Pi(runtime) = &mut self.runtime {
            let initialized = self.store.runtime_initialized(&mission.attempt.id)?;
            runtime.open(&binding, initialized)?;
            if !initialized {
                self.store.mark_runtime_initialized(&mission.attempt.id)?;
            }
            if runtime.needs_resume(&binding) {
                // Validate previous ACKs before admitting any new work. A valid
                // session with a missing command is not an empty session to refill.
                // One inspect covers the set; each row is still checked on its own.
                let mut acknowledged = Vec::new();
                for command in recorded
                    .iter()
                    .filter(|c| c.state == CommandState::Acknowledged && c.kind.is_runtime_bound())
                {
                    acknowledged.push(crate::execution::AcknowledgedCheck {
                        request: RuntimeRequest {
                            binding: binding.clone(),
                            request_id: command.request_id.clone(),
                            payload_hash: command.payload_hash.clone(),
                            method: runtime_method(command.kind),
                        },
                        payload: serde_json::from_str(&command.payload_canonical)
                            .map_err(Error::storage)?,
                        recorded: command.runtime_receipt_json.clone(),
                    });
                }
                runtime.require_acknowledged_batch(&acknowledged)?;
            }
        }
        let mut dispatched = stops_dispatched;
        for command in pending {
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id.clone(),
                payload_hash: command.payload_hash.clone(),
                method: runtime_method(command.kind),
            };
            let payload: Value =
                serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
            self.runtime.validate_command(&request, &payload)?;
            if let Some(receipt) = self.runtime.reconcile(&request, &payload)? {
                if receipt.payload_hash != command.payload_hash {
                    return Err(Error::CommandConflict {
                        command_id: command.id,
                    });
                }
                self.store
                    .acknowledge(&command.id, &receipt_json(&receipt, self.runtime.name()))?;
                admitted_runtime_work |= is_runtime_work(command.kind);
                dispatched += 1;
                continue;
            }
            if suppress_pending(mission.phase, command.kind, admitted_runtime_work) {
                self.store.supersede_pending(
                    &command.id,
                    "admission closed before the runtime accepted this command",
                )?;
                continue;
            }
            if is_runtime_work(command.kind) && self.store.ops_blocked(mission_id)? {
                return Err(Error::guard("uncertain effects block runtime dispatch"));
            }
            let receipt = match self.runtime.dispatch(&request, &payload) {
                Ok(receipt) => receipt,
                Err(error) => {
                    let Some(category) = error.rejected_admission().map(str::to_string) else {
                        return Err(error);
                    };
                    // Confirm Pi stored nothing. A receipt or a conflict stays
                    // fail-closed; only a definite refusal is settled.
                    match self.runtime.reconcile(&request, &payload)? {
                        Some(receipt) => receipt,
                        None => {
                            self.store.reject_pending(
                                &command.id,
                                &rejection_json(&category, self.runtime.name()),
                            )?;
                            continue;
                        }
                    }
                }
            };
            self.store
                .acknowledge(&command.id, &receipt_json(&receipt, self.runtime.name()))?;
            admitted_runtime_work |= is_runtime_work(command.kind);
            dispatched += 1;
        }
        #[cfg(feature = "pi-contract")]
        self.resume_reconciled_pi(&mission)?;
        #[cfg(feature = "pi-contract")]
        self.release_finished_sessions()?;
        Ok(dispatched)
    }

    /// Resume only an already reconciled, authorized submission. A pending stop
    /// is drained above, and a closed/uncertain mission never reaches this path.
    #[cfg(feature = "pi-contract")]
    fn resume_reconciled_pi(&mut self, mission: &MissionRecord) -> Result<()> {
        if !matches!(mission.phase, Phase::Planning | Phase::Implementing) {
            return Ok(());
        }
        let binding = self.binding(mission);
        self.accept_runtime_binding(&binding, &mission.id)?;
        let RuntimeAdapter::Pi(runtime) = &mut self.runtime else {
            return Ok(());
        };
        if !runtime.needs_resume(&binding) {
            return Ok(());
        }
        if self.store.ops_blocked(&mission.id)? {
            return Err(Error::guard("uncertain effects block Pi scheduling"));
        }
        let commands = self.store.list_commands(&mission.id)?;
        if commands.iter().any(|c| c.state == CommandState::Pending) {
            return Err(Error::guard("outbox must reconcile before Pi scheduling"));
        }
        // Stop at the most recent delivered abort. Earlier activity must not
        // suppress a new continuation or revive work from before that boundary.
        let candidate = commands
            .iter()
            .rev()
            .take_while(|c| {
                !(c.state == CommandState::Acknowledged
                    && matches!(c.kind, CommandKind::Pause | CommandKind::Cancel))
            })
            .find(|c| c.state == CommandState::Acknowledged && is_runtime_work(c.kind));
        let Some(command) = candidate else {
            return Ok(());
        };
        let request = RuntimeRequest {
            binding,
            request_id: command.request_id.clone(),
            payload_hash: command.payload_hash.clone(),
            method: runtime_method(command.kind),
        };
        let payload: Value =
            serde_json::from_str(&command.payload_canonical).map_err(Error::storage)?;
        let original = runtime.require_acknowledged(
            &request,
            &payload,
            command.runtime_receipt_json.as_deref(),
        )?;
        let resumed = runtime.dispatch(&request, &payload)?;
        if resumed != original {
            return Err(Error::guard("Pi replay changed the original receipt"));
        }
        Ok(())
    }

    /// Drop Pi children whose attempt is terminal or has been superseded.
    /// The sqlite file stays; only the process and its lock are released.
    #[cfg(feature = "pi-contract")]
    fn release_finished_sessions(&mut self) -> Result<()> {
        let open = match &self.runtime {
            RuntimeAdapter::Pi(runtime) => runtime.open_bindings(),
            RuntimeAdapter::Fake(_) => return Ok(()),
        };
        let mut finished = Vec::new();
        for binding in open {
            let mission = self.store.load_mission(&binding.mission_id)?;
            let superseded = self.store.attempt_has_successor(&binding.attempt_id)?;
            if mission.phase.is_terminal()
                || superseded
                || mission.attempt.id != binding.attempt_id
                || mission.attempt.session_id != binding.session_id
            {
                finished.push(binding);
            }
        }
        if let RuntimeAdapter::Pi(runtime) = &mut self.runtime {
            for binding in &finished {
                runtime.evict(binding);
            }
        }
        Ok(())
    }

    /// Reconcile one workspace, deliver pending commands, poll observations,
    /// and return a snapshot. An uncertain, busy, or transient workspace
    /// failure permits FakeRuntime stop delivery, polling and the snapshot;
    /// that failure is returned after those steps. Stale generation and other
    /// host-level failures abort before either runs.
    pub fn recover(&mut self, mission_id: &str) -> Result<MissionSnapshot> {
        self.reconcile_execution(mission_id)?;
        if self.store.linux_execution_blocked(mission_id)? {
            self.dispatch_stops(mission_id)?;
            self.poll_runtime(mission_id)?;
            return Err(Error::ExecutionBlocked {
                mission_id: mission_id.to_string(),
                detail: "unresolved linux execution effects; workspace was not verified or reused"
                    .into(),
            });
        }
        let workspace_error = match self.recover_workspace(mission_id) {
            Ok(_) => None,
            Err(error) if error.is_isolated_workspace_failure() => Some(error),
            Err(error) => return Err(error),
        };
        if workspace_error.is_none() {
            self.dispatch_reconciled(mission_id)?;
        } else if let Some(error) = &workspace_error {
            self.dispatch_stops(mission_id).map_err(|stop_error| {
                stop_error.with_context(format!("workspace reconciliation also failed: {error}"))
            })?;
        }
        self.poll_runtime(mission_id)?;
        let snapshot = self.snapshot(mission_id)?;
        if let Some(error) = workspace_error {
            return Err(error);
        }
        Ok(snapshot)
    }

    pub fn poll_runtime(&mut self, mission_id: &str) -> Result<Vec<NotifyOutcome>> {
        let mut outcomes = Vec::new();
        while let Some(note) = self
            .runtime
            .fake_mut()
            .and_then(FakeRuntime::pop_notification)
        {
            match self.apply_notification(mission_id, note.clone()) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) if error.is_transient() => {
                    self.runtime
                        .fake_mut()
                        .expect("notification was popped from FakeRuntime")
                        .requeue_front(note);
                    return Err(error);
                }
                Err(error) => {
                    let frame = note.frame_json().to_string();
                    if let Err(storage) = self.store.quarantine_notification(
                        mission_id,
                        error.category(),
                        &error.to_string(),
                        &frame,
                    ) {
                        self.runtime
                            .fake_mut()
                            .expect("notification was popped from FakeRuntime")
                            .requeue_front(note);
                        return Err(storage);
                    }
                }
            }
        }
        Ok(outcomes)
    }

    pub fn apply_notification_json(
        &mut self,
        mission_id: &str,
        value: &Value,
    ) -> Result<NotifyOutcome> {
        let notification = parse_notification(value)?;
        self.apply_notification(mission_id, notification)
    }

    /// Map one runtime observation through the transition table.
    ///
    /// The notification type has no mission phase. Illegal observations leave
    /// the stored phase unchanged because the transition transaction rolls back.
    pub fn apply_notification(
        &mut self,
        mission_id: &str,
        notification: RuntimeNotification,
    ) -> Result<NotifyOutcome> {
        if self.runtime.fake_mut().is_none() {
            return Err(Error::guard(
                "fake observations cannot certify Pi mission facts",
            ));
        }
        let mission = self.store.load_mission(mission_id)?;
        match notification {
            RuntimeNotification::Idle => Ok(unchanged(&mission)),
            RuntimeNotification::PlanCompleted {
                plan,
                digest,
                quiescent,
            } => {
                if mission.plan_digest.as_deref() == Some(digest.as_str())
                    && mission.phase != Phase::Planning
                {
                    return Ok(unchanged(&mission));
                }
                self.admit_observation(
                    &mission,
                    CommandBody::PlanCompleted {
                        plan,
                        digest,
                        quiescent,
                    },
                )
                .map(NotifyOutcome::Transition)
            }
            RuntimeNotification::ImplementationSettled { quiescent } => {
                if mission.phase == Phase::Verifying
                    || matches!(
                        mission.phase,
                        Phase::AwaitingReview
                            | Phase::Approved
                            | Phase::Publishing
                            | Phase::Published
                    )
                {
                    return Ok(unchanged(&mission));
                }
                self.admit_observation(&mission, CommandBody::RequestVerification { quiescent })
                    .map(NotifyOutcome::Transition)
            }
            RuntimeNotification::VerificationPass {
                content_digest,
                evidence,
                requirements,
                completeness,
            } => self.verification_pass(
                &mission,
                content_digest,
                evidence,
                requirements,
                completeness,
            ),
            RuntimeNotification::VerificationFail {
                content_digest,
                evidence,
                requirements,
                completeness,
            } => {
                if mission.phase.is_terminal() {
                    return Err(Error::TerminalImmutable {
                        phase: mission.phase.to_string(),
                    });
                }
                if mission.phase != Phase::Verifying {
                    return Err(Error::IllegalTransition {
                        from: mission.phase.to_string(),
                        command: "verification_fail".into(),
                    });
                }
                self.record_verification_row(
                    &mission,
                    VerificationOutcome::Fail,
                    &content_digest,
                    &evidence,
                    &requirements,
                    &completeness,
                )?;
                let current = self.store.load_mission(mission_id)?;
                if current.phase != Phase::Verifying
                    || current.attempt.repair_count != mission.attempt.repair_count
                {
                    return Err(Error::storage(
                        "verification failure must not change phase or repair count",
                    ));
                }
                Ok(NotifyOutcome::Recorded {
                    phase: current.phase,
                    revision: current.revision,
                })
            }
            RuntimeNotification::VerificationError { detail } => self.terminal_observation(
                &mission,
                VerificationOutcome::Error,
                &detail,
                AttentionReason::VerificationError,
            ),
            RuntimeNotification::VerificationIncomplete { detail } => self.terminal_observation(
                &mission,
                VerificationOutcome::Incomplete,
                &detail,
                AttentionReason::VerificationIncomplete,
            ),
            RuntimeNotification::Uncertainty { reason, .. } => self.uncertainty(&mission, reason),
            RuntimeNotification::PublicationObserved {
                content_digest,
                commit_sha,
                tree_sha,
                pr_id,
            } => self.publication_observed(
                &mission,
                PublicationProof {
                    content_digest,
                    commit_sha,
                    tree_sha,
                    pr_id,
                },
            ),
        }
    }

    pub fn note_content_digest(
        &mut self,
        mission_id: &str,
        expected_revision: u64,
        digest: &str,
        actor: &str,
    ) -> Result<MissionSnapshot> {
        let mission = self.store.load_mission(mission_id)?;
        if mission.phase.is_terminal() {
            return Err(Error::TerminalImmutable {
                phase: mission.phase.to_string(),
            });
        }
        if mission.content_digest == digest {
            return self.snapshot(mission_id);
        }
        if matches!(
            mission.phase,
            Phase::AwaitingReview | Phase::Approved | Phase::Publishing
        ) {
            self.admit(AdmitRequest {
                command_id: None,
                mission_id: mission_id.to_string(),
                attempt_id: Some(mission.attempt.id.clone()),
                expected_revision,
                actor: actor.to_string(),
                origin: CommandOrigin::Operator,
                body: CommandBody::DetectUncertainty {
                    reason: crate::domain::AttentionReason::InvalidApproval,
                    content_digest: Some(digest.to_string()),
                },
            })?;
        } else {
            self.store.observe_content(
                mission_id,
                expected_revision,
                digest,
                actor,
                &mission.attempt.id,
            )?;
        }
        self.snapshot(mission_id)
    }

    pub fn record_operation(
        &mut self,
        mission_id: &str,
        operation_id: &str,
        payload: &Value,
        state: OperationState,
    ) -> Result<()> {
        let mission = self.store.load_mission(mission_id)?;
        let hash = payload_hash(payload)?;
        let outcome = serde_json::to_string(payload).map_err(Error::storage)?;
        self.store.record_operation(
            mission_id,
            &mission.attempt.id,
            operation_id,
            &hash,
            state,
            Some(&outcome),
        )
    }

    pub fn settle_operation(&mut self, operation_id: &str, payload_hash: &str) -> Result<()> {
        self.store.settle_operation(operation_id, payload_hash)
    }

    pub fn accept_runtime_binding(&self, binding: &RuntimeBinding, mission_id: &str) -> Result<()> {
        let mission = self.store.load_mission(mission_id)?;
        let stored = self.store.generation()?;
        if binding.generation != stored || self.lock.generation != stored {
            let actual = if binding.generation != stored {
                binding.generation
            } else {
                self.lock.generation
            };
            return Err(Error::StaleGeneration {
                expected: stored,
                actual,
            });
        }
        if binding.mission_id != mission.id
            || binding.attempt_id != mission.attempt.id
            || binding.session_id != mission.attempt.session_id
        {
            return Err(Error::BindingMismatch {
                detail: "runtime binding does not match the active attempt".into(),
            });
        }
        Ok(())
    }

    pub fn snapshot(&self, mission_id: &str) -> Result<MissionSnapshot> {
        snapshot_from(&self.store, mission_id, self.lock.generation)
    }

    fn admit_observation(
        &mut self,
        mission: &MissionRecord,
        body: CommandBody,
    ) -> Result<AdmitResult> {
        self.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.id.clone(),
            attempt_id: Some(mission.attempt.id.clone()),
            expected_revision: mission.revision,
            actor: "runtime-observation".into(),
            origin: CommandOrigin::RuntimeObservation,
            body,
        })
    }

    fn uncertainty(
        &mut self,
        mission: &MissionRecord,
        reason: crate::domain::AttentionReason,
    ) -> Result<NotifyOutcome> {
        if mission.phase == Phase::NeedsAttention {
            return Ok(unchanged(mission));
        }
        self.admit_observation(
            mission,
            CommandBody::DetectUncertainty {
                reason,
                content_digest: None,
            },
        )
        .map(NotifyOutcome::Transition)
    }

    fn verification_pass(
        &mut self,
        mission: &MissionRecord,
        content_digest: String,
        evidence: String,
        requirements: Vec<String>,
        completeness: String,
    ) -> Result<NotifyOutcome> {
        if mission.phase.is_terminal() {
            return Err(Error::TerminalImmutable {
                phase: mission.phase.to_string(),
            });
        }
        if mission.phase == Phase::AwaitingReview && mission.content_digest == content_digest {
            return Ok(unchanged(mission));
        }
        if mission.phase != Phase::Verifying {
            return Err(Error::IllegalTransition {
                from: mission.phase.to_string(),
                command: "verification_pass".into(),
            });
        }
        let verification = verification_record(
            VerificationOutcome::Pass,
            &content_digest,
            &evidence,
            &requirements,
            &completeness,
        );
        self.admit_attached(
            AdmitRequest {
                command_id: None,
                mission_id: mission.id.clone(),
                attempt_id: Some(mission.attempt.id.clone()),
                expected_revision: mission.revision,
                actor: "runtime-observation".into(),
                origin: CommandOrigin::RuntimeObservation,
                body: CommandBody::CompletePass,
            },
            Some(&verification),
        )
        .map(NotifyOutcome::Transition)
    }

    fn terminal_observation(
        &mut self,
        mission: &MissionRecord,
        outcome: VerificationOutcome,
        detail: &str,
        reason: AttentionReason,
    ) -> Result<NotifyOutcome> {
        if mission.phase.is_terminal() {
            return Err(Error::TerminalImmutable {
                phase: mission.phase.to_string(),
            });
        }
        if mission.phase == Phase::NeedsAttention {
            return Ok(unchanged(mission));
        }
        let verification = verification_record(
            outcome,
            &mission.content_digest,
            detail,
            &[],
            Completeness::Incomplete.as_str(),
        );
        self.admit_attached(
            AdmitRequest {
                command_id: None,
                mission_id: mission.id.clone(),
                attempt_id: Some(mission.attempt.id.clone()),
                expected_revision: mission.revision,
                actor: "runtime-observation".into(),
                origin: CommandOrigin::RuntimeObservation,
                body: CommandBody::DetectUncertainty {
                    reason,
                    content_digest: None,
                },
            },
            Some(&verification),
        )
        .map(NotifyOutcome::Transition)
    }

    fn continuation_command(
        &self,
        mission: &MissionRecord,
        request: &AdmitRequest,
        next: Phase,
    ) -> Result<Option<NewCommand>> {
        let resumes = matches!(
            request.body,
            CommandBody::Resume { .. } | CommandBody::ExplicitRecovery { .. }
        );
        if !resumes || !matches!(next, Phase::Planning | Phase::Implementing) {
            return Ok(None);
        }
        let commands = self.store.list_commands(&mission.id)?;
        if resumable_work_for(&commands, next) {
            return Ok(None);
        }
        let activity = if next == Phase::Planning {
            "planning"
        } else if mission.attempt.repair_count > 0 {
            "repair"
        } else {
            "implementing"
        };
        let body = CommandBody::Continue {
            activity: activity.to_string(),
        };
        let attempt_id = mission.attempt.id.clone();
        let command_id = crate::domain::new_id("c");
        let payload = body.payload(&mission.id, &attempt_id);
        let canonical = canonicalize(&payload)?;
        let hash = payload_hash(&payload)?;
        Ok(Some(NewCommand {
            id: command_id.clone(),
            mission_id: mission.id.clone(),
            attempt_id: attempt_id.clone(),
            kind: CommandKind::Continue,
            payload_canonical: canonical,
            payload_hash: hash,
            expected_revision: request.expected_revision.saturating_add(1),
            resulting_revision: request.expected_revision.saturating_add(1),
            state: CommandState::Pending,
            request_id: format!("{attempt_id}:{command_id}"),
            origin: CommandOrigin::Operator,
            actor: request.actor.clone(),
            rationale: format!("continue {activity}"),
            decision_digest: None,
        }))
    }

    fn record_verification_row(
        &mut self,
        mission: &MissionRecord,
        outcome: VerificationOutcome,
        content_digest: &str,
        evidence: &str,
        requirements: &[String],
        completeness: &str,
    ) -> Result<()> {
        let completeness = completeness
            .parse::<Completeness>()
            .unwrap_or(Completeness::Incomplete);
        self.store.record_verification(
            &mission.id,
            &mission.attempt.id,
            &NewVerification {
                outcome,
                content_digest: content_digest.to_string(),
                evidence_body: evidence.to_string(),
                completeness,
                requirements: requirements.to_vec(),
            },
        )?;
        Ok(())
    }

    fn publication_observed(
        &mut self,
        mission: &MissionRecord,
        proof: PublicationProof,
    ) -> Result<NotifyOutcome> {
        let Some(delivery) = self.store.load_delivery(&mission.id)? else {
            return Err(Error::guard(
                "publication observation has no delivery intent",
            ));
        };
        let matches = proof.matches_digest(&delivery.approved_digest);
        if matches {
            let hash = publication_payload_hash(&proof)?;
            let accept_id = publication_operation_id(&delivery.id)?;
            self.store.record_operation(
                &mission.id,
                &mission.attempt.id,
                &accept_id,
                &hash,
                OperationState::Completed,
                Some(
                    &serde_json::to_string(&crate::delivery::publication_payload(&proof))
                        .map_err(Error::storage)?,
                ),
            )?;
            let current = self.store.load_mission(&mission.id)?;
            if current.phase == Phase::Published {
                return Ok(unchanged(&current));
            }
            let body = if current.phase == Phase::Cancelling {
                CommandBody::DiscoverPublicationCompleted {
                    content_digest: proof.content_digest,
                    commit_sha: proof.commit_sha,
                    tree_sha: proof.tree_sha,
                    pr_id: proof.pr_id,
                }
            } else if current.phase == Phase::Publishing {
                CommandBody::ReconcileDeliveryComplete {
                    content_digest: proof.content_digest,
                    commit_sha: proof.commit_sha,
                    tree_sha: proof.tree_sha,
                    pr_id: proof.pr_id,
                }
            } else {
                return Ok(NotifyOutcome::Recorded {
                    phase: current.phase,
                    revision: current.revision,
                });
            };
            return self
                .admit_observation(&current, body)
                .map(NotifyOutcome::Transition);
        }
        let observation_id = operation_id(
            &mission.attempt.id,
            &mission.attempt.session_id,
            &delivery.id,
            "publication:observe:v1",
        )?;
        let payload = crate::delivery::publication_payload(&proof);
        let hash = payload_hash(&payload)?;
        match self.store.record_operation(
            &mission.id,
            &mission.attempt.id,
            &observation_id,
            &hash,
            OperationState::Uncertain,
            Some(&serde_json::to_string(&payload).map_err(Error::storage)?),
        ) {
            Ok(()) => {}
            Err(Error::OperationConflict { .. }) => {
                let current = self.store.load_mission(&mission.id)?;
                if current.phase == Phase::NeedsAttention {
                    return Ok(unchanged(&current));
                }
            }
            Err(error) => return Err(error),
        }
        let current = self.store.load_mission(&mission.id)?;
        if current.phase == Phase::Publishing || current.phase == Phase::Cancelling {
            return self.uncertainty(&current, crate::domain::AttentionReason::ExecutionUncertain);
        }
        Ok(NotifyOutcome::Recorded {
            phase: current.phase,
            revision: current.revision,
        })
    }

    fn facts(&self, mission: &MissionRecord) -> Result<Facts> {
        let delivery = self.store.load_delivery(&mission.id)?;
        let publication_receipt = if let Some(delivery) = &delivery {
            self.store
                .publication_receipt(&delivery.id)?
                .filter(|proof| proof.matches_digest(&delivery.approved_digest))
        } else {
            None
        };
        let commands = self.store.list_commands(&mission.id)?;
        Ok(Facts {
            ops_blocked: self.store.ops_blocked(&mission.id)?,
            verification: self.store.latest_verification(&mission.id)?,
            delivery,
            publication_receipt,
            pending_pause: commands.iter().any(|command| {
                command.state == CommandState::Pending && command.kind == CommandKind::Pause
            }),
            pending_cancel: commands.iter().any(|command| {
                command.state == CommandState::Pending && command.kind == CommandKind::Cancel
            }),
            acknowledged_pause: commands.iter().any(|command| {
                command.state == CommandState::Acknowledged && command.kind == CommandKind::Pause
            }),
            acknowledged_cancel: commands.iter().any(|command| {
                command.state == CommandState::Acknowledged && command.kind == CommandKind::Cancel
            }),
            admitted_runtime_work: commands.iter().any(|command| {
                command.state == CommandState::Acknowledged && is_runtime_work(command.kind)
            }),
        })
    }

    fn binding(&self, mission: &MissionRecord) -> RuntimeBinding {
        RuntimeBinding {
            mission_id: mission.id.clone(),
            attempt_id: mission.attempt.id.clone(),
            session_id: mission.attempt.session_id.clone(),
            generation: self.lock.generation,
        }
    }
}

fn snapshot_from(store: &Store, mission_id: &str, generation: u64) -> Result<MissionSnapshot> {
    let mission = store.load_mission(mission_id)?;
    let workspace = store
        .load_workspace(&mission.attempt.id)?
        .map(|workspace| workspace.view);
    let commands = store.list_commands(mission_id)?;
    let views: Vec<CommandView> = commands.into_iter().map(command_view).collect();
    let pending = views
        .iter()
        .filter(|command| command.state == CommandState::Pending.as_str())
        .cloned()
        .collect();
    let delivery = store
        .load_delivery(mission_id)?
        .map(|delivery| DeliveryView {
            id: delivery.id,
            mission_id: delivery.mission_id,
            attempt_id: delivery.attempt_id,
            target_branch: delivery.target_branch,
            base_branch: delivery.base_branch,
            approved_digest: delivery.approved_digest,
            commit_sha: delivery.commit_sha,
            tree_sha: delivery.tree_sha,
            pr_identity: delivery.pr_identity,
            publication_state: delivery.publication_state,
        });
    Ok(MissionSnapshot {
        mission_id: mission.id,
        revision: mission.revision,
        phase: mission.phase,
        objective: mission.objective,
        attempt_id: mission.attempt.id,
        task_id: mission.task_id,
        session_id: mission.attempt.session_id,
        base_sha: mission.attempt.base_sha,
        repair_count: mission.attempt.repair_count,
        max_repairs: mission.attempt.max_repairs,
        plan_revision_count: mission.attempt.plan_revision_count,
        max_plan_revisions: mission.attempt.max_plan_revisions,
        content_digest: mission.content_digest,
        plan_digest: mission.plan_digest,
        plan_body: mission.plan_body,
        reviewed_digest: mission.reviewed_digest,
        resume_target: mission.resume_target,
        prior_phase: mission.prior_phase,
        attention_reason: mission
            .attention_reason
            .map(|reason| reason.as_str().to_string()),
        permitted_recovery_target: mission.permitted_recovery_target,
        reconcile_only: mission.reconcile_only,
        recorded_owner_generation: generation,
        pending_commands: pending,
        commands: views,
        successor_of_mission_id: mission.successor_of_mission_id,
        head_sha: mission.repo.head_sha,
        runtime_adapter: mission.attempt.runtime_adapter,
        verifier_json: mission.attempt.verifier_json,
        delivery,
        workspace,
        execution_effects: store.execution_views(mission_id)?,
        execution_resolutions: store.execution_resolutions(mission_id)?,
        linux_execution_effects: store.linux_views(mission_id)?,
        linux_execution_resolutions: store.linux_resolutions(mission_id)?,
        execution_blocked: store.ops_blocked(mission_id)?,
        quarantined: store.list_quarantine(mission_id)?,
    })
}

fn command_view(command: crate::domain::CommandRecord) -> CommandView {
    let receipt_id = command.runtime_receipt_json.as_deref().and_then(|text| {
        serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|value| {
                value
                    .get("receiptId")
                    .and_then(|id| id.as_str())
                    .map(str::to_string)
            })
    });
    CommandView {
        id: command.id,
        kind: command.kind.as_str().to_string(),
        state: command.state.as_str().to_string(),
        payload_hash: command.payload_hash,
        request_id: command.request_id,
        expected_revision: command.expected_revision,
        resulting_revision: command.resulting_revision,
        attempt_id: command.attempt_id,
        origin: command.origin.as_str().to_string(),
        receipt_id,
    }
}

fn command_json(command: &CommandView) -> Value {
    json!({
        "id": command.id,
        "kind": command.kind,
        "state": command.state,
        "payloadHash": command.payload_hash,
        "requestId": command.request_id,
        "expectedRevision": command.expected_revision,
        "resultingRevision": command.resulting_revision,
        "attemptId": command.attempt_id,
        "origin": command.origin,
        "receiptId": command.receipt_id,
    })
}

struct CommandDraft {
    command_id: String,
    canonical: String,
    hash: String,
    resulting_revision: u64,
    state: CommandState,
}

fn new_command(
    request: &AdmitRequest,
    mission: &MissionRecord,
    attempt_id: &str,
    draft: CommandDraft,
) -> NewCommand {
    NewCommand {
        id: draft.command_id.clone(),
        mission_id: mission.id.clone(),
        attempt_id: attempt_id.to_string(),
        kind: request.body.kind(),
        payload_canonical: draft.canonical,
        payload_hash: draft.hash,
        expected_revision: request.expected_revision,
        resulting_revision: draft.resulting_revision,
        state: draft.state,
        request_id: format!("{attempt_id}:{}", draft.command_id),
        origin: request.origin,
        actor: request.actor.clone(),
        rationale: rationale(&request.body),
        decision_digest: None,
    }
}

fn result_of(command: &NewCommand, phase: Phase, idempotent: bool) -> AdmitResult {
    AdmitResult {
        command_id: command.id.clone(),
        request_id: command.request_id.clone(),
        payload_hash: command.payload_hash.clone(),
        state: command.state,
        revision: command.resulting_revision,
        phase,
        attempt_id: command.attempt_id.clone(),
        mission_id: command.mission_id.clone(),
        idempotent,
    }
}

fn initial_state(request: &AdmitRequest) -> CommandState {
    if request.origin == CommandOrigin::RuntimeObservation
        || !request.body.kind().is_runtime_bound()
    {
        CommandState::Reconciled
    } else {
        CommandState::Pending
    }
}

fn is_runtime_observation(kind: CommandKind) -> bool {
    matches!(
        kind,
        CommandKind::PlanCompleted
            | CommandKind::CompletePass
            | CommandKind::ReconcileDeliveryComplete
            | CommandKind::DiscoverPublicationCompleted
    )
}

fn suppress_pending(phase: Phase, kind: CommandKind, admitted_runtime_work: bool) -> bool {
    if matches!(kind, CommandKind::Pause | CommandKind::Cancel) && admitted_runtime_work {
        return false;
    }
    let closed = phase.is_terminal()
        || matches!(
            phase,
            Phase::Pausing | Phase::Paused | Phase::Cancelling | Phase::NeedsAttention
        );
    if !closed {
        return false;
    }
    let stop_still_required = (kind == CommandKind::Pause && phase == Phase::Pausing)
        || (kind == CommandKind::Cancel && phase == Phase::Cancelling);
    !stop_still_required
}

fn is_runtime_work(kind: CommandKind) -> bool {
    kind.is_runtime_bound() && !matches!(kind, CommandKind::Pause | CommandKind::Cancel)
}

/// The latest phase-entry command identifies the current planning/repair
/// activity. A delivered abort ends submissions before it; later pending
/// commands keep their original identity, including an admission whose reply
/// was lost. `list_commands` supplies durable insertion order.
fn resumable_work_for(commands: &[crate::domain::CommandRecord], phase: Phase) -> bool {
    let starts_activity = |kind| match phase {
        Phase::Planning => {
            matches!(
                kind,
                CommandKind::StartPlanning | CommandKind::RequestRevisedPlan
            )
        }
        Phase::Implementing => matches!(
            kind,
            CommandKind::ApprovePlan
                | CommandKind::RequestChanges
                | CommandKind::AuthorizeBoundedRepair
        ),
        _ => false,
    };
    let Some(start) = commands
        .iter()
        .rposition(|command| starts_activity(command.kind))
    else {
        return false;
    };
    for command in commands[start..].iter().rev() {
        if matches!(command.kind, CommandKind::Pause | CommandKind::Cancel)
            && command.state == CommandState::Acknowledged
        {
            return false;
        }
        if (starts_activity(command.kind) || command.kind == CommandKind::Continue)
            && matches!(
                command.state,
                CommandState::Pending | CommandState::Acknowledged
            )
        {
            return true;
        }
    }
    false
}

fn verification_record(
    outcome: VerificationOutcome,
    content_digest: &str,
    evidence: &str,
    requirements: &[String],
    completeness: &str,
) -> NewVerification {
    let completeness = completeness
        .parse::<Completeness>()
        .unwrap_or(Completeness::Incomplete);
    NewVerification {
        outcome,
        content_digest: content_digest.to_string(),
        evidence_body: evidence.to_string(),
        completeness,
        requirements: requirements.to_vec(),
    }
}

fn runtime_method(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Pause | CommandKind::Cancel => "abort",
        CommandKind::Steer => "steer",
        _ => "submit",
    }
}

fn rejection_json(category: &str, adapter: &str) -> String {
    json!({
        "admission": "rejected",
        "category": category,
        "adapter": adapter,
        "credentials": "none",
    })
    .to_string()
}

fn receipt_json(receipt: &RuntimeReceipt, adapter: &str) -> String {
    json!({
        "receiptId": receipt.receipt_id,
        "requestId": receipt.request_id,
        "payloadHash": receipt.payload_hash,
        "adapter": adapter,
        "credentials": "none",
    })
    .to_string()
}

fn unchanged(mission: &MissionRecord) -> NotifyOutcome {
    NotifyOutcome::Unchanged {
        phase: mission.phase,
        revision: mission.revision,
    }
}

fn rationale(body: &CommandBody) -> String {
    match body {
        CommandBody::RequestRevisedPlan { feedback }
        | CommandBody::RequestChanges { feedback, .. } => feedback.clone(),
        CommandBody::AuthorizeBoundedRepair { rationale }
        | CommandBody::Reject { rationale }
        | CommandBody::ApproveReview { rationale, .. } => rationale.clone(),
        CommandBody::Steer { content } => content.clone(),
        CommandBody::Continue { activity } => format!("continue {activity}"),
        CommandBody::DetectUncertainty { reason, .. } => reason.as_str().to_string(),
        other => other.kind().as_str().to_string(),
    }
}

#[cfg(all(test, feature = "pi-contract"))]
#[path = "pi_host_tests.rs"]
mod pi_host_tests;
