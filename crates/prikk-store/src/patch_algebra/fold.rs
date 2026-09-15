//! Per-side net-effect folding for merge evidence (DC-75 two-edits handoff §6, ruling R1).
//!
//! The confluence engine judges every operation against the baseline, while authoring sequences a
//! side's operations on one node: each is authored against its predecessor's result (RFC 134 §7.4
//! item 1). So a side that touches a node twice cannot be judged operation by operation. Folding
//! rewrites each side, **for evidence only**, into one baseline-relative operation per node where
//! that is expressible, and the rest of the engine runs unchanged over the result.
//!
//! - **Fold kinds:** on one file node, any run of `EditText`, `ChangePerm` and `ReplaceBinary` (mixed
//!   kinds are ruling R6), optionally ending in a `DeleteNode`. Its net is an `EditText` or
//!   `ReplaceBinary` if the content changed plus a `ChangePerm` if the mode changed, or one `DeleteNode`
//!   of the baseline content if the run ends in a delete. Nothing else folds: renames and symlinks stay
//!   deferred, and create-then-delete never folds (R7: the run holds a path for part of the side).
//! - **A net no-op** (the run restores the baseline) drops the node from the side, but only when the
//!   other side has no operation on that node (handoff §7.2); otherwise that run is judged as authored.
//! - **The guard:** a fold is used only when replaying the folded side reproduces replaying the
//!   original side ([`equivalent`]). Otherwise the original sequence is judged, exactly as before.
//! - **Deterministic:** nodes are visited in `NodeId` order and the output keeps original order.
//!
//! Merge execution never sees a fold: it adopts the original patches verbatim.
//!
//! **Admissibility (handoff §7.2).** The evidence judges folds, but execution replays a side's
//! *originals* onto the *other* side's tip. So a fold may let a merge through only when the other side
//! has no operation on the folded node and the run changes no path before its end: then the originals
//! replay onto that tip exactly as they did on their own side. Every fold kind here changes no path
//! before its end. A folded operation kept in the sequence on a node the other side touches forms a
//! same-node pair, which the engine judges and refuses. A dropped node forms no pair at all, so a drop
//! is the one fold that must check the other side itself.

use std::collections::{BTreeMap, BTreeSet};

use prikk_object::{NodeId, NodeKind};

use super::evidence_types::{Evidence, EvidenceScope, PatchAlgebraEvidence};
use super::facts::operation_facts;
use super::replay_oracle::{OracleState, replay_operations};
use crate::node::node_lifecycle::{NodeContent, NodeLifecycleState};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation,
};
use crate::text_span;

/// One side as the engine judges it, with the way back to the original sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct FoldedSide {
    pub(super) operations: Vec<DecodedPatchOperation>,
    /// For each operation in `operations`, the first and last original index it stands for
    /// (equal for an operation that was not folded).
    pub(super) origins: Vec<(usize, usize)>,
}

impl FoldedSide {
    fn unfolded(operations: &[DecodedPatchOperation]) -> Self {
        Self {
            operations: operations.to_vec(),
            origins: (0..operations.len()).map(|index| (index, index)).collect(),
        }
    }
}

enum Net {
    /// One or two operations: content then mode, or a single delete.
    Replace(Vec<DecodedPatchOperation>),
    Drop,
}

/// Fold `operations` (one side) against `baseline`, or return it unfolded when no fold applies or
/// the guard refuses one. Callers validate every original operation first (R1 condition 1).
/// `other_side` is the opposite side's original sequence, consulted only for which nodes it touches.
pub(super) fn fold_side<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    operations: &[DecodedPatchOperation],
    other_side: &[DecodedPatchOperation],
) -> FoldedSide {
    fold(baseline, evidence, candidate_scope, operations, other_side)
        .unwrap_or_else(|| FoldedSide::unfolded(operations))
}

fn fold<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    candidate_scope: EvidenceScope,
    operations: &[DecodedPatchOperation],
    other_side: &[DecodedPatchOperation],
) -> Option<FoldedSide> {
    let mut by_node: BTreeMap<NodeId, Vec<usize>> = BTreeMap::new();
    for (index, operation) in operations.iter().enumerate() {
        let node_id = operation_facts(operation).ok()?.node_id?;
        by_node.entry(node_id).or_default().push(index);
    }
    if by_node.values().all(|indices| indices.len() < 2) {
        return None;
    }
    let original = replay_operations(baseline, evidence, candidate_scope, operations).ok()?;
    // `None` when an operation of the other side names no node: then no drop is admissible.
    let other_nodes: Option<BTreeSet<NodeId>> = other_side
        .iter()
        .map(|operation| {
            operation_facts(operation)
                .ok()
                .and_then(|facts| facts.node_id)
        })
        .collect();

    // Keyed by the last original index of each folded run: the folded operation takes that position,
    // so it stays after anything the run's last operation followed.
    let mut nets: BTreeMap<usize, (usize, Net)> = BTreeMap::new();
    let mut consumed = BTreeSet::new();
    let mut folded_deletes = BTreeSet::new();
    for (node_id, indices) in &by_node {
        let (Some(&first), Some(&last)) = (indices.first(), indices.last()) else {
            continue;
        };
        if first == last {
            continue;
        }
        let run: Vec<&DecodedPatchOperation> = indices
            .iter()
            .filter_map(|index| operations.get(*index))
            .collect();
        let Some(net) = net_effect(baseline, evidence, &original, *node_id, &run) else {
            continue;
        };
        let other_leaves_node_alone = other_nodes
            .as_ref()
            .is_some_and(|nodes| !nodes.contains(node_id));
        if matches!(net, Net::Drop) && !other_leaves_node_alone {
            continue;
        }
        if matches!(&net, Net::Replace(replacements)
            if replacements
                .iter()
                .any(|replacement| matches!(replacement.kind, DecodedOperationKind::DeleteNode { .. })))
        {
            folded_deletes.insert(*node_id);
        }
        consumed.extend(indices.iter().copied());
        nets.insert(last, (first, net));
    }
    if nets.is_empty() {
        return None;
    }

    let mut folded = FoldedSide {
        operations: Vec::new(),
        origins: Vec::new(),
    };
    for (index, operation) in operations.iter().enumerate() {
        if let Some((first, net)) = nets.remove(&index) {
            if let Net::Replace(replacements) = net {
                for replacement in replacements {
                    folded.operations.push(replacement);
                    folded.origins.push((first, index));
                }
            }
        } else if !consumed.contains(&index) {
            folded.operations.push(operation.clone());
            folded.origins.push((index, index));
        }
    }
    let replayed =
        replay_operations(baseline, evidence, candidate_scope, &folded.operations).ok()?;
    equivalent(&original, &replayed, &folded_deletes).then_some(folded)
}

/// The baseline-relative operations a run on `node_id` amounts to, or `None` when the run is not a
/// fold kind or its net effect cannot be expressed.
fn net_effect<R: PatchAlgebraEvidence>(
    baseline: &NodeLifecycleState,
    evidence: &R,
    original: &OracleState,
    node_id: NodeId,
    run: &[&DecodedPatchOperation],
) -> Option<Net> {
    let op_seq = run.first()?.op_seq;
    let (last, before_last) = run.split_last()?;
    let base = baseline.live_node(&node_id)?;
    let NodeContent::File {
        blob_id: base_blob_id,
        mode: base_mode,
    } = base.content
    else {
        return None;
    };
    let changes_content_or_mode = |operation: &&DecodedPatchOperation| {
        matches!(
            operation.kind,
            DecodedOperationKind::EditText { .. }
                | DecodedOperationKind::ChangePerm { .. }
                | DecodedOperationKind::ReplaceBinary { .. }
        )
    };
    let operation = |kind| DecodedPatchOperation { op_seq, kind };

    if let DecodedOperationKind::DeleteNode {
        path,
        preimage: DecodedDeletePreimage::File { .. },
        ..
    } = &last.kind
    {
        if before_last.is_empty() || !before_last.iter().all(changes_content_or_mode) {
            return None;
        }
        return Some(Net::Replace(vec![operation(
            DecodedOperationKind::DeleteNode {
                path: path.clone(),
                node_id,
                preimage: DecodedDeletePreimage::File {
                    old_node_kind: base.kind,
                    old_blob_id: base_blob_id,
                    old_mode: base_mode,
                },
            },
        )]));
    }
    if !run.iter().all(changes_content_or_mode) {
        return None;
    }

    let NodeContent::File {
        blob_id: final_blob_id,
        mode: final_mode,
    } = original.lifecycle.live_node(&node_id)?.content
    else {
        return None;
    };
    let mut net = Vec::new();
    match base.kind {
        NodeKind::TextFile => {
            let Evidence::Known(base_text) = evidence.baseline_text(
                EvidenceScope::SealedBaselineRequired,
                node_id,
                base_blob_id,
            ) else {
                return None;
            };
            // A run with no edit materializes no text: the content is the baseline's.
            let final_text = original.text(&node_id).unwrap_or(base_text.as_slice());
            if final_text != base_text.as_slice() {
                let plan =
                    text_span::plan_authored_text_span(&base_text, final_text, node_id).ok()??;
                net.push(operation(DecodedOperationKind::EditText {
                    node_id,
                    span_id: plan.span_id,
                    old_span_hash: plan.old_span_hash,
                    left_anchor_hash: plan.left_anchor_hash,
                    right_anchor_hash: plan.right_anchor_hash,
                    replacement_text: plan.replacement_text,
                    old_span_text: plan.old_span_text,
                    left_anchor_len: Some(plan.left_anchor_len),
                    right_anchor_len: Some(plan.right_anchor_len),
                }));
            }
        }
        NodeKind::BinaryFile => {
            if final_blob_id != base_blob_id {
                net.push(operation(DecodedOperationKind::ReplaceBinary {
                    node_id,
                    old_blob_id: base_blob_id,
                    new_blob_id: final_blob_id,
                }));
            }
        }
        _ => return None,
    }
    if final_mode != base_mode {
        net.push(operation(DecodedOperationKind::ChangePerm {
            node_id,
            old_mode: base_mode,
            new_mode: final_mode,
        }));
    }
    Some(if net.is_empty() {
        Net::Drop
    } else {
        Net::Replace(net)
    })
}

/// The guard (R1 condition 2): the folded side's replay equals the original side's.
///
/// - Live nodes (path, kind, content) must be equal.
/// - Tombstones must be equal, with one exception: a delete folded here names the **baseline** content
///   as its preimage, since that is what it replays against, while the original delete named the
///   content the run left. So for exactly those nodes, the tombstone's blob id and mode may differ
///   (R6); its path and kind may not.
/// - Materialized texts must agree. A text present on one side only must be the content of the node
///   the equal live state holds, or belong to a node that is no longer live.
///
/// Seen node ids need no separate check: a fold replaces operations on a node with operations on the
/// same node, and drops a node only when it was live in the baseline, so both replays see the same ids.
fn equivalent(
    original: &OracleState,
    folded: &OracleState,
    folded_deletes: &BTreeSet<NodeId>,
) -> bool {
    let (left, right) = (&original.lifecycle, &folded.lifecycle);
    if !left.live_nodes().eq(right.live_nodes()) {
        return false;
    }
    let left_tombstones: BTreeMap<_, _> = left.tombstones().collect();
    let right_tombstones: BTreeMap<_, _> = right.tombstones().collect();
    if left_tombstones.len() != right_tombstones.len() {
        return false;
    }
    for (node_id, left_tombstone) in &left_tombstones {
        let Some(right_tombstone) = right_tombstones.get(node_id) else {
            return false;
        };
        if left_tombstone == right_tombstone {
            continue;
        }
        let both_files = matches!(
            (&left_tombstone.content, &right_tombstone.content),
            (NodeContent::File { .. }, NodeContent::File { .. })
        );
        if !folded_deletes.contains(*node_id)
            || left_tombstone.kind != right_tombstone.kind
            || left_tombstone.path != right_tombstone.path
            || !both_files
        {
            return false;
        }
    }
    let text_nodes: BTreeSet<&NodeId> = original.text_nodes().chain(folded.text_nodes()).collect();
    text_nodes.into_iter().all(
        |node_id| match (original.text(node_id), folded.text(node_id)) {
            (Some(left_text), Some(right_text)) => left_text == right_text,
            (Some(text), None) | (None, Some(text)) => match left.live_node(node_id) {
                None => true,
                Some(live) => match live.content {
                    NodeContent::File { blob_id, .. } if live.kind == NodeKind::TextFile => {
                        text_span::text_blob_id(text).ok() == Some(blob_id)
                    }
                    _ => false,
                },
            },
            (None, None) => true,
        },
    )
}
