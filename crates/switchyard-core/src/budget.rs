//! Frozen §3.4 runtime budgets. Written once at `start_planning`.
//!
//! Nothing here rewrites a freeze. A different limit is a successor attempt.
//! Exhaustion is `detect_uncertainty` / `runtime_budget_exhausted`, then
//! `needs_attention`. This module does not admit `request_verification`.

use std::time::Duration;

use serde_json::Value;

/// CLI default. Frozen from the attempt; a later change is a successor.
pub const DEFAULT_MAX_PLAN_REVISIONS: u32 = 2;
/// CLI default. Frozen from the attempt; a later change is a successor.
pub const DEFAULT_MAX_REPAIRS: u32 = 1;
/// Per bridge generation. The conformance 60s watchdog stays on direct opens.
pub const BRIDGE_WALL_SECONDS: u64 = 20 * 60;
/// Attempt budget. What it counts is [`attempt_wall_charge`].
pub const ATTEMPT_WALL_SECONDS: u64 = 45 * 60;
/// Frozen per-mission model-request cap.
///
/// Owner: "40 requests is the frozen per-mission cap".
/// `ConfinedPiSession::model_request_cap` carries the frozen value to
/// `start_model_proxy`. Branch `d1b-splice-timeout-connection-cap` passes it
/// into `ModelProxy::start`. This slice does not edit `egress.rs`.
pub const MAX_MODEL_REQUESTS: u64 = 40;
pub const MAX_REQUEST_BYTES: u64 = 256 * 1024;
pub const MAX_RESPONSE_BYTES: u64 = 1024 * 1024;
pub const BRIDGE_CPU_SECONDS: u64 = 120;

/// What the 45-minute attempt wall counts. This is the only switch.
///
/// Confirmed by the owner for #218: "yes, #218's 45-min attempt wall counts
/// only bridge-alive time and excludes mission verify".
///
/// `false`: charge only time the bridge is alive outside host `mission verify`.
/// With `maxRepairs` 1 there can be two verifies, each with the frozen 1800s
/// verifier deadline, and that time is not charged. `true` would add verifier
/// time as well.
pub const ATTEMPT_WALL_COUNTS_VERIFIER: bool = false;

/// Time charged to the frozen attempt wall.
///
/// `bridge_outside_verify` is time the bridge was up and the host was not
/// inside `mission verify`. `verifier` is host `mission verify` time,
/// including the frozen 1800s runs. The switch is
/// [`ATTEMPT_WALL_COUNTS_VERIFIER`].
pub fn attempt_wall_charge(bridge_outside_verify: Duration, verifier: Duration) -> Duration {
    if ATTEMPT_WALL_COUNTS_VERIFIER {
        bridge_outside_verify.saturating_add(verifier)
    } else {
        bridge_outside_verify
    }
}

/// Clock for [`attempt_wall_charge`]. Verify pauses the bridge charge so a
/// verifier run is not runtime time unless the switch says to add it back.
#[derive(Clone, Debug, Default)]
pub struct AttemptClock {
    bridge_charged: Duration,
    verifier: Duration,
    bridge_since: Option<std::time::Instant>,
    bridge_up: bool,
    verify_started: Option<std::time::Instant>,
    #[cfg_attr(
        not(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        )),
        allow(dead_code)
    )]
    restored: bool,
}

impl AttemptClock {
    // Used by the confined host and by the attempt-wall test. The default
    // library target does not open a bridge.
    #[cfg_attr(
        not(any(
            test,
            all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            )
        )),
        allow(dead_code)
    )]
    pub(crate) fn bridge_up(&mut self) {
        if self.bridge_up {
            return;
        }
        self.bridge_up = true;
        if self.verify_started.is_none() {
            self.bridge_since = Some(std::time::Instant::now());
        }
    }

    #[cfg_attr(
        not(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        )),
        allow(dead_code)
    )]
    pub(crate) fn bridge_down(&mut self) {
        self.fold_bridge();
        self.bridge_up = false;
    }

    pub(crate) fn begin_verify(&mut self) {
        if self.verify_started.is_some() {
            return;
        }
        self.fold_bridge();
        self.verify_started = Some(std::time::Instant::now());
    }

    pub(crate) fn end_verify(&mut self) {
        if let Some(start) = self.verify_started.take() {
            self.verifier = self.verifier.saturating_add(start.elapsed());
        }
        if self.bridge_up {
            self.bridge_since = Some(std::time::Instant::now());
        }
    }

    /// Components stored across reopen. Not the charged total: the switch is
    /// applied when the wall is checked, so flipping it does not rewrite history.
    pub(crate) fn components(&mut self) -> (Duration, Duration) {
        self.fold_bridge();
        if self.bridge_up && self.verify_started.is_none() {
            self.bridge_since = Some(std::time::Instant::now());
        }
        (self.bridge_charged, self.verifier)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn charge(&mut self) -> Duration {
        let (bridge, verifier) = self.components();
        attempt_wall_charge(bridge, verifier)
    }

    #[cfg_attr(
        not(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        )),
        allow(dead_code)
    )]
    pub(crate) fn restore(&mut self, lifecycle: &Value) {
        if self.restored {
            return;
        }
        self.restored = true;
        if let Some(ms) = lifecycle.get("attemptBridgeMs").and_then(Value::as_u64) {
            self.bridge_charged = Duration::from_millis(ms);
        }
        if let Some(ms) = lifecycle.get("attemptVerifierMs").and_then(Value::as_u64) {
            self.verifier = Duration::from_millis(ms);
        }
    }

    #[cfg(test)]
    pub(crate) fn mark_restored(&mut self) {
        self.restored = true;
    }

    fn fold_bridge(&mut self) {
        if let Some(start) = self.bridge_since.take() {
            self.bridge_charged = self.bridge_charged.saturating_add(start.elapsed());
        }
    }

    /// Move the active timer forward without sleeping. Used by the verify
    /// exclusion test and the attempt-wall exhaustion test.
    #[cfg(test)]
    pub(crate) fn advance_for_test(&mut self, step: Duration) {
        if self.verify_started.is_some() {
            self.verifier = self.verifier.saturating_add(step);
        } else if self.bridge_up {
            self.bridge_charged = self.bridge_charged.saturating_add(step);
        }
    }
}

/// Budgets read back from a frozen document.
///
/// A missing key is absent, not an error. Absence is the pre-#218 ledger:
/// that cap is not enforced. The 40 model-request cap is frozen here and
/// enforced only by the splice allowance after #226 and #229 land.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrozenBudgets {
    pub bridge_wall: Option<Duration>,
    pub attempt_wall: Option<Duration>,
    pub max_model_requests: Option<u64>,
    pub bridge_cpu: Option<Duration>,
}

impl FrozenBudgets {
    /// Never fails because a budget key is missing.
    pub(crate) fn parse(document: &Value) -> Self {
        let limits = document.get("executionLimits");
        Self {
            bridge_wall: seconds(limits, "bridgeWallSeconds"),
            attempt_wall: seconds(limits, "attemptWallSeconds"),
            max_model_requests: number(limits, "maxModelRequests"),
            bridge_cpu: seconds(limits, "bridgeCpuSeconds"),
        }
    }

    /// `true` when a present cap is past the measurement.
    ///
    /// The attempt wall uses [`attempt_wall_charge`]. Model-request count and
    /// host RPC sizes are not enforced here.
    pub(crate) fn exceeded(&self, usage: &BudgetUsage) -> bool {
        let attempt = attempt_wall_charge(usage.bridge_outside_verify, usage.verifier);
        self.bridge_wall
            .is_some_and(|wall| usage.bridge_generation > wall)
            || self.attempt_wall.is_some_and(|wall| attempt > wall)
            || self.bridge_cpu.is_some_and(|cap| usage.bridge_cpu > cap)
    }
}

/// One observation of the counters. Zero means "not measured".
#[derive(Clone, Debug, Default)]
pub struct BudgetUsage {
    pub bridge_generation: Duration,
    pub bridge_outside_verify: Duration,
    pub verifier: Duration,
    pub bridge_cpu: Duration,
}

fn seconds(limits: Option<&Value>, key: &str) -> Option<Duration> {
    number(limits, key).map(Duration::from_secs)
}

fn number(limits: Option<&Value>, key: &str) -> Option<u64> {
    limits
        .and_then(|value| value.get(key))
        .and_then(Value::as_u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attempt_wall_excludes_mission_verify() {
        const {
            assert!(!ATTEMPT_WALL_COUNTS_VERIFIER);
        }
        let bridge = Duration::from_secs(20 * 60);
        // maxRepairs 1 allows the original verify and one repair verify.
        // Each keeps the frozen 1800s verifier deadline: 60 minutes.
        let verifier = Duration::from_secs(1800 * 2);
        let charged = attempt_wall_charge(bridge, verifier);
        assert_eq!(charged, bridge);
        assert!(charged < Duration::from_secs(ATTEMPT_WALL_SECONDS));
        assert!(bridge + verifier > Duration::from_secs(ATTEMPT_WALL_SECONDS));
    }

    #[test]
    fn verify_interval_is_not_bridge_time() {
        let mut clock = AttemptClock::default();
        clock.bridge_up();
        clock.advance_for_test(Duration::from_secs(20 * 60));
        clock.begin_verify();
        clock.advance_for_test(Duration::from_secs(1800 * 2));
        clock.end_verify();
        let charged = clock.charge();
        // A few milliseconds of real bridge time may sit beside the advanced
        // 20 minutes. The two 1800s verifier deadlines must not be in it.
        assert!(charged >= Duration::from_secs(20 * 60));
        assert!(charged < Duration::from_secs(20 * 60 + 5), "{charged:?}");
        assert!(clock.verifier >= Duration::from_secs(1800 * 2));
        assert!(charged < Duration::from_secs(ATTEMPT_WALL_SECONDS));
    }

    #[test]
    fn section_3_4_numbers_are_the_frozen_caps() {
        assert_eq!(DEFAULT_MAX_PLAN_REVISIONS, 2);
        assert_eq!(DEFAULT_MAX_REPAIRS, 1);
        assert_eq!(BRIDGE_WALL_SECONDS, 1200);
        assert_eq!(ATTEMPT_WALL_SECONDS, 2700);
        assert_eq!(MAX_MODEL_REQUESTS, 40);
        assert_eq!(MAX_REQUEST_BYTES, 256 * 1024);
        assert_eq!(MAX_RESPONSE_BYTES, 1024 * 1024);
        assert_eq!(BRIDGE_CPU_SECONDS, 120);
    }

    #[test]
    fn stop_and_rekill_do_not_resolve_dns_or_open_a_credential_file() {
        let session = include_str!("execution/confined_pi/mod.rs");
        let stop = slice_fn(session, "pub fn stop(", "pub fn stderr_tail");
        assert!(
            stop.contains("rekill_and_prove"),
            "stop must prove the cgroup through rekill_and_prove"
        );
        assert_local(stop);
        let cgroup = include_str!("linux_execution/cgroup.rs");
        let rekill = slice_fn(
            cgroup,
            "pub(crate) fn rekill_and_prove",
            "pub(crate) fn kill_and_prove",
        );
        assert_local(rekill);
        let dispatch = include_str!("service/pi_dispatch.rs");
        let cancel = slice_fn(dispatch, "fn dispatch_confined_stops", "fn poll_confined");
        assert!(cancel.contains(".stop()"));
        assert!(cancel.contains("reconcile_execution"));
        assert!(cancel.contains("ops_blocked"));
        assert!(
            !cancel.contains("RequestVerification"),
            "cancel must not admit verification"
        );
        assert_local(cancel);
    }

    fn slice_fn<'a>(source: &'a str, start: &str, end: &str) -> &'a str {
        let from = source
            .find(start)
            .unwrap_or_else(|| panic!("missing {start}"));
        let rest = &source[from..];
        let to = rest.find(end).unwrap_or_else(|| panic!("missing {end}"));
        &rest[..to]
    }

    fn assert_local(body: &str) {
        assert!(!body.contains("to_socket_addrs"), "{body}");
        assert!(!body.contains("getaddrinfo"), "{body}");
        assert!(!body.contains("credential"), "{body}");
        assert!(!body.contains("TcpStream"), "{body}");
    }
}
