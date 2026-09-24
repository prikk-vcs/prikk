//! RFC 136 increment 2c controls on the DC-64 ladder: a one-block step that needs text an earlier block's
//! `EditText` produced takes it from a replay-verified anchor instead of replaying the whole lineage.
//!
//! Every fixture is sealed through `seal_block` (`AnchoredHistory`), so a checkpoint snapshot and a
//! replay-verified record exist. The step is observed three ways: the cache header (`steps + 1` for an
//! incremental step, `0` for a full replay), the whole state against `replay_derived_state`, and a `cfg(test)`
//! counter of answers taken from an anchor. A control that cannot fail is not one, so each test says what
//! perturbation of the product turns it red.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::super::*;
use crate::anchor_fallback::take_anchor_fallbacks;
use crate::lifecycle_cache::anchored_text::{
    anchored_text_uses_for_test, without_anchoring_for_test,
};
use crate::test_gates::test_support::{AnchoredHistory, NODE_A};

fn steps(history: &AnchoredHistory) -> Option<u32> {
    load(&history.layout).map(|cache| cache.steps_since_reanchor)
}

/// Prime the cache at block `k - 1` (a full replay: `steps == 0`), then resolve block `k` through the ladder.
/// Returns the state, the header's step count after, and how many texts came from an anchor.
fn step_to(
    history: &AnchoredHistory,
    k: usize,
) -> (ReplayDerivedLifecycleState, Option<u32>, usize) {
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    resolve_baseline_state(
        &history.layout,
        &history.store,
        history.blocks[k - 2],
        horizon,
    )
    .unwrap();
    assert_eq!(steps(history), Some(0), "the priming step is a full replay");
    let before = anchored_text_uses_for_test();
    let state = resolve_baseline_state(
        &history.layout,
        &history.store,
        history.blocks[k - 1],
        horizon,
    )
    .unwrap();
    (
        state,
        steps(history),
        anchored_text_uses_for_test() - before,
    )
}

fn full_replay(history: &AnchoredHistory, k: usize) -> ReplayDerivedLifecycleState {
    replay_derived_state(&history.store, history.blocks[k - 1], history.horizon()).unwrap()
}

/// Control 1. The DC-65 miss becomes incremental: the tip (block 3, then block 7) edits text an earlier block
/// edited, with the cache one block behind. **Perturb:** `try_incremental_step`'s
/// `MissingBlobForLifecycleEffect` arm returns `Ok(None)` -- every assertion on `steps == 1` goes red.
#[test]
fn a_dc65_miss_becomes_an_incremental_step() {
    let history = AnchoredHistory::standard("anchored-ladder-miss", 9);
    for k in [3_usize, 7] {
        // The fixture is not vacuous: without anchoring the same step is a DC-65 miss and a full replay.
        let (_, without, uses) = without_anchoring_for_test(|| step_to(&history, k));
        assert_eq!(
            without,
            Some(0),
            "block {k} misses on DC-65 with anchoring off"
        );
        assert_eq!(uses, 0);

        let (state, with, uses) = step_to(&history, k);
        assert_eq!(
            with,
            Some(1),
            "block {k} steps incrementally over the anchor"
        );
        assert_eq!(uses, 1, "and the text came from an anchor");
        assert_eq!(state.state(), full_replay(&history, k).state());
    }
    assert!(
        take_anchor_fallbacks().is_empty(),
        "nothing failed, nothing is named"
    );
}

/// Control 2. An anchor that is not in the replay-verified record is never consulted -- the snapshot still
/// passes the loader, so only the record stands between it and the step. The rung is full replay and the state is
/// full replay's. **Perturb:** drop `verified.contains(&current)` from `anchored_texts` (P2a; also dropping the
/// empty-record early return, P2b, reddens the lying-snapshot control too): `steps` becomes 1 and `uses` 1, red.
#[test]
fn an_anchor_outside_the_verified_record_is_never_consulted() {
    let history = AnchoredHistory::standard("anchored-ladder-unverified", 9);
    let record = crate::verified_blocks::record_path(&history.layout);
    assert!(
        record.exists(),
        "seal_block recorded the blocks it verified"
    );
    // A record that exists and is not empty, yet lacks the anchor (block 1, the checkpoint): it holds one later
    // block only. So the guard that matters is "is *this* anchor in the record", not "is there a record".
    std::fs::remove_file(&record).unwrap();
    crate::verified_blocks::record_verified_blocks(&history.layout, [history.blocks[4]]);
    assert!(!crate::verified_blocks::load_verified_blocks(&history.layout).is_empty());

    let (state, steps_after, uses) = step_to(&history, 3);
    assert_eq!(steps_after, Some(0), "a full replay");
    assert_eq!(uses, 0, "no anchor was consulted");
    assert_eq!(state.state(), full_replay(&history, 3).state());
    assert!(
        take_anchor_fallbacks().is_empty(),
        "an unrecorded anchor is skipped without being read, so there is nothing to name"
    );
}

/// Control 3, end to end. A carried text that does not hash to the content id the cached state names for its
/// node makes the step fall back, names the anchor, and leaves the answer right. The cache is poisoned so the
/// true text cannot match: node A's cached content id is one no text has. **Perturb:** remove the hash check in
/// `anchored_texts` (see `anchored_text/tests.rs` for the unit form, which goes red at once).
#[test]
fn a_text_that_fails_its_hash_falls_back_and_is_named() {
    let history = AnchoredHistory::standard("anchored-ladder-poisoned", 9);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    resolve_baseline_state(&history.layout, &history.store, history.blocks[1], horizon).unwrap();
    let cached = load(&history.layout).expect("a cache over block 2");

    let wrong = crate::text_span::text_blob_id(b"a content id nobody stored\n").unwrap();
    let node_a = crate::test_gates::test_support::anchored_node(NODE_A);
    let mut poisoned = NodeLifecycleState::new();
    for (node_id, node) in cached.state.live_nodes() {
        let mut node = node.clone();
        if *node_id == node_a {
            if let NodeContent::File { mode, .. } = node.content {
                node.content = NodeContent::File {
                    blob_id: wrong,
                    mode,
                };
            }
        }
        poisoned.seed_live_node(*node_id, node).unwrap();
    }
    persist(&history.layout, history.blocks[1], horizon, 0, &poisoned);

    let before = anchored_text_uses_for_test();
    let state = resolve_baseline_state(&history.layout, &history.store, history.blocks[2], horizon)
        .unwrap();
    assert_eq!(anchored_text_uses_for_test(), before, "no text was used");
    assert_eq!(steps(&history), Some(0), "a full replay repaired the cache");
    assert_eq!(state.state(), full_replay(&history, 3).state());
    let named = take_anchor_fallbacks();
    assert_eq!(
        named.len(),
        1,
        "the failing anchor is named once: {named:?}"
    );
    assert_eq!(
        named[0].block_id, history.blocks[0],
        "the anchor is the first block"
    );
    assert!(
        named[0].finding.contains("does not hash"),
        "{}",
        named[0].finding
    );
}

/// Control 4. Whole-state identity: at **every** tip of a history that crosses a second checkpoint (block 65),
/// edits the same files repeatedly and keeps a tombstone, the state the ladder returns -- history fields
/// included -- equals full replay's. **Perturb:** apply the retried block to a snapshot-shaped state (live nodes
/// only, no tombstones): red at the first tip after block 5.
#[test]
fn the_ladder_equals_full_replay_at_every_tip_across_a_checkpoint() {
    let history = AnchoredHistory::standard("anchored-ladder-identity", 70);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    let mut anchored_steps = 0;
    for k in 1..=history.blocks.len() {
        let before = anchored_text_uses_for_test();
        let state = resolve_baseline_state(
            &history.layout,
            &history.store,
            history.blocks[k - 1],
            horizon,
        )
        .unwrap();
        anchored_steps += anchored_text_uses_for_test() - before;
        assert_eq!(
            state.state(),
            full_replay(&history, k).state(),
            "the ladder differs from full replay at tip {k}"
        );
    }
    assert!(
        anchored_steps >= 10,
        "the run exercised anchored steps ({anchored_steps})"
    );
    let last = full_replay(&history, history.blocks.len());
    assert!(
        last.state().tombstones().count() >= 1,
        "the history keeps a tombstone"
    );
    assert!(take_anchor_fallbacks().is_empty());
}

// ---- RFC 136 increment 2c, Addendum 2: a request for the cached baseline itself ---------------------------

/// Control 1. Two resolutions at one baseline: the second is a hit -- `steps + 1`, not a full replay (which would
/// leave `steps == 0`) -- and the state is full replay's. **Perturb:** remove the `same_baseline_hit` branch: the
/// second resolution replays in full and `steps` is 0.
#[test]
fn a_second_resolution_at_one_baseline_is_a_hit_counted_as_a_step() {
    let history = AnchoredHistory::standard("anchored-hit-two", 9);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    let resolve = |k: usize| {
        resolve_baseline_state(
            &history.layout,
            &history.store,
            history.blocks[k - 1],
            horizon,
        )
        .unwrap()
    };
    resolve(4);
    assert_eq!(steps(&history), Some(0), "the first is a full replay");
    resolve(5);
    assert_eq!(steps(&history), Some(1), "a one-block step");
    let hit = resolve(5);
    assert_eq!(
        steps(&history),
        Some(2),
        "the same baseline again is a hit, and a step"
    );
    assert_eq!(hit.state(), full_replay(&history, 5).state());
    let again = resolve(5);
    assert_eq!(steps(&history), Some(3));
    assert_eq!(again.state(), hit.state());
}

/// Control 2. The whole state -- history fields included -- equals full replay's after a hit, at **every** tip of
/// the 70-block fixture (two resolutions per tip: the step, then the hit). **Perturb:** hand the hit a snapshot-shaped
/// state (live nodes only): red at the first tip after block 5.
#[test]
fn a_hit_equals_full_replay_at_every_tip_across_a_checkpoint() {
    let history = AnchoredHistory::standard("anchored-hit-identity", 70);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    let mut hits = 0;
    for k in 1..=history.blocks.len() {
        let resolve = || {
            resolve_baseline_state(
                &history.layout,
                &history.store,
                history.blocks[k - 1],
                horizon,
            )
            .unwrap()
        };
        let first = resolve();
        let after_first = steps(&history);
        let second = resolve();
        if steps(&history) == after_first.map(|steps| steps + 1) {
            hits += 1;
        }
        let truth = full_replay(&history, k);
        assert_eq!(first.state(), truth.state(), "the step differs at tip {k}");
        assert_eq!(second.state(), truth.state(), "the hit differs at tip {k}");
    }
    assert!(hits >= 30, "the run exercised hits ({hits})");
    assert!(take_anchor_fallbacks().is_empty());
}

/// Control 3. The reanchor still fires. Sixty-four uses made of steps and hits fill the cache's budget; the next
/// resolution is a full replay that resets `steps` to 0. **Perturb:** persist a hit with `steps` unchanged (do not
/// count it): the budget never fills and `steps == Some(64)` is never reached.
#[test]
fn the_reanchor_fires_after_sixty_four_uses_made_of_hits_and_steps() {
    let history = AnchoredHistory::standard("anchored-hit-reanchor", 40);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    let resolve = |k: usize| {
        resolve_baseline_state(
            &history.layout,
            &history.store,
            history.blocks[k - 1],
            horizon,
        )
        .unwrap()
    };
    resolve(1);
    assert_eq!(steps(&history), Some(0));
    // Alternate: a step to block k, then hits at block k, until 64 uses are made.
    let mut uses = 0;
    let mut k = 1;
    while uses < 64 {
        if uses % 3 == 0 {
            k += 1;
            resolve(k);
        } else {
            resolve(k);
        }
        uses += 1;
        assert_eq!(steps(&history), Some(uses), "use {uses}");
    }
    assert_eq!(steps(&history), Some(64));
    let state = resolve(k);
    assert_eq!(
        steps(&history),
        Some(0),
        "the next resolution is an independent full replay"
    );
    assert_eq!(state.state(), full_replay(&history, k).state());
}

/// Control 4. A poisoned cache at the baseline being asked for. `verify`'s check (`verify_divergence`) reports
/// it; a hit trusts it (as a step trusts it: the cache is trusted state, DC-64 §5's exposure), but counts as a
/// step, and once the budget is spent the reanchor overwrites it with the truth. The CLI-level form (`prikk
/// verify` names it) is `tests/rfc136_anchored_text.rs`. **Perturb:** do not count a hit as a step: the poisoned
/// state is never replaced.
#[test]
fn a_poisoned_cache_at_the_same_baseline_is_reported_and_then_overwritten() {
    let history = AnchoredHistory::standard("anchored-hit-poison", 9);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    resolve_baseline_state(&history.layout, &history.store, history.blocks[4], horizon).unwrap();
    let cached = load(&history.layout).unwrap();
    let wrong = crate::text_span::text_blob_id(b"a content id nobody stored\n").unwrap();
    let node_a = crate::test_gates::test_support::anchored_node(NODE_A);
    let mut poisoned = NodeLifecycleState::new();
    for (node_id, node) in cached.state.live_nodes() {
        let mut node = node.clone();
        if *node_id == node_a {
            if let NodeContent::File { mode, .. } = node.content {
                node.content = NodeContent::File {
                    blob_id: wrong,
                    mode,
                };
            }
        }
        poisoned.seed_live_node(*node_id, node).unwrap();
    }
    for (node_id, tombstone) in cached.state.tombstones() {
        poisoned
            .seed_tombstone(*node_id, tombstone.clone())
            .unwrap();
    }
    // One use short of the budget.
    persist(
        &history.layout,
        history.blocks[4],
        horizon,
        CHECKPOINT_CADENCE - 1,
        &poisoned,
    );

    let divergences = verify_divergence(&history.store, &history.layout);
    assert_eq!(
        divergences.len(),
        1,
        "verify reports the poisoned cache: {divergences:?}"
    );

    let hit = resolve_baseline_state(&history.layout, &history.store, history.blocks[4], horizon)
        .unwrap();
    assert_eq!(
        steps(&history),
        Some(CHECKPOINT_CADENCE),
        "the hit spent the last use"
    );
    assert_ne!(
        hit.state(),
        full_replay(&history, 5).state(),
        "a hit trusts the cache, as a step does"
    );

    let repaired =
        resolve_baseline_state(&history.layout, &history.store, history.blocks[4], horizon)
            .unwrap();
    assert_eq!(steps(&history), Some(0), "the reanchor is a full replay");
    assert_eq!(repaired.state(), full_replay(&history, 5).state());
    assert!(
        verify_divergence(&history.store, &history.layout).is_empty(),
        "and it overwrote the poison"
    );
}

/// Control 5, store form. A hit under `CacheWrite::Never` (what `diff` passes) writes nothing: the cache file is
/// byte-identical after, the state is the cached one. **Perturb:** persist on a hit whatever `cache_write` says.
#[test]
fn a_hit_that_must_not_write_leaves_the_cache_byte_identical() {
    let history = AnchoredHistory::standard("anchored-hit-never", 9);
    let horizon = history.horizon();
    let _ = std::fs::remove_file(cache_path(&history.layout));
    resolve_baseline_state(&history.layout, &history.store, history.blocks[4], horizon).unwrap();
    let before = std::fs::read(cache_path(&history.layout)).unwrap();
    let state = resolve_baseline_state_with(
        &history.layout,
        &history.store,
        history.blocks[4],
        horizon,
        CacheWrite::Never,
    )
    .unwrap();
    assert_eq!(state.state(), full_replay(&history, 5).state());
    assert_eq!(
        std::fs::read(cache_path(&history.layout)).unwrap(),
        before,
        "nothing was written"
    );
}
