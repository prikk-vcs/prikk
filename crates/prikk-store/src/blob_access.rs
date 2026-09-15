//! Blob role validation (FDD-03 §9.3.0).
//!
//! Now that `BlobPayload.blob_kind` exists, store readers validate a blob's role at
//! decode time rather than trusting the reference: snapshot references must resolve
//! to a `SNAPSHOT` blob, and file-content references must not. This fails closed
//! before `blob_kind` becomes the semantic source for node-kind behavior in the
//! Phase 4 node model.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlobKind, BlobPayload, CanonicalEncode, NodeKind, ObjectId, ObjectType};

/// Decode a Blob that must be a `SNAPSHOT` blob, returning its content.
pub(crate) fn decode_snapshot_blob(canonical_payload: &[u8]) -> Result<Vec<u8>> {
    let blob = BlobPayload::decode_canonical(canonical_payload)?;
    if blob.blob_kind != BlobKind::Snapshot {
        return Err(PrikkError::Integrity(
            "snapshot reference points to a non-SNAPSHOT blob".to_string(),
        ));
    }
    Ok(blob.content)
}

/// Decode a Blob used as file content, returning its content. A file node MUST NOT
/// reference a `SNAPSHOT` blob (§9.3.0). `TEXT` and `BINARY` are both accepted in
/// the pre-node interim; Phase 4 node-kind derivation tightens this per operation.
pub(crate) fn decode_file_content_blob(canonical_payload: &[u8]) -> Result<Vec<u8>> {
    let blob = BlobPayload::decode_canonical(canonical_payload)?;
    if blob.blob_kind == BlobKind::Snapshot {
        return Err(PrikkError::Integrity(
            "file content reference points to a SNAPSHOT blob".to_string(),
        ));
    }
    Ok(blob.content)
}

/// Decode a file-content Blob, returning the derived node kind and content. A
/// file node MUST NOT reference a `SNAPSHOT` blob (§9.3.0); `TEXT` derives
/// `TextFile` and `BINARY` derives `BinaryFile`. Used by the inverse path to fill
/// `DeleteNode.old_node_kind` from a created file's blob.
pub(crate) fn decode_file_content_blob_with_kind(
    canonical_payload: &[u8],
) -> Result<(prikk_object::NodeKind, Vec<u8>)> {
    let blob = BlobPayload::decode_canonical(canonical_payload)?;
    if blob.blob_kind == BlobKind::Snapshot {
        return Err(PrikkError::Integrity(
            "file content reference points to a SNAPSHOT blob".to_string(),
        ));
    }
    let kind = prikk_object::NodeKind::from_file_blob_kind(blob.blob_kind)?;
    Ok((kind, blob.content))
}

/// Resolve a blob's `blob_kind` from its canonical payload and require `BINARY`
/// (FDD-03 §9.3 `ReplaceBinary`). `ReplaceBinary` operates on binary file nodes
/// only: v1 rejects text<->binary transitions and SNAPSHOT references on this path,
/// so applying it requires *both* the old and the new blob to resolve to `BINARY`.
/// This is the binary-only enforcement primitive; node-addressed `ReplaceBinary`
/// application (which calls it on both blob ids) is wired in at the node model
/// (increment 4.4).
#[cfg(test)]
pub(crate) fn ensure_blob_kind_is_binary(canonical_payload: &[u8]) -> Result<()> {
    let blob = BlobPayload::decode_canonical(canonical_payload)?;
    if blob.blob_kind == BlobKind::Binary {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "ReplaceBinary requires BINARY blobs; found blob_kind {:?}",
        blob.blob_kind
    )))
}
/// Verify that `bytes` identify as the expected file-content Blob *under the blob
/// kind derived from `old_node_kind`* (FDD-03 §9.3): `TEXT_FILE` -> `Text`,
/// `BINARY_FILE` -> `Binary`. This both confirms the `old_blob_id` precondition
/// and enforces the `old_node_kind` <-> `blob_kind` agreement, so a binary-file
/// deletion is not rejected by a Text-only recompute and a kind/blob mismatch
/// (including a snapshot-blob id) fails closed. Symlink nodes have no file-content
/// blob and are rejected.
pub(crate) fn ensure_blob_matches_node_kind(
    bytes: &[u8],
    expected: ObjectId,
    old_node_kind: NodeKind,
) -> Result<()> {
    let id = file_blob_id_for_kind(bytes, old_node_kind)?;
    if id == expected {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "DeleteNode old_blob_id/old_node_kind mismatch: expected {expected}, got {id}"
    )))
}

/// Whether `bytes`, as a file of `node_kind`, are exactly the content `expected` names. Content ids are
/// hashes of the canonical Blob payload, so this is byte equality with the named content.
pub(crate) fn bytes_match_blob_id(
    bytes: &[u8],
    expected: ObjectId,
    node_kind: NodeKind,
) -> Result<bool> {
    Ok(file_blob_id_for_kind(bytes, node_kind)? == expected)
}

fn file_blob_id_for_kind(bytes: &[u8], old_node_kind: NodeKind) -> Result<ObjectId> {
    let blob_kind = match old_node_kind {
        NodeKind::TextFile => BlobKind::Text,
        NodeKind::BinaryFile => BlobKind::Binary,
        NodeKind::Symlink => {
            return Err(PrikkError::Integrity(
                "DeleteNode symlink node has no file-content blob".to_string(),
            ));
        }
    };
    let payload = BlobPayload::new(blob_kind, bytes.to_vec());
    Ok(ObjectId::from_canonical_payload(
        ObjectType::Blob,
        1,
        &payload.to_canonical_bytes()?,
    ))
}

#[cfg(test)]
mod tests;
