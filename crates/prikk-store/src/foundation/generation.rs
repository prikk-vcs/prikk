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

use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::push_u16;
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{append_file_required, read_file_if_exists};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};

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
    sha256(&preimage)
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
    },
}

fn parse_generation_frame_at(bytes: &[u8], offset: usize) -> GenerationFrameAttempt {
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
            return GenerationFrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.0) else {
        return GenerationFrameAttempt::Invalid {
            message: "generation record body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return GenerationFrameAttempt::Invalid {
            message: "generation record body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return GenerationFrameAttempt::TrailingPartial { remaining };
    };
    let expected = generation_checksum(header_values.0, body);
    if expected != header_values.1 {
        return GenerationFrameAttempt::Invalid {
            message: format!("generation record checksum mismatch at byte offset {offset}"),
        };
    }
    match decode_generation_body(body) {
        Ok(record) => GenerationFrameAttempt::Record {
            record,
            next_offset: body_end,
        },
        Err(err) => GenerationFrameAttempt::Invalid {
            message: err.to_string(),
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
    loop {
        match parse_generation_frame_at(bytes, offset) {
            GenerationFrameAttempt::Record {
                record,
                next_offset,
            } => {
                record_outcomes.push(GenerationRecordOutcome {
                    offset,
                    status: GenerationRecordStatus::Evaluated,
                });
                records.push(record);
                offset = next_offset;
            }
            GenerationFrameAttempt::TrailingPartial { remaining } => {
                return Ok(GenerationReplay {
                    records,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            GenerationFrameAttempt::Invalid { message } => {
                record_outcomes.push(GenerationRecordOutcome {
                    offset,
                    status: GenerationRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, GENERATION_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(GenerationReplay {
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

fn replay_generation_log(
    layout: &RepositoryLayout,
    generation_log_path: &std::path::Path,
) -> Result<GenerationReplay> {
    let relative = layout.repository_relative(generation_log_path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(GenerationReplay {
            records: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_generation_records(&bytes)
}

/// Resolve which slot is currently live for one compacting container's generation log. `A` when no
/// generation record has ever been written -- Step 1's only reachable outcome, since nothing appends
/// one yet. Fails closed on a damaged record rather than silently resolving to an older, stale
/// generation (see module doc).
pub fn resolve_live_slot(
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

#[cfg(test)]
mod tests;
