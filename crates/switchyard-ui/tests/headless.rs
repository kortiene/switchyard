//! Headless guarantees: the app spec is constructible, every named scene
//! renders without a database, and nothing in the spec can reach the network
//! or a model.
//!
//! These are the properties CI relies on when it renders evidence with
//! `switchyard-ui --snap out.png --scene needs-attention`.

use rune::{Env, Runtime};
use switchyard_ui::{DB_ENV, Ui, spec, view};

/// The scenes a snapshot run may ask for.
const REQUIRED_SCENES: [&str; 7] = [
    "unread",
    "unreadable",
    "planning",
    "awaiting-review",
    "needs-attention",
    "published",
    "empty",
];

fn render(ui: Ui) -> Runtime<Ui> {
    let mut rt = Runtime::new(ui, view::view, (1080.0, 780.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

#[test]
fn the_spec_exposes_every_scene_a_snapshot_needs() {
    let spec = spec();
    let names: Vec<&str> = spec.scenes.iter().map(|(name, _)| *name).collect();
    for required in REQUIRED_SCENES {
        assert!(names.contains(&required), "scene {required} is missing");
    }
    assert_eq!(spec.title, "Switchyard Mission Control");
}

#[test]
fn every_scene_renders_without_a_database_and_without_auto_refresh() {
    for (name, scene) in spec().scenes.iter().copied() {
        let mut ui = Ui::default();
        scene(&mut ui);
        assert!(
            !ui.auto_refresh,
            "scene {name} must be deterministic, so it turns auto-refresh off"
        );

        let rt = render(ui);
        assert!(rt.find("window").is_some(), "{name} lays out a window");
        assert!(rt.find("header").is_some(), "{name} draws the header");
        assert!(
            rt.find("readonly-badge").is_some(),
            "{name} states that it is read-only"
        );
    }
}

#[test]
fn a_scene_with_a_mission_draws_the_full_panel_set() {
    let mut ui = Ui::default();
    for (name, scene) in spec().scenes.iter().copied() {
        if name != "planning" {
            continue;
        }
        scene(&mut ui);
    }
    let rt = render(ui);
    for panel in [
        "overview",
        "mission",
        "pipeline",
        "limits",
        "workspace",
        "commands",
        "attention",
        "cli-echo",
    ] {
        assert!(rt.find(panel).is_some(), "panel {panel} is drawn");
    }
}

#[test]
fn the_database_path_comes_from_the_environment_not_from_argv() {
    // `AppSpec::init` is a plain `fn() -> S` and cannot capture argv, so the
    // path is read from the environment. Assert the contract, not the value:
    // whatever is set must survive into the state.
    let expected = std::env::var(DB_ENV).unwrap_or_else(|_| "switchyard.db".to_string());
    let ui = Ui::default();
    assert_eq!(ui.source.display().to_string(), expected);
    assert_eq!(ui.ledger.source, expected, "the panel shows what is read");
}

#[test]
fn a_default_ui_starts_unread_rather_than_pretending_to_have_read() {
    let ui = Ui::default();
    assert!(ui.ledger.cards.is_empty());
    assert_eq!(ui.ledger.error, None);
    assert_eq!(
        ui.ledger.read_at_utc, None,
        "no read has completed yet, so no timestamp is claimed"
    );
    assert_eq!(ui.refreshes, 0);
}

#[test]
fn reading_a_ledger_that_does_not_exist_reports_rather_than_panics() {
    let ui = Ui {
        source: std::path::PathBuf::from("/nonexistent/switchyard-ui-headless.db"),
        ..Ui::default()
    };
    let ledger = switchyard_ui::ledger::Ledger::read(&ui.source);
    assert!(ledger.error.is_some());
    assert!(ledger.cards.is_empty());

    // And the dashboard still renders that failure.
    let rt = render(Ui {
        ledger,
        auto_refresh: false,
        ..Ui::default()
    });
    assert!(rt.find("overview").is_some());
    assert!(rt.find("mission").is_some());
    assert!(rt.find("no-mission-label").is_some());
}
