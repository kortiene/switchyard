use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::canonical::{canonicalize, payload_hash};
use crate::delivery::{publication_operation_id, publication_payload_hash};
use crate::domain::{
    CommandBody, CommandKind, CommandOrigin, CommandState, Completeness, MissionRecord,
    OperationState, Phase, PublicationProof, SourceIssue, VerificationOutcome, now_ms,
};
use crate::error::{Error, Result};
use crate::execution::{
    FakeRuntime, OwnerLock, RuntimeBinding, RuntimeNotification, RuntimeReceipt, RuntimeRequest,
    inspect_repo, operation_id, parse_notification, try_acquire,
};
use crate::persistence::{CreatedMission, NewCommand, Store};
use crate::policy::{self, Facts};
use crate::transitions::target_spec;
use crate::verification::NewVerification;

/// Open the host, take the single-host lock, and reuse the active attempt.
///
/// Opening does not dispatch pending commands and does not create an attempt.
/// Call [`Host::dispatch_pending`] or [`Host::recover`] to deliver work that was
/// admitted before a restart.
pub struct Host {
    store: Store,
    runtime: FakeRuntime,
    lock: OwnerLock,
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
}

impl MissionSnapshot {
    pub fn to_json(&self) -> Value {
        json!({
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
        })
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
        let mut lock = try_acquire(&db_path)?;
        let mut store = Store::open_writer(&db_path)?;
        store.migrate()?;
        store.bump_generation(&mut lock)?;
        store.bind_owner_generation(lock.generation);
        let runtime = FakeRuntime::open(&journal_path(&db_path))?;
        Ok(Self {
            store,
            runtime,
            lock,
        })
    }

    pub fn generation(&self) -> u64 {
        self.lock.generation
    }

    pub fn runtime_mut(&mut self) -> &mut FakeRuntime {
        &mut self.runtime
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
        self.snapshot(&id)
    }

    pub fn admit(&mut self, request: AdmitRequest) -> Result<AdmitResult> {
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
        if request.body.kind() == CommandKind::Steer {
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
        let facts = self.facts(&mission)?;
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
        self.store.apply_transition(&plan, &command)?;
        Ok(result_of(&command, plan.next, false))
    }

    pub fn dispatch_pending(&mut self, mission_id: &str) -> Result<usize> {
        let mission = self.store.load_mission(mission_id)?;
        let pending: Vec<_> = self
            .store
            .list_commands(mission_id)?
            .into_iter()
            .filter(|command| command.state == CommandState::Pending)
            .collect();
        let binding = self.binding(&mission);
        self.accept_runtime_binding(&binding, mission_id)?;
        let mut dispatched = 0;
        for command in pending {
            if let Some(receipt) = self.runtime.stored_receipt(&command.request_id) {
                if receipt.payload_hash != command.payload_hash {
                    return Err(Error::CommandConflict {
                        command_id: command.id,
                    });
                }
                self.store
                    .acknowledge(&command.id, &receipt_json(&receipt))?;
                dispatched += 1;
                continue;
            }
            if suppress_pending(mission.phase, command.kind) {
                self.store.supersede_pending(
                    &command.id,
                    "admission closed before the runtime accepted this command",
                )?;
                continue;
            }
            let request = RuntimeRequest {
                binding: binding.clone(),
                request_id: command.request_id.clone(),
                payload_hash: command.payload_hash.clone(),
                method: runtime_method(command.kind),
            };
            let receipt = match command.kind {
                CommandKind::Pause | CommandKind::Cancel => self.runtime.abort(&request)?,
                CommandKind::Steer => self.runtime.steer(&request)?,
                _ => self.runtime.submit(&request)?,
            };
            self.store
                .acknowledge(&command.id, &receipt_json(&receipt))?;
            dispatched += 1;
        }
        Ok(dispatched)
    }

    pub fn recover(&mut self, mission_id: &str) -> Result<MissionSnapshot> {
        self.dispatch_pending(mission_id)?;
        self.poll_runtime(mission_id)?;
        self.snapshot(mission_id)
    }

    pub fn poll_runtime(&mut self, mission_id: &str) -> Result<Vec<NotifyOutcome>> {
        let mut outcomes = Vec::new();
        while let Some(note) = self.runtime.pop_notification() {
            match self.apply_notification(mission_id, note.clone()) {
                Ok(outcome) => outcomes.push(outcome),
                Err(error) => {
                    self.runtime.requeue_front(note);
                    return Err(error);
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
            RuntimeNotification::VerificationError { detail } => {
                self.record_verification_row(
                    &mission,
                    VerificationOutcome::Error,
                    &mission.content_digest,
                    &detail,
                    &[],
                    "incomplete",
                )?;
                self.uncertainty(&mission, crate::domain::AttentionReason::VerificationError)
            }
            RuntimeNotification::VerificationIncomplete { detail } => {
                self.record_verification_row(
                    &mission,
                    VerificationOutcome::Incomplete,
                    &mission.content_digest,
                    &detail,
                    &[],
                    "incomplete",
                )?;
                self.uncertainty(
                    &mission,
                    crate::domain::AttentionReason::VerificationIncomplete,
                )
            }
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
        if mission.phase == Phase::AwaitingReview && mission.content_digest == content_digest {
            return Ok(unchanged(mission));
        }
        if mission.phase != Phase::Verifying {
            return Err(Error::IllegalTransition {
                from: mission.phase.to_string(),
                command: "verification_pass".into(),
            });
        }
        self.record_verification_row(
            mission,
            VerificationOutcome::Pass,
            &content_digest,
            &evidence,
            &requirements,
            &completeness,
        )?;
        let current = self.store.load_mission(&mission.id)?;
        self.admit_observation(&current, CommandBody::CompletePass)
            .map(NotifyOutcome::Transition)
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
        Ok(Facts {
            ops_blocked: self.store.ops_blocked(&mission.id)?,
            verification: self.store.latest_verification(&mission.id)?,
            delivery,
            publication_receipt,
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

fn suppress_pending(phase: Phase, kind: CommandKind) -> bool {
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

fn runtime_method(kind: CommandKind) -> &'static str {
    match kind {
        CommandKind::Pause | CommandKind::Cancel => "abort",
        CommandKind::Steer => "steer",
        _ => "submit",
    }
}

fn receipt_json(receipt: &RuntimeReceipt) -> String {
    json!({
        "receiptId": receipt.receipt_id,
        "requestId": receipt.request_id,
        "payloadHash": receipt.payload_hash,
        "adapter": "fake",
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
        CommandBody::DetectUncertainty { reason, .. } => reason.as_str().to_string(),
        other => other.kind().as_str().to_string(),
    }
}
