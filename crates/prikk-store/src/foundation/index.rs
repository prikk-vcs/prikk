//! Object index: append-only, rebuildable, off the durability path (RFC 102 Stage 3, design-v1.md
//! §4), and the write protocol that keeps it sound (design §5, handoff §3).
//!
//! One framed record per entry: object id, container (type + slot), offset, length, and the
//! container record's own frame checksum. Framing reuses the same shape as `container.rs` (magic,
//! version, body_len, checksum, body) with its own magic and a fixed-width body -- five fixed-size
//! fields, no length-prefixing needed inside the body itself.
//!
//! **The write protocol is the part no framing can enforce, and is stated here at the call site, not
//! only in the design doc**: [`write_object_to_container`] appends the object record to its
//! container and makes it durable *first*; only then does it append the index entry. A crash between
//! the two leaves an object present and unindexed -- recovered by [`rebuild_index_from_containers`],
//! the safe direction. The reverse order would let a reader see a valid, checksummed index entry
//! pointing at bytes that are not there, which must never happen.
//!
//! **Read validation (design §12/§10.3's ruling)**: an ordinary lookup trusts the index for
//! location -- one index read, one seek into the container, no container scan. The bytes found are
//! then validated by recomputing `ObjectEnvelope::object_id()` from the decoded content, which is
//! free of extra I/O since the object must be decoded anyway; a mismatch is a reported defect, never
//! a silent fallback to scanning. `verify` (a different call path, not this one) is what does the
//! full container scan.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::container::{self, ContainerRecordStatus, container_magic};
use crate::foundation::file_codec::push_u16;
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{
    append_file_required, len_to_u64, read_file_if_exists, write_file_atomically,
};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};
use prikk_hash::sha256;
use std::collections::BTreeMap;

const INDEX_MAGIC: &[u8; 8] = b"PIDXENT1";
const INDEX_VERSION: u16 = 1;
const INDEX_HEADER_LEN: usize = 8 + 2 + 8 + 32;
/// object_id(32) + object_type code(2) + slot(1) + offset(8) + length(8) + container_checksum(32).
const INDEX_BODY_LEN: usize = 32 + 2 + 1 + 8 + 8 + 32;

/// One index entry: where one object's container record lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct IndexEntry {
    pub(crate) object_id: ObjectId,
    pub(crate) object_type: ObjectType,
    pub(crate) slot: ContainerSlot,
    pub(crate) offset: u64,
    pub(crate) length: u64,
    pub(crate) container_checksum: [u8; 32],
}

/// Outcome of attempting to decode one index record frame. Mirrors `container::ContainerRecordStatus`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IndexRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexRecordOutcome {
    pub(crate) offset: usize,
    pub(crate) status: IndexRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IndexReplay {
    pub(crate) entries: Vec<IndexEntry>,
    pub(crate) trailing_partial_bytes: usize,
    pub(crate) record_outcomes: Vec<IndexRecordOutcome>,
}

impl IndexReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, IndexRecordStatus::Failed { .. }))
    }
}

fn slot_code(slot: ContainerSlot) -> u8 {
    match slot {
        ContainerSlot::A => 0,
        ContainerSlot::B => 1,
    }
}

fn slot_from_code(code: u8) -> Result<ContainerSlot> {
    match code {
        0 => Ok(ContainerSlot::A),
        1 => Ok(ContainerSlot::B),
        other => Err(PrikkError::MalformedData(format!(
            "unrecognized container slot code {other}"
        ))),
    }
}

fn encode_entry_body(entry: &IndexEntry) -> Vec<u8> {
    let mut body = Vec::with_capacity(INDEX_BODY_LEN);
    body.extend_from_slice(entry.object_id.as_bytes());
    push_u16(&mut body, entry.object_type.code());
    body.push(slot_code(entry.slot));
    body.extend_from_slice(&entry.offset.to_be_bytes());
    body.extend_from_slice(&entry.length.to_be_bytes());
    body.extend_from_slice(&entry.container_checksum);
    body
}

fn decode_entry_body(body: &[u8]) -> Result<IndexEntry> {
    let mut cursor = ByteCursor::new(body);
    let object_id = ObjectId::from_bytes(cursor.read_array::<32>()?);
    let object_type = ObjectType::from_code(cursor.read_u16()?)?;
    let slot = slot_from_code(cursor.read_array::<1>()?[0])?;
    let offset = cursor.read_u64()?;
    let length = cursor.read_u64()?;
    let container_checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in index entry body".to_string(),
        ));
    }
    Ok(IndexEntry {
        object_id,
        object_type,
        slot,
        offset,
        length,
        container_checksum,
    })
}

pub(crate) fn encode_index_record(entry: &IndexEntry) -> Result<Vec<u8>> {
    let body = encode_entry_body(entry);
    let body_len = len_to_u64(body.len())?;
    let checksum = index_record_checksum(body_len, &body);
    let mut out = Vec::with_capacity(INDEX_HEADER_LEN + body.len());
    out.extend_from_slice(INDEX_MAGIC);
    push_u16(&mut out, INDEX_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    Ok(out)
}

fn index_record_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(INDEX_MAGIC);
    preimage.extend_from_slice(&INDEX_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    sha256(&preimage)
}

struct IndexHeader {
    body_len: u64,
    checksum: [u8; 32],
}

fn parse_index_header(header: &[u8]) -> Result<IndexHeader> {
    let mut cursor = ByteCursor::new(header);
    let magic = cursor.read_array::<8>()?;
    if &magic != INDEX_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid index record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != INDEX_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in index header".to_string(),
        ));
    }
    Ok(IndexHeader { body_len, checksum })
}

enum FrameAttempt {
    Record {
        entry: IndexEntry,
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
    if remaining < INDEX_HEADER_LEN {
        return FrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + INDEX_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_index_header(header) {
        Ok(values) => values,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "index body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "index body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let expected = index_record_checksum(header_values.body_len, body);
    if expected != header_values.checksum {
        return FrameAttempt::Invalid {
            message: format!("index checksum mismatch at byte offset {offset}"),
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

/// Isolate-and-continue reading, matching the WAL/ref-log/container read path exactly (RFC 102
/// Stage 2's reader, reused via `frame_resync::resync_to_next_magic`, not re-derived): a damaged
/// index entry is named at its own offset and the scan continues past it.
///
/// `start_offset` lets a caller resume scanning partway through an already-known-current prefix
/// (RFC 111 §6.1, `ObjectWriteSession`'s tail-decode) rather than always from the beginning --
/// offsets in the returned `IndexRecordOutcome`s and any `checksum mismatch at byte offset` messages
/// stay absolute (true file position), not relative to `start_offset`, since callers report and
/// compare against the real file. Every existing caller passes `0`, unchanged.
pub(crate) fn decode_index_records(bytes: &[u8], start_offset: usize) -> Result<IndexReplay> {
    let mut entries = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = start_offset;
    loop {
        match parse_frame_at(bytes, offset) {
            FrameAttempt::Record { entry, next_offset } => {
                record_outcomes.push(IndexRecordOutcome {
                    offset,
                    status: IndexRecordStatus::Evaluated,
                });
                entries.push(entry);
                offset = next_offset;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                return Ok(IndexReplay {
                    entries,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            FrameAttempt::Invalid { message } => {
                record_outcomes.push(IndexRecordOutcome {
                    offset,
                    status: IndexRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, INDEX_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(IndexReplay {
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

/// Read and replay the on-disk index, off the durability path (design §4) -- a missing file replays
/// as empty, the same reader-equivalence rule Stage 1 established for the WAL.
pub(crate) fn replay_index(layout: &RepositoryLayout) -> Result<IndexReplay> {
    Ok(replay_index_with_extent(layout)?.0)
}

/// Like [`replay_index`], but also returns the byte length actually decoded into `entries` --
/// `bytes.len() - trailing_partial_bytes`, never the raw stat size -- the exact quantity
/// `ObjectWriteSession`'s snapshot needs to stay self-checking without ever mistaking a torn trailing
/// write for "decoded" (RFC 111 §6.1 addendum §3.2). Counted identically to `replay_index` (same
/// call, underneath).
pub(crate) fn replay_index_with_extent(layout: &RepositoryLayout) -> Result<(IndexReplay, u64)> {
    let relative = layout.repository_relative(&layout.container_index_path())?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok((
            IndexReplay {
                entries: Vec::new(),
                trailing_partial_bytes: 0,
                record_outcomes: Vec::new(),
            },
            0,
        ));
    };
    #[cfg(test)]
    record_replay_index_decode_for_test();
    let replay = decode_index_records(&bytes, 0)?;
    let extent = len_to_u64(bytes.len().saturating_sub(replay.trailing_partial_bytes))?;
    Ok((replay, extent))
}

/// Decode only from `start_offset` onward -- the byte length an `ObjectWriteSession`'s snapshot
/// already knows is current. Still reads the whole file (this path only runs when something else grew
/// the index since the snapshot was taken, which is expected to be rare) but decodes only the new
/// portion, reusing `decode_index_records`'s own frame parser entered partway through rather than
/// re-deriving one. Counted identically to `replay_index_with_extent` -- a tail decode is still a real
/// index decode, even if cheaper.
pub(crate) fn replay_index_tail_with_extent(
    layout: &RepositoryLayout,
    start_offset: u64,
) -> Result<(IndexReplay, u64)> {
    let relative = layout.repository_relative(&layout.container_index_path())?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok((
            IndexReplay {
                entries: Vec::new(),
                trailing_partial_bytes: 0,
                record_outcomes: Vec::new(),
            },
            0,
        ));
    };
    #[cfg(test)]
    record_replay_index_decode_for_test();
    let start = usize::try_from(start_offset)
        .map_err(|_| PrikkError::Integrity("index start offset exceeds usize".to_string()))?;
    let replay = decode_index_records(&bytes, start)?;
    let extent = len_to_u64(bytes.len().saturating_sub(replay.trailing_partial_bytes))?;
    Ok((replay, extent))
}

// RFC 111 §7/§8's cost gate: counts how many times this function actually decodes the on-disk index
// (the line above, not the early "missing file" return above it, which decodes nothing). Thread-local,
// matching `fsutil/anchored/failpoints.rs`'s existing counting shape -- so parallel `cargo test`
// threads never see each other's counts.
#[cfg(test)]
std::thread_local! {
    static REPLAY_INDEX_DECODE_COUNT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
fn record_replay_index_decode_for_test() {
    REPLAY_INDEX_DECODE_COUNT.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(crate) fn reset_replay_index_decode_count_for_test() {
    REPLAY_INDEX_DECODE_COUNT.with(|count| count.set(0));
}

#[cfg(test)]
pub(crate) fn replay_index_decode_count_for_test() -> usize {
    REPLAY_INDEX_DECODE_COUNT.with(|count| count.get())
}

/// Look up one object's container location. Trusts the index for location (design §12/§10.3): one
/// index read, then a linear search of its (already-decoded) entries -- no container scan. Refuses
/// if the index itself has a damaged entry, rather than silently searching around it: an index this
/// read depends on being sound is not the same question as which entries in it are damaged.
pub(crate) fn lookup_object_location(
    layout: &RepositoryLayout,
    object_id: ObjectId,
) -> Result<Option<IndexEntry>> {
    let replay = replay_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "object index has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .entries
        .into_iter()
        .rev()
        .find(|entry| entry.object_id == object_id))
}

/// Read exactly the container record `entry` names, decoding at its known offset ("one seek", design
/// §12/§10.3) rather than scanning its container from the start. Does **not** validate the decoded
/// envelope's recomputed id against `entry.object_id` -- callers that need that check (an ordinary
/// object read) do it themselves, since the error message differs by context (a read reports it
/// against the id the caller asked for; the write-time idempotency check above reports it as a
/// same-id-different-bytes conflict).
pub(crate) fn read_object_envelope_at(
    layout: &RepositoryLayout,
    entry: &IndexEntry,
) -> Result<ObjectEnvelope> {
    let container_relative =
        layout.repository_relative(&layout.container_slot_path(entry.object_type, entry.slot))?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &container_relative)?
    else {
        return Err(PrikkError::Integrity(format!(
            "index names container {:?} slot {:?}, which does not exist",
            entry.object_type, entry.slot
        )));
    };
    let offset = usize::try_from(entry.offset)
        .map_err(|_| PrikkError::Integrity("index entry offset exceeds usize".to_string()))?;
    let record = container::decode_container_record_at(entry.object_type, &bytes, offset)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "index entry for {} names an offset past its container's end",
                entry.object_id
            ))
        })?;
    Ok(record.envelope)
}

/// The outcome of deciding what a write should do, given the object's existing index location (if
/// any) -- wherever that location came from: a fresh decode (`FileObjectStore`) or an in-memory
/// snapshot (`ObjectWriteSession`). The decision logic itself does not care which, so it exists once
/// (RFC 111 §6.1 addendum, C2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WriteDecision {
    /// No existing entry names this object id: append it.
    New,
    /// An existing entry already names this exact object (same id, same bytes): no-op.
    AlreadyPresent(ObjectId),
}

/// RFC 102 Stage 3 preserves `publish_immutable_file`'s exact idempotency contract (the loose-file
/// mechanism this replaces): a same-id rewrite is a silent no-op only when its full envelope bytes
/// match what is already stored -- signatures included, which `object_id` itself does not cover. A
/// same-id rewrite with different bytes is an error, not a silent accept, exactly as
/// `compare_existing`'s `bytes != candidate` check already enforced.
pub(crate) fn decide_write_outcome(
    layout: &RepositoryLayout,
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
    existing: Option<&IndexEntry>,
) -> Result<WriteDecision> {
    let object_id = envelope.object_id();
    let Some(existing) = existing else {
        return Ok(WriteDecision::New);
    };
    if existing.object_type != object_type {
        return Err(PrikkError::Integrity(format!(
            "existing index entry for {object_id} has type {}, expected {object_type}",
            existing.object_type
        )));
    }
    let existing_envelope = read_object_envelope_at(layout, existing)?;
    if existing_envelope != *envelope {
        return Err(PrikkError::Integrity(format!(
            "existing container record for {object_id} differs from candidate"
        )));
    }
    Ok(WriteDecision::AlreadyPresent(object_id))
}

/// The write protocol (design §5, handoff §3): append the object record to its container and make
/// it durable, **then and only then** append the index entry. Never `atomic_replace` -- both appends
/// go through `append_file_required` (`durable_append`), matching every other durability-bearing
/// write in this codebase. Unconditional -- no idempotency check here; callers decide via
/// [`decide_write_outcome`] first and only reach this on [`WriteDecision::New`].
///
/// **Deliberately does not report the resulting index extent** (RFC 111 Stage 1 review v1, B1): a stat
/// taken here immediately after this call's own append cannot distinguish "just this record" from
/// "this record plus a concurrent writer's" -- both make the file longer, and a caller that trusted
/// this return value to set its own `known_length` could set it *past* the extent it has actually
/// decoded, which is `known_length`'s one invariant to never violate (a later genuine append could
/// then make a stat coincidentally match, and a stale idempotency decision would pass the freshness
/// check that exists to catch exactly that). The caller re-derives ground truth the same way every
/// other freshness check does -- by calling `IndexSnapshot::ensure_current` again, whose own
/// tail-decode is frame-aligned by construction and picks up this write, any concurrent one, or both.
pub(crate) fn append_object_to_container(
    layout: &RepositoryLayout,
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
) -> Result<IndexEntry> {
    let object_id = envelope.object_id();
    let record_bytes = container::encode_container_record(object_type, envelope)?;
    let container_relative =
        layout.repository_relative(&layout.container_slot_path(object_type, ContainerSlot::A))?;

    // RFC 102, ruled 2026-09-12: **this function must run under the object-store lock**, which
    // `object_store::append_object_under_lock` -- its only production caller -- holds across the
    // whole call. The exclusive region has to span from the length read just below through the index
    // append at the end of this function, and nothing shorter is correct: `offset` is derived from
    // the container's length *before* the append and recorded in the index *after* it, so two
    // writers that read the same length both record the same offset and one index entry ends up
    // pointing at the other's record.
    //
    // `O_APPEND` (`fsutil/anchored/regular.rs:81`) already makes the container *bytes* safe -- both
    // records land intact, in some order -- which is exactly why the damage showed up as an
    // unreadable index entry rather than a torn container, and why guarding only the write would
    // have fixed nothing.
    //
    // The lock is taken one level up rather than here because `foundation` is the bottom layer and
    // must not depend on `crate::lock` (the coupling gate rejects `foundation -> lock`; it caught
    // this placement on the first attempt). `append_object_under_lock` is the single chokepoint that
    // enforces it, and `every_object_append_goes_through_the_locked_wrapper` is the test that keeps
    // it the only one.
    let existing_len = read_file_if_exists(layout.repository_mutation_root(), &container_relative)?
        .map_or(0, |bytes| bytes.len());
    let offset = len_to_u64(existing_len)?;
    let length = len_to_u64(record_bytes.len())?;
    let container_checksum = frame_checksum(object_type, &record_bytes)?;

    // Step 1: append the object record to its container. Must be durable before step 2 -- a crash
    // here leaves nothing indexed yet, which is not a problem: nothing durable claims this object
    // exists, so there is nothing for a reader to find prematurely.
    append_file_required(
        layout.repository_mutation_root(),
        &container_relative,
        &record_bytes,
    )?;

    // Step 2: only now append the index entry. A crash between step 1 and here leaves the object
    // present-but-unindexed -- the safe direction (design §5): `rebuild_index_from_containers`
    // recovers it. The reverse order is never used, anywhere in this module.
    let entry = IndexEntry {
        object_id,
        object_type,
        slot: ContainerSlot::A,
        offset,
        length,
        container_checksum,
    };
    let index_bytes = encode_index_record(&entry)?;
    let index_relative = layout.repository_relative(&layout.container_index_path())?;
    append_file_required(
        layout.repository_mutation_root(),
        &index_relative,
        &index_bytes,
    )?;

    Ok(entry)
}

/// Extract a just-encoded container record's own frame checksum (the 32 bytes immediately following
/// magic + version + body_len), without re-parsing the whole frame -- `record_bytes` was built by
/// `container::encode_container_record` moments ago, so its shape is already known.
fn frame_checksum(object_type: ObjectType, record_bytes: &[u8]) -> Result<[u8; 32]> {
    let magic = container_magic(object_type)?;
    let checksum_start = magic.len() + 2 + 8;
    let checksum_end = checksum_start + 32;
    let checksum_bytes = record_bytes
        .get(checksum_start..checksum_end)
        .ok_or_else(|| {
            PrikkError::Integrity("just-encoded container record is too short".to_string())
        })?;
    let mut checksum = [0_u8; 32];
    checksum.copy_from_slice(checksum_bytes);
    Ok(checksum)
}

/// Rebuild the index by scanning every container. Not a new operation (design §4): the same
/// per-record content-hash check `verify` already performs, iterated over a scan of every
/// `persisted_object_types()` container's slot A (the only slot Stage 3 ever writes -- rebuilding
/// slot B is Stage 6's concern once compaction can produce one). Returns every sound record found,
/// skipping damaged ones (they are `verify`'s job to report, not rebuild's job to paper over) --
/// callers that need to know about damage should inspect the container replay themselves, not rely
/// on this function's silence about it.
///
/// **Reachable since RFC 102's repair round**: [`repair_index_from_containers`] is the caller, wired
/// to `prikk doctor --repair-index`. This function still only *computes* the rebuilt entries and
/// writes nothing -- the atomic replacement is that function's job, deliberately kept separate so
/// this one stays a pure scan that tests can assert against without touching the index on disk.
pub(crate) fn rebuild_index_from_containers(layout: &RepositoryLayout) -> Result<Vec<IndexEntry>> {
    let mut entries = Vec::new();
    for object_type in persisted_object_types() {
        let relative = layout
            .repository_relative(&layout.container_slot_path(object_type, ContainerSlot::A))?;
        let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
            continue;
        };
        let replay = container::decode_container_records(object_type, &bytes)?;
        // `records` holds only sound frames, in the same order `record_outcomes` visits its
        // `Evaluated` entries -- both are built in lockstep by `decode_container_records`, so
        // advancing this iterator once per `Evaluated` outcome pairs each with its own envelope.
        let mut records = replay.records.iter();
        for outcome in &replay.record_outcomes {
            let ContainerRecordStatus::Evaluated {
                frame_len,
                checksum,
            } = &outcome.status
            else {
                continue;
            };
            let Some(record) = records.next() else {
                return Err(PrikkError::Integrity(
                    "container replay outcome/record count mismatch".to_string(),
                ));
            };
            entries.push(IndexEntry {
                object_id: record.envelope.object_id(),
                object_type,
                slot: ContainerSlot::A,
                offset: len_to_u64(outcome.offset)?,
                length: len_to_u64(*frame_len)?,
                container_checksum: *checksum,
            });
        }
    }
    Ok(entries)
}

/// What one `--repair-index` run changed. Counts, not a narrative: an operator needs to know whether
/// the repair did anything, and a reader of the report needs to tell "nothing was wrong" from
/// "nothing could be done".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexRepairReport {
    /// Sound entries the index held before the repair.
    pub entries_before: usize,
    /// Entries the rebuilt index holds, scanned from the containers themselves.
    pub entries_after: usize,
    /// Objects whose recorded location changed -- the defect's own signature. A non-zero count here
    /// means entries were pointing at the wrong record, which is exactly what concurrent appends
    /// used to produce.
    pub entries_relocated: usize,
    /// Objects the index did not resolve to their own bytes before the repair and does after. This
    /// is the count that answers "did this fix my repository?"
    pub objects_recovered: usize,
    /// True when the rebuilt index is byte-identical to the existing one, so nothing was written.
    pub already_correct: bool,
}

/// Rebuild the object index from the containers and install it atomically.
///
/// RFC 102's repair round: concurrent object appends could record an entry against a stale container
/// length, leaving one entry pointing at another record's bytes -- the containers stayed intact
/// (`O_APPEND`) while the index did not, so every byte needed to rebuild is still on disk. This
/// scans them and replaces the index.
///
/// **Idempotent by construction**: the rebuilt bytes are compared against the existing ones and the
/// write is skipped entirely when they match, so a clean repository is not rewritten and
/// `already_correct` says so. Nothing here touches a container -- repair is an index-only operation,
/// and that is asserted by test, not just intended.
///
/// The install is [`write_file_atomically`]: write a temporary, fsync it, rename over the
/// destination, sync the parent. A crash before the rename leaves the old index whole; after it,
/// the new one. There is no window in which a reader sees a partial index.
pub(crate) fn repair_index_from_containers(layout: &RepositoryLayout) -> Result<IndexRepairReport> {
    let index_relative = layout.repository_relative(&layout.container_index_path())?;
    let existing_bytes = read_file_if_exists(layout.repository_mutation_root(), &index_relative)?
        .unwrap_or_default();
    let before = decode_index_records(&existing_bytes, 0)?.entries;

    let rebuilt = rebuild_index_from_containers(layout)?;

    // "Recovered" is measured against what the *read path* would have done, not against entry
    // counts: an object whose entry pointed at the wrong record was indexed but unreadable, and that
    // is the case an operator is actually asking about.
    let before_locations: BTreeMap<ObjectId, (u64, u64)> = before
        .iter()
        .map(|entry| (entry.object_id, (entry.offset, entry.length)))
        .collect();
    let mut entries_relocated = 0_usize;
    let mut objects_recovered = 0_usize;
    for entry in &rebuilt {
        match before_locations.get(&entry.object_id) {
            Some(&(offset, length)) if offset == entry.offset && length == entry.length => {}
            Some(_) => {
                entries_relocated += 1;
                objects_recovered += 1;
            }
            None => objects_recovered += 1,
        }
    }

    let mut rebuilt_bytes = Vec::with_capacity(rebuilt.len() * (INDEX_HEADER_LEN + INDEX_BODY_LEN));
    for entry in &rebuilt {
        rebuilt_bytes.extend_from_slice(&encode_index_record(entry)?);
    }

    // **Compared as a set, not as bytes.** The rebuild walks `persisted_object_types()` in type
    // order while the live index is in write order, so a perfectly healthy index is a permutation of
    // the rebuilt one and a byte comparison would rewrite it on every run -- the opposite of the
    // idempotence this verb promises. Measured, not predicted: the first version of this function
    // reported "rebuilt (4 -> 4 entries)" on an untouched repository.
    //
    // Order carries no meaning in the index (every reader decodes all records and looks up by id),
    // so equal sets mean equal indexes. Duplicates and stale entries both make the sets differ, which
    // is what should trigger a rewrite.
    let mut existing_sorted = before.clone();
    let mut rebuilt_sorted = rebuilt.clone();
    let sort_key = |entry: &IndexEntry| {
        (
            entry.object_id,
            entry.object_type.code(),
            entry.offset,
            entry.length,
        )
    };
    existing_sorted.sort_by_key(sort_key);
    rebuilt_sorted.sort_by_key(sort_key);
    let already_correct = existing_sorted == rebuilt_sorted;
    if !already_correct {
        write_file_atomically(
            layout.repository_mutation_root(),
            &index_relative,
            &rebuilt_bytes,
        )?;
    }

    Ok(IndexRepairReport {
        entries_before: before.len(),
        entries_after: rebuilt.len(),
        entries_relocated,
        objects_recovered,
        already_correct,
    })
}

/// Remove exactly one object's index entry, leaving its container bytes untouched -- the container-
/// native way to simulate "this object is genuinely missing" (as opposed to "damaged"): the object
/// exists nowhere the read path will find it, but nothing decodes incorrectly if directly scanned. No
/// production code ever does this (the index is append-only); it exists only so tests that used to
/// simulate a missing object via `std::fs::remove_file` on a loose object path have an equivalent
/// under containers. Every index entry frame is exactly `INDEX_HEADER_LEN + INDEX_BODY_LEN` bytes
/// (fixed-width body, unlike `wal.rs`/`container.rs`'s variable-length envelope bodies), so removal
/// is a direct byte-range splice, not a rewrite-and-reindex.
#[cfg(test)]
pub(crate) fn remove_index_entry_for_test(
    layout: &RepositoryLayout,
    object_id: ObjectId,
) -> Result<()> {
    let path = layout.container_index_path();
    let bytes = std::fs::read(&path)?;
    let replay = decode_index_records(&bytes, 0)?;
    let mut entries = replay.entries.iter();
    for outcome in &replay.record_outcomes {
        let IndexRecordStatus::Evaluated = &outcome.status else {
            continue;
        };
        let Some(entry) = entries.next() else {
            return Err(PrikkError::Integrity(
                "index replay outcome/entry count mismatch".to_string(),
            ));
        };
        if entry.object_id != object_id {
            continue;
        }
        let frame_len = INDEX_HEADER_LEN + INDEX_BODY_LEN;
        let end = outcome.offset + frame_len;
        let mut retained = bytes.get(..outcome.offset).unwrap_or_default().to_vec();
        retained.extend_from_slice(bytes.get(end..).unwrap_or_default());
        std::fs::write(&path, retained)?;
        return Ok(());
    }
    Err(PrikkError::Integrity(format!(
        "no index entry for {object_id} to remove"
    )))
}

#[cfg(test)]
mod tests;
