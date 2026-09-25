//! RFC 136 increment 1b controls: the checkpoint writer, sealed through the real seal path
//! (`simulate_one_seal`, which calls `seal_block`), against §10.3a -- a checkpoint changes cost, never
//! output.
//!
//! **The comparison repository** is sealed through the same path inside
//! `block_state::without_checkpoints_for_test`, a `cfg(test)` thread-local that makes `checkpoint_due`
//! answer `false`. It is compiled only into this crate's own unit tests: no production build and no
//! `test-support` build can reach it. Block and RefState ids differ between the two by design (the
//! snapshot reference is signed), so reports are compared with each block id mapped to its twin.

use std::collections::BTreeMap;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockPayload, CanonicalEncode, ChangePerm, CreateFile, DeleteNode,
    DeleteNodePreimage, EditText, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose, RefKind, RefStatePayload,
    RefUpdatePayload, RenamePath,
};

use crate::block_state::without_checkpoints_for_test;
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::snapshot::load_block_snapshot;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    BundleImportOptions, DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, Ed25519MaintainerSigner,
    FileObjectStore, MaintainerSigner, ObjectReader, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, Wal, add_trusted_maintainer, append_rollback_draft, author_signature,
    compute_patch_set_digest_from_block, execute_merge, export_bundle,
    materialize_patch_checkout_with_deletions, materialize_snapshot_checkout,
    plan_patch_checkout_deletions, prepare_patch_inverse_plan, prepare_patch_plan_content_report,
    prepare_patch_replay_plan, prepare_rollback_preview, preview_bundle, show, switch_branch,
    verify_repository, write_active_ref_metadata,
};

const MAIN: &str = "heads/main";
const REGULAR: u32 = 0o100644;

fn node(seed: u8) -> NodeId {
    NodeId::from_bytes([seed; 32])
}

/// The content identity of a text file's bytes: the id of the ordinary schema-1 Text Blob
/// (`text_span.rs`), whether or not anyone stored it.
fn text_blob_id(bytes: &[u8]) -> Result<ObjectId> {
    Ok(ObjectId::from_canonical_payload(
        ObjectType::Blob,
        1,
        &BlobPayload::new(BlobKind::Text, bytes.to_vec()).to_canonical_bytes()?,
    ))
}

fn integrity(message: &str) -> PrikkError {
    PrikkError::Integrity(message.to_string())
}

/// A repository sealed one patch per block through `simulate_one_seal`, with fixed signer seeds and
/// fixed node ids, so two sealers given the same operations produce the same patches.
struct Sealer {
    layout: RepositoryLayout,
    maintainer: Ed25519MaintainerSigner,
    author: Ed25519AuthorSigner,
    store: FileObjectStore,
    text: BTreeMap<u8, Vec<u8>>,
    modes: BTreeMap<u8, u32>,
    /// Every block sealed on `heads/main`, in order.
    blocks: Vec<ObjectId>,
    checkpoints: bool,
}

impl Sealer {
    fn new(name: &str, checkpoints: bool) -> Result<Self> {
        let layout = RepositoryLayout::init(unique_temp_dir(name))?;
        let maintainer = Ed25519MaintainerSigner::from_seed("rfc136-1b-maintainer", &[0x1B; 32])?;
        add_trusted_maintainer(
            &layout,
            maintainer.key_id(),
            &prikk_hash::to_hex(&maintainer.public_key_bytes()),
        )?;
        let author = Ed25519AuthorSigner::from_seed("rfc136-1b-author", &[0xB1; 32])?;
        Ok(Self {
            store: FileObjectStore::new(layout.clone()),
            layout,
            maintainer,
            author,
            text: BTreeMap::new(),
            modes: BTreeMap::new(),
            blocks: Vec::new(),
            checkpoints,
        })
    }

    fn create(&mut self, path: &str, seed: u8, bytes: &[u8]) -> Result<OperationKind> {
        let envelope = ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            BlobPayload::new(BlobKind::Text, bytes.to_vec()).to_canonical_bytes()?,
        );
        let blob_id = self.store.write_object(&envelope)?;
        self.text.insert(seed, bytes.to_vec());
        self.modes.insert(seed, REGULAR);
        Ok(OperationKind::CreateFile(CreateFile {
            path: path.to_string(),
            node_id: node(seed),
            blob_id,
            mode: REGULAR,
        }))
    }

    fn edit(&mut self, seed: u8, new: &[u8]) -> Result<OperationKind> {
        let old = self
            .text
            .get(&seed)
            .cloned()
            .ok_or_else(|| integrity("edit of an unknown node"))?;
        let span = crate::text_span::plan_authored_text_span(&old, new, node(seed))
            .map_err(|err| PrikkError::Integrity(err.to_string()))?
            .ok_or_else(|| integrity("the edit changes nothing"))?;
        self.text.insert(seed, new.to_vec());
        Ok(OperationKind::EditText(EditText {
            node_id: node(seed),
            span_id: span.span_id,
            old_span_hash: span.old_span_hash,
            left_anchor_hash: span.left_anchor_hash,
            right_anchor_hash: span.right_anchor_hash,
            replacement_text: span.replacement_text,
            presentation_hint_line: None,
            presentation_hint_column: None,
            old_span_text: span.old_span_text,
            left_anchor_len: Some(span.left_anchor_len),
            right_anchor_len: Some(span.right_anchor_len),
        }))
    }

    fn delete(&mut self, path: &str, seed: u8) -> Result<OperationKind> {
        let bytes = self
            .text
            .remove(&seed)
            .ok_or_else(|| integrity("delete of an unknown node"))?;
        let old_mode = self.modes.remove(&seed).unwrap_or(REGULAR);
        Ok(OperationKind::DeleteNode(DeleteNode {
            path: path.to_string(),
            node_id: node(seed),
            old_node_kind: NodeKind::TextFile,
            preimage: DeleteNodePreimage::File {
                old_blob_id: text_blob_id(&bytes)?,
                old_mode,
            },
        }))
    }

    fn rename(seed: u8, old_path: &str, new_path: &str) -> OperationKind {
        OperationKind::RenamePath(RenamePath {
            node_id: node(seed),
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
        })
    }

    fn chmod(&mut self, seed: u8, new_mode: u32) -> OperationKind {
        let old_mode = self.modes.insert(seed, new_mode).unwrap_or(REGULAR);
        OperationKind::ChangePerm(ChangePerm {
            node_id: node(seed),
            old_mode,
            new_mode,
        })
    }

    /// Seal one patch of `kinds` onto `ref_name` and return the new tip.
    fn seal_on(&mut self, ref_name: &str, kinds: Vec<OperationKind>) -> Result<ObjectId> {
        let operations = kinds
            .into_iter()
            .zip(1_u32..)
            .map(|(kind, op_seq)| Operation {
                op_seq,
                op_id: None,
                preconditions: Vec::new(),
                kind,
            })
            .collect();
        let payload = PatchPayload {
            operations,
            intent: None,
            preconditions: Vec::new(),
            purpose: PatchPurpose::Normal,
            message: None,
        };
        let mut patch = ObjectEnvelope::unsigned(
            ObjectType::Patch,
            prikk_object::PATCH_TEXT_SPAN_V2_SCHEMA,
            payload.to_canonical_bytes()?,
        );
        let patch_id = patch.object_id();
        patch.add_signature(author_signature(&self.author, patch_id)?)?;
        Wal::for_layout(&self.layout, DEFAULT_ACTIVE_NAME).append_patch(&patch)?;
        write_active_ref_metadata(&self.layout, ref_name)?;
        let seal = || simulate_one_seal(&self.layout, ref_name, &self.maintainer);
        if self.checkpoints {
            seal()?;
        } else {
            without_checkpoints_for_test(seal)?;
        }
        crate::refs::read_current_ref_tip_block(&self.layout, &self.store, ref_name)
    }

    fn seal(&mut self, kinds: Vec<OperationKind>) -> Result<ObjectId> {
        let tip = self.seal_on(MAIN, kinds)?;
        self.blocks.push(tip);
        Ok(tip)
    }

    fn block(&self, block_id: ObjectId) -> Result<BlockPayload> {
        let envelope = self
            .store
            .read_typed(block_id, ObjectType::Block)?
            .ok_or_else(|| integrity("sealed Block is missing"))?;
        BlockPayload::decode_canonical(&envelope.canonical_payload)
    }

    /// 1-based positions on `heads/main` of the blocks that carry a snapshot.
    fn checkpoint_positions(&self) -> Result<Vec<usize>> {
        let mut positions = Vec::new();
        for (index, block_id) in self.blocks.iter().enumerate() {
            if self.block(*block_id)?.snapshot_blob_ref.is_some() {
                positions.push(index + 1);
            }
        }
        Ok(positions)
    }

    fn publish_branch(&self, name: &str, block_id: ObjectId) -> Result<()> {
        let state = RefStatePayload {
            ref_name: name.to_string(),
            kind: RefKind::Branch,
            target_object_id: block_id,
            update_seq: 1,
            previous_ref_state_id: None,
            required_attestation_ids: Vec::new(),
            closed: false,
        };
        let mut state_envelope =
            ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes()?);
        let state_id = state_envelope.object_id();
        state_envelope.add_signature(crate::maintainer_signature(
            &self.maintainer,
            ObjectType::RefState,
            state_id,
        )?)?;
        let update = RefUpdatePayload {
            ref_name: name.to_string(),
            old_ref_state_id: None,
            new_ref_state_id: state_id,
            new_target_object_id: block_id,
            update_seq: 1,
            created_at: 0,
            author_key_id: self.maintainer.key_id().to_string(),
        };
        let mut update_envelope =
            ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update.to_canonical_bytes()?);
        let update_id = update_envelope.object_id();
        update_envelope.add_signature(crate::maintainer_signature(
            &self.maintainer,
            ObjectType::RefUpdate,
            update_id,
        )?)?;
        RefStore::new(self.layout.clone()).publish(&RefPublication {
            ref_name: name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state: state_envelope,
            ref_update: update_envelope,
        })?;
        Ok(())
    }
}

impl Drop for Sealer {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.layout.root());
    }
}

/// Every file under `root` except `.prikk`, as sorted (path, bytes).
fn worktree_listing(root: &Path) -> Result<Vec<(String, Vec<u8>)>> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.file_name().is_some_and(|name| name == ".prikk") {
                continue;
            }
            if path.is_dir() {
                walk(root, &path, out)?;
            } else {
                let relative = path
                    .strip_prefix(root)
                    .map_err(|err| PrikkError::Integrity(err.to_string()))?;
                out.push((relative.display().to_string(), std::fs::read(&path)?));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, root, &mut out)?;
    out.sort();
    Ok(out)
}

/// Before the second checkpoint (block 65): an `EditText`, a delete, a rename and a `ChangePerm`.
/// 66 blocks in all, so checkpoints fall at blocks 1 and 65.
fn seal_invisible_history(sealer: &mut Sealer) -> Result<()> {
    let ops = vec![
        sealer.create("a.txt", 0xA1, b"alpha beta\n")?,
        sealer.create("old.txt", 0x01, b"old\n")?,
        sealer.create("r.txt", 0x02, b"rename me\n")?,
        sealer.create("keep.txt", 0x03, b"keep\n")?,
    ];
    sealer.seal(ops)?;
    let op = sealer.edit(0xA1, b"alpha BETA\n")?;
    sealer.seal(vec![op])?;
    let op = sealer.delete("old.txt", 0x01)?;
    sealer.seal(vec![op])?;
    sealer.seal(vec![Sealer::rename(0x02, "r.txt", "renamed.txt")])?;
    let op = sealer.chmod(0x03, 0o100755);
    sealer.seal(vec![op])?;
    for index in 6_u8..=66 {
        let op = sealer.create(&format!("f{index}.txt"), 0x10 + index, b"filler\n")?;
        sealer.seal(vec![op])?;
    }
    Ok(())
}

/// Every report §10.3a names, as text, in a fixed order. The worktree starts with `old.txt` holding
/// its old bytes -- a stale file that history deleted.
fn every_report(sealer: &Sealer) -> Result<Vec<(&'static str, String)>> {
    let layout = &sealer.layout;
    std::fs::write(layout.root().join("old.txt"), b"old\n")?;
    let requested: Vec<String> = ["a.txt", "renamed.txt", "old.txt", "keep.txt"]
        .into_iter()
        .map(String::from)
        .collect();
    let mut reports = vec![
        (
            "patch plan",
            format!("{:?}", prepare_patch_replay_plan(layout, MAIN)),
        ),
        (
            "replay, coverage and deleted files",
            format!(
                "{:?}",
                crate::patch_replay::replay_supported_patch_chain(layout, MAIN)
            ),
        ),
        (
            "content report",
            format!(
                "{:?}",
                prepare_patch_plan_content_report(layout, MAIN, &requested)
            ),
        ),
        (
            "delete plan",
            format!("{:?}", plan_patch_checkout_deletions(layout, MAIN)),
        ),
        (
            "inverse plan",
            format!("{:?}", prepare_patch_inverse_plan(layout, MAIN)),
        ),
        (
            "rollback preview",
            format!("{:?}", prepare_rollback_preview(layout, MAIN)),
        ),
        (
            "rollback draft",
            format!(
                "{:?}",
                append_rollback_draft(layout, MAIN, "undo", &sealer.author)
            ),
        ),
        (
            "materialize with deletions",
            format!(
                "{:?}",
                materialize_patch_checkout_with_deletions(layout, MAIN)
            ),
        ),
    ];
    reports.push((
        "worktree after materialize",
        format!("{:?}", worktree_listing(layout.root())?),
    ));

    let (_, bundle) = export_bundle(layout, MAIN)?;
    let target_root = unique_temp_dir("rfc136-1b-preview-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let preview = preview_bundle(
        &target,
        &bundle,
        &BundleImportOptions::default_limits(),
        MAIN,
    )?;
    let _ = std::fs::remove_dir_all(target_root);
    // The bundle's own manifest counts stored objects -- an inventory §10.3a ruling 1 exempts.
    reports.push((
        "bundle preview",
        format!(
            "{:?} {:?} {:?}",
            preview.connectivity, preview.conflict, preview.effects
        ),
    ));

    let other = *sealer
        .blocks
        .get(2)
        .ok_or_else(|| integrity("history has no third block"))?;
    sealer.publish_branch("heads/other", other)?;
    reports.push((
        "branch switch",
        format!("{:?}", switch_branch(layout, Some(MAIN), "heads/other")),
    ));
    reports.push((
        "worktree after switch",
        format!("{:?}", worktree_listing(layout.root())?),
    ));
    Ok(reports)
}

/// Replace every block id of `from` with its twin in `to`, in both renderings.
fn map_block_ids(mut text: String, from: &[ObjectId], to: &[ObjectId]) -> String {
    for (a, b) in from.iter().zip(to) {
        text = text.replace(&format!("{a:?}"), &format!("{b:?}"));
        text = text.replace(&a.to_string(), &b.to_string());
    }
    text
}

/// §10.3a ruling 1: every report equals, output for output, the same history sealed with no
/// checkpoints -- with a delete whose stale file is in the worktree, an edit, a rename and a mode
/// change all before the second checkpoint.
#[test]
fn checkpoints_are_invisible_to_every_report() -> Result<()> {
    let mut sealed = Sealer::new("rfc136-1b-invisible", true)?;
    let mut plain = Sealer::new("rfc136-1b-invisible-plain", false)?;
    let mut again = Sealer::new("rfc136-1b-invisible-again", true)?;
    seal_invisible_history(&mut sealed)?;
    seal_invisible_history(&mut plain)?;
    seal_invisible_history(&mut again)?;

    assert_eq!(sealed.checkpoint_positions()?, vec![1, 65]);
    assert!(plain.checkpoint_positions()?.is_empty());
    assert_eq!(
        sealed.blocks, again.blocks,
        "two repositories sealing the same patches produce the same block ids"
    );
    let (sealed_tip, plain_tip) = (
        *sealed.blocks.last().ok_or_else(|| integrity("no blocks"))?,
        *plain.blocks.last().ok_or_else(|| integrity("no blocks"))?,
    );
    assert_ne!(
        sealed_tip, plain_tip,
        "fixture sanity: the snapshot reference is signed"
    );
    assert_eq!(
        compute_patch_set_digest_from_block(&sealed.store, sealed_tip)?,
        compute_patch_set_digest_from_block(&plain.store, plain_tip)?,
        "patch_set_digest is unchanged by a checkpoint"
    );

    let sealed_reports = every_report(&sealed)?;
    let plain_reports = every_report(&plain)?;
    assert!(
        !sealed.layout.root().join("old.txt").exists()
            || !plain.layout.root().join("old.txt").exists()
            || sealed_reports
                .iter()
                .any(|(name, _)| *name == "branch switch"),
        "fixture sanity"
    );
    for ((name, sealed_report), (_, plain_report)) in sealed_reports.into_iter().zip(plain_reports)
    {
        assert_eq!(
            map_block_ids(sealed_report, &sealed.blocks, &plain.blocks),
            plain_report,
            "{name} differs between the checkpointed repository and its twin"
        );
    }
    Ok(())
}

/// §10.3a: a checkpoint never fails a seal that would succeed without it. 130 blocks with edits
/// scattered before and after the merge's baseline (a side editing one text more than once), a delete of an edited node, a rename of an edited node, and a
/// merge whose mainline reaches the cadence (the merge block is the checkpoint at 129); block 130 edits
/// again after it.
#[test]
fn a_checkpoint_never_fails_a_seal_that_would_succeed_without_it() -> Result<()> {
    let mut sealer = Sealer::new("rfc136-1b-130", true)?;
    let ops = vec![
        sealer.create("e.txt", 0xE1, b"edit 1\n")?,
        sealer.create("g.txt", 0xE2, b"gone 1\n")?,
        sealer.create("h.txt", 0xE3, b"moved 1\n")?,
    ];
    sealer.seal(ops)?;
    let mut filler: u8 = 0;
    let mut next = |sealer: &mut Sealer, number: usize| -> Result<OperationKind> {
        match number {
            40 => sealer.edit(0xE2, b"gone 40\n"),
            41 => sealer.delete("g.txt", 0xE2),
            50 => sealer.edit(0xE3, b"moved 50\n"),
            51 => Ok(Sealer::rename(0xE3, "h.txt", "h2.txt")),
            // Edits continue on main after the merge's baseline (block 100), at 105, 112, 119 and 126:
            // a side that edits one text more than once merges (DC-75 two-edits handoff §3).
            number if number % 7 == 0 => sealer.edit(0xE1, format!("edit {number}\n").as_bytes()),
            number => {
                filler += 1;
                sealer.create(&format!("f{number}.txt"), filler, b"filler\n")
            }
        }
    };
    while sealer.blocks.len() < 128 {
        let number = sealer.blocks.len() + 1;
        let op = next(&mut sealer, number)?;
        sealer.seal(vec![op])?;
    }
    let baseline = *sealer
        .blocks
        .get(99)
        .ok_or_else(|| integrity("no block 100"))?;
    sealer.publish_branch("heads/side", baseline)?;
    let op = sealer.create("side.txt", 0xF0, b"side\n")?;
    sealer.seal_on("heads/side", vec![op])?;
    let report = execute_merge(
        &sealer.layout,
        baseline,
        MAIN,
        "heads/side",
        &sealer.maintainer,
    )?;
    sealer.blocks.push(report.block_id);
    let op = sealer.edit(0xE1, b"edit 130\n")?;
    sealer.seal(vec![op])?;

    assert_eq!(sealer.blocks.len(), 130);
    assert_eq!(sealer.checkpoint_positions()?, vec![1, 65, 129]);
    for position in [1_usize, 65, 129] {
        let block_id = *sealer
            .blocks
            .get(position - 1)
            .ok_or_else(|| integrity("no such block"))?;
        let block = sealer.block(block_id)?;
        assert!(
            load_block_snapshot(&sealer.store, block_id, &block)?.is_some(),
            "the checkpoint at {position} loads"
        );
    }
    let verification = verify_repository(&sealer.layout)?;
    assert!(
        !verification.has_stage_failure() && !verification.has_item_failure(),
        "verify is clean: {verification:?}"
    );
    Ok(())
}

/// Ruling 1 of the increment 1a review: a checkpoint stores every live file's content, edited or
/// not. On a checkpoint tip over 64 edits, `checkout --snapshot-materialize` writes the replayed
/// bytes; and `show` on a later delete of that file reports its content, where the same history
/// sealed without checkpoints reports it unavailable.
#[test]
fn a_checkpoint_over_edited_text_checks_out_the_replayed_bytes_and_show_can_read_them() -> Result<()>
{
    fn seal_edits(sealer: &mut Sealer) -> Result<ObjectId> {
        let op = sealer.create("t.txt", 0x7A, b"text 1\n")?;
        sealer.seal(vec![op])?;
        for number in 2..=65 {
            let op = sealer.edit(0x7A, format!("text {number}\n").as_bytes())?;
            sealer.seal(vec![op])?;
        }
        let op = sealer.delete("t.txt", 0x7A)?;
        let deleted = sealer.seal(vec![op])?;
        Ok(deleted)
    }
    let mut sealed = Sealer::new("rfc136-1b-edited", true)?;
    let mut plain = Sealer::new("rfc136-1b-edited-plain", false)?;
    let op = sealed.create("t.txt", 0x7A, b"text 1\n")?;
    sealed.seal(vec![op])?;
    for number in 2..=65 {
        let op = sealed.edit(0x7A, format!("text {number}\n").as_bytes())?;
        sealed.seal(vec![op])?;
    }
    assert_eq!(sealed.checkpoint_positions()?, vec![1, 65]);
    let checkpoint = materialize_snapshot_checkout(&sealed.layout, MAIN)?;
    assert_eq!(checkpoint.paths, vec!["t.txt".to_string()]);
    assert_eq!(
        std::fs::read(sealed.layout.root().join("t.txt"))?,
        b"text 65\n"
    );
    let replayed = prepare_patch_plan_content_report(&sealed.layout, MAIN, &["t.txt".to_string()])?;
    assert_eq!(
        format!("{:?}", replayed.entries),
        format!(
            "{:?}",
            prepare_patch_plan_content_report(&sealed.layout, MAIN, &["t.txt".to_string()])?
                .entries
        )
    );
    assert!(format!("{:?}", replayed.entries).contains(&format!("{:?}", b"text 65\n".to_vec())));

    let op = sealed.delete("t.txt", 0x7A)?;
    let deleted = sealed.seal(vec![op])?;
    let plain_deleted = seal_edits(&mut plain)?;
    let sealed_show = format!("{:?}", show(&sealed.layout, deleted)?);
    let plain_show = format!("{:?}", show(&plain.layout, plain_deleted)?);
    assert!(
        !sealed_show.contains("Unavailable"),
        "the checkpoint stored the deleted content: {sealed_show}"
    );
    assert!(
        plain_show.contains("Unavailable"),
        "fixture sanity -- without a checkpoint the edited content is unstored: {plain_show}"
    );
    Ok(())
}

// ---- RFC 136 increment 2a: anchored read-only reports (§10.3c ruling 1, §10.3a ruling 5) --------------

/// A binary file created on `seed`.
fn create_binary(
    sealer: &mut Sealer,
    path: &str,
    seed: u8,
    bytes: &[u8],
) -> Result<(OperationKind, ObjectId)> {
    let envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        BlobPayload::new(BlobKind::Binary, bytes.to_vec()).to_canonical_bytes()?,
    );
    let blob_id = sealer.store.write_object(&envelope)?;
    sealer.modes.insert(seed, REGULAR);
    Ok((
        OperationKind::CreateFile(CreateFile {
            path: path.to_string(),
            node_id: node(seed),
            blob_id,
            mode: REGULAR,
        }),
        blob_id,
    ))
}

/// A binary replacement of `seed`'s content `old` with `bytes`.
fn replace_binary(
    sealer: &mut Sealer,
    seed: u8,
    old: ObjectId,
    bytes: &[u8],
) -> Result<(OperationKind, ObjectId)> {
    let envelope = ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        BlobPayload::new(BlobKind::Binary, bytes.to_vec()).to_canonical_bytes()?,
    );
    let new_blob_id = sealer.store.write_object(&envelope)?;
    Ok((
        OperationKind::ReplaceBinary(prikk_object::ReplaceBinary {
            node_id: node(seed),
            old_blob_id: old,
            new_blob_id,
        }),
        new_blob_id,
    ))
}

/// 130 blocks, checkpoints at 1, 65 and 129: edits of `a.txt` across blocks, a `DeleteFile` of
/// `old.txt` at 20 (before the checkpoint at 65), a rename at 30, a `chmod` at 40, and binary
/// replacements at 50 (before the anchor) and 120 (after the checkpoint at 65).
fn seal_anchor_history(sealer: &mut Sealer) -> Result<()> {
    let (binary, mut binary_id) = create_binary(sealer, "bin.dat", 0xB1, b"\0binary 0")?;
    let first = vec![
        sealer.create("a.txt", 0xA1, b"alpha 0\n")?,
        sealer.create("keep.txt", 0xA2, b"keep\n")?,
        sealer.create("old.txt", 0xA3, b"old\n")?,
        binary,
    ];
    sealer.seal(first)?;
    let mut filler: u8 = 0;
    while sealer.blocks.len() < 130 {
        let number = sealer.blocks.len() + 1;
        let op = match number {
            20 => sealer.delete("old.txt", 0xA3)?,
            30 => Sealer::rename(0xA2, "keep.txt", "renamed.txt"),
            40 => sealer.chmod(0xA1, 0o100755),
            50 | 120 => {
                let bytes = format!("\0binary {number}");
                let (op, id) = replace_binary(sealer, 0xB1, binary_id, bytes.as_bytes())?;
                binary_id = id;
                op
            }
            number if number % 5 == 0 => {
                sealer.edit(0xA1, format!("alpha {number}\n").as_bytes())?
            }
            number => {
                filler += 1;
                sealer.create(&format!("f{number}.txt"), filler, b"filler\n")?
            }
        };
        sealer.seal(vec![op])?;
    }
    Ok(())
}

/// §10.3a ruling 1 and §10.3c ruling 1: every read-only report anchored at a snapshot equals the same
/// report replayed from genesis -- `checkout --patch-plan`, its content report, `--patch-delete-plan`
/// and bundle preview. The counter proves each one did anchor.
#[test]
fn anchored_read_only_reports_equal_the_same_reports_replayed_from_genesis() -> Result<()> {
    use crate::patch_replay::anchor::{snapshot_anchor_loads_for_test, without_anchoring_for_test};

    let mut sealer = Sealer::new("rfc136-2a-anchored", true)?;
    seal_anchor_history(&mut sealer)?;
    assert_eq!(sealer.checkpoint_positions()?, vec![1, 65, 129]);
    let layout = &sealer.layout;
    // The stale file the DeleteFile at block 20 removed, back in the worktree with its old bytes.
    std::fs::write(layout.root().join("old.txt"), b"old\n")?;
    let requested: Vec<String> = ["a.txt", "renamed.txt", "old.txt", "bin.dat", "missing.txt"]
        .into_iter()
        .map(String::from)
        .collect();
    let (_, bundle) = export_bundle(layout, MAIN)?;

    let reports = |label: &str| -> Result<Vec<String>> {
        let target_root = unique_temp_dir(&format!("rfc136-2a-preview-{label}"));
        let target = RepositoryLayout::init(target_root.clone())?;
        let (plan, plan_fallback) =
            crate::prepare_patch_replay_plan_reporting_anchor(layout, MAIN)?;
        let (content, content_fallback) =
            crate::prepare_patch_plan_content_report_reporting_anchor(layout, MAIN, &requested)?;
        let (deletions, deletion_fallback) =
            crate::plan_patch_checkout_deletions_reporting_anchor(layout, MAIN)?;
        let (preview, preview_fallbacks) = crate::preview_bundle_reporting_anchor(
            &target,
            &bundle,
            &BundleImportOptions::default_limits(),
            MAIN,
        )?;
        let _ = std::fs::remove_dir_all(target_root);
        assert!(
            plan_fallback.is_none()
                && content_fallback.is_none()
                && deletion_fallback.is_none()
                && preview_fallbacks.is_empty(),
            "{label}: no snapshot in this history fails validation"
        );
        Ok(vec![
            format!("{plan:?}"),
            format!("{content:?}"),
            format!("{deletions:?}"),
            format!(
                "{:?} {:?} {:?}",
                preview.connectivity, preview.conflict, preview.effects
            ),
        ])
    };

    let loads_before = snapshot_anchor_loads_for_test();
    let anchored = reports("anchored")?;
    let loads = snapshot_anchor_loads_for_test().saturating_sub(loads_before);
    assert!(
        loads >= 4,
        "fixture sanity: all four reports anchored ({loads} snapshot loads)"
    );
    let loads_before = snapshot_anchor_loads_for_test();
    let from_genesis = without_anchoring_for_test(|| reports("genesis"))?;
    assert_eq!(
        snapshot_anchor_loads_for_test(),
        loads_before,
        "fixture sanity: the comparison did not anchor"
    );
    for (anchored, genesis) in anchored.iter().zip(&from_genesis) {
        assert_eq!(
            anchored, genesis,
            "an anchored report differs from genesis replay"
        );
    }

    let [_, content, deletions, _] = anchored.as_slice() else {
        return Err(integrity("four reports"));
    };
    for kind in [
        "delete-node",
        "rename-path",
        "change-perm",
        "replace-binary",
        "edit-text",
    ] {
        assert!(
            content.contains(kind),
            "fixture sanity: pre-anchor history carries {kind}: {content}"
        );
    }
    assert!(
        deletions.contains("planned_deletions: 1") && deletions.contains("deletable_files: 1"),
        "fixture sanity: the pre-checkpoint deletion is planned: {deletions}"
    );
    Ok(())
}

/// §10.3c ruling 2: rollback preview replays from genesis and loads no snapshot, even with every checkpoint
/// recorded as replay-verified; worktree writes load none once the record is gone (RFC 136 increment 2b:
/// they anchor only at recorded blocks). Asserted through the anchor search's own counter, not through
/// timing. Rollback preview runs on a rename-free history, because inverse planning refuses a rename.
#[test]
fn worktree_writes_and_rollback_preview_load_no_snapshot() -> Result<()> {
    use crate::patch_replay::anchor::snapshot_anchor_loads_for_test;

    let mut sealer = Sealer::new("rfc136-2a-unanchored-writers", true)?;
    seal_anchor_history(&mut sealer)?;
    let other = *sealer
        .blocks
        .get(100)
        .ok_or_else(|| integrity("history has no block 101"))?;
    sealer.publish_branch("heads/other", other)?;
    let layout = &sealer.layout;

    let mut rename_free = Sealer::new("rfc136-2a-unanchored-rollback", true)?;
    let first = vec![rename_free.create("a.txt", 0xA1, b"alpha 0\n")?];
    rename_free.seal(first)?;
    while rename_free.blocks.len() < 70 {
        let number = rename_free.blocks.len() + 1;
        let op = rename_free.edit(0xA1, format!("alpha {number}\n").as_bytes())?;
        rename_free.seal(vec![op])?;
    }
    assert_eq!(rename_free.checkpoint_positions()?, vec![1, 65]);
    assert!(
        !crate::verified_blocks::load_verified_blocks(&rename_free.layout).is_empty(),
        "fixture sanity: rollback preview's repository keeps its record"
    );
    std::fs::remove_file(crate::verified_blocks::record_path(layout))?;

    let before = snapshot_anchor_loads_for_test();
    prepare_rollback_preview(&rename_free.layout, MAIN)?;
    materialize_patch_checkout_with_deletions(layout, MAIN)?;
    switch_branch(layout, Some(MAIN), "heads/other")?;
    assert_eq!(
        snapshot_anchor_loads_for_test(),
        before,
        "a worktree write or rollback preview loaded a snapshot"
    );

    // Control: a read-only report on each repository does load one.
    crate::prepare_patch_replay_plan_reporting_anchor(layout, "heads/other")?;
    let after_other = snapshot_anchor_loads_for_test();
    assert!(after_other > before);
    crate::prepare_patch_replay_plan_reporting_anchor(&rename_free.layout, MAIN)?;
    assert!(snapshot_anchor_loads_for_test() > after_other);
    Ok(())
}

// ---- RFC 136 increment 2b §3: the record's writers ----------------------------------------------------

/// `seal_block` records the lineage it verified and the Block it sealed; `execute_merge` records its merge
/// Block; `verify` records every Block it replay-verified, from an empty record.
#[test]
fn seal_merge_and_verify_record_the_blocks_they_replay_verified() -> Result<()> {
    use crate::verified_blocks::{load_verified_blocks, record_path};

    let mut sealer = Sealer::new("rfc136-2b-writers", true)?;
    let first = vec![sealer.create("a.txt", 0xA1, b"alpha\n")?];
    sealer.seal(first)?;
    for number in 2..=5_u8 {
        let op = sealer.create(&format!("f{number}.txt"), number, b"filler\n")?;
        sealer.seal(vec![op])?;
    }
    let recorded = load_verified_blocks(&sealer.layout);
    for block_id in &sealer.blocks {
        assert!(recorded.contains(block_id), "seal recorded {block_id}");
    }

    let baseline = *sealer
        .blocks
        .get(2)
        .ok_or_else(|| integrity("no third block"))?;
    sealer.publish_branch("heads/side", baseline)?;
    let side_op = sealer.create("side.txt", 0xF0, b"side\n")?;
    sealer.seal_on("heads/side", vec![side_op])?;
    let merged = execute_merge(
        &sealer.layout,
        baseline,
        MAIN,
        "heads/side",
        &sealer.maintainer,
    )?;
    assert!(
        load_verified_blocks(&sealer.layout).contains(&merged.block_id),
        "merge recorded its Block"
    );

    std::fs::remove_file(record_path(&sealer.layout))?;
    assert!(
        load_verified_blocks(&sealer.layout).is_empty(),
        "fixture sanity"
    );
    let verification = verify_repository(&sealer.layout)?;
    let verified: Vec<ObjectId> = verification
        .block_state_outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, crate::BlockStateStatus::Verified))
        .map(|outcome| outcome.block_id)
        .collect();
    assert!(
        verified.len() >= sealer.blocks.len(),
        "fixture sanity: verify replayed the history"
    );
    let recorded = load_verified_blocks(&sealer.layout);
    for block_id in &verified {
        assert!(recorded.contains(block_id), "verify recorded {block_id}");
    }
    Ok(())
}

// ---- RFC 136 increment 2b §3: anchored worktree writes ------------------------------------------------

/// Every file under the worktree with its bytes and executable bit, excluding `.prikk`.
fn worktree_with_modes(root: &Path) -> Result<Vec<(String, Vec<u8>, bool)>> {
    let mut out = Vec::new();
    for (path, bytes) in worktree_listing(root)? {
        #[cfg(unix)]
        let executable = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(root.join(&path))?.permissions().mode() & 0o111 != 0
        };
        #[cfg(not(unix))]
        let executable = false;
        out.push((path, bytes, executable));
    }
    Ok(out)
}

/// Run `--patch-materialize`, `--patch-materialize-delete` (with the pre-checkpoint deletion's stale file
/// back in the worktree) and `branch switch` on `sealer`, returning every report and worktree.
fn worktree_write_outputs(sealer: &Sealer) -> Result<Vec<String>> {
    let layout = &sealer.layout;
    let mut outputs = Vec::new();
    let (plain, plain_fallback) = crate::materialize_patch_checkout_reporting_anchor(layout, MAIN)?;
    outputs.push(format!("{plain:?} {plain_fallback:?}"));
    std::fs::write(layout.root().join("old.txt"), b"old\n")?;
    let (deleting, deleting_fallback) =
        crate::materialize_patch_checkout_with_deletions_reporting_anchor(layout, MAIN)?;
    outputs.push(format!("{deleting:?} {deleting_fallback:?}"));
    outputs.push(format!("{:?}", worktree_with_modes(layout.root())?));
    let switched = switch_branch(layout, Some(MAIN), "heads/other")?;
    outputs.push(format!("{switched:?}"));
    outputs.push(format!("{:?}", worktree_with_modes(layout.root())?));
    Ok(outputs)
}

/// §10.3c ruling 2: on a locally sealed history every checkpoint is replay-verified, so the worktree
/// writes anchor, and their reports and worktrees equal the same writes replayed from genesis.
#[test]
fn verified_anchors_leave_every_worktree_write_byte_identical() -> Result<()> {
    use crate::patch_replay::anchor::{snapshot_anchor_loads_for_test, without_anchoring_for_test};

    let mut anchored = Sealer::new("rfc136-2b-verified-anchored", true)?;
    let mut genesis = Sealer::new("rfc136-2b-verified-genesis", true)?;
    for sealer in [&mut anchored, &mut genesis] {
        seal_anchor_history(sealer)?;
        let other = *sealer
            .blocks
            .get(100)
            .ok_or_else(|| integrity("no block 101"))?;
        sealer.publish_branch("heads/other", other)?;
    }
    assert_eq!(
        anchored.blocks, genesis.blocks,
        "fixture sanity: identical histories"
    );

    let before = snapshot_anchor_loads_for_test();
    let anchored_outputs = worktree_write_outputs(&anchored)?;
    assert!(
        snapshot_anchor_loads_for_test() > before,
        "fixture sanity: the anchored writes loaded a verified snapshot"
    );
    let genesis_outputs = without_anchoring_for_test(|| worktree_write_outputs(&genesis))?;
    assert_eq!(anchored_outputs.len(), genesis_outputs.len());
    for (anchored_output, genesis_output) in anchored_outputs.iter().zip(&genesis_outputs) {
        assert_eq!(
            anchored_output, genesis_output,
            "a verified-anchor worktree write differs"
        );
    }
    Ok(())
}

/// §3, record failure is safe: a flipped byte, a truncation, or another recorded version gives a full
/// replay (no snapshot loaded), the same output as a verified anchor gives, and no error.
#[test]
fn a_damaged_record_gives_full_replay_and_the_same_output() -> Result<()> {
    use crate::patch_replay::anchor::snapshot_anchor_loads_for_test;
    use crate::verified_blocks::{load_verified_blocks, record_path};

    let mut reference = Sealer::new("rfc136-2b-record-reference", true)?;
    seal_rename_free_history(&mut reference, 70)?;
    let (reference_report, _) =
        crate::materialize_patch_checkout_reporting_anchor(&reference.layout, MAIN)?;
    let reference_tree = worktree_with_modes(reference.layout.root())?;

    for damage in ["flipped byte", "truncated", "other version"] {
        let mut sealer = Sealer::new(
            &format!("rfc136-2b-record-{}", damage.replace(' ', "-")),
            true,
        )?;
        seal_rename_free_history(&mut sealer, 70)?;
        let path = record_path(&sealer.layout);
        let mut bytes = std::fs::read(&path)?;
        match damage {
            "flipped byte" => {
                if let Some(byte) = bytes.last_mut() {
                    *byte ^= 0x01;
                }
            }
            "truncated" => bytes.truncate(bytes.len() / 2),
            _ => {
                // The version string sits after the magic, the checksum, the schema and its length.
                let at = b"PRIKK-REPLAY-VERIFIED-BLOCKS-v1\0".len() + 32 + 4 + 2;
                if let Some(byte) = bytes.get_mut(at) {
                    *byte = byte.wrapping_add(1);
                }
                let body_at = b"PRIKK-REPLAY-VERIFIED-BLOCKS-v1\0".len() + 32;
                let checksum = prikk_hash::sha256(bytes.get(body_at..).unwrap_or_default());
                if let Some(slot) = bytes.get_mut(body_at - 32..body_at) {
                    slot.copy_from_slice(&checksum);
                }
            }
        }
        std::fs::write(&path, &bytes)?;
        assert!(
            load_verified_blocks(&sealer.layout).is_empty(),
            "{damage}: reads empty"
        );
        let before = snapshot_anchor_loads_for_test();
        let (report, fallback) =
            crate::materialize_patch_checkout_reporting_anchor(&sealer.layout, MAIN)?;
        assert_eq!(
            snapshot_anchor_loads_for_test(),
            before,
            "{damage}: no snapshot loaded"
        );
        assert_eq!(
            fallback, None,
            "{damage}: no fallback line, because nothing anchored"
        );
        assert_eq!(
            format!("{report:?}"),
            format!("{reference_report:?}"),
            "{damage}: same report"
        );
        assert_eq!(
            worktree_with_modes(sealer.layout.root())?,
            reference_tree,
            "{damage}: same tree"
        );
    }
    Ok(())
}

/// A rename-free history of `blocks` blocks: `a.txt` edited in every block.
fn seal_rename_free_history(sealer: &mut Sealer, blocks: usize) -> Result<()> {
    let first = vec![sealer.create("a.txt", 0xA1, b"alpha 0\n")?];
    sealer.seal(first)?;
    while sealer.blocks.len() < blocks {
        let number = sealer.blocks.len() + 1;
        let op = sealer.edit(0xA1, format!("alpha {number}\n").as_bytes())?;
        sealer.seal(vec![op])?;
    }
    Ok(())
}

/// **RFC 159 whole-state identity, on a history with every shape** (handoff §2 control 1, and the merge site of control
/// 10): 130 blocks (checkpoints at 1, 65, 129) that create, edit (text and binary), rename (block 30), change a mode
/// (block 40), delete a file whose tombstone must survive (block 20), **restore it exactly** (block 70), and hold **a
/// merge block below the second checkpoint (block 26) and one above it (block 100)**. Every block is sealed again on
/// its own parent by the anchored derivation and compared with an independent forward replay from genesis, and every
/// 10th with the literal full walk (`rfc159_identity_probe`). The two `merge` operations that built the history must each
/// have used an anchor at the merge site.
///
/// **Perturb (identity):** skip `seed_tombstone` in `anchored_parent_state`: the restoration at block 70 differs. Skip
/// seeding the snapshot's mode or `seed_live_node`: block 41 onward differs. **Perturb (site):** `StateAnchoring::Never`
/// at the merge call in `merge/execute.rs`: `uses["merge"]` is 0 and only this control is red.
#[test]
fn rfc159_identity_on_a_history_with_renames_restorations_and_merges() -> Result<()> {
    use crate::{
        anchor_uses_for_test_support, reset_anchor_uses_for_test_support, rfc159_identity_probe,
    };

    reset_anchor_uses_for_test_support();
    let mut sealer = Sealer::new("rfc159-shapes", true)?;
    let (binary, mut binary_id) = create_binary(&mut sealer, "bin.dat", 0xB1, b"\0binary 0")?;
    let first = vec![
        sealer.create("a.txt", 0xA1, b"alpha 0\n")?,
        sealer.create("keep.txt", 0xA2, b"keep\n")?,
        sealer.create("old.txt", 0xA3, b"old\n")?,
        binary,
    ];
    sealer.seal(first)?;
    let mut filler: u8 = 0;
    let mut side = 0_u8;
    while sealer.blocks.len() < 130 {
        let number = sealer.blocks.len() + 1;
        let op = match number {
            20 => sealer.delete("old.txt", 0xA3)?,
            30 => Sealer::rename(0xA2, "keep.txt", "renamed.txt"),
            40 => sealer.chmod(0xA1, 0o100_755),
            70 => sealer.create("old.txt", 0xA3, b"old\n")?,
            50 | 120 => {
                let bytes = format!("\0binary {number}");
                let (op, id) = replace_binary(&mut sealer, 0xB1, binary_id, bytes.as_bytes())?;
                binary_id = id;
                op
            }
            26 | 100 => {
                // A merge block: a side branch off the current tip gets one commit, and is merged into main.
                let baseline = *sealer.blocks.last().ok_or_else(|| integrity("no tip"))?;
                side += 1;
                let branch = format!("heads/side{side}");
                sealer.publish_branch(&branch, baseline)?;
                let side_op = sealer.create(&format!("side{side}.txt"), 0xE0 + side, b"side\n")?;
                sealer.seal_on(&branch, vec![side_op])?;
                let merged =
                    execute_merge(&sealer.layout, baseline, MAIN, &branch, &sealer.maintainer)?;
                sealer.blocks.push(merged.block_id);
                continue;
            }
            number if number % 5 == 0 => {
                sealer.edit(0xA1, format!("alpha {number}\n").as_bytes())?
            }
            number => {
                filler += 1;
                sealer.create(&format!("f{number}.txt"), filler, b"filler\n")?
            }
        };
        sealer.seal(vec![op])?;
    }
    assert_eq!(sealer.checkpoint_positions()?, vec![1, 65, 129]);
    assert_eq!(
        anchor_uses_for_test_support("merge"),
        2,
        "each merge continued from an anchor at the merge site"
    );
    let kinds: Vec<_> = sealer
        .blocks
        .iter()
        .map(|id| sealer.block(*id).map(|block| block.kind))
        .collect::<Result<_>>()?;
    assert_eq!(
        kinds
            .iter()
            .filter(|kind| **kind == prikk_object::BlockKind::Merge)
            .count(),
        2,
        "fixture sanity: two merge blocks on the mainline"
    );

    let tip = sealer
        .blocks
        .last()
        .ok_or_else(|| integrity("no tip"))?
        .to_hex();
    let report = rfc159_identity_probe(&sealer.layout, &tip, 10)?;
    assert!(report.differences.is_empty(), "{:#?}", report.differences);
    assert_eq!(report.blocks, sealer.blocks.len());
    assert_eq!(
        report.fell_back, 1,
        "only the first block has nothing to anchor at"
    );
    assert!(report.literal_full_compared >= 13);
    Ok(())
}
