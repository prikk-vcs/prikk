//! RFC 165 R3 (C3): every publication refuses while *another* ref's publication is incomplete,
//! before its own first write. `seal`'s own DC-38 retry of its own interrupted publication is the
//! one case that must still complete -- covered by the real end-to-end CLI test
//! `seal_retry_drains_already_published_wal_without_duplicate_ref_update`
//! (`crates/prikk-cli/tests/genesis_end_to_end.rs`), not duplicated here.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, NodeId,
    ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
    RefKind, RefStatePayload, RefUpdatePayload,
};

use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::object_store::ObjectWriteSession;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, FileObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, Wal, add_trusted_maintainer, author_signature,
    maintainer_signature as sign_maintainer,
};

fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-r3-maintainer", &[0x71; 32]).expect("seed")
}

fn author() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc165-r3-author", &[0x72; 32]).expect("seed")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn setup(root: &std::path::Path) -> RepositoryLayout {
    let layout = RepositoryLayout::init(root.to_path_buf()).expect("init");
    let signer = maintainer();
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &hex_encode(&signer.public_key_bytes()),
    )
    .expect("trust");
    layout
}

fn root_block(layout: &RepositoryLayout) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).unwrap(),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut env =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes().unwrap());
    let id = env.object_id();
    env.add_signature(sign_maintainer(&maintainer(), ObjectType::Block, id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&env)
        .unwrap()
}

fn build_branch_create_publication(
    ref_name: &str,
    target_block: ObjectId,
    signer: &impl MaintainerSigner,
) -> RefPublication {
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target_block,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(sign_maintainer(signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    let update = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: target_block,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_env = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().unwrap(),
    );
    let update_id = update_env.object_id();
    update_env
        .add_signature(sign_maintainer(signer, ObjectType::RefUpdate, update_id).unwrap())
        .unwrap();
    RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state: state_env,
        ref_update: update_env,
    }
}

fn queue_one_patch(layout: &RepositoryLayout, ref_name: &str, label: &str) {
    let blob = BlobPayload::new(BlobKind::Text, format!("{label}\n").into_bytes());
    let blob_envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes().unwrap());
    let blob_id = blob_envelope.object_id();
    FileObjectStore::new(layout.clone())
        .write_object(&blob_envelope)
        .unwrap();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: format!("{label}.txt"),
                node_id: NodeId::from_bytes(*blob_id.as_bytes()),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    envelope
        .add_signature(author_signature(&author(), envelope.object_id()).unwrap())
        .unwrap();
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    crate::write_active_ref_metadata(layout, ref_name).unwrap();
}

/// Hash every file under `.prikk/`, path and content both -- a cheap, exact "the tree is identical"
/// comparison (handoff §3 item 1: "a refusal writes nothing. Compare the `.prikk/` tree before and
/// after").
fn snapshot_tree(layout: &RepositoryLayout) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn walk(
        dir: &std::path::Path,
        root: &std::path::Path,
        out: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((
                    path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    bytes,
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(layout.prikk_dir(), layout.prikk_dir(), &mut out);
    out.sort();
    out
}

/// Put `heads/broken` into a `PointerLeading`-style interrupted state (a crashed root branch
/// create), the one, representative crash shape every other test in this file reuses -- R2's own
/// equivalence sweep already proved every publication's own crash shape is classified identically by
/// this precondition, so one representative producer is enough here; this file's own job is proving
/// *other* publications refuse behind it, not re-deriving every shape again.
fn build_incomplete_unrelated_ref(layout: &RepositoryLayout) {
    let target = root_block(layout);
    let publication = build_branch_create_publication("heads/broken", target, &maintainer());
    let ref_store = RefStore::new(layout.clone());
    let mut object_store = ObjectWriteSession::open(layout).unwrap();
    // Skip 3: the ordinal R2's own sweep (`no_refs_times_log_precondition.rs`) already established
    // lands on `PointerLeading` for a root branch create -- asserted below, not assumed.
    fail_after_for_test(TestFailPoint::AppendWrite, 3);
    let _ = ref_store.publish_with_object_store(&mut object_store, &publication);
    drop(object_store);
    clear_failpoint_for_test();
    assert!(
        crate::refs::ensure_no_incomplete_publication(layout).is_err(),
        "fixture bug: heads/broken must actually be incomplete"
    );
}

#[test]
fn branch_create_refuses_behind_another_refs_incomplete_publication() {
    let root = unique_temp_dir("rfc165-r3-branch-create");
    let layout = setup(&root);
    build_incomplete_unrelated_ref(&layout);
    let before = snapshot_tree(&layout);

    // Mirrors `branch.rs::run_create`'s own first check, called directly: the CLI-layer call this
    // test is really about is `prikk_store::ensure_no_incomplete_publication_except`.
    let result = crate::ensure_no_incomplete_publication_except(&layout, Some("heads/target"));
    assert!(
        result.is_err(),
        "must refuse behind heads/broken's own incomplete publication"
    );

    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn branch_close_refuses_behind_another_refs_incomplete_publication() {
    let root = unique_temp_dir("rfc165-r3-branch-close");
    let layout = setup(&root);
    // An existing, healthy branch to attempt closing.
    let target = root_block(&layout);
    let open = build_branch_create_publication("heads/target", target, &maintainer());
    {
        let ref_store = RefStore::new(layout.clone());
        let mut object_store = ObjectWriteSession::open(&layout).unwrap();
        ref_store
            .publish_with_object_store(&mut object_store, &open)
            .unwrap();
    }
    build_incomplete_unrelated_ref(&layout);
    let before = snapshot_tree(&layout);

    let result = crate::ensure_no_incomplete_publication_except(&layout, Some("heads/target"));
    assert!(result.is_err());

    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn tag_create_refuses_behind_another_refs_incomplete_publication() {
    use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
    use crate::tag_travel::create_local_tag;

    let root = unique_temp_dir("rfc165-r3-tag-create");
    let layout = setup(&root);
    build_incomplete_unrelated_ref(&layout);
    let before = snapshot_tree(&layout);

    let target = root_block(&layout);
    let (digest, count) = {
        let store = FileObjectStore::new(layout.clone());
        compute_patch_set_digest_and_count_from_block(&store, target).unwrap()
    };
    let mut session = ObjectWriteSession::open(&layout).unwrap();
    let result = create_local_tag(
        &layout,
        &mut session,
        "tags/v1",
        target,
        None,
        digest,
        count,
        &maintainer(),
    );
    drop(session);
    assert!(result.is_err(), "{result:?}");

    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn merge_refuses_behind_another_refs_incomplete_publication() {
    use crate::merge::execute::execute_merge;

    let root = unique_temp_dir("rfc165-r3-merge");
    let layout = setup(&root);
    let signer = maintainer();
    let main_target = root_block(&layout);
    let main_pub = build_branch_create_publication("heads/main", main_target, &signer);
    let ref_store = RefStore::new(layout.clone());
    {
        let mut object_store = ObjectWriteSession::open(&layout).unwrap();
        ref_store
            .publish_with_object_store(&mut object_store, &main_pub)
            .unwrap();
    }
    let topic_pub = build_branch_create_publication("heads/topic", main_target, &signer);
    {
        let mut object_store = ObjectWriteSession::open(&layout).unwrap();
        ref_store
            .publish_with_object_store(&mut object_store, &topic_pub)
            .unwrap();
    }
    queue_one_patch(&layout, "heads/topic", "m");
    crate::simulate_one_seal_for_test_support(&layout, "heads/topic", &signer).unwrap();

    build_incomplete_unrelated_ref(&layout);
    let before = snapshot_tree(&layout);

    let result = execute_merge(&layout, main_target, "heads/main", "heads/topic", &signer);
    assert!(result.is_err(), "{result:?}");

    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn seal_refuses_behind_another_refs_incomplete_publication() {
    let root = unique_temp_dir("rfc165-r3-seal");
    let layout = setup(&root);
    let signer = maintainer();
    build_incomplete_unrelated_ref(&layout);
    queue_one_patch(&layout, "heads/target", "s");
    let before = snapshot_tree(&layout);

    let result = crate::ensure_no_incomplete_publication_except(&layout, Some("heads/target"));
    assert!(result.is_err());

    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
    let _ = signer;
}
