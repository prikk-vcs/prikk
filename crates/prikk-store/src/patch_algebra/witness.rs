use prikk_object::NodeId;

use crate::node::node_lifecycle::NodeLifecycleState;
use crate::patch_replay::decode::DecodedPatchOperation;
use crate::path::RepoPath;

use super::types::{
    Action, ConflictWitness, ConflictWitnessKind, OperationFacts, PairClass, RequiredOrder,
    UnknownReason,
};

pub(super) fn ordered(
    required_order: RequiredOrder,
    kind: ConflictWitnessKind,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: Option<NodeId>,
    path: Option<RepoPath>,
) -> PairClass {
    PairClass::OrderedDependency {
        required_order,
        witness: witness(kind, left.op_seq, right.op_seq, node_id, path, None),
    }
}

pub(super) fn conflict(
    kind: ConflictWitnessKind,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: Option<NodeId>,
    path: Option<RepoPath>,
) -> PairClass {
    PairClass::Conflict {
        witness: witness(kind, left.op_seq, right.op_seq, node_id, path, None),
    }
}

pub(super) fn conflict_with_span(
    kind: ConflictWitnessKind,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: NodeId,
    path: Option<RepoPath>,
    span_id: [u8; 32],
) -> PairClass {
    PairClass::Conflict {
        witness: witness(
            kind,
            left.op_seq,
            right.op_seq,
            Some(node_id),
            path,
            Some(span_id),
        ),
    }
}

pub(super) fn unknown(
    reason: UnknownReason,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
    node_id: Option<NodeId>,
    path: Option<RepoPath>,
) -> PairClass {
    PairClass::Unknown {
        reason,
        witness: witness(
            match reason {
                UnknownReason::MalformedOperation => ConflictWitnessKind::MalformedOperation,
                UnknownReason::RenameDeferred | UnknownReason::SymlinkDeferred => {
                    ConflictWitnessKind::UnsupportedOperation
                }
                _ => ConflictWitnessKind::UnknownRelation,
            },
            left.op_seq,
            right.op_seq,
            node_id,
            path,
            None,
        ),
    }
}

pub(super) fn unknown_from_facts(
    reason: UnknownReason,
    left: &OperationFacts,
    right: &OperationFacts,
    node_id: Option<NodeId>,
    path: Option<RepoPath>,
) -> PairClass {
    PairClass::Unknown {
        reason,
        witness: witness(
            ConflictWitnessKind::UnknownRelation,
            left.op_seq,
            right.op_seq,
            node_id,
            path,
            None,
        ),
    }
}

/// Conflict-witness-path-derivation handoff v1, §2/§3: the single shared path for a pair of
/// operands, when one exists. Not `PathEffects` alone -- `facts::operation_facts` builds
/// `PathEffects` with no baseline access, so `EditText`/`ReplaceBinary`/`ChangePerm` (whose
/// `Action` variant carries no path) get an empty `PathEffects` no matter what. Their path, when
/// they have one, is only knowable by asking the baseline what node `node_id` currently lives at.
///
/// One path, not one per side (§3): when both operands resolve to the same path, or only one
/// resolves at all, that is unambiguously *the* path. When they resolve to two different paths
/// (`NodeIdReuse`: two `CreateFile`s at different paths reusing one node id), neither is "the"
/// path -- and `MergeEvidenceDisplayItem::operation.path`/`peer_operation.path` already expose
/// each side's own recorded path independently for exactly the operation kinds that can disagree
/// (`CreateFile`/`DeleteNode`/`CreateSymlink` all carry a path in the raw operation), so nothing
/// is lost by this field staying `None` there.
///
/// **Callers of `invalid_preimage_class` (`preimage.rs`) do not use this.** Its `subject`/`peer`
/// pairing is a per-operand baseline defect, not a relation between two paths -- pairwise
/// derivation would wrongly go to `None` whenever the unrelated peer's own path differs from
/// `subject`'s. `PreimageStatus`'s own `path` field (already correctly subject-scoped at every
/// `validate_preimage`/`validate_text_preimage` site) is the right value there instead.
pub(super) fn derive_path(
    baseline: &NodeLifecycleState,
    left: &OperationFacts,
    right: &OperationFacts,
) -> Option<RepoPath> {
    match (operand_path(baseline, left), operand_path(baseline, right)) {
        (Some(left_path), Some(right_path)) if left_path == right_path => Some(left_path),
        (Some(left_path), None) => Some(left_path),
        (None, Some(right_path)) => Some(right_path),
        _ => None,
    }
}

/// A single operand's own path, independent of its peer. Used directly (not through
/// [`derive_path`]) when the peer is unrelated -- e.g. the deferred-operation check in
/// `classify.rs`, where a `CreateSymlink` paired with a completely unrelated `CreateFile` at a
/// different path should still report the symlink's own path, not `None` from an irrelevant
/// disagreement with an operand it shares no path or node with.
pub(super) fn operand_path(
    baseline: &NodeLifecycleState,
    facts: &OperationFacts,
) -> Option<RepoPath> {
    match &facts.action {
        Action::CreateFile { path, .. }
        | Action::DeleteFile { path, .. }
        | Action::DeleteSymlink { path, .. }
        | Action::CreateSymlink { path, .. } => Some(path.clone()),
        // RFC 144 §4o.5: the destination, mirroring `CreateFile`'s own "where it is being
        // created" -- the analogous "where it is heading" for a rename. Reachable now: a
        // `RenamePath` no longer always reaches `unknown()` before a witness is built (see
        // `classify.rs`'s own rename-only-deferred branch), so this operand's own path is a real
        // input to `derive_path`, not dead code.
        Action::RenamePath { new_path, .. } => Some(new_path.clone()),
        Action::EditText { node_id, .. }
        | Action::ReplaceBinary { node_id, .. }
        | Action::ChangePerm { node_id, .. } => {
            baseline.live_node(node_id).map(|live| live.path.clone())
        }
    }
}

fn witness(
    kind: ConflictWitnessKind,
    left_op_seq: u32,
    right_op_seq: u32,
    node_id: Option<NodeId>,
    path: Option<RepoPath>,
    text_span: Option<[u8; 32]>,
) -> ConflictWitness {
    ConflictWitness {
        kind,
        left_op_seq,
        right_op_seq,
        node_id,
        path,
        text_span,
    }
}

pub(super) fn common_node(left: &OperationFacts, right: &OperationFacts) -> Option<NodeId> {
    match (left.node_id, right.node_id) {
        (Some(left_id), Some(right_id)) if left_id == right_id => Some(left_id),
        _ => None,
    }
}
