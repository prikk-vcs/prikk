//! 0.58.0 S6: the record a hold keeps and a deferral scope (`DeferredRecords`) writes once.
//!
//! What these hold: nothing is kept outside a hold; under a hold the file is read once; inside a scope nothing is written until it ends and every
//! `load_verified_blocks` sees the ids already (so a later claim's derivation anchors on an earlier claim's Block); the one write merges with
//! the record as it is then (another process's ids survive) and adds only ids confirmed here; a scope that never ends (a kill) leaves the record
//! without its ids, which is a cost, never a wrong entry. **Linux and macOS only**: the kept set is compared by `(device, inode, size)`.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::ObjectId;

use super::super::{
    KeptVerified, defer_records, load_verified_blocks, record_path, record_verified_blocks,
};
use crate::foundation::fsutil::read_tally;
use crate::lock::acquire_container_locks;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{LockableContainer, RepositoryLayout};

fn id(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

fn relative(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout
        .repository_relative(&record_path(layout))
        .expect("relative")
}

fn another_process(layout: &RepositoryLayout) -> RepositoryLayout {
    RepositoryLayout::open(layout.root().to_path_buf()).expect("reopen")
}

#[test]
fn nothing_is_kept_outside_a_hold_and_a_scope_is_a_hold() -> Result<()> {
    let root = unique_temp_dir("s6v-absent");
    let layout = RepositoryLayout::init(root.clone())?;
    let mutation = layout.repository_mutation_root();
    record_verified_blocks(&layout, [id(1)]);
    let _ = load_verified_blocks(&layout);
    assert!(!mutation.kept_value_is_held_for_test::<KeptVerified>());

    let scope = defer_records(&layout);
    assert!(mutation.kept_value_is_held_for_test::<KeptVerified>());
    drop(scope);
    assert!(
        !mutation.kept_value_is_held_for_test::<KeptVerified>(),
        "the scope's hold was the last"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Inside a scope: nothing is written, the ids are seen at once, and the end of the scope writes them once.
#[test]
fn a_scope_writes_nothing_until_it_ends_and_every_load_sees_its_ids() -> Result<()> {
    let root = unique_temp_dir("s6v-defer");
    let layout = RepositoryLayout::init(root.clone())?;
    record_verified_blocks(&layout, [id(1), id(2)]);
    let path = record_path(&layout);
    let before = std::fs::read(&path)?;

    let scope = defer_records(&layout);
    for byte in 3..=8 {
        record_verified_blocks(&layout, [id(byte)]);
        assert!(
            load_verified_blocks(&layout).contains(&id(byte)),
            "a later load (a later claim's derivation) sees the entry"
        );
        assert_eq!(
            std::fs::read(&path)?,
            before,
            "and the record is not rewritten"
        );
    }
    drop(scope);
    let after = load_verified_blocks(&layout);
    assert_eq!(after.len(), 8);
    assert!(
        std::fs::read(&path)? != before,
        "written when the scope ended"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The reads: under a scope the record is read once however many times it is loaded.
#[test]
fn a_hold_reads_the_record_once() -> Result<()> {
    let root = unique_temp_dir("s6v-reads");
    let layout = RepositoryLayout::init(root.clone())?;
    record_verified_blocks(&layout, (1..=50).map(id));
    let whole = std::fs::metadata(record_path(&layout))?.len();

    read_tally::reset();
    for _ in 0..3 {
        let _ = load_verified_blocks(&layout);
    }
    assert_eq!(
        read_tally::bytes_read(&relative(&layout)),
        3 * whole,
        "outside a hold: whole each time"
    );

    read_tally::reset();
    let _scope = defer_records(&layout);
    for byte in 100..110 {
        let _ = load_verified_blocks(&layout);
        record_verified_blocks(&layout, [id(byte)]);
    }
    assert_eq!(
        read_tally::bytes_read(&relative(&layout)),
        whole,
        "inside a scope: the file once"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 4: another process rewrites the record while the scope is open. A load sees its ids and ours; the scope's one write keeps its ids
/// and adds ours -- it merges with the record as it is then, so it can lose only what another process lost, never invent an entry.
#[test]
fn another_processs_rewrite_is_seen_and_survives_the_one_write() -> Result<()> {
    let root = unique_temp_dir("s6v-other");
    let layout = RepositoryLayout::init(root.clone())?;
    record_verified_blocks(&layout, [id(1)]);
    let other = another_process(&layout);

    let scope = defer_records(&layout);
    record_verified_blocks(&layout, [id(2)]);
    record_verified_blocks(&other, [id(3)]); // the other process writes the record whole
    let seen = load_verified_blocks(&layout);
    assert!(seen.contains(&id(2)) && seen.contains(&id(3)) && seen.contains(&id(1)));
    // And another rewrite after the scope's last load: only the write's own fresh read of the record can keep it.
    record_verified_blocks(&other, [id(4)]);
    drop(scope);

    let fresh = std::fs::read(record_path(&layout))?;
    let on_disk = super::super::decode(&fresh).expect("a sound record");
    assert_eq!(
        on_disk.iter().copied().collect::<Vec<_>>(),
        vec![id(1), id(2), id(3), id(4)],
        "the other process's ids are kept, ours added, nothing else"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Rule 3: a kill between claims. The scope never ends (here: forgotten), so its ids are not in the record -- absence, which costs a later
/// run a replay and is never a wrong entry -- and a fresh process reads the record as it was.
#[test]
fn a_scope_that_never_ends_leaves_the_record_without_its_ids() -> Result<()> {
    let root = unique_temp_dir("s6v-kill");
    let layout = RepositoryLayout::init(root.clone())?;
    record_verified_blocks(&layout, [id(1)]);
    let scope = defer_records(&layout);
    record_verified_blocks(&layout, [id(2), id(3)]);
    std::mem::forget(scope);

    let fresh = another_process(&layout);
    let set = load_verified_blocks(&fresh);
    assert_eq!(set.iter().copied().collect::<Vec<_>>(), vec![id(1)]);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A non-deferred write under a hold keeps the new identity: the next load does not read the file again.
#[test]
fn an_own_write_under_a_hold_takes_the_new_identity() -> Result<()> {
    let root = unique_temp_dir("s6v-own");
    let layout = RepositoryLayout::init(root.clone())?;
    record_verified_blocks(&layout, (1..=20).map(id));
    let _hold = acquire_container_locks(&layout, &[LockableContainer::TrustPolicy])?;
    let _ = load_verified_blocks(&layout);
    record_verified_blocks(&layout, [id(99)]);
    read_tally::reset();
    let set = load_verified_blocks(&layout);
    assert!(set.contains(&id(99)));
    assert_eq!(
        read_tally::bytes_read(&relative(&layout)),
        0,
        "its own write is in the kept set"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
