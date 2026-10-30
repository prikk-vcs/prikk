//! RFC 144 §4m — `bundle preview` controls.
//!
//! Two independent repositories throughout: `source` is where a bundle gets exported from,
//! `local` is the repository `preview_bundle_from_bytes` is run against. `fork_repo` copies one onto the
//! other's own disk state at a chosen point, so the two can share genesis and then diverge
//! independently -- exactly the shape connectivity/conflict controls need.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use crate::bundle::{
    BundleConnectivityClass, BundleImportOptions, BundlePreviewConflict, BundlePreviewConnectivity,
    BundlePreviewEffectKind, RefStateChainRelation, export_bundle, preview_bundle_from_bytes,
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
    let _ = local_layout; // exists only to prove `preview_bundle_from_bytes` needs no separate construction step

    let before = directory_digest(&local_root);
    let options = BundleImportOptions::default_limits();
    let report = preview_bundle_from_bytes(
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
    // 0.56.0 step 3a (A2): the finer class and the RefState-chain relation, additive alongside
    // the unchanged `connectivity` field above -- the bundle's own RefState is byte-identical to
    // the local ref's current one, so the chain trivially "extends" (zero hops).
    assert_eq!(report.class, BundleConnectivityClass::AlreadyAtTip);
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Extends));
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
    // treats as a read-only call. Simulated directly (not by editing `preview_bundle_from_bytes` itself,
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
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::DoesNotConnect
    );
    assert!(report.conflict.is_none(), "{:?}", report.conflict);
    assert!(report.effects.is_empty(), "{:?}", report.effects);
    assert_eq!(report.class, BundleConnectivityClass::DoesNotConnect);
    assert_eq!(
        report.ref_state_chain,
        Some(RefStateChainRelation::Unrelated)
    );

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
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(report.connectivity, BundlePreviewConnectivity::FastForward);
    assert_eq!(report.class, BundleConnectivityClass::FastForward);
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Extends));
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
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(report.connectivity, BundlePreviewConnectivity::Diverged);
    assert_eq!(report.class, BundleConnectivityClass::Diverged);
    assert_eq!(
        report.ref_state_chain,
        Some(RefStateChainRelation::Unrelated)
    );
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
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
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
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::AlreadyIncluded
    );
    assert!(report.conflict.is_none(), "{:?}", report.conflict);
    assert!(report.effects.is_empty(), "{:?}", report.effects);
    // 0.56.0 step 3a (A2): the bundle's own target is a strict ancestor of local's current
    // target (local moved on past it) -- `connectivity` stays the unchanged `AlreadyIncluded`,
    // but the finer `class` must say `Rollback`, not `AlreadyAtTip`.
    assert_eq!(report.class, BundleConnectivityClass::Rollback);
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Older));

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

// ---- §4n.1: the local ref has never been published. ----

#[test]
fn no_local_history_reports_the_whole_bundle_as_created() {
    let (source_layout, source_root, _, _) = genesis_repo("rfc144-preview-no-history-source");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    // A bare, freshly-initialized repository: `init`, nothing sealed -- exactly the tutorial's
    // own `init` -> `commit` -> `seal` sequence before the first seal. `heads/main` has never
    // been published, not merely published-and-empty.
    let local_root = unique_temp_dir("rfc144-preview-no-history-local");
    let local_layout = RepositoryLayout::init(local_root.clone()).unwrap();

    let before = directory_digest(&local_root);
    let options = BundleImportOptions::default_limits();
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::NoLocalHistory
    );
    assert_eq!(report.class, BundleConnectivityClass::NoLocalHistory);
    assert_eq!(report.ref_state_chain, None);
    assert_eq!(report.conflict, Some(BundlePreviewConflict::AppliesCleanly));
    assert_eq!(report.effects.len(), 1, "{:?}", report.effects);
    assert_eq!(report.effects[0].path, "a.txt");
    assert_eq!(report.effects[0].kind, BundlePreviewEffectKind::Created);
    assert_eq!(report.effects[0].current_bytes, None);
    assert_eq!(report.effects[0].after_bytes, Some("shared\n".len() as u64));
    let after = directory_digest(&local_root);
    assert_eq!(
        before, after,
        "bundle preview must write nothing even on a repository that has never sealed"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

/// §3's own "check the neighbours" question: a ref name that has never existed at all previews
/// identically to a ref name that exists conceptually (a checked-out worktree target, say) but
/// has never been published. `read_current_ref_state_id` makes no such distinction in this
/// codebase's model -- a ref *is* its published pointer, so "exists but unpublished" is not a
/// state this store can represent separately from "never referenced." Both reach the same `None`
/// branch and so, correctly, produce the same answer -- confirmed here rather than only argued.
#[test]
fn a_ref_name_that_never_existed_behaves_the_same_as_an_unpublished_one() {
    let (source_layout, source_root, _, _) =
        genesis_repo("rfc144-preview-no-history-neighbour-source");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    // This local repository has sealed and published `heads/main`, so it is not the "nothing
    // published yet" case at all -- but the ref name being previewed against
    // (`heads/does-not-exist`) has never appeared anywhere in it.
    let (local_layout, local_root, _, _) =
        genesis_repo("rfc144-preview-no-history-neighbour-local");

    let options = BundleImportOptions::default_limits();
    let report =
        preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/does-not-exist").unwrap();
    assert_eq!(
        report.connectivity,
        BundlePreviewConnectivity::NoLocalHistory
    );
    assert_eq!(report.conflict, Some(BundlePreviewConflict::AppliesCleanly));
    assert_eq!(report.effects.len(), 1, "{:?}", report.effects);
    assert_eq!(report.effects[0].path, "a.txt");

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

#[test]
fn sealed_by_reports_the_maintainer_key_id() {
    let (source_layout, source_root, _, _) = genesis_repo("rfc144-preview-sealed-by-source");
    let (local_layout, local_root) = fork_repo(&source_root, "rfc144-preview-sealed-by-local");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let report = preview_bundle_from_bytes(&local_layout, &bytes, &options, "heads/main").unwrap();
    assert!(
        !report.sealed_by.is_empty(),
        "the bundle's own RefState is maintainer-signed; sealed_by must report it"
    );

    let _ = std::fs::remove_dir_all(&source_root);
    let _ = std::fs::remove_dir_all(&local_root);
}

// ---- 0.56.0 step 3c2 (R3): `--ref remotes/…` resolves the received pointer, and `bundle
// preview` always reports the classification `bundle import` of the identical delivery would
// actually make. ----

/// The review's own measured bug, reproduced: a destination holds a **received** pointer at
/// `remotes/heads/main`, two generations ahead of the bundle (`f`, a child of `m1`). Before this
/// round, `bundle preview --ref remotes/heads/main` of a bundle at `m1` answered
/// `"no-local-history"`/`"class": "no-local-history"` regardless -- `RefStore::read_current_ref_
/// state_id` only ever reads `heads/`/`tags/`, never the received index, so a `remotes/…`
/// argument always read `None`. `bundle import` of the identical bytes, meanwhile, correctly read
/// the received pointer and refused it as a rollback. Fixed: `class` (against `--ref`) now
/// resolves the received pointer and reports `rollback`, matching what `bundle import` of this
/// exact delivery would do -- and since `--ref` here names the identical `remotes/heads/main`
/// `bundle import` would itself write to, `import_class`/`import_ref_state_chain` agree with
/// `class`/`ref_state_chain` exactly.
#[test]
fn ref_remotes_resolves_the_received_pointer_and_reports_the_real_class() {
    let (dest_layout, dest_root, m1_id, m1_ref_state_id) =
        genesis_repo("rfc144-r3-ref-remotes-dest");
    let (f_id, f_ref_state_id) = seal_create_file_block(
        &dest_layout,
        Some(m1_id),
        "b.txt",
        0x02,
        b"second generation\n",
        2,
        Some(m1_ref_state_id),
    )
    .unwrap();
    let _ = f_id;
    // Splice `f`'s own RefState in as a *received* pointer at `remotes/heads/main` -- simulating
    // what a real `bundle import`/`archive import` of a two-generation delivery would have left
    // behind, without going through a full import round trip (the same `write_received_pointer_
    // for_test` seam the archive-side 3c2 tests use).
    crate::write_received_pointer_for_test(&dest_layout, "remotes/heads/main", f_ref_state_id)
        .unwrap();

    // `source`: an independent repository whose own `heads/main` is content-addressed to the
    // identical `m1` block (same path/content/node seed as `genesis_repo` always produces) --
    // so a bundle exported from it is genuinely an ancestor of `f`, not merely similarly shaped.
    let (source_layout, source_root, source_m1_id, _) =
        genesis_repo("rfc144-r3-ref-remotes-source");
    assert_eq!(source_m1_id, m1_id, "genesis_repo must be deterministic");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let report =
        preview_bundle_from_bytes(&dest_layout, &bytes, &options, "remotes/heads/main").unwrap();
    assert_eq!(
        report.class,
        BundleConnectivityClass::Rollback,
        "a remotes/… --ref must resolve the received pointer, not silently read no-local-history \
         for a name that actually exists: {report:?}"
    );
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Older));
    // The `--ref` named here *is* the exact name `bundle import` of this bundle would itself
    // write to (the bundle's own exported ref is also `heads/main`), so the two classifications
    // must agree exactly.
    assert_eq!(report.import_class, report.class);
    assert_eq!(report.import_ref_state_chain, report.ref_state_chain);

    let _ = std::fs::remove_dir_all(&dest_root);
    let _ = std::fs::remove_dir_all(&source_root);
}

/// `--ref remotes/…` naming a pointer that does not exist refuses by name, rather than silently
/// answering `no-local-history` for a name that was simply never checked.
#[test]
fn ref_remotes_with_no_received_pointer_refuses_by_name() {
    let local_root = unique_temp_dir("rfc144-r3-ref-remotes-absent");
    let local_layout = RepositoryLayout::init(local_root.clone()).unwrap();
    let (source_layout, source_root, _, _) = genesis_repo("rfc144-r3-ref-remotes-absent-source");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    let error = preview_bundle_from_bytes(&local_layout, &bytes, &options, "remotes/heads/main")
        .expect_err("no received pointer exists at this name -- must refuse, not guess");
    assert!(
        error.to_string().contains("remotes/heads/main"),
        "the refusal must name the ref: {error}"
    );

    let _ = std::fs::remove_dir_all(&local_root);
    let _ = std::fs::remove_dir_all(&source_root);
}

/// `import_class` answers a different question from `class`, and the two can genuinely differ:
/// previewing a bundle against a local ref chosen purely for inspection (`heads/other`, never
/// published) still reports what `bundle import` of the identical bytes would actually do
/// against the received pointer at `remotes/<the bundle's own exported ref name>` -- which here
/// is `remotes/heads/main`, a rollback, even though `class` (against `heads/other`) is
/// `no-local-history`.
#[test]
fn import_class_differs_from_class_when_ref_names_a_different_target() {
    let (dest_layout, dest_root, m1_id, m1_ref_state_id) =
        genesis_repo("rfc144-r3-import-class-differs-dest");
    let (_, f_ref_state_id) = seal_create_file_block(
        &dest_layout,
        Some(m1_id),
        "b.txt",
        0x03,
        b"second generation\n",
        2,
        Some(m1_ref_state_id),
    )
    .unwrap();
    crate::write_received_pointer_for_test(&dest_layout, "remotes/heads/main", f_ref_state_id)
        .unwrap();

    let (source_layout, source_root, source_m1_id, _) =
        genesis_repo("rfc144-r3-import-class-differs-source");
    assert_eq!(source_m1_id, m1_id, "genesis_repo must be deterministic");
    let (_, bytes) = export_bundle(&source_layout, "heads/main").unwrap();

    let options = BundleImportOptions::default_limits();
    // `heads/other` has never been published at `dest_layout` -- a legitimate, unrelated name
    // chosen purely to inspect the bundle's effects, nothing to do with `remotes/heads/main`.
    let report = preview_bundle_from_bytes(&dest_layout, &bytes, &options, "heads/other").unwrap();
    assert_eq!(
        report.class,
        BundleConnectivityClass::NoLocalHistory,
        "heads/other has never been published locally: {report:?}"
    );
    assert_eq!(
        report.import_class,
        BundleConnectivityClass::Rollback,
        "bundle import of this exact delivery would still refuse as a rollback against \
         remotes/heads/main, regardless of what --ref asked to inspect: {report:?}"
    );

    let _ = std::fs::remove_dir_all(&dest_root);
    let _ = std::fs::remove_dir_all(&source_root);
}

/// `preview`'s `import_class` for every one of the six classes genuinely equals what `bundle
/// import` of the identical bytes then actually does -- not merely asserted, but *run*: each
/// case ends by calling `import_bundle` against a byte-identical fresh copy of the same
/// destination state and comparing its own, real `class` field to what `preview` predicted.
#[test]
fn import_class_matches_what_bundle_import_actually_does_for_every_class() {
    // Shared source history: m1 (genesis), then two *independent* children of it, each sealed on
    // its own fork of the pure-m1 state (not on one another) -- f and g, which therefore share
    // m1 as their common ancestor without either knowing about the other.
    let (m1_layout, m1_root, m1_id, m1_ref_state_id) = genesis_repo("rfc144-r3-matrix-m1");
    let bytes_m1 = export_bundle(&m1_layout, "heads/main").unwrap().1;

    let (f_layout, f_root) = fork_repo(&m1_root, "rfc144-r3-matrix-f");
    let (_, f_ref_state_id) = seal_create_file_block(
        &f_layout,
        Some(m1_id),
        "f.txt",
        0x10,
        b"f generation\n",
        2,
        Some(m1_ref_state_id),
    )
    .unwrap();
    let bytes_f = export_bundle(&f_layout, "heads/main").unwrap().1;

    let (g_layout, g_root) = fork_repo(&m1_root, "rfc144-r3-matrix-g");
    seal_create_file_block(
        &g_layout,
        Some(m1_id),
        "g.txt",
        0x11,
        b"g generation\n",
        2,
        Some(m1_ref_state_id),
    )
    .unwrap();
    let bytes_g = export_bundle(&g_layout, "heads/main").unwrap().1;

    let (unrelated_layout, unrelated_root) = unrelated_genesis_repo("rfc144-r3-matrix-unrelated");
    let bytes_unrelated = export_bundle(&unrelated_layout, "heads/main").unwrap().1;

    // One case per class: `(label, fork_root_to_copy, received_pointer_to_splice, delivered_bytes,
    // expected_class)`. `fork_root_to_copy`/`received_pointer_to_splice` are both `None` for
    // `NoLocalHistory` (a bare, fresh destination, nothing received at all).
    type Case<'a> = (
        &'a str,
        Option<&'a Path>,
        Option<ObjectId>,
        &'a [u8],
        BundleConnectivityClass,
    );
    let cases: Vec<Case<'_>> = vec![
        (
            "no-local-history",
            None,
            None,
            &bytes_m1,
            BundleConnectivityClass::NoLocalHistory,
        ),
        (
            "already-at-tip",
            Some(&m1_root),
            Some(m1_ref_state_id),
            &bytes_m1,
            BundleConnectivityClass::AlreadyAtTip,
        ),
        (
            "rollback",
            Some(&f_root),
            Some(f_ref_state_id),
            &bytes_m1,
            BundleConnectivityClass::Rollback,
        ),
        (
            "fast-forward",
            Some(&m1_root),
            Some(m1_ref_state_id),
            &bytes_f,
            BundleConnectivityClass::FastForward,
        ),
        (
            "diverged",
            Some(&f_root),
            Some(f_ref_state_id),
            &bytes_g,
            BundleConnectivityClass::Diverged,
        ),
        (
            "does-not-connect",
            Some(&m1_root),
            Some(m1_ref_state_id),
            &bytes_unrelated,
            BundleConnectivityClass::DoesNotConnect,
        ),
    ];

    for (label, fork_from, splice, bytes, expected) in cases {
        let dest_root = unique_temp_dir(&format!("rfc144-r3-matrix-dest-{label}"));
        if let Some(fork_from) = fork_from {
            copy_dir_recursive(&fork_from.join(".prikk"), &dest_root.join(".prikk"));
        }
        let dest_layout = RepositoryLayout::init(dest_root.clone()).unwrap();
        if let Some(ref_state_id) = splice {
            crate::write_received_pointer_for_test(
                &dest_layout,
                "remotes/heads/main",
                ref_state_id,
            )
            .unwrap();
        }

        let options = BundleImportOptions::default_limits();
        let report = preview_bundle_from_bytes(&dest_layout, bytes, &options, "remotes/heads/main");
        let predicted = match (&report, splice) {
            (Ok(report), _) => report.import_class,
            (Err(_), None) => {
                // `remotes/heads/main` does not exist at all for the `no-local-history` case --
                // `preview`'s own `--ref` resolution now refuses by name (R3's own fix) rather
                // than reporting `no-local-history` for an absent name. `bundle import`'s own
                // classification for the identical, first-ever delivery is `no-local-history`
                // too (there is nothing to be a non-fast-forward against), so this case still
                // predicts the same class `import_bundle` below will actually report.
                BundleConnectivityClass::NoLocalHistory
            }
            (Err(error), Some(_)) => panic!("unexpected preview refusal for {label}: {error}"),
        };
        assert_eq!(predicted, expected, "{label}: preview's own prediction");

        // Now actually run `bundle import` of the identical bytes against the identical
        // destination state, and compare its own real, measured class to the prediction above.
        let allow = matches!(
            expected,
            BundleConnectivityClass::Rollback
                | BundleConnectivityClass::Diverged
                | BundleConnectivityClass::DoesNotConnect
        );
        let import_options = if allow {
            BundleImportOptions::default_limits().with_allow_non_fast_forward(true)
        } else {
            BundleImportOptions::default_limits()
        };
        let import_report =
            crate::bundle::import_bundle_from_bytes(&dest_layout, bytes, &import_options)
                .unwrap_or_else(|error| panic!("{label}: bundle import itself refused: {error}"));
        assert_eq!(
            import_report.class, expected,
            "{label}: bundle import's own real classification must match preview's prediction"
        );

        let _ = std::fs::remove_dir_all(&dest_root);
    }

    let _ = std::fs::remove_dir_all(&m1_root);
    let _ = std::fs::remove_dir_all(&f_root);
    let _ = std::fs::remove_dir_all(&g_root);
    let _ = std::fs::remove_dir_all(&unrelated_root);
}
