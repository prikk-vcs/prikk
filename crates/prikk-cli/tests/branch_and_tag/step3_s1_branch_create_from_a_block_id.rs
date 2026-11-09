//! 0.57.0 step 3, part S1: `branch create <name> --from <block id>` (stikk 029 §2, 1.1).
//!
//! The promise: yes, for a block that a local branch or tag reaches. A received-only block and an
//! unreached block are refused by name, before anything is written, because publishing either as a
//! local branch would take in history without the trust checks `archive import --adopt` makes. Each
//! case-table row is one test; the refusal tests also assert the way forward they name actually works.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use crate::support;
use prikk_object::{ObjectType, RefKind, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

/// A source repository with two sealed generations on `heads/main`; returns it and the two block ids
/// (oldest first).
fn source_with_two_blocks(tag: &str) -> (PathBuf, String, String) {
    let repo = support::unique_repo(&format!("s1-source-{tag}"));
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "f.txt", b"two\n", "two");
    let (old, new) = block_ids(&repo);
    (repo, old, new)
}

/// `(oldest, newest)` block ids of `heads/main`, and nothing else from `log --format json`.
fn block_ids(repo: &Path) -> (String, String) {
    let log = support::prikk(repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&log, "log --format json");
    let json = support::json::parse(&String::from_utf8_lossy(&log.stdout));
    let blocks = json.get("blocks").as_array();
    let newest = blocks[0].get("block_id").as_str().to_string();
    let oldest = blocks[blocks.len() - 1]
        .get("block_id")
        .as_str()
        .to_string();
    (oldest, newest)
}

fn export_archive(repo: &Path, tag: &str) -> PathBuf {
    let mut path = std::env::temp_dir();
    path.push(format!(
        "prikk-cli-s1-{tag}-{}.prepo",
        support::unique_suffix()
    ));
    support::ok(
        &support::prikk(repo)
            .args(["archive", "export"])
            .arg(&path)
            .output()
            .unwrap(),
        "archive export",
    );
    path
}

/// A fresh repository that holds the source's history only as a received ref (`remotes/heads/main`).
fn receiver_of(archive: &Path, tag: &str) -> PathBuf {
    let dest = support::unique_repo(&format!("s1-dest-{tag}"));
    support::init(&dest);
    support::trust_maintainer(&dest);
    support::ok(
        &support::prikk(&dest)
            .args(["archive", "import"])
            .arg(archive)
            .output()
            .unwrap(),
        "archive import (received landing)",
    );
    dest
}

fn text_of(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).to_string()
}

/// The RefState `ref_name` currently has, decoded.
fn ref_state_of(repo: &Path, ref_name: &str) -> RefStatePayload {
    let layout = RepositoryLayout::open(repo).unwrap();
    let ref_state_id = RefStore::new(layout.clone())
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap_or_else(|| panic!("{ref_name} is not published"));
    let envelope = FileObjectStore::new(layout)
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version).unwrap()
}

fn branch_listing(repo: &Path) -> String {
    let output = support::prikk(repo).arg("branch").output().unwrap();
    support::ok(&output, "branch");
    text_of(&output.stdout)
}

/// Row: a block reached from the current branch. The RefState written is the one a ref route writes
/// (stikk's reading of `branch.rs`: `update_seq: 1`, no previous RefState, no attestations, open),
/// and `branch switch` shows that block's files.
#[test]
fn a_block_the_current_branch_reaches_makes_a_branch_with_the_ref_routes_refstate() {
    let (repo, old, new) = source_with_two_blocks("current");

    support::ok(
        &support::branch_create(&repo, "heads/old", &old),
        "branch create --from <old block id>",
    );
    let by_id = ref_state_of(&repo, "heads/old");
    assert_eq!(by_id.target_object_id.to_string(), old);
    assert_eq!(by_id.update_seq, 1);
    assert!(by_id.previous_ref_state_id.is_none());
    assert!(by_id.required_attestation_ids.is_empty());
    assert!(!by_id.closed);
    assert_eq!(by_id.kind, RefKind::Branch);

    // The same block by a ref and by its id writes the same RefState fields (the tip is both).
    support::ok(
        &support::branch_create(&repo, "heads/by-ref", "heads/main"),
        "branch create --from heads/main",
    );
    support::ok(
        &support::branch_create(&repo, "heads/by-id", &new),
        "branch create --from <tip block id>",
    );
    let ref_route = ref_state_of(&repo, "heads/by-ref");
    let id_route = ref_state_of(&repo, "heads/by-id");
    assert_eq!(ref_route.target_object_id, id_route.target_object_id);
    assert_eq!(ref_route.update_seq, id_route.update_seq);
    assert_eq!(
        ref_route.previous_ref_state_id,
        id_route.previous_ref_state_id
    );
    assert_eq!(
        ref_route.required_attestation_ids,
        id_route.required_attestation_ids
    );
    assert_eq!(ref_route.closed, id_route.closed);
    assert_eq!(ref_route.kind, id_route.kind);

    // `branch switch` to the new branch shows the old block's files.
    support::ok(
        &support::prikk(&repo)
            .args(["branch", "switch", "heads/old"])
            .output()
            .unwrap(),
        "branch switch heads/old",
    );
    assert_eq!(
        std::fs::read_to_string(repo.join("f.txt")).unwrap(),
        "one\n",
        "the worktree holds the old block's file"
    );
}

/// Row: a block reached from another local branch (not the current one).
#[test]
fn a_block_only_another_local_branch_reaches_is_accepted() {
    let (repo, _old, new) = source_with_two_blocks("other");
    support::ok(
        &support::branch_create(&repo, "heads/side", &new),
        "branch create heads/side",
    );
    support::ok(
        &support::prikk(&repo)
            .args(["branch", "switch", "heads/side"])
            .output()
            .unwrap(),
        "branch switch heads/side",
    );
    support::generation(&repo, "heads/side", "g.txt", b"side\n", "side work");
    let (_, side_tip) = {
        let log = support::prikk(&repo)
            .args(["log", "--format", "json", "--ref", "heads/side"])
            .output()
            .unwrap();
        support::ok(&log, "log heads/side");
        let json = support::json::parse(&text_of(&log.stdout));
        let blocks = json.get("blocks").as_array();
        (
            blocks[blocks.len() - 1]
                .get("block_id")
                .as_str()
                .to_string(),
            blocks[0].get("block_id").as_str().to_string(),
        )
    };
    // The current branch is heads/side; heads/main does not reach `side_tip`. Switch back so the
    // reaching branch is not the current one.
    support::ok(
        &support::prikk(&repo)
            .args(["branch", "switch", "heads/main"])
            .output()
            .unwrap(),
        "branch switch heads/main",
    );
    support::ok(
        &support::branch_create(&repo, "heads/from-side", &side_tip),
        "branch create --from a block only heads/side reaches",
    );
    assert_eq!(
        ref_state_of(&repo, "heads/from-side")
            .target_object_id
            .to_string(),
        side_tip
    );
}

/// Row: a block reached from a tag. Here the tag is the only *local* ref that reaches it (the history
/// otherwise arrived as a received ref), so this fails if tags are not searched.
#[test]
fn a_block_only_a_tag_reaches_locally_is_accepted() {
    let (source, old, _new) = source_with_two_blocks("tag");
    let archive = export_archive(&source, "tag");
    let dest = receiver_of(&archive, "tag");

    // Held only through remotes/heads/main: refused.
    let refused = support::branch_create(&dest, "heads/viatag", &old);
    assert!(!refused.status.success(), "{refused:?}");

    support::ok(
        &support::tag_create(&dest, "tags/old", &old),
        "tag create at the held block",
    );
    support::ok(
        &support::branch_create(&dest, "heads/viatag", &old),
        "branch create --from a block a tag reaches",
    );
    assert_eq!(
        ref_state_of(&dest, "heads/viatag")
            .target_object_id
            .to_string(),
        old
    );
}

/// Row: a received-only block. Refused by name, nothing written, and the way forward it names works.
#[test]
fn a_received_only_block_is_refused_by_name_and_the_named_way_forward_works() {
    let (source, old, _new) = source_with_two_blocks("received");
    let archive = export_archive(&source, "received");
    let dest = receiver_of(&archive, "received");
    let before = support::store_bytes(&dest);

    let output = support::branch_create(&dest, "heads/rcv", &old);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = text_of(&output.stderr);
    assert!(stderr.contains("precondition not met"), "{stderr}");
    assert!(
        stderr.contains(&old),
        "the refusal names the block: {stderr}"
    );
    assert!(
        stderr.contains("not reached by any local branch or tag"),
        "{stderr}"
    );
    assert!(
        stderr.contains("remotes/heads/main"),
        "it names the received ref that holds the block: {stderr}"
    );
    assert!(
        stderr.contains("bypass the trust checks")
            && stderr.contains("prikk archive import")
            && stderr.contains("--adopt"),
        "it says why and names the way forward: {stderr}"
    );
    assert_eq!(
        before,
        support::store_bytes(&dest),
        "a refusal writes nothing"
    );
    assert!(!branch_listing(&dest).contains("heads/rcv"));

    // The way forward, typed as named: adopt the archive, then ask again.
    support::ok(
        &support::prikk(&dest)
            .args(["archive", "import"])
            .arg(&archive)
            .arg("--adopt")
            .output()
            .unwrap(),
        "archive import --adopt (the named way forward)",
    );
    support::ok(
        &support::branch_create(&dest, "heads/rcv", &old),
        "branch create after adopting",
    );
}

/// Row: an unreached block (no ref of any kind reaches it): refused by name, nothing written, and the
/// named way forward works.
#[test]
fn an_unreached_block_is_refused_by_name_and_the_named_way_forward_works() {
    let (source, old, _new) = source_with_two_blocks("unreached");
    let archive = export_archive(&source, "unreached");
    let dest = receiver_of(&archive, "unreached");
    support::ok(
        &support::prikk(&dest)
            .args(["received", "remove", "remotes/heads/main"])
            .output()
            .unwrap(),
        "received remove",
    );
    let before = support::store_bytes(&dest);

    let output = support::branch_create(&dest, "heads/un", &old);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    let stderr = text_of(&output.stderr);
    assert!(stderr.contains("precondition not met"), "{stderr}");
    assert!(stderr.contains(&old), "{stderr}");
    assert!(stderr.contains("no ref reaches it"), "{stderr}");
    assert!(
        stderr.contains("prikk archive import") && stderr.contains("--adopt"),
        "{stderr}"
    );
    assert_eq!(
        before,
        support::store_bytes(&dest),
        "a refusal writes nothing"
    );

    support::ok(
        &support::prikk(&dest)
            .args(["archive", "import"])
            .arg(&archive)
            .arg("--adopt")
            .output()
            .unwrap(),
        "archive import --adopt (the named way forward)",
    );
    support::ok(
        &support::branch_create(&dest, "heads/un", &old),
        "branch create after adopting",
    );
}

/// Rows: an id that is not a Block (a RefState, a patch) and an id that does not exist. Each refusal
/// says what the id is, and writes nothing.
#[test]
fn an_id_that_is_not_a_block_or_does_not_exist_is_refused_saying_what_it_is() {
    let (repo, _old, _new) = source_with_two_blocks("notblock");
    let log = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    let json = support::json::parse(&text_of(&log.stdout));
    let first = &json.get("blocks").as_array()[0];
    let ref_state_id = first.get("ref_state_id").as_str().to_string();
    let patch_id = first.get("patch_messages").as_array()[0]
        .get("patch_id")
        .as_str()
        .to_string();
    let absent = "0".repeat(64);
    let before = support::store_bytes(&repo);

    for (id, expected) in [
        (&ref_state_id, "is a ref-state, not a block"),
        (&patch_id, "is a patch, not a block"),
        (&absent, "is not in this repository"),
    ] {
        let output = support::branch_create(&repo, "heads/x", id);
        assert_eq!(output.status.code(), Some(1), "{id}: {output:?}");
        let stderr = text_of(&output.stderr);
        assert!(stderr.contains("precondition not met"), "{id}: {stderr}");
        assert!(stderr.contains(expected), "{id}: {stderr}");
    }
    assert_eq!(
        before,
        support::store_bytes(&repo),
        "a refusal writes nothing"
    );
}

/// Row: `--from` a ref is as it was (and a spelling that is not a 64-hex id is still a ref name).
#[test]
fn from_a_ref_is_as_before() {
    let (repo, _old, new) = source_with_two_blocks("ref");
    support::ok(
        &support::branch_create(&repo, "heads/copy", "heads/main"),
        "branch create --from heads/main",
    );
    assert_eq!(
        ref_state_of(&repo, "heads/copy")
            .target_object_id
            .to_string(),
        new
    );
    let short = support::branch_create(&repo, "heads/short", &new[..20]);
    assert_eq!(short.status.code(), Some(1), "{short:?}");
    assert!(
        text_of(&short.stderr).contains("does not resolve to a published ref"),
        "{}",
        text_of(&short.stderr)
    );
}

/// The in-the-way check for the unborn current branch is made on the block route too, before the
/// publish: a file in the way refuses with `heads/main` still unpublished; with the way clear the
/// branch is created and the block's files are written.
#[test]
fn onto_the_unborn_current_branch_a_file_in_the_way_refuses_before_publishing() {
    let (source, old, _new) = source_with_two_blocks("inway");
    let archive = export_archive(&source, "inway");
    let dest = receiver_of(&archive, "inway");
    support::ok(
        &support::tag_create(&dest, "tags/old", &old),
        "tag create at the held block",
    );

    // A different file already sits where the block's `f.txt` would be written.
    std::fs::write(dest.join("f.txt"), "mine, not the block's\n").unwrap();
    let output = support::branch_create(&dest, "heads/main", &old);
    assert_eq!(output.status.code(), Some(1), "{output:?}");
    assert!(
        !branch_listing(&dest).contains("* heads/main ") && ref_is_unpublished(&dest, "heads/main"),
        "the refusal came before the publish: {}",
        branch_listing(&dest)
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("f.txt")).unwrap(),
        "mine, not the block's\n",
        "the user's file is untouched"
    );

    // With the way clear, the branch is created and its files are written.
    std::fs::remove_file(dest.join("f.txt")).unwrap();
    support::ok(
        &support::branch_create(&dest, "heads/main", &old),
        "branch create heads/main --from the block",
    );
    assert_eq!(
        std::fs::read_to_string(dest.join("f.txt")).unwrap(),
        "one\n"
    );
}

fn ref_is_unpublished(repo: &Path, ref_name: &str) -> bool {
    let layout = RepositoryLayout::open(repo).unwrap();
    RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .is_none()
}
