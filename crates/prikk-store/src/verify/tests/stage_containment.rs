//! DC-95 Stage 2, Level 1: acceptance-criteria tests for scope containment itself (`stage-2-design-
//! v1.md` §11, the implementation handoff §6) -- distinct from Stage 1's per-check reachability
//! tests (`ref_cluster.rs`, `wal_cluster.rs`, `verify/tests.rs`), which prove a specific check is
//! load-bearing. These prove the *containment machinery* is sound: independent failures in
//! different stages are both reported, `NotEvaluated` correctly names its blocking dependency, and
//! the one correctness fix the Step 0 ruling required (`trust_is_valid` must not read `true` from a
//! trust check that never ran) actually holds.

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload, RefUpdatePayload,
};

use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::{
    signed_patch_blob_envelope, signed_patch_envelope, unique_temp_dir,
};
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519MaintainerSigner, FileObjectStore, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, StageOutcome, StageStatus, VerificationStage, VerifyOptions, Wal,
    add_trusted_maintainer, maintainer_signature, verify_repository,
    verify_repository_with_options,
};

fn trusted_signer(seed_label: &str, byte: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(seed_label, &[byte; 32])
}

fn adopt(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) -> Result<()> {
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    add_trusted_maintainer(layout, signer.key_id(), &public_key_hex)?;
    Ok(())
}

/// Local copy of `ref_cluster.rs`'s own private helper of the same shape -- kept file-local rather
/// than shared, matching this test tree's existing convention of small per-file helpers.
fn build_signed_ref_update(
    ref_name: &str,
    old_ref_state_id: Option<ObjectId>,
    new_ref_state_id: ObjectId,
    new_target_object_id: ObjectId,
    update_seq: u64,
    signer: &Ed25519MaintainerSigner,
) -> Result<ObjectEnvelope> {
    let payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id,
        new_ref_state_id,
        new_target_object_id,
        update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, ObjectType::RefUpdate, id)?)?;
    Ok(envelope)
}

fn find_stage(outcomes: &[StageOutcome], stage: VerificationStage) -> &StageOutcome {
    outcomes
        .iter()
        .find(|outcome| outcome.stage == stage)
        .unwrap_or_else(|| panic!("expected a StageOutcome for stage {stage}, found none"))
}

/// Acceptance criterion 1 (design §11, handoff §6.1): a `Failed` stage does not suppress a later,
/// independent stage's own findings. Two defects with nothing in common -- a structural directory-
/// shape violation (`Objects`; DC-95 Stage 2 Level 2 Step 0 §1.1: this class of defect stays
/// whole-stage hard-`Err` even under item containment, since it invalidates the directory-shape
/// assumption every per-item read in the stage relies on, not just one item) and a checksum-
/// corrupted active WAL record (`WalReplay`) -- planted in the same repository. Both must appear
/// `Failed` in the same report.
#[test]
fn verify_repository_reports_two_independent_stage_failures_together() -> Result<()> {
    let root = unique_temp_dir("stage2-two-independent-failures");
    let layout = RepositoryLayout::init(root.clone())?;

    // Objects: a stray directory inside an object prefix directory, where only object files are
    // expected -- structural, same technique as
    // `verify_repository_detects_every_directory_shape_violation` (`verify/tests.rs`).
    let mut objects = FileObjectStore::new(layout.clone());
    let stray_id = objects.write_object(&ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        b"payload".to_vec(),
    ))?;
    let prefix_dir = layout
        .object_path(ObjectType::Blob, stray_id)
        .parent()
        .ok_or_else(|| PrikkError::Io {
            kind: None,
            context: "object path has no parent".to_string(),
        })?
        .to_path_buf();
    std::fs::create_dir_all(prefix_dir.join("stray-directory"))?;

    // WalReplay: RFC 102 Stage 2 contains a checksum-damaged record to an item finding, not a
    // stage failure (`verify_repository_detects_wal_checksum_mismatch`,
    // `verify_repository_marks_every_wal_replay_dependent_as_not_evaluated` now cover that shape).
    // A genuine stage-level `WalReplay` failure still needs a schema-shape defect, which Stage 2
    // deliberately leaves a hard error (out of its own scope -- see `wal.rs::Wal::replay`).
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let mut patch = signed_patch_envelope();
    patch.schema_version = 99;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let bytes = crate::wal::encode_record_for_test(&crate::wal::WalRecord {
        seq: 1,
        envelope: patch,
    })?;
    std::fs::write(wal.path(), &bytes)?;

    let report = verify_repository(&layout)?;
    assert!(report.has_stage_failure());
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::Objects).status,
        StageStatus::Failed { .. }
    ));
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::WalReplay).status,
        StageStatus::Failed { .. }
    ));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 2 Level 2 acceptance criterion 1, at Phase A granularity (the criterion Level 1
/// could not satisfy one level in), re-targeted for RFC 102 Stage 3 at container scale: two
/// independent bad objects with nothing in common -- two checksum-corrupted Blob container records,
/// distinguished by different byte offsets -- are both reported `Failed` in `object_outcomes`, and
/// neither suppresses the other, the `Objects` stage itself, or a third, genuinely clean object.
/// (The pre-Stage-3 construction -- two id-mismatched loose files -- has no direct container
/// equivalent: an index entry resolving to the wrong object is a *stage*-level index-integrity
/// defect under containers, not an item-level one; see `verify_repository_detects_index_entry_
/// resolving_to_a_different_object` in the parent module for that shape instead.) Whole-map
/// assertion (handoff §4), not presence-of-one-expected-entry: the outcome set is walked and every
/// entry classified, the same pattern that caught Level 1's own `CommitIndex` and `blocked_by`
/// regressions.
///
/// **Also the end-to-end proof for the RFC 102 Stage 3 handoff's own acceptance criteria 4 and 5**,
/// through `verify_repository` rather than `container::decode_container_records` directly
/// (`container/tests.rs::isolates_a_damaged_record_and_reads_every_sound_record_around_it` is the
/// unit-level proof, mirroring `wal_cluster.rs`'s own pairing of the two levels): criterion 4
/// ("corruption isolation... re-proven at container scale") in the `Objects` stage staying
/// `Evaluated` with the third, undamaged object still `Evaluated` in `object_outcomes`; criterion 5
/// ("a repository that failed verification before still fails it") in `report.has_item_failure()`.
#[test]
fn verify_repository_reports_two_independent_bad_objects_in_the_same_stage() -> Result<()> {
    let root = unique_temp_dir("stage2-two-independent-bad-objects");
    let layout = RepositoryLayout::init(root.clone())?;

    let mut objects = FileObjectStore::new(layout.clone());
    let mut offsets = Vec::new();
    let container_path = layout.container_slot_path(
        ObjectType::Blob,
        crate::foundation::layout::ContainerSlot::A,
    );
    for label in ["first", "second", "third"] {
        let before = std::fs::read(&container_path)
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        offsets.push(before);
        let blob = prikk_object::BlobPayload::new(prikk_object::BlobKind::Text, label.into());
        let mut envelope =
            ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes()?);
        envelope.add_signature(maintainer_signature(
            &trusted_signer("stage2-two-bad-objects", 0x1c)?,
            ObjectType::Blob,
            envelope.object_id(),
        )?)?;
        objects.write_object(&envelope)?;
    }
    // Corrupt the first two records' own last byte -- stays inside the body, past the fixed header,
    // so the frame's magic/version/body_len all still parse and decode reaches the checksum
    // compare, mirroring `wal_cluster.rs`'s established two-damaged-records technique.
    let mut bytes = std::fs::read(&container_path)?;
    let end = bytes.len();
    let bounds: Vec<(usize, usize)> = offsets
        .iter()
        .enumerate()
        .map(|(i, &start)| (start, offsets.get(i + 1).copied().unwrap_or(end)))
        .collect();
    for &damaged in &[0_usize, 1_usize] {
        let (_, stop) = *bounds
            .get(damaged)
            .ok_or_else(|| PrikkError::Integrity("damaged index out of bounds".to_string()))?;
        let byte = bytes
            .get_mut(stop - 1)
            .ok_or_else(|| PrikkError::Integrity("record end out of bounds".to_string()))?;
        *byte ^= 0x01;
    }
    std::fs::write(&container_path, &bytes)?;

    let report = verify_repository(&layout)?;
    assert!(
        matches!(
            find_stage(&report.stage_outcomes, VerificationStage::Objects).status,
            StageStatus::Evaluated
        ),
        "criterion 4: corruption isolation must keep the Objects stage itself Evaluated"
    );
    assert!(
        report.has_item_failure(),
        "criterion 5: a repository with a damaged container record must still fail verification"
    );
    assert_eq!(
        report.object_outcomes.len(),
        3,
        "no object may be silently absent: {:?}",
        report.object_outcomes
    );
    let failed_count = report
        .object_outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, crate::ObjectItemStatus::Failed { .. }))
        .count();
    let evaluated_count = report
        .object_outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, crate::ObjectItemStatus::Evaluated(_)))
        .count();
    assert_eq!(
        failed_count, 2,
        "expected exactly the two id-mismatched Blobs to be Failed: {:?}",
        report.object_outcomes
    );
    assert_eq!(
        evaluated_count, 1,
        "the genuinely clean object must still evaluate: {:?}",
        report.object_outcomes
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 2 Level 2 acceptance criterion 1, refs half: two independent bad refs with nothing
/// in common -- two ref pointers each targeting a Block that was deleted after publishing -- are
/// both reported `Failed`, and neither suppresses the other, the `Refs` stage itself, or a third,
/// genuinely clean ref. Whole-map assertion (handoff §4): the outcome sets are walked and every
/// entry classified, not just checked for the presence of one expected entry.
#[test]
fn verify_repository_reports_two_independent_bad_refs_in_the_same_stage() -> Result<()> {
    let root = unique_temp_dir("stage2-two-independent-bad-refs");
    let layout = RepositoryLayout::init(root.clone())?;
    let signer = trusted_signer("stage2-two-bad-refs", 0x1d)?;
    adopt(&layout, &signer)?;
    let mut objects = FileObjectStore::new(layout.clone());

    let publish_dangling_ref = |objects: &mut FileObjectStore, ref_name: &str| -> Result<()> {
        // A distinguishing snapshot blob keeps each ref's Block from content-addressing to the
        // *same* id -- otherwise a later block with an identical (empty) payload would silently
        // recreate an earlier one's just-deleted file at their shared canonical path.
        let snapshot = prikk_object::BlobPayload::new(
            prikk_object::BlobKind::Text,
            ref_name.as_bytes().to_vec(),
        );
        let mut snapshot_envelope =
            ObjectEnvelope::unsigned(ObjectType::Blob, 1, snapshot.to_canonical_bytes()?);
        let snapshot_id = snapshot_envelope.object_id();
        snapshot_envelope.add_signature(maintainer_signature(
            &signer,
            ObjectType::Blob,
            snapshot_id,
        )?)?;
        objects.write_object(&snapshot_envelope)?;

        let block_payload = BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: crate::derive_next_state_root(objects, None, &[])?,
            snapshot_blob_ref: Some(snapshot_id),
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        };
        let mut block_envelope =
            ObjectEnvelope::unsigned(ObjectType::Block, 2, block_payload.to_canonical_bytes()?);
        let block_id = block_envelope.object_id();
        block_envelope.add_signature(maintainer_signature(
            &signer,
            ObjectType::Block,
            block_id,
        )?)?;
        objects.write_object(&block_envelope)?;

        let state = RefStatePayload {
            ref_name: ref_name.to_string(),
            kind: RefKind::Branch,
            target_object_id: block_id,
            update_seq: 1,
            previous_ref_state_id: None,
            required_attestation_ids: Vec::new(),
            closed: false,
        };
        let mut ref_state =
            ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes()?);
        let state_id = ref_state.object_id();
        ref_state.add_signature(maintainer_signature(
            &signer,
            ObjectType::RefState,
            state_id,
        )?)?;
        let update = build_signed_ref_update(ref_name, None, state_id, block_id, 1, &signer)?;
        RefStore::new(layout.clone()).publish(&RefPublication {
            ref_name: ref_name.to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update: update,
        })?;

        // Remove the target Block's index entry after publishing -- containers are append-only, so
        // there is no direct "delete one object" equivalent to the pre-Stage-3 `std::fs::remove_file`
        // this replaces; a genuinely dangling reference under containers is an object whose index
        // entry is gone, not a container record that was ever removed (same technique as
        // `verify_repository_detects_dangling_ref_target`, `ref_cluster.rs`).
        crate::foundation::index::remove_index_entry_for_test(&layout, block_id)?;
        Ok(())
    };
    publish_dangling_ref(&mut objects, "heads/first")?;
    publish_dangling_ref(&mut objects, "heads/second")?;

    // One genuinely clean ref too, so the test also confirms a bad ref does not suppress a good
    // one's own Evaluated outcome.
    let clean_block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::derive_next_state_root(&objects, None, &[])?,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut clean_block_envelope = ObjectEnvelope::unsigned(
        ObjectType::Block,
        2,
        clean_block_payload.to_canonical_bytes()?,
    );
    let clean_block = clean_block_envelope.object_id();
    clean_block_envelope.add_signature(maintainer_signature(
        &signer,
        ObjectType::Block,
        clean_block,
    )?)?;
    objects.write_object(&clean_block_envelope)?;
    let clean_state = RefStatePayload {
        ref_name: "heads/clean".to_string(),
        kind: RefKind::Branch,
        target_object_id: clean_block,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut clean_ref_state =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, clean_state.to_canonical_bytes()?);
    let clean_state_id = clean_ref_state.object_id();
    clean_ref_state.add_signature(maintainer_signature(
        &signer,
        ObjectType::RefState,
        clean_state_id,
    )?)?;
    let clean_update =
        build_signed_ref_update("heads/clean", None, clean_state_id, clean_block, 1, &signer)?;
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: "heads/clean".to_string(),
        expected_previous_ref_state_id: None,
        ref_state: clean_ref_state,
        ref_update: clean_update,
    })?;

    let report = verify_repository(&layout)?;
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::Refs).status,
        StageStatus::Evaluated
    ));
    assert!(report.has_item_failure());
    let failed_count = report
        .pointer_outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, crate::RefFileStatus::Failed { .. }))
        .count();
    let evaluated_count = report
        .pointer_outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, crate::RefFileStatus::Evaluated { .. }))
        .count();
    assert_eq!(
        failed_count, 2,
        "expected exactly the two dangling-target refs to be Failed: {:?}",
        report.pointer_outcomes
    );
    assert_eq!(
        evaluated_count, 1,
        "the genuinely clean ref must still evaluate: {:?}",
        report.pointer_outcomes
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Acceptance criterion 2 (design §11, handoff §6.2): every stage whose own execution genuinely
/// requires `WalReplay`'s output is `NotEvaluated`, naming `WalReplay` as the blocker -- not
/// silently absent, not defaulted to `Evaluated`. Covers all six real dependents from Step 0's
/// corrected graph (`stage-2-level-1-step0-ruling-v1.md` §1): `WalPersistence`, `RollbackDrafts`,
/// `WalRecordSchema`, `ActiveWalMetadata`, `PublicationReclassification`, `WalOrdering`.
#[test]
fn verify_repository_marks_every_wal_replay_dependent_as_not_evaluated() -> Result<()> {
    let root = unique_temp_dir("stage2-wal-replay-dependents");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    // RFC 102 Stage 2: a checksum-damaged record no longer fails the whole `WalReplay` stage --
    // isolate-and-continue reading contains it to an item finding instead (`wal_cluster.rs`'s own
    // `verify_repository_detects_wal_checksum_mismatch` now proves that). This test is about the
    // dependency-graph machinery itself, which still needs a genuine stage-level `WalReplay`
    // failure to exercise -- `validate_read_schema` (called after `decode_records` inside
    // `Wal::replay()` itself, deliberately left a hard error by Stage 2: a schema-shape defect is a
    // different concern from physical corruption, out of this stage's scope) still produces one, so
    // a record encoded with a wrong schema version is the new fixture.
    let mut patch = signed_patch_envelope();
    patch.schema_version = 99;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let bytes = crate::wal::encode_record_for_test(&crate::wal::WalRecord {
        seq: 1,
        envelope: patch,
    })?;
    std::fs::write(wal.path(), &bytes)?;

    let report = verify_repository(&layout)?;
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::WalReplay).status,
        StageStatus::Failed { .. }
    ));
    for dependent in [
        VerificationStage::WalPersistence,
        VerificationStage::RollbackDrafts,
        VerificationStage::WalRecordSchema,
        VerificationStage::ActiveWalMetadata,
        VerificationStage::PublicationReclassification,
        VerificationStage::WalOrdering,
    ] {
        let outcome = find_stage(&report.stage_outcomes, dependent);
        match &outcome.status {
            StageStatus::NotEvaluated { blocked_by } => {
                assert_eq!(
                    *blocked_by,
                    VerificationStage::WalReplay,
                    "stage {dependent} should name WalReplay as its blocker"
                );
            }
            other => panic!("expected stage {dependent} to be NotEvaluated, got: {other:?}"),
        }
    }
    // Objects, Refs, RefUpdateSchemaTrust, CommitIndex, LifecycleCache have no dependency on
    // WalReplay and must still evaluate.
    for independent in [
        VerificationStage::Objects,
        VerificationStage::Refs,
        VerificationStage::RefUpdateSchemaTrust,
        VerificationStage::CommitIndex,
        VerificationStage::LifecycleCache,
    ] {
        assert!(
            matches!(
                find_stage(&report.stage_outcomes, independent).status,
                StageStatus::Evaluated
            ),
            "stage {independent} does not depend on WalReplay and should still evaluate"
        );
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// `--stop-on-first-error` (`VerifyOptions::stop_on_first_error`), preserving today's bounded walk:
/// once the *first* stage in pipeline order fails, every later stage becomes blocking. Same fixture as
/// `verify_repository_reports_two_independent_stage_failures_together` (a structural directory-shape
/// violation in `Objects`, a checksum-corrupted active WAL record in `WalReplay`), which proves the
/// *opposite* under default options: both `Failed` independently. Here, with the flag set, `Objects`
/// runs first, fails, and the halt suppresses `WalReplay` from ever attempting its own (also
/// genuinely broken) check.
///
/// The required fix from `stage-2-level-1-implementation-review-v1.md` §4: a stage preempted by the
/// halt is `Halted { after: Objects }`, not `NotEvaluated { blocked_by: Objects }` -- `Objects` is not
/// a real dependency of, say, `LifecycleCache`, so naming it as a `blocked_by` would assert a
/// dependency-graph edge that does not exist. Stages with a *real* dependency on something other than
/// `Objects` still report that true dependency via `NotEvaluated`, even though the walk is halted: e.g.
/// `WalPersistence` genuinely depends on `WalReplay`, and `WalReplay` itself never evaluated (it was
/// pre-empted, `Halted { after: Objects }`) -- so `WalPersistence` is `NotEvaluated { blocked_by:
/// WalReplay }`, an accurate claim regardless of *why* `WalReplay` didn't evaluate, discoverable by
/// following the chain one stage at a time rather than by this stage reaching past its own dependency.
#[test]
fn verify_repository_with_options_halts_every_later_stage_when_stop_on_first_error_is_set()
-> Result<()> {
    let root = unique_temp_dir("stage2-stop-on-first-error");
    let layout = RepositoryLayout::init(root.clone())?;

    // Objects: a stray directory inside an object prefix directory, structural, same technique as
    // the two-independent-failures test above.
    let mut objects = FileObjectStore::new(layout.clone());
    let stray_id = objects.write_object(&ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        b"payload".to_vec(),
    ))?;
    let prefix_dir = layout
        .object_path(ObjectType::Blob, stray_id)
        .parent()
        .ok_or_else(|| PrikkError::Io {
            kind: None,
            context: "object path has no parent".to_string(),
        })?
        .to_path_buf();
    std::fs::create_dir_all(prefix_dir.join("stray-directory"))?;

    // WalReplay: a real, well-formed patch, then a corrupted checksum byte -- independently broken,
    // same as the two-independent-failures test, so this stage would fail on its own if it ran.
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&patch)?;
    let mut bytes = std::fs::read(wal.path())?;
    let last_byte = bytes.last_mut().ok_or_else(|| PrikkError::Io {
        kind: None,
        context: "WAL file unexpectedly empty".to_string(),
    })?;
    *last_byte ^= 0x01;
    std::fs::write(wal.path(), &bytes)?;

    let report = verify_repository_with_options(
        &layout,
        VerifyOptions {
            stop_on_first_error: true,
        },
    )?;
    assert!(report.has_stage_failure());
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::Objects).status,
        StageStatus::Failed { .. }
    ));
    // Stages with no real dependency of their own were simply pre-empted by the halt -- Halted, naming
    // Objects as the stage that triggered the stop, not a fabricated dependency.
    for halted in [
        VerificationStage::Refs,
        VerificationStage::WalReplay,
        VerificationStage::CommitIndex,
        VerificationStage::LifecycleCache,
    ] {
        let outcome = find_stage(&report.stage_outcomes, halted);
        match &outcome.status {
            StageStatus::Halted { after } => {
                assert_eq!(
                    *after,
                    VerificationStage::Objects,
                    "stage {halted} should name Objects as the stage that halted the walk"
                );
            }
            other => panic!("expected stage {halted} to be Halted, got: {other:?}"),
        }
    }
    // Stages with a real dependency still name that true dependency, even though it was itself
    // pre-empted rather than genuinely failed -- the claim "I could not run because WalReplay did not
    // evaluate" remains true regardless of why WalReplay didn't evaluate.
    for (dependent, dependency) in [
        (
            VerificationStage::RefUpdateSchemaTrust,
            VerificationStage::Refs,
        ),
        (
            VerificationStage::WalPersistence,
            VerificationStage::WalReplay,
        ),
        (
            VerificationStage::RollbackDrafts,
            VerificationStage::WalReplay,
        ),
        (
            VerificationStage::WalRecordSchema,
            VerificationStage::WalReplay,
        ),
        (
            VerificationStage::ActiveWalMetadata,
            VerificationStage::WalReplay,
        ),
        (
            VerificationStage::PublicationReclassification,
            VerificationStage::WalReplay,
        ),
        (VerificationStage::WalOrdering, VerificationStage::WalReplay),
    ] {
        let outcome = find_stage(&report.stage_outcomes, dependent);
        match &outcome.status {
            StageStatus::NotEvaluated { blocked_by } => {
                assert_eq!(
                    *blocked_by, dependency,
                    "stage {dependent} should name its real dependency {dependency}, not the stage that halted the walk"
                );
            }
            other => panic!("expected stage {dependent} to be NotEvaluated, got: {other:?}"),
        }
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Recovery-listing-tolerance follow-up, site 2's "after" control. Before this fix, a missing
/// `refs/tmp` directory made `refs/verify.rs::candidate_issues`'s bare `list_directory` call fail
/// outright, which `verify_refs`'s own `Err` propagation turned into a `Failed` `Refs` stage outcome
/// -- and, under `--stop-on-first-error`, halted every stage after it, exactly the shape asserted
/// above for a genuinely broken `Objects` stage. The report quotes a reverted-and-reran probe
/// showing that real "before" failure; this test is the "after": `Refs` now evaluates cleanly over
/// the missing directory, so nothing downstream is halted by it.
#[test]
fn verify_repository_tolerates_a_missing_ref_candidate_directory_even_with_stop_on_first_error()
-> Result<()> {
    let root = unique_temp_dir("stage2-missing-refs-tmp-tolerated");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::remove_dir_all(layout.refs_dir().join("tmp"))?;

    let report = verify_repository_with_options(
        &layout,
        VerifyOptions {
            stop_on_first_error: true,
        },
    )?;
    assert!(
        !report.has_stage_failure(),
        "expected a clean report despite the missing refs/tmp, got: {report:?}"
    );
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::Refs).status,
        StageStatus::Evaluated
    ));
    for later in [
        VerificationStage::WalReplay,
        VerificationStage::CommitIndex,
        VerificationStage::LifecycleCache,
    ] {
        assert!(
            matches!(
                find_stage(&report.stage_outcomes, later).status,
                StageStatus::Evaluated
            ),
            "stage {later} should not have been halted by a tolerated missing refs/tmp"
        );
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 118 stage 4 §3: the completeness invariant `RepositoryVerification::stage_outcomes`'s own
/// doc promises -- exactly one outcome per [`VerificationStage::ALL`] entry -- asserted against a
/// **real** `verify_repository` report, not a hand-built `RepositoryVerification`. A test that
/// constructs the value it then checks proves nothing about the pipeline that actually produces it;
/// a freshly-initialized, otherwise-untouched repository is enough, since every stage here is
/// expected to run to completion with nothing to find.
#[test]
fn verify_repository_reports_exactly_one_outcome_per_all_entry() -> Result<()> {
    let root = unique_temp_dir("stage2-completeness-full-accumulation");
    let layout = RepositoryLayout::init(root.clone())?;

    let report = verify_repository(&layout)?;
    assert_stage_outcomes_cover_all_exactly_once(&report.stage_outcomes);
    for outcome in &report.stage_outcomes {
        assert_eq!(
            outcome.status,
            StageStatus::Evaluated,
            "a freshly-initialized repository should have nothing to find, but {} resolved to {:?}",
            outcome.stage,
            outcome.status
        );
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 118 stage 4 §3: the same completeness invariant under `--stop-on-first-error`, the path
/// `verify_repository_reports_exactly_one_outcome_per_all_entry` (above) does not exercise --
/// `Halted` exists precisely so an early stop still emits an entry per stage, and an omission here
/// is the failure a CI completeness gate would actually hit, not a full-accumulation walk. One
/// planted structural defect in `Objects` (the same technique
/// `verify_repository_with_options_halts_every_later_stage_when_stop_on_first_error_is_set` above
/// uses) is enough to force a halt; that test already proves the halt's dependency-graph naming is
/// correct stage by stage, so this one only asserts the coverage count.
#[test]
fn verify_repository_with_options_stop_on_first_error_reports_exactly_one_outcome_per_all_entry()
-> Result<()> {
    let root = unique_temp_dir("stage2-completeness-stop-on-first-error");
    let layout = RepositoryLayout::init(root.clone())?;

    let mut objects = FileObjectStore::new(layout.clone());
    let stray_id = objects.write_object(&ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        b"payload".to_vec(),
    ))?;
    let prefix_dir = layout
        .object_path(ObjectType::Blob, stray_id)
        .parent()
        .ok_or_else(|| PrikkError::Io {
            kind: None,
            context: "object path has no parent".to_string(),
        })?
        .to_path_buf();
    std::fs::create_dir_all(prefix_dir.join("stray-directory"))?;

    let report = verify_repository_with_options(
        &layout,
        VerifyOptions {
            stop_on_first_error: true,
        },
    )?;
    assert!(report.has_stage_failure());
    assert!(matches!(
        find_stage(&report.stage_outcomes, VerificationStage::Objects).status,
        StageStatus::Failed { .. }
    ));
    assert_stage_outcomes_cover_all_exactly_once(&report.stage_outcomes);
    let halted: Vec<VerificationStage> = report
        .stage_outcomes
        .iter()
        .filter(|outcome| matches!(outcome.status, StageStatus::Halted { .. }))
        .map(|outcome| outcome.stage)
        .collect();
    assert!(
        halted.contains(&VerificationStage::ReceivedRefs)
            && halted.contains(&VerificationStage::LocalTagTrust),
        "ReceivedRefs and LocalTagTrust have no real dependency of their own, so a halt after \
         Objects should report both Halted, not silently drop them: {halted:?}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Shared by both completeness tests above: every [`VerificationStage::ALL`] member appears in
/// `outcomes` exactly once, and nothing appears that is not in `ALL`.
fn assert_stage_outcomes_cover_all_exactly_once(outcomes: &[StageOutcome]) {
    use std::collections::BTreeSet;
    let reported: BTreeSet<VerificationStage> =
        outcomes.iter().map(|outcome| outcome.stage).collect();
    assert_eq!(
        outcomes.len(),
        VerificationStage::ALL.len(),
        "expected exactly one outcome per VerificationStage::ALL entry ({}), got {}: {outcomes:?}",
        VerificationStage::ALL.len(),
        outcomes.len()
    );
    assert_eq!(
        reported.len(),
        VerificationStage::ALL.len(),
        "expected no duplicate stage in stage_outcomes: {outcomes:?}"
    );
    for &stage in VerificationStage::ALL {
        assert!(
            reported.contains(&stage),
            "VerificationStage::ALL names {stage}, which stage_outcomes does not report at all"
        );
    }
}
