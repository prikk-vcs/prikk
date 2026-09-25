//! RFC 159 §8.2 controls: the anchor-trust function, on **each of its users** -- stack A (`seal`, `merge`,
//! seal-from-accepted: the state a signed block continues from), RFC 136 2b's anchored worktree write
//! (`checkout --patch-materialize*`, `branch switch`), and whether `checkout --snapshot-materialize` may skip the
//! provisional marker. Each control drives all three through [`observe`] and names the perturbation that turned it red.
//!
//! **Masked guards.** Each condition is reached *past every earlier one*. The positive twin passes all four, and each
//! control breaks exactly the condition it names: an unadopted signer sits on a checkpoint that is recorded, near, and
//! whose manifest is valid; a mismatched signature likewise; a damaged manifest sits on an adopted, recorded, near anchor;
//! an unrecorded checkpoint is adopted, near and valid; a far anchor is recorded, adopted and valid.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;

use prikk_object::ObjectId;

use super::*;
use crate::anchor_fallback::take_anchor_fallbacks;
use crate::block_state::{StateAnchoring, derive_next_state_for_candidate_choosing};
use crate::patch_replay::anchor::snapshot_anchor_loads_for_test;
use crate::test_gates::test_support::{AnchoredHistory, Signing, unique_temp_dir};
use crate::verified_blocks::{load_verified_blocks, record_path, record_verified_blocks};
use crate::{DEFAULT_ACTIVE_NAME, MaintainerSigner, ObjectWriter, RepositoryLayout, Wal};

const MAIN: &str = "heads/main";
const SEAL: StateAnchoring = StateAnchoring::Anchored(AnchorSite::Seal);

/// What each user of the function did, on one repository.
#[derive(Debug)]
struct Seen {
    /// Stack A: whether the parent state came from an anchor.
    stack_a_anchored: bool,
    /// Stack A: whether the state and entries equal today's full walk's.
    stack_a_equals_full: bool,
    /// The findings `take_anchor_fallbacks` held after stack A.
    stack_a_findings: Vec<String>,
    /// The worktree write: the files it wrote (everything but `.prikk`), by path.
    worktree_files: BTreeMap<String, Vec<u8>>,
    /// The worktree write's returned finding.
    worktree_finding: Option<String>,
    /// The worktree write: how many snapshots its anchor search loaded.
    worktree_loads: usize,
    /// `--snapshot-materialize`: whether it set the provisional marker.
    provisional: bool,
    /// The findings after `--snapshot-materialize`.
    materialize_findings: Vec<String>,
    /// Admissions per site.
    uses: BTreeMap<&'static str, usize>,
}

fn tree_of(root: &std::path::Path) -> BTreeMap<String, Vec<u8>> {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut BTreeMap<String, Vec<u8>>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                if path.file_name().is_some_and(|name| name == ".prikk") {
                    continue;
                }
                walk(root, &path, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                out.insert(relative, std::fs::read(&path).unwrap());
            }
        }
    }
    let mut out = BTreeMap::new();
    walk(root, root, &mut out);
    out
}

/// Write one more patch (a new file) into the store and return its id: the candidate a `seal` would put on the tip.
fn next_patch(history: &mut AnchoredHistory) -> ObjectId {
    let op = history.create("candidate.txt", 0xEE, b"candidate\n");
    history.queue(vec![op]);
    let replay = Wal::for_layout(&history.layout, DEFAULT_ACTIVE_NAME)
        .replay()
        .unwrap();
    let envelope = replay.records.last().unwrap().envelope.clone();
    let mut store = history.store.clone();
    ObjectWriter::write_object(&mut store, &envelope).unwrap()
}

fn uses() -> BTreeMap<&'static str, usize> {
    [
        "seal",
        "merge",
        "seal-from-accepted",
        "worktree-write",
        "snapshot-materialize",
    ]
    .into_iter()
    .map(|site| (site, anchor_uses_for_test_support(site)))
    .collect()
}

/// Run every user of the function on `history`'s tip (the block after `history.blocks`' last), in the order stack A,
/// the worktree write, `--snapshot-materialize`. `tip` must be a checkpoint for the last to be meaningful.
fn observe(history: &mut AnchoredHistory) -> Seen {
    let _ = take_anchor_fallbacks();
    reset_anchor_uses_for_test_support();
    let tip = *history.blocks.last().unwrap();
    let candidate = next_patch(history);

    // Stack A: seal a block on the tip.
    let derive = |anchoring: StateAnchoring| {
        derive_next_state_for_candidate_choosing(
            &history.layout,
            &history.store,
            Some(tip),
            &[candidate],
            anchoring,
        )
        .expect("the candidate derives")
    };
    let anchored = derive(SEAL);
    let full = derive(StateAnchoring::Never);
    let stack_a_findings: Vec<String> = take_anchor_fallbacks()
        .into_iter()
        .map(|finding| format!("{finding}"))
        .collect();

    // The worktree write.
    let loads_before = snapshot_anchor_loads_for_test();
    let (_, worktree_finding) =
        crate::materialize_patch_checkout_reporting_anchor(&history.layout, MAIN)
            .expect("the worktree write");
    let worktree_loads = snapshot_anchor_loads_for_test() - loads_before;
    let worktree_files = tree_of(history.layout.root());

    // `--snapshot-materialize`.
    let report = crate::materialize_snapshot_checkout(&history.layout, MAIN)
        .expect("the snapshot materialization");
    let materialize_findings = take_anchor_fallbacks()
        .into_iter()
        .map(|finding| format!("{finding}"))
        .collect();
    Seen {
        stack_a_anchored: anchored.anchored,
        stack_a_equals_full: anchored.state == full.state && anchored.entries == full.entries,
        stack_a_findings,
        worktree_files,
        worktree_finding: worktree_finding.map(|finding| format!("{finding}")),
        worktree_loads,
        provisional: report.provisional,
        materialize_findings,
        uses: uses(),
    }
}

fn has_snapshot(history: &AnchoredHistory, block_id: ObjectId) -> bool {
    use crate::object_store::ObjectReader;
    let envelope = history.store.read_object(block_id).unwrap().unwrap();
    prikk_object::BlockPayload::decode_canonical(&envelope.canonical_payload)
        .unwrap()
        .snapshot_blob_ref
        .is_some()
}

/// The history every control uses: 65 blocks, so the tip **is** the second checkpoint (block 65, `blocks[64]`), which
/// a seal, a worktree write and `--snapshot-materialize` would all anchor at, at distance 0.
fn checkpoint_tip(name: &str, signing: Signing) -> AnchoredHistory {
    let history = AnchoredHistory::standard_signed(name, 65, signing);
    let tip = *history.blocks.last().unwrap();
    assert_eq!(history.blocks.len(), 65);
    assert!(
        has_snapshot(&history, tip),
        "fixture sanity: block 65 is a checkpoint"
    );
    history
}

/// **The positive twin, one control per call site** (RFC 159 handoff §2 control 10). Adopted, recorded, valid, near: every
/// user anchors -- and this is what every control below is measured against, so a control that goes red is red for the
/// one condition it breaks. Each site's control asserts *its* admission and nothing of the others', so removing the
/// anchoring at one site turns only that site's control red.
///
/// **`prikk seal`**: `seal_block_anchors_at_the_seal_site` (`block_state/anchored_parent/tests.rs`), through the public
/// `seal_block`. Stack A here is asserted at the derivation. **Perturb:** `StateAnchoring::Never` in `seal_block`.
#[test]
fn the_twin_anchors_at_stack_a() {
    let mut history = checkpoint_tip("rfc159-trust-twin-a", Signing::Adopted);
    let seen = observe(&mut history);
    assert!(seen.stack_a_anchored, "stack A anchors: {seen:?}");
    assert!(seen.stack_a_equals_full);
    assert!(seen.stack_a_findings.is_empty());
    assert_eq!(seen.uses["seal"], 1, "the seal site admitted it");
}

/// **The worktree write's site** (`patch_replay::replay_for_verified_worktree_write`). **Perturb:**
/// `Anchoring::Never` there: `uses["worktree-write"]` and the loads go to 0, and only this control (of the twins) is red.
#[test]
fn the_twin_anchors_at_the_worktree_write() {
    let mut history = checkpoint_tip("rfc159-trust-twin-w", Signing::Adopted);
    let seen = observe(&mut history);
    assert!(
        seen.uses["worktree-write"] >= 1,
        "the worktree write anchored: {seen:?}"
    );
    assert!(seen.worktree_loads >= 1);
    assert!(seen.worktree_finding.is_none());
}

/// **`--snapshot-materialize`'s marker** (`worktree.rs`): a snapshot the function admits needs none. **Perturb:**
/// `provisional = true` there: `uses` stays 1 (the function was asked) but the marker is set, and this goes red; make it
/// `false` and every refusal control goes red instead.
#[test]
fn the_twin_needs_no_provisional_marker() {
    let mut history = checkpoint_tip("rfc159-trust-twin-m", Signing::Adopted);
    let seen = observe(&mut history);
    assert!(
        !seen.provisional,
        "a snapshot the function admits needs no marker"
    );
    assert_eq!(seen.uses["snapshot-materialize"], 1);
    assert!(seen.materialize_findings.is_empty());
    for other in ["merge", "seal-from-accepted"] {
        assert_eq!(seen.uses[other], 0, "not the {other} site");
    }
}

/// What every refused case must show: no admission anywhere, and the results today's full derivation gives.
///
/// `silent` is whether the refusal is one the function makes without naming it: only then is the worktree write's search
/// known to have loaded nothing (a **named** refusal is one that read the snapshot to find it fail).
fn assert_full_walk_everywhere(seen: &Seen, twin: &Seen, silent: bool) {
    assert!(
        !seen.stack_a_anchored,
        "stack A ran the full walk: {seen:?}"
    );
    assert!(seen.stack_a_equals_full);
    assert_eq!(seen.uses["seal"], 0);
    assert_eq!(
        seen.uses["worktree-write"], 0,
        "the worktree write was not anchored: {seen:?}"
    );
    if silent {
        assert_eq!(seen.worktree_loads, 0, "and loaded no snapshot");
    }
    assert_eq!(seen.uses["snapshot-materialize"], 0);
    assert!(
        seen.provisional,
        "so `--snapshot-materialize` sets the marker"
    );
    assert_eq!(
        seen.worktree_files, twin.worktree_files,
        "the worktree holds what replay gives, the same as the anchored twin's"
    );
}

/// **An anchor whose signer is not adopted is never used** (control 4), on all three users. The checkpoint is recorded
/// (the seal that wrote it recorded it), near (distance 0) and its manifest valid; only the signer is unadopted. And
/// unadopted is **silent**: adoption is local, so a foreign signer is not damage.
/// **Perturb:** drop the signature condition in `admit` (make `verify_trusted_publication_envelope`'s `Err(_)` arm
/// `{}`): every user anchors and this goes red on each.
#[test]
fn an_anchor_signed_by_an_unadopted_key_is_never_used() {
    let mut twin_history = checkpoint_tip("rfc159-trust-twin2", Signing::Adopted);
    let twin = observe(&mut twin_history);
    let mut history = checkpoint_tip("rfc159-trust-unadopted", Signing::Unadopted);
    let tip = *history.blocks.last().unwrap();
    assert!(
        load_verified_blocks(&history.layout).contains(&tip),
        "fixture sanity: the checkpoint is recorded, so only the signer stands between it and the anchor"
    );
    let seen = observe(&mut history);
    assert_full_walk_everywhere(&seen, &twin, true);
    assert!(seen.stack_a_findings.is_empty(), "silent: {seen:?}");
    assert!(seen.worktree_finding.is_none());
    assert!(seen.materialize_findings.is_empty());
}

/// **The laundering case, closed** (control 5). A block whose snapshot **lies** -- its manifest recomputes to the block's
/// signed root, which its own patch does not produce -- signed by a key this repository has not adopted, its id **written
/// into the record by hand**. The record alone would have believed it (the prototype's `..._and_then_it_is_believed`).
/// Now: the worktree holds what replay gives, `--snapshot-materialize` sets the marker, and a `seal` on it does the full
/// walk, which refuses the lie. The **twin** signs with the adopted key and *is* believed, which is the stated residual
/// (a trusted maintainer signed a wrong root, and someone wrote `.prikk/cache/`), and proves the fixture reaches the
/// signature condition last.
/// **Perturb:** drop the signature condition: the unadopted case is believed again on all three.
#[test]
fn a_lying_snapshot_written_into_the_record_by_hand_is_refused_when_its_signer_is_not_adopted() {
    use crate::rfc111_seal_simulation::{
        SnapshotFixture, publish_snapshot_fixture_for_test_support,
    };

    let candidate_on = |layout: &RepositoryLayout, lying: ObjectId| {
        let mut store = crate::FileObjectStore::new(layout.clone());
        let patch = prikk_object::PatchPayload {
            operations: vec![prikk_object::Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: prikk_object::OperationKind::CreateFile(prikk_object::CreateFile {
                    path: "b.txt".to_string(),
                    node_id: prikk_object::NodeId::from_bytes([0x5B; 32]),
                    blob_id: crate::text_span::text_blob_id(b"b\n").unwrap(),
                    mode: 0o100_644,
                }),
            }],
            intent: None,
            preconditions: Vec::new(),
            purpose: prikk_object::PatchPurpose::Normal,
            message: None,
        };
        let blob = prikk_object::ObjectEnvelope::unsigned(
            prikk_object::ObjectType::Blob,
            1,
            prikk_object::CanonicalEncode::to_canonical_bytes(&prikk_object::BlobPayload::new(
                prikk_object::BlobKind::Text,
                b"b\n".to_vec(),
            ))
            .unwrap(),
        );
        ObjectWriter::write_object(&mut store, &blob).unwrap();
        let envelope = prikk_object::ObjectEnvelope::unsigned(
            prikk_object::ObjectType::Patch,
            1,
            prikk_object::CanonicalEncode::to_canonical_bytes(&patch).unwrap(),
        );
        let patch_id = ObjectWriter::write_object(&mut store, &envelope).unwrap();
        let derive = |anchoring| {
            derive_next_state_for_candidate_choosing(
                layout,
                &store,
                Some(lying),
                &[patch_id],
                anchoring,
            )
        };
        (derive(SEAL).is_ok(), derive(StateAnchoring::Never).is_ok())
    };

    // The lie, signed by a key that is not adopted; another key is adopted, so the policy is not empty.
    let unadopted =
        crate::Ed25519MaintainerSigner::from_seed("rfc159-lying-signer", &[0x3C; 32]).unwrap();
    let root = unique_temp_dir("rfc159-launder");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let other =
        crate::Ed25519MaintainerSigner::from_seed("rfc159-adopted-other", &[0x5A; 32]).unwrap();
    crate::add_trusted_maintainer(
        &layout,
        other.key_id(),
        &prikk_hash::to_hex(&other.public_key_bytes()),
    )
    .unwrap();
    let lying =
        publish_snapshot_fixture_for_test_support(&layout, &unadopted, SnapshotFixture::Lying)
            .unwrap();
    assert!(
        load_verified_blocks(&layout).is_empty(),
        "fixture sanity: nothing recorded yet"
    );
    record_verified_blocks(&layout, [lying]);
    assert!(
        load_verified_blocks(&layout).contains(&lying),
        "written by hand"
    );
    let _ = take_anchor_fallbacks();
    reset_anchor_uses_for_test_support();

    let (anchored_ok, full_ok) = candidate_on(&layout, lying);
    assert!(
        !full_ok,
        "the full walk refuses a block whose signed root its patch does not produce"
    );
    assert!(
        !anchored_ok,
        "and so does the anchored derivation, which must not believe the record"
    );
    assert!(
        take_anchor_fallbacks().is_empty(),
        "an unadopted signer is silent"
    );

    let loads = snapshot_anchor_loads_for_test();
    let (_, finding) = crate::materialize_patch_checkout_reporting_anchor(&layout, MAIN).unwrap();
    assert_eq!(
        snapshot_anchor_loads_for_test(),
        loads,
        "no snapshot was loaded"
    );
    assert!(finding.is_none());
    assert_eq!(
        std::fs::read(root.join("a.txt")).unwrap(),
        b"replayed\n",
        "the worktree holds what replay gives"
    );
    std::fs::remove_file(root.join("a.txt")).unwrap();
    let materialized = crate::materialize_snapshot_checkout(&layout, MAIN).unwrap();
    assert!(
        materialized.provisional,
        "`--snapshot-materialize` sets the provisional marker"
    );
    assert_eq!(anchor_uses_for_test_support("worktree-write"), 0);
    assert_eq!(anchor_uses_for_test_support("snapshot-materialize"), 0);
    assert_eq!(anchor_uses_for_test_support("seal"), 0);
    let _ = std::fs::remove_dir_all(&root);

    // The twin: the *adopted* maintainer signed the lie. This is the residual §8.2 states: believed.
    let root = unique_temp_dir("rfc159-launder-twin");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let trusted =
        crate::Ed25519MaintainerSigner::from_seed("rfc159-lying-signer", &[0x3C; 32]).unwrap();
    crate::add_trusted_maintainer(
        &layout,
        trusted.key_id(),
        &prikk_hash::to_hex(&trusted.public_key_bytes()),
    )
    .unwrap();
    let lying =
        publish_snapshot_fixture_for_test_support(&layout, &trusted, SnapshotFixture::Lying)
            .unwrap();
    record_verified_blocks(&layout, [lying]);
    reset_anchor_uses_for_test_support();
    let (anchored_ok, _) = candidate_on(&layout, lying);
    assert!(
        anchored_ok,
        "the twin: an adopted signer's recorded anchor is used"
    );
    assert_eq!(anchor_uses_for_test_support("seal"), 1);
    let _ = std::fs::remove_dir_all(&root);
    let _ = record_path(&layout);
}

/// **A recorded anchor whose signature fails against an adopted key is named on stderr** (control 6): the checkpoint is
/// signed under the *adopted key id* with another seed. Recorded, near, manifest valid: only the signature is wrong. Each
/// user names it (`take_anchor_fallbacks`, or the finding a replay returns) and runs the full walk; results equal the twin's.
/// **Perturb:** treat every verification error as `NotUsable` (drop the `INVALID_SIGNATURE` arm): the anchor is still
/// refused, and **nothing is named**, so every findings assertion goes red.
#[test]
fn a_recorded_anchor_whose_signature_fails_against_an_adopted_key_is_named() {
    let mut twin_history = checkpoint_tip("rfc159-trust-twin3", Signing::Adopted);
    let twin = observe(&mut twin_history);
    let mut history = checkpoint_tip("rfc159-trust-mismatched", Signing::Mismatched);
    let tip = *history.blocks.last().unwrap();
    assert!(
        load_verified_blocks(&history.layout).contains(&tip),
        "fixture sanity: recorded"
    );
    let seen = observe(&mut history);
    assert_full_walk_everywhere(&seen, &twin, false);
    let named = |findings: &[String]| {
        findings.len() == 1
            && findings[0].contains(&tip.to_hex())
            && findings[0].contains("was not used as an anchor")
    };
    assert!(
        named(&seen.stack_a_findings),
        "stack A names it: {:?}",
        seen.stack_a_findings
    );
    assert!(
        seen.worktree_finding
            .as_deref()
            .is_some_and(|finding| finding.contains(&tip.to_hex())),
        "the worktree write names it: {:?}",
        seen.worktree_finding
    );
    assert!(
        named(&seen.materialize_findings),
        "`--snapshot-materialize` names it: {:?}",
        seen.materialize_findings
    );
}

/// **A recorded anchor whose manifest fails validation is named**, on an adopted, recorded, near anchor (control 7,
/// manifest). One byte of the snapshot blob is flipped in the container. (`--snapshot-materialize` needs the manifest to
/// load at all and is refused before the function is asked, so it is not a user here.)
/// **Perturb:** treat `validate_snapshot_manifest`'s `Err` as `NotUsable`: nothing is named and this goes red.
#[test]
fn a_recorded_anchor_whose_manifest_fails_validation_is_named() {
    let mut history = checkpoint_tip("rfc159-trust-bad-manifest", Signing::Adopted);
    let tip = *history.blocks.last().unwrap();
    let container = history.layout.container_slot_path(
        prikk_object::ObjectType::Blob,
        crate::foundation::layout::ContainerSlot::A,
    );
    let mut bytes = std::fs::read(&container).unwrap();
    let marker = b"PRIKK-SNAPSHOT-MANIFEST";
    let at = bytes
        .windows(marker.len())
        .rposition(|window| window == marker)
        .expect("a snapshot manifest is in the blob container");
    bytes[at + marker.len() + 3] ^= 0x01;
    std::fs::write(&container, &bytes).unwrap();

    let _ = take_anchor_fallbacks();
    reset_anchor_uses_for_test_support();
    let candidate = next_patch(&mut history);
    let derive = |anchoring| {
        derive_next_state_for_candidate_choosing(
            &history.layout,
            &history.store,
            Some(tip),
            &[candidate],
            anchoring,
        )
        .expect("the full walk does not read the snapshot")
    };
    let anchored = derive(SEAL);
    let full = derive(StateAnchoring::Never);
    assert!(!anchored.anchored, "a damaged manifest is no anchor");
    assert_eq!(anchored.state, full.state);
    let findings = take_anchor_fallbacks();
    assert_eq!(findings.len(), 1, "named once: {findings:?}");
    assert_eq!(findings[0].block_id, tip);

    let loads = snapshot_anchor_loads_for_test();
    let (_, finding) =
        crate::materialize_patch_checkout_reporting_anchor(&history.layout, MAIN).unwrap();
    assert_eq!(anchor_uses_for_test_support("worktree-write"), 0);
    assert!(snapshot_anchor_loads_for_test() >= loads);
    assert!(
        finding.is_some_and(|finding| finding.block_id == tip),
        "the worktree write names it"
    );
}

/// **Not recorded** (control 7): the checkpoint is adopted, near and valid, and the record names *another* block, so the
/// membership test is reached (an empty record short-circuits before it). And with **no record at all**. Silent, on all
/// three users.
/// **Perturb:** drop the membership test in `admit`.
#[test]
fn a_checkpoint_the_record_omits_is_never_used_and_says_nothing() {
    let mut twin_history = checkpoint_tip("rfc159-trust-twin4", Signing::Adopted);
    let twin = observe(&mut twin_history);
    for (label, keep) in [("another block", true), ("an empty record", false)] {
        let mut history = checkpoint_tip("rfc159-trust-unrecorded", Signing::Adopted);
        let tip = *history.blocks.last().unwrap();
        std::fs::remove_file(record_path(&history.layout)).unwrap();
        if keep {
            record_verified_blocks(&history.layout, [history.blocks[3]]);
            assert!(!load_verified_blocks(&history.layout).contains(&tip));
            assert!(!load_verified_blocks(&history.layout).is_empty());
        }
        let seen = observe(&mut history);
        assert_full_walk_everywhere(&seen, &twin, true);
        assert!(seen.stack_a_findings.is_empty(), "{label}: silent");
        assert!(seen.worktree_finding.is_none(), "{label}: silent");
        assert!(seen.materialize_findings.is_empty(), "{label}: silent");
    }
}

/// **More than 63 blocks back** (control 7): the only recorded block is the first checkpoint (block 1), 64 blocks behind
/// the tip; it is adopted and valid. Silent, on stack A and the worktree write (`--snapshot-materialize` asks at
/// distance 0 and is not a user here).
/// **Perturb:** raise `MAX_ANCHOR_DISTANCE` by one or more.
#[test]
fn an_anchor_more_than_63_blocks_back_is_never_used_and_says_nothing() {
    let mut history = checkpoint_tip("rfc159-trust-far", Signing::Adopted);
    std::fs::remove_file(record_path(&history.layout)).unwrap();
    record_verified_blocks(&history.layout, [history.blocks[0]]);
    let seen = observe(&mut history);
    assert!(!seen.stack_a_anchored, "{seen:?}");
    assert!(seen.stack_a_equals_full);
    assert_eq!(seen.uses["seal"], 0);
    assert_eq!(seen.uses["worktree-write"], 0);
    assert_eq!(seen.worktree_loads, 0);
    assert!(seen.stack_a_findings.is_empty());
    assert!(seen.worktree_finding.is_none());
}
