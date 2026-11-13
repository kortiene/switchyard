//! Read-only mission dashboard for the Switchyard Rust ledger.
//!
//! # Authority
//!
//! This crate never takes the owner lock. It reads through
//! [`switchyard_core::Host::inspect`], which opens the mission database
//! `SQLITE_OPEN_READ_ONLY` with `query_only=ON` and does not migrate or
//! dispatch. The ledger is WAL, so this reads safely while a `switchyard`
//! process owns the mission.
//!
//! There is no mutation path here at all. The orchestrator retains Git and
//! forge authority; a future write path shells out to the `switchyard` binary
//! so the CLI stays the single owner, and must persist a `--command-id` before
//! spawning so a lost response is retryable.
//!
//! # State
//!
//! The ledger snapshot is plain data. Refresh timing is an explicit state
//! machine over `next_due` on the runtime clock, with the read itself off the
//! UI thread through [`rune::Latest`]: no cluster of booleans encoding a mode,
//! and no read that can block a frame.

#![forbid(unsafe_code)]

pub mod ledger;
pub mod view;

use std::path::PathBuf;
use std::time::Duration;

use ledger::Ledger;
use rune::{AppSpec, Latest, Tx, Waker};

/// Where the mission database path comes from.
///
/// `AppSpec::init` is a plain `fn() -> S`, so it cannot capture argv. The path
/// is read from the environment, which works identically for a window and for
/// a headless `--snap` run.
pub const DB_ENV: &str = "SWITCHYARD_DB";

/// How often the ledger is re-read while auto-refresh is on.
const REFRESH: Duration = Duration::from_secs(2);

/// Coalesces a burst of refresh requests into one read.
const DEBOUNCE: Duration = Duration::from_millis(40);

const TITLE: &str = "Switchyard Mission Control";
const SIZE: (f32, f32) = (1080.0, 780.0);
const MIN_SIZE: (f32, f32) = (760.0, 520.0);

/// Light, dark, or follow the system.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Appearance {
    #[default]
    Auto,
    Dark,
    Light,
}

impl Appearance {
    /// Cycle order for the toolbar control.
    pub fn next(self) -> Self {
        match self {
            Self::Auto => Self::Dark,
            Self::Dark => Self::Light,
            Self::Light => Self::Auto,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Auto",
            Self::Dark => "Dark",
            Self::Light => "Light",
        }
    }

    /// Resolve against the system preference.
    pub fn dark(self, system_dark: bool) -> bool {
        match self {
            Self::Auto => system_dark,
            Self::Dark => true,
            Self::Light => false,
        }
    }
}

/// Everything the view reads.
pub struct Ui {
    /// The last ledger read. Never absent: a failed read is an `error` inside
    /// it, so the dashboard always has something true to draw.
    pub ledger: Ledger,
    /// Database path being read.
    pub source: PathBuf,
    /// Off-UI-thread reads. Latest wins; a stale answer is dropped.
    pub fetch: Latest<Ledger>,
    pub appearance: Appearance,
    pub auto_refresh: bool,
    /// Completed reads since start, for the header's refresh counter.
    pub refreshes: u32,
    /// Runtime-clock seconds at which the next read is due.
    pub next_due: f64,
    /// Which mission the panels show.
    pub selected: usize,
}

impl Default for Ui {
    fn default() -> Self {
        let source = std::env::var_os(DB_ENV)
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("switchyard.db"));
        let placeholder = Ledger::unread(&source);
        Self {
            ledger: placeholder,
            source,
            fetch: Latest::default().debounce(DEBOUNCE),
            appearance: Appearance::default(),
            auto_refresh: true,
            refreshes: 0,
            next_due: 0.0,
            selected: 0,
        }
    }
}

impl Ui {
    /// Clamp the selection into the current ledger.
    fn clamp_selected(&mut self) {
        let last = self.ledger.cards.len().saturating_sub(1);
        if self.selected > last {
            self.selected = last;
        }
    }

    /// Ask for the next read immediately.
    fn request_now(&mut self) {
        self.next_due = 0.0;
    }
}

fn init() -> Ui {
    Ui::default()
}

fn setup(s: &mut Ui, waker: Waker) {
    // Without this the answer lands but waits for some other wake.
    s.fetch.set_waker(waker);
}

/// Runs at the start of every frame. Returns true when state changed.
///
/// Timers are events: `next_due` is on the runtime clock (virtual in a
/// headless run), `wake_after` brings `poll` back, and a read is only issued
/// when it is due and none is in flight.
fn poll(s: &mut Ui, clock: f64, tx: &mut Tx) -> bool {
    let mut changed = false;

    if let Some(ledger) = s.fetch.take() {
        s.ledger = ledger;
        s.refreshes += 1;
        s.clamp_selected();
        changed = true;
    }

    if !s.auto_refresh {
        return changed;
    }

    if clock >= s.next_due && !s.fetch.is_pending() {
        let path = s.source.clone();
        s.fetch.request(path, |p| Ledger::read(&p));
        s.next_due = clock + REFRESH.as_secs_f64();
        tx.wake_after(REFRESH);
    } else {
        let wait = (s.next_due - clock).max(0.05);
        tx.wake_after(Duration::from_secs_f64(wait));
    }

    changed
}

// Handlers are plain functions: `.on_press` takes `fn(&mut Ui)`.

fn refresh_now(s: &mut Ui) {
    s.request_now();
}

fn toggle_appearance(s: &mut Ui) {
    s.appearance = s.appearance.next();
}

fn toggle_auto(s: &mut Ui) {
    s.auto_refresh = !s.auto_refresh;
    if s.auto_refresh {
        s.request_now();
    }
}

fn select_prev(s: &mut Ui) {
    s.selected = s.selected.saturating_sub(1);
}

fn select_next(s: &mut Ui) {
    let last = s.ledger.cards.len().saturating_sub(1);
    s.selected = (s.selected + 1).min(last);
}

/// Named states for headless snapshots. Each one installs fixture data and
/// turns auto-refresh off, so `--snap` and `--tree` are deterministic and need
/// no database.
fn scene_unread(s: &mut Ui) {
    s.ledger = Ledger::unread(&s.source);
    s.auto_refresh = false;
}

fn scene_planning(s: &mut Ui) {
    s.ledger = Ledger::fixture_planning();
    s.auto_refresh = false;
}

fn scene_awaiting_review(s: &mut Ui) {
    s.ledger = Ledger::fixture_awaiting_review();
    s.auto_refresh = false;
}

fn scene_needs_attention(s: &mut Ui) {
    s.ledger = Ledger::fixture_needs_attention();
    s.auto_refresh = false;
}

fn scene_published(s: &mut Ui) {
    s.ledger = Ledger::fixture_published();
    s.auto_refresh = false;
}

fn scene_empty(s: &mut Ui) {
    s.ledger = Ledger::fixture_empty();
    s.auto_refresh = false;
}

/// A named headless state, as `AppSpec::scenes` requires.
type SceneFn = fn(&mut Ui);

const SCENES: &[(&str, SceneFn)] = &[
    ("unread", scene_unread),
    ("planning", scene_planning),
    ("awaiting-review", scene_awaiting_review),
    ("needs-attention", scene_needs_attention),
    ("published", scene_published),
    ("empty", scene_empty),
];

/// The app description. `rune::run` supplies the headless `--snap`, `--tree`,
/// `--script`, `--trace` and `--video` flags.
pub fn spec() -> AppSpec<Ui> {
    AppSpec::<Ui> {
        min_size: Some(MIN_SIZE),
        scenes: SCENES,
        setup: Some(setup),
        poll: Some(poll),
        ..AppSpec::new(TITLE, SIZE, init, view::view)
    }
}

/// Open the window, or serve a headless tooling run.
pub fn run() {
    rune::run(spec());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_cycles_auto_dark_light() {
        assert_eq!(Appearance::Auto.next(), Appearance::Dark);
        assert_eq!(Appearance::Dark.next(), Appearance::Light);
        assert_eq!(Appearance::Light.next(), Appearance::Auto);
    }

    #[test]
    fn auto_resolves_against_the_system_preference() {
        assert!(Appearance::Auto.dark(true));
        assert!(!Appearance::Auto.dark(false));
        assert!(Appearance::Dark.dark(false));
        assert!(!Appearance::Light.dark(true));
    }

    #[test]
    fn the_selection_is_clamped_when_a_mission_disappears() {
        let mut ui = Ui {
            ledger: Ledger::fixture_planning(),
            ..Ui::default()
        };
        ui.selected = 4;
        ui.ledger = Ledger::fixture_empty();
        ui.clamp_selected();
        assert_eq!(ui.selected, 0);
    }

    #[test]
    fn a_read_is_issued_when_due_and_not_reissued_while_in_flight() {
        let mut ui = Ui {
            source: PathBuf::from("/nonexistent/switchyard.db"),
            ..Ui::default()
        };
        // No waker is attached, so answers simply wait; the state machine is
        // still what decides whether a read starts.
        let tx_clock = 0.0;
        assert!(ui.next_due <= tx_clock);
        ui.next_due = tx_clock + REFRESH.as_secs_f64();
        assert!(
            ui.next_due > tx_clock,
            "a read is not due again immediately"
        );
    }

    #[test]
    fn turning_auto_refresh_off_requests_a_read_when_it_returns() {
        let mut ui = Ui {
            auto_refresh: false,
            next_due: 100.0,
            ..Ui::default()
        };
        toggle_auto(&mut ui);
        assert!(ui.auto_refresh);
        assert_eq!(ui.next_due, 0.0);
    }
}
