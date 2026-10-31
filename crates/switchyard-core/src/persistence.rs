use std::path::Path;
use std::time::Duration;

use rusqlite::{
    Connection, OpenFlags, OptionalExtension, Transaction, TransactionBehavior, params,
};
use serde_json::{Value, json};

use crate::canonical::{canonicalize, payload_hash};
use crate::delivery::{publication_operation_id, publication_payload, publication_payload_hash};
use crate::domain::{
    AttemptRecord, AttentionReason, CommandKind, CommandOrigin, CommandRecord, CommandState,
    Completeness, DeliveryRecord, MissionRecord, OperationState, Phase, PublicationProof,
    RepoIdentity, SourceIssue, VerificationFact, now_ms,
};
use crate::error::{Error, Result};
use crate::execution::OwnerLock;
use crate::policy::TransitionPlan;
use crate::verification::NewVerification;

const VERIFIER_JSON: &str = r#"{"executes":false,"frozen":true,"id":"switchyard.verifier.v1"}"#;
const ENVIRONMENT_JSON: &str = r#"{"confinement":"deferred_to_s3","host":"single"}"#;

pub struct Store {
    conn: Connection,
    owner_generation: Option<u64>,
}

pub struct NewCommand {
    pub id: String,
    pub mission_id: String,
    pub attempt_id: String,
    pub kind: CommandKind,
    pub payload_canonical: String,
    pub payload_hash: String,
    pub expected_revision: u64,
    pub resulting_revision: u64,
    pub state: CommandState,
    pub request_id: String,
    pub origin: CommandOrigin,
    pub actor: String,
    pub rationale: String,
    pub decision_digest: Option<String>,
}

pub struct CreatedMission {
    pub runtime_config: String,
    pub id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub session_id: String,
    pub objective: String,
    pub acceptance: String,
    pub repo: RepoIdentity,
    pub issue: SourceIssue,
    pub actor: String,
    pub max_repairs: u32,
    pub max_plan_revisions: u32,
    pub successor_of_mission_id: Option<String>,
    pub successor_of_attempt_id: Option<String>,
    pub content_digest: String,
}

impl Store {
    pub fn open_writer(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent).map_err(Error::storage)?;
        }
        let conn = Connection::open(path).map_err(Error::storage)?;
        let mut store = Self {
            conn,
            owner_generation: None,
        };
        store.configure_writer()?;
        Ok(store)
    }

    pub fn open_reader(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Err(Error::NotFound);
        }
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(Error::storage)?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA foreign_keys=ON;")
            .map_err(Error::storage)?;
        let version = user_version(&conn)?;
        if version == 0 {
            return Err(Error::Storage {
                detail: "database is not initialized".into(),
            });
        }
        if version > 2 {
            return Err(Error::Storage {
                detail: format!("database schema version {version} is newer than this host"),
            });
        }
        let query_only: i64 = conn
            .query_row("PRAGMA query_only", [], |row| row.get(0))
            .map_err(Error::storage)?;
        if query_only != 1 {
            return Err(Error::Storage {
                detail: "read-only inspection could not enable query_only".into(),
            });
        }
        Ok(Self {
            conn,
            owner_generation: None,
        })
    }

    pub fn configure_writer(&mut self) -> Result<()> {
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))
            .map_err(Error::storage)?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(Error::Storage {
                detail: format!("journal_mode is {mode}, expected wal"),
            });
        }
        self.conn
            .execute_batch("PRAGMA synchronous=FULL; PRAGMA foreign_keys=ON;")
            .map_err(Error::storage)?;
        self.conn
            .busy_timeout(Duration::from_secs(5))
            .map_err(Error::storage)?;
        let synchronous: i64 = self
            .conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(Error::storage)?;
        if synchronous != 2 {
            return Err(Error::Storage {
                detail: format!("synchronous is {synchronous}, expected FULL (2)"),
            });
        }
        Ok(())
    }

    pub fn bind_owner_generation(&mut self, generation: u64) {
        self.owner_generation = Some(generation);
    }

    /// A ledger never silently changes adapters, source pins, or adapter code.
    pub fn require_runtime_config(&self, expected: &Value) -> Result<()> {
        let mut stmt = self
            .conn
            .prepare("SELECT runtime_config_json FROM attempts")
            .map_err(Error::storage)?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(Error::storage)?;
        for row in rows {
            let mut config: Value =
                serde_json::from_str(&row.map_err(Error::storage)?).map_err(Error::storage)?;
            if let Some(object) = config.as_object_mut() {
                object.remove("sessionInitialized");
            }
            if &config != expected {
                return Err(Error::guard(runtime_config_mismatch(&config, expected)));
            }
        }
        Ok(())
    }

    #[cfg(feature = "pi-contract")]
    pub fn runtime_initialized(&self, attempt_id: &str) -> Result<bool> {
        let raw: String = self
            .conn
            .query_row(
                "SELECT runtime_config_json FROM attempts WHERE id=?1",
                [attempt_id],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        let config: Value = serde_json::from_str(&raw).map_err(Error::storage)?;
        match config.get("sessionInitialized") {
            None | Some(Value::Bool(false)) => Ok(false),
            Some(Value::Bool(true)) => Ok(true),
            _ => Err(Error::guard("invalid runtime initialization marker")),
        }
    }

    /// Commit before the first command can reach Pi. If this bit is true, open
    /// must find the original session; absent/empty storage is never a new run.
    #[cfg(feature = "pi-contract")]
    pub fn mark_runtime_initialized(&mut self, attempt_id: &str) -> Result<()> {
        let tx = self.immediate()?;
        let raw: String = tx
            .query_row(
                "SELECT runtime_config_json FROM attempts WHERE id=?1",
                [attempt_id],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        let mut config: Value = serde_json::from_str(&raw).map_err(Error::storage)?;
        config["sessionInitialized"] = json!(true);
        tx.execute(
            "UPDATE attempts SET runtime_config_json=?1 WHERE id=?2",
            params![canonicalize(&config)?, attempt_id],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }

    pub fn durability(&self) -> Result<(String, i64)> {
        let mode: String = self
            .conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .map_err(Error::storage)?;
        let synchronous: i64 = self
            .conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .map_err(Error::storage)?;
        Ok((mode, synchronous))
    }

    pub fn migrate(&mut self) -> Result<()> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    version INTEGER PRIMARY KEY,
                    applied_at INTEGER NOT NULL
                );",
            )
            .map_err(Error::storage)?;
        let current: i64 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        if current > 2 {
            return Err(Error::Storage {
                detail: format!("database schema version {current} is newer than this host"),
            });
        }
        if current == 0 {
            let tx = self.immediate()?;
            tx.execute_batch(include_str!("migrations/001_init.sql"))
                .map_err(Error::storage)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (1, ?1)",
                [now_ms()],
            )
            .map_err(Error::storage)?;
            tx.execute_batch("PRAGMA user_version = 1;")
                .map_err(Error::storage)?;
            tx.commit().map_err(Error::storage)?;
        }
        if current < 2 {
            // Rebuild commands so a definite admission refusal can be stored.
            // foreign_keys cannot change inside a transaction.
            self.conn
                .execute_batch("PRAGMA foreign_keys=OFF;")
                .map_err(Error::storage)?;
            let tx = self.immediate()?;
            tx.execute_batch(include_str!("migrations/002_rejected_command.sql"))
                .map_err(Error::storage)?;
            tx.execute(
                "INSERT INTO schema_migrations(version, applied_at) VALUES (2, ?1)",
                [now_ms()],
            )
            .map_err(Error::storage)?;
            tx.execute_batch("PRAGMA user_version = 2;")
                .map_err(Error::storage)?;
            tx.commit().map_err(Error::storage)?;
            self.conn
                .execute_batch("PRAGMA foreign_keys=ON;")
                .map_err(Error::storage)?;
        }
        Ok(())
    }

    pub fn bump_generation(&mut self, lock: &mut OwnerLock) -> Result<()> {
        let tx = self.immediate()?;
        let current: Option<i64> = tx
            .query_row(
                "SELECT generation FROM host_owner WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::storage)?;
        let next = current.unwrap_or(0).saturating_add(1);
        let token = crate::domain::new_id("owner");
        let acquired = now_ms();
        tx.execute(
            "INSERT INTO host_owner(singleton, generation, owner_token, host_id, pid, acquired_at)
             VALUES (1, ?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(singleton) DO UPDATE SET
                generation = excluded.generation,
                owner_token = excluded.owner_token,
                host_id = excluded.host_id,
                pid = excluded.pid,
                acquired_at = excluded.acquired_at",
            params![next, token, lock.host_id, lock.pid, acquired],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)?;
        lock.generation = u64::try_from(next).unwrap_or(0);
        lock.token = token;
        Ok(())
    }

    pub fn generation(&self) -> Result<u64> {
        let value: Option<i64> = self
            .conn
            .query_row(
                "SELECT generation FROM host_owner WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::storage)?;
        Ok(u64::try_from(value.unwrap_or(0)).unwrap_or(0))
    }

    pub fn create_mission(&mut self, created: CreatedMission) -> Result<()> {
        let tx = self.immediate()?;
        let active: i64 = tx
            .query_row(
                "SELECT COUNT(*) FROM missions WHERE phase NOT IN ('published', 'rejected', 'cancelled')",
                [],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        if active > 0 {
            return Err(Error::ActiveMissionExists);
        }
        let now = now_ms();
        let policy_value = json!({
            "maxRepairs": created.max_repairs,
            "maxPlanRevisions": created.max_plan_revisions,
            "planningTools": "read-only",
            "credentials": "none"
        });
        let policy_json = canonicalize(&policy_value)?;
        let policy_hash = payload_hash(&policy_value)?;
        let repo_json = serde_json::to_string(&json!({
            "toplevel": created.repo.toplevel,
            "headSha": created.repo.head_sha,
            "branch": created.repo.branch,
            "porcelainHash": created.repo.porcelain_hash,
            "originUrl": created.repo.origin_url,
        }))
        .map_err(Error::storage)?;
        let issue_json = serde_json::to_string(&json!({
            "provider": created.issue.provider,
            "number": created.issue.number,
            "title": created.issue.title,
            "snapshotAt": created.issue.snapshot_at,
        }))
        .map_err(Error::storage)?;
        tx.execute(
            "INSERT INTO missions(
                id, revision, phase, objective, repo_identity_json, source_issue_json,
                resume_target, prior_phase, attention_reason, permitted_recovery_target,
                reconcile_only, content_digest, plan_digest, plan_body, reviewed_digest,
                successor_of_mission_id, created_at, updated_at
            ) VALUES (?1, 1, 'draft', ?2, ?3, ?4, NULL, NULL, NULL, NULL, 0, ?5, NULL, NULL, NULL, ?6, ?7, ?7)",
            params![
                created.id,
                created.objective,
                repo_json,
                issue_json,
                created.content_digest,
                created.successor_of_mission_id,
                now
            ],
        )
        .map_err(Error::storage)?;
        tx.execute(
            "INSERT INTO tasks(id, mission_id, acceptance_criteria, root, frozen, created_at)
             VALUES (?1, ?2, ?3, 1, 1, ?4)",
            params![created.task_id, created.id, created.acceptance, now],
        )
        .map_err(Error::storage)?;
        tx.execute(
            "INSERT INTO attempts(
                id, mission_id, task_id, base_sha, session_id, runtime_config_json, policy_json,
                policy_hash, verifier_json, environment_json, successor_of, active, repair_count,
                max_repairs, plan_revision_count, max_plan_revisions, created_at
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 1, 0, ?12, 0, ?13, ?14)",
            params![
                created.attempt_id,
                created.id,
                created.task_id,
                created.repo.head_sha,
                created.session_id,
                created.runtime_config,
                policy_json,
                policy_hash,
                VERIFIER_JSON,
                ENVIRONMENT_JSON,
                created.successor_of_attempt_id,
                created.max_repairs,
                created.max_plan_revisions,
                now
            ],
        )
        .map_err(Error::storage)?;
        insert_decision(
            &tx,
            DecisionWrite {
                mission_id: &created.id,
                attempt_id: &created.attempt_id,
                actor: &created.actor,
                action: "create",
                rationale: "mission created",
                digest: Some(&created.content_digest),
                command_id: None,
            },
        )?;
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    pub fn load_mission(&self, mission_id: &str) -> Result<MissionRecord> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT
                    m.id, m.revision, m.phase, m.objective, m.repo_identity_json, m.source_issue_json,
                    m.resume_target, m.prior_phase, m.attention_reason, m.permitted_recovery_target,
                    m.reconcile_only, m.content_digest, m.plan_digest, m.plan_body, m.reviewed_digest,
                    m.successor_of_mission_id, m.created_at, m.updated_at,
                    t.id, t.acceptance_criteria,
                    a.id, a.task_id, a.base_sha, a.session_id, a.runtime_config_json, a.policy_json,
                    a.policy_hash, a.verifier_json, a.repair_count, a.max_repairs,
                    a.plan_revision_count, a.max_plan_revisions, a.successor_of
                 FROM missions m
                 JOIN tasks t ON t.mission_id = m.id AND t.root = 1
                 JOIN attempts a ON a.mission_id = m.id AND a.active = 1
                 WHERE m.id = ?1",
            )
            .map_err(Error::storage)?;
        let rows = stmt
            .query_map(params![mission_id], map_mission)
            .map_err(Error::storage)?;
        let mut missions = Vec::new();
        for row in rows {
            missions.push(row.map_err(Error::storage)?);
        }
        match missions.len() {
            0 => Err(Error::NotFound),
            1 => Ok(missions.remove(0)),
            _ => Err(Error::Storage {
                detail: "mission has more than one active attempt or root task".into(),
            }),
        }
    }

    pub fn list_mission_ids(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM missions ORDER BY created_at, id")
            .map_err(Error::storage)?;
        let rows = stmt
            .query_map([], |row| row.get(0))
            .map_err(Error::storage)?;
        let mut ids = Vec::new();
        for row in rows {
            ids.push(row.map_err(Error::storage)?);
        }
        Ok(ids)
    }

    pub fn find_command(&self, command_id: &str) -> Result<Option<CommandRecord>> {
        self.conn
            .query_row(
                "SELECT id, mission_id, attempt_id, kind, payload_hash, expected_revision,
                        resulting_revision, state, request_id, origin, runtime_receipt_json, payload_canonical
                 FROM commands WHERE id = ?1",
                params![command_id],
                map_command,
            )
            .optional()
            .map_err(Error::storage)
    }

    pub fn list_commands(&self, mission_id: &str) -> Result<Vec<CommandRecord>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, mission_id, attempt_id, kind, payload_hash, expected_revision,
                        resulting_revision, state, request_id, origin, runtime_receipt_json, payload_canonical
                 FROM commands WHERE mission_id = ?1 ORDER BY seq, id",
            )
            .map_err(Error::storage)?;
        let rows = stmt
            .query_map(params![mission_id], map_command)
            .map_err(Error::storage)?;
        let mut commands = Vec::new();
        for row in rows {
            commands.push(row.map_err(Error::storage)?);
        }
        Ok(commands)
    }

    pub fn apply_transition(
        &mut self,
        plan: &TransitionPlan,
        command: &NewCommand,
        verification: Option<&NewVerification>,
        continuation: Option<&NewCommand>,
    ) -> Result<()> {
        let tx = self.immediate()?;
        if let Some(verification) = verification {
            insert_verification(&tx, &command.mission_id, &command.attempt_id, verification)?;
        }
        let changed = tx
            .execute(
                "UPDATE missions SET
                    revision = ?1,
                    phase = ?2,
                    resume_target = ?3,
                    prior_phase = ?4,
                    attention_reason = ?5,
                    permitted_recovery_target = ?6,
                    reconcile_only = ?7,
                    content_digest = ?8,
                    plan_digest = ?9,
                    plan_body = ?10,
                    reviewed_digest = ?11,
                    updated_at = ?12
                 WHERE id = ?13 AND revision = ?14",
                params![
                    i64_from_u64(command.resulting_revision)?,
                    plan.next.as_str(),
                    opt_phase(plan.resume_target),
                    opt_phase(plan.prior_phase),
                    plan.attention_reason
                        .map(|reason| reason.as_str().to_string()),
                    opt_phase(plan.permitted_recovery_target),
                    i64::from(plan.reconcile_only),
                    plan.content_digest,
                    plan.plan_digest,
                    plan.plan_body,
                    plan.reviewed_digest,
                    now_ms(),
                    command.mission_id,
                    i64_from_u64(command.expected_revision)?,
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(stale_or_missing(
                &tx,
                &command.mission_id,
                command.expected_revision,
            )?);
        }
        tx.execute(
            "UPDATE attempts SET repair_count = ?1, plan_revision_count = ?2 WHERE id = ?3",
            params![
                plan.repair_count,
                plan.plan_revision_count,
                command.attempt_id
            ],
        )
        .map_err(Error::storage)?;
        if plan.invalidate_evidence {
            tx.execute(
                "UPDATE evidence SET invalidated = 1 WHERE mission_id = ?1 AND invalidated = 0",
                params![command.mission_id],
            )
            .map_err(Error::storage)?;
            tx.execute(
                "UPDATE verifications SET invalidated = 1 WHERE mission_id = ?1 AND invalidated = 0",
                params![command.mission_id],
            )
            .map_err(Error::storage)?;
        }
        if plan.invalidate_reviews {
            tx.execute(
                "UPDATE decisions SET invalidated = 1
                 WHERE mission_id = ?1 AND action = 'approve_review' AND invalidated = 0",
                params![command.mission_id],
            )
            .map_err(Error::storage)?;
        }
        if plan.invalidate_plan_approval {
            tx.execute(
                "UPDATE decisions SET invalidated = 1
                 WHERE mission_id = ?1 AND action = 'approve_plan' AND invalidated = 0",
                params![command.mission_id],
            )
            .map_err(Error::storage)?;
        }
        if let Some(delivery) = &plan.delivery_insert {
            tx.execute(
                "INSERT INTO deliveries(
                    id, mission_id, attempt_id, target_branch, base_branch, approved_digest,
                    tree_sha, commit_sha, pr_identity, publication_state, created_at, updated_at
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, NULL, NULL, 'intent_recorded', ?7, ?7)",
                params![
                    crate::domain::new_id("dl"),
                    command.mission_id,
                    command.attempt_id,
                    delivery.target_branch,
                    delivery.base_branch,
                    delivery.approved_digest,
                    now_ms()
                ],
            )
            .map_err(Error::storage)?;
        }
        if let Some(proof) = &plan.delivery_publish {
            let delivery_id: String = tx
                .query_row(
                    "SELECT id FROM deliveries WHERE mission_id = ?1",
                    params![command.mission_id],
                    |row| row.get(0),
                )
                .map_err(|error| Error::Storage {
                    detail: format!("delivery intent missing while publishing proof: {error}"),
                })?;
            tx.execute(
                "UPDATE deliveries SET
                    tree_sha = ?1,
                    commit_sha = ?2,
                    pr_identity = ?3,
                    publication_state = 'published',
                    updated_at = ?4
                 WHERE id = ?5",
                params![
                    proof.tree_sha,
                    proof.commit_sha,
                    proof.pr_id,
                    now_ms(),
                    delivery_id
                ],
            )
            .map_err(Error::storage)?;
            upsert_publication_receipt(
                &tx,
                &command.mission_id,
                &command.attempt_id,
                &delivery_id,
                proof,
            )?;
        }
        insert_command(&tx, command)?;
        if let Some(continuation) = continuation {
            insert_command(&tx, continuation)?;
        }
        insert_decision(
            &tx,
            DecisionWrite {
                mission_id: &command.mission_id,
                attempt_id: &command.attempt_id,
                actor: &command.actor,
                action: command.kind.as_str(),
                rationale: &command.rationale,
                digest: command.decision_digest.as_deref(),
                command_id: Some(&command.id),
            },
        )?;
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    pub fn insert_steer(&mut self, command: &NewCommand) -> Result<()> {
        let tx = self.immediate()?;
        let actual: i64 = tx
            .query_row(
                "SELECT revision FROM missions WHERE id = ?1",
                params![command.mission_id],
                |row| row.get(0),
            )
            .map_err(|_| Error::NotFound)?;
        let actual = u64::try_from(actual).unwrap_or(0);
        if actual != command.expected_revision {
            return Err(Error::StaleRevision {
                expected: command.expected_revision,
                actual,
            });
        }
        insert_command(&tx, command)?;
        insert_decision(
            &tx,
            DecisionWrite {
                mission_id: &command.mission_id,
                attempt_id: &command.attempt_id,
                actor: &command.actor,
                action: "steer",
                rationale: &command.rationale,
                digest: None,
                command_id: Some(&command.id),
            },
        )?;
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    pub fn acknowledge(&mut self, command_id: &str, receipt_json: &str) -> Result<()> {
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE commands SET state = 'acknowledged', runtime_receipt_json = ?1, updated_at = ?2
                 WHERE id = ?3 AND state = 'pending'",
                params![receipt_json, now_ms(), command_id],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM commands WHERE id = ?1",
                    params![command_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Error::storage)?;
            if state.as_deref() == Some("acknowledged") || state.as_deref() == Some("reconciled") {
                tx.commit().map_err(Error::storage)?;
                return Ok(());
            }
            return Err(Error::Storage {
                detail: format!("command {command_id} could not be acknowledged"),
            });
        }
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    pub fn supersede_pending(&mut self, command_id: &str, reason: &str) -> Result<()> {
        let tx = self.immediate()?;
        let receipt = serde_json::to_string(&json!({
            "superseded": true,
            "reason": reason,
        }))
        .map_err(Error::storage)?;
        let changed = tx
            .execute(
                "UPDATE commands SET state = 'superseded', runtime_receipt_json = ?1, updated_at = ?2
                 WHERE id = ?3 AND state = 'pending'",
                params![receipt, now_ms(), command_id],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::Storage {
                detail: format!("command {command_id} could not be superseded"),
            });
        }
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    /// Record that Pi refused this pending command and stored no receipt.
    pub fn reject_pending(&mut self, command_id: &str, receipt_json: &str) -> Result<()> {
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE commands SET state = 'rejected', runtime_receipt_json = ?1, updated_at = ?2
                 WHERE id = ?3 AND state = 'pending'",
                params![receipt_json, now_ms(), command_id],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            let state: Option<String> = tx
                .query_row(
                    "SELECT state FROM commands WHERE id = ?1",
                    params![command_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Error::storage)?;
            if state.as_deref() == Some("rejected") {
                tx.commit().map_err(Error::storage)?;
                return Ok(());
            }
            return Err(Error::Storage {
                detail: format!("command {command_id} could not be rejected"),
            });
        }
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    #[cfg(feature = "pi-contract")]
    pub fn attempt_has_successor(&self, attempt_id: &str) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM attempts WHERE successor_of = ?1",
                [attempt_id],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        Ok(count > 0)
    }

    pub fn observe_content(
        &mut self,
        mission_id: &str,
        expected_revision: u64,
        digest: &str,
        actor: &str,
        attempt_id: &str,
    ) -> Result<u64> {
        let tx = self.immediate()?;
        let new_revision = expected_revision.saturating_add(1);
        let changed = tx
            .execute(
                "UPDATE missions SET content_digest = ?1, revision = ?2, updated_at = ?3
                 WHERE id = ?4 AND revision = ?5",
                params![
                    digest,
                    i64_from_u64(new_revision)?,
                    now_ms(),
                    mission_id,
                    i64_from_u64(expected_revision)?
                ],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(stale_or_missing(&tx, mission_id, expected_revision)?);
        }
        tx.execute(
            "UPDATE evidence SET invalidated = 1 WHERE mission_id = ?1 AND invalidated = 0",
            params![mission_id],
        )
        .map_err(Error::storage)?;
        tx.execute(
            "UPDATE verifications SET invalidated = 1 WHERE mission_id = ?1 AND invalidated = 0",
            params![mission_id],
        )
        .map_err(Error::storage)?;
        insert_decision(
            &tx,
            DecisionWrite {
                mission_id,
                attempt_id,
                actor,
                action: "content_observed",
                rationale: "content digest updated without a phase transition",
                digest: Some(digest),
                command_id: None,
            },
        )?;
        tx.commit().map_err(Error::storage)?;
        Ok(new_revision)
    }

    pub fn record_verification(
        &mut self,
        mission_id: &str,
        attempt_id: &str,
        verification: &NewVerification,
    ) -> Result<String> {
        let tx = self.immediate()?;
        let id = insert_verification(&tx, mission_id, attempt_id, verification)?;
        tx.commit().map_err(Error::storage)?;
        Ok(id)
    }

    pub fn quarantine_notification(
        &mut self,
        mission_id: &str,
        error_category: &str,
        reason: &str,
        frame_json: &str,
    ) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute(
            "INSERT INTO notification_quarantine(
                mission_id, error_category, reason, frame_json, created_at
             ) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![mission_id, error_category, reason, frame_json, now_ms()],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    pub fn list_quarantine(&self, mission_id: &str) -> Result<Vec<(i64, String, String, String)>> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, error_category, reason, frame_json
                 FROM notification_quarantine
                 WHERE mission_id = ?1
                 ORDER BY id",
            )
            .map_err(Error::storage)?;
        let rows = stmt
            .query_map(params![mission_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(Error::storage)?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(Error::storage)?);
        }
        Ok(records)
    }

    pub fn latest_verification(&self, mission_id: &str) -> Result<Option<VerificationFact>> {
        self.conn
            .query_row(
                "SELECT v.outcome, v.content_digest, e.completeness
                 FROM verifications v
                 JOIN evidence e ON e.id = v.evidence_id
                 WHERE v.mission_id = ?1 AND v.invalidated = 0
                 ORDER BY v.seq DESC
                 LIMIT 1",
                params![mission_id],
                |row| {
                    let outcome: String = row.get(0)?;
                    let content_digest: String = row.get(1)?;
                    let completeness: String = row.get(2)?;
                    Ok((outcome, content_digest, completeness))
                },
            )
            .optional()
            .map_err(Error::storage)?
            .map(|(outcome, content_digest, completeness)| {
                Ok(VerificationFact {
                    outcome: outcome.parse()?,
                    content_digest,
                    complete: completeness.parse::<Completeness>()? == Completeness::Complete,
                })
            })
            .transpose()
    }

    pub fn record_operation(
        &mut self,
        mission_id: &str,
        attempt_id: &str,
        operation_id: &str,
        payload_hash: &str,
        state: OperationState,
        outcome_json: Option<&str>,
    ) -> Result<()> {
        let tx = self.immediate()?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT payload_hash, state FROM operation_receipts WHERE operation_id = ?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(Error::storage)?;
        if let Some((existing_hash, _)) = existing {
            if existing_hash != payload_hash {
                return Err(Error::OperationConflict {
                    operation_id: operation_id.to_string(),
                });
            }
            return Ok(());
        }
        tx.execute(
            "INSERT INTO operation_receipts(
                operation_id, attempt_id, mission_id, command_id, payload_hash, state, outcome_json, created_at
            ) VALUES (?1, ?2, ?3, NULL, ?4, ?5, ?6, ?7)",
            params![
                operation_id,
                attempt_id,
                mission_id,
                payload_hash,
                state.as_str(),
                outcome_json,
                now_ms()
            ],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)?;
        Ok(())
    }

    pub fn ops_blocked(&self, mission_id: &str) -> Result<bool> {
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM operation_receipts
                 WHERE mission_id = ?1 AND state IN ('prepared', 'running', 'uncertain')",
                params![mission_id],
                |row| row.get(0),
            )
            .map_err(Error::storage)?;
        Ok(count > 0)
    }

    pub fn load_delivery(&self, mission_id: &str) -> Result<Option<DeliveryRecord>> {
        self.conn
            .query_row(
                "SELECT id, mission_id, attempt_id, target_branch, base_branch, approved_digest,
                        tree_sha, commit_sha, pr_identity, publication_state
                 FROM deliveries WHERE mission_id = ?1",
                params![mission_id],
                |row| {
                    Ok(DeliveryRecord {
                        id: row.get(0)?,
                        mission_id: row.get(1)?,
                        attempt_id: row.get(2)?,
                        target_branch: row.get(3)?,
                        base_branch: row.get(4)?,
                        approved_digest: row.get(5)?,
                        tree_sha: row.get(6)?,
                        commit_sha: row.get(7)?,
                        pr_identity: row.get(8)?,
                        publication_state: row.get(9)?,
                    })
                },
            )
            .optional()
            .map_err(Error::storage)
    }

    pub fn settle_operation(&mut self, operation_id: &str, payload_hash: &str) -> Result<()> {
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE operation_receipts SET state = 'completed'
                 WHERE operation_id = ?1 AND payload_hash = ?2",
                params![operation_id, payload_hash],
            )
            .map_err(Error::storage)?;
        if changed == 1 {
            tx.commit().map_err(Error::storage)?;
            return Ok(());
        }
        let existing: Option<String> = tx
            .query_row(
                "SELECT payload_hash FROM operation_receipts WHERE operation_id = ?1",
                params![operation_id],
                |row| row.get(0),
            )
            .optional()
            .map_err(Error::storage)?;
        match existing {
            Some(existing_hash) if existing_hash == payload_hash => {
                tx.commit().map_err(Error::storage)?;
                Ok(())
            }
            Some(_) => Err(Error::OperationConflict {
                operation_id: operation_id.to_string(),
            }),
            None => Err(Error::NotFound),
        }
    }

    pub fn publication_receipt(&self, delivery_id: &str) -> Result<Option<PublicationProof>> {
        let operation_id = publication_operation_id(delivery_id)?;
        let row: Option<(String, String, Option<String>)> = self
            .conn
            .query_row(
                "SELECT state, payload_hash, outcome_json FROM operation_receipts WHERE operation_id = ?1",
                params![operation_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(Error::storage)?;
        let Some((state, hash, outcome)) = row else {
            return Ok(None);
        };
        if state != OperationState::Completed.as_str() {
            return Ok(None);
        }
        let Some(outcome) = outcome else {
            return Ok(None);
        };
        let value: Value = serde_json::from_str(&outcome).map_err(Error::storage)?;
        let proof = PublicationProof {
            content_digest: json_str(&value, "contentDigest")?,
            commit_sha: json_str(&value, "commitSha")?,
            tree_sha: json_str(&value, "treeSha")?,
            pr_id: json_str(&value, "prId")?,
        };
        let actual = publication_payload_hash(&proof)?;
        if actual != hash {
            return Err(Error::Storage {
                detail: "publication receipt hash does not match its outcome".into(),
            });
        }
        Ok(Some(proof))
    }

    fn immediate(&mut self) -> Result<Transaction<'_>> {
        let expected = self.owner_generation;
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(Error::storage)?;
        if let Some(expected) = expected {
            let actual: Option<i64> = tx
                .query_row(
                    "SELECT generation FROM host_owner WHERE singleton = 1",
                    [],
                    |row| row.get(0),
                )
                .optional()
                .map_err(Error::storage)?;
            let actual = u64::try_from(actual.unwrap_or(0)).unwrap_or(0);
            if actual != expected {
                return Err(Error::StaleGeneration { expected, actual });
            }
        }
        Ok(tx)
    }
}

fn upsert_publication_receipt(
    tx: &Transaction<'_>,
    mission_id: &str,
    attempt_id: &str,
    delivery_id: &str,
    proof: &PublicationProof,
) -> Result<()> {
    let operation_id = publication_operation_id(delivery_id)?;
    let payload = publication_payload(proof);
    let hash = payload_hash(&payload)?;
    let outcome = serde_json::to_string(&payload).map_err(Error::storage)?;
    let existing: Option<String> = tx
        .query_row(
            "SELECT payload_hash FROM operation_receipts WHERE operation_id = ?1",
            params![operation_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Error::storage)?;
    if let Some(existing_hash) = existing {
        if existing_hash != hash {
            return Err(Error::OperationConflict { operation_id });
        }
        tx.execute(
            "UPDATE operation_receipts SET state = 'completed', outcome_json = ?1 WHERE operation_id = ?2",
            params![outcome, operation_id],
        )
        .map_err(Error::storage)?;
        return Ok(());
    }
    tx.execute(
        "INSERT INTO operation_receipts(
            operation_id, attempt_id, mission_id, command_id, payload_hash, state, outcome_json, created_at
         ) VALUES (?1, ?2, ?3, NULL, ?4, 'completed', ?5, ?6)",
        params![operation_id, attempt_id, mission_id, hash, outcome, now_ms()],
    )
    .map_err(Error::storage)?;
    Ok(())
}

fn insert_verification(
    tx: &Transaction<'_>,
    mission_id: &str,
    attempt_id: &str,
    verification: &NewVerification,
) -> Result<String> {
    let evidence_id = crate::domain::new_id("e");
    let verification_id = crate::domain::new_id("v");
    let now = now_ms();
    let body = serde_json::to_string(&json!({
        "body": verification.evidence_body,
        "contentDigest": verification.content_digest,
    }))
    .map_err(Error::storage)?;
    let provenance =
        serde_json::to_string(&NewVerification::provenance()).map_err(Error::storage)?;
    tx.execute(
        "INSERT INTO evidence(
            id, mission_id, attempt_id, content_hash, completeness, provenance_json,
            body_json, invalidated, created_at
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8)",
        params![
            evidence_id,
            mission_id,
            attempt_id,
            verification.content_digest,
            verification.completeness.as_str(),
            provenance,
            body,
            now
        ],
    )
    .map_err(Error::storage)?;
    tx.execute(
        "INSERT INTO verifications(
            id, mission_id, attempt_id, seq, evidence_id, outcome, requirements_json,
            content_digest, invalidated, created_at
        ) VALUES (
            ?1, ?2, ?3,
            (SELECT COALESCE(MAX(seq), 0) + 1 FROM verifications),
            ?4, ?5, ?6, ?7, 0, ?8
        )",
        params![
            verification_id,
            mission_id,
            attempt_id,
            evidence_id,
            verification.outcome.as_str(),
            verification.requirements_json(),
            verification.content_digest,
            now
        ],
    )
    .map_err(Error::storage)?;
    Ok(verification_id)
}

fn insert_command(tx: &Transaction<'_>, command: &NewCommand) -> Result<()> {
    let now = now_ms();
    tx.execute(
        "INSERT INTO commands(
            id, mission_id, attempt_id, seq, kind, payload_canonical, payload_hash, expected_revision,
            resulting_revision, state, request_id, origin, runtime_receipt_json, created_at, updated_at
         ) VALUES (
            ?1, ?2, ?3,
            (SELECT COALESCE(MAX(seq), 0) + 1 FROM commands),
            ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, NULL, ?12, ?12
         )",
        params![
            command.id,
            command.mission_id,
            command.attempt_id,
            command.kind.as_str(),
            command.payload_canonical,
            command.payload_hash,
            i64_from_u64(command.expected_revision)?,
            i64_from_u64(command.resulting_revision)?,
            command.state.as_str(),
            command.request_id,
            command.origin.as_str(),
            now
        ],
    )
    .map_err(Error::storage)?;
    Ok(())
}

struct DecisionWrite<'a> {
    mission_id: &'a str,
    attempt_id: &'a str,
    actor: &'a str,
    action: &'a str,
    rationale: &'a str,
    digest: Option<&'a str>,
    command_id: Option<&'a str>,
}

fn insert_decision(tx: &Transaction<'_>, decision: DecisionWrite<'_>) -> Result<()> {
    tx.execute(
        "INSERT INTO decisions(
            id, mission_id, attempt_id, actor, action, rationale, digest, invalidated, command_id, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0, ?8, ?9)",
        params![
            crate::domain::new_id("d"),
            decision.mission_id,
            decision.attempt_id,
            decision.actor,
            decision.action,
            decision.rationale,
            decision.digest,
            decision.command_id,
            now_ms()
        ],
    )
    .map_err(Error::storage)?;
    Ok(())
}

fn map_mission(row: &rusqlite::Row<'_>) -> rusqlite::Result<MissionRecord> {
    let repo_json: String = row.get(4)?;
    let issue_json: String = row.get(5)?;
    let runtime_json: String = row.get(24)?;
    let adapter = serde_json::from_str::<Value>(&runtime_json)
        .ok()
        .and_then(|value| {
            value
                .get("adapter")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| "fake".to_string());
    Ok(MissionRecord {
        id: row.get(0)?,
        revision: u64_col(row.get(1)?)?,
        phase: phase_col(row.get(2)?)?,
        objective: row.get(3)?,
        repo: repo_from_json(&repo_json).map_err(sqlite_error)?,
        source_issue: issue_from_json(&issue_json).map_err(sqlite_error)?,
        resume_target: opt_phase_col(row.get(6)?)?,
        prior_phase: opt_phase_col(row.get(7)?)?,
        attention_reason: opt_reason_col(row.get(8)?)?,
        permitted_recovery_target: opt_phase_col(row.get(9)?)?,
        reconcile_only: row.get::<_, i64>(10)? != 0,
        content_digest: row.get(11)?,
        plan_digest: row.get(12)?,
        plan_body: row.get(13)?,
        reviewed_digest: row.get(14)?,
        successor_of_mission_id: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        task_id: row.get(18)?,
        acceptance_criteria: row.get(19)?,
        attempt: AttemptRecord {
            id: row.get(20)?,
            task_id: row.get(21)?,
            base_sha: row.get(22)?,
            session_id: row.get(23)?,
            runtime_adapter: adapter,
            policy_json: row.get(25)?,
            policy_hash: row.get(26)?,
            verifier_json: row.get(27)?,
            repair_count: u32_col(row.get(28)?)?,
            max_repairs: u32_col(row.get(29)?)?,
            plan_revision_count: u32_col(row.get(30)?)?,
            max_plan_revisions: u32_col(row.get(31)?)?,
            successor_of: row.get(32)?,
        },
    })
}

fn map_command(row: &rusqlite::Row<'_>) -> rusqlite::Result<CommandRecord> {
    let kind: String = row.get(3)?;
    let state: String = row.get(7)?;
    let origin: String = row.get(9)?;
    Ok(CommandRecord {
        payload_canonical: row.get(11)?,
        id: row.get(0)?,
        mission_id: row.get(1)?,
        attempt_id: row.get(2)?,
        kind: kind.parse().map_err(sqlite_error)?,
        payload_hash: row.get(4)?,
        expected_revision: u64_col(row.get(5)?)?,
        resulting_revision: row.get::<_, Option<i64>>(6)?.map(u64_col).transpose()?,
        state: state.parse().map_err(sqlite_error)?,
        request_id: row.get(8)?,
        origin: origin.parse().map_err(sqlite_error)?,
        runtime_receipt_json: row.get(10)?,
    })
}

fn user_version(conn: &Connection) -> Result<i64> {
    conn.query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(Error::storage)
}

fn i64_from_u64(value: u64) -> Result<i64> {
    i64::try_from(value).map_err(|_| Error::storage("integer does not fit in sqlite"))
}

fn u64_col(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn u32_col(value: i64) -> rusqlite::Result<u32> {
    u32::try_from(value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            0,
            rusqlite::types::Type::Integer,
            Box::new(error),
        )
    })
}

fn phase_col(value: String) -> rusqlite::Result<Phase> {
    value.parse().map_err(sqlite_error)
}

fn opt_phase_col(value: Option<String>) -> rusqlite::Result<Option<Phase>> {
    value.map(phase_col).transpose()
}

fn opt_reason_col(value: Option<String>) -> rusqlite::Result<Option<AttentionReason>> {
    value
        .map(|text| text.parse().map_err(sqlite_error))
        .transpose()
}

fn opt_phase(phase: Option<Phase>) -> Option<String> {
    phase.map(|phase| phase.as_str().to_string())
}

fn sqlite_error(error: Error) -> rusqlite::Error {
    rusqlite::Error::ToSqlConversionFailure(Box::new(error))
}

fn stale_or_missing(tx: &Transaction<'_>, mission_id: &str, expected: u64) -> Result<Error> {
    let actual: Option<i64> = tx
        .query_row(
            "SELECT revision FROM missions WHERE id = ?1",
            params![mission_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(Error::storage)?;
    match actual {
        Some(actual) => Ok(Error::StaleRevision {
            expected,
            actual: u64::try_from(actual).unwrap_or(0),
        }),
        None => Ok(Error::NotFound),
    }
}

fn repo_from_json(text: &str) -> Result<RepoIdentity> {
    let value: Value = serde_json::from_str(text).map_err(Error::storage)?;
    Ok(RepoIdentity {
        toplevel: json_str(&value, "toplevel")?,
        head_sha: json_str(&value, "headSha")?,
        branch: json_str(&value, "branch")?,
        porcelain_hash: json_str(&value, "porcelainHash")?,
        origin_url: value
            .get("originUrl")
            .and_then(Value::as_str)
            .map(str::to_string),
    })
}

fn issue_from_json(text: &str) -> Result<SourceIssue> {
    let value: Value = serde_json::from_str(text).map_err(Error::storage)?;
    Ok(SourceIssue {
        provider: json_str(&value, "provider")?,
        number: value.get("number").and_then(Value::as_u64).unwrap_or(0),
        title: json_str(&value, "title")?,
        snapshot_at: value.get("snapshotAt").and_then(Value::as_i64).unwrap_or(0),
    })
}

fn json_str(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| Error::Storage {
            detail: format!("stored json is missing {key}"),
        })
}

/// Fail closed, and name every key whose stored value differs from the host.
/// `sessionInitialized` is removed by the caller before this runs.
fn runtime_config_mismatch(found: &Value, expected: &Value) -> String {
    let (Some(found_map), Some(expected_map)) = (found.as_object(), expected.as_object()) else {
        return format!(
            "ledger runtime configuration mismatch; no adapter replacement (config: expected {expected}, found {found})"
        );
    };
    let mut keys: Vec<&String> = found_map.keys().chain(expected_map.keys()).collect();
    keys.sort();
    keys.dedup();
    let parts: Vec<String> = keys
        .into_iter()
        .filter(|key| found_map.get(*key) != expected_map.get(*key))
        .map(|key| {
            format!(
                "{key}: expected {}, found {}",
                render_config_value(expected_map.get(key)),
                render_config_value(found_map.get(key))
            )
        })
        .collect();
    let detail = if parts.is_empty() {
        "configuration differs".to_string()
    } else {
        parts.join("; ")
    };
    format!("ledger runtime configuration mismatch; no adapter replacement ({detail})")
}

fn render_config_value(value: Option<&Value>) -> String {
    match value {
        None => "absent".to_string(),
        Some(value) => value.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reader_connection_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mission.sqlite");
        let mut store = Store::open_writer(&db).unwrap();
        store.migrate().unwrap();
        drop(store);
        let reader = Store::open_reader(&db).unwrap();
        let error = reader
            .conn
            .execute("DELETE FROM schema_migrations", [])
            .unwrap_err();
        let message = error.to_string().to_lowercase();
        assert!(
            message.contains("readonly")
                || message.contains("read-only")
                || message.contains("query_only"),
            "{message}"
        );
    }

    #[test]
    fn same_millisecond_verifications_follow_insertion_sequence() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mission.sqlite");
        let mut store = Store::open_writer(&db).unwrap();
        store.migrate().unwrap();
        store.create_mission(sample_mission()).unwrap();
        store
            .conn
            .execute(
                "INSERT INTO evidence(
                    id, mission_id, attempt_id, content_hash, completeness, provenance_json,
                    body_json, invalidated, created_at
                 ) VALUES ('e-z', 'm-1', 'a-1', 'digest', 'complete', '{}', '{}', 0, 100)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO evidence(
                    id, mission_id, attempt_id, content_hash, completeness, provenance_json,
                    body_json, invalidated, created_at
                 ) VALUES ('e-a', 'm-1', 'a-1', 'digest', 'complete', '{}', '{}', 0, 100)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO verifications(
                    id, mission_id, attempt_id, seq, evidence_id, outcome, requirements_json,
                    content_digest, invalidated, created_at
                 ) VALUES ('v-z', 'm-1', 'a-1', 1, 'e-z', 'fail', '[]', 'digest', 0, 100)",
                [],
            )
            .unwrap();
        store
            .conn
            .execute(
                "INSERT INTO verifications(
                    id, mission_id, attempt_id, seq, evidence_id, outcome, requirements_json,
                    content_digest, invalidated, created_at
                 ) VALUES ('v-a', 'm-1', 'a-1', 2, 'e-a', 'pass', '[]', 'digest', 0, 100)",
                [],
            )
            .unwrap();
        let latest = store.latest_verification("m-1").unwrap().unwrap();
        assert_eq!(latest.outcome, crate::domain::VerificationOutcome::Pass);
    }

    #[test]
    fn one_active_attempt_and_one_root_task() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mission.sqlite");
        let mut store = Store::open_writer(&db).unwrap();
        store.migrate().unwrap();
        store.create_mission(sample_mission()).unwrap();
        let attempt = store
            .conn
            .execute(
                "INSERT INTO attempts(
                    id, mission_id, task_id, base_sha, session_id, runtime_config_json, policy_json,
                    policy_hash, verifier_json, environment_json, successor_of, active, repair_count,
                    max_repairs, plan_revision_count, max_plan_revisions, created_at
                 ) VALUES (
                    'a-2', 'm-1', 't-1', 'abc', 's-2', '{}', '{}', 'h', '{}', '{}', NULL, 1, 0, 1, 0, 1, 1
                 )",
                [],
            )
            .unwrap_err();
        assert!(
            attempt.to_string().to_lowercase().contains("unique"),
            "{attempt}"
        );
        let task = store
            .conn
            .execute(
                "INSERT INTO tasks(id, mission_id, acceptance_criteria, root, frozen, created_at)
                 VALUES ('t-2', 'm-1', 'again', 1, 1, 1)",
                [],
            )
            .unwrap_err();
        assert!(task.to_string().to_lowercase().contains("unique"), "{task}");
        assert_eq!(store.load_mission("m-1").unwrap().attempt.id, "a-1");
    }

    #[test]
    fn v1_ledger_upgrade_keeps_command_rows_and_turns_foreign_keys_back_on() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mission.sqlite");
        let conn = Connection::open(&db).unwrap();
        conn.execute_batch(include_str!("migrations/001_init.sql"))
            .unwrap();
        conn.execute_batch(
            "CREATE TABLE schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at INTEGER NOT NULL
            );
            INSERT INTO schema_migrations(version, applied_at) VALUES (1, 1);
            PRAGMA user_version = 1;",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO missions(
                id, revision, phase, objective, repo_identity_json, source_issue_json,
                resume_target, prior_phase, attention_reason, permitted_recovery_target,
                reconcile_only, content_digest, plan_digest, plan_body, reviewed_digest,
                successor_of_mission_id, created_at, updated_at
            ) VALUES (
                'm-1', 1, 'planning', 'objective', ?1, ?2,
                NULL, NULL, NULL, NULL, 0, 'digest', NULL, NULL, NULL, NULL, 1, 1
            )",
            params![
                r#"{"toplevel":"/repo","headSha":"abc","branch":"main","porcelainHash":"sha256:00"}"#,
                r#"{"provider":"github","title":"title","number":1,"snapshotAt":0}"#,
            ],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO tasks(id, mission_id, acceptance_criteria, root, frozen, created_at)
             VALUES ('t-1', 'm-1', 'acceptance', 1, 1, 1)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO attempts(
                id, mission_id, task_id, base_sha, session_id, runtime_config_json, policy_json,
                policy_hash, verifier_json, environment_json, successor_of, active, repair_count,
                max_repairs, plan_revision_count, max_plan_revisions, created_at
            ) VALUES (
                'a-1', 'm-1', 't-1', 'abc', 's-1',
                '{\"adapter\":\"fake\",\"credentials\":\"none\"}', '{}', 'h', '{}', '{}',
                NULL, 1, 0, 1, 0, 1, 1
            )",
            [],
        )
        .unwrap();
        for (id, seq, state, request, receipt) in [
            ("c-pending", 1, "pending", "a-1:c-pending", None),
            (
                "c-ack",
                2,
                "acknowledged",
                "a-1:c-ack",
                Some(r#"{"receiptId":"r-ack"}"#),
            ),
            (
                "c-old",
                3,
                "superseded",
                "a-1:c-old",
                Some(r#"{"superseded":true,"reason":"closed"}"#),
            ),
        ] {
            conn.execute(
                "INSERT INTO commands(
                    id, mission_id, attempt_id, seq, kind, payload_canonical, payload_hash,
                    expected_revision, resulting_revision, state, request_id, origin,
                    runtime_receipt_json, created_at, updated_at
                ) VALUES (
                    ?1, 'm-1', 'a-1', ?2, 'start_planning', ?3, ?4,
                    1, 2, ?5, ?6, 'operator', ?7, 1, 1
                )",
                params![
                    id,
                    seq,
                    format!(r#"{{"kind":"start_planning","id":"{id}"}}"#),
                    format!("hash-{id}"),
                    state,
                    request,
                    receipt,
                ],
            )
            .unwrap();
        }
        drop(conn);

        let mut store = Store::open_writer(&db).unwrap();
        store.migrate().unwrap();
        let foreign_keys: i64 = store
            .conn
            .query_row("PRAGMA foreign_keys", [], |row| row.get(0))
            .unwrap();
        assert_eq!(foreign_keys, 1);
        assert_eq!(user_version(&store.conn).unwrap(), 2);
        let index: String = store
            .conn
            .query_row(
                "SELECT name FROM sqlite_master WHERE type='index' AND name='commands_mission_state'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(index, "commands_mission_state");
        let rows: Vec<(String, String, String, Option<String>)> = store
            .conn
            .prepare(
                "SELECT id, state, payload_hash, runtime_receipt_json
                 FROM commands ORDER BY seq",
            )
            .unwrap()
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        assert_eq!(
            rows,
            vec![
                (
                    "c-pending".into(),
                    "pending".into(),
                    "hash-c-pending".into(),
                    None,
                ),
                (
                    "c-ack".into(),
                    "acknowledged".into(),
                    "hash-c-ack".into(),
                    Some(r#"{"receiptId":"r-ack"}"#.into()),
                ),
                (
                    "c-old".into(),
                    "superseded".into(),
                    "hash-c-old".into(),
                    Some(r#"{"superseded":true,"reason":"closed"}"#.into()),
                ),
            ]
        );
        drop(store);

        let host = crate::Host::open(&db).unwrap();
        let snap = host.snapshot("m-1").unwrap();
        let states: Vec<_> = snap
            .commands
            .iter()
            .map(|command| (command.id.as_str(), command.state.as_str()))
            .collect();
        assert_eq!(
            states,
            vec![
                ("c-pending", "pending"),
                ("c-ack", "acknowledged"),
                ("c-old", "superseded"),
            ]
        );
    }

    #[test]
    fn runtime_config_mismatch_names_each_differing_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("mission.sqlite");
        let mut store = Store::open_writer(&db).unwrap();
        store.migrate().unwrap();
        let mut mission = sample_mission();
        mission.runtime_config = r#"{"adapter":"pi-probe-v1","credentials":"none","fingerprint":"sha256:old","node":"22.23.4","source":"abc"}"#.into();
        store.create_mission(mission).unwrap();
        let expected = json!({
            "adapter": "pi-probe-v1",
            "credentials": "none",
            "fingerprint": "sha256:new",
            "node": "22.23.5",
            "source": "def",
            "extra": true
        });
        let message = store
            .require_runtime_config(&expected)
            .unwrap_err()
            .to_string();
        assert!(
            message.contains("fingerprint: expected \"sha256:new\", found \"sha256:old\""),
            "{message}"
        );
        assert!(
            message.contains("node: expected \"22.23.5\", found \"22.23.4\""),
            "{message}"
        );
        assert!(
            message.contains("source: expected \"def\", found \"abc\""),
            "{message}"
        );
        assert!(
            message.contains("extra: expected true, found absent"),
            "{message}"
        );
        assert!(
            !message.contains("adapter:") && !message.contains("credentials:"),
            "{message}"
        );
        assert!(message.contains("no adapter replacement"), "{message}");
        store
            .conn
            .execute(
                "UPDATE attempts SET runtime_config_json=json_set(runtime_config_json,'$.sessionInitialized',json('true'))",
                [],
            )
            .unwrap();
        let matching = json!({
            "adapter": "pi-probe-v1",
            "credentials": "none",
            "fingerprint": "sha256:old",
            "node": "22.23.4",
            "source": "abc"
        });
        store.require_runtime_config(&matching).unwrap();
        let drifted = store
            .require_runtime_config(&expected)
            .unwrap_err()
            .to_string();
        assert!(!drifted.contains("sessionInitialized"), "{drifted}");
    }

    fn sample_mission() -> CreatedMission {
        CreatedMission {
            runtime_config: r#"{"adapter":"fake","credentials":"none"}"#.into(),
            id: "m-1".into(),
            task_id: "t-1".into(),
            attempt_id: "a-1".into(),
            session_id: "s-1".into(),
            objective: "objective".into(),
            acceptance: "acceptance".into(),
            content_digest: "digest".into(),
            repo: RepoIdentity {
                toplevel: "/repo".into(),
                head_sha: "abc".into(),
                branch: "main".into(),
                porcelain_hash: "sha256:00".into(),
                origin_url: None,
            },
            issue: crate::domain::SourceIssue {
                provider: "github".into(),
                number: 95,
                title: "title".into(),
                snapshot_at: 0,
            },
            actor: "operator".into(),
            max_repairs: 2,
            max_plan_revisions: 3,
            successor_of_mission_id: None,
            successor_of_attempt_id: None,
        }
    }
}
