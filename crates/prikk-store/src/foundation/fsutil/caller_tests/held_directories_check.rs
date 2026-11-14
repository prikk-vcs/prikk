//! 0.58.0 P2b: the held directories are checked against their paths before a command publishes.
//!
//! The case table (RFC "P2b"): a held directory moves while the command is between its writes and its publish. The check is
//! `MutationRoot::check_held_directories`, called before the first ref, pointer or queue write that makes the work visible
//! (`refs::publication::publish_body`, `received::write_received_pointer`, `Wal::append_patch_reporting`).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use prikk_error::PrikkError;
use prikk_object::{ObjectEnvelope, ObjectType};

use crate::lock::acquire_container_locks;
use crate::test_gates::test_support::{
    dummy_signature, signed_empty_block_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    ActiveLock, LockableContainer, ObjectWriteSession, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, Wal,
};

fn signed_blob(payload: &[u8]) -> prikk_error::Result<ObjectEnvelope> {
    let mut object = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_vec());
    object.add_signature(dummy_signature())?;
    Ok(object)
}

/// A repository with the object-store lock held and two blobs written under it, so the hold's table keeps `containers` and
/// `containers/blob`. Returns the root, the layout, and the guard (dropped by the caller at the end).
fn held_blobs(
    tag: &str,
) -> prikk_error::Result<(PathBuf, RepositoryLayout, crate::lock::ContainerLockGuard)> {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone())?;
    let guard = acquire_container_locks(&layout, &[LockableContainer::ObjectStore])?;
    let mut session = ObjectWriteSession::open(&layout)?;
    for index in 0..2 {
        let object = signed_blob(format!("p2b-{index}").as_bytes())?;
        session.write_object_under_held_lock(&object, &guard)?;
    }
    Ok((root, layout, guard))
}

fn blob_dir(root: &Path) -> PathBuf {
    root.join(".prikk/containers/blob")
}

fn refusal(layout: &RepositoryLayout) -> String {
    match layout.repository_mutation_root().check_held_directories() {
        Err(PrikkError::Precondition(message)) => message,
        other => panic!("expected the held-directory refusal, got {other:?}"),
    }
}

fn assert_names_the_directory_and_the_way_out(message: &str, directory: &str) {
    assert!(message.contains(directory), "names {directory}: {message}");
    assert!(
        message.contains("moved or replaced while this command ran"),
        "{message}"
    );
    assert!(message.contains("nothing was published"), "{message}");
    assert!(
        message.contains("run the command again") && message.contains("`prikk verify`"),
        "a typeable way out: {message}"
    );
}

/// Row 1: nothing moves. The ordinary case is `Ok`, and so is a repeat.
#[test]
fn row1_nothing_moved_is_ok() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-row1")?;
    let mutation = layout.repository_mutation_root();
    assert!(mutation.held_directories_for_test().1 >= 2);
    mutation.check_held_directories()?;
    mutation.check_held_directories()?;
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 2: the held directory is renamed away.
#[test]
fn row2_renamed_away_refuses() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-row2")?;
    std::fs::rename(blob_dir(&root), root.join(".prikk/containers/blob-moved"))?;
    let message = refusal(&layout);
    assert_names_the_directory_and_the_way_out(&message, "containers/blob");
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 3: replaced by a new empty directory of the same name. The held handle still names the old inode.
#[test]
fn row3_replaced_by_an_empty_directory_refuses() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-row3")?;
    std::fs::rename(blob_dir(&root), root.join(".prikk/containers/blob-moved"))?;
    std::fs::create_dir(blob_dir(&root))?;
    let message = refusal(&layout);
    assert_names_the_directory_and_the_way_out(&message, "containers/blob");
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 4: replaced by a symlink to another directory. The path now resolves to the other directory, no-follow it is a symlink.
#[test]
fn row4_replaced_by_a_symlink_refuses() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-row4")?;
    let elsewhere = root.join("elsewhere");
    std::fs::create_dir(&elsewhere)?;
    std::fs::rename(blob_dir(&root), root.join(".prikk/containers/blob-moved"))?;
    std::os::unix::fs::symlink(&elsewhere, blob_dir(&root))?;
    let message = refusal(&layout);
    assert_names_the_directory_and_the_way_out(&message, "containers/blob");
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 5: removed.
#[test]
fn row5_removed_refuses() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-row5")?;
    std::fs::remove_dir_all(blob_dir(&root))?;
    let message = refusal(&layout);
    assert_names_the_directory_and_the_way_out(&message, "containers/blob");
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Row 6: an unrelated directory moves (one the hold never opened). Must not refuse.
#[test]
fn row6_an_unrelated_directory_moving_does_not_refuse() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-row6")?;
    assert!(
        root.join(".prikk/trust").is_dir(),
        "the fixture's unrelated directory"
    );
    std::fs::rename(root.join(".prikk/trust"), root.join(".prikk/trust-moved"))?;
    layout.repository_mutation_root().check_held_directories()?;
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// With no hold open there is nothing held: a moved directory is not this check's business.
#[test]
fn no_hold_no_check() -> prikk_error::Result<()> {
    let root = unique_temp_dir("p2b-no-hold");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::rename(
        root.join(".prikk/containers"),
        root.join(".prikk/containers-moved"),
    )?;
    layout.repository_mutation_root().check_held_directories()?;
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The real publish: with a held directory moved, `RefStore::publish` refuses before the pointer index and the ref log are touched.
#[test]
fn publish_refuses_before_writing_the_pointer_or_the_log() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-publish")?;
    // The hold from `held_blobs` stays open across the publish. The block and the RefState are written through a session under it
    // (`write_object` would take the object-store lock the guard already holds).
    let mut session = ObjectWriteSession::open(&layout)?;
    let block = signed_empty_block_envelope();
    session.write_object_under_held_lock(&block, &guard)?;
    let target = block.object_id();
    let ref_state = signed_ref_state_envelope("heads/main", None, target, 1);
    let ref_state_id = ref_state.object_id();
    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_update: signed_ref_update_envelope("heads/main", None, ref_state_id, target, 1),
        ref_state,
    };
    let pointer = root.join(".prikk/refs/containers/pointer-index-a.container");
    let log = root.join(".prikk/refs/containers/log-a.container");
    let before = (std::fs::read(&pointer)?, std::fs::read(&log)?);

    std::fs::rename(blob_dir(&root), root.join(".prikk/containers/blob-moved"))?;
    let error = RefStore::new(layout.clone())
        .publish_with_object_store(&mut SessionUnderGuard(&mut session, &guard), &publication)
        .expect_err("the publish must refuse");
    let PrikkError::Precondition(message) = error else {
        panic!("expected the held-directory refusal, got {error:?}");
    };
    assert_names_the_directory_and_the_way_out(&message, "containers/blob");
    assert_eq!(
        before,
        (std::fs::read(&pointer)?, std::fs::read(&log)?),
        "neither the pointer index nor the ref log was written"
    );
    assert_eq!(
        RefStore::new(layout.clone()).read_current_ref_state_id("heads/main")?,
        None,
        "nothing was published"
    );
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Adapter: an `ObjectWriter` that writes through a session under the guard the test holds (the publish writes its RefState with it).
struct SessionUnderGuard<'a>(
    &'a mut ObjectWriteSession,
    &'a crate::lock::ContainerLockGuard,
);

impl ObjectWriter for SessionUnderGuard<'_> {
    fn write_object(
        &mut self,
        envelope: &ObjectEnvelope,
    ) -> prikk_error::Result<prikk_object::ObjectId> {
        self.0.write_object_under_held_lock(envelope, self.1)
    }
}

/// The queue: a commit's WAL append refuses when a held directory moved, and appends nothing.
#[test]
fn the_queue_append_refuses_when_a_held_directory_moved() -> prikk_error::Result<()> {
    let root = unique_temp_dir("p2b-wal");
    let layout = RepositoryLayout::init(root.clone())?;
    let _lock = ActiveLock::acquire(&layout, "default")?;
    let wal = Wal::for_layout(&layout, crate::DEFAULT_ACTIVE_NAME);
    let patch = signed_patch_envelope();
    // The lock file's own walk kept `active/default`; move it away and put an empty one in its place.
    std::fs::rename(
        root.join(".prikk/active/default"),
        root.join(".prikk/active/default-moved"),
    )?;
    std::fs::create_dir(root.join(".prikk/active/default"))?;
    let error = wal
        .append_patch(&patch)
        .expect_err("the append must refuse");
    let PrikkError::Precondition(message) = error else {
        panic!("expected the held-directory refusal, got {error:?}");
    };
    assert_names_the_directory_and_the_way_out(&message, "active/default");
    assert!(
        !root.join(".prikk/active/default/queue.wal").exists()
            || std::fs::metadata(root.join(".prikk/active/default/queue.wal"))?.len() == 0,
        "nothing was appended to the live queue"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The received pointer (`bundle import`'s and `archive import`'s visible effect): refuses when a held directory moved, and writes
/// nothing to the received index.
#[test]
fn the_received_pointer_write_refuses_when_a_held_directory_moved() -> prikk_error::Result<()> {
    let (root, layout, guard) = held_blobs("p2b-received")?;
    drop(guard);
    let guard = acquire_container_locks(
        &layout,
        &[
            LockableContainer::ObjectStore,
            LockableContainer::ReceivedIndex,
        ],
    )?;
    let mut session = ObjectWriteSession::open(&layout)?;
    session.write_object_under_held_lock(&signed_blob(b"p2b-received-blob")?, &guard)?;
    let received_index = root.join(".prikk/refs/containers/received-index-a.container");
    let before = std::fs::read(&received_index)?;

    std::fs::rename(blob_dir(&root), root.join(".prikk/containers/blob-moved"))?;
    let error = crate::received::write_received_pointer_under_held_lock(
        &layout,
        "remotes/origin/main",
        prikk_object::ObjectId::from_bytes([7; 32]),
        &guard,
    )
    .expect_err("the received pointer write must refuse");
    let PrikkError::Precondition(message) = error else {
        panic!("expected the held-directory refusal, got {error:?}");
    };
    assert_names_the_directory_and_the_way_out(&message, "containers/blob");
    assert_eq!(
        before,
        std::fs::read(&received_index)?,
        "the received index is untouched"
    );
    drop(guard);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
