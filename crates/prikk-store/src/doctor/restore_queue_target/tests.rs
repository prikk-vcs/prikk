//! RFC 166 D5, §13 item 15: `prikk doctor --restore-queue-target --ref <ref>`.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use super::{plan_restore_queue_target, restore_queue_target};
use crate::commit_boundary::active::{ActiveRefMetadata, read_active_ref_metadata};
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::{clear_witness, read_witness};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::test_gates::test_support::unique_temp_dir;
use crate::wal::Wal;
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, WorktreePatchCommitOptions,
    add_trusted_maintainer,
};

fn signer() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("rfc166-d5-restore", &[0x92_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "d5-restore",
        WorktreePatchCommitOptions::file_level(),
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

fn snapshot_tree(layout: &RepositoryLayout) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![layout.prikk_dir().to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((
                    path.strip_prefix(layout.prikk_dir())
                        .unwrap()
                        .display()
                        .to_string(),
                    bytes,
                ));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Simulates the compounded crash RFC 166 §1.6 describes: ownership is durably present right up
/// until it is not -- a tear in the very write this verb exists to repair, not a slow corruption.
fn clear_ref_name(layout: &RepositoryLayout) {
    std::fs::write(layout.default_active_ref_name_path(), []).unwrap();
}

/// Row 9, the main case: a non-empty, wholly sound WAL with no durable owner. Restoring to the ref
/// the witness already names succeeds, writes `ref-name` back, and the session reads healthy again.
#[test]
fn row9_restores_ownership_and_writes_ref_name_atomically() {
    let root = unique_temp_dir("rfc166-d5-restore-row9");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let plan =
        restore_queue_target(&layout, "heads/main", false).expect("row 9 is this verb's own job");
    assert_eq!(plan.ref_name, "heads/main");
    assert_eq!(plan.patch_ids.len(), 1);

    // RFC 166 §14 item 7: the queued commit's own message and paths, never a bare block hash.
    assert_eq!(plan.queued_commits.len(), 1);
    assert_eq!(
        plan.queued_commits[0].message,
        Some("d5-restore".to_string())
    );
    assert_eq!(plan.queued_commits[0].paths, vec!["a.txt".to_string()]);
    assert_eq!(
        plan.latest_sealed_commit, None,
        "heads/main has never been published"
    );
    // §14 item 8: a valid witness decided the ref, so there is no uncertainty to disclose.
    assert!(!plan.current_branch_assumed);

    match read_active_ref_metadata(&layout).unwrap() {
        ActiveRefMetadata::Valid(name) => assert_eq!(name, "heads/main"),
        other => panic!("expected ownership restored, got {other:?}"),
    }
    assert!(matches!(
        classify_now(&layout),
        Verdict::Healthy { last_seq: 1 }
    ));
    std::fs::remove_dir_all(&root).ok();
}

/// D5/C2: the ref comes from the caller, but a witness naming a *different* ref refuses rather
/// than silently overruling it.
#[test]
fn witness_naming_a_different_ref_refuses_writing_nothing() {
    let root = unique_temp_dir("rfc166-d5-restore-witness-mismatch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/other", false)
        .expect_err("the witness names heads/main, not heads/other");
    assert!(
        err.to_string().contains("names heads/main"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// A healthy session has nothing to restore -- ownership already matches the witness.
#[test]
fn a_healthy_session_refuses_writing_nothing() {
    let root = unique_temp_dir("rfc166-d5-restore-healthy");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    assert!(matches!(classify_now(&layout), Verdict::Healthy { .. }));

    let before = snapshot_tree(&layout);
    let err =
        restore_queue_target(&layout, "heads/main", false).expect_err("ownership is not missing");
    assert!(
        err.to_string()
            .contains("already has a durable, matching owner"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// RFC 166 D5 K1: `--plan-only` and a real run share one computation, and `--plan-only` touches
/// nothing.
#[test]
fn plan_only_matches_the_real_runs_own_plan_and_touches_nothing() {
    let root = unique_temp_dir("rfc166-d5-restore-plan-only");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);

    let before = snapshot_tree(&layout);
    let plan = plan_restore_queue_target(&layout, "heads/main", false)
        .expect("plan-only does not refuse row 9");
    let after_plan_only = snapshot_tree(&layout);
    assert_eq!(before, after_plan_only, "plan-only must touch nothing");

    let real = restore_queue_target(&layout, "heads/main", false).expect("the real run");
    assert_eq!(
        plan, real,
        "the plan-only computation must equal the real run's own"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ---- RFC 166 §14: without a witness, `<ref>` must be the current branch ----------------------

/// §14 item 1, without a witness: this is the 0.20.0-0.48.0 legacy shape exactly -- no witness ever
/// recorded it, ownership is missing, and the caller's `<ref>` matches the current branch (the
/// unborn default, `heads/main`, since this repository never switched). Succeeds with no flag.
#[test]
fn without_a_witness_the_current_branch_succeeds_with_no_flag() {
    let root = unique_temp_dir("rfc166-d5-restore-no-witness-current-branch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);
    match read_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap() {
        crate::commit_boundary::witness::WitnessState::Absent => {}
        other => panic!("expected no witness, got {other:?}"),
    }

    let plan = restore_queue_target(&layout, "heads/main", false)
        .expect("heads/main is this repository's own current branch");
    assert_eq!(plan.ref_name, "heads/main");
    // §14 item 8: no witness decided this -- the current branch was assumed, so the plan must say
    // so (the uncertainty sentence applies).
    assert!(plan.current_branch_assumed);
    match read_active_ref_metadata(&layout).unwrap() {
        ActiveRefMetadata::Valid(name) => assert_eq!(name, "heads/main"),
        other => panic!("expected ownership restored, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

/// §14 item 1, without a witness but with `--not-current-branch` given explicitly: there is
/// nothing left to be uncertain about, even though no witness decided the ref either.
#[test]
fn not_current_branch_flag_clears_the_uncertainty_even_without_a_witness() {
    let root = unique_temp_dir("rfc166-s14-item8-flag-clears-uncertainty");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    clear_ref_name(&layout);

    let plan = restore_queue_target(&layout, "heads/main", true)
        .expect("--not-current-branch always succeeds when given, even redundantly");
    assert!(
        !plan.current_branch_assumed,
        "the flag was given explicitly; nothing was assumed"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// §14 item 7: the branch's own latest sealed commit, described for the plan when `<ref>` has
/// already been published -- never a bare block hash.
#[test]
fn the_plan_describes_the_branchs_own_latest_sealed_commit() {
    let root = unique_temp_dir("rfc166-s14-item7-latest-sealed");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed("rfc166-s14-item7-maintainer", &[0x94; 32]).unwrap();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .unwrap();
    commit(&layout, "a.txt", b"one");
    simulate_one_seal(&layout, "heads/main", &maintainer).expect("seal onto heads/main");

    commit(&layout, "b.txt", b"two");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    clear_ref_name(&layout);

    let plan = plan_restore_queue_target(&layout, "heads/main", false)
        .expect("heads/main is still this repository's own current branch");
    let latest = plan
        .latest_sealed_commit
        .as_ref()
        .expect("heads/main has already been sealed once");
    assert_eq!(latest.message, Some("d5-restore".to_string()));
    assert_eq!(latest.paths, vec!["a.txt".to_string()]);
    std::fs::remove_dir_all(&root).ok();
}

/// §14 item 1, without a witness: `<ref>` disagreeing with the current branch refuses, naming it --
/// without `--not-current-branch` a restore never attaches an unidentified queue to a branch the
/// caller did not explicitly ask to override. **Control:** the same request with the flag succeeds.
#[test]
fn without_a_witness_a_different_ref_needs_the_flag() {
    let root = unique_temp_dir("rfc166-d5-restore-no-witness-wrong-branch");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/other", false)
        .expect_err("heads/other is not the current branch, and no flag was given");
    assert!(
        err.to_string().contains("heads/main"),
        "the refusal must name the current branch: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );

    // Control: the one condition this case is built from (no flag) removed, the same request now
    // succeeds and attaches the queue to the requested, non-current branch.
    let plan = restore_queue_target(&layout, "heads/other", true)
        .expect("--not-current-branch explicitly overrides the current-branch rule");
    assert_eq!(plan.ref_name, "heads/other");
    match read_active_ref_metadata(&layout).unwrap() {
        ActiveRefMetadata::Valid(name) => assert_eq!(name, "heads/other"),
        other => panic!("expected ownership restored to heads/other, got {other:?}"),
    }
    std::fs::remove_dir_all(&root).ok();
}

/// §14 item 1: an unresolvable current branch (here, a malformed `.prikk/current-branch` pointer)
/// requires `--not-current-branch` too, fail closed -- there is nothing to compare `<ref>` against.
/// **Control:** the flag lets the same request through regardless.
#[test]
fn an_unresolvable_current_branch_needs_the_flag_too() {
    let root = unique_temp_dir("rfc166-d5-restore-no-witness-unresolvable");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    clear_ref_name(&layout);
    std::fs::write(layout.current_branch_path(), b"not a valid ref\n").unwrap();
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/main", false)
        .expect_err("the current branch cannot be resolved, and no flag was given");
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    drop(err);

    // Control: the flag lets it through even though the current branch stays unresolvable.
    restore_queue_target(&layout, "heads/main", true)
        .expect("--not-current-branch bypasses the comparison entirely");
    std::fs::remove_dir_all(&root).ok();
}

/// §14 item 1's fourth bullet: a restored owner is final for this verb. A second restore over an
/// already-owned queue refuses, the same as any other healthy session -- even to a different ref,
/// and even with `--not-current-branch`.
#[test]
fn a_second_restore_over_an_owned_queue_still_refuses() {
    let root = unique_temp_dir("rfc166-d5-restore-second-restore");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_witness(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    clear_ref_name(&layout);
    restore_queue_target(&layout, "heads/main", false).expect("the first restore succeeds");

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/other", true)
        .expect_err("ownership is already present; a second restore must refuse");
    assert!(
        err.to_string()
            .contains("already has a durable, matching owner"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ---- K3: the remaining refusal conditions, each with its own constructed case ----------------

/// K3, cross-verb boundary: row 4 (acknowledged damage) is `--discard-damaged-commits`'s job, not
/// this verb's. Built exactly like that verb's own row-4 fixture: a single damaged record reads
/// as `records.is_empty()` once excluded, so row 9's own gate never fires regardless of
/// `ref-name`'s state -- ownership here stays `Valid`, unaffected by the damage.
#[test]
fn row4_acknowledged_damage_is_not_this_verbs_job() {
    let root = unique_temp_dir("rfc166-d5-restore-row4-boundary");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    assert_eq!(
        classify_now(&layout),
        Verdict::AcknowledgedDamage { witnessed_seq: 1 }
    );

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/main", false)
        .expect_err("row 4 is discard_damaged_commits's job");
    assert!(
        err.to_string()
            .contains("already has a durable, matching owner"),
        "unexpected error: {err}"
    );
    assert!(
        !err.to_string().contains("discard-damaged-commits"),
        "this verb must not leak the other verb's own advice: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );
    std::fs::remove_dir_all(&root).ok();
}

/// K3: row 9 by itself does not excuse an unexplained tail *on top of* it -- this verb's own
/// `trailing_partial_bytes` check, run after the `OwnershipMissing` gate, catches a torn tail
/// that `classify`'s own row-9 priority does not look at.
#[test]
fn trailing_partial_bytes_refuses_even_though_classify_already_reached_ownership_missing() {
    let root = unique_temp_dir("rfc166-d5-restore-trailing-partial");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let mut bytes = std::fs::read(&wal_path).unwrap();
    bytes.extend_from_slice(&[0u8; 10]);
    std::fs::write(&wal_path, &bytes).unwrap();
    clear_ref_name(&layout);
    assert_eq!(
        classify_now(&layout),
        Verdict::OwnershipMissing,
        "row 9's own gate fires on the missing owner alone, before any tail is examined"
    );

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/main", false)
        .expect_err("a torn tail on top of row 9 is still a torn tail");
    assert!(
        err.to_string().contains("trailing partial bytes"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );

    // Control: removing the tail (the one condition this case is built from) lets the same
    // request succeed -- the refusal really did depend on the tail, not on something else.
    std::fs::write(&wal_path, &bytes[..bytes.len() - 10]).unwrap();
    restore_queue_target(&layout, "heads/main", false)
        .expect("with the tail gone, row 9 alone is this verb's own job");
    std::fs::remove_dir_all(&root).ok();
}

/// K3: a damaged *earlier* record (one sound record still follows it, so `records` stays
/// non-empty and row 9's own gate still fires) is caught by this verb's own `has_item_failure`
/// check, separately from the `OwnershipMissing` gate that let it through.
#[test]
fn a_damaged_earlier_record_refuses_even_though_classify_already_reached_ownership_missing() {
    let root = unique_temp_dir("rfc166-d5-restore-item-failure");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let wal_path = layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME);
    let record1_len = std::fs::read(&wal_path).unwrap().len();
    commit(&layout, "b.txt", b"two");
    let mut bytes = std::fs::read(&wal_path).unwrap();
    let flip_at = record1_len - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal_path, &bytes).unwrap();
    clear_ref_name(&layout);
    assert_eq!(
        classify_now(&layout),
        Verdict::OwnershipMissing,
        "the second record is still sound, so row 9's own gate still fires on the missing owner"
    );

    let before = snapshot_tree(&layout);
    let err = restore_queue_target(&layout, "heads/main", false)
        .expect_err("a damaged earlier record is still a damaged record");
    assert!(
        err.to_string().contains("damaged record"),
        "unexpected error: {err}"
    );
    assert_eq!(
        before,
        snapshot_tree(&layout),
        "a refusal must write nothing"
    );

    // Control: restoring record 1's own original bytes (the one condition this case is built
    // from) lets the same request succeed.
    std::fs::write(&wal_path, {
        let mut fixed = bytes.clone();
        fixed[flip_at] ^= 0xFF;
        fixed
    })
    .unwrap();
    restore_queue_target(&layout, "heads/main", false)
        .expect("with both records sound, row 9 alone is this verb's own job");
    std::fs::remove_dir_all(&root).ok();
}

// ---- K4: failpoints at the write ordinal, raced against commit and seal under the lock harness

/// K4: this verb is a writer -- it takes `ActiveLock` first, the same lock every commit-boundary
/// appender takes. A racing commit attempted while it holds the lock must refuse as a lock
/// conflict, never interleave with it.
#[test]
fn restore_races_an_ordinary_commit_under_the_shared_active_lock() {
    let root = unique_temp_dir("rfc166-d5-restore-k4-race");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);

    let held = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME).unwrap();
    let raced = restore_queue_target(&layout, "heads/main", false);
    assert!(
        matches!(raced, Err(prikk_error::PrikkError::LockConflict(_))),
        "restore_queue_target racing a held ActiveLock must refuse as a lock conflict, got \
         {raced:?}"
    );
    drop(held);

    restore_queue_target(&layout, "heads/main", false)
        .expect("succeeds once the active lock is free");
    std::fs::remove_dir_all(&root).ok();
}

/// K4: `ref-name`'s own write is a single atomic replace -- the whole point is that a crash during
/// it can never leave the file torn, only fully the old value or fully the new one. Crashed at
/// each of the three points `atomic_replace` itself can fail at, the file is always one or the
/// other, never a third, garbled state, and a second run (with no injected failure) completes it.
#[test]
fn a_crash_during_the_atomic_replace_never_tears_ref_name() {
    // `MutableParentSync` has no Windows counterpart at all (`foundation::fsutil::anchored::
    // failpoints::Point`'s own doc: no directory sync happens there) -- included only where the
    // platform actually has that boundary, matching `rfc163_stability_soak.rs::all_points`'s own
    // established pattern, rather than left unconditional and invisible to a Linux-only `cargo
    // test` run until cross-target clippy for Windows catches the missing variant.
    #[allow(unused_mut)]
    let mut points = vec![TestFailPoint::MutableFileSync, TestFailPoint::MutableRename];
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    points.push(TestFailPoint::MutableParentSync);
    for point in points {
        let root = unique_temp_dir(&format!("rfc166-d5-restore-k4-atomic-{point:?}"));
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        commit(&layout, "a.txt", b"one");
        clear_ref_name(&layout);
        let ref_name_path = layout.default_active_ref_name_path();
        let before = std::fs::read(&ref_name_path).unwrap();
        assert!(before.is_empty(), "the stranded state this verb repairs");

        fail_after_for_test(point, 0);
        let crashed = restore_queue_target(&layout, "heads/main", false);
        clear_failpoint_for_test();
        assert!(
            crashed.is_err(),
            "{point:?}: the injected failure must actually fire"
        );
        let after = std::fs::read(&ref_name_path).unwrap();
        assert!(
            after == before || after == b"heads/main",
            "{point:?}: ref-name must be fully the old value or fully the new one, got {after:?}"
        );

        if after == before {
            // The crash genuinely landed before the rename -- a second, clean run must finish
            // the job. (`MutableParentSync` fires *after* `renameat` already succeeded, so the
            // new value can already be on disk even though this call reported failure; that
            // case is handled below, not here.)
            restore_queue_target(&layout, "heads/main", false)
                .unwrap_or_else(|err| panic!("{point:?}: a clean second run completes it: {err}"));
        }
        assert_eq!(
            std::fs::read(&ref_name_path).unwrap(),
            b"heads/main",
            "{point:?}: the file must end up at the new value either way"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}

/// K3/K4's own "a control must be able to fail": the SAME failpoint, armed against the
/// truncate-then-append [`crate::write_active_ref_metadata`] this verb deliberately does NOT use
/// (its own doc explains why: that sequence is safe only when the WAL is empty, which it never is
/// here), does tear the file -- a crash between the truncate and the append leaves it empty,
/// neither the old value nor the new one. Confirms the atomic-replace sweep above is actually
/// sensitive to a real defect shape, not vacuously passing because nothing can tear this file.
#[test]
fn the_control_the_truncate_then_append_write_this_verb_avoids_can_still_tear() {
    let root = unique_temp_dir("rfc166-d5-restore-k4-control");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    let ref_name_path = layout.default_active_ref_name_path();
    assert_eq!(std::fs::read(&ref_name_path).unwrap(), b"heads/main");

    fail_after_for_test(TestFailPoint::AppendWrite, 0);
    let result = crate::write_active_ref_metadata(&layout, "heads/main");
    clear_failpoint_for_test();
    assert!(result.is_err(), "the injected failure must actually fire");
    let after = std::fs::read(&ref_name_path).unwrap();
    assert!(
        after.is_empty(),
        "the control must fail: a crashed truncate-then-append must leave ref-name empty -- \
         neither the old value nor the new one, got {after:?}"
    );
    std::fs::remove_dir_all(&root).ok();
}

// ---- K5: no other command writes the WAL, the witness, or ref-name ----------------------------

/// K5: `verify`, `status` (`doctor_repository`), and the other doctor repair modes all leave the
/// WAL, the witness, and `ref-name` byte-identical against a row-9 fixture -- none of them calls
/// this verb, and none of them can touch what only this verb's own classification has cleared to
/// act on.
#[test]
fn no_other_command_touches_the_wal_witness_or_ref_name_over_row9() {
    let root = unique_temp_dir("rfc166-d5-restore-k5");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    commit(&layout, "a.txt", b"one");
    clear_ref_name(&layout);
    assert_eq!(classify_now(&layout), Verdict::OwnershipMissing);

    // K5's own scope is the WAL, the witness, and `ref-name` specifically -- not the whole tree.
    // `--rebuild-pointer-index` legitimately advances the ref-pointer index's own generation log
    // even when it changes nothing else, so a whole-tree snapshot would be the wrong instrument.
    fn session_snapshot(layout: &RepositoryLayout) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        (
            std::fs::read(layout.active_queue_wal_path(DEFAULT_ACTIVE_NAME)).unwrap(),
            std::fs::read(
                layout
                    .active_session_dir(DEFAULT_ACTIVE_NAME)
                    .join("witness"),
            )
            .unwrap(),
            std::fs::read(layout.default_active_ref_name_path()).unwrap(),
        )
    }

    let before = session_snapshot(&layout);

    let _ = crate::verify_repository(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "verify must write nothing"
    );

    let _ = crate::doctor_repository(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "status/doctor_repository must write nothing"
    );

    let _ = crate::repair_repository(&layout, crate::DoctorRepairOptions::none());
    assert_eq!(
        before,
        session_snapshot(&layout),
        "repair_repository with nothing requested must write nothing"
    );
    let _ = crate::repair_repository(
        &layout,
        crate::DoctorRepairOptions {
            truncate_wal_tail: true,
            reconstruct_main_ref: false,
        },
    );
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--repair-wal-tail must write nothing over row 9 (no tail to repair)"
    );

    let _ = crate::repair_tails(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--repair-tails must write nothing over row 9 either"
    );

    let _ = crate::rebuild_pointer_index(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--rebuild-pointer-index touches the ref-pointer index only"
    );

    let _ = crate::discard_damaged_commits(&layout);
    assert_eq!(
        before,
        session_snapshot(&layout),
        "--discard-damaged-commits only acts on rows 4/5/7, never row 9"
    );

    std::fs::remove_dir_all(&root).ok();
}
