//! 0.53.0 step 3A (F4) controls 2-9: the cold `commit` baseline derives from the nearest trusted
//! checkpoint (RFC 159 stack B), instead of a full lineage replay. Control 1 (whole-state identity)
//! is `snapshot::tests::writer::rfc159_identity_on_a_history_with_renames_restorations_and_merges_anchored_at_baseline`,
//! reusing the same rich corpus stack A's own identity control uses.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::super::*;
use crate::anchor_trust::{anchor_uses_for_test_support, reset_anchor_uses_for_test_support};
use crate::commit_boundary::worktree_patch::{
    WorktreePatchCommitOptions, commit_worktree_changes_signed,
};
use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_after_for_test};
use crate::foundation::layout::ContainerSlot;
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout,
    add_trusted_maintainer,
};

const REF_NAME: &str = "heads/main";

fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("step3a-maintainer", &[0x72; 32]).expect("signer")
}

fn author() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("step3a-author", &[0x71; 32]).expect("signer")
}

/// A fresh repository with one checkpoint sealed (so `AnchorSite::Baseline` always has an admitted
/// anchor to find), under an adopted maintainer key.
fn repo_with_one_checkpoint(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let maintainer = maintainer();
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .unwrap();
    std::fs::write(layout.root().join("seed.txt"), b"seed\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        REF_NAME,
        "seed",
        WorktreePatchCommitOptions::default(),
        &author(),
    )
    .unwrap();
    // The genesis block is always a checkpoint (`checkpoint_due`: no snapshotted ancestor at all),
    // so this one seal is itself an admitted anchor for every later commit.
    simulate_one_seal(&layout, REF_NAME, &maintainer).unwrap();
    (layout, root)
}

fn commit_one_file(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        REF_NAME,
        "p3a",
        WorktreePatchCommitOptions::default(),
        &author(),
    )
    .unwrap();
}

/// Control 2. `AnchorSite::Baseline` is the site consulted by a cold `commit`'s own baseline
/// resolution, and no other site. **Perturb:** make the anchored arm of `resolve_baseline_state_with`
/// unconditionally return `None` (skip straight to the full-replay fallback): this test's own
/// `baseline` count goes to 0 and it fails, while nothing of stack A's own site controls
/// (`seal_block_anchors_at_the_seal_site` et al.) is affected, since they exercise `seal`, not
/// `commit`.
#[test]
fn a_cold_commit_anchors_at_the_baseline_site_and_no_other() {
    let (layout, root) = repo_with_one_checkpoint("step3a-site-baseline");
    // The seed commit above was already cold (no cache existed yet); it also resolved an anchored
    // baseline. Start the count fresh for the commit under test.
    reset_anchor_uses_for_test_support();
    // Force a cold resolution again: drop the incremental cache so this commit's baseline is not a
    // warm hit/step, exercising the anchored branch specifically, not DC-64's own warm path.
    let _ = std::fs::remove_file(cache_path(&layout));
    commit_one_file(&layout, "later.txt", b"later\n");
    assert_eq!(
        anchor_uses_for_test_support("baseline"),
        1,
        "one cold commit, one anchored baseline derivation"
    );
    for other in [
        "seal",
        "merge",
        "seal-from-accepted",
        "worktree-write",
        "snapshot-materialize",
    ] {
        assert_eq!(
            anchor_uses_for_test_support(other),
            0,
            "not the {other} site"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}

/// Control 3. The forced reanchor is always full, even with an admitted anchor available: it is
/// checked first, before the anchored attempt. **Perturb:** swap the order in
/// `resolve_baseline_state_with` so the anchored attempt runs before the forced-reanchor check:
/// this goes red (the mirror and the real path would then disagree, and `baseline` would be
/// consulted at the reanchor).
#[test]
fn the_forced_reanchor_is_full_even_with_an_anchor_available() {
    let (layout, root) = repo_with_one_checkpoint("step3a-forced-reanchor");
    commit_one_file(&layout, "a.txt", b"a\n");
    let mut at_bound = load(&layout).expect("warm cache");
    at_bound.steps_since_reanchor = CHECKPOINT_CADENCE;
    assert!(save(&layout, &at_bound).is_ok());

    reset_anchor_uses_for_test_support();
    commit_one_file(&layout, "b.txt", b"b\n");
    assert_eq!(
        anchor_uses_for_test_support("baseline"),
        0,
        "the forced reanchor must not consult the baseline anchor"
    );
    let after = load(&layout).expect("cache after the reanchor");
    assert_eq!(
        after.steps_since_reanchor, 0,
        "a forced reanchor is a full derivation, which always resets the counter"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Control 4 (F4-4). The counter counts baseline resolutions since the last **full** derivation;
/// an anchored derivation carries it forward, never resetting it. Across 200 resolutions
/// alternating a horizon switch (a branch publication, simulated by resetting the cache's own
/// baseline to a sibling block) and an ordinary commit, with an anchor available throughout, at
/// least 3 full derivations must still happen (the forced reanchor firing every `CHECKPOINT_CADENCE`
/// resolutions). **Perturb:** persist `0` on the anchored arm instead of `cached.steps_since_reanchor
/// + 1`: the reanchor condition is never reached and the full-derivation count drops to 0.
#[test]
fn an_anchored_derivation_carries_the_counter_forward_so_the_reanchor_still_fires() {
    let (layout, _root_guard) = repo_with_one_checkpoint("step3a-f4-4-counter");
    reset_anchor_uses_for_test_support();
    // A **genuine** full derivation is one that did not consult the baseline anchor at all --
    // `steps_since_reanchor == 0` alone is not enough to tell it apart from a *bugged* anchored
    // derivation that incorrectly also persists `0` (exactly the F4-4 defect this control is
    // for), since both would leave the same persisted value. The per-site anchor-use counter is
    // the one signal that actually distinguishes `replay_derived_state` (never touches `baseline`)
    // from `anchored_parent_state` (always does), whatever either persists afterward.
    let mut full_derivations = 0_usize;
    for i in 0..200 {
        if i % 2 == 0 {
            // A horizon switch: make the next resolution's claimed horizon differ from the cache's,
            // forcing it off the warm path and onto the anchored one (an admitted anchor is always
            // available in this fixture).
            if let Some(mut cached) = load(&layout) {
                cached.horizon_id = ObjectId::from_bytes([0xAB; 32]);
                let _ = save(&layout, &cached);
            }
        }
        let before = anchor_uses_for_test_support("baseline");
        commit_one_file(&layout, &format!("f{i}.txt"), b"x\n");
        let anchored_this_time = anchor_uses_for_test_support("baseline") > before;
        if load(&layout).map(|c| c.steps_since_reanchor) == Some(0) && !anchored_this_time {
            full_derivations += 1;
        }
    }
    assert!(
        full_derivations >= 3,
        "the forced reanchor must still fire periodically (as a genuine, un-anchored full \
         derivation) even though every other resolution is anchored or warm, got \
         {full_derivations} full derivations in 200"
    );
}

/// Control 5. With no adopted signer (an empty trust policy), there is no admitted anchor, so a cold
/// `commit` is a full derivation -- today's unmodified behaviour -- and still refuses over a damaged,
/// **untouched** historical blob, exactly as before this round.
#[test]
fn with_no_adopted_signer_a_cold_commit_still_full_replays_and_still_refuses_on_damage() {
    let root = unique_temp_dir("step3a-no-anchor-refuses");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let signer = maintainer();
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &prikk_hash::to_hex(&signer.public_key_bytes()),
    )
    .unwrap();
    std::fs::write(layout.root().join("gone.txt"), b"gone\n").unwrap();
    commit_worktree_changes_signed(
        &layout,
        REF_NAME,
        "gone",
        WorktreePatchCommitOptions::default(),
        &author(),
    )
    .unwrap();
    simulate_one_seal(&layout, REF_NAME, &signer).unwrap();
    // Now withdraw its adoption (keeping the policy non-empty with a different key, matching
    // `test_support::anchored_history::Signing::Unadopted`'s own established pattern): the
    // checkpoint is recorded and its manifest is valid, but its signer was never adopted here, so
    // `AnchorTrust::admit` (condition 2) can never return `Usable` for it.
    let other = Ed25519MaintainerSigner::from_seed("step3a-adopted-other", &[0x73; 32]).unwrap();
    add_trusted_maintainer(
        &layout,
        other.key_id(),
        &prikk_hash::to_hex(&other.public_key_bytes()),
    )
    .unwrap();
    crate::remove_trusted_maintainer(&layout, signer.key_id()).unwrap();

    // Damage the first file's blob on disk.
    let container = layout.container_slot_path(prikk_object::ObjectType::Blob, ContainerSlot::A);
    let mut bytes = std::fs::read(&container).unwrap();
    let marker = b"gone\n";
    let at = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("the marker is in the blob container");
    bytes[at] ^= 0x01;
    std::fs::write(&container, &bytes).unwrap();

    reset_anchor_uses_for_test_support();
    let _ = std::fs::remove_file(cache_path(&layout));
    std::fs::write(layout.root().join("unrelated.txt"), b"unrelated\n").unwrap();
    let result = commit_worktree_changes_signed(
        &layout,
        REF_NAME,
        "unrelated, over damage",
        WorktreePatchCommitOptions::default(),
        &author(),
    );
    assert!(
        result.is_err(),
        "with no admitted anchor, a cold commit still full-replays and still reads (and refuses \
         over) the damaged historical blob, unchanged by this round"
    );
    assert_eq!(
        anchor_uses_for_test_support("baseline"),
        0,
        "no anchor was ever usable, so the baseline site was never consulted"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// Control 6 (F4-2, the owner-approved exception, pinned). Below an admitted anchor, a damaged
/// blob of a file the commit does **not** touch no longer stops a cold `commit` -- `verify` still
/// reports it. The same damage, with a commit that **edits that exact file**, still refuses (the
/// edit forces a real read of the damaged blob regardless of the anchor).
#[test]
fn f4_2_a_cold_commit_proceeds_over_damage_below_the_anchor_in_a_file_it_does_not_touch_but_not_if_it_touches_it()
 {
    // (a) Untouched: succeeds, `verify` reports it.
    {
        let (layout, root) = repo_with_one_checkpoint("step3a-f4-2-untouched");
        let container =
            layout.container_slot_path(prikk_object::ObjectType::Blob, ContainerSlot::A);
        let mut bytes = std::fs::read(&container).unwrap();
        let marker = b"seed\n";
        let at = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("the marker is in the blob container");
        bytes[at] ^= 0x01;
        std::fs::write(&container, &bytes).unwrap();

        let _ = std::fs::remove_file(cache_path(&layout));
        std::fs::write(layout.root().join("new.txt"), b"new\n").unwrap();
        let result = commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "untouched, over damage",
            WorktreePatchCommitOptions::default(),
            &author(),
        );
        assert!(
            result.is_ok(),
            "F4-2: a cold commit no longer refuses over a damaged blob below the anchor in a file \
             it does not touch: {result:?}"
        );
        let verification = crate::verify_repository(&layout).unwrap();
        assert!(
            verification.has_item_failure() || verification.has_stage_failure(),
            "verify still reports the damage: {verification:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
    // (b) Touched: the commit edits the damaged file itself, and still refuses.
    {
        let (layout, root) = repo_with_one_checkpoint("step3a-f4-2-touched");
        let container =
            layout.container_slot_path(prikk_object::ObjectType::Blob, ContainerSlot::A);
        let mut bytes = std::fs::read(&container).unwrap();
        let marker = b"seed\n";
        let at = bytes
            .windows(marker.len())
            .position(|window| window == marker)
            .expect("the marker is in the blob container");
        bytes[at] ^= 0x01;
        std::fs::write(&container, &bytes).unwrap();

        let _ = std::fs::remove_file(cache_path(&layout));
        // Edit the damaged file itself.
        std::fs::write(layout.root().join("seed.txt"), b"replaced\n").unwrap();
        let result = commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "edits the damaged file",
            WorktreePatchCommitOptions::default(),
            &author(),
        );
        assert!(
            result.is_err(),
            "editing the damaged file itself must still read its blob and refuse, unchanged by \
             this round: {result:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

/// Control 7. A cold `diff` (`CacheWrite::Never`) on the anchored path leaves `.prikk/` byte for
/// byte identical, exactly as DC-64's own existing control for the warm path already requires.
#[test]
fn a_cold_diff_on_the_anchored_path_writes_nothing() {
    let (layout, root) = repo_with_one_checkpoint("step3a-diff-write-free");
    commit_one_file(&layout, "a.txt", b"a\n");
    let _ = std::fs::remove_file(cache_path(&layout));
    let before = directory_snapshot(&layout.root().join(".prikk"));

    let reader = crate::object_store::ObjectReadSnapshot::open(&layout).unwrap();
    let (baseline_block, horizon) =
        crate::patch_replay::resolve_node_lineage_bounds(&layout, REF_NAME).unwrap();
    let result =
        resolve_baseline_state_with(&layout, &reader, baseline_block, horizon, CacheWrite::Never);
    assert!(result.is_ok());

    let after = directory_snapshot(&layout.root().join(".prikk"));
    assert_eq!(
        before, after,
        "CacheWrite::Never must leave .prikk/ untouched"
    );
    let _ = std::fs::remove_dir_all(&root);
}

fn directory_snapshot(dir: &std::path::Path) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<(std::path::PathBuf, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else {
                out.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    walk(dir, &mut out);
    out.sort();
    out
}

/// Control 8. `verify` is never anchored: `verify_divergence` calls `replay_derived_state` directly
/// and never reaches `anchored_parent_state`, even with an admitted anchor available.
#[test]
fn verify_never_consults_the_baseline_anchor() {
    let (layout, root) = repo_with_one_checkpoint("step3a-verify-never-anchored");
    commit_one_file(&layout, "a.txt", b"a\n");
    reset_anchor_uses_for_test_support();
    let verification = crate::verify_repository(&layout).unwrap();
    assert!(
        !verification.has_item_failure() && !verification.has_stage_failure(),
        "fixture sanity: a healthy repository verifies clean: {verification:?}"
    );
    assert_eq!(
        anchor_uses_for_test_support("baseline"),
        0,
        "verify must never consult the baseline anchor"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// (0.53.0 step 3A round 2, R-2.) The shape of a healthy post-recovery (or never-crashed) queue
/// for this fixture: two queued patches, `"warm"` then `"cold anchored commit"`, the second a
/// single `create-file` of `killed.txt` and nothing else. Returns `None` on any read error, which
/// the caller then fails on, same as any other broken post-condition.
fn killed_file_queue_shape(layout: &RepositoryLayout) -> Option<(usize, bool)> {
    let entries = crate::enumerate_queued_patches(layout).ok()?;
    let killed_once_correctly = entries.iter().any(|entry| {
        entry.message.as_deref() == Some("cold anchored commit")
            && entry.operations.len() == 1
            && entry.operations[0].kind == "create-file"
            && entry.operations[0].paths.iter().any(
                |path| matches!(path, crate::QueuedPathResolution::Path(p) if p == "killed.txt"),
            )
    });
    Some((entries.len(), killed_once_correctly))
}

/// Control 9. A commit kill sweep: a cold, anchored commit crashed at every write ordinal of the
/// failpoint families a commit's own write path can hit (the project's established convention,
/// `commit_boundary::worktree_patch::ref_name_once_per_session_tests`). **0.53.0 step 3A round 2
/// (R-2):** the architect's own probe found the round-1 version of this test counted every *armed*
/// ordinal (100) as "recovered," including 86 that never actually reached their failpoint at all
/// (each family's own write sequence is shorter than 20 steps) -- overstating the evidence by 7x.
/// This version sweeps each family only up to the first ordinal that survives uninterrupted
/// (`failpoint_armed_for_test()` still `true` afterward, meaning the point was never reached --
/// every later ordinal in that fixed-length family would be the same), counts n/N over ordinals
/// that genuinely fired, and checks a real post-condition against an independently-run,
/// uninterrupted commit of the identical fixture: `verify` reports no failed item afterward, and
/// the queue holds exactly `killed.txt`'s own commit, once, with the right content -- not merely
/// that a retry's error message happened to mention a repair verb's name.
#[test]
fn a_cold_anchored_commit_kill_sweep_recovers_at_every_write_ordinal() {
    const MAX_ORDINAL: usize = 20;

    // The ground truth this fixture is checked against: build the identical sequence once, never
    // crashed, and capture its queue shape.
    let reference_root = unique_temp_dir("step3a-kill-reference");
    let reference_shape = {
        let layout = RepositoryLayout::init(reference_root.clone()).unwrap();
        let maintainer = maintainer();
        add_trusted_maintainer(
            &layout,
            maintainer.key_id(),
            &prikk_hash::to_hex(&maintainer.public_key_bytes()),
        )
        .unwrap();
        std::fs::write(layout.root().join("seed.txt"), b"seed\n").unwrap();
        commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "seed",
            WorktreePatchCommitOptions::default(),
            &author(),
        )
        .unwrap();
        simulate_one_seal(&layout, REF_NAME, &maintainer).unwrap();
        std::fs::write(layout.root().join("warm.txt"), b"warm\n").unwrap();
        commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "warm",
            WorktreePatchCommitOptions::default(),
            &author(),
        )
        .unwrap();
        std::fs::write(layout.root().join("killed.txt"), b"killed\n").unwrap();
        commit_worktree_changes_signed(
            &layout,
            REF_NAME,
            "cold anchored commit",
            WorktreePatchCommitOptions::default(),
            &author(),
        )
        .unwrap();
        killed_file_queue_shape(&layout).expect("the reference fixture reads back")
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
        let mut armed_count = 0_usize;
        let mut fired_count = 0_usize;
        let mut failed_count = 0_usize;
        for ordinal in 0..MAX_ORDINAL {
            armed_count += 1;
            let root = unique_temp_dir(&format!("step3a-kill-{point:?}-{ordinal}"));
            let layout = RepositoryLayout::init(root.clone()).unwrap();
            let maintainer = maintainer();
            add_trusted_maintainer(
                &layout,
                maintainer.key_id(),
                &prikk_hash::to_hex(&maintainer.public_key_bytes()),
            )
            .unwrap();
            std::fs::write(layout.root().join("seed.txt"), b"seed\n").unwrap();
            commit_worktree_changes_signed(
                &layout,
                REF_NAME,
                "seed",
                WorktreePatchCommitOptions::default(),
                &author(),
            )
            .unwrap();
            simulate_one_seal(&layout, REF_NAME, &maintainer).unwrap();
            let _ = std::fs::remove_file(cache_path(&layout));

            // An ordinary, uncrashed cold-anchored commit first (matching this project's own
            // established kill-sweep convention of crashing a *second* commit, not session/lock
            // setup itself -- lock acquisition is a pre-existing, separately tested concern, not
            // something this round changes).
            std::fs::write(layout.root().join("warm.txt"), b"warm\n").unwrap();
            commit_worktree_changes_signed(
                &layout,
                REF_NAME,
                "warm",
                WorktreePatchCommitOptions::default(),
                &author(),
            )
            .unwrap();
            // Force the commit under test to be cold/anchored too, not a DC-64 warm hit off the
            // "warm" commit just above.
            let _ = std::fs::remove_file(cache_path(&layout));

            std::fs::write(layout.root().join("killed.txt"), b"killed\n").unwrap();
            fail_after_for_test(point, ordinal);
            let crashed = commit_worktree_changes_signed(
                &layout,
                REF_NAME,
                "cold anchored commit",
                WorktreePatchCommitOptions::default(),
                &author(),
            );
            let still_armed = crate::foundation::fsutil::failpoint_armed_for_test();
            clear_failpoint_for_test();

            if still_armed {
                // This ordinal's own write sequence never reached `point` -- a fixed, finite
                // length per family, so every later ordinal would be the same. Stop here; this
                // family's own n is `fired_count`, not `MAX_ORDINAL`.
                let _ = std::fs::remove_dir_all(&root);
                break;
            }
            fired_count += 1;
            fired_total += 1;
            if crashed.is_ok() {
                // Fired, but the commit still succeeded: a swallowed write, not a required one.
                swallowed.push(format!("{point:?}@{ordinal}"));
            } else {
                failed_count += 1;
            }

            // The repository must not be left permanently stuck: either the crashed commit already
            // succeeded (nothing to retry), or retrying the identical commit now succeeds, or a
            // stale active lock is released through the existing recovery (`prikk unlock`'s own
            // primitive, `unlock::clear_lock`) and *that* retry succeeds, or a refusal that genuinely
            // names a `doctor` repair is resolved by running that repair for real, then retrying.
            let mut retry = if crashed.is_ok() {
                None
            } else {
                Some(commit_worktree_changes_signed(
                    &layout,
                    REF_NAME,
                    "cold anchored commit",
                    WorktreePatchCommitOptions::default(),
                    &author(),
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
                    REF_NAME,
                    "cold anchored commit",
                    WorktreePatchCommitOptions::default(),
                    &author(),
                ));
            }
            if matches!(&retry, Some(Err(err)) if err.to_string().contains("doctor")) {
                // A refusal that genuinely names a `doctor` repair: run the repair for real
                // (RFC 166 D5's own verb), not merely accept the message as success.
                let _ = crate::discard_damaged_commits(&layout);
                retry = Some(commit_worktree_changes_signed(
                    &layout,
                    REF_NAME,
                    "cold anchored commit",
                    WorktreePatchCommitOptions::default(),
                    &author(),
                ));
            }

            // `verify` is a read-only report: it must always complete (never itself error out),
            // whatever the crash left behind -- reporting a problem is a value it returns, not a
            // failure to run.
            let verification = crate::verify_repository(&layout);
            let verify_clean = verification
                .as_ref()
                .map(|v| !v.has_stage_failure() && !v.has_item_failure())
                .unwrap_or(false);

            let commit_recovers = crashed.is_ok()
                || match &retry {
                    Some(Ok(_)) => true,
                    // RFC 166 D3 row 2 (`Pending`): a crash between the WAL's durable append and
                    // the witness write leaves an acknowledged-looking record the witness has not
                    // caught up to yet. The content is already durably queued, so retrying the
                    // identical, unchanged file correctly says there is nothing new to commit --
                    // documented, intended behaviour (RFC 166 §13 item 13), not a dead end.
                    Some(Err(err)) => {
                        let message = err.to_string();
                        message.contains("nothing left to commit")
                            || message.contains("not confirmed")
                            || message.contains("no node-addressed changes to commit")
                    }
                    None => true,
                };
            let queue_shape = killed_file_queue_shape(&layout);
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
            "{point:?}: armed {armed_count}, fired {fired_count}, fired-and-failed {failed_count}"
        ));
    }
    assert!(
        fired_total >= 1,
        "the sweep must genuinely fire at least once; it fired {fired_total} times across {per_family:?}"
    );
    // Every ordinal counted in `fired_total` was confirmed to have actually fired (the loop only
    // increments it right after `still_armed` reads false) and recovered cleanly (the `assert!`
    // above panics on the first ordinal that does not) -- so reaching here means n/N = fired_total
    // / fired_total, every one of them a genuine crash, not an un-fired arm.
    eprintln!(
        "kill sweep: {fired_total} ordinals genuinely fired and recovered, {per_family:?}, \
         swallowed (fired but did not fail the commit): {swallowed:?}"
    );
}
