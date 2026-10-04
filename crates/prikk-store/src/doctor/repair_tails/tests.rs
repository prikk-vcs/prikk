//! RFC 164 Rule C: `repair_tails` -- one repair for every tail Rule A defines.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use super::repair_tails;
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectWriter,
    RefPublication, RefStore, RepositoryLayout, WorktreePatchCommitOptions, add_trusted_maintainer,
    commit_worktree_changes_signed,
};

/// A fresh repository with one adopted maintainer -- real trust-key and trust-policy content, so
/// there is something to repair (or, in the clean case, to confirm untouched).
fn repo_with_trust_content(tag: &str) -> RepositoryLayout {
    let layout = RepositoryLayout::init(unique_temp_dir(tag)).expect("init");
    let maintainer =
        Ed25519MaintainerSigner::from_seed("rfc164-repair-tails-maintainer", &[0x85; 32])
            .expect("signer");
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .expect("adopt maintainer");
    layout
}

#[test]
fn a_clean_repository_reports_every_file_untouched() {
    let layout = repo_with_trust_content("rfc164-repair-tails-clean");
    let report = repair_tails(&layout).expect("repair_tails");
    assert_eq!(
        report.files.len(),
        10,
        "the WAL, the pointer index, the seven Rule-A files, and the ref log (RFC 165 R5)"
    );
    for file in &report.files {
        assert_eq!(
            file.truncated_bytes, 0,
            "{}: a clean repository has nothing to repair",
            file.label
        );
        assert!(file.recovery_file.is_none(), "{}", file.label);
    }
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn a_tail_on_trust_keys_is_repaired_and_recovered() {
    let layout = repo_with_trust_content("rfc164-repair-tails-trust-keys-tail");
    let path = layout.trust_key_container_path();
    let before = std::fs::read(&path).unwrap();
    let mut with_tail = before.clone();
    with_tail.extend(vec![0_u8; 100]);
    std::fs::write(&path, &with_tail).unwrap();

    let report = repair_tails(&layout).expect("repair_tails");
    let row = report
        .files
        .iter()
        .find(|file| file.label == "trust keys")
        .expect("trust keys row");
    assert_eq!(row.truncated_bytes, 100);
    let recovery_file = row.recovery_file.as_ref().expect("recovery file recorded");
    let recovery_bytes =
        std::fs::read(layout.prikk_dir().join(recovery_file)).expect("read recovery file");
    assert_eq!(
        recovery_bytes,
        vec![0_u8; 100],
        "exactly the removed bytes, nothing else"
    );

    let after = std::fs::read(&path).unwrap();
    assert_eq!(after, before, "truncated back to exactly the sound prefix");

    // Every other file untouched.
    for file in &report.files {
        if file.label != "trust keys" {
            assert_eq!(
                file.truncated_bytes, 0,
                "{}: unaffected by trust keys' own tail",
                file.label
            );
        }
    }
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn interior_damage_on_one_file_refuses_and_touches_nothing() {
    let layout = repo_with_trust_content("rfc164-repair-tails-interior-damage");
    let path = layout.trust_key_container_path();
    let before_trust_keys = std::fs::read(&path).unwrap();

    // Addendum 1 control 4 ("--repair-tails touching files before its check must redden its
    // rows"): the pointer index also carries a genuine, unrelated TAIL at the same time trust keys
    // carries interior damage. All-or-nothing means this tail must survive the refusal
    // byte-for-byte -- if the all-or-nothing check ever moved after the truncation loop, this tail
    // would be gone even though the repair as a whole still reports an error.
    let pointer_index_path = layout.ref_pointer_index_slot_path(crate::ContainerSlot::A);
    let sound_pointer_entry = crate::refs::PointerIndexEntry {
        ref_name_key: [0x22; 32],
        ref_name: "heads/rfc164-repair-tails-control4".to_string(),
        ref_state_id: prikk_object::ObjectId::from_bytes([0x33; 32]),
    };
    let mut pointer_index_with_tail =
        crate::refs::encode_pointer_index_record(&sound_pointer_entry).expect("encode");
    pointer_index_with_tail.extend(vec![0_u8; 40]);
    std::fs::write(&pointer_index_path, &pointer_index_with_tail).unwrap();

    // Garbage with nothing sound after it would be a tail (RFC 164 Rule A); a genuinely sound
    // trust-key record after the garbage is what keeps this interior damage -- the same shape
    // `hostile_lengths.rs`/`rfc138_trust_read_surface.rs` construct at the CLI level, built here
    // directly via the crate's own (pub(crate)) encoder, since this is a store-internal test.
    let mut damaged = before_trust_keys.clone();
    damaged.extend(vec![0xAB_u8; 200]);
    let sound_entry = crate::trust_index::TrustKeyEntry {
        key_id: "rfc164-repair-tails-sound-entry".to_string(),
        public_key: [0x11; 32],
    };
    damaged.extend(crate::trust_index::encode_trust_key_record(&sound_entry).expect("encode"));
    std::fs::write(&path, &damaged).unwrap();

    let report = repair_tails(&layout);
    assert!(
        report.is_err(),
        "interior damage on trust keys must refuse the whole repair"
    );
    let message = report.unwrap_err().to_string();
    assert!(message.contains("trust keys"), "{message}");

    let after_trust_keys = std::fs::read(&path).unwrap();
    assert_eq!(
        after_trust_keys, damaged,
        "the damaged file itself is untouched"
    );
    let after_pointer_index = std::fs::read(&pointer_index_path).unwrap();
    assert_eq!(
        after_pointer_index, pointer_index_with_tail,
        "an unrelated file's own tail must survive the refusal byte-for-byte -- \
         the all-or-nothing check must run before any truncation, not after"
    );
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn a_second_run_after_repair_is_idempotent() {
    let layout = repo_with_trust_content("rfc164-repair-tails-idempotent");
    let path = layout.trust_key_container_path();
    let mut with_tail = std::fs::read(&path).unwrap();
    with_tail.extend(vec![0_u8; 30]);
    std::fs::write(&path, &with_tail).unwrap();

    let first = repair_tails(&layout).expect("first repair");
    assert_eq!(
        first
            .files
            .iter()
            .find(|file| file.label == "trust keys")
            .unwrap()
            .truncated_bytes,
        30
    );

    let second = repair_tails(&layout).expect("second repair");
    for file in &second.files {
        assert_eq!(
            file.truncated_bytes, 0,
            "{}: a second run has nothing left to repair",
            file.label
        );
    }
    let _ = std::fs::remove_dir_all(layout.root());
}

/// A repository with `heads/main` soundly published -- one real record in the ref log, so the
/// precondition (`ensure_no_incomplete_publication`) agrees nothing leads, and there is something
/// real to append a lead-free tail behind.
fn repo_with_published_main(tag: &str) -> RepositoryLayout {
    let layout = RepositoryLayout::init(unique_temp_dir(tag)).expect("init");
    let mut objects = FileObjectStore::new(layout.clone());
    let target = objects
        .write_object(&signed_empty_block_envelope())
        .expect("write target block");
    let ref_state = signed_ref_state_envelope("heads/main", None, target, 1);
    let ref_state_id = ref_state.object_id();
    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_update: signed_ref_update_envelope("heads/main", None, ref_state_id, target, 1),
        ref_state,
    };
    RefStore::new(layout.clone())
        .publish(&publication)
        .expect("publish heads/main");
    layout
}

/// RFC 165 R5 (§9.2 for the ref log): `--repair-tails` now covers the ref log too, saving what it
/// removes, the same shape every other covered file already has. Mirrors `a_tail_on_trust_keys_is_
/// repaired_and_recovered` exactly.
///
/// **Manual control (handoff §1 item 3, "the ref log removed from `--repair-tails`"), not an
/// automated toggle:** `check_appended_file_tails`'s own `ref_log_tail_status` call was wrapped in
/// `if false { ... }` (reproducing "the ref log is not one of the covered files," pre-R5), and this
/// exact test rerun. It failed at `.expect("ref log row")` (no such row exists at all) -- confirming
/// the wiring is load-bearing. Restored and reverified green (all six tests in this file) before this
/// file was committed.
#[test]
fn a_tail_on_the_ref_log_is_repaired_and_recovered() {
    let layout = repo_with_published_main("rfc165-r5-repair-tails-ref-log-tail");
    let path = layout.ref_log_container_slot_path(crate::ContainerSlot::A);
    let before = std::fs::read(&path).unwrap();
    let mut with_tail = before.clone();
    with_tail.extend(vec![0_u8; 100]);
    std::fs::write(&path, &with_tail).unwrap();

    let report = repair_tails(&layout).expect("repair_tails");
    let row = report
        .files
        .iter()
        .find(|file| file.label == "ref log")
        .expect("ref log row");
    assert_eq!(row.truncated_bytes, 100);
    let recovery_file = row.recovery_file.as_ref().expect("recovery file recorded");
    let recovery_bytes =
        std::fs::read(layout.prikk_dir().join(recovery_file)).expect("read recovery file");
    assert_eq!(
        recovery_bytes,
        vec![0_u8; 100],
        "exactly the removed bytes, nothing else"
    );

    let after = std::fs::read(&path).unwrap();
    assert_eq!(after, before, "truncated back to exactly the sound prefix");

    for file in &report.files {
        if file.label != "ref log" {
            assert_eq!(
                file.truncated_bytes, 0,
                "{}: unaffected by the ref log's own tail",
                file.label
            );
        }
    }
    let _ = std::fs::remove_dir_all(layout.root());
}

/// RFC 165 R5: `--repair-tails` must refuse to touch the ref log's own tail while a *different* ref's
/// pointer leads its log -- truncating would destroy exactly what a future `ref complete` (R4) needs
/// to finish it. Built the same way the round-1 report's own manual crash-state construction was: a
/// real `branch create` through the real `RefStore`, then the log truncated back to its own
/// pre-create length, leaving the pointer naming a state the log does not yet confirm.
#[test]
fn a_tail_while_a_ref_leads_is_not_repaired() {
    use std::io::Write;

    let layout = repo_with_published_main("rfc165-r5-repair-tails-ref-log-lead");
    let path = layout.ref_log_container_slot_path(crate::ContainerSlot::A);

    // Crash mid-log-append for `heads/topic`: its pointer is written (as a real publish would,
    // pointer before log), but only a short, torn fragment reaches the log -- both a genuine lead
    // (the pointer names a state the log's last sound record does not confirm) *and* a physical tail
    // in the same file, the shape `--repair-tails` must not truncate away (RFC 165 R4 completes it).
    let target_target = prikk_object::ObjectId::from_bytes([0x77; 32]);
    crate::refs::write_ref_pointer_candidate_for_test(&layout, "heads/topic", target_target)
        .expect("write pointer candidate");
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(&[0xDE, 0xAD, 0xBE, 0xEF]).unwrap();
    }

    assert!(
        crate::refs::ensure_no_incomplete_publication_except(&layout, None).is_err(),
        "fixture bug: heads/topic must actually be leading"
    );

    let before = std::fs::read(&path).unwrap();
    let result = repair_tails(&layout);
    assert!(
        result.is_err(),
        "a lead must refuse the whole repair, not just skip the ref log's own row"
    );
    let after = std::fs::read(&path).unwrap();
    assert_eq!(after, before, "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(layout.root());
}

fn author_signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-repair-tails-author", &[0x62; 32]).unwrap()
}

/// RFC 166 §13 item 5, end to end: a damaged commit witness over a wholly sound WAL (row 8) is
/// rebuilt from the WAL's own sound records, not refused and not left alone.
/// **Perturb:** skip the rebuild call this test exercises: the witness stays the corrupted bytes and
/// `classify` keeps reading `WitnessDamaged` after a `repair_tails` run that reports success, which
/// this test's own final `assert_eq!` on the verdict catches.
#[test]
fn a_damaged_witness_over_a_sound_wal_is_rebuilt_from_the_wal_not_refused() {
    let layout = RepositoryLayout::init(unique_temp_dir("rfc166-repair-tails-witness-rebuild"))
        .expect("init");
    std::fs::write(layout.root().join("a.txt"), b"hello\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "queued",
        WorktreePatchCommitOptions::file_level(),
        &author_signer(),
    )
    .expect("commit");

    let witness_path = layout
        .active_session_dir(crate::DEFAULT_ACTIVE_NAME)
        .join("witness");
    let wal_before = std::fs::read(
        layout
            .active_session_dir(crate::DEFAULT_ACTIVE_NAME)
            .join("queue.wal"),
    )
    .unwrap();
    let mut corrupted = std::fs::read(&witness_path).unwrap();
    // Flip one byte inside the body (past the 8-byte magic and 2-byte version), the same shape
    // `hostile_lengths.rs`'s own matrix uses elsewhere -- this must decode as `Damaged`, not `Valid`.
    let flip_at = corrupted.len() / 2;
    corrupted[flip_at] ^= 0xFF;
    std::fs::write(&witness_path, &corrupted).unwrap();
    assert!(
        matches!(
            crate::commit_boundary::witness::read_witness(&layout, crate::DEFAULT_ACTIVE_NAME)
                .unwrap(),
            crate::commit_boundary::witness::WitnessState::Damaged(_)
        ),
        "fixture: the flipped byte must actually damage the witness"
    );

    let report = repair_tails(&layout).expect("row 8 does not refuse");
    for file in &report.files {
        assert_eq!(
            file.truncated_bytes, 0,
            "{}: the WAL and every other covered file are already sound; nothing to truncate",
            file.label
        );
    }
    assert_eq!(
        std::fs::read(
            layout
                .active_session_dir(crate::DEFAULT_ACTIVE_NAME)
                .join("queue.wal")
        )
        .unwrap(),
        wal_before,
        "the repair touches the witness, never the WAL's own bytes"
    );

    let rebuilt = std::fs::read(&witness_path).unwrap();
    assert_ne!(
        rebuilt, corrupted,
        "the witness file must actually change, not be left as the damaged bytes"
    );
    let wal = crate::wal::Wal::for_layout(&layout, crate::DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    let owning_ref = crate::read_active_ref_metadata(&layout).unwrap();
    let witness =
        crate::commit_boundary::witness::read_witness(&layout, crate::DEFAULT_ACTIVE_NAME).unwrap();
    let verdict =
        crate::commit_boundary::classification::classify(&layout, &replay, &owning_ref, &witness)
            .unwrap();
    assert_eq!(
        verdict,
        crate::commit_boundary::classification::Verdict::Healthy { last_seq: 1 },
        "the rebuilt witness must agree with the sound WAL it was just derived from, not merely \
         decode -- a rebuild that echoed the damaged bytes' own (corrupted) claims would still fail \
         this"
    );

    let _ = std::fs::remove_dir_all(layout.root());
}

/// RFC 166 K4: `--repair-tails` is a writer (it can rebuild the witness). It takes `ActiveLock` first,
/// the same lock every commit-boundary appender takes -- an ordinary commit attempted while a repair
/// is in progress must refuse as a lock conflict, never interleave with it.
/// **Perturb:** have `repair_tails` take the active lock after its own witness-rebuild step instead of
/// before every read: the commit below would then succeed while the repair is still "in progress",
/// and this test's own `assert!(matches!(.., LockConflict))` goes red.
#[test]
fn repair_tails_races_an_ordinary_commit_under_the_shared_active_lock() {
    let layout =
        RepositoryLayout::init(unique_temp_dir("rfc166-repair-tails-k4-race")).expect("init");
    std::fs::write(layout.root().join("a.txt"), b"hello\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        "heads/main",
        "queued",
        WorktreePatchCommitOptions::file_level(),
        &author_signer(),
    )
    .expect("commit");

    let held = crate::lock::ActiveLock::acquire(&layout, crate::DEFAULT_ACTIVE_NAME).unwrap();
    let raced = repair_tails(&layout);
    assert!(
        matches!(raced, Err(prikk_error::PrikkError::LockConflict(_))),
        "repair_tails racing a held ActiveLock must refuse as a lock conflict, got {raced:?}"
    );
    drop(held);

    // One writer at a time, and nothing is lost: the repair succeeds once the lock is free.
    repair_tails(&layout).expect("repair_tails succeeds once the active lock is free");

    let _ = std::fs::remove_dir_all(layout.root());
}
