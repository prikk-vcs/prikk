//! Generation log framing and resolution (RFC 102 Stage 6 Step 1, design-v1.md §15.4/§15.6): a small,
//! per-container fixed-name log recording which slot (`A` or `B`) is currently authoritative. Readers
//! take the last complete generation record -- design-v1.md §4's own publish mechanism. Step 1 never
//! writes one (that is Step 2's compactor), so every resolver call in this stage returns `A`: an
//! empty log has no record to take, and `A` is the slot every write already targets, matching the
//! handoff's own "no behaviour change" acceptance criterion for this step.
//!
//! **One log per compacting container** (`ref_pointer_index`, `received_index`,
//! `trust_policy_container`), never shared -- confining a corrupt record's blast radius to the one
//! container it names, the reasoning design-v1.md §15.6 gave for rejecting the pre-existing global
//! `container_generation_log_path()` as this stage's mechanism.
//!
//! **Fail-closed on a damaged record, not silently stale.** The same "last entry wins, so a damaged
//! latest entry must not be silently skipped" reasoning already established for the ref pointer index
//! (design-v1.md §13.14) and the trust policy snapshot container (§14.9), applied here at the
//! generation-pointer level: resolving to an older, damaged-but-plausible generation would let a
//! reader silently address the wrong slot.
//!
//! **`append_generation_record` is Stage 6 Step 2's own production write path.** Step 1 shipped only
//! the decoder and resolver, `#[cfg(test)]`-gating the encoder -- there was no production caller yet,
//! and building one without a real caller would have been exactly the "orphan `pub(crate)`, no real
//! caller before merge" shape Stage 5 round 1's review flagged. Step 2's compactor (`compact.rs`) is
//! that caller now.
//!
//! **When the log itself names no live slot but the non-default slot holds data, content decides it
//! (0.50.0 step 1 Part E2), never a refusal.** `resolve_or_deduce`'s own doc has the two rules; a
//! refusal fires only when the deduction itself cannot be made (either slot is damaged).

use prikk_error::{PrikkError, Result};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::push_u16;
use crate::foundation::frame_resync::{
    ScanBudget, complete_by_checksum, partial_before_sound_frame_message, require_progress,
    resync_to_next_magic, sound_frame_after_partial, tallied_sha256,
};
use crate::foundation::fsutil::{append_file_required, read_file_if_exists};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};

/// A generation record's body is one byte: the live slot's code.
const GENERATION_BODY_LEN: usize = 1;
const GENERATION_MAGIC: &[u8; 8] = b"PGENREC1";
const GENERATION_VERSION: u16 = 1;
const GENERATION_HEADER_LEN: usize = 8 + 2 + 8 + 32;

/// One generation record: which slot is authoritative as of this append.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct GenerationRecord {
    pub(crate) live_slot: ContainerSlot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GenerationRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenerationRecordOutcome {
    pub(crate) offset: usize,
    pub(crate) status: GenerationRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GenerationReplay {
    pub(crate) records: Vec<GenerationRecord>,
    pub(crate) trailing_partial_bytes: usize,
    /// The byte offset where `trailing_partial_bytes` begins (RFC 163 §9's refusal names it). Only
    /// meaningful when `trailing_partial_bytes != 0`.
    pub(crate) tail_offset: usize,
    pub(crate) record_outcomes: Vec<GenerationRecordOutcome>,
}

impl GenerationReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, GenerationRecordStatus::Failed { .. }))
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

fn generation_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(GENERATION_MAGIC);
    preimage.extend_from_slice(&GENERATION_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    tallied_sha256(&preimage)
}

/// Encode one generation record. Promoted from Step 1's `#[cfg(test)]`-only helper (`encode_
/// generation_record_for_test`) now that Step 2's compactor is the real production caller Step 1's
/// own doc anticipated -- see `append_generation_record`, the only production call site.
pub(crate) fn encode_generation_record(record: &GenerationRecord) -> Vec<u8> {
    let body = vec![slot_code(record.live_slot)];
    let body_len = u64::try_from(body.len()).unwrap_or(0);
    let checksum = generation_checksum(body_len, &body);
    let mut out = Vec::with_capacity(GENERATION_HEADER_LEN + body.len());
    out.extend_from_slice(GENERATION_MAGIC);
    push_u16(&mut out, GENERATION_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    out
}

/// Durably append one generation record -- the compaction publish moment itself (design-v1.md §15.6
/// item 3/§4, criterion 1: "the compactor publishes by appending a generation record, after the new
/// slot's bytes are durable"). The caller is responsible for that ordering; this function only
/// guarantees the append itself is durable once called, the same as every other container's own
/// append primitive.
pub(crate) fn append_generation_record(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
    record: &GenerationRecord,
) -> Result<()> {
    let relative = layout.repository_relative(generation_log_path)?;
    let bytes = encode_generation_record(record);
    append_file_required(layout.repository_mutation_root(), &relative, &bytes)
}

fn decode_generation_body(body: &[u8]) -> Result<GenerationRecord> {
    let mut cursor = ByteCursor::new(body);
    let code = cursor.read_array::<1>()?[0];
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in generation record body".to_string(),
        ));
    }
    Ok(GenerationRecord {
        live_slot: slot_from_code(code)?,
    })
}

enum GenerationFrameAttempt {
    Record {
        record: GenerationRecord,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
        /// RFC 160 F3 and RFC 164 §9: a fixed-width record's header states its width; any other
        /// length is malformed by construction, whatever bytes follow. A complete record (the
        /// claimed one-byte body present) whose checksum or envelope fails was fully written --
        /// corruption, not a crash. Neither is the harmless remnant of an interrupted append, so
        /// both are excluded from Rule A's "damage only if a sound record follows" check
        /// (`decode_generation_records`'s own `Invalid` arm) and stay a failed item unconditionally.
        never_a_tail: bool,
    },
}

fn parse_generation_frame_at(
    bytes: &[u8],
    offset: usize,
    budget: &mut ScanBudget,
) -> GenerationFrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < GENERATION_HEADER_LEN {
        return GenerationFrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + GENERATION_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return GenerationFrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_generation_header(header) {
        Ok(values) => values,
        Err(err) => {
            // RFC 164 §9.2: a corrupted magic or version byte alone does not rule out a complete,
            // fully written record -- the checksum decides, computed with this format's own real
            // magic and version.
            if complete_by_checksum(
                bytes,
                offset,
                GENERATION_HEADER_LEN,
                budget,
                generation_checksum,
            )
            .is_some()
            {
                return GenerationFrameAttempt::Invalid {
                    message: format!("{err}, but a complete record's own checksum verifies"),
                    never_a_tail: true,
                };
            }
            return GenerationFrameAttempt::Invalid {
                message: err.to_string(),
                never_a_tail: false,
            };
        }
    };
    // RFC 160 F3: a fixed-width record's header states its width; any other length is malformed, never a torn tail.
    if u64::try_from(GENERATION_BODY_LEN).ok() != Some(header_values.0) {
        return GenerationFrameAttempt::Invalid {
            message: format!(
                "generation record at byte offset {offset} claims a body of {} bytes, but every generation record's body is exactly {GENERATION_BODY_LEN}",
                header_values.0
            ),
            never_a_tail: true,
        };
    }
    let Ok(body_len) = usize::try_from(header_values.0) else {
        return GenerationFrameAttempt::Invalid {
            message: "generation record body length does not fit usize".to_string(),
            never_a_tail: true,
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return GenerationFrameAttempt::Invalid {
            message: "generation record body end overflow".to_string(),
            never_a_tail: true,
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return GenerationFrameAttempt::TrailingPartial { remaining };
    };
    budget.charge((body.len() + GENERATION_HEADER_LEN) as u64);
    let expected = generation_checksum(header_values.0, body);
    if expected != header_values.1 {
        // RFC 164 §9: a complete record (full header, full one-byte body) whose checksum fails was
        // fully written -- corruption, not a crash mid-write.
        return GenerationFrameAttempt::Invalid {
            message: format!("generation record checksum mismatch at byte offset {offset}"),
            never_a_tail: true,
        };
    }
    match decode_generation_body(body) {
        Ok(record) => GenerationFrameAttempt::Record {
            record,
            next_offset: body_end,
        },
        Err(err) => GenerationFrameAttempt::Invalid {
            message: err.to_string(),
            never_a_tail: true,
        },
    }
}

fn parse_generation_header(header: &[u8]) -> Result<(u64, [u8; 32])> {
    let mut cursor = ByteCursor::new(header);
    let magic = cursor.read_array::<8>()?;
    if &magic != GENERATION_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid generation record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != GENERATION_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in generation record header".to_string(),
        ));
    }
    Ok((body_len, checksum))
}

/// Isolate-and-continue reading, matching every other container's decode loop in this codebase.
pub(crate) fn decode_generation_records(bytes: &[u8]) -> Result<GenerationReplay> {
    let mut records = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    let mut budget = ScanBudget::for_input(bytes.len());
    loop {
        match parse_generation_frame_at(bytes, offset, &mut budget) {
            GenerationFrameAttempt::Record {
                record,
                next_offset,
            } => {
                record_outcomes.push(GenerationRecordOutcome {
                    offset,
                    status: GenerationRecordStatus::Evaluated,
                });
                records.push(record);
                offset = require_progress("generation", offset, next_offset)?;
            }
            GenerationFrameAttempt::TrailingPartial { remaining } => {
                // RFC 160 F3: a torn tail is a prefix of ONE frame. If a sound frame starts in the remainder, this is damage.
                let sound_after =
                    sound_frame_after_partial(bytes, offset, GENERATION_MAGIC.as_slice(), |c| {
                        matches!(
                            parse_generation_frame_at(
                                bytes,
                                c,
                                &mut ScanBudget::for_input(bytes.len())
                            ),
                            GenerationFrameAttempt::Record { .. }
                        )
                    });
                let Some(next) = sound_after else {
                    return Ok(GenerationReplay {
                        records,
                        trailing_partial_bytes: remaining,
                        tail_offset: offset,
                        record_outcomes,
                    });
                };
                let message = partial_before_sound_frame_message(offset, next);
                record_outcomes.push(GenerationRecordOutcome {
                    offset,
                    status: GenerationRecordStatus::Failed { message },
                });
                offset = require_progress("generation", offset, next)?;
            }
            GenerationFrameAttempt::Invalid {
                message,
                never_a_tail,
            } => {
                // RFC 164 Rule A: an invalid frame is damage only if a sound frame follows it
                // somewhere in the rest of the buffer -- otherwise this frame and everything after
                // it is a tail, whatever its shape (zeros, random bytes), the same rule RFC 162
                // rule 3 already gives the WAL and the pointer index. Except a claim this format's
                // own records can never make (`never_a_tail`), which stays damage unconditionally,
                // resyncing past it like any other permanently-failed frame.
                let sound_after = (!never_a_tail)
                    .then(|| {
                        sound_frame_after_partial(bytes, offset, GENERATION_MAGIC.as_slice(), |c| {
                            matches!(
                                parse_generation_frame_at(
                                    bytes,
                                    c,
                                    &mut ScanBudget::for_input(bytes.len())
                                ),
                                GenerationFrameAttempt::Record { .. }
                            )
                        })
                    })
                    .flatten();
                if sound_after.is_none() && !never_a_tail {
                    return Ok(GenerationReplay {
                        records,
                        trailing_partial_bytes: bytes.len().saturating_sub(offset),
                        tail_offset: offset,
                        record_outcomes,
                    });
                }
                record_outcomes.push(GenerationRecordOutcome {
                    offset,
                    status: GenerationRecordStatus::Failed { message },
                });
                let resumed = match sound_after {
                    Some(next) => Some(next),
                    None => resync_to_next_magic(bytes, offset + 1, GENERATION_MAGIC.as_slice()),
                };
                match resumed {
                    Some(next) => offset = require_progress("generation", offset, next)?,
                    None => {
                        return Ok(GenerationReplay {
                            records,
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

/// RFC 164 Rule B: `verify`'s own reporting reads this directly (never refusing on interior damage
/// itself -- that is `resolve_live_slot_with_tail`'s own job for a real reader) so a tail or damaged
/// record is *reported*, not merely made to fail whatever incidentally reads the log next.
pub(crate) fn replay_generation_log(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
) -> Result<GenerationReplay> {
    let relative = layout.repository_relative(generation_log_path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(GenerationReplay {
            records: Vec::new(),
            trailing_partial_bytes: 0,
            tail_offset: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_generation_records(&bytes)
}

/// Resolve which slot is currently live for one compacting container's generation log. `A` when no
/// generation record has ever been written -- Step 1's only reachable outcome, since nothing appends
/// one yet. Fails closed on a damaged record rather than silently resolving to an older, stale
/// generation (see module doc). A reader: never refuses on the log's own trailing-partial tail (RFC
/// 163 §9's write-side guard, below, is what does that) -- every ordinary append to the container this
/// log names (a publication, a trust-policy snapshot, a received-ref import) resolves its target slot
/// through this function and must stay unaffected by a generation-log tail that has nothing to do with
/// it.
///
/// **0.50.0 step 1 Part E (019 §5.7): an empty or absent log is trusted to mean slot A only while
/// slot B itself holds no data.** Confirmed from source (`compact.rs` and `pointer_rebuild.rs` are
/// this repository's only two writers of a generation-aware container's "other" slot, each paired
/// with the one `append_generation_record` call that is this fact's own durable record): slot B is
/// never written except alongside a generation record naming it live. So an empty log *and* a
/// non-empty slot B can only mean one thing -- a compaction (or, for the pointer index, a rebuild)
/// happened and its own record of that fact was lost afterward, not that one never happened. Trusting
/// slot A in that state would silently serve stale data to every reader and let every writer append
/// behind it. `slot_b_path`, `container_label`, and `way_out` let every one of this function's three
/// callers (the pointer index, the received index, the trust policy) name itself and its own route
/// out in the refusal, the same shape `require_no_unclean_tail` already uses.
/// Whether this compacting container's generation log has lost the record of a compaction that
/// genuinely happened: the log itself names no live slot (empty, absent, or a pure tail -- not
/// interior damage, which [`resolve_live_slot`] already refuses on its own), yet slot B holds data
/// no compaction-free history could have put there. Exposed separately from [`resolve_live_slot`]
/// for `pointer_rebuild.rs`'s own use (0.50.0 step 1 Part E): the pointer-index rebuild's whole point
/// is to re-derive it from the ref log without trusting either slot as live, so it must detect this
/// state itself and deliberately bypass the resolver's own refusal, rather than propagate it the way
/// every other reader and writer of this container must.
pub(crate) fn generation_log_lost(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
    slot_b_path: &std::path::Path,
) -> Result<bool> {
    let replay = replay_generation_log(layout, generation_log_path)?;
    if replay.has_item_failure() || !replay.records.is_empty() {
        return Ok(false);
    }
    slot_b_has_data(layout, slot_b_path)
}

/// A stat, not a read: slot B is itself a store-growing file, and only its size -- not its content
/// -- is needed to tell "never written" apart from "holds data" (RFC 102's append-length round;
/// `whole_read_guard` catches exactly this class of read in tests, which is how this function's
/// first version -- a whole read -- was actually caught).
fn slot_b_has_data(layout: &RepositoryLayout, slot_b_path: &std::path::Path) -> Result<bool> {
    let relative = layout.repository_relative(slot_b_path)?;
    Ok(crate::foundation::fsutil::stat_file_state_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    .is_some_and(|stat| stat.size != 0))
}

/// What one slot decoded to, for Part E2's content-based deduction: the entries themselves (compared
/// by value, never by raw bytes -- compaction re-encodes from scratch, so two logically identical
/// entries need not be byte-identical), and whether the decode itself hit a damaged record.
pub(crate) struct DecodedEntries<T> {
    pub(crate) entries: Vec<T>,
    pub(crate) damaged: bool,
}

/// Which of the two content-deciding rules resolved the slot, named for `verify`/`doctor`'s own
/// warning text -- never for an ordinary reader or writer, which get the deduced slot silently.
pub(crate) enum DeductionReason {
    /// Slot B's own decoded entries equal `compaction(P)`, or a prefix of it, for some prefix `P` of
    /// slot A's own entries: a crash before the generation record landed (`P` = all of A at that
    /// point), a partly written B, a lost log with nothing written since, or a crash followed by
    /// ordinary writes to A (`P` = A as it stood at the crash) -- either way B is derived from A, and
    /// A is live.
    BIsDerivedFromSomePrefixOfA,
    /// Slot B's own decoded entries match no prefix of A this way: B took writes after becoming live,
    /// so only the record of that switch is missing.
    BTookWritesAfterBecomingLive,
}

impl DeductionReason {
    pub(crate) fn explain(&self) -> &'static str {
        match self {
            Self::BIsDerivedFromSomePrefixOfA => {
                "the other slot's entries equal, or are a prefix of, what compaction would have \
                 written at some earlier point in this one's own history, so a crash happened before \
                 the switch was recorded, the write was interrupted, or nothing live-affecting was \
                 written since"
            }
            Self::BTookWritesAfterBecomingLive => {
                "the other slot's entries match no earlier point in this one's own history, so it \
                 took writes after becoming live"
            }
        }
    }
}

/// The outcome of deducing a live slot from content rather than reading it off the log (Part E2):
/// which slot, and why.
pub(crate) struct DeducedFromContent {
    pub(crate) slot: ContainerSlot,
    pub(crate) reason: DeductionReason,
}

/// Resolve the live slot, or -- when the log names none but slot B holds data -- deduce it from the
/// two slots' own entries rather than refuse (Part E2, correcting Part E's own ruling: the refusal was
/// file-identical to an ordinary compaction crash, and blocked every reader and writer on it, with no
/// way out for the received index or the trust policy beyond a backup).
///
/// **Part E3 corrected the deduction once**: a membership test ("every entry B holds is somewhere in
/// A's history") is unsound when entries can repeat -- the trust policy's snapshot entries are a full
/// `{key_ids}` set with no sequence, so an earlier-then-reinstated set can reappear (the review's own
/// un-revocation sequence). Comparing against `C = compaction(A)` positionally fixed that.
///
/// **Part E4 corrects it again**: `compaction(A)`, A *as it is now*, is still wrong after a crash
/// that leaves A live and taking further ordinary writes (a new branch, a revocation) -- B was made
/// from an *earlier* A, and comparing against the current one can resolve to B, the stale slot,
/// losing those writes. The sound test: **B is derived from A if B equals `compaction(P)`, or a
/// prefix of it, for some prefix `P` of A's own entries** -- not only the full A. Computed in one
/// pass over A's entries, maintaining the running reduction `fold_entry` builds incrementally (the
/// same step `compact`'s own reduction takes, exposed so the two can never drift) and testing B
/// against it after every entry -- recomputing the whole reduction from scratch for each candidate
/// `P` would make this quadratic, not linear. `decode_entries` and `fold_entry` are the two pieces
/// only the caller can supply -- each compacting container's own entry type, decoder, and reduction
/// step -- so this stays generic over them rather than this module importing three sibling modules'
/// types.
fn resolve_or_deduce<T: PartialEq>(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
    slot_a_path: &std::path::Path,
    slot_b_path: &std::path::Path,
    damage_text: &str,
    decode_entries: &impl Fn(&[u8]) -> Result<DecodedEntries<T>>,
    fold_entry: &impl Fn(&mut Vec<T>, T),
) -> Result<(ContainerSlot, usize, usize, Option<DeducedFromContent>)> {
    let replay = replay_generation_log(layout, generation_log_path)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "generation log has a damaged record; run doctor before reading".to_string(),
        ));
    }
    if let Some(record) = replay.records.last() {
        return Ok((
            record.live_slot,
            replay.trailing_partial_bytes,
            replay.tail_offset,
            None,
        ));
    }
    // 0.50.0 step 1 Part E/E2: no record at all -- trust slot A unless slot B itself holds data,
    // which only a since-lost generation record could explain.
    if !slot_b_has_data(layout, slot_b_path)? {
        return Ok((
            ContainerSlot::A,
            replay.trailing_partial_bytes,
            replay.tail_offset,
            None,
        ));
    }
    // Ambiguous: deduce from the slots' own entries (Part E2) rather than refuse a state content can
    // decide. A whole read of each slot, declared: rare (only this ambiguous state reaches it), and
    // there is no cheaper way to compare entries by value.
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("generation-resolver-deduction");
    let slot_a_relative = layout.repository_relative(slot_a_path)?;
    let slot_a_bytes = read_file_if_exists(layout.repository_mutation_root(), &slot_a_relative)?
        .unwrap_or_default();
    let slot_b_relative = layout.repository_relative(slot_b_path)?;
    let slot_b_bytes = read_file_if_exists(layout.repository_mutation_root(), &slot_b_relative)?
        .unwrap_or_default();
    let decoded_a = decode_entries(&slot_a_bytes)?;
    let decoded_b = decode_entries(&slot_b_bytes)?;
    if decoded_a.damaged || decoded_b.damaged {
        // Rule 3: either slot is damaged, so the deduction itself cannot be made -- refuse, naming
        // the container's own existing damage text rather than inventing a new one.
        return Err(PrikkError::Integrity(damage_text.to_string()));
    }
    // Part E4: B is derived from A if B equals `compaction(P)`, or a prefix of it, for *some* prefix
    // `P` of A's own entries -- not only the full A (Part E3's own gap: a crash leaving A live and
    // taking further writes makes `compaction(A_now)` diverge from a B made from an earlier A).
    // One pass: fold A's entries into a running reduction, testing B against it after every step.
    let mut running = Vec::with_capacity(decoded_a.entries.len());
    let mut derived_from_a = decoded_b.entries.is_empty();
    for entry in decoded_a.entries {
        fold_entry(&mut running, entry);
        if running
            .get(..decoded_b.entries.len())
            .is_some_and(|prefix| decoded_b.entries.as_slice() == prefix)
        {
            derived_from_a = true;
            break;
        }
    }
    let (slot, reason) = if derived_from_a {
        // Rule 1.
        (
            ContainerSlot::A,
            DeductionReason::BIsDerivedFromSomePrefixOfA,
        )
    } else {
        // Rule 2.
        (
            ContainerSlot::B,
            DeductionReason::BTookWritesAfterBecomingLive,
        )
    };
    Ok((
        slot,
        replay.trailing_partial_bytes,
        replay.tail_offset,
        Some(DeducedFromContent { slot, reason }),
    ))
}

/// Like [`resolve_live_slot`], but never deduces or refuses on an ambiguous state -- trusts slot `A`
/// unconditionally when the log names none, matching every release before Part E. The one caller
/// this exists for, `recovery_log::meaning_paths_for` (RFC 168 §3.2), is a best-effort naming lookup
/// for an already-deliberate recovery operation, not an ordinary read or write -- and the module-
/// coupling boundary forbids `recovery_log` depending upward on a container module (`refs`,
/// `received`, `trust_index`) just to supply a decoder this one caller does not otherwise need.
pub(crate) fn resolve_live_slot_trusting_default_on_ambiguity(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
) -> Result<ContainerSlot> {
    let replay = replay_generation_log(layout, generation_log_path)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "generation log has a damaged record; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .records
        .last()
        .map_or(ContainerSlot::A, |record| record.live_slot))
}

pub(crate) fn resolve_live_slot<T: PartialEq>(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
    slot_a_path: &std::path::Path,
    slot_b_path: &std::path::Path,
    damage_text: &str,
    decode_entries: impl Fn(&[u8]) -> Result<DecodedEntries<T>>,
    fold_entry: impl Fn(&mut Vec<T>, T),
) -> Result<ContainerSlot> {
    Ok(resolve_or_deduce(
        layout,
        generation_log_path,
        slot_a_path,
        slot_b_path,
        damage_text,
        &decode_entries,
        &fold_entry,
    )?
    .0)
}

/// Like [`resolve_live_slot`], but also returns the log's own tail status from the same replay --
/// RFC 163 §9's write-side guard (`compact.rs`, its only caller) is built on this call so it never
/// pays for a second whole read just to learn what `resolve_live_slot` already decoded.
pub(crate) fn resolve_live_slot_with_tail<T: PartialEq>(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
    slot_a_path: &std::path::Path,
    slot_b_path: &std::path::Path,
    damage_text: &str,
    decode_entries: impl Fn(&[u8]) -> Result<DecodedEntries<T>>,
    fold_entry: impl Fn(&mut Vec<T>, T),
) -> Result<(ContainerSlot, usize, usize)> {
    let (slot, trailing_partial_bytes, tail_offset, _) = resolve_or_deduce(
        layout,
        generation_log_path,
        slot_a_path,
        slot_b_path,
        damage_text,
        &decode_entries,
        &fold_entry,
    )?;
    Ok((slot, trailing_partial_bytes, tail_offset))
}

/// `verify`/`doctor`'s own warning (Part E2): the identical deduction every reader and writer now
/// makes silently, surfaced explicitly so the ambiguous state stays visible and nameable, rather than
/// going unremarked once it stops being a refusal. `None` when the log names a slot outright, or slot
/// B is genuinely empty -- nothing to warn about.
pub(crate) fn resolve_live_slot_with_deduction_note<T: PartialEq>(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
    slot_a_path: &std::path::Path,
    slot_b_path: &std::path::Path,
    damage_text: &str,
    decode_entries: impl Fn(&[u8]) -> Result<DecodedEntries<T>>,
    fold_entry: impl Fn(&mut Vec<T>, T),
) -> Result<Option<DeducedFromContent>> {
    Ok(resolve_or_deduce(
        layout,
        generation_log_path,
        slot_a_path,
        slot_b_path,
        damage_text,
        &decode_entries,
        &fold_entry,
    )?
    .3)
}

#[cfg(test)]
mod tests;
