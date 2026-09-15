//! RFC 111 Stage 2's store-level replica of `seal_active_no_audit`'s "new content" path
//! (`crates/prikk-cli/src/seal.rs`), factored out so it has exactly one implementation instead of two
//! that could drift apart.
//!
//! [`simulate_one_seal`] was originally private to `rfc111_seal_decode_cost_gate`'s own module. The
//! Stage 2 gate review's blocking condition (C1) required a *drift guard*: run the real `prikk seal`
//! binary against a fixture, run this same simulation against an identical one, and assert the two
//! resulting repositories agree on object ids and ref state. A `prikk-cli` integration test is the
//! only place that can spawn the real binary (`CARGO_BIN_EXE_prikk`), so this function has to be
//! reachable from outside this crate -- hence the `test-support` feature wrapper below, following the
//! same shape as `refs::force_ref_pointer_to_arbitrary_state_for_test_support` (design-v1.md §13.9):
//! the real implementation is gated `cfg(any(test, feature = "test-support"))` so this crate's own
//! `#[cfg(test)]` gate (`rfc111_seal_decode_cost_gate`) can still call it directly without enabling
//! the feature, and the genuinely `pub`, cross-crate-visible wrapper is gated `feature = "test-support"`
//! only, never compiled into the shipped binary.

#![cfg(any(test, feature = "test-support"))]

use prikk_error::Result;
use prikk_object::{
    CanonicalEncode, ObjectId, ObjectType, RefKind, RefStatePayload, RefUpdatePayload,
};

use crate::{
    ActiveLock, BlockLineage, DEFAULT_ACTIVE_NAME, Ed25519MaintainerSigner, MaintainerSigner,
    ObjectReader, ObjectWriteSession, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    Wal, finish_active_publication_cleanup, maintainer_signature, seal_block,
};

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &Ed25519MaintainerSigner,
) -> Result<prikk_object::ObjectEnvelope> {
    let mut envelope =
        prikk_object::ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, object_type, object_id)?)?;
    Ok(envelope)
}

/// Mirrors `seal_active_no_audit`'s "new content" path -- see the module doc for exactly what is and
/// is not reproduced, and why. Must be called with a non-empty active WAL already present for
/// `ref_name` (a real commit, immediately before this call) and returns the published RefState id.
pub(crate) fn simulate_one_seal(
    layout: &RepositoryLayout,
    ref_name: &str,
    signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    let active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;

    let mut object_store = ObjectWriteSession::open(layout)?;
    let ref_store = RefStore::new(layout.clone());
    let current = match ref_store.read_current_ref_state_id(ref_name)? {
        Some(ref_state_id) => {
            let envelope = object_store
                .read_typed(ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    prikk_error::PrikkError::Integrity("current RefState missing".to_string())
                })?;
            let payload = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            Some((ref_state_id, payload))
        }
        None => None,
    };

    let mut patch_ids = Vec::with_capacity(replay.records.len());
    for record in &replay.records {
        patch_ids.push(object_store.write_object(&record.envelope)?);
    }

    let parent = current
        .as_ref()
        .map(|(_, payload)| payload.target_object_id);
    let block_id = seal_block(
        layout,
        &mut object_store,
        BlockLineage::Linear { parent },
        &patch_ids,
        signer,
    )?;

    let update_seq = current
        .as_ref()
        .map(|(_, payload)| payload.update_seq + 1)
        .unwrap_or(1);
    let previous_ref_state_id = current.as_ref().map(|(id, _)| *id);
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq,
        previous_ref_state_id,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
        signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: previous_ref_state_id,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        signer,
    )?;
    let publication = RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published = ref_store.finish_interrupted_publication_with_object_store(
        &mut object_store,
        &active_lock,
        &publication,
    )?;
    finish_active_publication_cleanup(layout, &active_lock)?;
    Ok(published)
}

/// Cross-crate wrapper around [`simulate_one_seal`], for use by another crate's own integration
/// tests only (RFC 111 Stage 2 gate review C1) -- gated by the non-default `test-support` feature,
/// never compiled into the shipped binary. Callers must supply a signer already trusted in `layout`'s
/// own trust policy (`add_trusted_maintainer`): `finish_interrupted_publication` performs real trust
/// verification internally and fails closed otherwise.
#[cfg(feature = "test-support")]
pub fn simulate_one_seal_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    simulate_one_seal(layout, ref_name, signer)
}

// ---- RFC 136 increment 2b §0: a block whose snapshot is absent, damaged or lying --------------------

/// What [`publish_snapshot_fixture_for_test_support`] puts in its block's snapshot field.
///
/// Gated on the item as well as by this module's inner attribute, so moving it out of this module
/// cannot ungate it (RFC 136 increment 2b review). `test` stays in the gate: this crate's own tests call
/// the fixture without the feature.
#[cfg(any(test, feature = "test-support"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnapshotFixture {
    /// No snapshot: the twin every other fixture is compared with.
    None,
    /// A `SNAPSHOT` Blob that is not a manifest, so the loader refuses it.
    Damaged,
    /// A manifest that recomputes to the block's **signed** root, which its own patch does not produce:
    /// the snapshot passes the loader, and only replay shows the lie (RFC 136 §10.3c).
    Lying,
}

/// Test-support only (RFC 136 increment 2b §0): publish `heads/main` as one root Block whose patch
/// creates `a.txt` with `replayed\n`, signed by `maintainer`, with `fixture` in its snapshot field.
/// Under [`SnapshotFixture::Lying`] the Block's signed root is that of `a.txt` holding `asserted\n`.
/// Returns the Block id. Never compiled into a shipped build: gated here and by the module's inner
/// attribute, with `test` kept so this crate's own tests can call it without the feature.
#[cfg(any(test, feature = "test-support"))]
pub fn publish_snapshot_fixture_for_test_support(
    layout: &RepositoryLayout,
    maintainer: &Ed25519MaintainerSigner,
    fixture: SnapshotFixture,
) -> Result<ObjectId> {
    use prikk_object::{
        BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, NodeId,
        NodeKind, ObjectEnvelope, Operation, OperationKind, PatchPayload, PatchPurpose,
    };

    fn blob(
        store: &mut crate::FileObjectStore,
        kind: BlobKind,
        bytes: Vec<u8>,
    ) -> Result<ObjectId> {
        store.write_object(&ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            BlobPayload::new(kind, bytes).to_canonical_bytes()?,
        ))
    }

    let mut store = crate::FileObjectStore::new(layout.clone());
    let node_id = NodeId::from_bytes([0x5A; 32]);
    let replayed = blob(&mut store, BlobKind::Text, b"replayed\n".to_vec())?;
    let author = crate::Ed25519AuthorSigner::from_seed("snapshot-fixture-author", &[0xA5; 32])?;
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id,
                blob_id: replayed,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    patch.add_signature(crate::author_signature(&author, patch.object_id())?)?;
    let patch_id = store.write_object(&patch)?;
    let replayed_root = crate::derive_next_state_root(&store, None, &[patch_id])?;

    let (state_merkle_root, snapshot_blob_ref) = match fixture {
        SnapshotFixture::None => (replayed_root, None),
        SnapshotFixture::Damaged => (
            replayed_root,
            Some(blob(
                &mut store,
                BlobKind::Snapshot,
                b"PRIKK-SNAPSHOT-MANIFEST-v2\nnot a manifest".to_vec(),
            )?),
        ),
        SnapshotFixture::Lying => {
            let asserted = blob(&mut store, BlobKind::Text, b"asserted\n".to_vec())?;
            let manifest = crate::SnapshotManifest {
                entries: vec![crate::StateRootEntry {
                    path: crate::RepoPath::parse("a.txt")?,
                    node_id,
                    kind: NodeKind::TextFile,
                    mode: 0o100644,
                    content: crate::state_root::StateRootContent::Blob(asserted),
                }],
            };
            let asserted_root = manifest.recomputed_state_root()?;
            let snapshot = blob(&mut store, BlobKind::Snapshot, manifest.encode()?)?;
            (asserted_root, Some(snapshot))
        }
    };
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: vec![patch_id],
        state_merkle_root,
        snapshot_blob_ref,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, block_payload.to_canonical_bytes()?);
    let block_id = block.object_id();
    block.add_signature(maintainer_signature(
        maintainer,
        ObjectType::Block,
        block_id,
    )?)?;
    store.write_object(&block)?;

    let ref_state = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state.to_canonical_bytes()?);
    let state_id = state_envelope.object_id();
    state_envelope.add_signature(maintainer_signature(
        maintainer,
        ObjectType::RefState,
        state_id,
    )?)?;
    let update = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: maintainer.key_id().to_string(),
    };
    let mut update_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update.to_canonical_bytes()?);
    let update_id = update_envelope.object_id();
    update_envelope.add_signature(maintainer_signature(
        maintainer,
        ObjectType::RefUpdate,
        update_id,
    )?)?;
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: state_envelope,
        ref_update: update_envelope,
    })?;
    Ok(block_id)
}
