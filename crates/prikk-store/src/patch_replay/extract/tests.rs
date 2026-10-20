//! Unit tests for `extract`'s own streamed replay (module doc). CLI-level, end-to-end coverage
//! (byte-identity against `cat`/`tree`, the collision/no-replace-rename/pre-walk controls, and the
//! `.prikk/`-untouched property) lives in `crates/prikk-cli/tests/extract.rs`, which can drive the
//! compiled binary and a real filesystem; this module pins the replay-internal shapes (streamed vs.
//! resident, rename-moves-nothing-but-the-path, the trust rule) directly against
//! [`ExtractReplayOutcome`], where a fixture's own signing/state-root crafting is easiest to reach.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeKind, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose, ReplaceBinary,
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
