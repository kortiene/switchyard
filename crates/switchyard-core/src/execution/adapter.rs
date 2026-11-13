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
///
/// Two dependencies hold for every variant. Skipping verify for a supervised
/// writer implies opening the session first: the host can only know it is
/// supervising that writer after the session is open.
/// Resuming after a delivered stop implies settling stops before reconcile:
/// that resume is the fall-through after the settlement clears the Linux block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)]
pub(crate) struct RecoveryProfile {
    pub(crate) open_session_before_reconcile: bool,
    pub(crate) settle_stops_before_reconcile: bool,
    pub(crate) resume_after_delivered_stop: bool,
    pub(crate) skip_verify_for_supervised_writers: bool,
}

/// Who delivers commands and observations. Fake and the Pi probe use
/// `RuntimeAdapter::dispatch`. Only the confined adapter routes stops, work,
/// and observation polling through the Host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DispatchProfile {
    pub(crate) host_owned: bool,
}

/// How uncertainty becomes a mission fact. Fake and the Pi probe admit it as
/// a runtime observation. The confined adapter records it as a supervisor fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct FactProfile {
    pub(crate) uncertainty_is_host_fact: bool,
}

/// Linux execution ledger. The Pi probe admits no Linux effects. Fake may
/// admit them and keeps an admitted stop. Only the confined adapter supervises
/// live effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct LinuxLedgerProfile {
    pub(crate) admits_effects: bool,
    pub(crate) preserves_admitted_stop: bool,
    pub(crate) supervises_live_effects: bool,
}

/// Operator status text. Only Fake names the `detect_uncertainty` command.
/// Read-only inspection has no live adapter, so it uses the recorded name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct GuidanceProfile {
    pub(crate) operator_resolution_hint: bool,
}

impl RuntimeAdapter {
    pub(crate) const FAKE_ADAPTER_NAME: &'static str = "fake";

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

    pub(crate) fn dispatch_profile(&self) -> DispatchProfile {
        match self {
            Self::Fake(_) => DispatchProfile::default(),
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => DispatchProfile::default(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => DispatchProfile { host_owned: true },
        }
    }

    pub(crate) fn fact_profile(&self) -> FactProfile {
        match self {
            Self::Fake(_) => FactProfile::default(),
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => FactProfile::default(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => FactProfile {
                uncertainty_is_host_fact: true,
            },
        }
    }

    pub(crate) fn linux_ledger(&self) -> LinuxLedgerProfile {
        match self {
            Self::Fake(_) => LinuxLedgerProfile {
                admits_effects: true,
                preserves_admitted_stop: true,
                supervises_live_effects: false,
            },
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => LinuxLedgerProfile::default(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => LinuxLedgerProfile {
                admits_effects: true,
                preserves_admitted_stop: true,
                supervises_live_effects: true,
            },
        }
    }

    pub(crate) fn guidance(&self) -> GuidanceProfile {
        match self {
            Self::Fake(_) => GuidanceProfile {
                operator_resolution_hint: true,
            },
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => GuidanceProfile::default(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => GuidanceProfile::default(),
        }
    }

    /// Recorded `attempts.runtime_config_json` adapter name. Inspection does
    /// not open a runtime, so status text uses this instead of a live variant.
    pub(crate) fn guidance_for_recorded_name(recorded: &str) -> GuidanceProfile {
        GuidanceProfile {
            operator_resolution_hint: recorded == Self::FAKE_ADAPTER_NAME,
        }
    }

    #[cfg(feature = "pi-contract")]
    fn is_fake(&self) -> bool {
        matches!(self, Self::Fake(_))
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Fake(_) => Self::FAKE_ADAPTER_NAME,
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
            Self::Fake(_) => json!({"adapter": Self::FAKE_ADAPTER_NAME, "credentials":"none"}),
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

    fn assert_flag_dependencies(profile: RecoveryProfile) {
        assert!(
            !profile.skip_verify_for_supervised_writers || profile.open_session_before_reconcile,
            "{profile:?}"
        );
        assert!(
            !profile.resume_after_delivered_stop || profile.settle_stops_before_reconcile,
            "{profile:?}"
        );
    }

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
        assert_flag_dependencies(fake.recovery_profile());
        #[cfg(feature = "pi-contract")]
        {
            let node = std::env::current_exe().unwrap();
            let pi = RuntimeAdapter::Pi(Box::new(
                crate::execution::PiRuntime::new(&dir.path().join("ledger.sqlite"), &node).unwrap(),
            ));
            assert_inert(pi.recovery_profile());
            assert_flag_dependencies(pi.recovery_profile());
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
        assert_flag_dependencies(profile);
    }

    /// Every `RuntimeAdapter` variant that this build can construct. A new
    /// variant has to be added here so its profile is checked.
    #[test]
    fn recovery_profile_variants_keep_flag_dependencies() {
        let dir = tempfile::tempdir().unwrap();
        assert_flag_dependencies(
            RuntimeAdapter::Fake(FakeRuntime::open(&dir.path().join("fake-runtime.json")).unwrap())
                .recovery_profile(),
        );
        #[cfg(feature = "pi-contract")]
        {
            let node = std::env::current_exe().unwrap();
            assert_flag_dependencies(
                RuntimeAdapter::Pi(Box::new(
                    crate::execution::PiRuntime::new(&dir.path().join("ledger.sqlite"), &node)
                        .unwrap(),
                ))
                .recovery_profile(),
            );
        }
        #[cfg(all(
            feature = "pi-contract",
            feature = "linux-execution",
            target_os = "linux"
        ))]
        {
            let path = || std::path::PathBuf::from("/uninstalled-conformance-fixture");
            let runtime = crate::execution::confined_pi::fixture_runtime(dir.path(), "deps");
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
            assert_flag_dependencies(
                RuntimeAdapter::Confined(Box::new(
                    crate::execution::confined_runtime::ConfinedRuntime::new(config, "answer")
                        .unwrap(),
                ))
                .recovery_profile(),
            );
        }
    }

    fn assert_declared(adapter: &RuntimeAdapter) {
        let dispatch = adapter.dispatch_profile();
        let facts = adapter.fact_profile();
        let linux = adapter.linux_ledger();
        let guidance = adapter.guidance();
        assert!(
            !dispatch.host_owned || facts.uncertainty_is_host_fact,
            "host-owned dispatch records uncertainty as a supervisor fact: {dispatch:?} {facts:?}"
        );
        assert!(
            !linux.supervises_live_effects || linux.admits_effects,
            "live supervision admits the effects it settles: {linux:?}"
        );
        assert_eq!(
            guidance,
            RuntimeAdapter::guidance_for_recorded_name(adapter.name()),
            "{}",
            adapter.name()
        );
    }

    #[test]
    fn declared_profiles_for_fake_and_pi_probe() {
        let dir = tempfile::tempdir().unwrap();
        let fake =
            RuntimeAdapter::Fake(FakeRuntime::open(&dir.path().join("fake-runtime.json")).unwrap());
        assert_eq!(fake.dispatch_profile(), DispatchProfile::default());
        assert_eq!(fake.fact_profile(), FactProfile::default());
        assert_eq!(
            fake.linux_ledger(),
            LinuxLedgerProfile {
                admits_effects: true,
                preserves_admitted_stop: true,
                supervises_live_effects: false,
            }
        );
        assert!(fake.guidance().operator_resolution_hint);
        assert!(
            RuntimeAdapter::guidance_for_recorded_name(RuntimeAdapter::FAKE_ADAPTER_NAME)
                .operator_resolution_hint
        );
        assert!(
            !RuntimeAdapter::guidance_for_recorded_name("pi-probe-v1").operator_resolution_hint
        );
        assert!(
            !RuntimeAdapter::guidance_for_recorded_name("pi-confined-v1").operator_resolution_hint
        );
        assert!(!RuntimeAdapter::guidance_for_recorded_name("other").operator_resolution_hint);
        assert_declared(&fake);
        #[cfg(feature = "pi-contract")]
        {
            let node = std::env::current_exe().unwrap();
            let pi = RuntimeAdapter::Pi(Box::new(
                crate::execution::PiRuntime::new(&dir.path().join("ledger.sqlite"), &node).unwrap(),
            ));
            assert_eq!(pi.dispatch_profile(), DispatchProfile::default());
            assert_eq!(pi.fact_profile(), FactProfile::default());
            assert_eq!(pi.linux_ledger(), LinuxLedgerProfile::default());
            assert_eq!(pi.guidance(), GuidanceProfile::default());
            assert_declared(&pi);
        }
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    #[test]
    fn confined_pi_declared_profiles_are_host_owned() {
        let tmp = tempfile::tempdir().unwrap();
        let path = || std::path::PathBuf::from("/uninstalled-conformance-fixture");
        let runtime = crate::execution::confined_pi::fixture_runtime(tmp.path(), "profiles");
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
        assert!(confined.dispatch_profile().host_owned);
        assert!(confined.fact_profile().uncertainty_is_host_fact);
        let linux = confined.linux_ledger();
        assert!(linux.admits_effects);
        assert!(linux.preserves_admitted_stop);
        assert!(linux.supervises_live_effects);
        assert_eq!(confined.guidance(), GuidanceProfile::default());
        assert_declared(&confined);
    }
}
