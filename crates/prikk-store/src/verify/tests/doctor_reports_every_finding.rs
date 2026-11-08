//! 0.57.0 step 1a: **`doctor` reports every finding `verify` fails on.** Five findings made `verify`
//! exit non-zero while `doctor` said nothing (the inventory is in that part's report): a merge block
//! whose recorded baseline is not a common ancestor, an active WAL whose sequence does not increase,
//! a record before the last acknowledged one that was substituted (the witness's running hash), a
//! received ref that failed, and a lifecycle-state cache that disagrees with an independent replay
//! (the last one is tested through the binary, in `dc64_baseline_cache.rs`). Each test builds the
//! finding the way the existing `verify` test for it does, asserts `verify` fails on it, and asserts
//! `doctor` now reports it under its own code, with its severity, its scope, and a way out.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload,
};

use super::{write_create_child, write_signed_block};
use crate::commit_boundary::worktree_patch::commit_worktree_changes_signed;
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::test_gates::test_support::{sample_object_id, unique_temp_dir};
use crate::wal::{Wal, WalRecord, encode_record_for_test};
use crate::{
    DoctorIssue, DoctorSeverity, Ed25519AuthorSigner, Ed25519MaintainerSigner, FileObjectStore,
    MaintainerSigner, ObjectWriter, WorktreePatchCommitOptions, add_trusted_maintainer,
    derive_next_state_root, doctor_repository, maintainer_signature, verify_repository,
};

fn issue<'a>(issues: &'a [DoctorIssue], code: &str) -> &'a DoctorIssue {
    issues
        .iter()
        .find(|issue| issue.code == code)
        .unwrap_or_else(|| panic!("doctor reports no {code}: {issues:#?}"))
}

fn author() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("step1a-doctor-reports", &[0x5a_u8; 32]).unwrap()
}

fn commit(layout: &RepositoryLayout, path: &str, body: &[u8]) {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "step1a",
        WorktreePatchCommitOptions::file_level(),
        &author(),
    )
    .unwrap();
}

/// **Merge-baseline divergence → `PRIKK-DOCTOR-MERGE-BASELINE-DIVERGENCE`, an error** (a signed block's
/// claim is false: history is affected), repository-wide, naming a sound copy as the way out.
/// **Perturb:** drop the `merge_baseline_divergences` loop from `doctor_repository` -- this goes red.
#[test]
fn doctor_reports_a_merge_block_whose_baseline_is_not_a_common_ancestor() -> Result<()> {
    let root = unique_temp_dir("step1a-merge-baseline");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());
    let genesis_root = derive_next_state_root(&store, None, &[])?;
    let genesis = write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: Vec::new(),
            kind: BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: genesis_root,
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    )?;
    let mainline = write_create_child(&mut store, genesis, "mainline.txt", 0x10)?;
    let secondary = write_create_child(&mut store, genesis, "secondary.txt", 0x20)?;
    let mut parents = vec![mainline, secondary];
    parents.sort();
    let forged = sample_object_id("step1a-forged-baseline");
    let merge_root = derive_next_state_root(&store, Some(mainline), &[])?;
    let merge_block = write_signed_block(
        &mut store,
        &BlockPayload {
            parent_block_ids: parents,
            kind: BlockKind::Merge,
            patch_ids: Vec::new(),
            state_merkle_root: merge_root,
            snapshot_blob_ref: None,
            mainline_parent_id: Some(mainline),
            merge_baseline_block_id: Some(forged),
        },
    )?;

    assert!(verify_repository(&layout)?.has_merge_baseline_divergence());
    let report = doctor_repository(&layout);
    let found = issue(&report.issues, "PRIKK-DOCTOR-MERGE-BASELINE-DIVERGENCE");
    assert_eq!(found.severity, DoctorSeverity::Error);
    assert_eq!(
        found.active_session, None,
        "repository-wide: it is signed history"
    );
    assert!(
        found.message.contains(&merge_block.to_string()),
        "{}",
        found.message
    );
    assert!(
        found.message.contains(&forged.to_string()),
        "{}",
        found.message
    );
    assert_eq!(
        found.recommendation,
        crate::WayOut::MergeBaseline.sentence(),
        "doctor says the shared sentence"
    );
    assert!(
        crate::ways_out(&verify_repository(&layout).unwrap())
            .iter()
            .any(|(_, kind)| *kind == crate::WayOut::MergeBaseline),
        "verify's way-out list carries the same kind"
    );
    assert!(
        found.recommendation.contains("prikk setup <new-dir>"),
        "the sound-copy sequence starts with `setup`: {}",
        found.recommendation
    );
    assert!(
        !report.is_healthy(),
        "doctor no longer reads healthy over a finding verify fails on"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Three real commits, then every record re-encoded fresh by `mutate` (each frame keeps a correct
/// checksum for its possibly substituted body -- forged but structurally sound, not damage).
fn forged_wal(
    name: &str,
    mutate: impl Fn(usize, &WalRecord, &[WalRecord]) -> WalRecord,
) -> RepositoryLayout {
    let layout = RepositoryLayout::init(unique_temp_dir(name)).unwrap();
    commit(&layout, "a.txt", b"one");
    commit(&layout, "b.txt", b"two");
    commit(&layout, "c.txt", b"three");
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let genuine = wal.replay().unwrap();
    assert_eq!(genuine.records.len(), 3);
    let mut bytes = Vec::new();
    for (index, record) in genuine.records.iter().enumerate() {
        bytes.extend(encode_record_for_test(&mutate(index, record, &genuine.records)).unwrap());
    }
    std::fs::write(wal.path(), &bytes).unwrap();
    layout
}

/// **A substituted earlier record → `PRIKK-DOCTOR-COMMIT-WITNESS-SUBSTITUTED-EARLIER-RECORD`, an
/// error scoped to the `default` active session** (RFC 166 D4, row 10: the last record is itself
/// sound, so the verdict reads `Healthy` and only the running hash sees it).
/// **Perturb:** drop the `has_commit_witness_substituted_earlier_record` block -- this goes red.
#[test]
fn doctor_reports_a_substituted_earlier_record_that_only_the_running_hash_sees() {
    let layout = forged_wal("step1a-running-hash", |index, record, all| WalRecord {
        seq: record.seq,
        envelope: if index == 1 {
            all[2].envelope.clone()
        } else {
            record.envelope.clone()
        },
    });
    let verification = verify_repository(&layout).unwrap();
    assert!(verification.has_commit_witness_substituted_earlier_record());
    let report = doctor_repository(&layout);
    let found = issue(
        &report.issues,
        "PRIKK-DOCTOR-COMMIT-WITNESS-SUBSTITUTED-EARLIER-RECORD",
    );
    assert_eq!(found.severity, DoctorSeverity::Error);
    assert_eq!(
        found.active_session.as_deref(),
        Some(std::ffi::OsStr::new(DEFAULT_ACTIVE_NAME)),
        "scoped to the one session it is about"
    );
    assert_eq!(
        found.recommendation,
        crate::WayOut::SubstitutedRecord.sentence(),
        "doctor says the shared sentence"
    );
    assert!(
        crate::ways_out(&verify_repository(&layout).unwrap())
            .iter()
            .any(|(_, kind)| *kind == crate::WayOut::SubstitutedRecord),
        "verify's way-out list carries the same kind"
    );
    assert!(
        found.recommendation.contains("prikk setup <new-dir>"),
        "the sound-copy sequence starts with `setup`: {}",
        found.recommendation
    );
    let _ = std::fs::remove_dir_all(layout.root());
}

/// **A WAL whose sequence does not increase → `PRIKK-DOCTOR-ACTIVE-WAL-ORDERING`, an error scoped to
/// `default`.** Record 3 is re-encoded with sequence 2 (a duplicate).
/// **Perturb:** drop the `active_wal_ordering_issues` loop -- this goes red.
#[test]
fn doctor_reports_an_active_wal_whose_sequence_does_not_increase() {
    let layout = forged_wal("step1a-wal-ordering", |index, record, _| WalRecord {
        seq: if index == 2 { 2 } else { record.seq },
        envelope: record.envelope.clone(),
    });
    let verification = verify_repository(&layout).unwrap();
    assert!(
        verification.has_active_wal_ordering_issue(),
        "the forged duplicate is an ordering issue: {:?}",
        verification.active_wal_ordering_issues
    );
    let report = doctor_repository(&layout);
    let found = issue(&report.issues, "PRIKK-DOCTOR-ACTIVE-WAL-ORDERING");
    assert_eq!(found.severity, DoctorSeverity::Error);
    assert_eq!(
        found.active_session.as_deref(),
        Some(std::ffi::OsStr::new(DEFAULT_ACTIVE_NAME))
    );
    assert!(found.message.contains("sequence 2"), "{}", found.message);
    assert_eq!(
        found.recommendation,
        crate::WayOut::WalOrdering.sentence(),
        "doctor says the shared sentence"
    );
    assert!(
        crate::ways_out(&verify_repository(&layout).unwrap())
            .iter()
            .any(|(_, kind)| *kind == crate::WayOut::WalOrdering),
        "verify's way-out list carries the same kind"
    );
    assert!(
        found.recommendation.contains("prikk setup <new-dir>"),
        "the sound-copy sequence starts with `setup`: {}",
        found.recommendation
    );
    // The one WAL verb that could look relevant does not clear it: the recommendation does not send
    // the user there (it acts on an acknowledged commit, and the ordering violation outlives it).
    crate::discard_damaged_commits(&layout).unwrap();
    assert!(
        verify_repository(&layout)
            .unwrap()
            .has_active_wal_ordering_issue(),
        "`--discard-damaged-commits` does not clear an ordering violation"
    );
    let _ = std::fs::remove_dir_all(layout.root());
}

/// **A failed received ref → `PRIKK-DOCTOR-RECEIVED-REF-DAMAGED`, a warning** (a pointer to what
/// another repository sent, not local history), whose way out is a prikk command that works: the plan
/// and then `received remove` clear it and `verify` stops failing.
/// **Perturb:** drop the `received_ref_item_outcomes` loop -- this goes red.
#[test]
fn doctor_reports_a_failed_received_ref_and_received_remove_clears_it() -> Result<()> {
    let layout = RepositoryLayout::init(unique_temp_dir("step1a-received-ref"))?;
    let mut objects = FileObjectStore::new(layout.clone());
    let signer = Ed25519MaintainerSigner::from_seed("step1a-received", &[0x93; 32])?;
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex)?;
    let payload = RefStatePayload {
        ref_name: "remotes/heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: ObjectId::from_bytes([0x94; 32]), // a Block nothing shipped
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(&signer, ObjectType::RefState, id)?)?;
    let ref_state_id = objects.write_object(&envelope)?;
    crate::received::write_received_pointer(&layout, "remotes/heads/main", ref_state_id)?;

    assert!(verify_repository(&layout)?.has_item_failure());
    let report = doctor_repository(&layout);
    let found = issue(&report.issues, "PRIKK-DOCTOR-RECEIVED-REF-DAMAGED");
    assert_eq!(found.severity, DoctorSeverity::Warning);
    assert_eq!(
        found.recommendation,
        crate::WayOut::ReceivedRef("remotes/heads/main".to_string()).sentence()
    );
    assert!(
        found
            .recommendation
            .contains("prikk received remove remotes/heads/main --plan-only"),
        "{}",
        found.recommendation
    );
    assert!(
        crate::ways_out(&verify_repository(&layout)?)
            .iter()
            .any(|(_, kind)| matches!(kind, crate::WayOut::ReceivedRef(_))),
        "verify's way-out list carries the same kind"
    );

    // The named commands: the plan writes nothing, the real run clears the failure.
    crate::plan_remove_received_pointer(&layout, "remotes/heads/main")?;
    assert!(
        verify_repository(&layout)?.has_item_failure(),
        "a plan changes nothing"
    );
    crate::remove_received_pointer(&layout, "remotes/heads/main")?;
    let after = verify_repository(&layout)?;
    assert!(!after.has_item_failure(), "verify stops failing: {after:?}");
    assert!(
        doctor_repository(&layout)
            .issues
            .iter()
            .all(|issue| issue.code != "PRIKK-DOCTOR-RECEIVED-REF-DAMAGED")
    );
    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}
