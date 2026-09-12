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
use prikk_hash::sha256;
use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_u16, push_u64,
};
use crate::foundation::frame_resync::resync_to_next_magic;
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
#[cfg(any(test, feature = "test-support"))]
pub fn encode_container_record_for_test(
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
    },
}

/// Attempt to parse one container frame at `offset`. Never trusts a not-yet-checksum-validated
/// header's own `body_len` for anything beyond locating where its claimed body would end.
fn parse_frame_at(
    object_type: ObjectType,
    magic: &[u8; 8],
    bytes: &[u8],
    offset: usize,
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
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "container body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "container body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let expected = record_checksum(magic, header_values.body_len, body);
    if expected != header_values.checksum {
        return FrameAttempt::Invalid {
            message: format!("container checksum mismatch at byte offset {offset}"),
        };
    }
    let envelope = match decode_envelope_file(body) {
        Ok(envelope) => envelope,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
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
                "container record at byte offset {offset} is under type {object_type} but \
                 envelope type is {}",
                envelope.object_type
            ),
        };
    }
    FrameAttempt::Record {
        record: ContainerRecord { envelope },
        next_offset: body_end,
        checksum: header_values.checksum,
    }
}

/// Decode exactly the frame at `offset` -- the "one seek" side of item 4's ruling (design §12/§10.3):
/// a reader that already knows an object's location (from the index) validates and decodes directly
/// at that offset, rather than scanning every record before it via [`decode_container_records`]. A
/// frame that fails to validate here (bad magic, checksum mismatch, an offset that does not land on a
/// real frame boundary) is exactly the "index points somewhere wrong" case the ruling calls a
/// reported defect, not a silent fallback to scanning -- so this returns `Err`, never `Ok(None)`,
/// for a bad frame. Only "not enough bytes remain" (`TrailingPartial`) is folded into `Ok(None)`,
/// since a location that runs off the end of the file is the same class of defect stated slightly
/// differently.
pub(crate) fn decode_container_record_at(
    object_type: ObjectType,
    bytes: &[u8],
    offset: usize,
) -> Result<Option<ContainerRecord>> {
    let magic = container_magic(object_type)?;
    match parse_frame_at(object_type, magic, bytes, offset) {
        FrameAttempt::Record { record, .. } => Ok(Some(record)),
        FrameAttempt::TrailingPartial { .. } => Ok(None),
        FrameAttempt::Invalid { message } => Err(PrikkError::Integrity(format!(
            "container record at offset {offset} failed to validate: {message}"
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
    loop {
        match parse_frame_at(object_type, magic, bytes, offset) {
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
                offset = next_offset;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                return Ok(ContainerReplay {
                    records,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            FrameAttempt::Invalid { message } => {
                record_outcomes.push(ContainerRecordOutcome {
                    offset,
                    status: ContainerRecordStatus::Failed { message },
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
    sha256(&preimage)
}

#[cfg(test)]
mod tests;
