pub(super) use super::super::classify::{
    classify_pair as classify_pair_result,
    classify_pair_with_text_resolver as classify_pair_with_text_resolver_result,
};
pub(super) use super::super::commutation::{
    check_confluence as check_confluence_result, commute_pair as commute_pair_result,
};
pub(super) use super::super::evidence::StorePatchAlgebraEvidence;
pub(super) use super::super::evidence_types::{
    Evidence, EvidenceError, EvidenceFact, EvidenceScope, PatchAlgebraEvidence,
};
pub(super) use super::super::facts::{operation_facts, path_effects};
pub(super) use super::super::replay_oracle::replay_operations;
pub(super) use super::super::report::{
    MergeEvidenceItem, MergeEvidenceOperationKind, MergeEvidenceOutcome, MergeEvidenceProofPhase,
    MergeEvidenceReasonCode, MergeEvidenceScope, MergeEvidenceSide, analyze_merge_evidence,
    analyze_pair_merge_evidence, pair_class_report,
};
pub(super) use super::super::types::{
    CommutationResult, ConflictWitness, ConflictWitnessKind, ConfluenceResult,
    ConfluenceWitnessKind, PairClass, RequiredOrder, UnknownReason,
};
use super::super::witness::operand_path;
pub(super) use crate::node::node_lifecycle::NodeLifecycleState;
pub(super) use crate::patch_replay::decode::DecodedOperationKind;
pub(super) use prikk_object::{BlobKind, NodeKind, ObjectId, ObjectType};
pub(super) use std::collections::BTreeMap;

use crate::patch_replay::decode::DecodedPatchOperation;
use prikk_object::NodeId;

pub(super) const MODE_REGULAR: u32 = 0o100644;
pub(super) const MODE_EXECUTABLE: u32 = 0o100755;

pub(super) struct TestTextResolver {
    pub(super) texts: BTreeMap<NodeId, Vec<u8>>,
    blobs: BTreeMap<ObjectId, (BlobKind, Vec<u8>)>,
    blob_kind_overrides: BTreeMap<ObjectId, Evidence<BlobKind>>,
}

impl TestTextResolver {
    pub(super) fn empty() -> Self {
        Self {
            texts: BTreeMap::new(),
            blobs: BTreeMap::new(),
            blob_kind_overrides: BTreeMap::new(),
        }
    }

    pub(super) fn new(entries: impl IntoIterator<Item = (NodeId, Vec<u8>)>) -> Self {
        Self {
            texts: entries.into_iter().collect(),
            blobs: BTreeMap::new(),
            blob_kind_overrides: BTreeMap::new(),
        }
    }

    pub(super) fn with_blob(mut self, blob_id: ObjectId, kind: BlobKind, content: Vec<u8>) -> Self {
        self.blobs.insert(blob_id, (kind, content));
        self
    }

    pub(super) fn with_blob_kind_evidence(
        mut self,
        blob_id: ObjectId,
        evidence: Evidence<BlobKind>,
    ) -> Self {
        self.blob_kind_overrides.insert(blob_id, evidence);
        self
    }
}

impl PatchAlgebraEvidence for TestTextResolver {
    fn baseline_text(
        &self,
        scope: EvidenceScope,
        node_id: NodeId,
        blob_id: ObjectId,
    ) -> Evidence<Vec<u8>> {
        self.texts
            .get(&node_id)
            .cloned()
            .map(Evidence::Known)
            .unwrap_or(Evidence::Missing {
                scope,
                fact: EvidenceFact::BaselineText,
                object_id: Some(blob_id),
                node_id: Some(node_id),
            })
    }

    fn blob_kind(&self, scope: EvidenceScope, blob_id: ObjectId) -> Evidence<BlobKind> {
        if let Some(evidence) = self.blob_kind_overrides.get(&blob_id) {
            return evidence.clone();
        }
        self.blobs
            .get(&blob_id)
            .map(|(kind, _)| Evidence::Known(*kind))
            .unwrap_or(Evidence::Missing {
                scope,
                fact: EvidenceFact::BlobKind,
                object_id: Some(blob_id),
                node_id: None,
            })
    }

    fn blob_content(
        &self,
        scope: EvidenceScope,
        blob_id: ObjectId,
    ) -> Evidence<(BlobKind, Vec<u8>)> {
        self.blobs
            .get(&blob_id)
            .cloned()
            .map(Evidence::Known)
            .unwrap_or(Evidence::Missing {
                scope,
                fact: EvidenceFact::BlobBytes,
                object_id: Some(blob_id),
                node_id: None,
            })
    }
}

pub(super) fn classify_pair(
    baseline: &NodeLifecycleState,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> PairClass {
    let class = classify_pair_result(baseline, left, right).expect("classification evidence");
    assert_path_available_when_touched(baseline, left, right, &class);
    class
}

pub(super) fn classify_pair_with_text_resolver(
    baseline: &NodeLifecycleState,
    evidence: &TestTextResolver,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> PairClass {
    let class = classify_pair_with_text_resolver_result(baseline, evidence, left, right)
        .expect("classification evidence");
    assert_path_available_when_touched(baseline, left, right, &class);
    class
}

/// Conflict-witness-path-derivation handoff v1, §4: for any conflict witness, if the two
/// operands resolve to the same path (via [`operand_path`] -- an operand's own path field, or
/// failing that its live path in `baseline`), or only one of them resolves to a path at all, the
/// witness must report it. This is a real predicate on the operands and the baseline, not a
/// hand-maintained list of kinds -- so it runs against every classification the whole test suite
/// already makes, not a curated subset.
///
/// When both operands resolve to two **different** paths (`NodeIdReuse`: two `CreateFile`s at
/// different paths reusing one node id), neither is uniquely "the" path -- §3 answers this by
/// leaving the witness's single `path` field unconstrained in that case (not required to be
/// `None`, not required to be `Some`), since a single field genuinely cannot pick one, and both
/// real paths stay independently visible via `MergeEvidenceDisplayItem.operation.path`/
/// `.peer_operation.path`. This is the one honest escape from "always require a path when
/// something is touched" -- it is a structural fact about the operand pair (two distinct
/// resolved paths), not a list of kind names to skip.
fn assert_path_available_when_touched(
    baseline: &NodeLifecycleState,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
    class: &PairClass,
) {
    let witness = match class {
        PairClass::Conflict { witness } | PairClass::OrderedDependency { witness, .. } => witness,
        PairClass::Unknown { witness, .. } => witness,
        PairClass::Independent => return,
    };
    let (Ok(left_facts), Ok(right_facts)) = (operation_facts(left), operation_facts(right)) else {
        return;
    };
    let left_path = operand_path(baseline, &left_facts);
    let right_path = operand_path(baseline, &right_facts);
    let requires_path = match (&left_path, &right_path) {
        (Some(l), Some(r)) => l == r,
        (Some(_), None) | (None, Some(_)) => true,
        (None, None) => false,
    };
    assert!(
        !requires_path || witness.path.is_some(),
        "witness kind {:?} discards an available path -- left={:?} right={:?}",
        witness.kind,
        left_facts.action,
        right_facts.action
    );
}
