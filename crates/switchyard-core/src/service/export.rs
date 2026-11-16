//! Read-only ledger export.
//!
//! No owner lock, no migration, and no dispatch. The mission object is
//! `MissionSnapshot::to_json` with `piHost`, `quarantine`, and `inputsFrozen`
//! removed. The freeze document is the stored canonical string plus its digest.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

use serde_json::{Value, json};

use super::{Error, Host, Result, reader_db_path, snapshot_from};
use crate::canonical::payload_hash;
use crate::domain::CommandRecord;
use crate::persistence::{Store, StoredDecision, StoredVerification};

impl Host {
    /// Read one mission's export document.
    ///
    /// Uses [`Store::open_reader`] and the same snapshot path as [`Host::inspect`].
    /// Does not take the owner lock, migrate, or dispatch.
    pub fn export_document(db_path: &Path, mission_id: &str, host_version: &str) -> Result<Value> {
        let db_path = reader_db_path(db_path)?;
        let store = Store::open_reader(&db_path)?;
        store.with_deferred_read(|store| {
            let generation = store.generation()?;
            let snapshot = snapshot_from(store, mission_id, generation, None)?;
            let schema_version = store.ledger_schema_version()?;
            let mut commands = Vec::new();
            for command in store.list_commands(mission_id)? {
                commands.push(command_json(&command));
            }
            let mut decisions = Vec::new();
            for decision in store.list_decisions(mission_id)? {
                decisions.push(decision_json(&decision));
            }
            let mut verifications = Vec::new();
            for row in store.list_verifications(mission_id)? {
                verifications.push(verification_json(&row)?);
            }
            Ok(json!({
                "schema": "switchyard.ledger-export.v1",
                "ledgerSchemaVersion": schema_version,
                "hostVersion": host_version,
                "mission": mission_json(&snapshot),
                "attempt": attempt_json(&snapshot)?,
                "commands": commands,
                "decisions": decisions,
                "verifications": verifications,
            }))
        })
    }
}

/// Receipt for a document written by [`write_export_exclusive`].
///
/// `sha256` is `sha256:` plus the hex SHA-256 of the bytes just written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportReceipt {
    pub bytes: u64,
    pub sha256: String,
}

/// Create `path` with `O_CREAT|O_EXCL|O_NOFOLLOW` and write `bytes`.
///
/// An existing path, including a symlink, is [`Error::ExportTargetExists`].
/// The caller must finish the ledger read before calling this. A write error
/// removes the file this call created, so the next try is not
/// `export_target_exists`.
pub fn write_export_exclusive(path: &Path, bytes: &[u8]) -> Result<ExportReceipt> {
    let mut file = create_export_target(path)?;
    let result = persist_export(&mut file, path, bytes);
    drop(file);
    result?;
    Ok(ExportReceipt {
        bytes: u64::try_from(bytes.len()).map_err(|_| Error::storage("export is too large"))?,
        sha256: format!("sha256:{}", crate::canonical::sha256_hex(bytes)),
    })
}

fn mission_json(snapshot: &super::MissionSnapshot) -> Value {
    let mut value = snapshot.to_json();
    if let Some(object) = value.as_object_mut() {
        object.remove("piHost");
        object.remove("quarantine");
        object.remove("inputsFrozen");
    }
    value
}

fn attempt_json(snapshot: &super::MissionSnapshot) -> Result<Value> {
    let (frozen_inputs_json, frozen_inputs_digest) = match &snapshot.frozen_inputs_json {
        Some(stored) => {
            let document: Value = serde_json::from_str(stored)
                .map_err(|_| Error::storage("frozen inputs are not valid JSON"))?;
            (
                Value::String(stored.clone()),
                Value::String(payload_hash(&document)?),
            )
        }
        None => (Value::Null, Value::Null),
    };
    let verifier = serde_json::from_str::<Value>(&snapshot.verifier_json).ok();
    Ok(json!({
        "frozenInputsJson": frozen_inputs_json,
        "frozenInputsDigest": frozen_inputs_digest,
        "planBody": snapshot.plan_body,
        "planDigest": snapshot.plan_digest,
        "verifier": verifier,
        "ownerGeneration": snapshot.attempt_owner_generation,
    }))
}

fn command_json(command: &CommandRecord) -> Value {
    let receipt = command.runtime_receipt_json.as_deref().map(|text| {
        serde_json::from_str::<Value>(text).unwrap_or_else(|_| Value::String(text.to_string()))
    });
    json!({
        "seq": command.seq,
        "id": command.id,
        "kind": command.kind.as_str(),
        "origin": command.origin.as_str(),
        "state": command.state.as_str(),
        "payloadCanonical": command.payload_canonical,
        "payloadHash": command.payload_hash,
        "expectedRevision": command.expected_revision,
        "resultingRevision": command.resulting_revision,
        "requestId": command.request_id,
        "receipt": receipt,
    })
}

fn decision_json(decision: &StoredDecision) -> Value {
    json!({
        "action": decision.action,
        "actor": decision.actor,
        "rationale": decision.rationale,
        "digest": decision.digest,
        "commandId": decision.command_id,
        "invalidated": decision.invalidated,
    })
}

fn verification_json(row: &StoredVerification) -> Result<Value> {
    let provenance = serde_json::from_str::<Value>(&row.provenance_json)
        .map_err(|_| Error::storage("verification provenance is not JSON"))?;
    let body = serde_json::from_str::<Value>(&row.body_json)
        .map_err(|_| Error::storage("verification evidence body is not JSON"))?;
    Ok(json!({
        "seq": row.seq,
        "outcome": row.outcome,
        "contentDigest": row.content_digest,
        "invalidated": row.invalidated,
        "evidence": {
            "completeness": row.completeness,
            "contentHash": row.content_hash,
            "provenance": provenance,
            "body": body,
        },
    }))
}

fn create_export_target(path: &Path) -> Result<std::fs::File> {
    let mut options = OpenOptions::new();
    options.write(true);
    options.custom_flags(libc::O_CREAT | libc::O_EXCL | libc::O_NOFOLLOW);
    match options.open(path) {
        Ok(file) => Ok(file),
        Err(error) if export_target_conflict(&error) => Err(Error::ExportTargetExists),
        Err(error) => Err(Error::storage(error)),
    }
}

fn persist_export(file: &mut dyn Write, path: &Path, bytes: &[u8]) -> Result<()> {
    let written = match file.write_all(bytes) {
        Ok(()) => file.flush(),
        Err(error) => Err(error),
    };
    if let Err(error) = written {
        let _ = std::fs::remove_file(path);
        return Err(Error::storage(error));
    }
    Ok(())
}

fn export_target_conflict(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::AlreadyExists
        || matches!(
            error.raw_os_error(),
            Some(code) if code == libc::EEXIST || code == libc::ELOOP || code == libc::EISDIR
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FailWrite;

    impl Write for FailWrite {
        fn write(&mut self, _buf: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::StorageFull,
                "no space",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn failed_export_write_removes_the_file_and_retry_is_not_export_target_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ledger.json");
        let created = create_export_target(&path).unwrap();
        drop(created);
        assert!(
            path.is_file(),
            "the exclusive create must land before the write"
        );
        let error = persist_export(
            &mut FailWrite,
            &path,
            br#"{"schema":"switchyard.ledger-export.v1"}"#,
        )
        .unwrap_err();
        assert_eq!(error.category(), "storage_failed");
        assert!(
            !path.exists(),
            "a failed write left an export file that would block the retry"
        );

        let bytes = b"{\"schema\":\"switchyard.ledger-export.v1\"}\n";
        write_export_exclusive(&path, bytes).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        let again = write_export_exclusive(&path, bytes).unwrap_err();
        assert_eq!(again.category(), "export_target_exists");
        assert!(again.to_string().contains("remove the file"));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }

    #[test]
    fn export_target_exists_for_a_file_and_a_symlink_leaves_them_unchanged() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("ledger.json");
        std::fs::write(&path, b"keep-me").unwrap();
        let error = write_export_exclusive(&path, b"new").unwrap_err();
        assert_eq!(error.category(), "export_target_exists");
        assert_eq!(std::fs::read(&path).unwrap(), b"keep-me");

        let target = tmp.path().join("secret");
        std::fs::write(&target, b"secret").unwrap();
        let link = tmp.path().join("link.json");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let error = write_export_exclusive(&link, b"new").unwrap_err();
        assert_eq!(error.category(), "export_target_exists");
        assert_eq!(std::fs::read(&target).unwrap(), b"secret");
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
}
