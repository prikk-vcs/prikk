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
use prikk_hash::sha256;
use prikk_object::{ObjectEnvelope, ObjectType, RefUpdatePayload};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_u16, push_u64,
};
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{append_file_required, len_to_u64, read_file_if_exists};
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefContainerRecord {
    /// This record's own header-carried ref-name key -- trusted (the frame's checksum covers it),
    /// since this variant is only ever produced for a frame that passed checksum validation.
    pub(crate) ref_name_key: [u8; 32],
    /// Exact signed RefUpdate envelope stored at append time.
    pub(crate) envelope: ObjectEnvelope,
}

/// Outcome of attempting to decode one ref-log container record frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RefContainerRecordStatus {
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
    },
}

/// One attempted ref-log container record frame's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefContainerRecordOutcome {
    /// The byte offset within the container this frame attempt started at.
    pub(crate) offset: usize,
    /// How this frame's own read/validation resolved.
    pub(crate) status: RefContainerRecordStatus,
}

/// Ref-log container replay result -- every ref's records, interleaved, in physical (write) order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefContainerReplay {
    /// Valid records read from the container, in file order -- includes records found after a
    /// damaged one, not merely a prefix up to the first failure.
    pub(crate) records: Vec<RefContainerRecord>,
    /// Number of trailing bytes ignored as an incomplete final record.
    pub(crate) trailing_partial_bytes: usize,
    /// One outcome per attempted frame, in scan order -- both `Evaluated` and `Failed`.
    pub(crate) record_outcomes: Vec<RefContainerRecordOutcome>,
}

/// Encode one signed RefUpdate envelope as a durable ref-log container record. `ref_name_key` is
/// supplied by the caller (already known from the decoded `RefUpdatePayload` at write time) rather
/// than re-derived here, so this function never has to decode its own input to frame it.
pub(crate) fn encode_ref_container_record(
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
) -> Result<Vec<u8>> {
    require_signed_type(envelope, ObjectType::RefUpdate)?;
    let body = encode_envelope_file(envelope)?;
    frame_record(ref_name_key, &body)
}

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
    },
}

/// Attempt to parse one ref-log container frame at `offset`. Never trusts a not-yet-checksum-validated
/// header's own `body_len` for anything beyond locating where its claimed body would end.
fn parse_frame_at(bytes: &[u8], offset: usize) -> FrameAttempt {
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
            return FrameAttempt::Invalid {
                message: err.to_string(),
                claimed_ref_name_key: None,
            };
        }
    };
    let claimed = Some(header_values.ref_name_key);
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "ref container body length does not fit usize".to_string(),
            claimed_ref_name_key: claimed,
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "ref container body end overflow".to_string(),
            claimed_ref_name_key: claimed,
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let expected = record_checksum(header_values.ref_name_key, header_values.body_len, body);
    if expected != header_values.checksum {
        return FrameAttempt::Invalid {
            message: format!("ref container checksum mismatch at byte offset {offset}"),
            claimed_ref_name_key: claimed,
        };
    }
    let envelope = match decode_envelope_file(body) {
        Ok(envelope) => envelope,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
                claimed_ref_name_key: claimed,
            };
        }
    };
    if let Err(err) = require_signed_type(&envelope, ObjectType::RefUpdate) {
        return FrameAttempt::Invalid {
            message: err.to_string(),
            claimed_ref_name_key: claimed,
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
    loop {
        match parse_frame_at(bytes, offset) {
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
                        record_outcomes.push(RefContainerRecordOutcome {
                            offset,
                            status: RefContainerRecordStatus::Failed {
                                message: err.to_string(),
                                claimed_ref_name_key: Some(record.ref_name_key),
                            },
                        });
                    }
                }
                offset = next_offset;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                return Ok(RefContainerReplay {
                    records,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            FrameAttempt::Invalid {
                message,
                claimed_ref_name_key,
            } => {
                record_outcomes.push(RefContainerRecordOutcome {
                    offset,
                    status: RefContainerRecordStatus::Failed {
                        message,
                        claimed_ref_name_key,
                    },
                });
                match resync_to_next_magic(bytes, offset + 1, REF_CONTAINER_MAGIC.as_slice()) {
                    Some(next) => offset = next,
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
pub(crate) fn append_ref_container_record(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    envelope: &ObjectEnvelope,
) -> Result<()> {
    // RFC 102 Stage 4 checkpoint review, design-v1.md §13.15: format-2 requires `created_at == 0`
    // for a RefUpdate -- DC-39's implementation of a DC-34 ruling, carried from the retired
    // `refs/log.rs::append_log_record`'s own write-time check. Placed here, the append function
    // itself, because it is the one choke point every publish path (`Ready`/`PointerLeading`/
    // `Complete`) already goes through -- anything upstream (e.g. `publish_locked`) is a layer a
    // future caller could bypass, which is how this check was lost in the first place.
    let update = RefUpdatePayload::decode_canonical(&envelope.canonical_payload)?;
    if update.created_at != 0 {
        return Err(PrikkError::MalformedData(
            "format-2 RefUpdate requires created_at == 0".to_string(),
        ));
    }
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    // Idempotency, preserved from `refs/log.rs::append_log_record`'s exact behavior (retired, not
    // dropped): a retry whose own ref-scoped subsequence already ends in this exact envelope is a
    // no-op sync, not a second record -- `publish_locked`'s `PointerLeading`/`Complete` branches
    // call this unconditionally on every retry, and without this check a `Complete`-state retry
    // (pointer and log already agree) would append a genuine duplicate record.
    let existing = replay_ref_subsequence(layout, ref_name_key)?;
    if existing
        .records
        .last()
        .is_some_and(|last| last.envelope == *envelope)
    {
        return append_file_required(layout.repository_mutation_root(), &relative, &[]);
    }
    let record = encode_ref_container_record(ref_name_key, envelope)?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
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
pub(crate) fn replay_ref_subsequence(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
) -> Result<RefLogReplay> {
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(RefLogReplay {
            records: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        });
    };
    let replay = decode_ref_container_records(&bytes)?;
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
    let attributed_trailing = trailing_tail_ref_name_key(&bytes, replay.trailing_partial_bytes);
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
pub(crate) fn incomplete_tail_matches(
    layout: &RepositoryLayout,
    ref_name_key: [u8; 32],
    expected: &ObjectEnvelope,
) -> Result<bool> {
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
pub(crate) fn truncate_incomplete_tail(layout: &RepositoryLayout) -> Result<usize> {
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
    sha256(&preimage)
}

#[cfg(test)]
mod tests;
