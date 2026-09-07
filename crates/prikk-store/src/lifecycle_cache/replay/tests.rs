#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ChangePerm, CreateFile,
    CreateSymlink, DeleteNode, DeleteNodePreimage, EditText, MerkleRoot, NodeId, NodeKind,
    ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload, RenamePath,
    ReplaceBinary, text_span_hash,
};

use super::{LifecycleReplayError, replay_lineage, walk_lineage};
use crate::node::node_lifecycle::NodeContent;
use crate::object_store::ObjectReader;
use crate::path::RepoPath;
use crate::text_span;

/// A reader whose id → object mapping is set by hand. Unlike a content-addressed store this can
/// host forged topologies (cycles, merges, wrong horizons) needed to prove the walk fails closed,
/// and lets a patch/blob be referenced by a chosen id.
struct MockReader {
    objects: BTreeMap<ObjectId, ObjectEnvelope>,
    reads: std::cell::RefCell<BTreeMap<ObjectId, usize>>,
}

impl MockReader {
    fn new() -> Self {
        Self {
            objects: BTreeMap::new(),
            reads: std::cell::RefCell::new(BTreeMap::new()),
        }
    }

    fn read_count(&self, id: ObjectId) -> usize {
        self.reads.borrow().get(&id).copied().unwrap_or(0)
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

    /// DC-75: a `Merge` block, distinct from `insert_block`'s always-`Root`/`Normal` inference.
    fn insert_merge_block(
        &mut self,
        id: ObjectId,
        parents: &[ObjectId; 2],
        mainline: ObjectId,
        baseline: ObjectId,
        patch_ids: &[ObjectId],
    ) {
        let payload = BlockPayload {
            parent_block_ids: parents.to_vec(),
            kind: BlockKind::Merge,
            patch_ids: patch_ids.to_vec(),
            state_merkle_root: MerkleRoot([0_u8; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: Some(mainline),
            merge_baseline_block_id: Some(baseline),
        };
        self.insert(
            id,
            ObjectType::Block,
            payload.to_canonical_bytes().expect("merge block"),
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

    fn insert_text_blob(&mut self, id: ObjectId, content: &[u8]) {
        let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
        self.insert(
            id,
            ObjectType::Blob,
            payload.to_canonical_bytes().expect("text blob"),
        );
    }

    fn insert_patch(&mut self, id: ObjectId, kinds: Vec<OperationKind>) {
        self.insert_patch_with_schema(id, kinds, 1);
    }

    /// RFC 134 §8: a `kinds` list carrying a v2-identified `EditText` (`left_anchor_len`/
    /// `right_anchor_len` both `Some`) must be inserted at `PATCH_TEXT_SPAN_V2_SCHEMA` or above, or
    /// decode refuses tags 10/11 as unauthorized at schema 1.
    fn insert_patch_with_schema(
        &mut self,
        id: ObjectId,
        kinds: Vec<OperationKind>,
        schema_version: u32,
    ) {
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
            purpose: prikk_object::PatchPurpose::Normal,
            message: None,
        };
        self.insert_with_schema(
            id,
            ObjectType::Patch,
            schema_version,
            payload.to_canonical_bytes().expect("patch"),
        );
    }

    fn insert_raw(&mut self, id: ObjectId, object_type: ObjectType, bytes: Vec<u8>) {
        self.insert(id, object_type, bytes);
    }

    fn insert(&mut self, id: ObjectId, object_type: ObjectType, bytes: Vec<u8>) {
        self.insert_with_schema(id, object_type, 1, bytes);
    }

    fn insert_with_schema(
        &mut self,
        id: ObjectId,
        object_type: ObjectType,
        schema_version: u32,
        bytes: Vec<u8>,
    ) {
        self.objects.insert(
            id,
            ObjectEnvelope::unsigned(object_type, schema_version, bytes),
        );
    }
}

impl ObjectReader for MockReader {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        *self.reads.borrow_mut().entry(id).or_insert(0) += 1;
        Ok(self.objects.get(&id).cloned())
    }
}

fn oid(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

fn nid(byte: u8) -> NodeId {
    NodeId::from_bytes([byte; 32])
}

/// Single-block genesis lineage carrying one patch; replay over it.
fn replay_single_patch(
    kinds: Vec<OperationKind>,
    blob: Option<(ObjectId, BlobKind)>,
) -> std::result::Result<crate::node::node_lifecycle::NodeLifecycleState, LifecycleReplayError> {
    let (genesis, patch) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    if let Some((blob_id, kind)) = blob {
        reader.insert_blob(blob_id, kind);
    }
    reader.insert_patch(patch, kinds);
    reader.insert_block(genesis, &[], &[patch]);
    replay_lineage(&reader, genesis, genesis)
}

// ---- lineage walk (2c-2a, unchanged behaviour) ----

#[test]
fn walks_single_parent_chain_in_apply_order() {
    let (genesis, mid, baseline) = (oid(1), oid(2), oid(3));
    let mut reader = MockReader::new();
    reader.insert_block(genesis, &[], &[]);
    reader.insert_block(mid, &[genesis], &[]);
    reader.insert_block(baseline, &[mid], &[]);

    let chain = walk_lineage(&reader, baseline, genesis).expect("walk");
    let ids: Vec<ObjectId> = chain.into_iter().map(|(id, _)| id).collect();
    assert_eq!(ids, vec![genesis, mid, baseline]);
}

#[test]
fn missing_block_is_an_error_not_genesis() {
    let (baseline, absent) = (oid(3), oid(9));
    let mut reader = MockReader::new();
    reader.insert_block(baseline, &[absent], &[]);

    let err = walk_lineage(&reader, baseline, absent).expect_err("missing parent");
    assert_eq!(
        err,
        LifecycleReplayError::MissingBlockInLineage { block_id: absent }
    );
}

#[test]
fn wrong_type_block_is_unreadable() {
    let blob_id = oid(5);
    let mut reader = MockReader::new();
    reader.insert_blob(blob_id, BlobKind::Binary);

    let err = walk_lineage(&reader, blob_id, blob_id).expect_err("blob as block");
    assert!(
        matches!(err, LifecycleReplayError::UnreadableBlockInLineage { .. }),
        "got {err:?}"
    );
}

#[test]
fn merge_lineage_fails_closed() {
    // DC-75 narrowed, not removed, this guard: `insert_block` always produces `BlockKind::Normal`
    // for a non-empty parent list, so this proves a *malformed* multi-parent Normal block still
    // fails closed exactly as before. See `merge_lineage_follows_mainline_only` below for the now-
    // open case: a properly `BlockKind::Merge`-shaped two-parent block.
    let (baseline, p1, p2) = (oid(3), oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_block(p1, &[], &[]);
    reader.insert_block(p2, &[], &[]);
    reader.insert_block(baseline, &[p1, p2], &[]);

    let err = walk_lineage(&reader, baseline, p1).expect_err("merge");
    assert_eq!(
        err,
        LifecycleReplayError::MergeLineageUnsupported {
            block_id: baseline,
            parent_count: 2,
        }
    );
}

/// DC-75: a properly-shaped `Merge` block's lineage walk follows its mainline parent only, exactly
/// the same shape as a `Normal` block's single-parent walk — proven by walking straight past the
/// secondary parent's own history (`off_mainline`, deliberately given no parent block at all here,
/// so the walk would error immediately if it ever touched that side).
#[test]
fn merge_lineage_follows_mainline_only() -> Result<()> {
    let (genesis, mainline_tip, off_mainline, merge) = (oid(1), oid(2), oid(3), oid(4));
    let mut reader = MockReader::new();
    reader.insert_block(genesis, &[], &[]);
    reader.insert_block(mainline_tip, &[genesis], &[]);
    let mut parents = [mainline_tip, off_mainline];
    parents.sort();
    reader.insert_merge_block(merge, &parents, mainline_tip, mainline_tip, &[]);

    let chain = walk_lineage(&reader, merge, genesis)?;
    let ids: Vec<ObjectId> = chain.iter().map(|(id, _)| *id).collect();
    assert_eq!(ids, vec![genesis, mainline_tip, merge]);
    assert_eq!(
        reader.read_count(off_mainline),
        0,
        "secondary parent never read"
    );
    Ok(())
}

#[test]
fn cycle_is_detected() {
    let (a, b) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_block(a, &[b], &[]);
    reader.insert_block(b, &[a], &[]);

    let err = walk_lineage(&reader, a, a).expect_err("cycle");
    assert!(
        matches!(err, LifecycleReplayError::LineageCycle { .. }),
        "got {err:?}"
    );
}

#[test]
fn genesis_that_is_not_the_horizon_fails_closed() {
    let (genesis, baseline, wrong_horizon) = (oid(1), oid(2), oid(8));
    let mut reader = MockReader::new();
    reader.insert_block(genesis, &[], &[]);
    reader.insert_block(baseline, &[genesis], &[]);

    let err = walk_lineage(&reader, baseline, wrong_horizon).expect_err("wrong horizon");
    assert_eq!(
        err,
        LifecycleReplayError::HorizonNotInLineage {
            horizon_id: wrong_horizon,
        }
    );
}

#[test]
fn patchless_lineage_replays_to_empty_state() {
    let (genesis, baseline) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_block(genesis, &[], &[]);
    reader.insert_block(baseline, &[genesis], &[]);

    let state = replay_lineage(&reader, baseline, genesis).expect("clean replay");
    assert_eq!(state.live_count(), 0);
}

#[test]
fn missing_patch_fails_closed() {
    let (genesis, absent_patch) = (oid(1), oid(7));
    let mut reader = MockReader::new();
    reader.insert_block(genesis, &[], &[absent_patch]);

    let err = replay_lineage(&reader, genesis, genesis).expect_err("missing patch");
    match err {
        LifecycleReplayError::MalformedPatchInLineage { patch_id, .. } => {
            assert_eq!(patch_id, absent_patch);
        }
        other => panic!("expected MalformedPatchInLineage, got {other:?}"),
    }
}

// ---- E2: malformed / wrong-type patch ----

#[test]
fn malformed_patch_payload_is_malformed() {
    let (genesis, patch) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_raw(patch, ObjectType::Patch, vec![0xff, 0xff, 0xff]);
    reader.insert_block(genesis, &[], &[patch]);

    let err = replay_lineage(&reader, genesis, genesis).expect_err("malformed");
    assert!(
        matches!(err, LifecycleReplayError::MalformedPatchInLineage { .. }),
        "got {err:?}"
    );
}

#[test]
fn wrong_type_patch_is_malformed() {
    let (genesis, not_a_patch) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_blob(not_a_patch, BlobKind::Binary);
    reader.insert_block(genesis, &[], &[not_a_patch]);

    let err = replay_lineage(&reader, genesis, genesis).expect_err("wrong type");
    assert!(
        matches!(err, LifecycleReplayError::MalformedPatchInLineage { .. }),
        "got {err:?}"
    );
}

// ---- 2c-2b state effects ----

#[test]
fn create_file_makes_node_live_with_exact_content() {
    let blob = oid(20);
    let state = replay_single_patch(
        vec![OperationKind::CreateFile(CreateFile {
            path: "a.txt".to_string(),
            node_id: nid(0x11),
            blob_id: blob,
            mode: 0o100_644,
        })],
        Some((blob, BlobKind::Text)),
    )
    .expect("create file");

    let node = state.live_node(&nid(0x11)).expect("live");
    assert_eq!(node.kind, NodeKind::TextFile);
    assert_eq!(node.path.as_str(), "a.txt");
    assert_eq!(
        node.content,
        NodeContent::File {
            blob_id: blob,
            mode: 0o100_644,
        }
    );
}

#[test]
fn create_file_missing_blob_fails_closed() {
    let blob = oid(20);
    // Blob not inserted: resolver yields the fail-closed None sentinel.
    let err = replay_single_patch(
        vec![OperationKind::CreateFile(CreateFile {
            path: "a.txt".to_string(),
            node_id: nid(0x11),
            blob_id: blob,
            mode: 0o100_644,
        })],
        None,
    )
    .expect_err("missing blob");
    assert_eq!(
        err,
        LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id: blob }
    );
}

#[test]
fn create_symlink_makes_symlink_live() {
    let state = replay_single_patch(
        vec![OperationKind::CreateSymlink(CreateSymlink {
            path: "link".to_string(),
            node_id: nid(0x22),
            target: "dest".to_string(),
        })],
        None,
    )
    .expect("create symlink");

    let node = state.live_node(&nid(0x22)).expect("live");
    assert_eq!(node.kind, NodeKind::Symlink);
    assert_eq!(
        node.content,
        NodeContent::Symlink {
            target: "dest".to_string(),
        }
    );
}

#[test]
fn create_chmod_delete_tombstone_carries_post_mutation_mode() {
    // O1: a deletion preimage must reflect the post-mutation mode, not the create-time mode.
    let blob = oid(20);
    let state = replay_single_patch(
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x11),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::ChangePerm(ChangePerm {
                node_id: nid(0x11),
                old_mode: 0o100_644,
                new_mode: 0o100_755,
            }),
            OperationKind::DeleteNode(DeleteNode {
                path: "a.txt".to_string(),
                node_id: nid(0x11),
                old_node_kind: NodeKind::TextFile,
                preimage: DeleteNodePreimage::File {
                    old_blob_id: blob,
                    old_mode: 0o100_755,
                },
            }),
        ],
        Some((blob, BlobKind::Text)),
    )
    .expect("create+chmod+delete");

    assert!(state.live_node(&nid(0x11)).is_none(), "node deleted");
    let tombstone = state.latest_tombstone(&nid(0x11)).expect("tombstone");
    assert_eq!(
        tombstone.content,
        NodeContent::File {
            blob_id: blob,
            mode: 0o100_755,
        },
        "tombstone must carry the post-chmod mode"
    );
}

#[test]
fn create_then_rename_preserves_node_id() {
    let blob = oid(20);
    let state = replay_single_patch(
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x11),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::RenamePath(RenamePath {
                node_id: nid(0x11),
                old_path: "a.txt".to_string(),
                new_path: "b.txt".to_string(),
            }),
        ],
        Some((blob, BlobKind::Text)),
    )
    .expect("create+rename");

    assert_eq!(
        state.live_node(&nid(0x11)).expect("live").path.as_str(),
        "b.txt"
    );
    assert_eq!(state.node_id_at(&RepoPath::parse("a.txt").unwrap()), None);
    assert_eq!(
        state.node_id_at(&RepoPath::parse("b.txt").unwrap()),
        Some(nid(0x11))
    );
}

#[test]
fn change_perm_old_mode_mismatch_is_inconsistent() {
    let blob = oid(20);
    let err = replay_single_patch(
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x11),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::ChangePerm(ChangePerm {
                node_id: nid(0x11),
                old_mode: 0o100_755, // wrong -- the live node's mode is 0o100_644
                new_mode: 0o100_644,
            }),
        ],
        Some((blob, BlobKind::Text)),
    )
    .expect_err("old_mode mismatch");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

// ---- 2c-2d: EditText ----

/// Author a valid `EditText` record for the occurrence at byte `start` in `text`, using the same
/// anchor/span primitives replay uses (so the round-trip is exact).
fn author_edit_text(
    text: &[u8],
    start: usize,
    old_span: &[u8],
    replacement: &[u8],
    node_id: NodeId,
) -> EditText {
    let end = start + old_span.len();
    let left = text_span::left_anchor(text, start);
    let right = text_span::right_anchor(text, end);
    // dup_index = index of this occurrence within the anchor-matching occurrences.
    let mut dup_index = 0_u32;
    let mut idx = 0_u32;
    for s in text_span::occurrences(text, old_span) {
        let e = s + old_span.len();
        if text_span::left_anchor(text, s) == left && text_span::right_anchor(text, e) == right {
            if s == start {
                dup_index = idx;
            }
            idx += 1;
        }
    }
    let old_span_hash = text_span_hash(old_span);
    let span_id = text_span::compute_span_id(node_id, &old_span_hash, &left, &right, dup_index);
    EditText {
        node_id,
        span_id,
        old_span_hash,
        left_anchor_hash: left,
        right_anchor_hash: right,
        replacement_text: replacement.to_vec(),
        presentation_hint_line: None,
        presentation_hint_column: None,
        old_span_text: old_span.to_vec(),
        left_anchor_len: None,
        right_anchor_len: None,
    }
}

/// Author a valid v2 (RFC 134 §8, content-unique) `EditText` record for the occurrence at byte
/// `start` in `text`, using the shared v2 authoring primitives (so the round-trip is exact).
fn author_edit_text_v2(
    text: &[u8],
    start: usize,
    old_span: &[u8],
    replacement: &[u8],
    node_id: NodeId,
) -> EditText {
    let end = start + old_span.len();
    let old_span_hash = text_span_hash(old_span);
    let (left_len, right_len) =
        text_span::choose_anchor_lengths_v2(text, old_span, start, end).expect("anchor lengths");
    let left = text_span::left_anchor_v2(text, start, left_len);
    let right = text_span::right_anchor_v2(text, end, right_len);
    let span_id =
        text_span::compute_span_id_v2(node_id, &old_span_hash, &left, &right, left_len, right_len);
    EditText {
        node_id,
        span_id,
        old_span_hash,
        left_anchor_hash: left,
        right_anchor_hash: right,
        replacement_text: replacement.to_vec(),
        presentation_hint_line: None,
        presentation_hint_column: None,
        old_span_text: old_span.to_vec(),
        left_anchor_len: Some(left_len),
        right_anchor_len: Some(right_len),
    }
}

/// Replay a CreateFile(text blob at `blob`, content `text`) followed by `ops`.
fn replay_text_node(
    blob: ObjectId,
    text: &[u8],
    node_id: NodeId,
    path: &str,
    mode: u32,
    ops: Vec<OperationKind>,
) -> (
    MockReader,
    std::result::Result<crate::node::node_lifecycle::NodeLifecycleState, LifecycleReplayError>,
) {
    let (genesis, patch) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_text_blob(blob, text);
    let mut kinds = vec![OperationKind::CreateFile(CreateFile {
        path: path.to_string(),
        node_id,
        blob_id: blob,
        mode,
    })];
    kinds.extend(ops);
    reader.insert_patch(patch, kinds);
    reader.insert_block(genesis, &[], &[patch]);
    let result = replay_lineage(&reader, genesis, genesis);
    (reader, result)
}

fn text_blob_id(content: &[u8]) -> ObjectId {
    text_span::text_blob_id(content).expect("text blob id")
}

#[test]
fn edit_text_updates_content_id_exactly() {
    let text = b"hello world";
    let edit = author_edit_text(text, 6, b"world", b"there", nid(0x40));
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x40),
        "a.txt",
        0o100_644,
        vec![OperationKind::EditText(edit)],
    );
    let state = state.expect("edit text");

    let node = state.live_node(&nid(0x40)).expect("live");
    assert_eq!(node.kind, NodeKind::TextFile);
    let expected_id = text_blob_id(b"hello there");
    assert_eq!(
        node.content,
        NodeContent::File {
            blob_id: expected_id,
            mode: 0o100_644,
        },
        "content id == BlobPayload(Text, new_text), mode preserved"
    );
}

#[test]
fn edit_text_preserves_mode_node_id_and_path() {
    let text = b"alpha beta";
    let edit = author_edit_text(text, 0, b"alpha", b"ALPHA", nid(0x41));
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x41),
        "f.txt",
        0o100_755,
        vec![OperationKind::EditText(edit)],
    );
    let state = state.expect("edit");
    let node = state.live_node(&nid(0x41)).expect("live");
    assert_eq!(node.path.as_str(), "f.txt");
    assert_eq!(
        state.node_id_at(&RepoPath::parse("f.txt").unwrap()),
        Some(nid(0x41))
    );
    match &node.content {
        NodeContent::File { mode, .. } => assert_eq!(*mode, 0o100_755),
        other => panic!("expected file, got {other:?}"),
    }
}

/// RFC 134 §8, §3.4 (mixed-history replay -- "the one most likely to be missed"): one node with a
/// v1 `EditText` (schema 1, positional `dup_index` identity) in the genesis block, then a later v2
/// `EditText` (`PATCH_TEXT_SPAN_V2_SCHEMA`, content-unique identity) in a child block, replayed
/// together through the same lineage walk. Each patch resolves through its own scheme
/// (`resolve_text_span`'s dispatch) without the other patch's schema affecting it.
#[test]
fn mixed_v1_then_v2_edit_text_history_replays_correctly() {
    let (genesis, child, patch_g, patch_c, blob) = (oid(1), oid(2), oid(10), oid(11), oid(20));
    let mut reader = MockReader::new();
    reader.insert_text_blob(blob, b"hello world");
    reader.insert_patch(
        patch_g,
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x60),
                blob_id: blob,
                mode: 0o100_644,
            }),
            // v1: "hello world" -> "hello there".
            OperationKind::EditText(author_edit_text(
                b"hello world",
                6,
                b"world",
                b"there",
                nid(0x60),
            )),
        ],
    );
    reader.insert_block(genesis, &[], &[patch_g]);

    // v2: "hello there" -> "howdy there".
    let v2_edit = author_edit_text_v2(b"hello there", 0, b"hello", b"howdy", nid(0x60));
    reader.insert_patch_with_schema(
        patch_c,
        vec![OperationKind::EditText(v2_edit)],
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
    );
    reader.insert_block(child, &[genesis], &[patch_c]);

    let state = replay_lineage(&reader, child, genesis).expect("mixed v1-then-v2 replay");
    let node = state.live_node(&nid(0x60)).expect("live");
    assert_eq!(
        node.content,
        NodeContent::File {
            blob_id: text_blob_id(b"howdy there"),
            mode: 0o100_644,
        },
        "v1 edit then v2 edit must compose to the doubly-edited text"
    );
}

#[test]
fn create_edit_delete_tombstone_carries_post_edit_blob_id() {
    // O1 witness for text: deletion tombstones the post-edit content id.
    let text = b"hello world";
    let edit = author_edit_text(text, 6, b"world", b"there", nid(0x42));
    let post_edit_id = text_blob_id(b"hello there");
    let delete = DeleteNode {
        path: "a.txt".to_string(),
        node_id: nid(0x42),
        old_node_kind: NodeKind::TextFile,
        preimage: DeleteNodePreimage::File {
            old_blob_id: post_edit_id,
            old_mode: 0o100_644,
        },
    };
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x42),
        "a.txt",
        0o100_644,
        vec![
            OperationKind::EditText(edit),
            OperationKind::DeleteNode(delete),
        ],
    );
    let state = state.expect("create+edit+delete");
    assert!(state.live_node(&nid(0x42)).is_none());
    let tombstone = state.latest_tombstone(&nid(0x42)).expect("tombstone");
    assert_eq!(
        tombstone.content,
        NodeContent::File {
            blob_id: post_edit_id,
            mode: 0o100_644,
        }
    );
}

#[test]
fn multiple_edits_use_materialized_cache_and_read_blob_once() {
    let text = b"one two three";
    let blob = oid(20);
    let edit1 = author_edit_text(text, 0, b"one", b"1", nid(0x43));
    // After edit1 the in-memory text is "1 two three"; author the second edit against THAT.
    let after1 = b"1 two three";
    let edit2 = author_edit_text(after1, 2, b"two", b"2", nid(0x43));
    let (reader, state) = replay_text_node(
        blob,
        text,
        nid(0x43),
        "a.txt",
        0o100_644,
        vec![
            OperationKind::EditText(edit1),
            OperationKind::EditText(edit2),
        ],
    );
    let state = state.expect("two edits");
    let expected = text_blob_id(b"1 2 three");
    match &state.live_node(&nid(0x43)).expect("live").content {
        NodeContent::File { blob_id, .. } => assert_eq!(*blob_id, expected),
        other => panic!("expected file, got {other:?}"),
    }
    // Blob reads: CreateFile kind-resolution (1) + first EditText content-read (1). The second
    // edit uses the materialized cache and adds no read (it would be 3 without the cache).
    assert_eq!(reader.read_count(blob), 2, "second edit uses the cache");
}

#[test]
fn edit_text_duplicate_occurrence_selects_anchor_filtered_span() {
    // "ab" occurs twice with distinct surrounding context, so anchors distinguish them.
    let text = b"X ab Y ab Z";
    // Target the SECOND "ab" (at byte 7).
    let edit = author_edit_text(text, 7, b"ab", b"QQ", nid(0x44));
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x44),
        "a.txt",
        0o100_644,
        vec![OperationKind::EditText(edit)],
    );
    let state = state.expect("edit second occurrence");
    let expected = text_blob_id(b"X ab Y QQ Z");
    match &state.live_node(&nid(0x44)).expect("live").content {
        NodeContent::File { blob_id, .. } => assert_eq!(*blob_id, expected),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn edit_text_empty_old_span_insertion_is_deterministic() {
    let text = b"abc";
    // Insert "XY" at position 1.
    let edit = author_edit_text(text, 1, b"", b"XY", nid(0x45));
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x45),
        "a.txt",
        0o100_644,
        vec![OperationKind::EditText(edit)],
    );
    let state = state.expect("insertion");
    let expected = text_blob_id(b"aXYbc");
    match &state.live_node(&nid(0x45)).expect("live").content {
        NodeContent::File { blob_id, .. } => assert_eq!(*blob_id, expected),
        other => panic!("got {other:?}"),
    }
}

#[test]
fn edit_text_anchor_mismatch_fails_closed() {
    let text = b"hello world";
    let mut edit = author_edit_text(text, 6, b"world", b"there", nid(0x46));
    edit.left_anchor_hash = [0xab_u8; 32]; // corrupt anchor
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x46),
        "a.txt",
        0o100_644,
        vec![OperationKind::EditText(edit)],
    );
    let err = state.expect_err("anchor mismatch");
    assert!(
        matches!(
            err,
            LifecycleReplayError::TextSpanResolutionFailed {
                reason: text_span::TextSpanResolutionFailure::AnchorMismatch,
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn edit_text_wrong_span_id_fails_closed() {
    let text = b"hello world";
    let mut edit = author_edit_text(text, 6, b"world", b"there", nid(0x47));
    edit.span_id = [0xcd_u8; 32]; // anchors still match, span_id does not
    let (_r, state) = replay_text_node(
        oid(20),
        text,
        nid(0x47),
        "a.txt",
        0o100_644,
        vec![OperationKind::EditText(edit)],
    );
    let err = state.expect_err("span id mismatch");
    assert!(
        matches!(
            err,
            LifecycleReplayError::TextSpanResolutionFailed {
                reason: text_span::TextSpanResolutionFailure::NoMatchingSpanId,
                ..
            }
        ),
        "got {err:?}"
    );
}

#[test]
fn edit_text_on_non_text_node_fails_closed() {
    // Binary node + EditText → inconsistent.
    let blob = oid(20);
    let edit = author_edit_text(b"x", 0, b"x", b"y", nid(0x48));
    let mut reader = MockReader::new();
    reader.insert_blob(blob, BlobKind::Binary);
    reader.insert_patch(
        oid(2),
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.bin".to_string(),
                node_id: nid(0x48),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::EditText(edit),
        ],
    );
    reader.insert_block(oid(1), &[], &[oid(2)]);
    let err = replay_lineage(&reader, oid(1), oid(1)).expect_err("edit on binary");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn edit_text_missing_current_blob_fails_closed() {
    // CreateFile references a Text blob that is then NOT inserted, so the current text is missing.
    let blob = oid(20);
    let edit = author_edit_text(b"hi", 0, b"hi", b"yo", nid(0x49));
    let mut reader = MockReader::new();
    // deliberately do not insert the blob
    reader.insert_patch(
        oid(2),
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x49),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::EditText(edit),
        ],
    );
    reader.insert_block(oid(1), &[], &[oid(2)]);
    // CreateFile itself fails first (blob-kind resolution) — missing blob, fail closed.
    let err = replay_lineage(&reader, oid(1), oid(1)).expect_err("missing blob");
    assert_eq!(
        err,
        LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id: blob }
    );
}

// ---- E1 (carried from 2c-2c review): ReplaceBinary old-side blob negatives ----

#[test]
fn replace_binary_missing_old_blob_fails_closed() {
    let (b1, b2) = (oid(20), oid(21));
    // old_blob_id b3 is referenced but never seeded.
    let b3 = oid(22);
    let err = replay_with_blobs(
        vec![
            create_binary(nid(0x33), "a.bin", b1),
            OperationKind::ReplaceBinary(ReplaceBinary {
                node_id: nid(0x33),
                old_blob_id: b3,
                new_blob_id: b2,
            }),
        ],
        &[(b1, BlobKind::Binary), (b2, BlobKind::Binary)],
    )
    .expect_err("missing old blob");
    assert_eq!(
        err,
        LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id: b3 }
    );
}

#[test]
fn replace_binary_non_binary_old_blob_fails_closed() {
    let (b1, b2, b3) = (oid(20), oid(21), oid(22));
    let err = replay_with_blobs(
        vec![
            create_binary(nid(0x33), "a.bin", b1),
            OperationKind::ReplaceBinary(ReplaceBinary {
                node_id: nid(0x33),
                old_blob_id: b3,
                new_blob_id: b2,
            }),
        ],
        &[
            (b1, BlobKind::Binary),
            (b2, BlobKind::Binary),
            (b3, BlobKind::Text), // old blob present but non-binary
        ],
    )
    .expect_err("non-binary old blob");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

/// Single-block genesis lineage carrying one patch over several pre-seeded blobs.
fn replay_with_blobs(
    kinds: Vec<OperationKind>,
    blobs: &[(ObjectId, BlobKind)],
) -> std::result::Result<crate::node::node_lifecycle::NodeLifecycleState, LifecycleReplayError> {
    let (genesis, patch) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    for (id, kind) in blobs {
        reader.insert_blob(*id, *kind);
    }
    reader.insert_patch(patch, kinds);
    reader.insert_block(genesis, &[], &[patch]);
    replay_lineage(&reader, genesis, genesis)
}

fn create_binary(node: NodeId, path: &str, blob: ObjectId) -> OperationKind {
    OperationKind::CreateFile(CreateFile {
        path: path.to_string(),
        node_id: node,
        blob_id: blob,
        mode: 0o100_644,
    })
}

#[test]
fn replace_binary_updates_blob_exactly() {
    let (b1, b2) = (oid(20), oid(21));
    let state = replay_with_blobs(
        vec![
            create_binary(nid(0x33), "a.bin", b1),
            OperationKind::ReplaceBinary(ReplaceBinary {
                node_id: nid(0x33),
                old_blob_id: b1,
                new_blob_id: b2,
            }),
        ],
        &[(b1, BlobKind::Binary), (b2, BlobKind::Binary)],
    )
    .expect("replace binary");

    let node = state.live_node(&nid(0x33)).expect("live");
    assert_eq!(node.kind, NodeKind::BinaryFile);
    assert_eq!(
        node.content,
        NodeContent::File {
            blob_id: b2,
            mode: 0o100_644,
        },
        "blob swapped exactly, mode preserved"
    );
}

#[test]
fn replace_binary_missing_new_blob_fails_closed() {
    let (b1, b2) = (oid(20), oid(21));
    // b2 not seeded.
    let err = replay_with_blobs(
        vec![
            create_binary(nid(0x33), "a.bin", b1),
            OperationKind::ReplaceBinary(ReplaceBinary {
                node_id: nid(0x33),
                old_blob_id: b1,
                new_blob_id: b2,
            }),
        ],
        &[(b1, BlobKind::Binary)],
    )
    .expect_err("missing new blob");
    assert_eq!(
        err,
        LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id: b2 }
    );
}

#[test]
fn replace_binary_non_binary_new_blob_is_inconsistent() {
    let (b1, b2) = (oid(20), oid(21));
    let err = replay_with_blobs(
        vec![
            create_binary(nid(0x33), "a.bin", b1),
            OperationKind::ReplaceBinary(ReplaceBinary {
                node_id: nid(0x33),
                old_blob_id: b1,
                new_blob_id: b2,
            }),
        ],
        &[(b1, BlobKind::Binary), (b2, BlobKind::Text)], // new blob is text
    )
    .expect_err("non-binary new blob");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn replace_binary_old_blob_mismatch_is_inconsistent() {
    let (b1, b3, b2) = (oid(20), oid(22), oid(21));
    // old_blob_id is b3 (present, binary) but the live node references b1.
    let err = replay_with_blobs(
        vec![
            create_binary(nid(0x33), "a.bin", b1),
            OperationKind::ReplaceBinary(ReplaceBinary {
                node_id: nid(0x33),
                old_blob_id: b3,
                new_blob_id: b2,
            }),
        ],
        &[
            (b1, BlobKind::Binary),
            (b3, BlobKind::Binary),
            (b2, BlobKind::Binary),
        ],
    )
    .expect_err("old blob mismatch");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

// ---- 2c-2bR: DeleteNode / RenamePath persisted old-state assertions ----

/// create a text file at "a.txt" (blob 20, mode 0o644), then a DeleteNode with the given fields.
fn create_then_delete(
    path: &str,
    old_node_kind: NodeKind,
    preimage: DeleteNodePreimage,
) -> std::result::Result<crate::node::node_lifecycle::NodeLifecycleState, LifecycleReplayError> {
    let blob = oid(20);
    replay_single_patch(
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x11),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::DeleteNode(DeleteNode {
                path: path.to_string(),
                node_id: nid(0x11),
                old_node_kind,
                preimage,
            }),
        ],
        Some((blob, BlobKind::Text)),
    )
}

fn file_preimage(blob: ObjectId, mode: u32) -> DeleteNodePreimage {
    DeleteNodePreimage::File {
        old_blob_id: blob,
        old_mode: mode,
    }
}

#[test]
fn delete_with_wrong_path_is_inconsistent() {
    let err = create_then_delete(
        "other.txt",
        NodeKind::TextFile,
        file_preimage(oid(20), 0o100_644),
    )
    .expect_err("wrong path");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn delete_with_wrong_kind_is_inconsistent() {
    let err = create_then_delete(
        "a.txt",
        NodeKind::BinaryFile,
        file_preimage(oid(20), 0o100_644),
    )
    .expect_err("wrong kind");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn delete_with_wrong_blob_is_inconsistent() {
    let err = create_then_delete(
        "a.txt",
        NodeKind::TextFile,
        file_preimage(oid(99), 0o100_644),
    )
    .expect_err("wrong blob");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn delete_with_wrong_mode_is_inconsistent() {
    let err = create_then_delete(
        "a.txt",
        NodeKind::TextFile,
        file_preimage(oid(20), 0o100_755), // wrong -- the live node's mode is 0o100_644
    )
    .expect_err("wrong mode");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn symlink_delete_with_wrong_target_is_inconsistent() {
    let err = replay_single_patch(
        vec![
            OperationKind::CreateSymlink(CreateSymlink {
                path: "link".to_string(),
                node_id: nid(0x22),
                target: "dest".to_string(),
            }),
            OperationKind::DeleteNode(DeleteNode {
                path: "link".to_string(),
                node_id: nid(0x22),
                old_node_kind: NodeKind::Symlink,
                preimage: DeleteNodePreimage::Symlink {
                    old_target: "wrong".to_string(),
                },
            }),
        ],
        None,
    )
    .expect_err("wrong symlink target");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

#[test]
fn correct_symlink_delete_tombstones_exactly() {
    let state = replay_single_patch(
        vec![
            OperationKind::CreateSymlink(CreateSymlink {
                path: "link".to_string(),
                node_id: nid(0x22),
                target: "dest".to_string(),
            }),
            OperationKind::DeleteNode(DeleteNode {
                path: "link".to_string(),
                node_id: nid(0x22),
                old_node_kind: NodeKind::Symlink,
                preimage: DeleteNodePreimage::Symlink {
                    old_target: "dest".to_string(),
                },
            }),
        ],
        None,
    )
    .expect("correct symlink delete");
    assert!(state.live_node(&nid(0x22)).is_none());
    let tombstone = state.latest_tombstone(&nid(0x22)).expect("tombstone");
    assert_eq!(
        tombstone.content,
        NodeContent::Symlink {
            target: "dest".to_string(),
        }
    );
}

#[test]
fn rename_with_wrong_old_path_is_inconsistent() {
    let blob = oid(20);
    let err = replay_single_patch(
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: nid(0x11),
                blob_id: blob,
                mode: 0o100_644,
            }),
            OperationKind::RenamePath(RenamePath {
                node_id: nid(0x11),
                old_path: "wrong.txt".to_string(),
                new_path: "b.txt".to_string(),
            }),
        ],
        Some((blob, BlobKind::Text)),
    )
    .expect_err("wrong old_path");
    assert!(
        matches!(
            err,
            LifecycleReplayError::InconsistentLifecycleEffect { .. }
        ),
        "got {err:?}"
    );
}

// ---- 2c-2e: producers (replay_derived_state, certified_compared_cache) ----

use crate::lifecycle_cache::{
    CACHE_SCHEMA_VERSION, CacheCertificationError, DecodedLifecycleCache, ParentPolicy,
    ReplayDerivedLifecycleState, certified_compared_cache, compute_window_hash,
    replay_derived_state,
};
use crate::node::node_lifecycle::LiveNode;

/// A genesis-only repo with one live text node (0x11 a.txt -> `blob`).
fn single_node_reader(blob: ObjectId, content: &[u8]) -> (MockReader, ObjectId) {
    let (genesis, patch) = (oid(1), oid(2));
    let mut reader = MockReader::new();
    reader.insert_text_blob(blob, content);
    reader.insert_patch(
        patch,
        vec![OperationKind::CreateFile(CreateFile {
            path: "a.txt".to_string(),
            node_id: nid(0x11),
            blob_id: blob,
            mode: 0o100_644,
        })],
    );
    reader.insert_block(genesis, &[], &[patch]);
    (reader, genesis)
}

fn cache_for(blob: ObjectId, genesis: ObjectId) -> DecodedLifecycleCache {
    DecodedLifecycleCache {
        schema_version: CACHE_SCHEMA_VERSION,
        baseline_block_id: genesis,
        lineage_horizon_id: genesis,
        parent_policy: ParentPolicy::SingleParent,
        replay_window_hash: compute_window_hash(&[genesis]),
        snapshot_blob_id: None,
        live_entries: vec![(
            nid(0x11),
            LiveNode {
                path: RepoPath::parse("a.txt").unwrap(),
                kind: NodeKind::TextFile,
                content: NodeContent::File {
                    blob_id: blob,
                    mode: 0o100_644,
                },
            },
        )],
        tombstones: vec![],
        seen_ids: vec![nid(0x11)],
    }
}

#[test]
fn replay_derived_state_equals_from_replay_of_replay_lineage() {
    let (reader, genesis) = single_node_reader(oid(20), b"hello");
    let state = replay_lineage(&reader, genesis, genesis).expect("replay");
    let expected = ReplayDerivedLifecycleState::from_replay(genesis, state).expect("wrap");
    let derived = replay_derived_state(&reader, genesis, genesis).expect("producer");
    assert_eq!(derived, expected);
}

#[test]
fn certified_compared_cache_accepts_matching_cache() {
    let (reader, genesis) = single_node_reader(oid(20), b"hello");
    let cache = cache_for(oid(20), genesis);
    assert!(certified_compared_cache(&reader, cache, genesis, genesis).is_ok());
}

#[test]
fn certified_compared_cache_rejects_content_disagreeing_with_replay() {
    // Cache claims the node references a *different* (but still valid Text) blob than replay
    // derives. Provenance + blob-kind pass; the full compare against replay must fail closed.
    let mut reader = MockReader::new();
    let (genesis, patch, real_blob, false_blob) = (oid(1), oid(2), oid(20), oid(21));
    reader.insert_text_blob(real_blob, b"hello");
    reader.insert_text_blob(false_blob, b"goodbye");
    reader.insert_patch(
        patch,
        vec![OperationKind::CreateFile(CreateFile {
            path: "a.txt".to_string(),
            node_id: nid(0x11),
            blob_id: real_blob,
            mode: 0o100_644,
        })],
    );
    reader.insert_block(genesis, &[], &[patch]);

    let cache = cache_for(false_blob, genesis); // wrong content id
    let err = certified_compared_cache(&reader, cache, genesis, genesis)
        .expect_err("content disagreement");
    assert!(matches!(err, CacheCertificationError::ContentMismatch));
    assert!(format!("{err}").contains("disagree with authoritative replay"));
}

#[test]
fn certified_compared_cache_rejects_wrong_baseline() {
    let (reader, genesis) = single_node_reader(oid(20), b"hello");
    let cache = cache_for(oid(20), genesis);
    // Caller intends a different baseline than the cache/repo genesis.
    let err = certified_compared_cache(&reader, cache, oid(0x7e), genesis)
        .expect_err("baseline mismatch");
    assert!(matches!(
        err,
        CacheCertificationError::BaselineMismatch { .. }
    ));
    assert!(format!("{err}").contains("baseline mismatch"));
}

#[test]
fn certified_compared_cache_rejects_horizon_mismatch() {
    // E2: the cache declares the genesis horizon, but the caller binds a different intended
    // horizon. This must fail closed up front, before validation or replay, with a structured
    // horizon mismatch — not implicitly via a downstream window-hash failure.
    let (reader, genesis) = single_node_reader(oid(20), b"hello");
    let cache = cache_for(oid(20), genesis);
    let err =
        certified_compared_cache(&reader, cache, genesis, oid(0x7e)).expect_err("horizon mismatch");
    assert!(matches!(
        err,
        CacheCertificationError::HorizonMismatch { .. }
    ));
    assert!(format!("{err}").contains("horizon mismatch"));
}

// ---- 2c-3 (E4): single-read lineage walk ----

/// A two-block single-parent lineage: genesis (creates 0x11 a.txt) <- child (creates 0x12 b.txt).
/// Returns (reader, genesis_block, child_block).
fn two_block_lineage() -> (MockReader, ObjectId, ObjectId) {
    let (genesis, child, patch_g, patch_c, blob_a, blob_b) =
        (oid(1), oid(2), oid(10), oid(11), oid(20), oid(21));
    let mut reader = MockReader::new();
    reader.insert_text_blob(blob_a, b"alpha");
    reader.insert_text_blob(blob_b, b"beta");
    reader.insert_patch(
        patch_g,
        vec![OperationKind::CreateFile(CreateFile {
            path: "a.txt".to_string(),
            node_id: nid(0x11),
            blob_id: blob_a,
            mode: 0o100_644,
        })],
    );
    reader.insert_patch(
        patch_c,
        vec![OperationKind::CreateFile(CreateFile {
            path: "b.txt".to_string(),
            node_id: nid(0x12),
            blob_id: blob_b,
            mode: 0o100_644,
        })],
    );
    reader.insert_block(genesis, &[], &[patch_g]);
    reader.insert_block(child, &[genesis], &[patch_c]);
    (reader, genesis, child)
}

#[test]
fn replay_reads_each_lineage_block_exactly_once() {
    // E4: the payload-retaining walk must read each lineage block exactly once. Before E4 the
    // ids-then-re-read shape read every block twice.
    let (reader, genesis, child) = two_block_lineage();
    let state = replay_lineage(&reader, child, genesis).expect("replay");
    assert!(state.live_node(&nid(0x11)).is_some());
    assert!(state.live_node(&nid(0x12)).is_some());
    assert_eq!(reader.read_count(genesis), 1, "genesis block read once");
    assert_eq!(reader.read_count(child), 1, "child block read once");
}

/// A reader wrapper that panics if any *Block* object is read more than once, so a re-read of a
/// lineage block during replay is caught structurally. Non-block objects (patches, blobs) are
/// unaffected.
struct PanicOnSecondBlockRead<'a> {
    inner: &'a MockReader,
    blocks_read: std::cell::RefCell<std::collections::BTreeSet<ObjectId>>,
}

impl<'a> PanicOnSecondBlockRead<'a> {
    fn new(inner: &'a MockReader) -> Self {
        Self {
            inner,
            blocks_read: std::cell::RefCell::new(std::collections::BTreeSet::new()),
        }
    }
}

impl ObjectReader for PanicOnSecondBlockRead<'_> {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        let envelope = self.inner.read_object(id)?;
        if let Some(ref e) = envelope {
            if e.object_type == ObjectType::Block && !self.blocks_read.borrow_mut().insert(id) {
                panic!("lineage block {id} read more than once: E4 double-read regression");
            }
        }
        Ok(envelope)
    }
}

#[test]
fn replay_does_not_re_read_lineage_blocks() {
    // E4 drift-prevention: replay must succeed using only the payload retained by the walk's single
    // read. If any block were read a second time, the guard panics and this test fails.
    let (reader, genesis, child) = two_block_lineage();
    let guard = PanicOnSecondBlockRead::new(&reader);
    let state = replay_lineage(&guard, child, genesis).expect("replay over read-once guard");
    assert!(state.live_node(&nid(0x11)).is_some());
    assert!(state.live_node(&nid(0x12)).is_some());
}
