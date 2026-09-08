use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, MerkleRoot,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind,
};

mod rename_convergence;
mod root_stability;

use super::{
    BlockStateStatus, LineageStateMemo, derive_next_state_root, validate_block_v2_shape,
    verify_block_v2_state, verify_blocks_topological,
};
use crate::lifecycle_cache::replay::{TextCache, apply_candidate_patches};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::state_root::entries_from_state;
use crate::{MemoryObjectStore, ObjectWriter, compute_state_root};

fn payload(kind: BlockKind, parents: Vec<ObjectId>, root: MerkleRoot) -> BlockPayload {
    merge_payload(kind, parents, root, None, None)
}

fn merge_payload(
    kind: BlockKind,
    parents: Vec<ObjectId>,
    root: MerkleRoot,
    mainline_parent_id: Option<ObjectId>,
    merge_baseline_block_id: Option<ObjectId>,
) -> BlockPayload {
    BlockPayload {
        parent_block_ids: parents,
        kind,
        patch_ids: Vec::new(),
        state_merkle_root: root,
        snapshot_blob_ref: None,
        mainline_parent_id,
        merge_baseline_block_id,
    }
}

#[test]
fn format2_parent_and_kind_matrix_is_closed() -> prikk_error::Result<()> {
    let parent = ObjectId::from_bytes([1; 32]);
    let root = compute_state_root(&[])?;
    assert!(validate_block_v2_shape(&payload(BlockKind::Root, Vec::new(), root)).is_ok());
    assert!(validate_block_v2_shape(&payload(BlockKind::Normal, vec![parent], root)).is_ok());
    assert!(validate_block_v2_shape(&payload(BlockKind::Root, vec![parent], root)).is_err());
    assert!(validate_block_v2_shape(&payload(BlockKind::Normal, Vec::new(), root)).is_err());
    assert!(
        validate_block_v2_shape(&payload(BlockKind::Normal, vec![parent, parent], root)).is_err()
    );
    // DC-75: `Merge` is no longer unconditionally closed (see `format2_merge_shape_matrix` below
    // for its now-open, narrowly-shaped case) -- but `Repair`/`Import` remain closed regardless of
    // parent count, per DC-75's own investigation §4.3, accepted without change. Proven at both 0
    // and 2 parents, since DC-75 is exactly the change that could have accidentally widened these
    // too (both share `candidate_blocks`' shape validation with `Merge`).
    for kind in [BlockKind::Repair, BlockKind::Import] {
        assert!(validate_block_v2_shape(&payload(kind, Vec::new(), root)).is_err());
        assert!(validate_block_v2_shape(&payload(kind, vec![parent, parent], root)).is_err());
    }
    assert!(validate_block_v2_shape(&payload(BlockKind::Merge, Vec::new(), root)).is_err());
    Ok(())
}

/// DC-75: `Merge` is open only for the exact shape — two parents, a mainline parent naming one of
/// them, and a recorded baseline. Every way to be short of that shape still fails closed; only the
/// fully-specified case succeeds, changed deliberately from the pre-DC-75 "any parent count errors."
#[test]
fn format2_merge_shape_matrix() -> prikk_error::Result<()> {
    let mainline = ObjectId::from_bytes([1; 32]);
    let secondary = ObjectId::from_bytes([2; 32]);
    let baseline = ObjectId::from_bytes([3; 32]);
    let not_a_parent = ObjectId::from_bytes([4; 32]);
    let root = compute_state_root(&[])?;
    let two_parents = || {
        let mut parents = vec![mainline, secondary];
        parents.sort();
        parents
    };

    // One parent, even with mainline/baseline set: still exactly the Normal/Root cardinality error.
    assert!(
        validate_block_v2_shape(&merge_payload(
            BlockKind::Merge,
            vec![mainline],
            root,
            Some(mainline),
            Some(baseline)
        ))
        .is_err()
    );
    // Two parents, no mainline named at all.
    assert!(
        validate_block_v2_shape(&merge_payload(
            BlockKind::Merge,
            two_parents(),
            root,
            None,
            Some(baseline)
        ))
        .is_err()
    );
    // Two parents, mainline named but not actually one of them.
    assert!(
        validate_block_v2_shape(&merge_payload(
            BlockKind::Merge,
            two_parents(),
            root,
            Some(not_a_parent),
            Some(baseline)
        ))
        .is_err()
    );
    // Two parents, valid mainline, but no recorded baseline.
    assert!(
        validate_block_v2_shape(&merge_payload(
            BlockKind::Merge,
            two_parents(),
            root,
            Some(mainline),
            None
        ))
        .is_err()
    );
    // Fully specified: open.
    assert!(
        validate_block_v2_shape(&merge_payload(
            BlockKind::Merge,
            two_parents(),
            root,
            Some(mainline),
            Some(baseline)
        ))
        .is_ok()
    );
    // Root/Normal must not carry either DC-75 field, even when otherwise valid.
    assert!(
        validate_block_v2_shape(&merge_payload(
            BlockKind::Normal,
            vec![mainline],
            root,
            Some(mainline),
            None
        ))
        .is_err()
    );
    Ok(())
}

#[test]
fn verification_recomputes_empty_root_and_rejects_mismatch() -> prikk_error::Result<()> {
    let store = MemoryObjectStore::new();
    let block_id = ObjectId::from_bytes([7; 32]);
    let valid = payload(BlockKind::Root, Vec::new(), compute_state_root(&[])?);
    verify_block_v2_state(&store, block_id, &valid, &mut LineageStateMemo::new())?;

    let invalid = payload(BlockKind::Root, Vec::new(), MerkleRoot([0; 32]));
    assert!(
        verify_block_v2_state(&store, block_id, &invalid, &mut LineageStateMemo::new()).is_err()
    );
    Ok(())
}

#[test]
fn format2_lineage_rejects_schema1_parent() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let parent_payload = payload(BlockKind::Root, Vec::new(), MerkleRoot([0; 32]));
    let parent =
        ObjectEnvelope::unsigned(ObjectType::Block, 1, parent_payload.to_canonical_bytes()?);
    let parent_id = store.write_object(&parent)?;
    assert!(derive_next_state_root(&store, Some(parent_id), &[]).is_err());
    Ok(())
}

#[test]
fn format2_lineage_rejects_parent_with_forged_state_root() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let parent_payload = payload(BlockKind::Root, Vec::new(), MerkleRoot([0; 32]));
    let parent =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, parent_payload.to_canonical_bytes()?);
    let parent_id = store.write_object(&parent)?;

    assert!(derive_next_state_root(&store, Some(parent_id), &[]).is_err());
    Ok(())
}

#[test]
fn equivalent_state_ignores_patch_identity() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let blob_payload = BlobPayload::new(BlobKind::Text, b"same state\n".to_vec());
    let blob = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_payload.to_canonical_bytes()?);
    let blob_id = store.write_object(&blob)?;
    let operation = Operation {
        op_seq: 1,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: "same.txt".to_string(),
            node_id: NodeId::from_bytes([0x41; 32]),
            blob_id,
            mode: 0o100644,
        }),
    };
    let first = patch_with_parents(operation.clone(), Vec::new())?;
    let second = patch_with_parents(operation, vec![ObjectId::from_bytes([0x77; 32])])?;
    let first_id = store.write_object(&first)?;
    let second_id = store.write_object(&second)?;
    assert_ne!(first_id, second_id);
    assert_eq!(
        derive_next_state_root(&store, None, &[first_id])?,
        derive_next_state_root(&store, None, &[second_id])?
    );
    Ok(())
}

/// `PatchPayload` no longer has a `parent_patch_ids` field (Patch schema 2 handoff: tag 2 retired,
/// never emitted by `encode_canonical`) -- schema 1 still legally carries it, so this bypasses
/// `PatchPayload` and writes tag 2 directly with `CanonicalWriter`, exactly where `encode_canonical`
/// used to emit it, to prove `equivalent_state_ignores_patch_identity`'s claim on the one shape no
/// production code can author anymore.
fn patch_with_parents(
    operation: Operation,
    parent_patch_ids: Vec<ObjectId>,
) -> prikk_error::Result<ObjectEnvelope> {
    let mut writer = prikk_object::CanonicalWriter::new();
    writer.repeated_record_list(1, &[operation])?;
    writer.repeated_object_id(2, &parent_patch_ids)?;
    let canonical_payload = writer.finish();
    Ok(ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        canonical_payload,
    ))
}

/// Patch-schema-2 handoff (v2 amendment) §5 item 3: `RefState` already proves a repository mixing
/// two admitted schemas for one `ObjectType` derives state correctly (DC-61,
/// `REF_STATE_CLOSED_SCHEMA`); this proves the same for `Patch`. One schema-1 patch (built the same
/// way `patch_with_parents` above does, the shape every patch written before this handoff has) and
/// one real schema-2 patch (built through the actual, current production `PatchPayload` encoder,
/// which can no longer emit tag 2 at all) both apply through `derive_next_state_root` -- the exact
/// function block-sealing and `verify` both call -- to a root matching the two files' combined
/// effect, proving `apply_candidate_patches`'s `require_schema_one = true` path (`replay.rs`) admits
/// both schemas via `format::admitted_schemas`, not a stale `!= 1` check.
#[test]
fn a_repository_holding_both_patch_schemas_derives_state_correctly() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let blob_payload = BlobPayload::new(BlobKind::Text, b"schema mix\n".to_vec());
    let blob = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_payload.to_canonical_bytes()?);
    let blob_id = store.write_object(&blob)?;

    let schema1_operation = Operation {
        op_seq: 1,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: "schema1.txt".to_string(),
            node_id: NodeId::from_bytes([0x9a; 32]),
            blob_id,
            mode: 0o100644,
        }),
    };
    let schema1_patch = patch_with_parents(schema1_operation.clone(), Vec::new())?;
    let schema1_id = store.write_object(&schema1_patch)?;

    let schema2_operation = Operation {
        op_seq: 1,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: "schema2.txt".to_string(),
            node_id: NodeId::from_bytes([0x9b; 32]),
            blob_id,
            mode: 0o100644,
        }),
    };
    let schema2_payload = prikk_object::PatchPayload {
        operations: vec![schema2_operation.clone()],
        intent: None,
        preconditions: Vec::new(),
        purpose: prikk_object::PatchPurpose::Normal,
        message: None,
    };
    let schema2_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_PARENT_IDS_RETIRED_SCHEMA,
        schema2_payload.to_canonical_bytes()?,
    );
    let schema2_id = store.write_object(&schema2_patch)?;

    let mixed_root = derive_next_state_root(&store, None, &[schema1_id, schema2_id])?;

    // Cross-check: a single schema-1 patch carrying *both* operations (bypassing the two-patch,
    // two-schema split entirely) must derive the identical root -- proving `mixed_root` reflects
    // both operations' effect, not just one of them silently winning or the other being dropped.
    let combined_second_operation = Operation {
        op_seq: 2,
        ..schema2_operation
    };
    let mut writer = prikk_object::CanonicalWriter::new();
    writer.repeated_record_list(1, &[schema1_operation, combined_second_operation])?;
    let combined_patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, writer.finish());
    let combined_id = store.write_object(&combined_patch)?;
    let expected_root = derive_next_state_root(&store, None, &[combined_id])?;
    assert_eq!(
        mixed_root, expected_root,
        "a repository holding both a schema-1 and a schema-2 patch must derive the same state as \
         a single patch carrying both operations"
    );

    // Negative control: a Patch envelope at a schema `admitted_schemas` does not accept (5 is
    // outside Patch's `&[1, PATCH_PARENT_IDS_RETIRED_SCHEMA, PATCH_TEXT_SPAN_V2_SCHEMA,
    // PATCH_MESSAGE_SCHEMA]`) must still be refused when mixed into the same candidate set --
    // proving the mixed-schema acceptance above is not because the admitted-schema check was
    // silently accepting everything.
    let out_of_range_patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 5, schema2_payload.to_canonical_bytes()?);
    let out_of_range_id = store.write_object(&out_of_range_patch)?;
    let result = derive_next_state_root(&store, None, &[schema1_id, out_of_range_id]);
    assert!(
        result.is_err(),
        "a Patch at schema 5 (outside Patch's admitted set) must be refused, not silently applied"
    );

    Ok(())
}

// --- DC-92: negative controls proving memoization does not weaken what verification catches. ---
//
// Corruption at genesis, mid-chain, and tip (RFC acceptance criterion 3), plus a fourth control the
// architect's review required: a shape violation reached as a lineage member, not as the outer
// loop's primary subject -- the position a memo checking only replay-and-compare could let through.
//
// Every corrupted block is checked by verifying a *later* block that names it as an ancestor, never
// the corrupted block itself directly -- that is what actually exercises the lineage-walk path
// memoization touches, matching how a real `verify` run would discover it while checking some other,
// later block.

fn create_file_patch(
    store: &mut MemoryObjectStore,
    path: &str,
    node_seed: u8,
) -> prikk_error::Result<ObjectId> {
    let blob_payload = BlobPayload::new(BlobKind::Text, format!("{path} content\n").into_bytes());
    let blob = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_payload.to_canonical_bytes()?);
    let blob_id = store.write_object(&blob)?;
    let operation = Operation {
        op_seq: 1,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: path.to_string(),
            node_id: NodeId::from_bytes([node_seed; 32]),
            blob_id,
            mode: 0o100_644,
        }),
    };
    let patch = patch_with_parents(operation, Vec::new())?;
    store.write_object(&patch)
}

/// Write a block claiming exactly `root`, whatever it is -- valid or deliberately wrong. Every
/// corruption test needs this rather than [`derive_next_state_root`]'s own (correctly validating)
/// computation, since that would refuse to build a block on top of an already-corrupted parent.
fn write_block(
    store: &mut MemoryObjectStore,
    parent: Option<ObjectId>,
    patch_ids: Vec<ObjectId>,
    root: MerkleRoot,
) -> prikk_error::Result<ObjectId> {
    let block_payload = BlockPayload {
        parent_block_ids: parent.into_iter().collect(),
        kind: if parent.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        },
        patch_ids,
        state_merkle_root: root,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, block_payload.to_canonical_bytes()?);
    store.write_object(&envelope)
}

/// Naive, non-validating continuation: apply `patch_ids` to `state`/`text_cache` in place and
/// return the resulting root, mirroring exactly what a forger recomputing "self-consistently" on
/// top of an already-tampered ancestor would do. Never used by production code -- the point of this
/// helper is to build a fixture [`derive_next_state_root`]'s own validation would refuse to
/// construct, so a corruption test can prove the validation still catches it via a *different* path
/// (a later block's lineage walk), not merely that construction fails.
fn naive_continue(
    store: &MemoryObjectStore,
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
    patch_ids: &[ObjectId],
) -> prikk_error::Result<MerkleRoot> {
    apply_candidate_patches(store, state, text_cache, patch_ids)?;
    compute_state_root(&entries_from_state(state)?)
}

const WRONG_ROOT: MerkleRoot = MerkleRoot([0xEE; 32]);

#[test]
fn genesis_corruption_is_caught_when_reached_as_a_lineage_member() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let mut state = NodeLifecycleState::new();
    let mut text_cache = TextCache::new();

    // Genesis claims WRONG_ROOT, not the true empty-state root -- corrupted from the start.
    let genesis_id = write_block(&mut store, None, Vec::new(), WRONG_ROOT)?;

    // Three more generations, naively continued (self-consistent with each other, but built on the
    // corrupted genesis) -- exactly what a forger recomputing downstream blocks would produce.
    let mut parent = genesis_id;
    for index in 0_u8..3 {
        let patch_id = create_file_patch(&mut store, &format!("g{index}.txt"), index + 1)?;
        let root = naive_continue(&store, &mut state, &mut text_cache, &[patch_id])?;
        parent = write_block(&mut store, Some(parent), vec![patch_id], root)?;
    }

    // Verifying the tip must still fail -- genesis's own corruption is caught while walking the
    // lineage genesis-to-tip, before any later block's own check even runs.
    let payload = block_payload_of(&store, parent)?;
    assert!(verify_block_v2_state(&store, parent, &payload, &mut LineageStateMemo::new()).is_err());
    Ok(())
}

#[test]
fn mid_chain_corruption_is_caught_when_reached_as_a_lineage_member() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let mut state = NodeLifecycleState::new();
    let mut text_cache = TextCache::new();

    // Two genuinely valid generations first, via the real validating derivation.
    let genesis_id = write_block(&mut store, None, Vec::new(), compute_state_root(&[])?)?;
    let mut parent = genesis_id;
    for index in 0_u8..2 {
        let patch_id = create_file_patch(&mut store, &format!("v{index}.txt"), index + 1)?;
        let root = derive_next_state_root(&store, Some(parent), &[patch_id])?;
        parent = write_block(&mut store, Some(parent), vec![patch_id], root)?;
        // Keep the naive-continuation state in sync with the valid chain so it is the correct
        // starting point once corruption is introduced below.
        naive_continue(&store, &mut state, &mut text_cache, &[patch_id])?;
    }

    // Mid-chain block (position 3) claims WRONG_ROOT.
    let corrupted_patch = create_file_patch(&mut store, "corrupted.txt", 200)?;
    let corrupted_id = write_block(&mut store, Some(parent), vec![corrupted_patch], WRONG_ROOT)?;

    // Two more naively-continued generations on top of the corruption.
    let mut parent = corrupted_id;
    for index in 0..2 {
        let patch_id = create_file_patch(&mut store, &format!("after{index}.txt"), 210 + index)?;
        let root = naive_continue(&store, &mut state, &mut text_cache, &[patch_id])?;
        parent = write_block(&mut store, Some(parent), vec![patch_id], root)?;
    }

    let payload = block_payload_of(&store, parent)?;
    assert!(verify_block_v2_state(&store, parent, &payload, &mut LineageStateMemo::new()).is_err());
    Ok(())
}

#[test]
fn tip_corruption_is_caught() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let genesis_id = write_block(&mut store, None, Vec::new(), compute_state_root(&[])?)?;
    let mut parent = genesis_id;
    for index in 0_u8..3 {
        let patch_id = create_file_patch(&mut store, &format!("t{index}.txt"), index + 1)?;
        let root = derive_next_state_root(&store, Some(parent), &[patch_id])?;
        parent = write_block(&mut store, Some(parent), vec![patch_id], root)?;
    }

    // The tip itself, freshly written with a wrong root -- every ancestor above it is genuinely
    // valid, so only the tip's own check should fail.
    let tip_patch = create_file_patch(&mut store, "tip.txt", 250)?;
    let tip_id = write_block(&mut store, Some(parent), vec![tip_patch], WRONG_ROOT)?;

    let payload = block_payload_of(&store, tip_id)?;
    assert!(verify_block_v2_state(&store, tip_id, &payload, &mut LineageStateMemo::new()).is_err());
    Ok(())
}

/// DC-90 review, restated for DC-92: a memo entry must reflect *every* check the unmemoized path
/// performs, not only replay-and-compare. This proves the shape check specifically: a mid-chain
/// block with an invalid shape (a `Normal` block naming two parents) is reached as a **lineage
/// member** of a later, otherwise-fine block -- never as the outer loop's own primary subject --
/// and verifying that later block must still fail.
#[test]
fn shape_violation_at_a_lineage_member_position_is_caught() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let genesis_id = write_block(&mut store, None, Vec::new(), compute_state_root(&[])?)?;
    // A second, genuinely distinct Root block -- carries one real patch so its content (and
    // therefore its object id) differs from `genesis_id`'s; two structurally-identical empty Root
    // blocks would collide on the same content-addressed id and defeat the two-distinct-parents
    // setup this test needs.
    let other_patch = create_file_patch(&mut store, "other-root.txt", 219)?;
    let other_root = naive_continue(
        &store,
        &mut NodeLifecycleState::new(),
        &mut TextCache::new(),
        &[other_patch],
    )?;
    let other_parent = write_block(&mut store, None, vec![other_patch], other_root)?;

    // A `Normal` block (kind default from `write_block` when a parent is given) cannot legally
    // carry two parent ids -- construct that shape violation directly, bypassing `write_block`'s
    // single-parent-only shape so the payload is exactly the invalid one under test.
    //
    // Deliberately **not** using `WRONG_ROOT` here: `state_derivation_parent`'s own rule for a
    // (would-be) `Normal` block is "first of `parent_block_ids`, no shape opinion" -- if this
    // block's claimed root were simply wrong, a memo that skipped shape validation but still ran
    // replay-and-compare would *also* catch it, and the test would prove nothing about shape
    // specifically. The claimed root is instead the **true** root that "first sorted parent, plus
    // this block's own patch" would produce -- correct under replay, wrong only in shape -- so
    // disabling shape validation alone is what this test isolates.
    let mut parents = vec![genesis_id, other_parent];
    parents.sort();
    let invalid_shape_patch = create_file_patch(&mut store, "shape.txt", 220)?;
    let (mut first_parent_state, mut first_parent_text_cache) =
        if parents.first() == Some(&genesis_id) {
            (NodeLifecycleState::new(), TextCache::new())
        } else {
            let mut state = NodeLifecycleState::new();
            let mut text_cache = TextCache::new();
            naive_continue(&store, &mut state, &mut text_cache, &[other_patch])?;
            (state, text_cache)
        };
    let true_root_if_shape_ignored = naive_continue(
        &store,
        &mut first_parent_state,
        &mut first_parent_text_cache,
        &[invalid_shape_patch],
    )?;
    let invalid_payload = BlockPayload {
        parent_block_ids: parents,
        kind: BlockKind::Normal,
        patch_ids: vec![invalid_shape_patch],
        state_merkle_root: true_root_if_shape_ignored,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let invalid_envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, invalid_payload.to_canonical_bytes()?);
    let invalid_id = store.write_object(&invalid_envelope)?;

    // One more, otherwise-ordinary generation naming the shape-invalid block as its parent --
    // continuing from `first_parent_state`/`first_parent_text_cache`, which already reflect the
    // shape-invalid block's own (replay-correct) transition applied above.
    let child_patch = create_file_patch(&mut store, "child.txt", 221)?;
    let root = naive_continue(
        &store,
        &mut first_parent_state,
        &mut first_parent_text_cache,
        &[child_patch],
    )?;
    let child_id = write_block(&mut store, Some(invalid_id), vec![child_patch], root)?;

    let payload = block_payload_of(&store, child_id)?;
    assert!(
        verify_block_v2_state(&store, child_id, &payload, &mut LineageStateMemo::new()).is_err()
    );
    Ok(())
}

fn block_payload_of(
    store: &MemoryObjectStore,
    block_id: ObjectId,
) -> prikk_error::Result<BlockPayload> {
    let envelope = crate::ObjectReader::read_object(store, block_id)?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("test block missing".to_string()))?;
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

/// DC-92 §4.2 review: proves boundedness itself, not merely that results stay correct. `BRANCHES`
/// independent chains of `DEPTH` blocks each, all forking from one shared genesis -- the peak number
/// of `LineageStateMemo` entries live at any point must track open-branch count, not total history
/// length. Submitted in ObjectId order (`blocks.sort_by_key`), matching what `verify`'s real Phase A
/// scan actually collects -- not lineage order -- so this also proves the algorithm's own dependency
/// ordering, not accidental input-order locality, is what bounds memory.
#[test]
fn multi_branch_history_bounds_peak_memo_size_by_branch_count_not_block_count()
-> prikk_error::Result<()> {
    const BRANCHES: usize = 4;
    const DEPTH: usize = 10;

    let mut store = MemoryObjectStore::new();
    let genesis_id = write_block(&mut store, None, Vec::new(), compute_state_root(&[])?)?;
    let mut blocks = vec![(genesis_id, block_payload_of(&store, genesis_id)?)];

    for branch in 0..BRANCHES {
        let mut parent = genesis_id;
        for step in 0..DEPTH {
            let seed = (branch * DEPTH + step + 1) as u8;
            let patch_id = create_file_patch(&mut store, &format!("b{branch}-{step}.txt"), seed)?;
            let root = derive_next_state_root(&store, Some(parent), &[patch_id])?;
            let block_id = write_block(&mut store, Some(parent), vec![patch_id], root)?;
            blocks.push((block_id, block_payload_of(&store, block_id)?));
            parent = block_id;
        }
    }
    blocks.sort_by_key(|(id, _)| *id);
    let total = blocks.len();

    let mut memo = LineageStateMemo::new();
    let result = verify_blocks_topological(&store, &blocks, &mut memo)?;
    let peak = result.peak_memo_entries;

    assert_eq!(
        memo.len(),
        0,
        "every entry must be evicted once nothing in the batch still needs it"
    );
    assert_eq!(
        result.outcomes.len(),
        total,
        "no block may be silently absent"
    );
    assert!(
        result
            .outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, BlockStateStatus::Verified)),
        "every block in this fixture is genuinely clean: {:?}",
        result.outcomes
    );
    assert!(
        peak <= BRANCHES + 2,
        "peak live memo entries ({peak}) should track open-branch count ({BRANCHES}), not grow \
         with history depth"
    );
    assert!(
        peak < total,
        "peak ({peak}) should be far below total block count ({total}) -- proving the entries \
         were never all live at once, which is the whole point of §4.2"
    );
    Ok(())
}

/// DC-95 Stage 2 Level 2 acceptance criterion 1, at Phase B granularity: two independent bad blocks
/// in unrelated trees (no shared ancestor in the batch) are both reported `Failed`, and neither
/// suppresses the other or the rest of the batch.
#[test]
fn verify_blocks_topological_reports_two_independent_bad_blocks_in_different_subtrees()
-> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    // Distinct wrong roots -- two Root blocks with identical (empty) content would otherwise
    // canonical-encode to the same bytes and collide on the same ObjectId.
    let bad_root_a = write_block(&mut store, None, Vec::new(), WRONG_ROOT)?;
    let bad_root_b = write_block(&mut store, None, Vec::new(), MerkleRoot([0xEF; 32]))?;
    let blocks = vec![
        (bad_root_a, block_payload_of(&store, bad_root_a)?),
        (bad_root_b, block_payload_of(&store, bad_root_b)?),
    ];

    let mut memo = LineageStateMemo::new();
    let result = verify_blocks_topological(&store, &blocks, &mut memo)?;

    assert_eq!(result.outcomes.len(), 2, "no block may be silently absent");
    for outcome in &result.outcomes {
        assert!(
            matches!(outcome.status, BlockStateStatus::Failed { .. }),
            "expected block {} to be Failed, got: {:?}",
            outcome.block_id,
            outcome.status
        );
    }
    Ok(())
}

/// The correctness fix the Level 2 handoff §7 Q2 ruling required: a descendant of a failed block is
/// `NotEvaluated`, naming its own *immediate* state-derivation parent -- not the root cause, even
/// when the root cause is further back in the chain. `root` (bad) -> `child` (real dependent) ->
/// `grandchild` (real dependent of `child`, not of `root`).
///
/// **Cannot use `derive_next_state_root` for `child`/`grandchild`'s roots here** -- state derivation
/// is chain-validating by construction (`resolved_parent_state` verifies a named parent's own root
/// before deriving from it), so deriving *through* a deliberately-corrupted `root` would itself fail
/// with the same error this test is trying to isolate downstream of. Every block in this fixture has
/// zero patches, so the state never actually changes from genesis regardless of chain position --
/// `child`/`grandchild` are given the correct *empty-state* root directly, bypassing the circularity,
/// so if either is marked `NotEvaluated` it is because of this containment logic, not because its own
/// root happened to be wrong too.
#[test]
fn verify_blocks_topological_names_immediate_parent_not_root_cause() -> prikk_error::Result<()> {
    let mut store = MemoryObjectStore::new();
    let correct_empty_root = compute_state_root(&entries_from_state(&NodeLifecycleState::new())?)?;
    let root_id = write_block(&mut store, None, Vec::new(), WRONG_ROOT)?;
    let child_id = write_block(&mut store, Some(root_id), Vec::new(), correct_empty_root)?;
    let grandchild_id = write_block(&mut store, Some(child_id), Vec::new(), correct_empty_root)?;
    let blocks = vec![
        (root_id, block_payload_of(&store, root_id)?),
        (child_id, block_payload_of(&store, child_id)?),
        (grandchild_id, block_payload_of(&store, grandchild_id)?),
    ];

    let mut memo = LineageStateMemo::new();
    let result = verify_blocks_topological(&store, &blocks, &mut memo)?;

    assert_eq!(result.outcomes.len(), 3, "no block may be silently absent");
    let status_of = |block_id: ObjectId| -> &BlockStateStatus {
        &result
            .outcomes
            .iter()
            .find(|outcome| outcome.block_id == block_id)
            .unwrap_or_else(|| panic!("expected an outcome for block {block_id}, found none"))
            .status
    };
    assert!(
        matches!(status_of(root_id), BlockStateStatus::Failed { .. }),
        "expected root to be Failed, got: {:?}",
        status_of(root_id)
    );
    assert_eq!(
        status_of(child_id),
        &BlockStateStatus::NotEvaluated {
            blocked_by: root_id
        },
        "child must name its own immediate parent (root), not attempt its own check"
    );
    assert_eq!(
        status_of(grandchild_id),
        &BlockStateStatus::NotEvaluated {
            blocked_by: child_id
        },
        "grandchild must name its own immediate parent (child), not eagerly resolve to root -- \
         the root cause is discoverable by following the chain one hop at a time"
    );
    Ok(())
}

/// DC-92 §4.2 review: a cycle-detection control for `verify_blocks_topological`'s own Kahn's-
/// algorithm path specifically, distinct from `validate_v2_lineage`'s walk-based cycle detection
/// (which a genuine content-addressed object store cannot actually be forced into, since a real
/// cycle would require two objects whose ids are chosen before their mutually-referencing content is
/// known -- a hash fixed point). Constructed directly against fabricated ids via `ObjectId::from_
/// bytes`, bypassing the store entirely: the two blocks never become ready, so `verify_block_v2_
/// state` is never called and the store is never touched, matching the fact that this is caught
/// purely from the batch's own dependency structure before any replay is attempted.
#[test]
fn two_blocks_naming_each_other_as_state_parent_are_caught_as_a_cycle() -> prikk_error::Result<()> {
    let id_a = ObjectId::from_bytes([0xC1; 32]);
    let id_b = ObjectId::from_bytes([0xC2; 32]);
    let payload_a = payload(BlockKind::Normal, vec![id_b], WRONG_ROOT);
    let payload_b = payload(BlockKind::Normal, vec![id_a], WRONG_ROOT);
    let blocks = vec![(id_a, payload_a), (id_b, payload_b)];

    let store = MemoryObjectStore::new();
    let mut memo = LineageStateMemo::new();
    match verify_blocks_topological(&store, &blocks, &mut memo) {
        Ok(_) => panic!("a two-block cycle must be rejected, not silently dropped from the batch"),
        Err(error) => {
            let message = error.to_string();
            assert!(
                message.contains("lineage cycle"),
                "expected a lineage-cycle error, got: {message}"
            );
        }
    }
    Ok(())
}
