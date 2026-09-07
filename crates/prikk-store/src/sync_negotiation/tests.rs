//! RFC 116 stage 2 handoff §5: rows 1 (state purity, across all three functions), 5 (asymmetric ref
//! sets, both directions reported), and 6 (the delta is exactly the set difference) -- the
//! properties that need more than one repository or more than one artifact type to demonstrate, so
//! they live at this top level rather than inside `summary::tests` or `have_list::tests`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;

use prikk_error::Result;
use prikk_object::ObjectId;

use super::{
    DEFAULT_HAVE_LIST_MAX_PATCH_COUNT, DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
    DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT, DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES, build_have_list,
    build_sync_summary, compute_sync_delta, decode_have_list, decode_sync_summary,
};
use crate::RefStore;
use crate::sync_negotiation::sync_test_support::{
    all_container_bytes, cleanup, fresh_repo, publish_branch,
};
use crate::trust::load_maintainer_trust_policy_or_empty;

/// §5 row 1: reading a summary, a have-list, or the delta changes no state at all. Checked against
/// **every** persisted object type's own container bytes
/// (`crate::foundation::layout::persisted_object_types`) -- not only Block/RefState, so a mutation that writes
/// through any object type (e.g. a stray Blob) is still caught -- the ref pointer index (via the
/// ref's own current RefState id), and the trust policy: the same properties
/// `seal_from_accepted`'s own no-op test pins.
#[test]
fn reading_summary_have_list_and_delta_changes_no_state() -> Result<()> {
    let layout = fresh_repo("sync-negotiation-row1")?;
    publish_branch(
        &layout,
        "heads/main",
        vec![ObjectId::from_bytes([0x51; 32])],
    )?;

    let containers_before = all_container_bytes(&layout)?;
    let ref_pointer_before =
        RefStore::new(layout.clone()).read_current_ref_state_id("heads/main")?;
    let trust_before = load_maintainer_trust_policy_or_empty(&layout)?;

    let summary_bytes = build_sync_summary(&layout)?;
    let _entries = decode_sync_summary(
        &summary_bytes,
        DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES,
        DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT,
    )?;
    let have_list_bytes = build_have_list(&layout, "heads/main")?;
    let have_list = decode_have_list(
        &have_list_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )?;
    let _delta = compute_sync_delta(&layout, &have_list)?;

    let containers_after = all_container_bytes(&layout)?;
    let ref_pointer_after =
        RefStore::new(layout.clone()).read_current_ref_state_id("heads/main")?;
    let trust_after = load_maintainer_trust_policy_or_empty(&layout)?;

    assert_eq!(
        containers_before, containers_after,
        "no persisted object container may change"
    );
    assert_eq!(
        ref_pointer_before, ref_pointer_after,
        "the ref pointer must not move"
    );
    assert_eq!(
        trust_before, trust_after,
        "the trust policy must not change"
    );
    cleanup(&layout);
    Ok(())
}

/// §5 row 5: a ref present in one repository and absent in the other is reported, not refused --
/// checked in both directions from two summaries built independently.
#[test]
fn asymmetric_ref_sets_are_reported_in_both_directions() -> Result<()> {
    let layout_a = fresh_repo("sync-negotiation-row5-a")?;
    publish_branch(
        &layout_a,
        "heads/main",
        vec![ObjectId::from_bytes([0x61; 32])],
    )?;
    publish_branch(
        &layout_a,
        "heads/only-a",
        vec![ObjectId::from_bytes([0x62; 32])],
    )?;

    let layout_b = fresh_repo("sync-negotiation-row5-b")?;
    publish_branch(
        &layout_b,
        "heads/main",
        vec![ObjectId::from_bytes([0x61; 32])],
    )?;
    publish_branch(
        &layout_b,
        "heads/only-b",
        vec![ObjectId::from_bytes([0x63; 32])],
    )?;

    let entries_a = decode_sync_summary(
        &build_sync_summary(&layout_a)?,
        DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES,
        DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT,
    )?;
    let entries_b = decode_sync_summary(
        &build_sync_summary(&layout_b)?,
        DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES,
        DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT,
    )?;
    let names_a: BTreeSet<&str> = entries_a
        .iter()
        .map(|entry| entry.ref_name.as_str())
        .collect();
    let names_b: BTreeSet<&str> = entries_b
        .iter()
        .map(|entry| entry.ref_name.as_str())
        .collect();

    assert!(names_a.contains("heads/only-a") && !names_b.contains("heads/only-a"));
    assert!(names_b.contains("heads/only-b") && !names_a.contains("heads/only-b"));
    assert!(names_a.contains("heads/main") && names_b.contains("heads/main"));

    // The asymmetry is reported through ordinary presence/absence -- neither build nor decode
    // erred over either repository's own, differently-shaped ref set.
    cleanup(&layout_a);
    cleanup(&layout_b);
    Ok(())
}

/// §5 row 6: the delta is exactly the set difference -- a patch only the sender has, a patch only
/// the receiver has, and a patch both share. Only the sender-only id must come back.
#[test]
fn the_delta_is_exactly_the_set_difference() -> Result<()> {
    let shared = ObjectId::from_bytes([0x71; 32]);
    let sender_only = ObjectId::from_bytes([0x72; 32]);
    let receiver_only = ObjectId::from_bytes([0x73; 32]);

    let sender_layout = fresh_repo("sync-negotiation-row6-sender")?;
    publish_branch(&sender_layout, "heads/main", vec![shared, sender_only])?;

    let receiver_layout = fresh_repo("sync-negotiation-row6-receiver")?;
    publish_branch(&receiver_layout, "heads/main", vec![shared, receiver_only])?;

    let have_list_bytes = build_have_list(&receiver_layout, "heads/main")?;
    let have_list = decode_have_list(
        &have_list_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )?;

    let delta = compute_sync_delta(&sender_layout, &have_list)?;
    assert_eq!(delta, vec![sender_only]);

    cleanup(&sender_layout);
    cleanup(&receiver_layout);
    Ok(())
}

/// A ref the sender does not hold at all produces an empty delta -- there is nothing to send, and
/// this must not be confused with the delta-computation refusing.
#[test]
fn a_ref_the_sender_does_not_hold_produces_an_empty_delta() -> Result<()> {
    let sender_layout = fresh_repo("sync-negotiation-sender-absent")?;
    let receiver_layout = fresh_repo("sync-negotiation-receiver-absent")?;
    publish_branch(
        &receiver_layout,
        "heads/main",
        vec![ObjectId::from_bytes([0x81; 32])],
    )?;

    let have_list_bytes = build_have_list(&receiver_layout, "heads/main")?;
    let have_list = decode_have_list(
        &have_list_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )?;

    let delta = compute_sync_delta(&sender_layout, &have_list)?;
    assert!(delta.is_empty());
    cleanup(&sender_layout);
    cleanup(&receiver_layout);
    Ok(())
}
