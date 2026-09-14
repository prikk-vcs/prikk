//! RFC 136 handoff v2, increment 0: every path that seals a Block yields the same block ids after
//! sealing became one function as before it.
//!
//! One history exercises all four callers -- the real `prikk seal` binary, `branch create`, the RFC
//! 111 seal simulation, `sync seal --claim` (`seal_from_accepted_claim`) and `merge`
//! (`execute_merge`) -- with fixed signer seeds and fixed NodeIds, so every object id is determined
//! by content alone. Increment 0 pinned the ids recorded on the tree *before* the refactor; RFC 136
//! increment 1b re-pinned them when the root block became a checkpoint, keeping the old ones beside
//! the new. They are pinned as ids, not counts: a changed payload field, field order, kind, parent set
//! or signature on any path changes one of them.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;
use std::process::Output;

use prikk_object::{
    BlobKind, BlobPayload, BlockPayload, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope,
    ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
    RecognitionClaimPayload, RefStatePayload,
};
use prikk_store::{
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore,
    ObjectReader, ObjectWriter, RefStore, RepositoryLayout, SealFromAcceptedOutcome, Wal,
    add_trusted_maintainer, author_signature, execute_merge, maintainer_signature,
    seal_from_accepted_claim, simulate_one_seal_for_test_support, write_active_ref_metadata,
};

mod support;

const MAIN: &str = "heads/main";
const FEATURE: &str = "heads/feature";

/// Pinned at RFC 136 increment 1b: `[prikk seal, RFC 111 simulation, sync seal --claim, merge]`. The
/// root block is a checkpoint (it has no snapshotted ancestor), so it carries a snapshot; every later
/// id follows its parent's.
const PINNED_BLOCK_IDS: [&str; 4] = [
    "10072f3a5ac26f44409db3a3392e6dc33e66971b3232f817bc1449ada771a29e",
    "07755db1486f1236b8620b0acdb193bb52c659849df900e29e1c05d74e29d38c",
    "cf381a995b245babe761e6ba7359aeb6ded903c397ffa46ec4ae2633d89e848a",
    "dd5f8312d9baf5cb20d06b3cdda3248fff60a7207fd757f43afbab36fa1dff86",
];

/// Which of those four blocks carry a snapshot: the root only. The merge's mainline is two blocks
/// deep and `sync seal --claim` sealed on a one-block branch, so neither reaches the cadence.
const CARRIES_SNAPSHOT: [bool; 4] = [true, false, false, false];

/// Increment 0's pins, recorded on the tree before `seal_block` existed. Forcing `checkpoint_due` to
/// `false` must restore exactly these -- the proof that increment 1b changed a block's payload in the
/// snapshot field and nowhere else.
#[allow(dead_code)]
const INCREMENT_0_BLOCK_IDS: [&str; 4] = [
    "bef81e32e0cdad8e11eb68c44102071704fd0438f8e86fb706d3df213aca0bac",
    "2298a33fa92560c590394e4b462e849b48225711e7c9b16c786b5296595bf380",
    "7a0ede9d89c3011362e2aca8ffdf678ea1364a2a7d37cf73d8d5b2bd6532ca32",
    "afd252c17b1ecfdaae196cdc9eda1602cb253deb4d6b2de9aea65a34bb78a603",
];

fn tip(layout: &RepositoryLayout, ref_name: &str) -> ObjectId {
    let ref_state_id = RefStore::new(layout.clone())
        .read_current_ref_state_id(ref_name)
        .expect("read current ref state id")
        .expect("ref has a published RefState");
    let envelope = FileObjectStore::new(layout.clone())
        .read_typed(ref_state_id, ObjectType::RefState)
        .expect("read RefState envelope")
        .expect("RefState object exists");
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .expect("decode RefState payload")
        .target_object_id
}

fn write_blob(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    content: &[u8],
) -> ObjectId {
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes().unwrap());
    let id = envelope.object_id();
    envelope
        .add_signature(maintainer_signature(signer, ObjectType::Blob, id).unwrap())
        .unwrap();
    objects.write_object(&envelope).unwrap()
}

/// An author-signed `CreateFile` patch with a fixed NodeId, so its id does not depend on the CLI's
/// OS-random NodeId minting.
fn create_file_patch(
    signer: &Ed25519AuthorSigner,
    path: &str,
    node_seed: u8,
    blob_id: ObjectId,
) -> ObjectEnvelope {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_seed; 32]),
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
    let id = envelope.object_id();
    envelope
        .add_signature(author_signature(signer, id).unwrap())
        .unwrap();
    envelope
}

fn write_claim(
    objects: &mut FileObjectStore,
    signer: &Ed25519MaintainerSigner,
    patch_ids: Vec<ObjectId>,
) -> ObjectId {
    let payload = RecognitionClaimPayload {
        block_id: ObjectId::from_bytes([0xAA; 32]),
        patch_ids,
        parent_block_ids: Vec::new(),
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes().unwrap(),
    );
    let id = envelope.object_id();
    envelope
        .add_signature(maintainer_signature(signer, ObjectType::RecognitionClaim, id).unwrap())
        .unwrap();
    objects.write_object(&envelope).unwrap()
}

fn carries_snapshot(layout: &RepositoryLayout, block_id: ObjectId) -> bool {
    let envelope = FileObjectStore::new(layout.clone())
        .read_typed(block_id, ObjectType::Block)
        .expect("read Block")
        .expect("Block exists");
    BlockPayload::decode_canonical(&envelope.canonical_payload)
        .expect("decode Block")
        .snapshot_blob_ref
        .is_some()
}

fn branch_create(repo: &Path, name: &str, from: &str) -> Output {
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["branch", "create", name, "--from", from])
        .output()
        .unwrap()
}

#[test]
fn every_seal_path_yields_its_pinned_block_id() {
    let root = support::unique_repo("rfc136-seal-block-ids");
    support::init(&root);
    let layout = RepositoryLayout::open(root.clone()).expect("open repo");
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    let author = Ed25519AuthorSigner::from_seed("rfc136-seal-block-author", &[0x36; 32])
        .expect("fixed author seed derives a valid signer");
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .expect("trust maintainer");
    let mut objects = FileObjectStore::new(layout.clone());
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);

    // 1. The real `prikk seal` binary: a Root block creating a.txt.
    let a_blob = write_blob(&mut objects, &maintainer, b"a\n");
    wal.append_patch(&create_file_patch(&author, "a.txt", 0xA1, a_blob))
        .expect("append a.txt patch");
    write_active_ref_metadata(&layout, MAIN).expect("own the active WAL for main");
    support::ok(&support::seal(&root, MAIN), "prikk seal");
    let sealed = tip(&layout, MAIN);

    support::ok(&branch_create(&root, FEATURE, MAIN), "branch create");

    // 2. The RFC 111 simulation: a Normal block on main creating b.txt.
    let b_blob = write_blob(&mut objects, &maintainer, b"b\n");
    wal.append_patch(&create_file_patch(&author, "b.txt", 0xB2, b_blob))
        .expect("append b.txt patch");
    write_active_ref_metadata(&layout, MAIN).expect("own the active WAL for main");
    simulate_one_seal_for_test_support(&layout, MAIN, &maintainer).expect("simulated seal");
    let simulated = tip(&layout, MAIN);

    // 3. `sync seal --claim`: a Normal block on feature creating c.txt from an accepted claim.
    let c_blob = write_blob(&mut objects, &maintainer, b"c\n");
    let c_patch = objects
        .write_object(&create_file_patch(&author, "c.txt", 0xC3, c_blob))
        .expect("write c.txt patch");
    let claim_id = write_claim(&mut objects, &maintainer, vec![c_patch]);
    let outcome =
        seal_from_accepted_claim(&layout, FEATURE, claim_id, &maintainer).expect("seal claim");
    assert!(
        matches!(outcome, SealFromAcceptedOutcome::Sealed { .. }),
        "the claim must seal: {outcome:?}"
    );
    let accepted = tip(&layout, FEATURE);

    // 4. `merge`: a Merge block on main adopting feature's c.txt from baseline `sealed`.
    let report = execute_merge(&layout, sealed, MAIN, FEATURE, &maintainer).expect("merge");
    assert_eq!(report.block_id, tip(&layout, MAIN));
    let merged = report.block_id;

    support::ok(&support::verify(&root), "verify");

    let blocks = [sealed, simulated, accepted, merged];
    assert_eq!(
        blocks
            .map(|id| id.to_string())
            .each_ref()
            .map(String::as_str),
        PINNED_BLOCK_IDS,
        "a seal path's block id changed: [prikk seal, RFC 111 simulation, sync seal --claim, merge]"
    );
    assert_eq!(
        blocks.map(|id| carries_snapshot(&layout, id)),
        CARRIES_SNAPSHOT,
        "which blocks are checkpoints: [prikk seal, RFC 111 simulation, sync seal --claim, merge]"
    );
}
