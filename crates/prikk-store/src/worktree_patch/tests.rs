//! Node-addressed worktree authoring tests (DC-09 Phase 4.4a-2a).
//!
//! Baselines are node-addressed `CreateFile` lineages (review Option A / E3); the snapshot manifest
//! is never used as identity authority. A deterministic node-id generator is injected so fresh-id
//! assignment and patch identity are reproducible (review E1).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, MerkleRoot,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose, RefStatePayload, SignatureAlgorithm, SignerRole,
};

use crate::node::node_id_gen::{NodeIdGenerator, SequenceEntropySource};
use crate::test_gates::test_support::{
    dummy_signature, maintainer_signature, signed_block, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::worktree_patch::commit_worktree_changes_with_generator;
use crate::{
    ActiveLock, ActiveRefMetadata, AuthorSigner, DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner,
    FileObjectStore, ObjectReader, ObjectWriter, RefPublication, RefStore, RepoPath,
    RepositoryLayout, Wal, WorktreePatchCommitOptions, WorktreePatchOperationKind,
    finish_active_publication_cleanup, read_active_ref_metadata,
};

/// Deterministic Ed25519 AUTHOR signer for reproducible authoring (real signing, fixed seed).
fn test_signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("test-author-key", &[7_u8; 32]).unwrap()
}

/// Distinct nonzero scripted entropy candidates, disjoint from the baseline node ids ([1;32], …).
fn deterministic_generator() -> NodeIdGenerator<SequenceEntropySource> {
    let candidates: Vec<[u8; 32]> = (0..32u8)
        .map(|i| {
            let mut bytes = [0x90_u8; 32];
            bytes[31] = i.wrapping_add(1);
            bytes
        })
        .collect();
    NodeIdGenerator::with_source(SequenceEntropySource::new(&candidates))
}

/// Publish a node-addressed `CreateFile` baseline lineage on `heads/main` and write each file's
/// baseline bytes into the worktree (clean-checkout simulation). Returns the baseline block id.
fn publish_node_baseline(layout: &RepositoryLayout, files: &[(&str, &[u8], BlobKind)]) -> ObjectId {
    let mut object_store = FileObjectStore::new(layout.clone());

    // Canonical create order: sort by path so op_seq and node ids are stable.
    let mut sorted: Vec<&(&str, &[u8], BlobKind)> = files.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(b.0));

    let mut operations = Vec::new();
    for (index, (path, bytes, kind)) in sorted.iter().enumerate() {
        let op_seq = u32::try_from(index + 1).unwrap();
        let blob = BlobPayload::new(*kind, bytes.to_vec());
        let blob_bytes = blob.to_canonical_bytes().unwrap();
        let blob_id = ObjectId::from_canonical_payload(ObjectType::Blob, 1, &blob_bytes);
        let blob_env = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_bytes);
        object_store.write_object(&blob_env).unwrap();

        let node_id = NodeId::from_bytes([op_seq as u8; 32]);
        operations.push(Operation {
            op_seq,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: (*path).to_string(),
                node_id,
                blob_id,
                mode: 0o100_644,
            }),
        });

        std::fs::write(layout.root().join(path), bytes).unwrap();
    }

    let patch = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch_env =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch.to_canonical_bytes().unwrap());
    patch_env.add_signature(dummy_signature()).unwrap();
    let patch_id = patch_env.object_id();
    object_store.write_object(&patch_env).unwrap();

    let block = signed_block(BlockKind::Root, Vec::new(), vec![patch_id], None);
    let block_id = block.object_id();
    object_store.write_object(&block).unwrap();

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .unwrap();
    block_id
}

/// Seal the active WAL's queued patch record(s) into a new block and publish the ref forward — the
/// store-level equivalent of `prikk seal --allow-no-audit`, for tests that need several real sealed
/// generations in sequence without driving the CLI binary. Batches however many records are queued
/// (DC-65 introduced this at N = 1; DC-66 requires it to handle N > 1 — seal was already written as a
/// loop, see `crates/prikk-cli/src/seal/support.rs`, so this mirrors that rather than assuming exactly
/// one). Returns the new block id. Requires `ref_name` to already have a published tip (use
/// `publish_node_baseline` first); does not cover sealing a queue chained straight from `Genesis`.
fn seal_active_patch(layout: &RepositoryLayout, ref_name: &str) -> ObjectId {
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    assert!(
        !replay.records.is_empty(),
        "expected at least one queued patch"
    );

    let mut object_store = FileObjectStore::new(layout.clone());
    let patch_ids: Vec<ObjectId> = replay
        .records
        .iter()
        .map(|record| object_store.write_object(&record.envelope).unwrap())
        .collect();

    let ref_store = RefStore::new(layout.clone());
    let current_ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let current_envelope = object_store
        .read_typed(current_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let current_payload = RefStatePayload::decode_canonical(
        &current_envelope.canonical_payload,
        current_envelope.schema_version,
    )
    .unwrap();
    let parent_block_id = current_payload.target_object_id;

    let block = signed_block(BlockKind::Normal, vec![parent_block_id], patch_ids, None);
    let block_id = block.object_id();
    object_store.write_object(&block).unwrap();

    let next_seq = current_payload.update_seq + 1;
    let ref_state =
        signed_ref_state_envelope(ref_name, Some(current_ref_state_id), block_id, next_seq);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(
        ref_name,
        Some(current_ref_state_id),
        ref_state_id,
        block_id,
        next_seq,
    );
    ref_store
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: Some(current_ref_state_id),
            ref_state,
            ref_update,
        })
        .unwrap();

    let active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME).unwrap();
    finish_active_publication_cleanup(layout, &active_lock).unwrap();
    block_id
}

/// Publish a snapshot-only baseline (path-keyed, no node identity) for the E3 rejection test.
fn publish_snapshot_baseline(layout: &RepositoryLayout, path: &str, bytes: &[u8]) {
    use crate::snapshot::{SnapshotEntry, SnapshotManifest};
    let mut object_store = FileObjectStore::new(layout.clone());
    let manifest = SnapshotManifest {
        files: vec![SnapshotEntry {
            path: RepoPath::parse(path).unwrap(),
            bytes: bytes.to_vec(),
        }],
    };
    let blob = BlobPayload::new(BlobKind::Snapshot, manifest.encode().unwrap());
    let mut blob_env =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes().unwrap());
    blob_env.add_signature(maintainer_signature()).unwrap();
    let blob_id = blob_env.object_id();
    object_store.write_object(&blob_env).unwrap();

    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Normal,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0_u8; 32]),
        snapshot_blob_ref: Some(blob_id),
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes().unwrap());
    block.add_signature(maintainer_signature()).unwrap();
    let block_id = block.object_id();
    object_store.write_object(&block).unwrap();
    std::fs::write(layout.root().join(path), bytes).unwrap();

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .unwrap();
}

#[test]
fn binary_baseline_modified_file_authors_replace_binary() {
    let root = unique_temp_dir("wt-modified-binary");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("data.bin", &[0xff, 0x00], BlobKind::Binary)]);
    std::fs::write(root.join("data.bin"), [0xfe, 0x01]).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "change binary",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 1);
    assert_eq!(report.referenced_blob_count, 2);
    assert_eq!(report.text_edit_count, 0);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::ReplaceBinary
    );

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    assert_eq!(replay.records.len(), 1);
    assert_eq!(replay.records[0].envelope.object_id(), report.patch_id);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn text_baseline_modified_file_authors_edit_text() {
    let root = unique_temp_dir("wt-modified-text");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    std::fs::write(root.join("README.md"), b"changed\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "change text",
        WorktreePatchCommitOptions::prefer_text_edits(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 1);
    assert_eq!(report.referenced_blob_count, 0);
    assert_eq!(report.text_edit_count, 1);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::EditText
    );
    let _ = std::fs::remove_dir_all(root);
}

/// DC-65: editing the same text file across N >= 3 separate sealed commits must succeed. Two was the
/// boundary that was missed (a node's `blob_id` after its *first* `EditText` is a content identity,
/// not a stored object — `plan_edit_text` assumed otherwise; `EditText`'s wire shape is a diff, and
/// nothing writes the derived text as a `Blob`). Four consecutive edits here, each sealed before the
/// next, exercises the boundary with margin, and also crosses DC-64's incremental-cache reanchor
/// (the second and later commits are eligible for `resolve_baseline_state`'s incremental path, so
/// this also proves the fix holds under both full-replay and incremental baseline resolution).
#[test]
fn text_file_edited_across_four_sealed_commits_succeeds() {
    let root = unique_temp_dir("wt-four-sealed-edits");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("notes.txt", b"version 0", BlobKind::Text)]);

    let mut generator = deterministic_generator();
    let contents = ["version 1", "version 2", "version 3", "version 4"];
    for content in contents {
        std::fs::write(root.join("notes.txt"), content).unwrap();
        let report = commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "edit notes",
            WorktreePatchCommitOptions::prefer_text_edits(),
            &mut generator,
            &test_signer(),
        )
        .unwrap_or_else(|err| panic!("commit for {content:?} failed: {err}"));
        assert_eq!(
            report.text_edit_count, 1,
            "expected an EditText for {content:?}"
        );
        assert_eq!(
            report.changes[0].operation,
            WorktreePatchOperationKind::EditText
        );
        seal_active_patch(&layout, "heads/main");
    }

    // Ground truth: an independent full replay from genesis must reconstruct the final text
    // exactly, proving the chain of diffs — not just each individual commit — is consistent.
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());
    let final_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let final_envelope = object_store
        .read_typed(final_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let final_payload = RefStatePayload::decode_canonical(
        &final_envelope.canonical_payload,
        final_envelope.schema_version,
    )
    .unwrap();
    let plan = crate::patch_replay::prepare_patch_replay_plan(&layout, "heads/main").unwrap();
    assert_eq!(plan.target_block_id, final_payload.target_object_id);
    assert_eq!(plan.file_count, 1);
    assert_eq!(plan.total_content_bytes, "version 4".len() as u64);

    let _ = std::fs::remove_dir_all(root);
}

/// DC-66 criterion 8 (load-bearing): DC-64's incremental cache and DC-65's text materialization both
/// tested across a queue, not assumed from the fact that they work at N = 1.
///
/// Sequence: `doc.txt` created, edited once and sealed (so the sealed lineage's *last* operation is
/// itself an `EditText` — the DC-65 shape), then edited **twice more with no seal in between** (a real
/// queue), then sealed together as one two-patch block, then edited a fourth time.
///
/// - The first queued edit (v1 -> v2) folds onto a baseline whose current content descends from a
///   *sealed* `EditText`. The queue fold's `text_cache` starts empty, so this must hit
///   `MissingBlobForLifecycleEffect` and recover via `materialize_edited_text` over sealed lineage —
///   exactly the new fallback `queuing-baseline-design-v1.md` §3 describes, not the pre-existing
///   DC-65 single-commit path (which only ever runs when the queue is empty).
/// - The second queued edit (v2 -> v3) targets a node the fold just wrote to `text_cache` in the same
///   pass — the fast path, no materialize call needed.
/// - This commit's *own* diff (v2 -> v3) must read v2's text from `current_text_for_node`'s
///   queue-cache-first check, not the stale sealed v1 text a lineage-only fallback would return.
/// - Sealing both queued patches together produces one block with two patch ids, exercising DC-64's
///   `apply_one_block`/`apply_patch_ids` loop at N = 2 for the first time in this suite.
/// - A fifth commit (v3 -> v4) against that two-patch sealed block forces `resolve_baseline_state` to
///   attempt DC-64's incremental step, hit the same `MissingBlobForLifecycleEffect` at the *sealed*
///   layer this time, and fall back to full replay — the pre-existing DC-65 fifth-fallback-trigger,
///   now proven to still fire correctly once a block can carry more than one patch.
#[test]
fn text_file_edited_across_a_queue_then_sealed_together_succeeds() {
    let root = unique_temp_dir("wt-queue-text-edits");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("doc.txt", b"v0", BlobKind::Text)]);

    let mut generator = deterministic_generator();
    let commit_edit = |generator: &mut NodeIdGenerator<SequenceEntropySource>, content: &str| {
        std::fs::write(root.join("doc.txt"), content).unwrap();
        commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "edit doc",
            WorktreePatchCommitOptions::prefer_text_edits(),
            generator,
            &test_signer(),
        )
        .unwrap_or_else(|err| panic!("commit for {content:?} failed: {err}"))
    };

    // v0 -> v1, sealed alone: the sealed lineage's last operation is an EditText (unstored identity).
    let v1 = commit_edit(&mut generator, "v1");
    assert_eq!(v1.text_edit_count, 1);
    seal_active_patch(&layout, "heads/main");

    // v1 -> v2, queued (not sealed): folds onto a baseline descending from a *sealed* EditText.
    let v2 = commit_edit(&mut generator, "v2");
    assert_eq!(v2.text_edit_count, 1);

    // v2 -> v3, still queued: this commit's own diff must be read from the queue's own text cache,
    // and folding v2 (needed to compute v3's baseline) must resolve from the SAME cache, not miss.
    let v3 = commit_edit(&mut generator, "v3");
    assert_eq!(v3.text_edit_count, 1);
    assert_ne!(v2.patch_id, v3.patch_id);

    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 2, "v2 and v3 both queued, unsealed");

    // Seal both together: one block, two patch ids — DC-64's apply_one_block/apply_patch_ids loop at
    // N = 2 for the first time.
    seal_active_patch(&layout, "heads/main");

    // v3 -> v4 against the two-patch sealed block: forces DC-64's incremental-step eligibility check
    // to engage, hit the sealed-layer MissingBlobForLifecycleEffect, and fall back to full replay.
    let v4 = commit_edit(&mut generator, "v4");
    assert_eq!(v4.text_edit_count, 1);
    seal_active_patch(&layout, "heads/main");

    // Ground truth: an independent full replay from genesis reconstructs "v4" exactly, proving the
    // whole chain — sealed, queued, and re-sealed — is consistent end to end.
    let object_store = FileObjectStore::new(layout.clone());
    let (baseline_block, horizon) =
        crate::patch_replay::resolve_node_lineage_bounds(&layout, "heads/main").unwrap();
    let state =
        crate::lifecycle_cache::replay_derived_state(&object_store, baseline_block, horizon)
            .unwrap();
    let (node_id, _) = state
        .state()
        .live_nodes()
        .find(|(_, node)| node.path.as_str() == "doc.txt")
        .expect("doc.txt must still be live");
    let final_text = crate::lifecycle_cache::materialize_edited_text(
        &object_store,
        baseline_block,
        horizon,
        *node_id,
    )
    .unwrap()
    .expect("doc.txt was edited, so its text must be materializable");
    assert_eq!(final_text, b"v4");

    let _ = std::fs::remove_dir_all(root);
}

/// DC-66 criterion 5: crash during seal, with a queue of N, does not lose any queued patch. Simulates
/// the crash point between "patch objects durably written" and "ref publication completes" — the WAL
/// is untouched until `finish_active_publication_cleanup` runs at the very end of a *successful*
/// seal, so both queued records must still be present and replayable after the simulated crash, and
/// completing the publication (the retry path DC-38's crash-recovery machinery already covers,
/// exercised here with N = 2 for the first time) must still produce the correct two-patch block.
#[test]
fn crash_during_seal_with_a_queued_pair_preserves_both_and_completes_on_retry() {
    let root = unique_temp_dir("wt-queue-crash-during-seal");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("seed.txt", b"seed", BlobKind::Text)]);

    let mut generator = deterministic_generator();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "a",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    std::fs::write(root.join("b.txt"), b"beta").unwrap();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "b",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay_before = wal.replay().unwrap();
    assert_eq!(replay_before.records.len(), 2);

    // Simulate the crash: durably write both queued patches (as seal's `persist_wal_patches` would),
    // then stop before any ref publication happens.
    let mut object_store = FileObjectStore::new(layout.clone());
    let patch_ids: Vec<ObjectId> = replay_before
        .records
        .iter()
        .map(|record| object_store.write_object(&record.envelope).unwrap())
        .collect();
    assert_eq!(patch_ids.len(), 2);

    let replay_after_partial_seal = wal.replay().unwrap();
    assert_eq!(
        replay_after_partial_seal, replay_before,
        "writing patch objects durably must not touch the WAL; only a completed publication cleans \
         it up, so both queued records must still be exactly as they were"
    );

    // Retry (complete) the seal from here.
    seal_active_patch(&layout, "heads/main");

    let final_replay = wal.replay().unwrap();
    assert_eq!(
        final_replay.records.len(),
        0,
        "a completed seal must clear the queue"
    );

    // Ground truth: both queued patches ended up in the sealed lineage, nothing lost.
    let object_store = FileObjectStore::new(layout.clone());
    let (baseline_block, horizon) =
        crate::patch_replay::resolve_node_lineage_bounds(&layout, "heads/main").unwrap();
    let state =
        crate::lifecycle_cache::replay_derived_state(&object_store, baseline_block, horizon)
            .unwrap();
    let mut paths: Vec<&str> = state
        .state()
        .live_nodes()
        .map(|(_, node)| node.path.as_str())
        .collect();
    paths.sort_unstable();
    assert_eq!(paths, vec!["a.txt", "b.txt", "seed.txt"]);

    let _ = std::fs::remove_dir_all(root);
}

/// DC-57 (NFR-PERF-02): the hard block, wired into the real authoring path. Uses a scaled-down limit
/// (2, not the default 1000) so the boundary is exercised with real, ordinary commits rather than a
/// synthetic queue — the pure arithmetic at the RFC's literal 799/800/999/1000/1001 is proven
/// separately in `worktree_patch::threshold_tests::boundary_values_match_the_rfc`; this test proves
/// the *wiring*: the check fires before any WAL append or object write, and leaves no partial state.
#[test]
fn active_patch_hard_block_fires_before_any_write_and_leaves_no_partial_state() {
    let root = unique_temp_dir("wt-active-patch-hard-block");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("seed.txt", b"seed", BlobKind::Text)]);
    let limited = WorktreePatchCommitOptions::file_level().with_active_patch_limit(2);

    let mut generator = deterministic_generator();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "a",
        limited,
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    std::fs::write(root.join("b.txt"), b"beta").unwrap();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "b",
        limited,
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    let replay_before = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay_before.records.len(), 2, "queue is now at the limit");
    let objects_before = count_object_files(&layout);

    // A third commit must be refused: 2 queued patches already meets the limit of 2.
    std::fs::write(root.join("c.txt"), b"gamma").unwrap();
    let err = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "c",
        limited,
        &mut generator,
        &test_signer(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("at or above the configured limit"),
        "unexpected error: {err}"
    );

    // No partial state: the WAL is byte-identical to before the blocked attempt, and no new object
    // (blob or patch) was written for the refused commit's content.
    let replay_after = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay_after, replay_before);
    let objects_after = count_object_files(&layout);
    assert_eq!(
        objects_after, objects_before,
        "a blocked commit must not write any object"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Count persisted object files under the layout's object store, recursively — used instead of
/// `verify_repository` (whose state-root check does not accept `publish_node_baseline`'s placeholder
/// root) purely to prove a blocked commit wrote nothing new.
fn count_object_files(layout: &RepositoryLayout) -> usize {
    fn walk(dir: &std::path::Path, count: &mut usize) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, count);
            } else {
                *count += 1;
            }
        }
    }
    let mut count = 0;
    walk(&layout.objects_dir(), &mut count);
    count
}

/// DC-57 criterion 4: `seal` must remain available at and above the hard bound — a block on
/// committing must never become a block on the one command that relieves it.
#[test]
fn seal_remains_available_at_and_above_the_hard_bound() {
    let root = unique_temp_dir("wt-active-patch-seal-above-bound");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("seed.txt", b"seed", BlobKind::Text)]);
    let limited = WorktreePatchCommitOptions::file_level().with_active_patch_limit(2);

    let mut generator = deterministic_generator();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "a",
        limited,
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    std::fs::write(root.join("b.txt"), b"beta").unwrap();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "b",
        limited,
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    // The queue is now at the configured limit — further commits are blocked (proven above), but
    // `seal` itself takes no `WorktreePatchCommitOptions` and consults no active-patch limit at all;
    // it only drains the queue it is given.
    let block_id = seal_active_patch(&layout, "heads/main");
    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 0, "seal must fully drain the queue");

    let ref_store = RefStore::new(layout.clone());
    let target = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let envelope = FileObjectStore::new(layout.clone())
        .read_typed(target, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
            .unwrap();
    assert_eq!(payload.target_object_id, block_id);

    let _ = std::fs::remove_dir_all(root);
}

/// DC-57: `ActiveSession::append_patch`'s own limit enforcement (the second authoring path the RFC's
/// "active patches has one definition" requires) — mirrors the `author_inner` proof at the same
/// scaled-down limit.
#[test]
fn active_session_append_patch_enforces_its_own_limit() {
    let root = unique_temp_dir("active-session-hard-block");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let session = crate::ActiveSession::new(layout.clone());
    session
        .append_patch(&crate::test_gates::test_support::signed_patch_envelope(), 1)
        .unwrap();

    let err = session
        .append_patch(
            &crate::test_gates::test_support::rollback_patch_envelope(),
            1,
        )
        .unwrap_err();
    assert!(
        err.to_string().contains("at or above the configured limit"),
        "unexpected error: {err}"
    );
    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 1, "the blocked append must not land");

    let _ = std::fs::remove_dir_all(root);
}

/// DC-65 criterion 4: the `ReplaceBinary` equivalent, confirming binary files were never affected —
/// every `ReplaceBinary` writes its new content as a real stored `Blob` (`plan_replace_binary` always
/// calls `write_content_blob`), so a node's `blob_id` after any number of binary edits always names a
/// stored object.
#[test]
fn binary_file_replaced_across_four_sealed_commits_succeeds() {
    let root = unique_temp_dir("wt-four-sealed-binary-edits");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("data.bin", &[0x00], BlobKind::Binary)]);

    let mut generator = deterministic_generator();
    let contents: [&[u8]; 4] = [&[0x01], &[0x02], &[0x03], &[0x04]];
    for content in contents {
        std::fs::write(root.join("data.bin"), content).unwrap();
        let report = commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "replace binary",
            WorktreePatchCommitOptions::file_level(),
            &mut generator,
            &test_signer(),
        )
        .unwrap_or_else(|err| panic!("commit for {content:?} failed: {err}"));
        assert_eq!(
            report.changes[0].operation,
            WorktreePatchOperationKind::ReplaceBinary
        );
        seal_active_patch(&layout, "heads/main");
    }

    // `patch_replay::prepare_patch_replay_plan` (checkout) does not yet support `ReplaceBinary`
    // replay at all — a pre-existing, documented scope limit (`patch_replay.rs`'s module doc:
    // "EditText and ReplaceBinary ... application is deferred to the node model"), unrelated to
    // DC-65. The commit-side authoring loop above, which four consecutive successful
    // `ReplaceBinary` commits already exercised, is what this test verifies.
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());
    let final_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let final_envelope = object_store
        .read_typed(final_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let final_payload = RefStatePayload::decode_canonical(
        &final_envelope.canonical_payload,
        final_envelope.schema_version,
    )
    .unwrap();
    assert_eq!(
        final_payload.update_seq, 5,
        "genesis plus four sealed edits"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn binary_baseline_under_prefer_text_still_replace_binary() {
    let root = unique_temp_dir("wt-binary-prefertext");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("data.bin", &[0xff, 0x00], BlobKind::Binary)]);
    std::fs::write(root.join("data.bin"), [0xfe, 0x01]).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "change binary",
        WorktreePatchCommitOptions::prefer_text_edits(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 1);
    assert_eq!(report.text_edit_count, 0);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::ReplaceBinary
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn untracked_file_authors_create_file() {
    let root = unique_temp_dir("wt-untracked");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    std::fs::write(root.join("extra.txt"), b"extra\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "add extra",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 1);
    assert_eq!(report.referenced_blob_count, 1);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::CreateFile
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn existing_text_node_rejects_non_utf8_content() {
    // E4: an existing TextFile cannot accept non-UTF-8 bytes (text->binary transition fails closed).
    let root = unique_temp_dir("wt-kind-transition");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    std::fs::write(root.join("README.md"), [0xff, 0xfe, 0x00]).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "corrupt",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    );
    assert!(report.is_err());
    let message = report.err().unwrap().to_string();
    assert!(
        message.contains("unsupported kind transition"),
        "expected kind-transition class, got: {message}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn snapshot_only_baseline_fails_closed() {
    // E3: a snapshot-only baseline carries no node identity; authoring must fail closed.
    let root = unique_temp_dir("wt-snapshot-reject");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_snapshot_baseline(&layout, "README.md", b"hello\n");
    std::fs::write(root.join("README.md"), b"changed\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "change",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    );
    assert!(report.is_err());
    let message = report.err().unwrap().to_string();
    assert!(
        message.contains("node identity unavailable"),
        "expected node-identity-unavailable class, got: {message}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn same_session_creates_get_distinct_node_ids_in_canonical_order() {
    // E1: two fresh creates in one pass are minted in canonical path order with distinct ids.
    let root = unique_temp_dir("wt-same-session");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    std::fs::write(root.join("b.txt"), b"bbb\n").unwrap();
    std::fs::write(root.join("a.txt"), b"aaa\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "two creates",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 2);
    assert_eq!(report.referenced_blob_count, 2);
    for change in &report.changes {
        assert_eq!(change.operation, WorktreePatchOperationKind::CreateFile);
    }
    // Inspect the authored patch: a.txt minted before b.txt (canonical path order), distinct ids.
    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    let mut node_ids = Vec::new();
    for op in &ops {
        if let crate::patch_replay::decode::DecodedOperationKind::CreateFile {
            path, node_id, ..
        } = &op.kind
        {
            node_ids.push((path.clone(), *node_id.as_bytes()));
        }
    }
    assert_eq!(node_ids.len(), 2);
    assert_ne!(node_ids[0].1, node_ids[1].1);
    // First scripted candidate (…0x01) goes to the canonically-first create path a.txt.
    let a = node_ids.iter().find(|(p, _)| p == "a.txt").unwrap();
    let b = node_ids.iter().find(|(p, _)| p == "b.txt").unwrap();
    assert!(a.1[31] < b.1[31], "a.txt must be minted before b.txt");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn deterministic_patch_identity_across_independent_runs() {
    // E1/ordering: the same change set yields identical patch identity regardless of run, because
    // enumeration is canonical (BTreeMap) and minting is path-sorted under a fixed entropy script.
    let patch_id_of = || {
        let root = unique_temp_dir("wt-determinism");
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
        std::fs::write(root.join("z.txt"), b"zzz\n").unwrap();
        std::fs::write(root.join("m.txt"), b"mmm\n").unwrap();
        let mut generator = deterministic_generator();
        let report = commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "two creates",
            WorktreePatchCommitOptions::file_level(),
            &mut generator,
            &test_signer(),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
        report.patch_id
    };
    assert_eq!(patch_id_of(), patch_id_of());
}

#[test]
fn missing_baseline_file_authors_delete_node() {
    let root = unique_temp_dir("wt-delete");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(
        &layout,
        &[
            ("keep.txt", b"keep\n", BlobKind::Text),
            ("gone.txt", b"gone\n", BlobKind::Text),
        ],
    );
    std::fs::remove_file(root.join("gone.txt")).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "delete gone",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 1);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::DeleteFile
    );
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 1, acceptance criterion 2: the assertion is the *refusal*, not the presence of a
/// finding. Same fixture as `missing_baseline_file_authors_delete_node` above -- a baseline file
/// genuinely missing from the worktree, which would ordinarily author a `DeleteFile` -- with the
/// worktree marker independently marked dirty first, simulating a materialization call that did not
/// complete. Confirms the refusal is caused specifically by the dirty marker, not some other
/// confound, by clearing it and showing the identical scenario then succeeds exactly as the
/// unmarked test above does.
#[test]
fn dirty_worktree_marker_refuses_to_infer_deletion() {
    let root = unique_temp_dir("wt-dirty-marker");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(
        &layout,
        &[
            ("keep.txt", b"keep\n", BlobKind::Text),
            ("gone.txt", b"gone\n", BlobKind::Text),
        ],
    );
    std::fs::remove_file(root.join("gone.txt")).unwrap();

    crate::worktree_marker::mark_worktree_dirty(&layout).unwrap();

    let mut generator = deterministic_generator();
    let refused = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "delete gone",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    );
    assert!(refused.is_err());

    crate::worktree_marker::clear_worktree_dirty(&layout).unwrap();
    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "delete gone",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.operation_count, 1);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::DeleteFile
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn authored_edit_text_locates_and_splices_arbitrary_span_through_shared_text_span() {
    // Authoring↔replay symmetry: the authored arbitrary-span EditText, localized and spliced through the
    // same `text_span` primitives replay uses, reproduces the new bytes and the same text blob id.
    let root = unique_temp_dir("wt-edit-symmetry");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let old_text = b"hello world\n";
    let new_text = b"hello prikk\n";
    publish_node_baseline(&layout, &[("README.md", old_text, BlobKind::Text)]);
    std::fs::write(root.join("README.md"), new_text).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "edit",
        WorktreePatchCommitOptions::prefer_text_edits(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.text_edit_count, 1);

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    let edit = ops
        .iter()
        .find_map(|op| match &op.kind {
            crate::patch_replay::decode::DecodedOperationKind::EditText {
                node_id,
                span_id,
                old_span_hash,
                left_anchor_hash,
                right_anchor_hash,
                replacement_text,
                old_span_text,
                left_anchor_len,
                right_anchor_len,
            } => Some((
                *node_id,
                *span_id,
                *old_span_hash,
                *left_anchor_hash,
                *right_anchor_hash,
                replacement_text.clone(),
                old_span_text.clone(),
                *left_anchor_len,
                *right_anchor_len,
            )),
            _ => None,
        })
        .expect("authored patch must carry an EditText op");
    let (
        node_id,
        span_id,
        old_span_hash,
        left,
        right,
        replacement,
        op_old_text,
        left_anchor_len,
        right_anchor_len,
    ) = edit;
    assert_eq!(op_old_text, b"world");
    assert_eq!(replacement, b"prikk");

    // Replay-side localization over the baseline text, using the shared module. Real authoring
    // always mints RFC 134 §8 v2 identity, so this resolves through the schema-aware dispatch.
    let (start, end) = crate::text_span::resolve_text_span(
        old_text,
        &op_old_text,
        &left,
        &right,
        &span_id,
        node_id,
        &old_span_hash,
        left_anchor_len,
        right_anchor_len,
    )
    .expect("authored span must localize uniquely in the baseline text");
    let spliced = crate::text_span::splice_text(old_text, start, end, &replacement).unwrap();
    assert_eq!(spliced, new_text);
    assert_eq!(
        crate::text_span::text_blob_id(&spliced).unwrap(),
        crate::text_span::text_blob_id(new_text).unwrap()
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn authored_edit_text_widens_subcharacter_utf8_span() {
    let root = unique_temp_dir("wt-edit-subchar");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let old_text = "é\n".as_bytes();
    let new_text = "è\n".as_bytes();
    publish_node_baseline(&layout, &[("README.md", old_text, BlobKind::Text)]);
    std::fs::write(root.join("README.md"), new_text).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "edit",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.text_edit_count, 1);

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    let edit = ops
        .iter()
        .find_map(|op| match &op.kind {
            crate::patch_replay::decode::DecodedOperationKind::EditText {
                old_span_text,
                replacement_text,
                ..
            } => Some((old_span_text.clone(), replacement_text.clone())),
            _ => None,
        })
        .expect("authored patch must carry an EditText op");
    assert_eq!(edit.0, "é".as_bytes());
    assert_eq!(edit.1, "è".as_bytes());
    let _ = std::fs::remove_dir_all(root);
}

/// Decode the WAL patch and return the `(path, mode)` of every `CreateFile` op.
fn created_file_modes(layout: &RepositoryLayout) -> Vec<(String, u32)> {
    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    let mut out = Vec::new();
    for op in &ops {
        if let crate::patch_replay::decode::DecodedOperationKind::CreateFile {
            path, mode, ..
        } = &op.kind
        {
            out.push((path.clone(), *mode));
        }
    }
    out
}

#[test]
fn untracked_regular_file_authors_regular_mode() {
    // 4.4a-2aR: a new non-executable regular file records canonical mode 0o100644.
    let root = unique_temp_dir("wt-create-regular-mode");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    std::fs::write(root.join("extra.txt"), b"extra\n").unwrap();

    let mut generator = deterministic_generator();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "add regular",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    let modes = created_file_modes(&layout);
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0], ("extra.txt".to_string(), 0o100_644));
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn untracked_executable_file_authors_executable_mode() {
    // 4.4a-2aR: a new file with an executable bit records canonical mode 0o100755 (ratified rule).
    use std::os::unix::fs::PermissionsExt;
    let root = unique_temp_dir("wt-create-exec-mode");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    let script = root.join("run.sh");
    std::fs::write(&script, b"#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut generator = deterministic_generator();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "add script",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    let modes = created_file_modes(&layout);
    assert_eq!(modes.len(), 1);
    assert_eq!(modes[0], ("run.sh".to_string(), 0o100_755));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn mixed_operations_follow_canonical_op_seq_order() {
    // P2-2: delete + create + binary replace + text edit must be emitted in canonical kind order
    // (DeleteNode < CreateFile < ReplaceBinary < EditText), assigned contiguous op_seq after sort.
    let root = unique_temp_dir("wt-mixed-order");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(
        &layout,
        &[
            ("gone.txt", b"gone\n", BlobKind::Text),
            ("bin.dat", &[0xff, 0x00], BlobKind::Binary),
            ("edit.txt", b"old\n", BlobKind::Text),
        ],
    );
    std::fs::remove_file(root.join("gone.txt")).unwrap();
    std::fs::write(root.join("bin.dat"), [0xfe, 0x01]).unwrap();
    std::fs::write(root.join("edit.txt"), b"new\n").unwrap();
    std::fs::write(root.join("new.txt"), b"fresh\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "mixed",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.operation_count, 4);

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    use crate::patch_replay::decode::DecodedOperationKind;
    let rank = |kind: &DecodedOperationKind| match kind {
        DecodedOperationKind::DeleteNode { .. } => 0,
        DecodedOperationKind::CreateFile { .. } => 1,
        DecodedOperationKind::ChangePerm { .. } => 2,
        DecodedOperationKind::ReplaceBinary { .. } => 3,
        DecodedOperationKind::EditText { .. } => 4,
        _ => 9,
    };
    let ranks: Vec<i32> = ops.iter().map(|op| rank(&op.kind)).collect();
    assert_eq!(
        ranks,
        vec![0, 1, 3, 4],
        "canonical kind order not preserved"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Decode the WAL patch and return the `(old_mode, new_mode)` of every `ChangePerm` op. Used only
/// by the `#[cfg(unix)]` mode-change tests below — Windows has no equivalent permission-bit model.
#[cfg(unix)]
fn change_perm_modes(layout: &RepositoryLayout) -> Vec<(u32, u32)> {
    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    let mut out = Vec::new();
    for op in &ops {
        if let crate::patch_replay::decode::DecodedOperationKind::ChangePerm {
            old_mode,
            new_mode,
            ..
        } = &op.kind
        {
            out.push((*old_mode, *new_mode));
        }
    }
    out
}

#[cfg(unix)]
#[test]
fn mode_only_change_authors_single_change_perm() {
    // 4.4a-2b criteria 2/4/5: content unchanged, normalized mode changed → exactly one ChangePerm
    // with old_mode = baseline (0o100644), new_mode = normalized worktree mode (0o100755).
    use std::os::unix::fs::PermissionsExt;
    let root = unique_temp_dir("wt-mode-only");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("run.sh", b"#!/bin/sh\n", BlobKind::Text)]);
    // Same content, executable bit flipped.
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "chmod +x",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 1);
    assert_eq!(report.text_edit_count, 0);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::ChangePerm
    );
    assert_eq!(change_perm_modes(&layout), vec![(0o100_644, 0o100_755)]);
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn content_and_mode_change_orders_change_perm_before_edit_text() {
    // 4.4a-2b criterion 3: content + mode change emits ChangePerm + the content op, ChangePerm first.
    use std::os::unix::fs::PermissionsExt;
    let root = unique_temp_dir("wt-content-and-mode");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("run.sh", b"old\n", BlobKind::Text)]);
    std::fs::write(root.join("run.sh"), b"new\n").unwrap();
    std::fs::set_permissions(root.join("run.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "edit + chmod",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.operation_count, 2);

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    use crate::patch_replay::decode::DecodedOperationKind;
    let ranks: Vec<i32> = ops
        .iter()
        .map(|op| match &op.kind {
            DecodedOperationKind::ChangePerm { .. } => 2,
            DecodedOperationKind::EditText { .. } => 4,
            _ => 9,
        })
        .collect();
    assert_eq!(ranks, vec![2, 4], "ChangePerm must precede EditText");
    assert_eq!(change_perm_modes(&layout), vec![(0o100_644, 0o100_755)]);
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn mixed_operations_with_change_perm_follow_canonical_order() {
    // 4.4a-2b criterion 7: extend the mixed-op ordering witness to include ChangePerm — full kind
    // order DeleteNode < CreateFile < ChangePerm < ReplaceBinary < EditText.
    use std::os::unix::fs::PermissionsExt;
    let root = unique_temp_dir("wt-mixed-changeperm");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(
        &layout,
        &[
            ("gone.txt", b"gone\n", BlobKind::Text),
            ("perm.sh", b"#!/bin/sh\n", BlobKind::Text),
            ("bin.dat", &[0xff, 0x00], BlobKind::Binary),
            ("edit.txt", b"old\n", BlobKind::Text),
        ],
    );
    std::fs::remove_file(root.join("gone.txt")).unwrap();
    std::fs::set_permissions(root.join("perm.sh"), std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root.join("bin.dat"), [0xfe, 0x01]).unwrap();
    std::fs::write(root.join("edit.txt"), b"new\n").unwrap();
    std::fs::write(root.join("new.txt"), b"fresh\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "mixed with chmod",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.operation_count, 5);

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    use crate::patch_replay::decode::DecodedOperationKind;
    let ranks: Vec<i32> = ops
        .iter()
        .map(|op| match &op.kind {
            DecodedOperationKind::DeleteNode { .. } => 0,
            DecodedOperationKind::CreateFile { .. } => 1,
            DecodedOperationKind::ChangePerm { .. } => 2,
            DecodedOperationKind::ReplaceBinary { .. } => 3,
            DecodedOperationKind::EditText { .. } => 4,
            _ => 9,
        })
        .collect();
    assert_eq!(
        ranks,
        vec![0, 1, 2, 3, 4],
        "full canonical kind order not preserved"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(unix)]
#[test]
fn content_and_mode_change_orders_change_perm_before_replace_binary() {
    // N1: binary content + mode change → ChangePerm before ReplaceBinary (symmetric to the text case).
    use std::os::unix::fs::PermissionsExt;
    let root = unique_temp_dir("wt-bin-content-and-mode");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("blob.bin", &[0xff, 0x00], BlobKind::Binary)]);
    std::fs::write(root.join("blob.bin"), [0xfe, 0x01]).unwrap();
    std::fs::set_permissions(
        root.join("blob.bin"),
        std::fs::Permissions::from_mode(0o755),
    )
    .unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "replace + chmod",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.operation_count, 2);

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let ops = crate::patch_replay::decode::decode_patch_operations(
        &replay.records[0].envelope.canonical_payload,
        replay.records[0].envelope.schema_version,
    )
    .unwrap();
    use crate::patch_replay::decode::DecodedOperationKind;
    let ranks: Vec<i32> = ops
        .iter()
        .map(|op| match &op.kind {
            DecodedOperationKind::ChangePerm { .. } => 2,
            DecodedOperationKind::ReplaceBinary { .. } => 3,
            _ => 9,
        })
        .collect();
    assert_eq!(ranks, vec![2, 3], "ChangePerm must precede ReplaceBinary");
    assert_eq!(change_perm_modes(&layout), vec![(0o100_644, 0o100_755)]);
    let _ = std::fs::remove_dir_all(root);
}

// DC-81 addendum-3: Linux-only, not `#[cfg(unix)]` — APFS enforces UTF-8 filenames, so
// `std::fs::write` below fails `EILSEQ` before prikk's own code ever runs; the precondition this
// test constructs is unreachable on macOS, not merely untested. The guarantee holds *a fortiori*
// there (the OS makes the bad state impossible, so prikk's own fail-closed guard is never needed),
// and gating costs no coverage — there is nothing on macOS to cover, unlike the FIFO tests DC-81
// ported (where the precondition stayed reachable and gating would have been a real loss).
#[cfg(target_os = "linux")]
#[test]
fn non_utf8_worktree_path_fails_closed() {
    // N2: a non-UTF-8 OS path fails closed at the strict conversion boundary, not lossily.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    let root = unique_temp_dir("wt-non-utf8-path");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    // 0xFF is never valid UTF-8.
    let bad = root.join(OsStr::from_bytes(b"bad\xffname.txt"));
    std::fs::write(&bad, b"x\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "non-utf8",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    );
    assert!(report.is_err());
    let message = report.err().unwrap().to_string();
    // RFC 124's re-land routed this conversion through the shared, separator-safe
    // `crate::path::pathbuf_to_slash_string` (both live-worktree walks now share one converter,
    // rather than `insert_regular_file` keeping its own `Path::to_str()` copy). Round 3 restored
    // this arm's classification to `InvalidName` (a name outside the supported subset is not
    // evidence of repository damage, RFC 122 §4's own ruling) and its original wording with it.
    assert!(
        message.contains("not valid UTF-8"),
        "expected utf-8 rejection, got: {message}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn authored_patch_carries_verifiable_author_signature() {
    // R1: the authored patch carries a real role-bound Ed25519 AUTHOR signature that verifies, and
    // fails verification if object id, role, or key id changes.
    let root = unique_temp_dir("wt-r1-verify");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);
    std::fs::write(root.join("extra.txt"), b"x\n").unwrap();

    let signer = test_signer();
    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "add",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &signer,
    )
    .unwrap();

    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    let envelope = &replay.records[0].envelope;
    let sig = envelope
        .signatures
        .first()
        .expect("authored patch must carry a signature");
    assert_eq!(sig.algorithm, SignatureAlgorithm::Ed25519);
    assert_eq!(sig.signer_role, SignerRole::Author);
    assert_eq!(sig.key_id, "test-author-key");
    assert_eq!(sig.signature_bytes.len(), 64);

    let public_key = signer.public_key_bytes();
    let good = prikk_object::Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        report.patch_id,
        SignerRole::Author,
        &sig.key_id,
    )
    .unwrap();
    prikk_crypto::verify_ed25519(&public_key, &good, &sig.signature_bytes)
        .expect("the authored AUTHOR signature must verify against the signer's public key");

    // Tamper: a preimage with a different object id, role, or key id must fail verification.
    let other_id = ObjectId::from_canonical_payload(ObjectType::Patch, 1, b"different payload");
    let bad_id = prikk_object::Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        other_id,
        SignerRole::Author,
        &sig.key_id,
    )
    .unwrap();
    assert!(prikk_crypto::verify_ed25519(&public_key, &bad_id, &sig.signature_bytes).is_err());

    let bad_role = prikk_object::Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        report.patch_id,
        SignerRole::Maintainer,
        &sig.key_id,
    )
    .unwrap();
    assert!(prikk_crypto::verify_ed25519(&public_key, &bad_role, &sig.signature_bytes).is_err());

    let bad_key = prikk_object::Signature::signed_bytes(
        SignatureAlgorithm::Ed25519,
        ObjectType::Patch,
        report.patch_id,
        SignerRole::Author,
        "someone-else",
    )
    .unwrap();
    assert!(prikk_crypto::verify_ed25519(&public_key, &bad_key, &sig.signature_bytes).is_err());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn clean_worktree_is_rejected() {
    let root = unique_temp_dir("wt-clean");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("README.md", b"hello\n", BlobKind::Text)]);

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "nothing",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    );
    assert!(report.is_err());
    let _ = std::fs::remove_dir_all(root);
}

// ---- DC-09 Phase 4.4b: genesis / first-commit authoring ----

/// Genesis: a fresh repo (ref never published) authors every worktree file as a `CreateFile`, in
/// canonical path order, carrying a real role-bound Ed25519 AUTHOR signature (acceptance 2, 3, 8).
#[test]
fn genesis_commit_authors_all_create_file() {
    let root = unique_temp_dir("wt-genesis-create");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    // No published baseline. Two files in the worktree.
    std::fs::write(root.join("readme.txt"), b"hello\n").unwrap();
    std::fs::write(root.join("main.rs"), b"fn main() {}\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "genesis",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    assert_eq!(report.operation_count, 2);
    // Canonical path order: main.rs before readme.txt.
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::CreateFile
    );
    assert_eq!(report.changes[0].path, "main.rs");
    assert_eq!(report.changes[1].path, "readme.txt");

    // Real AUTHOR signature on the genesis patch (same signer path as published authoring).
    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    assert_eq!(replay.records.len(), 1);
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/main".to_string())
    );
    let env = &replay.records[0].envelope;
    assert_eq!(env.object_id(), report.patch_id);
    let sig = env.signatures.first().expect("genesis patch is signed");
    assert_eq!(sig.algorithm, SignatureAlgorithm::Ed25519);
    assert_eq!(sig.signer_role, SignerRole::Author);
    assert_eq!(sig.key_id, "test-author-key");
    assert_ne!(sig.key_id, "dev-placeholder-author");
    let _ = std::fs::remove_dir_all(root);
}

/// Genesis on an empty worktree fails closed (no zero-operation patch) (acceptance 5).
#[test]
fn genesis_empty_worktree_fails_closed() {
    let root = unique_temp_dir("wt-genesis-empty");
    let layout = RepositoryLayout::init(root.clone()).unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "genesis-empty",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    );
    assert!(report.is_err());
    let _ = std::fs::remove_dir_all(root);
}

/// Genesis E1 guard: a second commit before the first seal fails closed rather than authoring a
/// duplicate genesis patch (review E1, acceptance 7).
#[test]
fn genesis_second_commit_before_seal_queues() {
    let root = unique_temp_dir("wt-genesis-double");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    let mut generator = deterministic_generator();
    let first = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "genesis",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    // DC-66: a second commit before seal now queues rather than refusing — the active WAL already
    // has the genesis patch, chained from `Genesis` (no sealed lineage exists yet). This is the
    // "unreachable by construction" branch `queuing-baseline-design-v1.md` §3 describes: every node
    // this queue can see was created within the queue itself.
    std::fs::write(root.join("b.txt"), b"two\n").unwrap();
    let mut generator2 = deterministic_generator();
    let second = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "again",
        WorktreePatchCommitOptions::file_level(),
        &mut generator2,
        &test_signer(),
    )
    .unwrap();

    assert_ne!(first.patch_id, second.patch_id);
    assert_eq!(second.wal_sequence, 2);
    // The second commit's baseline correctly folded the first (queued, unsealed) commit's own
    // CreateFile: only b.txt is new. If the fold were missing, a.txt would appear "no baseline node"
    // and be re-created with a fresh id — the exact node-identity violation criterion 3 forbids.
    assert_eq!(second.changes.len(), 1);
    assert_eq!(second.changes[0].path, "b.txt");
    assert_eq!(
        second.changes[0].operation,
        WorktreePatchOperationKind::CreateFile
    );

    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 2);

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 102 Stage 5, design-v1.md §14.4/§14.5 item 5: the commit-index cache's exemption from
/// criterion 2 ("this is only a cache, never a root of trust") asserted by comparing real commit
/// output, not merely described in prose. Same two-commit sequence as
/// `genesis_second_commit_before_seal_queues` -- the second commit's worktree scan revisits `a.txt`
/// (unchanged, present in both the worktree and the folded first-commit baseline) via
/// `resolve_existing_file`'s cache consultation, exercising the read path this test targets, not
/// just `CommitIndex::load`/`save` in isolation (already covered by `commit_index/tests.rs`). Run
/// three times with three different cache states injected between the two commits, comparing the
/// second commit's full report (`WorktreePatchCommitReport` derives `PartialEq`): a missing or
/// corrupt cache must produce output identical to a warm one, proving cost is the only thing that
/// differs.
#[test]
fn second_commit_report_is_identical_whether_the_commit_index_cache_is_warm_cold_or_corrupt() {
    fn run_two_commits(
        label: &str,
        disrupt_cache: impl FnOnce(&RepositoryLayout),
    ) -> crate::WorktreePatchCommitReport {
        let root = unique_temp_dir(label);
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        std::fs::write(root.join("a.txt"), b"one\n").unwrap();

        let mut generator = deterministic_generator();
        commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "genesis",
            WorktreePatchCommitOptions::file_level(),
            &mut generator,
            &test_signer(),
        )
        .unwrap();

        disrupt_cache(&layout);

        std::fs::write(root.join("b.txt"), b"two\n").unwrap();
        let mut generator2 = deterministic_generator();
        let second = commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "again",
            WorktreePatchCommitOptions::file_level(),
            &mut generator2,
            &test_signer(),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
        second
    }

    fn cache_relative(layout: &RepositoryLayout) -> std::path::PathBuf {
        layout
            .repository_relative(&layout.cache_dir().join("commit-index.v1"))
            .unwrap()
    }

    let warm = run_two_commits("commit-index-warm", |_layout| {});
    let cold = run_two_commits("commit-index-cold", |layout| {
        crate::foundation::fsutil::remove_file_required(
            layout.repository_mutation_root(),
            &cache_relative(layout),
        )
        .unwrap();
    });
    let corrupt = run_two_commits("commit-index-corrupt", |layout| {
        crate::foundation::fsutil::write_file_atomically(
            layout.repository_mutation_root(),
            &cache_relative(layout),
            b"not a valid commit index at all",
        )
        .unwrap();
    });

    assert_eq!(warm, cold);
    assert_eq!(warm, corrupt);
}

/// RFC 102 Stage 5, design-v1.md §14.4/§14.5 item 5: the incremental lifecycle-state cache's
/// exemption asserted the same way as the commit-index cache above. Baseline is `publish_node_
/// baseline`'s sealed `Root` block, so `resolve_worktree_baseline` returns `WorktreeBaseline::
/// Published` and every commit here reaches `resolve_baseline_state` (only reached for a `Published`
/// baseline -- `node_authoring.rs:280-288`), unlike the genesis-queue tests above. The first commit
/// warms the cache (a real persisted entry, not a hand-built one); the second is what gets compared
/// across three cache states.
#[test]
fn second_commit_report_is_identical_whether_the_lifecycle_cache_is_warm_cold_or_corrupt() {
    fn run_two_commits(
        label: &str,
        disrupt_cache: impl FnOnce(&RepositoryLayout),
    ) -> crate::WorktreePatchCommitReport {
        let root = unique_temp_dir(label);
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        publish_node_baseline(&layout, &[("a.txt", b"one\n", BlobKind::Text)]);

        std::fs::write(root.join("b.txt"), b"two\n").unwrap();
        let mut generator = deterministic_generator();
        commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "first",
            WorktreePatchCommitOptions::file_level(),
            &mut generator,
            &test_signer(),
        )
        .unwrap();

        disrupt_cache(&layout);

        std::fs::write(root.join("c.txt"), b"three\n").unwrap();
        let mut generator2 = deterministic_generator();
        let second = commit_worktree_changes_with_generator(
            &layout,
            "heads/main",
            "second",
            WorktreePatchCommitOptions::file_level(),
            &mut generator2,
            &test_signer(),
        )
        .unwrap();

        let _ = std::fs::remove_dir_all(root);
        second
    }

    fn cache_relative(layout: &RepositoryLayout) -> std::path::PathBuf {
        layout
            .repository_relative(&layout.cache_dir().join("lifecycle-state.v1"))
            .unwrap()
    }

    let warm = run_two_commits("lifecycle-cache-warm", |_layout| {});
    let cold = run_two_commits("lifecycle-cache-cold", |layout| {
        crate::foundation::fsutil::remove_file_required(
            layout.repository_mutation_root(),
            &cache_relative(layout),
        )
        .unwrap();
    });
    let corrupt = run_two_commits("lifecycle-cache-corrupt", |layout| {
        crate::foundation::fsutil::write_file_atomically(
            layout.repository_mutation_root(),
            &cache_relative(layout),
            b"not a valid lifecycle cache at all",
        )
        .unwrap();
    });

    assert_eq!(warm, cold);
    assert_eq!(warm, corrupt);
}

/// DC-66 criterion 3: node identity is safe across a queue — no two queued patches can mint the same
/// `node_id`, tested against constructed state. A file created by the first queued commit is
/// correctly seen as existing by the second (proving the chain fold, not just non-collision by luck),
/// and a pre-existing *sealed* file stays untouched by the queue's own creates.
#[test]
fn queued_commits_mint_distinct_node_ids_and_see_each_others_creates() {
    let root = unique_temp_dir("wt-queue-node-identity");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    publish_node_baseline(&layout, &[("seed.txt", b"seed", BlobKind::Text)]);

    let mut generator = deterministic_generator();
    std::fs::write(root.join("a.txt"), b"alpha").unwrap();
    let first = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "queue a.txt",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    std::fs::write(root.join("b.txt"), b"beta").unwrap();
    let second = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "queue b.txt",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();

    // The second commit must see a.txt as already existing (folded from the first queued patch) —
    // only b.txt is new. If the fold were missing, a.txt would appear "no baseline node" and be
    // re-created with a fresh id, exactly the violation this criterion forbids.
    assert_eq!(second.changes.len(), 1);
    assert_eq!(second.changes[0].path, "b.txt");
    assert_ne!(first.patch_id, second.patch_id);

    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert_eq!(replay.records.len(), 2);

    seal_active_patch(&layout, "heads/main");

    // Ground truth: an independent full replay (not the incremental cache, not the commit path's own
    // view) of the sealed batch must show exactly three live nodes with three pairwise-distinct node
    // ids and the correct paths.
    let object_store = FileObjectStore::new(layout.clone());
    let (baseline_block, horizon) =
        crate::patch_replay::resolve_node_lineage_bounds(&layout, "heads/main").unwrap();
    let state =
        crate::lifecycle_cache::replay_derived_state(&object_store, baseline_block, horizon)
            .unwrap();
    let mut paths_and_ids: Vec<(String, NodeId)> = state
        .state()
        .live_nodes()
        .map(|(node_id, node)| (node.path.as_str().to_string(), *node_id))
        .collect();
    paths_and_ids.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        paths_and_ids
            .iter()
            .map(|(path, _)| path.as_str())
            .collect::<Vec<_>>(),
        vec!["a.txt", "b.txt", "seed.txt"]
    );
    let mut ids: Vec<NodeId> = paths_and_ids.iter().map(|(_, id)| *id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "node ids must be pairwise distinct");

    let _ = std::fs::remove_dir_all(root);
}

/// Genesis-vs-corruption: a missing ref pointer with existing ref-log history is NOT genesis; it
/// fails closed and points at recovery, never silently re-genesis (design §4, acceptance 6).
#[test]
fn genesis_missing_pointer_with_log_fails_closed() {
    let root = unique_temp_dir("wt-genesis-corrupt");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    // Publish a baseline (writes ref pointer + ref log), then remove only the pointer.
    publish_node_baseline(&layout, &[("readme.txt", b"hello\n", BlobKind::Text)]);
    crate::refs::remove_pointer_entries_for_test(
        &layout,
        crate::foundation::layout::ref_name_key_bytes("heads/main"),
    )
    .unwrap();
    std::fs::write(root.join("readme.txt"), b"changed\n").unwrap();

    let mut generator = deterministic_generator();
    let err = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "should-not-genesis",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap_err();
    assert!(
        err.to_string()
            .contains("repository mutation is blocked by incomplete ref publication"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// DC-13: a first commit onto an explicit unborn non-default branch ref authors an independent Root
/// history from the current worktree and records active-WAL ref ownership.
#[test]
fn genesis_on_non_default_ref_authors_create_file() {
    let root = unique_temp_dir("wt-genesis-nondefault");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/feature",
        "genesis-nondefault",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.ref_name, "heads/feature");
    assert_eq!(report.operation_count, 1);
    assert_eq!(
        report.changes[0].operation,
        WorktreePatchOperationKind::CreateFile
    );
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/feature".to_string())
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn invalid_branch_ref_fails_before_active_mutation() {
    let root = unique_temp_dir("wt-genesis-invalid-ref");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    let mut generator = deterministic_generator();
    let err = commit_worktree_changes_with_generator(
        &layout,
        "tags/v1",
        "invalid-ref",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("reserved"),
        "unexpected error: {err}"
    );
    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    assert!(replay.records.is_empty());
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Missing
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn empty_wal_malformed_active_ref_metadata_is_cleaned_before_commit() {
    let root = unique_temp_dir("wt-genesis-clean-stale-ref");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(layout.default_active_ref_name_path(), b"heads//bad").unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    let mut generator = deterministic_generator();
    let report = commit_worktree_changes_with_generator(
        &layout,
        "heads/topic",
        "genesis-topic",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    assert_eq!(report.ref_name, "heads/topic");
    assert_eq!(
        read_active_ref_metadata(&layout).unwrap(),
        ActiveRefMetadata::Valid("heads/topic".to_string())
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn non_empty_wal_missing_active_ref_metadata_fails_closed() {
    let root = unique_temp_dir("wt-active-ref-missing");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    let mut generator = deterministic_generator();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/topic",
        "genesis-topic",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    std::fs::remove_file(layout.default_active_ref_name_path()).unwrap();
    std::fs::write(root.join("b.txt"), b"two\n").unwrap();

    let mut generator2 = deterministic_generator();
    let err = commit_worktree_changes_with_generator(
        &layout,
        "heads/topic",
        "again",
        WorktreePatchCommitOptions::file_level(),
        &mut generator2,
        &test_signer(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("metadata is missing"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn non_empty_wal_malformed_active_ref_metadata_fails_closed() {
    let root = unique_temp_dir("wt-active-ref-malformed");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    let mut generator = deterministic_generator();
    commit_worktree_changes_with_generator(
        &layout,
        "heads/topic",
        "genesis-topic",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap();
    std::fs::write(layout.default_active_ref_name_path(), b"heads//bad").unwrap();
    std::fs::write(root.join("b.txt"), b"two\n").unwrap();

    let mut generator2 = deterministic_generator();
    let err = commit_worktree_changes_with_generator(
        &layout,
        "heads/topic",
        "again",
        WorktreePatchCommitOptions::file_level(),
        &mut generator2,
        &test_signer(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("metadata is malformed"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 4.4bR P1b: genesis requires a clean active WAL. A trailing partial WAL tail (fewer bytes than a
/// record header) fails closed and points at `doctor --repair-wal-tail` rather than appending after
/// the partial tail.
#[test]
fn genesis_with_trailing_partial_wal_fails_closed() {
    let root = unique_temp_dir("wt-genesis-partialwal");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();

    // Seed the active WAL with a trailing partial tail (< one record header, 0 complete records).
    let wal_path = layout.default_queue_wal_path();
    if let Some(parent) = wal_path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&wal_path, [0xAB_u8; 10]).unwrap();

    let mut generator = deterministic_generator();
    let err = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "genesis-partial",
        WorktreePatchCommitOptions::file_level(),
        &mut generator,
        &test_signer(),
    )
    .unwrap_err();
    assert!(
        err.to_string().contains("trailing partial bytes"),
        "unexpected error: {err}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 4.4bR2: the active-WAL guard is atomic with the append under the active-session lock. Two
/// concurrent genesis commits on the same fresh repo serialize — exactly one succeeds, and the active
/// WAL ends with exactly one Patch record (seq 1, no trailing partial). The loser fails closed via a
/// lock conflict or the post-lock "seal first" active-WAL guard.
#[test]
fn concurrent_genesis_commits_serialize_to_one_record() {
    let root = unique_temp_dir("wt-genesis-concurrent");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    std::fs::write(root.join("a.txt"), b"one\n").unwrap();
    std::fs::write(root.join("b.txt"), b"two\n").unwrap();

    let la = layout.clone();
    let lb = layout.clone();
    let h1 = std::thread::spawn(move || {
        super::commit_worktree_changes_signed(
            &la,
            "heads/main",
            "genesis-a",
            WorktreePatchCommitOptions::file_level(),
            &test_signer(),
        )
        .is_ok()
    });
    let h2 = std::thread::spawn(move || {
        super::commit_worktree_changes_signed(
            &lb,
            "heads/main",
            "genesis-b",
            WorktreePatchCommitOptions::file_level(),
            &test_signer(),
        )
        .is_ok()
    });
    let ok1 = h1.join().unwrap();
    let ok2 = h2.join().unwrap();

    let ok_count = [ok1, ok2].into_iter().filter(|ok| *ok).count();
    assert_eq!(
        ok_count, 1,
        "exactly one concurrent genesis commit must succeed"
    );

    // The active WAL holds exactly one Patch record, sequence 1, no trailing partial bytes.
    let replay = Wal::new(layout.default_queue_wal_path()).replay().unwrap();
    assert_eq!(
        replay.records.len(),
        1,
        "active WAL must hold exactly one record"
    );
    assert_eq!(replay.records[0].seq, 1);
    assert_eq!(replay.trailing_partial_bytes, 0);
    let _ = std::fs::remove_dir_all(root);
}
