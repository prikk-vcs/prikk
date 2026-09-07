//! RFC 111 §7/§8 -- the cost gate.
//!
//! Asserts that verifying a repository performs a number of full object-index decodes that does not
//! grow with repository size. **Required to FAIL on current `main`**: `FileObjectStore::read_object`
//! (via `index::lookup_object_location` → `index::replay_index`) decodes the entire on-disk index on
//! every single object read, so a `verify` walking N blocks and their referenced patches/blobs decodes
//! the index roughly proportionally to N (RFC 111 §1, §5.1: ~82% of the regression). §6.1's fix turns
//! this green; nothing here implements that fix.
//!
//! **What is counted**: `index::replay_index`'s own decode of the on-disk index --
//! `index::record_replay_index_decode_for_test`, incremented at the one place `replay_index` actually
//! calls `decode_index_records`, not on its early "missing container index file" return (which decodes
//! nothing). This is "the number of full index decodes performed while verifying a repository of N
//! objects" (RFC 111 §7.2), read directly off the function the RFC's own §1/§5.1 names as the cause,
//! not inferred from timing.
//!
//! **What the bound is stated as**: exact equality between two sizes, not a constant. A repository of
//! N=1 already triggers at least one `replay_index` call from `verify`'s own index cross-check loop
//! (`verify/objects.rs`), so a hardcoded constant would need updating whenever unrelated verify-stage
//! work adds or removes a fixed-count read unconnected to this regression. Comparing two sizes and
//! requiring equality isolates the property this RFC cares about -- flat, not proportional to
//! repository size -- from that unrelated noise (RFC 111 §7.2's own "equal, or bounded by a small
//! stated constant" phrasing; equality is the stronger, noise-free version of that bound).

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, NodeId,
    ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
};

use crate::foundation::index::{
    replay_index_decode_count_for_test, reset_replay_index_decode_count_for_test,
};
use crate::test_gates::test_support::{maintainer_signature, unique_temp_dir};
use crate::{
    FileObjectStore, ObjectWriter, RepositoryLayout, derive_next_state_root, verify_repository,
};

fn write_signed_block(store: &mut FileObjectStore, payload: &BlockPayload) -> Result<ObjectId> {
    let payload_bytes = payload.to_canonical_bytes()?;
    let mut block = ObjectEnvelope::unsigned(ObjectType::Block, 2, payload_bytes);
    block.add_signature(maintainer_signature())?;
    store.write_object(&block)
}

/// One linear child block over `parent`, with one real `CreateFile` patch -- genuinely replayable
/// content, matching `verify/tests.rs::write_create_child`'s own established shape for building a
/// synthetic sealed-block chain without a full commit/seal pipeline.
fn write_linear_child(
    store: &mut FileObjectStore,
    parent: ObjectId,
    index: usize,
) -> Result<ObjectId> {
    let path = format!("f{index}.txt");
    let blob = BlobPayload::new(BlobKind::Text, format!("{path}\n").into_bytes());
    let mut blob_env = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
    blob_env.add_signature(maintainer_signature())?;
    let blob_id = store.write_object(&blob_env)?;

    let patch = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path,
                node_id: NodeId::from_bytes([((index % 255) + 1) as u8; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch_env = ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch.to_canonical_bytes()?);
    patch_env.add_signature(maintainer_signature())?;
    let patch_id = store.write_object(&patch_env)?;

    let state_merkle_root = derive_next_state_root(store, Some(parent), &[patch_id])?;
    write_signed_block(
        store,
        &BlockPayload {
            parent_block_ids: vec![parent],
            kind: BlockKind::Normal,
            patch_ids: vec![patch_id],
            state_merkle_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )
}

/// Builds a linear chain of `sealed_block_count` blocks (genesis plus that many children), each with
/// its own patch and blob -- the object shape `verify`'s topological/lineage pass walks. Returns the
/// number of index decodes `verify_repository` performs against the result.
fn measure_replay_index_decodes_for_verify(sealed_block_count: usize) -> Result<usize> {
    let root = unique_temp_dir(&format!("rfc111-cost-gate-{sealed_block_count}-blocks"));
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let genesis_root = derive_next_state_root(&store, None, &[])?;
    let mut tip = write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: genesis_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;
    for index in 0..sealed_block_count {
        tip = write_linear_child(&mut store, tip, index)?;
    }

    reset_replay_index_decode_count_for_test();
    let _report = verify_repository(&layout)?;
    let decode_count = replay_index_decode_count_for_test();

    let _ = std::fs::remove_dir_all(root);
    Ok(decode_count)
}

/// **This test is required to FAIL on current `main`** (RFC 111 §7.3, prerequisite handoff §8: "write
/// the gate before the fix, on current `main`, where it is required to FAIL"). It passes once §6.1's
/// fix lands. See the module doc for what is counted and why equality (not a constant) is the bound.
#[test]
fn verify_index_decode_count_does_not_grow_with_repository_size() -> Result<()> {
    const SMALL: usize = 4;
    const LARGE: usize = 16;

    let small_decodes = measure_replay_index_decodes_for_verify(SMALL)?;
    let large_decodes = measure_replay_index_decodes_for_verify(LARGE)?;

    assert_eq!(
        small_decodes, large_decodes,
        "verify's full-index-decode count must not grow with repository size (RFC 111 §7.2), \
         but a {SMALL}-sealed-block repository triggered {small_decodes} decode(s) while a \
         {LARGE}-sealed-block repository triggered {large_decodes} -- proportional to N, which is \
         exactly the O(N) `index::replay_index` regression RFC 111 §1/§5.1 measured \
         (`FileObjectStore::read_object` decodes the entire on-disk index on every object read)"
    );
    Ok(())
}
