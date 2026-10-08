//! 0.50.0 Part E2 reports a compacting container's own lost-generation-log deduction through
//! `verify_repository`'s `generation_log_deductions`; 0.50.0 P2b F2 gives each note its own
//! `compact_flag`, naming exactly the `prikk compact` flag that ends it (`prikk compact` alone takes
//! no container and is not a runnable command). Each test here runs the shape 019's own reproduction
//! used -- compact, then a real write landing only in the newly-live slot, then the generation log
//! lost -- and then **runs the command the note's own `compact_flag` names** (dispatched by the flag
//! string itself, not hardcoded per fixture, so a drifted mapping fails here) and confirms the
//! deduction warning for that container is gone afterward.

use prikk_error::Result;

use crate::compact::{compact_received_index, compact_ref_pointer_index, compact_trust_policy};
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{
    FileObjectStore, GenerationLogDeductionNote, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, RepositoryVerification, add_trusted_maintainer, verify_repository,
};

fn objects_target(layout: &RepositoryLayout) -> Result<prikk_object::ObjectId> {
    FileObjectStore::new(layout.clone()).write_object(&signed_empty_block_envelope())
}

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

/// Runs the `prikk compact` flag the deduction note names, dispatched by the flag string itself.
/// Matches this crate's own CLI mapping (`main.rs`'s `compact` arm): a hardcoded per-fixture call
/// here would not catch a drifted `compact_flag`, only a function call with the wrong *string* would.
fn run_named_compact(layout: &RepositoryLayout, flag: &str) -> Result<()> {
    match flag {
        "--pointer-index" => compact_ref_pointer_index(layout).map(|_| ()),
        "--received-index" => compact_received_index(layout).map(|_| ()),
        "--trust-policy" => compact_trust_policy(layout).map(|_| ()),
        other => panic!("generation_log_deduction test: unrecognised compact flag {other:?}"),
    }
}

fn deduction_for<'a>(
    verification: &'a RepositoryVerification,
    container_label: &str,
) -> Option<&'a GenerationLogDeductionNote> {
    verification
        .generation_log_deductions
        .iter()
        .find(|note| note.container_label == container_label)
}

#[test]
fn ref_pointer_index_lost_generation_log_after_a_write_names_pointer_index_flag() -> Result<()> {
    let root = unique_temp_dir("p2b-f2-pointer-index-lost-generation-log");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let store = RefStore::new(layout.clone());

    publish_update(&store, &mut objects, "heads/main", None, 1)?;
    publish_update(&store, &mut objects, "heads/topic", None, 1)?;
    compact_ref_pointer_index(&layout)?;
    // A real write after the switch, landing only in the newly-live slot.
    publish_update(&store, &mut objects, "heads/after", None, 1)?;
    // Lose the record of the switch.
    std::fs::write(layout.ref_pointer_index_generation_log_path(), b"")?;

    let before = verify_repository(&layout)?;
    let note = deduction_for(&before, "the ref pointer index")
        .unwrap_or_else(|| panic!("expected a deduction note for the ref pointer index, got none"));
    assert_eq!(note.compact_flag, "--pointer-index");

    run_named_compact(&layout, note.compact_flag)?;

    let after = verify_repository(&layout)?;
    assert!(
        deduction_for(&after, "the ref pointer index").is_none(),
        "running the named compact flag must end the deduction warning"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn received_index_lost_generation_log_after_a_write_names_received_index_flag() -> Result<()> {
    let root = unique_temp_dir("p2b-f2-received-index-lost-generation-log");
    let layout = RepositoryLayout::init(root.clone())?;
    let target = objects_target(&layout)?;
    let first_state = signed_ref_state_envelope("heads/main", None, target, 1);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/main",
        first_state.object_id(),
    )?;

    compact_received_index(&layout)?;
    // A real write after the switch, landing only in the newly-live slot.
    let second_state = signed_ref_state_envelope("heads/topic", None, target, 1);
    crate::received::write_received_pointer(
        &layout,
        "remotes/heads/topic",
        second_state.object_id(),
    )?;
    std::fs::write(layout.received_index_generation_log_path(), b"")?;

    let before = verify_repository(&layout)?;
    let note = deduction_for(&before, "the received index")
        .unwrap_or_else(|| panic!("expected a deduction note for the received index, got none"));
    assert_eq!(note.compact_flag, "--received-index");

    run_named_compact(&layout, note.compact_flag)?;

    let after = verify_repository(&layout)?;
    assert!(
        deduction_for(&after, "the received index").is_none(),
        "running the named compact flag must end the deduction warning"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn trust_policy_lost_generation_log_after_a_write_names_trust_policy_flag() -> Result<()> {
    let root = unique_temp_dir("p2b-f2-trust-policy-lost-generation-log");
    let layout = RepositoryLayout::init(root.clone())?;
    add_trusted_maintainer(&layout, "first", &"11".repeat(32))?;

    compact_trust_policy(&layout)?;
    // A real write after the switch, landing only in the newly-live slot.
    add_trusted_maintainer(&layout, "second", &"22".repeat(32))?;
    std::fs::write(layout.trust_policy_generation_log_path(), b"")?;

    let before = verify_repository(&layout)?;
    let note = deduction_for(&before, "the trust policy container")
        .unwrap_or_else(|| panic!("expected a deduction note for the trust policy, got none"));
    assert_eq!(note.compact_flag, "--trust-policy");

    run_named_compact(&layout, note.compact_flag)?;

    let after = verify_repository(&layout)?;
    assert!(
        deduction_for(&after, "the trust policy container").is_none(),
        "running the named compact flag must end the deduction warning"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
