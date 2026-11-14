//! Every command the dashboard echoes must parse with the real `switchyard`
//! clap definition (`crates/switchyard/src/cli.rs`), not a copy of the flags.

use clap::Parser;
use switchyard_core::Phase as MissionPhase;

use crate::ledger::{Ledger, MissionCard, shell_split};

#[allow(dead_code)]
mod switchyard_cli {
    include!("../../switchyard/src/cli.rs");
}

use switchyard_cli::{Cli, Command, MissionCommand};

#[test]
fn every_echoed_command_parses_with_the_real_cli() {
    let awkward = Ledger {
        source: "/tmp/operator's ledger/mission db.db".to_string(),
        cards: vec![MissionCard {
            mission_id: "msn x".to_string(),
            phase: MissionPhase::AwaitingReview,
            revision: 7,
            attempt_id: "att 01".to_string(),
            content_digest: "d41e".to_string(),
            workspace_state: Some("ready".to_string()),
            ..MissionCard::blank()
        }],
        ..Ledger::default()
    };
    let plan = Ledger {
        source: "/tmp/y.db".to_string(),
        cards: vec![MissionCard {
            mission_id: "msn-y".to_string(),
            phase: MissionPhase::AwaitingPlanApproval,
            revision: 3,
            attempt_id: "att-02".to_string(),
            plan_digest: Some("91af".to_string()),
            ..MissionCard::blank()
        }],
        ..Ledger::default()
    };
    let ledgers = [
        Ledger::fixture_planning(),
        Ledger::fixture_awaiting_review(),
        Ledger::fixture_needs_attention(),
        Ledger::fixture_published(),
        awkward,
        plan,
    ];

    for ledger in &ledgers {
        for card in &ledger.cards {
            let echoes = ledger.cli_echo(card);
            assert!(!echoes.is_empty(), "a mission always echoes status");
            for echo in &echoes {
                assert!(
                    !echo.command.contains(" --rationale"),
                    "mission command has no --rationale flag: {}",
                    echo.command
                );
                let argv = shell_split(&echo.command).unwrap_or_else(|error| {
                    panic!("{error} in {}", echo.command);
                });
                let cli = Cli::try_parse_from(&argv).unwrap_or_else(|error| {
                    panic!(
                        "echo {} did not parse with the switchyard CLI:\n{}\n{error}",
                        echo.label, echo.command
                    );
                });
                match cli.command {
                    Command::Mission { action } => {
                        assert_parsed(&echo.label, &action, ledger, card);
                    }
                }
            }
        }
    }
}

fn assert_parsed(label: &str, action: &MissionCommand, ledger: &Ledger, card: &MissionCard) {
    let db = ledger.source.as_str();
    match (label, action) {
        ("status", MissionCommand::Status { db: path, mission }) => {
            assert_eq!(path.display().to_string(), db);
            assert_eq!(mission.as_deref(), Some(card.mission_id.as_str()));
        }
        (
            "workspace",
            MissionCommand::Workspace {
                db: path,
                mission,
                attempt,
                root,
            },
        ) => {
            assert_eq!(path.display().to_string(), db);
            assert_eq!(mission, &card.mission_id);
            assert_eq!(attempt, &card.attempt_id);
            assert_eq!(root.display().to_string(), "<dir-outside-the-checkout>");
        }
        ("recover", MissionCommand::Recover { db: path, mission }) => {
            assert_eq!(path.display().to_string(), db);
            assert_eq!(mission, &card.mission_id);
            assert!(
                !card.pending_commands.is_empty(),
                "recover is the echo for commands that are already admitted"
            );
        }
        (
            label,
            MissionCommand::Command {
                db: path,
                mission,
                kind,
                expected_revision,
                attempt,
                command_id,
                actor,
                payload,
                persist_only,
            },
        ) if label == kind.as_str() => {
            assert_eq!(path.display().to_string(), db);
            assert_eq!(mission, &card.mission_id);
            assert_eq!(*expected_revision, card.revision);
            assert_eq!(attempt.as_deref(), Some(card.attempt_id.as_str()));
            assert_eq!(command_id.as_deref(), Some("<id-you-generate>"));
            assert_eq!(actor, "<operator>");
            assert!(!persist_only);
            match kind.as_str() {
                "approve_review" => assert_eq!(
                    payload,
                    &format!(
                        "{{\"digest\":\"{}\",\"rationale\":\"<why>\"}}",
                        card.content_digest
                    )
                ),
                "approve_plan" => {
                    let digest = card.plan_digest.as_deref().unwrap_or("");
                    assert_eq!(payload, &format!("{{\"planDigest\":\"{digest}\"}}"));
                }
                other => panic!("unexpected echoed command kind {other}"),
            }
        }
        (label, other) => panic!("echo {label} parsed as a different command: {other:?}"),
    }
}

#[test]
fn mission_command_rejects_a_rationale_flag() {
    let rejected = Cli::try_parse_from([
        "switchyard",
        "mission",
        "command",
        "--db",
        "x.db",
        "--mission",
        "m",
        "--kind",
        "approve_review",
        "--expected-revision",
        "1",
        "--rationale",
        "because",
    ]);
    assert!(
        rejected.is_err(),
        "the real CLI has no --rationale flag on mission command"
    );
}
