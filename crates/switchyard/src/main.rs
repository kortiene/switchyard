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
    about = "Switchyard Rust mission host (S1). Fake runtime only; no model credentials or GitHub writes."
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
            ExitCode::SUCCESS
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
                host.dispatch_pending(&mission)?;
                let current = host.snapshot(&mission)?;
                let body = command_body(
                    &kind,
                    &serde_json::from_str(&payload)
                        .map_err(|error| Error::guard(format!("payload is not JSON: {error}")))?,
                )?;
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
                    host.dispatch_pending(&mission)?;
                }
                Ok(json!({
                    "ok": true,
                    "idempotent": admitted.idempotent,
                    "commandId": admitted.command_id,
                    "requestId": admitted.request_id,
                    "payloadHash": admitted.payload_hash,
                    "commandState": admitted.state.as_str(),
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
