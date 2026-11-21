//! 0.59.0 RS2b: *an id that a pointer named is declared missing only after a lookup at least as new as the pointer read.*
//!
//! Each test lands a whole publication between two reads of a site that reads a pointer and an object handle (`refs::view_hooks`), asserts its hook
//! ran, and was made red by reverting the one thing it holds (`.git-exclude/logs/0.59.0-RS2b/perturb.sh`). The last tests hold the other half: an id
//! that is truly missing is still called missing, with 0.58.0's words.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_error::PrikkError;
use prikk_object::{BlockKind, ObjectId, ObjectType};

use crate::refs::view_hooks;
use crate::test_gates::test_support::{
    signed_block, signed_empty_block_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    FileObjectStore, ObjectReadSnapshot, ObjectReader, ObjectWriteSession, ObjectWriter,
    RefPublication, RefStore, RepositoryLayout,
};

fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf, ObjectId) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let block = signed_empty_block_envelope();
    let target = block.object_id();
    FileObjectStore::new(layout.clone())
        .write_object(&block)
        .unwrap();
    (layout, root, target)
}

/// Publish `ref_name` at sequence `seq`, over `previous` (None for the first), pointing at `target`.
fn publish(
    layout: &RepositoryLayout,
    ref_name: &str,
    previous: Option<ObjectId>,
    seq: u64,
    target: ObjectId,
) -> ObjectId {
    let ref_state = signed_ref_state_envelope(ref_name, previous, target, seq);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(ref_name, previous, ref_state_id, target, seq);
    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: previous,
            ref_state,
            ref_update,
        })
        .unwrap()
}

/// `open_at_ref_tip` (the helper behind `replay_ref_chain`, `resolve_node_lineage_bounds`, `resolve_point`, the inverse plan and the cache
/// rebuild) reads the pointer, then opens its snapshot. A publication landing between the two is harmless: the snapshot holds it. Opened first, the
/// snapshot lacked the RefState the pointer then named (`ref … points to missing RefState …`: `log --ref`, `tree`, `cat`, `diff`).
#[test]
fn the_snapshot_opens_after_the_pointer_is_read() {
    let (layout, root, target) = repository("rs2b-open-at-tip");
    let first = publish(&layout, "heads/main", None, 1, target);
    let publisher = layout.clone();
    // The landing publication carries a new Block too, so a snapshot opened before it cannot hold the tip it names.
    let newer = signed_block(BlockKind::Normal, vec![target], Vec::new(), None);
    let newer_id = newer.object_id();
    view_hooks::set("tip-between-pointer-and-snapshot", move || {
        FileObjectStore::new(publisher.clone())
            .write_object(&newer)
            .unwrap();
        publish(&publisher, "heads/main", Some(first), 2, newer_id);
    });
    let (snapshot, tip) =
        crate::refs::open_at_ref_tip(&layout, "heads/main").expect("a healthy ref");
    assert!(!view_hooks::pending("tip-between-pointer-and-snapshot"));
    assert_eq!(tip, target, "the tip as of the pointer read");
    assert!(
        snapshot
            .read_typed(newer_id, ObjectType::Block)
            .unwrap()
            .is_some(),
        "and the snapshot, opened after, holds even what landed after"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// `log`'s history walk: the pointer, then the snapshot.
#[test]
fn the_history_walk_opens_its_snapshot_after_the_pointer() {
    let (layout, root, target) = repository("rs2b-history");
    let first = publish(&layout, "heads/main", None, 1, target);
    let publisher = layout.clone();
    view_hooks::set("history-between-pointer-and-snapshot", move || {
        publish(&publisher, "heads/main", Some(first), 2, target);
    });
    let history =
        crate::history::load_ref_history(&layout, "heads/main", 10).expect("a healthy ref");
    assert!(!view_hooks::pending("history-between-pointer-and-snapshot"));
    assert_eq!(
        history.entries.len(),
        1,
        "the history as of the pointer read"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A site whose snapshot is opened before it reads the pointer (`sync build`/`compare`'s digest, `sync`'s delta) looks a missing id up once more
/// through the fresh reader: a first-ever publication landing after the snapshot is not `missing RefState object`.
#[test]
fn a_publication_landing_after_the_snapshot_is_found_on_the_second_lookup() {
    let (layout, root, target) = repository("rs2b-fresh-on-miss");
    let publisher = layout.clone();
    view_hooks::set("digest-after-snapshot", move || {
        publish(&publisher, "heads/main", None, 1, target);
    });
    crate::patch_set_digest::compute_patch_set_digest_for_ref(&layout, "heads/main")
        .expect("the ref published after the snapshot opened");
    assert!(!view_hooks::pending("digest-after-snapshot"));

    let publisher = layout.clone();
    let first = RefStore::new(layout.clone())
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    view_hooks::set("delta-after-snapshot", move || {
        publish(&publisher, "heads/main", Some(first), 2, target);
    });
    let have = crate::sync_negotiation::HaveList {
        ref_name: "heads/main".to_string(),
        digest: crate::patch_set_digest::compute_patch_set_digest(&[]).unwrap(),
        patch_ids: Vec::new(),
    };
    crate::sync_negotiation::compute_sync_delta(&layout, &have)
        .expect("the ref advanced after the snapshot opened");
    assert!(!view_hooks::pending("delta-after-snapshot"));
    let _ = std::fs::remove_dir_all(root);
}

/// The wrapper itself: a hit is the handle's answer; a miss is looked up through the live index; an id that is nowhere is `None`.
#[test]
fn the_fresh_reader_hits_for_free_and_misses_only_what_is_nowhere() {
    let (layout, root, target) = repository("rs2b-wrapper");
    let snapshot = ObjectReadSnapshot::open(&layout).unwrap();
    let later = signed_ref_state_envelope("heads/main", None, target, 1);
    FileObjectStore::new(layout.clone())
        .write_object(&later)
        .unwrap();
    assert!(
        snapshot
            .read_typed(later.object_id(), ObjectType::RefState)
            .unwrap()
            .is_none(),
        "the snapshot is a point in time"
    );
    let fresh = crate::refs::PointerFreshReader::over(&layout, &snapshot);
    assert!(
        fresh
            .read_typed(target, ObjectType::Block)
            .unwrap()
            .is_some()
    );
    assert!(
        fresh
            .read_typed(later.object_id(), ObjectType::RefState)
            .unwrap()
            .is_some()
    );
    assert!(
        fresh
            .has_object(later.object_id(), ObjectType::RefState)
            .unwrap()
    );
    let nowhere = signed_ref_state_envelope("heads/nowhere", None, target, 1).object_id();
    assert!(
        fresh
            .read_typed(nowhere, ObjectType::RefState)
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A mutable handle (a write session, a preview's snapshot) is brought up to date once, on a miss: the RefState is found, and so is everything
/// that RefState needs, which the same handle then holds.
#[test]
fn a_mutable_handle_is_refreshed_once_on_a_miss() {
    let (layout, root, target) = repository("rs2b-refresh");
    let mut session = ObjectWriteSession::open(&layout).unwrap();
    let mut snapshot = ObjectReadSnapshot::open(&layout).unwrap();
    let state = publish(&layout, "heads/main", None, 1, target);
    assert!(
        session
            .read_typed(state, ObjectType::RefState)
            .unwrap()
            .is_none()
    );
    assert!(
        crate::refs::read_pointer_named(&mut session, state, ObjectType::RefState)
            .unwrap()
            .is_some()
    );
    assert!(
        crate::refs::read_pointer_named(&mut snapshot, state, ObjectType::RefState)
            .unwrap()
            .is_some()
    );
    // nothing to find: still none after the refresh
    let nowhere = signed_ref_state_envelope("heads/nowhere", None, target, 1).object_id();
    assert!(
        crate::refs::read_pointer_named(&mut session, nowhere, ObjectType::RefState)
            .unwrap()
            .is_none()
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The other half: a RefState that is truly missing is still called missing, with 0.58.0's words (`states.sh` state 5 holds them at the command).
#[test]
fn a_truly_missing_ref_state_keeps_its_message() {
    let (layout, root, target) = repository("rs2b-missing");
    let state = publish(&layout, "heads/main", None, 1, target);
    crate::foundation::index::remove_index_entry_for_test(&layout, state).unwrap();
    let error = crate::refs::open_at_ref_tip(&layout, "heads/main")
        .map(|_| ())
        .expect_err("missing");
    assert_eq!(
        error,
        PrikkError::Integrity(format!("ref heads/main points to missing RefState {state}"))
    );
    let error = crate::history::load_ref_history(&layout, "heads/main", 10).expect_err("missing");
    assert!(
        error
            .to_string()
            .contains(&format!("history RefState {state} is missing")),
        "{error}"
    );
    let error = crate::patch_set_digest::compute_patch_set_digest_for_ref(&layout, "heads/main")
        .expect_err("missing");
    assert!(
        error
            .to_string()
            .contains(&format!("missing RefState object: {state}")),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(root);
}
