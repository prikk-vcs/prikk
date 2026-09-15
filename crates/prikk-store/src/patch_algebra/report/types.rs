use std::collections::BTreeMap;

use prikk_object::{NodeId, ObjectId};

use crate::patch_algebra::types::ConflictWitnessKind;
use crate::path::RepoPath;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeEvidenceReport {
    pub(crate) baseline_block_id: ObjectId,
    pub(crate) replay_horizon: Option<ObjectId>,
    pub(crate) left_sequence: MergeEvidenceSequence,
    pub(crate) right_sequence: MergeEvidenceSequence,
    pub(crate) outcome: MergeEvidenceOutcome,
    pub(crate) items: Vec<MergeEvidenceItem>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MergeEvidenceOutcome {
    Confluent,
    Conflict,
    OrderedDependency,
    Unsupported,
    Deferred,
    NotConfluent,
    EvidenceFailure,
    InvalidCandidate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeEvidenceSequence {
    pub(crate) label: &'static str,
    pub(crate) operation_count: usize,
    pub(crate) operations: Vec<MergeEvidenceOperation>,
    /// DC-75 two-edits handoff §6 (R1): each run of operations the evidence judged as one net
    /// operation, keyed by its first original index, valued by its last. Indices are original.
    pub(crate) folded_through: BTreeMap<usize, usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeEvidenceOperation {
    pub(crate) operation_index: usize,
    pub(crate) op_seq: u32,
    pub(crate) operation_kind: MergeEvidenceOperationKind,
    pub(crate) node_id: Option<NodeId>,
    pub(crate) path: Option<RepoPath>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MergeEvidenceOperationKind {
    CreateFile,
    DeleteNode,
    EditText,
    ReplaceBinary,
    RenamePath,
    ChangePerm,
    CreateSymlink,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MergeEvidenceItem {
    pub(crate) side: MergeEvidenceSide,
    pub(crate) operation_index: Option<usize>,
    pub(crate) peer_operation_index: Option<usize>,
    pub(crate) op_seq: Option<u32>,
    pub(crate) peer_op_seq: Option<u32>,
    pub(crate) operation_kind: Option<MergeEvidenceOperationKind>,
    pub(crate) node_id: Option<NodeId>,
    pub(crate) path: Option<RepoPath>,
    /// Conflict-witness-presentation handoff v1: the specific `ConflictWitnessKind` this item's
    /// underlying witness carries, when it has one (`Cross` items produced from a real
    /// `ConflictWitness` -- `PairClass::Conflict`/`OrderedDependency`). `None` for report-level
    /// items with no per-pair witness (confluent, evidence-failure, and coarse unknown-reason
    /// items) -- `reason_code` already covers those.
    pub(crate) witness_kind: Option<ConflictWitnessKind>,
    pub(crate) outcome: MergeEvidenceOutcome,
    pub(crate) evidence_scope: Option<MergeEvidenceScope>,
    pub(crate) proof_phase: MergeEvidenceProofPhase,
    pub(crate) reason_code: MergeEvidenceReasonCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MergeEvidenceSide {
    Left,
    Right,
    Cross,
    Report,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MergeEvidenceScope {
    SealedBaseline,
    SealedCandidate,
    UnsealedCandidate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MergeEvidenceProofPhase {
    Classification,
    ReplayBothOrders,
    Flatness,
    #[cfg(test)]
    ComposedReplay,
    FinalStateComparison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum MergeEvidenceReasonCode {
    ProvenConfluent,
    PairConflict,
    OrderedDependency,
    UnsupportedOperation,
    SameNodeTextTransformDeferred,
    SequenceInternalDependencyDeferred,
    PairReplayFailed,
    #[cfg(test)]
    ComposedReplayFailed,
    FinalStateMismatch,
    MissingRequiredEvidence,
    MalformedRequiredEvidence,
    WrongTypeRequiredEvidence,
    UnreadableRequiredEvidence,
    InvalidUnsealedCandidate,
    InsufficientUnsealedCandidateEvidence,
}
