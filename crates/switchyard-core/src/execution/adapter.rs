//! Internal dispatch seam. Mission policy and the outbox stay in Host/Store.
use serde_json::{Value, json};

#[cfg(feature = "pi-contract")]
use super::PiRuntime;
use super::{FakeRuntime, RuntimeReceipt, RuntimeRequest};
use crate::domain::{CommandBody, CommandOrigin};
use crate::error::Result;

pub(crate) enum RuntimeAdapter {
    Fake(FakeRuntime),
    #[cfg(feature = "pi-contract")]
    Pi(Box<PiRuntime>),
    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    Confined(Box<super::confined_runtime::ConfinedRuntime>),
}

/// Recovery steps that differ by runtime. Fake and the Pi probe stay inert.
/// Only the confined adapter opens a session, settles a stop before reconcile,
/// resumes after that stop clears a Linux block, and skips verify for a writer
/// it is still supervising.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct RecoveryProfile {
    pub(crate) open_session_before_reconcile: bool,
    pub(crate) settle_stops_before_reconcile: bool,
    pub(crate) resume_after_delivered_stop: bool,
    pub(crate) skip_verify_for_supervised_writers: bool,
}

impl RuntimeAdapter {
    pub(crate) fn recovery_profile(&self) -> RecoveryProfile {
        match self {
            Self::Fake(_) => RecoveryProfile::default(),
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => RecoveryProfile::default(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => RecoveryProfile {
                open_session_before_reconcile: true,
                settle_stops_before_reconcile: true,
                resume_after_delivered_stop: true,
                skip_verify_for_supervised_writers: true,
            },
        }
    }

    pub(crate) fn is_confined(&self) -> bool {
        #[cfg(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        ))]
        {
            matches!(self, Self::Confined(_))
        }
        #[cfg(not(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        )))]
        {
            let _ = self;
            false
        }
    }

    #[cfg(feature = "pi-contract")]
    fn is_fake(&self) -> bool {
        matches!(self, Self::Fake(_))
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Fake(_) => "fake",
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => "pi-probe-v1",
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => "pi-confined-v1",
        }
    }

    pub(crate) fn configuration(&self) -> Value {
        match self {
            Self::Fake(_) => json!({"adapter":"fake", "credentials":"none"}),
            #[cfg(feature = "pi-contract")]
            Self::Pi(runtime) => runtime.configuration(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(runtime) => runtime.configuration(),
        }
    }

    pub(crate) fn fake_mut(&mut self) -> Option<&mut FakeRuntime> {
        match self {
            Self::Fake(runtime) => Some(runtime),
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => None,
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => None,
        }
    }

    pub(crate) fn validate_admission(
        &self,
        body: &CommandBody,
        origin: CommandOrigin,
    ) -> Result<()> {
        #[cfg(feature = "pi-contract")]
        if !self.is_fake() {
            // S2-A has no trusted planning/verification/process-quiescence observer.
            // Do not expose S1 simulation inputs as facts about a child process.
            // HostSupervisor is not a Pi observation; the private host-fact path
            // never reaches this check.
            if matches!(
                origin,
                CommandOrigin::RuntimeObservation | CommandOrigin::HostSupervisor
            ) || matches!(
                body,
                CommandBody::ConfirmStopped
                    | CommandBody::ConfirmCancelled
                    | CommandBody::RequestVerification { .. }
                    | CommandBody::ExplicitRecovery { .. }
            ) {
                return Err(crate::Error::guard(
                    "Pi probe has no mission-fact or execution-quiescence authority",
                ));
            }
        }
        let _ = (body, origin);
        Ok(())
    }

    pub(crate) fn validate_command(&self, request: &RuntimeRequest, payload: &Value) -> Result<()> {
        #[cfg(feature = "pi-contract")]
        if !self.is_fake() {
            return PiRuntime::validate_command(request, payload);
        }
        let _ = (request, payload);
        Ok(())
    }

    pub(crate) fn reconcile(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
    ) -> Result<Option<RuntimeReceipt>> {
        match self {
            Self::Fake(runtime) => {
                let _ = payload;
                Ok(runtime.stored_receipt(&request.request_id))
            }
            #[cfg(feature = "pi-contract")]
            Self::Pi(runtime) => runtime.reconcile(request, payload),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => Err(crate::Error::guard(
                "confined Pi dispatch must be owned by Host",
            )),
        }
    }

    pub(crate) fn dispatch(
        &mut self,
        request: &RuntimeRequest,
        payload: &Value,
    ) -> Result<RuntimeReceipt> {
        match self {
            Self::Fake(runtime) => {
                let _ = payload;
                match request.method {
                    "abort" => runtime.abort(request),
                    "steer" => runtime.steer(request),
                    _ => runtime.submit(request),
                }
            }
            #[cfg(feature = "pi-contract")]
            Self::Pi(runtime) => runtime.dispatch(request, payload),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => Err(crate::Error::guard(
                "confined Pi dispatch must be owned by Host",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_inert(profile: RecoveryProfile) {
        assert!(!profile.open_session_before_reconcile);
        assert!(!profile.settle_stops_before_reconcile);
        assert!(!profile.resume_after_delivered_stop);
        assert!(!profile.skip_verify_for_supervised_writers);
        assert_eq!(profile, RecoveryProfile::default());
    }

    #[test]
    fn recovery_profile_fake_and_pi_probe_are_inert() {
        let dir = tempfile::tempdir().unwrap();
        let fake =
            RuntimeAdapter::Fake(FakeRuntime::open(&dir.path().join("fake-runtime.json")).unwrap());
        assert_inert(fake.recovery_profile());
        #[cfg(feature = "pi-contract")]
        {
            let node = std::env::current_exe().unwrap();
            let pi = RuntimeAdapter::Pi(Box::new(
                crate::execution::PiRuntime::new(&dir.path().join("ledger.sqlite"), &node).unwrap(),
            ));
            assert_inert(pi.recovery_profile());
        }
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    #[test]
    fn confined_pi_recovery_profile_declares_stop_settlement() {
        let tmp = tempfile::tempdir().unwrap();
        let path = || std::path::PathBuf::from("/uninstalled-conformance-fixture");
        let runtime = crate::execution::confined_pi::fixture_runtime(tmp.path(), "inert");
        let config = crate::execution::confined_pi::ConfinedPiConfig {
            root: path(),
            cgroup_root: path(),
            runtime,
            node: path(),
            bubblewrap: path(),
            launcher: path(),
            policy: path(),
            model_endpoint: None,
        };
        let confined = RuntimeAdapter::Confined(Box::new(
            crate::execution::confined_runtime::ConfinedRuntime::new(config, "answer").unwrap(),
        ));
        let profile = confined.recovery_profile();
        assert!(profile.open_session_before_reconcile);
        assert!(profile.settle_stops_before_reconcile);
        assert!(profile.resume_after_delivered_stop);
        assert!(profile.skip_verify_for_supervised_writers);
    }
}
