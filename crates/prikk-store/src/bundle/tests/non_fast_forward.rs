//! 0.56.0 step 3a (design review v2 rulings A2/A3): `import_bundle` classifies a delivery against
//! the received pointer it would replace, and refuses `rollback`, `diverged` and `does-not-connect`
//! by default, unless `--allow-non-fast-forward` (`BundleImportOptions::allow_non_fast_forward`) is
//! given. Reuses 2A's own two-block fixture shape (a block, then a child of that block) rather than
//! building a new one from scratch.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, Operation,
    OperationKind, PatchPayload, PatchPurpose,
};

use crate::bundle::{
    BundleConnectivityClass, BundleImportOptions, RefStateChainRelation,
    clear_forced_connectivity_class_for_test, during_import_writes_for_test, export_bundle,
    force_connectivity_class_to_for_test, import_bundle_from_bytes,
};
use crate::foundation::layout::LockableContainer;
use crate::lock::acquire_container_locks;
use crate::received::read_received_pointer;
use crate::test_gates::test_support::{
    dummy_signature, repository_bytes, signed_block, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir, write_blob,
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

/// Copy `source`'s `.prikk` onto a fresh directory -- the same shape
/// `bundle/tests/preview.rs::fork_repo` uses, for a history that starts byte-identical and then
/// diverges independently.
fn fork_repo(source: &Path, tag: &str) -> (RepositoryLayout, PathBuf) {
    let root = unique_temp_dir(tag);
    std::fs::create_dir_all(root.join(".prikk")).unwrap();
    copy_dir_recursive(&source.join(".prikk"), &root.join(".prikk"));
    (RepositoryLayout::init(root.clone()).unwrap(), root)
}

/// Seal one more block onto `layout` at `heads/main`, containing a single `CreateFile` at `path`
/// for a freshly seeded node -- `bundle/tests/preview.rs`'s own fixture shape, duplicated here
/// (rather than made `pub(crate)` there) since it is this file's own narrow need, not a second
/// public contract. Returns the new tip block id and its own RefState id.
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

    republish(layout, block_id, seq, previous_ref_state_id)
        .map(|ref_state_id| (block_id, ref_state_id))
}

/// Publish `heads/main` at `target_block_id` -- used both by `seal_create_file_block` (a genuinely
/// new block) and directly by the `already-at-tip`/`older` fixture below (the *same* block,
/// republished at a later `update_seq` to produce a second, distinct RefState with no block-content
/// change at all).
fn republish(
    layout: &RepositoryLayout,
    target_block_id: ObjectId,
    seq: u64,
    previous_ref_state_id: Option<ObjectId>,
) -> prikk_error::Result<ObjectId> {
    let ref_store = RefStore::new(layout.clone());
    let ref_state =
        signed_ref_state_envelope("heads/main", previous_ref_state_id, target_block_id, seq);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope(
        "heads/main",
        previous_ref_state_id,
        ref_state_id,
        target_block_id,
        seq,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state,
        ref_update,
    })?;
    Ok(ref_state_id)
}

/// One-block genesis repo: `heads/main` at a Root block creating `a.txt`.
fn genesis_repo(tag: &str) -> (RepositoryLayout, PathBuf, ObjectId, ObjectId) {
    let root = unique_temp_dir(tag);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let (block_id, ref_state_id) =
        seal_create_file_block(&layout, None, "a.txt", 0x01, b"block1\n", 1, None).unwrap();
    (layout, root, block_id, ref_state_id)
}

/// A genuinely unrelated genesis repo -- distinct path/node/content, so it is not merely a
/// content-addressed duplicate of `genesis_repo`'s own Root block.
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

fn cleanup(roots: &[PathBuf]) {
    for root in roots {
        let _ = std::fs::remove_dir_all(root);
    }
}

// ---- NoLocalHistory: the first import into a repository with no received pointer yet. ----

#[test]
fn first_import_classifies_no_local_history_and_writes() -> prikk_error::Result<()> {
    let (source, source_root, _block1, _) = genesis_repo("nff-first-source");
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("nff-first-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let report = import_bundle_from_bytes(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(report.class, BundleConnectivityClass::NoLocalHistory);
    assert_eq!(report.ref_state_chain, None);
    assert!(report.pointer_updated);
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_some());

    cleanup(&[source_root, target_root]);
    Ok(())
}

// ---- FastForward: never refuses, regardless of the flag. ----

#[test]
fn fast_forward_against_a_pointer_does_not_refuse() -> prikk_error::Result<()> {
    let (source, source_root, block1, ref_state1) = genesis_repo("nff-ff-source");
    let (_, bytes1) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("nff-ff-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    import_bundle_from_bytes(&target, &bytes1, &BundleImportOptions::default_limits())?;

    seal_create_file_block(
        &source,
        Some(block1),
        "block1b.txt",
        0x02,
        b"block1b\n",
        2,
        Some(ref_state1),
    )?;
    let (_, bytes1b) = export_bundle(&source, "heads/main")?;

    let report =
        import_bundle_from_bytes(&target, &bytes1b, &BundleImportOptions::default_limits())?;
    assert_eq!(report.class, BundleConnectivityClass::FastForward);
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Extends));
    assert!(report.pointer_updated);

    cleanup(&[source_root, target_root]);
    Ok(())
}

// ---- Rollback: refuses by default, accepted with the flag, and the refusal writes nothing. ----

/// Builds: `target` already holds `block1b` (a child of `block1`) as its received pointer; the
/// delivery under test is the *older* bundle naming `block1` alone -- a real rollback.
fn rollback_fixture(
    tag: &str,
) -> prikk_error::Result<(RepositoryLayout, PathBuf, PathBuf, Vec<u8>)> {
    let (source, source_root, block1, ref_state1) = genesis_repo(&format!("{tag}-source"));
    let (_, bytes1) = export_bundle(&source, "heads/main")?;

    seal_create_file_block(
        &source,
        Some(block1),
        "block1b.txt",
        0x02,
        b"block1b\n",
        2,
        Some(ref_state1),
    )?;
    let (_, bytes1b) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir(&format!("{tag}-target"));
    let target = RepositoryLayout::init(target_root.clone())?;
    import_bundle_from_bytes(&target, &bytes1b, &BundleImportOptions::default_limits())?;

    Ok((target, source_root, target_root, bytes1))
}

#[test]
fn rollback_against_a_pointer_refuses_and_writes_nothing() -> prikk_error::Result<()> {
    let (target, source_root, target_root, bytes1) = rollback_fixture("nff-rollback-refuse")?;

    let before = repository_bytes(&target)?;
    let result = import_bundle_from_bytes(&target, &bytes1, &BundleImportOptions::default_limits());
    assert!(
        matches!(result, Err(prikk_error::PrikkError::Precondition(ref message)) if message.contains("rollback")),
        "a rollback must refuse by name: {result:?}"
    );
    let after = repository_bytes(&target)?;
    assert_eq!(
        before, after,
        "a refused import must leave every file under .prikk byte-identical"
    );

    cleanup(&[source_root, target_root]);
    Ok(())
}

#[test]
fn rollback_with_the_flag_succeeds() -> prikk_error::Result<()> {
    let (target, source_root, target_root, bytes1) = rollback_fixture("nff-rollback-allow")?;

    let report = import_bundle_from_bytes(
        &target,
        &bytes1,
        &BundleImportOptions::default_limits().with_allow_non_fast_forward(true),
    )?;
    assert_eq!(report.class, BundleConnectivityClass::Rollback);
    assert!(report.pointer_updated);

    cleanup(&[source_root, target_root]);
    Ok(())
}

// ---- Diverged: refuses by default, accepted with the flag, and the refusal writes nothing. ----

fn diverged_fixture(
    tag: &str,
) -> prikk_error::Result<(RepositoryLayout, PathBuf, PathBuf, PathBuf, Vec<u8>)> {
    let (repo_a, root_a, block1, ref_state1) = genesis_repo(&format!("{tag}-a"));
    let (repo_b, root_b) = fork_repo(&root_a, &format!("{tag}-b"));

    seal_create_file_block(
        &repo_a,
        Some(block1),
        "from-a.txt",
        0x02,
        b"from a\n",
        2,
        Some(ref_state1),
    )?;
    let (_, bytes_a) = export_bundle(&repo_a, "heads/main")?;

    seal_create_file_block(
        &repo_b,
        Some(block1),
        "from-b.txt",
        0x03,
        b"from b\n",
        2,
        Some(ref_state1),
    )?;
    let (_, bytes_b) = export_bundle(&repo_b, "heads/main")?;

    let target_root = unique_temp_dir(&format!("{tag}-target"));
    let target = RepositoryLayout::init(target_root.clone())?;
    import_bundle_from_bytes(&target, &bytes_a, &BundleImportOptions::default_limits())?;

    Ok((target, root_a, root_b, target_root, bytes_b))
}

#[test]
fn diverged_against_a_pointer_refuses_and_writes_nothing() -> prikk_error::Result<()> {
    let (target, root_a, root_b, target_root, bytes_b) = diverged_fixture("nff-diverged-refuse")?;

    let before = repository_bytes(&target)?;
    let result =
        import_bundle_from_bytes(&target, &bytes_b, &BundleImportOptions::default_limits());
    assert!(
        matches!(result, Err(prikk_error::PrikkError::Precondition(ref message)) if message.contains("diverged")),
        "a diverged delivery must refuse by name: {result:?}"
    );
    let after = repository_bytes(&target)?;
    assert_eq!(
        before, after,
        "a refused import must leave every file under .prikk byte-identical"
    );

    cleanup(&[root_a, root_b, target_root]);
    Ok(())
}

#[test]
fn diverged_with_the_flag_succeeds() -> prikk_error::Result<()> {
    let (target, root_a, root_b, target_root, bytes_b) = diverged_fixture("nff-diverged-allow")?;

    let report = import_bundle_from_bytes(
        &target,
        &bytes_b,
        &BundleImportOptions::default_limits().with_allow_non_fast_forward(true),
    )?;
    assert_eq!(report.class, BundleConnectivityClass::Diverged);
    assert!(report.pointer_updated);

    cleanup(&[root_a, root_b, target_root]);
    Ok(())
}

// ---- DoesNotConnect: refuses by default, accepted with the flag, and the refusal writes nothing. ----

fn does_not_connect_fixture(
    tag: &str,
) -> prikk_error::Result<(RepositoryLayout, PathBuf, PathBuf, PathBuf, Vec<u8>)> {
    let (source, source_root, _, _) = genesis_repo(&format!("{tag}-source"));
    let (_, bytes_source) = export_bundle(&source, "heads/main")?;

    let (unrelated, unrelated_root) = unrelated_genesis_repo(&format!("{tag}-unrelated"));
    let (_, bytes_unrelated) = export_bundle(&unrelated, "heads/main")?;

    let target_root = unique_temp_dir(&format!("{tag}-target"));
    let target = RepositoryLayout::init(target_root.clone())?;
    import_bundle_from_bytes(
        &target,
        &bytes_source,
        &BundleImportOptions::default_limits(),
    )?;

    Ok((
        target,
        source_root,
        unrelated_root,
        target_root,
        bytes_unrelated,
    ))
}

#[test]
fn does_not_connect_against_a_pointer_refuses_and_writes_nothing() -> prikk_error::Result<()> {
    let (target, source_root, unrelated_root, target_root, bytes_unrelated) =
        does_not_connect_fixture("nff-disconnect-refuse")?;

    let before = repository_bytes(&target)?;
    let result = import_bundle_from_bytes(
        &target,
        &bytes_unrelated,
        &BundleImportOptions::default_limits(),
    );
    assert!(
        matches!(result, Err(prikk_error::PrikkError::Precondition(ref message)) if message.contains("does-not-connect")),
        "an unrelated delivery must refuse by name: {result:?}"
    );
    let after = repository_bytes(&target)?;
    assert_eq!(
        before, after,
        "a refused import must leave every file under .prikk byte-identical"
    );

    cleanup(&[source_root, unrelated_root, target_root]);
    Ok(())
}

#[test]
fn does_not_connect_with_the_flag_succeeds() -> prikk_error::Result<()> {
    let (target, source_root, unrelated_root, target_root, bytes_unrelated) =
        does_not_connect_fixture("nff-disconnect-allow")?;

    let report = import_bundle_from_bytes(
        &target,
        &bytes_unrelated,
        &BundleImportOptions::default_limits().with_allow_non_fast_forward(true),
    )?;
    assert_eq!(report.class, BundleConnectivityClass::DoesNotConnect);
    assert!(report.pointer_updated);

    cleanup(&[source_root, unrelated_root, target_root]);
    Ok(())
}

// ---- AlreadyAtTip: never refuses, either chain relation. ----

#[test]
fn already_at_tip_exact_reimport_extends_and_still_writes() -> prikk_error::Result<()> {
    let (source, source_root, _, _) = genesis_repo("nff-already-tip-source");
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("nff-already-tip-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    import_bundle_from_bytes(&target, &bytes, &BundleImportOptions::default_limits())?;

    // The identical bytes, imported a second time: same RefState id, same target block.
    let report = import_bundle_from_bytes(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(report.class, BundleConnectivityClass::AlreadyAtTip);
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Extends));
    assert!(report.pointer_updated);

    cleanup(&[source_root, target_root]);
    Ok(())
}

/// A2's own named case: `already-at-tip` with `ref_state_chain: older` writes every object but
/// leaves the pointer itself untouched, and exits `0` -- never a refusal. Built by republishing
/// `heads/main` at the *same* block (no content change at all): the later RefState (`newer`) is
/// delivered first, then the earlier one (`older`) is delivered second, so the second delivery's
/// own chain is strictly behind the pointer it would replace.
#[test]
fn already_at_tip_with_older_chain_writes_every_object_but_not_the_pointer()
-> prikk_error::Result<()> {
    let (source, source_root, block1, older_ref_state) =
        genesis_repo("nff-already-tip-older-source");
    let (_, older_bytes) = export_bundle(&source, "heads/main")?;

    // Republish at the *same* block1 -- a new, strictly later RefState naming identical content.
    let newer_ref_state = republish(&source, block1, 2, Some(older_ref_state))?;
    assert_ne!(newer_ref_state, older_ref_state);
    let (_, newer_bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("nff-already-tip-older-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    // Deliver the *newer* RefState first.
    import_bundle_from_bytes(
        &target,
        &newer_bytes,
        &BundleImportOptions::default_limits(),
    )?;
    let pointer_before = read_received_pointer(&target, "remotes/heads/main")?
        .expect("a pointer must exist after the first import");
    assert_eq!(pointer_before.ref_state_id, newer_ref_state);

    // Deliver the *older* RefState second: same block, strictly earlier chain position.
    let report = import_bundle_from_bytes(
        &target,
        &older_bytes,
        &BundleImportOptions::default_limits(),
    )?;
    assert_eq!(report.class, BundleConnectivityClass::AlreadyAtTip);
    assert_eq!(report.ref_state_chain, Some(RefStateChainRelation::Older));
    assert!(
        !report.pointer_updated,
        "an already-at-tip delivery older in its own RefState chain must not move the pointer"
    );

    let pointer_after = read_received_pointer(&target, "remotes/heads/main")?
        .expect("the pointer must still exist");
    assert_eq!(
        pointer_after.ref_state_id, newer_ref_state,
        "the pointer must still name the newer RefState, byte-for-byte unchanged"
    );

    cleanup(&[source_root, target_root]);
    Ok(())
}

// ---- Red control 1: the classifier forced to fast-forward defeats every refusal above. ----

#[test]
fn the_classifier_forced_to_fast_forward_defeats_every_refusal_test() -> prikk_error::Result<()> {
    let (target, source_root, target_root, bytes1) = rollback_fixture("nff-forced-ff-rollback")?;
    let (diverged_target, diverged_root_a, diverged_root_b, diverged_target_root, bytes_b) =
        diverged_fixture("nff-forced-ff-diverged")?;
    let (
        disconnect_target,
        disconnect_source_root,
        disconnect_unrelated_root,
        disconnect_target_root,
        bytes_unrelated,
    ) = does_not_connect_fixture("nff-forced-ff-disconnect")?;

    // Perturbation: force every classification decision this thread makes to FastForward,
    // regardless of what `classify_connectivity` actually computed -- proving the refusal tests
    // above are genuinely driven by the classifier, not merely matching by coincidence.
    force_connectivity_class_to_for_test(BundleConnectivityClass::FastForward);

    let rollback_result =
        import_bundle_from_bytes(&target, &bytes1, &BundleImportOptions::default_limits());
    assert!(
        rollback_result.is_ok(),
        "forced to fast-forward, the rollback delivery must no longer refuse: {rollback_result:?}"
    );

    let diverged_result = import_bundle_from_bytes(
        &diverged_target,
        &bytes_b,
        &BundleImportOptions::default_limits(),
    );
    assert!(
        diverged_result.is_ok(),
        "forced to fast-forward, the diverged delivery must no longer refuse: {diverged_result:?}"
    );

    let disconnect_result = import_bundle_from_bytes(
        &disconnect_target,
        &bytes_unrelated,
        &BundleImportOptions::default_limits(),
    );
    assert!(
        disconnect_result.is_ok(),
        "forced to fast-forward, the does-not-connect delivery must no longer refuse: \
         {disconnect_result:?}"
    );

    // Restore: the same three deliveries, against fresh targets, refuse again once the
    // perturbation is cleared -- proving the control is real (it can fail) and not a tautology.
    clear_forced_connectivity_class_for_test();

    let (target2, source_root2, target_root2, bytes1_again) =
        rollback_fixture("nff-forced-ff-rollback-restored")?;
    let restored_rollback = import_bundle_from_bytes(
        &target2,
        &bytes1_again,
        &BundleImportOptions::default_limits(),
    );
    assert!(
        restored_rollback.is_err(),
        "with the perturbation cleared, the rollback delivery must refuse again: \
         {restored_rollback:?}"
    );

    cleanup(&[
        source_root,
        target_root,
        diverged_root_a,
        diverged_root_b,
        diverged_target_root,
        disconnect_source_root,
        disconnect_unrelated_root,
        disconnect_target_root,
        source_root2,
        target_root2,
    ]);
    Ok(())
}

// ---- Red control 2: the lock genuinely held between the pointer read and the pointer write. ----

/// The handoff's own instruction: "the read moved before the lock: show that a test fails, or say
/// why none can, and add one that does." `import_bundle` holds `ActiveLock` and the
/// `ReceivedIndex`/`ObjectStore` container locks continuously from before pass 1 through the final
/// pointer write -- there is no literal "before the lock" position left to move
/// `read_received_pointer`'s own call to without first breaking that continuous hold, which would
/// be a larger, structurally different change (see the report for the full reasoning). The
/// meaningful control available instead: a second, simulated writer attempts to acquire the
/// `ReceivedIndex` container lock between this import's own read (long since completed, before
/// pass 2) and its own write (`write_received_pointer`, right after this seam fires) -- and is
/// refused, proving the lock the read and the write both run under genuinely excludes a concurrent
/// writer for the whole window between them, not only at the two endpoints.
#[test]
fn the_received_index_lock_excludes_a_concurrent_writer_between_the_read_and_the_write()
-> prikk_error::Result<()> {
    let (source, source_root, _, _) = genesis_repo("nff-lock-control-source");
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("nff-lock-control-target");
    let target = RepositoryLayout::init(target_root.clone())?;

    let concurrent = target.clone();
    let observed = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = std::rc::Rc::clone(&observed);
    during_import_writes_for_test(move || {
        seen.set(matches!(
            acquire_container_locks(&concurrent, &[LockableContainer::ReceivedIndex]),
            Err(prikk_error::PrikkError::LockConflict(_))
        ));
    });

    import_bundle_from_bytes(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert!(
        observed.get(),
        "a concurrent writer attempting the ReceivedIndex lock between the read and the write \
         must be refused"
    );

    cleanup(&[source_root, target_root]);
    Ok(())
}
