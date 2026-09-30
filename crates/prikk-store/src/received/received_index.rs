//! Received-ref index: append-only, off the durability path in the same sense the ref pointer index
//! is (RFC 102 Stage 5, design-v1.md §14, Step 0 item 2's ruling that `received.rs` belongs on the
//! refs container+pointer-index pattern -- the same unbounded, minted-after-`init` per-name shape
//! that forced Stage 4's ref pointer index, §13.2's argument applied to a second subsystem). Mirrors
//! `refs/pointer_index.rs` deliberately closely: same magic-framed, checksum-verified,
//! resync-on-corruption record shape, and the identical entry fields -- `received.rs`'s own doc
//! already states its semantics are "no CAS and no merge... this is what I have now," which is
//! exactly last-entry-wins, the same publish model the ref pointer index already proved sound.
//!
//! **No separate log.** Unlike refs (Stage 4), a received ref carries no update-sequence history to
//! preserve -- each import simply replaces the prior state outright -- so this one container plays
//! both the pointer-index and the (nonexistent) log's role at once.
//!
//! The I/O layer below landed once the format-bump question (design-v1.md §14.7) was answered: this
//! container name is allocated at `init` (`layout.rs::init`) under repository format 5.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{push_bytes_u64, push_u16};
use crate::foundation::frame_resync::{
    partial_before_sound_frame_message, require_progress, resync_to_next_magic,
    sound_frame_after_partial, tallied_sha256,
};
use crate::foundation::fsutil::{append_file_required, len_to_u64, read_file_if_exists};
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::RepositoryLayout;

const RECEIVED_INDEX_MAGIC: &[u8; 8] = b"PRECVIX1";
const RECEIVED_INDEX_VERSION: u16 = 1;
const RECEIVED_INDEX_HEADER_LEN: usize = 8 + 2 + 8 + 32;

/// One received-ref-index entry: the last-imported `RefState` id for one `ref_name_key`
/// (`layout::ref_name_key_bytes`, the same fixed-width key the ref pointer index uses -- `received.
/// rs`'s existing per-file naming already keys on its hex form, `ref_name_storage_key`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedIndexEntry {
    pub(crate) ref_name_key: [u8; 32],
    pub(crate) ref_name: String,
    pub(crate) ref_state_id: ObjectId,
}

/// Outcome of attempting to decode one received-index record frame. Mirrors `PointerIndexRecordStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ReceivedIndexRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedIndexRecordOutcome {
    pub(crate) offset: usize,
    pub(crate) status: ReceivedIndexRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReceivedIndexReplay {
    pub(crate) entries: Vec<ReceivedIndexEntry>,
    pub(crate) trailing_partial_bytes: usize,
    /// The byte offset where `trailing_partial_bytes` begins (RFC 163 §2's refusal names it). Only
    /// meaningful when `trailing_partial_bytes != 0`.
    pub(crate) tail_offset: usize,
    pub(crate) record_outcomes: Vec<ReceivedIndexRecordOutcome>,
}

impl ReceivedIndexReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, ReceivedIndexRecordStatus::Failed { .. }))
    }
}

fn encode_entry_body(entry: &ReceivedIndexEntry) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    body.extend_from_slice(&entry.ref_name_key);
    push_bytes_u64(&mut body, entry.ref_name.as_bytes())?;
    body.extend_from_slice(entry.ref_state_id.as_bytes());
    Ok(body)
}

fn decode_entry_body(body: &[u8]) -> Result<ReceivedIndexEntry> {
    let mut cursor = ByteCursor::new(body);
    let ref_name_key = cursor.read_array::<32>()?;
    let ref_name_bytes = cursor.read_bytes_u64()?;
    let ref_name = String::from_utf8(ref_name_bytes)
        .map_err(|err| PrikkError::MalformedData(format!("invalid ref name utf-8: {err}")))?;
    let ref_state_id = ObjectId::from_bytes(cursor.read_array::<32>()?);
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in received index entry body".to_string(),
        ));
    }
    Ok(ReceivedIndexEntry {
        ref_name_key,
        ref_name,
        ref_state_id,
    })
}

pub(crate) fn encode_received_index_record(entry: &ReceivedIndexEntry) -> Result<Vec<u8>> {
    let body = encode_entry_body(entry)?;
    let body_len = len_to_u64(body.len())?;
    let checksum = record_checksum(body_len, &body);
    let mut out = Vec::with_capacity(RECEIVED_INDEX_HEADER_LEN + body.len());
    out.extend_from_slice(RECEIVED_INDEX_MAGIC);
    push_u16(&mut out, RECEIVED_INDEX_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    Ok(out)
}

fn record_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(RECEIVED_INDEX_MAGIC);
    preimage.extend_from_slice(&RECEIVED_INDEX_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    tallied_sha256(&preimage)
}

struct ReceivedIndexHeader {
    body_len: u64,
    checksum: [u8; 32],
}

fn parse_header(header: &[u8]) -> Result<ReceivedIndexHeader> {
    let mut cursor = ByteCursor::new(header);
    let magic = cursor.read_array::<8>()?;
    if &magic != RECEIVED_INDEX_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid received index record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != RECEIVED_INDEX_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in received index header".to_string(),
        ));
    }
    Ok(ReceivedIndexHeader { body_len, checksum })
}

enum FrameAttempt {
    Record {
        entry: ReceivedIndexEntry,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
        /// RFC 164 §9: a complete record (full header, full claimed body) whose checksum or
        /// envelope fails is never the harmless remnant of an interrupted append -- excluded from
        /// Rule A's "damage only if a sound record follows" check and stays a failed item
        /// unconditionally. `false` for a header that is not this format's own (bad magic/version)
        /// or a length claim the bytes cannot satisfy, which stay tail-eligible.
        never_a_tail: bool,
    },
}

fn parse_frame_at(bytes: &[u8], offset: usize) -> FrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < RECEIVED_INDEX_HEADER_LEN {
        return FrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + RECEIVED_INDEX_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_header(header) {
        Ok(values) => values,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
                never_a_tail: false,
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "received index body length does not fit usize".to_string(),
            never_a_tail: false,
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "received index body end overflow".to_string(),
            never_a_tail: false,
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let expected = record_checksum(header_values.body_len, body);
    if expected != header_values.checksum {
        // RFC 164 §9: a complete record (full header, full claimed body) whose checksum fails was
        // fully written -- corruption, not a crash mid-write.
        return FrameAttempt::Invalid {
            message: format!("received index checksum mismatch at byte offset {offset}"),
            never_a_tail: true,
        };
    }
    match decode_entry_body(body) {
        Ok(entry) => FrameAttempt::Record {
            entry,
            next_offset: body_end,
        },
        Err(err) => FrameAttempt::Invalid {
            message: err.to_string(),
            never_a_tail: true,
        },
    }
}

/// Isolate-and-continue reading, matching `pointer_index.rs::decode_pointer_index_records` and
/// `frame_resync::resync_to_next_magic` exactly: a damaged received-index entry is named at its own
/// offset and the scan continues past it.
pub(crate) fn decode_received_index_records(bytes: &[u8]) -> Result<ReceivedIndexReplay> {
    let mut entries = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    loop {
        match parse_frame_at(bytes, offset) {
            FrameAttempt::Record { entry, next_offset } => {
                record_outcomes.push(ReceivedIndexRecordOutcome {
                    offset,
                    status: ReceivedIndexRecordStatus::Evaluated,
                });
                entries.push(entry);
                offset = require_progress("received index", offset, next_offset)?;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                // RFC 160 F3: a torn tail is a prefix of ONE frame. If a sound frame starts in the remainder, this is damage.
                let sound_after = sound_frame_after_partial(
                    bytes,
                    offset,
                    RECEIVED_INDEX_MAGIC.as_slice(),
                    |c| matches!(parse_frame_at(bytes, c), FrameAttempt::Record { .. }),
                );
                let Some(next) = sound_after else {
                    return Ok(ReceivedIndexReplay {
                        entries,
                        trailing_partial_bytes: remaining,
                        tail_offset: offset,
                        record_outcomes,
                    });
                };
                let message = partial_before_sound_frame_message(offset, next);
                record_outcomes.push(ReceivedIndexRecordOutcome {
                    offset,
                    status: ReceivedIndexRecordStatus::Failed { message },
                });
                offset = require_progress("received index", offset, next)?;
            }
            FrameAttempt::Invalid {
                message,
                never_a_tail,
            } => {
                // RFC 164 Rule A / §9: an invalid frame is damage only if a sound frame follows it
                // somewhere in the rest of the buffer -- otherwise this frame and everything after
                // it is a tail, whatever its shape (zeros, random bytes), the same rule RFC 162
                // rule 3 already gives the WAL and the pointer index. Except a complete record whose
                // checksum or envelope fails (`never_a_tail`), which stays damage unconditionally.
                let sound_after = (!never_a_tail)
                    .then(|| {
                        sound_frame_after_partial(
                            bytes,
                            offset,
                            RECEIVED_INDEX_MAGIC.as_slice(),
                            |c| matches!(parse_frame_at(bytes, c), FrameAttempt::Record { .. }),
                        )
                    })
                    .flatten();
                if sound_after.is_none() && !never_a_tail {
                    return Ok(ReceivedIndexReplay {
                        entries,
                        trailing_partial_bytes: bytes.len().saturating_sub(offset),
                        tail_offset: offset,
                        record_outcomes,
                    });
                }
                record_outcomes.push(ReceivedIndexRecordOutcome {
                    offset,
                    status: ReceivedIndexRecordStatus::Failed { message },
                });
                let resumed = match sound_after {
                    Some(next) => Some(next),
                    None => {
                        resync_to_next_magic(bytes, offset + 1, RECEIVED_INDEX_MAGIC.as_slice())
                    }
                };
                match resumed {
                    Some(next) => offset = require_progress("received index", offset, next)?,
                    None => {
                        return Ok(ReceivedIndexReplay {
                            entries,
                            trailing_partial_bytes: 0,
                            tail_offset: bytes.len(),
                            record_outcomes,
                        });
                    }
                }
            }
        }
    }
}

/// RFC 163 §2, Addendum 1 item 3: the tail-only walk `require_received_index_clean_tail` uses instead
/// of a full [`decode_received_index_records`]. Same frame-by-frame walk (magic, header, checksum),
/// but never decodes a sound frame's body into a [`ReceivedIndexEntry`] (no `ref_name` UTF-8 parse, no
/// `String`/`Vec` allocation per record) -- the corruption check a tail guard needs is entirely in the
/// header and the checksum; the entry's own fields are never read. Still reads and hashes every byte
/// of the file (the checksum cannot be skipped without losing the ability to tell a sound record from
/// damage), so this is a real but bounded saving, not a change of complexity class -- see the measured
/// costs in `received/tests.rs`.
/// Returns `(trailing_partial_bytes, tail_offset, damaged)`. `damaged` is set the moment any frame
/// decodes as [`FrameAttempt::Invalid`] -- a checksum mismatch or an unparseable body, not a torn
/// tail -- the same interior damage the four sibling files' own `has_item_failure()` already refuses
/// on (external review 016, N9: this walk used to resync past such a frame silently, so a write could
/// append behind it and bury it for good). No new read: the same checksum this walk already computes
/// for every frame is what tells a sound record from damage.
fn scan_received_index_tail(bytes: &[u8]) -> Result<(usize, usize, bool)> {
    let mut offset = 0_usize;
    let mut damaged = false;
    loop {
        match parse_frame_at(bytes, offset) {
            FrameAttempt::Record { next_offset, .. } => {
                offset = require_progress("received index", offset, next_offset)?;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                let sound_after = sound_frame_after_partial(
                    bytes,
                    offset,
                    RECEIVED_INDEX_MAGIC.as_slice(),
                    |c| matches!(parse_frame_at(bytes, c), FrameAttempt::Record { .. }),
                );
                let Some(next) = sound_after else {
                    return Ok((remaining, offset, damaged));
                };
                damaged = true;
                offset = require_progress("received index", offset, next)?;
            }
            FrameAttempt::Invalid { never_a_tail, .. } => {
                // RFC 164 Rule A / §9: see the matching comment in `decode_received_index_records`
                // above. A `never_a_tail` record (a complete record whose checksum or envelope
                // fails) is damage unconditionally, never resolved to a tail even when nothing
                // sound follows.
                let sound_after = (!never_a_tail)
                    .then(|| {
                        sound_frame_after_partial(
                            bytes,
                            offset,
                            RECEIVED_INDEX_MAGIC.as_slice(),
                            |c| matches!(parse_frame_at(bytes, c), FrameAttempt::Record { .. }),
                        )
                    })
                    .flatten();
                if sound_after.is_none() && !never_a_tail {
                    return Ok((bytes.len().saturating_sub(offset), offset, damaged));
                }
                damaged = true;
                let resumed = match sound_after {
                    Some(next) => Some(next),
                    None => {
                        resync_to_next_magic(bytes, offset + 1, RECEIVED_INDEX_MAGIC.as_slice())
                    }
                };
                match resumed {
                    Some(next) => offset = require_progress("received index", offset, next)?,
                    None => return Ok((0, bytes.len(), damaged)),
                }
            }
        }
    }
}

/// RFC 163 §2, Addendum 1 item 1: the received index's own write-side tail guard, decided in the
/// pre-write phase (`bundle.rs::import_bundle`, alongside `check_author_key_conflict`) rather than
/// inside `append_received_index_entry` -- a refused import must write nothing, and by the time the
/// append itself runs, objects and author-key material are already durable (0.44.0, GHSA-px5q-233r-6hq5).
/// Uses [`scan_received_index_tail`], not a full replay: this check does not need any entry's own
/// fields, only whether the file ends at its last sound record.
pub(crate) fn require_received_index_clean_tail(layout: &RepositoryLayout) -> Result<()> {
    let slot = resolve_live_slot(layout, &layout.received_index_generation_log_path())?;
    let relative = layout.repository_relative(&layout.received_index_slot_path(slot))?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(());
    };
    let (trailing_partial_bytes, tail_offset, damaged) = scan_received_index_tail(&bytes)?;
    // External review 016, N9: interior damage (not a torn tail) used to resync past silently here,
    // the only one of RFC 163's guarded writers that did -- the four sibling files already refuse on
    // any damaged entry, via their own `has_item_failure()`. Same message, same "run doctor" advice.
    if damaged {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    crate::foundation::tail_guard::require_no_unclean_tail(
        "the received index",
        trailing_partial_bytes,
        tail_offset,
        "back it up, truncate it to the named offset, then run `prikk verify`",
    )
}

/// Read and replay the on-disk received-ref index, off the durability path -- a missing file replays
/// as empty, the same reader-equivalence rule Stage 1 established for the WAL and Stage 4 for the ref
/// pointer index. Generation-aware (RFC 102 Stage 6 Step 1, design-v1.md §15.6): resolves to `A`
/// today, since nothing has ever appended a generation record.
pub(crate) fn replay_received_index(layout: &RepositoryLayout) -> Result<ReceivedIndexReplay> {
    let slot = resolve_live_slot(layout, &layout.received_index_generation_log_path())?;
    let relative = layout.repository_relative(&layout.received_index_slot_path(slot))?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(ReceivedIndexReplay {
            entries: Vec::new(),
            trailing_partial_bytes: 0,
            tail_offset: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_received_index_records(&bytes)
}

/// Look up one received ref's current pointer: the last entry matching `ref_name_key`, matching
/// `pointer_index::lookup_ref_pointer`'s own "last entry wins" reverse search exactly. Refuses if the
/// index itself has a damaged entry, rather than silently searching around it -- the same fail-closed
/// reasoning applies here as it does there (design-v1.md §13.14): this index is also last-entry-wins,
/// so silently skipping a damaged *latest* entry could let an older entry for the same ref resolve as
/// current.
pub(crate) fn lookup_received_index_entry(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
) -> Result<Option<ReceivedIndexEntry>> {
    let replay = replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .entries
        .into_iter()
        .rev()
        .find(|entry| entry.ref_name_key == ref_name_key))
}

/// Resolve every currently-received ref: the last entry per `ref_name_key`, unsorted (callers sort by
/// whatever key they need). Fails closed on any damaged entry, for the same reason `lookup_received_
/// index_entry` does -- listing is exactly the same last-entry-wins resolution, just for every key
/// instead of one.
pub(crate) fn list_resolved_received_entries(
    layout: &RepositoryLayout,
) -> Result<Vec<ReceivedIndexEntry>> {
    let replay = replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    let mut resolved: Vec<ReceivedIndexEntry> = Vec::new();
    for entry in replay.entries {
        match resolved
            .iter_mut()
            .find(|existing| existing.ref_name_key == entry.ref_name_key)
        {
            Some(existing) => *existing = entry,
            None => resolved.push(entry),
        }
    }
    Ok(resolved)
}

/// Durably append one new received-ref entry -- the import moment itself. Never checks for an
/// existing entry first, matching `append_ref_pointer_entry`'s own reasoning exactly: "last entry
/// wins" already makes a duplicate harmless, and `received.rs`'s own doc is explicit that a re-import
/// has no CAS to enforce -- "this is what I have now."
///
/// **Carries no tail guard of its own** (RFC 163 §2, Addendum 1 item 1): the sole production caller,
/// `bundle.rs::import_bundle`, decides that in its own pre-write phase now
/// (`require_received_index_clean_tail`, under the same received-index lock this append later runs
/// under), so that a refused import writes nothing at all -- not even the objects a check running this
/// late would have already durably written. Called by test fixtures too, which do not need the guard
/// re-run per append the way production code, with its single well-defined entry point, does.
pub(crate) fn append_received_index_entry(
    layout: &RepositoryLayout,
    entry: &ReceivedIndexEntry,
) -> Result<()> {
    let record = encode_received_index_record(entry)?;
    // RFC 102 Stage 6 Step 2, design-v1.md §15.7/§15.9: resolver-routed, not hardcoded to `A` --
    // see `pointer_index::append_ref_pointer_entry`'s identical comment for why, and why this is
    // safe against the compactor despite the resolve-then-append sequence not being atomic on its
    // own (the container lock, held by every caller for its whole critical section, is what closes
    // that gap).
    let slot = resolve_live_slot(layout, &layout.received_index_generation_log_path())?;
    let relative = layout.repository_relative(&layout.received_index_slot_path(slot))?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
}

#[cfg(test)]
mod tests;
