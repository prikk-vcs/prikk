//! Canonical format-2 clean-state Merkle authority.

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;
use prikk_object::{MerkleRoot, NodeId, NodeKind, ObjectId};

use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::path::{RepoPath, validate_no_path_collisions};

const LEAF_DOMAIN: &[u8] = b"PRIKK-STATE-LEAF-v2";
const NODE_DOMAIN: &[u8] = b"PRIKK-STATE-NODE-v2";
const ROOT_DOMAIN: &[u8] = b"PRIKK-STATE-ROOT-v2";
const REGULAR_MODE: u32 = 0o100644;
const EXECUTABLE_MODE: u32 = 0o100755;

/// Content identity committed by one canonical clean-state entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StateRootContent {
    /// Schema-1 Blob object identity for a text or binary file.
    Blob(ObjectId),
    /// Exact opaque schema-1 UTF-8 symlink target.
    Symlink(String),
}

/// One canonical format-2 clean-state entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StateRootEntry {
    /// Canonical repository path.
    pub path: RepoPath,
    /// Nonzero stable node identity.
    pub node_id: NodeId,
    /// Text, binary, or symlink node kind.
    pub kind: NodeKind,
    /// Normalized file mode; symlinks require zero.
    pub mode: u32,
    /// Blob identity or exact symlink target.
    pub content: StateRootContent,
}

/// Construct the exact format-2 leaf preimage for one validated entry.
pub fn state_leaf_preimage(entry: &StateRootEntry) -> Result<Vec<u8>> {
    let fields = state_leaf_fields(entry)?;
    let mut preimage = Vec::with_capacity(LEAF_DOMAIN.len() + fields.len());
    preimage.extend_from_slice(LEAF_DOMAIN);
    preimage.extend_from_slice(&fields);
    Ok(preimage)
}

/// One validated entry in the leaf grammar, without the leaf domain tag: path length (u32 BE), path,
/// node id (32 bytes), kind code (u16 BE), mode (u32 BE), content length (u64 BE), content (the Blob
/// id's bytes or the symlink target). A snapshot manifest (RFC 136 §10.1) is these fields for each
/// entry, so the manifest and the state root it recomputes to share this one encoder.
pub(crate) fn state_leaf_fields(entry: &StateRootEntry) -> Result<Vec<u8>> {
    validate_entry(entry)?;
    let path = entry.path.as_str().as_bytes();
    let path_len = u32::try_from(path.len())
        .map_err(|_| PrikkError::Integrity("state-root path length exceeds u32".to_string()))?;
    let content = content_bytes(entry);
    let content_len = u64::try_from(content.len())
        .map_err(|_| PrikkError::Integrity("state-root content length exceeds u64".to_string()))?;
    let mut fields = Vec::with_capacity(4 + path.len() + 32 + 2 + 4 + 8 + content.len());
    fields.extend_from_slice(&path_len.to_be_bytes());
    fields.extend_from_slice(path);
    fields.extend_from_slice(entry.node_id.as_bytes());
    fields.extend_from_slice(&entry.kind.code().to_be_bytes());
    fields.extend_from_slice(&entry.mode.to_be_bytes());
    fields.extend_from_slice(&content_len.to_be_bytes());
    fields.extend_from_slice(content);
    Ok(fields)
}

/// Decode one entry written by [`state_leaf_fields`] from the front of `bytes`, returning it and the
/// bytes after it. The entry is validated exactly as the encoder validates it; ordering across
/// entries is the caller's, via [`validate_entries`].
pub(crate) fn decode_state_leaf_fields(bytes: &[u8]) -> Result<(StateRootEntry, &[u8])> {
    let (path_len, rest) = take_array::<4>(bytes)?;
    let path_len = usize::try_from(u32::from_be_bytes(path_len))
        .map_err(|_| PrikkError::Integrity("state entry path length exceeds usize".to_string()))?;
    let (path, rest) = take(rest, path_len)?;
    let path = std::str::from_utf8(path)
        .map_err(|_| PrikkError::Integrity("state entry path is not UTF-8".to_string()))?;
    let path = RepoPath::parse(path)?;
    let (node_id, rest) = take_array::<32>(rest)?;
    let (kind, rest) = take_array::<2>(rest)?;
    let kind = NodeKind::from_code(u16::from_be_bytes(kind))?;
    let (mode, rest) = take_array::<4>(rest)?;
    let (content_len, rest) = take_array::<8>(rest)?;
    let content_len = usize::try_from(u64::from_be_bytes(content_len)).map_err(|_| {
        PrikkError::Integrity("state entry content length exceeds usize".to_string())
    })?;
    let (content, rest) = take(rest, content_len)?;
    let content = match kind {
        NodeKind::Symlink => StateRootContent::Symlink(
            std::str::from_utf8(content)
                .map_err(|_| {
                    PrikkError::Integrity("state entry symlink target is not UTF-8".to_string())
                })?
                .to_string(),
        ),
        NodeKind::TextFile | NodeKind::BinaryFile => {
            let blob_id: [u8; 32] = content.try_into().map_err(|_| {
                PrikkError::Integrity("state entry Blob id is not 32 bytes".to_string())
            })?;
            StateRootContent::Blob(ObjectId::from_bytes(blob_id))
        }
    };
    let entry = StateRootEntry {
        path,
        node_id: NodeId::from_bytes(node_id),
        kind,
        mode: u32::from_be_bytes(mode),
        content,
    };
    validate_entry(&entry)?;
    Ok((entry, rest))
}

fn take(bytes: &[u8], len: usize) -> Result<(&[u8], &[u8])> {
    if bytes.len() < len {
        return Err(PrikkError::Integrity(
            "state entry ends before its declared length".to_string(),
        ));
    }
    Ok(bytes.split_at(len))
}

fn take_array<const N: usize>(bytes: &[u8]) -> Result<([u8; N], &[u8])> {
    let (head, rest) = take(bytes, N)?;
    let mut out = [0_u8; N];
    out.copy_from_slice(head);
    Ok((out, rest))
}

/// Hash one validated canonical format-2 clean-state entry.
pub fn state_leaf_hash(entry: &StateRootEntry) -> Result<[u8; 32]> {
    Ok(sha256(&state_leaf_preimage(entry)?))
}

/// Compute the format-2 state root from entries in strict canonical path order.
pub fn compute_state_root(entries: &[StateRootEntry]) -> Result<MerkleRoot> {
    validate_entries(entries)?;
    let count = u64::try_from(entries.len())
        .map_err(|_| PrikkError::Integrity("state-root entry count exceeds u64".to_string()))?;
    if entries.is_empty() {
        let mut preimage = Vec::with_capacity(ROOT_DOMAIN.len() + 8);
        preimage.extend_from_slice(ROOT_DOMAIN);
        preimage.extend_from_slice(&count.to_be_bytes());
        return Ok(MerkleRoot(sha256(&preimage)));
    }
    let mut level = entries
        .iter()
        .map(state_leaf_hash)
        .collect::<Result<Vec<_>>>()?;
    while level.len() > 1 {
        let mut next = Vec::with_capacity(level.len().div_ceil(2));
        for pair in level.chunks(2) {
            match pair {
                [left, right] => {
                    let mut preimage = Vec::with_capacity(NODE_DOMAIN.len() + 64);
                    preimage.extend_from_slice(NODE_DOMAIN);
                    preimage.extend_from_slice(left);
                    preimage.extend_from_slice(right);
                    next.push(sha256(&preimage));
                }
                [single] => next.push(*single),
                _ => {}
            }
        }
        level = next;
    }
    let top = level.first().ok_or_else(|| {
        PrikkError::Integrity("non-empty state-root reduction produced no hash".to_string())
    })?;
    let mut preimage = Vec::with_capacity(ROOT_DOMAIN.len() + 8 + 32);
    preimage.extend_from_slice(ROOT_DOMAIN);
    preimage.extend_from_slice(&count.to_be_bytes());
    preimage.extend_from_slice(top);
    Ok(MerkleRoot(sha256(&preimage)))
}

pub(crate) fn entries_from_state(state: &NodeLifecycleState) -> Result<Vec<StateRootEntry>> {
    let mut entries = state
        .live_nodes()
        .map(|(node_id, node)| {
            let (mode, content) = match &node.content {
                NodeContent::File { blob_id, mode } => (*mode, StateRootContent::Blob(*blob_id)),
                NodeContent::Symlink { target } => (0, StateRootContent::Symlink(target.clone())),
            };
            StateRootEntry {
                path: node.path.clone(),
                node_id: *node_id,
                kind: node.kind,
                mode,
                content,
            }
        })
        .collect::<Vec<_>>();
    entries.sort_by(|left, right| {
        left.path
            .as_str()
            .as_bytes()
            .cmp(right.path.as_str().as_bytes())
    });
    validate_entries(&entries)?;
    Ok(entries)
}

pub(crate) fn validate_entries(entries: &[StateRootEntry]) -> Result<()> {
    let paths = entries
        .iter()
        .map(|entry| entry.path.clone())
        .collect::<Vec<_>>();
    validate_no_path_collisions(&paths)?;
    if !entries.windows(2).all(
        |pair| matches!(pair, [left, right] if left.path.as_str().as_bytes() < right.path.as_str().as_bytes()),
    ) {
        return Err(PrikkError::Integrity(
            "state-root entries are not in strict canonical path order".to_string(),
        ));
    }
    let mut node_ids = BTreeSet::new();
    for entry in entries {
        validate_entry(entry)?;
        if !node_ids.insert(entry.node_id) {
            return Err(PrikkError::Integrity(
                "state-root entries contain a duplicate node_id".to_string(),
            ));
        }
    }
    Ok(())
}

fn validate_entry(entry: &StateRootEntry) -> Result<()> {
    if entry.node_id.is_zero() {
        return Err(PrikkError::Integrity(
            "state-root entry node_id must be nonzero".to_string(),
        ));
    }
    match (&entry.kind, &entry.content, entry.mode) {
        (
            NodeKind::TextFile | NodeKind::BinaryFile,
            StateRootContent::Blob(_),
            REGULAR_MODE | EXECUTABLE_MODE,
        ) => Ok(()),
        (NodeKind::Symlink, StateRootContent::Symlink(_), 0) => Ok(()),
        _ => Err(PrikkError::Integrity(
            "state-root entry kind, content, or normalized mode is invalid".to_string(),
        )),
    }
}

fn content_bytes(entry: &StateRootEntry) -> &[u8] {
    match &entry.content {
        StateRootContent::Blob(blob_id) => blob_id.as_bytes(),
        StateRootContent::Symlink(target) => target.as_bytes(),
    }
}

#[cfg(test)]
mod tests;
