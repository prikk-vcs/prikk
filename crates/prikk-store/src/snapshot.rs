//! Sealed snapshot manifests (RFC 136 §10.1): a Block's own state, stored in a `SNAPSHOT` Blob.
//!
//! **A snapshot on block B is B's state after B's patches** (§10.1a) -- exactly the leaf set of B's
//! `state_merkle_root`. A reader that seeds from it does not apply B's patches. The manifest is
//! `PRIKK-SNAPSHOT-MANIFEST-v2\n` followed by each entry in the leaf grammar
//! ([`crate::state_root::state_leaf_fields`], the encoder the state root itself uses), in canonical
//! path order. It carries no file bytes: each file's content is the Blob its entry names.
//!
//! The v1 inline-bytes format is retired: no release ever wrote one, and any magic other than v2's is
//! `Integrity`. Nothing writes a snapshot yet (RFC 136 §10.5 increment 1b is the writer).

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockPayload, CanonicalEncode, MerkleRoot, NodeId, NodeKind,
    ObjectEnvelope, ObjectId, ObjectType,
};

use crate::blob_access::{decode_file_content_blob_with_kind, decode_snapshot_blob};
use crate::object_store::{ObjectReader, ObjectWriter};
use crate::state_root::{
    StateRootContent, StateRootEntry, compute_state_root, decode_state_leaf_fields,
    state_leaf_fields, validate_entries,
};

const SNAPSHOT_MAGIC: &[u8] = b"PRIKK-SNAPSHOT-MANIFEST-v2\n";

/// Blocks between checkpoints (RFC 136 §10.2): a sealed Block carries a snapshot when the nearest
/// snapshotted ancestor on its derivation line is this many blocks back or more, or there is none.
/// The same number bounds the lifecycle cache's incremental steps before it reanchors -- one number,
/// one place.
pub(crate) const CHECKPOINT_CADENCE: u32 = 64;

/// Write a checkpoint snapshot for a new Block whose state is `entries` -- the exact entries its
/// state root hashes -- and return the manifest Blob's id, for `snapshot_blob_ref` before the Block
/// is encoded and signed. Every write goes through `object_store`'s ordinary object write, which
/// takes the object-store lock.
///
/// **Content Blobs first** (RFC 136 increment 1a review, ruling 1). A file entry's `blob_id` is
/// normally a stored Blob, but a text file edited by `EditText` names content no one stored (DC-65).
/// For each entry whose Blob the store lacks, the bytes come from `edited_text` (the derivation's
/// own text cache) and are written as the same unsigned schema-1 Text Blob a fresh create writes. The
/// written id must equal the entry's `blob_id`; a mismatch is refused as `Integrity`, never a snapshot
/// written anyway. A binary entry's Blob is always stored by the patch that set it, so its absence is
/// `Integrity` too. Symlink entries carry their target inline and need nothing stored.
pub(crate) fn write_checkpoint_snapshot(
    object_store: &mut (impl ObjectReader + ObjectWriter),
    entries: Vec<StateRootEntry>,
    edited_text: impl Fn(&NodeId) -> Option<Vec<u8>>,
) -> Result<ObjectId> {
    for entry in &entries {
        let StateRootContent::Blob(blob_id) = &entry.content else {
            continue;
        };
        let blob_id = *blob_id;
        if object_store
            .read_typed(blob_id, ObjectType::Blob)?
            .is_some()
        {
            continue;
        }
        let path = entry.path.as_str();
        let bytes = match entry.kind {
            NodeKind::TextFile => edited_text(&entry.node_id).ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "checkpoint for {path}: Blob {blob_id} is not stored and replay holds no \
                     text for its node"
                ))
            })?,
            NodeKind::BinaryFile | NodeKind::Symlink => {
                return Err(PrikkError::Integrity(format!(
                    "checkpoint for {path}: {:?} Blob {blob_id} is not stored",
                    entry.kind
                )));
            }
        };
        let envelope = ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            BlobPayload::new(BlobKind::Text, bytes).to_canonical_bytes()?,
        );
        let actual = envelope.object_id();
        if actual != blob_id {
            return Err(PrikkError::Integrity(format!(
                "checkpoint content for {path} hashes to Blob {actual}, but its state entry names \
                 {blob_id}"
            )));
        }
        object_store.write_object(&envelope)?;
    }
    let content = SnapshotManifest { entries }.encode()?;
    let envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        BlobPayload::new(BlobKind::Snapshot, content).to_canonical_bytes()?,
    );
    object_store.write_object(&envelope)
}

/// A decoded snapshot manifest: one Block's state entries, in canonical path order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotManifest {
    /// State entries, exactly as the Block's state root hashes them.
    pub entries: Vec<StateRootEntry>,
}

impl SnapshotManifest {
    /// Decode a manifest from a `SNAPSHOT` Blob's content. Every failure is `Integrity`: a snapshot
    /// that does not decode is damage, whatever byte was wrong.
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        let Some(mut rest) = bytes.strip_prefix(SNAPSHOT_MAGIC) else {
            return Err(PrikkError::Integrity(format!(
                "snapshot manifest magic `{}` is not PRIKK-SNAPSHOT-MANIFEST-v2",
                magic_found(bytes)
            )));
        };
        let mut entries = Vec::new();
        while !rest.is_empty() {
            let (entry, after) = decode_state_leaf_fields(rest).map_err(not_decodable)?;
            entries.push(entry);
            rest = after;
        }
        validate_entries(&entries).map_err(not_decodable)?;
        Ok(Self { entries })
    }

    /// Encode the manifest. Refuses entries the state root itself would refuse.
    pub fn encode(&self) -> Result<Vec<u8>> {
        validate_entries(&self.entries)?;
        let mut out = SNAPSHOT_MAGIC.to_vec();
        for entry in &self.entries {
            out.extend_from_slice(&state_leaf_fields(entry)?);
        }
        Ok(out)
    }

    /// The state root these entries hash to. A reader compares it with the carrying Block's
    /// `state_merkle_root` before trusting a single entry (`load_block_snapshot`).
    pub fn recomputed_state_root(&self) -> Result<MerkleRoot> {
        compute_state_root(&self.entries)
    }
}

/// The first line of `bytes` (at most 64 bytes of it), for naming a wrong magic.
fn magic_found(bytes: &[u8]) -> String {
    let line = bytes
        .split(|&byte| byte == b'\n')
        .next()
        .unwrap_or_default();
    let shown = line.get(..64).unwrap_or(line);
    String::from_utf8_lossy(shown).escape_debug().to_string()
}

fn not_decodable(err: PrikkError) -> PrikkError {
    match err {
        PrikkError::Integrity(_) => err,
        other => PrikkError::Integrity(format!("snapshot manifest does not decode: {other}")),
    }
}

/// One file of a loaded snapshot: its state entry, and its content read by Blob id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotFile {
    pub(crate) path: String,
    pub(crate) node_id: NodeId,
    pub(crate) kind: NodeKind,
    pub(crate) mode: u32,
    pub(crate) blob_id: ObjectId,
    pub(crate) bytes: Vec<u8>,
}

/// Check `block`'s snapshot, or return `None` when it carries none -- the one validator the loader,
/// `verify` and bundle export share (RFC 136 increment 1b), so each checks the same things in the
/// same order:
///
/// 1. the snapshot Blob exists and is a `SNAPSHOT` Blob, and its manifest decodes as v2;
/// 2. the manifest recomputes to `block.state_merkle_root` (§10.1: a snapshot that does not describe
///    its own block is damage);
/// 3. every content Blob a file entry names is present -- asked of the store's index, never read.
///
/// All three are `Integrity`. Step 3 comes after step 2, so a forged manifest never directs a single
/// presence check.
pub(crate) fn validate_snapshot_manifest(
    reader: &impl ObjectReader,
    block_id: ObjectId,
    block: &BlockPayload,
) -> Result<Option<SnapshotManifest>> {
    let Some(snapshot_blob_id) = block.snapshot_blob_ref else {
        return Ok(None);
    };
    let envelope = reader
        .read_typed(snapshot_blob_id, ObjectType::Blob)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "snapshot Blob {snapshot_blob_id} of Block {block_id} is missing"
            ))
        })?;
    let manifest = SnapshotManifest::decode(&decode_snapshot_blob(&envelope.canonical_payload)?)?;
    if manifest.recomputed_state_root()? != block.state_merkle_root {
        return Err(PrikkError::Integrity(format!(
            "snapshot manifest does not recompute to its block's state root (Block {block_id}, \
             snapshot Blob {snapshot_blob_id})"
        )));
    }
    for entry in &manifest.entries {
        if let StateRootContent::Blob(blob_id) = &entry.content {
            if !reader.has_object(*blob_id, ObjectType::Blob)? {
                return Err(PrikkError::Integrity(format!(
                    "snapshot of Block {block_id} names Blob {blob_id} for {}, which is missing",
                    entry.path.as_str()
                )));
            }
        }
    }
    Ok(Some(manifest))
}

/// Load `block`'s snapshot for `checkout --snapshot-*`, or `None` when it carries none: validated by
/// [`validate_snapshot_manifest`], then each file's bytes read by Blob id and its kind matched to the
/// entry (`Integrity` on a mismatch). A symlink entry is refused as unsupported, the same refusal
/// replay gives a symlink operation.
pub(crate) fn load_block_snapshot(
    reader: &impl ObjectReader,
    block_id: ObjectId,
    block: &BlockPayload,
) -> Result<Option<Vec<SnapshotFile>>> {
    let Some(manifest) = validate_snapshot_manifest(reader, block_id, block)? else {
        return Ok(None);
    };
    let mut files = Vec::with_capacity(manifest.entries.len());
    for entry in manifest.entries {
        let path = entry.path.as_str().to_string();
        let StateRootContent::Blob(blob_id) = entry.content else {
            return Err(PrikkError::UnsupportedObjectType(format!(
                "snapshot of Block {block_id} holds symlink {path}; snapshot readers support the \
                 file-level subset only"
            )));
        };
        let content = reader
            .read_typed(blob_id, ObjectType::Blob)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "snapshot of Block {block_id} names Blob {blob_id} for {path}, which is missing"
                ))
            })?;
        let (blob_kind, bytes) = decode_file_content_blob_with_kind(&content.canonical_payload)?;
        if blob_kind != entry.kind {
            return Err(PrikkError::Integrity(format!(
                "snapshot of Block {block_id} records {path} as {:?}, but its Blob {blob_id} is \
                 {blob_kind:?}",
                entry.kind
            )));
        }
        files.push(SnapshotFile {
            path,
            node_id: entry.node_id,
            kind: entry.kind,
            mode: entry.mode,
            blob_id,
            bytes,
        });
    }
    Ok(Some(files))
}

#[cfg(test)]
mod tests;
