//! 0.58.0 R2: the history an adopting import walks and records (option (i)), and the commit's miss that walks instead of replaying
//! (option (ii)). Both write `verify`'s record, and both use the walk `seal` makes.
//!
//! Pinned by **counts**, not bytes: the walks made (`confirm_lineage_calls_for_test`) and the whole-lineage text replays made
//! (`whole_lineage_text_replays_for_test`). **Red controls**, each shown red when named: (1) record from the unchecked replay -- the
//! damaged-lineage tests; (2) skip the record -- `the_second_miss_anchors_instead_of_walking_again`; (3) run the walk and the old
//! replay both -- `a_miss_walks_and_never_also_replays`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeSet;

use prikk_object::CanonicalEncode as _;
use prikk_object::{BlockKind, BlockPayload, MerkleRoot, ObjectEnvelope, ObjectId, ObjectType};

use super::record_lineage;
use crate::block_state::confirm_lineage::confirm_lineage_calls_for_test;
use crate::confirmed_history::materialize_edited_text_recording;
use crate::lifecycle_cache::anchored_text::anchored_text_uses_for_test;
use crate::lifecycle_cache::materialize_edited_text;
use crate::lifecycle_cache::whole_lineage_text_replays_for_test;
use crate::object_store::{FileObjectStore, ObjectWriter};
use crate::test_gates::test_support::{
    AnchoredHistory, NODE_A, anchored_node, maintainer_signature,
};
use crate::verified_blocks::{load_verified_blocks, record_path};

/// A history as a receiver holds it: every Block present, no record of having confirmed any.
fn received(name: &str, total: usize) -> AnchoredHistory {
    let history = AnchoredHistory::standard(name, total);
    let _ = std::fs::remove_file(record_path(&history.layout));
    assert!(
        load_verified_blocks(&history.layout).is_empty(),
        "a receiver starts with an empty record"
    );
    history
}

fn recorded(history: &AnchoredHistory) -> BTreeSet<ObjectId> {
    (*load_verified_blocks(&history.layout)).clone()
}

/// A Block on `parent` that claims a state root its (empty) patch list does not produce: shape-valid, schema-valid, signed, and wrong
/// only in what it claims. A replay that applies its patches passes it; a walk that checks the root does not.
fn write_block_with_a_wrong_root(history: &mut AnchoredHistory, parent: ObjectId) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: vec![parent],
        kind: BlockKind::Normal,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0xEE_u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut block =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes().unwrap());
    block.add_signature(maintainer_signature()).unwrap();
    let store: &mut FileObjectStore = &mut history.store;
    store.write_object(&block).unwrap()
}

fn a_id(history: &AnchoredHistory) -> ObjectId {
    crate::text_span::text_blob_id(history.text(NODE_A)).unwrap()
}

/// (i). The import records the adopted lineage -- exactly the Blocks the walk confirmed, every one of them a held Block (S6's shape) --
/// and nothing past the tip it adopted.
#[test]
fn an_adopted_tip_records_its_lineage_and_only_that() {
    let history = received("r2-adopt-lineage", 9);
    let tip = history.blocks[5];
    let walks = confirm_lineage_calls_for_test();

    assert_eq!(record_lineage(&history.layout, &history.store, tip), None);

    assert_eq!(confirm_lineage_calls_for_test() - walks, 1);
    let record = recorded(&history);
    assert_eq!(
        record,
        history.blocks[..=5]
            .iter()
            .copied()
            .collect::<BTreeSet<_>>(),
        "the tip and its ancestors, not the Blocks after the tip"
    );
    for id in &record {
        assert!(
            crate::object_store::ObjectReader::read_typed(&history.store, *id, ObjectType::Block)
                .unwrap()
                .is_some(),
            "every recorded id is a held Block"
        );
    }
}

/// An import run again over a tip the record already holds walks nothing.
#[test]
fn an_import_over_a_recorded_tip_walks_nothing() {
    let history = received("r2-adopt-twice", 9);
    let tip = *history.blocks.last().unwrap();
    assert_eq!(record_lineage(&history.layout, &history.store, tip), None);
    let walks = confirm_lineage_calls_for_test();
    assert_eq!(record_lineage(&history.layout, &history.store, tip), None);
    assert_eq!(confirm_lineage_calls_for_test(), walks, "no second walk");
}

/// A Block that does not replay to the root it signs: the import is told which, the Blocks before it are recorded, it and everything
/// after it are not. **Red control 1 (record from the unchecked replay):** recording the lineage without the root check records the
/// bad Block and this goes red.
#[test]
fn a_block_that_does_not_replay_is_named_and_not_recorded() {
    let mut history = received("r2-adopt-damaged", 9);
    let good_tip = *history.blocks.last().unwrap();
    let bad = write_block_with_a_wrong_root(&mut history, good_tip);

    let note =
        record_lineage(&history.layout, &history.store, bad).expect("the walk names the Block");

    assert_eq!(note.block_id, bad);
    assert!(
        note.to_string().contains(&bad.to_string()) && note.to_string().contains("prikk verify"),
        "{note}"
    );
    let record = recorded(&history);
    assert!(
        !record.contains(&bad),
        "the Block that does not replay is not recorded"
    );
    assert_eq!(
        record,
        history.blocks.iter().copied().collect::<BTreeSet<_>>(),
        "the Blocks before it are"
    );
}

/// (ii). A miss on a repository that never confirmed this lineage: the walk replaces the unrecorded replay (0 whole-lineage replays),
/// the text is the old replay's, and the lineage is recorded. **Red control 3 (the walk and the old replay both):** a miss that also
/// replays makes the replay count 1 and this goes red.
#[test]
fn a_miss_walks_and_never_also_replays() {
    let history = received("r2-miss-walks", 9);
    let baseline = *history.blocks.last().unwrap();
    let horizon = history.blocks[0];
    let (walks, replays) = (
        confirm_lineage_calls_for_test(),
        whole_lineage_text_replays_for_test(),
    );

    let text = materialize_edited_text_recording(
        &history.layout,
        &history.store,
        baseline,
        horizon,
        anchored_node(NODE_A),
        a_id(&history),
    )
    .unwrap();

    assert_eq!(text.as_deref(), Some(history.text(NODE_A)));
    assert_eq!(confirm_lineage_calls_for_test() - walks, 1);
    assert_eq!(
        whole_lineage_text_replays_for_test() - replays,
        0,
        "the walk replaces the replay"
    );
    assert_eq!(
        recorded(&history),
        history.blocks.iter().copied().collect::<BTreeSet<_>>(),
        "and records what it confirmed"
    );
    // The text is the text the unrecorded replay yields.
    assert_eq!(
        materialize_edited_text(&history.store, baseline, horizon, anchored_node(NODE_A)).unwrap(),
        text
    );
}

/// The second miss reads the anchor: no walk, no whole-lineage replay. **Red control 2 (skip the record):** a miss that does not record
/// walks again, and this goes red.
#[test]
fn the_second_miss_anchors_instead_of_walking_again() {
    let history = received("r2-miss-twice", 9);
    let baseline = *history.blocks.last().unwrap();
    let horizon = history.blocks[0];
    let ask = || {
        materialize_edited_text_recording(
            &history.layout,
            &history.store,
            baseline,
            horizon,
            anchored_node(NODE_A),
            a_id(&history),
        )
        .unwrap()
    };
    assert!(ask().is_some());
    let (walks, replays, anchors) = (
        confirm_lineage_calls_for_test(),
        whole_lineage_text_replays_for_test(),
        anchored_text_uses_for_test(),
    );

    let again = ask();

    assert_eq!(again.as_deref(), Some(history.text(NODE_A)));
    assert_eq!(confirm_lineage_calls_for_test(), walks, "no second walk");
    assert_eq!(
        whole_lineage_text_replays_for_test(),
        replays,
        "0 whole-lineage replays"
    );
    assert_eq!(
        anchored_text_uses_for_test() - anchors,
        1,
        "it read the anchor"
    );
}

/// A lineage that does not replay: the commit's miss refuses, by Block, naming `prikk verify`, and records nothing. The old replay would
/// have authored on top of it. **Red control 1:** an unchecked replay that records would pass this lineage and record the bad Block.
#[test]
fn a_miss_over_a_damaged_lineage_refuses_and_records_nothing() {
    let mut history = received("r2-miss-damaged", 9);
    let good_tip = *history.blocks.last().unwrap();
    let bad = write_block_with_a_wrong_root(&mut history, good_tip);
    let replays = whole_lineage_text_replays_for_test();

    let refused = materialize_edited_text_recording(
        &history.layout,
        &history.store,
        bad,
        history.blocks[0],
        anchored_node(NODE_A),
        a_id(&history),
    )
    .expect_err("the walk refuses as verify would");

    let said = refused.to_string();
    assert!(
        said.contains(&bad.to_string()) && said.contains("prikk verify"),
        "{said}"
    );
    assert!(recorded(&history).is_empty(), "nothing is recorded");
    assert_eq!(
        whole_lineage_text_replays_for_test(),
        replays,
        "and no replay ran beside it"
    );
}
