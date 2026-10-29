use serde_json::json;

use crate::canonical::{canonicalize, payload_hash, sha256_hex};
use crate::domain::PublicationProof;
use crate::error::Result;

/// Stable id for the publication outcome of one delivery intent.
///
/// A second observation with a different payload is an operation conflict.
/// S1 does not contact GitHub; this id only deduplicates host-recorded proof.
pub fn publication_operation_id(delivery_id: &str) -> Result<String> {
    let canonical = canonicalize(&json!(["switchyard.publication.v1", delivery_id]))?;
    Ok(format!("pub-v1:{}", sha256_hex(canonical.as_bytes())))
}

pub fn publication_payload(proof: &PublicationProof) -> serde_json::Value {
    json!({
        "contentDigest": proof.content_digest,
        "commitSha": proof.commit_sha,
        "treeSha": proof.tree_sha,
        "prId": proof.pr_id,
    })
}

pub fn publication_payload_hash(proof: &PublicationProof) -> Result<String> {
    payload_hash(&publication_payload(proof))
}

/// Pure check used by publication guards. This module has no process or network API.
pub fn proof_matches(approved_digest: &str, proof: &PublicationProof) -> bool {
    proof.matches_digest(approved_digest)
}
