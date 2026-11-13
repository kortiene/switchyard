//! Read-only view model over the Rust mission ledger.
//!
//! The only entry point into `switchyard-core` here is [`Host::inspect`],
//! documented as "Read-only inspection. Does not take the owner lock, migrate,
//! or dispatch." It opens the database `SQLITE_OPEN_READ_ONLY` with
//! `query_only=ON`. The ledger is WAL with `busy_timeout(5s)`, so a reader
//! coexists with the single writer that holds the flock.
//!
//! Snapshots are flattened into [`MissionCard`] so the view depends on this
//! crate's shape, not on core's full record. A read failure becomes data
//! ([`Ledger::error`]), never a panic: an unavailable ledger is something the
//! dashboard reports, not something it dies on.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use switchyard_core::Phase as MissionPhase;
use switchyard_core::{Host, MissionSnapshot};

/// The forward path the pipeline meter draws.
///
/// The other six phases (`rejected`, `cancelled`, `pausing`, `paused`,
/// `cancelling`, `needs_attention`) are not progress along this path, so they
/// are reported as a status instead of being forced onto it.
pub const HAPPY_PATH: [MissionPhase; 9] = [
    MissionPhase::Draft,
    MissionPhase::Planning,
    MissionPhase::AwaitingPlanApproval,
    MissionPhase::Implementing,
    MissionPhase::Verifying,
    MissionPhase::AwaitingReview,
    MissionPhase::Approved,
    MissionPhase::Publishing,
    MissionPhase::Published,
];

/// The exact CLI invocation behind a read or a pending mutation, shown so it
/// can be copied and run by a person somewhere else.
///
/// This is the ADW Cockpit's "insert, not execute" move
/// (`adw_sdlc/docs/PI-UI-EXTENSION-BACKLOG.md`): `/adw-runs` inserts a resume
/// command into the editor rather than running it. The dashboard stays a
/// reader; a mutation stays an explicit, confirmed act outside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CliEcho {
    /// `status`, `workspace`, or a pending `CommandKind` string.
    pub label: String,
    /// The full invocation, as one selectable line.
    pub command: String,
    /// Whether running it would change anything outside the terminal.
    pub mutates: bool,
    /// What a person must know before running it.
    pub note: Option<String>,
}

/// One admitted command, as the ledger records it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRow {
    /// The ledger's own `CommandKind` string, e.g. `approve_plan`.
    pub kind: String,
    /// The ledger's own `CommandState` string.
    pub state: String,
    pub origin: String,
}

impl CommandRow {
    fn new(kind: &str, state: &str, origin: &str) -> Self {
        Self {
            kind: kind.to_string(),
            state: state.to_string(),
            origin: origin.to_string(),
        }
    }
}

/// What the dashboard shows for one mission.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissionCard {
    pub mission_id: String,
    pub phase: MissionPhase,
    pub revision: u64,
    pub objective: String,
    pub attempt_id: String,
    pub base_sha: String,
    pub head_sha: String,
    pub runtime_adapter: String,
    pub repair_count: u32,
    pub max_repairs: u32,
    pub plan_revision_count: u32,
    pub max_plan_revisions: u32,
    pub content_digest: String,
    pub plan_digest: Option<String>,
    /// The digest a human approved. The review panel compares this against
    /// `content_digest`, because approving something other than the exact
    /// inspected content is the failure this field exists to prevent.
    pub reviewed_digest: Option<String>,
    pub attention_reason: Option<String>,
    pub execution_blocked: bool,
    pub reconcile_only: bool,
    pub quarantined: usize,
    pub pending_commands: Vec<CommandRow>,
    pub commands: Vec<CommandRow>,
    pub workspace_state: Option<String>,
    pub workspace_path: Option<String>,
    pub delivery_state: Option<String>,
    pub target_branch: Option<String>,
}

impl MissionCard {
    fn from_snapshot(s: &MissionSnapshot) -> Self {
        let commands = |rows: &[switchyard_core::CommandView]| {
            rows.iter()
                .map(|c| CommandRow::new(&c.kind, &c.state, &c.origin))
                .collect()
        };
        Self {
            mission_id: s.mission_id.clone(),
            phase: s.phase,
            revision: s.revision,
            objective: s.objective.clone(),
            attempt_id: s.attempt_id.clone(),
            base_sha: s.base_sha.clone(),
            head_sha: s.head_sha.clone(),
            runtime_adapter: s.runtime_adapter.clone(),
            repair_count: s.repair_count,
            max_repairs: s.max_repairs,
            plan_revision_count: s.plan_revision_count,
            max_plan_revisions: s.max_plan_revisions,
            content_digest: s.content_digest.clone(),
            plan_digest: s.plan_digest.clone(),
            reviewed_digest: s.reviewed_digest.clone(),
            attention_reason: s.attention_reason.clone(),
            execution_blocked: s.execution_blocked,
            reconcile_only: s.reconcile_only,
            quarantined: s.quarantined.len(),
            pending_commands: commands(&s.pending_commands),
            commands: {
                // The ledger returns admission order (`ORDER BY seq, id`); the
                // dashboard shows newest first, so `last` and the panel head
                // mean what they say.
                let mut rows = commands(&s.commands);
                rows.reverse();
                rows
            },
            workspace_state: s.workspace.as_ref().map(|w| w.state.clone()),
            workspace_path: s.workspace.as_ref().map(|w| w.path.clone()),
            delivery_state: s.delivery.as_ref().map(|d| d.publication_state.clone()),
            target_branch: s.delivery.as_ref().map(|d| d.target_branch.clone()),
        }
    }

    /// A card with every field present but unremarkable, for fixtures.
    fn blank() -> Self {
        Self {
            mission_id: String::new(),
            phase: MissionPhase::Draft,
            revision: 0,
            objective: String::new(),
            attempt_id: String::new(),
            base_sha: String::new(),
            head_sha: String::new(),
            runtime_adapter: String::new(),
            repair_count: 0,
            max_repairs: 2,
            plan_revision_count: 0,
            max_plan_revisions: 3,
            content_digest: String::new(),
            plan_digest: None,
            reviewed_digest: None,
            attention_reason: None,
            execution_blocked: false,
            reconcile_only: false,
            quarantined: 0,
            pending_commands: Vec::new(),
            commands: Vec::new(),
            workspace_state: None,
            workspace_path: None,
            delivery_state: None,
            target_branch: None,
        }
    }

    /// `(completed, total)` along [`HAPPY_PATH`], or `None` when the phase is
    /// off the forward path and the meter would be a lie. A published mission
    /// counts as complete, not one short of it.
    pub fn progress(&self) -> Option<(usize, usize)> {
        if self.phase == MissionPhase::Published {
            return Some((HAPPY_PATH.len(), HAPPY_PATH.len()));
        }
        HAPPY_PATH
            .iter()
            .position(|p| *p == self.phase)
            .map(|i| (i, HAPPY_PATH.len()))
    }

    /// Whether anything needs an operator. Uncertainty is surfaced, never
    /// smoothed over: a blocked effect or an open attention reason is exactly
    /// the state the dashboard exists to make visible.
    pub fn needs_attention(&self) -> bool {
        self.execution_blocked
            || self.attention_reason.is_some()
            || self.phase == MissionPhase::NeedsAttention
    }

    /// Whether the approved digest still matches the inspected content.
    /// `None` means nothing has been reviewed yet.
    pub fn review_matches(&self) -> Option<bool> {
        self.reviewed_digest
            .as_ref()
            .map(|approved| approved == &self.content_digest)
    }
}

/// One read of the ledger.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    /// Database path, as displayed.
    pub source: String,
    pub cards: Vec<MissionCard>,
    /// Why the read produced no cards. `None` on success.
    pub error: Option<String>,
    /// `HH:MM:SS` UTC of the read, or `None` before any read completed.
    pub read_at_utc: Option<String>,
}

impl Ledger {
    /// Read every mission without taking the owner lock.
    pub fn read(path: &Path) -> Self {
        let source = path.display().to_string();
        match Host::inspect(path, None) {
            Ok(snapshots) => Self {
                cards: snapshots.iter().map(MissionCard::from_snapshot).collect(),
                error: None,
                source,
                read_at_utc: Some(utc_hms()),
            },
            Err(error) => Self {
                cards: Vec::new(),
                error: Some(error.to_string()),
                source,
                read_at_utc: Some(utc_hms()),
            },
        }
    }

    /// The state before any read has completed.
    pub fn unread(path: &Path) -> Self {
        Self {
            source: path.display().to_string(),
            cards: Vec::new(),
            error: None,
            read_at_utc: None,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    /// The CLI invocations for one mission: one honest read, and one flagged
    /// mutation per pending command. Nothing here is ever executed by this
    /// crate; the strings exist to be copied.
    pub fn cli_echo(&self, card: &MissionCard) -> Vec<CliEcho> {
        let db = &self.source;
        let mission = &card.mission_id;
        let mut out = vec![CliEcho {
            label: "status".to_string(),
            command: format!("switchyard mission status --db {db} --mission {mission}"),
            mutates: false,
            note: Some("read-only; does not take the owner lock".to_string()),
        }];

        if card.workspace_state.is_some() {
            out.push(CliEcho {
                label: "workspace".to_string(),
                command: format!(
                    "switchyard mission workspace --db {db} --mission {mission} --attempt {} --root <dir-outside-the-checkout>",
                    card.attempt_id
                ),
                mutates: true,
                note: Some(
                    "creates or reopens the owned worktree; runs no agent commands".to_string(),
                ),
            });
        }

        if !card.pending_commands.is_empty() {
            // Pending commands are already admitted and waiting for dispatch.
            // Echoing a fresh `mission command --kind <same>` would admit a
            // duplicate; the honest action is `recover`, which redelivers them.
            out.push(CliEcho {
                label: "recover".to_string(),
                command: format!("switchyard mission recover --db {db} --mission {mission}"),
                mutates: true,
                note: Some(
                    "takes the owner lock and redelivers the pending commands. They are \
                     already admitted; re-admitting them would duplicate."
                        .to_string(),
                ),
            });
            return out;
        }

        // The phase-gated human decision, with exactly the flags and payload
        // `switchyard mission command` parses: rationale lives inside the
        // payload JSON, never as a flag.
        let decision: Option<(&str, String)> = match card.phase {
            MissionPhase::AwaitingPlanApproval => card
                .plan_digest
                .as_ref()
                .map(|digest| ("approve_plan", format!("{{\"planDigest\":\"{digest}\"}}"))),
            MissionPhase::AwaitingReview => Some((
                "approve_review",
                format!(
                    "{{\"digest\":\"{}\",\"rationale\":\"<why>\"}}",
                    card.content_digest
                ),
            )),
            _ => None,
        };
        if let Some((kind, payload)) = decision {
            out.push(CliEcho {
                label: kind.to_string(),
                command: format!(
                    "switchyard mission command --db {db} --mission {mission} --kind {kind} \
                     --expected-revision {} --attempt {} --command-id <id-you-generate> \
                     --actor <operator> --payload '{payload}'",
                    card.revision, card.attempt_id
                ),
                mutates: true,
                note: Some(
                    "mutates the ledger. The payload binds the approval to the digest in \
                     panel 4; replace <why> first. Generate and persist --command-id before \
                     the first attempt, and reuse it with the same request after a lost \
                     response."
                        .to_string(),
                ),
            });
        }

        out
    }

    // Fixtures. Headless scenes and tests use these so a snapshot never
    // depends on a database existing.

    pub fn fixture_empty() -> Self {
        Self {
            source: "/tmp/switchyard/empty.db".to_string(),
            cards: Vec::new(),
            error: None,
            read_at_utc: Some("09:41:07".to_string()),
        }
    }

    pub fn fixture_unreadable() -> Self {
        Self {
            source: "/tmp/switchyard/missing.db".to_string(),
            cards: Vec::new(),
            error: Some("not found".to_string()),
            read_at_utc: Some("09:41:07".to_string()),
        }
    }

    pub fn fixture_planning() -> Self {
        Self {
            source: "/tmp/switchyard/mission.db".to_string(),
            cards: vec![MissionCard {
                mission_id: "msn-7f3a1c".to_string(),
                phase: MissionPhase::Planning,
                revision: 4,
                objective: "Record execution receipts for bounded repairs".to_string(),
                attempt_id: "att-01".to_string(),
                base_sha: "65a7ce7f0d2b".to_string(),
                head_sha: "65a7ce7f0d2b".to_string(),
                runtime_adapter: "fake".to_string(),
                content_digest: "b2c9".to_string(),
                plan_digest: Some("91af".to_string()),
                workspace_state: Some("ready".to_string()),
                workspace_path: Some("/tmp/switchyard/wt/msn-7f3a1c".to_string()),
                commands: vec![
                    CommandRow::new("start_planning", "committed", "operator"),
                    CommandRow::new("create", "committed", "operator"),
                ],
                ..MissionCard::blank()
            }],
            error: None,
            read_at_utc: Some("09:41:07".to_string()),
        }
    }

    pub fn fixture_awaiting_review() -> Self {
        Self {
            source: "/tmp/switchyard/mission.db".to_string(),
            cards: vec![MissionCard {
                mission_id: "msn-7f3a1c".to_string(),
                phase: MissionPhase::AwaitingReview,
                revision: 11,
                objective: "Record execution receipts for bounded repairs".to_string(),
                attempt_id: "att-01".to_string(),
                base_sha: "65a7ce7f0d2b".to_string(),
                head_sha: "cfd4340f700f".to_string(),
                runtime_adapter: "fake".to_string(),
                repair_count: 1,
                max_repairs: 2,
                content_digest: "d41e".to_string(),
                plan_digest: Some("91af".to_string()),
                reviewed_digest: Some("d41e".to_string()),
                workspace_state: Some("ready".to_string()),
                pending_commands: vec![CommandRow::new("approve_review", "pending", "operator")],
                commands: vec![
                    CommandRow::new("complete_pass", "committed", "runtime"),
                    CommandRow::new("request_verification", "committed", "operator"),
                ],
                ..MissionCard::blank()
            }],
            error: None,
            read_at_utc: Some("09:44:52".to_string()),
        }
    }

    pub fn fixture_needs_attention() -> Self {
        Self {
            source: "/tmp/switchyard/mission.db".to_string(),
            cards: vec![MissionCard {
                mission_id: "msn-2b8e44".to_string(),
                phase: MissionPhase::NeedsAttention,
                revision: 19,
                objective: "Publish one pull request for the receipt change".to_string(),
                attempt_id: "att-02".to_string(),
                base_sha: "65a7ce7f0d2b".to_string(),
                head_sha: "cfd4340f700f".to_string(),
                runtime_adapter: "fake".to_string(),
                repair_count: 2,
                max_repairs: 2,
                content_digest: "77aa".to_string(),
                reviewed_digest: Some("d41e".to_string()),
                attention_reason: Some(
                    "uncertain external effect: operation op-9c1 unanswered".to_string(),
                ),
                execution_blocked: true,
                reconcile_only: true,
                quarantined: 1,
                workspace_state: Some("uncertain".to_string()),
                commands: vec![CommandRow::new("detect_uncertainty", "committed", "host")],
                ..MissionCard::blank()
            }],
            error: None,
            read_at_utc: Some("09:51:33".to_string()),
        }
    }

    pub fn fixture_published() -> Self {
        Self {
            source: "/tmp/switchyard/mission.db".to_string(),
            cards: vec![MissionCard {
                mission_id: "msn-7f3a1c".to_string(),
                phase: MissionPhase::Published,
                revision: 24,
                objective: "Record execution receipts for bounded repairs".to_string(),
                attempt_id: "att-01".to_string(),
                base_sha: "65a7ce7f0d2b".to_string(),
                head_sha: "cfd4340f700f".to_string(),
                runtime_adapter: "fake".to_string(),
                content_digest: "d41e".to_string(),
                reviewed_digest: Some("d41e".to_string()),
                workspace_state: Some("released".to_string()),
                delivery_state: Some("published".to_string()),
                target_branch: Some("switchyard/receipts".to_string()),
                commands: vec![CommandRow::new("publish", "committed", "operator")],
                ..MissionCard::blank()
            }],
            error: None,
            read_at_utc: Some("10:02:18".to_string()),
        }
    }
}

/// `HH:MM:SS` in UTC, without a date/time dependency.
fn utc_hms() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let day = secs % 86_400;
    format!(
        "{:02}:{:02}:{:02}",
        day / 3_600,
        (day % 3_600) / 60,
        day % 60
    )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used, clippy::unwrap_used)]

    use super::*;

    #[test]
    fn an_unreadable_ledger_becomes_an_error_not_a_panic() {
        let ledger = Ledger::read(Path::new("/nonexistent/switchyard-ui-probe.db"));
        assert!(ledger.cards.is_empty());
        assert!(ledger.error.is_some(), "a failed read is reported");
        assert!(ledger.read_at_utc.is_some(), "and still timestamped");
    }

    #[test]
    fn a_directory_is_rejected_as_data() {
        let ledger = Ledger::read(Path::new("/tmp"));
        assert!(ledger.error.is_some());
    }

    #[test]
    fn progress_is_measured_along_the_forward_path_only() {
        let planning = &Ledger::fixture_planning().cards[0];
        assert_eq!(planning.progress(), Some((1, 9)));

        let attention = &Ledger::fixture_needs_attention().cards[0];
        assert_eq!(
            attention.progress(),
            None,
            "needs_attention is a status, not a position on the path"
        );
    }

    #[test]
    fn a_published_mission_counts_as_complete_not_one_short() {
        let published = &Ledger::fixture_published().cards[0];
        assert_eq!(published.progress(), Some((9, 9)));
    }

    #[test]
    fn uncertainty_always_needs_attention() {
        assert!(Ledger::fixture_needs_attention().cards[0].needs_attention());
        assert!(!Ledger::fixture_planning().cards[0].needs_attention());
        assert!(!Ledger::fixture_published().cards[0].needs_attention());
    }

    #[test]
    fn a_review_matches_only_the_exact_inspected_digest() {
        let reviewed = &Ledger::fixture_awaiting_review().cards[0];
        assert_eq!(reviewed.review_matches(), Some(true));

        let drifted = &Ledger::fixture_needs_attention().cards[0];
        assert_eq!(
            drifted.review_matches(),
            Some(false),
            "the approved digest no longer matches the content"
        );

        let unreviewed = &Ledger::fixture_planning().cards[0];
        assert_eq!(unreviewed.review_matches(), None);
    }

    #[test]
    fn pending_commands_echo_a_recover_not_a_duplicate_admit() {
        let ledger = Ledger::fixture_awaiting_review();
        let echoes = ledger.cli_echo(&ledger.cards[0]);

        assert_eq!(echoes[0].label, "status");
        assert!(!echoes[0].mutates, "a status read mutates nothing");
        assert!(
            echoes[0]
                .command
                .contains("--db /tmp/switchyard/mission.db")
        );

        let workspace = echoes.iter().find(|e| e.label == "workspace");
        assert!(workspace.is_some_and(|e| e.mutates));

        let recover = echoes
            .iter()
            .find(|e| e.label == "recover")
            .expect("pending commands echo a recover");
        assert!(recover.mutates);
        assert!(recover.command.contains("mission recover --db"));
        assert!(
            recover
                .note
                .as_deref()
                .is_some_and(|n| n.contains("already admitted"))
        );
        assert!(
            echoes.iter().all(|e| !e.command.contains("--rationale ")),
            "`mission command` has no --rationale flag"
        );
        assert!(
            echoes.iter().all(|e| e.label != "approve_review"),
            "re-admitting a pending command would duplicate it"
        );
    }

    #[test]
    fn a_phase_decision_echoes_flags_and_a_payload_the_cli_accepts() {
        let review = MissionCard {
            mission_id: "msn-x".to_string(),
            phase: MissionPhase::AwaitingReview,
            revision: 7,
            attempt_id: "att-01".to_string(),
            content_digest: "d41e".to_string(),
            ..MissionCard::blank()
        };
        let ledger = Ledger {
            source: "/tmp/x.db".to_string(),
            cards: vec![review.clone()],
            ..Ledger::default()
        };
        let approve = ledger
            .cli_echo(&review)
            .into_iter()
            .find(|e| e.label == "approve_review")
            .expect("awaiting review echoes approve_review");
        assert!(approve.mutates);
        assert!(approve.command.contains("--kind approve_review"));
        assert!(approve.command.contains("--expected-revision 7"));
        assert!(approve.command.contains("--attempt att-01"));
        assert!(approve.command.contains("--command-id <id-you-generate>"));
        assert!(
            approve
                .command
                .contains("--payload '{\"digest\":\"d41e\",\"rationale\":\"<why>\"}'"),
            "rationale lives in the payload, not as a flag: {}",
            approve.command
        );
        assert!(!approve.command.contains("--rationale "));
        assert!(
            approve
                .note
                .as_deref()
                .is_some_and(|n| n.contains("lost response"))
        );

        let plan = MissionCard {
            mission_id: "msn-y".to_string(),
            phase: MissionPhase::AwaitingPlanApproval,
            revision: 3,
            attempt_id: "att-02".to_string(),
            plan_digest: Some("91af".to_string()),
            ..MissionCard::blank()
        };
        let ledger = Ledger {
            source: "/tmp/y.db".to_string(),
            cards: vec![plan.clone()],
            ..Ledger::default()
        };
        let approve_plan = ledger
            .cli_echo(&plan)
            .into_iter()
            .find(|e| e.label == "approve_plan")
            .expect("awaiting plan approval echoes approve_plan");
        assert!(
            approve_plan
                .command
                .contains("--payload '{\"planDigest\":\"91af\"}'")
        );
    }

    #[test]
    fn a_mission_without_a_workspace_echoes_no_workspace_command() {
        let ledger = Ledger {
            source: "/tmp/switchyard/none.db".to_string(),
            cards: vec![MissionCard {
                mission_id: "msn-none".to_string(),
                ..MissionCard::blank()
            }],
            ..Ledger::default()
        };
        let echoes = ledger.cli_echo(&ledger.cards[0]);
        assert!(
            echoes.iter().all(|e| e.label != "workspace"),
            "no worktree exists, so no worktree command is offered"
        );
        assert_eq!(echoes.len(), 1, "only the honest read remains");
    }

    #[test]
    fn the_happy_path_ends_at_published() {
        assert_eq!(HAPPY_PATH[0], MissionPhase::Draft);
        assert_eq!(HAPPY_PATH[HAPPY_PATH.len() - 1], MissionPhase::Published);
        assert_eq!(HAPPY_PATH.len(), 9);
    }
}
