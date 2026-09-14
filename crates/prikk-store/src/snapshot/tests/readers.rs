//! RFC 136 handoff v2, increment 1a controls: what a snapshot means to every reader.
//!
//! Every reader that meets `snapshot_blob_ref`: patch replay (`checkout --patch-*`, `rollback-preview`,
//! `branch switch`), the deletion plan, patch materialization, the inverse plan, bundle preview, and
//! snapshot checkout's plan and materialization.

use prikk_error::PrikkError;
use prikk_object::{
    BlockKind, CanonicalEncode, ChangePerm, CreateFile, DeleteNode, DeleteNodePreimage, NodeId,
    NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose,
};

use crate::patch_replay::replay_supported_patch_chain;
use crate::test_gates::test_support::{
    SnapshotAt, dummy_signature, publish_snapshot_history, signed_block_with_state_root,
    signed_ref_state_envelope, signed_ref_update_envelope, text_entry, unique_temp_dir, write_blob,
    write_snapshot, write_snapshot_content,
};
use crate::{
    BundleImportOptions, FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    SnapshotManifest, StateRootEntry, export_bundle, materialize_patch_checkout,
    materialize_snapshot_checkout, plan_patch_checkout_deletions, prepare_patch_inverse_plan,
    prepare_patch_replay_plan, prepare_rollback_preview, prepare_snapshot_checkout_plan,
    preview_bundle, verify_repository,
};

const MAIN: &str = "heads/main";

/// What a sealed block carries in its snapshot field.
enum Snapshot {
    None,
    /// A v2 manifest of these entries; refused by the fixture unless it recomputes to the block.
    Entries(Vec<StateRootEntry>),
    /// Exactly these Blob bytes, whatever they are.
    Content(Vec<u8>),
}

/// A single-parent history on `heads/main`, sealed block by block with correct state roots.
struct Chain {
    layout: RepositoryLayout,
    store: FileObjectStore,
    tip: Option<ObjectId>,
    ref_state: Option<ObjectId>,
    update_seq: u64,
}

impl Chain {
    fn new(layout: &RepositoryLayout) -> Self {
        Self {
            layout: layout.clone(),
            store: FileObjectStore::new(layout.clone()),
            tip: None,
            ref_state: None,
            update_seq: 0,
        }
    }

    fn blob(&mut self, bytes: &[u8]) -> prikk_error::Result<ObjectId> {
        write_blob(&mut self.store, bytes)
    }

    fn seal(&mut self, operations: Vec<Operation>, snapshot: Snapshot) -> prikk_error::Result<()> {
        let payload = PatchPayload {
            operations,
            intent: None,
            preconditions: Vec::new(),
            purpose: PatchPurpose::Normal,
            message: None,
        };
        let mut patch =
            ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
        patch.add_signature(dummy_signature())?;
        let patch_id = self.store.write_object(&patch)?;
        let state_root = crate::derive_next_state_root(&self.store, self.tip, &[patch_id])?;
        let snapshot_blob_id = match snapshot {
            Snapshot::None => None,
            Snapshot::Entries(entries) => {
                Some(write_snapshot(&mut self.store, entries, state_root)?)
            }
            Snapshot::Content(content) => Some(write_snapshot_content(&mut self.store, content)?),
        };
        let kind = if self.tip.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        };
        let block = signed_block_with_state_root(
            kind,
            self.tip.into_iter().collect(),
            vec![patch_id],
            snapshot_blob_id,
            state_root,
        );
        let block_id = self.store.write_object(&block)?;
        self.update_seq += 1;
        let ref_state = signed_ref_state_envelope(MAIN, self.ref_state, block_id, self.update_seq);
        let ref_state_id = ref_state.object_id();
        let ref_update = signed_ref_update_envelope(
            MAIN,
            self.ref_state,
            ref_state_id,
            block_id,
            self.update_seq,
        );
        RefStore::new(self.layout.clone()).publish(&RefPublication {
            ref_name: MAIN.to_string(),
            expected_previous_ref_state_id: self.ref_state,
            ref_state,
            ref_update,
        })?;
        self.tip = Some(block_id);
        self.ref_state = Some(ref_state_id);
        Ok(())
    }
}

fn operation(op_seq: u32, kind: OperationKind) -> Operation {
    Operation {
        op_seq,
        op_id: None,
        preconditions: Vec::new(),
        kind,
    }
}

fn create(op_seq: u32, path: &str, node_seed: u8, blob_id: ObjectId) -> Operation {
    operation(
        op_seq,
        OperationKind::CreateFile(CreateFile {
            path: path.to_string(),
            node_id: NodeId::from_bytes([node_seed; 32]),
            blob_id,
            mode: 0o100644,
        }),
    )
}

/// Bundle this repository's `heads/main` and preview it into an empty repository: the preview
/// replays the bundle's whole chain, snapshot included.
fn preview_into_empty_repository(layout: &RepositoryLayout) -> prikk_error::Result<Vec<String>> {
    let (_, bytes) = export_bundle(layout, MAIN)?;
    let target_root = unique_temp_dir("snapshot-readers-bundle-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let report = preview_bundle(
        &target,
        &bytes,
        &BundleImportOptions::default_limits(),
        MAIN,
    );
    let _ = std::fs::remove_dir_all(target_root);
    Ok(report?
        .effects
        .into_iter()
        .map(|effect| effect.path)
        .collect())
}

/// Every snapshot reader over `heads/main`, by name. Read-only readers first, materializers last.
fn every_reader(layout: &RepositoryLayout) -> Vec<(&'static str, prikk_error::Result<()>)> {
    vec![
        (
            "patch replay plan",
            prepare_patch_replay_plan(layout, MAIN).map(|_| ()),
        ),
        (
            "patch deletion plan",
            plan_patch_checkout_deletions(layout, MAIN).map(|_| ()),
        ),
        (
            "inverse plan",
            prepare_patch_inverse_plan(layout, MAIN).map(|_| ()),
        ),
        (
            "rollback preview",
            prepare_rollback_preview(layout, MAIN).map(|_| ()),
        ),
        (
            "bundle preview",
            preview_into_empty_repository(layout).map(|_| ()),
        ),
        (
            "snapshot checkout plan",
            prepare_snapshot_checkout_plan(layout, MAIN).map(|_| ()),
        ),
        (
            "patch materialization",
            materialize_patch_checkout(layout, MAIN).map(|_| ()),
        ),
        (
            "snapshot materialization",
            materialize_snapshot_checkout(layout, MAIN).map(|_| ()),
        ),
    ]
}

fn assert_every_reader_refuses_as_integrity(layout: &RepositoryLayout, needle: &str) {
    for (reader, result) in every_reader(layout) {
        match result {
            Err(PrikkError::Integrity(message)) => assert!(
                message.contains(needle),
                "{reader} refused as Integrity, but not naming `{needle}`: {message}"
            ),
            other => panic!("{reader} must refuse as Integrity naming `{needle}`, got {other:?}"),
        }
    }
}

/// The team's §2.2 measurement, kept: a snapshot on the block whose own patch creates `a.txt`. Before
/// §10.1a every replay reader applied that patch on top of the snapshot and refused with `CreateFile
/// would overwrite existing path a.txt`. Each reader must now seed from the snapshot, skip the
/// block's patch, and yield `a.txt` exactly once. A second block creates `b.txt`, so the inverse plan
/// has something to invert (with nothing, it refuses to encode an empty patch -- unrelated to
/// snapshots).
#[test]
fn a_post_state_snapshot_on_the_block_that_creates_its_file_yields_the_file_once()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("snapshot-post-state-creating-block");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut chain = Chain::new(&layout);
    let a = chain.blob(b"a\n")?;
    let b = chain.blob(b"b\n")?;
    chain.seal(
        vec![create(1, "a.txt", 0xA1, a)],
        Snapshot::Entries(vec![text_entry("a.txt", 0xA1, a)?]),
    )?;
    chain.seal(vec![create(1, "b.txt", 0xB2, b)], Snapshot::None)?;

    let plan = prepare_patch_replay_plan(&layout, MAIN)?;
    assert_eq!(plan.paths, vec!["a.txt".to_string(), "b.txt".to_string()]);
    assert_eq!(
        plan.patch_count, 1,
        "the snapshot block's own patch is not replayed"
    );
    assert_eq!(
        plan_patch_checkout_deletions(&layout, MAIN)?.planned_deletions,
        0
    );
    let inverse = prepare_patch_inverse_plan(&layout, MAIN)?;
    assert_eq!(inverse.inverse_operation_count, 1);
    assert_eq!(
        inverse.operations.first().map(|op| op.path.as_str()),
        Some("b.txt")
    );
    assert!(prepare_rollback_preview(&layout, MAIN).is_ok());
    let previewed = preview_into_empty_repository(&layout)?;
    assert_eq!(
        previewed
            .iter()
            .filter(|path| path.as_str() == "a.txt")
            .count(),
        1,
        "bundle preview: {previewed:?}"
    );
    assert_eq!(materialize_patch_checkout(&layout, MAIN)?.written_files, 2);
    assert_eq!(std::fs::read(root.join("a.txt"))?, b"a\n");
    assert_eq!(std::fs::read(root.join("b.txt"))?, b"b\n");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Root creates README.md and old.txt; the second block deletes old.txt and creates extra.txt; the
/// third creates c.txt; the fourth makes README.md executable. `snapshot_on_second` puts the second
/// block's own state in its snapshot.
fn publish_four_block_history(
    layout: &RepositoryLayout,
    snapshot_on_second: bool,
) -> prikk_error::Result<()> {
    let mut chain = Chain::new(layout);
    let readme = chain.blob(b"hello\n")?;
    let old = chain.blob(b"old\n")?;
    let extra = chain.blob(b"extra\n")?;
    let c = chain.blob(b"c\n")?;
    chain.seal(
        vec![
            create(1, "README.md", 0x70, readme),
            create(2, "old.txt", 0x71, old),
        ],
        Snapshot::None,
    )?;
    let second_snapshot = if snapshot_on_second {
        Snapshot::Entries(vec![
            text_entry("README.md", 0x70, readme)?,
            text_entry("extra.txt", 0x72, extra)?,
        ])
    } else {
        Snapshot::None
    };
    chain.seal(
        vec![
            operation(
                1,
                OperationKind::DeleteNode(DeleteNode {
                    path: "old.txt".to_string(),
                    node_id: NodeId::from_bytes([0x71; 32]),
                    old_node_kind: NodeKind::TextFile,
                    preimage: DeleteNodePreimage::File {
                        old_blob_id: old,
                        old_mode: 0o100644,
                    },
                }),
            ),
            create(2, "extra.txt", 0x72, extra),
        ],
        second_snapshot,
    )?;
    chain.seal(vec![create(1, "c.txt", 0x73, c)], Snapshot::None)?;
    chain.seal(
        vec![operation(
            1,
            OperationKind::ChangePerm(ChangePerm {
                node_id: NodeId::from_bytes([0x70; 32]),
                old_mode: 0o100644,
                new_mode: 0o100755,
            }),
        )],
        Snapshot::None,
    )
}

/// Readers with and without an anchor agree: the same history, once with a snapshot on a block that
/// has patches and once without, replays to the same manifest -- paths, bytes, modes and kinds -- and
/// materializes to the same bytes. The patch counts prove the snapshot was used, not ignored.
#[test]
fn replay_from_a_snapshot_agrees_with_replay_without_one() -> prikk_error::Result<()> {
    let with_root = unique_temp_dir("snapshot-agrees-with");
    let without_root = unique_temp_dir("snapshot-agrees-without");
    let with = RepositoryLayout::init(with_root.clone())?;
    let without = RepositoryLayout::init(without_root.clone())?;
    publish_four_block_history(&with, true)?;
    publish_four_block_history(&without, false)?;

    let replay_with = replay_supported_patch_chain(&with, MAIN)?;
    let replay_without = replay_supported_patch_chain(&without, MAIN)?;
    assert_eq!(replay_with.manifest, replay_without.manifest);
    // The count is cumulative across the walk: block 1's patch, not block 2's (its snapshot already
    // holds it), then blocks 3 and 4.
    assert_eq!(
        replay_with.patch_count, 3,
        "the snapshot block's own patch is skipped"
    );
    assert_eq!(replay_without.patch_count, 4);

    materialize_patch_checkout(&with, MAIN)?;
    materialize_patch_checkout(&without, MAIN)?;
    for path in ["README.md", "extra.txt", "c.txt"] {
        assert_eq!(
            std::fs::read(with_root.join(path))?,
            std::fs::read(without_root.join(path))?,
            "{path}"
        );
    }
    assert!(!with_root.join("old.txt").exists());
    let _ = std::fs::remove_dir_all(with_root);
    let _ = std::fs::remove_dir_all(without_root);
    Ok(())
}

/// Every single-byte change to a manifest is refused: either it does not decode (and that is
/// `Integrity`), or it decodes to entries that recompute to a different root, which the loader
/// refuses as `Integrity`.
#[test]
fn every_single_byte_flip_of_a_manifest_is_refused() -> prikk_error::Result<()> {
    let manifest = SnapshotManifest {
        entries: vec![
            text_entry("README.md", 0x70, ObjectId::from_bytes([0x11; 32]))?,
            text_entry("src/main.rs", 0x71, ObjectId::from_bytes([0x22; 32]))?,
        ],
    };
    let bytes = manifest.encode()?;
    let root = manifest.recomputed_state_root()?;
    for index in 0..bytes.len() {
        let mut tampered = bytes.clone();
        if let Some(byte) = tampered.get_mut(index) {
            *byte ^= 0x01;
        }
        match SnapshotManifest::decode(&tampered) {
            Ok(decoded) => assert_ne!(
                decoded.recomputed_state_root()?,
                root,
                "a flip at byte {index} decoded and still recomputed to the original root"
            ),
            Err(PrikkError::Integrity(_)) => {}
            Err(other) => panic!("a flip at byte {index} was refused as {other:?}, not Integrity"),
        }
    }
    Ok(())
}

/// One manifest byte changed (inside `a.txt`'s Blob id) on a real block: every reader refuses as
/// `Integrity`, and the block still verifies by replay -- `verify` does not read the manifest in this
/// increment.
#[test]
fn a_tampered_manifest_is_refused_by_every_reader_while_the_block_still_verifies()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("snapshot-tampered");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut chain = Chain::new(&layout);
    let a = chain.blob(b"a\n")?;
    let mut content = SnapshotManifest {
        entries: vec![text_entry("a.txt", 0xA1, a)?],
    }
    .encode()?;
    if let Some(last) = content.last_mut() {
        *last ^= 0x01;
    }
    chain.seal(
        vec![create(1, "a.txt", 0xA1, a)],
        Snapshot::Content(content),
    )?;

    assert_every_reader_refuses_as_integrity(
        &layout,
        "snapshot manifest does not recompute to its block's state root",
    );
    let verification = verify_repository(&layout)?;
    assert!(
        !verification.has_stage_failure() && !verification.has_item_failure(),
        "the block still verifies by replay: {verification:?}"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A v1-magic Blob in `snapshot_blob_ref`: v1 is retired, and every reader says which magic it found.
#[test]
fn a_v1_manifest_is_refused_naming_its_magic() -> prikk_error::Result<()> {
    let root = unique_temp_dir("snapshot-v1-magic");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut chain = Chain::new(&layout);
    let a = chain.blob(b"a\n")?;
    let mut v1 = b"PRIKK-SNAPSHOT-MANIFEST-v1\n".to_vec();
    v1.extend_from_slice(&5_u32.to_be_bytes());
    v1.extend_from_slice(b"a.txt");
    v1.extend_from_slice(&2_u64.to_be_bytes());
    v1.extend_from_slice(b"a\n");
    chain.seal(vec![create(1, "a.txt", 0xA1, a)], Snapshot::Content(v1))?;

    assert_every_reader_refuses_as_integrity(
        &layout,
        "snapshot manifest magic `PRIKK-SNAPSHOT-MANIFEST-v1` is not PRIKK-SNAPSHOT-MANIFEST-v2",
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// `checkout --snapshot-materialize` on the shared fixture with the snapshot on the tip yields the
/// tip's state -- README.md and extra.txt -- not its parent's, which still had old.txt.
#[test]
fn snapshot_materialization_yields_the_tips_state_not_its_parents() -> prikk_error::Result<()> {
    let root = unique_temp_dir("snapshot-materialize-tip-state");
    let layout = RepositoryLayout::init(root.clone())?;
    publish_snapshot_history(&layout, SnapshotAt::Tip)?;
    let report = materialize_snapshot_checkout(&layout, MAIN)?;
    assert_eq!(
        report.paths,
        vec!["README.md".to_string(), "extra.txt".to_string()]
    );
    assert_eq!(std::fs::read(root.join("README.md"))?, b"hello\n");
    assert_eq!(std::fs::read(root.join("extra.txt"))?, b"extra\n");
    assert!(!root.join("old.txt").exists());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
