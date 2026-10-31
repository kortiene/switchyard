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
}

impl RuntimeAdapter {
    pub(crate) fn name(&self) -> &'static str {
        match self {
            Self::Fake(_) => "fake",
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => "pi-probe-v1",
        }
    }

    pub(crate) fn configuration(&self) -> Value {
        match self {
            Self::Fake(_) => json!({"adapter":"fake", "credentials":"none"}),
            #[cfg(feature = "pi-contract")]
            Self::Pi(runtime) => runtime.configuration(),
        }
    }

    pub(crate) fn fake_mut(&mut self) -> Option<&mut FakeRuntime> {
        match self {
            Self::Fake(runtime) => Some(runtime),
            #[cfg(feature = "pi-contract")]
            Self::Pi(_) => None,
        }
    }

    pub(crate) fn validate_admission(
        &self,
        body: &CommandBody,
        origin: CommandOrigin,
    ) -> Result<()> {
        #[cfg(feature = "pi-contract")]
        if let Self::Pi(_) = self {
            // S2-A has no trusted planning/verification/process-quiescence observer.
            // Do not expose S1 simulation inputs as facts about a child process.
            if origin == CommandOrigin::RuntimeObservation
                || matches!(
                    body,
                    CommandBody::ConfirmStopped
                        | CommandBody::ConfirmCancelled
                        | CommandBody::RequestVerification { .. }
                        | CommandBody::ExplicitRecovery { .. }
                )
            {
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
        if let Self::Pi(_) = self {
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
        }
    }
}
