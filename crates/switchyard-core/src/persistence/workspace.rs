use super::*;
use crate::workspace::{WorkspaceRecord, WorkspaceView};

impl Store {
    /// Fence even an idempotent filesystem operation before touching Git.
    pub(crate) fn assert_workspace_owner(&mut self) -> Result<()> {
        self.immediate()?.commit().map_err(Error::storage)
    }

    pub(crate) fn load_workspace(&self, attempt: &str) -> Result<Option<WorkspaceRecord>> {
        if user_version(&self.conn)? < 3 {
            return Ok(None); // Historical read-only inspection does not migrate.
        }
        self.conn
            .query_row(
                "SELECT id, mission_id, attempt_id, path, base_sha, state,
             root, root_identity, source, common_dir, common_identity,
             slot_identity, lease_identity, tree_identity, git_dir,
             created_generation, verified_generation, detail, ledger_path FROM workspaces WHERE attempt_id=?1",
                [attempt],
                |row| {
                    Ok(WorkspaceRecord {
                        ledger_path: row.get(18)?,
                        view: WorkspaceView {
                            id: row.get(0)?,
                            mission_id: row.get(1)?,
                            attempt_id: row.get(2)?,
                            path: row.get(3)?,
                            base_sha: row.get(4)?,
                            state: row.get(5)?,
                            created_generation: row.get(15)?,
                            verified_generation: row.get(16)?,
                            detail: row.get(17)?,
                        },
                        root: row.get(6)?,
                        root_identity: row.get(7)?,
                        source: row.get(8)?,
                        common_dir: row.get(9)?,
                        common_identity: row.get(10)?,
                        slot_identity: row.get(11)?,
                        lease_identity: row.get(12)?,
                        tree_identity: row.get(13)?,
                        git_dir: row.get(14)?,
                    })
                },
            )
            .optional()
            .map_err(Error::storage)
    }

    pub(crate) fn reserve_workspace(&mut self, w: &WorkspaceRecord) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute(
            "INSERT INTO workspaces (id, mission_id, attempt_id, path, base_sha, state,
             root, root_identity, source, common_dir, common_identity, created_generation,
             created_at, updated_at, ledger_path) VALUES (?1,?2,?3,?4,?5,'reserved',?6,?7,?8,?9,?10,?11,?12,?12,?13)",
            params![w.view.id, w.view.mission_id, w.view.attempt_id, w.view.path,
                w.view.base_sha, w.root, w.root_identity, w.source, w.common_dir,
                w.common_identity, w.view.created_generation, now_ms(), w.ledger_path],
        ).map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn start_workspace(&mut self, id: &str, slot: &str, lease: &str) -> Result<()> {
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE workspaces SET state='creating', slot_identity=?1, lease_identity=?2,
             updated_at=?3 WHERE id=?4 AND state='reserved'",
                params![slot, lease, now_ms(), id],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard("workspace is no longer reserved"));
        }
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn ready_workspace(&mut self, id: &str, tree: &str, git_dir: &str) -> Result<()> {
        let generation = self
            .owner_generation
            .ok_or_else(|| Error::guard("no workspace owner"))?;
        let tx = self.immediate()?;
        let changed = tx
            .execute(
                "UPDATE workspaces SET state='ready', tree_identity=?1, git_dir=?2,
             verified_generation=?3, detail=NULL, updated_at=?4
             WHERE id=?5 AND state IN ('creating','ready')",
                params![tree, git_dir, generation, now_ms(), id],
            )
            .map_err(Error::storage)?;
        if changed != 1 {
            return Err(Error::guard("workspace cannot become ready"));
        }
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn uncertain_workspace(&mut self, id: &str, detail: &str) -> Result<()> {
        let tx = self.immediate()?;
        tx.execute(
            "UPDATE workspaces SET state='uncertain', detail=?1, updated_at=?2 WHERE id=?3",
            params![detail, now_ms(), id],
        )
        .map_err(Error::storage)?;
        tx.commit().map_err(Error::storage)
    }

    pub(crate) fn require_workspace_ready(&self, attempt: &str) -> Result<()> {
        if let Some(w) = self.load_workspace(attempt)?
            && (w.view.state != "ready" || w.view.verified_generation != self.owner_generation)
        {
            return Err(Error::guard(format!(
                "workspace {} is not verified for this owner; recover it before dispatch",
                w.view.id
            )));
        }
        Ok(())
    }
}
