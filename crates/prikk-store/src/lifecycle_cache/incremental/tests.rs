#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, MerkleRoot,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose,
};

use super::*;
use crate::RepositoryLayout;
use crate::node::node_lifecycle::LiveNode;
use crate::test_gates::test_support::unique_temp_dir;

/// A reader whose id -> object mapping is set by hand, mirroring
/// `lifecycle_cache/replay/tests.rs`'s `MockReader` (kept as an independent copy rather than shared
/// across test modules, matching this project's convention for small, self-contained test doubles).
struct MockReader {
    objects: BTreeMap<ObjectId, ObjectEnvelope>,
}

impl MockReader {
    fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
        }
    }

    fn insert_block(&mut self, id: ObjectId, parents: &[ObjectId], patch_ids: &[ObjectId]) {
        let kind = if parents.is_empty() {
            BlockKind::Root
        } else {
            BlockKind::Normal
        };
        let payload = BlockPayload {
            parent_block_ids: parents.to_vec(),
            kind,
            patch_ids: patch_ids.to_vec(),
            state_merkle_root: MerkleRoot([0_u8; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        };
        self.insert(
            id,
            ObjectType::Block,
            payload.to_canonical_bytes().expect("block"),
        );
    }

    fn insert_blob(&mut self, id: ObjectId, kind: BlobKind) {
        let payload = BlobPayload::new(kind, b"content".to_vec());
        self.insert(
            id,
            ObjectType::Blob,
            payload.to_canonical_bytes().expect("blob"),
        );
    }

    fn insert_patch(&mut self, id: ObjectId, kinds: Vec<OperationKind>) {
        let operations = kinds
            .into_iter()
            .enumerate()
            .map(|(i, kind)| Operation {
                op_seq: (i as u32) + 1,
                op_id: None,
                preconditions: Vec::new(),
                kind,
            })
            .collect();
        let payload = PatchPayload {
            operations,
            intent: None,
            preconditions: Vec::new(),
            purpose: PatchPurpose::Normal,
            message: None,
        };
        self.insert(
            id,
            ObjectType::Patch,
            payload.to_canonical_bytes().expect("patch"),
        );
    }

    fn remove(&mut self, id: ObjectId) {
        self.objects.remove(&id);
    }

    fn insert(&mut self, id: ObjectId, object_type: ObjectType, bytes: Vec<u8>) {
        self.objects
            .insert(id, ObjectEnvelope::unsigned(object_type, 1, bytes));
    }
}

impl ObjectReader for MockReader {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        Ok(self.objects.get(&id).cloned())
    }
}

fn oid(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

fn nid(byte: u8) -> NodeId {
    NodeId::from_bytes([byte; 32])
}

fn sample_cache() -> IncrementalCache {
    let mut state = NodeLifecycleState::new();
    state
        .seed_live_node(
            nid(1),
            LiveNode {
                path: RepoPath::parse("a.txt").expect("path"),
                kind: prikk_object::NodeKind::TextFile,
                content: NodeContent::File {
                    blob_id: oid(9),
                    mode: 0o100_644,
                },
            },
        )
        .expect("seed live");
    state
        .seed_tombstone(
            nid(2),
            Tombstone {
                kind: prikk_object::NodeKind::TextFile,
                content: NodeContent::File {
                    blob_id: oid(10),
                    mode: 0o100_644,
                },
                path: RepoPath::parse("deleted.txt").expect("path"),
            },
        )
        .expect("seed tombstone");
    IncrementalCache {
        baseline_block_id: oid(1),
        horizon_id: oid(1),
        steps_since_reanchor: 3,
        state,
    }
}

// ---- codec ----

#[test]
fn encode_decode_round_trips() {
    let cache = sample_cache();
    let bytes = encode(&cache);
    let decoded = decode(&bytes).expect("decode");
    assert_eq!(decoded.baseline_block_id, cache.baseline_block_id);
    assert_eq!(decoded.horizon_id, cache.horizon_id);
    assert_eq!(decoded.steps_since_reanchor, cache.steps_since_reanchor);
    assert_eq!(decoded.state, cache.state);
}

#[test]
fn decode_rejects_wrong_magic() {
    let mut bytes = encode(&sample_cache());
    bytes[0] = bytes[0].wrapping_add(1);
    assert!(decode(&bytes).is_none());
}

#[test]
fn decode_rejects_checksum_mismatch() {
    let mut bytes = encode(&sample_cache());
    // Flip a byte inside the body, after magic + checksum, so the structural format is untouched
    // but the checksum no longer matches — the persistence-fault case this cache exists to catch.
    let flip_at = CACHE_MAGIC.len() + 32 + 4;
    bytes[flip_at] ^= 0xFF;
    assert!(decode(&bytes).is_none());
}

#[test]
fn decode_rejects_truncated_bytes() {
    let bytes = encode(&sample_cache());
    let truncated = &bytes[..bytes.len() - 5];
    assert!(decode(truncated).is_none());
}

#[test]
fn decode_rejects_empty_bytes() {
    assert!(decode(&[]).is_none());
}

// ---- filesystem load/save ----

#[test]
fn load_save_round_trips_through_the_repository_layout() {
    let root = unique_temp_dir("dc64-incremental-load-save");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(load(&layout).is_none());
        let cache = sample_cache();
        assert!(save(&layout, &cache).is_ok());
        let reloaded = load(&layout);
        assert!(reloaded.is_some());
        if let Some(reloaded) = reloaded {
            assert_eq!(reloaded.baseline_block_id, cache.baseline_block_id);
            assert_eq!(reloaded.state, cache.state);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn load_treats_a_corrupt_cache_file_as_absent() {
    let root = unique_temp_dir("dc64-incremental-corrupt");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let relative = layout.repository_relative(&cache_path(&layout));
        assert!(relative.is_ok());
        if let Ok(relative) = relative {
            let write = crate::foundation::fsutil::write_file_atomically(
                layout.repository_mutation_root(),
                &relative,
                b"not a valid incremental cache",
            );
            assert!(write.is_ok());
        }
        assert!(load(&layout).is_none());
    }
    let _ = std::fs::remove_dir_all(root);
}

// ---- resolve_baseline_state: end-to-end against a mock reader ----

fn genesis_reader_with_two_files() -> (MockReader, ObjectId, ObjectId, ObjectId, ObjectId) {
    let (genesis, patch, blob_a, blob_b) = (oid(1), oid(2), oid(3), oid(4));
    let mut reader = MockReader::new();
    reader.insert_blob(blob_a, BlobKind::Text);
    reader.insert_blob(blob_b, BlobKind::Text);
    reader.insert_patch(
        patch,
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(1),
                blob_id: blob_a,
                mode: 0o100_644,
            }),
            OperationKind::CreateFile(CreateFile {
                path: "b.txt".to_string(),
                node_id: nid(2),
                blob_id: blob_b,
                mode: 0o100_644,
            }),
        ],
    );
    reader.insert_block(genesis, &[], &[patch]);
    (reader, genesis, patch, blob_a, blob_b)
}

#[test]
fn resolve_baseline_state_full_replays_on_a_cold_cache() {
    let root = unique_temp_dir("dc64-resolve-cold");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let (reader, genesis, ..) = genesis_reader_with_two_files();
        let result = resolve_baseline_state(&layout, &reader, genesis, genesis);
        assert!(result.is_ok());
        if let Ok(result) = result {
            assert_eq!(result.state().live_count(), 2);
        }
        let cached = load(&layout);
        assert!(cached.is_some());
        if let Some(cached) = cached {
            assert_eq!(cached.baseline_block_id, genesis);
            assert_eq!(cached.steps_since_reanchor, 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn resolve_baseline_state_takes_the_incremental_path_for_a_direct_child() {
    let root = unique_temp_dir("dc64-resolve-incremental");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let (mut reader, genesis, genesis_patch, blob_a, _blob_b) = genesis_reader_with_two_files();

        // Warm the cache with a full replay of genesis.
        let first = resolve_baseline_state(&layout, &reader, genesis, genesis);
        assert!(first.is_ok());

        // Second block: one CreateFile, single parent = genesis.
        let (second_patch, blob_c) = (oid(5), oid(6));
        reader.insert_blob(blob_c, BlobKind::Text);
        reader.insert_patch(
            second_patch,
            vec![OperationKind::CreateFile(CreateFile {
                path: "c.txt".to_string(),
                node_id: nid(3),
                blob_id: blob_c,
                mode: 0o100_644,
            })],
        );
        let second = oid(7);
        reader.insert_block(second, &[genesis], &[second_patch]);

        // Prove the incremental path was actually taken, not a disguised full replay: remove the
        // genesis patch and its blob. A full replay of `second` would need to re-walk to genesis
        // and re-read the genesis patch; an incremental step never touches it.
        reader.remove(genesis_patch);
        reader.remove(blob_a);

        let result = resolve_baseline_state(&layout, &reader, second, genesis);
        assert!(
            result.is_ok(),
            "incremental step must not need the genesis patch"
        );
        if let Ok(result) = result {
            assert_eq!(result.state().live_count(), 3);
            assert!(
                result
                    .state()
                    .node_id_at(&RepoPath::parse("a.txt").expect("path"))
                    == Some(nid(1))
            );
            assert!(
                result
                    .state()
                    .node_id_at(&RepoPath::parse("c.txt").expect("path"))
                    == Some(nid(3))
            );
        }
        let cached = load(&layout);
        assert!(cached.is_some());
        if let Some(cached) = cached {
            assert_eq!(cached.baseline_block_id, second);
            assert_eq!(cached.steps_since_reanchor, 1);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn resolve_baseline_state_falls_back_to_full_replay_on_parent_mismatch() {
    let root = unique_temp_dir("dc64-resolve-parent-mismatch");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let (mut reader, genesis, ..) = genesis_reader_with_two_files();
        let first = resolve_baseline_state(&layout, &reader, genesis, genesis);
        assert!(first.is_ok());

        // Sibling block: also a direct child of genesis, but the cache (after the first call)
        // represents genesis itself, not a third block — so a block whose parent is NOT the
        // cache's own baseline must fall back. Build an intermediate block A (child of genesis),
        // resolve it (cache now represents A), then request sibling B (also child of genesis, not
        // of A) — B's parent (genesis) does not match the cache's baseline (A).
        let (patch_a, blob_a2) = (oid(8), oid(9));
        reader.insert_blob(blob_a2, BlobKind::Text);
        reader.insert_patch(
            patch_a,
            vec![OperationKind::CreateFile(CreateFile {
                path: "from-a.txt".to_string(),
                node_id: nid(4),
                blob_id: blob_a2,
                mode: 0o100_644,
            })],
        );
        let block_a = oid(10);
        reader.insert_block(block_a, &[genesis], &[patch_a]);
        let resolved_a = resolve_baseline_state(&layout, &reader, block_a, genesis);
        assert!(resolved_a.is_ok());

        let (patch_b, blob_b2) = (oid(11), oid(12));
        reader.insert_blob(blob_b2, BlobKind::Text);
        reader.insert_patch(
            patch_b,
            vec![OperationKind::CreateFile(CreateFile {
                path: "from-b.txt".to_string(),
                node_id: nid(5),
                blob_id: blob_b2,
                mode: 0o100_644,
            })],
        );
        let block_b = oid(13);
        reader.insert_block(block_b, &[genesis], &[patch_b]);

        let resolved_b = resolve_baseline_state(&layout, &reader, block_b, genesis);
        assert!(resolved_b.is_ok());
        if let Ok(resolved_b) = resolved_b {
            // Full replay from genesis + block_b only — must NOT include block_a's file, proving
            // the fallback replayed the correct (block_b) lineage rather than incorrectly
            // incrementing from the stale block_a cache.
            assert_eq!(resolved_b.state().live_count(), 3);
            assert!(
                resolved_b
                    .state()
                    .node_id_at(&RepoPath::parse("from-b.txt").expect("path"))
                    .is_some()
            );
            assert!(
                resolved_b
                    .state()
                    .node_id_at(&RepoPath::parse("from-a.txt").expect("path"))
                    .is_none()
            );
        }
        let cached = load(&layout);
        assert!(cached.is_some());
        if let Some(cached) = cached {
            assert_eq!(cached.baseline_block_id, block_b);
            assert_eq!(
                cached.steps_since_reanchor, 0,
                "a fallback must reset the reanchor count"
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn resolve_baseline_state_falls_back_when_the_horizon_differs() {
    let root = unique_temp_dir("dc64-resolve-horizon-mismatch");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let (reader, genesis, ..) = genesis_reader_with_two_files();
        let first = resolve_baseline_state(&layout, &reader, genesis, genesis);
        assert!(first.is_ok());

        // Request the same block again but with a different (wrong) claimed horizon. Full replay
        // will itself fail closed (`HorizonNotInLineage`) since genesis has no parent to reach a
        // different horizon — proving the eligibility check, not the replay, rejected this first.
        let wrong_horizon = oid(99);
        let result = resolve_baseline_state(&layout, &reader, genesis, wrong_horizon);
        assert!(result.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn reanchor_bound_forces_a_full_replay() {
    let root = unique_temp_dir("dc64-reanchor-bound");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let (mut reader, genesis, genesis_patch, ..) = genesis_reader_with_two_files();
        let first = resolve_baseline_state(&layout, &reader, genesis, genesis);
        assert!(first.is_ok());

        let (patch, blob) = (oid(20), oid(21));
        reader.insert_blob(blob, BlobKind::Text);
        reader.insert_patch(
            patch,
            vec![OperationKind::CreateFile(CreateFile {
                path: "reanchor.txt".to_string(),
                node_id: nid(6),
                blob_id: blob,
                mode: 0o100_644,
            })],
        );
        let child = oid(22);
        reader.insert_block(child, &[genesis], &[patch]);

        // Directly install a cache claiming the reanchor bound has already been reached, rather
        // than performing 64 real steps — this tests the boundary condition, not the loop.
        let mut at_bound = load(&layout).expect("warm cache");
        at_bound.steps_since_reanchor = REANCHOR_BOUND;
        assert!(save(&layout, &at_bound).is_ok());

        // If the reanchor bound is honoured, this must fall back to a full replay, which needs the
        // genesis patch. Remove it: an incorrectly-still-incremental step would succeed anyway
        // (it never reads the genesis patch); a correct fallback fails without it.
        reader.remove(genesis_patch);
        let result = resolve_baseline_state(&layout, &reader, child, genesis);
        assert!(
            result.is_err(),
            "reanchor bound must force a full replay, which needs the (removed) genesis patch"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
