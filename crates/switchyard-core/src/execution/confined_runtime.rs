//! Host-owned, credential-free transport. Sessions never escape into callers.
use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Value, json};

use super::confined_pi::{ConfinedPiConfig, ConfinedPiSession};
use crate::execution::RuntimeBinding;
use crate::{Error, Result};

pub(crate) struct ConfinedRuntime {
    pub(crate) config: ConfinedPiConfig,
    pub(crate) fixture: String,
    pub(crate) sessions: BTreeMap<String, ConfinedPiSession>,
    /// Session ids proven stopped and removed in this owner generation.
    /// A later call must not launch a replacement at the same generation.
    released: BTreeSet<String>,
}

impl ConfinedRuntime {
    pub(crate) fn new(config: ConfinedPiConfig, fixture: &str) -> Result<Self> {
        if !matches!(
            fixture,
            "answer"
                | "file-write"
                | "process-fast"
                | "process-hold"
                | "process-cancel"
                | "tools"
                | "s3-acceptance"
                | "ignore-abort"
        ) {
            return Err(Error::guard("unsupported credential-free Host fixture"));
        }
        Ok(Self {
            config,
            fixture: fixture.into(),
            sessions: BTreeMap::new(),
            released: BTreeSet::new(),
        })
    }

    pub(crate) fn open_bindings(&self) -> Vec<RuntimeBinding> {
        self.sessions
            .values()
            .map(|session| session.binding().clone())
            .collect()
    }

    pub(crate) fn is_released(&self, session_id: &str) -> bool {
        self.released.contains(session_id)
    }

    pub(crate) fn mark_released(&mut self, session_id: &str) {
        self.released.insert(session_id.to_string());
    }

    pub(crate) fn configuration(&self) -> Value {
        json!({"adapter":"pi-confined-v1", "credentials":"none",
            "network":super::confined_pi::egress_network(&self.config),
            "fixture":self.fixture, "root":self.config.root, "cgroupRoot":self.config.cgroup_root,
            "node":self.config.node, "bubblewrap":self.config.bubblewrap,
            "launcher":self.config.launcher, "policy":self.config.policy,
            "fingerprint":super::pi_bridge::adapter_fingerprint()})
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn confined_pi_host_acceptance_fixture_is_explicit_inert_and_credential_free() {
        let path = || PathBuf::from("/uninstalled-conformance-fixture");
        let config = ConfinedPiConfig {
            root: path(),
            cgroup_root: path(),
            node: path(),
            bubblewrap: path(),
            launcher: path(),
            policy: path(),
            model_endpoint: None,
        };
        let runtime = ConfinedRuntime::new(config.clone(), "s3-acceptance").unwrap();
        assert!(runtime.sessions.is_empty());
        let saved = runtime.configuration();
        assert_eq!(saved["fixture"], "s3-acceptance");
        assert_eq!(saved["credentials"], "none");
        assert_eq!(saved["network"], "denied");
        for forbidden in ["live", "https://model.example/v1", "../repo/extension"] {
            assert!(ConfinedRuntime::new(config.clone(), forbidden).is_err());
        }
    }
}
