//! Frozen-budget admission. Exhaustion is only `detect_uncertainty`.
//!
//! Operator cancel reuses [`Host::dispatch_stops`]. On the confined adapter
//! that is `stop` (terminate, then `rekill_and_prove`), then
//! `reconcile_execution` and the `ops_blocked` check. This file does not
//! open a second teardown and does not admit `request_verification`.

use serde_json::Value;

use super::*;
use crate::budget::{BudgetUsage, FrozenBudgets};
use crate::domain::AttentionReason;

impl Host {
    pub fn frozen_budgets(&self, mission_id: &str) -> Result<Option<FrozenBudgets>> {
        let mission = self.store.load_mission(mission_id)?;
        match mission.attempt.frozen_inputs_json.as_deref() {
            None => Ok(None),
            Some(document) => {
                let Ok(value) = serde_json::from_str::<Value>(document) else {
                    return Ok(None);
                };
                Ok(Some(FrozenBudgets::parse(&value)))
            }
        }
    }

    /// `detect_uncertainty` with `runtime_budget_exhausted`, then
    /// `needs_attention`. A later call does not write. The freeze, plan, and
    /// content digest stay as they were. This never admits verification.
    pub fn exhaust_runtime_budget(&mut self, mission_id: &str) -> Result<MissionSnapshot> {
        let mission = self.store.load_mission(mission_id)?;
        if mission.phase.is_terminal()
            || matches!(
                mission.phase,
                Phase::NeedsAttention | Phase::Cancelling | Phase::Pausing
            )
        {
            return self.snapshot(mission_id);
        }
        self.admit_checked(
            AdmitRequest {
                command_id: None,
                mission_id: mission.id.clone(),
                attempt_id: Some(mission.attempt.id.clone()),
                expected_revision: mission.revision,
                actor: "host".into(),
                origin: CommandOrigin::HostSupervisor,
                body: CommandBody::DetectUncertainty {
                    reason: AttentionReason::RuntimeBudgetExhausted,
                    content_digest: None,
                },
            },
            None,
            true,
        )?;
        self.snapshot(mission_id)
    }

    /// `true` when this mission's runtime config is the composed
    /// `s3-acceptance` replay. A missing fixture keeps that replay. Any other
    /// fixture, including a later product fixture, completes on idle.
    pub fn s3_acceptance_replay(&self, mission_id: &str) -> bool {
        let Ok(mission) = self.store.load_mission(mission_id) else {
            return true;
        };
        let Ok(value) = serde_json::from_str::<Value>(&mission.attempt.runtime_config_json) else {
            return true;
        };
        match value.get("fixture").and_then(Value::as_str) {
            Some("s3-acceptance") | None => true,
            Some(_) => false,
        }
    }

    pub fn enforce_attempt_wall(&mut self, mission_id: &str) -> Result<()> {
        let (bridge, verifier) = self.attempt_clock.components();
        self.enforce_usage(
            mission_id,
            &BudgetUsage {
                bridge_outside_verify: bridge,
                verifier,
                ..BudgetUsage::default()
            },
        )
    }

    pub(crate) fn enforce_usage(&mut self, mission_id: &str, usage: &BudgetUsage) -> Result<()> {
        let Some(budgets) = self.frozen_budgets(mission_id)? else {
            return Ok(());
        };
        if budgets.exceeded(usage) {
            self.exhaust_runtime_budget(mission_id)?;
            return Err(Error::RuntimeBudgetExhausted);
        }
        Ok(())
    }

    /// Ctrl-C / SIGTERM in the waiting process. Admits `cancel` as the
    /// operator, then [`Self::dispatch_stops`]. Confined stop proof is the
    /// existing `ConfinedPiSession::stop` path, not a second teardown.
    pub fn interrupt_operator(&mut self, mission_id: &str) -> Result<MissionSnapshot> {
        let mission = self.store.load_mission(mission_id)?;
        if mission.phase.is_terminal() {
            return self.snapshot(mission_id);
        }
        if mission.phase != Phase::Cancelling {
            self.admit(AdmitRequest {
                command_id: None,
                mission_id: mission_id.to_string(),
                attempt_id: Some(mission.attempt.id.clone()),
                expected_revision: mission.revision,
                actor: "operator".into(),
                origin: CommandOrigin::Operator,
                body: CommandBody::Cancel,
            })?;
        }
        self.dispatch_stops(mission_id)?;
        self.snapshot(mission_id)
    }

    /// True when this bridge generation has passed `wall`. The clock is the
    /// session open, not verifier time and not the old fixed 60s wait.
    pub fn generation_over_bridge_wall(
        &mut self,
        mission_id: &str,
        wall: std::time::Duration,
    ) -> Result<bool> {
        #[cfg(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        ))]
        {
            let mission = self.store.load_mission(mission_id)?;
            let Some(session) = self
                .confined_mut()?
                .sessions
                .get(&mission.attempt.session_id)
            else {
                return Ok(false);
            };
            Ok(session.wall_exhausted() || session.generation_elapsed() > wall)
        }
        #[cfg(not(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        )))]
        {
            let _ = (mission_id, wall);
            Ok(false)
        }
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    pub(crate) fn note_bridge_up(&mut self, attempt_id: &str) -> Result<()> {
        let lifecycle = self
            .store
            .pi_host_view(attempt_id)?
            .map(|view| view["lifecycle"].clone())
            .unwrap_or(Value::Null);
        // First call only. Later saves must not reload a stale total over
        // time this process has already charged.
        self.attempt_clock.restore(&lifecycle);
        self.attempt_clock.bridge_up();
        Ok(())
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    pub(crate) fn note_bridge_down(&mut self) {
        self.attempt_clock.bridge_down();
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    pub(crate) fn stamp_attempt_clock(&mut self, lifecycle: &mut Value) {
        let (bridge, verifier) = self.attempt_clock.components();
        let Some(object) = lifecycle.as_object_mut() else {
            return;
        };
        object.insert(
            "attemptBridgeMs".into(),
            serde_json::json!(u64_millis(bridge)),
        );
        object.insert(
            "attemptVerifierMs".into(),
            serde_json::json!(u64_millis(verifier)),
        );
    }

    #[cfg(test)]
    pub(crate) fn begin_bridge_for_test(&mut self) {
        self.attempt_clock.mark_restored();
        self.attempt_clock.bridge_up();
    }

    #[cfg(test)]
    pub(crate) fn begin_verify_for_test(&mut self) {
        self.attempt_clock.begin_verify();
    }

    #[cfg(test)]
    pub(crate) fn end_verify_for_test(&mut self) {
        self.attempt_clock.end_verify();
    }

    #[cfg(test)]
    pub(crate) fn advance_attempt_clock_for_test(&mut self, step: std::time::Duration) {
        self.attempt_clock.advance_for_test(step);
    }
}

#[cfg(all(
    feature = "pi-contract",
    feature = "linux-execution",
    target_os = "linux"
))]
fn u64_millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
