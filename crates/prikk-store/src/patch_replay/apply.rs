//! Operation-application logic for patch replay: folding one decoded operation into the in-memory
//! file/live-node state. Split out of `patch_replay.rs` (DC-58) — no behaviour change, all items
//! moved verbatim.

use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, NodeKind, ObjectId, text_span_hash};

use crate::object_store::ObjectReader;
use crate::path::RepoPath;
use crate::text_span;

use super::PatchReplayDeletedFile;
use super::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation, ensure_apply_supported,
};
use super::read::read_blob_bytes_with_kind;

/// One node as replay currently sees it: where it lives, what kind it is, and the blob and mode the
/// most recent operation left it with. The unit a materializing caller writes from.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReplayLiveNode {
    pub(crate) path: String,
    pub(crate) kind: NodeKind,
    /// Current mode bits, as recorded by the operation that most recently set them
    /// (`CreateFile`, then any `ChangePerm`) — DC-73. Threaded into materialization so a
    /// checked-out file's permission bits match what was authored, not the anchored write
    /// primitive's create-time default.
    pub(crate) mode: u32,
    /// The blob id backing this node's *current* content, as recorded by the operation that most
    /// recently set it (`CreateFile`, then any `ReplaceBinary`) — RFC 143 §5. Meaningful only when
    /// `kind` is `BinaryFile`: a `TextFile` node's tracked id goes stale the moment an `EditText`
    /// touches it (DC-65 — the pre-edit blob is deliberately never rewritten), so RFC 143's own
    /// content report never reads this field for text. `BinaryFile` nodes never take that path
    /// (`EditText` is text-only; binary content only ever changes via `ReplaceBinary`, which always
    /// updates this to a real, current, apply-validated blob), so for them this id never goes
    /// stale.
    pub(crate) blob_id: ObjectId,
}

pub(super) fn apply_decoded_operation(
    object_store: &impl ObjectReader,
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    deleted_files: &mut BTreeMap<String, PatchReplayDeletedFile>,
    operation: DecodedPatchOperation,
) -> Result<()> {
    // Erratum P1: decode success does not imply applicability. The apply-supported
    // subset is gated here as the single source of truth; the match below only needs
    // to handle the kinds the gate admits.
    ensure_apply_supported(&operation)?;
    match operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            if files.contains_key(&path) {
                return Err(PrikkError::Integrity(format!(
                    "CreateFile would overwrite existing path {path}"
                )));
            }
            if live_nodes.contains_key(&node_id) {
                return Err(PrikkError::Integrity(
                    "CreateFile would introduce an already-live node_id".to_string(),
                ));
            }
            let (kind, bytes) = read_blob_bytes_with_kind(object_store, blob_id)?;
            deleted_files.remove(&path);
            files.insert(path.clone(), bytes);
            live_nodes.insert(
                node_id,
                ReplayLiveNode {
                    path,
                    kind,
                    mode,
                    blob_id,
                },
            );
        }
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage:
                DecodedDeletePreimage::File {
                    old_node_kind,
                    old_blob_id,
                    old_mode: _,
                },
        } => {
            let old_bytes = files.get(&path).ok_or_else(|| {
                PrikkError::Integrity(format!("DeleteNode path is absent: {path}"))
            })?;
            crate::blob_access::ensure_blob_matches_node_kind(
                old_bytes,
                old_blob_id,
                old_node_kind,
            )?;
            let repo_path = RepoPath::parse(&path)?;
            let deleted = PatchReplayDeletedFile {
                path: repo_path,
                old_blob_id,
                old_bytes: old_bytes.clone(),
            };
            files.remove(&path);
            if let Some(live) = live_nodes.remove(&node_id) {
                if live.path != path {
                    return Err(PrikkError::Integrity(format!(
                        "DeleteNode path {path} does not match live node path {}",
                        live.path
                    )));
                }
                if live.kind != old_node_kind {
                    return Err(PrikkError::Integrity(
                        "DeleteNode old_node_kind does not match live node kind".to_string(),
                    ));
                }
            }
            deleted_files.insert(path, deleted);
        }
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            left_anchor_len,
            right_anchor_len,
        } => {
            apply_edit_text(
                files,
                live_nodes,
                node_id,
                &span_id,
                &old_span_hash,
                &left_anchor_hash,
                &right_anchor_hash,
                &replacement_text,
                &old_span_text,
                left_anchor_len,
                right_anchor_len,
            )?;
        }
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => {
            apply_replace_binary(
                object_store,
                files,
                live_nodes,
                node_id,
                old_blob_id,
                new_blob_id,
            )?;
        }
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => {
            apply_change_perm(live_nodes, node_id, old_mode, new_mode)?;
        }
        DecodedOperationKind::RenamePath { .. } => {
            return Err(PrikkError::MalformedData(
                "apply_decoded_operation received a RenamePath -- the caller must route \
                 consecutive RenamePath runs to apply_rename_batch instead, never here"
                    .to_string(),
            ));
        }
        _ => {
            return Err(PrikkError::MalformedData(
                "apply_decoded_operation received an operation kind ensure_apply_supported \
                 should have refused"
                    .to_string(),
            ));
        }
    }
    Ok(())
}

/// Apply a `ReplaceBinary` operation (DC-73): fold the node's content forward from `old_blob_id` to
/// `new_blob_id`. Node-addressed with no path field — `live_nodes` supplies the current path.
fn apply_replace_binary(
    object_store: &impl ObjectReader,
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    node_id: NodeId,
    old_blob_id: ObjectId,
    new_blob_id: ObjectId,
) -> Result<()> {
    let live = live_nodes.get(&node_id).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "ReplaceBinary target node {} is not live",
            hex32(node_id.as_bytes())
        ))
    })?;
    if live.kind != NodeKind::BinaryFile {
        return Err(PrikkError::Integrity(format!(
            "ReplaceBinary target node {} is {:?}, not BinaryFile",
            hex32(node_id.as_bytes()),
            live.kind
        )));
    }
    let current_bytes = files.get(&live.path).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "ReplaceBinary target path {} is absent for live node {}",
            live.path,
            hex32(node_id.as_bytes())
        ))
    })?;
    crate::blob_access::ensure_blob_matches_node_kind(current_bytes, old_blob_id, live.kind)?;
    let (new_kind, new_bytes) = read_blob_bytes_with_kind(object_store, new_blob_id)?;
    if new_kind != NodeKind::BinaryFile {
        return Err(PrikkError::Integrity(format!(
            "ReplaceBinary new blob {new_blob_id} is not a binary-file blob"
        )));
    }
    let path = live.path.clone();
    files.insert(path, new_bytes);
    // Safe to unwrap: the `live_nodes.get(&node_id)` lookup above already proved this key exists,
    // and nothing between there and here removes it.
    if let Some(live) = live_nodes.get_mut(&node_id) {
        live.blob_id = new_blob_id;
    }
    Ok(())
}

/// Apply a `ChangePerm` operation (DC-73): fold the node's mode forward from `old_mode` to
/// `new_mode`. Node-addressed with no path field or content change — only `live_nodes`' recorded
/// mode is affected; materialization reads it from there.
fn apply_change_perm(
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    node_id: NodeId,
    old_mode: u32,
    new_mode: u32,
) -> Result<()> {
    let live = live_nodes.get_mut(&node_id).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "ChangePerm target node {} is not live",
            hex32(node_id.as_bytes())
        ))
    })?;
    if live.mode != old_mode {
        return Err(PrikkError::Integrity(format!(
            "ChangePerm old_mode does not match live node {}'s current mode",
            hex32(node_id.as_bytes())
        )));
    }
    live.mode = new_mode;
    Ok(())
}

/// Apply one consecutive run of `RenamePath` operations (from the same patch, nothing else
/// interleaved) together, resolving node before path (RFC 144 §4h.7 / increment 1 handoff §2).
///
/// A rename cycle -- a swap, `a->b` alongside `b->a` -- applied one operation at a time collides:
/// the first half's target path is still occupied by the second half's own not-yet-moved node,
/// which is exactly the merge-ordering hazard's materialization analogue. Resolved here by
/// treating the whole run as one unit instead of a sequence: every assertion (`node_id` live,
/// its current path matches the operation's own `old_path`) is checked against the state as it
/// stood *before* this run, every source path is vacated before any target path is claimed, and
/// only *then* is occupancy checked -- so two nodes trading paths never observe an intermediate
/// collision with each other, and a path still occupied afterward belongs to a third,
/// non-participating live node: a genuine, structural collision, reported rather than silently
/// overwritten (`patch_replay/tests/rename.rs` control 3).
///
/// As of RFC 144 increment 2, `lifecycle_cache` resolves a rename run the same way: both paths now
/// apply the identical algorithm -- validate against the pre-run state, vacate every source before
/// claiming any target, then check occupancy -- each at its own state's own shape (this function
/// over `files`/`live_nodes`; `NodeLifecycleState::rename_nodes_checked_batch` over
/// `path_to_id`/`live_by_id`, in `prikk-replay/src/node_lifecycle/mutation.rs`). Neither is a port
/// of the other; each was written against its own state's own indexing.
///
/// This did not always hold. When this function was written (increment 1), `lifecycle_cache`'s own
/// `rename_node_checked` resolved by node identity (looked the node up by id, not by path) but
/// still checked occupancy against the *current*, already-partially-mutated path index on every
/// call, one operation at a time -- which did not survive a literal swap either (verified directly
/// against `NodeLifecycleState`, not assumed from reading it: a two-node swap applied through
/// `rename_node_checked` in sequence failed on the first half with exactly the collision this
/// function exists to avoid). That divergence is why increment 2 exists;
/// `rename_node_checked` has no production caller left. See that round's own report for the
/// original finding, and RFC 144 §4j/§4k for the ruling that closed the gap.
///
/// Scoped to one contiguous run within a single patch, not the whole patch and not across patch
/// boundaries: a patch is this system's own atomic unit of one signed intent, and a genuinely
/// simultaneous swap can only be meaningfully authored within one (splitting it across separately
/// sealed patches would require a real intermediate state where both nodes are momentarily at the
/// same path, which is not a swap, it is two sequential renames needing a third, temporary path).
/// A non-rename operation between two halves of an intended swap breaks the run and each half is
/// checked individually against the immediate state, exactly as today -- deliberate: nothing in
/// RFC 144 increment 1 asks for lookahead across an unrelated operation, and failing closed on
/// that unstated case is the safer default.
pub(super) fn apply_rename_batch(
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &mut BTreeMap<NodeId, ReplayLiveNode>,
    renames: Vec<DecodedPatchOperation>,
) -> Result<()> {
    if renames.is_empty() {
        return Ok(());
    }

    // Phase 0: validate every operation's own assertion against the pre-run state, and reject a
    // run that renames the same node twice or names the same destination twice -- neither is a
    // cycle this algorithm resolves, both are malformed regardless of approach.
    let mut resolved = Vec::with_capacity(renames.len());
    let mut sources_seen = std::collections::BTreeSet::new();
    let mut destinations_seen = std::collections::BTreeSet::new();
    for operation in renames {
        let DecodedOperationKind::RenamePath {
            node_id,
            old_path,
            new_path,
        } = operation.kind
        else {
            return Err(PrikkError::MalformedData(
                "apply_rename_batch received a non-RenamePath operation".to_string(),
            ));
        };
        let live = live_nodes.get(&node_id).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "RenamePath target node {} is not live",
                hex32(node_id.as_bytes())
            ))
        })?;
        if live.path != old_path {
            return Err(PrikkError::Integrity(format!(
                "RenamePath old_path {old_path} does not match live node {}'s current path {}",
                hex32(node_id.as_bytes()),
                live.path
            )));
        }
        if !sources_seen.insert(node_id) {
            return Err(PrikkError::Integrity(format!(
                "RenamePath renames node {} more than once in the same run",
                hex32(node_id.as_bytes())
            )));
        }
        if !destinations_seen.insert(new_path.clone()) {
            return Err(PrikkError::Integrity(format!(
                "RenamePath names destination path {new_path} more than once in the same run"
            )));
        }
        resolved.push((node_id, old_path, new_path));
    }

    // Phase 1: vacate every source path across the whole run before claiming any target path, so
    // a target about to be vacated by another rename in this same run never reads as occupied.
    let mut staged = Vec::with_capacity(resolved.len());
    for (node_id, old_path, new_path) in resolved {
        let bytes = files.remove(&old_path).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "RenamePath source path {old_path} has no file content"
            ))
        })?;
        staged.push((node_id, new_path, bytes));
    }

    // Phase 2: every source in this run is now vacant. A target still occupied belongs to a node
    // this run never touched -- report it rather than overwrite it.
    for (_, new_path, _) in &staged {
        if files.contains_key(new_path) {
            return Err(PrikkError::Integrity(format!(
                "RenamePath target path {new_path} is occupied by another live node"
            )));
        }
    }

    // Phase 3: commit. No further failure is possible past this point.
    for (node_id, new_path, bytes) in staged {
        files.insert(new_path.clone(), bytes);
        if let Some(live) = live_nodes.get_mut(&node_id) {
            live.path = new_path;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_edit_text(
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &BTreeMap<NodeId, ReplayLiveNode>,
    node_id: NodeId,
    span_id: &[u8; 32],
    old_span_hash: &[u8; 32],
    left_anchor_hash: &[u8; 32],
    right_anchor_hash: &[u8; 32],
    replacement_text: &[u8],
    old_span_text: &[u8],
    left_anchor_len: Option<u32>,
    right_anchor_len: Option<u32>,
) -> Result<()> {
    if text_span_hash(old_span_text) != *old_span_hash {
        return Err(PrikkError::Integrity(format!(
            "EditText hash verification failed before localization for node {} span {}",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        )));
    }
    let live = live_nodes.get(&node_id).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "EditText failed before blob load: node {} is not live (span {})",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        ))
    })?;
    if live.kind != NodeKind::TextFile {
        return Err(PrikkError::Integrity(format!(
            "EditText failed before blob load: node {} is {:?}, not TextFile (span {})",
            hex32(node_id.as_bytes()),
            live.kind,
            hex32(span_id)
        )));
    }
    let current_text = files.get(&live.path).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "EditText failed before blob load: live node {} path {} is absent (span {})",
            hex32(node_id.as_bytes()),
            live.path,
            hex32(span_id)
        ))
    })?;
    if core::str::from_utf8(current_text).is_err() {
        return Err(PrikkError::Integrity(format!(
            "EditText failed during UTF-8 validation for node {} span {}",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        )));
    }
    let (start, end) = text_span::resolve_text_span(
        current_text,
        old_span_text,
        left_anchor_hash,
        right_anchor_hash,
        span_id,
        node_id,
        old_span_hash,
        left_anchor_len,
        right_anchor_len,
    )
    .map_err(|reason| {
        PrikkError::Integrity(format!(
            "EditText failed during localization for node {} span {}: {reason}",
            hex32(node_id.as_bytes()),
            hex32(span_id)
        ))
    })?;
    let new_text =
        text_span::splice_text(current_text, start, end, replacement_text).map_err(|err| {
            PrikkError::Integrity(format!(
                "EditText failed during splice for node {} span {}: {err}",
                hex32(node_id.as_bytes()),
                hex32(span_id)
            ))
        })?;
    files.insert(live.path.clone(), new_text);
    Ok(())
}

fn hex32(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
