//! RFC 144 §4m — `bundle preview` controls.
//!
//! Two independent repositories throughout: `source` is where a bundle gets exported from,
//! `local` is the repository `preview_bundle` is run against. `fork_repo` copies one onto the
//! other's own disk state at a chosen point, so the two can share genesis and then diverge
//! independently -- exactly the shape connectivity/conflict controls need.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use crate::bundle::{
    BundleImportOptions, BundlePreviewConflict, BundlePreviewConnectivity, BundlePreviewEffectKind,
    export_bundle, preview_bundle,
};
use crate::test_gates::test_support::{
    dummy_signature, signed_block, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir, write_blob,
};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

fn copy_dir_recursive(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let dest = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir_recursive(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}

/// Copy `source`'s `.prikk` onto a fresh directory, so the fork starts byte-identical and can then
/// diverge independently.
fn fork_repo(source: &Path, tag: &str) -> (RepositoryLayout, PathBuf) {
    let root = unique_temp_dir(tag);
    std::fs::create_dir_all(root.join(".prikk")).unwrap();
    copy_dir_recursive(&source.join(".prikk"), &root.join(".prikk"));
    (RepositoryLayout::init(root.clone()).unwrap(), root)
}

/// Full on-disk state of `root` -- every regular file's path and bytes, sorted -- for asserting
/// byte-identity before/after a call that must write nothing.
fn directory_digest(root: &Path) -> Vec<(String, Vec<u8>)> {
    fn walk(dir: &Path, root: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap())
            .collect();
        entries.sort_by_key(|e| e.path());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .to_string();
                out.push((relative, std::fs::read(&path).unwrap()));
            }
        }
    }
    let mut files = Vec::new();
    walk(root, root, &mut files);
    files.sort();
    files
}

/// Seal one more block onto `layout`, at `heads/main`, containing a single `CreateFile` at `path`
/// for a freshly seeded node. `parent` is the prior tip block (`None` for the genesis Root block);
/// `previous_ref_state_id`/`seq` thread the ref publication chain exactly as `rename_history.rs`'s
/// own established two-block pattern does. Returns the new tip block id and its own RefState id.
#[allow(clippy::too_many_arguments)]
fn seal_create_file_block(
    layout: &RepositoryLayout,
    parent: Option<ObjectId>,
    path: &str,
    node_seed: u8,
    content: &[u8],
    seq: u64,
    previous_ref_state_id: Option<ObjectId>,
) -> prikk_error::Result<(ObjectId, ObjectId)> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let blob_id = write_blob(&mut object_store, content)?;
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_seed; 32]),
                blob_id,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        prikk_object::ObjectType::Patch,
        1,
        payload.to_canonical_bytes()?,
    );
    patch.add_signature(dummy_signature())?;
    let patch_id = object_store.write_object(&patch)?;

    let kind = if parent.is_none() {
        BlockKind::Root
    } else {
        BlockKind::Normal
    };
    let block = signed_block(kind, parent.into_iter().collect(), vec![patch_id], None);
    let block_id = object_store.write_object(&block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", previous_ref_state_id, block_id, seq);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(
        "heads/main",
        previous_ref_state_id,
        ref_state_id,
        block_id,
        seq,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state,
        ref_update,
    })?;
    Ok((block_id, ref_state_id))
}

/// One-block genesis repo: `heads/main` at a Root block creating `a.txt`. Returns the layout, its
/// temp-dir root, the tip block id, and the RefState id (needed to seal a *second* block on top).
fn genesis_repo(tag: &str) -> (RepositoryLayout, PathBuf, ObjectId, ObjectId) {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let (block_id, ref_state_id) =
        seal_create_file_block(&layout, None, "a.txt", 0x01, b"shared\n", 1, None).unwrap();
    (layout, root, block_id, ref_state_id)
}

/// A genesis repo whose Root block genuinely differs from `genesis_repo`'s own (distinct path,
/// node, and content, so it is not merely a content-addressed duplicate). Needed by
/// `does_not_connect_is_reported_not_errored`: two calls to `genesis_repo` with different `tag`s
/// but identical block content are content-addressed to the *same* object id, so they would (quite
/// correctly) report as sharing ancestry -- the two repositories need to actually be unrelated.
fn unrelated_genesis_repo(tag: &str) -> (RepositoryLayout, PathBuf) {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    seal_create_file_block(
        &layout,
        None,
        "unrelated.txt",
        0xaa,
        b"no shared history\n",
        1,
        None,
    )
    .unwrap();
    (layout, root)
}

// ---- Control (RFC 144 §4m.2): preview writes nothing, and the control can fail. ----

#[test]
fn bundle_preview_writes_nothing() {
    let (source_layout, source_root, tip, _) = genesis_repo("rfc144-preview-write-nothing-source");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let (local_layout, local_root) = fork_repo(&source_root, "rfc144-preview-write-nothing-local");
    let _ = local_layout; // exists only to prove `preview_bundle` needs no separate construction step

    let before = directory_digest(&local_root);
    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(
        &RepositoryLayout::init(local_root.clone()).unwrap(),
        &bytes,
        &options,
        "heads/main",
    )
    .unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::AlreadyIncluded,
        "the local fork already has the exact history it was forked from"
    );
    let after = directory_digest(&local_root);
    assert_eq!(
        before, after,
        "bundle preview must write nothing to the repository, but the on-disk state changed"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
    let _ = tip;
}

/// The perturbation half of the control above: prove it can actually fail. Writes one object into
/// the local repository from inside what should be a read-only preview call, and confirms the
/// digest comparison catches it -- the same "a control that cannot fail is not a control"
/// discipline this project applies everywhere else.
#[test]
fn bundle_preview_write_nothing_control_can_fail() {
    let (_source_layout, source_root, _, _) =
        genesis_repo("rfc144-preview-write-nothing-perturb-source");
    let (local_layout, local_root) =
        fork_repo(&source_root, "rfc144-preview-write-nothing-perturb-local");

    let before = directory_digest(&local_root);
    // Perturbation: something writes into the local repository during what the real control
    // treats as a read-only call. Simulated directly (not by editing `preview_bundle` itself,
    // which the write-nothing control above already proves does not do this) to prove the
    // digest-comparison mechanism itself is sensitive to a real write, not just a passing shape.
    let mut object_store = FileObjectStore::new(local_layout.clone());
    write_blob(
        &mut object_store,
        b"a write the real preview must never perform\n",
    )
    .unwrap();

    let after = directory_digest(&local_root);
    assert_ne!(
        before, after,
        "the perturbation itself must be observable, or this control proves nothing"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

// ---- Connectivity + effects. ----

#[test]
fn does_not_connect_is_reported_not_errored() {
    let (source_layout, source_root, _, _) = genesis_repo("rfc144-preview-disconnect-source");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    // An entirely unrelated repository: its own independent genesis, no shared ancestry.
    let (local_layout, local_root) = unrelated_genesis_repo("rfc144-preview-disconnect-local");

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::DoesNotConnect
    );
    assert!(report.conflict.is_none(), "{:?}", report.conflict);
    assert!(report.effects.is_empty(), "{:?}", report.effects);

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

#[test]
fn fast_forward_reports_the_new_file_as_created() {
    let (source_layout, source_root, tip, ref_state_id) =
        genesis_repo("rfc144-preview-fast-forward-source");
    // Local forks from the same genesis, then the source moves ahead by one more block.
    let (local_layout, local_root) = fork_repo(&source_root, "rfc144-preview-fast-forward-local");
    seal_create_file_block(
        &source_layout,
        Some(tip),
        "b.txt",
        0x02,
        b"new in source\n",
        2,
        Some(ref_state_id),
    )
    .unwrap();
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(report.connectivity, BundlePreviewConnectivity::FastForward);
    assert_eq!(report.conflict, Some(BundlePreviewConflict::AppliesCleanly));
    assert_eq!(report.effects.len(), 1, "{:?}", report.effects);
    assert_eq!(report.effects[0].path, "b.txt");
    assert_eq!(report.effects[0].kind, BundlePreviewEffectKind::Created);
    assert_eq!(report.effects[0].current_bytes, None);
    assert_eq!(
        report.effects[0].after_bytes,
        Some("new in source\n".len() as u64)
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

#[test]
fn diverged_with_disjoint_paths_applies_cleanly() {
    let (source_layout, source_root, tip, ref_state_id) =
        genesis_repo("rfc144-preview-diverge-clean-source");
    let (local_layout, local_root) = fork_repo(&source_root, "rfc144-preview-diverge-clean-local");
    // Source and local both move one block past the shared genesis, touching different paths.
    seal_create_file_block(
        &source_layout,
        Some(tip),
        "from-source.txt",
        0x02,
        b"source content\n",
        2,
        Some(ref_state_id),
    )
    .unwrap();
    seal_create_file_block(
        &local_layout,
        Some(tip),
        "from-local.txt",
        0x03,
        b"local content\n",
        2,
        Some(ref_state_id),
    )
    .unwrap();
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(report.connectivity, BundlePreviewConnectivity::Diverged);
    assert_eq!(report.conflict, Some(BundlePreviewConflict::AppliesCleanly));
    assert_eq!(report.effects.len(), 1, "{:?}", report.effects);
    assert_eq!(report.effects[0].path, "from-source.txt");
    assert_eq!(report.effects[0].kind, BundlePreviewEffectKind::Created);

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

#[test]
fn diverged_with_the_same_path_reports_a_conflict_not_a_wrong_answer() {
    let (source_layout, source_root, tip, ref_state_id) =
        genesis_repo("rfc144-preview-diverge-conflict-source");
    let (local_layout, local_root) =
        fork_repo(&source_root, "rfc144-preview-diverge-conflict-local");
    // Both sides independently create a file at the *same* path -- a genuine collision once the
    // bundle's own operations are replayed onto local's current state.
    seal_create_file_block(
        &source_layout,
        Some(tip),
        "collides.txt",
        0x02,
        b"source version\n",
        2,
        Some(ref_state_id),
    )
    .unwrap();
    seal_create_file_block(
        &local_layout,
        Some(tip),
        "collides.txt",
        0x04,
        b"local version\n",
        2,
        Some(ref_state_id),
    )
    .unwrap();
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(report.connectivity, BundlePreviewConnectivity::Diverged);
    match &report.conflict {
        Some(BundlePreviewConflict::Conflict { detail }) => {
            assert!(
                detail.contains("collides.txt")
                    || detail.to_lowercase().contains("occupied")
                    || detail.to_lowercase().contains("overwrite"),
                "conflict detail should name the collision: {detail}"
            );
        }
        other => panic!("expected a reported conflict, got {other:?}"),
    }
    assert!(
        report.effects.is_empty(),
        "a replay that failed partway has no well-defined after-state to diff: {:?}",
        report.effects
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

#[test]
fn already_included_reports_no_effects() {
    let (source_layout, source_root, tip, ref_state_id) =
        genesis_repo("rfc144-preview-already-included-source");
    // Export the bundle from the *older* tip, then advance the local repo further -- the bundle's
    // own target is now an ancestor of local's current state.
    let (local_layout, local_root) =
        fork_repo(&source_root, "rfc144-preview-already-included-local");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();
    seal_create_file_block(
        &local_layout,
        Some(tip),
        "moved-on.txt",
        0x05,
        b"local moved on\n",
        2,
        Some(ref_state_id),
    )
    .unwrap();

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::AlreadyIncluded
    );
    assert!(report.conflict.is_none(), "{:?}", report.conflict);
    assert!(report.effects.is_empty(), "{:?}", report.effects);

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

#[test]
fn sealed_by_reports_the_maintainer_key_id() {
    let (source_layout, source_root, _, _) = genesis_repo("rfc144-preview-sealed-by-source");
    let (local_layout, local_root) = fork_repo(&source_root, "rfc144-preview-sealed-by-local");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert!(
        !report.sealed_by.is_empty(),
        "the bundle's own RefState is maintainer-signed; sealed_by must report it"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}
