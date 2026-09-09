//! Bundle export/import tests.

mod preview;
mod proptest_decode_bundle;

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose, RefKind, RefStatePayload,
    RefUpdatePayload, TagPayload,
};

use crate::author::author_key_index::{
    AuthorKeyEntry, force_conflicting_author_key_entry_for_test, lookup_author_key_entries,
    record_author_key_material, verify_author_signature,
};
use crate::author::author_signing::{AuthorSigner, author_signature};
use crate::bundle::{
    BundleImportOptions, BundleManifest, BundleScope, DEFAULT_BUNDLE_MAX_OBJECT_COUNT,
    decode_bundle, encode_bundle, encode_bundle_v1_for_test, encode_bundle_v2_for_test,
    export_bundle, import_bundle, verify_bundle,
};
use crate::foundation::file_codec::{encode_envelope_file, push_bytes_u64, push_u64};
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::{ContainerSlot, DEFAULT_ACTIVE_NAME, LockableContainer};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::received::read_received_pointer;
use crate::test_gates::test_support::{
    publish_text_create_then_edit_block_v1, rollback_patch_blob_envelope, signed_block,
    signed_patch_blob_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader,
    ObjectWriter, RefPublication, RefStore, RepositoryLayout,
};

/// A `BundleManifest` for tests that re-encode already-decoded `objects`/`ref_name` and only care
/// about something else (author-key tampering, a declared-count boundary) -- `encode_bundle` itself
/// derives the manifest's `declared_ref_name`/`declared_object_count` from its own `ref_name`/
/// `objects` parameters, so this only needs to supply the three fields it does not derive.
fn test_manifest() -> BundleManifest {
    BundleManifest {
        repository_format: 6,
        tool_version: "test".to_string(),
        scope: BundleScope::SingleRef,
    }
}

/// Seal a two-block `heads/main` (a Root block plus a Normal child referencing one Patch, whose
/// `CreateFile` operation itself references one Blob) into `layout`, returning the tip Block id —
/// enough to exercise a genuinely multi-block, genesis-complete export rather than a single trivial
/// object, and to exercise blob discovery through a Patch's own operations, not just a Block's
/// `snapshot_blob_ref`.
fn seal_two_block_history(
    layout: &RepositoryLayout,
) -> prikk_error::Result<prikk_object::ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    object_store.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = object_store.write_object(&root_block)?;

    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = object_store.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(child_block_id)
}

/// `seal_two_block_history`'s own shape, plus a `snapshot_blob_ref` on the Root block -- a distinct
/// blob (`rollback_patch_blob_envelope`'s fixed "rollback fixture\n" content, not the Patch's own
/// "patch fixture\n" one) so the two blobs never collide by content-address. Returns the tip Block id
/// and the snapshot Blob's own id (review condition two,
/// `DC-78-import-closure-validation-review-v1.md` §3: a Block's own `snapshot_blob_ref` is a blob
/// reference too, and needed its own fixture -- `seal_two_block_history`'s blocks never set it).
fn seal_two_block_history_with_snapshot_blob(
    layout: &RepositoryLayout,
) -> prikk_error::Result<(ObjectId, ObjectId)> {
    let mut object_store = FileObjectStore::new(layout.clone());
    object_store.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;

    let snapshot_blob = rollback_patch_blob_envelope();
    let snapshot_blob_id = object_store.write_object(&snapshot_blob)?;

    let root_block = signed_block(
        BlockKind::Root,
        Vec::new(),
        Vec::new(),
        Some(snapshot_blob_id),
    );
    let root_block_id = object_store.write_object(&root_block)?;

    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = object_store.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok((child_block_id, snapshot_blob_id))
}

/// DC-53 Stage 2: a fixed-seed AUTHOR signer, distinct across callers via `discriminant` so tests
/// signing more than one Patch under this helper get distinct `key_id`s and object ids.
fn transport_test_signer(discriminant: u8) -> prikk_error::Result<Ed25519AuthorSigner> {
    Ed25519AuthorSigner::from_seed(
        format!("dc53-stage2-transport-{discriminant}"),
        &[discriminant; 32],
    )
}

/// Seal a two-block `heads/main` whose Patch carries a real AUTHOR signature from `signer`, rather
/// than `seal_two_block_history`'s fixed structural fixture -- DC-53 Stage 2's transport tests need
/// a signature `record_author_key_material`/`verify_author_signature` can actually be asked about.
/// If `record_locally` is true, `signer`'s key material is recorded on `layout` before this returns
/// (real authoring's own order, `node_authoring.rs:589`); if false, the Patch is signed but no local
/// material exists for it yet -- vector 7's starting state.
fn seal_two_block_history_with_author(
    layout: &RepositoryLayout,
    signer: &Ed25519AuthorSigner,
    record_locally: bool,
) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let blob = signed_patch_blob_envelope();
    object_store.write_object(&blob)?;

    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "transport.txt".to_string(),
                node_id: NodeId::from_bytes([0x54; 32]),
                blob_id: blob.object_id(),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    let patch_object_id = patch.object_id();
    let signature = author_signature(signer, patch_object_id)?;
    patch.add_signature(signature)?;
    let patch_id = object_store.write_object(&patch)?;

    if record_locally {
        let active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
        record_author_key_material(
            layout,
            signer.key_id(),
            signer.public_key_bytes(),
            &active_lock,
        )?;
    }

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = object_store.write_object(&root_block)?;
    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = object_store.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(child_block_id)
}

/// Find the imported Patch in `target`'s object store by scanning for the one carrying
/// `transport.txt` -- `seal_two_block_history_with_author`'s own fixture path, distinct from
/// `seal_two_block_history`'s `a.txt`.
fn find_imported_transport_patch(target: &RepositoryLayout) -> prikk_error::Result<ObjectEnvelope> {
    // The Patch's object id is derived from its canonical payload, which the sender and receiver
    // both compute identically -- recompute it here rather than threading it through `import_bundle`'s
    // own report, which deliberately reports only counts (DC-53 Stage 2, D6/D7's own "no new
    // verification path" -- this helper is test-only introspection, not part of the transport
    // contract).
    let target_store = FileObjectStore::new(target.clone());
    let ref_state_id = read_received_pointer(target, "remotes/heads/main")?
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("received ref state missing".to_string())
        })?
        .ref_state_id;
    let ref_state_envelope = target_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("received RefState missing".to_string())
        })?;
    let ref_state_payload = prikk_object::RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )?;
    let block_envelope = target_store
        .read_typed(ref_state_payload.target_object_id, ObjectType::Block)?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("received Block missing".to_string()))?;
    let block_payload =
        prikk_object::BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    for patch_id in &block_payload.patch_ids {
        if let Some(envelope) = target_store.read_typed(*patch_id, ObjectType::Patch)? {
            return Ok(envelope);
        }
    }
    Err(prikk_error::PrikkError::Integrity(
        "no Patch found in received Block".to_string(),
    ))
}

#[test]
fn export_of_missing_ref_fails() {
    let root = unique_temp_dir("bundle-export-missing-ref");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(export_bundle(&layout, "heads/main").is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn import_of_malformed_bytes_fails_closed() {
    let root = unique_temp_dir("bundle-import-malformed");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let options = BundleImportOptions::default_limits();
        assert!(import_bundle(&layout, b"not a bundle", &options).is_err());
        assert!(import_bundle(&layout, b"PBNDL001", &options).is_err());
        assert!(import_bundle(&layout, &[], &options).is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn export_then_import_carries_the_full_genesis_complete_closure() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-export-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let child_block_id = seal_two_block_history(&source)?;

    let (report, bytes) = export_bundle(&source, "heads/main")?;
    assert_eq!(report.ref_name, "heads/main");
    assert_eq!(report.tip_block_id, child_block_id);
    // RefState + 2 Blocks + 1 Patch + 1 Blob (the Patch's CreateFile references it) = 5 objects;
    // no Attestation in this fixture.
    assert_eq!(report.object_count, 5);

    let target_root = unique_temp_dir("bundle-import-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_report = import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(import_report.ref_name, "remotes/heads/main");
    assert_eq!(import_report.object_count, 5);
    assert_eq!(import_report.written_object_count, 5);

    let pointer = read_received_pointer(&target, "remotes/heads/main")?;
    assert!(pointer.is_some());
    if let Some(pointer) = pointer {
        assert_eq!(pointer.ref_state_id, import_report.ref_state_id);
    }

    let target_objects = FileObjectStore::new(target.clone());
    assert!(
        target_objects
            .read_typed(child_block_id, ObjectType::Block)?
            .is_some()
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// RFC 134 §8, §3.2: a repository containing genuine v1 (schema 1, `dup_index`-positional
/// identity) `EditText` history must still `bundle export`/`import`/verify cleanly through the
/// post-§8 code -- demonstrated, not assumed. `PATCH_TEXT_SPAN_V2_SCHEMA`'s admission and the new
/// decode-side tag-10/11 schema gate touch the same code paths this exercises end to end.
#[test]
fn export_then_import_carries_genuine_v1_edit_text_history() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-v1-edittext-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    publish_text_create_then_edit_block_v1(&source, b"hello world\n", b"hello prikk\n")?;
    let source_checked = crate::verify_repository(&source)?.checked_objects;
    assert!(
        source_checked.is_some_and(|count| count > 0),
        "source must verify at least one object"
    );

    let (report, bytes) = export_bundle(&source, "heads/main")?;
    assert_eq!(report.ref_name, "heads/main");

    let target_root = unique_temp_dir("bundle-v1-edittext-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_report = import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(import_report.ref_name, "remotes/heads/main");
    assert_eq!(import_report.object_count, report.object_count);
    assert_eq!(import_report.written_object_count, report.object_count);

    assert_eq!(
        crate::verify_repository(&target)?.checked_objects,
        source_checked,
        "target must verify exactly what source verified"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// RFC 102 Stage 6 Step 2, design-v1.md §15.8: `import_bundle` acquires the `ReceivedIndex`
/// container lock -- the gap the received-index concurrency investigation surfaced in the first
/// place (`FINDINGS.md`), closed here. Proven the same way as the ref/trust equivalents: hold the
/// lock externally, observe the refusal, release, observe success. Object writes must not have
/// happened either -- the lock is acquired after them, so a refused import still leaves the target
/// repository's received-index untouched, but confirming the objects themselves aren't silently
/// re-imported on retry is the property this test also checks.
#[test]
fn import_refuses_while_received_index_lock_is_externally_held() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-lock-conflict-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("bundle-lock-conflict-target");
    let target = RepositoryLayout::init(target_root.clone())?;

    let held = acquire_container_locks(&target, &[LockableContainer::ReceivedIndex])?;
    assert!(import_bundle(&target, &bytes, &BundleImportOptions::default_limits()).is_err());
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_none());
    drop(held);

    let report = import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(report.ref_name, "remotes/heads/main");
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_some());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

#[test]
fn import_never_writes_a_local_ref_pointer() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-negctrl-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("bundle-negctrl-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let ref_store = RefStore::new(target.clone());
    let before = ref_store.list_ref_pointers()?;
    assert!(before.is_empty());

    import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;

    let after = RefStore::new(target.clone()).list_ref_pointers()?;
    assert_eq!(
        before, after,
        "bundle import must never advance or create a local heads/*-or-tags/* ref pointer"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

#[test]
fn reimporting_the_same_bundle_is_idempotent() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-reimport-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("bundle-reimport-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let options = BundleImportOptions::default_limits();
    let first = import_bundle(&target, &bytes, &options)?;
    assert_eq!(first.written_object_count, 5);

    let second = import_bundle(&target, &bytes, &options)?;
    assert_eq!(second.written_object_count, 0);
    assert_eq!(second.ref_state_id, first.ref_state_id);

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-86 criterion 3, the negative control: a bundle whose declared object count is exactly at the
/// limit is accepted, and the identical bundle against a limit one below its actual count is refused.
/// A bound nobody has seen fire is a bound nobody knows exists.
#[test]
fn import_object_count_limit_fires_exactly_at_the_boundary() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-limit-count-boundary-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (report, bytes) = export_bundle(&source, "heads/main")?;
    assert_eq!(report.object_count, 5);

    let under_root = unique_temp_dir("bundle-limit-count-boundary-under");
    let under_target = RepositoryLayout::init(under_root.clone())?;
    let refused = import_bundle(
        &under_target,
        &bytes,
        &BundleImportOptions::default_limits().with_max_object_count(4),
    );
    assert!(
        refused.is_err(),
        "a limit one below the actual count (5) must refuse"
    );

    let at_root = unique_temp_dir("bundle-limit-count-boundary-at");
    let at_target = RepositoryLayout::init(at_root.clone())?;
    let accepted = import_bundle(
        &at_target,
        &bytes,
        &BundleImportOptions::default_limits().with_max_object_count(5),
    );
    assert!(
        accepted.is_ok(),
        "a limit exactly at the actual count (5) must accept"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(under_root);
    let _ = std::fs::remove_dir_all(at_root);
    Ok(())
}

/// The same negative control for the total-bytes bound: one byte short of the encoded bundle's own
/// length refuses; the exact length accepts.
#[test]
fn import_total_bytes_limit_fires_exactly_at_the_boundary() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-limit-bytes-boundary-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let under_root = unique_temp_dir("bundle-limit-bytes-boundary-under");
    let under_target = RepositoryLayout::init(under_root.clone())?;
    let refused = import_bundle(
        &under_target,
        &bytes,
        &BundleImportOptions::default_limits().with_max_total_bytes(bytes.len() - 1),
    );
    assert!(
        refused.is_err(),
        "a byte limit one below the bundle's own length must refuse"
    );

    let at_root = unique_temp_dir("bundle-limit-bytes-boundary-at");
    let at_target = RepositoryLayout::init(at_root.clone())?;
    let accepted = import_bundle(
        &at_target,
        &bytes,
        &BundleImportOptions::default_limits().with_max_total_bytes(bytes.len()),
    );
    assert!(
        accepted.is_ok(),
        "a byte limit exactly at the bundle's own length must accept"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(under_root);
    let _ = std::fs::remove_dir_all(at_root);
    Ok(())
}

/// DC-86 criterion 2, measured rather than asserted: a refused over-limit import leaves the target's
/// object store with exactly the same object count as before the attempt — "it returned an error"
/// does not by itself prove nothing was written.
#[test]
fn import_refused_over_the_object_count_limit_writes_nothing() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("bundle-limit-writes-nothing-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("bundle-limit-writes-nothing-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let before = crate::verify_repository(&target)?.checked_objects;

    let refused = import_bundle(
        &target,
        &bytes,
        &BundleImportOptions::default_limits().with_max_object_count(4),
    );
    assert!(refused.is_err());

    let after = crate::verify_repository(&target)?.checked_objects;
    assert_eq!(
        before, after,
        "a refused import must leave the object store's checked-object count unchanged"
    );
    assert_eq!(before, Some(0));

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-53 Stage 2, §7 vector 7: a bundle whose author-key section omits a key for a Patch it
/// contains -- material is optional per-author -- must still import, and the Patch must read
/// Unverifiable, not Sound and not a failure.
#[test]
fn dc53_stage2_vector7_omitted_material_imports_as_unverifiable() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-vector7-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer = transport_test_signer(0xa1)?;
    seal_two_block_history_with_author(&source, &signer, false)?;

    let (report, bytes) = export_bundle(&source, "heads/main")?;
    assert_eq!(
        report.author_key_count, 0,
        "no local material exists for this key_id, so the section must omit it"
    );

    let target_root = unique_temp_dir("dc53-vector7-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_report = import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(import_report.recorded_author_key_count, 0);

    let imported_patch = find_imported_transport_patch(&target)?;
    let outcome = verify_author_signature(&target, &imported_patch)?;
    assert_eq!(
        outcome,
        Some((signer.key_id().to_string(), false)),
        "expected Unverifiable, got {outcome:?}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// Positive control alongside vector 7: material that *is* recorded locally transports and the
/// imported Patch reads Sound, not merely "not a failure" -- `Unverifiable` would also pass that
/// weaker bar.
#[test]
fn dc53_stage2_transported_material_imports_as_sound() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-vector-sound-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer = transport_test_signer(0xa2)?;
    seal_two_block_history_with_author(&source, &signer, true)?;

    let (report, bytes) = export_bundle(&source, "heads/main")?;
    assert_eq!(report.author_key_count, 1);

    let target_root = unique_temp_dir("dc53-vector-sound-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_report = import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(import_report.recorded_author_key_count, 1);

    let imported_patch = find_imported_transport_patch(&target)?;
    let outcome = verify_author_signature(&target, &imported_patch)?;
    assert_eq!(outcome, Some((signer.key_id().to_string(), true)));

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-53 Stage 2, §7 vector 8: a bundle whose author-key section carries a key that does not
/// verify the Patch's signature -- the transport-layer forgery case. Import records the material
/// anyway (D7: import records, `verify` decides, no cryptographic check at the transport layer,
/// matching how objects are written without re-verifying them); `verify_author_signature` on the
/// imported Patch is what fails, reached through the transport path rather than local authoring
/// but the same underlying check as D3's third row.
#[test]
fn dc53_stage2_vector8_a_transported_key_that_does_not_verify_reads_failed()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-vector8-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer = transport_test_signer(0xa3)?;
    seal_two_block_history_with_author(&source, &signer, true)?;

    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, mut author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert_eq!(author_keys.len(), 1);
    if let Some(entry) = author_keys.first_mut() {
        // Swap in an unrelated public key for the same key_id -- the forgery this vector targets.
        entry.public_key = [0xbb; 32];
    }
    let tampered = encode_bundle(&ref_name, &objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc53-vector8-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_report = import_bundle(&target, &tampered, &BundleImportOptions::default_limits())?;
    assert_eq!(
        import_report.recorded_author_key_count, 1,
        "import records material without checking it, D7"
    );

    let imported_patch = find_imported_transport_patch(&target)?;
    let outcome = verify_author_signature(&target, &imported_patch);
    assert!(
        outcome.is_err(),
        "the transported key must not verify the Patch's real signature -- got {outcome:?}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-53 Stage 2, D7/C2: a bundle whose own author-key section carries two different public keys
/// for one `key_id` must be refused before any write -- a hostile or merely stale bundle must not
/// be able to leave a receiver with an unresolvable, permanently unverifiable `key_id`.
#[test]
fn import_rejects_a_bundle_whose_author_key_section_disagrees_with_itself()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-bundle-internal-conflict-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer = transport_test_signer(0xa4)?;
    seal_two_block_history_with_author(&source, &signer, true)?;

    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, mut author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert_eq!(author_keys.len(), 1);
    let key_id = author_keys
        .first()
        .map(|entry| entry.key_id.clone())
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("expected one decoded author-key entry".to_string())
        })?;
    author_keys.push(AuthorKeyEntry {
        key_id: key_id.clone(),
        public_key: [0xcc; 32],
    });
    let hostile = encode_bundle(&ref_name, &objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc53-bundle-internal-conflict-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let before = crate::verify_repository(&target)?.checked_objects;
    let result = import_bundle(&target, &hostile, &BundleImportOptions::default_limits());
    assert!(
        result.is_err(),
        "a bundle whose own author-key section disagrees with itself must be refused"
    );
    let after = crate::verify_repository(&target)?.checked_objects;
    assert_eq!(
        before, after,
        "refused before any write -- the object store must be untouched"
    );
    assert_eq!(before, Some(0));
    // DC-53 Stage 2 implementation review v1, C1: the object store is not the container this
    // check protects -- the hazard is a partial write to the author-key container itself, which
    // has no prune/repair path. Asserted directly, not inferred from the object store happening
    // to move together with it (a coincidence of today's ordering, not a guarantee).
    assert_eq!(
        lookup_author_key_entries(&target, &key_id)?,
        Vec::new(),
        "a refused hostile bundle must leave no author-key entry behind, not even the attacker's \
         first-listed one"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-53 Stage 2, D7: reusing Step 1's `record_author_key_material` at import means a transported
/// key conflicting with material this receiver *already* has locally is refused too -- distinct
/// from the bundle-internal case above, and the receiver's own existing material must survive
/// untouched.
#[test]
fn import_rejects_a_transported_key_conflicting_with_local_material() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-import-local-conflict-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer = transport_test_signer(0xa5)?;
    seal_two_block_history_with_author(&source, &signer, true)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("dc53-import-local-conflict-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let active_lock = ActiveLock::acquire(&target, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(&target, signer.key_id(), [0xdd; 32], &active_lock)?;
    drop(active_lock);

    let result = import_bundle(&target, &bytes, &BundleImportOptions::default_limits());
    assert!(
        result.is_err(),
        "a transported key conflicting with existing local material must be refused"
    );
    assert!(
        read_received_pointer(&target, "remotes/heads/main")?.is_none(),
        "a refused import must not create the received pointer"
    );
    let entries = lookup_author_key_entries(&target, signer.key_id())?;
    assert_eq!(
        entries,
        vec![AuthorKeyEntry {
            key_id: signer.key_id().to_string(),
            public_key: [0xdd; 32],
        }],
        "the transported conflicting key must never be recorded -- {entries:?}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-53 Stage 2 follow-up (`multi-key-import-partial-write-v1.md`): with `m > 1` transported keys,
/// a conflict at a later entry must not leave an earlier, non-conflicting entry durably recorded --
/// the container that leak would land in has no prune, no compaction and no repair. Built by
/// exporting one signer's real bundle, then splicing in a second, synthetic transported key whose
/// `key_id` the target already holds under a *different* public key -- the first entry has nothing
/// wrong with it; the second is the one that must refuse the whole import.
#[test]
fn import_rejects_a_later_conflicting_key_without_recording_an_earlier_one()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-multi-key-import-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer_a = transport_test_signer(0xb1)?;
    seal_two_block_history_with_author(&source, &signer_a, true)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, mut author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert_eq!(
        author_keys.len(),
        1,
        "expected exactly one transported key from a single-author export"
    );

    let signer_b = transport_test_signer(0xb2)?;
    author_keys.push(AuthorKeyEntry {
        key_id: signer_b.key_id().to_string(),
        public_key: signer_b.public_key_bytes(),
    });
    let hostile = encode_bundle(&ref_name, &objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc53-multi-key-import-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let active_lock = ActiveLock::acquire(&target, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(&target, signer_b.key_id(), [0xdd; 32], &active_lock)?;
    drop(active_lock);

    let result = import_bundle(&target, &hostile, &BundleImportOptions::default_limits());
    assert!(
        result.is_err(),
        "a later transported key conflicting with local material must refuse the whole import"
    );
    assert!(
        read_received_pointer(&target, "remotes/heads/main")?.is_none(),
        "a refused import must not create the received pointer"
    );
    assert_eq!(
        lookup_author_key_entries(&target, signer_a.key_id())?,
        Vec::new(),
        "the earlier entry, which conflicted with nothing, must not have been recorded either -- \
         the whole import is refused before any entry is written"
    );
    assert_eq!(
        lookup_author_key_entries(&target, signer_b.key_id())?,
        vec![AuthorKeyEntry {
            key_id: signer_b.key_id().to_string(),
            public_key: [0xdd; 32],
        }],
        "the target's own pre-existing material for the conflicting key_id must survive untouched"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// `multi-key-import-partial-write-v1.md` §5.3: re-importing an unchanged bundle must stay
/// idempotent under the new pre-validation pass too -- a repeat import must hit
/// `check_author_key_conflict`'s `AlreadyRecorded` outcome, not a manufactured conflict, and must
/// not grow the container.
#[test]
fn reimporting_the_same_bundle_records_no_new_author_key_entries() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-multi-key-reimport-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let signer = transport_test_signer(0xb3)?;
    seal_two_block_history_with_author(&source, &signer, true)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    let target_root = unique_temp_dir("dc53-multi-key-reimport-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let options = BundleImportOptions::default_limits();
    import_bundle(&target, &bytes, &options)?;
    let after_first = lookup_author_key_entries(&target, signer.key_id())?;
    assert_eq!(after_first.len(), 1);

    import_bundle(&target, &bytes, &options)?;
    let after_second = lookup_author_key_entries(&target, signer.key_id())?;
    assert_eq!(
        after_second, after_first,
        "re-importing an unchanged bundle must not grow the author-key container"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-53 Stage 2, §1's ratified edge case: exporting a `key_id` whose *local* material already
/// disagrees with itself (only reachable via a legacy pre-Stage-2 state, planted directly here)
/// must fail the export rather than silently pick one of the two conflicting keys -- presenting the
/// receiver with an arbitrarily-chosen key would look like a provenance claim this sender's own
/// repository does not actually make.
#[test]
fn export_fails_when_local_material_already_conflicts_for_an_exported_key_id()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("dc53-export-local-conflict");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = transport_test_signer(0xa6)?;
    seal_two_block_history_with_author(&layout, &signer, true)?;
    force_conflicting_author_key_entry_for_test(&layout, signer.key_id(), [0xee; 32])?;

    let result = export_bundle(&layout, "heads/main");
    assert!(
        result.is_err(),
        "export must refuse rather than silently pick one of two conflicting local keys"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-53 Stage 2, C1 (plan review): the author-key section's own declared count needs the same
/// DC-86 bound the object count already has -- a second declared count in a format a hostile
/// sender fully controls, with no bound, would reopen the hole DC-86 closed.
#[test]
fn author_key_count_limit_fires_exactly_at_the_boundary() -> prikk_error::Result<()> {
    let ref_name = "heads/main".to_string();
    let objects: Vec<ObjectEnvelope> = Vec::new();
    let author_keys = vec![
        AuthorKeyEntry {
            key_id: "a".to_string(),
            public_key: [1; 32],
        },
        AuthorKeyEntry {
            key_id: "b".to_string(),
            public_key: [2; 32],
        },
    ];
    let bytes = encode_bundle(&ref_name, &objects, &author_keys, &test_manifest())?;

    let refused = decode_bundle(&bytes, 1);
    assert!(
        refused.is_err(),
        "a limit one below the actual author-key count (2) must refuse"
    );

    let accepted = decode_bundle(&bytes, 2);
    assert!(
        accepted.is_ok(),
        "a limit exactly at the actual author-key count (2) must accept"
    );

    Ok(())
}

/// DC-53 Stage 2 follow-up (bundle-v1-import-regression-v1.md): the actual migration path
/// `layout.rs`'s retired-format messages promise, walked end to end. A `PBNDL001` bundle -- encoded
/// the way a Stage-1-or-earlier build really produced one, not hand-edited bytes -- must import, its
/// Patch must read `Unverifiable` (never `Sound`: this bundle carries no author-key section at all,
/// regardless of what material the sender happened to have locally), and `verify_repository` must
/// pass and say so.
#[test]
fn a_pbndl001_bundle_imports_and_its_patch_reads_unverifiable() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc53-pbndl001-import-source");
    let source = RepositoryLayout::init(source_root.clone())?;

    // A genuinely, fully sealed history -- real commit, real seal, real adopted maintainer -- not
    // the lightweight `signed_block` structural fixture other tests in this file use, which never
    // computes a real state-merkle-root and so would fail `verify_repository`'s block-state stage
    // for reasons unrelated to this test. `verify passes and says so` (handoff §4) means a genuine
    // pass, not one read past unrelated fixture noise.
    let author = transport_test_signer(0xa8)?;
    let maintainer = Ed25519MaintainerSigner::from_seed("dc53-pbndl001-maintainer", &[0xa9; 32])?;
    crate::trust::add_trusted_maintainer(
        &source,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    std::fs::write(source.root().join("v1-import.txt"), b"v1 import\n")?;
    crate::worktree_patch::commit_worktree_changes_signed(
        &source,
        "heads/main",
        "dc53 pbndl001 fixture",
        crate::worktree_patch::WorktreePatchCommitOptions::default(),
        &author,
    )?;
    // Records `author`'s key material locally on `source` as a side effect (the same production
    // path `node_authoring.rs` uses) -- the sender genuinely has material to carry, so the v1
    // path's own omission below is a real assertion, not an accident of the fixture having
    // nothing to drop.
    crate::rfc111_seal_simulation::simulate_one_seal(&source, "heads/main", &maintainer)?;

    // DC-44 increment 3: renamed from "v2" -- `export_bundle` now always emits `PBNDL003`, not
    // `PBNDL002`; only `v1_bytes` below is genuinely format-specific.
    let (_, current_bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&current_bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert_eq!(
        author_keys.len(),
        1,
        "sanity: the sender really did have material to carry"
    );
    let v1_bytes = encode_bundle_v1_for_test(&ref_name, &objects)?;

    let target_root = unique_temp_dir("dc53-pbndl001-import-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_report = import_bundle(&target, &v1_bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(
        import_report.recorded_author_key_count, 0,
        "a PBNDL001 bundle carries no author-key section to record"
    );

    let imported_patch = find_imported_transport_patch(&target)?;
    let outcome = verify_author_signature(&target, &imported_patch)?;
    assert_eq!(
        outcome,
        Some((author.key_id().to_string(), false)),
        "expected Unverifiable, got {outcome:?}"
    );

    let report = crate::verify_repository(&target)?;
    assert!(
        !report.has_item_failure(),
        "verify must pass against a v1-imported repository: {report:?}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

fn author_key_container_bytes(layout: &RepositoryLayout) -> prikk_error::Result<Vec<u8>> {
    let relative = layout.repository_relative(&layout.author_key_container_path())?;
    Ok(crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    .unwrap_or_default())
}

/// Byte-for-byte snapshot of the received-ref index's full on-disk state: both container slots plus
/// the generation log that decides which slot is live (RFC 102 Stage 6 Step 1's compaction shape).
/// An ordinary import only ever appends to the live slot, but comparing all three files, not just
/// one, is what makes "unchanged" a real claim rather than one that only holds for today's write
/// pattern.
fn received_index_bytes(
    layout: &RepositoryLayout,
) -> prikk_error::Result<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let read = |path: std::path::PathBuf| -> prikk_error::Result<Vec<u8>> {
        let relative = layout.repository_relative(&path)?;
        Ok(crate::foundation::fsutil::read_file_if_exists(
            layout.repository_mutation_root(),
            &relative,
        )?
        .unwrap_or_default())
    };
    Ok((
        read(layout.received_index_slot_path(ContainerSlot::A))?,
        read(layout.received_index_slot_path(ContainerSlot::B))?,
        read(layout.received_index_generation_log_path())?,
    ))
}

// DC-78 `import-closure-validation-handoff-v1.md` §5: `import_bundle` validates the bundle's own
// closure -- ref target, patch-referenced blobs, block-named patches, block-named parents -- before
// any write, matching `accept_exchange_artifact`'s own precedent. Each row below has its own test;
// the corresponding negative control (removing the one check that row proves) was run by hand in an
// isolated, discarded worktree per this project's own standing review discipline, not encoded here.

/// §5 row 1: a bundle whose exported ref's target does not resolve -- carried by the bundle nor
/// already local -- is refused. Built exactly as report item 3 asks: a genuine tag-ref bundle, minus
/// the Tag envelope the DC-78 tag-export fix (`d605c10`) added -- the literal shape a pre-fix
/// `export_bundle` build used to emit for a tag ref. Confirms a pre-DC-78-shaped tag bundle can
/// actually be constructed in a test, not merely asserted to exist.
#[test]
fn row1_a_bundle_whose_ref_target_is_absent_is_refused() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row1-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let tip_block_id = seal_two_block_history(&source)?;
    let maintainer =
        Ed25519MaintainerSigner::from_seed("dc78-closure-row1-maintainer", &[0xc1; 32])?;
    let mut object_store = FileObjectStore::new(source.clone());
    publish_tag(
        &source,
        &mut object_store,
        "tags/v1",
        tip_block_id,
        &maintainer,
    )?;

    let (_, bytes) = export_bundle(&source, "tags/v1")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        objects
            .iter()
            .any(|envelope| envelope.object_type == ObjectType::Tag),
        "fixture sanity: a genuine tag bundle must carry the Tag envelope"
    );
    let pre_fix_shaped_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| envelope.object_type != ObjectType::Tag)
        .collect();
    let pre_fix_shaped_bytes = encode_bundle(
        &ref_name,
        &pre_fix_shaped_objects,
        &author_keys,
        &test_manifest(),
    )?;

    let target_root = unique_temp_dir("dc78-closure-row1-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let result = import_bundle(
        &target,
        &pre_fix_shaped_bytes,
        &BundleImportOptions::default_limits(),
    );
    let err = match result {
        Ok(report) => panic!(
            "a bundle whose RefState targets a Tag object it never carried must be refused: \
             {report:?}"
        ),
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("targets missing tag"),
        "unexpected error: {err}"
    );
    assert!(read_received_pointer(&target, "remotes/tags/v1")?.is_none());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// §5 row 2: a bundle missing a blob a carried patch's operations reference is refused.
#[test]
fn row2_a_bundle_missing_a_referenced_blob_is_refused() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row2-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        objects
            .iter()
            .any(|envelope| envelope.object_type == ObjectType::Blob),
        "fixture sanity"
    );
    let broken_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| envelope.object_type != ObjectType::Blob)
        .collect();
    let broken_bytes = encode_bundle(&ref_name, &broken_objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc78-closure-row2-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let result = import_bundle(
        &target,
        &broken_bytes,
        &BundleImportOptions::default_limits(),
    );
    let err = match result {
        Ok(report) => {
            panic!("a bundle missing a patch-referenced blob must be refused: {report:?}")
        }
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("references blob"),
        "unexpected error: {err}"
    );
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_none());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// Review condition two (`DC-78-import-closure-validation-review-v1.md` §3): a Block's own
/// `snapshot_blob_ref` is a blob reference too, on the same terms as a Patch's own operations
/// (row 2's own property, restated for the other blob-naming site). A well-formed bundle carrying
/// the snapshot blob still imports and the blob lands; a bundle missing it is refused.
#[test]
fn row2b_a_bundle_missing_a_blocks_snapshot_blob_is_refused() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row2b-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let (_, snapshot_blob_id) = seal_two_block_history_with_snapshot_blob(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        objects
            .iter()
            .any(|envelope| envelope.object_id() == snapshot_blob_id),
        "fixture sanity: the exported bundle must carry the block's own snapshot blob"
    );

    // Positive half: the well-formed bundle, snapshot blob included, still imports and the blob
    // actually lands.
    let good_target_root = unique_temp_dir("dc78-closure-row2b-good-target");
    let good_target = RepositoryLayout::init(good_target_root.clone())?;
    import_bundle(&good_target, &bytes, &BundleImportOptions::default_limits())?;
    let good_target_objects = FileObjectStore::new(good_target.clone());
    assert!(
        good_target_objects
            .read_typed(snapshot_blob_id, ObjectType::Blob)?
            .is_some(),
        "the snapshot blob must actually land in the receiving repository's store"
    );

    // Negative half: the same bundle, minus only the snapshot blob, is refused.
    let broken_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| envelope.object_id() != snapshot_blob_id)
        .collect();
    let broken_bytes = encode_bundle(&ref_name, &broken_objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc78-closure-row2b-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let result = import_bundle(
        &target,
        &broken_bytes,
        &BundleImportOptions::default_limits(),
    );
    let err = match result {
        Ok(report) => {
            panic!("a bundle missing a block's own snapshot blob must be refused: {report:?}")
        }
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("names snapshot blob"),
        "unexpected error: {err}"
    );
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_none());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(good_target_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// §5 row 3: a bundle missing a block's own named patch is refused.
#[test]
fn row3_a_bundle_missing_a_blocks_patch_is_refused() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row3-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        objects
            .iter()
            .any(|envelope| envelope.object_type == ObjectType::Patch),
        "fixture sanity"
    );
    let broken_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| envelope.object_type != ObjectType::Patch)
        .collect();
    let broken_bytes = encode_bundle(&ref_name, &broken_objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc78-closure-row3-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let result = import_bundle(
        &target,
        &broken_bytes,
        &BundleImportOptions::default_limits(),
    );
    let err = match result {
        Ok(report) => {
            panic!("a bundle missing a block's own named patch must be refused: {report:?}")
        }
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("names patch"),
        "unexpected error: {err}"
    );
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_none());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// §5 row 4: a bundle missing a block's own named parent is refused.
#[test]
fn row4_a_bundle_missing_a_blocks_parent_is_refused() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row4-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let child_block_id = seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let is_root_block = |envelope: &ObjectEnvelope| {
        envelope.object_type == ObjectType::Block && envelope.object_id() != child_block_id
    };
    assert!(
        objects.iter().any(is_root_block),
        "fixture sanity: the root block must be present before removal"
    );
    let broken_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| !is_root_block(envelope))
        .collect();
    let broken_bytes = encode_bundle(&ref_name, &broken_objects, &author_keys, &test_manifest())?;

    let target_root = unique_temp_dir("dc78-closure-row4-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let result = import_bundle(
        &target,
        &broken_bytes,
        &BundleImportOptions::default_limits(),
    );
    let err = match result {
        Ok(report) => {
            panic!("a bundle missing a block's own named parent must be refused: {report:?}")
        }
        Err(err) => err,
    };
    assert!(
        err.to_string().contains("names parent"),
        "unexpected error: {err}"
    );
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_none());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// §5 row 5: objects already held locally satisfy "present" -- the ordinary incremental case, not an
/// edge case (handoff §2/§5's own warning: this is the row most likely to be got wrong). Two
/// independent partial-bundle scenarios, one per surviving call site (review condition one,
/// `DC-78-import-closure-validation-review-v1.md` §2: the first version of this test only pinned the
/// patch and parent sites, not the blob site -- the blob happened to always ride along inside the
/// bundle whenever the Patch that references it did, so the blob check's own local-fallback half was
/// never actually exercised).
///
/// Scenario A hand-truncates a genuine bundle down to only its RefState and tip Block, after
/// pre-seeding the receiver with the Root block, Patch, and Blob directly -- pins the block's own
/// patch-presence and parent-presence checks (item 3, item 4).
///
/// Scenario B carries everything *except* the Blob -- RefState, both Blocks, and the Patch that
/// references it -- after pre-seeding only the Blob directly. This is what actually exercises the
/// blob-presence check's local-fallback half (item 2): that check only ever scans blobs referenced by
/// a Patch's operations, and it only scans a Patch that is itself decoded from the bundle's own
/// bytes -- so the blob site can only be pinned by a fixture where the Patch travels with the bundle
/// but its own Blob does not.
#[test]
fn row5_objects_already_held_locally_satisfy_present() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row5-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let child_block_id = seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;

    // Scenario A: item 3 (block's own patch) and item 4 (block's own parent).
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let target_root = unique_temp_dir("dc78-closure-row5-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let mut target_objects = FileObjectStore::new(target.clone());
    let mut carried_objects: Vec<ObjectEnvelope> = Vec::new();
    for envelope in objects {
        let already_local = match envelope.object_type {
            ObjectType::Block => envelope.object_id() != child_block_id,
            ObjectType::Patch | ObjectType::Blob => true,
            _ => false,
        };
        if already_local {
            target_objects.write_object(&envelope)?;
        } else {
            carried_objects.push(envelope);
        }
    }
    assert_eq!(
        carried_objects.len(),
        2,
        "fixture sanity: only the RefState and tip Block should remain in the partial bundle"
    );
    let partial_bytes = encode_bundle(&ref_name, &carried_objects, &author_keys, &test_manifest())?;

    let report = import_bundle(
        &target,
        &partial_bytes,
        &BundleImportOptions::default_limits(),
    )?;
    assert_eq!(report.object_count, 2);
    assert_eq!(
        report.written_object_count, 2,
        "the pre-seeded objects must not be double-counted as newly written"
    );
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_some());
    assert!(
        target_objects
            .read_typed(child_block_id, ObjectType::Block)?
            .is_some()
    );

    // Scenario B: item 2 (blob referenced by a carried patch's own operations).
    let (ref_name_b, objects_b, author_keys_b, _manifest_b) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let target_b_root = unique_temp_dir("dc78-closure-row5-target-b");
    let target_b = RepositoryLayout::init(target_b_root.clone())?;
    let mut target_b_objects = FileObjectStore::new(target_b.clone());
    let mut carried_objects_b: Vec<ObjectEnvelope> = Vec::new();
    for envelope in objects_b {
        if envelope.object_type == ObjectType::Blob {
            target_b_objects.write_object(&envelope)?;
        } else {
            carried_objects_b.push(envelope);
        }
    }
    assert_eq!(
        carried_objects_b.len(),
        4,
        "fixture sanity: everything except the Blob should remain in this partial bundle"
    );
    assert!(
        carried_objects_b
            .iter()
            .any(|envelope| envelope.object_type == ObjectType::Patch),
        "fixture sanity: the Patch that references the omitted Blob must itself be carried, or \
         the blob check's own loop never runs"
    );
    let partial_bytes_b = encode_bundle(
        &ref_name_b,
        &carried_objects_b,
        &author_keys_b,
        &test_manifest(),
    )?;

    let report_b = import_bundle(
        &target_b,
        &partial_bytes_b,
        &BundleImportOptions::default_limits(),
    )?;
    assert_eq!(report_b.object_count, 4);
    assert_eq!(
        report_b.written_object_count, 4,
        "the pre-seeded Blob must not be double-counted as newly written"
    );
    assert!(read_received_pointer(&target_b, "remotes/heads/main")?.is_some());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    let _ = std::fs::remove_dir_all(target_b_root);
    Ok(())
}

/// §5 row 6: a refused import writes no received pointer and records no key material. Compared
/// byte-for-byte against both the received-ref index (all three on-disk files) and the author-key
/// container, each pre-seeded with a genuine, unrelated entry first -- an empty-to-empty comparison
/// would only prove nothing was ever written to an empty container, not that a refusal leaves
/// existing state untouched (the Stage 3 review's own row 1 model, mirrored here for
/// `import_bundle`'s second receiving path). The pre-seed is a genuine successful import for the
/// *same* ref name, so this also proves a hostile re-import cannot clobber a good existing pointer.
#[test]
fn row6_a_refused_import_writes_no_pointer_and_records_no_key_material() -> prikk_error::Result<()>
{
    let good_source_root = unique_temp_dir("dc78-closure-row6-good-source");
    let good_source = RepositoryLayout::init(good_source_root.clone())?;
    seal_two_block_history(&good_source)?;
    let (_, good_bytes) = export_bundle(&good_source, "heads/main")?;

    let target_root = unique_temp_dir("dc78-closure-row6-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let good_report = import_bundle(&target, &good_bytes, &BundleImportOptions::default_limits())?;

    let unrelated_signer = transport_test_signer(0xc6)?;
    let unrelated_lock = ActiveLock::acquire(&target, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(
        &target,
        unrelated_signer.key_id(),
        unrelated_signer.public_key_bytes(),
        &unrelated_lock,
    )?;
    drop(unrelated_lock);

    let received_before = received_index_bytes(&target)?;
    let author_keys_before = author_key_container_bytes(&target)?;
    assert!(
        !received_before.0.is_empty() || !received_before.1.is_empty(),
        "fixture sanity"
    );
    assert!(!author_keys_before.is_empty(), "fixture sanity");

    // A different, hostile bundle for the SAME ref name -- row 3's shape (missing a block's own
    // named patch) -- carrying its own transportable author-key material, so "records no key
    // material" is a real assertion, not a vacuous one. Row 2's shape (a missing blob) will not do
    // here: `seal_two_block_history_with_author` and the good pre-seed above both build their Blob
    // from the same fixed fixture bytes, so it is content-addressed to the same id and the target
    // already holds it from the good import -- "already present locally" would correctly let it
    // through, proving the wrong thing for this test. The Patch and child Block differ between the
    // two fixtures (distinct paths/node ids), so removing the Patch is genuinely absent everywhere.
    let attack_signer = transport_test_signer(0xc7)?;
    let attack_source_root = unique_temp_dir("dc78-closure-row6-attack-source");
    let attack_source = RepositoryLayout::init(attack_source_root.clone())?;
    seal_two_block_history_with_author(&attack_source, &attack_signer, true)?;
    let (_, attack_bytes) = export_bundle(&attack_source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&attack_bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert_eq!(
        author_keys.len(),
        1,
        "fixture sanity: the attack bundle must carry material"
    );
    let broken_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| envelope.object_type != ObjectType::Patch)
        .collect();
    let broken_bytes = encode_bundle(&ref_name, &broken_objects, &author_keys, &test_manifest())?;

    let result = import_bundle(
        &target,
        &broken_bytes,
        &BundleImportOptions::default_limits(),
    );
    assert!(result.is_err(), "the hostile re-import must be refused");

    let received_after = received_index_bytes(&target)?;
    let author_keys_after = author_key_container_bytes(&target)?;
    assert_eq!(
        received_before, received_after,
        "byte-for-byte: the received-ref index must be untouched by a refused import"
    );
    assert_eq!(
        author_keys_before, author_keys_after,
        "byte-for-byte: the author-key container must be untouched by a refused import"
    );

    let pointer = read_received_pointer(&target, "remotes/heads/main")?;
    assert_eq!(
        pointer.map(|pointer| pointer.ref_state_id),
        Some(good_report.ref_state_id),
        "the genuine earlier import's pointer must survive a refused re-import unchanged"
    );
    assert!(
        lookup_author_key_entries(&target, attack_signer.key_id())?.is_empty(),
        "the attack bundle's own key material must not be recorded"
    );

    let _ = std::fs::remove_dir_all(good_source_root);
    let _ = std::fs::remove_dir_all(attack_source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// §5 row 7 (regression guard): a well-formed bundle still imports, the current format and
/// `PBNDL001` alike. **Renamed from "v2" to "current" (DC-44 increment 3):** `export_bundle` now
/// always emits `PBNDL003`, so `bytes` below is a `PBNDL003` bundle, not `PBNDL002` -- see
/// `a_pbndl002_bundle_imports_with_no_manifest_but_recorded_author_key_material` for the dedicated,
/// real-bytes `PBNDL002` case control 2 asks for.
#[test]
fn row7_a_well_formed_bundle_still_imports_both_formats() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-closure-row7-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, _author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let v1_bytes = encode_bundle_v1_for_test(&ref_name, &objects)?;

    let current_target_root = unique_temp_dir("dc78-closure-row7-current-target");
    let current_target = RepositoryLayout::init(current_target_root.clone())?;
    import_bundle(
        &current_target,
        &bytes,
        &BundleImportOptions::default_limits(),
    )?;
    assert!(read_received_pointer(&current_target, "remotes/heads/main")?.is_some());

    let v1_target_root = unique_temp_dir("dc78-closure-row7-v1-target");
    let v1_target = RepositoryLayout::init(v1_target_root.clone())?;
    import_bundle(
        &v1_target,
        &v1_bytes,
        &BundleImportOptions::default_limits(),
    )?;
    assert!(read_received_pointer(&v1_target, "remotes/heads/main")?.is_some());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(current_target_root);
    let _ = std::fs::remove_dir_all(v1_target_root);
    Ok(())
}

/// DC-44 increment 3, control 2: `PBNDL002` (an author-key section, but no manifest) still
/// imports, with real bytes -- the same real export's own objects/author-keys, re-encoded in the
/// `PBNDL002` shape via `encode_bundle_v2_for_test`, not a hand-built approximation. Uses
/// `seal_two_block_history_with_author` (not the plain structural fixture) so the author-key
/// section actually carries something, the same reason the existing `PBNDL001` test does.
#[test]
fn a_pbndl002_bundle_imports_with_no_manifest_but_recorded_author_key_material()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-manifest-pbndl002-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    // A genuinely, fully sealed history, not the lightweight `signed_block` structural fixture
    // (`seal_two_block_history_with_author`) -- see
    // `verify_of_a_real_export_succeeds_and_matches_the_export_report`'s own doc comment for why
    // this test's own `verify_repository` call below needs one that can actually pass it.
    let author = transport_test_signer(0xc1)?;
    let maintainer =
        Ed25519MaintainerSigner::from_seed("dc44-manifest-pbndl002-maintainer", &[0xc2; 32])?;
    crate::trust::add_trusted_maintainer(
        &source,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    std::fs::write(
        source.root().join("dc44-pbndl002.txt"),
        b"dc44 pbndl002 fixture\n",
    )?;
    crate::worktree_patch::commit_worktree_changes_signed(
        &source,
        "heads/main",
        "dc44 pbndl002 fixture",
        crate::worktree_patch::WorktreePatchCommitOptions::default(),
        &author,
    )?;
    crate::rfc111_seal_simulation::simulate_one_seal(&source, "heads/main", &maintainer)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert_eq!(author_keys.len(), 1, "fixture sanity: material to carry");
    let v2_bytes = encode_bundle_v2_for_test(&ref_name, &objects, &author_keys)?;

    // `verify_bundle` sees no manifest at all -- absence, not a decode failure (§4.2).
    let verify_report = verify_bundle(&v2_bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(verify_report.manifest, None);
    assert_eq!(verify_report.author_key_count, 1);

    let target_root = unique_temp_dir("dc44-manifest-pbndl002-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let report = import_bundle(&target, &v2_bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(report.recorded_author_key_count, 1);
    assert!(read_received_pointer(&target, "remotes/heads/main")?.is_some());
    let repository_report = crate::verify_repository(&target)?;
    assert!(
        !repository_report.has_item_failure(),
        "verify must pass against a repository that imported a PBNDL002 bundle: \
         {repository_report:?}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// DC-44 increment 3, control 3 -- the control that proves the manifest is additive, not a
/// change to object identity. The exact same real objects, re-encoded once in the `PBNDL002` shape
/// (no manifest) and once by the real, current `export_bundle` (`PBNDL003`, with a manifest), must
/// decode to the identical set of object ids either way.
#[test]
fn object_ids_are_unchanged_across_the_pbndl003_bump() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-manifest-object-ids-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, current_bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, manifest) =
        decode_bundle(&current_bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        manifest.is_some(),
        "fixture sanity: a real export carries a manifest"
    );

    let v2_bytes = encode_bundle_v2_for_test(&ref_name, &objects, &author_keys)?;
    let (_, v2_objects, _, v2_manifest) =
        decode_bundle(&v2_bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        v2_manifest.is_none(),
        "fixture sanity: PBNDL002 carries no manifest"
    );

    let current_ids: std::collections::BTreeSet<ObjectId> =
        objects.iter().map(ObjectEnvelope::object_id).collect();
    let v2_ids: std::collections::BTreeSet<ObjectId> =
        v2_objects.iter().map(ObjectEnvelope::object_id).collect();
    assert_eq!(
        current_ids, v2_ids,
        "the manifest section must not change a single object's own id"
    );
    assert!(
        !current_ids.is_empty(),
        "fixture sanity: a non-trivial closure"
    );

    let _ = std::fs::remove_dir_all(source_root);
    Ok(())
}

/// DC-44 increment 3, control 4: a manifest that disagrees with the payload is refused, by both
/// `verify` and `import` -- the same treatment the bundle-internal author-key self-consistency
/// check above already gets. Three independent disagreements, each its own case: the manifest's
/// declared ref name against the bundle's own header, the manifest's declared ref name against the
/// exported RefState's own signed name, and the manifest's declared object count against how many
/// objects the bundle actually carries.
#[test]
fn a_manifest_that_disagrees_with_the_payload_is_refused_by_verify_and_import()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-manifest-disagreement-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let Some(manifest) = manifest else {
        return Err(prikk_error::PrikkError::Integrity(
            "fixture sanity: a real export carries a manifest".to_string(),
        ));
    };
    let manifest = BundleManifest {
        repository_format: manifest.repository_format,
        tool_version: manifest.tool_version,
        scope: manifest.scope,
    };

    let options = BundleImportOptions::default_limits();
    let target_root = unique_temp_dir("dc44-manifest-disagreement-target");

    // Case 1: the manifest's own declared object count disagrees with what the bundle actually
    // carries -- built by hand-encoding the manifest section with a wrong count, since
    // `encode_bundle` itself always derives an agreeing count from the real `objects` slice.
    let wrong_count_bytes = encode_bundle_with_wrong_manifest_object_count(
        &ref_name,
        &objects,
        &author_keys,
        &manifest,
    )?;
    assert!(verify_bundle(&wrong_count_bytes, &options).is_err());
    let target = RepositoryLayout::init(target_root.clone())?;
    assert!(import_bundle(&target, &wrong_count_bytes, &options).is_err());
    assert!(
        read_received_pointer(&target, "remotes/heads/main")?.is_none(),
        "a refused import must leave no pointer"
    );

    // Case 2: the manifest's declared ref name disagrees with the bundle's own header ref name.
    let wrong_header_bytes =
        encode_bundle_with_wrong_manifest_ref_name(&ref_name, &objects, &author_keys, &manifest)?;
    assert!(verify_bundle(&wrong_header_bytes, &options).is_err());
    assert!(import_bundle(&target, &wrong_header_bytes, &options).is_err());

    // Case 3: the manifest's declared ref name disagrees with the exported RefState's own signed
    // `ref_name` -- both the header and the manifest agree with each other, but not with the
    // signed payload underneath, so the header-vs-manifest check alone would miss this.
    let mismatched_signed_name = format!("{ref_name}-does-not-exist");
    let bytes_with_mismatched_header =
        encode_bundle(&mismatched_signed_name, &objects, &author_keys, &manifest)?;
    assert!(verify_bundle(&bytes_with_mismatched_header, &options).is_err());
    assert!(import_bundle(&target, &bytes_with_mismatched_header, &options).is_err());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

fn encode_bundle_with_wrong_manifest_object_count(
    ref_name: &str,
    objects: &[ObjectEnvelope],
    author_keys: &[AuthorKeyEntry],
    manifest: &BundleManifest,
) -> prikk_error::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PBNDL003");
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())?);
    for envelope in objects {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(author_keys.len())?);
    for entry in author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    push_u64(&mut out, u64::from(manifest.repository_format));
    push_bytes_u64(&mut out, manifest.tool_version.as_bytes())?;
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())? + 1); // wrong: one more than reality
    push_u64(&mut out, 0);
    Ok(out)
}

fn encode_bundle_with_wrong_manifest_ref_name(
    ref_name: &str,
    objects: &[ObjectEnvelope],
    author_keys: &[AuthorKeyEntry],
    manifest: &BundleManifest,
) -> prikk_error::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PBNDL003");
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, len_to_u64(objects.len())?);
    for envelope in objects {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(author_keys.len())?);
    for entry in author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    push_u64(&mut out, u64::from(manifest.repository_format));
    push_bytes_u64(&mut out, manifest.tool_version.as_bytes())?;
    push_bytes_u64(&mut out, format!("{ref_name}-wrong").as_bytes())?; // wrong: disagrees with header
    push_u64(&mut out, len_to_u64(objects.len())?);
    push_u64(&mut out, 0);
    Ok(out)
}

// DC-78 bundle-export tag-ref gap follow-up (`bundle-export-tag-ref-gap-v1.md`): a tag ref must
// resolve its second hop (ref -> Tag object -> Block) and the Tag envelope itself must travel,
// not just the Block closure it points to -- omitting it hands the receiver a signed RefState
// naming an object they do not have, and their own `verify` fails on exactly that (ruling §1).

/// Publish `tag_name` (a `tags/*` ref) pointing, via a Tag object, at `target_block_id`, every
/// object genuinely MAINTAINER-signed by `maintainer` -- matching `tag.rs`'s own doc, tags are
/// "maintainer-signed, on the same terms as `seal`/`branch create`", not the dummy structural
/// signature `test_support::signed_ref_state_envelope` uses (which cannot pass a real trust check).
fn publish_tag(
    layout: &RepositoryLayout,
    object_store: &mut FileObjectStore,
    tag_name: &str,
    target_block_id: ObjectId,
    maintainer: &Ed25519MaintainerSigner,
) -> prikk_error::Result<ObjectId> {
    let (patch_set_digest, patch_count) =
        crate::compute_patch_set_digest_and_count_from_block(object_store, target_block_id)?;
    let tag_payload = TagPayload {
        name: tag_name.to_string(),
        target_block_id,
        message: None,
        created_at: 0,
        author_key_id: maintainer.key_id().to_string(),
        patch_set_digest,
        patch_count,
    };
    let mut tag_envelope =
        ObjectEnvelope::unsigned(ObjectType::Tag, 1, tag_payload.to_canonical_bytes()?);
    let tag_object_id = tag_envelope.object_id();
    tag_envelope.add_signature(crate::maintainer_signature(
        maintainer,
        ObjectType::Tag,
        tag_object_id,
    )?)?;
    let tag_id = object_store.write_object(&tag_envelope)?;

    let ref_state_payload = RefStatePayload {
        ref_name: tag_name.to_string(),
        kind: RefKind::Tag,
        target_object_id: tag_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut ref_state_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
    );
    let ref_state_id = ref_state_envelope.object_id();
    ref_state_envelope.add_signature(crate::maintainer_signature(
        maintainer,
        ObjectType::RefState,
        ref_state_id,
    )?)?;

    let ref_update_payload = RefUpdatePayload {
        ref_name: tag_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: tag_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: maintainer.key_id().to_string(),
    };
    let mut ref_update_envelope = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
    );
    let ref_update_id = ref_update_envelope.object_id();
    ref_update_envelope.add_signature(crate::maintainer_signature(
        maintainer,
        ObjectType::RefUpdate,
        ref_update_id,
    )?)?;

    let ref_store = RefStore::new(layout.clone());
    ref_store.publish_with_object_store(
        object_store,
        &RefPublication {
            ref_name: tag_name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        },
    )?;
    Ok(tag_id)
}

/// Ruling §3's required addition, stronger than asserting the Tag object is merely present:
/// exporting a tag ref succeeds, and the receiver's own `verify_repository` passes against the
/// imported result -- the property that fails today (before the fix) because the receiver holds a
/// signed RefState naming a Tag object it never received.
///
/// Uses a genuinely, fully sealed history -- real commit, real seal, real adopted maintainer, the
/// same discipline `a_pbndl001_bundle_imports_and_its_patch_reads_unverifiable` documents above --
/// not the lightweight `signed_block` structural fixture, which never computes a real
/// state-merkle-root and would fail `verify_repository`'s block-state stage for reasons unrelated
/// to this test.
#[test]
fn export_of_a_tag_ref_succeeds_and_the_imported_bundle_verifies() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc78-tag-export-source");
    let source = RepositoryLayout::init(source_root.clone())?;

    let author = transport_test_signer(0xb4)?;
    let maintainer = Ed25519MaintainerSigner::from_seed("dc78-tag-maintainer", &[0xb5; 32])?;
    crate::trust::add_trusted_maintainer(
        &source,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    std::fs::write(source.root().join("dc78-tag.txt"), b"dc78 tag fixture\n")?;
    crate::worktree_patch::commit_worktree_changes_signed(
        &source,
        "heads/main",
        "dc78 tag fixture",
        crate::worktree_patch::WorktreePatchCommitOptions::default(),
        &author,
    )?;
    let sealed_ref_state_id =
        crate::rfc111_seal_simulation::simulate_one_seal(&source, "heads/main", &maintainer)?;
    let source_object_store = FileObjectStore::new(source.clone());
    let sealed_ref_state_envelope = source_object_store
        .read_typed(sealed_ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("missing sealed RefState".to_string()))?;
    let sealed_ref_state_payload = RefStatePayload::decode_canonical(
        &sealed_ref_state_envelope.canonical_payload,
        sealed_ref_state_envelope.schema_version,
    )?;
    let tip_block_id = sealed_ref_state_payload.target_object_id;

    let mut object_store = FileObjectStore::new(source.clone());
    let tag_id = publish_tag(
        &source,
        &mut object_store,
        "tags/v1",
        tip_block_id,
        &maintainer,
    )?;

    let (_, bytes) = export_bundle(&source, "tags/v1")?;

    let target_root = unique_temp_dir("dc78-tag-export-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    import_bundle(&target, &bytes, &BundleImportOptions::default_limits())?;

    let report = crate::verify_repository(&target)?;
    assert!(
        !report.has_item_failure(),
        "verify must pass against a repository that imported a tag bundle: {report:?}"
    );

    // Review condition (`DC-78-bundle-tag-gap-implementation-review-v1.md` §4): the export-side
    // closure-count test does not prove the Tag object actually *arrived* -- if import ever grew
    // object-type filtering, the exported count would stay +1 while the Tag stopped landing here,
    // and nothing above would notice. Assert arrival directly, on the receiving side, by id.
    let target_object_store = FileObjectStore::new(target.clone());
    assert!(
        target_object_store
            .read_typed(tag_id, ObjectType::Tag)?
            .is_some(),
        "the Tag object must be present in the receiving repository's store after import"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// Ruling §1's structural claim, made executable: a tag ref and a `heads/*` ref pointing at the
/// same block export the identical Block/Patch/Blob closure -- the property that says the second
/// hop landed in the right place, not merely that it landed somewhere.
#[test]
fn tag_ref_and_heads_ref_at_the_same_block_export_the_same_object_closure()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("dc78-tag-vs-heads-closure");
    let layout = RepositoryLayout::init(root.clone())?;
    let tip_block_id = seal_two_block_history(&layout)?;
    let maintainer =
        Ed25519MaintainerSigner::from_seed("dc78-tag-vs-heads-maintainer", &[0xb6; 32])?;
    let mut object_store = FileObjectStore::new(layout.clone());
    publish_tag(
        &layout,
        &mut object_store,
        "tags/v1",
        tip_block_id,
        &maintainer,
    )?;

    let (_, heads_bytes) = export_bundle(&layout, "heads/main")?;
    let (_, tag_bytes) = export_bundle(&layout, "tags/v1")?;

    let (_, heads_objects, _, _) = decode_bundle(&heads_bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let (_, tag_objects, _, _) = decode_bundle(&tag_bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;

    let closure_only = |objects: &[ObjectEnvelope]| {
        objects
            .iter()
            .filter(|envelope| {
                matches!(
                    envelope.object_type,
                    ObjectType::Block | ObjectType::Patch | ObjectType::Blob
                )
            })
            .map(|envelope| (envelope.object_type, envelope.object_id()))
            .collect::<std::collections::BTreeSet<_>>()
    };
    assert_eq!(
        closure_only(&heads_objects),
        closure_only(&tag_objects),
        "a tag ref and a heads ref at the same block must export the identical Block/Patch/Blob \
         closure"
    );
    // The tag bundle carries one more object than the heads bundle: the Tag envelope itself.
    assert_eq!(tag_objects.len(), heads_objects.len() + 1);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

// DC-44 increment 1 (`bundle-offline-verify-handoff-v1.md`): `bundle verify`, offline. Every test
// below exercises `verify_bundle` through the same `validate_bundle_contents` `import_bundle`
// itself calls -- §2's "reuse the decode path, do not re-implement it" -- and several also run
// `import_bundle` against the identical bytes to demonstrate control 3's "verify and import
// agree" in both directions, not merely assert it.

/// Control 1: a good bundle, produced by a real `export_bundle`, verifies -- and the report
/// matches what export itself reported, field for field. A genuinely, fully sealed history --
/// real commit, real seal, real adopted maintainer, the same discipline
/// `a_pbndl001_bundle_imports_and_its_patch_reads_unverifiable` documents -- not the lightweight
/// `signed_block` structural fixture (`seal_two_block_history`), which never computes a real
/// state-merkle-root and would fail `verify_repository`'s block-state stage below for reasons
/// unrelated to this test (DC-44 increment 3, control 1's own "verify the restored repository"
/// half needs a fixture that can actually pass it).
#[test]
fn verify_of_a_real_export_succeeds_and_matches_the_export_report() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-verify-good");
    let source = RepositoryLayout::init(source_root.clone())?;
    let author = transport_test_signer(0xc4)?;
    let maintainer =
        Ed25519MaintainerSigner::from_seed("dc44-verify-good-maintainer", &[0xc5; 32])?;
    crate::trust::add_trusted_maintainer(
        &source,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;
    std::fs::write(
        source.root().join("dc44-verify-good.txt"),
        b"dc44 manifest fixture\n",
    )?;
    crate::worktree_patch::commit_worktree_changes_signed(
        &source,
        "heads/main",
        "dc44 manifest fixture",
        crate::worktree_patch::WorktreePatchCommitOptions::default(),
        &author,
    )?;
    let sealed_ref_state_id =
        crate::rfc111_seal_simulation::simulate_one_seal(&source, "heads/main", &maintainer)?;
    let source_object_store = FileObjectStore::new(source.clone());
    let sealed_ref_state_envelope = source_object_store
        .read_typed(sealed_ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| prikk_error::PrikkError::Integrity("missing sealed RefState".to_string()))?;
    let tip_block_id = RefStatePayload::decode_canonical(
        &sealed_ref_state_envelope.canonical_payload,
        sealed_ref_state_envelope.schema_version,
    )?
    .target_object_id;
    let (export_report, bytes) = export_bundle(&source, "heads/main")?;

    let verify_report = verify_bundle(&bytes, &BundleImportOptions::default_limits())?;
    assert_eq!(verify_report.ref_name, export_report.ref_name);
    assert_eq!(verify_report.tip_block_id, tip_block_id);
    assert_eq!(verify_report.object_count, export_report.object_count);
    assert_eq!(
        verify_report.author_key_count,
        export_report.author_key_count
    );
    // DC-44 increment 3, control 1 (round trip) + control 5 (verify reports the manifest):
    // `verify_bundle`'s own manifest must match `export_bundle`'s exactly, and it must be present
    // at all -- a real export always emits `PBNDL003`.
    assert_eq!(verify_report.manifest, Some(export_report.manifest));

    // Control 1's other half: the same bundle, imported and then verified by ordinary,
    // unmodified `verify_repository` -- the manifest changes nothing about what import writes or
    // what verify checks (module doc: "no new verification path").
    let import_target_root = unique_temp_dir("dc44-verify-good-import-target");
    let import_target = RepositoryLayout::init(import_target_root.clone())?;
    import_bundle(
        &import_target,
        &bytes,
        &BundleImportOptions::default_limits(),
    )?;
    let repository_report = crate::verify_repository(&import_target)?;
    assert!(
        !repository_report.has_item_failure(),
        "verify must pass against a repository that imported a PBNDL003 bundle: \
         {repository_report:?}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(import_target_root);
    Ok(())
}

/// Control 2 (framing) + control 3 (agreement): the exact three malformed byte sequences
/// `import_of_malformed_bytes_fails_closed` already covers for import -- confirmed here to be
/// refused by `verify_bundle` too, with no repository at all.
#[test]
fn verify_rejects_the_same_malformed_bytes_import_rejects() {
    let options = BundleImportOptions::default_limits();
    assert!(verify_bundle(b"not a bundle", &options).is_err());
    assert!(verify_bundle(b"PBNDL001", &options).is_err());
    assert!(verify_bundle(&[], &options).is_err());
}

/// Control 2 (framing): a bundle truncated partway through its object list is refused, by both
/// `verify` and `import` -- exercising `ByteCursor`'s own "unexpected end of record" rather than
/// a bespoke truncation check, since decode is shared.
#[test]
fn verify_and_import_agree_a_truncated_bundle_is_refused() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-verify-truncated");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (truncated, _) = bytes.split_at(bytes.len() / 2);
    let options = BundleImportOptions::default_limits();

    assert!(
        verify_bundle(truncated, &options).is_err(),
        "a truncated bundle must fail verification"
    );
    let target_root = unique_temp_dir("dc44-verify-truncated-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    assert!(
        import_bundle(&target, truncated, &options).is_err(),
        "the same truncated bytes must fail import too"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// Control 2 (framing): a bundle whose declared object count disagrees with what actually
/// follows -- both directions (claims more than it carries, claims fewer than it carries) --
/// refused by `verify` and `import` alike.
#[test]
fn verify_and_import_agree_a_declared_count_that_disagrees_with_content_is_refused()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-verify-count-mismatch");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    assert!(
        objects.len() > 1,
        "fixture sanity: need room to under-count"
    );
    let options = BundleImportOptions::default_limits();

    for declared_count in [
        len_to_u64(objects.len())? + 1, // claims one more object than it carries
        len_to_u64(objects.len())? - 1, // claims one fewer -- leaves trailing bytes unconsumed
    ] {
        let malformed_bytes =
            encode_bundle_with_declared_count(&ref_name, &objects, &author_keys, declared_count)?;
        assert!(
            verify_bundle(&malformed_bytes, &options).is_err(),
            "declared count {declared_count} against {} real objects must fail verification",
            objects.len()
        );
        let target_root = unique_temp_dir("dc44-verify-count-mismatch-target");
        let target = RepositoryLayout::init(target_root.clone())?;
        assert!(
            import_bundle(&target, &malformed_bytes, &options).is_err(),
            "the same bytes must fail import too"
        );
        let _ = std::fs::remove_dir_all(target_root);
    }

    let _ = std::fs::remove_dir_all(source_root);
    Ok(())
}

/// Hand-encode a `PBNDL002` bundle with a caller-chosen declared object count, independent of
/// `objects.len()` -- `encode_bundle` always writes the true count, so producing a disagreeing one
/// needs this sibling rather than a parameter on the real encoder (which must never be able to
/// lie about its own input).
fn encode_bundle_with_declared_count(
    ref_name: &str,
    objects: &[ObjectEnvelope],
    author_keys: &[AuthorKeyEntry],
    declared_count: u64,
) -> prikk_error::Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(b"PBNDL002");
    push_bytes_u64(&mut out, ref_name.as_bytes())?;
    push_u64(&mut out, declared_count);
    for envelope in objects {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, len_to_u64(author_keys.len())?);
    for entry in author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    Ok(out)
}

/// Control 2's decisive case, and the reason this increment is worth building (§3.1): a Patch
/// object whose bytes were corrupted -- replaced with different, still individually well-formed
/// Patch content, so it decodes cleanly on its own -- recomputes to a different object id than the
/// one the child Block's own `patch_ids` names. Neither `verify` nor `import` can find "that id"
/// among the bundle's objects any more, because it no longer exists; both refuse, for the same
/// reason, via the same closure check.
#[test]
fn verify_and_import_agree_a_corrupted_object_whose_id_no_longer_matches_its_bytes_is_refused()
-> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-verify-corrupted-id");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let original_patch_id = objects
        .iter()
        .find(|envelope| envelope.object_type == ObjectType::Patch)
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("fixture sanity: exactly one patch".to_string())
        })?
        .object_id();

    // A different, still-valid Patch payload -- different node id and path from
    // `signed_patch_envelope`'s own fixture, referencing the same blob so only the patch's own
    // identity changes, not blob closure.
    let blob_id = objects
        .iter()
        .find(|envelope| envelope.object_type == ObjectType::Blob)
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity(
                "fixture sanity: the patch's own blob travels too".to_string(),
            )
        })?
        .object_id();
    let corrupted_payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "corrupted.txt".to_string(),
                node_id: NodeId::from_bytes([0x99; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let corrupted_bytes = corrupted_payload.to_canonical_bytes()?;
    let corrupted_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .map(|mut envelope| {
            if envelope.object_type == ObjectType::Patch {
                envelope.canonical_payload = corrupted_bytes.clone();
            }
            envelope
        })
        .collect();
    let corrupted_patch_id = corrupted_objects
        .iter()
        .find(|envelope| envelope.object_type == ObjectType::Patch)
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity(
                "fixture sanity: still exactly one patch".to_string(),
            )
        })?
        .object_id();
    assert_ne!(
        original_patch_id, corrupted_patch_id,
        "fixture sanity: the corruption must actually change the recomputed id"
    );
    let corrupted_bundle_bytes = encode_bundle(
        &ref_name,
        &corrupted_objects,
        &author_keys,
        &test_manifest(),
    )?;

    let options = BundleImportOptions::default_limits();
    let verify_err = match verify_bundle(&corrupted_bundle_bytes, &options) {
        Ok(report) => panic!("a corrupted patch id must fail verification: {report:?}"),
        Err(err) => err,
    };
    assert!(
        verify_err.to_string().contains("names patch"),
        "expected a closure-check failure naming the missing patch id: {verify_err}"
    );

    let target_root = unique_temp_dir("dc44-verify-corrupted-id-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_err = match import_bundle(&target, &corrupted_bundle_bytes, &options) {
        Ok(report) => panic!("the same corrupted bytes must fail import too: {report:?}"),
        Err(err) => err,
    };
    assert!(
        import_err.to_string().contains("names patch"),
        "expected the identical closure-check failure at import: {import_err}"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// Control 3, the other direction: a bundle missing a patch-referenced blob is refused by
/// `verify` exactly as `row2_a_bundle_missing_a_referenced_blob_is_refused` already proves it is
/// refused by `import` -- same fixture-construction technique, both callers checked here.
#[test]
fn verify_and_import_agree_a_bundle_missing_a_referenced_blob_is_refused() -> prikk_error::Result<()>
{
    let source_root = unique_temp_dir("dc44-verify-missing-blob");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let (ref_name, objects, author_keys, _manifest) =
        decode_bundle(&bytes, DEFAULT_BUNDLE_MAX_OBJECT_COUNT)?;
    let broken_objects: Vec<ObjectEnvelope> = objects
        .into_iter()
        .filter(|envelope| envelope.object_type != ObjectType::Blob)
        .collect();
    let broken_bytes = encode_bundle(&ref_name, &broken_objects, &author_keys, &test_manifest())?;
    let options = BundleImportOptions::default_limits();

    let verify_err = match verify_bundle(&broken_bytes, &options) {
        Ok(report) => {
            panic!("a bundle missing a patch-referenced blob must fail verification: {report:?}")
        }
        Err(err) => err,
    };
    assert!(verify_err.to_string().contains("references blob"));

    let target_root = unique_temp_dir("dc44-verify-missing-blob-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    let import_err = match import_bundle(&target, &broken_bytes, &options) {
        Ok(report) => panic!("the same bundle must fail import too: {report:?}"),
        Err(err) => err,
    };
    assert!(import_err.to_string().contains("references blob"));

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}

/// Control 3, positive direction restated for `verify` specifically (control 1 already proves
/// `verify` accepts a good bundle; this proves `import` accepts the identical bytes too, closing
/// the loop both ways in one place).
#[test]
fn verify_and_import_agree_a_well_formed_bundle_is_accepted() -> prikk_error::Result<()> {
    let source_root = unique_temp_dir("dc44-verify-agree-good");
    let source = RepositoryLayout::init(source_root.clone())?;
    seal_two_block_history(&source)?;
    let (_, bytes) = export_bundle(&source, "heads/main")?;
    let options = BundleImportOptions::default_limits();

    assert!(verify_bundle(&bytes, &options).is_ok());

    let target_root = unique_temp_dir("dc44-verify-agree-good-target");
    let target = RepositoryLayout::init(target_root.clone())?;
    assert!(import_bundle(&target, &bytes, &options).is_ok());

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}
