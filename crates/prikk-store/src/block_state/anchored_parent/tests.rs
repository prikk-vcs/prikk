//! RFC 159 stack A controls: the state `seal`, `merge` and seal-from-accepted continue from, derived from an
//! authenticated snapshot. Each control's doc comment names the perturbation that turns it red (each was shown red).
//!
//! **Beware the masked guard** -- this project has met it three times (RFC 136 2c, RFC 159's design round twice): a control
//! whose perturbed guard is hidden behind an earlier one stays green. Each condition below is reached *past every earlier
//! one*: the positive twin of a control passes all four conditions, and the control breaks exactly one.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType,
};

use super::*;
use crate::anchor_fallback::take_anchor_fallbacks;
use crate::anchor_trust::{anchor_uses_for_test_support, reset_anchor_uses_for_test_support};
use crate::block_state::{
    LineageStateMemo, StateAnchoring, derive_next_state_for_candidate_choosing,
    resolved_parent_state,
};
use crate::memory_store::MemoryObjectStore;
use crate::test_gates::test_support::AnchoredHistory;
use crate::verified_blocks::{load_verified_blocks, record_path, record_verified_blocks};
use crate::{
    DEFAULT_ACTIVE_NAME, ObjectWriter, RepositoryLayout, SnapshotManifest, Wal,
    maintainer_signature,
};

const SEAL: StateAnchoring = StateAnchoring::Anchored(AnchorSite::Seal);
/// The fixture's node C (`anchored_history.rs`'s `NODE_C`, not re-exported).
const NODE_C: u8 = 0xC1;

fn patches_of(history: &AnchoredHistory, index: usize) -> Vec<ObjectId> {
    read_checked_block(&history.store, history.blocks[index])
        .expect("a block")
        .patch_ids
}

fn parent_of(history: &AnchoredHistory, index: usize) -> Option<ObjectId> {
    index.checked_sub(1).map(|p| history.blocks[p])
}

fn derive(
    history: &AnchoredHistory,
    index: usize,
    anchoring: StateAnchoring,
) -> crate::block_state::CandidateDerivation {
    derive_next_state_for_candidate_choosing(
        &history.layout,
        &history.store,
        parent_of(history, index),
        &patches_of(history, index),
        anchoring,
    )
    .expect("derivation")
}

/// **Whole-state identity at every block** (RFC 159 §2 control 1): the standard fixture -- three checkpoints, a tombstone
/// that stays, a delete-and-restore of one node id, repeated edits -- with every block sealed again on its own parent by
/// the anchored derivation and compared with an **independent forward replay from genesis** (the lifecycle state,
/// tombstones and `seen_ids` included; the entries; the root against the signed root; the text of every unstored blob),
/// and every 8th block with the literal full walk. The history with renames, mode changes, and merge blocks below and
/// above the anchor is `snapshot::tests::writer::rfc159_identity_on_a_history_with_renames_restorations_and_merges`.
/// **Perturb:** skip `seed_tombstone` in `anchored_parent_state`: the restoration at block 9 and every block after it
/// differ from the forward replay.
#[test]
fn every_block_derives_the_same_state_anchored_and_by_the_full_walk() {
    let history = AnchoredHistory::standard("rfc159-identity", 127);
    let tip = history.blocks.last().unwrap().to_hex();
    let report = rfc159_identity_probe(&history.layout, &tip, 8).expect("probe");
    assert!(report.differences.is_empty(), "{:?}", report.differences);
    assert_eq!(report.blocks, 127);
    assert!(
        report.anchored >= report.blocks - 1,
        "every block after the first is anchored, not {}",
        report.anchored
    );
    assert!(
        report.max_folded <= MAX_ANCHOR_DISTANCE,
        "the fold never exceeds the bound"
    );
    assert!(report.literal_full_compared >= 15);
}

/// **A deleted node comes back only as itself** (RFC 159 §2 control 2). Node C is deleted at block 5, so its tombstone lives
/// below the anchor (block 65). A candidate that recreates it with other content is refused by the full walk
/// (restoration-equivalence against the tombstone) and must be refused by the anchored derivation; one that restores it
/// exactly is accepted by both, with the same state. A *valid* restoration produces the same store with or without the
/// history fields (the guard never fires), so **byte-identity of commands cannot see them: only this negative case can.**
/// **Perturb:** skip `seed_tombstone`, **or** make the id-only walk return no tombstones: the anchored derivation accepts
/// the bad restoration and this goes red.
#[test]
fn a_restoration_that_does_not_match_the_tombstone_is_refused_by_both_derivations() {
    let mut history = AnchoredHistory::standard("rfc159-tombstone", 80);
    let tip = *history.blocks.last().unwrap();
    let mut store = history.store.clone();
    let mut candidate = |path: &str, bytes: &[u8]| -> ObjectId {
        let op = history.create(path, NODE_C, bytes);
        history.queue(vec![op]);
        let replay = Wal::for_layout(&history.layout, DEFAULT_ACTIVE_NAME)
            .replay()
            .unwrap();
        let envelope = replay.records.last().unwrap().envelope.clone();
        ObjectWriter::write_object(&mut store, &envelope).unwrap()
    };
    let bad = candidate("c.txt", b"not the original\n");
    let good = candidate("c.txt", b"c\n");
    let derive = |patch: ObjectId, anchoring: StateAnchoring| {
        derive_next_state_for_candidate_choosing(
            &history.layout,
            &store,
            Some(tip),
            &[patch],
            anchoring,
        )
    };
    assert!(
        derive(bad, StateAnchoring::Never).is_err(),
        "the full walk refuses it"
    );
    assert!(
        derive(bad, SEAL).is_err(),
        "so must the anchored derivation"
    );
    let full = derive(good, StateAnchoring::Never).expect("an exact restoration is accepted");
    let anchored = derive(good, SEAL).expect("and by the anchored derivation");
    assert!(anchored.anchored, "the anchor was used");
    assert_eq!(full.state, anchored.state);
    assert_eq!(full.entries, anchored.entries);
}

/// **A missing record, or a record that does not name the anchor, means the full walk, and the same result.** Two cases,
/// because an empty record short-circuits before the membership test: with none, the membership test is never reached (a
/// **masked guard**), so case (a) records *another* block and leaves the checkpoint -- signed by an adopted key,
/// its manifest valid, 5 blocks back -- unrecorded, which reaches condition 1 past conditions 2-4 alone.
/// **Perturb:** drop the membership test in `admit`: case (a) goes red (case (b) cannot: it never reaches `admit`).
#[test]
fn a_missing_record_and_a_record_that_omits_the_anchor_mean_the_full_walk_with_the_same_result() {
    let history = AnchoredHistory::standard("rfc159-no-record", 80);
    let index = 70;
    // The positive twin: recorded, adopted, valid, near: anchored.
    assert!(
        derive(&history, index, SEAL).anchored,
        "the twin is anchored"
    );
    let full = derive(&history, index, StateAnchoring::Never);

    // (a) A record that names other blocks and not the checkpoint at block 65.
    let checkpoint = history.blocks[65 - 1];
    assert!(
        read_checked_block(&history.store, checkpoint)
            .expect("a block")
            .snapshot_blob_ref
            .is_some(),
        "fixture sanity: block 65 is a checkpoint"
    );
    std::fs::remove_file(record_path(&history.layout)).expect("the seal wrote a record");
    record_verified_blocks(&history.layout, [history.blocks[3]]);
    assert!(!load_verified_blocks(&history.layout).contains(&checkpoint));
    let unrecorded = derive(&history, index, SEAL);
    assert!(
        !unrecorded.anchored,
        "an unrecorded checkpoint is no anchor"
    );
    assert_eq!(unrecorded.state, full.state);
    assert_eq!(unrecorded.entries, full.entries);

    // (b) No record at all.
    std::fs::remove_file(record_path(&history.layout)).expect("the record");
    assert!(load_verified_blocks(&history.layout).is_empty());
    let none = derive(&history, index, SEAL);
    assert!(!none.anchored, "with no record there is no anchor");
    assert_eq!(none.state, full.state);
    assert!(
        take_anchor_fallbacks().is_empty(),
        "not recorded is not an integrity signal: nothing is named"
    );
}

/// **The distance bound, both sides of it** (RFC 159 §8.2 condition 4). With only block 0 recorded, the parent at index 63
/// is exactly 63 blocks past the anchor and is anchored; the parent at index 64 is 64 past it and is not. Both reach the
/// bound past every other condition: the anchor is recorded, adopted and its manifest valid (the positive side proves it).
/// **Perturb:** raise `MAX_ANCHOR_DISTANCE`: the 64-back parent is anchored and this goes red.
#[test]
fn the_distance_bound_is_exactly_63_blocks() {
    let history = AnchoredHistory::standard("rfc159-far", 110);
    std::fs::remove_file(record_path(&history.layout)).unwrap();
    record_verified_blocks(&history.layout, [history.blocks[0]]);
    let at_63 = derive(&history, 64, SEAL);
    assert!(at_63.anchored, "63 past the anchor is anchored");
    assert_eq!(at_63.folded, 63);
    let at_64 = derive(&history, 65, SEAL);
    assert!(!at_64.anchored, "64 past the anchor is not");
    assert!(
        take_anchor_fallbacks().is_empty(),
        "too far back is silent, not a signal"
    );
    let full = derive(&history, 65, StateAnchoring::Never);
    assert_eq!(at_64.state, full.state);
}

/// **A block after the anchor whose signed root its own patches do not produce is not believed.** The block after the
/// anchor is rewritten to carry the root of the block after *that*: it decodes, is shape-valid, and its parent is a
/// recorded, adopted checkpoint. The anchored derivation must refuse (`None`, so the full walk, which errors, runs).
/// **Perturb:** drop the root comparison in the fold: the anchored derivation returns a state for a lineage the full walk
/// refuses, and this goes red.
#[test]
fn a_folded_block_whose_root_does_not_replay_is_refused() {
    let history = AnchoredHistory::standard("rfc159-wrong-root", 20);
    let mut wrong = read_checked_block(&history.store, history.blocks[1]).expect("a block");
    let other = read_checked_block(&history.store, history.blocks[2]).expect("a block");
    assert_ne!(wrong.state_merkle_root, other.state_merkle_root);
    wrong.state_merkle_root = other.state_merkle_root;
    let mut store = history.store.clone();
    let envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, wrong.to_canonical_bytes().unwrap());
    let forged = envelope.object_id();
    ObjectWriter::write_object(&mut store, &envelope).unwrap();
    assert!(
        resolved_parent_state(&history.store, Some(forged), &mut LineageStateMemo::new()).is_err(),
        "the full walk refuses a block whose root does not replay"
    );
    assert!(
        anchored_parent_state(&history.layout, &history.store, forged, AnchorSite::Seal).is_none(),
        "the anchored derivation refuses it too, and leaves the verdict to the full walk"
    );
}

// ---- The (b) items: every check the id-only walk keeps, each with a damaged object below the anchor ------------------

/// A hand-built lineage in an in-memory store, which -- unlike the file store -- will hold an object the repository
/// format would refuse: `root` (a valid Root), `middle` (the block under test, below the anchor), and `anchor` (a
/// checkpoint carrying a valid empty-state snapshot, **signed by the fixture's adopted maintainer and recorded**, so the
/// anchor passes all four conditions and only what is below it is in question). The layout is the fixture's, so the
/// record and the trust policy are real.
struct Damaged {
    layout_holder: AnchoredHistory,
    store: MemoryObjectStore,
    anchor: ObjectId,
}

/// What the block `middle` is.
enum Middle {
    /// A valid empty Normal block: the positive twin.
    Ok,
    /// Names a patch the store does not hold.
    MissingPatch,
    /// Names an object that is not a Patch.
    NotAPatch,
    /// A Patch whose envelope schema the format does not admit.
    UnadmittedPatchSchema,
    /// A Patch whose payload does not decode.
    UndecodablePatch,
    /// A Block of an envelope schema other than 2.
    WrongBlockSchema,
    /// A block of a kind format 2 does not authorize (`Repair`).
    WrongShape,
    /// A Block whose payload does not decode.
    UndecodableBlock,
    /// The anchor's parent is an object that is not a Block.
    NotABlock,
    /// The anchor's parent is not in the store.
    MissingBlock,
}

fn empty_state_root() -> prikk_object::MerkleRoot {
    crate::state_root::compute_state_root(&[]).unwrap()
}

fn damaged_chain(name: &str, middle: &Middle) -> Damaged {
    let history = AnchoredHistory::new(name);
    let mut store = MemoryObjectStore::new();
    let empty = empty_state_root();
    let block = |kind: BlockKind,
                 parents: Vec<ObjectId>,
                 patches: Vec<ObjectId>,
                 snapshot: Option<ObjectId>,
                 schema: u32| {
        let payload = BlockPayload {
            parent_block_ids: parents,
            kind,
            patch_ids: patches,
            state_merkle_root: empty,
            snapshot_blob_ref: snapshot,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        };
        ObjectEnvelope::unsigned(
            ObjectType::Block,
            schema,
            payload.to_canonical_bytes().unwrap(),
        )
    };
    let root_envelope = block(BlockKind::Root, Vec::new(), Vec::new(), None, 2);
    let root = store.write_object(&root_envelope).unwrap();
    // A real one-operation patch, taken from a scratch history's queue, re-wrapped where a variant needs it.
    let real_patch = {
        let mut scratch = AnchoredHistory::new("rfc159-damaged-patch-source");
        let op = scratch.create("p.txt", 0xA1, b"p\n");
        scratch.queue(vec![op]);
        let replay = Wal::for_layout(&scratch.layout, DEFAULT_ACTIVE_NAME)
            .replay()
            .unwrap();
        replay.records.last().unwrap().envelope.clone()
    };
    let empty_patch = |schema: u32| {
        ObjectEnvelope::unsigned(
            ObjectType::Patch,
            schema,
            real_patch.canonical_payload.clone(),
        )
    };
    let mut parent_of_anchor: Option<ObjectId> = None;
    let middle_id = match middle {
        Middle::Ok => store
            .write_object(&block(BlockKind::Normal, vec![root], Vec::new(), None, 2))
            .unwrap(),
        Middle::MissingPatch => store
            .write_object(&block(
                BlockKind::Normal,
                vec![root],
                vec![ObjectId::from_bytes([0x77; 32])],
                None,
                2,
            ))
            .unwrap(),
        Middle::NotAPatch => {
            // A valid patch's bytes under another object type: decoding alone would accept it, so only the
            // object-type check refuses it.
            let not_a_patch = store
                .write_object(&ObjectEnvelope::unsigned(
                    ObjectType::Blob,
                    1,
                    real_patch.canonical_payload.clone(),
                ))
                .unwrap();
            store
                .write_object(&block(
                    BlockKind::Normal,
                    vec![root],
                    vec![not_a_patch],
                    None,
                    2,
                ))
                .unwrap()
        }
        Middle::UnadmittedPatchSchema => {
            let patch = store.write_object(&empty_patch(99)).unwrap();
            store
                .write_object(&block(BlockKind::Normal, vec![root], vec![patch], None, 2))
                .unwrap()
        }
        Middle::UndecodablePatch => {
            let garbage = ObjectEnvelope::unsigned(ObjectType::Patch, 1, vec![0xFF; 9]);
            let patch = store.write_object(&garbage).unwrap();
            store
                .write_object(&block(BlockKind::Normal, vec![root], vec![patch], None, 2))
                .unwrap()
        }
        Middle::WrongBlockSchema => store
            .write_object(&block(BlockKind::Normal, vec![root], Vec::new(), None, 3))
            .unwrap(),
        Middle::WrongShape => store
            .write_object(&block(BlockKind::Repair, vec![root], Vec::new(), None, 2))
            .unwrap(),
        Middle::UndecodableBlock => store
            .write_object(&ObjectEnvelope::unsigned(
                ObjectType::Block,
                2,
                vec![0xFF; 9],
            ))
            .unwrap(),
        // A valid Block's bytes, schema 2, under another object type: only the object-type check refuses it.
        Middle::NotABlock => store
            .write_object(&ObjectEnvelope::unsigned(
                ObjectType::Patch,
                2,
                root_envelope.canonical_payload.clone(),
            ))
            .unwrap(),
        Middle::MissingBlock => {
            parent_of_anchor = Some(ObjectId::from_bytes([0x78; 32]));
            root
        }
    };
    // The anchor's snapshot: the empty state, which recomputes to the anchor's signed (empty) root.
    let manifest = SnapshotManifest {
        entries: Vec::new(),
    };
    let snapshot = store
        .write_object(&ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            prikk_object::BlobPayload::new(
                prikk_object::BlobKind::Snapshot,
                manifest.encode().unwrap(),
            )
            .to_canonical_bytes()
            .unwrap(),
        ))
        .unwrap();
    let mut anchor = block(
        BlockKind::Normal,
        vec![parent_of_anchor.unwrap_or(middle_id)],
        Vec::new(),
        Some(snapshot),
        2,
    );
    let anchor_id = anchor.object_id();
    anchor
        .add_signature(
            maintainer_signature(history.signer(), ObjectType::Block, anchor_id).unwrap(),
        )
        .unwrap();
    let anchor = store.write_object(&anchor).unwrap();
    record_verified_blocks(&history.layout, [anchor]);
    Damaged {
        layout_holder: history,
        store,
        anchor,
    }
}

impl Damaged {
    fn layout(&self) -> &RepositoryLayout {
        &self.layout_holder.layout
    }
}

/// **The (b) items** (RFC 159 §2 control 8, and §1 items 1-8 of the design round): everything the id-only walk still checks
/// below the anchor. The positive twin (`Middle::Ok`) passes all four conditions **and** the walk, so it is anchored and
/// the full walk agrees; each damaged variant leaves the anchor admitted and breaks one thing below it, and the anchored
/// derivation must refuse **while the full walk refuses too** (which is what makes `seal` refuse).
/// **Perturb**, one at a time, in `read_checked_block` / `id_only_tombstones`: skip the schema check, skip
/// `validate_block_v2_shape`, skip the object-type check, read patches with `require_schema_one = false`, ignore a patch
/// decode error, ignore a missing patch. Each turns the variant it names red, and only it.
#[test]
fn every_check_the_id_only_walk_keeps_still_refuses_a_damaged_object_below_the_anchor() {
    let twin = damaged_chain("rfc159-damaged-ok", &Middle::Ok);
    assert!(
        anchored_parent_state(twin.layout(), &twin.store, twin.anchor, AnchorSite::Seal).is_some(),
        "the positive twin passes every condition and the walk"
    );
    assert!(
        resolved_parent_state(&twin.store, Some(twin.anchor), &mut LineageStateMemo::new()).is_ok(),
        "and the full walk accepts it"
    );
    let mut failures = Vec::new();
    for (name, middle) in [
        ("a missing patch", Middle::MissingPatch),
        ("an object that is not a patch", Middle::NotAPatch),
        (
            "a patch of an unadmitted schema",
            Middle::UnadmittedPatchSchema,
        ),
        ("a patch that does not decode", Middle::UndecodablePatch),
        ("a block of the wrong schema", Middle::WrongBlockSchema),
        ("a block of the wrong shape", Middle::WrongShape),
        ("a block that does not decode", Middle::UndecodableBlock),
        ("a parent that is not a block", Middle::NotABlock),
        ("a parent that is missing", Middle::MissingBlock),
    ] {
        let chain = damaged_chain("rfc159-damaged", &middle);
        if anchored_parent_state(chain.layout(), &chain.store, chain.anchor, AnchorSite::Seal)
            .is_some()
        {
            failures.push(format!("{name}: the anchored derivation accepted it"));
        }
        if resolved_parent_state(
            &chain.store,
            Some(chain.anchor),
            &mut LineageStateMemo::new(),
        )
        .is_ok()
        {
            failures.push(format!(
                "{name}: the full walk accepted it, so this variant checks nothing"
            ));
        }
        if !take_anchor_fallbacks().is_empty() {
            failures.push(format!(
                "{name}: damage below an admitted anchor is the full walk's to report, not a finding about the anchor"
            ));
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// **C1, pinned** (RFC 159 ruling 1): damaged *content* below the anchor that **no live node names** does not stop a
/// `seal`, and `verify` reports it. A file created and deleted before the anchor leaves a blob nothing live refers to; one
/// byte of it is flipped in the container. The anchored derivation still succeeds; the full walk (which reads every
/// blob it creates) refuses; `verify` reports the damage. This pins the accepted behavior deliberately: a state root
/// commits to blob *ids*, never to their content.
/// **Perturb:** make the id-only walk read each create's blob (`reader.read_object(blob_id)`): the anchored derivation
/// refuses the damaged history and the first assertion goes red.
#[test]
fn damaged_content_below_the_anchor_that_no_live_node_names_does_not_stop_a_seal_and_verify_reports_it()
 {
    let mut history = AnchoredHistory::new("rfc159-c1");
    let marker = b"C1-UNIQUE-CONTENT-OF-A-FILE-DELETED-LONG-AGO-0123456789\n";
    let create = history.create("gone.txt", 0x91, marker);
    let keep = history.create("keep.txt", 0x92, b"keep\n");
    history.seal(vec![create, keep]);
    let delete = history.delete(0x91);
    history.seal(vec![delete]);
    for index in 0..70_u8 {
        let op = history.create(
            &format!("f{index}.txt"),
            0xB0_u8.wrapping_add(index),
            b"filler\n",
        );
        history.seal(vec![op]);
    }
    assert!(
        read_checked_block(&history.store, history.blocks[64])
            .expect("block 65")
            .snapshot_blob_ref
            .is_some(),
        "fixture sanity: block 65 is a checkpoint, so the anchor is above the deleted file's blob"
    );
    // Flip one byte of the deleted file's blob in the container.
    let container = history.layout.container_slot_path(
        ObjectType::Blob,
        crate::foundation::layout::ContainerSlot::A,
    );
    let mut bytes = std::fs::read(&container).unwrap();
    let at = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("the marker is in the blob container");
    bytes[at + 5] ^= 0x01;
    std::fs::write(&container, &bytes).unwrap();

    let index = history.blocks.len() - 1;
    let anchored = derive(&history, index, SEAL);
    assert!(
        anchored.anchored,
        "the anchored derivation succeeds over the damaged content"
    );
    assert!(
        derive_next_state_for_candidate_choosing(
            &history.layout,
            &history.store,
            parent_of(&history, index),
            &patches_of(&history, index),
            StateAnchoring::Never,
        )
        .is_err(),
        "today's full walk reads the blob a create names and refuses"
    );
    let verification = crate::verify_repository(&history.layout).unwrap();
    assert!(
        verification.has_item_failure() || verification.has_stage_failure(),
        "verify reports the damage: {verification:?}"
    );
}

/// **The snapshot writer's presence check, pinned** (RFC 159 §8.6, with C1). The checkpoint writer used to *read* each
/// live file's blob to learn whether it was stored, which also caught a stored blob whose bytes were damaged; it now asks
/// the object index. A live file created below the anchor, its blob damaged in the container: the checkpoint at block 65
/// is written (it names the blob by id, as a state root always has, and the manifest validator checks presence), and
/// `verify` reports the damage. Before this round that `seal` refused.
/// **Perturb:** `read_typed(..).is_some()` in `write_checkpoint_snapshot`: the seal of block 65 fails and this goes red.
#[test]
fn a_checkpoint_is_written_over_a_damaged_live_blob_below_the_anchor_and_verify_reports_it() {
    let mut history = AnchoredHistory::new("rfc159-writer-presence");
    let marker = b"WRITER-PRESENCE-UNIQUE-CONTENT-OF-A-LIVE-FILE-0123456789\n";
    let create = history.create("live.txt", 0x93, marker);
    history.seal(vec![create]);
    for index in 0..63_u8 {
        let op = history.create(
            &format!("g{index}.txt"),
            0xB0_u8.wrapping_add(index),
            b"filler\n",
        );
        history.seal(vec![op]);
    }
    assert_eq!(history.blocks.len(), 64);
    let container = history.layout.container_slot_path(
        ObjectType::Blob,
        crate::foundation::layout::ContainerSlot::A,
    );
    let mut bytes = std::fs::read(&container).unwrap();
    let at = bytes
        .windows(marker.len())
        .position(|window| window == marker)
        .expect("the marker is in the blob container");
    bytes[at + 5] ^= 0x01;
    std::fs::write(&container, &bytes).unwrap();

    let op = history.create("h.txt", 0x05, b"h\n");
    let checkpoint = history.seal(vec![op]);
    assert!(
        read_checked_block(&history.store, checkpoint)
            .expect("block 65")
            .snapshot_blob_ref
            .is_some(),
        "the checkpoint was written over the damaged blob"
    );
    let verification = crate::verify_repository(&history.layout).unwrap();
    assert!(
        verification.has_item_failure() || verification.has_stage_failure(),
        "verify reports the damage: {verification:?}"
    );
}

/// Whether a source file, given by its path **relative to `src`**, is test code: some component is `tests` or
/// `test_gates`, or its file name ends in `tests.rs`. **By components, never by the path's spelling**: a rendered path
/// spells its separator `\` on Windows, so a `contains("/tests/")` test skipped nothing there and this scan flagged a test
/// file as production (CI run 36202895907, Windows mutation suite, RFC 159 Addendum 2). `Path` compares and splits by
/// component on every platform.
fn is_test_source(relative: &std::path::Path) -> bool {
    relative
        .components()
        .any(|part| part.as_os_str() == "tests" || part.as_os_str() == "test_gates")
        || relative
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with("tests.rs"))
}

/// **The classifier, on paths built from components** -- what a Windows directory walk yields (`a\b\c.rs`) is a path
/// whose components are `a`, `b`, `c.rs`, exactly as `PathBuf::from_iter` builds it here. Linux and macOS cannot render a
/// backslash path, so this control proves the component reading, and the Windows mutation suite proves the platform.
/// **Perturb:** classify by `relative.to_string_lossy().contains("/tests/")`: this stays green on Linux (its separator
/// is `/`), which is the defect's whole shape; the scan below is what the Windows job runs.
#[test]
fn test_sources_are_classified_by_component_not_by_spelling() {
    use std::path::PathBuf;
    let path = |parts: &[&str]| parts.iter().collect::<PathBuf>();
    assert!(is_test_source(&path(&["snapshot", "tests", "writer.rs"])));
    assert!(is_test_source(&path(&[
        "block_state",
        "tests",
        "deep",
        "x.rs"
    ])));
    assert!(is_test_source(&path(&["merge", "execute", "tests.rs"])));
    assert!(is_test_source(&path(&["seal_from_accepted", "tests.rs"])));
    assert!(is_test_source(&path(&["test_gates", "test_support.rs"])));
    assert!(!is_test_source(&path(&["block_state.rs"])));
    assert!(!is_test_source(&path(&[
        "block_state",
        "anchored_parent.rs"
    ])));
    assert!(!is_test_source(&path(&["merge", "execute.rs"])));
    // A component that merely contains the word is not the directory.
    assert!(!is_test_source(&path(&["contests", "x.rs"])));
    assert!(!is_test_source(&path(&["tests_helper.rs"])));
}

/// **The anchored derivation names no function of the environment.** No knob decides how a signed root is derived: this
/// source never reads a process environment variable, and only a control (`cfg(test)` / `test-support`) names
/// `StateAnchoring::Never` (the RFC 159 handoff §1.7, in the pattern of `every_read_only_anchor_caller_is_a_read_only_report`).
/// Files are classified by their path **relative to `src`, by component** ([`is_test_source`]).
/// **Perturb (each shown red on Linux):** name a `PRIKK_RFC159*` variable in `anchored_parent.rs`, or put
/// `StateAnchoring::Never` into `merge/execute.rs`.
#[test]
fn no_environment_knob_and_no_production_never() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    fn walk(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    walk(&src, &mut files);
    let mut scanned = 0_usize;
    for path in files {
        let relative = path.strip_prefix(&src).unwrap_or(&path).to_path_buf();
        // Test modules and controls may name both.
        if is_test_source(&relative) {
            continue;
        }
        scanned += 1;
        let name = relative.display().to_string();
        let text = std::fs::read_to_string(&path).unwrap();
        if text.contains("PRIKK_RFC159") {
            offenders.push(format!("{name}: names a PRIKK_RFC159 variable"));
        }
        // `StateAnchoring::Never` may appear only where it is declared or handled behind `cfg(any(test, ..))`.
        let declares_it = relative == std::path::Path::new("block_state.rs")
            || relative == std::path::Path::new("block_state").join("anchored_parent.rs");
        for (line_no, line) in text.lines().enumerate() {
            if line.contains("StateAnchoring::Never") && !declares_it {
                offenders.push(format!(
                    "{name}:{}: names StateAnchoring::Never",
                    line_no + 1
                ));
            }
        }
    }
    assert!(
        scanned > 100,
        "fixture sanity: the scan read the production sources ({scanned} files)"
    );
    assert!(offenders.is_empty(), "{offenders:#?}");
}

/// **`prikk seal`'s site** (RFC 159 handoff §2 control 10). Sealing a block on a checkpoint through the public
/// `seal_block` anchors at it, and counts as the `seal` site, and no other. **Perturb:** make `seal_block` pass
/// `StateAnchoring::Never`: this goes red, and the merge and seal-from-accepted controls (which state their own site) do not.
#[test]
fn seal_block_anchors_at_the_seal_site() {
    reset_anchor_uses_for_test_support();
    let mut history = AnchoredHistory::standard("rfc159-site-seal", 70);
    let before = anchor_uses_for_test_support("seal");
    let op = history.create("later.txt", 0xE0, b"later\n");
    history.seal(vec![op]);
    assert_eq!(
        anchor_uses_for_test_support("seal") - before,
        1,
        "one seal, one anchored derivation, at the seal site"
    );
    for other in [
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
}
