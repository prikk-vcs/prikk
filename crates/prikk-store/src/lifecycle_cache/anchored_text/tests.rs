//! RFC 136 increment 2c controls on the anchored-text helper and on every production caller of the
//! `materialize_edited_text` family: `commit`, `merge-evidence`, the queued-patch fold and `diff`.
//!
//! **Per call site.** Each site's test builds a fresh history, runs **only that site** with no baseline cache (so
//! the ladder does a full replay and takes nothing from an anchor), and asserts the site answered from an anchor:
//! the `cfg(test)` counter moved by exactly what the site asked for. Removing the anchoring at one site (calling
//! `materialize_edited_text` there) turns **that** test red and no other; the perturbation each test names was
//! shown to do so. **Outputs.** The same call under `without_anchoring_for_test` must print/author the same.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_object::ObjectId;

use super::*;
use crate::anchor_fallback::take_anchor_fallbacks;
use crate::commit_boundary::worktree_patch::{Retain, read_worktree_for_diff};
use crate::lifecycle_cache::{materialize_edited_text, materialize_edited_text_anchored};
use crate::rfc111_seal_simulation::{SnapshotFixture, publish_snapshot_fixture_for_test_support};
use crate::test_gates::test_support::{AnchoredHistory, NODE_A, anchored_node};
use crate::{DEFAULT_ACTIVE_NAME, MergeEvidenceTarget, Wal, prepare_merge_evidence};

fn a_id(history: &AnchoredHistory) -> ObjectId {
    crate::text_span::text_blob_id(history.text(NODE_A)).unwrap()
}

fn uses() -> usize {
    anchored_text_uses_for_test()
}

/// Control 3, unit form. A verified anchor yields a text that does not hash to the id the caller says the file has:
/// `None`, and the anchor is named. With the right id the same call answers. **Perturb:** remove the hash check
/// in `anchored_texts` -- the wrong id is accepted and this goes red.
#[test]
fn a_text_that_does_not_hash_to_the_expected_id_is_refused_and_named() {
    let history = AnchoredHistory::standard("anchored-hash", 9);
    let from = history.blocks[6];
    let wrong = BTreeMap::from([(
        anchored_node(NODE_A),
        crate::text_span::text_blob_id(b"other\n").unwrap(),
    )]);
    assert!(anchored_texts(&history.layout, &history.store, from, &wrong).is_none());
    let named = take_anchor_fallbacks();
    assert_eq!(named.len(), 1);
    assert_eq!(
        named[0].block_id, history.blocks[0],
        "named after the anchor it used"
    );

    let right = BTreeMap::from([(anchored_node(NODE_A), a_id(&history))]);
    let texts =
        anchored_texts(&history.layout, &history.store, from, &right).expect("the anchor answers");
    assert_eq!(texts[&anchored_node(NODE_A)], history.text(NODE_A));
    assert!(take_anchor_fallbacks().is_empty());
}

/// Control 2 with the RFC 136 2b lying-snapshot helper. The snapshot **passes the loader** and claims `a.txt` holds
/// `asserted\n`; its block's patch produces `replayed\n`. Not in the verified record it is never consulted, and
/// nothing is named. Recorded as verified (a lie a repository could only tell by replay) it is consulted, and the
/// hash check -- the content id a full replay names -- refuses it and names it.
/// **Perturb:** remove the verified-record guard: the first half names a finding (red).
#[test]
fn a_lying_snapshot_is_skipped_unless_recorded_and_refused_when_recorded() {
    let history = AnchoredHistory::new("anchored-lying");
    let maintainer =
        crate::Ed25519MaintainerSigner::from_seed("anchored-lying-maintainer", &[0x2D; 32])
            .unwrap();
    let block = publish_snapshot_fixture_for_test_support(
        &history.layout,
        &maintainer,
        SnapshotFixture::Lying,
    )
    .unwrap();
    let node = prikk_object::NodeId::from_bytes([0x5A; 32]);
    let replayed = crate::text_span::text_blob_id(b"replayed\n").unwrap();
    let wanted = BTreeMap::from([(node, replayed)]);

    assert!(anchored_texts(&history.layout, &history.store, block, &wanted).is_none());
    assert!(
        take_anchor_fallbacks().is_empty(),
        "an unrecorded anchor is skipped without being read"
    );

    crate::verified_blocks::record_verified_blocks(&history.layout, [block]);
    assert!(anchored_texts(&history.layout, &history.store, block, &wanted).is_none());
    let named = take_anchor_fallbacks();
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].block_id, block);
}

/// A verified anchor whose manifest fails validation is named, never swallowed. **Perturb:** turn the
/// `refuse(err.to_string())` on the validation error into a bare `return None`: nothing is named (red).
#[test]
fn a_verified_anchor_whose_manifest_fails_validation_is_named() {
    let history = AnchoredHistory::new("anchored-damaged");
    let maintainer =
        crate::Ed25519MaintainerSigner::from_seed("anchored-damaged-maintainer", &[0x2E; 32])
            .unwrap();
    let block = publish_snapshot_fixture_for_test_support(
        &history.layout,
        &maintainer,
        SnapshotFixture::Damaged,
    )
    .unwrap();
    crate::verified_blocks::record_verified_blocks(&history.layout, [block]);
    let wanted = BTreeMap::from([(
        prikk_object::NodeId::from_bytes([0x5A; 32]),
        crate::text_span::text_blob_id(b"replayed\n").unwrap(),
    )]);
    assert!(anchored_texts(&history.layout, &history.store, block, &wanted).is_none());
    let named = take_anchor_fallbacks();
    assert_eq!(named.len(), 1, "{named:?}");
    assert_eq!(named[0].block_id, block);
    assert!(named[0].to_string().contains("verified snapshot of Block"));
}

// ---- per call site ---------------------------------------------------------------------------------------

/// The helper itself, as `commit`/`merge-evidence`/the queue call it. **Perturb:** the function skips the anchor
/// and calls `materialize_edited_text` straight away.
#[test]
fn the_helper_answers_from_an_anchor_and_matches_full_replay() {
    let history = AnchoredHistory::standard("anchored-site-helper", 9);
    let baseline = history.blocks[8];
    let before = uses();
    let text = materialize_edited_text_anchored(
        &history.layout,
        &history.store,
        baseline,
        history.horizon(),
        anchored_node(NODE_A),
        a_id(&history),
    )
    .unwrap()
    .expect("a text");
    assert_eq!(uses() - before, 1);
    assert_eq!(text, history.text(NODE_A));
    let replayed = materialize_edited_text(
        &history.store,
        baseline,
        history.horizon(),
        anchored_node(NODE_A),
    )
    .unwrap()
    .expect("a text");
    assert_eq!(text, replayed);
}

/// The queued-patch fallback (`replay.rs` `apply_queued_patch_envelopes`): a queued edit of A, folded on the
/// sealed baseline. **Perturb:** the `Some(layout)` arm calls `materialize_edited_text` instead.
#[test]
fn the_queued_patch_fold_takes_its_text_from_an_anchor() {
    let history = AnchoredHistory::standard("anchored-site-queue", 9);
    let mut history = history;
    let next = edited_a(&history, "queued");
    let op = history.edit(NODE_A, &next);
    history.queue(vec![op]);
    let replay = Wal::for_layout(&history.layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();

    let fold = || {
        let mut text_cache = replay::TextCache::new();
        crate::patch_replay::resolve_folded_worktree_baseline(
            &history.layout,
            &history.store,
            "heads/main",
            &replay,
            &mut text_cache,
        )
        .unwrap()
    };
    let before = uses();
    let anchored = fold();
    assert_eq!(
        uses() - before,
        1,
        "the queued edit's text came from an anchor"
    );
    let plain = without_anchoring_for_test(fold);
    assert_eq!(
        anchored.state, plain.state,
        "the folded baseline is the same either way"
    );
}

/// `merge-evidence` (`patch_algebra/evidence.rs` `baseline_text`): baseline block 6, the sides edit A. Its printed
/// evidence is identical with anchoring off. **Perturb:** drop `.with_layout(layout)` in `prepare_merge_evidence`.
#[test]
fn merge_evidence_takes_its_baseline_text_from_an_anchor_and_prints_the_same() {
    let history = AnchoredHistory::standard("anchored-site-evidence", 9);
    let run = || {
        prepare_merge_evidence(
            &history.layout,
            history.blocks[5],
            MergeEvidenceTarget::Block(history.blocks[6]),
            MergeEvidenceTarget::Block(history.blocks[7]),
        )
        .unwrap()
    };
    let before = uses();
    let anchored = run();
    assert!(
        uses() - before >= 1,
        "the sides edit A, whose baseline text nobody stored"
    );
    let plain = without_anchoring_for_test(run);
    assert_eq!(anchored, plain, "the evidence is byte-identical");
}

pub(crate) fn edited_a(history: &AnchoredHistory, tag: &str) -> Vec<u8> {
    let mut lines: Vec<String> = String::from_utf8(history.text(NODE_A).to_vec())
        .unwrap()
        .split_inclusive('\n')
        .map(str::to_string)
        .collect();
    lines[9] = format!("A-line-ten-{tag}\n");
    lines.concat().into_bytes()
}

pub(crate) fn write_tip_worktree(history: &AnchoredHistory, a: &[u8]) {
    let root = history.layout.root();
    std::fs::write(root.join(history.path(NODE_A)), a).unwrap();
    std::fs::write(
        root.join("b.txt"),
        history.text(crate::test_gates::test_support::NODE_B),
    )
    .unwrap();
    std::fs::write(
        root.join("d.txt"),
        history.text(crate::test_gates::test_support::NODE_D),
    )
    .unwrap();
}

/// `diff` (`worktree_read.rs` `baseline_entry`): the baseline side of a changed text file. **Perturb:**
/// `baseline_entry`'s text call goes through `materialize_edited_text`.
#[test]
fn diff_takes_the_baseline_text_from_an_anchor_and_reads_the_same() {
    let history = AnchoredHistory::standard("anchored-site-diff", 9);
    let a = edited_a(&history, "worktree");
    write_tip_worktree(&history, &a);
    let read = || {
        read_worktree_for_diff(&history.layout, "heads/main", Retain::ChangedFromBaseline).unwrap()
    };
    let before = uses();
    let anchored = read();
    assert_eq!(
        uses() - before,
        1,
        "the baseline side of a.txt came from an anchor"
    );
    let plain = without_anchoring_for_test(read);
    let bytes = |entries: &[crate::patch_replay::ReplayManifestEntry]| {
        entries
            .iter()
            .map(|entry| (entry.path.as_str().to_string(), entry.bytes.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(bytes(&anchored.baseline), bytes(&plain.baseline));
    assert_eq!(
        bytes(&anchored.baseline),
        vec![("a.txt".to_string(), history.text(NODE_A).to_vec())]
    );
}
