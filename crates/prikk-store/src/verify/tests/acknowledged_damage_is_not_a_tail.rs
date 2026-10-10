//! 0.51.0 step 1 Part B item 1 (020's grade): a record the commit witness explains as acknowledged
//! damage is complete, written, and already named by the witness -- not an unresolved crash shape.
//! It must not be counted as trailing partial bytes at all, not merely have its warning sentence
//! suppressed (0.50.0 step 1 A6 item 1's own fix, which left the count itself unchanged).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use crate::commit_boundary::classification::Verdict;
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions, verify_repository};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc165-step1-b1-ack-damage", &[0x93_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "b1",
        WorktreePatchCommitOptions::file_level(),
        &signer(),
    )
    .unwrap();
}

fn classify_now(layout: &RepositoryLayout) -> Verdict {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let owning_ref = crate::commit_boundary::active::read_active_ref_metadata(layout).unwrap();
    let witness =
        crate::commit_boundary::witness::read_witness(layout, DEFAULT_ACTIVE_NAME).unwrap();
    crate::commit_boundary::classification::classify(layout, &replay, &owning_ref, &witness)
        .unwrap()
}

/// The queued record's own checksum is flipped (the exact fixture
/// `doctor::discard_damaged_commits::tests::row4_discards_the_damaged_record_and_rebuilds_the_witness`
/// uses) -- the witness still names the now-unsound seq, so `classify` reads row 4,
/// `AcknowledgedDamage`, confirmed below before this test's own assertions.
fn build_acknowledged_damage(layout: &RepositoryLayout) {
    commit(layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 },
        "fixture bug: must read as acknowledged damage"
    );
}

#[test]
fn acknowledged_damage_is_not_counted_as_trailing_partial_wal_bytes() {
    let root = unique_temp_dir("rfc165-step1-b1-ack-damage");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    build_acknowledged_damage(&layout);

    // The raw WAL replay, independent of `verify`, really does have a nonzero tail here --
    // confirming the fixture produces the shape this test is actually about, not a vacuous pass.
    let raw_replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    assert!(
        raw_replay.trailing_partial_bytes > 0,
        "fixture bug: rule 3 must fold the damaged record into the tail"
    );

    let report = verify_repository(&layout).unwrap();
    assert_eq!(
        report.commit_witness_verdict,
        Some(Verdict::AcknowledgedDamage { witnessed_seq: 1 })
    );
    assert_eq!(
        report.trailing_partial_wal_bytes,
        Some(0),
        "an acknowledged damaged record is not a tail, so it must not be counted as one"
    );
    assert!(!report.has_trailing_partial_wal());

    let _ = std::fs::remove_dir_all(&root);
}

/// The control: an ordinary, unexplained crash tail (no witness at all, since none is ever
/// written here) still counts its own bytes -- this round changes only the acknowledged-damage
/// case, not rule 3 itself.
#[test]
fn an_unexplained_crash_tail_still_counts_its_bytes() {
    use crate::test_gates::test_support::signed_patch_envelope;

    let root = unique_temp_dir("rfc165-step1-b1-crash-tail-control");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    crate::commit_boundary::active::write_active_ref_metadata(&layout, "heads/main").unwrap();
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope()).unwrap();
    let mut bytes = std::fs::read(wal.path()).unwrap();
    bytes.extend_from_slice(&[0_u8; 30]);
    std::fs::write(wal.path(), &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::NoWitness {
            wal_otherwise_sound: true,
            stale: false
        },
        "fixture bug: must read as a genuine, unexplained crash tail"
    );

    let report = verify_repository(&layout).unwrap();
    assert!(
        report.trailing_partial_wal_bytes.is_some_and(|n| n != 0),
        "an ordinary crash tail must still be counted"
    );
    assert!(report.has_trailing_partial_wal());

    let _ = std::fs::remove_dir_all(&root);
}
