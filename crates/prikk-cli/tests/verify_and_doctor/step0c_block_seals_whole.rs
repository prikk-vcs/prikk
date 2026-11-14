//! 0.58.0 step 0, part 0c: the reference says when `verify --format json`'s `block_seals` is whole
//! (`docs/src/reference/commands.md`, the `block_seals` item).
//!
//! The sentence: **when `stages` reports `objects` as `evaluated` and `failed_objects` is empty, `block_seals` holds
//! every sealed Block reached from committed state.** A damaged Block is named in `failed_objects` and is not in the list.
//! A consumer reads both conditions from fields it already has; no field is added.
//!
//! What these tests hold (measured on the published 0.57.0 by the architect, and on the build; see the part 0c report):
//! - a sound repository: the list is `log`'s blocks;
//! - one damaged Block record: the list loses it and `failed_objects` names a `block`;
//! - a table of one-byte damages over the containers a Block's reachability runs through: whenever the two conditions
//!   hold, the list is exactly the sound repository's list, and a damage that hides a Block is always one of the rows
//!   where a condition fails (a damaged RefState or Patch is listed in `failed_objects`; a damaged trust container or
//!   pointer index leaves the `objects` stage not `evaluated`).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::path::Path;

use crate::support;
use crate::support::json::Value;

fn verify_json(repo: &Path) -> (Option<i32>, Value) {
    let output = support::prikk(repo)
        .args(["verify", "--format", "json"])
        .output()
        .unwrap();
    (
        output.status.code(),
        support::json::parse(&String::from_utf8_lossy(&output.stdout)),
    )
}

fn log_block_ids(repo: &Path) -> BTreeSet<String> {
    let output = support::prikk(repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&output, "log --format json");
    support::json::parse(&String::from_utf8_lossy(&output.stdout))
        .get("blocks")
        .as_array()
        .iter()
        .map(|block| block.get("block_id").as_str().to_string())
        .collect()
}

fn sealed_ids(document: &Value) -> BTreeSet<String> {
    document
        .get("block_seals")
        .as_array()
        .iter()
        .map(|seal| seal.get("block_id").as_str().to_string())
        .collect()
}

/// The first of the two conditions the reference states.
fn objects_evaluated(document: &Value) -> bool {
    document
        .get("stages")
        .as_array()
        .iter()
        .find(|stage| stage.get("stage").as_str() == "objects")
        .expect("the objects stage")
        .get("status")
        .as_str()
        == "evaluated"
}

/// The second.
fn failed_object_types(document: &Value) -> Vec<String> {
    document
        .get("failed_objects")
        .as_array()
        .iter()
        .map(|entry| entry.get("object_type").as_str().to_string())
        .collect()
}

/// Three sealed Blocks on `heads/main`.
fn three_blocks(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "f.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "f.txt", b"two\n", "two");
    support::generation(&repo, "heads/main", "f.txt", b"three\n", "three");
    repo
}

/// Flip one byte of `.prikk/<container>.container` at `offset_from_end` bytes from its end (or the middle for `None`).
fn damage(repo: &Path, container: &str, offset_from_end: Option<usize>) {
    let path = repo.join(".prikk").join(format!("{container}.container"));
    let mut bytes = std::fs::read(&path).unwrap();
    let at = match offset_from_end {
        Some(back) => bytes.len() - back,
        None => bytes.len() / 2,
    };
    bytes[at] ^= 0xff;
    std::fs::write(&path, bytes).unwrap();
}

/// A sound repository: both conditions hold, and the list is `log`'s blocks.
#[test]
fn a_sound_repository_lists_every_block_log_lists() {
    let repo = three_blocks("0c-sound");
    let (code, document) = verify_json(&repo);
    assert_eq!(code, Some(0));
    assert!(objects_evaluated(&document));
    assert!(failed_object_types(&document).is_empty());
    let listed = sealed_ids(&document);
    assert_eq!(listed.len(), 3, "{document:?}");
    assert_eq!(listed, log_block_ids(&repo), "block_seals is log's blocks");
}

/// One Block record damaged: the list loses it, and `failed_objects` names a `block`. (Measured on 0.57.0: 3 -> 2.)
#[test]
fn a_damaged_block_is_named_in_failed_objects_and_is_not_in_the_list() {
    let repo = three_blocks("0c-block");
    let sound = sealed_ids(&verify_json(&repo).1);
    assert_eq!(sound.len(), 3);

    damage(&repo, "containers/block/a", Some(40));
    let (code, document) = verify_json(&repo);
    assert_eq!(code, Some(1));
    let listed = sealed_ids(&document);
    assert!(
        listed.len() < sound.len() && listed.is_subset(&sound),
        "the list lost a Block: {listed:?} of {sound:?}"
    );
    assert!(
        failed_object_types(&document).contains(&"block".to_string()),
        "the damaged Block is named: {document:?}"
    );
}

/// The table. For every damage: if `objects` is `evaluated` and `failed_objects` is empty, the list is the sound list.
/// A damage that loses a Block must therefore fail a condition, and the table says which.
#[test]
fn when_both_conditions_hold_the_list_is_whole() {
    let repo = three_blocks("0c-table");
    let sound = sealed_ids(&verify_json(&repo).1);
    assert_eq!(sound.len(), 3);
    let pristine = support::unique_repo("0c-table-pristine");
    let _ = std::fs::remove_dir_all(&pristine);
    support::copy_dir_recursive(&repo.join(".prikk"), &pristine);

    // (container, offset from the end or the middle, expected: the two conditions hold?)
    let rows: &[(&str, Option<usize>, bool)] = &[
        ("containers/block/a", None, false),
        ("containers/block/a", Some(40), false),
        ("containers/ref-state/a", None, false),
        ("containers/ref-state/a", Some(40), false),
        ("containers/patch/a", None, false),
        ("containers/patch/a", Some(40), false),
        ("containers/blob/a", None, false),
        // A damaged ref log or object index does not hide a Block, and `failed_objects` stays empty.
        ("containers/index", None, true),
        ("refs/containers/log-a", None, true),
        ("refs/containers/log-a", Some(40), true),
        // A damaged pointer index or trust container leaves the `objects` stage not `evaluated`.
        ("refs/containers/pointer-index-a", None, false),
        ("trust/policy-a", None, false),
        ("trust/keys", None, false),
    ];
    let mut conditions_held = 0;
    for (container, offset, holds) in rows {
        let _ = std::fs::remove_dir_all(repo.join(".prikk"));
        support::copy_dir_recursive(&pristine, &repo.join(".prikk"));
        damage(&repo, container, *offset);
        let (_, document) = verify_json(&repo);
        let both = objects_evaluated(&document) && failed_object_types(&document).is_empty();
        let row = format!("{container} {offset:?}");
        assert_eq!(both, *holds, "{row}: the two conditions: {document:?}");
        if both {
            conditions_held += 1;
            assert_eq!(sealed_ids(&document), sound, "{row}: the list is whole");
        } else {
            // Not claimed: only that the damage is visible in a field the consumer already reads.
            assert!(
                !objects_evaluated(&document) || !failed_object_types(&document).is_empty(),
                "{row}"
            );
        }
    }
    assert_eq!(conditions_held, 3, "the rows where the sentence is tested");

    // A damaged RefState hides Blocks while `failed_objects` lists it (measured: 3 -> 0 with the tail damaged).
    let _ = std::fs::remove_dir_all(repo.join(".prikk"));
    support::copy_dir_recursive(&pristine, &repo.join(".prikk"));
    damage(&repo, "containers/ref-state/a", Some(40));
    let (_, document) = verify_json(&repo);
    assert!(sealed_ids(&document).len() < sound.len(), "{document:?}");
    assert!(failed_object_types(&document).contains(&"ref-state".to_string()));
}
