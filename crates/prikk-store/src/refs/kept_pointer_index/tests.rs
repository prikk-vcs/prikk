//! 0.58.0 S5: the ref pointer index a hold keeps (`refs/kept_pointer_index.rs`).
//!
//! What these hold: nothing is kept outside a hold and the last release drops it; under a hold the slot is read whole once and every later use
//! reads the canary and what was appended; the copy equals a fresh read after every kind of change (another ref's publication, a compaction that
//! switches the live slot, a replaced file, a truncate-and-regrow, a damaged or torn tail); and this command's own publication is in the copy
//! without a re-read. **Linux and macOS only**: the identity is `(device, inode)`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::ObjectId;

use super::{KeptPointerIndex, replay_pointer_index_shared};
use crate::foundation::fsutil::read_tally;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::lock::acquire_container_locks;
use crate::refs::pointer_index::replay_pointer_index;
use crate::test_gates::test_support::{
    sample_object_id, signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::{LockableContainer, RefPublication, RefStore};

/// Publish the `seq`-th state of `ref_name` (1 = the first), as a fresh pointer entry and log record. The target block is a bare id: a
/// publication does not dereference it.
pub(in crate::refs) fn publish_ref(
    layout: &RepositoryLayout,
    ref_name: &str,
    seq: u64,
) -> Result<ObjectId> {
    let store = RefStore::new(layout.clone());
    let previous = store.read_current_ref_state_id(ref_name)?;
    let target = sample_object_id(&format!("{ref_name} target {seq}"));
    let ref_state = signed_ref_state_envelope(ref_name, previous, target, seq);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(ref_name, previous, ref_state_id, target, seq);
    store.publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: previous,
        ref_state,
        ref_update,
    })
}

fn slot_relative(layout: &RepositoryLayout, slot: ContainerSlot) -> std::path::PathBuf {
    layout
        .repository_relative(&layout.ref_pointer_index_slot_path(slot))
        .expect("relative")
}

fn bytes_read(layout: &RepositoryLayout, slot: ContainerSlot) -> u64 {
    read_tally::bytes_read(&slot_relative(layout, slot))
}

fn file_len(layout: &RepositoryLayout, slot: ContainerSlot) -> u64 {
    std::fs::metadata(layout.ref_pointer_index_slot_path(slot))
        .expect("slot")
        .len()
}

/// Another process: the same repository opened afresh has its own `MutationRoot`, and so no hold and no kept copy.
fn another_process(layout: &RepositoryLayout) -> RepositoryLayout {
    RepositoryLayout::open(layout.root().to_path_buf()).expect("reopen")
}

fn hold(layout: &RepositoryLayout) -> crate::lock::ContainerLockGuard {
    acquire_container_locks(layout, &[LockableContainer::TrustPolicy]).expect("hold")
}

/// The shared replay must equal a fresh whole read, every time.
fn assert_equals_a_fresh_read(layout: &RepositoryLayout, label: &str) {
    let shared = replay_pointer_index_shared(layout).expect("shared");
    let fresh = replay_pointer_index(layout).expect("fresh");
    assert_eq!(*shared, fresh, "{label}: kept and fresh replay differ");
}

fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    (RepositoryLayout::init(root.clone()).expect("init"), root)
}

#[test]
fn nothing_is_kept_outside_a_hold_and_the_last_release_drops_it() -> Result<()> {
    let (layout, root) = repository("s5p-absent");
    publish_ref(&layout, "heads/main", 1)?;
    let mutation = layout.repository_mutation_root();
    let _ = replay_pointer_index_shared(&layout)?;
    assert!(!mutation.kept_value_is_held_for_test::<KeptPointerIndex>());

    let first = hold(&layout);
    let _ = replay_pointer_index_shared(&layout)?;
    assert!(mutation.kept_value_is_held_for_test::<KeptPointerIndex>());
    let second = acquire_container_locks(&layout, &[LockableContainer::RefLog])?;
    drop(first);
    assert!(mutation.kept_value_is_held_for_test::<KeptPointerIndex>());
    drop(second);
    assert!(!mutation.kept_value_is_held_for_test::<KeptPointerIndex>());
    let _ = replay_pointer_index_shared(&layout)?;
    assert!(!mutation.kept_value_is_held_for_test::<KeptPointerIndex>());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The reads: whole once under a hold, then the canary (the last record) per use, then the canary and what was appended; outside a hold every
/// use reads the whole slot, as before. And this command's own publication is already in the copy.
#[test]
fn a_hold_reads_the_slot_whole_once_and_then_the_canary_and_what_was_appended() -> Result<()> {
    let (layout, root) = repository("s5p-reads");
    for seq in 1..=20 {
        publish_ref(&layout, "heads/main", seq)?;
    }
    let whole = file_len(&layout, ContainerSlot::A);

    read_tally::reset();
    for _ in 0..3 {
        let _ = replay_pointer_index_shared(&layout)?;
    }
    assert_eq!(bytes_read(&layout, ContainerSlot::A), 3 * whole);

    let _hold = hold(&layout);
    read_tally::reset();
    let _ = replay_pointer_index_shared(&layout)?;
    assert_eq!(bytes_read(&layout, ContainerSlot::A), whole);
    let record = whole / 20;
    read_tally::reset();
    for _ in 0..5 {
        let _ = replay_pointer_index_shared(&layout)?;
    }
    assert_eq!(
        bytes_read(&layout, ContainerSlot::A),
        5 * record,
        "each later use reads the canary, one record"
    );

    // This command's own publication: the reads inside `publish` (the CAS, the agreement check) keep the copy current, so the next use reads
    // the canary and nothing else.
    publish_ref(&layout, "heads/main", 21)?;
    read_tally::reset();
    let shared = replay_pointer_index_shared(&layout)?;
    assert_eq!(shared.entries.len(), 21);
    assert_eq!(bytes_read(&layout, ContainerSlot::A), record);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 2: another writer publishes a **different** ref between two of this command's publications, and the next sees it.
#[test]
fn another_commands_publication_of_a_different_ref_is_seen_by_the_next_use() -> Result<()> {
    let (layout, root) = repository("s5p-other-ref");
    publish_ref(&layout, "heads/main", 1)?;
    let other = another_process(&layout);
    let _hold = hold(&layout);
    let store = RefStore::new(layout.clone());
    assert_eq!(store.list_ref_pointers()?.len(), 1);

    let tag_state = publish_ref(&other, "heads/other", 1)?;
    assert_eq!(
        store.read_current_ref_state_id("heads/other")?,
        Some(tag_state),
        "the next lookup reads the appended tail"
    );
    assert_eq!(store.list_ref_pointers()?.len(), 2);
    // And this command's own next publication classifies against it: a duplicate name differing only in case is refused (the collision scan
    // reads the list the other command just extended).
    assert_equals_a_fresh_read(&layout, "after the other command's publication");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The generation: a compaction (another process, the pointer-index lock only) switches the live slot between two uses. The copy of the old slot is
/// not used for the new one: the next use is the whole new slot, and equals a fresh read.
#[test]
fn a_compaction_between_two_uses_is_a_full_re_read_of_the_new_slot() -> Result<()> {
    let (layout, root) = repository("s5p-compaction");
    for seq in 1..=6 {
        publish_ref(&layout, "heads/main", seq)?;
    }
    let other = another_process(&layout);
    let _hold = hold(&layout);
    let _ = replay_pointer_index_shared(&layout)?;
    let before = RefStore::new(layout.clone()).read_current_ref_state_id("heads/main")?;

    crate::compact::compact_ref_pointer_index(&other)?;
    assert_eq!(
        crate::refs::pointer_index::live_pointer_index_slot(&layout)?.0,
        ContainerSlot::B,
        "fixture sanity: the compaction switched the live slot"
    );
    read_tally::reset();
    let shared = replay_pointer_index_shared(&layout)?;
    assert_eq!(
        bytes_read(&layout, ContainerSlot::B),
        file_len(&layout, ContainerSlot::B),
        "the new slot is read whole"
    );
    assert_eq!(bytes_read(&layout, ContainerSlot::A), 0);
    assert_eq!(shared.entries.len(), 1, "the compacted slot");
    assert_eq!(
        RefStore::new(layout.clone()).read_current_ref_state_id("heads/main")?,
        before
    );
    assert_equals_a_fresh_read(&layout, "after the compaction");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 4: damage or a torn tail appearing in the tail between two uses. Nothing damaged is kept, and the call returns what a fresh read returns.
#[test]
fn damage_and_a_torn_tail_between_two_uses_behave_as_a_fresh_read_does() -> Result<()> {
    for torn in [false, true] {
        let (layout, root) = repository(if torn { "s5p-torn" } else { "s5p-damaged" });
        publish_ref(&layout, "heads/main", 1)?;
        let other = another_process(&layout);
        let _hold = hold(&layout);
        let _ = replay_pointer_index_shared(&layout)?;
        assert!(
            layout
                .repository_mutation_root()
                .kept_value_is_held_for_test::<KeptPointerIndex>()
        );

        publish_ref(&other, "heads/other", 1)?;
        let path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
        if torn {
            let file = std::fs::OpenOptions::new().write(true).open(&path)?;
            file.set_len(std::fs::metadata(&path)?.len() - 7)?;
        } else {
            let mut bytes = std::fs::read(&path)?;
            let flip = bytes.len() - 10;
            bytes[flip] ^= 0xff;
            std::fs::write(&path, &bytes)?;
        }
        let shared = replay_pointer_index_shared(&layout)?;
        let fresh = replay_pointer_index(&layout)?;
        assert_eq!(*shared, fresh);
        if torn {
            assert_ne!(shared.trailing_partial_bytes, 0, "fixture sanity");
        } else {
            assert!(shared.has_item_failure(), "fixture sanity");
        }
        assert!(
            !layout
                .repository_mutation_root()
                .kept_value_is_held_for_test::<KeptPointerIndex>(),
            "nothing damaged or torn is kept"
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

/// The file replaced by another of the same length (another inode), and truncated in place and regrown past the copy with other records.
#[test]
fn a_replaced_file_and_a_truncate_and_regrow_are_read_whole() -> Result<()> {
    let (layout, root) = repository("s5p-replaced");
    for seq in 1..=4 {
        publish_ref(&layout, "heads/main", seq)?;
    }
    let other = another_process(&layout);
    let path = layout.ref_pointer_index_slot_path(ContainerSlot::A);
    let _hold = hold(&layout);
    let _ = replay_pointer_index_shared(&layout)?;

    // Replaced by a copy of itself: another inode, the same bytes and length.
    let bytes = std::fs::read(&path)?;
    let temporary = path.with_extension("swap");
    std::fs::write(&temporary, &bytes)?;
    std::fs::rename(&temporary, &path)?;
    read_tally::reset();
    let _ = replay_pointer_index_shared(&layout)?;
    assert_eq!(bytes_read(&layout, ContainerSlot::A), bytes.len() as u64);
    assert_equals_a_fresh_read(&layout, "after the file was replaced");

    // Truncated in place below the copy, then regrown past it with other records.
    let known = std::fs::metadata(&path)?.len();
    let record = known / 4;
    let file = std::fs::OpenOptions::new().write(true).open(&path)?;
    file.set_len(known - 2 * record)?;
    drop(file);
    // Names of the same length as `heads/main`, so the regrown records are the cut ones' size and the copy's end is still a record boundary:
    // only the record at the copy's end (the canary) can tell.
    publish_ref(&other, "heads/aaaa", 1)?;
    publish_ref(&other, "heads/bbbb", 1)?;
    publish_ref(&other, "heads/cccc", 1)?;
    assert_eq!(
        std::fs::metadata(&path)?.len(),
        known + record,
        "fixture sanity: one record longer than the copy, on a record boundary"
    );
    assert_equals_a_fresh_read(&layout, "after the truncate and regrow");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The kept copy equals a fresh read after every step of a mixed sequence of this command's and another command's publications.
#[test]
fn the_kept_copy_equals_a_fresh_read_after_every_publication() -> Result<()> {
    let (layout, root) = repository("s5p-sequence");
    let other = another_process(&layout);
    let _hold = hold(&layout);
    let mut seqs = std::collections::BTreeMap::<String, u64>::new();
    for step in 0..40_usize {
        let name = format!("heads/r{}", step % 5);
        let seq = seqs.entry(name.clone()).or_insert(0);
        *seq += 1;
        let writer = if step % 3 == 0 { &other } else { &layout };
        publish_ref(writer, &name, *seq)?;
        assert_equals_a_fresh_read(&layout, &format!("step {step}"));
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
