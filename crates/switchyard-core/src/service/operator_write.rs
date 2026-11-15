//! Operator worktree write. One file per call, recorded as a decision.
//! Not a transition: revision, commands, and content digest stay put.
//! FakeRuntime `WriteFile` is not used.

use std::path::{Path, PathBuf};

use super::*;

/// Receipt for one operator-authored file in the owned worktree.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OperatorWrite {
    pub path: String,
    pub bytes: u64,
    pub sha256: String,
    pub actor: String,
    pub workspace_path: String,
    pub base_sha: String,
}

impl Host {
    /// Copy `from` to `path` inside the attempt's worktree.
    ///
    /// `path` is workspace-relative. The ledger row is a `decisions` insert
    /// (`action` `operator_write`, `command_id` NULL). A successful write does
    /// not bump the revision. Drift uses the existing check and, on the first
    /// refusal, records `needs_attention` before returning [`Error::FrozenInputDrift`].
    pub fn operator_write(
        &mut self,
        mission_id: &str,
        attempt_id: &str,
        path: &str,
        from: &Path,
        actor: &str,
    ) -> Result<OperatorWrite> {
        if self.runtime.fake_mut().is_none() {
            return Err(Error::OperatorWriteUnavailable);
        }
        let mission = self.store.load_mission(mission_id)?;
        if mission.attempt.id != attempt_id {
            return Err(Error::BindingMismatch {
                detail: "operator write attempt does not belong to this mission".into(),
            });
        }
        if mission.phase != Phase::Implementing {
            return Err(Error::OperatorWritePhaseClosed {
                phase: mission.phase.as_str().into(),
            });
        }
        if self.frozen_input_drifted(&mission)? {
            if mission.phase != Phase::NeedsAttention && !mission.phase.is_terminal() {
                self.record_frozen_input_drift(&mission)?;
            }
            return Err(Error::FrozenInputDrift);
        }
        let workspace = match self.recover_workspace(mission_id)? {
            Some(view)
                if view.state == "ready" && view.verified_generation == Some(self.generation()) =>
            {
                view
            }
            Some(view) => {
                return Err(Error::WorkspaceUncertain {
                    workspace_id: view.id,
                    detail: view
                        .detail
                        .unwrap_or_else(|| "workspace is not ready for this owner".into()),
                });
            }
            None => {
                return Err(Error::WorkspaceUncertain {
                    workspace_id: attempt_id.to_string(),
                    detail: "no workspace is recorded for this attempt".into(),
                });
            }
        };
        let destination = operator_destination(Path::new(&workspace.path), path)?;
        let bytes = read_operator_source(from)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(Error::storage)?;
        }
        std::fs::write(&destination, &bytes).map_err(Error::storage)?;
        let digest = format!("sha256:{}", crate::canonical::sha256_hex(&bytes));
        let rationale = crate::canonical::canonicalize(&json!({
            "actor": actor,
            "authoredBy": "operator",
            "bytes": bytes.len(),
            "path": path,
            "sha256": digest,
        }))?;
        self.store
            .record_operator_write(mission_id, attempt_id, actor, &rationale, &digest)?;
        Ok(OperatorWrite {
            path: path.to_string(),
            bytes: u64::try_from(bytes.len()).unwrap_or(u64::MAX),
            sha256: digest,
            actor: actor.to_string(),
            workspace_path: workspace.path,
            base_sha: workspace.base_sha,
        })
    }
}

fn read_operator_source(from: &Path) -> Result<Vec<u8>> {
    let meta = match std::fs::symlink_metadata(from) {
        Ok(meta) => meta,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::guard("operator write source file is missing"));
        }
        Err(error) => return Err(Error::storage(error)),
    };
    if !meta.file_type().is_file() {
        return Err(Error::guard("operator write source is not a regular file"));
    }
    if meta.len() > crate::linux_execution::MAX_FILE_BYTES as u64 {
        return Err(Error::guard("operator write content exceeds 64 KiB"));
    }
    let bytes = std::fs::read(from).map_err(Error::storage)?;
    if bytes.len() > crate::linux_execution::MAX_FILE_BYTES {
        return Err(Error::guard("operator write content exceeds 64 KiB"));
    }
    Ok(bytes)
}

/// Workspace-relative destination. Reuses the Linux path rules, then refuses
/// a symlink whose resolution leaves the worktree.
fn operator_destination(workspace: &Path, relative: &str) -> Result<PathBuf> {
    crate::linux_execution::validate_workspace_path(relative, false)?;
    let root = std::fs::canonicalize(workspace).map_err(Error::storage)?;
    let parts: Vec<&str> = relative.split('/').collect();
    let mut cursor = root.clone();
    for (index, part) in parts.iter().enumerate() {
        let next = cursor.join(part);
        let last = index + 1 == parts.len();
        match std::fs::symlink_metadata(&next) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                cursor = next;
                for rest in &parts[index + 1..] {
                    cursor = cursor.join(rest);
                }
                break;
            }
            Err(error) => return Err(Error::storage(error)),
            Ok(meta) if meta.file_type().is_symlink() => {
                let resolved = std::fs::canonicalize(&next)
                    .map_err(|_| Error::guard("symlinked path resolves outside the workspace"))?;
                if !stays_inside(&root, &resolved) {
                    return Err(Error::guard(
                        "symlinked path resolves outside the workspace",
                    ));
                }
                if last {
                    cursor = next;
                } else if resolved.is_dir() {
                    cursor = resolved;
                } else {
                    return Err(Error::guard("symlinked parent is not a directory"));
                }
            }
            Ok(meta) if !last && !meta.is_dir() => {
                return Err(Error::guard("operator write parent is not a directory"));
            }
            Ok(_) => cursor = next,
        }
    }
    if !cursor.starts_with(&root) {
        return Err(Error::guard(
            "symlinked path resolves outside the workspace",
        ));
    }
    let parent = cursor.parent().unwrap_or(root.as_path());
    let ancestor = existing_ancestor(parent)?;
    if !stays_inside(&root, &ancestor) {
        return Err(Error::guard(
            "symlinked path resolves outside the workspace",
        ));
    }
    Ok(cursor)
}

fn stays_inside(root: &Path, path: &Path) -> bool {
    path == root || path.starts_with(root)
}

fn existing_ancestor(path: &Path) -> Result<PathBuf> {
    let mut cursor = path.to_path_buf();
    loop {
        if cursor.exists() {
            return std::fs::canonicalize(&cursor).map_err(Error::storage);
        }
        if !cursor.pop() {
            return Err(Error::guard("operator write has no workspace parent"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution::RuntimeNotification;
    use std::fs;
    use std::process::Command;

    struct Fixture {
        _tmp: tempfile::TempDir,
        db: PathBuf,
        root: PathBuf,
        host: Host,
        mission: String,
        attempt: String,
        tree: Option<PathBuf>,
    }

    fn git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn admit(host: &mut Host, mission: &str, body: CommandBody) {
        let revision = host.snapshot(mission).unwrap().revision;
        host.admit(AdmitRequest {
            command_id: None,
            mission_id: mission.to_string(),
            attempt_id: None,
            expected_revision: revision,
            actor: "operator".into(),
            origin: CommandOrigin::Operator,
            body,
        })
        .unwrap();
    }

    fn opened() -> Fixture {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        let root = tmp.path().join("workspaces");
        fs::create_dir_all(&repo).unwrap();
        fs::create_dir(&root).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "switchyard@example.com"]);
        git(&repo, &["config", "user.name", "Switchyard"]);
        fs::write(repo.join("README.md"), "hello\n").unwrap();
        git(&repo, &["add", "README.md"]);
        git(&repo, &["commit", "-m", "init"]);
        let db = tmp.path().join("mission.sqlite");
        let mut host = Host::open(&db).unwrap();
        let created = host
            .create_mission(CreateMission {
                repo,
                issue_number: 196,
                issue_title: "write".into(),
                objective: "operator write".into(),
                acceptance: "file lands in the worktree".into(),
                actor: "operator".into(),
                max_repairs: 1,
                max_plan_revisions: 1,
            })
            .unwrap();
        Fixture {
            db,
            root,
            host,
            mission: created.mission_id,
            attempt: created.attempt_id,
            tree: None,
            _tmp: tmp,
        }
    }

    fn implementing(workspace: bool) -> Fixture {
        let mut fix = opened();
        admit(&mut fix.host, &fix.mission, CommandBody::StartPlanning);
        fix.host.dispatch_pending(&fix.mission).unwrap();
        fix.host
            .apply_notification(
                &fix.mission,
                RuntimeNotification::PlanCompleted {
                    plan: "write the file".into(),
                    quiescent: true,
                },
            )
            .unwrap();
        let digest = fix
            .host
            .snapshot(&fix.mission)
            .unwrap()
            .plan_digest
            .unwrap();
        admit(
            &mut fix.host,
            &fix.mission,
            CommandBody::ApprovePlan {
                plan_digest: digest,
            },
        );
        assert_eq!(
            fix.host.snapshot(&fix.mission).unwrap().phase,
            Phase::Implementing
        );
        if workspace {
            let view = fix
                .host
                .prepare_workspace(&fix.mission, &fix.attempt, &fix.root)
                .unwrap();
            assert_eq!(view.state, "ready");
            fix.tree = Some(PathBuf::from(view.path));
        }
        fix
    }

    fn source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn refuse(fix: &mut Fixture, path: &str, from: &Path) -> Error {
        fix.host
            .operator_write(&fix.mission, &fix.attempt, path, from, "operator")
            .unwrap_err()
    }

    #[test]
    fn operator_write_lands_the_file_and_a_decision_without_a_revision_bump() {
        let mut fix = implementing(true);
        let before = fix.host.snapshot(&fix.mission).unwrap();
        let from = source(fix._tmp.path(), "body.txt", b"hello");
        let wrote = fix
            .host
            .operator_write(&fix.mission, &fix.attempt, "src/note.txt", &from, "sam")
            .unwrap();
        let tree = fix.tree.unwrap();
        let file = tree.join("src/note.txt");
        let bytes = fs::read(&file).unwrap();
        assert_eq!(bytes, b"hello");
        assert_eq!(
            wrote.sha256,
            format!("sha256:{}", crate::canonical::sha256_hex(&bytes))
        );
        assert_eq!(wrote.bytes, 5);
        assert_eq!(wrote.path, "src/note.txt");
        assert_eq!(wrote.actor, "sam");
        assert_eq!(wrote.workspace_path, tree.display().to_string());
        assert_eq!(wrote.base_sha, before.base_sha);
        let after = fix.host.snapshot(&fix.mission).unwrap();
        assert_eq!(after.phase, Phase::Implementing);
        assert_eq!(after.revision, before.revision);
        assert_eq!(after.content_digest, before.content_digest);
        assert_eq!(after.commands.len(), before.commands.len());
        assert_eq!(fix.host.runtime_mut().unwrap().simulated_effect_count(), 0);
        let decision = fix
            .host
            .decisions(&fix.mission)
            .unwrap()
            .into_iter()
            .find(|decision| decision.action == "operator_write")
            .unwrap();
        assert_eq!(decision.actor, "sam");
        assert_eq!(decision.digest.as_deref(), Some(wrote.sha256.as_str()));
        assert_eq!(decision.command_id, None);
        assert!(!decision.invalidated);
        let rationale = crate::canonical::canonicalize(&json!({
            "actor": "sam",
            "authoredBy": "operator",
            "bytes": 5,
            "path": "src/note.txt",
            "sha256": wrote.sha256,
        }))
        .unwrap();
        assert_eq!(decision.rationale, rationale);
        let version: i64 = rusqlite::Connection::open(&fix.db)
            .unwrap()
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        assert_eq!(version, 12);
    }

    #[test]
    fn operator_write_refuses_a_closed_phase_an_unready_workspace_and_a_bad_source() {
        let mut draft = opened();
        let from = source(draft._tmp.path(), "body.txt", b"hello");
        assert_eq!(
            refuse(&mut draft, "note.txt", &from).category(),
            "operator_write_phase_closed"
        );
        let mut bare = implementing(false);
        let from = source(bare._tmp.path(), "body.txt", b"hello");
        assert_eq!(
            refuse(&mut bare, "note.txt", &from).category(),
            "workspace_uncertain"
        );
        let mut fix = implementing(true);
        let missing = fix._tmp.path().join("missing");
        assert_eq!(
            refuse(&mut fix, "note.txt", &missing).category(),
            "guard_failed"
        );
        let directory = fix.root.clone();
        assert_eq!(
            refuse(&mut fix, "note.txt", &directory).category(),
            "guard_failed"
        );
        let big = source(
            fix._tmp.path(),
            "big.bin",
            &vec![b'a'; crate::linux_execution::MAX_FILE_BYTES + 1],
        );
        assert_eq!(refuse(&mut fix, "big.bin", &big).category(), "guard_failed");
        assert!(!fix.tree.unwrap().join("big.bin").exists());
    }

    #[test]
    fn operator_write_refuses_paths_that_leave_the_worktree() {
        let mut fix = implementing(true);
        let from = source(fix._tmp.path(), "body.txt", b"hello");
        let tree = fix.tree.clone().unwrap();
        for path in ["../escaped.txt", ".git/config", "/etc/passwd"] {
            assert_eq!(
                refuse(&mut fix, path, &from).category(),
                "guard_failed",
                "{path}"
            );
        }
        assert!(!tree.join("../escaped.txt").exists());
        let outside = fix._tmp.path().join("outside");
        fs::create_dir(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, tree.join("escape")).unwrap();
        assert_eq!(
            refuse(&mut fix, "escape/pwned.txt", &from).category(),
            "guard_failed"
        );
        assert!(!outside.join("pwned.txt").exists());
        let target = outside.join("secret");
        fs::write(&target, b"keep").unwrap();
        std::os::unix::fs::symlink(&target, tree.join("secret")).unwrap();
        assert_eq!(refuse(&mut fix, "secret", &from).category(), "guard_failed");
        assert_eq!(fs::read(&target).unwrap(), b"keep");
    }

    #[test]
    fn operator_write_refuses_after_frozen_input_drift_and_creates_no_file() {
        let mut fix = implementing(true);
        let before = fix.host.snapshot(&fix.mission).unwrap().revision;
        fix.host
            .tamper_attempt_base_sha(&fix.mission, "deadbeef")
            .unwrap();
        let from = source(fix._tmp.path(), "body.txt", b"hello");
        let error = refuse(&mut fix, "src/note.txt", &from);
        assert_eq!(error.category(), "frozen_input_drift");
        let tree = fix.tree.unwrap();
        assert!(!tree.join("src/note.txt").exists());
        assert!(!tree.join("src").exists());
        let drifted = fix.host.snapshot(&fix.mission).unwrap();
        assert_eq!(drifted.phase, Phase::NeedsAttention);
        assert_eq!(
            drifted.attention_reason.as_deref(),
            Some("frozen_input_drift")
        );
        assert_eq!(drifted.revision, before + 1);
        assert!(
            fix.host
                .decisions(&fix.mission)
                .unwrap()
                .iter()
                .all(|decision| decision.action != "operator_write")
        );
    }

    #[test]
    #[cfg(feature = "pi-contract")]
    fn operator_write_is_unavailable_off_fake() {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path().join("repo");
        fs::create_dir(&repo).unwrap();
        git(&repo, &["init", "--initial-branch=main"]);
        git(&repo, &["config", "user.email", "switchyard@example.com"]);
        git(&repo, &["config", "user.name", "Switchyard"]);
        git(&repo, &["commit", "--allow-empty", "-m", "init"]);
        let db = tmp.path().join("mission.sqlite");
        let node = Path::new("/bin/true");
        let node = if node.exists() {
            node
        } else {
            Path::new("/usr/bin/true")
        };
        let mut host = Host::open_pi_probe(&db, node).unwrap();
        let created = host
            .create_mission(CreateMission {
                repo,
                issue_number: 196,
                issue_title: "write".into(),
                objective: "unavailable".into(),
                acceptance: "fake only".into(),
                actor: "operator".into(),
                max_repairs: 1,
                max_plan_revisions: 1,
            })
            .unwrap();
        let from = source(tmp.path(), "body.txt", b"hello");
        let error = host
            .operator_write(
                &created.mission_id,
                &created.attempt_id,
                "note.txt",
                &from,
                "operator",
            )
            .unwrap_err();
        assert_eq!(error.category(), "operator_write_unavailable");
    }
}
