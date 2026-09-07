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
    validate_entry(entry)?;
    let path = entry.path.as_str().as_bytes();
    let path_len = u32::try_from(path.len())
        .map_err(|_| PrikkError::Integrity("state-root path length exceeds u32".to_string()))?;
    let content = content_bytes(entry);
    let content_len = u64::try_from(content.len())
        .map_err(|_| PrikkError::Integrity("state-root content length exceeds u64".to_string()))?;
    let mut preimage =
        Vec::with_capacity(LEAF_DOMAIN.len() + 4 + path.len() + 32 + 2 + 4 + 8 + content.len());
    preimage.extend_from_slice(LEAF_DOMAIN);
    preimage.extend_from_slice(&path_len.to_be_bytes());
    preimage.extend_from_slice(path);
    preimage.extend_from_slice(entry.node_id.as_bytes());
    preimage.extend_from_slice(&entry.kind.code().to_be_bytes());
    preimage.extend_from_slice(&entry.mode.to_be_bytes());
    preimage.extend_from_slice(&content_len.to_be_bytes());
    preimage.extend_from_slice(content);
    Ok(preimage)
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

fn validate_entries(entries: &[StateRootEntry]) -> Result<()> {
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
