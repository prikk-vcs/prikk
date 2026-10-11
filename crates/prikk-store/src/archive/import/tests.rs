//! Import tests (RFC 155 implementation handoff, Part I-A: stages 0-2, the journal, the writer
//! interlock, and `doctor --cancel-import`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope,
    ObjectId, ObjectType, RefKind, RefStatePayload,
};

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
