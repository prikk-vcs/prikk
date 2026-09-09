use prikk_object::{NodeId, ObjectId};

use crate::patch_algebra::{
    ConflictWitnessKind, MergeEvidenceItem, MergeEvidenceOperationKind, MergeEvidenceOutcome,
    MergeEvidenceProofPhase, MergeEvidenceReasonCode, MergeEvidenceReport, MergeEvidenceScope,
    MergeEvidenceSide,
};

/// Public display view for read-only merge/conflict evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeEvidenceDisplay {
    /// Explicit sealed baseline block.
    pub baseline_block_id: ObjectId,
    /// Submitted left selector plus resolved target block.
    pub left_selector: MergeEvidenceDisplaySelector,
    /// Submitted right selector plus resolved target block.
    pub right_selector: MergeEvidenceDisplaySelector,
    /// DC-21 outcome name.
    pub outcome: &'static str,
    /// Primary reason code, if the report produced an item.
    pub reason: Option<&'static str>,
    /// Left sequence operation count.
    pub left_operation_count: usize,
    /// Right sequence operation count.
    pub right_operation_count: usize,
    /// Deterministic display items.
    pub items: Vec<MergeEvidenceDisplayItem>,
}

impl MergeEvidenceDisplay {
    /// Whether the evidence proved confluence — the only outcome merge execution may seal against
    /// (DC-74). Compares against the same `outcome_name` mapping `from_report` used to produce
    /// `self.outcome`, rather than a second string literal at each call site.
    #[must_use]
    pub(crate) fn is_confluent(&self) -> bool {
        self.outcome == outcome_name(MergeEvidenceOutcome::Confluent)
    }
}

/// Submitted selector and resolved block identity for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeEvidenceDisplaySelector {
    /// User-facing selector text.
    pub selector: String,
    /// Resolved target block id.
    pub target_block_id: ObjectId,
}

/// Public display item for a merge-evidence report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeEvidenceDisplayItem {
    /// Sequence side.
    pub side: &'static str,
    /// Left/current operation summary.
    pub operation: Option<MergeEvidenceDisplayOperation>,
    /// Right/peer operation summary for cross items.
    pub peer_operation: Option<MergeEvidenceDisplayOperation>,
    /// DC-21 outcome name.
    pub outcome: &'static str,
    /// Evidence scope, when relevant.
    pub evidence_scope: Option<&'static str>,
    /// Proof phase.
    pub proof_phase: &'static str,
    /// Stable DC-21 reason code.
    pub reason_code: &'static str,
    /// Conflict-witness-presentation handoff v1: the specific reason two operations conflict or
    /// order, when this item carries a witness (`reason_code` names only the coarse outcome
    /// bucket -- `pair_conflict` covers all twelve `ConflictWitnessKind` variants alike; this
    /// names which one). **Stable kebab-case, an external interface** -- the same footing
    /// `VerificationStage::label()` and `reason_code` are already on: renaming, removing, or
    /// reusing one is a breaking change to any tool reading it. `None` for items with no
    /// underlying witness (confluent, evidence-failure, coarse unknown-reason items).
    pub witness_kind: Option<&'static str>,
    /// Repository-relative path the conflicting operations share, when the witness recorded one.
    /// Distinct from `operation.path`/`peer_operation.path` (each side's *own* recorded path,
    /// present only for path-bearing operation kinds like `CreateFile`/`DeleteNode`): this is the
    /// witness's own account of *why* the pair conflicts, present even for node-identity
    /// conflicts (e.g. `ChangePerm` vs `ReplaceBinary`) where neither side's own operation
    /// summary carries a path at all.
    pub witness_path: Option<String>,
    /// The shared node identity two conflicting operations both act on, when the witness recorded
    /// one. Exposed as the typed [`NodeId`] (the same footing `baseline_block_id: ObjectId`
    /// already is in [`MergeEvidenceDisplay`]) rather than a rendered string, since no stable
    /// human-facing rendering for a node identity exists anywhere in this codebase to freeze into
    /// this one struct. **The one field this handoff explicitly left to argument**: published
    /// because it remains the *only* correlating signal for the conflicts that are genuinely
    /// path-less -- a `ChangePerm`/`ReplaceBinary` mismatch on a node with no live entry in the
    /// baseline at all (conflict-witness-path-derivation handoff v1 derives `witness_path` from a
    /// node's live baseline path when its own operation carries no path field, so most same-node
    /// `ChangePerm`/`ReplaceBinary` mismatches do carry one; only a mismatch against a node that
    /// was never live stays path-less) -- not because a raw 32-byte identity is legible to a
    /// person on its own.
    pub witness_node_id: Option<NodeId>,
}

/// Public operation summary for merge-evidence display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeEvidenceDisplayOperation {
    /// Zero-based operation index in the displayed side.
    pub index: usize,
    /// One-based operation sequence recorded by the patch.
    pub op_seq: Option<u32>,
    /// Operation kind.
    pub kind: Option<&'static str>,
    /// Repository-relative path, when safe and available.
    pub path: Option<String>,
    /// What this operation carries beyond `kind`/`path` -- RFC 144 §4o.6a's honesty invariant,
    /// the same mechanism `ShowOperationContent` already uses: a rename presented here cannot be
    /// constructed without its asserting AUTHOR key id, so this is the one value a surface must
    /// consult to know a rename is being shown, and it cannot omit the signer.
    pub content: MergeEvidenceDisplayOperationContent,
}

/// What a [`MergeEvidenceDisplayOperation`] carries beyond kind, op_seq, and path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeEvidenceDisplayOperationContent {
    /// A path rename. RFC 144 §4o.6a: a signed `RenamePath` asserts *this signer* rewrote `old`
    /// to `new`, so the asserting signer must be recoverable in the same answer.
    RenamePath {
        /// The patch's AUTHOR signature key id -- the signer who asserted this rename.
        author_key_id: String,
    },
    /// Every other operation kind carries nothing beyond `kind`/`path` today.
    Other,
}

impl MergeEvidenceDisplay {
    /// `left_author_key_ids`/`right_author_key_ids` are the AUTHOR key id resolved for each
    /// operation in `report.left_sequence`/`report.right_sequence`, in lockstep by index (RFC 144
    /// §4o.6a) -- `Some` only for a `RenamePath` operation, `None` for every other kind. Callers
    /// build these from the exact same operation slice `analyze_merge_evidence` itself consumed
    /// (`candidate_sequence`, in `merge_evidence.rs`), so an index that names a `RenamePath` here
    /// is guaranteed to have a resolved key id -- a missing one is an internal invariant failure,
    /// not a value this constructor can degrade gracefully.
    pub(crate) fn from_report(
        report: MergeEvidenceReport,
        left_selector: MergeEvidenceDisplaySelector,
        right_selector: MergeEvidenceDisplaySelector,
        left_author_key_ids: &[Option<String>],
        right_author_key_ids: &[Option<String>],
    ) -> Self {
        let reason = report
            .items
            .first()
            .map(|item| reason_code_name(item.reason_code));
        let items = report
            .items
            .iter()
            .map(|item| item_from_report(&report, item, left_author_key_ids, right_author_key_ids))
            .collect();
        Self {
            baseline_block_id: report.baseline_block_id,
            left_selector,
            right_selector,
            outcome: outcome_name(report.outcome),
            reason,
            left_operation_count: report.left_sequence.operation_count,
            right_operation_count: report.right_sequence.operation_count,
            items,
        }
    }

    /// Number of report items displayed by this view.
    pub fn displayed_item_count(&self) -> usize {
        self.items.len()
    }

    /// Number of report items in the full evidence report.
    pub fn total_item_count(&self) -> usize {
        self.items.len()
    }
}

fn item_from_report(
    report: &MergeEvidenceReport,
    item: &MergeEvidenceItem,
    left_author_key_ids: &[Option<String>],
    right_author_key_ids: &[Option<String>],
) -> MergeEvidenceDisplayItem {
    MergeEvidenceDisplayItem {
        side: side_name(item.side),
        operation: operation_for_item(
            report,
            operation_side(item.side),
            item.operation_index,
            item.op_seq,
            left_author_key_ids,
            right_author_key_ids,
        ),
        peer_operation: operation_for_item(
            report,
            peer_side(item.side),
            item.peer_operation_index,
            item.peer_op_seq,
            left_author_key_ids,
            right_author_key_ids,
        ),
        outcome: outcome_name(item.outcome),
        evidence_scope: item.evidence_scope.map(scope_name),
        proof_phase: proof_phase_name(item.proof_phase),
        reason_code: reason_code_name(item.reason_code),
        witness_kind: item.witness_kind.map(ConflictWitnessKind::label),
        witness_path: item.path.as_ref().map(|path| path.as_str().to_string()),
        witness_node_id: item.node_id,
    }
}

fn operation_side(side: MergeEvidenceSide) -> MergeEvidenceSide {
    match side {
        MergeEvidenceSide::Cross => MergeEvidenceSide::Left,
        MergeEvidenceSide::Left | MergeEvidenceSide::Right | MergeEvidenceSide::Report => side,
    }
}

fn operation_for_item(
    report: &MergeEvidenceReport,
    side: MergeEvidenceSide,
    index: Option<usize>,
    fallback_op_seq: Option<u32>,
    left_author_key_ids: &[Option<String>],
    right_author_key_ids: &[Option<String>],
) -> Option<MergeEvidenceDisplayOperation> {
    let index = index?;
    let (sequence, author_key_ids) = match side {
        MergeEvidenceSide::Left => (&report.left_sequence, left_author_key_ids),
        MergeEvidenceSide::Right => (&report.right_sequence, right_author_key_ids),
        MergeEvidenceSide::Cross | MergeEvidenceSide::Report => return None,
    };
    let operation = sequence.operations.get(index);
    let content = match operation.map(|op| op.operation_kind) {
        Some(MergeEvidenceOperationKind::RenamePath) => match author_key_ids.get(index) {
            Some(Some(author_key_id)) => MergeEvidenceDisplayOperationContent::RenamePath {
                author_key_id: author_key_id.clone(),
            },
            // `candidate_sequence` resolves an AUTHOR key id for every `RenamePath` operation it
            // pushes, in the same order -- a `RenamePath` operation here with no resolved key id
            // means the two vecs desynchronized, an internal invariant this display layer cannot
            // repair. Fails loudly rather than rendering a rename with a silently blank signer.
            _ => {
                unreachable!("RenamePath operation at index {index} has no resolved AUTHOR key id")
            }
        },
        _ => MergeEvidenceDisplayOperationContent::Other,
    };
    Some(MergeEvidenceDisplayOperation {
        index,
        op_seq: operation.map_or(fallback_op_seq, |op| Some(op.op_seq)),
        kind: operation.map(|op| operation_kind_name(op.operation_kind)),
        path: operation.and_then(|op| op.path.as_ref().map(|path| path.as_str().to_string())),
        content,
    })
}

fn peer_side(side: MergeEvidenceSide) -> MergeEvidenceSide {
    match side {
        MergeEvidenceSide::Left => MergeEvidenceSide::Right,
        MergeEvidenceSide::Right | MergeEvidenceSide::Cross | MergeEvidenceSide::Report => {
            MergeEvidenceSide::Right
        }
    }
}

fn outcome_name(outcome: MergeEvidenceOutcome) -> &'static str {
    match outcome {
        MergeEvidenceOutcome::Confluent => "Confluent",
        MergeEvidenceOutcome::Conflict => "Conflict",
        MergeEvidenceOutcome::OrderedDependency => "OrderedDependency",
        MergeEvidenceOutcome::Unsupported => "Unsupported",
        MergeEvidenceOutcome::Deferred => "Deferred",
        MergeEvidenceOutcome::NotConfluent => "NotConfluent",
        MergeEvidenceOutcome::EvidenceFailure => "EvidenceFailure",
        MergeEvidenceOutcome::InvalidCandidate => "InvalidCandidate",
    }
}

fn side_name(side: MergeEvidenceSide) -> &'static str {
    match side {
        MergeEvidenceSide::Left => "left",
        MergeEvidenceSide::Right => "right",
        MergeEvidenceSide::Cross => "cross",
        MergeEvidenceSide::Report => "report",
    }
}

fn scope_name(scope: MergeEvidenceScope) -> &'static str {
    match scope {
        MergeEvidenceScope::SealedBaseline => "sealed-baseline",
        MergeEvidenceScope::SealedCandidate => "sealed-candidate",
        MergeEvidenceScope::UnsealedCandidate => "unsealed-candidate",
    }
}

fn proof_phase_name(phase: MergeEvidenceProofPhase) -> &'static str {
    match phase {
        MergeEvidenceProofPhase::Classification => "classification",
        MergeEvidenceProofPhase::ReplayBothOrders => "replay-both-orders",
        MergeEvidenceProofPhase::Flatness => "flatness",
        #[cfg(test)]
        MergeEvidenceProofPhase::ComposedReplay => "composed-replay",
        MergeEvidenceProofPhase::FinalStateComparison => "final-state-comparison",
    }
}

fn operation_kind_name(kind: MergeEvidenceOperationKind) -> &'static str {
    match kind {
        MergeEvidenceOperationKind::CreateFile => "CreateFile",
        MergeEvidenceOperationKind::DeleteNode => "DeleteNode",
        MergeEvidenceOperationKind::EditText => "EditText",
        MergeEvidenceOperationKind::ReplaceBinary => "ReplaceBinary",
        MergeEvidenceOperationKind::RenamePath => "RenamePath",
        MergeEvidenceOperationKind::ChangePerm => "ChangePerm",
        MergeEvidenceOperationKind::CreateSymlink => "CreateSymlink",
    }
}

fn reason_code_name(reason_code: MergeEvidenceReasonCode) -> &'static str {
    match reason_code {
        MergeEvidenceReasonCode::ProvenConfluent => "proven_confluent",
        MergeEvidenceReasonCode::PairConflict => "pair_conflict",
        MergeEvidenceReasonCode::OrderedDependency => "ordered_dependency",
        MergeEvidenceReasonCode::UnsupportedOperation => "unsupported_operation",
        MergeEvidenceReasonCode::SameNodeTextTransformDeferred => {
            "same_node_text_transform_deferred"
        }
        MergeEvidenceReasonCode::SequenceInternalDependencyDeferred => {
            "sequence_internal_dependency_deferred"
        }
        MergeEvidenceReasonCode::PairReplayFailed => "pair_replay_failed",
        #[cfg(test)]
        MergeEvidenceReasonCode::ComposedReplayFailed => "composed_replay_failed",
        MergeEvidenceReasonCode::FinalStateMismatch => "final_state_mismatch",
        MergeEvidenceReasonCode::MissingRequiredEvidence => "missing_required_evidence",
        MergeEvidenceReasonCode::MalformedRequiredEvidence => "malformed_required_evidence",
        MergeEvidenceReasonCode::WrongTypeRequiredEvidence => "wrong_type_required_evidence",
        MergeEvidenceReasonCode::UnreadableRequiredEvidence => "unreadable_required_evidence",
        MergeEvidenceReasonCode::InvalidUnsealedCandidate => "invalid_unsealed_candidate",
        MergeEvidenceReasonCode::InsufficientUnsealedCandidateEvidence => {
            "insufficient_unsealed_candidate_evidence"
        }
    }
}
