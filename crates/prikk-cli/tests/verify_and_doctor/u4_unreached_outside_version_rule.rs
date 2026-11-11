//! 0.57.0 U4 (the unreached-block fix): a block this version's path rule cannot read, that nothing reaches, is a
//! warning like every other unreached object. Before this, `verify` failed on it
//! (`PRIKK-DOCTOR-VERIFY-BLOCK-STATE-OUTSIDE-VERSION-RULE`) after `received remove` had made it unreached, and no
//! command cleared the finding (0.56.0 Security (c)).
//!
//! The fixture is the 0.55.0 hole's shape: a received ref whose chain carries a non-ASCII path. No honest `commit`
//! can author that path (0.56.0 3d2), so the Block and Patch are written with `CanonicalWriter` directly, the way
//! `step3f_claim_chain_report.rs` writes them; the received pointer is `write_received_pointer_for_test`'s. The
//! compiled binary does every read and the `received remove`.
//!
//! Case table (what `verify` says; `doctor` agrees):
//!
//! | the block | reached by | `verify` |
//! |---|---|---|
//! | outside the rule | a received ref | **fails** (unchanged: history this version cannot read) |
//! | outside the rule | nothing, after `received remove` | **passes**, a warning for the block and its patch |
//! | a child of that block | nothing | **passes**, the same warning |
//! | wrong state root (real damage) | nothing | **fails** (unchanged: only the path rule is excused) |

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use crate::support;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, CanonicalWriter, NodeId, ObjectEnvelope, ObjectId,
    ObjectType, RefKind, RefStatePayload, WireType,
};
use prikk_store::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RepositoryLayout,
    author_signature, compute_state_root, maintainer_signature,
};

pub(super) fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A `CreateFile` patch at `path`, written with `CanonicalWriter` directly (no validation, as no honest `commit`
/// can author a non-ASCII path).
pub(super) fn write_create_file_patch(
    store: &mut FileObjectStore,
    path: &str,
    node_byte: u8,
) -> ObjectId {
    let author = Ed25519AuthorSigner::from_seed("u4-author", &[0x71; 32]).unwrap();
    let mut create_file = CanonicalWriter::new();
    create_file.field_repo_path(1, path).unwrap();
    create_file
        .field_bytes(2, NodeId::from_bytes([node_byte; 32]).as_bytes())
        .unwrap();
    create_file
        .field_object_id(3, &ObjectId::from_bytes([node_byte.wrapping_add(1); 32]))
        .unwrap();
    create_file.field_u32(4, 0o100_644).unwrap();
    let create_file_bytes = create_file.finish();
    let mut operation = CanonicalWriter::new();
    operation.field_u32(1, 1).unwrap();
    operation
        .field_raw(10, WireType::Record, &create_file_bytes)
        .unwrap();
    let operation_bytes = operation.finish();
    let mut payload = CanonicalWriter::new();
    payload
        .field_raw(1, WireType::RecordListItem, &operation_bytes)
        .unwrap();
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.finish());
    let patch_id = patch.object_id();
    patch
        .add_signature(author_signature(&author, patch_id).unwrap())
        .unwrap();
    store.write_object(&patch).unwrap()
}

pub(super) fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
        .unwrap()
}

/// A maintainer-signed Block over `parent` carrying `patch_ids`, written and published nowhere.
pub(super) fn write_block(
    store: &mut FileObjectStore,
    parent: ObjectId,
    patch_ids: Vec<ObjectId>,
) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: vec![parent],
        kind: BlockKind::Normal,
        patch_ids,
        state_merkle_root: compute_state_root(&[]).unwrap(),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes().unwrap());
    let block_id = block.object_id();
    block
        .add_signature(maintainer_signature(&maintainer(), ObjectType::Block, block_id).unwrap())
        .unwrap();
    store.write_object(&block).unwrap()
}

pub(super) fn write_ref_state(
    store: &mut FileObjectStore,
    ref_name: &str,
    target: ObjectId,
) -> ObjectId {
    let payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    let id = state.object_id();
    state
        .add_signature(maintainer_signature(&maintainer(), ObjectType::RefState, id).unwrap())
        .unwrap();
    store.write_object(&state).unwrap()
}

pub(super) fn genesis_block(repo: &Path) -> ObjectId {
    let output = support::prikk(repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let marker = "\"block_id\": \"";
    let at = stdout.find(marker).expect("a block in the log") + marker.len();
    stdout[at..at + 64].parse().unwrap()
}

pub(super) fn fresh(tag: &str) -> (PathBuf, FileObjectStore, ObjectId) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "README.md", b"genesis\n", "genesis");
    let genesis = genesis_block(&repo);
    let store = FileObjectStore::new(RepositoryLayout::open(&repo).unwrap());
    (repo, store, genesis)
}

pub(super) fn verify(repo: &Path) -> std::process::Output {
    support::prikk(repo).args(["verify"]).output().unwrap()
}

pub(super) fn doctor(repo: &Path) -> std::process::Output {
    support::prikk(repo).args(["doctor"]).output().unwrap()
}

/// The chain the 0.55.0 hole admitted: a received ref `remotes/heads/side` at a block with a non-ASCII path.
pub(super) fn received_non_ascii_chain(tag: &str) -> (PathBuf, ObjectId, ObjectId) {
    let (repo, mut store, genesis) = fresh(tag);
    let patch = write_create_file_patch(&mut store, "pages/日本語.md", 0x72);
    let block = write_block(&mut store, genesis, vec![patch]);
    let state = write_ref_state(&mut store, "heads/side", block);
    prikk_store::write_received_pointer_for_test(
        &RepositoryLayout::open(&repo).unwrap(),
        "remotes/heads/side",
        state,
    )
    .unwrap();
    (repo, block, patch)
}

pub(super) fn remove_received(repo: &Path) {
    let output = support::prikk(repo)
        .args(["received", "remove", "remotes/heads/side"])
        .output()
        .unwrap();
    support::ok(&output, "received remove");
}

#[test]
fn a_block_reached_by_a_received_ref_still_fails_verify() {
    let (repo, block, _patch) = received_non_ascii_chain("u4-unreached-reached");
    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(&format!("block {block}: state-root failed")),
        "history this version cannot read, and something reaches it: still a failure\n{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn after_received_remove_the_unreached_block_is_a_warning_and_verify_passes() {
    let (repo, block, patch) = received_non_ascii_chain("u4-unreached-after-remove");
    assert_eq!(
        verify(&repo).status.code(),
        Some(1),
        "red before the removal"
    );
    remove_received(&repo);

    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "verify must pass\n{said}");
    assert!(
        said.contains(&format!("warning: block {block}: state root not verified")),
        "the block's state is reported, as a warning\n{said}"
    );
    assert!(
        said.contains("nothing reaches this block, so this is not a failure"),
        "{said}"
    );
    assert!(
        said.contains("unreached objects: 3"),
        "block, patch and ref-state\n{said}"
    );
    assert!(
        said.contains(&format!("block {block} is reached by nothing")),
        "the existing unreached warning stays\n{said}"
    );
    assert!(
        said.contains(&format!("patch {patch} is reached by nothing")),
        "{said}"
    );
    assert!(
        said.contains("block state items: 2 checked, 0 incomplete"),
        "{said}"
    );

    let output = doctor(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "doctor must pass\n{said}");
    assert!(
        said.contains("warning [PRIKK-DOCTOR-VERIFY-BLOCK-STATE-UNREACHED-OUTSIDE-VERSION-RULE]"),
        "{said}"
    );
    assert!(
        !said.contains("error [PRIKK-DOCTOR-VERIFY-BLOCK-STATE"),
        "no block-state error remains\n{said}"
    );
    assert!(
        !said.contains("way out [a block's state root failed]"),
        "{said}"
    );

    let json = support::prikk(&repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(json.status.code(), Some(0), "{}", text(&json));
    let json = String::from_utf8_lossy(&json.stdout).to_string();
    assert!(
        json.contains(&block.to_string()),
        "the JSON still lists the unreached block (a precision change must not report less)\n{json}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn an_unreached_child_of_the_unreadable_block_is_excused_too() {
    let (repo, mut store, genesis) = fresh("u4-unreached-child");
    let patch = write_create_file_patch(&mut store, "pages/日本語.md", 0x72);
    let parent = write_block(&mut store, genesis, vec![patch]);
    let child = write_block(&mut store, parent, Vec::new());
    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(
        said.contains(&format!("warning: block {child}: state root not verified")),
        "{said}"
    );
    assert!(
        said.contains(&format!(
            "its state-derivation parent {parent} is outside what this version of prikk accepts"
        )),
        "the child says why: {said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn an_unreached_block_with_a_wrong_state_root_is_still_damage() {
    let (repo, mut store, genesis) = fresh("u4-unreached-damage");
    // An ASCII path, but the block's state root is that of an empty tree: the recorded root does not verify.
    let patch = write_create_file_patch(&mut store, "pages/a.md", 0x73);
    let block = write_block(&mut store, genesis, vec![patch]);
    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(
        output.status.code(),
        Some(1),
        "only the path rule is excused, never a state root that does not verify\n{said}"
    );
    assert!(
        said.contains(&format!("block {block}: state-root failed")),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
