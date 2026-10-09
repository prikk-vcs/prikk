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
    add_trusted_maintainer, compact_trust_policy, doctor_repository, rebuild_pointer_index,
    remove_trusted_maintainer, verify_repository,
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

    // G3 (review v1): the way out names the whole `.prikk/` directory, never a single file --
    // restoring the slots alone can leave a generation log naming a slot the restored copy
    // disagrees with.
    let recommendation =
        issue_recommendation(&layout, "PRIKK-DOCTOR-RECEIVED-INDEX-INTERIOR-DAMAGE");
    assert!(
        recommendation.as_deref().is_some_and(|text| {
            text.contains("copy of this repository's own `.prikk/` directory")
                && text.contains("backup")
                && !text.contains("container`")
        }),
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

    // G3: the whole `.prikk/` directory, and trust policy additionally says to re-apply every
    // trust change made since the backup (restoring the policy container alone could re-trust a
    // key that was revoked after the backup).
    let recommendation = issue_recommendation(&layout, "PRIKK-DOCTOR-TRUST-POLICY-INTERIOR-DAMAGE");
    assert!(
        recommendation.as_deref().is_some_and(|text| {
            text.contains("copy of this repository's own `.prikk/` directory")
                && text.contains("backup")
                && text.contains("re-apply every trust change")
        }),
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

/// G1 (review v1): when the pointer index's own interior damage is already known, every
/// `VERIFY-STAGE-INCOMPLETE` recommendation names the damage issue's own code instead of the bare
/// "inspect the failing stage" -- so the two no longer disagree.
#[test]
fn stage_incomplete_recommendations_name_the_damage_issue_when_one_is_set() -> Result<()> {
    let root = unique_temp_dir("p3d-g1-stage-incomplete-names-damage-issue");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    flip_a_body_byte(&layout.ref_pointer_index_slot_path(ContainerSlot::A))?;

    let report = doctor_repository(&layout);
    let stage_incomplete: Vec<_> = report
        .issues
        .iter()
        .filter(|issue| issue.code == "PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE")
        .collect();
    assert!(!stage_incomplete.is_empty(), "{report:?}");
    for issue in &stage_incomplete {
        assert!(
            issue
                .recommendation
                .contains("PRIKK-DOCTOR-POINTER-INDEX-INTERIOR-DAMAGE"),
            "{issue:?}"
        );
        assert!(
            !issue.recommendation.contains("inspect the failing stage"),
            "a recommendation that names the damage issue must not also say the bare \
             'inspect' text: {issue:?}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// G2 (review v1): the one issue that names the real way out is printed first in that state --
/// before the current-branch warning and the stage errors that follow from the identical damage.
#[test]
fn the_damage_issue_is_printed_before_current_branch_and_stage_incomplete() -> Result<()> {
    let root = unique_temp_dir("p3d-g2-damage-issue-printed-first");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    flip_a_body_byte(&layout.ref_pointer_index_slot_path(ContainerSlot::A))?;

    let report = doctor_repository(&layout);
    let damage_index = report
        .issues
        .iter()
        .position(|issue| issue.code == "PRIKK-DOCTOR-POINTER-INDEX-INTERIOR-DAMAGE")
        .expect("the damage issue must be present");
    let current_branch_index = report
        .issues
        .iter()
        .position(|issue| issue.code == "PRIKK-DOCTOR-CURRENT-BRANCH");
    let stage_incomplete_index = report
        .issues
        .iter()
        .position(|issue| issue.code == "PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE");
    if let Some(current_branch_index) = current_branch_index {
        assert!(
            damage_index < current_branch_index,
            "the damage issue ({damage_index}) must print before current-branch \
             ({current_branch_index})"
        );
    }
    if let Some(stage_incomplete_index) = stage_incomplete_index {
        assert!(
            damage_index < stage_incomplete_index,
            "the damage issue ({damage_index}) must print before the first stage-incomplete \
             error ({stage_incomplete_index})"
        );
    }
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Handoff 165 Q1b review, the one item for Q2: `PRIKK-TRUST-POLICY-INVALID` is always and only
/// `load_maintainer_trust_policy` failing. When that failure is the trust policy container's own
/// generation-log ambiguity (already known, pushed before this issue), the recommendation must name
/// `PRIKK-DOCTOR-GENERATION-LOG-AMBIGUOUS` instead of the generic "configure trusted MAINTAINER
/// keys," which does nothing to end an ambiguity no key configuration can resolve.
#[test]
fn publication_trust_invalid_names_the_ambiguous_issue_when_that_is_the_cause() -> Result<()> {
    let root = unique_temp_dir("q2-publication-trust-invalid-names-ambiguous");
    let layout = RepositoryLayout::init(root.clone())?;
    // A real committed Block, so `PublicationTrustVerifier` is actually invoked (verify's own
    // Objects stage calls it per checked record) -- without one, the trust-policy read this test
    // is about never runs at all, and `Objects` simply fails on its own for an unrelated reason.
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());
    publish_update(&store, &mut objects, "heads/main", None, 1)?;

    add_trusted_maintainer(&layout, "k", "11".repeat(32).as_str())?;
    add_trusted_maintainer(&layout, "l", "22".repeat(32).as_str())?;
    remove_trusted_maintainer(&layout, "l")?;
    compact_trust_policy(&layout)?;
    add_trusted_maintainer(&layout, "l", "22".repeat(32).as_str())?;
    compact_trust_policy(&layout)?;
    remove_trusted_maintainer(&layout, "l")?;
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    let report = doctor_repository(&layout);
    let invalid = report
        .issues
        .iter()
        .find(|issue| issue.code == "PRIKK-TRUST-POLICY-INVALID")
        .unwrap_or_else(|| {
            panic!(
                "expected PRIKK-TRUST-POLICY-INVALID, got: {:?}",
                report.issues
            )
        });
    assert!(
        invalid
            .recommendation
            .contains("PRIKK-DOCTOR-GENERATION-LOG-AMBIGUOUS"),
        "{invalid:?}"
    );
    assert!(
        !invalid
            .recommendation
            .contains("configure trusted MAINTAINER keys"),
        "a recommendation that names the ambiguous issue must not also say the generic \
         configure-keys text: {invalid:?}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
