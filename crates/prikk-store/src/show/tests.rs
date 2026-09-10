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
fn append_raw_patch(layout: &RepositoryLayout, operations: Vec<Operation>) {
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
/// sequence: create `a.txt`, seal; edit it, seal; delete it, seal. Every operation still renders,
/// the edit's own node id is unresolved (deleted later in the same block), and the delete's own
/// preimage content degrades to [`ShowBlobContent::Unavailable`] (§6a) rather than failing the
/// command -- exit `0`, not `1`.
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
    let [edit_path] = edit_op.paths.as_slice() else {
        panic!("expected one path, got {:?}", edit_op.paths);
    };
    match edit_path {
        ShowPathResolution::Unresolved { .. } => {}
        other => panic!("expected Unresolved for the edited-then-deleted node, got {other:?}"),
    }

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
            author_key_id: "show-author".to_string()
        }
    );

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
