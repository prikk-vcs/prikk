#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use super::{RestoreRefusalContext, Verdict, classify, write_refusal_reason};
use crate::commit_boundary::active::read_active_ref_metadata;
use crate::commit_boundary::witness::{WitnessState, clear_witness, read_witness};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_with_generator;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::node::node_id_gen::NodeIdGenerator;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{Ed25519AuthorSigner, WorktreePatchCommitOptions};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-d3-classify", &[0x81_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_with_generator(
        layout,
        "heads/main",
        "d3",
        WorktreePatchCommitOptions::file_level(),
        &mut NodeIdGenerator::production(),
        &signer(),
    )
    .unwrap();
}

fn classify_now(layout: &RepositoryLayout) -> Verdict {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let owning_ref = read_active_ref_metadata(layout).unwrap();
    let witness = read_witness(layout, DEFAULT_ACTIVE_NAME).unwrap();
    classify(layout, &replay, &owning_ref, &witness).unwrap()
}

// Row 1: sound x agrees -> Healthy.
#[test]
fn row1_healthy() {
    let root = unique_temp_dir("rfc166-d3-row1");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    assert_eq!(classify_now(&layout), Verdict::Healthy { last_seq: 1 });
    // Control: a second, un-witnessed commit must NOT also read as Healthy (the verdict is
    // seq-specific, not "any sound WAL").
    let witness_before = std::fs::read(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
    )
    .unwrap();
    commit(&layout, "b.txt", b"two");
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
        witness_before,
    )
    .unwrap();
    assert_ne!(
        classify_now(&layout),
        Verdict::Healthy { last_seq: 2 },
        "control failed: a stale witness must not read as healthy at the new seq"
    );
    std::fs::remove_dir_all(&root).ok();
}

// Row 2: sound x behind -> Pending.
#[test]
fn row2_pending() {
    let root = unique_temp_dir("rfc166-d3-row2");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let witness_after_first = std::fs::read(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
    )
    .unwrap();
    commit(&layout, "b.txt", b"two");
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
        witness_after_first,
    )
    .unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::Pending {
            witnessed_seq: Some(1),
            last_seq: 2
        }
    );
    // Control: restoring the up-to-date witness must turn this back into Healthy, not stay Pending.
    commit(&layout, "c.txt", b"three"); // advances the witness for real to seq 3
    assert_eq!(classify_now(&layout), Verdict::Healthy { last_seq: 3 });
    std::fs::remove_dir_all(&root).ok();
}

// Row 3: crash tail x agrees -> CrashTail.
#[test]
fn row3_crash_tail() {
    let root = unique_temp_dir("rfc166-d3-row3");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    bytes.extend_from_slice(&[0u8; 10]); // a genuine torn-shaped tail
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::CrashTail {
            sound_through: Some(1)
        }
    );
    // Control: removing the tail bytes must turn this back into Healthy.
    std::fs::write(&wal_path, &bytes[..bytes.len() - 10]).unwrap();
    assert_eq!(classify_now(&layout), Verdict::Healthy { last_seq: 1 });
    std::fs::remove_dir_all(&root).ok();
}

// Row 4 (N6): acknowledged damage.
#[test]
fn row4_acknowledged_damage() {
    let root = unique_temp_dir("rfc166-d3-row4");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF; // a complete record, damaged, not torn
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );
    // Control: a legacy (no-witness) session over the identical damage must NOT read as
    // acknowledged -- it is unexplained-tail-with-no-witness, i.e. NoWitness.
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    match classify_now(&layout) {
        Verdict::NoWitness { stale: false, .. } => {}
        other => {
            panic!(
                "control failed: expected NoWitness with stale: false once the witness is gone, \
                 got {other:?}"
            )
        }
    }
    std::fs::remove_dir_all(&root).ok();
}

// Row 4, the connectivity escape: a seal the reader missed turns this into NoWitness (stale), not
// AcknowledgedDamage.
#[test]
fn row4_connectivity_finds_a_drain_not_damage() {
    let root = unique_temp_dir("rfc166-d3-row4-drain");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let maintainer = crate::maintainer_signing::Ed25519MaintainerSigner::from_seed(
        "rfc166-d3-row4-drain",
        &[0x82_u8; 32],
    )
    .unwrap();
    use crate::maintainer_signing::MaintainerSigner as _;
    let pub_hex: String = maintainer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::trust::add_trusted_maintainer(&layout, maintainer.key_id(), &pub_hex).unwrap();
    commit(&layout, "a.txt", b"one");
    let witness_before_seal = std::fs::read(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
    )
    .unwrap();
    crate::simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer).unwrap();
    assert_eq!(
        read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap(),
        WitnessState::Absent
    );
    // Restore the stale, pre-seal witness -- exactly the "an older binary's own seal" shape.
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
        witness_before_seal,
    )
    .unwrap();
    match classify_now(&layout) {
        Verdict::NoWitness { stale: true, .. } => {}
        other => panic!("expected NoWitness with stale: true (a found drain), got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

// Row 5: acknowledged loss (shorter than the witness, connectivity fails).
#[test]
fn row5_acknowledged_loss() {
    let root = unique_temp_dir("rfc166-d3-row5");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    // Wipe the WAL to empty -- a genuine loss (never sealed), the simplest construction shorter
    // than the witness's own last_seq=2.
    std::fs::write(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME), []).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedLoss { witnessed_seq: 2 }
    );
    // Control: a legacy (no-witness) session with an empty WAL is just NoActiveWork-shaped
    // (NoWitness), never a reported loss.
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    match classify_now(&layout) {
        Verdict::NoWitness { stale: false, .. } => {}
        other => panic!("control failed: expected NoWitness with stale: false, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

// Row 6: substituted record (same seq, different identity, connectivity fails).
#[test]
fn row6_substituted_record() {
    let root_a = unique_temp_dir("rfc166-d3-row6-a");
    let layout_a = RepositoryLayout::init(root_a.clone()).unwrap();
    commit(&layout_a, "a.txt", b"one");

    let root_b = unique_temp_dir("rfc166-d3-row6-b");
    let layout_b = RepositoryLayout::init(root_b.clone()).unwrap();
    commit(&layout_b, "a.txt", b"DIFFERENT");

    std::fs::write(
        layout_a.active_queue_wal_path(DEFAULT_ACTIVE_NAME),
        std::fs::read(layout_b.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap(),
    )
    .unwrap();
    assert_eq!(
        classify_now(&layout_a),
        Verdict::SubstitutedRecord { witnessed_seq: 1 }
    );
    // Control: if the witness is simply absent, this is a legacy/no-witness session, never a
    // reported substitution -- confirming the verdict comes from the witness's own disagreement,
    // not merely from the WAL's own content being "unexpected" in some other sense.
    clear_witness(&layout_a, DEFAULT_ACTIVE_NAME).unwrap();
    match classify_now(&layout_a) {
        Verdict::NoWitness { stale: false, .. } => {}
        other => panic!("control failed: expected NoWitness with stale: false, got {other:?}"),
    }
    std::fs::remove_dir_all(&root_a).ok();
    std::fs::remove_dir_all(&root_b).ok();
}

// Row 7: a tail plus a damaged witness -> UnknownWithDamagedWitness.
#[test]
fn row7_unknown_with_damaged_witness() {
    let root = unique_temp_dir("rfc166-d3-row7");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut wal_bytes = std::fs::read(&wal_path).unwrap();
    wal_bytes.extend_from_slice(&[0u8; 10]);
    std::fs::write(&wal_path, &wal_bytes).unwrap();
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let mut witness_bytes = std::fs::read(&witness_path).unwrap();
    let last = witness_bytes.len() - 1;
    witness_bytes[last] ^= 0xFF;
    std::fs::write(&witness_path, &witness_bytes).unwrap();
    assert_eq!(classify_now(&layout), Verdict::UnknownWithDamagedWitness);
    // Control: fixing the witness (restoring it) with the same tail present must turn this into
    // the ordinary CrashTail verdict (row 3), not stay "unknown".
    witness_bytes[last] ^= 0xFF; // un-flip
    std::fs::write(&witness_path, &witness_bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::CrashTail {
            sound_through: Some(1)
        }
    );
    std::fs::remove_dir_all(&root).ok();
}

// Row 8: sound, no tail, damaged witness -> WitnessDamaged (a warning, not a refusal-worthy verdict).
#[test]
fn row8_witness_damaged_over_a_sound_wal() {
    let root = unique_temp_dir("rfc166-d3-row8");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let witness_path = layout
        .active_session_dir(DEFAULT_ACTIVE_NAME)
        .join("witness");
    let mut bytes = std::fs::read(&witness_path).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xFF;
    std::fs::write(&witness_path, &bytes).unwrap();
    assert_eq!(classify_now(&layout), Verdict::WitnessDamaged);
    // Control: adding a tail on top must escalate this to row 7, not stay row 8.
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut wal_bytes = std::fs::read(&wal_path).unwrap();
    wal_bytes.extend_from_slice(&[0u8; 10]);
    std::fs::write(&wal_path, &wal_bytes).unwrap();
    assert_eq!(classify_now(&layout), Verdict::UnknownWithDamagedWitness);
    std::fs::remove_dir_all(&root).ok();
}

// Row 9: ownership missing (the WAL has records, no durable owner) -- takes priority over any
// witness-based verdict.
#[test]
fn row9_ownership_missing_takes_priority() {
    let root = unique_temp_dir("rfc166-d3-row9");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        [],
    )
    .unwrap();
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);
    // Control: restoring ref-name must turn this back into Healthy (the witness itself was never
    // touched, confirming row 9's own priority was the only thing masking it).
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        b"heads/main",
    )
    .unwrap();
    assert_eq!(classify_now(&layout), Verdict::Healthy { last_seq: 1 });
    std::fs::remove_dir_all(&root).ok();
}

// D6: a ref-name/witness mismatch is row 9's own refusal, same as no owner at all.
#[test]
fn d6_ref_name_witness_mismatch_is_ownership_missing() {
    let root = unique_temp_dir("rfc166-d3-d6-mismatch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        b"heads/other",
    )
    .unwrap();
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);
    // Control: restoring agreement must turn this back into Healthy.
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        b"heads/main",
    )
    .unwrap();
    assert_eq!(classify_now(&layout), Verdict::Healthy { last_seq: 1 });
    std::fs::remove_dir_all(&root).ok();
}

// D3 item 3: "show the walk's length in a test: one seal after the witness means one step." Tested
// by its own observable consequence (the walk is genuinely *bounded*, not merely fast): a patch
// that only exists *before* the witness's own recorded tip must never be found reachable, even
// though it really is in the ref's full history -- if the walk continued past `stop_at` to
// genesis, it would (wrongly) find it.
#[test]
fn connectivity_walk_never_finds_a_patch_that_predates_the_witnessed_tip() {
    let root_a = unique_temp_dir("rfc166-d3-walk-bounded-a");
    let layout_a = RepositoryLayout::init(root_a.clone()).unwrap();
    let maintainer = crate::maintainer_signing::Ed25519MaintainerSigner::from_seed(
        "rfc166-d3-walk-bounded",
        &[0x83_u8; 32],
    )
    .unwrap();
    use crate::maintainer_signing::MaintainerSigner as _;
    let pub_hex: String = maintainer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::trust::add_trusted_maintainer(&layout_a, maintainer.key_id(), &pub_hex).unwrap();
    // gen1: sealed for real, before any witness this test ever reads exists.
    commit(&layout_a, "a.txt", b"gen1");
    crate::simulate_one_seal_for_test_support(&layout_a, "heads/main", &maintainer).unwrap();
    let gen1_wal = {
        // Recover gen1's own witnessed patch id from a parallel, single-commit repository sharing
        // the same deterministic content (the real gen1 session's own witness no longer exists
        // after its own seal drained it).
        let root_probe = unique_temp_dir("rfc166-d3-walk-bounded-probe");
        let layout_probe = RepositoryLayout::init(root_probe.clone()).unwrap();
        commit(&layout_probe, "a.txt", b"gen1");
        let witness = match read_witness(&layout_probe, DEFAULT_ACTIVE_NAME).unwrap() {
            WitnessState::Valid(record) => record.patch_id,
            other => panic!("expected Valid, got {other:?}"),
        };
        std::fs::remove_dir_all(&root_probe).ok();
        witness
    };
    // gen2: the witnessed commit this test's own bounded walk is about, sealed afterward for real.
    commit(&layout_a, "b.txt", b"gen2");
    let witness_before_seal = std::fs::read(
        layout_a
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
    )
    .unwrap();
    crate::simulate_one_seal_for_test_support(&layout_a, "heads/main", &maintainer).unwrap();
    // Forge the stale witness to claim gen1's own patch (genuinely reachable, but strictly *before*
    // this witness's own recorded tip -- gen1 was already sealed when this witness was written).
    let mut record = match super::super::witness::decode_for_test(&witness_before_seal) {
        Some(record) => record,
        None => panic!("the witness this test just read must decode"),
    };
    record.patch_id = gen1_wal;
    std::fs::write(
        layout_a
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("witness"),
        super::super::witness::encode_for_test(&record),
    )
    .unwrap();
    match classify_now(&layout_a) {
        Verdict::AcknowledgedLoss { .. } | Verdict::SubstitutedRecord { .. } => {}
        other => panic!(
            "expected the bounded walk to miss gen1's own patch (it predates this witness's own \
             recorded tip), got {other:?} -- if this is NoWitness, the walk continued past \
             stop_at to genesis, which is exactly the bug §13 item 3 fixed"
        ),
    }
    std::fs::remove_dir_all(&root_a).ok();
}

// ---- RFC 166 §14 item 6: write_refusal_reason's own enriched row-9 text --------------------------

// With a valid witness (unchanged, C2): the text names the witness's own ref, never "your current
// branch" (which would be a different, and here coincidentally equal, fact), and no internal words.
#[test]
fn row9_refusal_text_with_a_witness_names_the_witness_ref() {
    let root = unique_temp_dir("rfc166-s14-refusal-text-witness");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        [],
    )
    .unwrap();
    let verdict = classify_now(&layout);
    assert_eq!(verdict, Verdict::OwnershipMissing);
    let witness = read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let reason = write_refusal_reason(
        &verdict,
        Some(RestoreRefusalContext {
            layout: &layout,
            witness: &witness,
            queued_count: 1,
        }),
    )
    .expect("row 9 blocks a write");
    assert!(
        reason.contains("This session's own commit record names heads/main"),
        "unexpected text: {reason}"
    );
    assert!(
        reason.contains("`prikk doctor --restore-queue-target --ref heads/main --plan-only`"),
        "must name a concrete, runnable command: {reason}"
    );
    assert!(
        !reason.contains("durable, matching owner"),
        "no internal words: {reason}"
    );
    std::fs::remove_dir_all(&root).ok();
}

// Without a witness, with a resolvable current branch (the unborn default, heads/main here): the
// text names the current branch instead.
#[test]
fn row9_refusal_text_without_a_witness_names_the_current_branch() {
    let root = unique_temp_dir("rfc166-s14-refusal-text-no-witness");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        [],
    )
    .unwrap();
    let verdict = classify_now(&layout);
    assert_eq!(verdict, Verdict::OwnershipMissing);
    let witness = read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    assert_eq!(witness, WitnessState::Absent);
    let reason = write_refusal_reason(
        &verdict,
        Some(RestoreRefusalContext {
            layout: &layout,
            witness: &witness,
            queued_count: 1,
        }),
    )
    .expect("row 9 blocks a write");
    assert!(
        reason.contains("Your current branch is heads/main"),
        "unexpected text: {reason}"
    );
    assert!(
        reason.contains("`prikk doctor --restore-queue-target --ref heads/main --plan-only`"),
        "must name a concrete, runnable command: {reason}"
    );
    std::fs::remove_dir_all(&root).ok();
}

// Without a witness, with an unresolvable current branch: falls back to directing the caller to
// name the branch themselves, still with no internal words and still a concrete, runnable command.
#[test]
fn row9_refusal_text_with_an_unresolvable_current_branch_asks_for_the_ref() {
    let root = unique_temp_dir("rfc166-s14-refusal-text-unresolvable");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    std::fs::write(
        layout
            .active_session_dir(DEFAULT_ACTIVE_NAME)
            .join("ref-name"),
        [],
    )
    .unwrap();
    std::fs::write(layout.current_branch_path(), b"not a valid ref\n").unwrap();
    let verdict = classify_now(&layout);
    assert_eq!(verdict, Verdict::OwnershipMissing);
    let witness = read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let reason = write_refusal_reason(
        &verdict,
        Some(RestoreRefusalContext {
            layout: &layout,
            witness: &witness,
            queued_count: 1,
        }),
    )
    .expect("row 9 blocks a write");
    assert!(
        reason.contains("--not-current-branch"),
        "unexpected text: {reason}"
    );
    assert!(
        !reason.contains("durable, matching owner"),
        "no internal words: {reason}"
    );
    std::fs::remove_dir_all(&root).ok();
}

// `None` (verify/doctor's own report construction, which has no branch to resolve): the plainer,
// still internal-word-free fallback text, naming the verb with a placeholder ref.
#[test]
fn row9_refusal_text_with_no_context_falls_back_to_a_placeholder() {
    let reason = write_refusal_reason(&Verdict::OwnershipMissing, None)
        .expect("row 9 blocks a write even with no context");
    assert!(
        reason.contains("`prikk doctor --restore-queue-target --ref <ref>`"),
        "unexpected text: {reason}"
    );
}

// The "no witness" item (not a numbered row, D3 item 1): legacy session, never worse than 0.48.0.
#[test]
fn no_witness_falls_back_to_rule_3() {
    let root = unique_temp_dir("rfc166-d3-no-witness");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::NoWitness {
            wal_otherwise_sound: true,
            stale: false,
        }
    );
    std::fs::remove_dir_all(&root).ok();
}
