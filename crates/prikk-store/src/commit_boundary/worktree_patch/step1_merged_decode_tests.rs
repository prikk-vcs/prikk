//! 0.54.0 step 1, item 1: `author_inner` now decodes the active WAL once, sharing it between the
//! publication-cleanup check and everything `active_replay` feeds, instead of two separate full
//! decodes. This module exercises every shape the handoff's own case table names, against the
//! real `commit_worktree_changes_signed` entry point, and asserts today's outcome (text, not just
//! ok/err) -- the shapes are then re-run under a hand perturbation (restoring the old, separate
//! decode) to confirm the merge changed nothing about what any of them says.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose,
};

use crate::author::author_signing::Ed25519AuthorSigner;
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectWriter, RepositoryLayout, Wal,
    WorktreePatchCommitOptions, add_trusted_maintainer, author_signature,
    commit_worktree_changes_signed,
};

fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("step1-case-table-maintainer", &[0x71; 32]).unwrap()
}

fn author() -> Ed25519AuthorSigner {
    Ed25519AuthorSigner::from_seed("step1-case-table-author", &[0x72; 32]).unwrap()
}

fn setup(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let signer = maintainer();
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &prikk_hash::to_hex(&signer.public_key_bytes()),
    )
    .unwrap();
    (layout, root)
}

/// Append one signed patch directly to the active WAL, bypassing `commit` entirely -- the same
/// technique `refs/tests/no_refs_times_log_precondition.rs::queue_one_patch` already uses, for
/// the identical reason: building a WAL state `commit` itself cannot produce (a settled Block
/// whose queue has not drained, or a WAL with an interior damaged record ahead of a sound one).
/// Returns the record's own starting byte offset in the WAL file, found by measuring the
/// file's length immediately before this append (append is the only writer in these fixtures,
/// so this is exact, not an estimate) -- the same technique
/// `verify/tests/wal_cluster.rs::append_distinct_records` uses, for the identical reason: an
/// exact record span to flip a byte inside, rather than a guessed offset.
fn append_raw_patch(layout: &RepositoryLayout, ref_name: &str, label: &str) -> usize {
    let offset = std::fs::read(queue_wal_path(layout))
        .map(|bytes| bytes.len())
        .unwrap_or(0);
    let blob = BlobPayload::new(BlobKind::Text, format!("{label}\n").into_bytes());
    let blob_envelope =
        ObjectEnvelope::unsigned(ObjectType::Blob, 1, blob.to_canonical_bytes().unwrap());
    let blob_id = blob_envelope.object_id();
    FileObjectStore::new(layout.clone())
        .write_object(&blob_envelope)
        .unwrap();
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: format!("{label}.txt"),
                node_id: NodeId::from_bytes(*blob_id.as_bytes()),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    envelope
        .add_signature(author_signature(&author(), envelope.object_id()).unwrap())
        .unwrap();
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    crate::write_active_ref_metadata(layout, ref_name).unwrap();
    offset
}

fn commit_one(layout: &RepositoryLayout, path: &str, body: &[u8]) -> Result<(), String> {
    std::fs::write(layout.root().join(path), body).unwrap();
    commit_worktree_changes_signed(
        layout,
        "heads/main",
        "step1 case table",
        WorktreePatchCommitOptions::default(),
        &author(),
    )
    .map(|_| ())
    .map_err(|err| err.to_string())
}

fn queue_wal_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.root().join(".prikk/active/default/queue.wal")
}

/// Flip the last byte of the record starting at `offset` -- stays inside that record's own
/// body (well past its fixed-width header), so magic/version/seq/body_len all still parse and
/// `parse_frame_at` reaches the checksum comparison, exactly like
/// `verify/tests/wal_cluster.rs::corrupt_records_last_byte`. `next_offset` is the following
/// record's own starting offset, if any (`None` when `offset`'s record is the file's last).
fn damage_record_at(layout: &RepositoryLayout, _offset: usize, next_offset: Option<usize>) {
    let path = queue_wal_path(layout);
    let mut bytes = std::fs::read(&path).unwrap();
    let stop = next_offset.unwrap_or(bytes.len());
    bytes[stop - 1] ^= 0x01;
    std::fs::write(&path, &bytes).unwrap();
}

fn append_torn_tail(layout: &RepositoryLayout) {
    let path = queue_wal_path(layout);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(&mut file, &[0xAB; 11]).unwrap();
}

/// For each of the handoff's own five shapes: build it, attempt one more commit, and return
/// the outcome as `Ok(())` or `Err(<display text>)`. Run once under today's merged decode, and
/// once (by the caller, via a hand perturbation of `author_inner` itself) under the old,
/// separate-decode shape -- the two runs must agree byte for byte.
fn five_shapes() -> Vec<(&'static str, Result<(), String>)> {
    let mut out = Vec::new();

    {
        let (layout, root) = setup("step1-case-table-damaged-interior");
        append_raw_patch(&layout, "heads/main", "a");
        let offset_b = append_raw_patch(&layout, "heads/main", "b");
        let offset_c = append_raw_patch(&layout, "heads/main", "c");
        damage_record_at(&layout, offset_b, Some(offset_c));
        out.push((
            "damaged interior record",
            commit_one(&layout, "new.txt", b"new\n"),
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
    {
        let (layout, root) = setup("step1-case-table-torn-tail");
        append_raw_patch(&layout, "heads/main", "a");
        append_torn_tail(&layout);
        out.push(("torn tail", commit_one(&layout, "new.txt", b"new\n")));
        let _ = std::fs::remove_dir_all(&root);
    }
    {
        let (layout, root) = setup("step1-case-table-pending-cleanup");
        append_raw_patch(&layout, "heads/main", "p");
        crate::simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer()).unwrap();
        // Re-queue the exact same content directly, bypassing the drain
        // `finish_active_publication_cleanup` would otherwise run -- `has_incomplete_active_
        // cleanup`'s own fixture shape, reused from `no_refs_times_log_precondition.rs`.
        append_raw_patch(&layout, "heads/main", "p");
        out.push((
            "pending cleanup alone",
            commit_one(&layout, "new.txt", b"new\n"),
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
    {
        let (layout, root) = setup("step1-case-table-pending-cleanup-and-damage");
        append_raw_patch(&layout, "heads/main", "p");
        crate::simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer()).unwrap();
        // The seal's own cleanup truncated the WAL to empty (`finish_active_publication_
        // cleanup`). Re-queue "p" (the pending-cleanup shape) *and* a second, distinct record
        // ("q") -- RFC 162 rule 3 (wal.rs:770): a damaged frame with nothing sound after it is
        // tail, not damage, so without "q" sound behind it this fixture would be a torn tail,
        // not the genuine damaged-record shape this row needs.
        let offset_p2 = append_raw_patch(&layout, "heads/main", "p");
        let offset_q = append_raw_patch(&layout, "heads/main", "q");
        damage_record_at(&layout, offset_p2, Some(offset_q));
        out.push((
            "pending cleanup and a damaged record",
            commit_one(&layout, "new.txt", b"new\n"),
        ));
        let _ = std::fs::remove_dir_all(&root);
    }
    {
        let (layout, root) = setup("step1-case-table-clean");
        out.push((
            "clean queue (control)",
            commit_one(&layout, "new.txt", b"new\n"),
        ));
        let _ = std::fs::remove_dir_all(&root);
    }

    out
}

/// The exact text each shape must produce, against both the merged decode and (by hand
/// perturbation of `author_inner`, run separately) the restored two-call shape -- these two
/// runs must print byte-for-byte identical lines, confirming the merge changed no refusal's
/// text or order (0.54.0 step 1 item 1's own case table).
#[test]
fn the_case_table_today() {
    for (name, outcome) in five_shapes() {
        match name {
            "damaged interior record" => {
                assert_eq!(
                    outcome,
                    Err("integrity error: active WAL has a damaged record (damaged record at byte offset 343; 1 sound record(s) follow it); run doctor for diagnosis before mutating this repository (a damaged record with sound ones behind it is not a torn tail, and no automatic repair applies to it)".to_string())
                );
            }
            "torn tail" => {
                assert_eq!(
                    outcome,
                    Err("integrity error: active WAL has 11 trailing partial bytes; run `prikk doctor --repair-wal-tail` before committing".to_string())
                );
            }
            "pending cleanup alone" => {
                assert_eq!(
                    outcome,
                    Err("precondition not met: repository mutation is blocked by incomplete ref publication; run verify/doctor and use signer-backed seal retry".to_string())
                );
            }
            "pending cleanup and a damaged record" => {
                assert_eq!(
                    outcome,
                    Err("integrity error: active WAL has a damaged record (damaged record at byte offset 0; 1 sound record(s) follow it); run doctor for diagnosis before mutating this repository (a damaged record with sound ones behind it is not a torn tail, and no automatic repair applies to it)".to_string())
                );
            }
            "clean queue (control)" => {
                assert!(outcome.is_ok(), "{name}: expected success, got {outcome:?}");
            }
            other => panic!("unknown shape {other}"),
        }
        eprintln!("STEP1 CASE TABLE | {name} | {outcome:?}");
    }
}

// 0.54.0 step 1 round 2, R2: the kill sweep that used to live here
// (`a_commit_fails_safely_at_every_fired_write_ordinal`) asserted only `crashed.is_err()` -- it
// never retried, never cleared a held lock, never checked `verify` afterward, and never compared
// the queue's own shape against an uncrashed reference run. `witness::tests::m6_commit_kill_
// sweep_recovers_at_every_write_ordinal` already exercises this identical, now-merged
// `author_inner` path with all of that, and is the established, stronger standard (review
// `0.54.0-step-1-review-v1`, R2: "one sweep, the stronger one, with no second test claiming
// less"). Removed outright rather than built up to that standard, per the review's own stated
// preference.
