// Contract-focused acceptance tests. Dependencies are explicit in each module.

mod admission_identity;
mod build_contract;
mod fixtures;
mod host_verifier;
#[cfg(all(feature = "linux-execution", target_os = "linux"))]
mod host_verifier_confined;
mod ownership;
mod pause_cancel;
mod planning_approval;
mod planning_freeze;
mod publication;
mod recovery_outbox;
mod recovery_regressions;
mod runtime_authority;
mod successor;
mod verification_approval;
