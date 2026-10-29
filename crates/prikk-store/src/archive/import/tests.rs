//! Import tests (RFC 155 implementation handoff, Part I-A: stages 0-2, the journal, the writer
//! interlock, and `doctor --cancel-import`).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use std::io::Cursor;

use crate::object_store::ObjectReader as _;

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope,
    ObjectId, ObjectType, RefKind, RefStatePayload,
};

use std::sync::{Arc, Mutex};

use crate::archive::{
    ArchiveImportOptions, export_archive, import_archive, interrupted_import_short_id,
};
use crate::author::author_signing::AuthorSigner as _;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lock::ActiveLock;
use crate::maintainer_signing::MaintainerSigner as _;
use crate::test_gates::test_support::{signed_ref_update_envelope, unique_temp_dir};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore, ObjectWriteSession,
    ObjectWriter, RefPublication, RefStore, RepositoryLayout, author_signature,
    maintainer_signature,
};

/// A genuinely internally-consistent source repository: one real author-signed `Patch` (and its
/// unsigned `Blob`), one real maintainer-signed `Block` naming it, and a real maintainer-signed
/// `RefState` publishing `heads/main` at that block -- every signature in this fixture actually
/// verifies *and* is by a key this repository itself has adopted, unlike `archive::tests::
/// seal_one_block`'s deliberate placeholders, which are by keys nothing ever carries or adopts --
/// since N1's own fix (review `rfc155-impl-IC-review-v1`), that alone no longer makes
/// `is_internally_consistent` false (a signature by a key the archive does not carry is named, not
/// refused on). This fixture is still the right one for tests that need signature *resolution* to
/// positively succeed, not merely avoid refusing. Returns the layout, the block id, and the author
/// signer's own key id (for assertions).
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

/// A genuinely internally-consistent source repository, like [`build_signed_source_repository`],
/// but with a **really** maintainer-signed `RefUpdate` -- `build_signed_source_repository`'s own
/// ref update goes through `test_support::signed_ref_update_envelope`, whose own signature is a
/// placeholder that verifies against nothing (the same `test_support` convention
/// `archive::tests::seal_one_block` uses), which is fine for Part I-A/I-A2's own tests (nothing
/// there ever checks a ref update's signature) but would make every `--adopt` test in this module
/// fall through to `remotes/` regardless of whether the signer is adopted, since
/// `verify_trusted_publication_envelope` would find no verifying signature either way. Returns the
/// layout, the ref's own tip `RefState` id, the maintainer signer's key id, and its public key hex
/// (so a test can choose whether to adopt it at the destination before importing).
fn build_adoptable_source_repository(
    dir: &std::path::Path,
    ref_name: &str,
) -> (RepositoryLayout, ObjectId, String, String) {
    let layout = RepositoryLayout::init(dir).expect("init");

    let maintainer_signer =
        Ed25519MaintainerSigner::from_seed("adopt-test-maintainer", &[0x33; 32])
            .expect("maintainer signer");
    let maintainer_key_id = maintainer_signer.key_id().to_string();
    let maintainer_public_key_hex: String = maintainer_signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    // Adopted locally at the *source*, so the source's own `verify`/export carries this key in
    // its `MaintainerKeys`/`TrustPolicy` sections -- without this, the source repository's own
    // `verify_archive` would find an unresolvable signature and refuse the import at stage 0
    // before any `--adopt` logic is ever reached.
    crate::add_trusted_maintainer(&layout, &maintainer_key_id, &maintainer_public_key_hex)
        .expect("adopt the maintainer key at the source, so export carries it");

    let mut store = FileObjectStore::new(layout.clone());
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).expect("empty state root"),
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
        ref_name: ref_name.to_string(),
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

    let ref_update_payload = prikk_object::RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: maintainer_key_id.clone(),
    };
    let ref_update_bytes = ref_update_payload
        .to_canonical_bytes()
        .expect("encode ref-update payload");
    let mut ref_update_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update_bytes);
    let ref_update_id = ref_update_envelope.object_id();
    ref_update_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::RefUpdate, ref_update_id)
                .expect("sign the ref update for real -- the point of this fixture"),
        )
        .expect("attach the real maintainer signature");

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        })
        .expect("publish the ref");

    (
        layout,
        ref_state_id,
        maintainer_key_id,
        maintainer_public_key_hex,
    )
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
    let report = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("import");
    assert!(!report.resumed);
    assert!(
        report
            .written_object_counts
            .iter()
            .any(|(object_type, count)| *object_type == ObjectType::Block && *count == 1)
    );
    assert_eq!(report.recorded_author_key_count, 1);
    assert_eq!(report.landing_outcomes.len(), 1);
    assert_eq!(report.landing_outcomes[0].ref_name, "heads/main");
    assert_eq!(report.landing_outcomes[0].landed_as, "remotes/heads/main");
    assert!(!report.landing_outcomes[0].adopted);

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

/// N1 (review `rfc155-impl-IC-review-v1`): a placeholder-signed archive (`seal_one_block`'s own
/// fixture) is no longer internally consistent by itself -- neither signature is by a key the
/// archive carries, so both are named "not checked," never refused on. Stage 0's own refusal is
/// now exercised with genuine damage instead: a flipped byte in the block's own record body, which
/// both fails that record's own checksum and breaks the archive's own reference closure.
#[test]
fn import_refuses_an_inconsistent_archive_and_writes_nothing() {
    let source_dir = unique_temp_dir("archive-import-inconsistent-source");
    let layout = RepositoryLayout::init(&source_dir).expect("init");
    crate::archive::tests::seal_one_block(&layout).expect("seal");
    let mut bytes = export_bytes(&layout);
    let flip_at = bytes.len() / 3;
    bytes[flip_at] ^= 0xFF;

    let dest_dir = unique_temp_dir("archive-import-inconsistent-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let fingerprint_before = directory_fingerprint(&dest_dir);

    let mut cursor = Cursor::new(bytes);
    let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("a flipped byte makes the archive genuinely inconsistent");
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

/// 0.56.0 step 3c (A5, 2C v2 §R7): on 0.55.0, `archive import` landed a `Patch` whose own
/// operations do not decode, with exit 0, while `bundle import` refused the identical shape --
/// confirmed missing admission parity. `import_archive`'s own stage 0 reuses `verify_archive`
/// directly (this module's own doc comment), and `verify_archive` itself now refuses this archive
/// (see `archive::verify::tests::verify_refuses_an_archive_carrying_an_undecodable_patch`) -- so
/// this refusal fires at stage 0, before the journal even exists, and nothing at all is written,
/// the same strength [`import_refuses_an_inconsistent_archive_and_writes_nothing`] proves for a
/// flipped byte.
#[test]
fn import_refuses_an_archive_carrying_an_undecodable_patch_and_writes_nothing() {
    let source_dir = unique_temp_dir("archive-import-undecodable-patch-source");
    let layout = RepositoryLayout::init(&source_dir).expect("init");
    crate::archive::tests::seal_one_block_with_an_undecodable_patch(&layout).expect("seal");
    let bytes = export_bytes(&layout);

    let dest_dir = unique_temp_dir("archive-import-undecodable-patch-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let fingerprint_before = directory_fingerprint(&dest_dir);

    let mut cursor = Cursor::new(bytes);
    let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("a patch that does not decode must refuse the import");
    assert!(
        error.to_string().contains("PatchPurpose")
            || error.to_string().contains("not internally consistent"),
        "{error}"
    );
    assert_eq!(
        fingerprint_before,
        directory_fingerprint(&dest_dir),
        "a refused import must write nothing -- the refusal fires at stage 0, before the journal \
         or any object write"
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
    let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
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
    let resumed = import_archive(&dest_layout, &mut cursor2, &ArchiveImportOptions::default())
        .expect("resume finishes the import");
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
    let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
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
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("the test seam stops the import");

    let error = crate::archive::cancel_import(&dest_layout, "0000000000000000", false)
        .expect_err("the present journal's real id does not match this one");
    assert!(error.to_string().contains("no import with id"), "{error}");
}

/// RFC 155 Part I-B2, review finding IB1(b): `acquire_container_locks` now refuses while a
/// journal is present, so `archive::export`'s own call (never named in the review's own
/// exemption list, unlike the import/cancel/recovery call sites) refuses too -- deliberately, not
/// by oversight. Exporting a repository mid-import would read whatever the import has landed so
/// far, which a cancel might still cut away: not the consistent, settled snapshot §9.3 promises.
#[test]
fn export_refuses_while_an_import_journal_is_present_and_succeeds_once_it_is_gone() {
    let source_dir = unique_temp_dir("archive-ib2-export-refuse-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib2-export-refuse-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    crate::archive::import::stop_after_journal_for_test();
    let mut cursor = Cursor::new(bytes.clone());
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("the test seam stops the import right after the journal's own write");
    let id = interrupted_import_short_id(&dest_layout)
        .expect("read journal state")
        .expect("a journal is present after the simulated kill");

    let mut export_buffer = Vec::new();
    let export_err = export_archive(&dest_layout, &mut export_buffer)
        .expect_err("archive export must refuse while an import journal is present");
    assert!(export_err.to_string().contains(&id), "{export_err}");
    assert!(
        export_buffer.is_empty(),
        "a refusal must write nothing, not even a partial header"
    );

    // Resume, finishing the import and emptying the journal -- export must now succeed.
    let mut cursor2 = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor2, &ArchiveImportOptions::default())
        .expect("resume finishes the import");
    let mut export_buffer2 = Vec::new();
    export_archive(&dest_layout, &mut export_buffer2)
        .expect("archive export succeeds once the journal is gone");
}

/// RFC 155 Part I-B2, review finding IB2: every kill this module's own test seams exercise
/// returns an `Err`, so `Drop` releases every lock before `doctor` ever runs -- `doctor`'s own
/// `PRIKK-DOCTOR-INTERRUPTED-IMPORT` issue must therefore name only resume/cancel here, with no
/// mention of `prikk unlock`. A lock file left behind by something that does not release on drop
/// (a real `SIGKILL`, simulated here the same way `unlock`'s own tests simulate a dead process --
/// a lock body naming a `pid=` astronomically unlikely to exist) changes that recommendation to
/// name `prikk unlock` first.
#[test]
fn doctor_names_unlock_first_only_when_a_lock_from_the_import_appears_abandoned() {
    let source_dir = unique_temp_dir("archive-ib2-doctor-unlock-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib2-doctor-unlock-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    crate::archive::import::stop_after_journal_for_test();
    let mut cursor = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("the test seam stops the import right after the journal's own write");

    let before = crate::doctor_repository(&dest_layout);
    let issue = before
        .issues
        .iter()
        .find(|issue| issue.code == "PRIKK-DOCTOR-INTERRUPTED-IMPORT")
        .expect("the interrupted-import issue is present");
    assert!(
        !issue.recommendation.contains("prikk unlock"),
        "a clean seam releases every lock on drop, so there is nothing to unlock yet: {}",
        issue.recommendation
    );
    assert!(issue.recommendation.contains("run the same import again"));

    // A lock file that, for whatever reason, is present and does not appear to belong to a
    // running process -- the same fabricated-dead-pid technique `unlock`'s own tests use.
    std::fs::write(
        dest_layout.default_active_lock_path(),
        "pid=999999\nkind=active\nnote=test\n",
    )
    .expect("write a fabricated stale lock");

    let after = crate::doctor_repository(&dest_layout);
    let issue = after
        .issues
        .iter()
        .find(|issue| issue.code == "PRIKK-DOCTOR-INTERRUPTED-IMPORT")
        .expect("the interrupted-import issue is still present");
    assert!(
        issue.recommendation.contains("prikk unlock"),
        "a lock that does not appear to be running must be named first: {}",
        issue.recommendation
    );
    assert!(issue.recommendation.contains("run the same import again"));
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
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("import completes normally");

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

/// RFC 155 Part I-B2, review finding IB1(a): the ref-side container locks (`RefPointerIndex`,
/// `RefLog`, `ReceivedIndex`) are now held structurally through stage 3, not just by inspection, on
/// top of `ObjectStore`. A callback runs right before stage 3's own first landing, while all four
/// container locks are genuinely held, and attempts a concurrent, wholly unrelated `publish`
/// through an ordinary `RefStore` handle (never through this import's own). The published ref's
/// own `RefState` object is written to the destination *before* the import starts (while nothing
/// holds any lock yet), so the concurrent `publish` call's own `write_object` call is a no-op
/// (`AlreadyPresent`, never touching `ObjectStore`) and the conflict this test actually proves is
/// on `RefPointerIndex`/`RefLog`, not a re-run of IA1(a)'s own `ObjectStore` test.
#[test]
fn concurrent_publish_refuses_while_stage_3_holds_the_ref_side_locks() {
    let source_dir = unique_temp_dir("archive-ib2-ib1a-source");
    let (source_layout, _ref_state_id, maintainer_key_id, maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib2-ib1a-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    crate::add_trusted_maintainer(&dest_layout, &maintainer_key_id, &maintainer_public_key_hex)
        .expect("adopt the maintainer key at the destination before import");

    // A second, wholly unrelated ref's own RefState object, written at the destination before the
    // import ever starts -- so the concurrent `publish` below finds it already present and never
    // touches the `ObjectStore` lock itself.
    let other_ref_state_payload = RefStatePayload {
        ref_name: "heads/other".to_string(),
        kind: RefKind::Branch,
        target_object_id: _ref_state_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let other_ref_state_bytes = other_ref_state_payload
        .to_canonical_bytes()
        .expect("encode other ref-state payload");
    let mut other_ref_state_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, other_ref_state_bytes);
    let other_ref_state_id = other_ref_state_envelope.object_id();
    // `publish`'s own `validate_publication` requires a non-empty signature on both envelopes
    // (`require_signed_type`) -- any real signature does, since `publish` itself never checks
    // whether the signer is adopted (that is only `--adopt`'s own `verify_trusted_publication_
    // envelope`, not exercised by this ordinary, unrelated probe).
    let probe_signer = Ed25519MaintainerSigner::from_seed("concurrent-publish-probe", &[0x55; 32])
        .expect("probe signer");
    other_ref_state_envelope
        .add_signature(
            maintainer_signature(&probe_signer, ObjectType::RefState, other_ref_state_id)
                .expect("sign the probe's own ref state for real"),
        )
        .expect("attach the probe's own real signature");
    FileObjectStore::new(dest_layout.clone())
        .write_object(&other_ref_state_envelope)
        .expect("pre-write the concurrent probe's own RefState object");

    let other_ref_update_payload = prikk_object::RefUpdatePayload {
        ref_name: "heads/other".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: other_ref_state_id,
        new_target_object_id: _ref_state_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: maintainer_key_id.clone(),
    };
    let other_ref_update_bytes = other_ref_update_payload
        .to_canonical_bytes()
        .expect("encode other ref-update payload");
    let mut other_ref_update_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, other_ref_update_bytes);
    let other_ref_update_id = other_ref_update_envelope.object_id();
    other_ref_update_envelope
        .add_signature(
            maintainer_signature(&probe_signer, ObjectType::RefUpdate, other_ref_update_id)
                .expect("sign the probe's own ref update for real"),
        )
        .expect("attach the probe's own real signature");

    let concurrent_result: Arc<Mutex<Option<prikk_error::Result<ObjectId>>>> =
        Arc::new(Mutex::new(None));
    let probe_layout = dest_layout.clone();
    let probe_result = concurrent_result.clone();
    crate::archive::import::run_before_stage_3_for_test(move || {
        // A fresh `ObjectWriteSession`, not the default `FileObjectStore` -- `RefStore::publish`'s
        // own plain path always acquires `ObjectStore` unconditionally for the ref-state write
        // (`FileObjectStore::write_object` has no lock-free "already present" fast path at all,
        // unlike `ObjectWriteSession::write_object`'s own `decide_write`), which would make this
        // probe conflict on `ObjectStore` regardless of whatever this test is actually trying to
        // isolate -- the ref-side locks, not the (already proven, IA1(a)) object-store one. The
        // probe's own ref-state object was pre-written above, so `ObjectWriteSession`'s own
        // `AlreadyPresent` fast path means this call never touches `ObjectStore` at all, and
        // whatever it does touch is purely the ref-side containers this test is actually about.
        let mut probe_object_store =
            ObjectWriteSession::open(&probe_layout).expect("open a fresh object-write session");
        let outcome = RefStore::new(probe_layout).publish_with_object_store(
            &mut probe_object_store,
            &RefPublication {
                ref_name: "heads/other".to_string(),
                expected_previous_ref_state_id: None,
                ref_state: other_ref_state_envelope,
                ref_update: other_ref_update_envelope,
            },
        );
        *probe_result.lock().expect("lock") = Some(outcome);
    });

    let mut cursor = Cursor::new(bytes);
    import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import completes normally");

    let outcome = concurrent_result
        .lock()
        .expect("lock")
        .take()
        .expect("the callback ran before stage 3");
    let error = outcome.expect_err(
        "a concurrent publish must refuse with LockConflict while stage 3 holds the ref-side \
         container locks for the whole import",
    );
    assert!(
        matches!(error, prikk_error::PrikkError::LockConflict(_)),
        "expected LockConflict, got: {error:?}"
    );
}

/// RFC 155 Part I-B2, review finding IB1(b): `compact --pointer-index`/`--received-index`/
/// `--trust-policy` and `doctor --rebuild-pointer-index` each take only a container lock, with no
/// `ActiveLock` at all -- none of them passed through Part I-A2's own writer-refusal fix.
/// `acquire_container_locks` itself now refuses while a journal is present, so each of these must
/// refuse too, and must succeed again once the journal is gone.
#[test]
fn compact_and_rebuild_refuse_while_a_journal_is_present_and_succeed_once_it_is_gone() {
    let source_dir = unique_temp_dir("archive-ib2-ib1b-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib2-ib1b-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    crate::archive::import::stop_after_journal_for_test();
    let mut cursor = Cursor::new(bytes.clone());
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("the test seam stops the import right after the journal's own write");
    let id = interrupted_import_short_id(&dest_layout)
        .expect("read journal state")
        .expect("a journal is present after the simulated kill");

    let compact_pointer_err = crate::compact_ref_pointer_index(&dest_layout)
        .expect_err("compact --pointer-index must refuse while the journal is present");
    assert!(
        compact_pointer_err.to_string().contains(&id),
        "{compact_pointer_err}"
    );

    let compact_received_err = crate::compact_received_index(&dest_layout)
        .expect_err("compact --received-index must refuse while the journal is present");
    assert!(
        compact_received_err.to_string().contains(&id),
        "{compact_received_err}"
    );

    let compact_trust_err = crate::compact_trust_policy(&dest_layout)
        .expect_err("compact --trust-policy must refuse while the journal is present");
    assert!(
        compact_trust_err.to_string().contains(&id),
        "{compact_trust_err}"
    );

    let rebuild_err = crate::rebuild_pointer_index(&dest_layout)
        .expect_err("doctor --rebuild-pointer-index must refuse while the journal is present");
    assert!(rebuild_err.to_string().contains(&id), "{rebuild_err}");

    // Resume, finishing the import and emptying the journal -- every one of them must now succeed.
    let mut cursor2 = Cursor::new(bytes);
    import_archive(&dest_layout, &mut cursor2, &ArchiveImportOptions::default())
        .expect("resume finishes the import");
    assert!(
        interrupted_import_short_id(&dest_layout).unwrap().is_none(),
        "the commit point emptied the journal"
    );

    crate::compact_ref_pointer_index(&dest_layout)
        .expect("compact --pointer-index succeeds once the journal is gone");
    crate::compact_received_index(&dest_layout)
        .expect("compact --received-index succeeds once the journal is gone");
    crate::compact_trust_policy(&dest_layout)
        .expect("compact --trust-policy succeeds once the journal is gone");
    crate::rebuild_pointer_index(&dest_layout)
        .expect("doctor --rebuild-pointer-index succeeds once the journal is gone");
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
    let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
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
        let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
            .expect_err("the test seam stops strictly between two object writes");
        assert!(error.to_string().contains("simulated kill"), "{error}");
        interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .expect("a journal is present after the simulated kill");

        let mut cursor2 = Cursor::new(bytes.clone());
        let resumed = import_archive(&dest_layout, &mut cursor2, &ArchiveImportOptions::default())
            .expect("resume finishes");
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
        import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
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
        let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
            .expect_err("the test seam fails strictly between two object writes");
        assert!(
            matches!(error, prikk_error::PrikkError::Io { .. }),
            "expected a real i/o error, got: {error:?}"
        );
        interrupted_import_short_id(&dest_layout)
            .expect("read journal state")
            .expect("a journal is present after the simulated i/o failure");

        let mut cursor2 = Cursor::new(bytes.clone());
        let resumed = import_archive(&dest_layout, &mut cursor2, &ArchiveImportOptions::default())
            .expect("resume finishes");
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
        import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
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
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
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
    import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("import completes normally");

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

/// §9.6's narrowed `--adopt`, condition 2/3: a ref absent locally, signed by an adopted key, and
/// verified, lands as a real local ref -- not a received pointer.
#[test]
fn adopt_lands_a_ref_signed_by_an_adopted_key_as_a_real_local_branch() {
    let source_dir = unique_temp_dir("archive-ib-adopt-source");
    let (source_layout, ref_state_id, maintainer_key_id, maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib-adopt-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    crate::add_trusted_maintainer(&dest_layout, &maintainer_key_id, &maintainer_public_key_hex)
        .expect("adopt the maintainer key at the destination before import");

    let mut cursor = Cursor::new(bytes);
    let report = import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import with --adopt");

    assert_eq!(report.landing_outcomes.len(), 1);
    assert_eq!(report.landing_outcomes[0].ref_name, "heads/main");
    assert_eq!(report.landing_outcomes[0].landed_as, "heads/main");
    assert!(
        report.landing_outcomes[0].adopted,
        "a ref signed by an adopted, verified key must adopt: {:?}",
        report.landing_outcomes[0]
    );
    assert!(report.landing_outcomes[0].remote_reason.is_none());
    assert!(!report.already_imported);

    let dest_ref_store = RefStore::new(dest_layout.clone());
    assert_eq!(
        dest_ref_store
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state"),
        Some(ref_state_id),
        "heads/main must be a real local branch at the archive's own tip"
    );
    assert_eq!(
        crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
            .expect("read received pointer"),
        None,
        "a successfully adopted ref must not also land under remotes/"
    );
}

/// §9.6: every other ref goes to `remotes/`, with the reason named -- here, a key this
/// repository has not adopted.
#[test]
fn adopt_lands_a_ref_signed_by_a_non_adopted_key_under_remotes_instead() {
    let source_dir = unique_temp_dir("archive-ib-no-adopt-source");
    let (source_layout, ref_state_id, _maintainer_key_id, _maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib-no-adopt-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    // Deliberately never adopted at the destination.

    let mut cursor = Cursor::new(bytes);
    let report = import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import with --adopt");

    assert_eq!(report.landing_outcomes.len(), 1);
    assert!(!report.landing_outcomes[0].adopted);
    assert_eq!(report.landing_outcomes[0].landed_as, "remotes/heads/main");
    assert!(
        report.landing_outcomes[0].remote_reason.is_some(),
        "the fallback must name why"
    );

    let dest_ref_store = RefStore::new(dest_layout.clone());
    assert_eq!(
        dest_ref_store
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state"),
        None,
        "a non-adopted ref must never become a real local branch"
    );
    assert_eq!(
        crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
            .expect("read received pointer"),
        Some(crate::ReceivedPointer {
            ref_name: "remotes/heads/main".to_string(),
            ref_state_id,
        })
    );
}

/// §9.7: re-importing the same archive reports "already imported" -- a second, identical run
/// changes nothing (no new object, no merge, no new author key, no landing change).
#[test]
fn reimporting_the_same_archive_reports_already_imported() {
    let source_dir = unique_temp_dir("archive-ib-reimport-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib-reimport-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    let first = import_archive(
        &dest_layout,
        &mut Cursor::new(bytes.clone()),
        &ArchiveImportOptions::default(),
    )
    .expect("first import");
    assert!(
        !first.already_imported,
        "the first import of genuinely new content must not report already-imported"
    );

    let second = import_archive(
        &dest_layout,
        &mut Cursor::new(bytes),
        &ArchiveImportOptions::default(),
    )
    .expect("second import of the identical archive");
    assert!(
        second.already_imported,
        "a byte-identical re-import must report already-imported: {second:?}"
    );
    assert!(
        second
            .written_object_counts
            .iter()
            .all(|(_, count)| *count == 0)
    );
    assert!(
        second
            .merged_object_counts
            .iter()
            .all(|(_, count)| *count == 0)
    );
    assert!(second.dropped_signatures.is_empty());
}

/// §8's remaining controls, combined: a carried maintainer signature for an id already stored
/// merges in when its own key is adopted at the destination, and is dropped (named) when it is
/// not -- both over the exact same object id.
#[test]
fn a_carried_maintainer_signature_merges_when_adopted_and_drops_when_not() {
    let maintainer_a =
        Ed25519MaintainerSigner::from_seed("merge-drop-maintainer-a", &[0x41; 32]).expect("m-a");
    let maintainer_b =
        Ed25519MaintainerSigner::from_seed("merge-drop-maintainer-b", &[0x42; 32]).expect("m-b");
    let public_key_hex = |signer: &Ed25519MaintainerSigner| -> String {
        signer
            .public_key_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };

    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).expect("empty state root"),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_bytes = block_payload
        .to_canonical_bytes()
        .expect("encode block payload");

    // Archive 1: the block, signed only by maintainer A, published under a throwaway ref name so
    // its own landing never interacts with this test's real subject (object admission).
    let source_a_dir = unique_temp_dir("archive-ib-merge-drop-source-a");
    let source_a = RepositoryLayout::init(&source_a_dir).expect("init source a");
    crate::add_trusted_maintainer(
        &source_a,
        maintainer_a.key_id(),
        &public_key_hex(&maintainer_a),
    )
    .expect("adopt maintainer a at source a");
    let mut envelope_a = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes.clone());
    let block_id = envelope_a.object_id();
    envelope_a
        .add_signature(
            maintainer_signature(&maintainer_a, ObjectType::Block, block_id).expect("sign a"),
        )
        .expect("attach a's signature");
    FileObjectStore::new(source_a.clone())
        .write_object(&envelope_a)
        .expect("write block under maintainer a alone");
    let bytes_a = export_bytes(&source_a);

    // Archive 2: the *same* block id, now carrying both A's and B's signatures. The source for
    // this archive adopts both, so its own export's carried MaintainerKeys/TrustPolicy sections
    // resolve both signatures (stage 0's own `verify_archive` would otherwise refuse it).
    let source_b_dir = unique_temp_dir("archive-ib-merge-drop-source-b");
    let source_b = RepositoryLayout::init(&source_b_dir).expect("init source b");
    crate::add_trusted_maintainer(
        &source_b,
        maintainer_a.key_id(),
        &public_key_hex(&maintainer_a),
    )
    .expect("adopt maintainer a at source b");
    crate::add_trusted_maintainer(
        &source_b,
        maintainer_b.key_id(),
        &public_key_hex(&maintainer_b),
    )
    .expect("adopt maintainer b at source b");
    let mut envelope_ab = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes);
    assert_eq!(envelope_ab.object_id(), block_id, "same content, same id");
    envelope_ab
        .add_signature(
            maintainer_signature(&maintainer_a, ObjectType::Block, block_id).expect("sign a"),
        )
        .expect("attach a's signature");
    envelope_ab
        .add_signature(
            maintainer_signature(&maintainer_b, ObjectType::Block, block_id).expect("sign b"),
        )
        .expect("attach b's signature");
    FileObjectStore::new(source_b.clone())
        .write_object(&envelope_ab)
        .expect("write block under both maintainers");
    let bytes_ab = export_bytes(&source_b);

    // Destination adopts only A to start.
    let dest_dir = unique_temp_dir("archive-ib-merge-drop-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    crate::add_trusted_maintainer(
        &dest_layout,
        maintainer_a.key_id(),
        &public_key_hex(&maintainer_a),
    )
    .expect("adopt maintainer a at the destination");

    import_archive(
        &dest_layout,
        &mut Cursor::new(bytes_a),
        &ArchiveImportOptions::default(),
    )
    .expect("first import establishes the block under maintainer a alone");

    // B is not yet adopted: its signature must be dropped and named; nothing merges.
    let second = import_archive(
        &dest_layout,
        &mut Cursor::new(bytes_ab.clone()),
        &ArchiveImportOptions::default(),
    )
    .expect("second import");
    assert!(
        second
            .merged_object_counts
            .iter()
            .all(|(_, count)| *count == 0),
        "nothing qualifies to merge while b is not adopted: {:?}",
        second.merged_object_counts
    );
    assert_eq!(
        second.dropped_signatures.len(),
        1,
        "b's signature must be dropped and named: {:?}",
        second.dropped_signatures
    );
    assert_eq!(second.dropped_signatures[0].key_id, maintainer_b.key_id());
    assert_eq!(second.dropped_signatures[0].object_id, block_id);
    let stored_after_second = FileObjectStore::new(dest_layout.clone())
        .read_object(block_id)
        .expect("read")
        .expect("stored");
    assert_eq!(
        stored_after_second.signatures.len(),
        1,
        "b's dropped signature must never be attached"
    );

    // Now adopt b too, and re-import the same carrying archive: b's signature now qualifies and
    // merges in.
    crate::add_trusted_maintainer(
        &dest_layout,
        maintainer_b.key_id(),
        &public_key_hex(&maintainer_b),
    )
    .expect("adopt maintainer b at the destination");
    let third = import_archive(
        &dest_layout,
        &mut Cursor::new(bytes_ab),
        &ArchiveImportOptions::default(),
    )
    .expect("third import");
    assert!(
        third
            .merged_object_counts
            .iter()
            .any(|(object_type, count)| *object_type == ObjectType::Block && *count == 1),
        "b's signature must now merge in: {:?}",
        third.merged_object_counts
    );
    assert!(third.dropped_signatures.is_empty());
    let stored_after_third = FileObjectStore::new(dest_layout.clone())
        .read_object(block_id)
        .expect("read")
        .expect("stored");
    assert_eq!(
        stored_after_third.signatures.len(),
        2,
        "both signatures must now be stored"
    );
}

/// The full carry-forward story (§5/§9's own "whole repository, verbatim" promise), end to end:
/// export from repository 1; `init` repository 2; verify lists the maintainer keys; `trust
/// maintainer add`; `import --adopt`; every branch is a branch; verify exits 0. Doubles as the
/// handoff's own required round-trip proof: `branch list` and the object ids are identical.
#[test]
fn carry_forward_story_end_to_end_and_round_trip() {
    let source_dir = unique_temp_dir("archive-ib-carry-forward-source");
    let (source_layout, ref_state_id, maintainer_key_id, maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let source_object_ids: std::collections::BTreeSet<ObjectId> = {
        let store = FileObjectStore::new(source_layout.clone());
        [RefStore::new(source_layout.clone())
            .read_current_ref_state_id("heads/main")
            .expect("source current ref state")
            .expect("heads/main exists at the source")]
        .into_iter()
        .filter(|id| store.contains_object(ObjectType::RefState, *id))
        .collect()
    };
    let bytes = export_bytes(&source_layout);

    // `init` repository 2.
    let dest_dir = unique_temp_dir("archive-ib-carry-forward-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init repository 2");

    // verify lists the maintainer keys.
    let verify_report = {
        let mut cursor = Cursor::new(bytes.clone());
        crate::archive::verify_archive(&mut cursor).expect("verify")
    };
    assert!(verify_report.is_internally_consistent());
    assert!(
        verify_report
            .maintainer_keys
            .iter()
            .any(|key| key.key_id == maintainer_key_id),
        "verify must list the archive's own carried maintainer key: {:?}",
        verify_report.maintainer_keys
    );

    // trust maintainer add.
    crate::add_trusted_maintainer(&dest_layout, &maintainer_key_id, &maintainer_public_key_hex)
        .expect("trust maintainer add");

    // import --adopt.
    let import_report = import_archive(
        &dest_layout,
        &mut Cursor::new(bytes),
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import --adopt");

    // every branch is a branch.
    assert!(import_report.landing_outcomes[0].adopted);
    let dest_ref_store = RefStore::new(dest_layout.clone());
    let dest_pointers = dest_ref_store.list_ref_pointers().expect("list branches");
    assert_eq!(dest_pointers.len(), 1);
    assert_eq!(dest_pointers[0].ref_name, "heads/main");
    assert_eq!(dest_pointers[0].ref_state_id, ref_state_id);

    // The round trip: `branch list` and the object ids are identical.
    let source_pointers = RefStore::new(source_layout.clone())
        .list_ref_pointers()
        .expect("list source branches");
    assert_eq!(
        source_pointers, dest_pointers,
        "branch list must round-trip"
    );
    let dest_store = FileObjectStore::new(dest_layout.clone());
    for id in &source_object_ids {
        assert!(
            dest_store.contains_object(ObjectType::RefState, *id),
            "every source object id must round-trip to the destination"
        );
    }

    // Whether `prikk verify` itself would exit 0 is a `prikk-cli`-level question (its own
    // `VERDICT_CONDITIONS`, nine conditions, not a `prikk-store` predicate -- RFC 118 stage 5
    // deliberately removed the single-predicate shortcut this test could otherwise have reached
    // for). That real claim is checked by the release-build smoke test instead
    // (`carry_forward_story_smoke`, `crates/prikk-cli/tests/`), which runs the actual `prikk
    // verify` command and reads its real exit code. Here, at the library level, the two
    // structural signals `prikk-store` itself can answer cleanly: no stage failed outright, and
    // no individual item failed within a stage that did.
    let repository_verification =
        crate::verify_repository(&dest_layout).expect("verify the destination repository");
    assert!(
        !repository_verification.has_stage_failure(),
        "no verification stage must fail after the carry-forward story: {repository_verification:?}"
    );
    assert!(
        !repository_verification.has_item_failure(),
        "no individual item must fail after the carry-forward story: {repository_verification:?}"
    );
}

/// §9.6's condition 1: a ref signed by an adopted, verified key, but already present locally,
/// still falls through to `remotes/` -- adopting onto an existing ref is RFC 154's, not this
/// part's.
#[test]
fn adopt_over_an_existing_local_ref_falls_through_to_remotes_instead_of_overwriting_it() {
    let source_dir = unique_temp_dir("archive-ib-adopt-existing-source");
    let (source_layout, ref_state_id, maintainer_key_id, maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ib-adopt-existing-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    crate::add_trusted_maintainer(&dest_layout, &maintainer_key_id, &maintainer_public_key_hex)
        .expect("adopt the maintainer key at the destination before import");

    // The destination already has its own, unrelated `heads/main` before the import ever runs.
    let existing_block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).expect("empty state root"),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let existing_block_bytes = existing_block_payload
        .to_canonical_bytes()
        .expect("encode block payload");
    let mut existing_block_envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, existing_block_bytes);
    let existing_block_id = existing_block_envelope.object_id();
    let dest_maintainer =
        Ed25519MaintainerSigner::from_seed("adopt-existing-dest-maintainer", &[0x44; 32])
            .expect("dest maintainer signer");
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
    .expect("adopt the destination's own maintainer key");
    existing_block_envelope
        .add_signature(
            maintainer_signature(&dest_maintainer, ObjectType::Block, existing_block_id)
                .expect("sign the destination's own existing block for real"),
        )
        .expect("attach the real maintainer signature");
    FileObjectStore::new(dest_layout.clone())
        .write_object(&existing_block_envelope)
        .expect("write the destination's own existing block");

    let existing_ref_state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: existing_block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let existing_ref_state_bytes = existing_ref_state_payload
        .to_canonical_bytes()
        .expect("encode ref-state payload");
    let mut existing_ref_state_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, existing_ref_state_bytes);
    let existing_ref_state_id = existing_ref_state_envelope.object_id();
    existing_ref_state_envelope
        .add_signature(
            maintainer_signature(
                &dest_maintainer,
                ObjectType::RefState,
                existing_ref_state_id,
            )
            .expect("sign the destination's own existing ref state for real"),
        )
        .expect("attach the real maintainer signature");
    let existing_ref_update_envelope = {
        let payload = prikk_object::RefUpdatePayload {
            ref_name: "heads/main".to_string(),
            old_ref_state_id: None,
            new_ref_state_id: existing_ref_state_id,
            new_target_object_id: existing_block_id,
            update_seq: 1,
            created_at: 0,
            author_key_id: dest_maintainer.key_id().to_string(),
        };
        let bytes = payload.to_canonical_bytes().expect("encode ref update");
        let mut envelope = ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, bytes);
        let id = envelope.object_id();
        envelope
            .add_signature(
                maintainer_signature(&dest_maintainer, ObjectType::RefUpdate, id)
                    .expect("sign the destination's own existing ref update for real"),
            )
            .expect("attach signature");
        envelope
    };
    RefStore::new(dest_layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: existing_ref_state_envelope,
            ref_update: existing_ref_update_envelope,
        })
        .expect("publish the destination's own existing heads/main");

    let mut cursor = Cursor::new(bytes);
    let report = import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import with --adopt over an existing ref must not fail the whole import");

    assert_eq!(report.landing_outcomes.len(), 1);
    assert!(!report.landing_outcomes[0].adopted);
    assert_eq!(report.landing_outcomes[0].landed_as, "remotes/heads/main");
    assert!(report.landing_outcomes[0].remote_reason.is_some());

    let dest_ref_store = RefStore::new(dest_layout.clone());
    assert_eq!(
        dest_ref_store
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state"),
        Some(existing_ref_state_id),
        "the destination's own existing heads/main must be untouched"
    );
    assert_eq!(
        crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
            .expect("read received pointer"),
        Some(crate::ReceivedPointer {
            ref_name: "remotes/heads/main".to_string(),
            ref_state_id,
        })
    );
}

/// External review 023's own shape: the source repository carries, for the *same* ref name, both
/// its own live local ref (from [`build_adoptable_source_repository`]) and a carried received
/// pointer at `remotes/<ref_name>` -- as if this source repository itself had once `bundle
/// import`ed something for that name. A real second `Block`+`RefState` pair, maintainer-signed by
/// the fixture's own already-adopted key (so export's own `verify` resolves it, not a placeholder
/// signature), is the received pointer's own target, distinct from the exporter's own tip -- so a
/// test can tell, after import, which one actually won. Returns the planted `RefState` id.
fn plant_colliding_received_pointer(layout: &RepositoryLayout, ref_name: &str) -> ObjectId {
    // The same deterministic seed `build_adoptable_source_repository` used -- already adopted at
    // this source, so this second `RefState`'s own signature resolves too, without a second trust
    // call.
    let maintainer_signer =
        Ed25519MaintainerSigner::from_seed("adopt-test-maintainer", &[0x33; 32])
            .expect("reconstruct the same maintainer signer");

    let mut store = FileObjectStore::new(layout.clone());
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        // A distinct state root keeps this block's own id different from the exporter's own root
        // block, which otherwise carries an identical payload.
        state_merkle_root: MerkleRoot([0xAB; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_bytes = block_payload
        .to_canonical_bytes()
        .expect("encode the colliding block payload");
    let mut block_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes);
    let block_id = block_envelope.object_id();
    block_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::Block, block_id)
                .expect("sign the colliding block for real"),
        )
        .expect("attach the real maintainer signature");
    store
        .write_object(&block_envelope)
        .expect("write the colliding block");

    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_bytes = ref_state_payload
        .to_canonical_bytes()
        .expect("encode the colliding ref-state payload");
    let mut ref_state_envelope = ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state_bytes);
    let ref_state_id = ref_state_envelope.object_id();
    ref_state_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::RefState, ref_state_id)
                .expect("sign the colliding ref state for real"),
        )
        .expect("attach the real maintainer signature");
    store
        .write_object(&ref_state_envelope)
        .expect("write the colliding ref state");

    crate::received::write_received_pointer(layout, &format!("remotes/{ref_name}"), ref_state_id)
        .expect("plant the colliding received pointer");
    ref_state_id
}

/// External review 023: by default (no `--adopt`), the exporter's own local ref must win the
/// `remotes/<ref>` name over a carried received pointer that collides with it -- the received
/// pointer is named, not landed, and its own objects were still imported by stage 1.
#[test]
fn landing_collision_lets_the_exporters_own_ref_win_by_default() {
    let source_dir = unique_temp_dir("archive-ic-collision-source-default");
    let (source_layout, exporter_ref_state_id, _maintainer_key_id, _maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let received_ref_state_id = plant_colliding_received_pointer(&source_layout, "heads/main");

    let bytes = export_bytes(&source_layout);
    let mut verify_cursor = Cursor::new(bytes.clone());
    assert!(
        crate::archive::verify_archive(&mut verify_cursor)
            .expect("verify")
            .is_internally_consistent(),
        "the fixture's own two maintainer signatures must both resolve, or this test proves \
         nothing"
    );

    let dest_dir = unique_temp_dir("archive-ic-collision-dest-default");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let mut cursor = Cursor::new(bytes);
    let report = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("import");

    assert_eq!(report.landing_outcomes.len(), 2);
    let destination = "remotes/heads/main";
    let winner = report
        .landing_outcomes
        .iter()
        .find(|outcome| outcome.landed)
        .expect("exactly one entry must actually land at the collision destination");
    assert_eq!(winner.ref_name, "heads/main");
    assert_eq!(winner.landed_as, destination);

    let loser = report
        .landing_outcomes
        .iter()
        .find(|outcome| !outcome.landed)
        .expect("the colliding received pointer must be named, not silently dropped");
    assert_eq!(loser.ref_name, destination);
    assert_eq!(loser.collision_target, Some(received_ref_state_id));
    assert!(loser.remote_reason.is_some());

    let actual = crate::received::read_received_pointer(&dest_layout, destination)
        .expect("read")
        .expect("something must actually be on disk at the collision destination");
    assert_eq!(
        actual.ref_state_id, exporter_ref_state_id,
        "the exporter's own ref must be what actually landed, never the colliding received \
         pointer"
    );

    // Stage 1 is unaffected by stage 3's landing decision: both RefState objects, including the
    // one the colliding, unlanded received pointer named, are present.
    let object_store = FileObjectStore::new(dest_layout.clone());
    assert!(
        object_store
            .read_object(received_ref_state_id)
            .expect("read")
            .is_some(),
        "the colliding received pointer's own object must still be imported, even though its \
         pointer was not landed"
    );
}

/// External review 023: a successful `--adopt` lands the exporter's own ref as a real local
/// branch (a bare name, never `remotes/`-shaped), so there is no collision at all -- the carried
/// received pointer lands normally, exactly as it would without any collision in the picture.
#[test]
fn landing_collision_is_not_a_collision_at_all_once_adopt_succeeds() {
    let source_dir = unique_temp_dir("archive-ic-collision-source-adopt-ok");
    let (source_layout, exporter_ref_state_id, maintainer_key_id, maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let received_ref_state_id = plant_colliding_received_pointer(&source_layout, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ic-collision-dest-adopt-ok");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    crate::add_trusted_maintainer(&dest_layout, &maintainer_key_id, &maintainer_public_key_hex)
        .expect("adopt the maintainer key at the destination before import");

    let mut cursor = Cursor::new(bytes);
    let report = import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import with --adopt");

    let adopted = report
        .landing_outcomes
        .iter()
        .find(|outcome| outcome.ref_name == "heads/main")
        .expect("the exporter's own ref must be in the report");
    assert!(adopted.adopted, "it must actually adopt: {adopted:?}");
    assert_eq!(adopted.landed_as, "heads/main");

    let received = report
        .landing_outcomes
        .iter()
        .find(|outcome| outcome.ref_name == "remotes/heads/main")
        .expect("the received pointer must be in the report");
    assert!(
        received.landed,
        "a bare-named adopted ref never collides with a remotes/-shaped received pointer: \
         {received:?}"
    );

    let dest_ref_store = RefStore::new(dest_layout.clone());
    assert_eq!(
        dest_ref_store
            .read_current_ref_state_id("heads/main")
            .expect("read"),
        Some(exporter_ref_state_id)
    );
    assert_eq!(
        crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
            .expect("read")
            .map(|pointer| pointer.ref_state_id),
        Some(received_ref_state_id),
        "the received pointer lands normally once adoption removes the collision"
    );
}

/// External review 023: `--adopt` requested but the signer not adopted at the destination still
/// falls through to the default `remotes/<ref>` landing -- which still collides, and still wins,
/// the same way the plain default case does.
#[test]
fn landing_collision_still_wins_when_adopt_falls_through() {
    let source_dir = unique_temp_dir("archive-ic-collision-source-adopt-fallthrough");
    let (source_layout, exporter_ref_state_id, _maintainer_key_id, _maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    plant_colliding_received_pointer(&source_layout, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ic-collision-dest-adopt-fallthrough");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    // Deliberately never adopted at the destination.

    let mut cursor = Cursor::new(bytes);
    let report = import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect("import with --adopt");

    let fell_through = report
        .landing_outcomes
        .iter()
        .find(|outcome| outcome.ref_name == "heads/main")
        .expect("the exporter's own ref must be in the report");
    assert!(!fell_through.adopted);
    assert_eq!(fell_through.landed_as, "remotes/heads/main");
    assert!(fell_through.landed);

    let loser = report
        .landing_outcomes
        .iter()
        .find(|outcome| !outcome.landed)
        .expect("the colliding received pointer must be named, not silently dropped");
    assert_eq!(loser.ref_name, "remotes/heads/main");

    let actual = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read")
        .expect("something must be on disk");
    assert_eq!(actual.ref_state_id, exporter_ref_state_id);
}

/// External review 023's own "also": the plan lists a ref at most once, from its own latest
/// entry -- the exporter's own container held two pointer-index entries for the same ref (two
/// `seal`s advanced it), which the plan must fold to one, not two.
#[test]
fn landing_plan_lists_a_ref_at_most_once_even_after_two_seals() {
    let source_dir = unique_temp_dir("archive-ic-collision-source-dedup");
    let (source_layout, _first_ref_state_id, _maintainer_key_id, _maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");

    // A second, real advance of the same ref -- the exporter's own live container now holds two
    // pointer-index entries for `heads/main`, the superseded one and the new tip.
    let maintainer_signer =
        Ed25519MaintainerSigner::from_seed("adopt-test-maintainer", &[0x33; 32])
            .expect("reconstruct the same maintainer signer");
    let mut store = FileObjectStore::new(source_layout.clone());
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([0xCD; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_bytes = block_payload
        .to_canonical_bytes()
        .expect("encode the second block payload");
    let mut block_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes);
    let block_id = block_envelope.object_id();
    block_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::Block, block_id)
                .expect("sign the second block for real"),
        )
        .expect("attach the real maintainer signature");
    store
        .write_object(&block_envelope)
        .expect("write the second block");

    let current_ref_state_id = RefStore::new(source_layout.clone())
        .read_current_ref_state_id("heads/main")
        .expect("read")
        .expect("heads/main must already exist from the fixture");

    let ref_state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq: 2,
        previous_ref_state_id: Some(current_ref_state_id),
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_bytes = ref_state_payload
        .to_canonical_bytes()
        .expect("encode the second ref-state payload");
    let mut ref_state_envelope = ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state_bytes);
    let second_ref_state_id = ref_state_envelope.object_id();
    ref_state_envelope
        .add_signature(
            maintainer_signature(
                &maintainer_signer,
                ObjectType::RefState,
                second_ref_state_id,
            )
            .expect("sign the second ref state for real"),
        )
        .expect("attach the real maintainer signature");

    let ref_update_payload = prikk_object::RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: Some(current_ref_state_id),
        new_ref_state_id: second_ref_state_id,
        new_target_object_id: block_id,
        update_seq: 2,
        created_at: 0,
        author_key_id: maintainer_signer.key_id().to_string(),
    };
    let ref_update_bytes = ref_update_payload
        .to_canonical_bytes()
        .expect("encode the second ref-update payload");
    let mut ref_update_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, ref_update_bytes);
    let ref_update_id = ref_update_envelope.object_id();
    ref_update_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::RefUpdate, ref_update_id)
                .expect("sign the second ref update for real"),
        )
        .expect("attach the real maintainer signature");

    RefStore::new(source_layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: Some(current_ref_state_id),
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        })
        .expect("publish the second advance");

    let bytes = export_bytes(&source_layout);
    let dest_dir = unique_temp_dir("archive-ic-collision-dest-dedup");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let mut cursor = Cursor::new(bytes);
    let report = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("import");

    assert_eq!(
        report.landing_outcomes.len(),
        1,
        "a ref advanced twice must still be planned once, from its own latest entry: {:?}",
        report.landing_outcomes
    );
    assert_eq!(report.landing_outcomes[0].ref_name, "heads/main");

    let dest_ref_store = RefStore::new(dest_layout.clone());
    assert_eq!(
        dest_ref_store
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state"),
        None,
        "no --adopt was requested, so heads/main must not become a real local branch"
    );
    assert_eq!(
        crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
            .expect("read")
            .map(|pointer| pointer.ref_state_id),
        Some(second_ref_state_id),
        "the landed pointer must be the ref's own latest entry, not the superseded one"
    );
}

/// Control, run and reverted (RFC 155 §9.6, external review 023): disabling the collision rule
/// reproduces the exact bug -- the received pointer, landed in plan order with no check, silently
/// overwrites the exporter's own ref at the same name.
#[test]
fn control_disabling_the_collision_rule_lets_the_received_pointer_overwrite_the_exporters_ref() {
    let source_dir = unique_temp_dir("archive-ic-collision-control-source");
    let (source_layout, exporter_ref_state_id, _maintainer_key_id, _maintainer_public_key_hex) =
        build_adoptable_source_repository(&source_dir, "heads/main");
    let received_ref_state_id = plant_colliding_received_pointer(&source_layout, "heads/main");
    let bytes = export_bytes(&source_layout);

    let dest_dir = unique_temp_dir("archive-ic-collision-control-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    crate::archive::import::disable_landing_collision_rule_for_test();
    let mut cursor = Cursor::new(bytes);
    // 0.56.0 step 3c: within this one import, the `Local` landing writes `remotes/heads/main`
    // first (the exporter's own ref, absent `--adopt`); the planted, genuinely unrelated
    // `Received` entry then lands at the identical destination second (the collision rule is
    // disabled here, on purpose, to reproduce the old bug) -- against the pointer the first
    // landing just wrote, that is a `does-not-connect`, which this round's own new refusal would
    // otherwise catch. `--allow-non-fast-forward` keeps this control testing only what it is
    // for (the disabled collision rule, not connectivity).
    let report = import_archive(
        &dest_layout,
        &mut cursor,
        &ArchiveImportOptions {
            allow_non_fast_forward: true,
            ..ArchiveImportOptions::default()
        },
    )
    .expect("import");

    assert!(
        report.landing_outcomes.iter().all(|outcome| outcome.landed),
        "with the collision rule disabled, nothing is skipped -- both land, in plan order"
    );

    let actual = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read")
        .expect("something is on disk");
    assert_eq!(
        actual.ref_state_id, received_ref_state_id,
        "the control's own real value: with the rule disabled, the received pointer -- landed \
         last, in plan order -- silently overwrites the exporter's own ref {exporter_ref_state_id}, \
         exactly the bug external review 023 found"
    );
}

/// §9.1, amended 2026-10-12 (review `rfc155-impl-IC-review-v1`): this fixture's own construction
/// (shrinking a record's own declared body length without fixing *that record's own* inner
/// checksum) produces a **complete frame whose checksum fails** -- damage, which still refuses --
/// not a pure "tail with everything else sound" shape (the section's own *outer* checksum is
/// recomputed to match, but the shrunk record's own checksum is not, deliberately, since this is
/// what actually makes the refusal attributable to something real rather than the premise I-C's
/// own fix got wrong). See `archive::verify::tests::section_frame_classification`'s own suite for
/// the shapes that are genuinely *only* an interrupted append, which import does **not** refuse on
/// (the `pure_interrupted_append...` test below).
#[test]
fn import_refuses_an_archive_whose_complete_record_fails_its_own_checksum() {
    let source_dir = unique_temp_dir("archive-ic-tail-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let mut bytes = export_bytes(&source_layout);

    let (_, _, entries) = crate::archive::tests::decode_manifest_for_test(&bytes);
    let block_entry = entries
        .iter()
        .find(|(kind, ..)| {
            *kind
                == crate::archive::ArchiveSectionKind::ObjectContainer(ObjectType::Block)
                    .wire_code()
        })
        .expect("block section present");
    let (_, offset, length, _) = *block_entry;
    let offset = offset as usize;
    let length = length as usize;

    const SECTION_HEADER_LEN: usize = 18;
    const CONTAINER_HEADER_LEN: usize = 8 + 2 + 8 + 32;
    const TAIL_LEN: usize = 16;

    let record_header_start = offset + SECTION_HEADER_LEN;
    let declared_len_field = record_header_start + 10..record_header_start + 18;
    let real_body_len = u64::from_be_bytes(
        bytes[declared_len_field.clone()]
            .try_into()
            .expect("8 bytes"),
    ) as usize;
    assert_eq!(
        length,
        CONTAINER_HEADER_LEN + real_body_len,
        "this fixture's block section must hold exactly one record, filling it"
    );
    assert!(real_body_len > TAIL_LEN);

    let new_body_len = (real_body_len - TAIL_LEN) as u64;
    bytes[declared_len_field].copy_from_slice(&new_body_len.to_be_bytes());

    let section_checksum_at = offset + SECTION_HEADER_LEN + length;
    let mut hasher = prikk_hash::IncrementalSha256::new();
    hasher.update(&bytes[offset..offset + SECTION_HEADER_LEN + length]);
    let recomputed = hasher.finalize();
    bytes[section_checksum_at..section_checksum_at + 32].copy_from_slice(&recomputed);

    let dest_dir = unique_temp_dir("archive-ic-tail-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let fingerprint_before = directory_fingerprint(&dest_dir);

    let mut cursor = Cursor::new(bytes);
    let error = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect_err("a tail inside a section must refuse the whole import");
    assert!(
        error.to_string().contains("not internally consistent"),
        "{error}"
    );
    assert_eq!(
        fingerprint_before,
        directory_fingerprint(&dest_dir),
        "a refused import must write nothing"
    );
}

/// §9.1, amended 2026-10-12: a *pure* interrupted append (no other damage) must never refuse the
/// import -- the source repository's own `prikk verify` only ever warns about it, so a backup of
/// that repository must still restore. Built by inserting raw bytes inside a section, never by
/// shrinking an existing record's own declared length (which, as the sibling test above shows,
/// introduces a genuine checksum failure instead).
#[test]
fn pure_interrupted_append_inside_a_section_does_not_refuse_the_import() {
    let source_dir = unique_temp_dir("archive-id-pure-tail-source");
    let (source_layout, _block_id, _author_key_id) = build_signed_source_repository(&source_dir);
    let mut bytes = export_bytes(&source_layout);
    crate::archive::tests::insert_tail_bytes_into_section_for_test(
        &mut bytes,
        crate::archive::ArchiveSectionKind::ObjectContainer(ObjectType::Block).wire_code(),
        &[0u8; 100],
    );

    let dest_dir = unique_temp_dir("archive-id-pure-tail-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    let mut cursor = Cursor::new(bytes);
    let report = import_archive(&dest_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("a pure interrupted append must not refuse the import");
    assert_eq!(
        report
            .written_object_counts
            .iter()
            .find(|(object_type, _)| *object_type == ObjectType::Block)
            .map(|(_, count)| *count),
        Some(1),
        "the one real block must still be written"
    );
}

/// N1 (review `rfc155-impl-IC-review-v1`), the full end-to-end scenario the finding is about: a
/// repository that `bundle import`s another repository's ref **without ever trusting its signing
/// key** (the default, documented outcome) must still produce an archive that can be verified and
/// imported *anywhere* -- including into a destination that already trusts the key, and including
/// a destination that initially does not. Before this fix, `archive verify` refused the whole
/// archive on exactly this untrusted signature, making it unimportable everywhere, forever.
#[test]
fn an_untrusted_received_signature_never_blocks_verify_or_import_and_the_destination_decides_trust()
{
    // "upstream": its own real maintainer key, a real `heads/main`.
    let upstream_dir = unique_temp_dir("archive-n1-upstream");
    let (upstream_layout, upstream_block_id, _) = build_signed_source_repository(&upstream_dir);
    let (_report, bundle_bytes) = crate::bundle::export_bundle(&upstream_layout, "heads/main")
        .expect("export a bundle of upstream's own heads/main");

    // "a": the archive's own source. Receives upstream's bundle, but never trusts its key -- the
    // default, documented `bundle import` outcome. "a" never adopts upstream's maintainer key.
    let a_dir = unique_temp_dir("archive-n1-a");
    let a_layout = RepositoryLayout::init(&a_dir).expect("init a");
    crate::bundle::import_bundle_from_bytes(
        &a_layout,
        &bundle_bytes,
        &crate::bundle::BundleImportOptions::default_limits(),
    )
    .expect("bundle import succeeds even though the key is not trusted -- that is the point");

    let archive_bytes = export_bytes(&a_layout);

    // `archive verify` must exit 0 -- the untrusted signature is named, not refused on.
    let verify_report = {
        let mut cursor = Cursor::new(archive_bytes.clone());
        super::super::verify_archive(&mut cursor).expect("verify")
    };
    assert!(
        verify_report.is_internally_consistent(),
        "N1: an untrusted-but-carried received signature must never make the archive \
         inconsistent: {verify_report:?}"
    );
    assert!(
        verify_report
            .not_checked_signatures
            .iter()
            .any(|line| line.contains(&upstream_block_id.to_string())),
        "the untrusted signature must be named: {:?}",
        verify_report.not_checked_signatures
    );

    // A default import into a destination that has never heard of upstream's key must succeed.
    let dest1_dir = unique_temp_dir("archive-n1-dest1-untrusting");
    let dest1_layout = RepositoryLayout::init(&dest1_dir).expect("init destination 1");
    let mut cursor = Cursor::new(archive_bytes.clone());
    import_archive(&dest1_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("N1: a default import must succeed even though the carried signature is untrusted");

    // The destination's own `verify` must now report the same kind of trust issue the source
    // itself would -- `doctor` surfaces it as an error, naming the publication-trust code.
    let issue_before = crate::doctor::doctor_repository(&dest1_layout)
        .issues
        .into_iter()
        .find(|issue| issue.message.contains("PRIKK-TRUST-PUBLICATION-UNTRUSTED"))
        .or_else(|| {
            // The issue's own code field, if `doctor` surfaces this one under its own code rather
            // than embedding the trust code in the message -- checked either way, not assumed.
            crate::doctor::doctor_repository(&dest1_layout)
                .issues
                .into_iter()
                .find(|issue| issue.code.contains("TRUST"))
        });
    assert!(
        issue_before.is_some(),
        "the destination's own doctor/verify must flag the landed-but-untrusted publication"
    );

    // Trusting the key after the fact resolves it.
    let upstream_signer = Ed25519MaintainerSigner::from_seed("import-test-maintainer", &[0x22; 32])
        .expect("re-derive upstream's own signer to read its key id/public key");
    let upstream_public_key_hex: String = upstream_signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(
        &dest1_layout,
        upstream_signer.key_id(),
        &upstream_public_key_hex,
    )
    .expect("adopt upstream's key at the destination, after the fact");
    let issue_after = crate::doctor::doctor_repository(&dest1_layout)
        .issues
        .into_iter()
        .find(|issue| {
            issue.message.contains("PRIKK-TRUST-PUBLICATION-UNTRUSTED")
                || issue.code.contains("TRUST")
        });
    assert!(
        issue_after.is_none(),
        "adopting the key after import must resolve the trust issue: {issue_after:?}"
    );

    // A destination that already trusts the key *before* import is clean immediately after.
    let dest2_dir = unique_temp_dir("archive-n1-dest2-already-trusting");
    let dest2_layout = RepositoryLayout::init(&dest2_dir).expect("init destination 2");
    crate::add_trusted_maintainer(
        &dest2_layout,
        upstream_signer.key_id(),
        &upstream_public_key_hex,
    )
    .expect("adopt upstream's key before import");
    let mut cursor = Cursor::new(archive_bytes);
    import_archive(&dest2_layout, &mut cursor, &ArchiveImportOptions::default())
        .expect("import into an already-trusting destination");
    let issue_immediately = crate::doctor::doctor_repository(&dest2_layout)
        .issues
        .into_iter()
        .find(|issue| {
            issue.message.contains("PRIKK-TRUST-PUBLICATION-UNTRUSTED")
                || issue.code.contains("TRUST")
        });
    assert!(
        issue_immediately.is_none(),
        "a destination that already trusts the key must read clean immediately after import: \
         {issue_immediately:?}"
    );
}

// 0.56.0 step 3c (A2/A3): `archive import` classifies a received landing (and, below, a
// `--adopt` landing) against the pointer it would replace, and refuses a `rollback`, `diverged`,
// or `does-not-connect` landing by default, the identical rule 3a built for `bundle import`.

/// Extends [`build_signed_source_repository`]'s own one-block fixture with a second, child block
/// (`block1b`, a fast-forward child of the root `build_signed_source_repository` already builds),
/// published as a second `RefState` (`update_seq: 2`, chaining `previous_ref_state_id`) at the
/// same `heads/main` -- 3a's own fixture shape (`block1`, then `block1b`, a child of it),
/// reused here for `archive import` instead of `bundle import`. Returns the archive bytes at
/// **both** points: `(archive_at_block1, archive_at_block1b)`.
fn build_two_generation_source_archives(dir: &std::path::Path) -> (Vec<u8>, Vec<u8>) {
    let (layout, block1_id, author_key_id) = build_signed_source_repository(dir);
    let archive_at_block1 = export_bytes(&layout);

    let author_signer =
        Ed25519AuthorSigner::from_seed("import-test-author", &[0x11; 32]).expect("author signer");
    assert_eq!(
        author_signer.key_id(),
        author_key_id,
        "same seed must reproduce the same key id"
    );
    let maintainer_signer =
        Ed25519MaintainerSigner::from_seed("import-test-maintainer", &[0x22; 32])
            .expect("maintainer signer");

    let mut store = FileObjectStore::new(layout.clone());

    let blob_payload = BlobPayload::new(BlobKind::Text, b"second generation\n".to_vec());
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
                path: "b.txt".to_string(),
                node_id: prikk_object::NodeId::from_bytes([0x52; 32]),
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
        .add_signature(author_signature(&author_signer, patch_id).expect("sign the patch"))
        .expect("attach the real author signature");
    store.write_object(&patch_envelope).expect("write patch");

    let block_payload = BlockPayload {
        parent_block_ids: vec![block1_id],
        kind: BlockKind::Normal,
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
    let block1b_id = block_envelope.object_id();
    block_envelope
        .add_signature(
            maintainer_signature(&maintainer_signer, ObjectType::Block, block1b_id)
                .expect("sign the block"),
        )
        .expect("attach the real maintainer signature");
    store.write_object(&block_envelope).expect("write block");

    let previous_ref_state_id = RefStore::new(layout.clone())
        .read_current_ref_state_id("heads/main")
        .expect("read current ref state")
        .expect("heads/main must already be published");

    let ref_state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block1b_id,
        update_seq: 2,
        previous_ref_state_id: Some(previous_ref_state_id),
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
                .expect("sign the ref state"),
        )
        .expect("attach the real maintainer signature");

    let ref_update_envelope = signed_ref_update_envelope(
        "heads/main",
        Some(previous_ref_state_id),
        ref_state_id,
        block1b_id,
        2,
    );

    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: Some(previous_ref_state_id),
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        })
        .expect("publish the second generation");

    let archive_at_block1b = export_bytes(&layout);
    (archive_at_block1, archive_at_block1b)
}

#[test]
fn received_landing_fast_forward_does_not_refuse() {
    let source_dir = unique_temp_dir("archive-3c-received-ff-source");
    let (archive_at_block1, archive_at_block1b) = build_two_generation_source_archives(&source_dir);

    let dest_dir = unique_temp_dir("archive-3c-received-ff-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    import_archive(
        &dest_layout,
        &mut Cursor::new(archive_at_block1),
        &ArchiveImportOptions::default(),
    )
    .expect("the first, no-local-history landing must never refuse");

    let before = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("a pointer must be present after the first landing");

    let report = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_at_block1b),
        &ArchiveImportOptions::default(),
    )
    .expect("a fast-forward landing must not refuse");
    assert!(
        report.landing_outcomes.iter().all(|landing| landing.landed),
        "{:?}",
        report.landing_outcomes
    );
    let after = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("a pointer must still be present");
    assert_ne!(
        before.ref_state_id, after.ref_state_id,
        "the pointer must have advanced to the fast-forwarded tip"
    );
}

#[test]
fn received_landing_rollback_refuses_and_leaves_the_pointer_unchanged() {
    let source_dir = unique_temp_dir("archive-3c-received-rollback-source");
    let (archive_at_block1, archive_at_block1b) = build_two_generation_source_archives(&source_dir);

    let dest_dir = unique_temp_dir("archive-3c-received-rollback-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");
    import_archive(
        &dest_layout,
        &mut Cursor::new(archive_at_block1b),
        &ArchiveImportOptions::default(),
    )
    .expect("landing the tip first must never refuse");
    let before = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("a pointer must be present");

    let error = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_at_block1.clone()),
        &ArchiveImportOptions::default(),
    )
    .expect_err("a rollback against the current received pointer must refuse by default");
    assert!(
        error.to_string().contains("rollback"),
        "the refusal must name the class: {error}"
    );
    let after = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("the pointer must still be present");
    assert_eq!(
        before.ref_state_id, after.ref_state_id,
        "a refused landing must leave the received pointer exactly as it was"
    );

    // The same delivery, with the flag, succeeds -- the import journal left by the refused
    // attempt above (same archive bytes, same identity) lets this resume rather than redo stage
    // 1's own object writes.
    let report = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_at_block1),
        &ArchiveImportOptions {
            allow_non_fast_forward: true,
            ..ArchiveImportOptions::default()
        },
    )
    .expect("the same rollback, with the flag, must succeed");
    assert!(report.landing_outcomes.iter().all(|landing| landing.landed));
}

#[test]
fn received_landing_does_not_connect_refuses_and_leaves_the_pointer_unchanged() {
    let dest_dir = unique_temp_dir("archive-3c-received-unrelated-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    let source_a_dir = unique_temp_dir("archive-3c-received-unrelated-source-a");
    let (layout_a, _block_a, _author_key_a) = build_signed_source_repository(&source_a_dir);
    let archive_a = export_bytes(&layout_a);
    import_archive(
        &dest_layout,
        &mut Cursor::new(archive_a),
        &ArchiveImportOptions::default(),
    )
    .expect("the first landing must never refuse");
    let before = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("a pointer must be present");

    // A second, genuinely unrelated source repository -- same ref name, no shared ancestry at
    // all (a fresh seed's own fresh genesis block).
    let source_c_dir = unique_temp_dir("archive-3c-received-unrelated-source-c");
    let layout_c = RepositoryLayout::init(&source_c_dir).expect("init source c");
    let maintainer_c =
        Ed25519MaintainerSigner::from_seed("unrelated-maintainer-c", &[0x44; 32]).expect("signer");
    let mut store_c = FileObjectStore::new(layout_c.clone());
    let block_c_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).expect("empty state root"),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_c_bytes = block_c_payload
        .to_canonical_bytes()
        .expect("encode block payload");
    let mut block_c_envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, block_c_bytes);
    let block_c_id = block_c_envelope.object_id();
    block_c_envelope
        .add_signature(
            maintainer_signature(&maintainer_c, ObjectType::Block, block_c_id)
                .expect("sign the block"),
        )
        .expect("attach the real maintainer signature");
    store_c
        .write_object(&block_c_envelope)
        .expect("write block");
    let ref_state_c_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: block_c_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_c_bytes = ref_state_c_payload
        .to_canonical_bytes()
        .expect("encode ref-state payload");
    let mut ref_state_c_envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, ref_state_c_bytes);
    let ref_state_c_id = ref_state_c_envelope.object_id();
    ref_state_c_envelope
        .add_signature(
            maintainer_signature(&maintainer_c, ObjectType::RefState, ref_state_c_id)
                .expect("sign the ref state"),
        )
        .expect("attach the real maintainer signature");
    let ref_update_c_envelope =
        signed_ref_update_envelope("heads/main", None, ref_state_c_id, block_c_id, 1);
    RefStore::new(layout_c.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: ref_state_c_envelope,
            ref_update: ref_update_c_envelope,
        })
        .expect("publish heads/main");
    let archive_c = export_bytes(&layout_c);

    let error = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_c.clone()),
        &ArchiveImportOptions::default(),
    )
    .expect_err("an unrelated landing against the current received pointer must refuse");
    assert!(
        error.to_string().contains("does-not-connect"),
        "the refusal must name the class: {error}"
    );
    let after = crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("the pointer must still be present");
    assert_eq!(
        before.ref_state_id, after.ref_state_id,
        "a refused landing must leave the received pointer exactly as it was"
    );

    let report = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_c),
        &ArchiveImportOptions {
            allow_non_fast_forward: true,
            ..ArchiveImportOptions::default()
        },
    )
    .expect("the same unrelated landing, with the flag, must succeed");
    assert!(report.landing_outcomes.iter().all(|landing| landing.landed));
}

/// 2A v2's own measured `--adopt` case, reproduced: a destination (`repoB`) already holds a
/// **received** pointer at `remotes/heads/main` (from an earlier, ordinary import); a third,
/// completely unrelated archive (`repoE`) also names `heads/main`, with no local `heads/main` at
/// the destination. Before this round, `--adopt` adopted it as a real local branch with no
/// mention anywhere of the received pointer of the identical name already held. After this
/// round, it refuses by default.
#[test]
fn adopt_refuses_an_unrelated_received_pointer_at_the_destination_by_default() {
    let dest_dir = unique_temp_dir("archive-3c-adopt-unrelated-dest");
    let dest_layout = RepositoryLayout::init(&dest_dir).expect("init destination");

    let repo_a_dir = unique_temp_dir("archive-3c-adopt-unrelated-repo-a");
    let (layout_a, _block_a, _author_key_a) = build_signed_source_repository(&repo_a_dir);
    let archive_a = export_bytes(&layout_a);
    import_archive(
        &dest_layout,
        &mut Cursor::new(archive_a),
        &ArchiveImportOptions::default(),
    )
    .expect("the ordinary received landing must never refuse");
    assert_eq!(
        RefStore::new(dest_layout.clone())
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state"),
        None,
        "heads/main must still be absent locally -- only received"
    );

    let repo_e_dir = unique_temp_dir("archive-3c-adopt-unrelated-repo-e");
    let (layout_e, _ref_state_e_id, maintainer_e_key_id, maintainer_e_public_key_hex) =
        build_adoptable_source_repository(&repo_e_dir, "heads/main");
    crate::add_trusted_maintainer(
        &dest_layout,
        &maintainer_e_key_id,
        &maintainer_e_public_key_hex,
    )
    .expect(
        "adopt repo e's maintainer key at the destination, so --adopt's own trust check passes",
    );
    let archive_e = export_bytes(&layout_e);

    let error = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_e.clone()),
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: false,
        },
    )
    .expect_err(
        "adopting an unrelated history against an existing received pointer of the same name \
         must refuse",
    );
    assert!(
        error.to_string().contains("does-not-connect"),
        "the refusal must name the class: {error}"
    );
    assert_eq!(
        RefStore::new(dest_layout.clone())
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state"),
        None,
        "a refused adoption must not create the local ref"
    );
    crate::received::read_received_pointer(&dest_layout, "remotes/heads/main")
        .expect("read received pointer")
        .expect("the received pointer must still be present after the refused adoption");

    let report = import_archive(
        &dest_layout,
        &mut Cursor::new(archive_e),
        &ArchiveImportOptions {
            adopt: true,
            allow_non_fast_forward: true,
        },
    )
    .expect("the same adoption, with the flag, must succeed");
    let landing = report
        .landing_outcomes
        .iter()
        .find(|landing| landing.ref_name == "heads/main")
        .expect("heads/main must be among the landings");
    assert!(landing.adopted, "{landing:?}");
    assert_eq!(
        landing
            .remote_reason
            .as_deref()
            .map(|reason| reason.contains("does-not-connect")),
        Some(true),
        "remote_reason must name the class even though adoption succeeded: {landing:?}"
    );
    assert!(
        RefStore::new(dest_layout.clone())
            .read_current_ref_state_id("heads/main")
            .expect("read current ref state")
            .is_some(),
        "the second attempt, with the flag, must have adopted a real local heads/main"
    );
}
