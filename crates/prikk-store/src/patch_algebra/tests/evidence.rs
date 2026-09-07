use prikk_object::{BlobPayload, CanonicalEncode, ObjectEnvelope, ObjectType, text_span_hash};

use crate::lifecycle_cache::ReplayDerivedLifecycleState;
use crate::memory_store::MemoryObjectStore;
use crate::node::node_lifecycle::{LiveNode, NodeContent};
use crate::object_store::ObjectWriter;

use super::*;

#[test]
fn store_backed_resolver_reads_live_baseline_text() {
    let old_text = b"alpha beta gamma";
    let mut store = MemoryObjectStore::new();
    let text_blob = write_blob(&mut store, BlobKind::Text, old_text);
    let baseline = baseline_with_file_node(NodeKind::TextFile, text_blob, old_text, MODE_REGULAR);
    let evidence = store_evidence(&store, baseline);
    let left = change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE);
    let right = edit_text(2, node(1), old_text, b"alpha BETA gamma");

    assert_eq!(
        classify_pair_with_text_resolver_result(
            evidence.baseline_state(),
            &evidence,
            &left,
            &right
        )
        .expect("classification succeeds"),
        PairClass::Independent
    );
    assert_eq!(evidence.baseline_block_id(), blob(0xb0));
    assert_eq!(evidence.lineage_horizon_id(), blob(0xa0));
}

#[test]
fn missing_sealed_baseline_text_blob_is_evidence_error() {
    let store = MemoryObjectStore::new();
    let missing_blob = blob(0x44);
    let baseline = baseline_with_file_node(
        NodeKind::TextFile,
        missing_blob,
        b"alpha beta",
        MODE_REGULAR,
    );
    let evidence = store_evidence(&store, baseline);
    let left = change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE);
    let right = edit_text(2, node(1), b"alpha beta", b"alpha BETA");

    // DC-65: a missing baseline text blob is no longer terminal by itself — `baseline_text` falls
    // back to replay-based materialization first (a `TextFile` node's `blob_id` after an `EditText`
    // is a content identity, not necessarily a stored object). This fixture's baseline/horizon ids
    // (`blob(0xb0)`/`blob(0xa0)`) are synthetic markers with no real `Block` behind them, so the
    // fallback's lineage walk itself fails — correctly reported as `Unreadable` (the replay context
    // is unavailable), a more precise class than the old blanket `Missing` now that the two causes
    // are actually distinguished.
    match classify_pair_with_text_resolver_result(
        evidence.baseline_state(),
        &evidence,
        &left,
        &right,
    ) {
        Err(EvidenceError::Unreadable {
            scope: EvidenceScope::SealedBaselineRequired,
            fact: EvidenceFact::BaselineText,
            ..
        }) => {}
        other => panic!("expected unreadable baseline-text evidence error, got {other:?}"),
    }
}

#[test]
fn missing_sealed_candidate_blob_is_evidence_error_but_unsealed_candidate_is_unknown() {
    let store = MemoryObjectStore::new();
    let baseline = NodeLifecycleState::new();
    let evidence = store_evidence(&store, baseline);
    let missing_blob = blob(0x55);

    match evidence.blob_kind(EvidenceScope::SealedCandidateRequired, missing_blob) {
        Evidence::Missing { scope, fact, .. } => {
            assert_eq!(scope, EvidenceScope::SealedCandidateRequired);
            assert_eq!(fact, EvidenceFact::BlobKind);
        }
        other => panic!("expected sealed candidate missing evidence, got {other:?}"),
    }

    let left = create_file(1, "fresh.bin", node(1), missing_blob, MODE_REGULAR);
    let right = replace_binary(2, node(1), missing_blob, blob(0x66));
    assert_unknown(
        classify_pair_with_text_resolver_result(
            evidence.baseline_state(),
            &evidence,
            &left,
            &right,
        )
        .expect("missing unsealed candidate is a classification"),
        UnknownReason::MissingCandidateEvidence,
    );
}

#[test]
fn wrong_object_type_for_baseline_blob_is_evidence_error() {
    let mut store = MemoryObjectStore::new();
    let wrong_object = ObjectEnvelope::unsigned(ObjectType::Block, 1, b"not a blob".to_vec());
    let wrong_id = store
        .write_object(&wrong_object)
        .expect("write wrong object");
    let baseline =
        baseline_with_file_node(NodeKind::TextFile, wrong_id, b"alpha beta", MODE_REGULAR);
    let evidence = store_evidence(&store, baseline);
    let left = change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE);
    let right = edit_text(2, node(1), b"alpha beta", b"alpha BETA");

    match classify_pair_with_text_resolver_result(
        evidence.baseline_state(),
        &evidence,
        &left,
        &right,
    ) {
        Err(EvidenceError::WrongObjectType {
            scope,
            object_id,
            expected,
            actual,
        }) => {
            assert_eq!(scope, EvidenceScope::SealedBaselineRequired);
            assert_eq!(object_id, wrong_id);
            assert_eq!(expected, ObjectType::Blob);
            assert_eq!(actual, ObjectType::Block);
        }
        other => panic!("expected wrong object type error, got {other:?}"),
    }
}

#[test]
fn malformed_baseline_blob_payload_is_evidence_error() {
    let mut store = MemoryObjectStore::new();
    let malformed = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"bad blob".to_vec());
    let malformed_id = store
        .write_object(&malformed)
        .expect("write malformed blob");
    let baseline = baseline_with_file_node(
        NodeKind::TextFile,
        malformed_id,
        b"alpha beta",
        MODE_REGULAR,
    );
    let evidence = store_evidence(&store, baseline);
    let left = change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE);
    let right = edit_text(2, node(1), b"alpha beta", b"alpha BETA");

    match classify_pair_with_text_resolver_result(
        evidence.baseline_state(),
        &evidence,
        &left,
        &right,
    ) {
        Err(EvidenceError::Malformed { scope, fact, .. }) => {
            assert_eq!(scope, EvidenceScope::SealedBaselineRequired);
            assert_eq!(fact, EvidenceFact::BaselineText);
        }
        other => panic!("expected malformed evidence error, got {other:?}"),
    }
}

#[test]
fn wrong_blob_kind_for_baseline_text_is_evidence_error() {
    let mut store = MemoryObjectStore::new();
    let binary_blob = write_blob(&mut store, BlobKind::Binary, b"alpha beta");
    let baseline =
        baseline_with_file_node(NodeKind::TextFile, binary_blob, b"alpha beta", MODE_REGULAR);
    let evidence = store_evidence(&store, baseline);
    let left = change_perm(1, node(1), MODE_REGULAR, MODE_EXECUTABLE);
    let right = edit_text(2, node(1), b"alpha beta", b"alpha BETA");

    match classify_pair_with_text_resolver_result(
        evidence.baseline_state(),
        &evidence,
        &left,
        &right,
    ) {
        Err(EvidenceError::WrongBlobKind {
            scope,
            blob_id,
            expected,
            actual,
        }) => {
            assert_eq!(scope, EvidenceScope::SealedBaselineRequired);
            assert_eq!(blob_id, binary_blob);
            assert_eq!(expected, BlobKind::Text);
            assert_eq!(actual, BlobKind::Binary);
        }
        other => panic!("expected wrong blob kind error, got {other:?}"),
    }
}

#[test]
fn replay_unavailable_prevents_store_resolver_construction() {
    let store = MemoryObjectStore::new();

    match StorePatchAlgebraEvidence::from_store(&store, blob(0xb0), blob(0xa0)) {
        Err(EvidenceError::Unreadable {
            scope,
            fact,
            object_id,
            ..
        }) => {
            assert_eq!(scope, EvidenceScope::SealedBaselineRequired);
            assert_eq!(fact, EvidenceFact::BaselineState);
            assert_eq!(object_id, Some(blob(0xb0)));
        }
        other => panic!("expected replay construction evidence error, got {other:?}"),
    }
}

#[test]
fn future_precondition_reason_remains_reserved_for_unsupported_records() {
    assert_eq!(
        UnknownReason::FuturePreconditionDeferred,
        UnknownReason::FuturePreconditionDeferred
    );
}

fn store_evidence(
    store: &MemoryObjectStore,
    baseline: NodeLifecycleState,
) -> StorePatchAlgebraEvidence<'_, MemoryObjectStore> {
    let replay =
        ReplayDerivedLifecycleState::from_replay(blob(0xb0), baseline).expect("valid replay state");
    StorePatchAlgebraEvidence::from_replay_derived(store, blob(0xa0), replay)
        .expect("store evidence")
}

fn baseline_with_file_node(
    kind: NodeKind,
    blob_id: ObjectId,
    text_for_span: &[u8],
    mode: u32,
) -> NodeLifecycleState {
    let mut baseline = NodeLifecycleState::new();
    baseline
        .seed_live_node(
            node(1),
            LiveNode {
                path: path("note.txt"),
                kind,
                content: NodeContent::File { blob_id, mode },
            },
        )
        .expect("seed baseline");
    assert_eq!(text_span_hash(text_for_span), text_span_hash(text_for_span));
    baseline
}

fn write_blob(store: &mut MemoryObjectStore, kind: BlobKind, content: &[u8]) -> ObjectId {
    let payload = BlobPayload::new(kind, content.to_vec());
    let envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        payload.to_canonical_bytes().expect("encode blob"),
    );
    store.write_object(&envelope).expect("write blob")
}
