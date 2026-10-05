//! Shared ref-log container framing and the isolate-and-continue read path (RFC 102 Stage 4, Step 0
//! §13.1/§13.2/§13.5, ruled in design-v1.md §13). One container holds every ref's log records,
//! interleaved -- acceptance criterion 1 forces this: ref names do not exist at `init`, so a per-ref
//! container is architecturally impossible (`branch create`/`tag create` mint them later, as ordinary
//! recurring operations, not an `init`-only event).
//!
//! Framing mirrors the top-level object `container.rs` (magic, version, length, checksum, body), with
//! one deliberate addition: **`ref_name_key` (`sha256(ref_name)`) lives in the frame header itself,
//! not only inside the encoded envelope body.** Reasoning, not copied from anywhere: corruption
//! isolation (Step 0 §13.5, promoted to an acceptance criterion) needs a damaged record attributed to
//! its own ref, matching today's per-file granularity -- but a record whose body fails to decode has
//! no *trusted* way to reveal which ref it belonged to from its own (corrupted) envelope. Carrying
//! `ref_name_key` in the header gives every reader a best-effort attribution even when the checksum
//! fails and the body cannot be decoded at all: for the overwhelming common case (corruption localized
//! to the body, header otherwise intact), the header's own claim is correct; for pathological
//! corruption that happens to land on the header field itself, the checksum has already failed the
//! whole frame regardless, so nothing trusts the record's *content* either way -- only its
//! attribution for reporting purposes is at stake, not data integrity.
//!
//! No sequence field. Step 0 §13.1 found `refs/log.rs`'s old per-file `validate_log` check carried
//! three properties (ref-name uniformity, the chain link, and the positional `update_seq == index +
//! 1`), and the positional one was positional only as a shortcut, holding today because one file
//! happened to be exactly one ref's own subsequence. Under a shared container `expected_seq` is
//! computed by the *reader*, from a record's position within its own ref's filtered subsequence
//! (`refs/verify/scan.rs`'s rewritten `validate_log`) -- the container itself guarantees nothing
//! beyond Stage 3's plain append-only, and `RefLock` (unchanged) is what keeps one ref's own writes in
//! that order.
//!
//! The byte-wise resync scan is `frame_resync::resync_to_next_magic`, shared with `wal.rs`,
//! `refs/log.rs` (the retired per-file codec), and the top-level `container.rs` -- not a fourth copy.
//!
//! **Every `ContainerSlot::A` reference below is hardcoded, not resolver-routed -- deliberately, not
//! an oversight.** RFC 102 Stage 6 Step 1 (design-v1.md §15.6) gave three *other* containers
//! (`ref_pointer_index`, `received_index`, `trust_policy_container`) their own generation logs and a
//! `generation::resolve_live_slot` reader, because §15.1 found those three are the genuine
//! last-entry-wins garbage producers Stage 6 exists to compact. This container is not one of them: it
//! is DC-38's audit trail and DC-69's "prikk does not forget" ruling made durable, so it must never be
//! compacted, and `B` stays reserved-but-unused forever (`ContainerSlot`'s own doc, §15.2's "forward
//! reservation, not dead" framing). Routing these sites through the resolver would be uniformity
//! ceremony on a container that will never exercise it -- exactly the staging error §15.4's original
//! (now-superseded) Step 1 proposal made for the ref-pointer-index and received-index containers
//! before the restructure, so it is not repeated here on purpose.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectType, RefUpdatePayload};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_u16, push_u64,
};
use crate::foundation::frame_resync::{
    BoundedScan, ScanBudget, complete_by_checksum, partial_before_sound_frame_message,
    require_progress, resync_to_next_magic, scan_budget_exceeded_message,
    sound_frame_after_partial_budgeted, tallied_sha256,
};
use crate::foundation::fsutil::{
    append_file_reporting_offset_required, append_file_required, len_to_u64, read_file_if_exists,
};
use crate::foundation::layout::RepositoryLayout;
use crate::refs::require_signed_type;

/// One decoded ref-log record, scoped to one ref's own subsequence. Was `refs/log.rs`'s own type
/// before RFC 102 Stage 4 retired that per-file codec; kept the exact same name and shape since
/// `RefStore::replay_log`'s public return type (and every one of its 13 production callers) never
/// changed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLogRecord {
    /// Exact signed RefUpdate envelope stored in the log.
    pub envelope: ObjectEnvelope,
}

/// Outcome of attempting to decode one ref-log record, scoped to one ref's own subsequence (RFC 102
/// Stage 2: isolate-and-continue reading). Mirrors `wal::WalRecordOutcome`; see its doc for the
/// reasoning this shares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefLogRecordStatus {
    /// The frame at this offset was read and validated successfully.
    Evaluated,
    /// The frame at this offset failed to validate (bad magic/version, checksum mismatch, or a
    /// malformed/unsigned envelope) -- resync moved past it byte-wise to find the next candidate.
    Failed {
        /// The error this frame's own validation raised.
        message: String,
    },
}

/// One attempted ref-log record's resolved outcome, scoped to one ref's own subsequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLogRecordOutcome {
    /// The byte offset within the shared container this frame attempt started at.
    pub offset: usize,
    /// How this frame's own read/validation resolved.
    pub status: RefLogRecordStatus,
}

/// One ref's own log replay result -- `replay_ref_subsequence`'s own return type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefLogReplay {
    /// Valid records read from this ref's own subsequence, in relative order -- includes records
    /// found after a damaged one (RFC 102 Stage 2), not merely a prefix up to the first failure.
    pub records: Vec<RefLogRecord>,
    /// This ref's own attributed trailing-partial byte count (design-v1.md §13.6 point 2) -- zero
    /// unless the container's own physical trailing partial tail's header could be read and claims
    /// this ref specifically.
    pub trailing_partial_bytes: usize,
    /// One outcome per attempted frame attributable to this ref, in scan order -- both `Evaluated`
    /// and `Failed`.
    pub record_outcomes: Vec<RefLogRecordOutcome>,
}

impl RefLogReplay {
    /// Return true when any attempted frame attributable to this ref failed to validate.
    #[must_use]
    pub fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, RefLogRecordStatus::Failed { .. }))
    }
}

const REF_CONTAINER_MAGIC: &[u8; 8] = b"PREFCON1";
const REF_CONTAINER_VERSION: u16 = 1;
/// magic(8) + version(2) + ref_name_key(32) + body_len(8) + checksum(32).
const REF_CONTAINER_HEADER_LEN: usize = 8 + 2 + 32 + 8 + 32;

/// One durable ref-log container record.
///
/// RFC 131 §3/§5: `pub(in crate::refs)`, not `pub(crate)` -- `container` is already a private
/// submodule of `refs` (no other top-level module can name `crate::refs::container::` at all), so
/// this marking changes no real reach; it makes that already-true fact self-evident from the item
/// itself rather than only from tracing `refs.rs`'s own `mod container;` privacy. Verified: no
/// file outside `refs`'s own tree references this type (checked crate-wide, code sites only).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::refs) struct RefContainerRecord {
    /// This record's own header-carried ref-name key -- trusted (the frame's checksum covers it),
    /// since this variant is only ever produced for a frame that passed checksum validation.
    pub(in crate::refs) ref_name_key: [u8; 32],
    /// Exact signed RefUpdate envelope stored at append time.
    pub(in crate::refs) envelope: ObjectEnvelope,
}

/// Outcome of attempting to decode one ref-log container record frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::refs) enum RefContainerRecordStatus {
    /// The frame at this offset was read and validated successfully.
    Evaluated,
    /// The frame at this offset failed to validate (bad magic/version, checksum mismatch, or a
    /// malformed/unsigned envelope) -- resync moved past it byte-wise to find the next candidate.
    Failed {
        /// The error this frame's own validation raised.
        message: String,
        /// The header's own `ref_name_key` claim, when the header parsed structurally far enough to
        /// read it -- **not checksum-verified** (the frame as a whole already failed that), so this
        /// is a best-effort attribution for reporting, never trusted for anything else. `None` only
        /// when the failure occurred before the header's own bytes were even readable (`TrailingPartial`
        /// never reaches here at all; only a structurally-short header on a corrupted-but-not-torn
        /// tail could leave this `None`).
        claimed_ref_name_key: Option<[u8; 32]>,
        /// RFC 165 R5 (§9.2): true when a checksum-verified interpretation of this frame exists (its
        /// stored checksum matches either the claimed length's body or the length-to-end-of-file
        /// body, computed with the format's own real magic/version constants, never the possibly-
        /// corrupted on-disk ones) -- the record was fully written, so whatever caused *this*
        /// validation to fail (a corrupted magic/version byte, a checksum mismatch at the claimed
        /// length, a malformed envelope, an unsigned RefUpdate) is damage, never a tail, even when
        /// this is the container's own last attempted frame.
        never_a_tail: bool,
    },
}

/// One attempted ref-log container record frame's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::refs) struct RefContainerRecordOutcome {
    /// The byte offset within the container this frame attempt started at.
    pub(in crate::refs) offset: usize,
    /// How this frame's own read/validation resolved.
    pub(in crate::refs) status: RefContainerRecordStatus,
}

/// Ref-log container replay result -- every ref's records, interleaved, in physical (write) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefContainerReplay {
    /// Valid records read from the container, in file order -- includes records found after a
    /// damaged one, not merely a prefix up to the first failure.
    pub(in crate::refs) records: Vec<RefContainerRecord>,
    /// Number of trailing bytes ignored as an incomplete final record.
    pub(in crate::refs) trailing_partial_bytes: usize,
    /// One outcome per attempted frame, in scan order -- both `Evaluated` and `Failed`.
    pub(in crate::refs) record_outcomes: Vec<RefContainerRecordOutcome>,
}

impl RefContainerReplay {
    /// `(records decoded, frames reported failed, trailing partial bytes)`, the same shape and for the same reason as
    /// `PointerIndexReplay::counts` -- without leaking `RefContainerRecordStatus` (`pub(in crate::refs)`) past this module.
    #[cfg(test)]
    #[must_use]
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let failed = self
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, RefContainerRecordStatus::Failed { .. }))
            .count();
        (self.records.len(), failed, self.trailing_partial_bytes)
    }

    /// RFC 165 R5 (§9.2 for the ref log): true when the container has **damage**, in the handoff's own
    /// classification table's sense -- never excusable as the container's own tail, whatever the
    /// pointer index says about any ref's lead. Two ways a `Failed` outcome earns this:
    /// - `never_a_tail: true` -- a complete, fully-written record (its checksum verifies) whose later
    ///   validation nonetheless failed. Damage, unconditionally, **even when it is the container's own
    ///   last attempted frame** (RFC 164 §9's own rule, extended here to the ref log).
    /// - any `Failed` outcome that is **not** the container's own last attempted frame -- something
    ///   sound or failed follows it, so whatever reached this point in the scan was not the physical
    ///   end of the container, regardless of its own `never_a_tail` value.
    ///
    /// Every `Failed` outcome is pushed in scan order (`decode_ref_container_records`'s own loop), so
    /// the last element of `record_outcomes`, if any, is always the physically-last attempted frame.
    /// Distinct from a lead-free tail (`ref_log_container_tail`, below), which must not be confused
    /// with an incomplete publication -- see `refs::ensure_no_incomplete_publication_except`'s own doc
    /// for why that distinction matters.
    #[must_use]
    pub(in crate::refs) fn has_damage(&self) -> bool {
        let last_index = self.record_outcomes.len().checked_sub(1);
        self.record_outcomes
            .iter()
            .enumerate()
            .any(|(index, outcome)| match &outcome.status {
                RefContainerRecordStatus::Failed { never_a_tail, .. } => {
                    *never_a_tail || Some(index) != last_index
                }
                RefContainerRecordStatus::Evaluated => false,
            })
    }
}

/// The ref-log container's own trailing tail, by position (RFC 162 rule 3 / RFC 164 Rule A: everything
/// after the last sound record, when nothing sound follows, regardless of shape). Covers both frame
/// shapes `decode_ref_container_records` can leave at the physical end: a torn partial too short to
/// parse a header from (`trailing_partial_bytes > 0`, no `Failed` outcome pushed for it), and a
/// frame-sized-or-larger span whose header or checksum fails with no sound resync point after it (a
/// `Failed` outcome that is the replay's own last attempted frame). `None` when the container ends
/// cleanly, or when `has_interior_damage` is true (a terminal `Failed` entry reached by continuing past
/// earlier interior damage is still interior damage's business, not this function's -- callers check
/// `has_interior_damage` first).
///
/// RFC 165 Addendum 1 §1: a lead-free tail is not an incomplete publication -- `publish_locked` writes
/// the pointer before the log, so a genuine crash mid-publication always leaves a pointer lead, which
/// `ensure_no_incomplete_publication_except`'s own agreement check still catches regardless of whether
/// a tail also exists. This exists so a publishing command can refuse over the tail specifically
/// (RFC 164 Rule D: a writer refuses over a tail in a file it appends to), without conflating that with
/// "an interrupted publication," and without a second whole-container read: callers reuse the one read
/// `decode_ref_container_records` already did for them.
///
/// No ref-name attribution here -- an earlier version carried the tail's own best-effort
/// header-claimed `ref_name_key` so a caller could exclude "its own" tail by name, but a torn tail
/// short enough to carry no readable name at all (as little as a few bytes) made that refuse a ref's
/// own first-ever, very-short interrupted write, which DC-38's retry must still complete regardless of
/// whether the physical tail can be attributed to it by header inspection. Callers instead ask whether
/// *their own excluded ref* currently has a pointer lead (`refs::ensure_may_publish`'s own doc explains
/// why that is the right question, and why it is always safe).
pub(in crate::refs) struct RefLogContainerTail {
    pub(in crate::refs) offset: usize,
    pub(in crate::refs) len: usize,
}

pub(in crate::refs) fn ref_log_container_tail(
    bytes: &[u8],
    discovery: &RefContainerReplay,
) -> Option<RefLogContainerTail> {
    if discovery.trailing_partial_bytes == 0 {
        return None;
    }
    // A genuine tail is represented *only* by `trailing_partial_bytes > 0`: `decode_ref_container_
    // records`'s own loop never pushes a `Failed` outcome for a terminal frame that is not
    // `never_a_tail` and has nothing sound after it (RFC 165 R5, §9.2) -- it returns
    // `trailing_partial_bytes` directly instead, the same as the too-short-for-a-header case. So
    // `discovery.has_damage()` and `trailing_partial_bytes > 0` are mutually exclusive by
    // construction; no separate check against `has_damage()` is needed here.
    let offset = bytes.len().checked_sub(discovery.trailing_partial_bytes)?;
    Some(RefLogContainerTail {
        offset,
        len: discovery.trailing_partial_bytes,
    })
}

/// Encode one signed RefUpdate envelope as a durable ref-log container record. `ref_name_key` is
/// supplied by the caller (already known from the decoded `RefUpdatePayload` at write time) rather
/// than re-derived here, so this function never has to decode its own input to frame it.
pub(in crate::refs) fn encode_ref_container_record(
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
) -> Result<Vec<u8>> {
    require_signed_type(envelope, ObjectType::RefUpdate)?;
    let body = encode_envelope_file(envelope)?;
    frame_record(ref_name_key, &body)
}

/// Frame one ref-container record from an envelope that is structurally, but not strictly, valid.
///
/// Test fixtures only: it is how a test plants a record the real writer would refuse to produce, so
/// that a reader's handling of such a record can be exercised at all.
#[cfg(test)]
pub(crate) fn encode_ref_container_record_for_test(
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
) -> Result<Vec<u8>> {
    require_signed_type(envelope, ObjectType::RefUpdate)?;
    let body = crate::foundation::file_codec::encode_envelope_file_structural(envelope)?;
    frame_record(ref_name_key, &body)
}

fn frame_record(ref_name_key: [u8; 32], body: &[u8]) -> Result<Vec<u8>> {
    let body_len = len_to_u64(body.len())?;
    let checksum = record_checksum(ref_name_key, body_len, body);
    let mut out = Vec::with_capacity(REF_CONTAINER_HEADER_LEN + body.len());
    out.extend_from_slice(REF_CONTAINER_MAGIC);
    push_u16(&mut out, REF_CONTAINER_VERSION);
    out.extend_from_slice(&ref_name_key);
    push_u64(&mut out, body_len);
    out.extend_from_slice(&checksum);
    out.extend_from_slice(body);
    Ok(out)
}

/// Result of attempting to parse one frame at a given offset. Mirrors `container::FrameAttempt`.
enum FrameAttempt {
    Record {
        record: RefContainerRecord,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
        claimed_ref_name_key: Option<[u8; 32]>,
        /// RFC 165 R5 (§9.2): see `RefContainerRecordStatus::Failed`'s own doc.
        never_a_tail: bool,
    },
}

/// Best-effort, not checksum-verified, read of the header's own `ref_name_key` field at its fixed
/// offset (10..42: magic(8) + version(2), then 32 bytes) -- usable even when `parse_header` itself
/// failed (a bad magic or version byte does not disturb this field's own bytes), the same reasoning
/// `trailing_tail_ref_name_key` already relies on for the torn-tail case.
fn raw_ref_name_key_at(header: &[u8]) -> Option<[u8; 32]> {
    header.get(10..42)?.try_into().ok()
}

/// Attempt to parse one ref-log container frame at `offset`. Never trusts a not-yet-checksum-validated
/// header's own `body_len` for anything beyond locating where its claimed body would end.
fn parse_frame_at(bytes: &[u8], offset: usize, budget: &mut ScanBudget) -> FrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < REF_CONTAINER_HEADER_LEN {
        return FrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + REF_CONTAINER_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_header(header) {
        Ok(values) => values,
        Err(err) => {
            // RFC 165 R5 (§9.2): a corrupted magic or version byte alone does not rule out a
            // complete, fully written record -- the checksum decides, computed with this format's
            // own real magic and version constants (`record_checksum` always uses the constants, never
            // whatever bytes are actually on disk at this offset).
            let never_a_tail = raw_ref_name_key_at(header).is_some_and(|key| {
                complete_by_checksum(
                    bytes,
                    offset,
                    REF_CONTAINER_HEADER_LEN,
                    budget,
                    |body_len, body| record_checksum(key, body_len, body),
                )
                .is_some()
            });
            return FrameAttempt::Invalid {
                message: err.to_string(),
                claimed_ref_name_key: None,
                never_a_tail,
            };
        }
    };
    let claimed = Some(header_values.ref_name_key);
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "ref container body length does not fit usize".to_string(),
            claimed_ref_name_key: claimed,
            never_a_tail: false,
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "ref container body end overflow".to_string(),
            claimed_ref_name_key: claimed,
            never_a_tail: false,
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        // RFC 165 R5 (§9.2): a corrupted length field can claim a body past the end of the file --
        // before conceding this is a torn tail, check whether the checksum verifies against the
        // length to the end of the file instead.
        let never_a_tail = complete_by_checksum(
            bytes,
            offset,
            REF_CONTAINER_HEADER_LEN,
            budget,
            |len, body| record_checksum(header_values.ref_name_key, len, body),
        )
        .is_some();
        if never_a_tail {
            return FrameAttempt::Invalid {
                message: "ref container record length claims more bytes than remain, but a \
                          complete record's own checksum verifies against the length to the end \
                          of the file"
                    .to_string(),
                claimed_ref_name_key: claimed,
                never_a_tail: true,
            };
        }
        return FrameAttempt::TrailingPartial { remaining };
    };
    budget.charge(body.len() as u64);
    let expected = record_checksum(header_values.ref_name_key, header_values.body_len, body);
    if expected != header_values.checksum {
        // RFC 165 R5 (§9.2): a complete record (full header, full claimed body) whose checksum
        // fails was fully written -- corruption, not a crash mid-write.
        return FrameAttempt::Invalid {
            message: format!("ref container checksum mismatch at byte offset {offset}"),
            claimed_ref_name_key: claimed,
            never_a_tail: true,
        };
    }
    let envelope = match decode_envelope_file(body) {
        Ok(envelope) => envelope,
        Err(err) => {
            // The checksum already verified above: a complete record whose envelope fails to decode
            // is damage, never a tail.
            return FrameAttempt::Invalid {
                message: err.to_string(),
                claimed_ref_name_key: claimed,
                never_a_tail: true,
            };
        }
    };
    if let Err(err) = require_signed_type(&envelope, ObjectType::RefUpdate) {
        return FrameAttempt::Invalid {
            message: err.to_string(),
            claimed_ref_name_key: claimed,
            never_a_tail: true,
        };
    }
    FrameAttempt::Record {
        record: RefContainerRecord {
            ref_name_key: header_values.ref_name_key,
            envelope,
        },
        next_offset: body_end,
    }
}

/// Isolate-and-continue reading (RFC 102 Stage 2's reader, reused here per the same discipline Stage
/// 3 already followed): a frame that fails to validate no longer aborts replay -- its offset and
/// error are recorded as a `Failed` outcome, and `frame_resync::resync_to_next_magic` finds the next
/// candidate frame so every subsequent sound record, for every ref, is still read.
pub(crate) fn decode_ref_container_records(bytes: &[u8]) -> Result<RefContainerReplay> {
    let mut records = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    let mut budget = ScanBudget::for_input(bytes.len());
    loop {
        if budget.exceeded() {
            record_outcomes.push(RefContainerRecordOutcome {
                offset,
                status: RefContainerRecordStatus::Failed {
                    message: scan_budget_exceeded_message(offset),
                    claimed_ref_name_key: None,
                    never_a_tail: false,
                },
            });
            return Ok(RefContainerReplay {
                records,
                trailing_partial_bytes: 0,
                record_outcomes,
            });
        }
        match parse_frame_at(bytes, offset, &mut budget) {
            FrameAttempt::Record {
                record,
                next_offset,
            } => {
                // RFC 102 Stage 4 checkpoint review, design-v1.md §13.15: checksum decides whether
                // this is a frame; envelope validation decides whether the record it contains is
                // admissible -- two different questions. A frame whose checksum matches but whose
                // envelope fails `validate_strict` is a real frame with a bad record, not a false
                // magic match, so it must not be routed through `resync_to_next_magic` (which would
                // scan past a genuine frame boundary and put every record after it at risk of being
                // lost or misattributed). Recorded as a per-record `Failed` outcome instead, offset
                // still advances to this frame's own already-known `next_offset`.
                match record.envelope.validate_strict() {
                    Ok(()) => {
                        record_outcomes.push(RefContainerRecordOutcome {
                            offset,
                            status: RefContainerRecordStatus::Evaluated,
                        });
                        records.push(record);
                    }
                    Err(err) => {
                        // The checksum, the envelope decode, and the signed-type check already
                        // passed (that is how a `FrameAttempt::Record` was reached at all): a
                        // complete record, damage, never a tail -- RFC 165 R5 (§9.2).
                        record_outcomes.push(RefContainerRecordOutcome {
                            offset,
                            status: RefContainerRecordStatus::Failed {
                                message: err.to_string(),
                                claimed_ref_name_key: Some(record.ref_name_key),
                                never_a_tail: true,
                            },
                        });
                    }
                }
                offset = require_progress("ref container", offset, next_offset)?;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                // RFC 160 F3: a torn tail is a prefix of ONE frame. If a sound frame starts in the remainder, this is damage.
                let sound_after = sound_frame_after_partial_budgeted(
                    bytes,
                    offset,
                    REF_CONTAINER_MAGIC.as_slice(),
                    &mut budget,
                    |c, budget| {
                        matches!(
                            parse_frame_at(bytes, c, budget),
                            FrameAttempt::Record { .. }
                        )
                    },
                );
                let next = match sound_after {
                    BoundedScan::NoSoundFrame => {
                        return Ok(RefContainerReplay {
                            records,
                            trailing_partial_bytes: remaining,
                            record_outcomes,
                        });
                    }
                    BoundedScan::Undetermined => {
                        record_outcomes.push(RefContainerRecordOutcome {
                            offset,
                            status: RefContainerRecordStatus::Failed {
                                message: scan_budget_exceeded_message(offset),
                                claimed_ref_name_key: None,
                                never_a_tail: false,
                            },
                        });
                        return Ok(RefContainerReplay {
                            records,
                            trailing_partial_bytes: 0,
                            record_outcomes,
                        });
                    }
                    BoundedScan::Sound(next) => next,
                };
                let message = partial_before_sound_frame_message(offset, next);
                let claimed = bytes.get(offset..).and_then(raw_ref_name_key_at);
                record_outcomes.push(RefContainerRecordOutcome {
                    offset,
                    status: RefContainerRecordStatus::Failed {
                        message,
                        claimed_ref_name_key: claimed,
                        never_a_tail: false,
                    },
                });
                offset = require_progress("ref container", offset, next)?;
            }
            FrameAttempt::Invalid {
                message,
                claimed_ref_name_key,
                never_a_tail,
            } => {
                // RFC 165 R5 (§9.2): a `never_a_tail` frame is damage unconditionally, so the scan
                // never checks whether a *sound* frame follows it (that question only matters for
                // deciding tail vs. damage, and this is already damage) -- it still tries a plain
                // magic-byte resync to keep reading whatever comes after, the same as before this
                // round. A frame that is not `never_a_tail` and has nothing sound after it is a
                // genuine tail: RFC 162 rule 3's own rule, unchanged -- no `Failed` outcome is
                // recorded for it at all, exactly like `TrailingPartial`'s own tail case above;
                // `trailing_partial_bytes` alone represents it.
                let sound_after = if never_a_tail {
                    None
                } else {
                    Some(sound_frame_after_partial_budgeted(
                        bytes,
                        offset,
                        REF_CONTAINER_MAGIC.as_slice(),
                        &mut budget,
                        |c, budget| {
                            matches!(
                                parse_frame_at(bytes, c, budget),
                                FrameAttempt::Record { .. }
                            )
                        },
                    ))
                };
                if matches!(sound_after, Some(BoundedScan::NoSoundFrame)) {
                    return Ok(RefContainerReplay {
                        records,
                        trailing_partial_bytes: bytes.len().saturating_sub(offset),
                        record_outcomes,
                    });
                }
                if matches!(sound_after, Some(BoundedScan::Undetermined)) {
                    record_outcomes.push(RefContainerRecordOutcome {
                        offset,
                        status: RefContainerRecordStatus::Failed {
                            message: format!("{message}; {}", scan_budget_exceeded_message(offset)),
                            claimed_ref_name_key,
                            never_a_tail,
                        },
                    });
                    return Ok(RefContainerReplay {
                        records,
                        trailing_partial_bytes: 0,
                        record_outcomes,
                    });
                }
                record_outcomes.push(RefContainerRecordOutcome {
                    offset,
                    status: RefContainerRecordStatus::Failed {
                        message,
                        claimed_ref_name_key,
                        never_a_tail,
                    },
                });
                let resumed = match sound_after {
                    Some(BoundedScan::Sound(next)) => Some(next),
                    _ => resync_to_next_magic(bytes, offset + 1, REF_CONTAINER_MAGIC.as_slice()),
                };
                match resumed {
                    Some(next) => offset = require_progress("ref container", offset, next)?,
                    None => {
                        return Ok(RefContainerReplay {
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

/// Durably append one record to the shared log container. **No pre-append refusal on any existing
/// trailing-partial tail** -- ruled in design-v1.md §13.6: a torn tail never enters any ref's own
/// filtered subsequence (`replay_ref_subsequence` below only ever sees frames that parsed), so
/// appending past one can never produce a sequence gap. Today's per-file refusal
/// (`refs/log.rs::append_log_record`) enforced hygiene, not integrity, and hygiene enforced this way
/// would mean one ref's crash blocks every other ref's publishes under a shared container -- exactly
/// the availability regression the ruling rejects. Mirrors `write_object_to_container`'s own
/// unconditional container-append exactly.
///
/// RFC 165 R1: `publish_locked` (this function's only production caller before this round) now calls
/// [`append_ref_container_record_with_replay`] instead, to reuse `classify_state`'s own read rather
/// than reading the container again here. This function has no production caller left -- `#[cfg(test)]`
/// matches its own sole re-export in `refs.rs`, so a non-test build never compiles it (and so never
/// flags it as dead code; its test-only fixture callers, `refs.rs::append_log_record_for_signature_test`
/// and `container/tests.rs`'s own direct calls, are unaffected).
#[cfg(test)]
pub(in crate::refs) fn append_ref_container_record(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
) -> Result<()> {
    let relative = require_sound_update(envelope, layout)?;
    let existing = replay_ref_subsequence(layout, ref_name_key)?;
    append_ref_container_record_against(layout, &relative, ref_name_key, &existing, envelope)?;
    Ok(())
}

/// RFC 165 R1: identical write behavior and identical `created_at == 0` enforcement to
/// [`append_ref_container_record`], but takes an already-loaded [`RefLogReplay`] instead of reading
/// the whole container again for the idempotency check -- the caller (`publish_locked`) reuses the
/// one read `classify_state`'s own replay already paid for (F1). Reports [`AppendOutcome`] so the
/// caller's own post-write check can be a ranged read of just the bytes this call wrote, not another
/// whole-container read (review v2 §2 item 2: a trusted return was rejected, a ranged read-back kept
/// `ensure_agreement`'s actual purpose -- did the bytes land as intended).
pub(in crate::refs) fn append_ref_container_record_with_replay(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
    preloaded: &RefLogReplay,
) -> Result<AppendOutcome> {
    let relative = require_sound_update(envelope, layout)?;
    append_ref_container_record_against(layout, &relative, ref_name_key, preloaded, envelope)
}

/// Decode and validate the `created_at == 0` requirement, and resolve the container's own relative
/// path -- the part [`append_ref_container_record`] and
/// [`append_ref_container_record_with_replay`] both need before either reads or writes anything.
fn require_sound_update(
    envelope: &ObjectEnvelope,
    layout: &RepositoryLayout,
) -> Result<std::path::PathBuf> {
    // RFC 102 Stage 4 checkpoint review, design-v1.md §13.15: format-2 requires `created_at == 0`
    // for a RefUpdate -- DC-39's implementation of a DC-34 ruling, carried from the retired
    // `refs/log.rs::append_log_record`'s own write-time check. Placed here, the one choke point
    // every publish path (`Ready`/`PointerLeading`/`Complete`) already goes through -- anything
    // upstream (e.g. `publish_locked`) is a layer a future caller could bypass, which is how this
    // check was lost in the first place.
    let update = RefUpdatePayload::decode_canonical(&envelope.canonical_payload)?;
    if update.created_at != 0 {
        return Err(PrikkError::MalformedData(
            "format-2 RefUpdate requires created_at == 0".to_string(),
        ));
    }
    layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )
}

/// What one append call actually did, so a caller's own post-write check knows what to verify.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::refs) enum AppendOutcome {
    /// The retry's own envelope already matched this ref's last record (idempotency, below) -- a
    /// zero-byte sync landed, nothing new was written, and the pre-write replay already confirmed
    /// this exact envelope is this ref's own current tip. Nothing new to range-read.
    AlreadyPresent,
    /// A new record was durably appended at `offset`, with exactly `bytes` on disk there -- the
    /// caller's own ranged read-back compares against these bytes, not a recomputed copy.
    Wrote { offset: u64, bytes: Vec<u8> },
}

fn append_ref_container_record_against(
    layout: &RepositoryLayout,
    relative: &std::path::Path,
    ref_name_key: [u8; 32],
    existing: &RefLogReplay,
    envelope: &ObjectEnvelope,
) -> Result<AppendOutcome> {
    // Idempotency, preserved from `refs/log.rs::append_log_record`'s exact behavior (retired, not
    // dropped): a retry whose own ref-scoped subsequence already ends in this exact envelope is a
    // no-op sync, not a second record -- `publish_locked`'s `PointerLeading`/`Complete` branches
    // call this unconditionally on every retry, and without this check a `Complete`-state retry
    // (pointer and log already agree) would append a genuine duplicate record.
    if existing
        .records
        .last()
        .is_some_and(|last| last.envelope == *envelope)
    {
        append_file_required(layout.repository_mutation_root(), relative, &[])?;
        return Ok(AppendOutcome::AlreadyPresent);
    }
    let record = encode_ref_container_record(ref_name_key, envelope)?;
    let offset = append_file_reporting_offset_required(
        layout.repository_mutation_root(),
        relative,
        &record,
    )?;
    Ok(AppendOutcome::Wrote {
        offset,
        bytes: record,
    })
}

/// Replay one ref's own subsequence from the shared container: every sound record whose header
/// claims `ref_name_key`, in relative physical order (Step 0 §13.1: `RefLock` already serializes one
/// ref's own writes, so this order is already correct sequence order for that ref specifically).
/// Reuses `refs::log::RefLogRecord`/`RefLogReplay`'s exact shape so `RefStore::replay_log`'s own
/// public return type never changes.
///
/// `trailing_partial_bytes` is **ref-scoped, not container-wide** (design-v1.md §13.6 point 2): a
/// torn tail at the container's own physical end is attributed to this ref only when enough of its
/// header survived to read a `ref_name_key` that matches -- an unattributable or foreign-ref tail
/// reports zero here, so this ref's own classification proceeds as if no partial tail exists (safe:
/// the retry append lands correctly regardless, per the ruling's own point 1; an unattributed tail
/// only loses the specific "N incomplete trailing byte(s)" diagnostic wording and the truncate-before-
/// retry hygiene step, never the underlying detection or recovery).
/// RFC 165 R1: read back exactly the bytes [`AppendOutcome::Wrote`] reported, for a caller's own
/// post-write check -- a ranged read (`read_file_range_if_exists`), never a whole-container one.
/// `None` only if the container itself is absent, which cannot happen for an offset an append into
/// it just reported.
pub(in crate::refs) fn read_back_ref_container_bytes(
    layout: &RepositoryLayout,
    offset: u64,
    len: usize,
) -> Result<Option<Vec<u8>>> {
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    crate::foundation::fsutil::read_file_range_if_exists(
        layout.repository_mutation_root(),
        &relative,
        offset,
        len,
    )
}

/// The ref log container, read and decoded once. `verify` shares one of these across every reader it
/// runs over the container (0.49.0 step 5 round 1 addendum F2): each decode of this shape costs about
/// nine times the file, so a second decode is a second full cost, not a cheap re-check.
pub(crate) struct DecodedRefLog {
    pub(crate) bytes: Vec<u8>,
    pub(crate) replay: RefContainerReplay,
}

/// One read and one decode of the ref log container, or `None` when the file does not exist. `scope`
/// names the whole-read guard scope this read belongs to (`verify-scan` for `verify`'s shared read).
pub(crate) fn read_and_decode_ref_log(
    layout: &RepositoryLayout,
    scope: &'static str,
) -> Result<Option<DecodedRefLog>> {
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare(scope);
    #[cfg(not(test))]
    let _ = scope;
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(None);
    };
    let replay = decode_ref_container_records(&bytes)?;
    Ok(Some(DecodedRefLog { bytes, replay }))
}

pub(in crate::refs) fn replay_ref_subsequence(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
) -> Result<RefLogReplay> {
    match read_and_decode_ref_log(layout, "ref-log-replay")? {
        Some(decoded) => ref_subsequence_of(&decoded, ref_name_key),
        None => Ok(RefLogReplay {
            records: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        }),
    }
}

/// One ref's own subsequence of an already-decoded ref log: [`replay_ref_subsequence`]'s body, without
/// its own read or decode, so `verify` can replay every key from the one decode it already has.
pub(in crate::refs) fn ref_subsequence_of(
    decoded: &DecodedRefLog,
    ref_name_key: [u8; 32],
) -> Result<RefLogReplay> {
    let bytes = &decoded.bytes;
    let replay = &decoded.replay;
    let mut records = replay.records.iter();
    let mut ref_records = Vec::new();
    let mut ref_outcomes = Vec::new();
    for outcome in &replay.record_outcomes {
        match &outcome.status {
            RefContainerRecordStatus::Evaluated => {
                let Some(record) = records.next() else {
                    return Err(PrikkError::Integrity(
                        "ref container replay outcome/record count mismatch".to_string(),
                    ));
                };
                if record.ref_name_key != ref_name_key {
                    continue;
                }
                ref_outcomes.push(RefLogRecordOutcome {
                    offset: outcome.offset,
                    status: RefLogRecordStatus::Evaluated,
                });
                ref_records.push(RefLogRecord {
                    envelope: record.envelope.clone(),
                });
            }
            RefContainerRecordStatus::Failed {
                message,
                claimed_ref_name_key,
                ..
            } => {
                if *claimed_ref_name_key != Some(ref_name_key) {
                    continue;
                }
                ref_outcomes.push(RefLogRecordOutcome {
                    offset: outcome.offset,
                    status: RefLogRecordStatus::Failed {
                        message: message.clone(),
                    },
                });
            }
        }
    }
    let attributed_trailing = trailing_tail_ref_name_key(bytes, replay.trailing_partial_bytes);
    let trailing_partial_bytes = if attributed_trailing == Some(ref_name_key) {
        replay.trailing_partial_bytes
    } else {
        0
    };
    Ok(RefLogReplay {
        records: ref_records,
        trailing_partial_bytes,
        record_outcomes: ref_outcomes,
    })
}

/// Best-effort attribution of the container's own trailing partial tail: read just enough of its
/// header to learn the `ref_name_key` it claims, when enough bytes survive to reach that field at
/// all. Not checksum-verified (a torn tail's checksum is, by construction, never fully present to
/// verify) -- see `replay_ref_subsequence`'s own doc for why this is safe to use for classification
/// despite that.
fn trailing_tail_ref_name_key(bytes: &[u8], trailing_partial_bytes: usize) -> Option<[u8; 32]> {
    if trailing_partial_bytes == 0 {
        return None;
    }
    let start = bytes.len().checked_sub(trailing_partial_bytes)?;
    let key_start = start.checked_add(10)?;
    let key_end = key_start.checked_add(32)?;
    bytes
        .get(key_start..key_end)
        .map(|slice| slice.try_into().unwrap_or([0_u8; 32]))
}

/// Return whether the container's own trailing partial suffix is an exact prefix of the record
/// `expected` would produce if appended now under `ref_name_key`. Mirrors
/// `refs::log::incomplete_tail_matches`, generalized from "the one file this ref owns" to "the
/// container's own physical tail".
pub(in crate::refs) fn incomplete_tail_matches(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    expected: &ObjectEnvelope,
) -> Result<bool> {
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let bytes =
        read_file_if_exists(layout.repository_mutation_root(), &relative)?.unwrap_or_default();
    let replay = decode_ref_container_records(&bytes)?;
    if replay.trailing_partial_bytes == 0 {
        return Ok(false);
    }
    let retained = bytes
        .len()
        .checked_sub(replay.trailing_partial_bytes)
        .ok_or_else(|| {
            PrikkError::Integrity("ref container retained length underflow".to_string())
        })?;
    let expected_record = encode_ref_container_record(ref_name_key, expected)?;
    let suffix = bytes.get(retained..).ok_or_else(|| {
        PrikkError::Integrity("ref container incomplete suffix range overflow".to_string())
    })?;
    Ok(expected_record.starts_with(suffix))
}

/// Truncate only a structurally incomplete final frame from the shared container and required-sync
/// the retained bytes. Safe regardless of which ref (if any) the torn tail is attributable to
/// (design-v1.md §13.6 point 3): "trailing" already means "past the last fully-parseable frame", so
/// nothing sound is ever removed. Mirrors `refs::log::truncate_incomplete_tail`, generalized from a
/// per-ref file to the shared container.
pub(in crate::refs) fn truncate_incomplete_tail(layout: &RepositoryLayout) -> Result<usize> {
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let bytes =
        read_file_if_exists(layout.repository_mutation_root(), &relative)?.unwrap_or_default();
    let replay = decode_ref_container_records(&bytes)?;
    if replay.trailing_partial_bytes == 0 {
        return Ok(0);
    }
    let retained = bytes
        .len()
        .checked_sub(replay.trailing_partial_bytes)
        .ok_or_else(|| {
            PrikkError::Integrity("ref container retained length underflow".to_string())
        })?;
    crate::foundation::fsutil::truncate_existing_file_required(
        layout.repository_mutation_root(),
        &relative,
        u64::try_from(retained)
            .map_err(|_| PrikkError::Integrity("ref container length exceeds u64".to_string()))?,
    )?;
    Ok(replay.trailing_partial_bytes)
}

/// Append an attributable torn tail: encode `envelope` under `ref_name_key` exactly as a real
/// publish would, then append only a truncated prefix of it (past the header, short of the full
/// frame) -- the appended bytes carry a genuine, correctly-attributed `ref_name_key` without
/// depending on any record already being durably present (a first-ever publish interrupted at its
/// own log append has none). Fixture construction only -- see the CLI-side equivalent
/// (`prikk-cli/tests/support/mod.rs::append_torn_ref_log_tail`, which instead duplicates whichever
/// real record already sits last in the container, since CLI tests have no in-crate encoder) for why
/// bare garbage bytes no longer simulate "this ref's own torn write" under the shared container: a
/// tail shorter than `REF_CONTAINER_HEADER_LEN` cannot be attributed to any ref at all.
#[cfg(test)]
pub(crate) fn append_torn_ref_log_tail_for_test(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
) -> Result<()> {
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let full = encode_ref_container_record_for_test(ref_name_key, envelope)?;
    let torn_len = (REF_CONTAINER_HEADER_LEN + 8).min(full.len().saturating_sub(1));
    let torn = full.get(..torn_len).ok_or_else(|| {
        PrikkError::Integrity("torn tail length exceeds encoded record".to_string())
    })?;
    crate::foundation::fsutil::append_file_required(
        layout.repository_mutation_root(),
        &relative,
        torn,
    )
}

struct RefContainerHeader {
    ref_name_key: [u8; 32],
    body_len: u64,
    checksum: [u8; 32],
}

fn parse_header(header: &[u8]) -> Result<RefContainerHeader> {
    let mut cursor = ByteCursor::new(header);
    let magic = cursor.read_array::<8>()?;
    if &magic != REF_CONTAINER_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid ref container record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != REF_CONTAINER_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let ref_name_key = cursor.read_array::<32>()?;
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in ref container header".to_string(),
        ));
    }
    Ok(RefContainerHeader {
        ref_name_key,
        body_len,
        checksum,
    })
}

fn record_checksum(ref_name_key: [u8; 32], body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(REF_CONTAINER_MAGIC);
    preimage.extend_from_slice(&REF_CONTAINER_VERSION.to_be_bytes());
    preimage.extend_from_slice(&ref_name_key);
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    tallied_sha256(&preimage)
}

#[cfg(test)]
mod tests;
