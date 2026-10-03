//! RFC 166 D2: the commit witness. One record, in its own file
//! (`.prikk/active/<name>/witness`), replaced atomically after every acknowledged commit, written
//! by the one session-level function every appender goes through
//! ([`append_patch_and_witness`]) -- `commit` (`author_inner`), `ActiveSession::append_patch` and
//! `rollback-draft` all call it; `Wal::append_patch` itself is reachable from nowhere else
//! (`tests::no_caller_of_wal_append_patch_outside_this_module` lists its callers from source).
//!
//! **The record** (§4 D2): magic and version; the owning ref name; the last acknowledged seq; its
//! Patch id; its frame hash (W2); a running hash over every acknowledged frame (W3); the ref's own
//! tip `RefState` id at the time of writing, or none (bounding D3's connectivity walk); a SHA-256
//! over all of it.
//!
//! **The write** comes after the durable WAL append and before the caller's own report -- a crash
//! between the two leaves a sound record the witness does not cover, not acknowledged, kept exactly
//! as it is today (§4 D2).

use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;
use prikk_object::{ObjectEnvelope, ObjectId};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{push_u16, push_u64};
use crate::foundation::fsutil::{read_file_if_exists, write_file_atomically};
use crate::foundation::layout::RepositoryLayout;
use crate::refs::RefStore;
use crate::wal::{Wal, record_frame_checksum};

const WITNESS_MAGIC: &[u8; 8] = b"PWTN0003";
const WITNESS_VERSION: u16 = 1;

/// One acknowledged commit's witness, as read back.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WitnessRecord {
    /// The ref this session's queue belongs to, as it was when this record was written.
    pub ref_name: String,
    /// The last acknowledged WAL sequence.
    pub last_seq: u64,
    /// That record's own Patch id.
    pub patch_id: ObjectId,
    /// That record's own frame checksum (W2).
    pub frame_hash: [u8; 32],
    /// A running hash over every acknowledged frame from seq 1 through `last_seq` (W3).
    pub running_hash: [u8; 32],
    /// The ref's own tip `RefState` id at the time this record was written, or `None` if the ref
    /// had never been published yet. Bounds D3's own connectivity walk.
    pub ref_tip_at_write: Option<ObjectId>,
}

/// Outcome of reading the witness file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WitnessState {
    /// No witness file, or an empty one (the cleared state, written at drain). A legacy session, or
    /// one whose witness was removed -- they cannot be told apart, and both fall back to rule 3 for
    /// this session (D3 item 1), exactly as a pre-0.49.0 repository always has.
    Absent,
    /// A full-length record whose checksum and magic/version check out.
    Valid(WitnessRecord),
    /// Present, but its magic, version, checksum, or internal structure does not check out. Fails
    /// closed: a reader never falls back to treating this as `Absent`.
    Damaged(String),
}

fn witness_path(layout: &RepositoryLayout, name: impl AsRef<std::path::Path>) -> std::path::PathBuf {
    layout.active_session_dir(name).join("witness")
}

/// Read the witness for the named active session.
pub fn read_witness(
    layout: &RepositoryLayout,
    name: impl AsRef<std::path::Path>,
) -> Result<WitnessState> {
    let path = witness_path(layout, name);
    let relative = layout.repository_relative(&path)?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(WitnessState::Absent);
    };
    if bytes.is_empty() {
        return Ok(WitnessState::Absent);
    }
    decode_witness(&bytes).map_or_else(
        |err| Ok(WitnessState::Damaged(err.to_string())),
        |record| Ok(WitnessState::Valid(record)),
    )
}

fn decode_witness(bytes: &[u8]) -> Result<WitnessRecord> {
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_array::<8>()?;
    if &magic != WITNESS_MAGIC {
        return Err(PrikkError::MalformedData("witness has an unrecognized magic".to_string()));
    }
    let version = cursor.read_u16()?;
    if version != WITNESS_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let ref_name = cursor.read_string_u16()?;
    let last_seq = cursor.read_u64()?;
    let patch_id_bytes = cursor.read_array::<32>()?;
    let frame_hash = cursor.read_array::<32>()?;
    let running_hash = cursor.read_array::<32>()?;
    let has_tip = cursor.read_array::<1>()?[0];
    let tip_bytes = cursor.read_array::<32>()?;
    let stored_checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData("trailing bytes in witness record".to_string()));
    }
    let expected = witness_checksum(
        &ref_name, last_seq, &patch_id_bytes, &frame_hash, &running_hash, has_tip, &tip_bytes,
    );
    if expected != stored_checksum {
        return Err(PrikkError::MalformedData("witness checksum mismatch".to_string()));
    }
    crate::refs::validate_local_branch_ref(&ref_name)
        .map_err(|err| PrikkError::MalformedData(format!("witness ref name invalid: {err}")))?;
    let ref_tip_at_write = (has_tip != 0).then(|| ObjectId::from_bytes(tip_bytes));
    Ok(WitnessRecord {
        ref_name,
        last_seq,
        patch_id: ObjectId::from_bytes(patch_id_bytes),
        frame_hash,
        running_hash,
        ref_tip_at_write,
    })
}

fn encode_witness(record: &WitnessRecord) -> Vec<u8> {
    let patch_id_bytes = *record.patch_id.as_bytes();
    let has_tip = u8::from(record.ref_tip_at_write.is_some());
    let tip_bytes = record.ref_tip_at_write.map_or([0u8; 32], |id| *id.as_bytes());
    let checksum = witness_checksum(
        &record.ref_name, record.last_seq, &patch_id_bytes, &record.frame_hash,
        &record.running_hash, has_tip, &tip_bytes,
    );
    let mut out = Vec::with_capacity(8 + 2 + 2 + record.ref_name.len() + 8 + 32 + 32 + 32 + 1 + 32 + 32);
    out.extend_from_slice(WITNESS_MAGIC);
    push_u16(&mut out, WITNESS_VERSION);
    // `validate_local_branch_ref` bounds ref names well under `u16::MAX` bytes before one ever
    // reaches here; `unwrap_or(0)` only matters for matching `witness_checksum`'s own identical
    // fallback below, never for a real ref name.
    push_u16(&mut out, u16::try_from(record.ref_name.len()).unwrap_or(0));
    out.extend_from_slice(record.ref_name.as_bytes());
    push_u64(&mut out, record.last_seq);
    out.extend_from_slice(&patch_id_bytes);
    out.extend_from_slice(&record.frame_hash);
    out.extend_from_slice(&record.running_hash);
    out.push(has_tip);
    out.extend_from_slice(&tip_bytes);
    out.extend_from_slice(&checksum);
    out
}

#[allow(clippy::too_many_arguments)]
fn witness_checksum(
    ref_name: &str, last_seq: u64, patch_id: &[u8; 32], frame_hash: &[u8; 32],
    running_hash: &[u8; 32], has_tip: u8, tip_bytes: &[u8; 32],
) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(8 + 2 + 2 + ref_name.len() + 8 + 32 + 32 + 32 + 1 + 32);
    preimage.extend_from_slice(WITNESS_MAGIC);
    preimage.extend_from_slice(&WITNESS_VERSION.to_be_bytes());
    preimage.extend_from_slice(&(u16::try_from(ref_name.len()).unwrap_or(0)).to_be_bytes());
    preimage.extend_from_slice(ref_name.as_bytes());
    preimage.extend_from_slice(&last_seq.to_be_bytes());
    preimage.extend_from_slice(patch_id);
    preimage.extend_from_slice(frame_hash);
    preimage.extend_from_slice(running_hash);
    preimage.push(has_tip);
    preimage.extend_from_slice(tip_bytes);
    sha256(&preimage)
}

/// Clear the witness (the drain's own second step, after the WAL truncate and before `ref-name`'s
/// own clear -- §4 D2, today's order, the safe one).
pub fn clear_witness(layout: &RepositoryLayout, name: impl AsRef<std::path::Path>) -> Result<()> {
    let relative = layout.repository_relative(&witness_path(layout, &name))?;
    write_file_atomically(layout.repository_mutation_root(), &relative, &[])
}

/// RFC 166 §13 item 2: fold every sound record the current witness does not yet cover, not only
/// the newest one. `covered_through` is the current witness's own `last_seq` (`0` if absent or
/// damaged -- nothing trustworthy to build on, so the whole queue is covered from scratch, which is
/// correct for a first witness over a legacy queue: those commits were acknowledged by the older
/// binary that wrote them).
fn fold_running_hash(
    previous_running_hash: [u8; 32],
    sound_records: &[crate::wal::WalRecord],
    covered_through: u64,
    through_seq: u64,
) -> Result<[u8; 32]> {
    let mut running = previous_running_hash;
    for record in sound_records {
        if record.seq <= covered_through {
            continue;
        }
        if record.seq > through_seq {
            break;
        }
        let frame_hash = record_frame_checksum(record)?;
        let mut preimage = Vec::with_capacity(64);
        preimage.extend_from_slice(&running);
        preimage.extend_from_slice(&frame_hash);
        running = sha256(&preimage);
    }
    Ok(running)
}

/// The one session-level function every appender goes through (§13 item 1): appends `envelope` to
/// the named session's WAL, then writes the witness, atomically, after the durable append and
/// before returning. Returns the assigned (or, for an idempotent retry, the pre-existing) seq.
pub fn append_patch_and_witness(
    layout: &RepositoryLayout,
    name: impl AsRef<std::path::Path> + Copy,
    ref_name: &str,
    envelope: &ObjectEnvelope,
) -> Result<u64> {
    let previous = read_witness(layout, name)?;
    let (previous_running_hash, covered_through) = match &previous {
        WitnessState::Valid(record) => (record.running_hash, record.last_seq),
        WitnessState::Absent | WitnessState::Damaged(_) => ([0u8; 32], 0),
    };
    let wal = Wal::for_layout(layout, name);
    let seq = wal.append_patch(envelope)?;
    let replay = wal.replay()?;
    let running_hash = fold_running_hash(previous_running_hash, &replay.records, covered_through, seq)?;
    let ref_store = RefStore::new(layout.clone());
    let ref_tip_at_write = ref_store.read_current_ref_state_id(ref_name)?;
    let record = WitnessRecord {
        ref_name: ref_name.to_string(),
        last_seq: seq,
        patch_id: envelope.object_id(),
        frame_hash: record_frame_checksum(&crate::wal::WalRecord {
            seq,
            envelope: envelope.clone(),
        })?,
        running_hash,
        ref_tip_at_write,
    };
    let bytes = encode_witness(&record);
    let relative = layout.repository_relative(&witness_path(layout, name))?;
    write_file_atomically(layout.repository_mutation_root(), &relative, &bytes)?;
    Ok(seq)
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
