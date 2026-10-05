//! Object-container framing and the isolate-and-continue read path (RFC 102 Stage 3, design-v1.md
//! §2-§3). One container file per persisted object type; framing and the read path are the WAL's
//! proven shape (magic, version, length, checksum, body -- reused, per the handoff, not re-derived),
//! with two deliberate differences from `wal.rs`:
//!
//! - **No sequence field.** RFC 102 Stage 3 Step 0 item 2 (design-v1.md §12/§10.1) ruled that
//!   container-record ordering within a type is not required -- objects are immutable and
//!   content-addressed, so nothing downstream ever consults write order. This mirrors `refs/log.rs`'s
//!   frame shape, not `wal.rs`'s.
//! - **A distinct magic per object type** (`container_magic`), so a frame decoded from the wrong
//!   container is a detectable magic mismatch rather than silently accepted -- one object type's
//!   container should never contain another type's frame.
//!
//! The byte-wise resync scan itself is `frame_resync::resync_to_next_magic`, shared with `wal.rs` and
//! `refs/log.rs`, not a third copy of the same logic.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_u16, push_u64,
};
use crate::foundation::frame_resync::{
    BoundedScan, ScanBudget, partial_before_sound_frame_message, require_progress,
    resync_to_next_magic, scan_budget_exceeded_message, sound_frame_after_partial_budgeted,
    tallied_sha256,
};
use crate::foundation::fsutil::len_to_u64;

const CONTAINER_VERSION: u16 = 1;
const CONTAINER_HEADER_LEN: usize = 8 + 2 + 8 + 32;

/// Return the fixed 8-byte magic for one persisted object type's container frames. Every
/// `persisted_object_types()` entry has one; called with anything else is a programmer error (no
/// non-persisted type is ever containerized), reported rather than panicking.
pub(crate) fn container_magic(object_type: ObjectType) -> Result<&'static [u8; 8]> {
    match object_type {
        ObjectType::Patch => Ok(b"PCONPAT1"),
        ObjectType::Block => Ok(b"PCONBLK1"),
        ObjectType::RefState => Ok(b"PCONRFS1"),
        ObjectType::Tag => Ok(b"PCONTAG1"),
        ObjectType::Attestation => Ok(b"PCONATT1"),
        ObjectType::Blob => Ok(b"PCONBLB1"),
        ObjectType::RecognitionClaim => Ok(b"PCONRCL1"),
        other => Err(PrikkError::UnsupportedObjectType(format!(
            "{other} has no object container"
        ))),
    }
}

/// One durable container record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContainerRecord {
    /// Exact signed object envelope stored at append time.
    pub(crate) envelope: ObjectEnvelope,
}

/// Outcome of attempting to decode one container record frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ContainerRecordStatus {
    /// The frame at this offset was read and validated successfully.
    Evaluated {
        /// This frame's total length in bytes (header + body) -- offset + `frame_len` is the next
        /// frame's own offset. Carried here, unlike `wal::WalRecordOutcome`'s equivalent, because the
        /// index (design §4) needs `(offset, length)` per record and rebuild-by-scan (design §4:
        /// "rebuild is not a new operation ... a rebuild is that, iterated over a scan") reads it
        /// straight from a replay rather than re-parsing to recover it.
        frame_len: usize,
        /// This frame's own checksum bytes, as persisted in its header -- the index's own
        /// `container_checksum` field is this value, so rebuild-by-scan reads it directly rather
        /// than re-deriving it.
        checksum: [u8; 32],
    },
    /// The frame at this offset failed to validate (bad magic/version, checksum mismatch, or a
    /// malformed envelope) -- resync moved past it byte-wise to find the next candidate frame.
    Failed {
        /// The error this frame's own validation raised.
        message: String,
        /// **True when this frame cannot be a torn tail.** An interrupted append leaves a *prefix* of
        /// a frame, so a checksum mismatch where nothing at all follows the claimed body (0.49.0 step
        /// 5, D11/U5; RFC 165 R5 §9.2's rule) is corruption of an already-complete write, not a crash
        /// mid-write -- `verify` never calls that an interrupted append (RFC 160 F3 Addendum 1 / RFC
        /// 162 rule 2). **Known gap:** a checksum mismatch where later bytes *do* follow stays
        /// `false` even when those bytes are really an unrelated, later frame rather than evidence
        /// this one was torn -- see `parse_frame_at_reporting`'s own comment at the checksum check.
        complete: bool,
    },
}

/// One attempted container record frame's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContainerRecordOutcome {
    /// The byte offset within the container this frame attempt started at.
    pub(crate) offset: usize,
    /// How this frame's own read/validation resolved.
    pub(crate) status: ContainerRecordStatus,
}

/// Container replay result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContainerReplay {
    /// Valid records read from the container, in file order -- includes records found after a
    /// damaged one, not merely a prefix up to the first failure.
    pub(crate) records: Vec<ContainerRecord>,
    /// Number of trailing bytes ignored as an incomplete final record -- a legitimate torn tail from
    /// an interrupted append.
    pub(crate) trailing_partial_bytes: usize,
    /// One outcome per attempted frame, in scan order -- both `Evaluated` and `Failed`.
    pub(crate) record_outcomes: Vec<ContainerRecordOutcome>,
}

impl ContainerReplay {
    /// Return true when any attempted frame failed to validate. `verify/objects.rs` inspects
    /// `record_outcomes` directly instead (item-level, matching each entry to `Evaluated`/
    /// `Unindexed`/`Failed`), so this aggregate has no production caller yet -- kept for parity with
    /// `WalReplay`/`RefLogReplay`'s equivalent, and for whatever repair/CLI tooling next needs "is
    /// this container damaged at all" without walking every outcome itself.
    #[allow(dead_code)]
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, ContainerRecordStatus::Failed { .. }))
    }
}

/// Encode one object envelope as a durable container record for `object_type`'s container.
pub(crate) fn encode_container_record(
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
) -> Result<Vec<u8>> {
    let body = encode_envelope_file(envelope)?;
    frame_record(object_type, &body)
}

/// Encode one container record exactly as the writer would, for a test that needs the bytes without
/// the write.
#[cfg(test)]
pub(crate) fn encode_container_record_for_test(
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
) -> Result<Vec<u8>> {
    let body = crate::foundation::file_codec::encode_envelope_file_structural(envelope)?;
    frame_record(object_type, &body)
}

fn frame_record(object_type: ObjectType, body: &[u8]) -> Result<Vec<u8>> {
    let magic = container_magic(object_type)?;
    let body_len = len_to_u64(body.len())?;
    let checksum = record_checksum(magic, body_len, body);
    let mut out = Vec::with_capacity(CONTAINER_HEADER_LEN + body.len());
    out.extend_from_slice(magic);
    push_u16(&mut out, CONTAINER_VERSION);
    push_u64(&mut out, body_len);
    out.extend_from_slice(&checksum);
    out.extend_from_slice(body);
    Ok(out)
}

/// Result of attempting to parse one frame at a given offset. Mirrors `wal::FrameAttempt`.
enum FrameAttempt {
    Record {
        record: ContainerRecord,
        next_offset: usize,
        checksum: [u8; 32],
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
        /// See [`ContainerRecordStatus::Failed`]'s own doc -- mirrored here, not redefined.
        complete: bool,
    },
}

/// Attempt to parse one container frame at `offset`. Never trusts a not-yet-checksum-validated
/// header's own `body_len` for anything beyond locating where its claimed body would end.
fn parse_frame_at(
    object_type: ObjectType,
    magic: &[u8; 8],
    bytes: &[u8],
    offset: usize,
    budget: &mut ScanBudget,
) -> FrameAttempt {
    parse_frame_at_reporting(object_type, magic, bytes, offset, offset, budget)
}

/// [`parse_frame_at`] over a buffer that is **not the whole container**: `report_offset` is where the frame sits in the container, so
/// the messages that name a byte offset name the container's, not the buffer's (an object read decodes one record from a window read
/// at its offset -- RFC 102, the append-length round -- and its errors must read as they always did).
fn parse_frame_at_reporting(
    object_type: ObjectType,
    magic: &[u8; 8],
    bytes: &[u8],
    offset: usize,
    report_offset: usize,
    budget: &mut ScanBudget,
) -> FrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < CONTAINER_HEADER_LEN {
        return FrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + CONTAINER_HEADER_LEN;
    // In range by construction: `remaining >= CONTAINER_HEADER_LEN` was just checked above --
    // `.get()` used anyway to satisfy `clippy::indexing_slicing`, not because this can fail.
    let Some(header) = bytes.get(offset..header_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_header(magic, header) {
        Ok(values) => values,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
                complete: false,
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "container body length does not fit usize".to_string(),
            complete: false,
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "container body end overflow".to_string(),
            complete: false,
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    budget.charge(body.len() as u64);
    let expected = record_checksum(magic, header_values.body_len, body);
    if expected != header_values.checksum {
        // 0.49.0 step 5, D11/U5 (RFC 165 R5 §9.2's rule, applied here): a complete record -- full
        // header, full claimed body, both physically read above -- whose checksum fails was fully
        // written: corruption, not a crash mid-write, so it must not be called an interrupted append.
        //
        // **Narrowed to `body_end == bytes.len()`, not every checksum mismatch**: when bytes remain
        // past `body_end`, this claimed body may have read past a genuine torn frame's own short
        // write into a *later*, unrelated frame's bytes (RFC 160 F3 Addendum 1's own motivating
        // shape: a torn frame immediately followed by further commits) -- the checksum mismatch
        // there proves nothing about whether THIS frame was itself fully written. Telling the two
        // apart in general needs the same "is a sound frame hiding in the claimed range" scan
        // `sound_frame_after_partial_budgeted` already does for a short read, budgeted the same way;
        // doing that here is unscheduled this round and left as a known gap (the fix below covers
        // only the unambiguous case: nothing at all follows the claimed body).
        let complete = body_end == bytes.len();
        return FrameAttempt::Invalid {
            message: format!("container checksum mismatch at byte offset {report_offset}"),
            complete,
        };
    }
    let envelope = match decode_envelope_file(body) {
        Ok(envelope) => envelope,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
                complete: true,
            };
        }
    };
    // The frame's magic only proves which *container* this byte range belongs to; nothing about the
    // header constrains what `object_type` the body's own envelope claims. A well-formed, correctly
    // checksummed frame can still decode to a mismatched envelope (e.g. a Blob container frame whose
    // body is a valid Patch envelope) -- checked explicitly here, at the one place every reader of
    // this container passes through, matching the pre-Stage-3 loose-file `verify_object_file`'s own
    // `envelope.object_type != object_type` check it replaces.
    if envelope.object_type != object_type {
        return FrameAttempt::Invalid {
            message: format!(
                "container record at byte offset {report_offset} is under type {object_type} but \
                 envelope type is {}",
                envelope.object_type
            ),
            complete: true,
        };
    }
    FrameAttempt::Record {
        record: ContainerRecord { envelope },
        next_offset: body_end,
        checksum: header_values.checksum,
    }
}

/// The container header's own length in bytes (magic, version, body length, checksum): what an object read fetches first to learn how
/// long the frame at a known offset is.
pub(crate) const FRAME_HEADER_LEN: usize = CONTAINER_HEADER_LEN;

/// The total length (header + body) the frame whose first [`FRAME_HEADER_LEN`] bytes are `header` claims, or the same `Err` a full parse
/// gives for a bad magic or version. `header` must be [`FRAME_HEADER_LEN`] bytes.
pub(crate) fn frame_len_from_header(object_type: ObjectType, header: &[u8]) -> Result<usize> {
    let magic = container_magic(object_type)?;
    let values = parse_header(magic, header)?;
    let body_len = usize::try_from(values.body_len).map_err(|_| {
        PrikkError::Integrity("container body length does not fit usize".to_string())
    })?;
    CONTAINER_HEADER_LEN
        .checked_add(body_len)
        .ok_or_else(|| PrikkError::Integrity("container body end overflow".to_string()))
}

/// [`decode_container_record_at`] for a `window` that starts at the frame: the record at the window's first byte, where that frame sits
/// at `container_offset` in its container (used only in messages, so they name the container's offset). `Ok(None)` when the window is
/// shorter than the frame it holds.
pub(crate) fn decode_container_record_in_window(
    object_type: ObjectType,
    window: &[u8],
    container_offset: usize,
) -> Result<Option<ContainerRecord>> {
    let magic = container_magic(object_type)?;
    // Not part of any candidate scan -- one already-located record, read once. A fresh, throwaway
    // budget is correct here: this call can never reach `Undetermined` (there is no loop to cut
    // short), so nothing downstream of it ever needs to know this budget existed.
    let mut budget = ScanBudget::for_input(window.len());
    match parse_frame_at_reporting(object_type, magic, window, 0, container_offset, &mut budget) {
        FrameAttempt::Record { record, .. } => Ok(Some(record)),
        FrameAttempt::TrailingPartial { .. } => Ok(None),
        FrameAttempt::Invalid { message, .. } => Err(PrikkError::Integrity(format!(
            "container record at offset {container_offset} failed to validate: {message}"
        ))),
    }
}

/// Isolate-and-continue reading (RFC 102 Stage 2's reader, reused here per the Stage 3 handoff's
/// explicit instruction, not re-derived): a frame that fails to validate no longer aborts replay --
/// its offset and error are recorded as a `Failed` outcome, and
/// `frame_resync::resync_to_next_magic` finds the next candidate frame so every subsequent sound
/// record is still read. Corruption is therefore confined to the records it actually damaged, at
/// container scale exactly as it already is for the WAL and ref log.
pub(crate) fn decode_container_records(
    object_type: ObjectType,
    bytes: &[u8],
) -> Result<ContainerReplay> {
    let magic = container_magic(object_type)?;
    let mut records = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    // RFC 167 D1: one budget per decode call. This reader's own `Invalid` arm never calls the scan
    // (below) at all -- it goes straight to a bare `resync_to_next_magic` -- so the broad placement,
    // checked at the top of every iteration, is this reader's *only* guard against that arm's own
    // cost; the design round measured the narrow placement alone leaving it fully quadratic.
    let mut budget = ScanBudget::for_input(bytes.len());
    loop {
        if budget.exceeded() {
            record_outcomes.push(ContainerRecordOutcome {
                offset,
                status: ContainerRecordStatus::Failed {
                    message: scan_budget_exceeded_message(offset),
                    complete: false,
                },
            });
            return Ok(ContainerReplay {
                records,
                trailing_partial_bytes: 0,
                record_outcomes,
            });
        }
        match parse_frame_at(object_type, magic, bytes, offset, &mut budget) {
            FrameAttempt::Record {
                record,
                next_offset,
                checksum,
            } => {
                record_outcomes.push(ContainerRecordOutcome {
                    offset,
                    status: ContainerRecordStatus::Evaluated {
                        frame_len: next_offset - offset,
                        checksum,
                    },
                });
                records.push(record);
                offset = require_progress("container", offset, next_offset)?;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                // RFC 160 F3: a torn tail is a prefix of ONE frame. If a sound frame starts in the remainder, this is damage.
                let sound_after = sound_frame_after_partial_budgeted(
                    bytes,
                    offset,
                    magic.as_slice(),
                    &mut budget,
                    |c, budget| {
                        matches!(
                            parse_frame_at(object_type, magic, bytes, c, budget),
                            FrameAttempt::Record { .. }
                        )
                    },
                );
                let next = match sound_after {
                    BoundedScan::NoSoundFrame => {
                        return Ok(ContainerReplay {
                            records,
                            trailing_partial_bytes: remaining,
                            record_outcomes,
                        });
                    }
                    BoundedScan::Undetermined => {
                        record_outcomes.push(ContainerRecordOutcome {
                            offset,
                            status: ContainerRecordStatus::Failed {
                                message: scan_budget_exceeded_message(offset),
                                complete: false,
                            },
                        });
                        return Ok(ContainerReplay {
                            records,
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                    BoundedScan::Sound(next) => next,
                };
                let message = partial_before_sound_frame_message(offset, next);
                record_outcomes.push(ContainerRecordOutcome {
                    offset,
                    status: ContainerRecordStatus::Failed {
                        message,
                        complete: false,
                    },
                });
                offset = require_progress("container", offset, next)?;
            }
            FrameAttempt::Invalid { message, complete } => {
                record_outcomes.push(ContainerRecordOutcome {
                    offset,
                    status: ContainerRecordStatus::Failed { message, complete },
                });
                match resync_to_next_magic(bytes, offset + 1, magic.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(ContainerReplay {
                            records,
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                }
            }
        }
    }
}

struct ContainerHeader {
    body_len: u64,
    checksum: [u8; 32],
}

fn parse_header(magic: &[u8; 8], header: &[u8]) -> Result<ContainerHeader> {
    let mut cursor = ByteCursor::new(header);
    let actual_magic = cursor.read_array::<8>()?;
    if &actual_magic != magic {
        return Err(PrikkError::MalformedData(
            "invalid container record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != CONTAINER_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in container header".to_string(),
        ));
    }
    Ok(ContainerHeader { body_len, checksum })
}

fn record_checksum(magic: &[u8; 8], body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(magic);
    preimage.extend_from_slice(&CONTAINER_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    tallied_sha256(&preimage)
}

#[cfg(test)]
mod tests;
