mod cli;

use std::path::Path;
#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
use std::path::PathBuf;
use std::process::ExitCode;
#[cfg(any(
    test,
    all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    )
))]
use std::sync::atomic::{AtomicBool, Ordering};

use clap::Parser;
use cli::{Cli, Command, MissionCommand, RuntimeSelection};
use serde_json::{Value, json};
use switchyard_core::{
    AdmitRequest, CommandBody, CommandKind, CommandOrigin, CreateMission, EffectProof, Error, Host,
    Phase, PlanSource, SuccessorChanges, write_export_exclusive,
};

/// Set by SIGINT and SIGTERM in this process. The waiting command admits
/// `cancel` as the operator and uses the existing confined stop proof.
#[cfg(any(
    test,
    all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    )
))]
static OPERATOR_STOP: AtomicBool = AtomicBool::new(false);

#[cfg(any(
    test,
    all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    )
))]
extern "C" fn on_operator_stop(_: libc::c_int) {
    OPERATOR_STOP.store(true, Ordering::SeqCst);
    // The next Ctrl-C uses the default disposition and ends the process.
    unsafe {
        libc::signal(libc::SIGINT, libc::SIG_DFL);
        libc::signal(libc::SIGTERM, libc::SIG_DFL);
    }
}

#[cfg(any(
    test,
    all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    )
))]
fn arm_operator_stop() {
    unsafe {
        let mut action: libc::sigaction = std::mem::zeroed();
        action.sa_sigaction = on_operator_stop as *const () as usize;
        action.sa_flags = 0;
        libc::sigemptyset(&mut action.sa_mask);
        libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
        libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
    }
}

#[cfg(any(
    test,
    all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    )
))]
fn operator_stop_requested() -> bool {
    OPERATOR_STOP.load(Ordering::SeqCst)
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
    let runtime = cli.runtime.unwrap_or(RuntimeSelection::Fake);
    // Flags only. Opening the host first would take the owner lock and
    // create the database. Same shape as the empty-acceptance refusal.
    let model_endpoint = model_endpoint_from_flag(runtime, cli.model_endpoint.as_deref())?;
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
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
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
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
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
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let workspace = host.prepare_workspace(&mission, &attempt, &root)?;
                Ok(json!({"ok": true, "workspace": workspace.to_json()}))
            }
            MissionCommand::Write {
                db,
                mission,
                attempt,
                path,
                from,
                actor,
            } => {
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let write = host.operator_write(&mission, &attempt, &path, &from, &actor)?;
                Ok(json!({
                    "ok": true,
                    "write": {
                        "path": write.path,
                        "bytes": write.bytes,
                        "sha256": write.sha256,
                        "authoredBy": "operator",
                        "actor": write.actor,
                        "workspacePath": write.workspace_path,
                        "baseSha": write.base_sha,
                    },
                    "mission": host.snapshot(&mission)?.to_json(),
                }))
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
                verifier,
                plan_source,
            } => {
                // This refusal uses only the flag. Opening the host first would
                // take the owner lock and bump host_owner.generation.
                if acceptance.trim().is_empty() {
                    return Err(Error::guard("acceptance criteria is required"));
                }
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let snapshot = host.create_mission_with_plan_source(
                    CreateMission {
                        repo,
                        issue_number: issue,
                        issue_title,
                        objective,
                        acceptance,
                        actor: actor.clone(),
                        max_repairs,
                        max_plan_revisions,
                    },
                    verifier,
                    PlanSource::parse(&plan_source)?,
                )?;
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
                        json!(settle_confined(&mut host, &snapshot.mission_id, true));
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
            MissionCommand::Export { db, mission, out } => {
                let document = Host::export_document(&db, &mission, env!("CARGO_PKG_VERSION"))?;
                let Some(path) = out else {
                    return Ok(document);
                };
                let text = format!("{document}\n");
                let written = write_export_exclusive(&path, text.as_bytes())?;
                Ok(json!({
                    "ok": true,
                    "out": path.display().to_string(),
                    "bytes": written.bytes,
                    "sha256": written.sha256,
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
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let body = command_body(
                    &kind,
                    &serde_json::from_str(&payload)
                        .map_err(|error| Error::guard(format!("payload is not JSON: {error}")))?,
                )?;
                let is_stop = matches!(body, CommandBody::Pause | CommandBody::Cancel);
                let approve_plan = matches!(body, CommandBody::ApprovePlan { .. });
                let repair = matches!(
                    body,
                    CommandBody::AuthorizeBoundedRepair { .. } | CommandBody::RequestChanges { .. }
                );
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
                    && (approve_plan || repair)
                    && !persist_only
                    && workspace_error.is_none()
                {
                    Some(settle_confined(&mut host, &mission, approve_plan))
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
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let snapshot = host.recover(&mission)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
            MissionCommand::Notify {
                db,
                mission,
                json_body,
            } => {
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
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
            MissionCommand::Plan {
                db,
                mission,
                file,
                quiescent,
            } => {
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let outcome = host.apply_plan_file(&mission, &file, quiescent)?;
                Ok(json!({
                    "ok": true,
                    "phase": outcome.phase().as_str(),
                    "revision": outcome.revision(),
                    "mission": host.snapshot(&mission)?.to_json(),
                }))
            }
            MissionCommand::Lock { db } => {
                let host = open_host(&db, runtime, model_endpoint.as_ref())?;
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
            MissionCommand::Successor {
                db,
                mission,
                actor,
                objective,
                acceptance,
                max_repairs,
                max_plan_revisions,
                verifier,
                plan_source,
            } => {
                // An acceptance override is CLI input. Refuse it before the
                // host open bumps host_owner.generation. Omitting the flag
                // still copies the source, inside the host.
                let acceptance = acceptance_override(&acceptance)?;
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let snapshot = host.create_successor_with(
                    &mission,
                    &actor,
                    &SuccessorChanges {
                        objective,
                        acceptance,
                        max_repairs,
                        max_plan_revisions,
                        verifier_json: verifier,
                        plan_source: match plan_source {
                            Some(value) => Some(PlanSource::parse(&value)?),
                            None => None,
                        },
                    },
                )?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
            MissionCommand::Verify { db, mission } => {
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let (snapshot, outcome) = host.verify_mission(&mission)?;
                Ok(json!({
                    "ok": true,
                    "outcome": outcome,
                    "mission": snapshot.to_json(),
                }))
            }
            MissionCommand::Content {
                db,
                mission,
                digest,
                expected_revision,
                actor,
            } => {
                let mut host = open_host(&db, runtime, model_endpoint.as_ref())?;
                let snapshot =
                    host.note_content_digest(&mission, expected_revision, &digest, &actor)?;
                Ok(json!({"ok": true, "mission": snapshot.to_json()}))
            }
        },
    }
}

/// Parsed endpoint once `pi-contract` and `linux-execution` are both enabled.
/// Other builds keep the flag checks and refuse the runtime before launch.
#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
type CliModelEndpoint = switchyard_core::ModelEndpoint;
#[cfg(not(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
)))]
type CliModelEndpoint = ();

fn open_host(
    db: &Path,
    runtime: RuntimeSelection,
    model_endpoint: Option<&CliModelEndpoint>,
) -> switchyard_core::Result<Host> {
    match runtime {
        RuntimeSelection::Fake => Host::open(db).map_err(suggest_confined_runtime),
        RuntimeSelection::ConfinedPi => open_confined(db, model_endpoint.cloned()),
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
fn open_confined(
    db: &Path,
    model_endpoint: Option<switchyard_core::ModelEndpoint>,
) -> switchyard_core::Result<Host> {
    use switchyard_core::{ConfinedPiConfig, LinuxExecutionConfig};

    // The endpoint was parsed from `--model-endpoint` before this open.
    // Unset keeps default-deny egress. This fixture is the faux provider;
    // it records the pin and does not send model traffic.
    let pi_runtime = env_path("SWITCHYARD_PI_RUNTIME")?;
    let bubblewrap = env_path("SWITCHYARD_BWRAP")?;
    let cgroup_root = env_path("SWITCHYARD_CGROUP_ROOT")?;
    let node = env_path("SWITCHYARD_PI_NODE")?;
    let launcher = env_path("SWITCHYARD_BRIDGE_LAUNCHER")?;
    let policy = env_path("SWITCHYARD_BRIDGE_POLICY")?;
    let helper = env_path("SWITCHYARD_EXECUTION_HELPER")?;
    let root = bridge_root(db)?;
    // The composed scenario's trusted fixture (`s3-acceptance`, the faux
    // provider). Nothing on this path is model egress. Egress stays denied
    // when the flag is omitted. No credential or TLS route is configured.
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
            model_endpoint,
        },
        LinuxExecutionConfig {
            cgroup_root,
            bubblewrap,
            helper,
            crash_at: None,
            probe_request: None,
            fail_launch: false,
        },
        confined_fixture().as_str(),
    )
}

/// Default is the composed `s3-acceptance` replay. Tests may set
/// `SWITCHYARD_CONFINED_FIXTURE` to another allowlisted fixture so the
/// foreground wait stays open. Unknown values stay on `s3-acceptance`.
#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn confined_fixture() -> String {
    match std::env::var("SWITCHYARD_CONFINED_FIXTURE") {
        Ok(value)
            if matches!(
                value.as_str(),
                "answer"
                    | "file-write"
                    | "process-fast"
                    | "process-hold"
                    | "process-cancel"
                    | "tools"
                    | "s3-acceptance"
                    | "ignore-abort"
            ) =>
        {
            value
        }
        _ => "s3-acceptance".into(),
    }
}

#[cfg(not(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
)))]
fn open_confined(db: &Path, model_endpoint: Option<()>) -> switchyard_core::Result<Host> {
    let _ = (db, model_endpoint);
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

/// `--model-endpoint` is a `String`, so clap rejects a non-Unicode argument
/// before `run`. There is no `OsString` branch and this function never sees
/// those bytes.
fn model_endpoint_from_flag(
    runtime: RuntimeSelection,
    value: Option<&str>,
) -> switchyard_core::Result<Option<CliModelEndpoint>> {
    let Some(value) = value else {
        return Ok(None);
    };
    if runtime != RuntimeSelection::ConfinedPi {
        return Err(Error::guard(
            "--model-endpoint is valid only with --runtime confined-pi",
        ));
    }
    if !endpoint_host_is_loopback_literal(value) {
        return Err(Error::guard(
            "--model-endpoint accepts only a loopback IP literal in 127.0.0.0/8 or ::1, with or without a port; hostnames and other addresses are refused without a DNS lookup",
        ));
    }
    Ok(Some(parse_pinned_loopback(value)?))
}

/// True only for an `http` or `https` URL whose host is an IPv4 address in
/// `127.0.0.0/8` or the IPv6 address `::1`.
///
/// This never resolves a name. [`parse_pinned_loopback`] calls
/// `ModelEndpoint::parse` only after this returns true, and only an IP
/// literal reaches that parser, so `to_socket_addrs` does not query DNS.
fn endpoint_host_is_loopback_literal(input: &str) -> bool {
    let Some((scheme, rest)) = input.split_once("://") else {
        return false;
    };
    if scheme != "http" && scheme != "https" {
        return false;
    }
    // Userinfo, query, fragment, and escapes are not a loopback literal.
    // Rejecting them here avoids handing a non-literal to the resolver.
    if rest.contains(['@', '\\', '#', '?', '%'])
        || rest.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return false;
    }
    let authority = rest
        .split_once('/')
        .map(|(authority, _)| authority)
        .unwrap_or(rest);
    let host = if let Some(body) = authority.strip_prefix('[') {
        let Some((host, after)) = body.split_once(']') else {
            return false;
        };
        if !(after.is_empty() || after.starts_with(':')) {
            return false;
        }
        host
    } else if authority.matches(':').count() > 1 {
        return false;
    } else {
        match authority.rsplit_once(':') {
            Some((host, port))
                if !port.is_empty() && port.bytes().all(|byte| byte.is_ascii_digit()) =>
            {
                host
            }
            Some(_) => return false,
            None => authority,
        }
    };
    if host.is_empty() {
        return false;
    }
    if let Ok(ip) = host.parse::<std::net::Ipv4Addr>() {
        return ip.is_loopback();
    }
    host.parse::<std::net::Ipv6Addr>()
        .is_ok_and(|ip| ip.is_loopback())
}

/// Pins a loopback literal. Callers have already rejected every hostname.
#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn parse_pinned_loopback(value: &str) -> switchyard_core::Result<switchyard_core::ModelEndpoint> {
    // Return the parser's guard unchanged. Wrapping `error.to_string()`
    // prints "guard failed" twice, because that display already includes it.
    switchyard_core::ModelEndpoint::parse(value)
}

#[cfg(not(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
)))]
fn parse_pinned_loopback(value: &str) -> switchyard_core::Result<()> {
    // The shape is a loopback literal. This build has no `ModelEndpoint`;
    // `open_confined` refuses the runtime before launch.
    let _ = value;
    Ok(())
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

/// Foreground wait after `approve_plan` or a repair has been admitted.
///
/// The old fixed 60s cap is the frozen bridge wall (20 minutes per
/// generation) and the attempt wall. SIGINT/SIGTERM admits `cancel` as the
/// operator and delivers it through [`Host::interrupt_operator`]. A missing
/// effect or a poll failure stays `"pending"`; this never turns a committed
/// command into `guard_failed`.
///
/// TODO(#217): the product fixture uses this same wait. There is no
/// product-mode flag; the frozen budgets are the gate.
#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn has_pending_runtime_work(snapshot: &switchyard_core::MissionSnapshot) -> bool {
    snapshot.commands.iter().any(|command| {
        command.state == "pending" && command.kind != "pause" && command.kind != "cancel"
    })
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn latest_repair_state(snapshot: &switchyard_core::MissionSnapshot) -> Option<&str> {
    snapshot
        .commands
        .iter()
        .rev()
        .find(|command| {
            command.kind == "authorize_bounded_repair" || command.kind == "request_changes"
        })
        .map(|command| command.state.as_str())
}

fn settle_confined(host: &mut Host, mission: &str, require_composed: bool) -> &'static str {
    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    {
        OPERATOR_STOP.store(false, Ordering::SeqCst);
        arm_operator_stop();
        let budgets = host.frozen_budgets(mission).ok().flatten();
        let s3_acceptance = host.s3_acceptance_replay(mission);
        loop {
            if operator_stop_requested() {
                return match host.interrupt_operator(mission) {
                    Ok(snapshot) if snapshot.phase == Phase::Cancelled => "cancelled",
                    _ => "pending",
                };
            }
            if host.poll_runtime(mission).is_err() {
                let exhausted = host.snapshot(mission).is_ok_and(|snapshot| {
                    snapshot.attention_reason.as_deref() == Some("runtime_budget_exhausted")
                });
                return if exhausted {
                    "needs_attention"
                } else {
                    "pending"
                };
            }
            if host.enforce_attempt_wall(mission).is_err() {
                return "needs_attention";
            }
            let Ok(snapshot) = host.snapshot(mission) else {
                return "pending";
            };
            if snapshot.attention_reason.as_deref() == Some("runtime_budget_exhausted") {
                return "needs_attention";
            }
            let value = snapshot.to_json();
            let idle = value["piHost"]["observation"]["runtimeIdle"] == true;
            let pending = has_pending_runtime_work(&snapshot);
            if require_composed {
                if switchyard_core::foreground_run_complete(
                    s3_acceptance,
                    idle,
                    pending,
                    composed_scenario_settled(&value),
                ) {
                    return "complete";
                }
            } else if let Some(outcome) =
                switchyard_core::repair_wait_outcome(idle, latest_repair_state(&snapshot))
            {
                return outcome;
            }
            if let Some(wall) = budgets.as_ref().and_then(|budgets| budgets.bridge_wall)
                && host
                    .generation_over_bridge_wall(mission, wall)
                    .unwrap_or(false)
            {
                let _ = host.exhaust_runtime_budget(mission);
                return "needs_attention";
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
        let _ = (host, mission, require_composed);
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

fn acceptance_override(values: &[String]) -> switchyard_core::Result<Option<String>> {
    if values.is_empty() {
        return Ok(None);
    }
    let mut parts = Vec::with_capacity(values.len());
    for value in values {
        if let Some(path) = value.strip_prefix('@') {
            if path.is_empty() {
                return Err(Error::guard("acceptance file path is empty"));
            }
            let text = std::fs::read_to_string(path).map_err(|error| {
                Error::guard(format!("acceptance file {path} could not be read: {error}"))
            })?;
            let text = text
                .strip_suffix("\r\n")
                .or_else(|| text.strip_suffix('\n'))
                .unwrap_or(&text);
            push_acceptance(&mut parts, text)?;
        } else {
            push_acceptance(&mut parts, value)?;
        }
    }
    Ok(Some(parts.join("\n")))
}

fn push_acceptance(parts: &mut Vec<String>, text: &str) -> switchyard_core::Result<()> {
    if text.trim().is_empty() {
        return Err(Error::guard("acceptance criteria is required"));
    }
    parts.push(text.to_string());
    Ok(())
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

#[cfg(test)]
mod operator_stop_tests {
    use super::{OPERATOR_STOP, arm_operator_stop, operator_stop_requested};
    use clap::Parser;
    use std::sync::atomic::Ordering;

    #[test]
    fn create_defaults_are_the_section_3_4_limits() {
        let cli = crate::cli::Cli::try_parse_from([
            "switchyard",
            "mission",
            "create",
            "--db",
            "mission.sqlite",
            "--repo",
            "repo",
            "--issue",
            "218",
            "--objective",
            "defaults",
            "--acceptance",
            "caps",
        ])
        .unwrap();
        let crate::cli::Command::Mission { action } = cli.command;
        let crate::cli::MissionCommand::Create {
            max_repairs,
            max_plan_revisions,
            ..
        } = action
        else {
            panic!("expected create");
        };
        assert_eq!(max_repairs, 1);
        assert_eq!(max_plan_revisions, 2);
        assert!(cli.runtime.is_none(), "FakeRuntime stays the default");
    }

    #[test]
    fn sigint_sets_the_operator_stop_flag() {
        arm_operator_stop();
        OPERATOR_STOP.store(false, Ordering::SeqCst);
        unsafe {
            libc::raise(libc::SIGINT);
        }
        assert!(operator_stop_requested());
        let mut current: libc::sigaction = unsafe { std::mem::zeroed() };
        unsafe {
            libc::sigaction(libc::SIGINT, std::ptr::null(), &mut current);
        }
        assert_eq!(
            current.sa_sigaction,
            libc::SIG_DFL,
            "the first Ctrl-C restores the default disposition"
        );
        OPERATOR_STOP.store(false, Ordering::SeqCst);
        unsafe {
            let mut action: libc::sigaction = std::mem::zeroed();
            action.sa_sigaction = libc::SIG_DFL;
            libc::sigemptyset(&mut action.sa_mask);
            libc::sigaction(libc::SIGINT, &action, std::ptr::null_mut());
            libc::sigaction(libc::SIGTERM, &action, std::ptr::null_mut());
        }
    }
}
