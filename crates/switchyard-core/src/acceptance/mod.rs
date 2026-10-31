// Contract-focused acceptance tests. Shared helpers remain test-only.
use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::json;
use tempfile::TempDir;

use crate::delivery::{publication_operation_id, publication_payload, publication_payload_hash};
use crate::domain::AttentionReason;
use crate::execution::{FakeRuntime, RuntimeNotification, RuntimeRequest, operation_id};
use crate::policy::plan_digest_for;
use crate::{
    AdmitRequest, CommandBody, CommandOrigin, CommandState, EffectProof, Error, Host,
    OperationState, Phase, PublicationProof,
};

mod fixtures;
use fixtures::*;

mod admission_identity;
mod build_contract;
mod ownership;
mod pause_cancel;
mod publication;
mod recovery_outbox;
mod recovery_regressions;
mod runtime_authority;
mod verification_approval;
