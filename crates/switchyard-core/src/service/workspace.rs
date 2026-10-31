use super::*;
use crate::execution::workspace as fs_workspace;
use crate::workspace::{WorkspaceRecord, WorkspaceView};

impl Host {
    /// Explicit S3-A1 preparation. `root` must already exist at its canonical
    /// absolute path. The host chooses `<root>/<attempt>/tree`; callers cannot
    /// adopt an existing directory or choose another attempt's workspace.
    pub fn prepare_workspace(
        &mut self,
        mission_id: &str,
        attempt_id: &str,
        root: &Path,
    ) -> Result<WorkspaceView> {
        self.prepare_workspace_at(mission_id, attempt_id, root, |_| Ok(()))
    }

    fn prepare_workspace_at(
        &mut self,
        mission_id: &str,
        attempt_id: &str,
        root: &Path,
        mut checkpoint: impl FnMut(&str) -> Result<()>,
    ) -> Result<WorkspaceView> {
        // One filter-discovery cache for this prepare/verify. Dropping it
        // forces the next call to see drivers added in between.
        let _filters = fs_workspace::FilterCacheScope::enter();
        self.store.assert_workspace_owner()?;
        let mission = self.store.load_mission(mission_id)?;
        if mission.attempt.id != attempt_id {
            return Err(Error::BindingMismatch {
                detail: "workspace attempt does not belong to this mission".into(),
            });
        }
        let existing = self.store.load_workspace(attempt_id)?;
        let root = match fs_workspace::canonical_directory(root) {
            Ok(root) => root,
            Err(error) => {
                let Some(workspace) = existing else {
                    return Err(error);
                };
                if same_workspace_root(&workspace.root, root) {
                    return self.block_workspace(&workspace, error);
                }
                return Err(Error::guard(
                    "workspace identity/path conflicts with its reservation",
                ));
            }
        };
        let mut workspace = match existing {
            Some(workspace) => {
                if Path::new(&workspace.root) != root.as_path()
                    || workspace.view.mission_id != mission_id
                    || workspace.view.base_sha != mission.attempt.base_sha
                    || Path::new(&workspace.ledger_path) != self.ledger_path
                {
                    return Err(Error::guard(
                        "workspace identity/path conflicts with its reservation",
                    ));
                }
                workspace
            }
            None => {
                if mission.phase.is_terminal() {
                    return Err(Error::TerminalImmutable {
                        phase: mission.phase.as_str().into(),
                    });
                }
                let workspace =
                    fs_workspace::reserve(&mission, &root, &self.ledger_path, self.generation())?;
                self.store.reserve_workspace(&workspace)?;
                workspace
            }
        };
        if workspace.view.state == "uncertain" {
            return Err(Error::WorkspaceUncertain {
                workspace_id: workspace.view.id,
                detail: workspace
                    .view
                    .detail
                    .unwrap_or_else(|| "operator reconciliation required".into()),
            });
        }
        // Test barriers return outside error settlement to model abrupt loss of
        // the host. They are private, and the production path supplies no hook.
        checkpoint("reserved")?;
        let lease = if workspace.view.state == "reserved" {
            if mission.phase.is_terminal() {
                return Err(Error::guard("cannot create a terminal attempt's workspace"));
            }
            let (lease, slot, file) = match fs_workspace::initialize_slot(&workspace) {
                Ok(value) => value,
                Err(error) => return self.block_workspace(&workspace, error),
            };
            checkpoint("slot")?;
            self.store
                .start_workspace(&workspace.view.id, &slot, &file)?;
            workspace.slot_identity = Some(slot);
            workspace.lease_identity = Some(file);
            workspace.view.state = "creating".into();
            checkpoint("before_git")?;
            self.store.assert_workspace_owner()?;
            if let Err(error) = fs_workspace::create(&workspace, &lease) {
                return self.settle_create_failure(&workspace, error);
            }
            checkpoint("after_git")?;
            lease
        } else {
            let lease = match fs_workspace::acquire(&workspace) {
                Ok(Some(lease)) => lease,
                Ok(None) => {
                    return Err(Error::WorkspaceBusy {
                        workspace_id: workspace.view.id,
                    });
                }
                Err(error) => return self.block_workspace(&workspace, error),
            };
            // A clean `git worktree add` failure stays in `creating` with a
            // retry note. Absence without that note is still the crash gap.
            if workspace.view.state == "creating" && workspace.view.detail.is_some() {
                match fs_workspace::creation_left_evidence(&workspace) {
                    Ok(false) => {
                        if let Err(error) = fs_workspace::create(&workspace, &lease) {
                            return self.settle_create_failure(&workspace, error);
                        }
                    }
                    Ok(true) => {}
                    Err(error) if error.is_transient() => return Err(error),
                    Err(error) => return self.block_workspace(&workspace, error),
                }
            }
            lease
        };
        let (tree, git_dir) = match fs_workspace::verify(&workspace) {
            Ok(value) => value,
            Err(error) => return self.block_workspace(&workspace, error),
        };
        self.store
            .ready_workspace(&workspace.view.id, &tree, &git_dir)?;
        drop(lease);
        Ok(self
            .store
            .load_workspace(attempt_id)?
            .ok_or(Error::NotFound)?
            .view)
    }

    /// Reconcile a recorded workspace only. A mission with no reservation does
    /// not gain a workspace as a side effect of ordinary Host recovery.
    pub fn recover_workspace(&mut self, mission_id: &str) -> Result<Option<WorkspaceView>> {
        self.store.assert_workspace_owner()?;
        let mission = self.store.load_mission(mission_id)?;
        let Some(workspace) = self.store.load_workspace(&mission.attempt.id)? else {
            return Ok(None);
        };
        self.prepare_workspace(mission_id, &mission.attempt.id, Path::new(&workspace.root))
            .map(Some)
    }

    fn settle_create_failure<T>(&mut self, workspace: &WorkspaceRecord, error: Error) -> Result<T> {
        if error.is_transient() {
            match fs_workspace::creation_left_evidence(workspace) {
                Ok(false) => {
                    self.store
                        .record_workspace_retry(&workspace.view.id, &error.to_string())?;
                    return Err(error);
                }
                Ok(true) => return self.quarantine_workspace(workspace, error),
                Err(probe) if probe.is_transient() => return Err(error),
                Err(probe) => return self.quarantine_workspace(workspace, probe),
            }
        }
        self.quarantine_workspace(workspace, error)
    }

    fn block_workspace<T>(&mut self, workspace: &WorkspaceRecord, error: Error) -> Result<T> {
        if error.is_transient() {
            return Err(error);
        }
        self.quarantine_workspace(workspace, error)
    }

    fn quarantine_workspace<T>(&mut self, workspace: &WorkspaceRecord, error: Error) -> Result<T> {
        let detail = error.to_string();
        self.store
            .uncertain_workspace(&workspace.view.id, &detail)?;
        Err(Error::WorkspaceUncertain {
            workspace_id: workspace.view.id.clone(),
            detail,
        })
    }
}

fn same_workspace_root(stored: &str, requested: &Path) -> bool {
    let normalized: std::path::PathBuf = requested.components().collect();
    Path::new(stored) == normalized
}

#[cfg(test)]
mod tests;
