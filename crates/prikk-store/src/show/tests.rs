//! `prikk show` tests (RFC 142). Each control named in the handoff is checked here at the
//! `prikk-store` level; the CLI's own prose/JSON rendering and exit codes are checked separately
//! in `crates/prikk-cli/tests/`.
//!
//! `RenamePath`, `ChangePerm`, `CreateSymlink`, and `ReplaceBinary` are not reachable through the
//! ordinary `commit` path (`WorktreeChangeKind` only ever detects `Missing`/`Modified`/
//! `Untracked` -- renames, permission changes, symlinks, and binary content are never diffed from
//! a real worktree today; `worktree_status/tests.rs`'s own `append_raw_patch` doc says so for
//! `RenamePath` and the same reasoning covers the others). Built with directly-appended raw Patch
//! envelopes, the same mechanism that file already established, not a shortcut around a real
//! path. A node-addressed raw operation's target must already be a *live* node at replay/seal
//! time (found empirically: an arbitrary node id is rejected at `seal`, not at `show`), so every
//! such fixture creates its node in its own sealed block first, then operates on that same real
//! node id in a later block.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, ChangePerm, CreateFile, CreateSymlink, DeleteNode,
    DeleteNodePreimage, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose, RenamePath, ReplaceBinary,
};

use super::{ShowBlobContent, ShowDeletePreimage, ShowOperationContent, ShowPathResolution, show};
use crate::author::author_signing::author_signature;
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::object_store::{ObjectWriteSession, ObjectWriter};
use crate::refs::RefStore;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout,
    WorktreePatchCommitOptions,
};

fn author_signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("show-author", &[0x33; 32]).unwrap()
}

fn maintainer_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("show-maintainer", &[0x44; 32]).unwrap()
}

fn trust_maintainer(layout: &RepositoryLayout, maintainer: &Ed25519MaintainerSigner) {
    crate::trust::add_trusted_maintainer(
        layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .unwrap();
}

fn commit(layout: &RepositoryLayout, message: &str) {
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        message,
        WorktreePatchCommitOptions::default(),
        &author_signer(),
    )
    .unwrap();
}

fn seal(layout: &RepositoryLayout) {
    crate::rfc111_seal_simulation::simulate_one_seal(layout, "heads/main", &maintainer_signer())
        .unwrap();
}

/// Write, commit, and seal one file in its own block -- the genesis fixture most tests start
/// from.
fn generation(layout: &RepositoryLayout, path: &str, bytes: &[u8], message: &str) {
    std::fs::write(layout.root().join(path), bytes).unwrap();
    commit(layout, message);
    seal(layout);
}

/// The block id `heads/main` currently points at, after sealing.
fn current_block_id(layout: &RepositoryLayout) -> ObjectId {
    let ref_store = RefStore::new(layout.clone());
    let ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .expect("heads/main has been sealed");
    let object_store = crate::object_store::ObjectReadSnapshot::open(layout).unwrap();
    let envelope = crate::object_store::ObjectReader::read_typed(
        &object_store,
        ref_state_id,
        ObjectType::RefState,
    )
    .unwrap()
    .expect("RefState exists");
    let payload = prikk_object::RefStatePayload::decode_canonical(
        &envelope.canonical_payload,
        envelope.schema_version,
    )
    .unwrap();
    payload.target_object_id
}

/// An envelope for tests that call `show_operation` directly on a non-`RenamePath` operation --
/// its own content is never inspected by any of those kinds, only `RenamePath` reads the
/// envelope's AUTHOR signature.
fn dummy_envelope() -> ObjectEnvelope {
    ObjectEnvelope::unsigned(ObjectType::Patch, 1, Vec::new())
}

/// Append a raw, directly-constructed Patch envelope to the active WAL, bypassing ordinary
/// authoring (see this file's own module doc). Also writes the active-ref metadata
/// `commit_worktree_changes_signed` would normally set up and `seal` requires
/// (`validate_signer_backed_recovery`) -- cleared after every successful seal, so a raw append
/// must re-establish it itself, real commits never need to (found the same way as this file's
/// other raw-append gaps: by trying the append alone first and letting `seal` reject it).
fn append_raw_patch(layout: &RepositoryLayout, operations: Vec<Operation>) -> ObjectId {
    crate::write_active_ref_metadata(layout, "heads/main").unwrap();
    let payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let bytes = payload.to_canonical_bytes().unwrap();
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, bytes);
    let id = envelope.object_id();
    envelope
        .add_signature(author_signature(&author_signer(), id).unwrap())
        .unwrap();
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    id
}

/// Write a Blob object directly, returning its id.
fn write_blob(layout: &RepositoryLayout, kind: BlobKind, content: Vec<u8>) -> ObjectId {
    let declared_size = content.len() as u64;
    let payload = BlobPayload {
        blob_kind: kind,
        content,
        declared_size,
    };
    let bytes = payload.to_canonical_bytes().unwrap();
    let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
    let mut object_store = ObjectWriteSession::open(layout).unwrap();
    object_store.write_object(&envelope).unwrap()
}

/// Raw `CreateFile`, sealed in its own block, giving a real live node at a chosen id -- the
/// prerequisite every node-addressed raw fixture below needs (see this file's own module doc).
/// Also writes `content` to the real worktree file at `path`: a later *real* commit diffs the
/// actual filesystem against the replayed baseline, and a baseline file absent from the worktree
/// is queued as a `DeleteNode` automatically (RFC 142 §3's own "reachable case," triggered here by
/// accident rather than on purpose the first time this file was written -- found by that exact
/// failure, not anticipated).
fn create_raw_file(
    layout: &RepositoryLayout,
    node_id: NodeId,
    path: &str,
    kind: BlobKind,
    content: &[u8],
) -> ObjectId {
    let blob_id = write_blob(layout, kind, content.to_vec());
    std::fs::write(layout.root().join(path), content).unwrap();
    append_raw_patch(
        layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id,
                blob_id,
                mode: 0o100_644,
            }),
        }],
    );
    seal(layout);
    blob_id
}

/// stikk 020, "the end must agree": after folding every one of `block_id`'s own patches onto its
/// parent's state (the identical primitives `show`'s own `Block` arm uses --
/// `super::block_starting_state` and
/// [`crate::lifecycle_cache::replay::apply_one_patch_with_materialize_fallback`], 0.54.0 Part 5c),
/// the result must equal `lifecycle_state_at(block_id)`'s own state -- computed independently, by
/// the *existing*, unmodified full-lineage replay that never calls either of those two. A control
/// on the test suite (per the ruling), not a runtime check.
fn assert_fold_to_end_matches_lifecycle_state_at(layout: &RepositoryLayout, block_id: ObjectId) {
    let object_store = crate::object_store::ObjectReadSnapshot::open(layout).unwrap();
    let envelope =
        crate::object_store::ObjectReader::read_typed(&object_store, block_id, ObjectType::Block)
            .unwrap()
            .unwrap();
    let block = prikk_object::BlockPayload::decode_canonical(&envelope.canonical_payload).unwrap();
    let (mut state, parent_id) =
        super::block_starting_state(&object_store, block_id, &block).unwrap();
    let sealed_lineage = parent_id.map(|parent_id| {
        (
            parent_id,
            crate::merge::evidence::lineage_horizon(&object_store, parent_id).unwrap(),
        )
    });
    let mut text_cache = crate::lifecycle_cache::replay::TextCache::new();
    for patch_id in &block.patch_ids {
        crate::lifecycle_cache::replay::apply_one_patch_with_materialize_fallback(
            Some(layout),
            &object_store,
            *patch_id,
            &mut state,
            &mut text_cache,
            sealed_lineage,
        )
        .unwrap();
    }
    let end_state = crate::merge::evidence::lifecycle_state_at(&object_store, block_id).unwrap();
    assert_eq!(
        &state,
        end_state.state(),
        "folding this block's own patches onto its parent's state must agree with \
         lifecycle_state_at(block_id)'s own, independently-computed, full-lineage state"
    );
}

/// Control 1: a text edit shows its before and after.
#[test]
fn show_reports_a_text_edits_before_and_after() {
    let root = unique_temp_dir("show-text-edit");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"before\n", "genesis");

    std::fs::write(root.join("a.txt"), b"after\n").unwrap();
    commit(&layout, "edit a.txt");
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [patch] = patches.as_slice() else {
        panic!("expected exactly one patch, got {patches:?}");
    };
    let [operation] = patch.operations.as_slice() else {
        panic!("expected exactly one operation, got {:?}", patch.operations);
    };
    assert_eq!(operation.kind, "edit-text");
    match &operation.content {
        ShowOperationContent::EditText {
            old_span_text,
            replacement_text,
        } => {
            // The trailing `\n` is common to both sides, so the content-anchored span the real
            // text-diff machinery identifies is "before"/"after" without it -- not a `show` choice,
            // the diff itself never touches the shared suffix.
            assert_eq!(old_span_text, b"before");
            assert_eq!(replacement_text, b"after");
        }
        other => panic!("expected EditText, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// Control 2: a node-addressed operation resolves to a path, not a node id (RFC 140's control 1
/// one level up).
#[test]
fn show_resolves_a_node_addressed_operation_to_a_path() {
    let root = unique_temp_dir("show-resolve-path");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"before\n", "genesis");

    std::fs::write(root.join("a.txt"), b"after\n").unwrap();
    commit(&layout, "edit a.txt");
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [patch] = patches.as_slice() else {
        panic!("expected exactly one patch, got {patches:?}");
    };
    let [operation] = patch.operations.as_slice() else {
        panic!("expected exactly one operation, got {:?}", patch.operations);
    };
    assert_eq!(
        operation.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())]
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Control 3, rebuilt as originally specified (RFC 142 follow-up handoff §5 item 1): round 1
/// substituted `ChangePerm` for the handoff's own `EditText` example to avoid a code path it
/// mischaracterized as a `worktree_patch` defect (RFC 142 §3a: it is not one -- a text node's
/// pre-edit `blob_id` is *deliberately* unbacked once the node has been edited, DC-65). **A
/// control rebuilt to avoid the thing it tests has stopped being a control**, so this is the exact
/// sequence: create `a.txt`, seal; edit it, seal; delete it, seal.
///
/// **stikk 020 (0.52.0 step 1 Part C):** the edit and the delete are two *separate* patches sealed
/// into one block. Per the ruling, each patch resolves against its own state -- the block's
/// starting state, folded with the block's own patches up to and including that patch -- so the
/// edit patch's own node-addressed path resolves against the state *right after the edit*, where
/// the node is still live: `Path("a.txt")`, not `Unresolved`. (Before this fix, the whole block
/// resolved against its *end* state, where the node is already dead, and this assertion read
/// `Unresolved` -- that was the defect, not a true "unresolvable" case: see
/// [`show_degrades_when_one_patch_both_edits_and_deletes_the_same_node`] below for the case that is
/// still genuinely unresolvable under the fix.) Every operation still renders, and the delete's own
/// preimage content still degrades to [`ShowBlobContent::Unavailable`] (§6a) rather than failing
/// the command -- exit `0`, not `1`.
#[test]
fn show_reports_unresolved_and_degrades_when_a_block_edits_and_deletes_the_same_node() {
    let root = unique_temp_dir("show-unresolved");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"before\n", "genesis");

    std::fs::write(root.join("a.txt"), b"after\n").unwrap();
    commit(&layout, "edit a.txt");
    std::fs::remove_file(root.join("a.txt")).unwrap();
    commit(&layout, "delete a.txt");
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [edit_patch, delete_patch] = patches.as_slice() else {
        panic!("expected exactly two patches (edit, delete), got {patches:?}");
    };
    let [edit_op] = edit_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", edit_patch.operations);
    };
    assert_eq!(edit_op.kind, "edit-text");
    assert_eq!(
        edit_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())],
        "stikk 020: the edit patch resolves against its OWN state (node still live), not the \
         block's end state (node already dead by the later delete patch)"
    );

    let [delete_op] = delete_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", delete_patch.operations);
    };
    assert_eq!(delete_op.kind, "delete-node");
    assert_eq!(
        delete_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())]
    );
    match &delete_op.content {
        ShowOperationContent::DeleteNode {
            preimage: ShowDeletePreimage::File(ShowBlobContent::Unavailable { .. }),
        } => {}
        other => panic!(
            "expected a File preimage degraded to Unavailable (the pre-edit blob id is never \
             backed by a stored object, DC-65), got {other:?}"
        ),
    }

    assert_fold_to_end_matches_lifecycle_state_at(&layout, block_id);

    let _ = std::fs::remove_dir_all(root);
}

/// stikk 020, item 3 of "what the round needs": the still-genuinely-unresolvable shape, now that
/// cross-patch edit-then-delete no longer degrades. One patch whose own operations both change and
/// delete the same node, sealed into its own block -- the block form's sibling of
/// [`show_reports_unresolved_when_a_queued_patch_both_changes_and_deletes_the_same_node`], which
/// already establishes this exact shape at the queued level. `ChangePerm` stands in for `EditText`
/// for the same reason that test uses it: both are node-addressed and resolved by the identical
/// `resolve_node_path` call, so this exercises the same resolution mechanism without `EditText`'s
/// span-anchor construction.
#[test]
fn show_degrades_when_one_patch_both_edits_and_deletes_the_same_node() {
    let root = unique_temp_dir("show-unresolved-same-patch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_id = NodeId::from_bytes([0x61; 32]);
    let blob_id = create_raw_file(&layout, node_id, "a.txt", BlobKind::Text, b"content\n");

    append_raw_patch(
        &layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id,
                    old_mode: 0o100_644,
                    new_mode: 0o100_755,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "a.txt".to_string(),
                    node_id,
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: blob_id,
                        old_mode: 0o100_755,
                    },
                }),
            },
        ],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [patch] = patches.as_slice() else {
        panic!("expected exactly one patch, got {patches:?}");
    };
    let [perm_op, delete_op] = patch.operations.as_slice() else {
        panic!("expected two operations, got {:?}", patch.operations);
    };
    assert_eq!(perm_op.kind, "change-perm");
    match perm_op.paths.as_slice() {
        [ShowPathResolution::Unresolved { .. }] => {}
        other => panic!(
            "expected Unresolved for a node this same patch goes on to delete, got {other:?}"
        ),
    }
    assert_eq!(delete_op.kind, "delete-node");
    assert_eq!(
        delete_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())]
    );

    assert_fold_to_end_matches_lifecycle_state_at(&layout, block_id);

    let _ = std::fs::remove_dir_all(root);
}

/// Handoff control 3: each of the four blob dereference sites degrades. Driven directly at this
/// level (not through a real `commit`/`seal`), because seal-time replay *validates* three of the
/// four against a real object already: `CreateFile`'s own blob (`lifecycle_cache/replay/
/// effect.rs`'s `blob_resolver.blob_kind`) and both of `ReplaceBinary`'s (`require_binary_blob`)
/// are checked to exist and resolve at the moment they are sealed, so an ordinarily-sealed
/// repository can never carry an invented, never-written reference at those three sites -- only
/// `DeleteNode`'s preimage can (DC-65, demonstrated above). Constructing the operations directly
/// against a store that never wrote the referenced blobs exercises the same code
/// (`show_operation`/`show_blob_content`) without needing to simulate post-seal object-store
/// corruption to reach the other three.
#[test]
fn show_degrades_all_four_blob_dereference_sites_on_absence() {
    use crate::memory_store::MemoryObjectStore;
    use crate::patch_replay::decode::{
        DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation,
    };

    let store = MemoryObjectStore::new();
    let never_written_a = ObjectId::from_bytes([0xA1; 32]);
    let never_written_b = ObjectId::from_bytes([0xA2; 32]);

    let create_file = DecodedPatchOperation {
        op_seq: 1,
        kind: DecodedOperationKind::CreateFile {
            path: "a.txt".to_string(),
            node_id: NodeId::from_bytes([0xB1; 32]),
            blob_id: never_written_a,
            mode: 0o100_644,
        },
        patch_id: None,
    };
    let shown = super::show_operation(&store, &dummy_envelope(), &create_file, None).unwrap();
    match shown.content {
        ShowOperationContent::CreateFile {
            content: ShowBlobContent::Unavailable { blob_id },
            ..
        } => assert_eq!(blob_id, never_written_a),
        other => panic!("expected CreateFile content Unavailable, got {other:?}"),
    }

    let delete_node = DecodedPatchOperation {
        op_seq: 1,
        kind: DecodedOperationKind::DeleteNode {
            path: "b.txt".to_string(),
            node_id: NodeId::from_bytes([0xB2; 32]),
            preimage: DecodedDeletePreimage::File {
                old_node_kind: NodeKind::TextFile,
                old_blob_id: never_written_a,
                old_mode: 0o100_644,
            },
        },
        patch_id: None,
    };
    let shown = super::show_operation(&store, &dummy_envelope(), &delete_node, None).unwrap();
    match shown.content {
        ShowOperationContent::DeleteNode {
            preimage: ShowDeletePreimage::File(ShowBlobContent::Unavailable { blob_id }),
        } => assert_eq!(blob_id, never_written_a),
        other => panic!("expected DeleteNode preimage Unavailable, got {other:?}"),
    }

    let replace_binary = DecodedPatchOperation {
        op_seq: 1,
        kind: DecodedOperationKind::ReplaceBinary {
            node_id: NodeId::from_bytes([0xB3; 32]),
            old_blob_id: never_written_a,
            new_blob_id: never_written_b,
        },
        patch_id: None,
    };
    let shown = super::show_operation(&store, &dummy_envelope(), &replace_binary, None).unwrap();
    match shown.content {
        ShowOperationContent::ReplaceBinary {
            old: ShowBlobContent::Unavailable { blob_id: old_id },
            new: ShowBlobContent::Unavailable { blob_id: new_id },
        } => {
            assert_eq!(old_id, never_written_a);
            assert_eq!(new_id, never_written_b);
        }
        other => panic!("expected both ReplaceBinary sides Unavailable, got {other:?}"),
    }
}

/// RFC 142 §6b, control 3's error-case sibling: an object the store affirmatively reports as
/// damaged -- a type mismatch, a malformed payload, or a `SNAPSHOT`-kind blob named by a
/// file-content operation -- propagates a real error rather than degrading. Three distinct error
/// classes, each asserted to actually fail (not merely "not panic"), so this cannot pass by
/// accident the way an unconstrained `Result` check could.
#[test]
fn show_propagates_an_error_for_a_damaged_object_distinct_from_absence() {
    use crate::memory_store::MemoryObjectStore;
    use crate::patch_replay::decode::{DecodedOperationKind, DecodedPatchOperation};

    // Type mismatch: the id genuinely resolves, but to a Patch, not a Blob.
    let mut store = MemoryObjectStore::new();
    let wrong_type_envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, b"not a blob".to_vec());
    let wrong_type_id = store.write_object(&wrong_type_envelope).unwrap();
    let create_file = DecodedPatchOperation {
        op_seq: 1,
        kind: DecodedOperationKind::CreateFile {
            path: "a.txt".to_string(),
            node_id: NodeId::from_bytes([0xC1; 32]),
            blob_id: wrong_type_id,
            mode: 0o100_644,
        },
        patch_id: None,
    };
    assert!(
        super::show_operation(&store, &dummy_envelope(), &create_file, None).is_err(),
        "a type-mismatched object must propagate an error, not degrade"
    );

    // Malformed payload: a real Blob-typed object whose bytes are not a valid BlobPayload.
    let mut store = MemoryObjectStore::new();
    let malformed_envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, b"garbage".to_vec());
    let malformed_id = store.write_object(&malformed_envelope).unwrap();
    let create_file = DecodedPatchOperation {
        op_seq: 1,
        kind: DecodedOperationKind::CreateFile {
            path: "a.txt".to_string(),
            node_id: NodeId::from_bytes([0xC2; 32]),
            blob_id: malformed_id,
            mode: 0o100_644,
        },
        patch_id: None,
    };
    assert!(
        super::show_operation(&store, &dummy_envelope(), &create_file, None).is_err(),
        "a malformed Blob payload must propagate an error, not degrade"
    );

    // A real, well-formed Blob -- but SNAPSHOT-kind, which no file-content operation may
    // legitimately name (RFC 142 §6b: "goes back to being loud").
    let mut store = MemoryObjectStore::new();
    let snapshot_payload = BlobPayload {
        blob_kind: BlobKind::Snapshot,
        content: b"snapshot bytes".to_vec(),
        declared_size: b"snapshot bytes".len() as u64,
    };
    let snapshot_envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        snapshot_payload.to_canonical_bytes().unwrap(),
    );
    let snapshot_id = store.write_object(&snapshot_envelope).unwrap();
    let create_file = DecodedPatchOperation {
        op_seq: 1,
        kind: DecodedOperationKind::CreateFile {
            path: "a.txt".to_string(),
            node_id: NodeId::from_bytes([0xC3; 32]),
            blob_id: snapshot_id,
            mode: 0o100_644,
        },
        patch_id: None,
    };
    assert!(
        super::show_operation(&store, &dummy_envelope(), &create_file, None).is_err(),
        "a SNAPSHOT-kind blob named by CreateFile must propagate an error, not degrade"
    );
}

/// Control 4: a mixed block -- create, edit, delete, rename -- each renders with its own content
/// and path(s); the rename reports both endpoints.
#[test]
fn show_renders_a_mixed_block_with_its_own_content_and_paths() {
    let root = unique_temp_dir("show-mixed-block");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "edited.txt", b"before\n", "genesis");
    generation(&layout, "deleted.txt", b"gone\n", "second");
    let rename_node_id = NodeId::from_bytes([0x9F; 32]);
    create_raw_file(
        &layout,
        rename_node_id,
        "to-rename.txt",
        BlobKind::Text,
        b"rename me\n",
    );

    std::fs::write(root.join("new.txt"), b"brand new\n").unwrap();
    commit(&layout, "create new.txt");
    std::fs::write(root.join("edited.txt"), b"after\n").unwrap();
    commit(&layout, "edit edited.txt");
    std::fs::remove_file(root.join("deleted.txt")).unwrap();
    commit(&layout, "delete deleted.txt");
    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id: rename_node_id,
                old_path: "to-rename.txt".to_string(),
                new_path: "renamed.txt".to_string(),
            }),
        }],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [create_patch, edit_patch, delete_patch, rename_patch] = patches.as_slice() else {
        panic!("expected exactly four patches, one per commit, got {patches:?}");
    };

    let [create_op] = create_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", create_patch.operations);
    };
    assert_eq!(create_op.kind, "create-file");
    assert_eq!(
        create_op.paths,
        vec![ShowPathResolution::Path("new.txt".to_string())]
    );
    match &create_op.content {
        ShowOperationContent::CreateFile { content, .. } => {
            assert_eq!(content, &ShowBlobContent::Text(b"brand new\n".to_vec()));
        }
        other => panic!("expected CreateFile, got {other:?}"),
    }

    let [edit_op] = edit_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", edit_patch.operations);
    };
    assert_eq!(edit_op.kind, "edit-text");
    assert_eq!(
        edit_op.paths,
        vec![ShowPathResolution::Path("edited.txt".to_string())]
    );

    let [delete_op] = delete_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", delete_patch.operations);
    };
    assert_eq!(delete_op.kind, "delete-node");
    assert_eq!(
        delete_op.paths,
        vec![ShowPathResolution::Path("deleted.txt".to_string())]
    );
    match &delete_op.content {
        ShowOperationContent::DeleteNode {
            preimage: ShowDeletePreimage::File(ShowBlobContent::Text(bytes)),
        } => assert_eq!(bytes, b"gone\n"),
        other => panic!("expected a File preimage with text content, got {other:?}"),
    }

    let [rename_op] = rename_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", rename_patch.operations);
    };
    assert_eq!(rename_op.kind, "rename-path");
    assert_eq!(
        rename_op.paths,
        vec![
            ShowPathResolution::Path("to-rename.txt".to_string()),
            ShowPathResolution::Path("renamed.txt".to_string()),
        ]
    );
    // RFC 144 §4o.6a control 1: the rename's asserting AUTHOR key id is recoverable in the same
    // answer, through the real `show` path (`append_raw_patch` signs with `author_signer()`,
    // key id "show-author") -- not a hand-constructed `ShowOperationContent` value.
    assert_eq!(
        rename_op.content,
        ShowOperationContent::RenamePath {
            author_key_id: "show-author".to_string(),
            author_key_ids: vec!["show-author".to_string()],
        }
    );

    assert_fold_to_end_matches_lifecycle_state_at(&layout, block_id);

    let _ = std::fs::remove_dir_all(root);
}

/// RFC 144 §4o.6a control 3: a `RenamePath` operand whose patch carries no AUTHOR signature at
/// all fails `show` outright -- never a rendered rename with a blank or absent signer. A
/// missing-signature patch is constructed directly (bypassing `append_raw_patch`, which always
/// signs), the same way `show_propagates_an_error_for_a_damaged_object_distinct_from_absence`
/// above constructs its own error cases directly rather than simulating post-seal corruption.
#[test]
fn show_fails_a_rename_whose_patch_has_no_author_signature() {
    let root = unique_temp_dir("show-rename-no-author-signature");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let rename_node_id = NodeId::from_bytes([0x9E; 32]);
    create_raw_file(
        &layout,
        rename_node_id,
        "to-rename.txt",
        BlobKind::Text,
        b"rename me\n",
    );

    crate::write_active_ref_metadata(&layout, "heads/main").unwrap();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id: rename_node_id,
                old_path: "to-rename.txt".to_string(),
                new_path: "renamed.txt".to_string(),
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    // A signed envelope (the WAL itself refuses an entirely unsigned one), but deliberately no
    // AUTHOR-role signature -- unlike `append_raw_patch`, which always signs with one. A
    // MAINTAINER-role signature is not a stand-in AUTHOR signature (§3: a specific role, not "any
    // signature"); it exists only to get the envelope past the WAL's own signed-envelope check.
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    envelope
        .add_signature(crate::test_gates::test_support::maintainer_signature())
        .unwrap();
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    seal(&layout);

    let block_id = current_block_id(&layout);
    let err = show(&layout, block_id).expect_err(
        "a RenamePath whose patch carries no AUTHOR signature must fail, not render a blank signer",
    );
    let message = err.to_string();
    assert!(
        message.contains("AUTHOR signature"),
        "expected an AUTHOR-signature error, got: {message}"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Control 5: `show` on a patch id and on a block id both work, and a block's output is the union
/// of its patches in order.
#[test]
fn show_on_a_patch_id_and_a_block_id_both_work() {
    let root = unique_temp_dir("show-patch-and-block");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"one\n", "genesis");

    std::fs::write(root.join("b.txt"), b"two\n").unwrap();
    commit(&layout, "create b.txt");
    std::fs::write(root.join("c.txt"), b"three\n").unwrap();
    commit(&layout, "create c.txt");
    seal(&layout);

    let block_id = current_block_id(&layout);
    let from_block = show(&layout, block_id).unwrap();
    let [first_from_block, _second_from_block] = from_block.as_slice() else {
        panic!("expected exactly two patches, got {from_block:?}");
    };

    let from_patch = show(&layout, first_from_block.patch_id).unwrap();
    let [only_from_patch] = from_patch.as_slice() else {
        panic!("expected exactly one patch, got {from_patch:?}");
    };
    assert_eq!(only_from_patch.patch_id, first_from_block.patch_id);
    assert_eq!(only_from_patch.operations, first_from_block.operations);

    let _ = std::fs::remove_dir_all(root);
}

/// A bare (sealed) patch id has no block context passed to `show` at all -- reported unresolved,
/// never fatal, the same mechanism as control 3 for a different reason. Uses a *sealed* patch (an
/// unsealed, still-queued patch id is not yet a readable object at all -- `show` reads through
/// the object store, which only ever sees sealed content; found the same way as control 3's own
/// finding, by trying the unsealed case first).
#[test]
fn show_on_a_bare_sealed_patch_reports_node_addressed_operations_unresolved() {
    let root = unique_temp_dir("show-bare-patch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"before\n", "genesis");

    std::fs::write(root.join("a.txt"), b"after\n").unwrap();
    commit(&layout, "edit a.txt");
    seal(&layout);

    let block_id = current_block_id(&layout);
    let from_block = show(&layout, block_id).unwrap();
    let [block_patch] = from_block.as_slice() else {
        panic!("expected exactly one patch, got {from_block:?}");
    };

    let patches = show(&layout, block_patch.patch_id).unwrap();
    let [patch] = patches.as_slice() else {
        panic!("expected exactly one patch, got {patches:?}");
    };
    let [operation] = patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", patch.operations);
    };
    let [path] = operation.paths.as_slice() else {
        panic!("expected one path, got {:?}", operation.paths);
    };
    match path {
        ShowPathResolution::Unresolved { .. } => {}
        other => panic!("expected Unresolved with no block context, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// `ChangePerm` decodes and renders, appended raw (see this file's own module doc).
#[test]
fn show_renders_change_perm() {
    let root = unique_temp_dir("show-change-perm");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_id = NodeId::from_bytes([0x9B; 32]);
    create_raw_file(&layout, node_id, "a.txt", BlobKind::Text, b"content\n");

    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id,
                old_mode: 0o100_644,
                new_mode: 0o100_755,
            }),
        }],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let Some(last_patch) = patches.last() else {
        panic!("expected at least one patch");
    };
    let [operation] = last_patch.operations.as_slice() else {
        panic!(
            "expected exactly one operation, got {:?}",
            last_patch.operations
        );
    };
    assert_eq!(operation.kind, "change-perm");
    assert_eq!(
        operation.content,
        ShowOperationContent::ChangePerm {
            old_mode: 0o100_644,
            new_mode: 0o100_755,
        }
    );
    assert_eq!(
        operation.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())]
    );

    let _ = std::fs::remove_dir_all(root);
}

/// `CreateSymlink` decodes and renders, appended raw (see this file's own module doc).
#[test]
fn show_renders_create_symlink() {
    let root = unique_temp_dir("show-symlink");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"content\n", "genesis");

    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateSymlink(CreateSymlink {
                path: "link.txt".to_string(),
                node_id: NodeId::from_bytes([0x9C; 32]),
                target: "a.txt".to_string(),
            }),
        }],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let Some(last_patch) = patches.last() else {
        panic!("expected at least one patch");
    };
    let [operation] = last_patch.operations.as_slice() else {
        panic!(
            "expected exactly one operation, got {:?}",
            last_patch.operations
        );
    };
    assert_eq!(operation.kind, "create-symlink");
    assert_eq!(
        operation.paths,
        vec![ShowPathResolution::Path("link.txt".to_string())]
    );
    match &operation.content {
        ShowOperationContent::CreateSymlink { target } => assert_eq!(target, "a.txt"),
        other => panic!("expected CreateSymlink, got {other:?}"),
    }

    let _ = std::fs::remove_dir_all(root);
}

/// `ReplaceBinary` reports blob ids and sizes, never content (RFC 142 §7).
#[test]
fn show_replace_binary_reports_ids_and_sizes_not_content() {
    let root = unique_temp_dir("show-replace-binary");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_id = NodeId::from_bytes([0x9D; 32]);
    let old_blob_id = create_raw_file(&layout, node_id, "bin.dat", BlobKind::Binary, &[0xAA; 10]);

    let new_blob_id = write_blob(&layout, BlobKind::Binary, vec![0xBB; 20]);
    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ReplaceBinary(ReplaceBinary {
                node_id,
                old_blob_id,
                new_blob_id,
            }),
        }],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let Some(last_patch) = patches.last() else {
        panic!("expected at least one patch");
    };
    let [operation] = last_patch.operations.as_slice() else {
        panic!(
            "expected exactly one operation, got {:?}",
            last_patch.operations
        );
    };
    assert_eq!(operation.kind, "replace-binary");
    assert_eq!(
        operation.content,
        ShowOperationContent::ReplaceBinary {
            old: ShowBlobContent::Binary {
                blob_id: old_blob_id,
                size: 10,
            },
            new: ShowBlobContent::Binary {
                blob_id: new_blob_id,
                size: 20,
            },
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// `DeleteNode`'s symlink preimage needs no blob read at all -- the old target is already inline.
#[test]
fn show_delete_node_symlink_preimage_needs_no_blob() {
    let root = unique_temp_dir("show-delete-symlink");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"content\n", "genesis");
    let node_id = NodeId::from_bytes([0x9E; 32]);
    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateSymlink(CreateSymlink {
                path: "link.txt".to_string(),
                node_id,
                target: "a.txt".to_string(),
            }),
        }],
    );
    seal(&layout);

    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::DeleteNode(DeleteNode {
                path: "link.txt".to_string(),
                node_id,
                old_node_kind: NodeKind::Symlink,
                preimage: DeleteNodePreimage::Symlink {
                    old_target: "a.txt".to_string(),
                },
            }),
        }],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let Some(last_patch) = patches.last() else {
        panic!("expected at least one patch");
    };
    let [operation] = last_patch.operations.as_slice() else {
        panic!(
            "expected exactly one operation, got {:?}",
            last_patch.operations
        );
    };
    assert_eq!(
        operation.content,
        ShowOperationContent::DeleteNode {
            preimage: ShowDeletePreimage::Symlink {
                old_target: "a.txt".to_string(),
            },
        }
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Handoff control 1 (the queued-patch-paths handoff, RFC 142): a queued patch's node-addressed
/// operation resolves its path from the folded baseline, and the same patch renders identically
/// once sealed -- `queued` the only difference.
///
/// *Perturb*: resolve against the sealed tip instead of the folded baseline (the pre-fix behavior)
/// -- with nothing sealed since genesis, the tip alone has no live node here, so the queued
/// assertion below goes red.
#[test]
fn show_resolves_a_queued_patchs_node_addressed_path_identically_before_and_after_seal() {
    let root = unique_temp_dir("show-queued-resolve");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"before\n", "genesis");

    std::fs::write(root.join("a.txt"), b"after\n").unwrap();
    commit(&layout, "edit a.txt"); // queued, not sealed

    let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let queued_id = replay.records.last().unwrap().envelope.object_id();

    let queued = show(&layout, queued_id).unwrap();
    let [queued_patch] = queued.as_slice() else {
        panic!("expected exactly one patch, got {queued:?}");
    };
    assert!(queued_patch.queued);
    let [queued_op] = queued_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", queued_patch.operations);
    };
    assert_eq!(queued_op.kind, "edit-text");
    assert_eq!(
        queued_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())],
        "a queued edit-text resolves its path from the folded baseline"
    );

    seal(&layout);
    let block_id = current_block_id(&layout);
    let sealed = show(&layout, block_id).unwrap();
    let [sealed_patch] = sealed.as_slice() else {
        panic!("expected exactly one patch, got {sealed:?}");
    };
    assert_eq!(sealed_patch.patch_id, queued_patch.patch_id);
    assert!(!sealed_patch.queued);
    assert_eq!(
        sealed_patch.operations, queued_patch.operations,
        "a patch renders identically before and after seal, apart from `queued`"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Handoff control 2: truncation matters. Two separate queued patches, the second renaming a node
/// the first changed the mode of -- the first patch's own path must be what it was *at that patch*,
/// before the later rename, not the queue's later state.
///
/// *Perturb*: fold the whole queue for every patch instead of truncating -- the assertion below
/// goes red, reading `b.txt` instead of `a.txt`.
#[test]
fn show_truncates_the_folded_queue_at_the_patch_being_shown() {
    let root = unique_temp_dir("show-queued-truncate");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_id = NodeId::from_bytes([0x96; 32]);
    create_raw_file(&layout, node_id, "a.txt", BlobKind::Text, b"content\n");

    let perm_patch_id = append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id,
                old_mode: 0o100_644,
                new_mode: 0o100_755,
            }),
        }],
    );
    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id,
                old_path: "a.txt".to_string(),
                new_path: "b.txt".to_string(),
            }),
        }],
    );

    let perm_shown = show(&layout, perm_patch_id).unwrap();
    let [perm_patch] = perm_shown.as_slice() else {
        panic!("expected exactly one patch, got {perm_shown:?}");
    };
    assert!(perm_patch.queued);
    let [perm_op] = perm_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", perm_patch.operations);
    };
    assert_eq!(perm_op.kind, "change-perm");
    assert_eq!(
        perm_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())],
        "the earlier patch's path is what it was at that patch, before the later rename"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Handoff control 3: a node the same queued patch later deletes reports `unresolved_node_id` for
/// its earlier operation -- the same mechanism the sealed-block case already has
/// (`show_reports_unresolved_and_degrades_when_a_block_edits_and_deletes_the_same_node`), applied
/// within one still-queued patch rather than across a sealed block's several patches: control 2's
/// truncation means a *later, separate* queued patch's deletion cannot reach back into an earlier
/// patch's own answer, so this needs both operations in the one patch being shown.
#[test]
fn show_reports_unresolved_when_a_queued_patch_both_changes_and_deletes_the_same_node() {
    let root = unique_temp_dir("show-queued-unresolved");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_id = NodeId::from_bytes([0x97; 32]);
    let blob_id = create_raw_file(&layout, node_id, "a.txt", BlobKind::Text, b"content\n");

    let patch_id = append_raw_patch(
        &layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id,
                    old_mode: 0o100_644,
                    new_mode: 0o100_755,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "a.txt".to_string(),
                    node_id,
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: blob_id,
                        old_mode: 0o100_755,
                    },
                }),
            },
        ],
    );

    let shown = show(&layout, patch_id).unwrap();
    let [patch] = shown.as_slice() else {
        panic!("expected exactly one patch, got {shown:?}");
    };
    assert!(patch.queued);
    let [perm_op, delete_op] = patch.operations.as_slice() else {
        panic!("expected two operations, got {:?}", patch.operations);
    };
    assert_eq!(perm_op.kind, "change-perm");
    let [perm_path] = perm_op.paths.as_slice() else {
        panic!("expected one path, got {:?}", perm_op.paths);
    };
    match perm_path {
        ShowPathResolution::Unresolved { .. } => {}
        other => {
            panic!("expected Unresolved for a node the same patch goes on to delete, got {other:?}")
        }
    }
    assert_eq!(delete_op.kind, "delete-node");
    assert_eq!(
        delete_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())],
        "DeleteNode always carries its own path from the payload, never node resolution"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Handoff control 5: `show` on a queued patch derives its folded baseline read-only
/// (`CacheWrite::Never`) and must leave the rebuildable lifecycle cache untouched, even when it is
/// stale relative to the sealed tip.
///
/// Built so the cache is genuinely stale at the moment `show` runs -- unreachable through a real
/// `commit`, which is itself a `CacheWrite::Refresh` caller and would leave the cache fresh again
/// before a queued patch it created could ever be shown. Here the cache is primed at the *first*
/// sealed block directly, a second block is sealed with no reader in between (sealing alone never
/// touches this cache), and the queued patch is appended raw (see this file's own module doc), so
/// nothing refreshes the cache on the way to `show`.
#[test]
fn show_on_a_queued_patch_leaves_a_stale_lifecycle_cache_untouched() {
    let root = unique_temp_dir("show-queued-writes-nothing");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    generation(&layout, "a.txt", b"one\n", "block one");

    let cache_path = layout.cache_dir().join("lifecycle-state.v1");
    let prime_at = |block_id: ObjectId| {
        let crate::patch_replay::WorktreeBaseline::Published {
            baseline_block,
            horizon,
        } = crate::patch_replay::resolve_worktree_baseline(&layout, "heads/main").unwrap()
        else {
            panic!("heads/main is published by now");
        };
        assert_eq!(baseline_block, block_id, "the tip this call means to prime");
        let object_store = crate::object_store::ObjectReadSnapshot::open(&layout).unwrap();
        crate::lifecycle_cache::incremental::resolve_baseline_state_with(
            &layout,
            &object_store,
            baseline_block,
            horizon,
            crate::lifecycle_cache::incremental::CacheWrite::Refresh,
        )
        .unwrap();
    };
    let block_one = current_block_id(&layout);
    prime_at(block_one);
    let first_prime = std::fs::read(&cache_path).unwrap();

    // A second block, sealed with no reader call in between: sealing alone never touches this cache,
    // so it still describes block one -- now stale. (The `commit` inside `generation` asks for the
    // cached baseline itself, block one: since RFC 136 increment 2c, Addendum 2, that is a hit counted
    // as a step, so the file's step count moves by one; it still describes block one, and that is the
    // staleness this test needs. Before Addendum 2 the same commit replayed in full and rewrote the same
    // bytes, which is why this used to compare equal.)
    generation(&layout, "b.txt", b"two\n", "block two");
    let block_two = current_block_id(&layout);
    assert_ne!(block_one, block_two);
    let primed = std::fs::read(&cache_path).unwrap();
    assert_eq!(
        primed.len(),
        first_prime.len(),
        "the cache still holds block one's state (only its step count moved)"
    );

    let node_id = NodeId::from_bytes([0x99; 32]);
    let blob_id = write_blob(&layout, BlobKind::Text, b"three\n".to_vec());
    let queued_id = append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "c.txt".to_string(),
                node_id,
                blob_id,
                mode: 0o100_644,
            }),
        }],
    );

    let shown = show(&layout, queued_id).unwrap();
    assert_eq!(shown.len(), 1);
    assert_eq!(
        std::fs::read(&cache_path).unwrap(),
        primed,
        "show on a queued patch must not refresh the (still stale) lifecycle cache"
    );

    // The contrast that makes the equality above a real test, not a vacuous one: refreshing against
    // the now-current tip (block two) does change the cache's bytes.
    prime_at(block_two);
    assert_ne!(
        std::fs::read(&cache_path).unwrap(),
        primed,
        "precondition: refreshing against the newer tip does change the cache, so the staleness \
         above was real"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// stikk 020's own repro, as "what the round needs" item 1: patch 1 changes `a.txt`'s mode and
/// `d.txt`'s mode; patch 2 renames `a.txt` to `z.txt` and deletes `d.txt`; both sealed into one
/// block. Before the fix, patch 1's own node-addressed paths resolved against the block's *end*
/// state, so the renamed node showed `z.txt` (a name patch 1 never touched under) and the deleted
/// node showed `unresolved_node_id` (removing a path prikk had given). Per the ruling, each patch
/// now resolves against its own state: patch 1's two `ChangePerm` operations must still show
/// `a.txt` and `d.txt`, matching **byte for byte** what `show` on patch 1's own queued id reported
/// before patch 2 ever existed. `ChangePerm` stands in for stikk's own `EditText` example for the
/// same reason used elsewhere in this file: same node-addressed resolution, no span-anchor
/// construction.
#[test]
fn show_block_form_resolves_rename_and_delete_cases_matching_queued_form_byte_for_byte() {
    let root = unique_temp_dir("show-stikk-020");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_a = NodeId::from_bytes([0x41; 32]);
    let node_d = NodeId::from_bytes([0x44; 32]);
    create_raw_file(&layout, node_a, "a.txt", BlobKind::Text, b"a content\n");
    let blob_d = create_raw_file(&layout, node_d, "d.txt", BlobKind::Text, b"d content\n");

    let patch1_id = append_raw_patch(
        &layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id: node_a,
                    old_mode: 0o100_644,
                    new_mode: 0o100_755,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id: node_d,
                    old_mode: 0o100_644,
                    new_mode: 0o100_755,
                }),
            },
        ],
    );

    // The queued form's own answer for patch 1, captured before patch 2 exists -- the reference
    // "what the queued form reported" byte for byte.
    let queued_patch1 = show(&layout, patch1_id).unwrap();
    let [queued_patch1] = queued_patch1.as_slice() else {
        panic!("expected exactly one patch, got {queued_patch1:?}");
    };

    let patch2_id = append_raw_patch(
        &layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id: node_a,
                    old_path: "a.txt".to_string(),
                    new_path: "z.txt".to_string(),
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "d.txt".to_string(),
                    node_id: node_d,
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: blob_d,
                        old_mode: 0o100_755,
                    },
                }),
            },
        ],
    );
    seal(&layout);

    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [sealed_patch1, sealed_patch2] = patches.as_slice() else {
        panic!("expected exactly two patches, got {patches:?}");
    };
    assert_eq!(sealed_patch1.patch_id, patch1_id);
    assert_eq!(sealed_patch2.patch_id, patch2_id);

    assert_eq!(
        sealed_patch1.operations, queued_patch1.operations,
        "the rename case and the delete case: patch 1's own operations must render identically \
         sealed as they did queued, unaffected by patch 2's later rename and delete"
    );
    let [perm_a, perm_d] = sealed_patch1.operations.as_slice() else {
        panic!(
            "expected two operations, got {:?}",
            sealed_patch1.operations
        );
    };
    assert_eq!(
        perm_a.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())],
        "the rename case: patch 1 never touched z.txt"
    );
    assert_eq!(
        perm_d.paths,
        vec![ShowPathResolution::Path("d.txt".to_string())],
        "the delete case: patch 1's own edit is not unresolved just because a later patch deletes \
         the node"
    );

    assert_fold_to_end_matches_lifecycle_state_at(&layout, block_id);

    let _ = std::fs::remove_dir_all(root);
}

/// stikk 020's "what the round needs" item 2, extended past one patch: queued equals sealed for
/// **every** patch in a block of three patches, not just the first and last. Patch 3 also proves
/// the fold runs forward correctly, not just "unaffected backward": its own `ChangePerm` on `node_a`
/// must resolve to `z.txt`, the name patch 2's rename gave it, since patch 3 comes *after* that
/// rename.
#[test]
fn show_queued_equals_sealed_for_every_patch_in_a_three_patch_block() {
    let root = unique_temp_dir("show-three-patch-block");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_a = NodeId::from_bytes([0x51; 32]);
    let node_d = NodeId::from_bytes([0x54; 32]);
    create_raw_file(&layout, node_a, "a.txt", BlobKind::Text, b"a content\n");
    let blob_d = create_raw_file(&layout, node_d, "d.txt", BlobKind::Text, b"d content\n");

    let patch1_id = append_raw_patch(
        &layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id: node_a,
                    old_mode: 0o100_644,
                    new_mode: 0o100_755,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::ChangePerm(ChangePerm {
                    node_id: node_d,
                    old_mode: 0o100_644,
                    new_mode: 0o100_755,
                }),
            },
        ],
    );
    let queued_patch1 = show(&layout, patch1_id).unwrap();
    let [queued_patch1] = queued_patch1.as_slice() else {
        panic!("expected exactly one patch, got {queued_patch1:?}");
    };

    let patch2_id = append_raw_patch(
        &layout,
        vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id: node_a,
                    old_path: "a.txt".to_string(),
                    new_path: "z.txt".to_string(),
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::DeleteNode(DeleteNode {
                    path: "d.txt".to_string(),
                    node_id: node_d,
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: blob_d,
                        old_mode: 0o100_755,
                    },
                }),
            },
        ],
    );
    let queued_patch2 = show(&layout, patch2_id).unwrap();
    let [queued_patch2] = queued_patch2.as_slice() else {
        panic!("expected exactly one patch, got {queued_patch2:?}");
    };

    let patch3_id = append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ChangePerm(ChangePerm {
                node_id: node_a,
                old_mode: 0o100_755,
                new_mode: 0o100_644,
            }),
        }],
    );
    let queued_patch3 = show(&layout, patch3_id).unwrap();
    let [queued_patch3] = queued_patch3.as_slice() else {
        panic!("expected exactly one patch, got {queued_patch3:?}");
    };
    // Precondition on the fixture, not the fold under test: patch 3 addresses `node_a` *after*
    // patch 2's own rename, so its own queued-form answer (computed by the independent
    // queued-patch-paths fold) must already show the renamed name -- otherwise the byte-for-byte
    // comparison below would pass by both sides being wrong the same way.
    let [queued_perm3] = queued_patch3.operations.as_slice() else {
        panic!("expected one operation, got {:?}", queued_patch3.operations);
    };
    assert_eq!(
        queued_perm3.paths,
        vec![ShowPathResolution::Path("z.txt".to_string())],
        "precondition: the queued form must already see patch 2's rename"
    );

    seal(&layout);
    let block_id = current_block_id(&layout);
    let patches = show(&layout, block_id).unwrap();
    let [sealed1, sealed2, sealed3] = patches.as_slice() else {
        panic!("expected exactly three patches, got {patches:?}");
    };
    assert_eq!(
        sealed1.operations, queued_patch1.operations,
        "patch 1: queued equals sealed"
    );
    assert_eq!(
        sealed2.operations, queued_patch2.operations,
        "patch 2: queued equals sealed"
    );
    assert_eq!(
        sealed3.operations, queued_patch3.operations,
        "patch 3: queued equals sealed, including seeing patch 2's rename forward"
    );

    assert_fold_to_end_matches_lifecycle_state_at(&layout, block_id);

    let _ = std::fs::remove_dir_all(root);
}

/// 0.54.0 Part 5c: the real `EditText` fields a block's own sealed patch carries, read directly
/// from the object store, independently of `show`'s own decode path -- `patch_from_envelope` and
/// `show_operation` call the identical `decode_patch_operations` this reads through, so this is not
/// a from-scratch reimplementation of decoding, only a second, independent call to it, used to check
/// `show`'s reported operation against the patch's own stored bytes rather than only against itself.
fn real_edit_text_fields(
    object_store: &crate::object_store::ObjectReadSnapshot,
    patch_id: ObjectId,
) -> (Vec<u8>, Vec<u8>) {
    let envelope =
        crate::object_store::ObjectReader::read_typed(object_store, patch_id, ObjectType::Patch)
            .unwrap()
            .unwrap();
    let operations = crate::patch_replay::decode::decode_patch_operations(
        &envelope.canonical_payload,
        envelope.schema_version,
    )
    .unwrap();
    let [operation] = operations.as_slice() else {
        panic!("expected exactly one operation in patch {patch_id}, got {operations:?}");
    };
    match &operation.kind {
        crate::patch_replay::decode::DecodedOperationKind::EditText {
            old_span_text,
            replacement_text,
            ..
        } => (old_span_text.clone(), replacement_text.clone()),
        other => panic!("expected EditText, got {other:?}"),
    }
}

/// A chain of `n` consecutive sealed `EditText`s of one file, each edit's own preimage the prior
/// edit's own output -- never separately stored as a fresh `Blob` (DC-65's by-design gap: only
/// `CreateFile` writes one) -- built for real: genesis (`CreateFile`), then one further sealed
/// block per entry in `versions` (each entry is the file's full content after that edit). Returns
/// every block id in the chain, oldest (genesis) first, so `n + 1` ids for `n` edits.
fn edit_chain(
    layout: &RepositoryLayout,
    root: &std::path::Path,
    path: &str,
    versions: &[&[u8]],
) -> Vec<ObjectId> {
    let (genesis_version, edit_versions) = versions
        .split_first()
        .expect("fixture: at least a genesis version");
    generation(layout, path, genesis_version, "genesis");
    let mut block_ids = vec![current_block_id(layout)];
    for (index, version) in edit_versions.iter().enumerate() {
        std::fs::write(root.join(path), version).unwrap();
        commit(layout, &format!("edit {index}"));
        seal(layout);
        block_ids.push(current_block_id(layout));
    }
    block_ids
}

/// Item 1: a chain of (at least) five consecutive sealed text edits of one file. Before this
/// round's fix, `show <block-id>` failed outright on every block in this chain from the edit whose
/// own preimage was itself an earlier, unstored edit's own output onward (`lifecycle replay: blob
/// … required for a state effect is missing`) -- confirmed by temporarily reverting the fix and
/// re-running this exact test (see this test's own doc below); `prikk verify` was never affected,
/// since the stored history itself was always sound (this is `show`'s own read-path bug, not a
/// store defect).
///
/// **Checkpoints suppressed** (`without_checkpoints_for_test`): without this, genesis (parentless)
/// is *always* a checkpoint (`checkpoint_due` trivially returns `true` for a `None` parent,
/// `block_state.rs`), so every block in a short chain like this one would have a replay-verified
/// snapshot within reach and `materialize_edited_text_anchored`'s anchor-first branch (RFC 136
/// increment 2c) would resolve every edit, never falling through to the plain full-replay fallback.
/// Suppressing checkpoints here means no block in this chain ever carries a snapshot, so
/// `anchored_text::anchored_texts` always returns `None` (its own walk reaches genesis, finds no
/// snapshot there either, and `derivation_parent` on a parentless root ends the walk with nothing
/// found) and every materialization in this test exercises the plain, unanchored
/// `materialize_edited_text` fallback -- the chain-across-a-checkpoint sibling test below is where
/// the anchor-first branch itself is exercised.
///
/// **Red, demonstrated by reverting the fix, not by a second code path in this file**: with
/// `apply_one_patch_with_materialize_fallback`'s call in `show.rs` reverted back to the pre-fix
/// `apply_one_patch(&object_store, *patch_id, &mut state, &mut text_cache)` (no fallback, no
/// `sealed_lineage` computed), this exact test fails at block index 2 (the second edit, zero-indexed
/// after genesis) onward with `PrikkError::Integrity` wrapping exactly
/// `lifecycle replay: blob <id> required for a state effect is missing` -- measured by hand while
/// writing this test, reported in this round's own report rather than asserted here as a second,
/// parallel "old code" implementation (there is only one `show`, and this test exercises it as
/// shipped).
#[test]
fn show_resolves_every_block_in_a_chain_of_five_consecutive_text_edits() {
    let root = unique_temp_dir("show-edit-chain");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());

    let versions: Vec<Vec<u8>> = (0..6)
        .map(|index| format!("content before marker{index} after\n").into_bytes())
        .collect();
    let version_refs: Vec<&[u8]> = versions.iter().map(Vec::as_slice).collect();

    let uses_before = crate::lifecycle_cache::anchored_text::anchored_text_uses_for_test();
    let block_ids = crate::block_state::without_checkpoints_for_test(|| {
        edit_chain(&layout, &root, "a.txt", &version_refs)
    });
    // genesis + 5 edits = 6 blocks.
    assert_eq!(block_ids.len(), 6, "fixture: 5 edits after genesis");

    let object_store = crate::object_store::ObjectReadSnapshot::open(&layout).unwrap();
    for payload in block_ids.iter().map(|&block_id| {
        let envelope = crate::object_store::ObjectReader::read_typed(
            &object_store,
            block_id,
            ObjectType::Block,
        )
        .unwrap()
        .unwrap();
        prikk_object::BlockPayload::decode_canonical(&envelope.canonical_payload).unwrap()
    }) {
        assert!(
            payload.snapshot_blob_ref.is_none(),
            "fixture precondition: checkpoints suppressed, no block in this chain carries a snapshot"
        );
    }

    // Block 0 is genesis (`CreateFile`), not an edit -- confirm it renders, then check every edit
    // block (1..=5).
    let (genesis_block_id, edit_block_ids) = block_ids
        .split_first()
        .expect("fixture: genesis plus at least one edit");
    let genesis_patches = show(&layout, *genesis_block_id).unwrap();
    let [genesis_patch] = genesis_patches.as_slice() else {
        panic!("expected exactly one patch, got {genesis_patches:?}");
    };
    let [genesis_op] = genesis_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", genesis_patch.operations);
    };
    assert_eq!(genesis_op.kind, "create-file");

    for &block_id in edit_block_ids {
        let patches = show(&layout, block_id).unwrap_or_else(|err| {
            panic!(
                "show({block_id}) must succeed -- every block in this chain is a real sealed \
                 edit, and `verify` (checked separately below) agrees the stored history is sound: \
                 {err}"
            )
        });
        let [patch] = patches.as_slice() else {
            panic!("expected exactly one patch in block {block_id}, got {patches:?}");
        };
        let [operation] = patch.operations.as_slice() else {
            panic!("expected one operation, got {:?}", patch.operations);
        };
        assert_eq!(operation.kind, "edit-text");
        let (real_old, real_replacement) = real_edit_text_fields(&object_store, patch.patch_id);
        match &operation.content {
            ShowOperationContent::EditText {
                old_span_text,
                replacement_text,
            } => {
                assert_eq!(
                    old_span_text, &real_old,
                    "block {block_id}: show's own old_span_text must match the real sealed \
                     patch's own bytes"
                );
                assert_eq!(
                    replacement_text, &real_replacement,
                    "block {block_id}: show's own replacement_text must match the real sealed \
                     patch's own bytes"
                );
            }
            other => panic!("expected EditText, got {other:?}"),
        }
        assert_eq!(
            operation.paths,
            vec![ShowPathResolution::Path("a.txt".to_string())]
        );
        assert_fold_to_end_matches_lifecycle_state_at(&layout, block_id);
    }

    // The store itself was always sound throughout -- this is a `show` read-path bug, not damage.
    let verification = crate::verify_repository(&layout).unwrap();
    assert!(
        !verification.has_stage_failure() && !verification.has_item_failure(),
        "verify must pass: the stored history in this fixture was never damaged: {verification:?}"
    );

    assert_eq!(
        crate::lifecycle_cache::anchored_text::anchored_text_uses_for_test() - uses_before,
        0,
        "fixture precondition: with checkpoints suppressed, no block carries a snapshot, so the \
         anchor-first branch (RFC 136 increment 2c) can never fire -- every materialization in this \
         test must have gone through the plain, unanchored full-replay fallback \
         (`materialize_edited_text`), which is specifically what this test demonstrates works"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// Item 1's own checkpoint-crossing sibling: a text edit, then a rename, then a text edit, across a
/// real replay-verified checkpoint (RFC 136 increment 2c). Unlike the plain chain above, checkpoints
/// are **not** suppressed here -- genesis (parentless) is always a checkpoint
/// (`checkpoint_due`/`block_state.rs` trivially returns `true` for a `None` parent), and `seal_block`
/// records it into the replay-verified set (`verified_blocks.rs`) the moment it is sealed, so by the
/// time the second edit needs to materialize its own preimage, a real, nearby, replay-verified
/// snapshot is already in reach. This is the shape where `materialize_edited_text_anchored`'s
/// anchor-first branch actually answers, not only its full-replay fallback -- confirmed directly
/// below via `anchored_text_uses_for_test`'s own counter, not assumed from the fixture's shape alone.
///
/// The rename in the middle exercises the anchor-carry's own node-identity tracking
/// (`materialize_wanted_text_forward`): the wanted node's text is carried forward by node id through
/// every block after the anchor, a `RenamePath` among them, which has no content effect and is
/// simply passed over -- the same "tracked by node identity rather than path" property `extract`'s own
/// anchor-first read already relies on (0.54.0 step 3's own CHANGELOG entry).
#[test]
fn show_resolves_a_text_edit_across_a_replay_verified_checkpoint_and_a_rename() {
    let root = unique_temp_dir("show-edit-chain-checkpoint");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());

    let node_id = NodeId::from_bytes([0x5A; 32]);
    create_raw_file(&layout, node_id, "a.txt", BlobKind::Text, b"version zero\n");
    let genesis_block_id = current_block_id(&layout);

    // Fixture precondition, not the mechanism under test: genesis really is a recorded checkpoint.
    let object_store = crate::object_store::ObjectReadSnapshot::open(&layout).unwrap();
    let genesis_envelope = crate::object_store::ObjectReader::read_typed(
        &object_store,
        genesis_block_id,
        ObjectType::Block,
    )
    .unwrap()
    .unwrap();
    let genesis_payload =
        prikk_object::BlockPayload::decode_canonical(&genesis_envelope.canonical_payload).unwrap();
    assert!(
        genesis_payload.snapshot_blob_ref.is_some(),
        "fixture precondition: genesis (parentless) is always a checkpoint"
    );
    assert!(
        crate::verified_blocks::load_verified_blocks(&layout).contains(&genesis_block_id),
        "fixture precondition: sealing genesis records it into the replay-verified record"
    );

    // Edit 1: preimage is genesis's own real, stored blob -- resolvable with or without this
    // round's fix, no materialization needed.
    std::fs::write(root.join("a.txt"), b"version one\n").unwrap();
    commit(&layout, "edit 1");
    seal(&layout);
    let edit1_block_id = current_block_id(&layout);

    // Rename: no content change. `RenamePath` is not reachable through ordinary `commit` (this
    // file's own module doc) -- raw-appended, with the real worktree file renamed to match so a
    // later ordinary commit does not read this as a delete-then-create.
    std::fs::rename(root.join("a.txt"), root.join("b.txt")).unwrap();
    append_raw_patch(
        &layout,
        vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id,
                old_path: "a.txt".to_string(),
                new_path: "b.txt".to_string(),
            }),
        }],
    );
    seal(&layout);
    let rename_block_id = current_block_id(&layout);

    // Edit 2, at the renamed path: preimage is edit 1's own output -- never separately stored
    // (DC-65), and this is the one that needs materialization.
    let uses_before = crate::lifecycle_cache::anchored_text::anchored_text_uses_for_test();
    std::fs::write(root.join("b.txt"), b"version two\n").unwrap();
    commit(&layout, "edit 2");
    seal(&layout);
    let edit2_block_id = current_block_id(&layout);
    // Fresh snapshot: `object_store` above was opened before edit 1/the rename/edit 2 were sealed,
    // and `ObjectReadSnapshot::open` is a point-in-time index snapshot -- it would not see any of
    // their own objects.
    let object_store = crate::object_store::ObjectReadSnapshot::open(&layout).unwrap();

    let patches1 = show(&layout, edit1_block_id).unwrap();
    let [edit1_patch] = patches1.as_slice() else {
        panic!("expected exactly one patch, got {patches1:?}");
    };
    let [edit1_op] = edit1_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", edit1_patch.operations);
    };
    assert_eq!(edit1_op.kind, "edit-text");
    assert_eq!(
        edit1_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())]
    );

    let patches_rename = show(&layout, rename_block_id).unwrap();
    let [rename_patch] = patches_rename.as_slice() else {
        panic!("expected exactly one patch, got {patches_rename:?}");
    };
    let [rename_op] = rename_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", rename_patch.operations);
    };
    assert_eq!(rename_op.kind, "rename-path");
    assert_eq!(
        rename_op.paths,
        vec![
            ShowPathResolution::Path("a.txt".to_string()),
            ShowPathResolution::Path("b.txt".to_string()),
        ]
    );

    let patches2 = show(&layout, edit2_block_id).unwrap_or_else(|err| {
        panic!(
            "show({edit2_block_id}) must succeed -- its own EditText's preimage is edit 1's \
             unstored output, materializable from genesis's own replay-verified checkpoint, \
             carried forward through edit 1 and the rename: {err}"
        )
    });
    let [edit2_patch] = patches2.as_slice() else {
        panic!("expected exactly one patch, got {patches2:?}");
    };
    let [edit2_op] = edit2_patch.operations.as_slice() else {
        panic!("expected one operation, got {:?}", edit2_patch.operations);
    };
    assert_eq!(edit2_op.kind, "edit-text");
    assert_eq!(
        edit2_op.paths,
        vec![ShowPathResolution::Path("b.txt".to_string())],
        "edit 2 resolves at its own, post-rename path"
    );
    let (real_old, real_replacement) = real_edit_text_fields(&object_store, edit2_patch.patch_id);
    match &edit2_op.content {
        ShowOperationContent::EditText {
            old_span_text,
            replacement_text,
        } => {
            assert_eq!(old_span_text, &real_old);
            assert_eq!(replacement_text, &real_replacement);
        }
        other => panic!("expected EditText, got {other:?}"),
    }

    assert_fold_to_end_matches_lifecycle_state_at(&layout, edit1_block_id);
    assert_fold_to_end_matches_lifecycle_state_at(&layout, rename_block_id);
    assert_fold_to_end_matches_lifecycle_state_at(&layout, edit2_block_id);

    assert!(
        crate::lifecycle_cache::anchored_text::anchored_text_uses_for_test() > uses_before,
        "edit 2's own materialization must have gone through the anchor-first branch -- genesis \
         is a real, nearby, replay-verified checkpoint, not only the full-replay fallback"
    );

    let verification = crate::verify_repository(&layout).unwrap();
    assert!(
        !verification.has_stage_failure() && !verification.has_item_failure(),
        "verify must pass: the stored history in this fixture was never damaged: {verification:?}"
    );

    let _ = std::fs::remove_dir_all(root);
}
