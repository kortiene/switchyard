use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// Identity evidence only: `ready` is not confinement or process quiescence.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceView {
    pub id: String,
    pub mission_id: String,
    pub attempt_id: String,
    pub path: String,
    pub base_sha: String,
    pub state: String,
    pub created_generation: u64,
    pub verified_generation: Option<u64>,
    pub detail: Option<String>,
}

impl WorkspaceView {
    pub fn to_json(&self) -> Value {
        json!({
            "id": self.id, "missionId": self.mission_id, "attemptId": self.attempt_id,
            "path": self.path, "baseSha": self.base_sha, "state": self.state,
            "createdGeneration": self.created_generation,
            "verifiedGeneration": self.verified_generation, "detail": self.detail,
        })
    }
}

#[derive(Clone, Debug)]
pub(crate) struct WorkspaceRecord {
    pub view: WorkspaceView,
    pub ledger_path: String,
    pub root: String,
    pub root_identity: String,
    pub source: String,
    pub common_dir: String,
    pub common_identity: String,
    pub slot_identity: Option<String>,
    pub lease_identity: Option<String>,
    pub tree_identity: Option<String>,
    pub git_dir: Option<String>,
}

impl WorkspaceRecord {
    pub fn slot(&self) -> PathBuf {
        Path::new(&self.root).join(&self.view.attempt_id)
    }

    pub fn reason(&self) -> String {
        format!("switchyard:{}", self.view.id)
    }

    pub fn marker(&self) -> Value {
        json!({
            "version": 1, "workspaceId": self.view.id,
            "missionId": self.view.mission_id, "attemptId": self.view.attempt_id,
            "path": self.view.path, "baseSha": self.view.base_sha,
            "ledgerPath": self.ledger_path,
            "root": self.root, "rootIdentity": self.root_identity,
            "source": self.source, "commonDir": self.common_dir,
            "commonIdentity": self.common_identity,
        })
    }
}
