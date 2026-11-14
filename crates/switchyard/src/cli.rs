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
