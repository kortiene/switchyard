use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use switchyard_core::{
    AdmitRequest, CommandBody, CommandKind, CommandOrigin, CreateMission, EffectProof, Error, Host,
    Phase,
};

#[derive(Parser)]
#[command(
    name = "switchyard",
    version,
    about = "Switchyard Rust mission host. Fake runtime and owned worktrees; no model credentials or GitHub writes."
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Mission ledger commands.
    Mission {
        #[command(subcommand)]
        action: MissionCommand,
    },
}

#[derive(Subcommand)]
enum MissionCommand {
    /// Audit an uncertain effect without clearing its block or replaying it.
    /// The mission stays blocked in needs_attention. You can request cancel,
    /// but unresolved effects still prevent cancellation from completing.
    /// Lifting the block requires a future evidence protocol (#125).
    /// Reuse command ID and all arguments after a lost response.
    AbandonExecution {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long)]
        attempt: String,
        #[arg(long)]
        session: String,
        #[arg(long)]
        operation_id: String,
        #[arg(long)]
        payload_hash: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        command_id: String,
        #[arg(long)]
        actor: String,
        #[arg(long)]
        rationale: String,
    },
    /// Create or reopen this attempt's owned worktree. Does not execute agent commands.
    Workspace {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long)]
        attempt: String,
        /// Existing canonical absolute directory, outside the source checkout.
        #[arg(long)]
        root: PathBuf,
    },
    /// Create a draft mission and one attempt from a local Git repository.
    Create {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        repo: PathBuf,
        #[arg(long)]
        issue: u64,
        #[arg(long, default_value = "")]
        issue_title: String,
        #[arg(long)]
        objective: String,
        #[arg(long)]
        acceptance: String,
        #[arg(long, default_value = "operator")]
        actor: String,
        #[arg(long, default_value_t = 2)]
        max_repairs: u32,
        #[arg(long, default_value_t = 3)]
        max_plan_revisions: u32,
        /// Also admit this command before exiting.
        #[arg(long)]
        admit: Option<String>,
        /// Persist an admitted command without dispatching it to the runtime.
        #[arg(long)]
        persist_only: bool,
        #[arg(long, default_value = "{}")]
        payload: String,
    },
    /// Read mission state without taking the owner lock.
    Status {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: Option<String>,
    },
    /// Admit a command. Dispatches pending runtime work unless `--persist-only`.
    /// `--persist-only` does not reconcile a workspace or run `git worktree add`.
    /// If the command is recorded but the workspace blocks dispatch, the process
    /// exits 3 and the JSON includes `workspaceError`. Exit 2 means rejection or
    /// failure, possibly after recording. Supply `--command-id` before the first
    /// attempt and reuse it with the same request to resolve an ambiguous result.
    Command {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long)]
        kind: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long)]
        attempt: Option<String>,
        #[arg(long)]
        command_id: Option<String>,
        #[arg(long, default_value = "operator")]
        actor: String,
        #[arg(long, default_value = "{}")]
        payload: String,
        /// Record the command only. Does not reconcile a workspace, dispatch,
        /// or run `git worktree add`.
        #[arg(long)]
        persist_only: bool,
    },
    /// Acquire the owner lock, redeliver pending commands, and apply queued observations.
    Recover {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
    },
    /// Apply one fake-runtime observation. The JSON `status` field is ignored.
    Notify {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long)]
        json_body: String,
    },
    /// Hold the owner lock until stdin closes.
    Lock {
        #[arg(long)]
        db: PathBuf,
    },
    /// Create a new mission after a terminal one. The source mission stays terminal.
    Successor {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long, default_value = "operator")]
        actor: String,
    },
    /// Record a content digest. Review-gated phases enter needs_attention.
    Content {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long)]
        digest: String,
        #[arg(long)]
        expected_revision: u64,
        #[arg(long, default_value = "operator")]
        actor: String,
    },
}

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
                let mut host = Host::open(&db)?;
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
                    "nextSteps":"The execution block remains. On abandonment the mission stays in needs_attention. You can request cancel, but unresolved effects prevent cancellation from completing. Lifting the block requires a future evidence protocol (#125).",
                    "mission":host.snapshot(&mission)?.to_json()}),
                )
            }
            MissionCommand::Workspace {
                db,
                mission,
                attempt,
                root,
            } => {
                let mut host = Host::open(&db)?;
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
                let mut host = Host::open(&db)?;
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
                Ok(json!({"ok": true, "mission": host.snapshot(&snapshot.mission_id)?.to_json()}))
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
                let mut host = Host::open(&db)?;
                let body = command_body(
                    &kind,
                    &serde_json::from_str(&payload)
                        .map_err(|error| Error::guard(format!("payload is not JSON: {error}")))?,
                )?;
                let is_stop = matches!(body, CommandBody::Pause | CommandBody::Cancel);
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
                        host.dispatch_stops(&mission)?;
                    } else {
                        host.dispatch_reconciled(&mission)?;
                    }
                }
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
                    return Ok(json!({
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
                    }));
                }
                Ok(json!({
                    "ok": true,
                    "dispatched": dispatched,
                    "idempotent": admitted.idempotent,
                    "commandId": admitted.command_id,
                    "requestId": admitted.request_id,
                    "payloadHash": admitted.payload_hash,
                    "commandState": command_state,
                    "mission": host.snapshot(&mission)?.to_json(),
                }))
            }
            MissionCommand::Recover { db, mission } => {
                let mut host = Host::open(&db)?;
                let snapshot = host.recover(&mission)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
            MissionCommand::Notify {
                db,
                mission,
                json_body,
            } => {
                let mut host = Host::open(&db)?;
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
                let host = Host::open(&db)?;
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
                let mut host = Host::open(&db)?;
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
                let mut host = Host::open(&db)?;
                let snapshot =
                    host.note_content_digest(&mission, expected_revision, &digest, &actor)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
        },
    }
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
