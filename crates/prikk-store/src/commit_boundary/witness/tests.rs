#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{WitnessState, clear_witness, read_witness};
use crate::Ed25519AuthorSigner;
use crate::commit_boundary::worktree_patch::{
    WorktreePatchCommitOptions, commit_worktree_changes_signed,
    commit_worktree_changes_with_generator,
};
use crate::foundation::fsutil::{
    TestFailPoint, clear_failpoint_for_test, fail_after_for_test, failpoint_armed_for_test,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::node::node_id_gen::NodeIdGenerator;
use crate::test_gates::source_classification::is_test_source;
use crate::test_gates::test_support::unique_temp_dir;

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-d2-witness", &[0x71_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_with_generator(
        layout,
        "heads/main",
        "d2",
        WorktreePatchCommitOptions::file_level(),
        &mut NodeIdGenerator::production(),
        &signer(),
    )
    .unwrap();
}

#[test]
#[cfg(unix)]
fn a_symlinked_witness_path_refuses_the_same_way_every_other_session_file_does() {
    // RFC 166 D2 item 3: the witness is written only through the anchored `MutationRoot`
    // primitives, which refuse to follow a symlink at the final path component for *any* file --
    // the same protection every other session file (`ref-name`, `declarations`, `active.lock`)
    // already has. Confirmed here for `witness` specifically, not assumed from the shared primitive
    // alone.
    let root = unique_temp_dir("rfc166-d2-symlink");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    std::fs::remove_file(&witness_path).unwrap();
    let elsewhere = unique_temp_dir("rfc166-d2-symlink-target");
    std::os::unix::fs::symlink(&elsewhere, &witness_path).unwrap();
    std::fs::write(layout.root().join("b.txt"), b"two").unwrap();
    let result = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "d2",
        WorktreePatchCommitOptions::file_level(),
        &mut NodeIdGenerator::production(),
        &signer(),
    );
    assert!(
        result.is_err(),
        "a symlinked witness path must refuse the write, not follow it elsewhere"
    );
    assert!(
        !elsewhere.join("witness").exists(),
        "nothing must have been written through the symlink"
    );
    std::fs::remove_file(&witness_path).ok();
    std::fs::remove_dir_all(&root).ok();
    std::fs::remove_dir_all(&elsewhere).ok();
}

#[test]
fn absent_before_any_commit() {
    let root = unique_temp_dir("rfc166-d2-absent");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    assert_eq!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Absent
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn first_commit_witnesses_seq_1_with_no_prior_tip() {
    let root = unique_temp_dir("rfc166-d2-first-commit");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => {
            assert_eq!(record.ref_name, "heads/main");
            assert_eq!(record.last_seq, 1);
            assert_eq!(
                record.ref_tip_at_write, None,
                "heads/main has never been sealed yet"
            );
        }
        other => panic!("expected Valid, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn second_commit_advances_the_witness_and_folds_the_running_hash() {
    let root = unique_temp_dir("rfc166-d2-second-commit");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let first = match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record,
        other => panic!("expected Valid, got {other:?}"),
    };
    commit(&layout, "b.txt", b"two");
    let second = match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record,
        other => panic!("expected Valid, got {other:?}"),
    };
    assert_eq!(second.last_seq, 2);
    assert_ne!(
        second.running_hash, first.running_hash,
        "the running hash must advance"
    );
    assert_ne!(
        second.frame_hash, first.frame_hash,
        "a different record's own frame hash"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// RFC 166 D4 (row 10): `verify_running_hash` catches a substituted *earlier* record -- the shape W2
/// (the last record's own frame hash alone, which D3's `identity_matches` already checks) cannot see,
/// since a record's own frame hash never depends on any record before it.
/// **Perturb:** compare `witness_record.frame_hash` against the forged replay's own last record
/// instead of recomputing the running hash: it would still agree (the last record is untouched here),
/// and this test's own `assert!(!...)` goes red.
#[test]
fn verify_running_hash_catches_a_substituted_earlier_record() {
    let root = unique_temp_dir("rfc166-d4-running-hash");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    commit(&layout, "c.txt", b"three");
    let witness = match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record,
        other => panic!("expected Valid, got {other:?}"),
    };
    assert_eq!(witness.last_seq, 3);

    let wal = crate::wal::Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let genuine = wal.replay().unwrap();
    assert!(
        super::verify_running_hash(&witness, &genuine).unwrap(),
        "the genuine, untouched replay must agree with its own witness"
    );

    // Substitute record 2's own envelope for record 3's (same shape: a real, decodable, signed
    // envelope -- just not the one this witness actually acknowledged at that position). Record 3's
    // own envelope, the last one, is left byte-for-byte alone: W2 (the last record's own frame hash)
    // would see nothing wrong here.
    let mut forged = genuine.records.clone();
    forged[1].envelope = genuine.records[2].envelope.clone();
    let forged_replay = crate::wal::WalReplay {
        records: forged,
        trailing_partial_bytes: 0,
        record_outcomes: Vec::new(),
    };
    assert!(
        !super::verify_running_hash(&witness, &forged_replay).unwrap(),
        "a substituted earlier record must disagree with the witness's own running hash"
    );

    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn clear_returns_to_absent() {
    let root = unique_temp_dir("rfc166-d2-clear");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    assert_eq!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Absent
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_flipped_byte_is_damaged_not_absent() {
    let root = unique_temp_dir("rfc166-d2-damaged");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Damaged(_) => {}
        other => panic!("expected Damaged, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn a_commit_after_a_legacy_queue_with_no_witness_folds_the_whole_queue_never_a_false_alarm() {
    // RFC 166 §13 item 2's own named defect in the design round's own prototype: "any queue begun or
    // extended by 0.48.0 would read as 'a substituted earlier record', a false alarm that refuses
    // commits." Simulated here by removing the witness after real commits (indistinguishable from
    // 0.48.0 having written them at all -- D3 item 1's own point exactly) rather than shelling out to
    // the real 0.48.0 binary, since the code path cannot tell the two apart either.
    let root = unique_temp_dir("rfc166-d2-legacy-queue");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one"); // simulates 0.48.0's own first commit
    commit(&layout, "b.txt", b"two"); // simulates 0.48.0's own second commit
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    std::fs::remove_file(&witness_path).unwrap(); // 0.48.0 never wrote one at all
    assert_eq!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Absent
    );
    commit(&layout, "c.txt", b"three"); // this binary's own first commit over the legacy queue
    let witnessed = match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record,
        other => panic!("expected Valid, got {other:?}"),
    };
    assert_eq!(witnessed.last_seq, 3);
    let wal = crate::wal::Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    assert_eq!(
        replay.records.len(),
        3,
        "all three records, old and new, are sound"
    );
    let mut expected = [0u8; 32];
    for record in &replay.records {
        let frame_hash = crate::wal::record_frame_checksum(record).unwrap();
        let mut preimage = Vec::with_capacity(64);
        preimage.extend_from_slice(&expected);
        preimage.extend_from_slice(&frame_hash);
        expected = prikk_hash::sha256(&preimage);
    }
    assert_eq!(
        witnessed.running_hash, expected,
        "the running hash must cover the whole queue (seq 1-3), including the two records this \
         binary never witnessed itself -- not just the one new record folded onto an empty prior \
         hash, which is exactly the false-alarm shape the design round's own prototype had"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn an_appended_commit_after_a_damaged_witness_folds_the_whole_queue_from_scratch() {
    // RFC 166 §13 item 2's own fix: a witness that does not fully cover the queue (damaged, here)
    // folds every sound record after the last *covered* one -- which is "none", so it covers the
    // whole queue, as a first witness over a legacy queue must.
    let root = unique_temp_dir("rfc166-d2-fold-from-damaged");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    let healthy_running_hash = match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record.running_hash,
        other => panic!("expected Valid, got {other:?}"),
    };
    let path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let mut bytes = std::fs::read(&path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&path, bytes).unwrap();
    assert!(matches!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Damaged(_)
    ));
    commit(&layout, "c.txt", b"three");
    let recovered = match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record,
        other => panic!("expected Valid, got {other:?}"),
    };
    assert_eq!(recovered.last_seq, 3);
    assert_ne!(
        recovered.running_hash, healthy_running_hash,
        "the hash must be recomputed over records 1-3, not folded onto the damaged record's own \
         (unreadable) value"
    );
    // Confirm independently: folding records 1-3 from scratch gives the same answer the real append
    // just produced, proving the fold-from-scratch path was actually taken (not merely "some other
    // value").
    let wal = crate::wal::Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    let mut expected = [0u8; 32];
    for record in &replay.records {
        let frame_hash = crate::wal::record_frame_checksum(record).unwrap();
        let mut preimage = Vec::with_capacity(64);
        preimage.extend_from_slice(&expected);
        preimage.extend_from_slice(&frame_hash);
        expected = prikk_hash::sha256(&preimage);
    }
    assert_eq!(recovered.running_hash, expected);
    std::fs::remove_dir_all(&root).ok();
}

fn repo_root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(std::path::Path::parent)
        .expect("prikk-store's manifest dir has a workspace root two levels up")
        .to_path_buf()
}

#[test]
fn append_patch_and_witness_is_the_only_caller_of_wal_append_patch() {
    // RFC 166 §13 item 1: `Wal::append_patch` is reachable from nowhere else. A fourth appender
    // calling it directly, anywhere else in production (non-test) source, fails this test.
    //
    // 0.53.0 step 3B (M6): `append_patch_and_witness` now calls the lower-level
    // `append_patch_reporting` directly (the sibling that also hands back the records its own
    // `replay()` already read, so the witness's running hash never needs a second, redundant
    // decode of the whole queue) -- `append_patch` itself survives only as a thin wrapper around
    // it, kept for every other (test) caller. Either name reaching production code outside
    // `witness.rs` would be the same bypass this test exists to catch, so both are checked.
    let src = repo_root().join("crates/prikk-store/src");
    let mut non_test_callers = Vec::new();
    visit_rs_files(&src, &mut |path, contents| {
        let relative = path.strip_prefix(&src).unwrap_or(path);
        if is_test_source(relative) {
            return;
        }
        // `wal.rs` is where both methods are defined; `append_patch`'s own one-line delegation to
        // `append_patch_reporting` is the method's own internal structure, not a second caller
        // reaching it from elsewhere -- excluded from this scan for that reason, not to hide it.
        if relative == std::path::Path::new("wal.rs") {
            return;
        }
        for (line_number, line) in contents.lines().enumerate() {
            if line.contains(".append_patch(") || line.contains(".append_patch_reporting(") {
                non_test_callers.push(format!(
                    "{}:{}: {}",
                    path.display(),
                    line_number + 1,
                    line.trim()
                ));
            }
        }
    });
    assert_eq!(
        non_test_callers.len(),
        1,
        "expected exactly one production caller of Wal::append_patch/append_patch_reporting \
         (inside witness.rs itself); found: {non_test_callers:#?}"
    );
    assert!(non_test_callers[0].contains("witness.rs"));
}

fn visit_rs_files(dir: &std::path::Path, visit: &mut impl FnMut(&std::path::Path, &str)) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit_rs_files(&path, visit);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            if let Ok(contents) = std::fs::read_to_string(&path) {
                visit(&path, &contents);
            }
        }
    }
}

/// **RFC 168 §3.3, torn at the witness's clear:** the clear overwrites nothing and truncates to zero, so a failed truncate
/// leaves the old record whole. The witness must read as its record, never as the empty "cleared" state, which would read as
/// no witness at all.
#[test]
fn a_witness_clear_torn_at_its_truncate_still_reads_as_its_record_never_as_cleared() {
    use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
    let root = unique_temp_dir("rfc168-witness-torn-clear");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    assert!(matches!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Valid(_)
    ));

    fail_once_for_test(TestFailPoint::Truncate);
    assert!(clear_witness(&layout, DEFAULT_ACTIVE_NAME).is_err());
    clear_failpoint_for_test();
    assert!(
        matches!(
            read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
            WitnessState::Valid(_)
        ),
        "a torn clear keeps the record whole"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// **RFC 168 §3.3, torn at the witness's write:** the new bytes are written over the old from offset 0, and the length is set
/// after. A shorter record whose length step fails leaves its own prefix over the old tail: a damaged witness, which RFC 166
/// row 8 rebuilds from the WAL. It is never the empty "cleared" state.
#[test]
fn a_shorter_witness_torn_before_its_length_is_set_reads_as_damaged_never_as_absent() {
    use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
    let root = unique_temp_dir("rfc168-witness-torn-write");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let path = layout
        .repository_relative(
            &layout
                .active_session_dir(DEFAULT_ACTIVE_NAME)
                .join("witness"),
        )
        .unwrap();
    let whole = std::fs::read(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
    )
    .unwrap();
    let mut shorter = whole[..whole.len() / 2].to_vec();
    shorter[0] ^= 0xFF;

    fail_once_for_test(TestFailPoint::Truncate);
    let torn = super::write_witness_in_place(layout.repository_mutation_root(), &path, &shorter);
    clear_failpoint_for_test();
    assert!(torn.is_err(), "the length step fails");
    assert!(
        matches!(
            read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
            WitnessState::Damaged(_)
        ),
        "the shorter prefix over the old tail is damage, not a cleared witness"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// **RFC 168 §6, residual (c), lost-rename run:** a repository from before 0.49.0 has a WAL and no witness. The witness's first
/// creation is the new name a Windows power loss can lose; its loss falls back to RFC 166 rule 3, the classification a
/// pre-0.49.0 session always had. The next commit creates the witness, and `verify` is clean.
#[test]
fn a_lost_first_witness_creation_falls_back_to_rule_3_and_the_next_commit_recreates_it() {
    let root = unique_temp_dir("rfc168-witness-first-creation-lost");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let witness = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    // The lost creation: the name is gone, the WAL keeps its record.
    std::fs::remove_file(&witness).unwrap();
    assert!(matches!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Absent
    ));

    commit(&layout, "b.txt", b"two");
    assert!(matches!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Valid(_)
    ));
    let verdict = crate::verify_repository(&layout).unwrap();
    assert!(
        !verdict.has_item_failure(),
        "verify is clean after the fall-back"
    );
    let _ = std::fs::remove_dir_all(root);
}

// 0.53.0 step 3B (M6): `append_patch_and_witness` now folds over the records its own, single
// `append_patch_reporting` call already read, instead of a second, redundant `wal.replay()`
// (`commit_boundary::witness::append_patch_and_witness`). Controls 1-4 below confirm this real
// implementation change, not merely the design round's own prototype: the running hash it writes
// is exactly what an independent, from-scratch replay computes, for every witness state the
// handoff names, and the production code genuinely still folds every record a behind witness does
// not cover (not only the newest one).

/// An independent, from-scratch recomputation of the running hash over every sound record
/// currently in the WAL, through `last_seq` -- the same check `verify_running_hash` performs
/// internally, done here directly against the real WAL so a test can compare it to whatever the
/// real `append_patch_and_witness` call just wrote, without relying on that function's own
/// correctness to verify itself.
fn independent_running_hash(layout: &RepositoryLayout, through_seq: u64) -> [u8; 32] {
    let wal = crate::wal::Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    let mut running = [0u8; 32];
    for record in &replay.records {
        if record.seq > through_seq {
            break;
        }
        let frame_hash = crate::wal::record_frame_checksum(record).unwrap();
        let mut preimage = Vec::with_capacity(64);
        preimage.extend_from_slice(&running);
        preimage.extend_from_slice(&frame_hash);
        running = prikk_hash::sha256(&preimage);
    }
    running
}

fn current_witness(layout: &RepositoryLayout) -> super::WitnessRecord {
    match read_witness(layout, DEFAULT_ACTIVE_NAME).unwrap() {
        WitnessState::Valid(record) => record,
        other => panic!("expected Valid, got {other:?}"),
    }
}

/// Control 1 (valid and current): the ordinary case every `commit()` call in this file already
/// exercises -- stated explicitly here, once, as its own control rather than left implicit in
/// every other test's own incidental use of `commit()`.
#[test]
fn m6_witness_valid_and_current_matches_an_independent_replay() {
    let root = unique_temp_dir("m6-valid-current");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    commit(&layout, "c.txt", b"three");
    let witness = current_witness(&layout);
    assert_eq!(witness.last_seq, 3);
    assert_eq!(
        witness.running_hash,
        independent_running_hash(&layout, 3),
        "the witness the real, single-replay append path wrote must equal an independent, \
         from-scratch replay's own running hash"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Control 1 (valid and behind): a witness that genuinely covers only a *prefix* of the queue (not
/// absent, not damaged -- a real, checksummed record for an earlier seq), simulating a witness this
/// binary wrote once and then fell behind by one record (for example, a `test-support` append that
/// advanced the WAL without going through `append_patch_and_witness` at all -- the same shape an
/// older binary's own commit leaves, one step short of the "no witness at all" legacy case the
/// existing `a_commit_after_a_legacy_queue_with_no_witness_folds_the_whole_queue_never_a_false_alarm`
/// test already covers). The next real commit must fold **both** the record the witness missed and
/// its own new one, not just its own.
#[test]
fn m6_witness_valid_and_behind_folds_every_record_it_missed() {
    let root = unique_temp_dir("m6-valid-behind");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    // Roll the witness back to claim only seq 1, as if this binary's own witness write for seq 2
    // had never happened (a crash between the durable WAL append and the witness write, RFC 166
    // D2's own documented gap -- "acknowledged" is decided by the witness, and seq 2 here is simply
    // not yet acknowledged by it).
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let wal = crate::wal::Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay().unwrap();
    let seq1 = replay.records.iter().find(|r| r.seq == 1).unwrap();
    let mut record = super::decode_for_test(&std::fs::read(&witness_path).unwrap()).unwrap();
    record.last_seq = 1;
    record.patch_id = seq1.envelope.object_id();
    record.frame_hash = crate::wal::record_frame_checksum(seq1).unwrap();
    record.running_hash = independent_running_hash(&layout, 1);
    std::fs::write(&witness_path, super::encode_for_test(&record)).unwrap();
    assert_eq!(
        current_witness(&layout).last_seq,
        1,
        "the witness is genuinely behind now, not absent or damaged"
    );

    commit(&layout, "c.txt", b"three");
    let recovered = current_witness(&layout);
    assert_eq!(recovered.last_seq, 3);
    assert_eq!(
        recovered.running_hash,
        independent_running_hash(&layout, 3),
        "the fold must cover seq 2 (missed by the behind witness) and seq 3 (the new commit), not \
         only seq 3 alone"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Control 1 (idempotent retry): submitting the identical, already-queued envelope again must
/// leave the witness untouched -- no new record, same seq, same running hash -- through the real,
/// single-replay path.
#[test]
fn m6_witness_idempotent_retry_leaves_the_witness_unchanged() {
    let root = unique_temp_dir("m6-idempotent-retry");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    let before = current_witness(&layout);
    let wal = crate::wal::Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let envelope = wal
        .replay()
        .unwrap()
        .records
        .last()
        .unwrap()
        .envelope
        .clone();
    let seq =
        super::append_patch_and_witness(&layout, DEFAULT_ACTIVE_NAME, "heads/main", &envelope)
            .unwrap();
    assert_eq!(
        seq, before.last_seq,
        "the retry returns the pre-existing seq, not a new one"
    );
    let after = current_witness(&layout);
    assert_eq!(
        after, before,
        "an idempotent retry must not change the witness at all"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Control 3: a damaged record strictly inside the queue (not the tail) still refuses the next
/// commit, with the message naming `doctor` and `--discard-damaged-commits`, exactly as before
/// 0.53.0 step 3B -- `append_patch_reporting`'s own `has_item_failure()` check is unchanged, and
/// this confirms it still fires correctly through the new wrapper, not merely by reading the diff.
#[test]
fn m6_interior_damage_in_an_800_deep_queue_still_refuses() {
    let root = unique_temp_dir("m6-interior-damage-800");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    for index in 0..800 {
        commit(
            &layout,
            &format!("f{index}.txt"),
            format!("{index}").as_bytes(),
        );
    }
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    // Flip one byte inside record 3's own body -- far from the tail, deep in the queue's interior.
    let target = bytes.len() / 800 * 3 + 40;
    bytes[target] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();

    std::fs::write(layout.root().join("one-more.txt"), b"one more\n").unwrap();
    let result = commit_worktree_changes_with_generator(
        &layout,
        "heads/main",
        "one more",
        WorktreePatchCommitOptions::file_level(),
        &mut NodeIdGenerator::production(),
        &signer(),
    );
    let message = result.unwrap_err().to_string();
    assert!(
        message.contains("doctor") && message.contains("damaged"),
        "expected a refusal naming doctor over the damaged record, got: {message}"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// Control 4: if `append_patch_and_witness`'s own fold ever used a stale record set (not what it
/// actually just appended), the witness it writes would disagree with an independent replay --
/// `verify_running_hash` (RFC 166 D4, unchanged by this round) is what would catch that. Confirmed
/// by a real perturbation: fold through `seq - 1` (as if the newest record itself had been left
/// out of the fold -- a stale view of the queue), and show the resulting witness fails
/// `verify_running_hash` against the real, genuine replay.
#[test]
fn m6_a_stale_fold_is_caught_by_verify_running_hash() {
    let root = unique_temp_dir("m6-stale-fold");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    let witness = current_witness(&layout);
    let wal = crate::wal::Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let genuine = wal.replay().unwrap();
    assert!(
        super::verify_running_hash(&witness, &genuine).unwrap(),
        "the real, unperturbed witness must agree with a genuine replay"
    );

    // Simulate the fold having used a stale view: a witness claiming `last_seq` 2 but whose
    // running hash was actually folded only through seq 1 (as `fold_running_hash` would produce if
    // handed a stale record set that omitted the newest one).
    let stale_running_hash = independent_running_hash(&layout, 1);
    let mut stale = witness.clone();
    stale.running_hash = stale_running_hash;
    assert!(
        !super::verify_running_hash(&stale, &genuine).unwrap(),
        "a witness whose running hash was folded from a stale record set must fail verification \
         against the real, genuine replay"
    );
    let _ = std::fs::remove_dir_all(root);
}

fn m6_killed_file_queue_shape(layout: &RepositoryLayout) -> Option<(usize, bool)> {
    let entries = crate::enumerate_queued_patches(layout).ok()?;
    let killed_once_correctly = entries.iter().any(|entry| {
        entry.message.as_deref() == Some("killed")
            && entry.operations.len() == 1
            && entry.operations[0].kind == "create-file"
            && entry.operations[0].paths.iter().any(
                |path| matches!(path, crate::QueuedPathResolution::Path(p) if p == "killed.txt"),
            )
    });
    Some((entries.len(), killed_once_correctly))
}

/// Control 5: a commit kill sweep over `append_patch_and_witness`'s own write ordinals (the
/// durable WAL append, then the witness write in place) -- the same families and the same honest
/// evidence standard 3A round 2 had to learn the hard way (R-2): n/N counted only over ordinals
/// that genuinely fired, each checked against a real post-condition (the queue's shape after
/// recovery equals an independent, uninterrupted reference run; `verify` reports no failed item),
/// never a vague "did not error" or a substring match accepted as success on its own.
#[test]
fn m6_commit_kill_sweep_recovers_at_every_write_ordinal() {
    const MAX_ORDINAL: usize = 20;

    let reference_root = unique_temp_dir("m6-kill-reference");
    let reference_shape = {
        let layout = RepositoryLayout::init(reference_root.clone()).unwrap();
        commit(&layout, "warm.txt", b"warm");
        std::fs::write(layout.root().join("killed.txt"), b"killed\n").unwrap();
        commit_worktree_changes_signed(
            &layout,
            "heads/main",
            "killed",
            WorktreePatchCommitOptions::default(),
            &signer(),
        )
        .unwrap();
        m6_killed_file_queue_shape(&layout).expect("the reference fixture reads back")
    };
    assert_eq!(
        reference_shape,
        (2, true),
        "fixture sanity: the reference run queues warm and killed.txt exactly, uncrashed"
    );
    let _ = std::fs::remove_dir_all(&reference_root);

    let mut fired_total = 0_usize;
    let mut per_family = Vec::new();
    let mut swallowed = Vec::new();

    for point in [
        TestFailPoint::AppendWrite,
        TestFailPoint::RequiredFileSync,
        TestFailPoint::MutableFileSync,
        TestFailPoint::MutableRename,
        TestFailPoint::Truncate,
    ] {
        let mut fired_count = 0_usize;
        let mut failed_count = 0_usize;
        for ordinal in 0..MAX_ORDINAL {
            let root = unique_temp_dir(&format!("m6-kill-{point:?}-{ordinal}"));
            let layout = RepositoryLayout::init(root.clone()).unwrap();
            commit(&layout, "warm.txt", b"warm");

            std::fs::write(layout.root().join("killed.txt"), b"killed\n").unwrap();
            fail_after_for_test(point, ordinal);
            let crashed = commit_worktree_changes_signed(
                &layout,
                "heads/main",
                "killed",
                WorktreePatchCommitOptions::default(),
                &signer(),
            );
            let still_armed = failpoint_armed_for_test();
            clear_failpoint_for_test();

            if still_armed {
                let _ = std::fs::remove_dir_all(&root);
                break;
            }
            fired_count += 1;
            fired_total += 1;
            if crashed.is_ok() {
                swallowed.push(format!("{point:?}@{ordinal}"));
            } else {
                failed_count += 1;
            }

            let mut retry = if crashed.is_ok() {
                None
            } else {
                Some(commit_worktree_changes_signed(
                    &layout,
                    "heads/main",
                    "killed",
                    WorktreePatchCommitOptions::default(),
                    &signer(),
                ))
            };
            if matches!(&retry, Some(Err(err)) if matches!(err, prikk_error::PrikkError::LockConflict(_)))
            {
                if let Ok(held) = crate::unlock::list_held_locks(&layout) {
                    for lock in &held {
                        let _ = crate::unlock::clear_lock(&layout, &lock.path);
                    }
                }
                retry = Some(commit_worktree_changes_signed(
                    &layout,
                    "heads/main",
                    "killed",
                    WorktreePatchCommitOptions::default(),
                    &signer(),
                ));
            }
            if matches!(&retry, Some(Err(err)) if err.to_string().contains("doctor")) {
                let _ = crate::discard_damaged_commits(&layout);
                retry = Some(commit_worktree_changes_signed(
                    &layout,
                    "heads/main",
                    "killed",
                    WorktreePatchCommitOptions::default(),
                    &signer(),
                ));
            }

            let verification = crate::verify_repository(&layout);
            let verify_clean = verification
                .as_ref()
                .map(|v| !v.has_stage_failure() && !v.has_item_failure())
                .unwrap_or(false);

            let commit_recovers = crashed.is_ok()
                || match &retry {
                    Some(Ok(_)) => true,
                    Some(Err(err)) => {
                        let message = err.to_string();
                        message.contains("nothing left to commit")
                            || message.contains("not confirmed")
                            || message.contains("no node-addressed changes to commit")
                    }
                    None => true,
                };
            let queue_shape = m6_killed_file_queue_shape(&layout);
            let queue_recovers = queue_shape == Some((2, true));

            assert!(
                verify_clean && commit_recovers && queue_recovers,
                "{point:?}@{ordinal}: the repository did not recover cleanly (crashed={crashed:?}, \
                 verify_clean={verify_clean} ({verification:?}), retry={retry:?}, \
                 queue_shape={queue_shape:?}, expected (2, true))"
            );
            let _ = std::fs::remove_dir_all(&root);
        }
        per_family.push(format!(
            "{point:?}: armed {MAX_ORDINAL}, fired {fired_count}, fired-and-failed {failed_count}"
        ));
    }
    assert!(
        fired_total >= 1,
        "the sweep must genuinely fire at least once; it fired {fired_total} times across {per_family:?}"
    );
    eprintln!(
        "M6 kill sweep: {fired_total} ordinals genuinely fired and recovered, {per_family:?}, \
         swallowed (fired but did not fail the commit): {swallowed:?}"
    );
}
