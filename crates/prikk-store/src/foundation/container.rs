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
//!
//! **0.52.0 step 2 (RFC 155 §9.4):** the decode loop below is generic over [`FrameSource`]
//! (`frame_source.rs`) rather than hand-written over `&[u8]` -- the one primitive the store's own
//! slice reads and (from Part 2B) the archive's own streamed reads both go through, so the two
//! stop being two hand-maintained copies of the identical classification rule. [`decode_container_records`]
//! itself is a thin call into it, over [`SliceFrameSource`].

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_u16, push_u64,
};
use crate::foundation::frame_resync::{
    BoundedScan, ScanBudget, interrupted_append_before_sound_frame_message, require_progress,
    scan_budget_exceeded_message, tallied_sha256,
};
use crate::foundation::frame_source::{FrameSource, SliceFrameSource};
use crate::foundation::fsutil::len_to_u64;

pub(crate) const CONTAINER_VERSION: u16 = 1;
pub(crate) const CONTAINER_HEADER_LEN: usize = 8 + 2 + 8 + 32;

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
        /// a frame. A checksum mismatch over a fully-present claimed body is complete (RFC 165 R5
        /// §9.2) unless a later sound frame starts inside that claimed range, which means a later
        /// write overran a torn frame (0.49.0 step 5, D11/U5 and round 2 U2; see
        /// `checksum_mismatch_is_complete`).
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
    encode_container_record_with_known_body(object_type, envelope, None)
}

/// As [`encode_container_record`], but when the caller already holds bytes it can prove equal
/// `envelope`'s own canonical encoding (`known_body`), skips the `encode_envelope_file` call and
/// frames those bytes directly -- 0.55.0 step 3 Part 3b-ii, for `bundle import`'s own `New`
/// outcome, where `known_body` is the frame body already read and digest-verified from the
/// carried bundle. `None` here is exactly [`encode_container_record`]'s own body, so every
/// existing caller of that function is unaffected by this one's existence.
pub(crate) fn encode_container_record_with_known_body(
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
    known_body: Option<&[u8]>,
) -> Result<Vec<u8>> {
    let body: std::borrow::Cow<'_, [u8]> = match known_body {
        Some(known) => {
            // Test-only parity guard: `known_body` must be provably what a fresh encode would
            // produce (the caller's own argument for this), checked here rather than trusted
            // silently, so a future change that breaks the invariant fails loudly in tests
            // instead of writing a quietly wrong record. Never a release-build cost.
            #[cfg(test)]
            {
                let fresh = encode_envelope_file(envelope)?;
                assert_eq!(
                    fresh, known,
                    "known_body hint disagrees with a fresh encode_envelope_file for the same \
                     envelope -- the caller's own byte-identity claim does not hold"
                );
            }
            std::borrow::Cow::Borrowed(known)
        }
        None => std::borrow::Cow::Owned(encode_envelope_file(envelope)?),
    };
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

/// Result of attempting to parse one frame at a given offset, over any [`FrameSource`] (0.52.0 step
/// 2: the one shared primitive, RFC 155 §9.4). Mirrors `wal::FrameAttempt`, and mirrors what was
/// `archive/verify.rs`'s own `StreamedFrameAttempt` before this step -- the two were already making
/// the identical decision by hand in two places; this is that one place.
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
    /// The header and its full claimed body are both present, and the checksum over that body fails. Whether this is
    /// damage or a torn frame that a later write overran is decided by the decode loop, which can scan (see
    /// [`checksum_mismatch_is_complete`]); this variant cannot, since the classifier below would then recurse.
    ///
    /// Carries no message of its own (2B-2): which text is right -- "damage" or "an interrupted append, naming the
    /// later sound frame" -- depends entirely on what [`checksum_mismatch_is_complete`] finds, so building one here,
    /// before that is known, is exactly the bug this round fixes (a pre-built "checksum mismatch" sentence used
    /// unchanged for both the damage and the interrupted-append case).
    ChecksumMismatch {
        body_end: usize,
    },
}

/// [`checksum_mismatch_is_complete`]'s own resolution (2B-2): a bare `bool` could not carry the one
/// fact an incomplete resolution's own message needs -- the later sound frame's own offset, the
/// evidence that this is a harmless overrun and not damage. `Complete` needs nothing further: its own
/// message never changes. `Interrupted` carries the offset [`BoundedScan::Sound`] already found,
/// rather than discarding it into a bare `true`/`false`.
enum ChecksumMismatchResolution {
    /// Damage: a fully-present, checksummed-wrong body with nothing sound genuinely after it.
    Complete,
    /// An interrupted append: a later write's own sound frame starts inside this frame's own claimed
    /// range, at `sound_frame_offset` -- proof a later write finished past a torn one, never damage.
    Interrupted { sound_frame_offset: usize },
}

/// 0.49.0 step 5, round 2 U2 (RFC 164 §9, RFC 160 F3 Addendum 1): a complete record whose checksum fails
/// is damage, unless a later, sound frame starts **inside the range its own header claims** -- then the
/// frame was torn and a later write overran it, which is an interrupted append. Found by the same budgeted
/// scan the short-read path uses, so exhaustion is damage (RFC 167 C3), never a tail. Nothing after the
/// claimed body is always damage.
fn checksum_mismatch_is_complete<S: FrameSource>(
    object_type: ObjectType,
    magic: &[u8; 8],
    source: &mut S,
    offset: usize,
    body_end: usize,
    max_body_len: Option<u64>,
    budget: &mut ScanBudget,
) -> Result<ChecksumMismatchResolution> {
    if body_end == source.total_len() {
        return Ok(ChecksumMismatchResolution::Complete);
    }
    Ok(
        match sound_frame_after_partial(object_type, magic, source, offset, max_body_len, budget)? {
            BoundedScan::Sound(next) if next >= body_end => ChecksumMismatchResolution::Complete,
            BoundedScan::Sound(sound_frame_offset) => {
                ChecksumMismatchResolution::Interrupted { sound_frame_offset }
            }
            BoundedScan::NoSoundFrame | BoundedScan::Undetermined => {
                ChecksumMismatchResolution::Complete
            }
        },
    )
}

/// 2B-2: the message for a checksum mismatch that is genuinely damage -- a fully-present body whose
/// checksum simply does not match, with nothing sound after it to explain why.
fn checksum_mismatch_message(report_offset: usize) -> String {
    format!("container checksum mismatch at byte offset {report_offset}")
}

/// 2B-2: the message for a checksum mismatch that is an interrupted append, not damage -- naming
/// both the frame's own offset and the later sound frame's own offset, the evidence a later write
/// genuinely finished past a torn one. Mirrors `frame_resync::interrupted_append_before_sound_frame_
/// message`'s own two-offset shape for the short-read case, for the identical reason: the evidence,
/// not just the verdict, is what tells a reader this is harmless.
///
/// **W1 (0.52.0 release prep):** describes the frame only, with no verdict clause of its own -- see
/// `interrupted_append_before_sound_frame_message`'s own doc for why: each caller's own wrapper
/// already supplies the verdict, and a verdict baked in here collided with it (the archive's own
/// line read "interrupted append" twice; the store's own "missing object(s)" branch read "not
/// damage" beside "not a harmless remnant until repaired").
fn checksum_mismatch_interrupted_message(
    report_offset: usize,
    sound_frame_offset: usize,
) -> String {
    format!(
        "the frame at byte offset {report_offset} claims bytes that a later sound frame at byte \
         offset {sound_frame_offset} already uses"
    )
}

/// [`sound_frame_after_partial_budgeted`] (`frame_resync.rs`)'s own shape, specialized to this one
/// format rather than left generic over an arbitrary "is this candidate sound" closure: both of
/// this function's own two call sites (here, and inside the main decode loop below) always ask the
/// identical question -- does a full [`parse_frame_at`] at this candidate offset come back
/// `Record`? -- so a closure parameter would only forward straight back to this same call, never to
/// anything else. `frame_resync::sound_frame_after_partial_budgeted` itself stays untouched for the
/// WAL and the ref log, which still use it directly over `&[u8]` and never need a streamed source.
fn sound_frame_after_partial<S: FrameSource>(
    object_type: ObjectType,
    magic: &[u8; 8],
    source: &mut S,
    offset: usize,
    max_body_len: Option<u64>,
    budget: &mut ScanBudget,
) -> Result<BoundedScan> {
    let Some(mut from) = offset.checked_add(1) else {
        return Ok(BoundedScan::NoSoundFrame);
    };
    loop {
        let Some(candidate) = source.scan_for_magic(from, magic.as_slice())? else {
            return Ok(BoundedScan::NoSoundFrame);
        };
        if budget.exceeded() {
            return Ok(BoundedScan::Undetermined);
        }
        let sound = matches!(
            parse_frame_at_reporting(
                object_type,
                magic,
                source,
                candidate,
                candidate,
                max_body_len,
                budget,
            )?,
            FrameAttempt::Record { .. }
        );
        if sound {
            return Ok(BoundedScan::Sound(candidate));
        }
        from = match candidate.checked_add(1) {
            Some(next) => next,
            None => return Ok(BoundedScan::NoSoundFrame),
        };
    }
}

/// Attempt to parse one container frame at `offset`. Never trusts a not-yet-checksum-validated
/// header's own `body_len` for anything beyond locating where its claimed body would end.
fn parse_frame_at<S: FrameSource>(
    object_type: ObjectType,
    magic: &[u8; 8],
    source: &mut S,
    offset: usize,
    max_body_len: Option<u64>,
    budget: &mut ScanBudget,
) -> Result<FrameAttempt> {
    parse_frame_at_reporting(
        object_type,
        magic,
        source,
        offset,
        offset,
        max_body_len,
        budget,
    )
}

/// [`parse_frame_at`] over a source that may not start at the container's own byte 0:
/// `report_offset` is where the frame sits in the container, so the messages that name a byte
/// offset name the container's, not the source's (an object read decodes one record from a window
/// read at its offset -- RFC 102, the append-length round -- and its errors must read as they
/// always did).
///
/// `max_body_len`, `None` for the store's own commit-path reader (a local container has never
/// bounded a single object's size, and this part does not start now): `Some(bound)` refuses a
/// claim exceeding it with `PrikkError::ObjectOverBound`, **after** the claim is already known to
/// fit entirely within `source`'s own real, present bytes (the past-the-end check below always
/// runs first -- a corrupted length field read as a huge number must resync to the next *sound*
/// frame, never be named "over bound" for a claim that was never real to begin with; I-E item
/// 0(a)'s fix to the archive's own pre-2A streamed decoder, preserved here). This is a parameter,
/// not a constant the primitive enforces unconditionally: the one caller that will ever pass
/// `Some` is the archive (0.52.0 step 2 Part 2B, untrusted input, DC-86's own bound), and the
/// store's own slice path must keep accepting an object of any size exactly as it always has.
fn parse_frame_at_reporting<S: FrameSource>(
    object_type: ObjectType,
    magic: &[u8; 8],
    source: &mut S,
    offset: usize,
    report_offset: usize,
    max_body_len: Option<u64>,
    budget: &mut ScanBudget,
) -> Result<FrameAttempt> {
    let total_len = source.total_len();
    let remaining = total_len.saturating_sub(offset);
    if remaining < CONTAINER_HEADER_LEN {
        return Ok(FrameAttempt::TrailingPartial { remaining });
    }
    let header_end = offset + CONTAINER_HEADER_LEN;
    let Some(header) = source.read_at(offset, CONTAINER_HEADER_LEN)? else {
        return Ok(FrameAttempt::TrailingPartial { remaining });
    };
    let header_values = match parse_header(magic, header) {
        Ok(values) => values,
        Err(err) => {
            return Ok(FrameAttempt::Invalid {
                message: format!("at byte offset {report_offset}: {err}"),
                complete: false,
            });
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return Ok(FrameAttempt::Invalid {
            message: format!(
                "container body length does not fit usize at byte offset {report_offset}"
            ),
            complete: false,
        });
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return Ok(FrameAttempt::Invalid {
            message: format!("container body end overflow at byte offset {report_offset}"),
            complete: false,
        });
    };
    if body_end > total_len {
        return Ok(FrameAttempt::TrailingPartial { remaining });
    }
    if let Some(bound) = max_body_len {
        if header_values.body_len > bound {
            return Err(PrikkError::ObjectOverBound {
                declared_bytes: header_values.body_len,
                bound_bytes: bound,
            });
        }
    }
    let Some(body) = source.read_at(header_end, body_len)? else {
        return Ok(FrameAttempt::TrailingPartial { remaining });
    };
    // The checksum hashes a preimage of the header's own 18 bytes plus the body, so a candidate costs its
    // header too: charging the body alone let a zero-length candidate hash 18 bytes for free (0.49.0 step 5,
    // round 2 U2 -- measured at 12.5x hashed on a claimed-range file, over the 9x one-decode bound).
    budget.charge((body.len() + CONTAINER_HEADER_LEN) as u64);
    let expected = record_checksum(magic, header_values.body_len, body);
    if expected != header_values.checksum {
        return Ok(FrameAttempt::ChecksumMismatch { body_end });
    }
    let envelope = match decode_envelope_file(body) {
        Ok(envelope) => envelope,
        Err(err) => {
            return Ok(FrameAttempt::Invalid {
                message: format!("at byte offset {report_offset}: {err}"),
                complete: true,
            });
        }
    };
    // The frame's magic only proves which *container* this byte range belongs to; nothing about the
    // header constrains what `object_type` the body's own envelope claims. A well-formed, correctly
    // checksummed frame can still decode to a mismatched envelope (e.g. a Blob container frame whose
    // body is a valid Patch envelope) -- checked explicitly here, at the one place every reader of
    // this container passes through, matching the pre-Stage-3 loose-file `verify_object_file`'s own
    // `envelope.object_type != object_type` check it replaces.
    if envelope.object_type != object_type {
        return Ok(FrameAttempt::Invalid {
            message: format!(
                "container record at byte offset {report_offset} is under type {object_type} but \
                 envelope type is {}",
                envelope.object_type
            ),
            complete: true,
        });
    }
    Ok(FrameAttempt::Record {
        record: ContainerRecord { envelope },
        next_offset: body_end,
        checksum: header_values.checksum,
    })
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
    let mut source = SliceFrameSource::new(window);
    match parse_frame_at_reporting(
        object_type,
        magic,
        &mut source,
        0,
        container_offset,
        None,
        &mut budget,
    )? {
        FrameAttempt::Record { record, .. } => Ok(Some(record)),
        FrameAttempt::TrailingPartial { .. } => Ok(None),
        FrameAttempt::Invalid { message, .. } => Err(PrikkError::Integrity(format!(
            "container record at offset {container_offset} failed to validate: {message}"
        ))),
        // This one-shot, already-located read never scans for a later sound frame to decide
        // "damage" vs. "interrupted append" (there is no decode loop here to resume) -- it only
        // ever reports a failure, so the plain "checksum mismatch" text is the right one regardless
        // of which this would have been in the full decode loop.
        FrameAttempt::ChecksumMismatch { .. } => Err(PrikkError::Integrity(format!(
            "container record at offset {container_offset} failed to validate: {}",
            checksum_mismatch_message(container_offset)
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
    let mut source = SliceFrameSource::new(bytes);
    // `None`: the store's own commit-path reader has never bounded a single object's size, and
    // this call keeps it that way -- see `parse_frame_at_reporting`'s own doc for why this is a
    // parameter, not something the shared loop below decides on its own.
    decode_frames(object_type, &mut source, None)
}

/// [`decode_frames_with`]'s own summary, without the `Vec<ContainerRecord>` [`ContainerReplay`]
/// collects every sound record into: a caller that streams records through a callback (the archive,
/// Part 2B) must never hold more than one record's own envelope in memory at a time, and this type
/// is what makes that possible -- `record_outcomes`/`trailing_partial_bytes` are small (offsets and
/// short enums/strings), never a `Vec` of decoded envelopes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ContainerReplaySummary {
    /// Number of trailing bytes ignored as an incomplete final record. See [`ContainerReplay`]'s own
    /// field of the same name.
    pub(crate) trailing_partial_bytes: usize,
    /// One outcome per attempted frame, in scan order. See [`ContainerReplay`]'s own field of the
    /// same name.
    pub(crate) record_outcomes: Vec<ContainerRecordOutcome>,
}

/// The one container-frame decode loop (0.52.0 step 2, RFC 155 §9.4): generic over [`FrameSource`]
/// so the store's own slice reads and the archive's own streamed reads (Part 2B) run the identical
/// classification, not two hand-written copies of it. [`decode_container_records`] calls this with
/// [`SliceFrameSource`] and `max_body_len: None`; the archive's own `classify_section_records`
/// (`archive/verify.rs`) calls it with a section-scoped streamed source and
/// `max_body_len: Some(DEFAULT_BUNDLE_MAX_OBJECT_BYTES)` (DC-86).
///
/// `on_record` is called once per sound frame, in scan order, immediately -- never collected into a
/// `Vec` here. [`decode_frames`] is the thin, `Vec`-collecting wrapper the store's own five
/// production callers already depend on; this is the lower-level form a streaming caller (the
/// archive) needs instead, so that caller never holds more than one record's own envelope at a time
/// (RFC 155 §9.2's own memory bound -- see `archive/verify.rs`'s own doc on why this matters there).
///
/// Isolate-and-continue reading (RFC 102 Stage 2's reader, reused here per the Stage 3 handoff's
/// explicit instruction, not re-derived): a frame that fails to validate no longer aborts replay --
/// its offset and error are recorded as a `Failed` outcome, and [`FrameSource::scan_for_magic`]
/// finds the next candidate frame so every subsequent sound record is still read. Corruption is
/// therefore confined to the records it actually damaged, at container scale exactly as it already
/// is for the WAL and ref log.
pub(crate) fn decode_frames_with<S: FrameSource>(
    object_type: ObjectType,
    source: &mut S,
    max_body_len: Option<u64>,
    mut on_record: impl FnMut(&ContainerRecord) -> Result<()>,
) -> Result<ContainerReplaySummary> {
    let magic = container_magic(object_type)?;
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    // RFC 167 D1: one budget per decode call. This reader's own `Invalid` arm never calls the scan
    // (below) at all -- it goes straight to a bare scan for the next magic -- so the broad
    // placement, checked at the top of every iteration, is this reader's *only* guard against that
    // arm's own cost; the design round measured the narrow placement alone leaving it fully
    // quadratic.
    let mut budget = ScanBudget::for_input(source.total_len());
    loop {
        if budget.exceeded() {
            record_outcomes.push(ContainerRecordOutcome {
                offset,
                status: ContainerRecordStatus::Failed {
                    message: scan_budget_exceeded_message(offset),
                    complete: false,
                },
            });
            return Ok(ContainerReplaySummary {
                trailing_partial_bytes: 0,
                record_outcomes,
            });
        }
        match parse_frame_at(
            object_type,
            magic,
            source,
            offset,
            max_body_len,
            &mut budget,
        )? {
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
                on_record(&record)?;
                offset = require_progress("container", offset, next_offset)?;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                // RFC 160 F3: a torn tail is a prefix of ONE frame. If a sound frame starts in the remainder, this is damage.
                let sound_after = sound_frame_after_partial(
                    object_type,
                    magic,
                    source,
                    offset,
                    max_body_len,
                    &mut budget,
                )?;
                let next = match sound_after {
                    BoundedScan::NoSoundFrame => {
                        return Ok(ContainerReplaySummary {
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
                        return Ok(ContainerReplaySummary {
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                    BoundedScan::Sound(next) => next,
                };
                let message = interrupted_append_before_sound_frame_message(offset, next);
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
                match source.scan_for_magic(offset + 1, magic.as_slice())? {
                    Some(next) => offset = next,
                    None => {
                        return Ok(ContainerReplaySummary {
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                }
            }
            FrameAttempt::ChecksumMismatch { body_end } => {
                let resolution = checksum_mismatch_is_complete(
                    object_type,
                    magic,
                    source,
                    offset,
                    body_end,
                    max_body_len,
                    &mut budget,
                )?;
                // 2B-2: the message is built *after* the resolution is known, not before -- a
                // checksum mismatch that is actually an interrupted append must say so, naming the
                // later sound frame as its own evidence, never the plain "checksum mismatch" text
                // that belongs to the damage case alone.
                let (complete, message) = match resolution {
                    ChecksumMismatchResolution::Complete => {
                        (true, checksum_mismatch_message(offset))
                    }
                    ChecksumMismatchResolution::Interrupted { sound_frame_offset } => (
                        false,
                        checksum_mismatch_interrupted_message(offset, sound_frame_offset),
                    ),
                };
                record_outcomes.push(ContainerRecordOutcome {
                    offset,
                    status: ContainerRecordStatus::Failed { message, complete },
                });
                match source.scan_for_magic(offset + 1, magic.as_slice())? {
                    Some(next) => offset = next,
                    None => {
                        return Ok(ContainerReplaySummary {
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                }
            }
        }
    }
}

/// [`decode_frames_with`], collecting every sound record into a `Vec` -- the shape the store's own
/// five production callers (`decode_container_records`'s own callers) already depend on, which read
/// a whole container into memory anyway and want every record back at once. A streaming caller (the
/// archive, Part 2B) calls [`decode_frames_with`] directly instead, so it never pays this `Vec`'s
/// own memory cost.
pub(crate) fn decode_frames<S: FrameSource>(
    object_type: ObjectType,
    source: &mut S,
    max_body_len: Option<u64>,
) -> Result<ContainerReplay> {
    let mut records = Vec::new();
    let summary = decode_frames_with(object_type, source, max_body_len, |record| {
        records.push(record.clone());
        Ok(())
    })?;
    Ok(ContainerReplay {
        records,
        trailing_partial_bytes: summary.trailing_partial_bytes,
        record_outcomes: summary.record_outcomes,
    })
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

/// RFC 155 Part V: the archive verifier streams one container record at a time from a raw section
/// body and must recompute this exact checksum to confirm an object survived transport -- reused,
/// not re-derived, so a flipped byte is detected the identical way a local repository's own replay
/// would detect it.
pub(crate) fn record_checksum(magic: &[u8; 8], body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(magic);
    preimage.extend_from_slice(&CONTAINER_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    tallied_sha256(&preimage)
}

#[cfg(test)]
mod tests;
