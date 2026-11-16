// Clap definition for the `switchyard` binary.
//
// `crates/switchyard-ui` `include!`s this file in a test so echoed commands
// are parsed with this definition rather than a copy of the flags. Keep the
// file free of `crate::` and `super::` imports; those paths would point at
// the UI crate when the file is included there. Inner doc comments (`//!`)
// are also unusable here: `include!` does not treat them as the module docs.
// `std` and `clap` only.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "switchyard",
    version,
    about = "Switchyard Rust mission host. Fake runtime and owned worktrees; no model credentials or GitHub writes."
)]
pub(crate) struct Cli {
    /// Runtime adapter. Omitted means `fake` (FakeRuntime). `confined-pi` is an
    /// explicit opt-in for the confined Pi runtime and fails closed when its
    /// prerequisites are missing. It is never selected from the environment.
    #[arg(long, value_enum, global = true)]
    pub(crate) runtime: Option<RuntimeSelection>,
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, clap::ValueEnum)]
pub(crate) enum RuntimeSelection {
    /// FakeRuntime. Product default when `--runtime` is omitted.
    Fake,
    /// Credential-free confined Pi runtime. Runs the composed `s3-acceptance`
    /// scenario and does not select a model endpoint.
    #[value(name = "confined-pi")]
    ConfinedPi,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// Mission ledger commands.
    Mission {
        #[command(subcommand)]
        action: MissionCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum MissionCommand {
    /// Audit an uncertain effect without clearing its block or replaying it.
    /// The mission stays blocked in needs_attention. You can request cancel,
    /// but unresolved effects still prevent cancellation from completing.
    /// Lifting that effect's block requires `supersede-execution` (#125).
    /// The outcome stays unknown. Reuse command ID and all arguments after a
    /// lost response.
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
    /// Supersede one abandoned simulated effect. The outcome stays unknown.
    /// This command does not supersede a Linux row; a default build refuses
    /// those with guidance because it cannot prove cgroup quiescence.
    /// Reuse the command ID and every argument after a lost response.
    SupersedeExecution {
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
        abandonment_command_id: String,
        #[arg(long)]
        abandonment_request_hash: String,
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
    /// Copy one local file into this attempt's owned worktree.
    /// Admitted only in phase `implementing`, and only on the fake runtime.
    /// Does not bump the revision. One path per call.
    Write {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long)]
        attempt: String,
        /// Workspace-relative destination. One file per call.
        #[arg(long)]
        path: String,
        /// Local file to copy. Must be a regular file of at most 64 KiB.
        #[arg(long)]
        from: PathBuf,
        #[arg(long, default_value = "operator")]
        actor: String,
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
        /// Verifier document JSON. Omitted stores `switchyard.verifier.v1`.
        #[arg(long)]
        verifier: Option<String>,
    },
    /// Read mission state without taking the owner lock.
    Status {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: Option<String>,
    },
    /// Read-only ledger export. Does not take the owner lock, migrate, or dispatch.
    ///
    /// Without `--out`, the document is written to stdout. With `--out`, the
    /// ledger read finishes first, the file is created with
    /// `O_CREAT|O_EXCL|O_NOFOLLOW`, and stdout prints a receipt. An existing
    /// path, including a symlink or a directory, is `export_target_exists`:
    /// pick a new path, or remove it.
    Export {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        /// New file for the document. Created only after the ledger read succeeds.
        #[arg(long)]
        out: Option<PathBuf>,
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
    /// Admit a `plan_completed` observation from a file.
    ///
    /// The file bytes are the plan text verbatim: nothing is trimmed and
    /// newlines are not normalized. This differs from successor
    /// `--acceptance @path`, which strips one trailing newline.
    Plan {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        /// Regular file of at most 64 KiB. A symlink leaf is refused; a
        /// symlinked parent directory is followed. Bytes are kept verbatim,
        /// including a trailing newline.
        #[arg(long)]
        file: PathBuf,
        /// Required. `true` or `false`. There is no default.
        #[arg(
            long,
            action = clap::ArgAction::Set,
            value_name = "true|false",
            value_parser = parse_quiescent
        )]
        quiescent: bool,
    },
    /// Hold the owner lock until stdin closes.
    Lock {
        #[arg(long)]
        db: PathBuf,
    },
    /// Create a new mission after a terminal one. The source mission stays terminal.
    ///
    /// Omitted overrides are copied from the source. Base SHA and runtime
    /// configuration are always re-read from the host. A source refused with
    /// `frozen_input_drift`, or a plan-digest mismatch, must `cancel`, then
    /// `confirm_cancelled`, first. A source refused with `frozen_inputs_missing`
    /// must `reject` or `cancel`, then successor.
    Successor {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
        #[arg(long, default_value = "operator")]
        actor: String,
        /// Replace the source objective.
        #[arg(long)]
        objective: Option<String>,
        /// Replace acceptance criteria. Repeat the flag, or pass `@path` to
        /// read one value from a file. A value that starts with `@` is read
        /// as a file path. Empty values are refused. Values are joined with
        /// newlines.
        #[arg(long)]
        acceptance: Vec<String>,
        /// Replace the source repair limit.
        #[arg(long)]
        max_repairs: Option<u32>,
        /// Replace the source plan-revision limit.
        #[arg(long)]
        max_plan_revisions: Option<u32>,
        /// Replace the source verifier. A supported document:
        /// `switchyard.verifier.v1` or `switchyard.verifier.rust-verify.v1`.
        #[arg(long)]
        verifier: Option<String>,
    },
    /// Run the frozen changed-path gate. Does not execute the verifier.
    Verify {
        #[arg(long)]
        db: PathBuf,
        #[arg(long)]
        mission: String,
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

fn parse_quiescent(value: &str) -> Result<bool, QuiescentError> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(QuiescentError),
    }
}

#[derive(Debug)]
struct QuiescentError;

impl std::fmt::Display for QuiescentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("expected true or false")
    }
}

impl std::error::Error for QuiescentError {}
