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
