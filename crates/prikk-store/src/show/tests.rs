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
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::object_store::{ObjectWriteSession, ObjectWriter};
use crate::refs::RefStore;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::worktree_patch::commit_worktree_changes_signed;
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

/// Control 3: an unresolved node id does not fail the command. A node-addressed operation
/// (`ChangePerm`) and a deletion target the same node in one block; every operation reports, the
/// mode change's own node id is unresolved (deleted by the same block's own later patch).
///
/// Not `EditText` (the handoff's own example): editing a text node before deleting it hits a
/// real, separate gap this round found rather than caused -- `worktree_patch::node_authoring
/// ::plan_delete` carries the *pre-edit* baseline `blob_id` into `DeleteNode`'s preimage
/// unconditionally, and DC-65's own doc says a text node's baseline `blob_id` becomes a content
/// identity (never a stored object) once its most recent operation was an `EditText`; `plan_delete`
/// never calls the materialization fallback `plan_edit_text` already uses for exactly this case.
/// Reading that preimage's blob -- exactly what showing a deleted file's old content requires --
/// then fails with "missing Blob". Filed in the round's own report rather than routed around
/// silently; `ChangePerm` reaches the same "node-addressed op, deleted later" shape without
/// touching a blob at all, so it exercises this control cleanly.
#[test]
fn show_reports_unresolved_when_a_block_changes_and_deletes_the_same_node() {
    let root = unique_temp_dir("show-unresolved");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    trust_maintainer(&layout, &maintainer_signer());
    let node_id = NodeId::from_bytes([0x9A; 32]);
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
    let [change_perm_op, delete_op] = patch.operations.as_slice() else {
        panic!(
            "expected exactly two operations, got {:?}",
            patch.operations
        );
    };
    assert_eq!(change_perm_op.kind, "change-perm");
    let [change_perm_path] = change_perm_op.paths.as_slice() else {
        panic!("expected one path, got {:?}", change_perm_op.paths);
    };
    match change_perm_path {
        ShowPathResolution::Unresolved { .. } => {}
        other => panic!("expected Unresolved for the changed-then-deleted node, got {other:?}"),
    }
    assert_eq!(delete_op.kind, "delete-node");
    assert_eq!(
        delete_op.paths,
        vec![ShowPathResolution::Path("a.txt".to_string())]
    );

    let _ = std::fs::remove_dir_all(root);
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
            old_blob_id,
            old_size: 10,
            new_blob_id,
            new_size: 20,
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
