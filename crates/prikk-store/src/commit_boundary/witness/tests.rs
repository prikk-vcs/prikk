#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{WitnessState, clear_witness, read_witness};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_with_generator;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::node::node_id_gen::NodeIdGenerator;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions};

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
#[cfg(target_os = "linux")]
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
    let src = repo_root().join("crates/prikk-store/src");
    // This crate's own established naming for test-only code, consistently used everywhere a file
    // or directory is test-only but does not literally match `/tests/` or end in `tests.rs`: found
    // empirically by running this exact scan first and checking every match it returned traces to
    // one of these (`test_gates/`, `caller_tests*`, `*_test_support*`), not assumed in advance.
    const TEST_ONLY_MARKERS: &[&str] = &[
        "/tests/",
        "tests.rs",
        "test_gates",
        "caller_tests",
        "test_support",
    ];
    let mut non_test_callers = Vec::new();
    visit_rs_files(&src, &mut |path, contents| {
        let path_text = path.to_string_lossy();
        if TEST_ONLY_MARKERS
            .iter()
            .any(|marker| path_text.contains(marker))
        {
            return;
        }
        for (line_number, line) in contents.lines().enumerate() {
            if line.contains(".append_patch(") {
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
        "expected exactly one production caller of Wal::append_patch (inside witness.rs itself); \
         found: {non_test_callers:#?}"
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
