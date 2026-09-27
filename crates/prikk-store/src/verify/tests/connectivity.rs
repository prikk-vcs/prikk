//! RFC 162 rule 2: connectivity from a queued patch to the blob its own operation references,
//! independent of the object index -- the exact shape of the external review's M1.

use prikk_error::Result;

use crate::test_gates::test_support::{
    signed_patch_blob_envelope, signed_patch_envelope, unique_temp_dir,
};
use crate::{
    ConnectivityIssue, DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RepositoryLayout, Wal,
    repair_object_index, verify_repository,
};

/// M1, reproduced end to end: a blob referenced only by a *queued* (unsealed) patch is damaged.
/// `verify` must fail and name the patch -- and, the exact regression the external review found,
/// `doctor --repair-index` must not make that failure disappear (rule 1's rebuild dropping the
/// unreadable blob's own entry must not silence rule 2's connectivity check).
#[test]
fn a_queued_patch_referencing_a_damaged_blob_fails_verify_before_and_after_repair_index()
-> Result<()> {
    let root = unique_temp_dir("connectivity-m1");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = signed_patch_blob_envelope();
    let blob_id = objects.write_object(&blob)?;

    let patch = signed_patch_envelope();
    let patch_id = patch.object_id();
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&patch)?;

    // Damage the blob's own container frame (flip a body byte, past the header, so the frame's own
    // checksum fails -- exactly M1's "one byte flipped in the blob" construction).
    let blob_container = layout.container_slot_path(
        prikk_object::ObjectType::Blob,
        crate::foundation::layout::ContainerSlot::A,
    );
    let mut bytes = std::fs::read(&blob_container)?;
    let last = bytes
        .last_mut()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("blob container empty".to_string()))?;
    *last ^= 0x01;
    std::fs::write(&blob_container, &bytes)?;

    let before = verify_repository(&layout)?;
    assert!(
        before.has_item_failure(),
        "a queued patch referencing a damaged blob must fail verify: {before:?}"
    );
    assert!(
        before
            .connectivity_issues
            .iter()
            .any(|issue| issue.object_id == blob_id
                && issue.referencing_work.contains(&patch_id.to_string())),
        "the connectivity issue must name the referencing patch: {:?}",
        before.connectivity_issues
    );

    // M1's exact regression: `doctor --repair-index` must not silently clear this. The rebuild drops
    // the damaged blob's own index entry (it cannot re-derive it), but the queued patch still
    // references it, so verify must still fail afterward.
    let repair = repair_object_index(&layout)?;
    assert_eq!(
        repair.objects_recovered, 0,
        "the damaged blob cannot be recovered by the rebuild"
    );
    // RFC 162 rule 2: "`--repair-index` never forgets silently" -- the blob's own id, named by the old
    // index, is not re-derivable and must be recorded, not dropped without a trace.
    assert_eq!(
        repair.lost_ids,
        vec![blob_id],
        "the repair must name exactly the one id it could not re-derive"
    );
    let recovery_file = repair.recovery_file.as_ref().ok_or_else(|| {
        prikk_error::PrikkError::Integrity("expected a recovery file".to_string())
    })?;
    let recovered = std::fs::read_to_string(layout.prikk_dir().join(recovery_file))?;
    assert!(
        recovered.contains(&blob_id.to_string()),
        "the recovery file must name the lost id: {recovered}"
    );
    let after = verify_repository(&layout)?;
    assert!(
        after.has_item_failure(),
        "after the index repair, the queued patch still references the damaged blob, so verify \
         must still fail: {after:?}"
    );
    assert!(
        after
            .connectivity_issues
            .iter()
            .any(|issue: &ConnectivityIssue| issue.object_id == blob_id),
        "the connectivity issue must survive the index repair: {:?}",
        after.connectivity_issues
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
