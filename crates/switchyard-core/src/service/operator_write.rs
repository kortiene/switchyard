//! Operator worktree write. One file per call, recorded as a decision.
//! Not a transition: revision, commands, and content digest stay put.
//! FakeRuntime `WriteFile` is not used.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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
        // Drift, then phase, then ready_workspace. A wrong phase returns the
        // phase error. ready_workspace is the only blocked-execution check, and
        // it does not deliver an admitted stop.
        if self.frozen_input_drifted(&mission)? {
            return self.refuse_frozen_input_drift(&mission);
        }
        if mission.phase != Phase::Implementing {
            return Err(Error::OperatorWritePhaseClosed {
                phase: mission.phase.as_str().into(),
            });
        }
        let workspace = self.ready_workspace(mission_id)?;
        let destination = operator_destination(Path::new(&workspace.path), path)?;
        let bytes = read_operator_source(from)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(Error::storage)?;
        }
        install_regular_file(&destination, &bytes)?;
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
    let limit = crate::linux_execution::MAX_FILE_BYTES.saturating_add(1);
    let bytes = crate::regular_file::read_regular_nofollow(from, limit)?;
    if bytes.len() > crate::linux_execution::MAX_FILE_BYTES {
        return Err(Error::guard("operator write content exceeds 64 KiB"));
    }
    Ok(bytes)
}

/// Workspace-relative destination. Reuses the Linux path rules, then refuses
/// any symlink component and a leaf that exists but is not a regular file.
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
                return Err(Error::guard("operator write path contains a symlink"));
            }
            Ok(meta) if last && !meta.is_file() => {
                return Err(Error::guard("operator write target is not a regular file"));
            }
            Ok(meta) if !last && !meta.is_dir() => {
                return Err(Error::guard("operator write parent is not a directory"));
            }
            Ok(_) => cursor = next,
        }
    }
    if !cursor.starts_with(&root) {
        return Err(Error::guard("operator write path leaves the workspace"));
    }
    Ok(cursor)
}

/// Write `bytes` via `.switchyard-write-<random>` in the destination directory,
/// opened with `O_CREAT|O_EXCL|O_NOFOLLOW`, then rename it over the leaf.
/// Any error removes the temp file.
fn install_regular_file(destination: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| Error::guard("operator write has no workspace parent"))?;
    let temp = parent.join(format!(
        ".switchyard-write-{}",
        crate::domain::new_id("f").trim_start_matches("f-")
    ));
    let written = (|| -> Result<()> {
        let mut options = OpenOptions::new();
        options.write(true);
        options.custom_flags(libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW);
        let mut file = options.open(&temp).map_err(Error::storage)?;
        file.write_all(bytes).map_err(Error::storage)?;
        if let Ok(leaf) = std::fs::symlink_metadata(destination)
            && leaf.file_type().is_file()
        {
            let mode = leaf.permissions().mode() & 0o7777;
            let mut perms = file.metadata().map_err(Error::storage)?.permissions();
            perms.set_mode(mode);
            file.set_permissions(perms).map_err(Error::storage)?;
        }
        std::fs::rename(&temp, destination).map_err(Error::storage)?;
        Ok(())
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    written
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
        let again = refuse(&mut fix, "src/note.txt", &from);
        assert_eq!(again.category(), "frozen_input_drift");
        let tree = fix.tree.as_ref().unwrap();
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
    fn operator_write_refuses_symlinks_and_non_regular_leaves_without_a_temp_file() {
        let mut fix = implementing(true);
        let from = source(fix._tmp.path(), "body.txt", b"hello");
        let tree = fix.tree.clone().unwrap();
        let inside = tree.join("inside");
        fs::create_dir(&inside).unwrap();
        std::os::unix::fs::symlink(&inside, tree.join("link")).unwrap();
        assert_eq!(
            refuse(&mut fix, "link/pwned.txt", &from).category(),
            "guard_failed"
        );
        assert!(!inside.join("pwned.txt").exists());

        let gitfile = fs::read(tree.join(".git")).unwrap();
        std::os::unix::fs::symlink(".git", tree.join("x")).unwrap();
        assert_eq!(refuse(&mut fix, "x", &from).category(), "guard_failed");
        assert_eq!(fs::read(tree.join(".git")).unwrap(), gitfile);

        assert!(
            Command::new("mkfifo")
                .arg(tree.join("pipe"))
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(refuse(&mut fix, "pipe", &from).category(), "guard_failed");
        fs::create_dir(tree.join("adir")).unwrap();
        assert_eq!(refuse(&mut fix, "adir", &from).category(), "guard_failed");
        assert!(
            !stray_temp(&tree),
            "a refused write left a temp file in the worktree"
        );

        let nested = fix._tmp.path().join("not-a-file");
        fs::create_dir(&nested).unwrap();
        let failed = install_regular_file(&nested, b"hello");
        assert!(failed.is_err(), "renaming onto a directory should fail");
        assert!(
            !stray_temp(fix._tmp.path()),
            "a failed install left {}",
            stray_names(fix._tmp.path()).join(", ")
        );
    }

    #[test]
    fn overwriting_keeps_the_existing_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let mut fix = implementing(true);
        let tree = fix.tree.clone().unwrap();
        let leaf = tree.join("script.sh");
        fs::write(&leaf, b"old").unwrap();
        let mut perms = fs::metadata(&leaf).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&leaf, perms).unwrap();
        let from = source(fix._tmp.path(), "body.txt", b"new");
        fix.host
            .operator_write(&fix.mission, &fix.attempt, "script.sh", &from, "operator")
            .unwrap();
        let mode = fs::symlink_metadata(&leaf).unwrap().permissions().mode() & 0o7777;
        assert_eq!(mode, 0o755);
        assert_eq!(fs::read(&leaf).unwrap(), b"new");
    }

    #[test]
    fn operator_write_accepts_exactly_64_kib_and_refuses_a_fifo_source() {
        let mut fix = implementing(true);
        let exact = vec![b'a'; crate::linux_execution::MAX_FILE_BYTES];
        let from = source(fix._tmp.path(), "exact.bin", &exact);
        let wrote = fix
            .host
            .operator_write(&fix.mission, &fix.attempt, "exact.bin", &from, "operator")
            .unwrap();
        assert_eq!(wrote.bytes, crate::linux_execution::MAX_FILE_BYTES as u64);
        let tree = fix.tree.clone().unwrap();
        assert_eq!(fs::read(tree.join("exact.bin")).unwrap(), exact);

        let fifo = fix._tmp.path().join("source.pipe");
        assert!(
            Command::new("mkfifo")
                .arg(&fifo)
                .status()
                .unwrap()
                .success()
        );
        assert_eq!(
            refuse(&mut fix, "from-fifo.txt", &fifo).category(),
            "guard_failed"
        );
        assert!(!tree.join("from-fifo.txt").exists());
    }

    #[test]
    fn operator_write_refuses_a_blocked_execution_and_an_unready_workspace() {
        let mut blocked = implementing(true);
        let snap = blocked.host.snapshot(&blocked.mission).unwrap();
        rusqlite::Connection::open(&blocked.db)
            .unwrap()
            .execute(
                "INSERT INTO linux_execution_effects(
                    operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                    started_generation, state, created_at
                 ) VALUES ('op-v1:operator-write', ?1, ?2, ?3, 'sha256:row', '{}', 1, 'prepared', 1)",
                rusqlite::params![blocked.mission, blocked.attempt, snap.session_id],
            )
            .unwrap();
        let from = source(blocked._tmp.path(), "body.txt", b"hello");
        let error = refuse(&mut blocked, "blocked.txt", &from);
        assert_eq!(error.category(), "execution_blocked", "{error}");
        assert!(!blocked.tree.as_ref().unwrap().join("blocked.txt").exists());

        let mut stopped = implementing(true);
        let snap = stopped.host.snapshot(&stopped.mission).unwrap();
        rusqlite::Connection::open(&stopped.db)
            .unwrap()
            .execute(
                "INSERT INTO linux_execution_effects(
                    operation_id, mission_id, attempt_id, session_id, payload_hash, request_json,
                    started_generation, state, created_at
                 ) VALUES ('op-v1:write-stop', ?1, ?2, ?3, 'sha256:row', '{}', 1, 'prepared', 1)",
                rusqlite::params![stopped.mission, stopped.attempt, snap.session_id],
            )
            .unwrap();
        admit(&mut stopped.host, &stopped.mission, CommandBody::Pause);
        let from = source(stopped._tmp.path(), "body.txt", b"hello");
        let error = refuse(&mut stopped, "stopped.txt", &from);
        assert_eq!(error.category(), "operator_write_phase_closed", "{error}");
        let after = stopped.host.snapshot(&stopped.mission).unwrap();
        let pause = after
            .commands
            .iter()
            .find(|command| command.kind == "pause")
            .unwrap();
        assert_eq!(pause.state, "pending");
        assert!(pause.receipt_id.is_none());
        assert!(!stopped.tree.as_ref().unwrap().join("stopped.txt").exists());

        let mut stale = implementing(true);
        let changed = rusqlite::Connection::open(&stale.db)
            .unwrap()
            .execute(
                "UPDATE workspaces SET state='uncertain', detail='not verified for this owner'
                 WHERE mission_id=?1",
                [&stale.mission],
            )
            .unwrap();
        assert_eq!(changed, 1);
        let from = source(stale._tmp.path(), "body.txt", b"hello");
        assert_eq!(
            refuse(&mut stale, "later.txt", &from).category(),
            "workspace_uncertain"
        );
        assert!(!stale.tree.unwrap().join("later.txt").exists());
    }

    #[test]
    #[cfg(feature = "pi-contract")]
    fn operator_write_is_unavailable_off_fake() {
        // The default build's only runtime is Fake, so this refusal runs only with pi-contract.
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

    fn stray_temp(dir: &Path) -> bool {
        !stray_names(dir).is_empty()
    }

    fn stray_names(dir: &Path) -> Vec<String> {
        let mut names = Vec::new();
        let entries = fs::read_dir(dir).unwrap();
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".switchyard-write-") {
                names.push(name);
            }
            if entry.file_type().unwrap().is_dir()
                && let Ok(nested) = fs::read_dir(entry.path())
            {
                for child in nested.flatten() {
                    let child_name = child.file_name().to_string_lossy().into_owned();
                    if child_name.starts_with(".switchyard-write-") {
                        names.push(child_name);
                    }
                }
            }
        }
        names
    }
}
