//! DC-95 Stage 1, round 11: the `wal.rs` / `verify_wal_persistence` / `rollback_verify.rs` cluster
//! (classified inventory §4, all 4 remaining rows).
//!
//! **Two of the eight sub-checks the prerequisite report listed for this cluster are provably
//! unreachable, discovered by code inspection rather than by a failed construction attempt:**
//!
//! - `verify_rollback_patch_envelope`'s own `envelope.object_type != ObjectType::Patch` check
//!   (`rollback_verify.rs`) can never fire from `verify_repository`'s call path. The function's
//!   first line is `if !is_rollback_draft_envelope(envelope)? { return Ok(false); }`, and
//!   `is_rollback_draft_envelope` (`rollback_draft.rs`) itself returns `Ok(false)` immediately
//!   whenever `object_type != ObjectType::Patch`. Reaching past that guard already proves
//!   `object_type == Patch`; the second check is dead code, not merely hard to construct.
//! - The same function's `decoded.is_empty()` check is dead code for a stronger reason: its own
//!   input, `decode_patch_operations`, already returns `Err("Patch missing operations")` before
//!   ever producing an empty `Vec` (`patch_replay/decode.rs`'s own `if operations.is_empty() {
//!   return Err(...) }`, ahead of the `?` at the call site). `decoded` can never be observed empty
//!   by its caller.
//!
//! A third sub-check (`require_rollback_author_signature`'s "wrong algorithm" arm) is unreachable
//! for a stronger reason still: `SignatureAlgorithm` has exactly one variant (`Ed25519`), so
//! `signature.algorithm != SignatureAlgorithm::Ed25519` cannot be true for any value constructible
//! in safe Rust, regardless of call path. Kept, untested, ruled on here rather than attempted —
//! matching round 6's duplicate-pointer/log-identity precedent for genuinely impossible inputs.
//!
//! The remaining seven sub-checks were all end-to-end reachable and covered below, one fixture
//! each: `Wal::replay()`'s checksum mismatch (`wal.rs`); `verify_wal_persistence`'s Patch-type
//! check (`verify.rs`); `verify_rollback_patch_envelope`'s decode and apply-support arms
//! (`rollback_verify.rs`, `patch_replay/decode.rs`); and `require_rollback_author_signature`'s
//! missing-signature, legacy-marker-key-id, and wrong-length arms (`rollback_verify.rs`).
//!
//! **RFC 103: the wrong-length arm's end-to-end test is removed.** It was reachable only under
//! format-1 (its own doc comment already said so); with format-1 retired, `RepositoryLayout::open`
//! refuses the fixture before `verify_repository` is ever called. The arm itself is kept, per round
//! 6's ruling on unreachable checks -- see its own comment in `rollback_verify.rs` -- so six of the
//! seven remain covered here, not seven.
//!
//! Every raw-byte fixture below follows `verify/tests.rs`'s own established technique
//! (`verify_repository_reports_active_wal_ordering_violation`): build a `WalRecord`, frame it with
//! `wal::encode_record_for_test` (bypasses `Wal::append_patch`'s own validation gates, needed
//! whenever the fixture's defect is exactly the kind of malformed shape `append_patch` would
//! itself refuse), and `std::fs::write` it directly to `Wal::path()`.

#![allow(clippy::indexing_slicing)]

use prikk_error::Result;
use prikk_object::{
    CanonicalEncode, DeleteNode, DeleteNodePreimage, NodeId, NodeKind, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, Signature,
    SignatureAlgorithm, SignerRole,
};

use super::{assert_stage_failed, assert_wal_item_failed};
use crate::test_gates::test_support::{
    rollback_author_signature, rollback_patch_blob_envelope, sample_object_id,
    signed_patch_blob_envelope, unique_temp_dir,
};
use crate::wal::{WalRecord, WalRecordStatus, encode_record_for_test};
use crate::{
    ActiveSessionRepairStatus, DEFAULT_ACTIVE_NAME, DoctorRepairOptions, FileObjectStore,
    ObjectWriter, RepositoryLayout, VerificationStage, Wal, repair_repository, verify_repository,
};

fn write_wal_records(wal: &Wal, records: &[WalRecord]) -> Result<()> {
    let mut bytes = Vec::new();
    for record in records {
        bytes.extend(encode_record_for_test(record)?);
    }
    std::fs::write(wal.path(), &bytes)?;
    Ok(())
}

fn rollback_payload_with_operations(operations: Vec<Operation>) -> PatchPayload {
    PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::RollbackDraft,
        message: None,
    }
}

fn create_file_operation(op_seq: u32, blob_id: ObjectId) -> Result<Operation> {
    Ok(Operation {
        op_seq,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(prikk_object::CreateFile {
            path: "a.txt".to_string(),
            node_id: NodeId::from_bytes([0x61; 32]),
            blob_id,
            mode: 0o100_644,
        }),
    })
}

/// DC-95 Stage 1, round 11: `Wal::replay()`'s checksum-mismatch hard `Err` (`wal.rs`'s
/// `decode_records`) -- listed by the prerequisite report as untested at any level, unit or
/// end-to-end. Construction: append one real, well-formed patch through `Wal::append_patch`, then
/// flip one byte inside the record's body (past the fixed-size header) via raw `fs::write` --
/// leaving the framing (magic, version, seq, body length) intact so `decode_records` reaches the
/// checksum comparison rather than failing an earlier structural check.
///
/// Probed: commenting out the checksum comparison (`expected != header_values.checksum`) lets
/// `verify_repository` return `Ok` with the tampered body silently accepted as the record's real
/// content -- no other check in this codebase re-derives or cross-checks WAL body integrity.
/// Load-bearing.
#[test]
fn verify_repository_detects_wal_checksum_mismatch() -> Result<()> {
    let root = unique_temp_dir("verify-wal-checksum");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = signed_patch_blob_envelope();
    objects.write_object(&blob)?;
    let payload =
        rollback_payload_with_operations(vec![create_file_operation(1, blob.object_id())?]);
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(rollback_author_signature())?;

    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&envelope)?;

    let mut bytes = std::fs::read(wal.path())?;
    let last_byte = bytes
        .last_mut()
        .ok_or_else(|| prikk_error::PrikkError::Io {
            kind: None,
            context: "WAL file unexpectedly empty".to_string(),
        })?;
    *last_byte ^= 0x01;
    std::fs::write(wal.path(), &bytes)?;

    let report = verify_repository(&layout)?;
    // RFC 102 Stage 2: isolate-and-continue reading means this is now an item finding, not a
    // WalReplay stage failure -- the stage itself evaluates cleanly around the one damaged record.
    assert_wal_item_failed(&report, "WAL checksum mismatch");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 11: `verify_wal_persistence`'s Patch-type hard `Err` (`verify.rs`) --
/// `Wal::append_patch` itself refuses to append a non-Patch envelope (`wal.rs`'s own type check),
/// so this defence is reachable only by raw WAL-byte construction, exactly like
/// `verify_repository_reports_active_wal_ordering_violation`'s own established technique.
///
/// Probed: forcing `verify_wal_persistence`'s type check to always pass lets `verify_repository`
/// return `Ok`, with the Blob envelope silently counted as an active-WAL patch record. Load-bearing.
#[test]
fn verify_repository_detects_non_patch_active_wal_record() -> Result<()> {
    let root = unique_temp_dir("verify-wal-type-mismatch");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    let record = WalRecord {
        seq: 1,
        envelope: signed_patch_blob_envelope(),
    };
    write_wal_records(&wal, &[record])?;

    let report = verify_repository(&layout)?;
    assert_stage_failed(&report, VerificationStage::WalPersistence, "expected patch");

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 11: `verify_rollback_patch_envelope`'s decode arm
/// (`decode_patch_operations`'s own `op_seq` contiguity check, `patch_replay/decode.rs`).
/// `PatchPayload::validate` -- called from `PatchPayload::encode_canonical` itself -- already
/// rejects non-contiguous `op_seq` at *encode* time (`is_contiguous_op_seq`), so no combination of
/// struct fields reaches `to_canonical_bytes()` with a decode-failing shape; this defect is
/// reachable only by raw payload-byte surgery, one level lower than every other fixture in this
/// file. Construction: encode a normal, valid single-operation payload (`op_seq: 1`, the only value
/// `validate` accepts for one operation), then flip the *value* bytes of that operation's `op_seq`
/// field in place -- located precisely, not by blind search, from `CanonicalWriter::field_raw`'s own
/// fixed wire format (`tag: u16 BE` + `wire_type: u8` + `len: u64 BE` + value) and the fact that
/// `op_seq` is `Operation::encode_canonical`'s first-written field: `00 01 03 00 00 00 00 00 00 00
/// 04` (tag 1, `WireType::U32`, length 4) immediately followed by the 4-byte value. The resulting
/// bytes still carry a `RollbackDraft` purpose tag `PatchPurpose::decode_from_patch_payload` reads
/// independently of the operations field, so `is_rollback_draft_envelope` still classifies it
/// correctly; only the operation's own `op_seq` value (now 2, at physical position 0) disagrees
/// with `decode_operation`'s "`op_seq == index + 1`" check.
///
/// Probed: relaxing `decode_operation`'s `op_seq` check to accept any value lets `verify_repository`
/// return `Ok`, with the misnumbered operation silently decoded and counted. Load-bearing.
#[test]
fn verify_repository_detects_rollback_draft_operation_sequence_mismatch() -> Result<()> {
    let root = unique_temp_dir("verify-rollback-decode-failure");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = rollback_patch_blob_envelope();
    objects.write_object(&blob)?;
    let payload =
        rollback_payload_with_operations(vec![create_file_operation(1, blob.object_id())?]);
    let mut payload_bytes = payload.to_canonical_bytes()?;
    let op_seq_field_header: [u8; 11] = [
        0x00, 0x01, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04,
    ];
    let matches: Vec<usize> = payload_bytes
        .windows(op_seq_field_header.len())
        .enumerate()
        .filter(|(_, window)| *window == op_seq_field_header)
        .map(|(index, _)| index)
        .collect();
    assert_eq!(
        matches.len(),
        1,
        "expected exactly one op_seq field header in the encoded payload, found {}",
        matches.len()
    );
    let match_offset = *matches.first().ok_or_else(|| prikk_error::PrikkError::Io {
        kind: None,
        context: "unreachable: length just checked".to_string(),
    })?;
    let value_start = match_offset + op_seq_field_header.len();
    let value_end = value_start
        .checked_add(4)
        .ok_or_else(|| prikk_error::PrikkError::Io {
            kind: None,
            context: "op_seq value range overflow".to_string(),
        })?;
    let value_bytes = payload_bytes
        .get_mut(value_start..value_end)
        .ok_or_else(|| prikk_error::PrikkError::Io {
            kind: None,
            context: "op_seq value out of range".to_string(),
        })?;
    assert_eq!(value_bytes, &1_u32.to_be_bytes());
    value_bytes.copy_from_slice(&2_u32.to_be_bytes());

    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload_bytes);
    envelope.add_signature(rollback_author_signature())?;

    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&envelope)?;

    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::RollbackDrafts,
        "does not match physical position",
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 11: `verify_rollback_patch_envelope`'s apply-support arm
/// (`ensure_apply_supported`, `patch_replay/decode.rs`). Construction: a `RollbackDraft`-purpose
/// Patch whose sole operation is `DeleteNode` with a `Symlink` preimage -- decodes structurally
/// (unlike the op_seq-mismatch fixture above, nothing here is malformed), but
/// `ensure_apply_supported` explicitly refuses `DeleteNode(symlink)` as an apply-time gap (DC-73).
///
/// Probed: relaxing `ensure_apply_supported` to accept every `DecodedOperationKind` lets
/// `verify_repository` return `Ok`, with the unsupported operation silently accepted into an
/// active rollback draft. Load-bearing.
#[test]
fn verify_repository_detects_rollback_draft_unsupported_operation() -> Result<()> {
    let root = unique_temp_dir("verify-rollback-apply-unsupported");
    let layout = RepositoryLayout::init(root.clone())?;
    let payload = rollback_payload_with_operations(vec![Operation {
        op_seq: 1,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::DeleteNode(DeleteNode {
            path: "link.txt".to_string(),
            node_id: NodeId::from_bytes([0x62; 32]),
            old_node_kind: NodeKind::Symlink,
            preimage: DeleteNodePreimage::Symlink {
                old_target: "target.txt".to_string(),
            },
        }),
    }]);
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(rollback_author_signature())?;

    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&envelope)?;

    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::RollbackDrafts,
        "DeleteNode(symlink)",
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 11: `require_rollback_author_signature`'s missing-AUTHOR-signature arm
/// (`rollback_verify.rs`). Unit-tested directly against `verify_rollback_patch_envelope`
/// (`rollback_verify/tests.rs`'s `rollback_purpose_without_author_signature_is_rejected`), not
/// end to end. Construction: a well-formed `RollbackDraft` Patch signed only by a `Maintainer`-role
/// signature -- non-empty, so `Wal::append_patch`'s own "commit WAL entries must be signed" gate is
/// satisfied, but no signature carries `SignerRole::Author`.
///
/// Probed: relaxing the "find an Author-role signature" step to accept any role lets
/// `verify_repository` return `Ok`, with the Maintainer signature silently accepted as rollback
/// authorship. Load-bearing (end to end; already unit-tested at the function level).
#[test]
fn verify_repository_detects_rollback_draft_missing_author_signature() -> Result<()> {
    let root = unique_temp_dir("verify-rollback-missing-author");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = rollback_patch_blob_envelope();
    objects.write_object(&blob)?;
    let payload =
        rollback_payload_with_operations(vec![create_file_operation(1, blob.object_id())?]);
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: "not-the-author".to_string(),
        signature_bytes: vec![3; 64],
        created_at: 1,
        signer_role: SignerRole::Maintainer,
    })?;

    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&envelope)?;

    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::RollbackDrafts,
        "must carry an AUTHOR signature",
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// DC-95 Stage 1, round 11: `require_rollback_author_signature`'s legacy-marker-key-id arm
/// (`rollback_verify.rs`). Unit-tested directly (`rollback_purpose_with_legacy_marker_signature_is_
/// rejected`), not end to end. Construction: `test_support::rollback_author_signature`'s well-formed
/// shape, but with the `key_id` swapped to `LEGACY_ROLLBACK_MARKER_KEY_ID` -- passes every shape
/// check `Wal::append_patch` itself performs.
///
/// Probed: removing the legacy-marker key-id comparison lets `verify_repository` return `Ok`, with
/// the placeholder key id silently accepted as a real author identity. Load-bearing (end to end;
/// already unit-tested at the function level).
#[test]
fn verify_repository_detects_rollback_draft_legacy_marker_key_id() -> Result<()> {
    let root = unique_temp_dir("verify-rollback-legacy-marker");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = rollback_patch_blob_envelope();
    objects.write_object(&blob)?;
    let payload =
        rollback_payload_with_operations(vec![create_file_operation(1, blob.object_id())?]);
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    envelope.add_signature(crate::test_gates::test_support::legacy_rollback_marker_signature())?;

    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&envelope)?;

    let report = verify_repository(&layout)?;
    assert_stage_failed(
        &report,
        VerificationStage::RollbackDrafts,
        "legacy rollback marker key id",
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 102 Stage 2 acceptance criteria (`stage-2-implementation-handoff-v1.md`): a `Normal`-purpose
/// Patch envelope, distinguished from the `RollbackDraft` payloads above only by `purpose` and the
/// referenced blob id -- `is_rollback_draft_envelope` (`rollback_draft.rs`) returns `Ok(false)` for
/// `Normal`, so the `RollbackDrafts` stage skips these entirely and this fixture exercises
/// `WalReplay`/`WalPersistence`/`WalRecordSchema` alone, without also depending on rollback-specific
/// decode/apply-support validation staying satisfied.
fn normal_patch_envelope(label: &str) -> Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![create_file_operation(1, sample_object_id(label))?],
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

/// Append `labels.len()` distinct, well-formed patch records to `wal` via the real
/// `Wal::append_patch` path (not raw byte construction) -- guarantees the fixture is genuinely
/// valid by construction, the same way `verify_repository_detects_wal_checksum_mismatch` above
/// earns its one real record. Returns each record's own starting byte offset in file order, found by
/// measuring the file's length immediately before each append (append is the only writer here, so
/// this is exact, not an estimate).
fn append_distinct_records(wal: &Wal, labels: &[&str]) -> Result<Vec<usize>> {
    let mut offsets = Vec::with_capacity(labels.len());
    for label in labels {
        let before = std::fs::read(wal.path())
            .map(|bytes| bytes.len())
            .unwrap_or(0);
        offsets.push(before);
        wal.append_patch(&normal_patch_envelope(label)?)?;
    }
    Ok(offsets)
}

/// Flip the last byte of each `bytes[bounds[i]]` record's own span, for every `i` in `damaged` --
/// stays inside that record's body (well past the fixed 58-byte header), so magic/version/seq/
/// body_len all still parse and `parse_frame_at` reaches the checksum comparison, exactly like
/// `verify_repository_detects_wal_checksum_mismatch`'s single-record fixture above. `bounds` gives
/// each record's own `(start, stop)` span, `stop` exclusive.
fn corrupt_records_last_byte(bytes: &mut [u8], bounds: &[(usize, usize)], damaged: &[usize]) {
    for &index in damaged {
        let (_, stop) = bounds[index];
        bytes[stop - 1] ^= 0x01;
    }
}

fn record_bounds(offsets: &[usize], total_len: usize) -> Vec<(usize, usize)> {
    offsets
        .iter()
        .enumerate()
        .map(|(index, &start)| {
            let stop = offsets.get(index + 1).copied().unwrap_or(total_len);
            (start, stop)
        })
        .collect()
}

/// RFC 102 Stage 2 acceptance criterion 1 ("two independently damaged records in one WAL are both
/// reported, with their offsets") and criterion 2 ("every sound record after a damaged one is still
/// read"), asserted directly against `Wal::replay()` -- the most precise level to check exact
/// offsets and which records survive, one level below `verify_repository`'s own reporting (covered
/// separately by `verify_repository_reports_two_independently_damaged_wal_records_with_offsets`
/// below, per the acceptance criteria's own end-to-end wording).
///
/// Five records; index 1 ("two") and index 3 ("four") are damaged, leaving index 0/2/4 sound --
/// deliberately not a single damaged-then-sound pair, so a bug that only handles resync-then-stop
/// (rather than resync-then-keep-going) cannot pass by accident.
#[test]
fn wal_replay_isolates_two_damaged_records_and_reads_every_sound_record() -> Result<()> {
    let root = unique_temp_dir("wal-replay-two-damaged");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);

    let offsets = append_distinct_records(&wal, &["one", "two", "three", "four", "five"])?;
    let mut bytes = std::fs::read(wal.path())?;
    let bounds = record_bounds(&offsets, bytes.len());
    corrupt_records_last_byte(&mut bytes, &bounds, &[1, 3]);
    std::fs::write(wal.path(), &bytes)?;

    let replay = wal.replay()?;
    assert!(replay.has_item_failure());
    assert_eq!(
        replay.trailing_partial_bytes, 0,
        "the last record is sound and complete; nothing should be left over"
    );

    let failed_offsets: Vec<usize> = replay
        .record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            WalRecordStatus::Failed { .. } => Some(outcome.offset),
            WalRecordStatus::Evaluated => None,
        })
        .collect();
    assert_eq!(
        failed_offsets,
        vec![bounds[1].0, bounds[3].0],
        "expected exactly the two damaged records' own starting offsets, got {failed_offsets:?} against bounds {bounds:?}"
    );

    let sound_seqs: Vec<u64> = replay.records.iter().map(|record| record.seq).collect();
    assert_eq!(
        sound_seqs,
        vec![1, 3, 5],
        "every sound record -- including the ones immediately after each damaged record -- must \
         still be read, in file order"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 102 Stage 2 acceptance criteria 1, 2, 3, and 5, asserted end to end through
/// `verify_repository`/`repair_repository` -- the acceptance criteria's own literal level, layered
/// on top of the unit-level proof in `wal_replay_isolates_two_damaged_records_and_reads_every_sound_
/// record` above. Also confirms containment explicitly: `WalReplay` and its dependent stages stay
/// `Evaluated` (criterion true even though the repository's overall verification still fails --
/// criterion 3, "a repository that failed verification before still fails it" -- just via
/// `has_item_failure()` now, not a stage abort).
#[test]
fn verify_repository_reports_two_independently_damaged_wal_records_with_offsets() -> Result<()> {
    let root = unique_temp_dir("verify-wal-two-damaged");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);

    let offsets = append_distinct_records(&wal, &["alpha", "beta", "gamma"])?;
    let mut bytes = std::fs::read(wal.path())?;
    let bounds = record_bounds(&offsets, bytes.len());
    corrupt_records_last_byte(&mut bytes, &bounds, &[0, 2]);
    std::fs::write(wal.path(), &bytes)?;

    let report = verify_repository(&layout)?;
    assert!(
        report.has_item_failure(),
        "criterion 3: a repository with a damaged WAL record must still fail verification"
    );
    assert!(
        !report.has_stage_failure(),
        "item containment means WalReplay and its dependent stages stay Evaluated around the two \
         damaged records: {report:?}"
    );

    let mut failed_offsets: Vec<usize> = report
        .wal_record_outcomes
        .iter()
        .filter_map(|outcome| match &outcome.status {
            WalRecordStatus::Failed { .. } => Some(outcome.offset),
            WalRecordStatus::Evaluated => None,
        })
        .collect();
    failed_offsets.sort_unstable();
    let mut expected_offsets = vec![bounds[0].0, bounds[2].0];
    expected_offsets.sort_unstable();
    assert_eq!(
        failed_offsets, expected_offsets,
        "criterion 1: both damaged records must be reported, each with its own offset"
    );

    // RFC 108 increment 3d: criterion 5 is now "refuses to repair *this* active session" rather
    // than "the whole call errors" -- a damaged record in `default`'s own WAL is exactly as
    // active-scoped as every other WAL-record finding this arc has attributed, and with only
    // `default` on disk there is no other active session for the call to succeed on regardless.
    // `repair_repository` itself still succeeds (nothing repository-wide is wrong), but `default`'s
    // own entry in `active_repairs` must show it was skipped, not silently repaired over.
    let repair = repair_repository(&layout, DoctorRepairOptions::truncate_wal_tail())?;
    assert!(
        repair.active_repairs.iter().any(|outcome| {
            outcome.active_session.to_str() == Some(DEFAULT_ACTIVE_NAME)
                && matches!(outcome.status, ActiveSessionRepairStatus::Skipped { .. })
        }),
        "criterion 5: repair_repository must still refuse to repair a WAL carrying a damaged \
         record, now expressed as `default` being skipped rather than the whole call erroring: \
         {repair:?}"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 102 Stage 2 acceptance criterion 4: a trailing partial frame is still tolerated unchanged --
/// resync/item-containment must never engage for a genuinely incomplete final record. `parse_frame_
/// at`'s own `remaining < WAL_HEADER_LEN` branch (`wal.rs`) makes this unconditional on content: any
/// tail shorter than one full 58-byte header is `TrailingPartial`, regardless of what bytes it holds,
/// so a plain zero-filled short tail already exercises the real code path -- no need to construct a
/// realistically-interrupted frame to prove this.
#[test]
fn wal_replay_still_tolerates_a_trailing_partial_frame_unchanged() -> Result<()> {
    let root = unique_temp_dir("wal-replay-trailing-partial");
    let layout = RepositoryLayout::init(root.clone())?;
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);

    wal.append_patch(&normal_patch_envelope("one")?)?;
    wal.append_patch(&normal_patch_envelope("two")?)?;

    let mut bytes = std::fs::read(wal.path())?;
    let torn_tail = vec![0_u8; 30];
    bytes.extend_from_slice(&torn_tail);
    std::fs::write(wal.path(), &bytes)?;

    let replay = wal.replay()?;
    assert!(
        !replay.has_item_failure(),
        "a torn tail is not corruption and must not surface as an item failure: {replay:?}"
    );
    assert_eq!(replay.records.len(), 2);
    assert_eq!(replay.trailing_partial_bytes, torn_tail.len());
    assert!(
        replay
            .record_outcomes
            .iter()
            .all(|outcome| matches!(outcome.status, WalRecordStatus::Evaluated)),
        "a torn tail must not produce a Failed record_outcomes entry: {:?}",
        replay.record_outcomes
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
