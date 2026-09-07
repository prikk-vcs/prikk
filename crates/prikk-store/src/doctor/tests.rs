//! Doctor tests.

use std::io::Write;

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, NodeId, ObjectEnvelope, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose, RefKind, RefStatePayload,
    RefUpdatePayload,
};

use crate::{
    ActiveSessionRepairStatus, DEFAULT_ACTIVE_NAME, DoctorRepairOptions, DoctorSeverity,
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectWriter, RepositoryLayout,
    Wal, add_trusted_maintainer, derive_next_state_root, doctor_repository,
    maintainer_signature as real_maintainer_signature, repair_repository,
    write_active_ref_metadata,
};

use crate::test_gates::test_support::{
    maintainer_signature as legacy_maintainer_signature, rollback_author_signature,
    sample_object_id, signed_patch_envelope, unique_temp_dir,
};
use crate::wal::{WalRecord, encode_record_for_test};

/// A `Normal`-purpose Patch envelope distinguished by `label` (via `sample_object_id`), for tests
/// that need two or more genuinely different patches to tell apart by identity, not just count --
/// `signed_patch_envelope()` always produces the same fixed content and therefore the same object
/// id. Mirrors `verify/tests/wal_cluster.rs::normal_patch_envelope`'s own shape (kept file-local
/// rather than shared, matching this test tree's established per-file-helper convention).
fn distinct_patch_envelope(label: &str) -> prikk_error::Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(prikk_object::CreateFile {
                path: "a.txt".to_string(),
                node_id: NodeId::from_bytes([0x61; 32]),
                blob_id: sample_object_id(label),
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(rollback_author_signature())?;
    Ok(envelope)
}

#[test]
fn doctor_reports_healthy_repository() {
    let root = unique_temp_dir("doctor-healthy");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let report = doctor_repository(&layout);
        assert!(report.is_healthy());
        assert!(report.verification.is_some());
        assert_eq!(report.count_by_severity(DoctorSeverity::Error), 0);
        assert_eq!(report.count_by_severity(DoctorSeverity::Info), 1);
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn doctor_reports_trailing_partial_wal_warning() {
    let root = unique_temp_dir("doctor-partial-wal");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal_path = layout.default_queue_wal_path();
        assert!(std::fs::write(&wal_path, b"partial").is_ok());
        let report = doctor_repository(&layout);
        assert!(report.is_healthy());
        assert_eq!(report.count_by_severity(DoctorSeverity::Warning), 1);
        assert_eq!(
            report
                .verification
                .as_ref()
                .map(|summary| summary.trailing_partial_wal_bytes),
            Some(Some(7))
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 108 §D3.3/§D3.4, increment 3b, control 3 (the silent hole this half of the increment
/// exists to close): a second active session's trailing partial WAL was previously invisible to
/// `doctor` entirely -- neither `verification.trailing_partial_wal_bytes` (a `default`-only scalar)
/// nor anything else touched it. Plants `active/second/` by hand (nothing in the codebase creates a
/// second active yet, the same technique increment 2's own controls use) with a trailing partial
/// WAL tail, and asserts `doctor` now names it. `default`'s own warning-count assertion in the
/// sibling test above is untouched by this -- confirming the two paths do not interfere.
#[test]
fn doctor_reports_a_trailing_partial_wal_for_a_non_default_active_session()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-non-default-partial-wal");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    std::fs::write(layout.active_queue_wal_path("second"), b"partial")?;

    // This is a warning, not an error -- asserted explicitly, not left to an ambiguous "unhealthy"
    // check that a reader might misread as implying the opposite severity.
    let report = doctor_repository(&layout);
    assert!(report.is_healthy());
    assert!(report.issues.iter().any(|issue| issue.code
        == "PRIKK-DOCTOR-ACTIVE-SESSION-WAL-TRAILING-PARTIAL"
        && issue.message.contains("second")
        && issue.message.contains('7')));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The other new arm: a non-default active whose WAL fails to *replay* at all -- distinct from a
/// merely-partial tail -- is reported as `PRIKK-DOCTOR-ACTIVE-SESSION-WAL-UNREADABLE`, an error.
/// `default`'s equivalent failure is already caught by `verification`'s own `WalReplay` stage
/// outcome (`repair_repository_still_refuses_when_the_wal_replay_stage_fails` below covers that
/// side); a non-default active has no stage watching it, so this is the only thing that will.
///
/// **`Wal::replay()` tolerates almost everything by design** (RFC 102 Stage 2 isolate-and-continue:
/// a bad magic or checksum is a per-record item finding, resynced past, never a hard `Err` from
/// `decode_records` itself) -- so garbage bytes alone do not reach this arm; confirmed directly
/// before writing this test, not assumed. The one thing that does make `replay()` itself return
/// `Err` is `validate_read_schema` rejecting an otherwise well-formed record's envelope schema.
/// Constructed here via `encode_record_for_test` (the same structural, validation-bypassing encoder
/// `verify/tests/wal_cluster.rs` uses) with `schema_version` mutated to a value
/// `format.rs::admitted_schemas` does not accept for `Patch`.
#[test]
fn doctor_reports_an_unreadable_wal_for_a_non_default_active_session() -> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-non-default-unreadable-wal");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    let mut envelope = signed_patch_envelope();
    envelope.schema_version = 99;
    let record = WalRecord { seq: 1, envelope };
    std::fs::write(
        layout.active_queue_wal_path("second"),
        encode_record_for_test(&record)?,
    )?;

    let report = doctor_repository(&layout);
    assert!(!report.is_healthy());
    assert!(report.issues.iter().any(|issue| issue.code
        == "PRIKK-DOCTOR-ACTIVE-SESSION-WAL-UNREADABLE"
        && issue.message.contains("second")));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3c control 1: the gap increment 3b recorded, flipped. A non-default active
/// with WAL records but no ref-name metadata file was previously invisible to doctor entirely
/// (measured before this change: `healthy=true`, no code at all) -- now reported under its own code,
/// distinct from `default`'s `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING`.
#[test]
fn doctor_reports_missing_ref_metadata_for_a_non_default_active_with_wal_records()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-non-default-ref-metadata-missing");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    let record = WalRecord {
        seq: 1,
        envelope: signed_patch_envelope(),
    };
    std::fs::write(
        layout.active_queue_wal_path("second"),
        encode_record_for_test(&record)?,
    )?;
    // No `active/second/ref-name` file at all -- nothing creates one for a hand-planted active.

    let report = doctor_repository(&layout);
    assert!(!report.is_healthy());
    assert!(report.issues.iter().any(|issue| issue.code
        == "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MISSING"
        && issue.message.contains("second")));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3c control 2: parity means both arms `default` distinguishes, not just the
/// missing one -- malformed ref-name metadata (present, but not a valid `heads/<name>` local branch
/// ref) on a non-default active with WAL records reports
/// `PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MALFORMED`, distinct from the missing case above and
/// from `default`'s own `PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED`.
#[test]
fn doctor_reports_malformed_ref_metadata_for_a_non_default_active_with_wal_records()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-non-default-ref-metadata-malformed");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    let record = WalRecord {
        seq: 1,
        envelope: signed_patch_envelope(),
    };
    std::fs::write(
        layout.active_queue_wal_path("second"),
        encode_record_for_test(&record)?,
    )?;
    std::fs::write(
        layout.active_session_dir("second").join("ref-name"),
        b"not-a-local-branch-ref",
    )?;

    let report = doctor_repository(&layout);
    assert!(!report.is_healthy());
    assert!(report.issues.iter().any(|issue| issue.code
        == "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MALFORMED"
        && issue.message.contains("second")));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3c control 3: the easy mistake this handoff named explicitly -- a check that
/// fires on a healthy second active is worse than no check. An **empty** non-default WAL with no
/// ref-name metadata (the ordinary, unremarkable state of a hand-planted active nobody has used yet)
/// must stay quiet: `ActiveWalMetadataStatus::MissingForEmptyWal` is the one arm both `default`'s and
/// this function's own match treat as silent.
#[test]
fn doctor_stays_quiet_about_an_empty_non_default_active_with_no_ref_metadata()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-non-default-empty-no-metadata");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    std::fs::write(layout.active_queue_wal_path("second"), b"")?;

    let report = doctor_repository(&layout);
    assert!(report.is_healthy());
    assert!(!report.issues.iter().any(|issue| {
        issue
            .code
            .starts_with("PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA")
    }));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Recovery-listing-tolerance follow-up, the silent-hole control: now that
/// `unlock::list_held_locks` tolerates its own required directory (`refs/locks`) being absent
/// rather than erroring, something must still say so -- otherwise a repository missing it reports
/// "no locks held", confidently and wrongly, with nothing anywhere naming the real defect.
/// `doctor_repository` is that something, sourced from `RepositoryLayout::required_directories`
/// rather than a second hand-typed list of paths.
#[test]
fn doctor_reports_a_missing_refs_locks_directory_even_though_unlock_tolerates_it() {
    let root = unique_temp_dir("doctor-missing-refs-locks");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let refs_locks_dir = layout.refs_dir().join("locks");
        assert!(std::fs::remove_dir_all(&refs_locks_dir).is_ok());

        // `unlock` itself must not fail -- that is the fix this check exists alongside, not the
        // thing under test here, but worth pinning at the seam between the two.
        assert!(crate::list_held_locks(&layout).is_ok_and(|locks| locks.is_empty()));

        let report = doctor_repository(&layout);
        assert!(!report.is_healthy());
        // Derived from `refs_locks_dir`, not re-typed as a `/`-separated literal -- `dir.display()`
        // renders `refs\locks` on Windows, and a hardcoded `"refs/locks"` substring never matches
        // that (the `Windows mutation test suite` failure this fix addresses).
        let expected_fragment = refs_locks_dir.display().to_string();
        assert!(report.issues.iter().any(|issue| issue.code
            == "PRIKK-DOCTOR-MISSING-REQUIRED-DIRECTORY"
            && issue.severity == DoctorSeverity::Error
            && issue.message.contains(&expected_fragment)));
    }
    let _ = std::fs::remove_dir_all(root);
}

/// Same control, the other site: `refs/tmp` missing is what made
/// `refs/verify.rs::candidate_issues` fail outright before this follow-up (and, under
/// `--stop-on-first-error`, halt every later verification stage -- see
/// `verify::tests::stage_containment`). `verify_repository` itself now returns cleanly over it;
/// `doctor` is what still calls the repository unhealthy.
#[test]
fn doctor_reports_a_missing_refs_tmp_directory_even_though_verify_tolerates_it() {
    let root = unique_temp_dir("doctor-missing-refs-tmp");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let refs_tmp_dir = layout.refs_dir().join("tmp");
        assert!(std::fs::remove_dir_all(&refs_tmp_dir).is_ok());

        assert!(crate::verify_repository(&layout).is_ok_and(|report| !report.has_stage_failure()));

        let report = doctor_repository(&layout);
        assert!(!report.is_healthy());
        // Derived from `refs_tmp_dir`, not re-typed as a `/`-separated literal -- see the sibling
        // test above for why a hardcoded `"refs/tmp"` substring never matches on Windows.
        let expected_fragment = refs_tmp_dir.display().to_string();
        assert!(report.issues.iter().any(|issue| issue.code
            == "PRIKK-DOCTOR-MISSING-REQUIRED-DIRECTORY"
            && issue.severity == DoctorSeverity::Error
            && issue.message.contains(&expected_fragment)));
    }
    let _ = std::fs::remove_dir_all(root);
}

/// The `Some(_)` arm of the new required-directory check: a required directory location occupied by
/// something other than a directory is a distinct, real defect from absence, reported under its own
/// code -- a reader fixing "missing" by creating an empty directory would not think to first remove
/// the file already sitting in its way.
#[test]
fn doctor_reports_a_required_directory_occupied_by_a_file() {
    let root = unique_temp_dir("doctor-required-dir-wrong-type");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let locks_dir = layout.refs_dir().join("locks");
        assert!(std::fs::remove_dir_all(&locks_dir).is_ok());
        assert!(std::fs::write(&locks_dir, b"not a directory").is_ok());

        let report = doctor_repository(&layout);
        assert!(!report.is_healthy());
        assert!(
            report
                .issues
                .iter()
                .any(|issue| issue.code == "PRIKK-DOCTOR-REQUIRED-DIRECTORY-WRONG-TYPE")
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

/// DC-95 Stage 2 Level 1: a hard error inside a verification stage no longer aborts
/// `verify_repository` itself -- it's contained as a `Failed` outcome for that stage, so
/// `doctor_repository` still gets a full `RepositoryVerification` to report against (`verification`
/// is `Some`, not `None`), and the stage-outcome loop is what makes doctor refuse to call the
/// repository healthy, not the absence of a report.
///
/// **Two errors, not one, and that is the point of containment.** This fixture's Block is signed by
/// `maintainer_signature()` (aliased `legacy_maintainer_signature` here), never adopted via
/// `add_trusted_maintainer` -- the DC-95 Stage 1 "fake signer" hazard, deliberately reused here.
/// `verify_objects`'s Phase A reaches this Block's own trust check (pushing a real
/// `PRIKK-TRUST-POLICY-INVALID`/`PRIKK-TRUST-PUBLICATION-UNTRUSTED` finding into the shared
/// `PublicationTrustVerifier`) before Phase B's topological pass reaches the missing-patch error
/// that fails the whole `Objects` stage. Both survive: the trust finding because `trust_verifier` is
/// mutated by reference and outlives `verify_objects`'s own `Err` return, and the stage failure via
/// its own `StageOutcome`. Pre-Level-1, the second was invisible -- the first hard error any object
/// hit aborted everything after it, so only one of the two ever got reported. This is a strict
/// improvement in diagnosis, not a construction bug to route around.
#[test]
fn doctor_reports_verification_error() {
    let root = unique_temp_dir("doctor-bad-block");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut store = FileObjectStore::new(layout.clone());
        let missing_patch = sample_object_id("doctor-missing-patch");
        let payload = BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: vec![missing_patch],
            state_merkle_root: MerkleRoot([0_u8; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        };
        let payload_bytes = payload.to_canonical_bytes();
        assert!(payload_bytes.is_ok());
        if let Ok(payload_bytes) = payload_bytes {
            let mut block = ObjectEnvelope::unsigned(ObjectType::Block, 2, payload_bytes);
            assert!(block.add_signature(legacy_maintainer_signature()).is_ok());
            assert!(store.write_object(&block).is_ok());
            let report = doctor_repository(&layout);
            assert!(!report.is_healthy());
            assert_eq!(report.count_by_severity(DoctorSeverity::Error), 2);
            assert!(report.verification.is_some());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn doctor_repair_truncates_only_trailing_partial_wal() {
    let root = unique_temp_dir("doctor-repair-wal-tail");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());
        assert!(wal.append_patch(&signed_patch_envelope()).is_ok());
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(layout.default_queue_wal_path());
        assert!(file.is_ok());
        if let Ok(file) = file.as_mut() {
            assert!(file.write_all(b"partial").is_ok());
            assert!(file.sync_all().is_ok());
        }

        let before = doctor_repository(&layout);
        assert!(before.is_healthy());
        assert_eq!(
            before
                .verification
                .as_ref()
                .map(|summary| summary.trailing_partial_wal_bytes),
            Some(Some(7))
        );

        let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail());
        assert!(repair.is_ok());
        if let Ok(repair) = repair {
            assert_eq!(repair.wal_repair.truncated_bytes, 7);
            assert_eq!(repair.wal_repair.preserved_records, 1);
            assert!(repair.after.is_healthy());
            assert_eq!(
                repair
                    .after
                    .verification
                    .as_ref()
                    .map(|summary| summary.trailing_partial_wal_bytes),
                Some(Some(0))
            );
        }

        let replay = wal.replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert_eq!(replay.records.len(), 1);
            assert_eq!(replay.trailing_partial_bytes, 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn doctor_reports_non_empty_active_wal_missing_metadata_as_error() {
    let root = unique_temp_dir("doctor-active-metadata-missing");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert!(wal.append_patch(&signed_patch_envelope()).is_ok());

        let report = doctor_repository(&layout);
        assert!(!report.is_healthy());
        assert_eq!(report.count_by_severity(DoctorSeverity::Error), 1);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING" })
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn doctor_reports_empty_active_metadata_debris_as_warning() {
    let root = unique_temp_dir("doctor-active-metadata-debris");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        assert!(std::fs::write(layout.default_active_ref_name_path(), b"tags/v1").is_ok());

        let report = doctor_repository(&layout);
        assert!(report.is_healthy());
        assert_eq!(report.count_by_severity(DoctorSeverity::Warning), 1);
        assert!(
            report
                .issues
                .iter()
                .any(|issue| { issue.code == "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED-DEBRIS" })
        );
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn doctor_refuses_missing_main_ref_pointer_reconstruction() {
    let root = unique_temp_dir("doctor-repair-main-ref");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let maintainer_seed = [0x44_u8; 32];
        let maintainer =
            match Ed25519MaintainerSigner::from_seed("doctor-maintainer", &maintainer_seed) {
                Ok(signer) => signer,
                Err(error) => panic!("test maintainer signer should be constructible: {error}"),
            };
        let public_key = maintainer
            .public_key_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert!(add_trusted_maintainer(&layout, "doctor-maintainer", &public_key).is_ok());
        let mut object_store = FileObjectStore::new(layout.clone());
        // DC-95 Stage 2 Level 1: this must be the true empty-history root, not an arbitrary one --
        // an arbitrary root fails the Objects stage's own state-root check first, which pre-Level-1
        // hard-aborted verify_repository before ever reaching the missing-pointer condition this
        // fixture is actually meant to exercise, silently making this test check the wrong thing.
        // Stage containment surfaced this: both failures are now reported instead of just the first.
        let state_merkle_root = match derive_next_state_root(&object_store, None, &[]) {
            Ok(root) => root,
            Err(error) => panic!("empty-history state root should be derivable: {error}"),
        };
        let block_payload = BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        };
        let block = signed_publication_envelope(
            ObjectType::Block,
            block_payload.to_canonical_bytes().unwrap_or_default(),
            &maintainer,
        );
        let target = block.object_id();
        assert!(object_store.write_object(&block).is_ok());
        let store = crate::RefStore::new(layout.clone());
        let ref_state_payload = RefStatePayload {
            ref_name: "heads/main".to_string(),
            kind: RefKind::Branch,
            target_object_id: target,
            update_seq: 1,
            previous_ref_state_id: None,
            required_attestation_ids: Vec::new(),
            closed: false,
        };
        let ref_state = signed_publication_envelope(
            ObjectType::RefState,
            ref_state_payload.to_canonical_bytes().unwrap_or_default(),
            &maintainer,
        );
        let ref_state_id = ref_state.object_id();
        let ref_update_payload = RefUpdatePayload {
            ref_name: "heads/main".to_string(),
            old_ref_state_id: None,
            new_ref_state_id: ref_state_id,
            new_target_object_id: target,
            update_seq: 1,
            created_at: 0,
            author_key_id: "doctor-maintainer".to_string(),
        };
        let ref_update = signed_publication_envelope(
            ObjectType::RefUpdate,
            ref_update_payload.to_canonical_bytes().unwrap_or_default(),
            &maintainer,
        );
        let publication = crate::RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        };
        assert!(store.publish(&publication).is_ok());
        assert!(
            crate::refs::remove_pointer_entries_for_test(
                &layout,
                crate::foundation::layout::ref_name_key_bytes("heads/main"),
            )
            .is_ok()
        );

        let before = doctor_repository(&layout);
        assert!(!before.is_healthy());
        assert_eq!(before.count_by_severity(DoctorSeverity::Error), 1);

        let repair = repair_repository(&layout, DoctorRepairOptions::reconstruct_main_ref());
        assert!(repair.is_err());
        assert_eq!(store.read_current_ref_state_id("heads/main"), Ok(None));
    }
    let _ = std::fs::remove_dir_all(root);
}

/// RFC 108 increment 3d, §2.1: a busy active-session lock used to fail the whole `repair_repository`
/// call (`Err(LockConflict)`); now it is expressed as that one active session being `Skipped`, with
/// the reason naming the lock conflict -- the requirement itself ("you cannot mutate a WAL without
/// its lock") is unchanged, only how "cannot mutate right now" is reported now that a busy `default`
/// must not stop some *other* active session from being repaired in the same call (§D3.3). With only
/// `default` on disk here there is no other active session to demonstrate that half with -- see
/// `repair_repository_does_not_fail_the_whole_run_when_one_active_sessions_lock_is_busy` for that.
#[test]
fn doctor_repair_requires_active_lock_before_wal_mutation() -> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-active-lock");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::write(layout.default_queue_wal_path(), b"partial")?;
    let before = std::fs::read(layout.default_queue_wal_path())?;
    let active_lock = crate::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;

    let report = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert!(report.active_repairs.iter().any(|outcome| {
        outcome.active_session.to_str() == Some(DEFAULT_ACTIVE_NAME)
            && matches!(&outcome.status, ActiveSessionRepairStatus::Skipped { reason } if reason.contains("lock"))
    }));
    assert_eq!(report.wal_repair.truncated_bytes, 0);
    assert_eq!(std::fs::read(layout.default_queue_wal_path())?, before);

    drop(active_lock);
    let report = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert_eq!(report.wal_repair.truncated_bytes, before.len());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn doctor_rechecks_publication_guard_after_acquiring_active_lock() -> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-under-lock-guard");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::write(layout.default_queue_wal_path(), b"partial")?;
    let before = std::fs::read(layout.default_queue_wal_path())?;
    let active_lock = crate::ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    let candidate = layout.ref_tmp_path("heads/main");
    let parent = candidate
        .parent()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("candidate has no parent".to_string()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&candidate, b"candidate")?;

    assert!(repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail()).is_err());
    drop(active_lock);
    let error = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())
        .err()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("repair unexpectedly ran".to_string()))?;
    assert!(
        error
            .to_string()
            .contains("repository mutation is blocked by incomplete ref publication")
    );
    // RFC 132 part 2: an incomplete publication is a caller precondition, not a lock -- nothing is
    // held and no other writer is racing this one.
    assert!(
        error.to_string().starts_with("precondition not met:"),
        "unexpected error: {error}"
    );
    assert_eq!(std::fs::read(layout.default_queue_wal_path())?, before);
    assert_eq!(std::fs::read(&candidate)?, b"candidate");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3b, §2.3's second carried item: `repair_repository`'s refusal message used to
/// say *"repository verification has errors"* unconditionally, but `before.is_healthy()` can be
/// `false` from a doctor-level check that never calls `verify_repository` at all --
/// `push_missing_required_directory_issues` (increment 2) is exactly that: it pushes its
/// `DoctorIssue::error` before `verify_repository` is even invoked. Removes `refs/locks` (a required
/// directory) to construct precisely that case -- `verify_repository` itself would report a clean
/// pass here (nothing it checks reads `refs/locks`) -- and pins that the corrected message no longer
/// blames "repository verification," which is exactly what a reader would have gone looking at and
/// found nothing wrong with.
#[test]
fn repair_repository_refusal_message_does_not_blame_verify_for_a_doctor_level_check()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-repair-refusal-message");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::remove_dir_all(layout.refs_dir().join("locks"))?;

    let before = doctor_repository(&layout);
    assert!(!before.is_healthy());
    assert!(
        before
            .issues
            .iter()
            .any(|issue| issue.code == "PRIKK-DOCTOR-MISSING-REQUIRED-DIRECTORY"),
        "the unhealthy verdict here must come from the doctor-level check, not verify -- \
         confirming the fixture actually exercises the case this message fix is for"
    );

    let error = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())
        .err()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("repair unexpectedly ran".to_string()))?;
    let message = error.to_string();
    assert!(
        !message.contains("repository verification"),
        "message must not blame `verify` for a doctor-level check: {message}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 2 Level 1 acceptance criterion 3 (design §11, implementation handoff §6.3):
/// `repair_repository` must still refuse for every defect it refused pre-Level-1, proven per stage
/// by test -- the assertion is the refusal itself, not merely that a finding is present. This is
/// the ruling's own binding constraint (`stage-2-prerequisite-3.3-3.4-ruling-v1.md` §2): a hard
/// `Err` used to make `repair_repository` refuse "for free," and that protection now comes from the
/// stage-outcome loop in `doctor_repository` mapping every non-`Evaluated` stage to
/// `DoctorSeverity::Error` uniformly, not from `verify_repository` itself returning `Err`.
///
/// Covers two stages deliberately not the one `doctor_reports_verification_error` above already
/// exercises (`Objects`), to demonstrate the refusal holds regardless of which stage failed --
/// `Refs` (a dangling ref target) and `WalReplay` (a checksum-corrupted active record).
///
/// **Function name now describes Level 1's framing, not Level 2's.** DC-95 Stage 2 Level 2 (refs
/// half) contains a dangling ref target one level further, to the specific ref's own pointer
/// outcome -- the `Refs` `StageOutcome` itself now reads `Evaluated` for this exact fixture, not
/// `Failed`. The test's own two assertions (`!before.is_healthy()`, `repair.is_err()`) never
/// depended on which granularity the defect surfaced at, so it continues to prove criterion 4 at
/// item granularity without modification -- only this comment needed correcting, not the test.
#[test]
fn repair_repository_still_refuses_when_the_refs_stage_fails() -> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-repair-refuses-refs");
    let layout = RepositoryLayout::init(root.clone())?;
    let maintainer_seed = [0x45_u8; 32];
    let maintainer =
        match Ed25519MaintainerSigner::from_seed("doctor-refs-maintainer", &maintainer_seed) {
            Ok(signer) => signer,
            Err(error) => panic!("test maintainer signer should be constructible: {error}"),
        };
    let public_key = maintainer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    assert!(add_trusted_maintainer(&layout, "doctor-refs-maintainer", &public_key).is_ok());
    let mut object_store = FileObjectStore::new(layout.clone());
    let block_payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: match derive_next_state_root(&object_store, None, &[]) {
            Ok(root) => root,
            Err(error) => panic!("empty-history state root should be derivable: {error}"),
        },
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block = signed_publication_envelope(
        ObjectType::Block,
        block_payload.to_canonical_bytes().unwrap_or_default(),
        &maintainer,
    );
    let target = block.object_id();
    assert!(object_store.write_object(&block).is_ok());
    let store = crate::RefStore::new(layout.clone());
    let ref_state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state = signed_publication_envelope(
        ObjectType::RefState,
        ref_state_payload.to_canonical_bytes().unwrap_or_default(),
        &maintainer,
    );
    let ref_state_id = ref_state.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: target,
        update_seq: 1,
        created_at: 0,
        author_key_id: "doctor-refs-maintainer".to_string(),
    };
    let ref_update = signed_publication_envelope(
        ObjectType::RefUpdate,
        ref_update_payload.to_canonical_bytes().unwrap_or_default(),
        &maintainer,
    );
    let publication = crate::RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    };
    assert!(store.publish(&publication).is_ok());
    // A dangling ref target: remove the just-published RefState's target Block's index entry.
    // Containers are append-only, so there is no direct "delete one object" equivalent to the
    // pre-Stage-3 `std::fs::remove_file` this replaces.
    assert!(crate::foundation::index::remove_index_entry_for_test(&layout, target).is_ok());

    let before = doctor_repository(&layout);
    assert!(!before.is_healthy());
    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail());
    assert!(repair.is_err());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// See `repair_repository_still_refuses_when_the_refs_stage_fails` above for the acceptance
/// criterion this proves. Here: `WalReplay` itself fails (a checksum-corrupted active record), which
/// per Step 0's dependency graph also makes five other stages `NotEvaluated` -- confirming the
/// refusal holds even when most of the pipeline never ran.
#[test]
fn repair_repository_still_refuses_when_the_wal_replay_stage_fails() -> prikk_error::Result<()> {
    let root = unique_temp_dir("doctor-repair-refuses-wal-replay");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    assert!(write_active_ref_metadata(&layout, "heads/main").is_ok());
    assert!(wal.append_patch(&signed_patch_envelope()).is_ok());
    let mut bytes = std::fs::read(wal.path())?;
    if let Some(last_byte) = bytes.last_mut() {
        *last_byte ^= 0x01;
    }
    std::fs::write(wal.path(), &bytes)?;

    let before = doctor_repository(&layout);
    assert!(!before.is_healthy());
    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail());
    assert!(repair.is_err());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3d control 1 (first row): the handoff's own measurement, quoted --
/// `second=unreadable-wal -> repair of default: REFUSED`, before this increment. `default` carries
/// a genuinely repairable trailing partial tail; `second`'s WAL is genuinely unreadable (the same
/// inadmissible-`schema_version` construction increment 3c's own tests use, since garbage bytes
/// alone never make `Wal::replay()` itself return `Err` -- RFC 102 Stage 2's isolate-and-continue
/// design). `default` must still repair.
#[test]
fn repair_repository_still_repairs_default_when_second_wal_is_unreadable() -> prikk_error::Result<()>
{
    let root = unique_temp_dir("repair-default-ok-second-unreadable");
    let layout = RepositoryLayout::init(root.clone())?;
    let default_wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    write_active_ref_metadata(&layout, "heads/main")?;
    default_wal.append_patch(&signed_patch_envelope())?;
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(layout.default_queue_wal_path())?;
        file.write_all(b"partial")?;
        file.sync_all()?;
    }
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    let mut envelope = signed_patch_envelope();
    envelope.schema_version = 99;
    let record = WalRecord { seq: 1, envelope };
    std::fs::write(
        layout.active_queue_wal_path("second"),
        encode_record_for_test(&record)?,
    )?;

    let before = doctor_repository(&layout);
    assert!(
        !before.is_healthy(),
        "second's unreadable WAL is a real error: {before:?}"
    );

    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert_eq!(
        repair.wal_repair.truncated_bytes, 7,
        "default must still repair despite second being broken: {repair:?}"
    );
    assert!(repair.active_repairs.iter().any(|outcome| {
        outcome.active_session.to_str() == Some(DEFAULT_ACTIVE_NAME)
            && matches!(outcome.status, ActiveSessionRepairStatus::Repaired(_))
    }));
    assert!(repair.active_repairs.iter().any(|outcome| {
        outcome.active_session.to_str() == Some("second")
            && matches!(outcome.status, ActiveSessionRepairStatus::Skipped { .. })
    }));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3d control 1 (second row): `second=missing-ref-metadata -> repair of default:
/// REFUSED`, before this increment. `second` has WAL records but no ref-name metadata at all
/// (nothing creates one for a hand-planted active); `default` again carries a repairable trailing
/// partial tail. `default` must still repair.
#[test]
fn repair_repository_still_repairs_default_when_second_ref_metadata_is_missing()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("repair-default-ok-second-missing-metadata");
    let layout = RepositoryLayout::init(root.clone())?;
    let default_wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    write_active_ref_metadata(&layout, "heads/main")?;
    default_wal.append_patch(&signed_patch_envelope())?;
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(layout.default_queue_wal_path())?;
        file.write_all(b"partial")?;
        file.sync_all()?;
    }
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    let record = WalRecord {
        seq: 1,
        envelope: distinct_patch_envelope("second-record")?,
    };
    std::fs::write(
        layout.active_queue_wal_path("second"),
        encode_record_for_test(&record)?,
    )?;
    // No `active/second/ref-name` file at all.

    let before = doctor_repository(&layout);
    assert!(
        !before.is_healthy(),
        "second's missing ref metadata is a real error: {before:?}"
    );

    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert_eq!(
        repair.wal_repair.truncated_bytes, 7,
        "default must still repair despite second being broken: {repair:?}"
    );
    assert!(repair.active_repairs.iter().any(|outcome| {
        outcome.active_session.to_str() == Some("second")
            && matches!(outcome.status, ActiveSessionRepairStatus::Skipped { .. })
    }));

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3d control 2 -- §2.2's mirror direction, named in the handoff as "the control
/// most likely to be missing, because the bug it catches is invisible if you only test the
/// direction §1 names." `default` itself has a blocking issue (WAL records present, ref-name
/// metadata never written); `second` is healthy with its own repairable trailing partial. `second`
/// must still repair -- the direction an implementation that attributed only the *new* per-active
/// issues, and left `default`'s own untouched, would still get wrong.
#[test]
fn repair_repository_still_repairs_second_when_default_ref_metadata_is_missing()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("repair-second-ok-default-broken");
    let layout = RepositoryLayout::init(root.clone())?;
    let default_wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    default_wal.append_patch(&signed_patch_envelope())?;
    // No `write_active_ref_metadata` call for `default` -- WAL has a record, metadata is Missing.

    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    std::fs::write(layout.active_queue_wal_path("second"), b"partial")?;

    let before = doctor_repository(&layout);
    assert!(!before.is_healthy());
    assert!(before.issues.iter().any(|issue| {
        issue.code == "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING"
            && issue.active_session.as_deref() == Some(std::ffi::OsStr::new(DEFAULT_ACTIVE_NAME))
    }));

    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert!(repair.active_repairs.iter().any(|outcome| {
        outcome.active_session.to_str() == Some(DEFAULT_ACTIVE_NAME)
            && matches!(outcome.status, ActiveSessionRepairStatus::Skipped { .. })
    }));
    let second_outcome = repair
        .active_repairs
        .iter()
        .find(|outcome| outcome.active_session.to_str() == Some("second"))
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("second not present in active_repairs".to_string())
        })?;
    match &second_outcome.status {
        ActiveSessionRepairStatus::Repaired(wal_repair) => {
            assert_eq!(wal_repair.truncated_bytes, 7);
        }
        other => {
            return Err(prikk_error::PrikkError::Integrity(format!(
                "expected second to repair, got {other:?}"
            )));
        }
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3d control 3 -- §D3.3's own demonstration, as the RFC words it: "show a
/// Workspace's WAL recovering independently of every other." Two active sessions, each with its own
/// trailing partial tail *and its own distinct patch record* (not the same fixed content
/// `signed_patch_envelope()` always produces) -- repair must recover each one's own record and must
/// not swap or mix them. Asserted on **preserved record identity**, not merely counts (DC-66's own
/// reasoning applies here exactly: "N records preserved" does not say whose work survived).
#[test]
fn repair_repository_recovers_each_active_sessions_own_records_independently()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("repair-two-actives-independent-recovery");
    let layout = RepositoryLayout::init(root.clone())?;

    let default_wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    write_active_ref_metadata(&layout, "heads/main")?;
    let default_envelope = distinct_patch_envelope("default-record")?;
    let default_patch_id = default_envelope.object_id();
    default_wal.append_patch(&default_envelope)?;
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(layout.default_queue_wal_path())?;
        file.write_all(b"partial")?;
        file.sync_all()?;
    }

    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    // `write_active_ref_metadata` is `default`-only by design (RFC 108 increment 3c, §2.2) -- write
    // `second`'s own ref-name metadata directly, the same way its WAL is hand-planted.
    std::fs::write(
        layout.active_session_dir("second").join("ref-name"),
        b"heads/other",
    )?;
    let second_envelope = distinct_patch_envelope("second-record")?;
    let second_patch_id = second_envelope.object_id();
    assert_ne!(
        default_patch_id, second_patch_id,
        "the two fixtures must actually be distinct for this control to prove anything"
    );
    let second_record = WalRecord {
        seq: 1,
        envelope: second_envelope,
    };
    let mut second_bytes = encode_record_for_test(&second_record)?;
    second_bytes.extend_from_slice(b"partial");
    std::fs::write(layout.active_queue_wal_path("second"), &second_bytes)?;

    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    for (name, expected_patch_id) in [
        (DEFAULT_ACTIVE_NAME, default_patch_id),
        ("second", second_patch_id),
    ] {
        let outcome = repair
            .active_repairs
            .iter()
            .find(|outcome| outcome.active_session.to_str() == Some(name))
            .ok_or_else(|| {
                prikk_error::PrikkError::Integrity(format!("{name} not present in active_repairs"))
            })?;
        match &outcome.status {
            ActiveSessionRepairStatus::Repaired(wal_repair) => {
                assert_eq!(
                    wal_repair.truncated_bytes, 7,
                    "{name}'s own trailing partial tail"
                );
                assert_eq!(
                    wal_repair.preserved_patch_ids,
                    vec![expected_patch_id],
                    "{name} must recover exactly its own record, never the other active \
                     session's"
                );
            }
            other => {
                return Err(prikk_error::PrikkError::Integrity(format!(
                    "expected {name} to repair, got {other:?}"
                )));
            }
        }
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3d control 4 -- §2.1: a busy lock on one **non-default** active session must
/// not fail the run. `second`'s lock is held for the whole call; `default` must still repair, and
/// `second` must be reported as skipped with a reason naming the lock, not silently dropped or
/// treated as a run-wide failure.
#[test]
fn repair_repository_does_not_fail_the_whole_run_when_one_active_sessions_lock_is_busy()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("repair-second-lock-busy");
    let layout = RepositoryLayout::init(root.clone())?;
    write_active_ref_metadata(&layout, "heads/main")?;
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&signed_patch_envelope())?;
    {
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(layout.default_queue_wal_path())?;
        file.write_all(b"partial")?;
        file.sync_all()?;
    }
    std::fs::create_dir_all(layout.active_session_dir("second"))?;
    std::fs::write(layout.active_queue_wal_path("second"), b"partial")?;
    let second_lock = crate::ActiveLock::acquire(&layout, "second")?;

    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert_eq!(
        repair.wal_repair.truncated_bytes, 7,
        "default must still repair: {repair:?}"
    );
    assert!(repair.active_repairs.iter().any(|outcome| {
        outcome.active_session.to_str() == Some("second")
            && matches!(&outcome.status, ActiveSessionRepairStatus::Skipped { reason } if reason.contains("lock"))
    }));

    drop(second_lock);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 108 increment 3d control 5: repository-wide damage must still refuse the **entire** run, not
/// repair a subset -- "a repair that proceeds over [repository-wide damage] would be a worse bug
/// than the one you are fixing" (§4). Removing a required directory is increment 2's own
/// silent-hole check (`push_missing_required_directory_issues`), which pushes an `active_session:
/// None` error -- unrelated to any one active session, so it must refuse before the per-active loop
/// is even reached.
#[test]
fn repair_repository_still_refuses_everything_for_repository_wide_damage() -> prikk_error::Result<()>
{
    let root = unique_temp_dir("repair-repository-wide-damage");
    let layout = RepositoryLayout::init(root.clone())?;
    std::fs::remove_dir_all(layout.refs_dir().join("locks"))?;

    let error = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())
        .err()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("repair unexpectedly ran".to_string()))?;
    assert!(!error.to_string().is_empty());

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

fn signed_publication_envelope(
    object_type: ObjectType,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> ObjectEnvelope {
    let schema_version = if object_type == ObjectType::Block {
        2
    } else {
        1
    };
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    let signature = real_maintainer_signature(signer, object_type, object_id);
    assert!(signature.is_ok());
    if let Ok(signature) = signature {
        assert!(envelope.add_signature(signature).is_ok());
    }
    envelope
}
