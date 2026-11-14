mod cli;

use std::path::Path;
#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use cli::{Cli, Command, MissionCommand, RuntimeSelection};
use serde_json::{Value, json};
use switchyard_core::{
    AdmitRequest, CommandBody, CommandKind, CommandOrigin, CreateMission, EffectProof, Error, Host,
    Phase,
};

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(value) => {
            println!("{value}");
            // Exit 3: workspace remains blocked; inspect `dispatched` and the receipt.
            // Exit 0 is success (including persist-only). Exit 2 is rejection
            // or failure and does not prove that admission did not occur.
            if value.get("workspaceError").is_some() {
                ExitCode::from(3)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(error) => {
            let body = json!({
                "ok": false,
                "error": {
                    "category": error.category(),
                    "message": error.to_string(),
                }
            });
            eprintln!("{body}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> switchyard_core::Result<Value> {
    let runtime = cli.runtime.unwrap_or(RuntimeSelection::Fake);
    match cli.command {
        Command::Mission { action } => match action {
            MissionCommand::AbandonExecution {
                db,
                mission,
                attempt,
                session,
                operation_id,
                payload_hash,
                expected_revision,
                command_id,
                actor,
                rationale,
            } => {
                let mut host = open_host(&db, runtime)?;
                let audit =
                    host.resolve_fake_execution(&switchyard_core::ExecutionResolutionRequest {
                        command_id,
                        binding: switchyard_core::RuntimeBinding {
                            mission_id: mission.clone(),
                            attempt_id: attempt,
                            session_id: session,
                            generation: host.generation(),
                        },
                        operation_id,
                        payload_hash,
                        expected_revision,
                        actor,
                        rationale,
                        decision: switchyard_core::ExecutionResolutionDecision::Abandon,
                    })?;
                Ok(
                    json!({"ok":true, "resolution":audit, "executionBlockRetained":true,
                    "nextSteps":"The execution block remains. On abandonment the mission stays in needs_attention. You can request cancel, but unresolved effects prevent cancellation from completing. Lifting this effect's block requires mission supersede-execution with fresh Host evidence (#125); the outcome stays unknown.",
                    "mission":host.snapshot(&mission)?.to_json()}),
                )
            }
            MissionCommand::SupersedeExecution {
                db,
                mission,
                attempt,
                session,
                operation_id,
                payload_hash,
                abandonment_command_id,
                abandonment_request_hash,
                expected_revision,
                command_id,
                actor,
                rationale,
            } => {
                let mut host = open_host(&db, runtime)?;
                let audit =
                    host.supersede_execution(&switchyard_core::ExecutionSupersessionRequest {
                        command_id,
                        binding: switchyard_core::RuntimeBinding {
                            mission_id: mission.clone(),
                            attempt_id: attempt,
                            session_id: session,
                            generation: host.generation(),
                        },
                        operation_id,
                        payload_hash,
                        abandonment_command_id,
                        abandonment_request_hash,
                        expected_revision,
                        actor,
                        rationale,
                    })?;
                let snapshot = host.snapshot(&mission)?;
                Ok(json!({
                    "ok": true,
                    "supersession": audit,
                    "outcomeKnown": false,
                    "executionBlocked": snapshot.execution_blocked,
                    "nextSteps": "The effect stays uncertain and the outcome stays unknown. The workspace digest is on the supersession. This command covers only the simulated journal; a Linux row stays blocked on a default build. Other unresolved effects still block pause, cancel, and recovery.",
                    "mission": snapshot.to_json()
                }))
            }
            MissionCommand::Workspace {
                db,
                mission,
                attempt,
                root,
            } => {
                let mut host = open_host(&db, runtime)?;
                let workspace = host.prepare_workspace(&mission, &attempt, &root)?;
                Ok(json!({"ok": true, "workspace": workspace.to_json()}))
            }
            MissionCommand::Create {
                db,
                repo,
                issue,
                issue_title,
                objective,
                acceptance,
                actor,
                max_repairs,
                max_plan_revisions,
                admit,
                persist_only,
                payload,
            } => {
                let mut host = open_host(&db, runtime)?;
                let snapshot = host.create_mission(CreateMission {
                    repo,
                    issue_number: issue,
                    issue_title,
                    objective,
                    acceptance,
                    actor: actor.clone(),
                    max_repairs,
                    max_plan_revisions,
                })?;
                let schedule_scenario = runtime == RuntimeSelection::ConfinedPi
                    && !persist_only
                    && admit.as_deref() == Some("approve_plan");
                if let Some(kind) = admit {
                    let body = command_body(
                        &kind,
                        &serde_json::from_str(&payload).map_err(|error| {
                            Error::guard(format!("payload is not JSON: {error}"))
                        })?,
                    )?;
                    host.admit(AdmitRequest {
                        command_id: None,
                        mission_id: snapshot.mission_id.clone(),
                        attempt_id: Some(snapshot.attempt_id.clone()),
                        expected_revision: snapshot.revision,
                        actor,
                        origin: CommandOrigin::Operator,
                        body,
                    })?;
                    if !persist_only {
                        host.dispatch_pending(&snapshot.mission_id)?;
                    }
                }
                let mut created =
                    json!({"ok": true, "mission": host.snapshot(&snapshot.mission_id)?.to_json()});
                if schedule_scenario {
                    created["composedScenario"] =
                        json!(settle_approve_plan(&mut host, &snapshot.mission_id));
                    created["mission"] = host.snapshot(&snapshot.mission_id)?.to_json();
                }
                Ok(created)
            }
            MissionCommand::Status { db, mission } => {
                let snapshots = Host::inspect(&db, mission.as_deref())?;
                Ok(json!({
                    "ok": true,
                    "readOnly": true,
                    "missions": snapshots.iter().map(|snapshot| snapshot.to_json()).collect::<Vec<_>>(),
                }))
            }
            MissionCommand::Command {
                db,
                mission,
                kind,
                expected_revision,
                attempt,
                command_id,
                actor,
                payload,
                persist_only,
            } => {
                let mut host = open_host(&db, runtime)?;
                let body = command_body(
                    &kind,
                    &serde_json::from_str(&payload)
                        .map_err(|error| Error::guard(format!("payload is not JSON: {error}")))?,
                )?;
                let is_stop = matches!(body, CommandBody::Pause | CommandBody::Cancel);
                let approve_plan = matches!(body, CommandBody::ApprovePlan { .. });
                // Reconcile once per invocation. `--persist-only` skips that
                // and both dispatches, so it cannot create a worktree.
                // Isolated workspace failures still allow admission and stop
                // delivery. Never drain earlier work before admitting a stop.
                let workspace_error = if persist_only {
                    None
                } else {
                    match host.recover_workspace(&mission) {
                        Ok(_) => None,
                        Err(error) if error.is_isolated_workspace_failure() => Some(error),
                        Err(error) => return Err(error),
                    }
                };
                if !persist_only && !is_stop && workspace_error.is_none() {
                    host.dispatch_reconciled(&mission)?;
                }
                let current = host.snapshot(&mission)?;
                let admitted = host.admit(AdmitRequest {
                    command_id,
                    mission_id: mission.clone(),
                    attempt_id: attempt.or(Some(current.attempt_id)),
                    expected_revision,
                    actor,
                    origin: CommandOrigin::Operator,
                    body,
                })?;
                if !persist_only {
                    if is_stop || workspace_error.is_some() {
                        host.dispatch_stops(&mission).map_err(|stop_error| {
                            if let Some(error) = &workspace_error {
                                stop_error.with_context(format!(
                                    "workspace reconciliation also failed: {error}"
                                ))
                            } else {
                                stop_error
                            }
                        })?;
                    } else {
                        host.dispatch_reconciled(&mission)?;
                    }
                }
                let scenario = if runtime == RuntimeSelection::ConfinedPi
                    && approve_plan
                    && !persist_only
                    && workspace_error.is_none()
                {
                    Some(settle_approve_plan(&mut host, &mission))
                } else {
                    None
                };
                let snapshot = host.snapshot(&mission)?;
                let command = snapshot
                    .commands
                    .iter()
                    .find(|c| c.id == admitted.command_id)
                    .ok_or(Error::NotFound)?;
                let command_state = command.state.clone();
                let dispatched = command.receipt_id.is_some();
                if let Some(error) = workspace_error {
                    let mission_json = snapshot.to_json();
                    let revision = mission_json["revision"].clone();
                    return Ok(with_scenario(
                        json!({
                            "ok": true,
                            "dispatched": dispatched,
                            "idempotent": admitted.idempotent,
                            "commandId": admitted.command_id,
                            "requestId": admitted.request_id,
                            "payloadHash": admitted.payload_hash,
                            "commandState": command_state,
                            "revision": revision,
                            "workspaceError": {
                                "category": error.category(),
                                "message": error.to_string(),
                            },
                            "mission": mission_json,
                        }),
                        scenario,
                    ));
                }
                Ok(with_scenario(
                    json!({
                        "ok": true,
                        "dispatched": dispatched,
                        "idempotent": admitted.idempotent,
                        "commandId": admitted.command_id,
                        "requestId": admitted.request_id,
                        "payloadHash": admitted.payload_hash,
                        "commandState": command_state,
                        "mission": snapshot.to_json(),
                    }),
                    scenario,
                ))
            }
            MissionCommand::Recover { db, mission } => {
                let mut host = open_host(&db, runtime)?;
                let snapshot = host.recover(&mission)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
            MissionCommand::Notify {
                db,
                mission,
                json_body,
            } => {
                let mut host = open_host(&db, runtime)?;
                let value: Value = serde_json::from_str(&json_body)
                    .map_err(|error| Error::guard(format!("notification is not JSON: {error}")))?;
                let outcome = host.apply_notification_json(&mission, &value)?;
                Ok(json!({
                    "ok": true,
                    "phase": outcome.phase().as_str(),
                    "revision": outcome.revision(),
                    "mission": host.snapshot(&mission)?.to_json(),
                }))
            }
            MissionCommand::Lock { db } => {
                let host = open_host(&db, runtime)?;
                println!(
                    "{}",
                    json!({
                        "ok": true,
                        "holding": true,
                        "generation": host.generation(),
                    })
                );
                let _ = std::io::Write::flush(&mut std::io::stdout());
                let mut ignored = String::new();
                let _ = std::io::Read::read_to_string(&mut std::io::stdin(), &mut ignored);
                Ok(json!({"ok": true, "released": true, "generation": host.generation()}))
            }
            MissionCommand::Successor { db, mission, actor } => {
                let mut host = open_host(&db, runtime)?;
                let snapshot = host.create_successor(&mission, &actor)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
            MissionCommand::Content {
                db,
                mission,
                digest,
                expected_revision,
                actor,
            } => {
                let mut host = open_host(&db, runtime)?;
                let snapshot =
                    host.note_content_digest(&mission, expected_revision, &digest, &actor)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
        },
    }
}

fn open_host(db: &Path, runtime: RuntimeSelection) -> switchyard_core::Result<Host> {
    match runtime {
        RuntimeSelection::Fake => Host::open(db).map_err(suggest_confined_runtime),
        RuntimeSelection::ConfinedPi => open_confined(db),
    }
}

fn suggest_confined_runtime(error: Error) -> Error {
    if error.to_string().contains("no adapter replacement") {
        error.with_context(
            "pass --runtime confined-pi to reopen a confined ledger; FakeRuntime cannot replace it",
        )
    } else {
        error
    }
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn open_confined(db: &Path) -> switchyard_core::Result<Host> {
    use switchyard_core::{ConfinedPiConfig, LinuxExecutionConfig};

    let pi_runtime = env_path("SWITCHYARD_PI_RUNTIME")?;
    let bubblewrap = env_path("SWITCHYARD_BWRAP")?;
    let cgroup_root = env_path("SWITCHYARD_CGROUP_ROOT")?;
    let node = env_path("SWITCHYARD_PI_NODE")?;
    let launcher = env_path("SWITCHYARD_BRIDGE_LAUNCHER")?;
    let policy = env_path("SWITCHYARD_BRIDGE_POLICY")?;
    let helper = env_path("SWITCHYARD_EXECUTION_HELPER")?;
    let root = bridge_root(db)?;
    // The composed scenario's trusted fixture. No model endpoint, credential,
    // or TLS route is configured on this path.
    Host::open_confined_pi(
        db,
        ConfinedPiConfig {
            root,
            cgroup_root: cgroup_root.clone(),
            runtime: pi_runtime,
            node,
            bubblewrap: bubblewrap.clone(),
            launcher,
            policy,
            model_endpoint: None,
        },
        LinuxExecutionConfig {
            cgroup_root,
            bubblewrap,
            helper,
            crash_at: None,
            probe_request: None,
            fail_launch: false,
        },
        "s3-acceptance",
    )
}

#[cfg(not(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
)))]
fn open_confined(db: &Path) -> switchyard_core::Result<Host> {
    let _ = db;
    Err(Error::guard(
        "confined Pi runtime requires Linux with the pi-contract and linux-execution build; refusing to fall back to FakeRuntime",
    ))
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn env_path(key: &str) -> switchyard_core::Result<PathBuf> {
    let value = std::env::var(key).map_err(|_| {
        Error::guard(format!(
            "confined Pi runtime requires {key}; refusing to fall back to FakeRuntime"
        ))
    })?;
    let path = PathBuf::from(&value);
    if !path.is_absolute() || !path.exists() {
        return Err(Error::guard(format!(
            "confined Pi runtime requires {key} to be an existing absolute path; refusing to fall back to FakeRuntime"
        )));
    }
    Ok(path)
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn bridge_root(db: &Path) -> switchyard_core::Result<PathBuf> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
    let canonical_db = if db.exists() {
        std::fs::canonicalize(db).map_err(Error::storage)?
    } else {
        let parent = db
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        if !parent.exists() {
            return Err(Error::guard(
                "database directory must already exist; the confined runtime does not create parent directories",
            ));
        }
        let name = db
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| Error::guard("database path has no UTF-8 file name"))?;
        std::fs::canonicalize(parent)
            .map_err(Error::storage)?
            .join(name)
    };
    let parent = canonical_db
        .parent()
        .ok_or_else(|| Error::guard("database path has no parent"))?;
    let name = canonical_db
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| Error::guard("database path has no UTF-8 file name"))?;
    let root = parent.join(format!("{name}.pi-bridge"));
    let owner = std::fs::metadata("/proc/self")
        .map_err(Error::storage)?
        .uid();
    match std::fs::symlink_metadata(&root) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut builder = std::fs::DirBuilder::new();
            builder.mode(0o700);
            builder.create(&root).map_err(Error::storage)?;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(Error::storage)?;
        }
        Err(error) => return Err(Error::storage(error)),
        Ok(meta) => {
            if meta.file_type().is_symlink() || !meta.is_dir() {
                return Err(Error::guard(
                    "confined Pi bridge root must be a real directory",
                ));
            }
            if meta.uid() != owner || meta.mode() & 0o7777 != 0o700 {
                return Err(Error::guard(
                    "confined Pi bridge root has an unexpected owner or mode; refusing to change it",
                ));
            }
        }
    }
    std::fs::canonicalize(&root).map_err(Error::storage)
}

/// Wait for the composed scenario after `approve_plan` has already been
/// admitted. A missing effect or a poll failure stays `"pending"`; this never
/// turns a committed command into `guard_failed`.
fn settle_approve_plan(host: &mut Host, mission: &str) -> &'static str {
    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        loop {
            if host.poll_runtime(mission).is_err() {
                return "pending";
            }
            let Ok(snapshot) = host.snapshot(mission) else {
                return "pending";
            };
            let value = snapshot.to_json();
            if value["piHost"]["observation"]["runtimeIdle"] == true
                && composed_scenario_settled(&value)
            {
                return "complete";
            }
            if std::time::Instant::now() >= deadline {
                return "pending";
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    #[cfg(not(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    )))]
    {
        let _ = (host, mission);
        "pending"
    }
}

fn with_scenario(mut value: Value, scenario: Option<&str>) -> Value {
    if let Some(scenario) = scenario {
        value["composedScenario"] = json!(scenario);
    }
    value
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn composed_scenario_settled(value: &Value) -> bool {
    let idle = value["piHost"]["observation"]["runtimeIdle"] == true;
    let effects = value["linuxExecutionEffects"].as_array();
    let has =
        |method: &str| effects.is_some_and(|rows| rows.iter().any(|row| row["method"] == method));
    idle && has("write_file") && has("exec_start") && has("exec_status")
}

fn command_body(kind: &str, payload: &Value) -> switchyard_core::Result<CommandBody> {
    let kind: CommandKind = kind.parse()?;
    let text = |key: &str| -> switchyard_core::Result<String> {
        payload
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| Error::guard(format!("{kind} requires {key}")))
    };
    let flag = |key: &str| payload.get(key).and_then(Value::as_bool).unwrap_or(false);
    Ok(match kind {
        CommandKind::StartPlanning => CommandBody::StartPlanning,
        CommandKind::PlanCompleted => CommandBody::PlanCompleted {
            plan: text("plan")?,
            digest: text("digest")?,
            quiescent: flag("quiescent"),
        },
        CommandKind::RequestRevisedPlan => CommandBody::RequestRevisedPlan {
            feedback: text("feedback")?,
        },
        CommandKind::ApprovePlan => CommandBody::ApprovePlan {
            plan_digest: text("planDigest")?,
        },
        CommandKind::RequestVerification => CommandBody::RequestVerification {
            quiescent: flag("quiescent"),
        },
        CommandKind::CompletePass => CommandBody::CompletePass,
        CommandKind::AuthorizeBoundedRepair => CommandBody::AuthorizeBoundedRepair {
            rationale: text("rationale")?,
        },
        CommandKind::RequestChanges => CommandBody::RequestChanges {
            feedback: text("feedback")?,
            within_frozen: flag("withinFrozen"),
        },
        CommandKind::ApproveReview => CommandBody::ApproveReview {
            digest: text("digest")?,
            rationale: text("rationale")?,
        },
        CommandKind::Publish => CommandBody::Publish {
            target_branch: text("targetBranch")?,
            base_branch: text("baseBranch")?,
            authorized: flag("authorized"),
        },
        CommandKind::ReconcileDeliveryComplete => CommandBody::ReconcileDeliveryComplete {
            content_digest: text("contentDigest")?,
            commit_sha: text("commitSha")?,
            tree_sha: text("treeSha")?,
            pr_id: text("prId")?,
        },
        CommandKind::Reject => CommandBody::Reject {
            rationale: text("rationale")?,
        },
        CommandKind::Pause => CommandBody::Pause,
        CommandKind::ConfirmStopped => CommandBody::ConfirmStopped,
        CommandKind::Resume => CommandBody::Resume {
            authorized: flag("authorized"),
        },
        CommandKind::Cancel => CommandBody::Cancel,
        CommandKind::ConfirmCancelled => CommandBody::ConfirmCancelled,
        CommandKind::DiscoverPublicationCompleted => CommandBody::DiscoverPublicationCompleted {
            content_digest: text("contentDigest")?,
            commit_sha: text("commitSha")?,
            tree_sha: text("treeSha")?,
            pr_id: text("prId")?,
        },
        CommandKind::DetectUncertainty => CommandBody::DetectUncertainty {
            reason: text("reason")?.parse()?,
            content_digest: payload
                .get("contentDigest")
                .and_then(Value::as_str)
                .map(str::to_string),
        },
        CommandKind::ExplicitRecovery => CommandBody::ExplicitRecovery {
            expected_target: text("expectedTarget")?.parse::<Phase>()?,
            ownership_proven: flag("ownershipProven"),
            effects: text("effects")?.parse::<EffectProof>()?,
        },
        CommandKind::Steer => CommandBody::Steer {
            content: text("content")?,
        },
        CommandKind::Continue => CommandBody::Continue {
            activity: text("activity")?,
        },
    })
}
