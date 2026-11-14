//! Internal dispatch seam. Mission policy and the outbox stay in Host/Store.
use serde_json::{Value, json};

#[cfg(feature = "pi-contract")]
use super::PiRuntime;
use super::{FakeRuntime, RuntimeNotification, RuntimeReceipt, RuntimeRequest};
use crate::domain::{CommandBody, CommandKind, CommandOrigin};
use crate::error::Result;
use crate::execution_env::ExecutionEnv;
use crate::{ExecutionReceipt, ExecutionRequest};

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

impl GuidanceProfile {
    const FAKE: Self = Self {
        operator_resolution_hint: true,
    };
}

/// What an adapter may certify, and which command check it declared.
/// [`RuntimeAdapter::authority`] matches every variant, so a forgotten
/// variant fails to compile. This struct has no `Default`, which blocks
/// `..Default::default()` as a permissive fallback. [`GuidanceProfile`] and
/// [`ExecutionProfile`] still derive `Default` because their zero value is
/// the inert profile those variants already use.
///
/// `certifies_mission_facts` is an authority gate only. It does not check
/// plan shape, digest, or freeze. Those stay in Host admission. S4-A (#172)
/// should read this flag from [`RuntimeAdapter::validate_admission`] instead
/// of adding a runtime-identity check. Plan approval itself stays a Host
/// operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct AuthorityProfile {
    pub(crate) certifies_mission_facts: bool,
    pub(crate) command_check: CommandCheck,
}

/// How [`RuntimeAdapter::validate_command`] checks a runtime-bound command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CommandCheck {
    /// Fake: the Host already shaped the command. No protocol bounds check.
    Simulated,
    /// Pi and Confined share the S2 frame, hash, and binding bounds.
    #[cfg(feature = "pi-contract")]
    PiProtocol,
}

/// In-process simulated [`ExecutionEnv`](crate::execution_env::ExecutionEnv).
/// Only Fake applies. Pi and Confined refuse with the same guard an empty
/// `fake_mut()` used to produce. This does not run live effects.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct ExecutionProfile {
    pub(crate) simulated: bool,
}

impl RuntimeAdapter {
    pub(crate) const FAKE_ADAPTER_NAME: &'static str = "fake";
    #[cfg(feature = "pi-contract")]
    const PI_PROBE_NAME: &'static str = "pi-probe-v1";
    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    const CONFINED_ADAPTER_NAME: &'static str = "pi-confined-v1";

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
            Self::Fake(_) => GuidanceProfile::FAKE,
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

    /// Names [`Self::name`] publishes, each with the [`Self::guidance`] value
    /// for that variant. A new variant has to be added here and in `name` /
    /// `guidance`; [`tests::assert_declared`] checks they agree. Unknown
    /// recorded names stay inert.
    fn recorded_guidance() -> Vec<(&'static str, GuidanceProfile)> {
        vec![
            (Self::FAKE_ADAPTER_NAME, GuidanceProfile::FAKE),
            #[cfg(feature = "pi-contract")]
            (Self::PI_PROBE_NAME, GuidanceProfile::default()),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            (Self::CONFINED_ADAPTER_NAME, GuidanceProfile::default()),
        ]
    }

    /// Recorded `attempts.runtime_adapter` name. Inspection does not open a
    /// runtime, so status text resolves through the declared guidance table
    /// instead of a live variant.
    pub(crate) fn guidance_for_recorded_name(recorded: &str) -> GuidanceProfile {
        Self::recorded_guidance()
            .into_iter()
            .find(|(name, _)| *name == recorded)
            .map(|(_, profile)| profile)
            .unwrap_or_default()
    }

    pub(crate) fn authority(&self) -> AuthorityProfile {
        match self {
            Self::Fake(_) => AuthorityProfile {
                certifies_mission_facts: true,
                command_check: CommandCheck::Simulated,
            },
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => AuthorityProfile {
                certifies_mission_facts: false,
                command_check: CommandCheck::PiProtocol,
            },
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => AuthorityProfile {
                certifies_mission_facts: false,
                command_check: CommandCheck::PiProtocol,
            },
        }
    }

    pub(crate) fn execution_profile(&self) -> ExecutionProfile {
        match self {
            Self::Fake(_) => ExecutionProfile { simulated: true },
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => ExecutionProfile::default(),
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => ExecutionProfile::default(),
        }
    }

    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Fake(_) => Self::FAKE_ADAPTER_NAME,
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => Self::PI_PROBE_NAME,
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => Self::CONFINED_ADAPTER_NAME,
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
        // Authority gate only. A certifying adapter, Fake today, passes every
        // body and origin, including PlanCompleted. Plan shape, digest, and
        // freeze checks stay in Host admission (#172).
        if self.authority().certifies_mission_facts {
            return Ok(());
        }
        // S2-A has no trusted planning/verification/process-quiescence observer.
        // Do not expose S1 simulation inputs as facts about a child process.
        // HostSupervisor is not a Pi observation; the private host-fact path
        // never reaches this check. Planning admission (#172) should keep
        // reading `certifies_mission_facts` instead of naming a variant.
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
        Ok(())
    }

    pub(crate) fn validate_command(&self, request: &RuntimeRequest, payload: &Value) -> Result<()> {
        match self.authority().command_check {
            CommandCheck::Simulated => {
                let _ = (request, payload);
                Ok(())
            }
            #[cfg(feature = "pi-contract")]
            CommandCheck::PiProtocol => PiRuntime::validate_command(request, payload),
        }
    }

    /// Next queued observation. `Ok(None)` means this adapter's queue is empty.
    pub(crate) fn pop_observation(&mut self) -> Result<Option<RuntimeNotification>> {
        match self {
            Self::Fake(runtime) => Ok(runtime.pop_notification()),
            // The Pi probe has no queue. A non-host-owned poll used to see
            // `fake_mut()` as empty and drain nothing, so this stays `Ok(None)`.
            // Confined does not: it returns the host-owned guard. That stricter
            // result is intentional. `DispatchProfile.host_owned` routes Confined
            // before this method, and a direct call must not look like Pi's
            // empty drain.
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => Ok(None),
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

    /// Put an observation back at the front of the queue. This is the typed
    /// path for a notification that was popped and then could not be applied.
    pub(crate) fn requeue_observation(&mut self, note: RuntimeNotification) -> Result<()> {
        match self {
            Self::Fake(runtime) => {
                runtime.requeue_front(note);
                Ok(())
            }
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => {
                let _ = note;
                Err(crate::Error::guard(
                    "Pi probe has no observation queue to restore",
                ))
            }
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => {
                let _ = note;
                Err(crate::Error::guard(
                    "confined Pi dispatch must be owned by Host",
                ))
            }
        }
    }

    /// Simulated effect apply. Pi and Confined return the refusal the Host
    /// used to get from `fake_mut()` and do not execute.
    pub(crate) fn apply_simulated(
        &mut self,
        request: &ExecutionRequest,
    ) -> Result<ExecutionReceipt> {
        match self {
            Self::Fake(runtime) => ExecutionEnv::apply(runtime, request),
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => {
                let _ = request;
                Err(crate::Error::guard(
                    "simulated execution requires FakeRuntime",
                ))
            }
            #[cfg(all(
                feature = "pi-contract",
                feature = "linux-execution",
                target_os = "linux"
            ))]
            Self::Confined(_) => {
                let _ = request;
                Err(crate::Error::guard(
                    "simulated execution requires FakeRuntime",
                ))
            }
        }
    }

    /// Wire method for one command kind. Exhaustive over [`CommandKind`]: a
    /// new kind does not compile until it names abort, steer, or submit.
    /// Kinds that are not runtime-bound still map to submit, which is what
    /// the previous `_` arm returned when a caller asked.
    pub(crate) fn runtime_method(kind: CommandKind) -> &'static str {
        match kind {
            CommandKind::Pause | CommandKind::Cancel => "abort",
            CommandKind::Steer => "steer",
            CommandKind::StartPlanning
            | CommandKind::PlanCompleted
            | CommandKind::RequestRevisedPlan
            | CommandKind::ApprovePlan
            | CommandKind::RequestVerification
            | CommandKind::CompletePass
            | CommandKind::AuthorizeBoundedRepair
            | CommandKind::RequestChanges
            | CommandKind::ApproveReview
            | CommandKind::Publish
            | CommandKind::ReconcileDeliveryComplete
            | CommandKind::Reject
            | CommandKind::ConfirmStopped
            | CommandKind::Resume
            | CommandKind::ConfirmCancelled
            | CommandKind::DiscoverPublicationCompleted
            | CommandKind::DetectUncertainty
            | CommandKind::ExplicitRecovery
            | CommandKind::Continue => "submit",
        }
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
        assert!(fake.authority().certifies_mission_facts);
        assert_eq!(fake.authority().command_check, CommandCheck::Simulated);
        assert!(fake.execution_profile().simulated);
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
            assert!(!pi.authority().certifies_mission_facts);
            assert_eq!(pi.authority().command_check, CommandCheck::PiProtocol);
            assert!(!pi.execution_profile().simulated);
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
        assert!(!confined.authority().certifies_mission_facts);
        assert_eq!(confined.authority().command_check, CommandCheck::PiProtocol);
        assert!(!confined.execution_profile().simulated);
        assert_declared(&confined);
    }

    fn bare_request() -> RuntimeRequest {
        RuntimeRequest {
            binding: crate::RuntimeBinding {
                mission_id: "m".into(),
                attempt_id: "a".into(),
                session_id: "s".into(),
                generation: 1,
            },
            request_id: "a:c".into(),
            payload_hash: "not-a-real-hash".into(),
            method: "submit",
        }
    }

    fn simulated_write() -> crate::ExecutionRequest {
        crate::ExecutionRequest {
            binding: crate::RuntimeBinding {
                mission_id: "m".into(),
                attempt_id: "a".into(),
                session_id: "s".into(),
                generation: 1,
            },
            tool_task_id: "task".into(),
            effect_key: "write".into(),
            capability_scope: crate::FAKE_WRITE_SCOPE.into(),
            deadline_ms: i64::MAX,
            operation: crate::ExecutionOperation::WriteFile {
                path: "notes.txt".into(),
                content: "hello".into(),
            },
        }
    }

    #[cfg(feature = "pi-contract")]
    fn authority_error(
        adapter: &RuntimeAdapter,
        body: &CommandBody,
        origin: CommandOrigin,
    ) -> String {
        adapter
            .validate_admission(body, origin)
            .expect_err("adapter does not certify this admission")
            .to_string()
    }

    #[test]
    fn admission_and_command_checks_follow_the_authority_profile() {
        let dir = tempfile::tempdir().unwrap();
        let fake =
            RuntimeAdapter::Fake(FakeRuntime::open(&dir.path().join("fake-runtime.json")).unwrap());
        let plan = CommandBody::PlanCompleted {
            plan: "do the work".into(),
            digest: "sha256:abc".into(),
            quiescent: true,
        };
        fake.validate_admission(&CommandBody::ConfirmStopped, CommandOrigin::Operator)
            .unwrap();
        fake.validate_admission(&plan, CommandOrigin::RuntimeObservation)
            .unwrap();
        fake.validate_admission(&CommandBody::StartPlanning, CommandOrigin::Operator)
            .unwrap();
        fake.validate_command(&bare_request(), &json!({})).unwrap();

        #[cfg(feature = "pi-contract")]
        {
            let node = std::env::current_exe().unwrap();
            let pi = RuntimeAdapter::Pi(Box::new(
                crate::execution::PiRuntime::new(&dir.path().join("ledger.sqlite"), &node).unwrap(),
            ));
            let guard = "no mission-fact or execution-quiescence authority";
            assert!(
                authority_error(&pi, &CommandBody::ConfirmStopped, CommandOrigin::Operator)
                    .contains(guard)
            );
            assert!(authority_error(&pi, &plan, CommandOrigin::RuntimeObservation).contains(guard));
            assert!(
                authority_error(
                    &pi,
                    &CommandBody::StartPlanning,
                    CommandOrigin::HostSupervisor
                )
                .contains(guard)
            );
            pi.validate_admission(&CommandBody::StartPlanning, CommandOrigin::Operator)
                .unwrap();
            assert!(pi.validate_command(&bare_request(), &json!({})).is_err());
        }
    }

    #[test]
    fn observation_intake_and_simulated_apply_go_through_the_adapter() {
        let dir = tempfile::tempdir().unwrap();
        let mut runtime = FakeRuntime::open(&dir.path().join("fake-runtime.json")).unwrap();
        runtime.push_notification(RuntimeNotification::Idle);
        let mut fake = RuntimeAdapter::Fake(runtime);
        let popped = fake.pop_observation().unwrap().unwrap();
        assert_eq!(popped, RuntimeNotification::Idle);
        fake.requeue_observation(popped).unwrap();
        assert_eq!(
            fake.pop_observation().unwrap(),
            Some(RuntimeNotification::Idle)
        );
        assert!(fake.pop_observation().unwrap().is_none());
        let receipt = fake.apply_simulated(&simulated_write()).unwrap();
        assert!(receipt.simulated);
        assert_eq!(receipt.bytes_written, "hello".len());
        assert!(!receipt.content_hash.is_empty());

        #[cfg(feature = "pi-contract")]
        {
            let node = std::env::current_exe().unwrap();
            let mut pi = RuntimeAdapter::Pi(Box::new(
                crate::execution::PiRuntime::new(&dir.path().join("ledger.sqlite"), &node).unwrap(),
            ));
            assert!(pi.pop_observation().unwrap().is_none());
            let requeue = pi
                .requeue_observation(RuntimeNotification::Idle)
                .unwrap_err();
            assert!(
                requeue
                    .to_string()
                    .contains("Pi probe has no observation queue to restore"),
                "{requeue}"
            );
            let refused = pi.apply_simulated(&simulated_write()).unwrap_err();
            assert!(
                refused
                    .to_string()
                    .contains("simulated execution requires FakeRuntime"),
                "{refused}"
            );
        }
    }

    #[cfg(all(
        feature = "pi-contract",
        feature = "linux-execution",
        target_os = "linux"
    ))]
    #[test]
    fn confined_pi_refuses_simulated_execution_and_the_observation_queue() {
        let tmp = tempfile::tempdir().unwrap();
        let path = || std::path::PathBuf::from("/uninstalled-conformance-fixture");
        let runtime = crate::execution::confined_pi::fixture_runtime(tmp.path(), "contract");
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
        let mut confined = RuntimeAdapter::Confined(Box::new(
            crate::execution::confined_runtime::ConfinedRuntime::new(config, "answer").unwrap(),
        ));
        let guard = "no mission-fact or execution-quiescence authority";
        assert!(
            authority_error(
                &confined,
                &CommandBody::ConfirmStopped,
                CommandOrigin::Operator
            )
            .contains(guard)
        );
        confined
            .validate_admission(&CommandBody::StartPlanning, CommandOrigin::Operator)
            .unwrap();
        assert!(
            confined
                .validate_command(&bare_request(), &json!({}))
                .is_err()
        );
        let popped = confined.pop_observation().unwrap_err();
        assert!(
            popped
                .to_string()
                .contains("confined Pi dispatch must be owned by Host"),
            "{popped}"
        );
        let requeue = confined
            .requeue_observation(RuntimeNotification::Idle)
            .unwrap_err();
        assert!(
            requeue
                .to_string()
                .contains("confined Pi dispatch must be owned by Host"),
            "{requeue}"
        );
        let refused = confined.apply_simulated(&simulated_write()).unwrap_err();
        assert!(
            refused
                .to_string()
                .contains("simulated execution requires FakeRuntime"),
            "{refused}"
        );
    }

    #[test]
    fn runtime_method_names_every_command_kind() {
        for kind in CommandKind::all() {
            let method = RuntimeAdapter::runtime_method(kind);
            match kind {
                CommandKind::Pause | CommandKind::Cancel => assert_eq!(method, "abort"),
                CommandKind::Steer => assert_eq!(method, "steer"),
                _ => assert_eq!(method, "submit", "{kind}"),
            }
        }
    }
}
