//! Unit tests for `extract`'s own streamed replay (module doc). CLI-level, end-to-end coverage
//! (byte-identity against `cat`/`tree`, the collision/no-replace-rename/pre-walk controls, and the
//! `.prikk/`-untouched property) lives in `crates/prikk-cli/tests/extract.rs`, which can drive the
//! compiled binary and a real filesystem; this module pins the replay-internal shapes (streamed vs.
//! resident, rename-moves-nothing-but-the-path, the trust rule) directly against
//! [`ExtractReplayOutcome`], where a fixture's own signing/state-root crafting is easiest to reach.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_object::{
    BlockKind, CanonicalEncode, ChangePerm, CreateFile, DeleteNode, EditText, NodeKind,
    ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
    ReplaceBinary,
};

use super::*;
use crate::state_root::StateRootEntry;
use crate::test_gates::test_support::{
    AnchoredHistory, Signing, anchored_node, dummy_signature, publish_binary_create_then_replace,
    signed_block_with_state_root, unique_temp_dir, write_binary_blob, write_blob, write_snapshot,
};
use crate::{FileObjectStore, ObjectReadSnapshot, ObjectWriter, RepositoryLayout};

fn content_by_path(outcome: &ExtractReplayOutcome) -> BTreeMap<String, ExtractContent> {
    outcome
        .files
        .iter()
        .map(|file| (file.path.clone(), file.content.clone()))
        .collect()
}

/// An admitted anchor (genesis is always a checkpoint; `AnchoredHistory` seals it through the real
/// `seal_block` path, so it genuinely enters the replay-verified record) seeds every file straight
/// from a Blob id; nothing is held resident before any patch past it is read.
#[test]
fn files_at_an_admitted_anchor_are_not_resident() {
    let mut history = AnchoredHistory::standard("extract-anchor-seed", 9);
    let tip = history.blocks[0]; // genesis alone: distance 0, trivially admitted
    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);
    let outcome = replay_for_extract(&reader, &block_ids, &trust, false).unwrap();
    assert!(!outcome.provisional, "genesis's own checkpoint must admit");
    let by_path = content_by_path(&outcome);
    for path in ["a.txt", "b.txt", "c.txt"] {
        assert!(
            matches!(by_path.get(path), Some(ExtractContent::Blob(_))),
            "{path} must stream from its anchor blob id, {by_path:?}"
        );
    }
    let _ = &mut history; // keep the fixture's temp directory alive for the whole test
}

/// Past the anchor: a file edited after it is held resident; a file only created (never edited)
/// after it is resident too (its bytes were already read to classify create -- discarding them would
/// cost a second read for nothing, module doc); a file deleted after it is simply absent; a file
/// never touched since the anchor still streams from its anchor id, at its node id, not its path.
#[test]
fn edits_deletes_and_untouched_files_after_the_anchor() {
    let mut history = AnchoredHistory::standard("extract-anchor-forward", 9);
    let tip = *history.blocks.last().unwrap();
    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);
    let outcome = replay_for_extract(&reader, &block_ids, &trust, false).unwrap();
    assert!(!outcome.provisional);
    let by_path = content_by_path(&outcome);
    // a.txt and b.txt were edited after genesis: resident.
    assert!(matches!(
        by_path.get("a.txt"),
        Some(ExtractContent::Resident(_))
    ));
    assert!(matches!(
        by_path.get("b.txt"),
        Some(ExtractContent::Resident(_))
    ));
    // c.txt was deleted and never recreated: absent.
    assert!(!by_path.contains_key("c.txt"));
    // d.txt was created, deleted, then recreated after genesis: resident (a fresh node id each
    // time -- its content was just read to learn its kind, so it stays resident rather than a
    // second read at write time).
    assert!(matches!(
        by_path.get("d.txt"),
        Some(ExtractContent::Resident(_))
    ));
    let _ = &mut history;
}

/// `ReplaceBinary` after the anchor reads the new blob exactly once, to confirm it is actually
/// binary (the same check `apply.rs`'s own `apply_replace_binary` makes), then discards the bytes:
/// the node's content becomes `Blob(new_blob_id)`, streamed fresh only when `extract` actually
/// writes it, never held resident past this one validating read.
#[test]
fn replace_binary_after_the_anchor_validates_then_never_holds_the_new_blob_resident() {
    let root = unique_temp_dir("extract-replace-binary");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let new_blob_id =
        publish_binary_create_then_replace(&layout, b"old bytes", b"new bytes").unwrap();
    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let tip = crate::refs::read_current_ref_tip_block(&layout, &reader, "heads/main").unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&layout);
    let outcome = replay_for_extract(&reader, &block_ids, &trust, false).unwrap();
    assert!(
        outcome.provisional,
        "no admitted anchor in this raw fixture"
    );
    let by_path = content_by_path(&outcome);
    assert_eq!(
        by_path.get("asset.bin"),
        Some(&ExtractContent::Blob(new_blob_id))
    );
    let _ = std::fs::remove_dir_all(root);
}

/// The trust rule, the hardest case: a Block whose own signature is from a key this repository
/// never adopted (a bare `RepositoryLayout::init` adopts nothing), and whose snapshot's manifest
/// names content that disagrees with what the block's own patch actually produces -- self-consistent
/// with a *forged* state root, which `Anchoring::ReadOnlyReport`'s loader alone cannot distinguish
/// from a real one (it only checks the manifest recomputes to the block's own signed root, never
/// that the root matches a replay of the block's patches). `extract`'s own admitted-anchor rule
/// requires the block to be in the **replay-verified record** too, which this one never entered (no
/// `verify`/`seal` ever ran it), so the forged anchor is never used, and the real patch replay --
/// genesis only, one `CreateFile` -- gives the true content.
#[test]
fn trust_rule_prefers_true_patch_content_over_a_forged_but_self_consistent_snapshot() {
    let root = unique_temp_dir("extract-forged-snapshot");
    let layout = RepositoryLayout::init(root.clone()).unwrap(); // adopts no maintainer key at all
    let mut store = FileObjectStore::new(layout.clone());
    let node_id = anchored_node(0x99);
    let true_blob = write_blob(&mut store, b"true content from the patch").unwrap();
    let forged_blob = write_blob(&mut store, b"FORGED content no patch ever wrote").unwrap();

    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "secret.txt".to_string(),
                node_id,
                blob_id: true_blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        patch_payload.to_canonical_bytes().unwrap(),
    );
    patch.add_signature(dummy_signature()).unwrap();
    let patch_id = store.write_object(&patch).unwrap();

    // A manifest that is internally self-consistent with a *forged* root -- never the root this
    // patch actually produces (`derive_next_state_root` is deliberately never called here).
    let forged_entries = vec![StateRootEntry {
        path: crate::RepoPath::parse("secret.txt").unwrap(),
        node_id,
        kind: NodeKind::TextFile,
        mode: 0o100644,
        content: crate::state_root::StateRootContent::Blob(forged_blob),
    }];
    let forged_root = crate::state_root::compute_state_root(&forged_entries).unwrap();
    let true_root = crate::derive_next_state_root(&store, None, &[patch_id]).unwrap();
    assert_ne!(
        forged_root, true_root,
        "fixture sanity: the forged root must actually disagree"
    );
    let snapshot_blob_id = write_snapshot(&mut store, forged_entries, forged_root).unwrap();

    let block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![patch_id],
        Some(snapshot_blob_id),
        forged_root,
    );
    let block_id = store.write_object(&block).unwrap();

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let block_ids = vec![block_id];
    let trust = crate::anchor_trust::AnchorTrust::load(&layout);

    // extract's own rule: never admitted (unadopted key, and never replay-verified), so it falls
    // back to genesis and reads the true patch content.
    let outcome = replay_for_extract(&reader, &block_ids, &trust, false).unwrap();
    assert!(
        outcome.provisional,
        "the forged block must never be admitted"
    );
    let by_path = content_by_path(&outcome);
    match by_path.get("secret.txt") {
        Some(ExtractContent::Resident(bytes)) => {
            assert_eq!(bytes.as_slice(), b"true content from the patch");
        }
        other => panic!("expected resident true content, got {other:?}"),
    }

    // Contrast, not asserted as this module's own behavior but to show precisely what would go
    // wrong without the trust rule: the weaker `Anchoring::ReadOnlyReport` path (`cat`/`tree`'s own
    // anchoring) *does* accept this same forged snapshot, because its loader never checks the
    // replay-verified record or the signing key -- only that the manifest recomputes to the block's
    // own (here, forged) root, which it does by construction.
    let read_only = super::super::anchor::replay_chain(
        &reader,
        &block_ids,
        super::super::anchor::Anchoring::ReadOnlyReport,
    )
    .unwrap();
    let forged_bytes = read_only
        .files
        .get("secret.txt")
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        forged_bytes, b"FORGED content no patch ever wrote",
        "sanity: ReadOnlyReport's own weaker anchoring is what the trust rule exists to avoid \
         reaching for extract's own output"
    );

    // Cross-check, independent of this module's own `search_admitted_anchor`: the *shared*
    // `find_anchor` mechanism, under `AnchorSite::Extract` via `Anchoring::VerifiedExtract`, makes
    // the identical admission decision (never admitted) and so also falls back to the true patch
    // content -- confirming this module's own parallel search agrees with the mechanism every
    // other verified anchor site shares, not a second, differently-wrong rule.
    let via_shared_mechanism = super::super::anchor::replay_chain(
        &reader,
        &block_ids,
        super::super::anchor::Anchoring::VerifiedExtract(&trust),
    )
    .unwrap();
    assert_eq!(
        via_shared_mechanism.files.get("secret.txt"),
        Some(&b"true content from the patch".to_vec()),
        "the shared Anchoring::VerifiedExtract mechanism must refuse the forged anchor too"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// The pre-walk refuses, by name, before reading a single patch, once no admitted anchor exists and
/// the point's own history is longer than this repository ever expects a healthy one to need -- here,
/// every checkpoint in a 70-block lineage fails trust at once (the adopted key changed after sealing),
/// so none is ever admitted. `--allow-full-replay` reads it anyway.
#[test]
fn pre_walk_refuses_past_the_bound_without_an_admitted_anchor() {
    let history = AnchoredHistory::standard_signed("extract-prewalk-bound", 70, Signing::Unadopted);
    let tip = *history.blocks.last().unwrap();
    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);

    let refused = replay_for_extract(&reader, &block_ids, &trust, false);
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("no admitted snapshot within"), "{message}");
    assert!(message.contains("--allow-full-replay"), "{message}");

    let allowed = replay_for_extract(&reader, &block_ids, &trust, true).unwrap();
    assert!(allowed.provisional);
    assert!(!allowed.files.is_empty());
}

/// The claim path (`--claim`) takes the identical pre-walk bound as the ref/block-id path (0.54.0
/// step 3 Part 3b, R1): a claim whose own declared parent reaches back 70 blocks, every one signed
/// but never admitted (`Signing::Unadopted`), refuses by name without `--allow-full-replay`, and
/// succeeds (fully provisional) with it set -- the identical shape
/// `pre_walk_refuses_past_the_bound_without_an_admitted_anchor` already pins for the ref/block-id
/// path, reused here for the claim path instead.
#[test]
fn claim_pre_walk_refuses_past_the_bound_without_an_admitted_anchor() {
    let mut history =
        AnchoredHistory::standard_signed("extract-claim-prewalk-bound", 70, Signing::Unadopted);
    let parent = *history.blocks.last().unwrap();

    let blob = write_blob(&mut history.store, b"claimed content").unwrap();
    let patch_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "claimed.txt".to_string(),
                node_id: anchored_node(0xEE),
                blob_id: blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        patch_payload.to_canonical_bytes().unwrap(),
    );
    patch.add_signature(dummy_signature()).unwrap();
    let patch_id = history.store.write_object(&patch).unwrap();

    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();

    let refused = replay_for_extract_chain(
        &history.layout,
        &reader,
        Some(parent),
        0,
        &[vec![patch_id]],
        false,
    );
    let message = refused.unwrap_err().to_string();
    assert!(message.contains("no admitted snapshot within"), "{message}");
    assert!(message.contains("--allow-full-replay"), "{message}");

    let allowed = replay_for_extract_chain(
        &history.layout,
        &reader,
        Some(parent),
        0,
        &[vec![patch_id]],
        true,
    )
    .unwrap();
    assert!(allowed.provisional);
    assert!(!allowed.files.is_empty());
}

/// 0.54.0 step 3 Part 5d: `no-checkpoint-in-reach`'s own `reason` sub-classification is genuinely
/// new behavior, not a relabeling -- proven here by producing all three reasons, each from what
/// [`super::search_admitted_anchor`]'s own walk actually found, not merely that *a* refusal
/// happened:
/// - `signer-not-adopted`: the identical 70-block, `Signing::Unadopted` fixture
///   [`pre_walk_refuses_past_the_bound_without_an_admitted_anchor`] already builds -- every
///   checkpoint stays in the replay-verified record (sealing itself recorded it), but the key that
///   signed it is no longer adopted;
/// - `not-recorded`: the identical history, still signed by a key this repository adopts, but its
///   own replay-verified record is wiped after sealing -- every checkpoint the walk finds was
///   simply never recorded;
/// - `no-checkpoint`: a hand-built, 70-block chain of plain blocks that never carries a snapshot at
///   all -- `AnchoredHistory`'s own seal path always makes genesis a checkpoint (this module's own
///   doc, "genesis is always a checkpoint"), so this one genuinely different shape needs its own,
///   lower-level fixture rather than a variant of the same one.
#[test]
fn no_checkpoint_in_reach_reports_the_right_reason_for_each_of_its_three_causes() {
    use crate::extract_refusal::{ExtractRefusalKind, NoCheckpointReason};
    use crate::verified_blocks::{load_verified_blocks, record_path};

    {
        let history = AnchoredHistory::standard_signed(
            "extract-reason-signer-not-adopted",
            70,
            Signing::Unadopted,
        );
        let tip = *history.blocks.last().unwrap();
        let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
        let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
        let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);
        let refused = replay_for_extract(&reader, &block_ids, &trust, false).unwrap_err();
        assert_eq!(
            refused.kind,
            Some(ExtractRefusalKind::NoCheckpointInReach(
                NoCheckpointReason::SignerNotAdopted
            )),
            "{refused}"
        );
    }

    {
        let history =
            AnchoredHistory::standard_signed("extract-reason-not-recorded", 70, Signing::Adopted);
        std::fs::remove_file(record_path(&history.layout)).unwrap();
        assert!(load_verified_blocks(&history.layout).is_empty());
        let tip = *history.blocks.last().unwrap();
        let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
        let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
        let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);
        let refused = replay_for_extract(&reader, &block_ids, &trust, false).unwrap_err();
        assert_eq!(
            refused.kind,
            Some(ExtractRefusalKind::NoCheckpointInReach(
                NoCheckpointReason::NotRecorded
            )),
            "{refused}"
        );
    }

    {
        let root = unique_temp_dir("extract-reason-no-checkpoint");
        let layout = RepositoryLayout::init(root).unwrap();
        let mut store = FileObjectStore::new(layout.clone());
        let mut parent_ids = Vec::new();
        let mut tip = None;
        for index in 0..70 {
            let kind = if index == 0 {
                BlockKind::Root
            } else {
                BlockKind::Normal
            };
            let block = crate::test_gates::test_support::signed_block(
                kind,
                parent_ids.clone(),
                Vec::new(),
                None,
            );
            let id = store.write_object(&block).unwrap();
            parent_ids = vec![id];
            tip = Some(id);
        }
        let tip = tip.unwrap();
        let reader = ObjectReadSnapshot::open(&layout).unwrap();
        let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
        assert_eq!(
            block_ids.len(),
            70,
            "fixture sanity: a plain 70-block chain"
        );
        let trust = crate::anchor_trust::AnchorTrust::load(&layout);
        let refused = replay_for_extract(&reader, &block_ids, &trust, false).unwrap_err();
        assert_eq!(
            refused.kind,
            Some(ExtractRefusalKind::NoCheckpointInReach(
                NoCheckpointReason::NoCheckpoint
            )),
            "{refused}"
        );

        let allowed = replay_for_extract(&reader, &block_ids, &trust, true).unwrap();
        assert!(allowed.provisional);
        assert!(allowed.files.is_empty(), "no patch ever created a file");
        let _ = std::fs::remove_dir_all(layout.root());
    }
}

/// `ReplaceBinary` must not accept a `new_blob_id` whose actual, stored kind is not binary -- a
/// patch's own claim is not proof (the same reasoning the trust-rule test above applies to a
/// forged snapshot, applied here to a forged operation instead). `apply.rs`'s shared replay
/// (`cat`/`tree`/`checkout`) already refuses this; `extract`'s own parallel replay must refuse it
/// identically, not silently stream the wrong content. **Red**: before this check existed,
/// `replay_for_extract` returned `Ok`, producing `asset.bin` from a blob that is actually text,
/// while the shared replay correctly refused -- a real output divergence from the `cat`/`tree`
/// oracle on malformed history. **Green**, pinned here.
#[test]
fn replace_binary_refuses_a_new_blob_whose_real_kind_is_not_binary() {
    let root = unique_temp_dir("extract-replace-binary-wrong-kind");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let mut store = FileObjectStore::new(layout.clone());
    let node_id = anchored_node(0x77);
    let old_blob = write_binary_blob(&mut store, b"old binary bytes").unwrap();
    let wrong_kind_blob: ObjectId = write_blob(&mut store, b"actually a TEXT blob").unwrap();

    let create_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "asset.bin".to_string(),
                node_id,
                blob_id: old_blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut create_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        create_payload.to_canonical_bytes().unwrap(),
    );
    create_patch.add_signature(dummy_signature()).unwrap();
    let create_patch_id = store.write_object(&create_patch).unwrap();
    let root_state = crate::derive_next_state_root(&store, None, &[create_patch_id]).unwrap();
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![create_patch_id],
        None,
        root_state,
    );
    let root_block_id = store.write_object(&root_block).unwrap();

    let replace_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::ReplaceBinary(ReplaceBinary {
                node_id,
                old_blob_id: old_blob,
                new_blob_id: wrong_kind_blob,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut replace_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        replace_payload.to_canonical_bytes().unwrap(),
    );
    replace_patch.add_signature(dummy_signature()).unwrap();
    let replace_patch_id = store.write_object(&replace_patch).unwrap();
    // Not re-derived via `derive_next_state_root`: that helper itself replays the patch and would
    // (rightly) refuse to build this malformed block in the first place. Reusing `root_state` is
    // fine since this block carries no snapshot (`None`), so nothing ever reads this field.
    let second_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![replace_patch_id],
        None,
        root_state,
    );
    let second_block_id = store.write_object(&second_block).unwrap();

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, second_block_id).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&layout);
    let err = replay_for_extract(&reader, &block_ids, &trust, false).unwrap_err();
    assert!(
        err.to_string().contains("is not a binary-file blob"),
        "{err}"
    );
}

// ---- 0.54.0 step 3 Part 2b R3: a standing differential test against the shared replay ----------
//
// `apply_one_for_extract` is a parallel reimplementation of `apply.rs`'s own per-operation
// semantics. A one-time review cannot prove there is no divergence beyond the `ReplaceBinary` one
// already found and fixed (above); this section is the permanent, standing check: over a range of
// histories, `extract`'s own materialized state must equal the shared replay's
// (`Anchoring::ReadOnlyReport`, the identical path `cat`/`tree` use), and every malformed-operation
// shape the shared replay refuses must be refused by `extract` too.

use prikk_object::{RefKind, RenamePath};

use crate::{MaintainerSigner, maintainer_signature};

/// The shared replay's own materialized state for `block_ids` -- the same `Anchoring::
/// ReadOnlyReport` path `cat`/`tree` use.
fn shared_replay_files(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
) -> BTreeMap<String, Vec<u8>> {
    super::super::anchor::replay_chain(
        reader,
        block_ids,
        super::super::anchor::Anchoring::ReadOnlyReport,
    )
    .unwrap()
    .files
}

/// `extract`'s own outcome, with every file's content actually read (`Blob` streamed fresh,
/// `Resident` already in hand) -- what the CLI's own `write_plan` would have written to disk.
fn materialize_extract_files(
    reader: &impl ObjectReader,
    outcome: &ExtractReplayOutcome,
) -> BTreeMap<String, Vec<u8>> {
    outcome
        .files
        .iter()
        .map(|file| {
            let bytes = match &file.content {
                ExtractContent::Blob(id) => read_blob_bytes_with_kind(reader, *id).unwrap().1,
                ExtractContent::Resident(bytes) => bytes.clone(),
            };
            (file.path.clone(), bytes)
        })
        .collect()
}

/// `extract`'s own outcome's mode per path, for the same-state comparison (content alone cannot
/// show a `ChangePerm` divergence).
fn extract_modes(outcome: &ExtractReplayOutcome) -> BTreeMap<String, u32> {
    outcome
        .files
        .iter()
        .map(|file| (file.path.clone(), file.mode))
        .collect()
}

/// Assert `extract`'s own replay of `block_ids` matches the shared replay's, path for path, byte
/// for byte, mode for mode -- the standing same-state check (R3). `context` names the fixture in
/// any failure message.
fn assert_extract_matches_shared_replay(
    reader: &impl ObjectReader,
    block_ids: &[ObjectId],
    trust: &crate::anchor_trust::AnchorTrust,
    context: &str,
) {
    let shared = shared_replay_files(reader, block_ids);
    let outcome = replay_for_extract(reader, block_ids, trust, true)
        .unwrap_or_else(|err| panic!("{context}: extract's own replay failed: {err}"));
    let extracted = materialize_extract_files(reader, &outcome);
    assert_eq!(
        extracted, shared,
        "{context}: extract's own output must equal the shared replay's, path for path and byte \
         for byte"
    );

    // Mode agreement: the shared replay's own `live_nodes` carries it too, keyed by node id rather
    // than path, so compare by resolving each shared path's own live node.
    let shared_chain = super::super::anchor::replay_chain(
        reader,
        block_ids,
        super::super::anchor::Anchoring::ReadOnlyReport,
    )
    .unwrap();
    let shared_modes: BTreeMap<String, u32> = shared_chain
        .live_nodes
        .values()
        .map(|node| (node.path.clone(), node.mode))
        .collect();
    assert_eq!(
        extract_modes(&outcome),
        shared_modes,
        "{context}: extract's own modes must equal the shared replay's"
    );
}

/// Same state, anchored through a real admitted checkpoint: `AnchoredHistory::standard`'s own
/// history (creates, edits, a delete, a restoration -- module doc, `anchored_history.rs`), at its
/// tip, genesis sealed as a real checkpoint.
#[test]
fn same_state_as_shared_replay_anchored_history() {
    let mut history = AnchoredHistory::standard("extract-differential-anchored", 9);
    let tip = *history.blocks.last().unwrap();
    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);
    assert_extract_matches_shared_replay(
        &reader,
        &block_ids,
        &trust,
        "AnchoredHistory::standard at its tip",
    );
    let _ = &mut history;
}

/// Same state, from genesis (no admitted anchor at all): a text file, a binary file, an edit, a
/// `ReplaceBinary`, and a delete, all in one small from-scratch history -- the shape
/// `publish_binary_create_then_replace` already builds, replayed through both engines.
#[test]
fn same_state_as_shared_replay_text_and_binary_from_genesis() {
    let root = unique_temp_dir("extract-differential-genesis");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let new_blob_id =
        publish_binary_create_then_replace(&layout, b"old bytes", b"new bytes").unwrap();
    let _ = new_blob_id;
    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let tip = crate::refs::read_current_ref_tip_block(&layout, &reader, "heads/main").unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, tip).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&layout);
    assert_extract_matches_shared_replay(
        &reader,
        &block_ids,
        &trust,
        "publish_binary_create_then_replace from genesis",
    );
    let _ = std::fs::remove_dir_all(root);
}

/// A true swap: two files exchange names in one patch, batched (`apply_rename_batch`'s own
/// vacate-then-claim two-phase algorithm, mirrored by `apply_rename_batch_for_extract`) -- neither
/// rename is valid read in isolation, since each one's own destination is the other's current,
/// not-yet-vacated path.
#[test]
fn same_state_on_a_two_file_swap_in_one_patch() {
    let root = unique_temp_dir("extract-differential-swap");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let mut store = FileObjectStore::new(layout.clone());
    let node_a = anchored_node(0x0A);
    let node_b = anchored_node(0x0B);
    let blob_a = write_blob(&mut store, b"content of a").unwrap();
    let blob_b = write_blob(&mut store, b"content of b").unwrap();

    let genesis_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "a.txt".to_string(),
                    node_id: node_a,
                    blob_id: blob_a,
                    mode: 0o100644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "b.txt".to_string(),
                    node_id: node_b,
                    blob_id: blob_b,
                    mode: 0o100644,
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut genesis_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        genesis_payload.to_canonical_bytes().unwrap(),
    );
    genesis_patch.add_signature(dummy_signature()).unwrap();
    let genesis_patch_id = store.write_object(&genesis_patch).unwrap();
    let genesis_root = crate::derive_next_state_root(&store, None, &[genesis_patch_id]).unwrap();
    let genesis_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![genesis_patch_id],
        None,
        genesis_root,
    );
    let genesis_block_id = store.write_object(&genesis_block).unwrap();

    let swap_payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id: node_a,
                    old_path: "a.txt".to_string(),
                    new_path: "b.txt".to_string(),
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::RenamePath(RenamePath {
                    node_id: node_b,
                    old_path: "b.txt".to_string(),
                    new_path: "a.txt".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut swap_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        swap_payload.to_canonical_bytes().unwrap(),
    );
    swap_patch.add_signature(dummy_signature()).unwrap();
    let swap_patch_id = store.write_object(&swap_patch).unwrap();
    let swap_root =
        crate::derive_next_state_root(&store, Some(genesis_block_id), &[swap_patch_id]).unwrap();
    let swap_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![genesis_block_id],
        vec![swap_patch_id],
        None,
        swap_root,
    );
    let swap_block_id = store.write_object(&swap_block).unwrap();

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, swap_block_id).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&layout);

    // Fixture sanity: the swap actually happened (the shared replay's own oracle, not extract's).
    let shared = shared_replay_files(&reader, &block_ids);
    assert_eq!(shared.get("a.txt"), Some(&b"content of b".to_vec()));
    assert_eq!(shared.get("b.txt"), Some(&b"content of a".to_vec()));

    assert_extract_matches_shared_replay(&reader, &block_ids, &trust, "a two-file swap");
    let _ = std::fs::remove_dir_all(root);
}

/// Same state across a real merge: `AnchoredHistory`'s own trusted maintainer seals a side branch
/// and `execute_merge` folds it into `heads/main` -- `prepare_extract_plan`'s own block chain comes
/// from the identical `single_parent_chain` `cat`/`tree` use (`read.rs`'s own `mainline_or_sole_
/// parent`, mainline only for a `Merge` block), so this confirms that shared chain-selection
/// combined with `apply_one_for_extract`'s own duplicated per-operation logic still agrees with the
/// shared replay on a real merge tip -- not a claim that `extract` does anything special for merges
/// itself (it does not; there is no merge-specific code in `patch_replay::extract` at all).
#[test]
fn same_state_across_a_merge() {
    let mut history = AnchoredHistory::new("extract-differential-merge");
    let genesis_op = history.create("a.txt", 0x0A, b"alpha\n");
    history.seal(vec![genesis_op]);
    let baseline = *history.blocks.last().unwrap();

    // An independent maintainer key, trusted alongside `AnchoredHistory`'s own one (the policy
    // supports more than one, the same shape `retro_sign(Signing::Unadopted)` already relies on)
    // -- avoids borrowing `history`'s own private signer while also needing `&mut history.store`
    // below, and keeps this fixture's own signing self-contained.
    let signer =
        crate::Ed25519MaintainerSigner::from_seed("extract-differential-merge-signer", &[0x5D; 32])
            .unwrap();
    crate::add_trusted_maintainer(
        &history.layout,
        signer.key_id(),
        &prikk_hash::to_hex(&signer.public_key_bytes()),
    )
    .unwrap();

    // Publish `heads/side` at the baseline, then queue and seal one commit on it -- the same shape
    // `every_publication_refuses_first.rs`'s own `build_branch_create_publication` uses, inlined
    // here rather than shared across modules (this project's own standing practice: each fixture
    // site keeps its own small construction).
    let side_ref = "heads/side";
    let state = prikk_object::RefStatePayload {
        ref_name: side_ref.to_string(),
        kind: RefKind::Branch,
        target_object_id: baseline,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(maintainer_signature(&signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    let update = prikk_object::RefUpdatePayload {
        ref_name: side_ref.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: baseline,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_env = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().unwrap(),
    );
    let update_id = update_env.object_id();
    update_env
        .add_signature(maintainer_signature(&signer, ObjectType::RefUpdate, update_id).unwrap())
        .unwrap();
    crate::RefStore::new(history.layout.clone())
        .publish(&crate::RefPublication {
            ref_name: side_ref.to_string(),
            expected_previous_ref_state_id: None,
            ref_state: state_env,
            ref_update: update_env,
        })
        .unwrap();

    // One commit on the side branch, authored (not maintainer-signed) the same way
    // `AnchoredHistory::append` signs its own patches.
    let author =
        crate::Ed25519AuthorSigner::from_seed("extract-differential-merge-author", &[0x5E; 32])
            .unwrap();
    let side_blob = write_blob(&mut history.store, b"side content\n").unwrap();
    let side_node = anchored_node(0x5D);
    let side_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "side.txt".to_string(),
                node_id: side_node,
                blob_id: side_blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut side_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
        side_payload.to_canonical_bytes().unwrap(),
    );
    let side_patch_id = side_patch.object_id();
    side_patch
        .add_signature(crate::author_signature(&author, side_patch_id).unwrap())
        .unwrap();
    crate::write_active_ref_metadata(&history.layout, side_ref).unwrap();
    crate::Wal::for_layout(&history.layout, crate::DEFAULT_ACTIVE_NAME)
        .append_patch(&side_patch)
        .unwrap();
    crate::rfc111_seal_simulation::simulate_one_seal(&history.layout, side_ref, &signer).unwrap();

    // Merge `heads/side` into `heads/main`.
    let report =
        crate::execute_merge(&history.layout, baseline, "heads/main", side_ref, &signer).unwrap();

    let reader = ObjectReadSnapshot::open(&history.layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, report.block_id).unwrap();
    let trust = crate::anchor_trust::AnchorTrust::load(&history.layout);

    // Fixture sanity: both files are really there after the merge, in the shared replay's own view.
    let shared = shared_replay_files(&reader, &block_ids);
    assert!(shared.contains_key("a.txt"));
    assert!(shared.contains_key("side.txt"));

    assert_extract_matches_shared_replay(&reader, &block_ids, &trust, "a real merge tip");
}

// ---- R3: the "same refusals" table -- one malformed row per operation kind --------------------

use prikk_object::DeleteNodePreimage;

/// A genesis-only chain carrying one well-formed `CreateFile` and one further, malformed patch on
/// top of it -- never through `derive_next_state_root` for the second block (which would itself
/// replay the malformed patch and refuse to build the fixture at all, as it should): the second
/// block's own signed root is a meaningless, never-admitted placeholder (it carries no snapshot,
/// so nothing ever reads or validates it).
fn write_patch(store: &mut FileObjectStore, ops: Vec<OperationKind>) -> ObjectId {
    let payload = PatchPayload {
        operations: ops
            .into_iter()
            .zip(1_u32..)
            .map(|(kind, op_seq)| Operation {
                op_seq,
                op_id: None,
                preconditions: Vec::new(),
                kind,
            })
            .collect(),
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    patch.add_signature(dummy_signature()).unwrap();
    store.write_object(&patch).unwrap()
}

/// A genesis-only chain carrying one well-formed `CreateFile` and one further, malformed patch on
/// top of it -- never through `derive_next_state_root` for the second block (which would itself
/// replay the malformed patch and refuse to build the fixture at all, as it should): the second
/// block's own signed root is a meaningless, never-admitted placeholder (it carries no snapshot,
/// so nothing ever reads or validates it). Both op builders take the real store, so any blob they
/// write lands in the same repository the chain itself reads from.
fn malformed_second_block_chain(
    label: &str,
    genesis_ops: impl FnOnce(&mut FileObjectStore) -> Vec<OperationKind>,
    malformed_ops: impl FnOnce(&mut FileObjectStore) -> Vec<OperationKind>,
) -> (RepositoryLayout, Vec<ObjectId>) {
    let root = unique_temp_dir(label);
    let layout = RepositoryLayout::init(root).unwrap();
    let mut store = FileObjectStore::new(layout.clone());

    let genesis_kinds = genesis_ops(&mut store);
    let genesis_patch_id = write_patch(&mut store, genesis_kinds);
    let genesis_root = crate::derive_next_state_root(&store, None, &[genesis_patch_id]).unwrap();
    let genesis_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![genesis_patch_id],
        None,
        genesis_root,
    );
    let genesis_block_id = store.write_object(&genesis_block).unwrap();

    let malformed_kinds = malformed_ops(&mut store);
    let malformed_patch_id = write_patch(&mut store, malformed_kinds);
    let malformed_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![genesis_block_id],
        vec![malformed_patch_id],
        None,
        genesis_root, // never read: this block carries no snapshot.
    );
    let malformed_block_id = store.write_object(&malformed_block).unwrap();

    let reader = ObjectReadSnapshot::open(&layout).unwrap();
    let block_ids = super::super::read::single_parent_chain(&reader, malformed_block_id).unwrap();
    (layout, block_ids)
}

/// Both engines must refuse -- not necessarily with the same words, but neither may silently
/// accept what the other refuses.
fn assert_both_engines_refuse(label: &str, layout: &RepositoryLayout, block_ids: &[ObjectId]) {
    let reader = ObjectReadSnapshot::open(layout).unwrap();
    let shared = super::super::anchor::replay_chain(
        &reader,
        block_ids,
        super::super::anchor::Anchoring::ReadOnlyReport,
    );
    assert!(
        shared.is_err(),
        "{label}: fixture sanity -- the shared replay must refuse this row"
    );
    let trust = crate::anchor_trust::AnchorTrust::load(layout);
    let extracted = replay_for_extract(&reader, block_ids, &trust, true);
    assert!(
        extracted.is_err(),
        "{label}: extract's own replay must refuse this row too, matching the shared replay"
    );
}

/// The standing "same refusals" table (R3): one malformed-operation row per kind. Every row the
/// shared replay refuses, `extract` must refuse too -- shown red against the pre-fix code by
/// temporarily reverting `953ae32c`'s own `ReplaceBinary` kind check (the report's own addendum
/// documents the manual red run; restored here, this row (and the whole table) is green).
#[test]
fn same_refusals_table() {
    let node_a = anchored_node(0x01);
    let node_b = anchored_node(0x02);

    // Row 1 -- CreateFile: a path that already exists (a different node id, same path).
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-createfile",
            |store| {
                let blob = write_blob(store, b"first").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "dup.txt".to_string(),
                    node_id: node_a,
                    blob_id: blob,
                    mode: 0o100644,
                })]
            },
            |store| {
                let other_blob = write_blob(store, b"second").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "dup.txt".to_string(),
                    node_id: node_b,
                    blob_id: other_blob,
                    mode: 0o100644,
                })]
            },
        );
        assert_both_engines_refuse("CreateFile (duplicate path)", &layout, &block_ids);
    }

    // Row 2 -- EditText: targets a node id that was never created.
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-edittext",
            |store| {
                let blob = write_blob(store, b"alpha").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "a.txt".to_string(),
                    node_id: node_a,
                    blob_id: blob,
                    mode: 0o100644,
                })]
            },
            |_store| {
                vec![OperationKind::EditText(EditText {
                    node_id: node_b, // never created
                    span_id: [0u8; 32],
                    old_span_hash: text_span_hash(b"y"),
                    left_anchor_hash: [0u8; 32],
                    right_anchor_hash: [0u8; 32],
                    replacement_text: b"x".to_vec(),
                    presentation_hint_line: None,
                    presentation_hint_column: None,
                    old_span_text: b"y".to_vec(),
                    left_anchor_len: None,
                    right_anchor_len: None,
                })]
            },
        );
        assert_both_engines_refuse("EditText (node never live)", &layout, &block_ids);
    }

    // Row 3 -- ReplaceBinary: new blob's real kind is not binary (953ae32c's own fix).
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-replacebinary",
            |store| {
                let binary_blob = write_binary_blob(store, b"old binary").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "asset.bin".to_string(),
                    node_id: node_a,
                    blob_id: binary_blob,
                    mode: 0o100644,
                })]
            },
            |store| {
                // The anchor id must match what genesis actually wrote -- recomputed identically,
                // since `write_binary_blob` is deterministic over the same bytes.
                let binary_blob = write_binary_blob(store, b"old binary").unwrap();
                let text_blob = write_blob(store, b"actually text").unwrap();
                vec![OperationKind::ReplaceBinary(ReplaceBinary {
                    node_id: node_a,
                    old_blob_id: binary_blob,
                    new_blob_id: text_blob,
                })]
            },
        );
        assert_both_engines_refuse(
            "ReplaceBinary (new blob is not binary)",
            &layout,
            &block_ids,
        );
    }

    // Row 4 -- DeleteNode: `old_blob_id` does not match the live node's actual current blob.
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-deletenode",
            |store| {
                let real_blob = write_blob(store, b"real content").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "d.txt".to_string(),
                    node_id: node_a,
                    blob_id: real_blob,
                    mode: 0o100644,
                })]
            },
            |store| {
                let wrong_blob = write_blob(store, b"wrong content").unwrap();
                vec![OperationKind::DeleteNode(DeleteNode {
                    path: "d.txt".to_string(),
                    node_id: node_a,
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: wrong_blob,
                        old_mode: 0o100644,
                    },
                })]
            },
        );
        assert_both_engines_refuse("DeleteNode (old_blob_id mismatch)", &layout, &block_ids);
    }

    // Row 5 -- ChangePerm: `old_mode` does not match the live node's actual current mode.
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-changeperm",
            |store| {
                let blob = write_blob(store, b"e").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "e.txt".to_string(),
                    node_id: node_a,
                    blob_id: blob,
                    mode: 0o100644,
                })]
            },
            |_store| {
                vec![OperationKind::ChangePerm(ChangePerm {
                    node_id: node_a,
                    old_mode: 0o100755, // wrong: the live node is 0o100644
                    new_mode: 0o100644,
                })]
            },
        );
        assert_both_engines_refuse("ChangePerm (old_mode mismatch)", &layout, &block_ids);
    }

    // Row 6 -- RenamePath batch: two sources named the same destination in one run.
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-renamepath",
            |store| {
                let blob_f = write_blob(store, b"f").unwrap();
                let blob_g = write_blob(store, b"g").unwrap();
                vec![
                    OperationKind::CreateFile(CreateFile {
                        path: "f.txt".to_string(),
                        node_id: node_a,
                        blob_id: blob_f,
                        mode: 0o100644,
                    }),
                    OperationKind::CreateFile(CreateFile {
                        path: "g.txt".to_string(),
                        node_id: node_b,
                        blob_id: blob_g,
                        mode: 0o100644,
                    }),
                ]
            },
            |_store| {
                vec![
                    OperationKind::RenamePath(RenamePath {
                        node_id: node_a,
                        old_path: "f.txt".to_string(),
                        new_path: "z.txt".to_string(),
                    }),
                    OperationKind::RenamePath(RenamePath {
                        node_id: node_b,
                        old_path: "g.txt".to_string(),
                        new_path: "z.txt".to_string(), // duplicate destination
                    }),
                ]
            },
        );
        assert_both_engines_refuse(
            "RenamePath batch (duplicate destination)",
            &layout,
            &block_ids,
        );
    }

    // Row 7 -- a symlink preimage: unsupported at `ensure_apply_supported`, the one check shared
    // verbatim by both engines (so this row is refused for the identical reason, not merely both
    // refusing independently).
    {
        let (layout, block_ids) = malformed_second_block_chain(
            "extract-refusal-row-symlink",
            |store| {
                let blob = write_blob(store, b"unrelated").unwrap();
                vec![OperationKind::CreateFile(CreateFile {
                    path: "unrelated.txt".to_string(),
                    node_id: node_b,
                    blob_id: blob,
                    mode: 0o100644,
                })]
            },
            |_store| {
                vec![OperationKind::DeleteNode(DeleteNode {
                    path: "link".to_string(),
                    node_id: node_a,
                    old_node_kind: NodeKind::Symlink,
                    preimage: DeleteNodePreimage::Symlink {
                        old_target: "target".to_string(),
                    },
                })]
            },
        );
        assert_both_engines_refuse(
            "DeleteNode (symlink preimage, unsupported)",
            &layout,
            &block_ids,
        );
    }
}
