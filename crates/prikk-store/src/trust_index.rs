//! Trust store containers (RFC 102 Stage 5, design-v1.md §14/§14.9): the publication-trust key
//! material and the adopted-key policy, each on its own append-only container, keyed and framed the
//! same way `received_index.rs`/`refs/pointer_index.rs` are.
//!
//! **Two containers, two shapes, because the two things stored are not the same kind of state.**
//!
//! - **Key material** (`TrustKeyEntry`): one entry per adopted key id, keyed by `key_id` directly --
//!   already storage-safe ASCII (`validate_maintainer_key_id_storage_safety`), no hashing needed the
//!   way ref names required. No separate lookup index: every production read of the key material goes
//!   through the policy's own resolution, which already needs the whole set.
//! - **Policy** (`TrustPolicySnapshotEntry`): **not** an append-only log of individual key adoptions.
//!   Each write is a **complete snapshot** of the currently-adopted key id list, mirroring design-v1.md
//!   §4's compaction-generation-log precedent ("readers take the last complete generation record").
//!   `add_trusted_maintainer` already rebuilt the whole `keys = [...]` list on every call under the old
//!   TOML file (§14.9's own finding) -- the container changes the encoding, not the semantics.
//!   `required = 1` is never stored; it is a DC-11 constant, and storing it would let it drift into a
//!   count (design-v1.md §14 constraint 4).
//!
//! **Revocation is representable because the policy is snapshots, not an append log of adoptions**
//! (§14.9): `remove_trusted_maintainer` appends a new snapshot with the key id absent. No tombstone
//! record, no new format concept -- the same operation `add_trusted_maintainer` already performs,
//! just with a shorter list.
//!
//! **Fail-closed on the latest snapshot, not silently older.** A damaged trailing policy record must
//! never resolve to an earlier, stale snapshot -- the same "last entry wins, so a damaged latest entry
//! must not be silently skipped" reasoning `refs/pointer_index.rs`'s §13.14 established, applied here
//! at whole-snapshot granularity instead of per-key.
//!
//! **An empty policy container (no snapshot ever appended) is a *missing* policy, not an *empty* one**
//! -- preserving `PublicationTrustVerifier`'s existing `PRIKK-TRUST-POLICY-INVALID` signal for a
//! repository where no maintainer has ever been adopted, exactly as the old `read_file_required`-based
//! load did.

use prikk_crypto::ED25519_KEY_LEN;
use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{push_string_u16, push_u16, push_u32};
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{append_file_required, len_to_u64, read_file_if_exists};
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::RepositoryLayout;

const TRUST_KEY_MAGIC: &[u8; 8] = b"PTRUKEY1";
const TRUST_KEY_VERSION: u16 = 1;
const TRUST_KEY_HEADER_LEN: usize = 8 + 2 + 8 + 32;

const TRUST_POLICY_MAGIC: &[u8; 8] = b"PTRUPOL1";
const TRUST_POLICY_VERSION: u16 = 1;
const TRUST_POLICY_HEADER_LEN: usize = 8 + 2 + 8 + 32;

// ---------------------------------------------------------------------------------------------
// Key material container
// ---------------------------------------------------------------------------------------------

/// One adopted maintainer key's material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustKeyEntry {
    pub(crate) key_id: String,
    pub(crate) public_key: [u8; ED25519_KEY_LEN],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustKeyRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustKeyRecordOutcome {
    pub(crate) offset: usize,
    pub(crate) status: TrustKeyRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustKeyReplay {
    pub(crate) entries: Vec<TrustKeyEntry>,
    pub(crate) trailing_partial_bytes: usize,
    pub(crate) record_outcomes: Vec<TrustKeyRecordOutcome>,
}

impl TrustKeyReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, TrustKeyRecordStatus::Failed { .. }))
    }
}

fn encode_trust_key_body(entry: &TrustKeyEntry) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    push_string_u16(&mut body, &entry.key_id)?;
    body.extend_from_slice(&entry.public_key);
    Ok(body)
}

fn decode_trust_key_body(body: &[u8]) -> Result<TrustKeyEntry> {
    let mut cursor = ByteCursor::new(body);
    let key_id = cursor.read_string_u16()?;
    let public_key = cursor.read_array::<ED25519_KEY_LEN>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in trust key entry body".to_string(),
        ));
    }
    Ok(TrustKeyEntry { key_id, public_key })
}

pub(crate) fn encode_trust_key_record(entry: &TrustKeyEntry) -> Result<Vec<u8>> {
    let body = encode_trust_key_body(entry)?;
    let body_len = len_to_u64(body.len())?;
    let checksum = trust_key_checksum(body_len, &body);
    let mut out = Vec::with_capacity(TRUST_KEY_HEADER_LEN + body.len());
    out.extend_from_slice(TRUST_KEY_MAGIC);
    push_u16(&mut out, TRUST_KEY_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    Ok(out)
}

fn trust_key_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(TRUST_KEY_MAGIC);
    preimage.extend_from_slice(&TRUST_KEY_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    sha256(&preimage)
}

enum TrustKeyFrameAttempt {
    Record {
        entry: TrustKeyEntry,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
    },
}

fn parse_trust_key_frame_at(bytes: &[u8], offset: usize) -> TrustKeyFrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < TRUST_KEY_HEADER_LEN {
        return TrustKeyFrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + TRUST_KEY_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return TrustKeyFrameAttempt::TrailingPartial { remaining };
    };
    let mut cursor = ByteCursor::new(header);
    let (magic, version, body_len, checksum) = match (|| -> Result<_> {
        let magic = cursor.read_array::<8>()?;
        let version = cursor.read_u16()?;
        let body_len = cursor.read_u64()?;
        let checksum = cursor.read_array::<32>()?;
        Ok((magic, version, body_len, checksum))
    })() {
        Ok(values) => values,
        Err(err) => {
            return TrustKeyFrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    if &magic != TRUST_KEY_MAGIC {
        return TrustKeyFrameAttempt::Invalid {
            message: "invalid trust key record magic".to_string(),
        };
    }
    if version != TRUST_KEY_VERSION {
        return TrustKeyFrameAttempt::Invalid {
            message: format!("unsupported trust key record version {version}"),
        };
    }
    let Ok(body_len_usize) = usize::try_from(body_len) else {
        return TrustKeyFrameAttempt::Invalid {
            message: "trust key body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len_usize) else {
        return TrustKeyFrameAttempt::Invalid {
            message: "trust key body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return TrustKeyFrameAttempt::TrailingPartial { remaining };
    };
    let expected = trust_key_checksum(body_len, body);
    if expected != checksum {
        return TrustKeyFrameAttempt::Invalid {
            message: format!("trust key checksum mismatch at byte offset {offset}"),
        };
    }
    match decode_trust_key_body(body) {
        Ok(entry) => TrustKeyFrameAttempt::Record {
            entry,
            next_offset: body_end,
        },
        Err(err) => TrustKeyFrameAttempt::Invalid {
            message: err.to_string(),
        },
    }
}

/// Isolate-and-continue reading, matching every other container's decode loop in this codebase.
pub(crate) fn decode_trust_key_records(bytes: &[u8]) -> Result<TrustKeyReplay> {
    let mut entries = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    loop {
        match parse_trust_key_frame_at(bytes, offset) {
            TrustKeyFrameAttempt::Record { entry, next_offset } => {
                record_outcomes.push(TrustKeyRecordOutcome {
                    offset,
                    status: TrustKeyRecordStatus::Evaluated,
                });
                entries.push(entry);
                offset = next_offset;
            }
            TrustKeyFrameAttempt::TrailingPartial { remaining } => {
                return Ok(TrustKeyReplay {
                    entries,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            TrustKeyFrameAttempt::Invalid { message } => {
                record_outcomes.push(TrustKeyRecordOutcome {
                    offset,
                    status: TrustKeyRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, TRUST_KEY_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(TrustKeyReplay {
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

pub(crate) fn replay_trust_keys(layout: &RepositoryLayout) -> Result<TrustKeyReplay> {
    let relative = layout.repository_relative(&layout.trust_key_container_path())?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(TrustKeyReplay {
            entries: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_trust_key_records(&bytes)
}

/// Look up one key id's last-appended key material. Refuses if the container has any damaged entry,
/// matching `pointer_index::lookup_ref_pointer`'s fail-closed reasoning.
pub(crate) fn lookup_trust_key_entry(
    layout: &RepositoryLayout,
    key_id: &str,
) -> Result<Option<TrustKeyEntry>> {
    let replay = replay_trust_keys(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "trust key container has a damaged entry; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .entries
        .into_iter()
        .rev()
        .find(|entry| entry.key_id == key_id))
}

pub(crate) fn append_trust_key_entry(
    layout: &RepositoryLayout,
    entry: &TrustKeyEntry,
) -> Result<()> {
    let record = encode_trust_key_record(entry)?;
    let relative = layout.repository_relative(&layout.trust_key_container_path())?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
}

// ---------------------------------------------------------------------------------------------
// Policy snapshot container
// ---------------------------------------------------------------------------------------------

/// One complete policy snapshot: every currently-adopted key id, in adoption order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustPolicySnapshotEntry {
    pub(crate) key_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TrustPolicyRecordStatus {
    Evaluated,
    Failed { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustPolicyRecordOutcome {
    pub(crate) offset: usize,
    pub(crate) status: TrustPolicyRecordStatus,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TrustPolicyReplay {
    pub(crate) entries: Vec<TrustPolicySnapshotEntry>,
    pub(crate) trailing_partial_bytes: usize,
    pub(crate) record_outcomes: Vec<TrustPolicyRecordOutcome>,
}

impl TrustPolicyReplay {
    #[must_use]
    pub(crate) fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, TrustPolicyRecordStatus::Failed { .. }))
    }
}

fn encode_trust_policy_body(entry: &TrustPolicySnapshotEntry) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    let count = u32::try_from(entry.key_ids.len())
        .map_err(|_| PrikkError::MalformedData("trust policy key count exceeds u32".to_string()))?;
    push_u32(&mut body, count);
    for key_id in &entry.key_ids {
        push_string_u16(&mut body, key_id)?;
    }
    Ok(body)
}

fn decode_trust_policy_body(body: &[u8]) -> Result<TrustPolicySnapshotEntry> {
    let mut cursor = ByteCursor::new(body);
    let count = cursor.read_u32()?;
    let mut key_ids = Vec::with_capacity(count as usize);
    for _ in 0..count {
        key_ids.push(cursor.read_string_u16()?);
    }
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in trust policy snapshot body".to_string(),
        ));
    }
    // DC-11 defense in depth: the write path (`trust.rs::add_trusted_maintainer`) never produces a
    // duplicate, but the old TOML parser rejected one structurally (`parse_policy_keys`'s own
    // duplicate check) and this decode is the container-era equivalent choke point for the same
    // property -- every reader goes through it, unlike a write-side-only guarantee.
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for key_id in &key_ids {
        if !seen.insert(key_id.as_str()) {
            return Err(PrikkError::MalformedData(format!(
                "trust policy snapshot lists key id {key_id} more than once"
            )));
        }
    }
    Ok(TrustPolicySnapshotEntry { key_ids })
}

pub(crate) fn encode_trust_policy_record(entry: &TrustPolicySnapshotEntry) -> Result<Vec<u8>> {
    let body = encode_trust_policy_body(entry)?;
    let body_len = len_to_u64(body.len())?;
    let checksum = trust_policy_checksum(body_len, &body);
    let mut out = Vec::with_capacity(TRUST_POLICY_HEADER_LEN + body.len());
    out.extend_from_slice(TRUST_POLICY_MAGIC);
    push_u16(&mut out, TRUST_POLICY_VERSION);
    out.extend_from_slice(&body_len.to_be_bytes());
    out.extend_from_slice(&checksum);
    out.extend_from_slice(&body);
    Ok(out)
}

fn trust_policy_checksum(body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::new();
    preimage.extend_from_slice(TRUST_POLICY_MAGIC);
    preimage.extend_from_slice(&TRUST_POLICY_VERSION.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    sha256(&preimage)
}

enum TrustPolicyFrameAttempt {
    Record {
        entry: TrustPolicySnapshotEntry,
        next_offset: usize,
    },
    TrailingPartial {
        remaining: usize,
    },
    Invalid {
        message: String,
    },
}

fn parse_trust_policy_frame_at(bytes: &[u8], offset: usize) -> TrustPolicyFrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < TRUST_POLICY_HEADER_LEN {
        return TrustPolicyFrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + TRUST_POLICY_HEADER_LEN;
    let Some(header) = bytes.get(offset..header_end) else {
        return TrustPolicyFrameAttempt::TrailingPartial { remaining };
    };
    let mut cursor = ByteCursor::new(header);
    let (magic, version, body_len, checksum) = match (|| -> Result<_> {
        let magic = cursor.read_array::<8>()?;
        let version = cursor.read_u16()?;
        let body_len = cursor.read_u64()?;
        let checksum = cursor.read_array::<32>()?;
        Ok((magic, version, body_len, checksum))
    })() {
        Ok(values) => values,
        Err(err) => {
            return TrustPolicyFrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    if &magic != TRUST_POLICY_MAGIC {
        return TrustPolicyFrameAttempt::Invalid {
            message: "invalid trust policy record magic".to_string(),
        };
    }
    if version != TRUST_POLICY_VERSION {
        return TrustPolicyFrameAttempt::Invalid {
            message: format!("unsupported trust policy record version {version}"),
        };
    }
    let Ok(body_len_usize) = usize::try_from(body_len) else {
        return TrustPolicyFrameAttempt::Invalid {
            message: "trust policy body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len_usize) else {
        return TrustPolicyFrameAttempt::Invalid {
            message: "trust policy body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return TrustPolicyFrameAttempt::TrailingPartial { remaining };
    };
    let expected = trust_policy_checksum(body_len, body);
    if expected != checksum {
        return TrustPolicyFrameAttempt::Invalid {
            message: format!("trust policy checksum mismatch at byte offset {offset}"),
        };
    }
    match decode_trust_policy_body(body) {
        Ok(entry) => TrustPolicyFrameAttempt::Record {
            entry,
            next_offset: body_end,
        },
        Err(err) => TrustPolicyFrameAttempt::Invalid {
            message: err.to_string(),
        },
    }
}

pub(crate) fn decode_trust_policy_records(bytes: &[u8]) -> Result<TrustPolicyReplay> {
    let mut entries = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    loop {
        match parse_trust_policy_frame_at(bytes, offset) {
            TrustPolicyFrameAttempt::Record { entry, next_offset } => {
                record_outcomes.push(TrustPolicyRecordOutcome {
                    offset,
                    status: TrustPolicyRecordStatus::Evaluated,
                });
                entries.push(entry);
                offset = next_offset;
            }
            TrustPolicyFrameAttempt::TrailingPartial { remaining } => {
                return Ok(TrustPolicyReplay {
                    entries,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            TrustPolicyFrameAttempt::Invalid { message } => {
                record_outcomes.push(TrustPolicyRecordOutcome {
                    offset,
                    status: TrustPolicyRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, TRUST_POLICY_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(TrustPolicyReplay {
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

/// Generation-aware (RFC 102 Stage 6 Step 1, design-v1.md §15.6): resolves to `A` today, since
/// nothing has ever appended a generation record.
pub(crate) fn replay_trust_policy(layout: &RepositoryLayout) -> Result<TrustPolicyReplay> {
    let slot = resolve_live_slot(layout, &layout.trust_policy_generation_log_path())?;
    let relative = layout.repository_relative(&layout.trust_policy_container_slot_path(slot))?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(TrustPolicyReplay {
            entries: Vec::new(),
            trailing_partial_bytes: 0,
            record_outcomes: Vec::new(),
        });
    };
    decode_trust_policy_records(&bytes)
}

/// Read the current policy: the last complete snapshot record. `Ok(None)` means no snapshot has ever
/// been appended -- a repository where no maintainer has ever been adopted -- which callers must treat
/// as a missing policy, not an empty one (see module doc). Fails closed on any damaged record, so a
/// corrupt latest snapshot never silently resolves to an older, stale one.
pub(crate) fn read_current_trust_policy_snapshot(
    layout: &RepositoryLayout,
) -> Result<Option<Vec<String>>> {
    let replay = replay_trust_policy(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "trust policy container has a damaged snapshot; run doctor before reading".to_string(),
        ));
    }
    Ok(replay
        .entries
        .into_iter()
        .next_back()
        .map(|entry| entry.key_ids))
}

/// Durably append a complete policy snapshot -- the publish moment for the whole adopted-key list.
pub(crate) fn append_trust_policy_snapshot(
    layout: &RepositoryLayout,
    key_ids: &[String],
) -> Result<()> {
    let record = encode_trust_policy_record(&TrustPolicySnapshotEntry {
        key_ids: key_ids.to_vec(),
    })?;
    // RFC 102 Stage 6 Step 2, design-v1.md §15.7/§15.9: resolver-routed, not hardcoded to `A` --
    // see `pointer_index::append_ref_pointer_entry`'s identical comment for why, and why the
    // container lock (held by `add_trusted_maintainer`/`remove_trusted_maintainer` for their whole
    // critical section) closes the resolve-then-append race against the compactor.
    let slot = resolve_live_slot(layout, &layout.trust_policy_generation_log_path())?;
    let relative = layout.repository_relative(&layout.trust_policy_container_slot_path(slot))?;
    append_file_required(layout.repository_mutation_root(), &relative, &record)
}

#[cfg(test)]
mod tests;
