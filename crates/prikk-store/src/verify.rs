//! Repository verification routines.
//!
//! Verification is read-only. It checks object identity, object-type placement, envelope decoding,
//! sealed block references, joint ref publication state, active WAL replay checksums, and retained
//! active-publication cleanup state. Mutation belongs to narrow doctor or signer-backed seal paths.
//!
//! **A check's own code being present does not establish that a defect actually reaches it.** Earlier
//! gates in this module's pipeline can intercept a malformed input before a specific, later check ever
//! sees it -- so a check existing, and even a fixture that constructs the shape that check is meant to
//! reject, are not proof the check is exercised. Two independent instances (DC-95 Stage 1 rounds 10 and
//! 11): `ref_publication::require_retained_evidence` reclassifies several `refs/verify.rs` codes before
//! they're returned, so a raw pointer/log-shape fixture can silently land on a different code than the
//! one under test; `crate::format::validate_read_schema`, called from `Wal::replay()` itself, already
//! rejects a malformed-shape signature under `RepositoryFormat::CurrentV6` before
//! `rollback_verify::verify_rollback_draft_wal_records` is ever reached, so the same defect is reachable
//! only under `RepositoryFormat::LegacyV1`. Building a fixture for a specific check in this module means
//! tracing its actual call path from `verify_repository`, not just constructing input shaped to match
//! the check's own condition.
//!
//! **Do not report a result derived from a step that may not have run.** The general form of a rule
//! found three times, from three directions, across DC-95 Stage 1 and Stage 2 (`stage-2-level-1-sweep-
//! ruling-v1.md` §4): a check's presence in the source is not proof a defect reaches it (above); an
//! empty accumulator is not "none found" if its producer did not run to completion; a partial count is
//! not "this many verified" if the stage computing it stopped partway through. All three are the same
//! error seen from different angles -- inferring a result from the absence of evidence when the
//! evidence-gathering step itself may not have happened. `require_retained_evidence`'s own `trust_is_
//! valid` (see [`VerificationStage::PublicationReclassification`] below) is the concrete instance the
//! second form was caught in: `trust_verifier.issues.is_empty()` alone reads `true` when the `Objects`
//! stage failed before checking a single Block or RefState, which is not the same fact as "every
//! relevant object was checked and found trustworthy." It is `issues.is_empty() && objects_evaluated`
//! for exactly this reason. The third form is why `RepositoryVerification`'s per-stage counts
//! (`checked_objects` and its siblings) are `Option<usize>`, `None` rather than a partial number, when
//! their producing stage did not evaluate to completion -- a partial count is a completeness claim in
//! miniature, and `verify_objects`'s own topological pass over the whole object store means a partial
//! per-type count says nothing about whether the objects it did see are individually sound.
//!
//! # DC-95 Stage 2, Level 1: scope containment
//!
//! `verify_repository`'s pipeline is organized into the stages named by [`VerificationStage::ALL`],
//! each a `?`-propagating call in the pre-Stage-2 source. Stage 1 (above) proved which checks
//! inside those stages are load-
//! bearing; Stage 2 Level 1 changed what happens when one fails. **Before:** the first hard error
//! anywhere aborted `verify_repository` entirely -- every later stage silently never ran, and
//! `print_verify_report` never printed anything but the one error string. **After:** each stage's own
//! `?` is caught at the boundary and recorded as a [`StageOutcome`] rather than propagated; the
//! pipeline continues to the remaining stages regardless. A `Failed` stage's own error becomes its
//! `StageOutcome`'s message; a stage that could not run because a real dependency ([`StageStatus::
//! NotEvaluated`], naming that dependency) is blocking on the same footing -- an incomplete
//! verification is not a passing one, so `RepositoryVerification::has_stage_failure` covers both, plus
//! a third state, [`StageStatus::Halted`], for `--stop-on-first-error`: a stage that was never attempted
//! because an *unrelated* earlier stage's failure already stopped the walk. `Halted` is kept distinct
//! from `NotEvaluated` because `blocked_by` is a dependency-graph claim -- reporting `NotEvaluated {
//! blocked_by: Objects }` for `LifecycleCache`, which does not depend on `Objects` at all, would assert
//! an edge that does not exist (implementation review v1 §4). The distinction only matters under
//! `--stop-on-first-error`; in the default full-accumulation walk, every `NotEvaluated` names a real
//! dependency and `Halted` never appears.
//!
//! **The Stage 1 classification table below is unchanged by this**: which checks are load-bearing,
//! downstream-redundant, excluded, or unreachable is a fact about the checks themselves, not about how
//! their failures propagate out of `verify_repository`. What changed is the shape a reader (or
//! `doctor_repository`, or `prikk verify`'s own exit-code chain) sees a failing check through: a
//! `StageOutcome` against the owning stage, not a bare `Result::Err` from the whole function. Checks
//! were not rewritten, moved, or deleted to get here -- only the top-level boundaries around them
//! ([`VerificationStage::ALL`]).
//!
//! # RFC 103: format-1 retirement, and its effect on the table below
//!
//! `RepositoryFormat::LegacyV1` is deleted; a format-1 repository is now rejected at
//! `RepositoryLayout::open`, before `verify_repository` (or anything else) can run against it. Three
//! rows in the table below changed status as a direct consequence, all still classified, none left to
//! drift: `LEGACY-LOG-LEADS` and `LEGACY-TIMESTAMP` (both format-1-gated branches that can now never
//! execute, since the format they branched on no longer exists) are deleted outright rather than kept
//! as unreachable, since deleting the dead branch was possible and cheaper than documenting it;
//! rollback AUTHOR signature wrong-length moves from load-bearing to unreachable, since round 11 had
//! already established format-1 as its only reachable path (`rollback_verify.rs` carries the argument
//! at the check site, per round 6's ruling on unreachable checks). `legacy_state_roots_unverifiable`
//! (a precondition fact, not a table row) is deleted with it. The three-row figure matches RFC 103's
//! own accounting, corrected once during that RFC's own prerequisite investigation:
//! `LEGACY-TIMESTAMP`'s status change was found only by re-reading this table, not carried in the
//! RFC's original draft.
//!
//! # RFC 102 Stage 2: isolate-and-continue reading, and its effect on the table below
//!
//! A mid-stream checksum mismatch in the active WAL or a ref log no longer aborts the whole read: the
//! frame at that offset is recorded as a failed item (`wal::WalRecordOutcome` /
//! `refs::log::RefLogRecordOutcome`) and a byte-wise resync finds the next candidate frame, so every
//! sound record after the damaged one is still read (RFC 102 §3, `wal.rs`'s `decode_records` and
//! `refs/log.rs`'s `decode_log_records`). **The two rows this changes stay Load-bearing** -- disabling
//! either check still lets a defective repository pass with no trace, exactly the property this table
//! tracks -- but the failure they produce is now item-level (`RepositoryVerification::has_item_failure`,
//! via the new `wal_record_outcomes` field), not a `WalReplay` stage failure, the same shift DC-95 Stage
//! 2 Level 2 already made for `verify_objects` and `verify_refs`. `Wal::replay()`'s own post-decode
//! `validate_read_schema` call is deliberately untouched -- a schema-shape defect is a different concern
//! from physical corruption and stays a hard stage failure, out of this stage's scope (`stage-2-
//! implementation-handoff-v1.md` §2's own boundary). Ref-log record corruption is exposed at file
//! granularity (`refs::RefFileStatus::Failed` in `log_outcomes`, unchanged shape), not per-record like
//! the WAL's `wal_record_outcomes` -- a deliberate, narrower scope decision for this round, recorded in
//! the implementation's own review submission. **Every mutation-authorizing caller of `Wal::replay`/
//! `RefStore::replay_log` that read `records`/`trailing_partial_bytes` without also checking the new
//! `has_item_failure()` was audited and fixed in the same increment** -- the same discipline DC-95 Stage
//! 2's ref-item-containment round established for `ensure_no_incomplete_publication`, applied here to a
//! substantially larger caller set (`wal.rs`, `active.rs`, `rollback_draft.rs`,
//! `worktree_patch/node_authoring.rs`, `seal.rs`, `branch.rs`, `refs/evidence.rs`, `rollback_verify.rs`,
//! `refs/publication.rs`, `refs.rs`, `patch_replay.rs`, `seal/support.rs`,
//! `verify/ref_publication.rs`).
//!
//! # RFC 102 Stage 3: container/index storage, and its effect on the table below
//!
//! Persisted-object storage moved from one loose file per object to per-type append-only containers
//! plus one append-only index (design-v1.md §2/§4/§5); `verify_objects` (`verify/objects.rs`) now
//! scans containers instead of listing directories. **Every check inside `verify_object_record` itself
//! is untouched** -- schema validation, signature-envelope classification, publication trust, and
//! `verify_block_payload` all still run on the same decoded `ObjectEnvelope`, just fed from a container
//! record instead of a loose-file read. Rows whose check reads an object only through
//! `FileObjectStore`'s public API (`ObjectReader`/`ObjectWriter`) are unaffected: that API's behavior
//! toward callers is unchanged (task 129 of this stage confirmed this directly, not assumed from the
//! type signature alone) -- only its internal storage moved. Three rows are not covered by that blanket
//! statement, because their own mechanism is specifically about object *storage structure*, not about
//! what a decoded envelope contains:
//!
//! - **Envelope type mismatch** (half of the row above) moved, and in moving exposed a real gap: a
//!   container's magic constrains which container a frame's bytes live in, but nothing already
//!   enforced that the frame's own decoded `envelope.object_type` agreed with it -- a well-formed,
//!   correctly checksummed Blob-container frame could hold a validly encoded Patch envelope
//!   undetected. Found and fixed during this stage's own test migration (not assumed correct from the
//!   design alone), at the one place every container reader passes through:
//!   `container::parse_frame_at`. **Stays Load-bearing** -- proven at the container level
//!   (`container::tests::envelope_type_disagreeing_with_its_own_containers_type_is_rejected`) and end
//!   to end (`verify_repository_rejects_envelope_type_mismatch`, `assert_object_item_failed(&report,
//!   "is under type")`).
//! - **Object id mismatch** (the other half) is now enforced in two places instead of one: ordinary
//!   reads (`FileObjectStore::read_object`, unchanged in shape -- still a lazy, per-id check) and,
//!   newly, `verify_objects`'s own proactive full-scan cross-validation of every index entry against
//!   what its own claimed location actually decodes to (design §12/§10.2's "the bytes found are
//!   validated by recomputing the content hash... `verify` does the full scan" ruling, read as
//!   including index-to-container consistency, not just container enumeration in isolation). A decode
//!   *failure* at an entry's location is deliberately not escalated by this new check -- it is already
//!   an item-level `Failed` outcome from the per-record container scan (Stage 2's own containment) --
//!   only "decodes fine but to the wrong id" is, and that is treated as a genuine index-integrity
//!   defect, propagated as a stage-level `Err`, not an isolable item defect (an index that lies about
//!   content is corruption at a different scale than one damaged record). **This is a new row, not
//!   merely this stage's proof of the old one: Load-bearing**, proven end to end by
//!   `verify_repository_detects_index_entry_resolving_to_a_different_object`.
//! - **Directory/file shape structural errors** keeps its exact code, unmoved and unchanged
//!   (`scan_loose_file_temp_debris`, design-v1.md §12.3 item 3's ruling: the loose-file tree's
//!   temp-debris scan is kept, dormant, since retiring diagnostic surface is an RFC-level act, not a
//!   stage side effect) -- but that tree is no longer written by anything under format-3, so the row's
//!   own downstream-redundant partners (`list_directory`'s own directory-vs-file rejection;
//!   `object_id_from_path`'s own extension check) are reachable today only by a fixture that plants the
//!   stray entry directly, never as a byproduct of an ordinary write. **Classification unchanged**
//!   (still Downstream-redundant, both sub-arms -- the redundant partners are generic filesystem-safety
//!   checks, not specific to which tree is live), re-confirmed passing unmodified:
//!   `verify_repository_detects_every_directory_shape_violation` already used `FileObjectStore::
//!   write_object` only to produce a real object elsewhere in the fixture, and planted its two stray
//!   entries directly at the loose-file paths either way, so this row needed no test change at all.
//!
//! # DC-95 Stage 1: end-to-end coverage, by cluster
//!
//! Every check `verify_repository` performs, classified by whether disabling it lets a defective
//! repository pass through `verify_repository` as `Ok` with no trace -- **Load-bearing** (the check is
//! the last line of defence; some are load-bearing only via a non-blocking-sibling mechanism, named
//! where that applies); **Downstream-redundant** (something else independently catches the same defect,
//! blocking, under a different code); **Excluded** (non-blocking finding, out of mandatory scope);
//! **Unreachable** (provably impossible to construct -- kept, untested, ruled on explicitly, not merely
//! unattempted). Full reasoning for each row lives in the test file cited, not duplicated here; this
//! table is the current-state index, not the round-by-round record (that's
//! `DC-95-VERIFY-COVERAGE-AND-FINDING-ACCUMULATION.md`'s own handoff trail).
//!
//! **Scope limit**: this enumerates checks `verify` *has*, not checks it *should have* -- a gap of a
//! different kind this method cannot surface. One known instance: the received-ref index
//! (`received_index.rs`, RFC 102 Stage 5 -- formerly `refs/received/`) is never read by
//! `verify_repository` at all (RFC 101 §5.2's independently-derived transition trace); registered in
//! reported in the review result, not a row here, since there is no existing check to classify.
//!
//! ## `verify/objects.rs` + `block_state.rs` (`verify/tests.rs`)
//!
//! | Check | Classification |
//! |---|---|
//! | Block parent-block existence | Downstream-redundant (`validate_v2_lineage`) |
//! | Block patch existence | Downstream-redundant (lifecycle-replay layer's own read) |
//! | Block snapshot-blob existence | Load-bearing |
//! | Block format-2 shape validation (8 arms) | Load-bearing, all 8 |
//! | Topological cycle detection | Unreachable (needs a SHA-256 fixed point; unit-level substitute in `block_state/tests.rs`) |
//! | Envelope type mismatch / object id mismatch | Load-bearing, both -- RFC 102 Stage 3 moved the type-mismatch half into `container::parse_frame_at`; see the section above |
//! | Index entry resolves to the wrong object (RFC 102 Stage 3) | Load-bearing -- new this stage; see the section above |
//! | `validate_read_schema` strict-signature-shape | Load-bearing, via non-blocking-sibling mechanism |
//! | Publication-trust failure (Block/RefState) | Demonstrated via trusted/untrusted contrast |
//! | Directory/file shape structural errors | Downstream-redundant, both sub-arms -- RFC 102 Stage 3 made the tree it scans dormant; see the section above |
//!
//! ## `refs/verify.rs` + `refs/verify/scan.rs` (`verify/tests/ref_cluster.rs`)
//!
//! **RFC 102 Stage 4 (design-v1.md §13.12-§13.15) moved ref publication state from one loose file
//! per pointer/log to two shared containers** (a ref-pointer index, a ref-log container), the shape
//! Stage 3 established for objects. Two of this cluster's sixteen original checks had no per-file
//! path left to be non-canonical or wrong-shaped, and are genuinely retired -- not merely renamed --
//! confirmed by grepping the rewritten `scan.rs`/`container.rs` for their own diagnostic strings and
//! finding neither. A third pair (duplicate identity) describes a scenario the new "last entry wins"
//! model makes structurally impossible to even attempt, not merely astronomically unlikely. All three
//! retirements are marked below rather than silently dropped -- **the sixteen are still all sixteen,
//! four now retired with a named reason and, where one exists, a named replacement**, not twelve.
//!
//! - **Non-canonical ref pointer path** and **`ensure_ref_path_shape`** (`by-id/`, `logs/`, both
//!   sub-arms) retired: there is no per-ref file and no per-entry path under the shared-container
//!   model to be non-canonical or wrong-shaped -- entries are located by offset within one shared
//!   file, discovered by replaying it, never by listing a directory and checking a filename.
//!   **Replaced, not merely removed**: `read_one_pointer_entry`'s `ref_name_key_bytes(&entry.ref_name)
//!   != entry.ref_name_key` check and `validate_log_replay`'s identical check on the log-container
//!   side are the direct successors -- header/key-vs-content coherence replacing filename-vs-content
//!   coherence, the same underlying property ("this record's own claimed identity disagrees with what
//!   it actually contains") through the new storage shape. Both new rows below.
//! - **Duplicate pointer identity / duplicate ref-log identity** retired: under "last entry wins,"
//!   multiple entries sharing a `ref_name_key` are the ordinary republish mechanism, not a collision
//!   to detect -- there is no "second insert into a name-keyed map" operation left to collide, so the
//!   scenario these two rows described cannot be attempted at all under the current design, not merely
//!   requiring a SHA-256 collision to reach as before.
//!
//! | Check | Classification |
//! |---|---|
//! | Incomplete log tail without pointer lead | Load-bearing (unchanged mechanism, container-based replay) |
//! | Catch-all "unexplained pointer/log divergence" | Load-bearing (unchanged mechanism) |
//! | `created_at == 0` | Load-bearing -- **now enforced at write time too** (`container::append_ref_container_record`, design-v1.md §13.15), not only at read time; a real production gap in the Stage 4 rewrite, found and closed |
//! | `CANDIDATE-DEBRIS` | Non-blocking -- **now reachable only via a directly-planted fixture, never a real crash** (the candidate-write mechanism it detected is gone entirely); a dormant mutation wedge (design-v1.md §13.14), not fixed here |
//! | Duplicate pointer identity / duplicate ref-log identity | **Retired** (design-v1.md §13.12-13.13) -- see prose above |
//! | Non-canonical ref pointer path | **Retired**, replaced by "Pointer-index entry key mismatch" below |
//! | RefState name mismatches pointer | Downstream-redundant (`classify_ref_state`'s own coherence arm), unchanged |
//! | Pointer-index entry key mismatch (new, RFC 102 Stage 4) | Load-bearing (`refs/pointer_index.rs::read_one_pointer_entry`), design-v1.md §13.13 -- was untested before this stage's own checkpoint review found it, not merely newly added |
//! | Log-container record key mismatch (new, RFC 102 Stage 4) | Load-bearing (`refs/verify/scan.rs::validate_log_replay`) |
//! | Pointer index fails closed on any damaged entry (new, RFC 102 Stage 4) | Load-bearing -- deliberately asymmetric with the log container's own item-contained isolation: "last entry wins" makes silently skipping a damaged *latest* entry dangerous (an older entry for the same ref could resolve as current instead), so `read_pointers` refuses the whole read rather than isolating the damage. Design-v1.md §13.14 accepts this for Stage 4 but registers the wider blast radius (one bad entry now blocks every ref, not just its own) as a known regression against amended constraint 5, not a design choice -- not to be changed inside this stage |
//! | `ensure_ref_target_valid` (dangling Branch/Tag target) | Load-bearing, unchanged |
//! | Ref-log chain/sequence divergence | Load-bearing -- `expected_seq` now computed from a record's position within its own ref's *filtered subsequence* of the shared container (design-v1.md §13.1), not the container's raw physical position; re-proven directly against a physically reordered, individually-valid pair of records (Stage 4 acceptance criterion 3), not assumed carried over |
//! | Ref-log checksum mismatch | Load-bearing -- mechanism moved from the retired `refs/log.rs::read_one_log` to `refs/container.rs`'s own frame decode (`parse_frame_at`) plus `refs/verify/scan.rs::read_logs`; same classification, new code |
//! | `verify_update` RefState/RefUpdate coherence | Load-bearing, unchanged |
//! | RefState unsigned | Downstream-redundant (`PublicationTrustVerifier`), unchanged |
//! | `ensure_ref_path_shape` (`by-id/`, `logs/`) | **Retired**, both sub-arms -- see prose above; the log-container half's coverage is subsumed by `container/tests.rs`'s own corruption-isolation tests, the pointer-index half's by the fail-closed row above and `pointer_index/tests.rs`'s new corruption-isolation tests (a genuine coverage gap this stage's checkpoint review found and closed, design-v1.md §13.13, not merely a redundancy claim carried over) |
//! | Signature-envelope issues, `RefLog` source | Excluded (see `signature_envelope_issues` caveat below), unchanged |
//!
//! ## `verify/ref_publication.rs`
//!
//! `mark_unproved` reclassification and `ACTIVE-CLEANUP-PENDING` were both already end-to-end covered
//! before DC-95 started; not part of Stage 1's gap-closing scope.
//!
//! ## `wal.rs` / `verify_wal_persistence` / `rollback_verify.rs` (`verify/tests/wal_cluster.rs`)
//!
//! | Check | Classification |
//! |---|---|
//! | `Wal::replay()` checksum mismatch | Load-bearing; RFC 102 Stage 2 made the failure item-level (`wal_record_outcomes`) rather than a whole-`WalReplay`-stage abort -- see the RFC 102 Stage 2 section above |
//! | `verify_wal_persistence` type mismatch | Load-bearing |
//! | Rollback WAL envelope type | Unreachable (`is_rollback_draft_envelope` already guarantees Patch type before this check runs) |
//! | Rollback WAL decode (op_seq contiguity) | Load-bearing |
//! | Rollback WAL apply-support (`DeleteNode(symlink)`) | Load-bearing |
//! | Rollback WAL empty-ops | Unreachable (`decode_patch_operations` already errors before returning empty) |
//! | Rollback AUTHOR signature: missing | Load-bearing |
//! | Rollback AUTHOR signature: wrong algorithm | Unreachable (`SignatureAlgorithm` has exactly one variant) |
//! | Rollback AUTHOR signature: legacy marker key id | Load-bearing |
//! | Rollback AUTHOR signature: wrong length | Unreachable (RFC 103: was reachable only under format-1, now retired; kept, untested, argument recorded in `rollback_verify.rs`) |
//! | Signature-envelope issues, `ActiveWal` source | Excluded (see caveat below) |
//!
//! ## Active-WAL metadata status + WAL ordering (DC-66, `verify/tests.rs`)
//!
//! | Check | Classification |
//! |---|---|
//! | `MissingForEmptyWal` / `ValidForEmptyWal` | Excluded, non-blocking, both |
//! | `InvalidForNonEmptyWal` | Load-bearing |
//! | Active-WAL ordering violations | Load-bearing |
//!
//! ## `verify/trust.rs` / `trust.rs` (`verify/tests/trust.rs`)
//!
//! | Check | Classification |
//! |---|---|
//! | `PRIKK-TRUST-POLICY-INVALID` (missing/malformed policy) | Load-bearing |
//! | `PRIKK-TRUST-PUBLICATION-UNTRUSTED` | Load-bearing |
//!
//! ## `commit_index.rs` (DC-56) / `lifecycle_cache/incremental.rs` (DC-64) (`crates/prikk-cli/tests/dc64_baseline_cache.rs`)
//!
//! | Check | Classification |
//! |---|---|
//! | Commit-index content divergence | Load-bearing |
//! | Lifecycle-cache content-disagrees divergence | Load-bearing |
//! | Lifecycle-cache "could not be independently verified" | Load-bearing (horizon-anchored replay vs. `block_state.rs`'s non-horizon-anchored one -- the one genuine asymmetry between two otherwise-equivalent replay paths) |
//!
//! ## Standing caveat: `signature_envelope_issues`
//!
//! `signature_envelope_issues` (one `Vec` on [`RepositoryVerification`], populated from every
//! `SignatureEnvelopeSource`) backs no `has_*` blocking predicate, for any source -- an open question,
//! not a settled design: should the `MALFORMED` variant be wired into a blocking predicate? Every
//! "Excluded" row above that names this caveat, plus every "Load-bearing, via non-blocking-sibling
//! mechanism" row, depends on this staying `false`. If it's ever answered the other way, those rows
//! reopen.
//!
//! **RFC 103 note, not a status change:** `signature_diagnostics.rs::classify_signature_envelope`'s
//! own doc records that its non-empty-result path is now provably unreachable through
//! `verify_repository` at all (every call site runs it immediately after a `validate_read_schema`
//! call that already hard-errors on the same three conditions, once format-1's lenient read branch is
//! gone). This does not reopen anything above: every row this caveat covers was already "Excluded" --
//! never blocking, for any source -- so a diagnostic layer becoming unreachable removes no coverage
//! `verify` depended on.

use std::path::PathBuf;

mod objects;
mod ref_publication;
mod trust;

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectId, ObjectType, RefKind, RefStatePayload};

use crate::block_state::{BlockStateOutcome, BlockStateStatus};
use crate::commit_boundary::active::ActiveRefMetadata;
use crate::commit_index::{CommitIndexDivergence, verify_divergence};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryFormat, RepositoryLayout};
use crate::lifecycle_cache::incremental::{
    LifecycleCacheDivergence, verify_divergence as verify_lifecycle_cache_divergence,
};
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::received::list_received_pointers;
use crate::refs::{RefItemOutcome, RefItemStatus, RefStore, ensure_ref_target_valid, verify_refs};
use crate::rollback::verify::{verify_rollback_draft_wal_records, verify_rollback_patch_envelope};
use crate::signature_diagnostics::{
    SignatureEnvelopeIssue, SignatureEnvelopeSource, classify_signature_envelope,
};
use crate::trust::PublicationTrustIssue;
use crate::wal::Wal;

use objects::verify_objects;
pub use objects::{ObjectItemOutcome, ObjectItemStatus};
use trust::PublicationTrustVerifier;

/// Verification summary for a single persisted object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectVerification {
    /// The object ID parsed from the object filename.
    pub object_id: ObjectId,
    /// The object type implied by the directory being scanned.
    pub object_type: ObjectType,
    /// The object file path that was checked.
    pub path: PathBuf,
    /// Rollback-marked Patch references verified for this object when it is a Block.
    pub rollback_patch_count: usize,
    /// For a Block or RefState, the adopted MAINTAINER key id whose signature was trusted (DC-78
    /// §D3). `None` for other object types, or when publication trust could not be established.
    pub sealed_by_key_id: Option<String>,
    /// For a Patch, the outcome of checking its AUTHOR signature against recorded key material
    /// (DC-53 Stage 1). `None` for other object types, or when the Patch carries no AUTHOR-role
    /// signature at all (out of this increment's scope). A signature that does not verify against
    /// *recorded* material never reaches this field -- it fails this object's own item check
    /// instead, the same as any other authorship-integrity defect.
    pub author_verification: Option<AuthorSignatureVerification>,
}

/// The result of checking one Patch's AUTHOR signature (DC-53 Stage 1, D3's first two rows --
/// there is no `Fails` variant here because that outcome is a genuine item-level failure,
/// propagated as an `Err` the same way every other authorship-integrity defect in this pipeline is,
/// not a value this type carries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthorSignatureVerification {
    /// The signature verifies against key material recorded for this `key_id`.
    Sound {
        /// The AUTHOR key id the signature named and verified against.
        key_id: String,
    },
    /// No key material has ever been recorded for this `key_id` -- authored before this
    /// increment, or by a signer whose material this repository never observed. **Not a
    /// failure**: `verify` still passes, but this must be visible, not silent (DC-53 Stage 1 D3's
    /// second row).
    Unverifiable {
        /// The AUTHOR key id named, for which no key material is on file.
        key_id: String,
    },
}

/// Which adopted MAINTAINER key sealed a given Block (DC-78 §D3). Reporting only: the sealer's key
/// id already lives, non-strippably, inside the block's own signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockSealVerification {
    /// The sealed Block's object id.
    pub block_id: ObjectId,
    /// The MAINTAINER key id whose trusted signature matched this Block.
    pub sealed_by_key_id: String,
}

// RFC 118 stage 4: `VerificationStage`, `VerificationStage::ALL`, and `VerificationStage::label()`
// are generated together from the single variant list below, by this macro, rather than hand-
// written as three separately-maintained lists that a hand-added variant could silently omit from
// one or two of. This is not a checked invariant -- it is a structural one: `ALL` and `label()`'s
// match are built from literally the same token list the enum's own variants come from, in the
// same macro expansion, so a variant absent from `ALL` or from `label()` is impossible to write,
// not merely absent from something that verifies it was included. Adding a fifteenth stage means
// adding one arm to this one invocation; there is nowhere else to add it, and nothing else to keep
// in sync.
macro_rules! verification_stages {
    (
        $(
            $(#[$doc:meta])*
            $variant:ident => $label:literal,
        )+
    ) => {
        /// One of the top-level scopes `verify_repository`'s pipeline is organized into (DC-95
        /// Stage 2 Level 1: scope containment). Named in pipeline order; the order itself is
        /// load-bearing for `NotEvaluated` naming (`StageStatus::NotEvaluated`'s `blocked_by` is
        /// always an earlier stage). **The exact count is [`VerificationStage::ALL`]'s length,
        /// never spelled as a number** (RFC 118 stage 4) -- see this enum's own defining macro,
        /// immediately above.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum VerificationStage {
            $(
                $(#[$doc])*
                $variant,
            )+
        }

        impl VerificationStage {
            /// Every stage, in pipeline order. **Generated by this enum's own defining macro
            /// invocation, not a second, independently maintained list** -- a variant cannot be
            /// added to the enum without appearing here, because both come from the same macro
            /// expansion of the same token list (RFC 118 stage 4; see `crates/prikk-store/src/
            /// verify.rs`'s own `verification_stages!` definition for why this cannot drift, and
            /// this module's `all_covers_every_variant` test, control 2, for a demonstration).
            pub const ALL: &'static [Self] = &[$(Self::$variant),+];

            /// Stable, lowercase-hyphenated scope name for diagnostics and CLI output. **This is
            /// an external interface** (RFC 118 stage 4): `prikk verify --format json` (stage 2)
            /// uses these strings verbatim as JSON keys. Renaming, removing, or reusing one is a
            /// breaking change to any tool that parses that output, not a cosmetic rename -- treat
            /// it with the same care as a public API, even though nothing enforces that today.
            #[must_use]
            pub const fn label(self) -> &'static str {
                match self {
                    $(Self::$variant => $label,)+
                }
            }
        }
    };
}

verification_stages! {
    /// Persisted-object scan: identity, placement, envelope decoding, sealed block references,
    /// publication trust for Block/RefState (shares `PublicationTrustVerifier` with
    /// `RefUpdateSchemaTrust`).
    Objects => "objects",
    /// Joint ref pointer/log verification: structural shape, publication-state classification.
    Refs => "refs",
    /// Per-`RefUpdate`-envelope format-read-schema validation and publication trust (shares
    /// `PublicationTrustVerifier` with `Objects`).
    RefUpdateSchemaTrust => "ref-update-schema-trust",
    /// Active-WAL replay: framing, checksums, envelope decoding.
    WalReplay => "wal-replay",
    /// Active-WAL patch persistence cross-check against the object store.
    WalPersistence => "wal-persistence",
    /// Active-WAL rollback-draft classification and structural validation.
    RollbackDrafts => "rollback-drafts",
    /// Per-active-WAL-record format-read-schema validation and signature-envelope classification.
    WalRecordSchema => "wal-record-schema",
    /// Active-WAL ref-ownership metadata classification.
    ActiveWalMetadata => "active-wal-metadata",
    /// Retained-evidence reclassification of interrupted-publication ref issues.
    PublicationReclassification => "publication-reclassification",
    /// DC-56 commit-index cache divergence check.
    CommitIndex => "commit-index",
    /// DC-64 incremental lifecycle-state cache divergence check.
    LifecycleCache => "lifecycle-cache",
    /// DC-66 active-WAL queue-ordering check.
    WalOrdering => "wal-ordering",
    /// RFC 115 Stage 3 §6: received (`remotes/*`) ref target validation -- the same kind-aware
    /// two-hop check local refs already get, applied to the received namespace, which nothing
    /// scanned before this stage existed.
    ReceivedRefs => "received-refs",
    /// DC-78 verify-local-tag-publication-trust (v2 amendment): a locally-published Tag's own
    /// MAINTAINER signature, checked against the repository-local trust policy (shares
    /// `PublicationTrustVerifier` with `Objects` and `RefUpdateSchemaTrust`). Enumerates local tag
    /// refs independently via `RefStore::list_ref_pointers` rather than reading `Refs`' own output --
    /// deliberately not threaded through `verify_refs`, which `ensure_no_incomplete_publication`
    /// (a pre-mutation guard, not `prikk verify`) also calls. A received, not-yet-adopted tag is
    /// never reached here: `list_ref_pointers` never enumerates the received namespace.
    LocalTagTrust => "local-tag-trust",
}

impl std::fmt::Display for VerificationStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Outcome of attempting to evaluate one verification stage (DC-95 Stage 2 Level 1). **No stage may be
/// silently absent from a report.** A stage's own check raising an error is recorded as a blocking
/// finding against its scope rather than aborting the rest of verification (`Failed`); a stage that
/// could not run because a real dependency did not evaluate is itself blocking, not silently skipped
/// (`NotEvaluated`); a stage that could have run on its own terms but was preempted by an operator-
/// requested early stop is also blocking, but for a different reason it must not be confused with
/// (`Halted`) — a repository whose verification is incomplete is not verified, regardless of which of
/// the three non-`Evaluated` states explains the gap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StageStatus {
    /// The stage ran to completion; its findings and counts are authoritative.
    Evaluated,
    /// The stage's own check raised an error.
    Failed {
        /// The error the stage raised.
        message: String,
    },
    /// The stage could not run because a *real* dependency did not evaluate — `blocked_by` names a
    /// stage this one's own logic actually reads output from. This is a dependency-graph claim, and
    /// must remain true of the graph even when `--stop-on-first-error` is in effect; see `Halted` for
    /// the case where a stage merely followed an unrelated earlier stop.
    NotEvaluated {
        /// The earlier stage whose own non-evaluation is why this one could not run.
        blocked_by: VerificationStage,
    },
    /// The stage was never attempted because an earlier, *unrelated* stage's failure already stopped
    /// the walk under `--stop-on-first-error` (DC-95 Stage 2 Level 1 implementation review v1 §4) —
    /// `after` names the stage whose failure triggered the stop, not a dependency of this stage. Kept
    /// distinct from `NotEvaluated` because `blocked_by` is a dependency-graph claim: reporting
    /// `NotEvaluated { blocked_by: Objects }` for a stage that does not actually depend on `Objects`
    /// (e.g. `LifecycleCache`) would assert an edge that does not exist.
    Halted {
        /// The stage whose failure caused the walk to stop before this stage was reached.
        after: VerificationStage,
    },
}

impl StageStatus {
    /// Return true for any status other than a clean, completed evaluation. `NotEvaluated` and
    /// `Halted` are both blocking on the same footing as `Failed` — an incomplete verification is not
    /// a passing one, whichever of the three explains the gap.
    #[must_use]
    pub const fn is_blocking(&self) -> bool {
        !matches!(self, Self::Evaluated)
    }
}

/// One stage's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StageOutcome {
    /// Which of [`VerificationStage::ALL`] this outcome is for.
    pub stage: VerificationStage,
    /// How that stage resolved.
    pub status: StageStatus,
}

/// Repository verification summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepositoryVerification {
    /// Outcome of each stage in [`VerificationStage::ALL`] (DC-95 Stage 2 Level 1), in pipeline
    /// order. Always exactly one entry per [`VerificationStage::ALL`] member — no stage may be
    /// silently absent, including under `--stop-on-first-error`
    /// (`verify_repository_reports_exactly_one_outcome_per_all_entry` and its
    /// `stop_on_first_error` sibling, `verify/tests/stage_containment.rs`, gate this against a real
    /// repository, not a hand-built value).
    pub stage_outcomes: Vec<StageOutcome>,
    /// Phase A: one outcome per persisted object file scanned, in scan order (DC-95 Stage 2 Level 2).
    /// Empty when the `Objects` stage itself did not evaluate (a structural directory-shape error) —
    /// nothing was attempted, distinct from a non-empty set where every entry happens to be `Failed`.
    pub object_outcomes: Vec<ObjectItemOutcome>,
    /// Phase B: one outcome per `CurrentV6` Block whose Phase A check succeeded, in the
    /// state-dependency order `verify_blocks_topological` resolved them — not scan order (DC-92
    /// §4.2). Empty when the `Objects` stage did not evaluate, or when no `CurrentV6` Block passed
    /// Phase A at all.
    pub block_state_outcomes: Vec<BlockStateOutcome>,
    /// Number of persisted object files whose own Phase A checks ran to completion (DC-95 Stage 2
    /// Level 2). `None` only when the `Objects` stage itself did not evaluate (a structural
    /// directory-shape error) — under item containment this is no longer the same claim as "every
    /// object in the store is individually sound": some entries in `object_outcomes` may themselves
    /// be `Failed` while this count still reflects how many succeeded. Never a partial claim about
    /// state-root soundness, which `block_state_outcomes` is the only source of truth for (Level 2
    /// handoff §7 Q3 — `checked_blocks` below keeps its pre-Level-2 meaning unchanged).
    pub checked_objects: Option<usize>,
    /// Number of active WAL records replayed successfully. `None` when the WAL-replay stage did not
    /// evaluate to completion.
    pub checked_wal_records: Option<usize>,
    /// One outcome per attempted WAL record frame, in scan order (RFC 102 Stage 2: isolate-and-
    /// continue reading). Empty when the `WalReplay` stage itself did not evaluate.
    pub wal_record_outcomes: Vec<crate::wal::WalRecordOutcome>,
    /// Number of persisted Block objects whose references (parent, patch, snapshot existence, merge
    /// baseline) were checked successfully — a Phase A claim only, never a claim about state-root
    /// soundness (see `block_state_outcomes`). `None` only when the `Objects` stage itself did not
    /// evaluate. This field's meaning is unchanged by Level 2 (handoff §7 Q3) — only *when* it is
    /// `None` changed, from "the whole stage failed" to "the whole stage did not evaluate at all."
    pub checked_blocks: Option<usize>,
    /// Number of persisted Block objects classified as rollback blocks, among those whose Phase A
    /// check succeeded. `None` only when the `Objects` stage itself did not evaluate.
    pub checked_rollback_blocks: Option<usize>,
    /// Number of sealed rollback-marked Patch objects referenced by Blocks whose Phase A check
    /// succeeded. `None` only when the `Objects` stage itself did not evaluate.
    pub checked_sealed_rollback_patches: Option<usize>,
    /// Number of active WAL patch records that already exist as persisted patch objects. `None` when
    /// the WAL-persistence stage did not evaluate to completion.
    pub persisted_wal_patches: Option<usize>,
    /// Number of ref pointer files whose own Phase-A-equivalent read succeeded (DC-95 Stage 2
    /// Level 2). `None` only when the `Refs` stage itself did not evaluate.
    pub checked_refs: Option<usize>,
    /// Number of inline ref-log records read successfully. `None` only when the `Refs` stage
    /// itself did not evaluate.
    pub checked_ref_log_records: Option<usize>,
    /// Interrupted ref-publication and candidate-debris conditions found by joint verification. Stays
    /// a plain `Vec` under stage containment: entries already pushed by a stage that later failed
    /// remain real findings; only the count/emptiness-as-proof reasoning needed a stage-aware guard
    /// (see `require_retained_evidence`'s own `trust_is_valid` computation).
    pub ref_publication_issues: Vec<crate::refs::RefPublicationIssue>,
    /// One outcome per ref pointer file scanned, in scan order (DC-95 Stage 2 Level 2). Empty when
    /// the `Refs` stage itself did not evaluate.
    pub pointer_outcomes: Vec<crate::refs::RefFileOutcome>,
    /// One outcome per ref log file scanned, in scan order. Empty when the `Refs` stage itself did
    /// not evaluate.
    pub log_outcomes: Vec<crate::refs::RefFileOutcome>,
    /// One outcome per ref name reached via a successfully-read pointer or log. Empty when the
    /// `Refs` stage itself did not evaluate.
    pub ref_item_outcomes: Vec<crate::refs::RefItemOutcome>,
    /// Warning-level format-1 signature-envelope compatibility findings in deterministic order.
    pub signature_envelope_issues: Vec<SignatureEnvelopeIssue>,
    /// Number of active WAL records classified and decoded as rollback drafts. `None` when the
    /// rollback-drafts stage did not evaluate to completion.
    pub checked_rollback_draft_records: Option<usize>,
    /// Number of publication envelopes checked against repository-local trust. `None` unless *both*
    /// the objects stage and the ref-update schema/trust stage evaluated to completion — this count is
    /// contributed to by both, sharing one `PublicationTrustVerifier` instance across them.
    pub checked_publication_trust_records: Option<usize>,
    /// Publication-trust issues found while structural verification succeeded. Stays a plain `Vec` —
    /// entries genuinely found before an interrupting failure remain real findings.
    pub publication_trust_issues: Vec<PublicationTrustIssue>,
    /// Recognized non-authoritative object publication temps left for explicit maintenance.
    pub object_temp_paths: Vec<PathBuf>,
    /// Number of trailing bytes in the active WAL that look like an incomplete final record. `None`
    /// when the WAL-replay stage did not evaluate to completion.
    pub trailing_partial_wal_bytes: Option<usize>,
    /// Active-WAL ref metadata status relative to the replayed WAL. `None` when the active-WAL-metadata
    /// stage did not evaluate to completion.
    pub active_wal_metadata_status: Option<ActiveWalMetadataStatus>,
    /// DC-56 commit-index entries whose recorded content hash disagrees with the worktree's actual
    /// current content despite a matching stat — a stale-but-trusted cache entry, reported per the
    /// cache-validity specification §6 rather than silently trusted by a future commit.
    pub commit_index_divergences: Vec<CommitIndexDivergence>,
    /// DC-64 incremental lifecycle-state cache entries whose contents disagree with an independent
    /// full replay of the block they claim to represent — reported per the design document §6
    /// rather than silently trusted by a future commit.
    pub lifecycle_cache_divergences: Vec<LifecycleCacheDivergence>,
    /// DC-66: active WAL queue-ordering violations — a record whose sequence does not strictly
    /// increase over its predecessor. Adversarial-only under normal operation (`Wal::append_patch`
    /// always assigns the next sequence), but a queue of N gives ordering a meaning ("patches seal in
    /// append order") worth verifying explicitly rather than assuming from decode success alone.
    pub active_wal_ordering_issues: Vec<ActiveWalOrderingIssue>,
    /// DC-75: `Merge` blocks whose recorded `merge_baseline_block_id` is not, in fact, a common
    /// ancestor of both parents — independently re-derived, not trusted, per
    /// `baseline-recording-answer-v1.md` §3 ("record it, then check it, unconditionally"). A recorded
    /// baseline that legitimate merge execution ever produced always passes this; a false claim (data
    /// corruption or tampering) does not.
    pub merge_baseline_divergences: Vec<MergeBaselineDivergence>,
    /// Which adopted MAINTAINER key sealed each checked Block (DC-78 §D3), in on-disk scan order.
    /// Reporting only — surfaces provenance that was already intrinsic to each block's own
    /// signature, so an auditor can ask "which parts of this history did I seal" and get an answer.
    pub block_seals: Vec<BlockSealVerification>,
    /// RFC 115 Stage 3 §6: one outcome per received (`remotes/*`) pointer, in
    /// `list_received_pointers`' sorted-by-name order — the same kind-aware two-hop target check
    /// local refs already get, applied here for the first time. Empty when the `ReceivedRefs` stage
    /// itself did not evaluate. A repository with a genuinely dangling received ref (its target
    /// object was never shipped) now reports an item failure here where nothing reported anything
    /// before this stage existed — see the stage's own module note for what that means for a
    /// repository that already holds one.
    pub received_ref_item_outcomes: Vec<RefItemOutcome>,
    /// Number of active sessions present on disk when this verification ran (RFC 108 §D3.4,
    /// increment 3b). **Not itself a verification result** -- every other field on this report
    /// describes exactly `DEFAULT_ACTIVE_NAME`'s sealed-history state, unconditionally, regardless
    /// of this count. `verify`'s claim is about sealed history; an active session's own unsealed WAL
    /// is not sealed history by definition (§D3.4's own ruling: *"`verify` reports Workspaces as out
    /// of scope, explicitly... Silence would be the wrong answer"*). This field exists so that scope
    /// boundary is stated, not silent -- the project's rule that absence must be explicit, applied
    /// here to a *coverage* absence rather than a missing-directory one. Sourced from
    /// `RepositoryLayout::active_session_names`, never a second, independently derived count.
    pub active_session_count: usize,
}

/// A `Merge` block (DC-75) whose recorded `merge_baseline_block_id` is not a common ancestor of its
/// two parents. Precision note: this checks *validity* (is the claim even a common ancestor), not
/// *nearest-ness* (is it the single nearest one) — a merge legitimately sealed against an older-than-
/// necessary common ancestor is unusual but not what this finding is for; a baseline that is not a
/// common ancestor at all can only arise from a forged or corrupted field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeBaselineDivergence {
    /// The `Merge` block whose recorded baseline failed re-derivation.
    pub block_id: ObjectId,
    /// The recorded (claimed) baseline.
    pub recorded_baseline: ObjectId,
    /// The block's mainline parent.
    pub mainline_parent_id: ObjectId,
    /// The block's secondary parent.
    pub secondary_parent_id: ObjectId,
}

/// One active-WAL record whose sequence did not strictly increase over the previous record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveWalOrderingIssue {
    /// Zero-based position of the offending record within the replayed WAL.
    pub index: usize,
    /// Sequence of the previous record.
    pub previous_seq: u64,
    /// Sequence of the offending record (not greater than `previous_seq`).
    pub seq: u64,
}

impl RepositoryVerification {
    /// Return true when any stage in [`VerificationStage::ALL`] did not evaluate cleanly — either its
    /// own check raised an error (`Failed`) or a dependency's non-evaluation prevented it from running
    /// at all (`NotEvaluated`). A repository whose verification did not run to completion is not
    /// verified, regardless of what the stages that did run found. Checked first, ahead of every
    /// finding-specific predicate below: those predicates' own backing data can itself be incomplete
    /// precisely because a stage failed, so this is the more fundamental question.
    ///
    /// **Does not, by itself, cover item-level defects (DC-95 Stage 2 Level 2)** — the `Objects`
    /// stage evaluates cleanly (`Evaluated`) even when one of its items individually failed, since
    /// item containment means a bad object no longer aborts the whole stage. See
    /// [`Self::has_item_failure`] for that question. **Neither this predicate, nor
    /// `has_item_failure`, nor any other single predicate on this type answers "would `prikk
    /// verify` refuse this repository"** — that question has nine conditions, not two, declared
    /// once in `prikk-cli`'s own `VERDICT_CONDITIONS` (RFC 118 stage 5) and read by both the exit
    /// code and `--format json`. A `has_blocking_defect()` combining only these two used to exist
    /// here and was removed (RFC 118 stage 5): it had no caller, and its doc claimed to be "the
    /// yes/no answer" while silently excluding publication-trust, commit-index, lifecycle-cache,
    /// active-WAL-ordering, merge-baseline, and ref-publication issues — exactly the shape of claim
    /// this project has spent a month removing.
    #[must_use]
    pub fn has_stage_failure(&self) -> bool {
        self.stage_outcomes
            .iter()
            .any(|outcome| outcome.status.is_blocking())
    }

    /// Return true when any individual item did not evaluate cleanly (DC-95 Stage 2 Level 2) — a
    /// Phase A object whose own check failed, a Phase B `CurrentV6` Block whose state-root check
    /// failed or could not be attempted because its own state-derivation parent did not evaluate,
    /// or a ref (its pointer file, log file, or classification) that failed. Item containment means
    /// these no longer make [`Self::has_stage_failure`] true: the owning stage itself completed, so
    /// this is a genuinely separate question, not a more detailed view of the same one. The backing
    /// `Vec`s are empty (not merely all-`Evaluated`) when their owning stage itself did not
    /// evaluate — this method reads that case as `false`, same as every other item-backed predicate
    /// in this type; `has_stage_failure` is what is already true for it.
    #[must_use]
    pub fn has_item_failure(&self) -> bool {
        self.object_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, ObjectItemStatus::Failed { .. }))
            || self
                .block_state_outcomes
                .iter()
                .any(|outcome| !matches!(outcome.status, BlockStateStatus::Verified))
            || self
                .pointer_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, crate::refs::RefFileStatus::Failed { .. }))
            || self
                .log_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, crate::refs::RefFileStatus::Failed { .. }))
            || self
                .ref_item_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, crate::refs::RefItemStatus::Failed { .. }))
            || self
                .wal_record_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, crate::wal::WalRecordStatus::Failed { .. }))
            || self
                .received_ref_item_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, RefItemStatus::Failed { .. }))
    }

    /// Return true if the active WAL contained an incomplete trailing record. `None` (the WAL-replay
    /// stage did not evaluate) reads as false here — that condition is already surfaced, more
    /// precisely, by `has_stage_failure`.
    #[must_use]
    pub fn has_trailing_partial_wal(&self) -> bool {
        self.trailing_partial_wal_bytes.is_some_and(|n| n != 0)
    }

    /// Return true when all structurally verified publication objects also passed trust checks.
    #[must_use]
    pub fn has_publication_trust_issues(&self) -> bool {
        !self.publication_trust_issues.is_empty()
    }

    /// Return true when pointer/log state requires signer-backed recovery or manual intervention.
    #[must_use]
    pub fn has_blocking_ref_publication_issues(&self) -> bool {
        self.ref_publication_issues
            .iter()
            .any(|issue| issue.blocking)
    }

    /// Return true when a non-empty active WAL lacks valid ownership metadata. `None` (the
    /// active-WAL-metadata stage did not evaluate) reads as false here — see `has_trailing_partial_wal`.
    #[must_use]
    pub fn has_active_wal_metadata_integrity_issue(&self) -> bool {
        self.active_wal_metadata_status
            .as_ref()
            .is_some_and(ActiveWalMetadataStatus::has_integrity_issue)
    }

    /// Return true when an empty active WAL has stale local metadata debris. `None` reads as false —
    /// see `has_trailing_partial_wal`.
    #[must_use]
    pub fn has_active_wal_metadata_warning(&self) -> bool {
        self.active_wal_metadata_status
            .as_ref()
            .is_some_and(ActiveWalMetadataStatus::has_local_debris_warning)
    }

    /// Return true when the commit-index cache disagrees with the worktree for at least one path.
    #[must_use]
    pub fn has_commit_index_divergence(&self) -> bool {
        !self.commit_index_divergences.is_empty()
    }

    /// Return true when the incremental lifecycle-state cache disagrees with an independent replay.
    #[must_use]
    pub fn has_lifecycle_cache_divergence(&self) -> bool {
        !self.lifecycle_cache_divergences.is_empty()
    }

    /// Return true when the active WAL contains an out-of-order or duplicate sequence.
    #[must_use]
    pub fn has_active_wal_ordering_issue(&self) -> bool {
        !self.active_wal_ordering_issues.is_empty()
    }

    /// Return true when a `Merge` block's recorded baseline is not a common ancestor of its parents
    /// (DC-75) — a false claim, from data corruption or tampering.
    #[must_use]
    pub fn has_merge_baseline_divergence(&self) -> bool {
        !self.merge_baseline_divergences.is_empty()
    }
}

/// Active-WAL ref metadata status derived during repository verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveWalMetadataStatus {
    /// Empty active WAL and no metadata.
    MissingForEmptyWal,
    /// Empty active WAL with stale but valid local metadata.
    ValidForEmptyWal {
        /// Ref recorded in the stale metadata.
        ref_name: String,
    },
    /// Empty active WAL with malformed local metadata.
    InvalidForEmptyWal {
        /// Parse or validation failure.
        reason: String,
    },
    /// Non-empty active WAL with valid ownership metadata.
    ValidForNonEmptyWal {
        /// Ref recorded in the active metadata.
        ref_name: String,
    },
    /// Non-empty active WAL missing required ownership metadata.
    MissingForNonEmptyWal,
    /// Non-empty active WAL with malformed ownership metadata.
    InvalidForNonEmptyWal {
        /// Parse or validation failure.
        reason: String,
    },
}

impl ActiveWalMetadataStatus {
    /// Return true when the status represents a repository-integrity issue.
    #[must_use]
    pub const fn has_integrity_issue(&self) -> bool {
        matches!(
            self,
            Self::MissingForNonEmptyWal | Self::InvalidForNonEmptyWal { .. }
        )
    }

    /// Return true when the status represents local debris on an otherwise empty active WAL.
    #[must_use]
    pub const fn has_local_debris_warning(&self) -> bool {
        matches!(
            self,
            Self::ValidForEmptyWal { .. } | Self::InvalidForEmptyWal { .. }
        )
    }
}

/// Threads stage outcomes, and (optionally) an early-halt decision, through `verify_repository`'s
/// pipeline (DC-95 Stage 2 Level 1's `--stop-on-first-error`, design §7 and §12.3).
struct StagePipeline {
    outcomes: Vec<StageOutcome>,
    stop_on_first_error: bool,
    halted_by: Option<VerificationStage>,
}

impl StagePipeline {
    fn new(stop_on_first_error: bool) -> Self {
        Self {
            outcomes: Vec::with_capacity(12),
            stop_on_first_error,
            halted_by: None,
        }
    }

    /// Attempt a stage with no real dependency beyond a possible earlier halt. Returns the value on
    /// success; `None` on failure, or if an earlier stage already halted the walk. A stage reached
    /// through `run` never has a real declared dependency (a stage that does is gated behind an
    /// `if`/`else` at its call site and reaches `not_evaluated` instead when ungated) -- so an
    /// already-halted walk is always reported as `Halted`, never a fabricated `NotEvaluated`.
    fn run<T>(&mut self, stage: VerificationStage, result: Result<T>) -> Option<T> {
        if let Some(halted_by) = self.halted_by {
            self.outcomes.push(StageOutcome {
                stage,
                status: StageStatus::Halted { after: halted_by },
            });
            return None;
        }
        match result {
            Ok(value) => {
                self.outcomes.push(StageOutcome {
                    stage,
                    status: StageStatus::Evaluated,
                });
                Some(value)
            }
            Err(err) => {
                self.outcomes.push(StageOutcome {
                    stage,
                    status: StageStatus::Failed {
                        message: err.to_string(),
                    },
                });
                if self.stop_on_first_error {
                    self.halted_by = Some(stage);
                }
                None
            }
        }
    }

    /// Record a stage that cannot run because `blocked_by` -- a real dependency -- did not evaluate.
    /// Always reports `blocked_by` as given, never substituted: a caller only reaches this method when
    /// `blocked_by`'s own stage failed to produce a usable value (DC-95 Stage 2 Level 1 implementation
    /// review v1 §4), so the claim is true of the dependency graph regardless of *why* `blocked_by`
    /// itself did not evaluate -- including when `blocked_by` was itself `Halted`, in which case this
    /// stage is transitively halted too, discoverable by following the chain rather than by this call
    /// reaching past its own real dependency to name an unrelated stage. `--stop-on-first-error` never
    /// originates a fresh halt here: every halt traces back to a `Failed` outcome from `run`, which
    /// already recorded it.
    fn not_evaluated(&mut self, stage: VerificationStage, blocked_by: VerificationStage) {
        self.outcomes.push(StageOutcome {
            stage,
            status: StageStatus::NotEvaluated { blocked_by },
        });
    }

    /// Record a stage whose own check cannot fail (a pure function, or one that already converts
    /// errors into findings) -- still subject to an earlier halt. Returns whether the stage should
    /// actually run its own work. Never a real dependency (see `run`), so an already-halted walk is
    /// `Halted`, not `NotEvaluated`.
    fn run_infallible(&mut self, stage: VerificationStage) -> bool {
        if let Some(halted_by) = self.halted_by {
            self.outcomes.push(StageOutcome {
                stage,
                status: StageStatus::Halted { after: halted_by },
            });
            false
        } else {
            self.outcomes.push(StageOutcome {
                stage,
                status: StageStatus::Evaluated,
            });
            true
        }
    }
}

/// Options controlling how `verify_repository` walks [`VerificationStage::ALL`] (DC-95 Stage 2
/// Level 1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VerifyOptions {
    /// When true, stop at the first stage that fails or cannot evaluate, leaving every later stage
    /// `NotEvaluated` (naming the first halting stage) rather than continuing to accumulate.
    /// Preserves the pre-Stage-2 bounded-walk behavior for a large, badly-damaged repository where a
    /// full accumulating scan would be costly (design §7) -- unbounded growth is concentrated in the
    /// `Objects` stage's whole-store scan. Default `false` (full accumulation).
    pub stop_on_first_error: bool,
}

/// Verify a repository layout without modifying it, with the default options (full accumulation
/// across every stage in [`VerificationStage::ALL`]). See [`verify_repository_with_options`] for
/// `--stop-on-first-error`.
pub fn verify_repository(layout: &RepositoryLayout) -> Result<RepositoryVerification> {
    verify_repository_with_options(layout, VerifyOptions::default())
}

/// Verify a repository layout without modifying it.
pub fn verify_repository_with_options(
    layout: &RepositoryLayout,
    options: VerifyOptions,
) -> Result<RepositoryVerification> {
    // RFC 111 §6.1: `verify` is read-only end to end (never calls `write_object` anywhere in its own
    // call graph, confirmed by RFC 111 Q1's inventory), so it takes one decoded index snapshot here,
    // once, instead of the O(N) per-object-read decode `FileObjectStore` used to pay.
    let object_store = ObjectReadSnapshot::open(layout)?;
    // RFC 108 §D3.4, increment 3b: propagated with `?`, unconditionally, the same footing as
    // `ObjectReadSnapshot::open` immediately above -- a repository whose `active/` root is not
    // enumerable at all is not a case any downstream stage could meaningfully evaluate either, and
    // `active_session_names` itself already tolerates the one case that must not hard-fail (`active/`
    // absent entirely, `Ok(vec![])`). Sourced from `RepositoryLayout::active_session_names`, the same
    // enumeration `unlock`/`doctor` already use -- never a second, independently derived count.
    let active_session_count = layout.active_session_names()?.len();
    let mut trust_verifier = PublicationTrustVerifier::new(layout);
    let mut pipeline = StagePipeline::new(options.stop_on_first_error);

    // Stage: Objects. No upstream stage dependency. `trust_verifier` is mutated by reference and its
    // state survives a `Failed` outcome here, since it lives in this function's own frame rather than
    // inside `verify_objects` -- reused safely by RefUpdateSchemaTrust and PublicationReclassification
    // below (DC-95 Stage 2 Step 0 §3: `PublicationTrustVerifier` cannot manufacture a false "trusted"
    // result from partial evaluation).
    let object_summary = pipeline.run(
        VerificationStage::Objects,
        verify_objects(layout, &object_store, &mut trust_verifier),
    );
    let objects_evaluated = object_summary.is_some();
    let (
        object_outcomes,
        block_state_outcomes,
        checked_objects,
        checked_blocks,
        checked_rollback_blocks,
        checked_sealed_rollback_patches,
        object_temp_paths,
        merge_baseline_divergences,
        block_seals,
        mut signature_envelope_issues,
    ) = match object_summary {
        Some(summary) => {
            let counts = phase_a_counts(&summary.item_outcomes)?;
            (
                summary.item_outcomes,
                summary.topological_outcomes,
                Some(counts.objects),
                Some(counts.blocks),
                Some(counts.rollback_blocks),
                Some(counts.rollback_patches),
                summary.temp_paths,
                summary.merge_baseline_divergences,
                summary.block_seals,
                summary.signature_issues,
            )
        }
        None => (
            Vec::new(),
            Vec::new(),
            None,
            None,
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ),
    };

    // Stage: Refs. No upstream stage dependency.
    let ref_verification = pipeline.run(VerificationStage::Refs, verify_refs(layout));

    // Stage: ReceivedRefs (RFC 115 Stage 3 §6). No upstream stage dependency -- reads the received
    // index and the object store directly, the same footing `Refs` has.
    let received_ref_item_outcomes = pipeline
        .run(
            VerificationStage::ReceivedRefs,
            verify_received_refs(layout, &object_store),
        )
        .unwrap_or_default();

    // Stage: LocalTagTrust (DC-78 verify-local-tag-publication-trust v2 amendment). No upstream stage
    // dependency -- same footing as ReceivedRefs above: enumerates local tag refs directly via
    // `RefStore::list_ref_pointers` instead of reading `Refs`' own output, so `verify_refs` never sees
    // `trust_verifier` (the escalation this stage exists to resolve found a caller of `verify_refs`,
    // `ensure_no_incomplete_publication`, that is not `prikk verify` and must not pay for this).
    // Shares `trust_verifier` with `Objects`/`RefUpdateSchemaTrust`, so an untrusted local Tag surfaces
    // through the identical `publication_trust_issues` path those two already report through.
    let local_tag_trust_evaluated = pipeline
        .run(
            VerificationStage::LocalTagTrust,
            verify_local_tag_publication_trust(layout, &object_store, &mut trust_verifier),
        )
        .is_some();

    // Stage: RefUpdateSchemaTrust. Depends on Refs for the envelope list.
    let ref_update_schema_trust_evaluated = if let Some(rv) = &ref_verification {
        pipeline
            .run(
                VerificationStage::RefUpdateSchemaTrust,
                (|| -> Result<()> {
                    for envelope in &rv.ref_update_envelopes {
                        crate::format::validate_read_schema(layout.format(), envelope)?;
                        trust_verifier.verify(envelope)?;
                    }
                    Ok(())
                })(),
            )
            .is_some()
    } else {
        pipeline.not_evaluated(
            VerificationStage::RefUpdateSchemaTrust,
            VerificationStage::Refs,
        );
        false
    };

    let refs_evaluated = ref_verification.is_some();
    let (
        checked_refs,
        checked_ref_log_records,
        mut ref_publication_issues,
        refs_signature_envelope_issues,
        pointer_outcomes,
        log_outcomes,
        ref_item_outcomes,
    ) = match ref_verification {
        Some(rv) => (
            Some(rv.pointer_count),
            Some(rv.log_record_count),
            rv.publication_issues,
            rv.signature_envelope_issues,
            rv.pointer_outcomes,
            rv.log_outcomes,
            rv.ref_item_outcomes,
        ),
        None => (
            None,
            None,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
            Vec::new(),
        ),
    };

    // Stage: WalReplay. No upstream stage dependency.
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = pipeline.run(VerificationStage::WalReplay, wal.replay());

    // Stage: WalPersistence. Depends on WalReplay.
    let persisted_wal_patches = if let Some(replay) = &replay {
        pipeline.run(
            VerificationStage::WalPersistence,
            verify_wal_persistence(&object_store, &replay.records),
        )
    } else {
        pipeline.not_evaluated(
            VerificationStage::WalPersistence,
            VerificationStage::WalReplay,
        );
        None
    };

    // Stage: RollbackDrafts. Depends on WalReplay.
    let checked_rollback_draft_records = if let Some(replay) = &replay {
        pipeline.run(
            VerificationStage::RollbackDrafts,
            verify_rollback_draft_wal_records(&replay.records),
        )
    } else {
        pipeline.not_evaluated(
            VerificationStage::RollbackDrafts,
            VerificationStage::WalReplay,
        );
        None
    };

    // Stage: WalRecordSchema. Depends on WalReplay.
    if let Some(replay) = &replay {
        pipeline.run(
            VerificationStage::WalRecordSchema,
            (|| -> Result<()> {
                for record in &replay.records {
                    crate::format::validate_read_schema(layout.format(), &record.envelope)?;
                    signature_envelope_issues.extend(classify_signature_envelope(
                        &record.envelope,
                        SignatureEnvelopeSource::ActiveWal {
                            sequence: record.seq,
                            object_id: record.envelope.object_id(),
                        },
                    )?);
                }
                Ok(())
            })(),
        );
        // Matches the pre-Level-1 merge order (Objects, then WAL, then Refs) -- deferred until here,
        // after the WAL per-record loop's own contributions, rather than appended immediately after
        // the Refs stage above, purely to preserve that existing, asserted-on order.
        signature_envelope_issues.extend(refs_signature_envelope_issues);
    } else {
        pipeline.not_evaluated(
            VerificationStage::WalRecordSchema,
            VerificationStage::WalReplay,
        );
        signature_envelope_issues.extend(refs_signature_envelope_issues);
    }

    // Stage: ActiveWalMetadata. Depends on WalReplay.
    let active_wal_metadata_status = if let Some(replay) = &replay {
        pipeline.run(
            VerificationStage::ActiveWalMetadata,
            classify_active_wal_metadata(layout, DEFAULT_ACTIVE_NAME, replay.records.is_empty()),
        )
    } else {
        pipeline.not_evaluated(
            VerificationStage::ActiveWalMetadata,
            VerificationStage::WalReplay,
        );
        None
    };

    // Stage: PublicationReclassification. Cannot run at all without Refs (needs `issues` to mutate),
    // WalReplay (needs `records`), or ActiveWalMetadata (needs `metadata`) -- `NotEvaluated`, naming
    // whichever of those three failed first, if any. Objects failing does *not* block this stage from
    // running: it only degrades `trust_is_valid` to a safe `false` (DC-95 Stage 2 Step 0 ruling §2-§3
    // -- an accumulator's emptiness means "none found" only if its producer ran to completion; reading
    // `trust_verifier.issues.is_empty()` alone would silently claim "proved" from an unrun check).
    match (&replay, refs_evaluated, &active_wal_metadata_status) {
        (Some(replay), true, Some(metadata)) => {
            let trust_is_valid = objects_evaluated && trust_verifier.issues.is_empty();
            pipeline.run(
                VerificationStage::PublicationReclassification,
                ref_publication::require_retained_evidence(
                    layout,
                    &replay.records,
                    metadata,
                    trust_is_valid,
                    &mut ref_publication_issues,
                ),
            );
        }
        _ => {
            let blocked_by = if replay.is_none() {
                VerificationStage::WalReplay
            } else if !refs_evaluated {
                VerificationStage::Refs
            } else {
                VerificationStage::ActiveWalMetadata
            };
            pipeline.not_evaluated(VerificationStage::PublicationReclassification, blocked_by);
        }
    }

    // Stage: CommitIndex. No upstream stage dependency; contained like any other fallible stage --
    // `commit_index::verify_divergence` does return `Result`, unlike `LifecycleCache`'s. An empty
    // `Vec` on `Failed`/`NotEvaluated` is safe here (unlike a count): the stage's own outcome above
    // already says whether "no divergences" was actually established.
    let commit_index_divergences = pipeline
        .run(VerificationStage::CommitIndex, verify_divergence(layout))
        .unwrap_or_default();

    // Stage: LifecycleCache. No upstream stage dependency; cannot fail by construction -- a replay
    // error is itself converted into a divergence entry rather than propagated (DC-95 Stage 1 round
    // 12). Still subject to an earlier halt under `--stop-on-first-error`.
    let lifecycle_cache_divergences = if pipeline.run_infallible(VerificationStage::LifecycleCache)
    {
        verify_lifecycle_cache_divergence(&object_store, layout)
    } else {
        Vec::new()
    };

    // Stage: WalOrdering. Depends on WalReplay; the check itself cannot fail (a pure function).
    let active_wal_ordering_issues = if let Some(replay) = &replay {
        if pipeline.run_infallible(VerificationStage::WalOrdering) {
            check_active_wal_ordering(&replay.records)
        } else {
            Vec::new()
        }
    } else {
        pipeline.not_evaluated(VerificationStage::WalOrdering, VerificationStage::WalReplay);
        Vec::new()
    };

    let checked_publication_trust_records =
        (objects_evaluated && ref_update_schema_trust_evaluated && local_tag_trust_evaluated)
            .then_some(trust_verifier.checked_records);

    Ok(RepositoryVerification {
        stage_outcomes: pipeline.outcomes,
        object_outcomes,
        block_state_outcomes,
        checked_objects,
        checked_wal_records: replay.as_ref().map(|replay| replay.records.len()),
        wal_record_outcomes: replay
            .as_ref()
            .map(|replay| replay.record_outcomes.clone())
            .unwrap_or_default(),
        checked_blocks,
        checked_rollback_blocks,
        checked_sealed_rollback_patches,
        persisted_wal_patches,
        checked_refs,
        checked_ref_log_records,
        ref_publication_issues,
        pointer_outcomes,
        log_outcomes,
        ref_item_outcomes,
        signature_envelope_issues,
        checked_rollback_draft_records,
        checked_publication_trust_records,
        publication_trust_issues: trust_verifier.issues,
        object_temp_paths,
        trailing_partial_wal_bytes: replay.as_ref().map(|replay| replay.trailing_partial_bytes),
        active_wal_metadata_status,
        commit_index_divergences,
        lifecycle_cache_divergences,
        active_wal_ordering_issues,
        merge_baseline_divergences,
        block_seals,
        received_ref_item_outcomes,
        active_session_count,
    })
}

/// Aggregate counts derived from Phase A's per-item outcomes (DC-95 Stage 2 Level 2). Each field
/// counts only `Evaluated` entries -- a `Failed` object contributes to none of them, same as it never
/// contributed to the pre-Level-2 running totals a whole-stage failure would have zeroed out entirely.
struct PhaseACounts {
    objects: usize,
    blocks: usize,
    rollback_blocks: usize,
    rollback_patches: usize,
}

fn phase_a_counts(object_outcomes: &[ObjectItemOutcome]) -> Result<PhaseACounts> {
    let mut objects = 0_usize;
    let mut blocks = 0_usize;
    let mut rollback_blocks = 0_usize;
    let mut rollback_patches = 0_usize;
    for outcome in object_outcomes {
        // RFC 102 Stage 3: an `Unindexed` object is just as real and sound as an `Evaluated` one --
        // design-v1.md §12/§10.2's ruling that it is not a failure means it belongs in every count a
        // healthy object contributes to, not only in `has_item_failure()`'s exclusion.
        let verification = match &outcome.status {
            ObjectItemStatus::Evaluated(verification)
            | ObjectItemStatus::Unindexed(verification) => verification,
            ObjectItemStatus::Failed { .. } => continue,
        };
        objects = objects.checked_add(1).ok_or_else(|| {
            PrikkError::Integrity("object verification count overflow".to_string())
        })?;
        if verification.object_type == ObjectType::Block {
            blocks = blocks.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("block verification count overflow".to_string())
            })?;
            if verification.rollback_patch_count != 0 {
                rollback_blocks = rollback_blocks.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity("rollback block count overflow".to_string())
                })?;
                rollback_patches = rollback_patches
                    .checked_add(verification.rollback_patch_count)
                    .ok_or_else(|| {
                        PrikkError::Integrity("rollback patch count overflow".to_string())
                    })?;
            }
        }
    }
    Ok(PhaseACounts {
        objects,
        blocks,
        rollback_blocks,
        rollback_patches,
    })
}

/// Check that active WAL record sequences strictly increase in replay (append) order. Reachable only
/// under direct file tampering — `Wal::append_patch` always assigns `previous.seq + 1` — but a queue
/// of N gives "ordering" its own meaning worth verifying explicitly (RFC criterion 6), not merely
/// assumed from successful structural decode.
fn check_active_wal_ordering(records: &[crate::wal::WalRecord]) -> Vec<ActiveWalOrderingIssue> {
    records
        .iter()
        .zip(records.iter().skip(1))
        .enumerate()
        .filter(|(_, (previous, current))| current.seq <= previous.seq)
        .map(|(index, (previous, current))| ActiveWalOrderingIssue {
            index: index + 1,
            previous_seq: previous.seq,
            seq: current.seq,
        })
        .collect()
}

/// RFC 115 Stage 3 §6: the received-namespace verification gap, closed. `verify_repository` never
/// scanned `remotes/*` before this stage — `ReceivedIndex` appeared nowhere in this file or in
/// `refs/verify/scan.rs`, so a received ref whose target object was never shipped dangled
/// invisibly, on both the sender's and receiver's side, discovered while reviewing the DC-78
/// bundle-export tag-ref gap (`DC-78-bundle-tag-gap-implementation-review-v1.md` §5).
///
/// Reuses `ensure_ref_target_valid` as-is — the exact kind-aware, two-hop-for-tags check local refs
/// already get (`refs/verify/scan.rs`) — applied here for the first time to the received namespace.
/// This is wiring, not new logic: no new validation rule is introduced, only a new place the
/// existing one now runs.
fn verify_received_refs(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
) -> Result<Vec<RefItemOutcome>> {
    let pointers = list_received_pointers(layout)?;
    let mut outcomes = Vec::with_capacity(pointers.len());
    for pointer in pointers {
        let outcome = (|| -> Result<()> {
            let envelope = object_store
                .read_typed(pointer.ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!(
                        "received pointer {} names missing RefState {}",
                        pointer.ref_name, pointer.ref_state_id
                    ))
                })?;
            let payload = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            ensure_ref_target_valid(
                object_store,
                payload.kind,
                payload.target_object_id,
                pointer.ref_state_id,
            )
        })();
        outcomes.push(RefItemOutcome {
            ref_name: pointer.ref_name,
            status: match outcome {
                Ok(()) => RefItemStatus::Evaluated,
                Err(error) => RefItemStatus::Failed {
                    message: error.to_string(),
                },
            },
        });
    }
    Ok(outcomes)
}

/// DC-78 verify-local-tag-publication-trust (v2 amendment, ruling on
/// `verify-local-tag-publication-trust-escalation-v1.md`): a locally-published Tag's own MAINTAINER
/// signature gets the same publication-trust expectation `Block`/`RefState`/`RefUpdate` already carry
/// -- `053e442` gates both `prikk tag create` and `sync adopt-tag` on this same trust policy, so a
/// local tag's trust is re-derivable offline, exactly what `verify` exists to do.
///
/// Deliberately **not** wired through `verify_refs`: the escalation this amends found `verify_refs` has
/// a caller `ensure_ref_target_valid`'s own four-caller sweep never named --
/// `ensure_no_incomplete_publication`, a pre-mutation structural guard reached from eight sites
/// (`add_trusted_maintainer`, `seal_from_accepted`, `ActiveLock::acquire`, rollback draft, worktree
/// commit authoring, `doctor`), none of them `prikk verify`. Threading a trust verifier through
/// `verify_refs` would have put a trust-policy read and an Ed25519 verification per local tag on all
/// eight. This function enumerates independently instead, exactly like `verify_received_refs` above
/// sits alongside `Refs` rather than reading its output.
///
/// `RefPointerSummary` does not itself carry `RefKind` -- each pointer's `RefState` is read and decoded
/// here to find out, a second, independent read of the same envelope `ensure_ref_target_valid` already
/// reads (and discards) inside the ordinary ref scan. The ruling accepted this as the cost of not
/// contaminating a shared function: "`verify` is an audit, tags are few."
///
/// A received (not-yet-adopted) tag is never reached here: `list_ref_pointers` enumerates only the
/// local pointer index (`refs/by-id`), never the received namespace (`remotes/*`) -- the provenance
/// principle (a Tag's trust expectation follows *how it arrived*, not its type) holds structurally,
/// not by a flag threaded through a shared check.
fn verify_local_tag_publication_trust(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    trust_verifier: &mut PublicationTrustVerifier<'_>,
) -> Result<()> {
    let ref_store = RefStore::new(layout.clone());
    for summary in ref_store.list_ref_pointers()? {
        let ref_state_envelope = object_store
            .read_typed(summary.ref_state_id, ObjectType::RefState)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "ref {} names missing RefState {}",
                    summary.ref_name, summary.ref_state_id
                ))
            })?;
        let ref_state_payload = RefStatePayload::decode_canonical(
            &ref_state_envelope.canonical_payload,
            ref_state_envelope.schema_version,
        )?;
        if ref_state_payload.kind != RefKind::Tag {
            continue;
        }
        let tag_envelope = object_store
            .read_typed(ref_state_payload.target_object_id, ObjectType::Tag)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "ref {} targets missing tag {}",
                    summary.ref_name, ref_state_payload.target_object_id
                ))
            })?;
        trust_verifier.verify(&tag_envelope)?;
    }
    Ok(())
}

/// RFC 108 increment 3c: widened from `default`-only to `name: impl AsRef<Path>`, matching every
/// other `active_*` accessor's own generalization, so `doctor.rs`'s per-active reporting can reuse
/// this exact classification for a non-default active rather than re-deriving the same six-arm match
/// a second time. `pub(crate)` (was private) for that one new caller; the sole existing call site
/// below still passes `DEFAULT_ACTIVE_NAME`, so `default`'s own classification is byte-for-byte
/// unchanged.
pub(crate) fn classify_active_wal_metadata(
    layout: &RepositoryLayout,
    name: impl AsRef<std::path::Path>,
    wal_is_empty: bool,
) -> Result<ActiveWalMetadataStatus> {
    match (
        wal_is_empty,
        crate::commit_boundary::active::read_active_ref_metadata_for(layout, name)?,
    ) {
        (true, ActiveRefMetadata::Missing) => Ok(ActiveWalMetadataStatus::MissingForEmptyWal),
        (true, ActiveRefMetadata::Valid(ref_name)) => {
            Ok(ActiveWalMetadataStatus::ValidForEmptyWal { ref_name })
        }
        (true, ActiveRefMetadata::Invalid(reason)) => {
            Ok(ActiveWalMetadataStatus::InvalidForEmptyWal { reason })
        }
        (false, ActiveRefMetadata::Missing) => Ok(ActiveWalMetadataStatus::MissingForNonEmptyWal),
        (false, ActiveRefMetadata::Valid(ref_name)) => {
            Ok(ActiveWalMetadataStatus::ValidForNonEmptyWal { ref_name })
        }
        (false, ActiveRefMetadata::Invalid(reason)) => {
            Ok(ActiveWalMetadataStatus::InvalidForNonEmptyWal { reason })
        }
    }
}

/// Phase A (DC-92 §4.2): every check that does not depend on lineage-state derivation order —
/// existence of referenced objects, rollback-patch counting, and (independent of the shared memo)
/// the merge-baseline re-derivation. A `CurrentV6` block's own state-root verification is
/// deliberately **not** done here; it is deferred to a batch, dependency-ordered pass
/// (`crate::block_state::verify_blocks_topological`) run once after every object type has been
/// scanned, so `pending_v3_blocks` collects this block's already-decoded payload rather than
/// discarding it. See that function's own doc for why deferring is what bounds
/// `LineageStateMemo`'s memory instead of merely avoiding redundant re-derivation.
fn verify_block_payload(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
    format: RepositoryFormat,
    canonical_payload: &[u8],
    pending_v3_blocks: &mut Vec<(ObjectId, BlockPayload)>,
) -> Result<(usize, Option<MergeBaselineDivergence>)> {
    let payload = BlockPayload::decode_canonical(canonical_payload)?;
    for parent in &payload.parent_block_ids {
        ensure_object_exists(
            object_store,
            ObjectType::Block,
            *parent,
            "parent block",
            block_id,
        )?;
    }
    let mut rollback_patch_count = 0_usize;
    for patch in &payload.patch_ids {
        let Some(envelope) = object_store.read_typed(*patch, ObjectType::Patch)? else {
            return Err(PrikkError::Integrity(format!(
                "object {block_id} references missing block patch {patch}"
            )));
        };
        let context = format!("sealed Block {block_id} Patch {patch}");
        if verify_rollback_patch_envelope(&envelope, &context)? {
            rollback_patch_count = rollback_patch_count.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("sealed rollback patch count overflow".to_string())
            })?;
        }
    }
    if let Some(snapshot) = payload.snapshot_blob_ref {
        ensure_object_exists(
            object_store,
            ObjectType::Blob,
            snapshot,
            "snapshot blob",
            block_id,
        )?;
    }
    let merge_baseline_divergence = if format == RepositoryFormat::CurrentV6 {
        verify_merge_baseline(object_store, block_id, &payload)?
    } else {
        None
    };
    if format == RepositoryFormat::CurrentV6 {
        pending_v3_blocks.push((block_id, payload));
    }
    Ok((rollback_patch_count, merge_baseline_divergence))
}

/// DC-75: for a `Merge` block, independently re-derive whether the recorded
/// `merge_baseline_block_id` is a common ancestor of both parents — a claim, not trusted. Shape
/// (kind, parent count, mainline/baseline presence) is already guaranteed by
/// `verify_block_v2_state`'s `validate_block_v2_shape` call above, so this only checks the claim's
/// content. Cost is the same full-parent reachability walk measured linear in
/// `baseline-recording-answer-v1.md` §1 — unconditional, not a gated "deep verify" mode.
fn verify_merge_baseline(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
    payload: &BlockPayload,
) -> Result<Option<MergeBaselineDivergence>> {
    if payload.kind != prikk_object::BlockKind::Merge {
        return Ok(None);
    }
    let (Some(mainline_parent_id), Some(recorded_baseline)) =
        (payload.mainline_parent_id, payload.merge_baseline_block_id)
    else {
        // Malformed shape already failed closed above via `validate_block_v2_shape`.
        return Ok(None);
    };
    let Some(&secondary_parent_id) = payload
        .parent_block_ids
        .iter()
        .find(|&&id| id != mainline_parent_id)
    else {
        return Ok(None);
    };
    let mainline_ancestors =
        crate::merge::evidence::ancestors_inclusive(object_store, mainline_parent_id)?;
    let secondary_ancestors =
        crate::merge::evidence::ancestors_inclusive(object_store, secondary_parent_id)?;
    let is_common_ancestor = mainline_ancestors.contains_key(&recorded_baseline)
        && secondary_ancestors.contains_key(&recorded_baseline);
    if is_common_ancestor {
        Ok(None)
    } else {
        Ok(Some(MergeBaselineDivergence {
            block_id,
            recorded_baseline,
            mainline_parent_id,
            secondary_parent_id,
        }))
    }
}

fn ensure_object_exists(
    object_store: &impl ObjectReader,
    object_type: ObjectType,
    object_id: ObjectId,
    role: &str,
    owner: ObjectId,
) -> Result<()> {
    let exists = object_store.read_typed(object_id, object_type)?.is_some();
    if exists {
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "object {owner} references missing {role} {object_id}"
    )))
}

fn verify_wal_persistence(
    object_store: &impl ObjectReader,
    records: &[crate::WalRecord],
) -> Result<usize> {
    let mut persisted = 0_usize;
    for record in records {
        if record.envelope.object_type != ObjectType::Patch {
            return Err(PrikkError::Integrity(format!(
                "active WAL record {} contains {}, expected patch",
                record.seq, record.envelope.object_type
            )));
        }
        // `FileObjectStore::contains_object`'s exact existing semantics, reproduced generically:
        // "does this exist, as this type" -- and, matching its own silent-on-error tolerance, any
        // read error here means "not found," not a propagated failure. `contains_object` itself is
        // inherent, not on `ObjectReader`, so a generic reader uses `read_object` directly instead.
        if object_store
            .read_object(record.envelope.object_id())
            .ok()
            .flatten()
            .is_some_and(|envelope| envelope.object_type == ObjectType::Patch)
        {
            persisted = persisted.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("persisted WAL patch count overflow".to_string())
            })?;
        }
    }
    Ok(persisted)
}

#[cfg(test)]
mod tests;
