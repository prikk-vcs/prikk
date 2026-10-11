//! Import tests (RFC 155 implementation handoff, Part I-A: stages 0-2, the journal, the writer
//! interlock, and `doctor --cancel-import`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope,
    ObjectId, ObjectType, RefKind, RefStatePayload,
};

use std::sync::{Arc, Mutex};

use crate::archive::{export_archive, import_archive, interrupted_import_short_id};
use crate::author::author_signing::AuthorSigner as _;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lock::ActiveLock;
use crate::maintainer_signing::MaintainerSigner as _;
use crate::test_gates::test_support::{signed_ref_update_envelope, unique_temp_dir};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, author_signature, maintainer_signature,
};

/// A genuinely internally-consistent source repository: one real author-signed `Patch` (and its
/// unsigned `Blob`), one real maintainer-signed `Block` naming it, and a real maintainer-signed
/// `RefState` publishing `heads/main` at that block -- every signature in this fixture actually
/// verifies, unlike `archive::tests::seal_one_block`'s deliberate placeholders (which make
/// `is_internally_consistent` false, and so cannot be used for import: stage 0 refuses on exactly
/// that). Returns the layout, the block id, and the author signer's own key id (for assertions).
fn build_signed_source_repository(dir: &std::path::Path) -> (RepositoryLayout, ObjectId, String) {
    let layout = RepositoryLayout::init(dir).expect("init");

    let author_signer =
        Ed25519AuthorSigner::from_seed("import-test-author", &[0x11; 32]).expect("author signer");
    let author_key_id = author_signer.key_id().to_string();
    {
        let active_lock = ActiveLock::acquire_for_write(&layout, DEFAULT_ACTIVE_NAME)
            .expect("active lock for author key recording");
        crate::author::author_key_index::record_author_key_material(
            &layout,
            &author_key_id,
            author_signer.public_key_bytes(),
            &active_lock,
        )
        .expect("record the author key locally, so the exported archive carries it");
    }

    let maintainer_signer =
        Ed25519MaintainerSigner::from_seed("import-test-maintainer", &[0x22; 32])
            .expect("maintainer signer");
    let maintainer_public_key_hex: String = maintainer_signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(
        &layout,
        maintainer_signer.key_id(),
        &maintainer_public_key_hex,
    )
    .expect("adopt the maintainer key locally");

    let mut store = FileObjectStore::new(layout.clone());

    let blob_payload = BlobPayload::new(BlobKind::Text, b"import test content\n".to_vec());
    let blob_bytes = blob_payload
        .to_canonical_bytes()
        .expect("encode blob payload");
    let blob_envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_bytes);
    let blob_id = blob_envelope.object_id();
    store.write_object(&blob_envelope).expect("write blob");

    let patch_payload = prikk_object::PatchPayload {
        operations: vec![prikk_object::Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: prikk_object::OperationKind::CreateFile(prikk_object::CreateFile {
                path: "a.txt".to_string(),
                node_id: prikk_object::NodeId::from_bytes([0x51; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: prikk_object::PatchPurpose::Normal,
        message: None,
    };
    let patch_bytes = patch_payload
        .to_canonical_bytes()
        .expect("encode patch payload");
    let mut patch_envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch_bytes);
    let patch_id = patch_envelope.object_id();
    patch_envelope
        .add_signature(author_signature(&author_signer, patch_id).expect("sign the patch for real"))
        .expect("attach the real author signature");
    store.write_object(&patch_envelope).expect("write patch");

    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: vec![patch_id],
        state_merkle_root: MerkleRoot([0u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_bytes = block_payload
        .to_canonical_bytes()
        .expect("encode block payload");
    let mut block_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes);
    let block_id = block_envelope.object_id();
    block_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::Block, block_id)
                .expect("sign the block for real"),
        )
        .expect("attach the real maintainer signature");
    store.write_object(&block_envelope).expect("write block");

    let ref_state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_bytes = ref_state_payload
        .to_canonical_bytes()
        .expect("encode ref-state payload");
    let mut ref_state_envelope = ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state_bytes);
    let ref_state_id = ref_state_envelope.object_id();
    ref_state_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::RefState, ref_state_id)
                .expect("sign the ref state for real"),
        )
        .expect("attach the real maintainer signature");

    let ref_update_envelope =
        signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        })
        .expect("publish heads/main");

    (layout, block_id, author_key_id)
}

fn export_bytes(layout: &RepositoryLayout) -> Vec<u8> {
    let mut bytes = Vec::new();
    export_archive(layout, &mut bytes).expect("export");
    bytes
}

#[test]
fn source_fixture_is_genuinely_internally_consistent() {
    let dir = unique_temp_dir("archive-import-fixture");
    let (layout, _block_id, _author_key_id) = build_signed_source_repository(&dir);
    let bytes = export_bytes(&layout);
    let mut cursor = Cursor::new(bytes);
    let report = crate::archive::verify_archive(&mut cursor).expect("verify");
    assert!(
        report.is_internally_consistent(),
        "the import fixture must be genuinely consistent, or every import test below is testing \
         nothing: {report:?}"
    );
}

#[test]
fn import_writes_objects_records_the_author_key_and_commits_the_journal() {
    let source_dir = unique_temp_dir("archive-import-source");
    let (source_layout, block_id, author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    let mut cursor = Cursor::new(bytes);
    let report = import_archive(&dest_layout, &mut cursor).expect("import");
    assert!(!report.resumed);
    assert!(
        report
            .written_object_counts
            .iter()
            .any(|(object_type, count)| *object_type == ObjectType::Block && *count == 1)
    );
    assert_eq!(report.recorded_author_key_count, 1);
    assert_eq!(report.planned_landings.len(), 1);
    assert_eq!(report.planned_landings[0].ref_name, "heads/main");
    assert_eq!(report.planned_landings[0].destination, "remotes/heads/main");

    // Stage 1 really wrote the block through the ordinary path -- content-addressed, readable
    // back.
    let dest_store = FileObjectStore::new(dest_layout.clone());
    assert!(dest_store.contains_object(ObjectType::Block, block_id));

    // Stage 2 really recorded the author key -- a second author signing under the same id would
    // now resolve against it, which `author_key_binding` reports as `Matches`.
    let binding = crate::author_key_binding(
        &dest_layout,
        &author_key_id,
        Ed25519AuthorSigner::from_seed("import-test-author", &[0x11; 32])
            .expect("signer")
            .public_key_bytes(),
    )
    .expect("author_key_binding");
    assert_eq!(binding, crate::AuthorKeyBinding::Matches);

    // The commit point emptied the journal -- no import is reported as in progress.
    assert!(
        interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .is_none()
    );

    // No journal file exists, no ordinary writer refuses.
    ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME)
        .expect("an ordinary writer must not refuse once the import has committed");
}

#[test]
fn import_refuses_an_inconsistent_archive_and_writes_nothing() {
    let source_dir = unique_temp_dir("archive-import-inconsistent-source");
    let layout = RepositoryLayout::init(&source_dir).expect("init");
    crate::archive::tests::seal_one_block(&layout).expect("seal");
    let bytes = export_bytes(&layout);

    let dest_dir = unique_temp_dir("archive-import-inconsistent-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let fingerprint_before = directory_fingerprint(&dest_dir);

    let mut cursor = Cursor::new(bytes);
    let error = import_archive(&dest_layout, &mut cursor)
        .expect_err("a placeholder-signed archive is not internally consistent");
    assert!(
        error.to_string().contains("not internally consistent"),
        "{error}"
    );
    assert_eq!(
        fingerprint_before,
        directory_fingerprint(&dest_dir),
        "a refused import must write nothing"
    );
    assert!(interrupted_import_short_id(&dest_layout).unwrap().is_none());
}

#[test]
fn killed_right_after_the_journal_blocks_writers_and_doctor_sees_it_then_resumes() {
    let source_dir = unique_temp_dir("archive-import-kill-journal-source");
    let (source_layout, block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-kill-journal-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    // The case table's own "kill right after the journal" row.
    crate::archive::import::stop_after_journal_for_test();
    let mut cursor = Cursor::new(bytes.clone());
    let error = import_archive(&dest_layout, &mut cursor)
        .expect_err("the test seam stops the import right after the journal's own write");
    assert!(error.to_string().contains("simulated kill"), "{error}");

    // `doctor` sees the interrupted import.
    let id = interrupted_import_short_id(&dest_layout)
        .expect("read journal state")
        .expect("a journal is present after the simulated kill");
    assert_eq!(id.len(), 16);

    // Every ordinary writer refuses while it is present.
    let refusal = ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME)
        .expect_err("an ordinary writer must refuse while the journal is present");
    assert!(refusal.to_string().contains(&id), "{refusal}");

    // Nothing was written yet (the journal was the first and only write) -- `doctor`'s own two
    // ways out both apply cleanly from here. Resume: running the same import again finishes it.
    let mut cursor2 = Cursor::new(bytes);
    let resumed = import_archive(&dest_layout, &mut cursor2).expect("resume finishes the import");
    assert!(resumed.resumed);
    let dest_store = FileObjectStore::new(dest_layout.clone());
    assert!(dest_store.contains_object(ObjectType::Block, block_id));
    assert!(
        interrupted_import_short_id(&dest_layout).unwrap().is_none(),
        "the commit point emptied the journal"
    );
    ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME)
        .expect("an ordinary writer must not refuse once the resumed import has committed");
}

#[test]
fn killed_after_the_objects_is_cancelled_and_every_container_is_cut_back() {
    let source_dir = unique_temp_dir("archive-import-kill-stage1-source");
    let (source_layout, block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-kill-stage1-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    // The case table's own "kill after the objects" row: stage 1 really wrote the block, then the
    // test seam stops before stage 2 or the commit point.
    crate::archive::import::stop_after_stage_1_for_test();
    let mut cursor = Cursor::new(bytes);
    let error = import_archive(&dest_layout, &mut cursor)
        .expect_err("the test seam stops the import right after stage 1");
    assert!(error.to_string().contains("simulated kill"), "{error}");

    let dest_store = FileObjectStore::new(dest_layout.clone());
    assert!(
        dest_store.contains_object(ObjectType::Block, block_id),
        "stage 1 really wrote the block before the simulated kill"
    );
    let id = interrupted_import_short_id(&dest_layout)
        .expect("read journal state")
        .expect("a journal is present after the simulated kill");

    // Cancel, under `--plan-only` first: previews without writing.
    let plan = crate::archive::cancel_import(&dest_layout, &id, true).expect("plan-only cancel");
    assert!(plan.plan_only);
    assert!(
        plan.steps.iter().any(|step| step.cut_bytes > 0),
        "at least the block container grew past its journaled starting length: {:?}",
        plan.steps
    );
    assert!(
        interrupted_import_short_id(&dest_layout).unwrap().is_some(),
        "--plan-only must write nothing, including to the journal itself"
    );

    // The real cancel: every container goes back to its journaled starting length, and the
    // journal itself is emptied.
    let real = crate::archive::cancel_import(&dest_layout, &id, false).expect("real cancel");
    assert!(!real.plan_only);
    assert_eq!(
        real.steps.iter().map(|step| step.cut_bytes).sum::<u64>(),
        plan.steps.iter().map(|step| step.cut_bytes).sum::<u64>(),
        "the plan and the real run must agree on what they cut"
    );
    assert!(
        real.steps
            .iter()
            .filter(|step| step.cut_bytes > 0)
            .all(|step| step.recovery.is_some()),
        "every real cut is saved to the recovery log first: {:?}",
        real.steps
    );
    assert!(
        !FileObjectStore::new(dest_layout.clone()).contains_object(ObjectType::Block, block_id),
        "cancel must restore \"untouched\": the block the kill left behind is gone"
    );
    assert!(interrupted_import_short_id(&dest_layout).unwrap().is_none());
    ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME)
        .expect("an ordinary writer must not refuse once cancel has emptied the journal");

    // Cancelling an already-cancelled import refuses by name, rather than silently doing nothing.
    let error = crate::archive::cancel_import(&dest_layout, &id, false)
        .expect_err("no journal is present after cancel");
    assert!(
        error.to_string().contains("no archive import journal"),
        "{error}"
    );
}

#[test]
fn cancel_refuses_an_id_that_does_not_match_the_present_journal() {
    let source_dir = unique_temp_dir("archive-import-cancel-wrong-id-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-cancel-wrong-id-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    crate::archive::import::stop_after_journal_for_test();
    let mut cursor = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor).expect_err("the test seam stops the import");

    let error = crate::archive::cancel_import(&dest_layout, "0000000000000000", false)
        .expect_err("the present journal's real id does not match this one");
    assert!(error.to_string().contains("no import with id"), "{error}");
}

/// RFC 155 Part I-A2, review finding IA1(a): stage 1 now holds the `ObjectStore` container lock
/// for its whole run, not just per append -- this proves it structurally, not by inspection. A
/// callback runs right after stage 1's own first object write, while the lock is genuinely held,
/// and attempts a *second*, wholly unrelated object append through an ordinary `FileObjectStore`
/// handle (never through this import's own session). That append must refuse with `LockConflict`.
#[test]
fn concurrent_object_append_refuses_while_stage_1_holds_the_object_store_lock() {
    let source_dir = unique_temp_dir("archive-import-ia1-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-ia1-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    let concurrent_result: Arc<Mutex<Option<prikk_error::Result<()>>>> = Arc::new(Mutex::new(None));
    let probe_layout = dest_layout.clone();
    let probe_result = concurrent_result.clone();
    crate::archive::import::run_after_first_object_write_for_test(move || {
        let blob_payload = BlobPayload::new(BlobKind::Text, b"concurrent append\n".to_vec());
        let blob_bytes = blob_payload
            .to_canonical_bytes()
            .expect("encode concurrent blob payload");
        let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob_bytes);
        let mut concurrent_store = FileObjectStore::new(probe_layout);
        let outcome = concurrent_store.write_object(&envelope).map(|_| ());
        *probe_result.lock().expect("lock") = Some(outcome);
    });

    let mut cursor = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor).expect("import completes normally");

    let outcome = concurrent_result
        .lock()
        .expect("lock")
        .take()
        .expect("the callback ran during stage 1");
    let error = outcome.expect_err(
        "a concurrent object append must refuse with LockConflict while stage 1 holds the \
         object-store lock for its whole run",
    );
    assert!(
        matches!(error, prikk_error::PrikkError::LockConflict(_)),
        "expected LockConflict, got: {error:?}"
    );
}

/// RFC 155 Part I-A2, review finding IA2: the author-key container is journaled and cut back on
/// cancel. The kill point is *after* stage 2 has actually recorded the author key (the seam right
/// after stage 1 would never exercise this at all -- the key would never have been recorded in
/// the first place), so this test genuinely depends on the cut-back: without it, the key id would
/// stay bound to the cancelled import's own public key, and recording a different one for the
/// same id afterward would refuse as a conflict.
#[test]
fn cancelled_import_cuts_back_the_author_key_so_a_different_key_can_be_recorded_for_the_same_id() {
    let source_dir = unique_temp_dir("archive-import-ia2-source");
    let (source_layout, _block_id, author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-ia2-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    crate::archive::import::stop_after_stage_2_for_test();
    let mut cursor = Cursor::new(bytes);
    let error = import_archive(&dest_layout, &mut cursor)
        .expect_err("the test seam stops right after stage 2");
    assert!(error.to_string().contains("simulated kill"), "{error}");

    let id = interrupted_import_short_id(&dest_layout)
        .expect("read journal state")
        .expect("a journal is present after the simulated kill");

    // Stage 2 really ran before the kill: the archive's own author key is bound, under the
    // archive's own public key.
    let source_author_signer =
        Ed25519AuthorSigner::from_seed("import-test-author", &[0x11; 32]).expect("signer");
    assert_eq!(
        crate::author_key_binding(
            &dest_layout,
            &author_key_id,
            source_author_signer.public_key_bytes(),
        )
        .expect("author_key_binding"),
        crate::AuthorKeyBinding::Matches,
        "stage 2 ran before the kill -- the archive's own author key is already recorded"
    );

    crate::archive::cancel_import(&dest_layout, &id, false).expect("cancel");
    assert!(interrupted_import_short_id(&dest_layout).unwrap().is_none());

    // The cut-back's own proof: a genuinely different public key for the *same* key id is now
    // recordable. Before IA2's fix, the cancelled import's own record would still be on file,
    // and this would refuse as a key-rotation conflict.
    let rotated_signer =
        Ed25519AuthorSigner::from_seed("import-test-author-rotated", &[0x33; 32]).expect("signer");
    let active_lock =
        ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME).expect("active lock");
    crate::author::author_key_index::record_author_key_material(
        &dest_layout,
        &author_key_id,
        rotated_signer.public_key_bytes(),
        &active_lock,
    )
    .expect(
        "a different public key must be recordable for this key id -- the cancelled import's \
         own record was cut back",
    );
}

/// RFC 155 Part I-A2, review finding IA3(a): a kill strictly *between* two object writes (not
/// only before or after the whole of stage 1). A resume from here must finish the import; a
/// cancel from here must restore every journaled container to a state byte-identical to before
/// the import ever started.
#[test]
fn killed_mid_objects_resumes_cleanly_and_a_separate_cancel_restores_every_container() {
    let source_dir = unique_temp_dir("archive-import-ia3a-source");
    let (source_layout, block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    // Resume branch.
    {
        let dest_dir = unique_temp_dir("archive-import-ia3a-resume-dest");
        let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
        crate::archive::import::kill_mid_objects_after_n_for_test(2);
        let mut cursor = Cursor::new(bytes.clone());
        let error = import_archive(&dest_layout, &mut cursor)
            .expect_err("the test seam stops strictly between two object writes");
        assert!(error.to_string().contains("simulated kill"), "{error}");
        interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .expect("a journal is present after the simulated kill");

        let mut cursor2 = Cursor::new(bytes.clone());
        let resumed = import_archive(&dest_layout, &mut cursor2).expect("resume finishes");
        assert!(resumed.resumed);
        let dest_store = FileObjectStore::new(dest_layout.clone());
        assert!(dest_store.contains_object(ObjectType::Block, block_id));
        assert!(
            interrupted_import_short_id(&dest_layout).unwrap().is_none(),
            "the commit point emptied the journal"
        );
    }

    // Cancel branch -- a separate destination, so the hash-before snapshot is genuinely "before
    // the import ever started" rather than "before a resume."
    {
        let dest_dir = unique_temp_dir("archive-import-ia3a-cancel-dest");
        let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
        let hashes_before =
            crate::archive::import::hash_journaled_containers_for_test(&dest_layout)
                .expect("hash before");

        crate::archive::import::kill_mid_objects_after_n_for_test(2);
        let mut cursor = Cursor::new(bytes);
        import_archive(&dest_layout, &mut cursor)
            .expect_err("the test seam stops strictly between two object writes");
        let id = interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .expect("a journal is present after the simulated kill");

        crate::archive::cancel_import(&dest_layout, &id, false).expect("cancel");
        assert!(interrupted_import_short_id(&dest_layout).unwrap().is_none());

        let hashes_after = crate::archive::import::hash_journaled_containers_for_test(&dest_layout)
            .expect("hash after");
        assert_eq!(
            hashes_before, hashes_after,
            "cancel must restore every journaled container to a state byte-identical to before \
             the import ever started"
        );
    }
}

/// RFC 155 Part I-A2, review finding IA3(b): the same mid-objects failpoint, but a real
/// `std::io::Error` (simulating a full disk) rather than a clean kill -- resume and cancel must
/// behave identically to IA3(a)'s clean kill.
#[test]
fn full_disk_mid_objects_resumes_cleanly_and_a_separate_cancel_restores_every_container() {
    let source_dir = unique_temp_dir("archive-import-ia3b-source");
    let (source_layout, block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    {
        let dest_dir = unique_temp_dir("archive-import-ia3b-resume-dest");
        let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
        crate::archive::import::fail_mid_objects_after_n_for_test(2);
        let mut cursor = Cursor::new(bytes.clone());
        let error = import_archive(&dest_layout, &mut cursor)
            .expect_err("the test seam fails strictly between two object writes");
        assert!(
            matches!(error, prikk_error::PrikkError::Io { .. }),
            "expected a real i/o error, got: {error:?}"
        );
        interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .expect("a journal is present after the simulated i/o failure");

        let mut cursor2 = Cursor::new(bytes.clone());
        let resumed = import_archive(&dest_layout, &mut cursor2).expect("resume finishes");
        assert!(resumed.resumed);
        let dest_store = FileObjectStore::new(dest_layout.clone());
        assert!(dest_store.contains_object(ObjectType::Block, block_id));
        assert!(interrupted_import_short_id(&dest_layout).unwrap().is_none());
    }

    {
        let dest_dir = unique_temp_dir("archive-import-ia3b-cancel-dest");
        let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
        let hashes_before =
            crate::archive::import::hash_journaled_containers_for_test(&dest_layout)
                .expect("hash before");

        crate::archive::import::fail_mid_objects_after_n_for_test(2);
        let mut cursor = Cursor::new(bytes);
        import_archive(&dest_layout, &mut cursor)
            .expect_err("the test seam fails strictly between two object writes");
        let id = interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .expect("a journal is present after the simulated i/o failure");

        crate::archive::cancel_import(&dest_layout, &id, false).expect("cancel");
        let hashes_after = crate::archive::import::hash_journaled_containers_for_test(&dest_layout)
            .expect("hash after");
        assert_eq!(
            hashes_before, hashes_after,
            "cancel must restore every journaled container to a state byte-identical to before \
             the import ever started, the same as a clean kill"
        );
    }
}

/// RFC 155 Part I-A2, review finding IA3(c): a cancel right after the journal (before stage 1 has
/// written anything) has nothing to cut, empties the journal, and leaves every session's writer
/// unrefused afterward -- including a non-default session, per IA1(b)'s "every session" fix.
#[test]
fn cancel_right_after_the_journal_cuts_nothing_and_every_session_proceeds_after() {
    let source_dir = unique_temp_dir("archive-import-ia3c-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-ia3c-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    crate::archive::import::stop_after_journal_for_test();
    let mut cursor = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor)
        .expect_err("the test seam stops right after the journal");
    let id = interrupted_import_short_id(&dest_layout)
        .expect("read journal state")
        .expect("a journal is present after the simulated kill");

    // Before cancel: every session refuses, not only the default one (IA1(b)).
    assert!(
        ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME).is_err(),
        "the default session must refuse while the journal is present"
    );
    assert!(
        ActiveLock::acquire_for_write(&dest_layout, "second").is_err(),
        "a non-default session must refuse while the journal is present too"
    );

    let report = crate::archive::cancel_import(&dest_layout, &id, false).expect("cancel");
    assert!(
        report.steps.iter().all(|step| step.cut_bytes == 0),
        "nothing was ever written past the journal's own starting lengths: {:?}",
        report.steps
    );
    assert!(interrupted_import_short_id(&dest_layout).unwrap().is_none());

    // After cancel: every session proceeds.
    ActiveLock::acquire_for_write(&dest_layout, DEFAULT_ACTIVE_NAME)
        .expect("the default session must not refuse once the journal is gone");
    ActiveLock::acquire_for_write(&dest_layout, "second")
        .expect("a non-default session must not refuse once the journal is gone either");
}

/// RFC 155 Part I-A2, review finding IA4 (§8's "the adopted set is unchanged," tested rather than
/// reasoned): the archive carries its own maintainer keys, inert and never adopted (§9.1); after
/// a real import, the destination's own, already-adopted maintainer set -- a different key from
/// the archive's own -- is untouched: the trust container files are byte-identical, and the
/// policy query reports the same set.
#[test]
fn import_leaves_the_destinations_own_adopted_maintainer_set_untouched() {
    let source_dir = unique_temp_dir("archive-import-ia4-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-import-ia4-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let dest_maintainer =
        Ed25519MaintainerSigner::from_seed("import-test-dest-maintainer", &[0x44; 32])
            .expect("signer");
    let dest_maintainer_public_key_hex: String = dest_maintainer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(
        &dest_layout,
        dest_maintainer.key_id(),
        &dest_maintainer_public_key_hex,
    )
    .expect("adopt the destination's own maintainer key, distinct from the archive's own");

    let policy_before =
        crate::load_maintainer_trust_policy(&dest_layout).expect("policy before import");
    let trust_keys_before =
        std::fs::read(dest_layout.trust_key_container_path()).expect("read trust keys before");
    let trust_policy_a_before =
        std::fs::read(dest_layout.trust_policy_container_slot_path(crate::ContainerSlot::A))
            .unwrap_or_default();
    let trust_policy_b_before =
        std::fs::read(dest_layout.trust_policy_container_slot_path(crate::ContainerSlot::B))
            .unwrap_or_default();

    let mut cursor = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor).expect("import completes normally");

    let policy_after =
        crate::load_maintainer_trust_policy(&dest_layout).expect("policy after import");
    assert_eq!(
        policy_before, policy_after,
        "the destination's own adopted maintainer set must be unchanged by import"
    );
    assert_eq!(
        trust_keys_before,
        std::fs::read(dest_layout.trust_key_container_path()).expect("read trust keys after"),
        "the trust-keys container must be byte-identical"
    );
    assert_eq!(
        trust_policy_a_before,
        std::fs::read(dest_layout.trust_policy_container_slot_path(crate::ContainerSlot::A))
            .unwrap_or_default(),
        "trust-policy slot a must be byte-identical"
    );
    assert_eq!(
        trust_policy_b_before,
        std::fs::read(dest_layout.trust_policy_container_slot_path(crate::ContainerSlot::B))
            .unwrap_or_default(),
        "trust-policy slot b must be byte-identical"
    );
}

fn directory_fingerprint(dir: &std::path::Path) -> Vec<(String, u64)> {
    fn walk(dir: &std::path::Path, out: &mut Vec<(String, u64)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if let Ok(metadata) = entry.metadata() {
                out.push((path.display().to_string(), metadata.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, &mut out);
    out.sort();
    out
}
