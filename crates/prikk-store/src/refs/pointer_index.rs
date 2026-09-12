//! Ref-pointer index: append-only, rebuildable, off the durability path (RFC 102 Stage 4, Step 0
//! §13.4, ruled in design-v1.md §13.4). Same *pattern* as the object index (`index.rs`) -- append-only,
//! last-entry-wins lookup, rebuildable by scan -- but its own type and its own container: the ruling
//! is explicit that `index.rs`'s already-shipped object-index schema is never widened for this.
//!
//! One framed record per entry: `ref_name_key` (`sha256(ref_name)`, `layout::ref_name_key_bytes` --
//! the ruling's own point: this key already exists, naming every ref pointer/log file on disk today,
//! and did not need inventing), the human-readable `ref_name` itself, and the RefState id it names.
//! **`ref_name` is carried here even though `ref_name_key` alone would suffice for lookup** --
//! preserving `refs/pointer.rs`'s own existing coherence check (`read_one_pointer`'s `payload.
//! ref_name != pointer.ref_name`, `refs/verify/scan.rs`): the pointer's own claimed name must agree
//! with the RefState object's claimed name, an independent cross-check this entry would silently
//! lose if it carried only the hash. **No `update_seq` field** -- it would duplicate the RefState
//! object's own `RefStatePayload.update_seq`, which every consumer already cross-validates against
//! (`refs/verify.rs`'s `PointerState.payload`), so a second, independently-writable copy here would
//! only be a place for the two to silently disagree, not a fact anything reads.
//!
//! **This is the crash-safety-critical half of publication.** A single durable append here *is* the
//! publish (Step 0 §13.3: "an append-only record has no candidate value to stage -- the append is the
//! publish"), appended **before** the corresponding ref-log container record (`refs/container.rs`) --
//! unchanged from today's pointer-first ordering (Step 0's own "must not change" list), just backed by
//! a container append instead of a candidate-write-then-promote file dance. `refs/tmp/`'s candidate
//! mechanism has no equivalent here because there is nothing left for it to stage.

use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;
use prikk_object::ObjectId;

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{push_bytes_u64, push_u16};
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{append_file_required, len_to_u64, read_file_if_exists};
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::RepositoryLayout;

const POINTER_INDEX_MAGIC: &[u8; 8] = b"PREFPTI1";
const POINTER_INDEX_VERSION: u16 = 1;
const POINTER_INDEX_HEADER_LEN: usize = 8 + 2 + 8 + 32;

/// One ref-pointer-index entry: the published RefState id for one `ref_name_key`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PointerIndexEntry {
    pub(crate) ref_name_key: [u8; 32],
    pub(crate) ref_name: String,
    pub(crate) ref_state_id: ObjectId,
}

/// Outcome of attempting to decode one pointer-index record frame. Mirrors `index::IndexRecordStatus`.
///
/// RFC 131 §3/§5: `pub(in crate::refs)`, not `pub(crate)`, on this and the two structs below --
/// `pointer_index` is already a private submodule of `refs`, so this changes no real reach; it
/// makes that already-true fact explicit. `PointerIndexEntry` itself stays `pub(crate)`: `compact`
/// constructs and reads it directly, a real crate-wide need these three do not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::refs) enum PointerIndexRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::refs) struct PointerIndexRecordOutcome {
    pub(in crate::refs) offset: usize,
    pub(in crate::refs) status: PointerIndexRecordStatus,
}

/// The type itself must stay `pub(crate)`: `replay_pointer_index` (a `pub(crate)` fn) returns it,
/// and Rust requires a function's return type to be at least as visible as the function -- a type
/// cannot be narrower than the least-restrictive signature that names it, regardless of whether
/// every caller ever writes the type name explicitly (found by trying the narrower form first and
/// letting the compiler reject it, not assumed). `trailing_partial_bytes`/`record_outcomes` still
/// narrow safely: `compact::compact_ref_pointer_index`, the sole external caller, reads only
/// `entries` and calls `has_item_failure`, never these two.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PointerIndexReplay {
    pub(crate) entries: Vec<PointerIndexEntry>,
    pub(in crate::refs) trailing_partial_bytes: usize,
    pub(in crate::refs) record_outcomes: Vec<PointerIndexRecordOutcome>,
}

impl PointerIndexReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, PointerIndexRecordStatus::Failed { .. }))
    }
}

fn encode_entry_body(entry: &PointerIndexEntry) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    body.extend_from_slice(&entry.ref_name_key);
    push_bytes_u64(&mut body, entry.ref_name.as_bytes())?;
    body.extend_from_slice(entry.ref_state_id.as_bytes());
    Ok(body)
}

fn decode_entry_body(body: &[u8]) -> Result<PointerIndexEntry> {
    let mut cursor = ByteCursor::new(body);
    let ref_name_key = cursor.read_array::<32>()?;
    let ref_name_bytes = cursor.read_bytes_u64()?;
    let ref_name = String::from_utf8(ref_name_bytes)
        .map_err(|err| PrikkError::MalformedData(format!("invalid ref name utf-8: {err}")))?;
    let ref_state_id = ObjectId::from_bytes(cursor.read_array::<32>()?);
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in pointer index entry body".to_string(),
        ));
    }
    Ok(PointerIndexEntry {
        ref_name_key,
        ref_name,
        ref_state_id,
    })
}

/// Encode one pointer-index record: length-prefixed body plus checksum, the exact bytes the shared
/// index container appends.
pub fn encode_pointer_index_record(entry: &PointerIndexEntry) -> Result<Vec<u8>> {
    let body = encode_entry_body(entry)?;
    let body_len = len_to_u64(body.len())?;
    let checksum = record_checksum(body_len, &body);
    let mut out = Vec::with_capacity(POINTER_INDEX_HEADER_LEN + body.len());
    out.extend_from_slice(POINTER_INDEX_MAGIC);
    push_u16(&mut out, POINTER_INDEX_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    Ok(out)
}

fn record_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(POINTER_INDEX_MAGIC);
    preimage.extend_from_slice(&POINTER_INDEX_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    sha256(&preimage)
}

struct PointerIndexHeader {
    body_len: u64,
    checksum: [u8; 32],
}

fn parse_header(header: &[u8]) -> Result<PointerIndexHeader> {
    let mut cursor = ByteCursor::new(header);
    let magic = cursor.read_array::<8>()?;
    if &magic != POINTER_INDEX_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid pointer index record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != POINTER_INDEX_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in pointer index header".to_string(),
        ));
    }
    Ok(PointerIndexHeader { body_len, checksum })
}

enum FrameAttempt {
    Record {
        entry: PointerIndexEntry,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
    },
}

fn parse_frame_at(bytes: &[u8], offset: usize) -> FrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < POINTER_INDEX_HEADER_LEN {
        return FrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + POINTER_INDEX_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_header(header) {
        Ok(values) => values,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "pointer index body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "pointer index body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let expected = record_checksum(header_values.body_len, body);
    if expected != header_values.checksum {
        return FrameAttempt::Invalid {
            message: format!("pointer index checksum mismatch at byte offset {offset}"),
        };
    }
    match decode_entry_body(body) {
        Ok(entry) => FrameAttempt::Record {
            entry,
            next_offset: body_end,
        },
        Err(err) => FrameAttempt::Invalid {
            message: err.to_string(),
        },
    }
}

/// Isolate-and-continue reading, matching every other reader in the codebase (`frame_resync::
/// resync_to_next_magic`, not re-derived): a damaged pointer-index entry is named at its own offset
/// and the scan continues past it.
pub(crate) fn decode_pointer_index_records(bytes: &[u8]) -> Result<PointerIndexReplay> {
    let mut entries = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    loop {
        match parse_frame_at(bytes, offset) {
            FrameAttempt::Record { entry, next_offset } => {
                record_outcomes.push(PointerIndexRecordOutcome {
                    offset,
                    status: PointerIndexRecordStatus::Evaluated,
                });
                entries.push(entry);
                offset = next_offset;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                return Ok(PointerIndexReplay {
                    entries,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            FrameAttempt::Invalid { message } => {
                record_outcomes.push(PointerIndexRecordOutcome {
                    offset,
                    status: PointerIndexRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, POINTER_INDEX_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(PointerIndexReplay {
                            entries,
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                }
            }
        }
    }
}

/// Read and replay the on-disk pointer index, off the durability path -- a missing file replays as
/// empty, the same reader-equivalence rule Stage 1 established for the WAL. Generation-aware (RFC 102
/// Stage 6 Step 1, design-v1.md §15.6): resolves to `A` today, since nothing has ever appended a
/// generation record -- Step 2's compactor is what will ever make this resolve to `B`.
pub(crate) fn replay_pointer_index(layout: &RepositoryLayout) -> Result<PointerIndexReplay> {
    let slot = resolve_live_slot(layout, &layout.ref_pointer_index_generation_log_path())?;
    let relative = layout.repository_relative(&layout.ref_pointer_index_slot_path(slot))?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(PointerIndexReplay {
            entries: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_pointer_index_records(&bytes)
}

/// Look up one ref's current published pointer: the last entry matching `ref_name_key`, matching
/// `index::lookup_object_location`'s own "last entry wins" reverse search exactly. Refuses if the
/// index itself has a damaged entry, rather than silently searching around it.
pub(in crate::refs) fn lookup_ref_pointer(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
) -> Result<Option<PointerIndexEntry>> {
    let replay = replay_pointer_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .entries
        .into_iter()
        .rev()
        .find(|entry| entry.ref_name_key == ref_name_key))
}

/// Test-only convenience matching the retired `refs/pointer.rs::write_ref_pointer_candidate`'s own
/// 3-argument call shape exactly, for fixtures that need to plant a specific pointer state directly
/// without going through a real publish. Computes `ref_name_key` itself.
///
/// `cfg(any(test, feature = "test-support"))`, not just `cfg(test)`: the `test-support` half exists
/// solely so `force_ref_pointer_to_arbitrary_state_for_test_support` below (design-v1.md §13.10) has
/// something to call -- same reasoning as `remove_pointer_entries_for_test`'s own doc.
#[cfg(any(test, feature = "test-support"))]
pub fn write_ref_pointer_candidate_for_test(
    layout: &RepositoryLayout,
    ref_name: &str,
    ref_state_id: ObjectId,
) -> Result<()> {
    append_ref_pointer_entry(
        layout,
        &PointerIndexEntry {
            ref_name_key: crate::foundation::layout::ref_name_key_bytes(ref_name),
            ref_name: ref_name.to_string(),
            ref_state_id,
        },
    )
}

/// Test-only: like `write_ref_pointer_candidate_for_test`, but takes `ref_name_key` explicitly
/// instead of deriving it from `ref_name` -- for fixtures that need the two to disagree
/// (`read_one_pointer_entry`'s own coherence check, design-v1.md §13.12).
#[cfg(any(test, feature = "test-support"))]
pub fn write_ref_pointer_entry_with_explicit_key_for_test(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    ref_name: &str,
    ref_state_id: ObjectId,
) -> Result<()> {
    append_ref_pointer_entry(
        layout,
        &PointerIndexEntry {
            ref_name_key,
            ref_name: ref_name.to_string(),
            ref_state_id,
        },
    )
}

/// Durably append one new pointer entry -- the publish moment itself (see module doc). Never checks
/// for an existing entry first: unlike the object index's same-id-different-bytes idempotency guard
/// (`write_object_to_container`), a duplicate pointer entry is not wasteful the way a duplicate
/// signed object would be (this record is small, though not fixed-width -- `ref_name` varies), and
/// "last entry wins" already makes a benign duplicate harmless -- so the caller's own CAS check
/// (`expected_previous_ref_state_id` against the current lookup) is what refuses a genuine conflict,
/// not this function.
pub(in crate::refs) fn append_ref_pointer_entry(
    layout: &RepositoryLayout,
    entry: &PointerIndexEntry,
) -> Result<()> {
    let record = encode_pointer_index_record(entry)?;
    // RFC 102 Stage 6 Step 2, design-v1.md §15.7/§15.9: resolver-routed, not hardcoded to `A` --
    // Step 1's own hardcoding was correct only because nothing could ever make `B` live yet. Once
    // Step 2's compactor exists, a writer still appending to `A` after compaction published `B`
    // would be a silently lost write (the review's own finding: "the reader resolves `B`, the record
    // went to `A`, and nothing reports it"). Safe against the compactor racing this exact
    // resolve-then-append sequence only because every caller already holds this container's lock
    // (`acquire_container_locks`) for the whole critical section -- the compactor cannot run
    // concurrently, so "resolve, then write to what was just resolved" cannot go stale mid-sequence.
    let slot = resolve_live_slot(layout, &layout.ref_pointer_index_generation_log_path())?;
    let relative = layout.repository_relative(&layout.ref_pointer_index_slot_path(slot))?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
}

/// Splice out every entry matching `ref_name_key` -- the container-native way to simulate "this
/// ref's pointer is genuinely missing" under an append-only model, mirroring `index.rs`'s own
/// `remove_index_entry_for_test` (RFC 102 Stage 3) exactly in spirit. Entries here are
/// variable-width (`ref_name`), so each record's own span is derived from consecutive outcome
/// offsets rather than a fixed frame length.
///
/// `cfg(any(test, feature = "test-support"))`, not just `cfg(test)`: the `test-support` half exists
/// solely so `remove_ref_pointer_entry_for_test_support` below (the genuinely `pub`,
/// cross-crate-visible wrapper, design-v1.md §13.9) has something to call. `#[cfg(test)]` alone would
/// never be active when this crate is compiled as a normal dependency of another crate's integration
/// tests -- see that function's own doc for why a `pub` method was rejected in favor of this feature.
#[cfg(any(test, feature = "test-support"))]
pub fn remove_pointer_entries_for_test(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
) -> Result<()> {
    // RFC 102 Stage 6 Step 2 review's residual note: hardcoding `A` here would silently splice the
    // *retired* slot after a compaction, passing for the wrong reason (or failing confusingly) rather
    // than acting on the live slot every real reader resolves. Resolver-routed, matching every
    // production writer this stage just fixed for the identical reason.
    let slot = resolve_live_slot(layout, &layout.ref_pointer_index_generation_log_path())?;
    let path = layout.ref_pointer_index_slot_path(slot);
    let bytes = std::fs::read(&path)?;
    let replay = decode_pointer_index_records(&bytes)?;
    let mut entries = replay.entries.iter();
    let mut retained = Vec::new();
    for (index, outcome) in replay.record_outcomes.iter().enumerate() {
        let end = replay
            .record_outcomes
            .get(index + 1)
            .map_or(bytes.len() - replay.trailing_partial_bytes, |next| {
                next.offset
            });
        let span = bytes.get(outcome.offset..end).unwrap_or_default();
        match &outcome.status {
            PointerIndexRecordStatus::Evaluated => {
                let Some(entry) = entries.next() else {
                    return Err(PrikkError::Integrity(
                        "pointer index replay outcome/entry count mismatch".to_string(),
                    ));
                };
                if entry.ref_name_key == ref_name_key {
                    continue;
                }
            }
            PointerIndexRecordStatus::Failed { .. } => {}
        }
        retained.extend_from_slice(span);
    }
    retained.extend_from_slice(
        bytes
            .get(bytes.len() - replay.trailing_partial_bytes..)
            .unwrap_or_default(),
    );
    std::fs::write(&path, retained)?;
    Ok(())
}

/// Drop one ref's pointer entry, simulating the state left by a crash between "pointer promoted" and
/// "before the next read" -- for use by another crate's own integration tests only (design-v1.md
/// §13.9). Gated by the non-default `test-support` feature, never `#[cfg(test)]`: an integration test
/// in a dependent crate compiles this crate as a normal (non-test) dependency, so a `#[cfg(test)]`
/// item would never be visible to it regardless of feature flags.
///
/// **Deliberately not exposed without the feature.** This function's only purpose is to damage a
/// repository -- a genuinely `pub`, always-available method with that purpose has no place in a
/// product whose entire claim is that history is not silently lost. Nothing outside `#[cfg(test)]` or
/// this feature-gated module may call it, checked by review each time this feature's surface changes.
#[cfg(feature = "test-support")]
pub fn remove_ref_pointer_entry_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<()> {
    remove_pointer_entries_for_test(
        layout,
        crate::foundation::layout::ref_name_key_bytes(ref_name),
    )
}

/// Force-append a pointer entry for `ref_state_id`, bypassing publish's CAS check and the ref log
/// entirely -- for use by another crate's own integration tests only (design-v1.md §13.10). This can
/// point a ref at a **false but validly-shaped** state: any `ObjectId` accepted here, including one
/// that does not name the ref's own current or next real transition, becomes the ref's current
/// pointer the instant this returns. That is a materially different, and more dangerous, hazard than
/// `remove_ref_pointer_entry_for_test_support`'s "make a pointer appear absent": a missing pointer is
/// detectably wrong, a wrong-but-present one is not, until something cross-checks it against the log.
/// The name says so on purpose -- see that function's own doc for why a bare `#[cfg(test)]` cannot
/// reach a dependent crate's integration tests regardless of feature flags, and why this stays behind
/// the same non-default `test-support` feature rather than becoming genuinely `pub`.
#[cfg(feature = "test-support")]
pub fn force_ref_pointer_to_arbitrary_state_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    ref_state_id: ObjectId,
) -> Result<()> {
    write_ref_pointer_candidate_for_test(layout, ref_name, ref_state_id)
}

#[cfg(test)]
mod tests;
