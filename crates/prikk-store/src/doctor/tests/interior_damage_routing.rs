//! 0.50.0 P3c: a complete, corrupted record (interior damage, never a tail) in the ref pointer
//! index, the received index, or the trust policy container used to send the user in a circle --
//! every stage that reads the damaged container failed with "run doctor before reading", `doctor`'s
//! own current-branch issue (unrelated to which branch is named) recommended `branch switch`, which
//! does nothing to the damaged index, and nothing in `doctor`'s own output named the one command
//! (`--rebuild-pointer-index`) that actually fixes the pointer-index case. These tests damage a
//! complete record in each container's live slot, then run doctor's own recommendation and confirm
//! it is the right one -- for the pointer index, that command, run next, clears it.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use prikk_error::Result;

use crate::foundation::layout::ContainerSlot;
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    add_trusted_maintainer, doctor_repository, rebuild_pointer_index, verify_repository,
};

fn publish_update(
    store: &RefStore,
    objects: &mut FileObjectStore,
    ref_name: &str,
    expected_previous: Option<prikk_object::ObjectId>,
    seq: u64,
) -> Result<prikk_object::ObjectId> {
    let target = objects.write_object(&signed_empty_block_envelope())?;
    let ref_state = signed_ref_state_envelope(ref_name, expected_previous, target, seq);
    let ref_state_id = ref_state.object_id();
    store.publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: expected_previous,
        ref_update: signed_ref_update_envelope(
            ref_name,
            expected_previous,
            ref_state_id,
            target,
            seq,
        ),
        ref_state,
    })
}

/// Flip the record's own last byte (always inside its body, never its header, whatever the
/// record's own total length is) -- a checksum mismatch, never a tail.
fn flip_a_body_byte(path: &std::path::Path) -> Result<()> {
    let mut bytes = std::fs::read(path)?;
    assert!(
        bytes.len() > 50,
        "fixture must have at least one real record (past the 50-byte header)"
    );
    let offset = bytes.len() - 1;
    bytes[offset] ^= 0xFF;
    std::fs::write(path, bytes)?;
    Ok(())
}

fn issue_recommendation(layout: &RepositoryLayout, code: &str) -> Option<String> {
    doctor_repository(layout)
        .issues
        .into_iter()
        .find(|issue| issue.code == code)
        .map(|issue| issue.recommendation)
}

#[test]
fn pointer_index_interior_damage_names_the_rebuild_and_the_rebuild_clears_it() -> Result<()> {
    let root = unique_temp_dir("p3c-pointer-index-interior-damage");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    flip_a_body_byte(&layout.ref_pointer_index_slot_path(ContainerSlot::A))?;

    let recommendation =
        issue_recommendation(&layout, "PRIKK-DOCTOR-POINTER-INDEX-INTERIOR-DAMAGE");
    assert!(
        recommendation
            .as_deref()
            .is_some_and(|text| text.contains("--rebuild-pointer-index")),
        "{recommendation:?}"
    );
    // Control: the current-branch issue, caused by the identical damage, must not recommend
    // `branch switch` -- the branch name is not the problem.
    let current_branch_recommendation =
        issue_recommendation(&layout, "PRIKK-DOCTOR-CURRENT-BRANCH");
    assert!(
        current_branch_recommendation
            .as_deref()
            .is_some_and(|text| !text.contains("branch switch")),
        "{current_branch_recommendation:?}"
    );

    rebuild_pointer_index(&layout)?;
    let verification = verify_repository(&layout)?;
    assert!(
        !verification.has_stage_failure(),
        "the rebuild must clear the damage: {verification:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn received_index_interior_damage_says_no_repair_exists() -> Result<()> {
    let root = unique_temp_dir("p3c-received-index-interior-damage");
    let layout = RepositoryLayout::init(root.clone())?;
    let target =
        FileObjectStore::new(layout.clone()).write_object(&signed_empty_block_envelope())?;
    let state = signed_ref_state_envelope("heads/main", None, target, 1);
    crate::received::write_received_pointer(&layout, "remotes/heads/main", state.object_id())?;
    flip_a_body_byte(&layout.received_index_slot_path(ContainerSlot::A))?;

    let recommendation =
        issue_recommendation(&layout, "PRIKK-DOCTOR-RECEIVED-INDEX-INTERIOR-DAMAGE");
    assert!(
        recommendation
            .as_deref()
            .is_some_and(|text| text.contains("no repair exists") && text.contains("backup")),
        "{recommendation:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn trust_policy_interior_damage_says_no_repair_exists() -> Result<()> {
    let root = unique_temp_dir("p3c-trust-policy-interior-damage");
    let layout = RepositoryLayout::init(root.clone())?;
    add_trusted_maintainer(&layout, "only", &"11".repeat(32))?;
    flip_a_body_byte(&layout.trust_policy_container_slot_path(ContainerSlot::A))?;

    let recommendation = issue_recommendation(&layout, "PRIKK-DOCTOR-TRUST-POLICY-INTERIOR-DAMAGE");
    assert!(
        recommendation
            .as_deref()
            .is_some_and(|text| text.contains("no repair exists") && text.contains("backup")),
        "{recommendation:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Control: a stage failure from a cause these routing fixes do not cover (an unrelated, damaged
/// author-key container -- out of this round's scope) must still read as a generic "inspect the
/// failing stage" -- proving the fix is scoped to the three containers it names, not every stage
/// failure.
#[test]
fn an_unrelated_stage_failure_still_says_inspect() -> Result<()> {
    let root = unique_temp_dir("p3c-unrelated-stage-failure-still-inspect");
    let layout = RepositoryLayout::init(root.clone())?;
    add_trusted_maintainer(&layout, "only", &"11".repeat(32))?;
    // The trust *key* container (not the policy, one of this round's three) -- out of scope for
    // every fix this round makes, so its own stage failure must still read as the generic
    // "inspect the failing stage" this control checks for.
    flip_a_body_byte(&layout.trust_key_container_path())?;

    let report = doctor_repository(&layout);
    assert!(
        report
            .issues
            .iter()
            .any(|issue| issue.code == "PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE"
                && issue.recommendation.contains("inspect the failing stage")),
        "{report:?}"
    );
    assert!(
        !report.issues.iter().any(|issue| issue.code
            == "PRIKK-DOCTOR-POINTER-INDEX-INTERIOR-DAMAGE"
            || issue.code == "PRIKK-DOCTOR-RECEIVED-INDEX-INTERIOR-DAMAGE"
            || issue.code == "PRIKK-DOCTOR-TRUST-POLICY-INTERIOR-DAMAGE"),
        "an unrelated container's damage must not raise any of these three issues: {report:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
