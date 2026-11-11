//! 0.57.0 U3b, through the compiled binary: `verify` judges a sealed block's **integrity**, and reports a collision under this
//! version's rule as a **warning**; the sites that write the disk keep refusing.
//!
//! The fixture is a real colliding history, written with `prikk-store`'s public API the way `u4_unreached_outside_version_rule`
//! writes its blocks (no `seal` can author it): a block over `genesis` whose patch creates `Straße.txt` and `STRASSE.txt`, with
//! the state root its patch derives, reached by a received ref. Both names are valid on their own; the collision rule (U3) makes
//! them one name. Under the old behaviour `verify` re-derived the state root through that rule and failed the block.
//!
//! | what | result |
//! |---|---|
//! | `verify` | **exit 0**, one `warning:` naming both paths (escaped), `extract`/`checkout`/`branch switch`, and `prikk mv` |
//! | `verify --format json` | exit 0, an additive `sealed_path_collisions` entry |
//! | `doctor` | exit 0, `PRIKK-DOCTOR-SEALED-PATH-COLLISION` (warning) |
//! | `extract --ref remotes/heads/side` | **refuses** (`case-collision`), nothing written |
//! | a block with a real state-root mismatch | still **fails** (only the policy is a warning) |

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use super::u4_unreached_outside_version_rule::{
    fresh, text, verify, write_create_file_patch_with_blob, write_ref_state,
};
use crate::support;
use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId,
    ObjectType,
};
use prikk_store::{
    FileObjectStore, ObjectWriter, RepositoryLayout, derive_next_state_root, maintainer_signature,
};

fn maintainer() -> prikk_store::Ed25519MaintainerSigner {
    super::u4_unreached_outside_version_rule::maintainer()
}

/// A blob the patches can name (the state derivation needs it present).
fn blob(store: &mut FileObjectStore, bytes: &[u8]) -> ObjectId {
    let payload = BlobPayload::new(BlobKind::Text, bytes.to_vec());
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes().unwrap());
    let id = envelope.object_id();
    envelope
        .add_signature(maintainer_signature(&maintainer(), ObjectType::Blob, id).unwrap())
        .unwrap();
    store.write_object(&envelope).unwrap()
}

/// A maintainer-signed block over `parent` carrying `patch_id`, claiming `claimed_root`.
fn block_claiming(
    store: &mut FileObjectStore,
    parent: ObjectId,
    patch_id: ObjectId,
    claimed_root: prikk_object::MerkleRoot,
) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: vec![parent],
        kind: BlockKind::Normal,
        patch_ids: vec![patch_id],
        state_merkle_root: claimed_root,
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

/// A repository with a received ref whose tip creates `left` and `right`, with the **true** state root of that.
fn colliding_tip(tag: &str, left: &str, right: &str) -> (PathBuf, ObjectId) {
    colliding_tip_reached(tag, left, right, true)
}

/// [`colliding_tip`], optionally left **unreached** (no ref points at it).
fn colliding_tip_reached(tag: &str, left: &str, right: &str, reached: bool) -> (PathBuf, ObjectId) {
    let (repo, mut store, genesis) = fresh(tag);
    let left_blob = blob(&mut store, b"left\n");
    let right_blob = blob(&mut store, b"right\n");
    let first = write_create_file_patch_with_blob(&mut store, left, 0x31, left_blob);
    let second = write_create_file_patch_with_blob(&mut store, right, 0x41, right_blob);
    // One patch creating both would need both operations in one payload; two patches in two blocks keep the fixture on the
    // helper that exists. The tip holds both names.
    let root_one = derive_next_state_root(&store, Some(genesis), &[first]).unwrap();
    let one = block_claiming(&mut store, genesis, first, root_one);
    let root_two = derive_next_state_root(&store, Some(one), &[second]).unwrap();
    let tip = block_claiming(&mut store, one, second, root_two);
    let state = write_ref_state(&mut store, "heads/side", tip);
    if reached {
        prikk_store::write_received_pointer_for_test(
            &RepositoryLayout::open(&repo).unwrap(),
            "remotes/heads/side",
            state,
        )
        .unwrap();
    }
    (repo, tip)
}

fn run(repo: &Path, args: &[&str]) -> std::process::Output {
    support::prikk(repo).args(args).output().unwrap()
}

const LEFT: &str = "Stra\u{df}e.txt";
const RIGHT: &str = "STRASSE.txt";

#[test]
fn a_sealed_tip_whose_paths_collide_is_one_warning_in_verify_and_doctor_and_not_a_failure() {
    let (repo, tip) = colliding_tip("u3b-collision", LEFT, RIGHT);

    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "valid history is not a failure:\n{said}"
    );
    assert_eq!(
        said.matches("warning: the sealed tip block ").count(),
        1,
        "one warning:\n{said}"
    );
    assert!(
        said.contains(&format!("the sealed tip block {tip}")),
        "{said}"
    );
    assert!(
        said.contains("Stra\\u{df}e.txt") && said.contains("STRASSE.txt"),
        "both paths, the non-ASCII one escaped: {said}"
    );
    assert!(
        said.contains("`extract`, `checkout` and `branch switch` will refuse to write both"),
        "{said}"
    );
    assert!(
        said.contains("prikk mv ") && said.contains("then `commit` and `seal`"),
        "{said}"
    );
    assert!(!said.contains("state-root failed"), "{said}");

    let json = run(&repo, &["verify", "--format", "json"]);
    assert_eq!(json.status.code(), Some(0), "{}", text(&json));
    let json = String::from_utf8_lossy(&json.stdout).to_string();
    assert!(
        json.contains("\"sealed_path_collisions\": [\n    {\"block_id\": \""),
        "{json}"
    );
    assert!(json.contains("\"blocks\": 1}"), "{json}");

    let doctor = run(&repo, &["doctor"]);
    let said = text(&doctor);
    assert_eq!(doctor.status.code(), Some(0), "{said}");
    assert!(
        said.contains("warning [PRIKK-DOCTOR-SEALED-PATH-COLLISION]"),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_writers_keep_refusing_the_same_state() {
    let (repo, _tip) = colliding_tip("u3b-extract", LEFT, RIGHT);
    let destination = repo.join("out");
    let output = run(
        &repo,
        &[
            "extract",
            "--ref",
            "remotes/heads/side",
            destination.to_str().unwrap(),
            "--format",
            "json",
        ],
    );
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(said.contains("\"refusal\": \"case-collision\""), "{said}");
    assert!(!destination.exists(), "nothing was written");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_real_state_root_mismatch_is_still_a_failure() {
    // The same colliding tip, but claiming a state root that is not the true one: only the policy became a warning.
    let (repo, mut store, genesis) = fresh("u3b-mismatch");
    let left_blob = blob(&mut store, b"left\n");
    let first = write_create_file_patch_with_blob(&mut store, LEFT, 0x31, left_blob);
    let wrong = derive_next_state_root(&store, Some(genesis), &[]).unwrap();
    let tip = block_claiming(&mut store, genesis, first, wrong);
    let state = write_ref_state(&mut store, "heads/side", tip);
    prikk_store::write_received_pointer_for_test(
        &RepositoryLayout::open(&repo).unwrap(),
        "remotes/heads/side",
        state,
    )
    .unwrap();
    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains(&format!("block {tip}: state-root failed")),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn two_names_that_do_not_collide_say_nothing() {
    let (repo, _tip) = colliding_tip("u3b-distinct", "e.txt", "\u{e9}.txt");
    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(!said.contains("sealed tip block"), "{said}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_colliding_tip_that_nothing_reaches_says_nothing() {
    // An abandoned block is not a state anyone would write or fix: no ref reaches it, so there is nothing to rename. The
    // block is still an unreached object (its own warning); the collision is not reported on top.
    let (repo, tip) = colliding_tip_reached("u3b-unreached", LEFT, RIGHT, false);
    let output = verify(&repo);
    let said = text(&output);
    assert_eq!(output.status.code(), Some(0), "{said}");
    assert!(!said.contains("sealed tip block"), "{said}");
    assert!(
        said.contains(&format!("block {tip} is reached by nothing")),
        "{said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
