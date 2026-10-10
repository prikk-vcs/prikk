//! 0.51.0 step 1 Part B item 4 (021's grade): a compaction cut short between truncating its own
//! target (retired) slot and finishing the append leaves a torn tail there -- the live slot and the
//! generation log are both untouched and sound, so no reader uses this slot and nothing else reports
//! it, until a later crash also loses the generation log. `verify`/`doctor` now read the retired
//! slot directly, independent of the live-slot-only path every ordinary reader and writer take.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::foundation::layout::ContainerSlot;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{RepositoryLayout, add_trusted_maintainer, compact_trust_policy, verify_repository};

fn public_key_hex(seed: &[u8; 32]) -> String {
    prikk_crypto::Ed25519KeyPair::from_seed(seed)
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The first compaction (B live by E/E2's own default) succeeds and fully completes normally. The
/// second is then also let run fully to completion -- real content, built by a real command, never
/// written by hand -- which durably writes slot A and advances the generation log to name A. Only
/// *after* both real writes exist on disk does this fixture reach back and construct the crash
/// shape directly: slot A is truncated to a prefix of its own real bytes (a torn tail, the same
/// shape `append_torn_prefix`/`crash_branch_create_with_torn_tail` use elsewhere for exactly this
/// reason -- no failpoint here produces a partial write; `durable_append`'s own failpoint is
/// all-or-nothing), and the generation log is truncated back to its own length right after the
/// *first* compaction -- restoring "B is still live" exactly as the crash would have left it: the
/// slot write interrupted, the generation record for it never reached.
fn build_cut_short_compaction(layout: &RepositoryLayout) {
    add_trusted_maintainer(layout, "base", &public_key_hex(&[61_u8; 32])).unwrap();
    compact_trust_policy(layout).unwrap(); // ordinary: B live
    let generation_log_path = layout.trust_policy_generation_log_path();
    let generation_log_after_first = std::fs::read(&generation_log_path).unwrap();

    add_trusted_maintainer(layout, "second", &public_key_hex(&[62_u8; 32])).unwrap();
    compact_trust_policy(layout).unwrap(); // ordinary: A live, built by a real command

    let slot_a_path = layout.trust_policy_container_slot_path(ContainerSlot::A);
    let slot_a_full_bytes = std::fs::read(&slot_a_path).unwrap();
    assert!(
        slot_a_full_bytes.len() > 2,
        "fixture bug: the real record must be long enough to truncate mid-record"
    );
    let torn_len = slot_a_full_bytes.len() / 2;
    std::fs::write(&slot_a_path, &slot_a_full_bytes[..torn_len]).unwrap();
    std::fs::write(&generation_log_path, &generation_log_after_first).unwrap();
}

#[test]
fn a_cut_short_compaction_reports_the_retired_slots_own_tail() {
    let root = unique_temp_dir("rfc165-step1-b4-retired-slot-tail");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    build_cut_short_compaction(&layout);

    // Confirm the fixture's own premise directly: the generation log still names B, untouched by
    // the interrupted second attempt, and slot A (the cut-short target) now carries a real tail.
    let raw_a = crate::trust_index::decode_trust_policy_records(
        &std::fs::read(layout.trust_policy_container_slot_path(ContainerSlot::A)).unwrap(),
    )
    .unwrap();
    assert!(
        raw_a.trailing_partial_bytes > 0,
        "fixture bug: the cut-short append must leave a torn tail on slot A"
    );
    assert!(
        crate::load_maintainer_trust_policy(&layout).is_ok(),
        "fixture bug: an ordinary read must still resolve cleanly (the generation log names B, \
         untouched by the reverted second compaction, and never reads the torn slot A at all)"
    );

    let report = verify_repository(&layout).unwrap();
    assert_eq!(report.container_interior_damage.trust_policy, None);
    assert_eq!(report.container_generation_ambiguity.trust_policy, None);
    assert!(
        report.generation_log_deductions.is_empty(),
        "the generation log still names a slot cleanly; nothing here was deduced"
    );
    let note = report
        .retired_slot_tails
        .iter()
        .find(|note| note.container_label == "the trust policy container")
        .expect("the retired slot's own tail must be reported");
    assert_eq!(note.trailing_partial_bytes, raw_a.trailing_partial_bytes);
    assert_eq!(note.tail_offset, raw_a.tail_offset);
    assert_eq!(note.compact_flag, "--trust-policy");

    // The way out, proved by following it: the next ordinary compaction overwrites the slot, and
    // the warning disappears.
    compact_trust_policy(&layout).unwrap();
    let after = verify_repository(&layout).unwrap();
    assert!(
        after
            .retired_slot_tails
            .iter()
            .all(|note| note.container_label != "the trust policy container"),
        "the next compaction must overwrite the torn slot and end the warning"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The control: an ordinary repository, with no cut-short compaction, reports no retired-slot
/// tail at all -- this is not a check that fires on every compacted container.
#[test]
fn an_ordinary_compaction_reports_no_retired_slot_tail() {
    let root = unique_temp_dir("rfc165-step1-b4-control");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    add_trusted_maintainer(&layout, "base", &public_key_hex(&[63_u8; 32])).unwrap();
    compact_trust_policy(&layout).unwrap();

    let report = verify_repository(&layout).unwrap();
    assert!(report.retired_slot_tails.is_empty());

    let _ = std::fs::remove_dir_all(&root);
}
