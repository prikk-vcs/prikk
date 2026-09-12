//! Write-ahead log for active patch envelopes.

use std::fs;
use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, push_u16, push_u64,
};
use crate::foundation::frame_resync::resync_to_next_magic;
use crate::foundation::fsutil::{
    MutationRoot, append_file_required, len_to_u64, read_file_if_exists,
    truncate_existing_file_required, truncate_file_empty_required,
};
use crate::foundation::layout::RepositoryLayout;

const WAL_RECORD_MAGIC: &[u8; 8] = b"PWALR001";
const WAL_RECORD_VERSION: u16 = 1;
const WAL_HEADER_LEN: usize = 8 + 2 + 8 + 8 + 32;

/// One durable WAL record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRecord {
    /// Monotonic WAL sequence.
    pub seq: u64,
    /// Exact signed object envelope stored at commit time.
    pub envelope: ObjectEnvelope,
}

/// Outcome of attempting to decode one WAL record frame (RFC 102 Stage 2: isolate-and-continue
/// reading). File identity here is the byte offset a frame attempt started at, not a stored id --
/// a frame that failed to validate has no other identity to report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalRecordStatus {
    /// The frame at this offset was read and validated successfully.
    Evaluated,
    /// The frame at this offset failed to validate (bad magic/version, checksum mismatch, or a
    /// malformed envelope) -- resync moved past it byte-wise to find the next candidate frame.
    Failed {
        /// The error this frame's own validation raised.
        message: String,
    },
}

/// One attempted WAL record frame's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRecordOutcome {
    /// The byte offset within the WAL this frame attempt started at.
    pub offset: usize,
    /// How this frame's own read/validation resolved.
    pub status: WalRecordStatus,
}

/// WAL replay result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalReplay {
    /// Valid records read from the WAL, in file order -- includes records found after a damaged
    /// one (RFC 102 Stage 2), not merely a prefix up to the first failure.
    pub records: Vec<WalRecord>,
    /// Number of trailing bytes ignored as an incomplete final record -- a legitimate torn tail
    /// from an interrupted append, unchanged in meaning from before Stage 2. Zero when the WAL's
    /// end was reached via resync after unrecoverable corruption (see `record_outcomes`) rather
    /// than a genuinely incomplete final frame.
    pub trailing_partial_bytes: usize,
    /// One outcome per attempted frame, in scan order -- both `Evaluated` and `Failed`. A frame
    /// that failed and was then found again via resync (a false-positive magic match inside
    /// corrupted bytes) gets its own entry too; this is deliberately not deduplicated into "one
    /// finding per corrupted region", since each is an independently true statement about what was
    /// attempted at that offset.
    pub record_outcomes: Vec<WalRecordOutcome>,
}

impl WalReplay {
    /// Return true when any attempted frame failed to validate.
    #[must_use]
    pub fn has_item_failure(&self) -> bool {
        self.record_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, WalRecordStatus::Failed { .. }))
    }
}

/// Result of a safe WAL tail truncation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalRepair {
    /// Number of valid records preserved after repair.
    pub preserved_records: usize,
    /// Number of trailing partial bytes truncated.
    pub truncated_bytes: usize,
    /// Patch object ids of the preserved records, in WAL order. DC-66 criterion 5: a repair against a
    /// queue of N must say *which* authors' work survived, not just how many records — "3 records
    /// preserved" does not answer that for N > 1 the way it unambiguously did for N = 1.
    pub preserved_patch_ids: Vec<ObjectId>,
}

/// File-backed active-session WAL.
#[derive(Debug, Clone)]
pub struct Wal {
    path: PathBuf,
    mutation: Option<(MutationRoot, PathBuf)>,
    layout: Option<RepositoryLayout>,
}

impl Wal {
    /// Create a WAL handle for a path.
    #[must_use]
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            mutation: None,
            layout: None,
        }
    }

    /// Create a WAL handle authorized by a validated repository layout, for the active session
    /// named `name` (RFC 108 increment 1: previously hardcoded to `"default"`; the layout stays
    /// the authority on the absolute path, so this only accepts a name rather than building path
    /// segments of its own beyond the relative literal it already carried).
    ///
    /// RFC 108 increment 3a: `name` widened from `&str` to `impl AsRef<Path>`, matching
    /// `layout.rs`'s own `active_*` methods, so a name obtained from `active_session_names()` (an
    /// `OsString`, not necessarily valid UTF-8) reaches this constructor without a lossy `String`
    /// round-trip -- and the relative path is now `layout.active_queue_wal_relative_path(name)`,
    /// derived from the same components as the absolute path, not rebuilt through `format!` (which
    /// cannot even accept a non-UTF-8 name, `Display` requires valid text).
    #[must_use]
    pub fn for_layout(layout: &RepositoryLayout, name: impl AsRef<Path>) -> Self {
        let name = name.as_ref();
        let path = layout.active_queue_wal_path(name);
        let relative = layout.active_queue_wal_relative_path(name);
        Self {
            path,
            mutation: Some((layout.repository_mutation_root().clone(), relative)),
            layout: Some(layout.clone()),
        }
    }

    /// Return the WAL path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Append a signed patch envelope and fsync the WAL file.
    pub fn append_patch(&self, envelope: &ObjectEnvelope) -> Result<u64> {
        self.require_current_format()?;
        if envelope.object_type != ObjectType::Patch {
            return Err(PrikkError::ObjectTypeMismatch {
                expected: ObjectType::Patch.to_string(),
                actual: envelope.object_type.to_string(),
            });
        }
        if envelope.signatures.is_empty() {
            return Err(PrikkError::InvalidSignature(
                "commit WAL entries must store signed patch envelopes".to_string(),
            ));
        }
        envelope.validate_strict()?;
        // Patch schema 2 handoff v2 amendment §2: read the authoritative admitted-schema table
        // `format.rs::validate_format2_schema` owns, rather than a second, hand-maintained `!= 1`
        // check -- the same reasoning `lifecycle_cache::replay::read_patch_operations` applies.
        let accepted = crate::format::admitted_schemas(ObjectType::Patch).unwrap_or(&[]);
        if !accepted.contains(&envelope.schema_version) {
            return Err(PrikkError::Integrity(format!(
                "format-2 Patch does not accept envelope schema {} (accepted: {accepted:?})",
                envelope.schema_version
            )));
        }
        let replay = self.replay()?;
        if replay.trailing_partial_bytes != 0 {
            return Err(PrikkError::Integrity(
                "cannot append after an incomplete WAL tail".to_string(),
            ));
        }
        // RFC 102 Stage 2: isolate-and-continue reading means a damaged record no longer makes
        // `replay()` itself return `Err` -- append must refuse explicitly, since `next_seq` and
        // "does the last record already match" below are both computed from `replay.records`,
        // which silently omits a damaged record rather than surfacing it as an error here.
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(
                "cannot append after a damaged WAL record; run doctor before appending".to_string(),
            ));
        }
        let (root, relative) = self.mutation()?;
        match replay.records.last() {
            Some(last) if last.envelope == *envelope => {
                append_file_required(root, relative, &[])?;
                return Ok(last.seq);
            }
            _ => {}
        }
        let next_seq = replay.records.last().map_or(Ok(1), |last| {
            last.seq
                .checked_add(1)
                .ok_or_else(|| PrikkError::MalformedData("WAL sequence overflow".to_string()))
        })?;
        let record = WalRecord {
            seq: next_seq,
            envelope: envelope.clone(),
        };
        let bytes = encode_record(&record)?;
        // No `ensure_directory_required` here -- same justification as `truncate_empty_authorized`
        // below (RFC 102 Stage 5, design-v1.md §14.8): `default_active_dir()` is in
        // `required_repository_directories()`, permanent from `init`, and since `d8f5240` made
        // `durable_append` strict, the WAL file must already exist for the append to succeed, so it
        // cannot exist without its directory -- this call could not help even if the directory were
        // somehow absent.
        if relative.parent().is_none() {
            return Err(PrikkError::Io {
                kind: None,
                context: "WAL path has no parent directory".to_string(),
            });
        }
        append_file_required(root, relative, &bytes)?;
        Ok(next_seq)
    }

    /// Replay valid WAL records from the beginning.
    pub fn replay(&self) -> Result<WalReplay> {
        let Some(bytes) = self.read_bytes()? else {
            return Ok(WalReplay {
                records: Vec::new(),
                trailing_partial_bytes: 0,
                record_outcomes: Vec::new(),
            });
        };
        let replay = decode_records(&bytes)?;
        if let Some(layout) = &self.layout {
            for record in &replay.records {
                crate::format::validate_read_schema(layout.format(), &record.envelope)?;
            }
        }
        Ok(replay)
    }

    /// Safely truncate an incomplete trailing WAL record, if one exists.
    ///
    /// This repairs only the case that FDD-02 defines as safe: valid records followed by an
    /// incomplete final record. **RFC 102 Stage 2**: a mid-stream checksum mismatch in a complete
    /// record no longer makes `decode_records` return `Err` -- it is now an item finding
    /// (`WalReplay::has_item_failure`), which this function refuses on explicitly below rather than
    /// silently truncating around. The only production caller (`doctor.rs`'s `repair_repository`)
    /// already refuses earlier via `doctor_repository`'s own item-outcome reporting; this check is
    /// defense in depth for this function's own contract, not the only thing enforcing it.
    pub fn truncate_trailing_partial(&self) -> Result<WalRepair> {
        self.require_current_format()?;
        let Some(bytes) = self.read_bytes()? else {
            return Ok(WalRepair {
                preserved_records: 0,
                truncated_bytes: 0,
                preserved_patch_ids: Vec::new(),
            });
        };
        let replay = decode_records(&bytes)?;
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(
                "WAL has a damaged record; repair does not modify it".to_string(),
            ));
        }
        let preserved_patch_ids: Vec<ObjectId> = replay
            .records
            .iter()
            .map(|record| record.envelope.object_id())
            .collect();
        if replay.trailing_partial_bytes == 0 {
            return Ok(WalRepair {
                preserved_records: replay.records.len(),
                truncated_bytes: 0,
                preserved_patch_ids,
            });
        }
        let current_len = u64::try_from(bytes.len())
            .map_err(|_| PrikkError::MalformedData("WAL length does not fit u64".to_string()))?;
        let trailing = u64::try_from(replay.trailing_partial_bytes).map_err(|_| {
            PrikkError::MalformedData("trailing WAL byte count does not fit u64".to_string())
        })?;
        let repaired_len = current_len.checked_sub(trailing).ok_or_else(|| {
            PrikkError::MalformedData("trailing WAL byte count exceeds file length".to_string())
        })?;
        let (root, relative) = self.mutation()?;
        truncate_existing_file_required(root, relative, repaired_len)?;
        Ok(WalRepair {
            preserved_records: replay.records.len(),
            truncated_bytes: replay.trailing_partial_bytes,
            preserved_patch_ids,
        })
    }

    /// Truncate the WAL after a successful publication that made all entries durable elsewhere.
    pub fn truncate_empty(&self) -> Result<()> {
        self.require_current_format()?;
        self.truncate_empty_authorized()
    }

    fn truncate_empty_authorized(&self) -> Result<()> {
        let (root, relative) = self.mutation()?;
        // RFC 102 Stage 5, design-v1.md §14.8: no `ensure_directory_required` here -- unlike
        // `append_patch`'s own call (a separate, not-yet-ruled-on finding from this same round, left
        // untouched), this one is established dead rather than assumed: `default_active_dir()` is in
        // `required_repository_directories()` (`layout.rs:389`), permanent from `init`, and
        // `durable_truncate_to_empty` no longer creates a missing directory to paper over its absence.
        truncate_file_empty_required(root, relative)
    }

    /// Return the next sequence number for append.
    pub fn next_sequence(&self) -> Result<u64> {
        let replay = self.replay()?;
        let Some(last) = replay.records.last() else {
            return Ok(1);
        };
        last.seq
            .checked_add(1)
            .ok_or_else(|| PrikkError::MalformedData("WAL sequence overflow".to_string()))
    }

    fn mutation(&self) -> Result<(&MutationRoot, &Path)> {
        self.mutation
            .as_ref()
            .map(|(root, relative)| (root, relative.as_path()))
            .ok_or_else(|| PrikkError::Io {
                kind: None,
                context: "WAL mutation requires a validated repository layout capability"
                    .to_string(),
            })
    }

    fn require_current_format(&self) -> Result<()> {
        self.layout
            .as_ref()
            .ok_or_else(|| PrikkError::Io {
                kind: None,
                context: "WAL mutation requires a validated repository layout capability"
                    .to_string(),
            })?
            .require_current_format()
    }

    fn read_bytes(&self) -> Result<Option<Vec<u8>>> {
        if let Some((root, relative)) = &self.mutation {
            read_file_if_exists(root, relative)
        } else {
            match fs::read(&self.path) {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
                Err(error) => Err(error.into()),
            }
        }
    }
}

fn encode_record(record: &WalRecord) -> Result<Vec<u8>> {
    let body = encode_envelope_file(&record.envelope)?;
    frame_record(record.seq, &body)
}

// RFC 149 §6c: a test helper by name and by purpose; it travels with the tests that use it.
#[cfg(any(test, feature = "test-support"))]
/// Encode one WAL record exactly as the writer would, for a test that needs the bytes without the
/// write.
pub fn encode_record_for_test(record: &WalRecord) -> Result<Vec<u8>> {
    let body = crate::foundation::file_codec::encode_envelope_file_structural(&record.envelope)?;
    frame_record(record.seq, &body)
}

fn frame_record(sequence: u64, body: &[u8]) -> Result<Vec<u8>> {
    let body_len = len_to_u64(body.len())?;
    let checksum = record_checksum(sequence, body_len, body);
    let mut out = Vec::with_capacity(WAL_HEADER_LEN + body.len());
    out.extend_from_slice(WAL_RECORD_MAGIC);
    push_u16(&mut out, WAL_RECORD_VERSION);
    push_u64(&mut out, sequence);
    push_u64(&mut out, body_len);
    out.extend_from_slice(&checksum);
    out.extend_from_slice(body);
    Ok(out)
}

/// Result of attempting to parse one frame at a given offset.
enum FrameAttempt {
    /// A complete, checksummed, decodable record.
    Record {
        record: WalRecord,
        next_offset: usize,
    },
    /// A structurally incomplete final frame -- too few bytes remain for a full header, or the
    /// header parsed but its claimed body does not fully fit the remaining bytes. Legitimate torn
    /// tail from an interrupted append; unchanged in meaning from before Stage 2. Only ever reached
    /// when `offset` is within one frame's length of the true end of `bytes` (see the module doc on
    /// `WalReplay::trailing_partial_bytes`), so there is nothing left to resync into either way.
    TrailingPartial { remaining: usize },
    /// The frame at this offset failed to validate for a reason that is not "ran out of bytes":
    /// bad magic/version, checksum mismatch, or a malformed envelope after the checksum passed.
    Invalid { message: String },
}

/// Attempt to parse one frame at `offset`. Never trusts a not-yet-checksum-validated header's own
/// `body_len` for anything beyond locating where its claimed body would end -- the checksum, not
/// the length field, is what makes a `Record` result trustworthy.
fn parse_frame_at(bytes: &[u8], offset: usize) -> FrameAttempt {
    let remaining = bytes.len().saturating_sub(offset);
    if remaining < WAL_HEADER_LEN {
        return FrameAttempt::TrailingPartial { remaining };
    }
    let header_end = offset + WAL_HEADER_LEN;
    // In range by construction: `remaining >= WAL_HEADER_LEN` was just checked above -- `.get()`
    // used anyway to satisfy `clippy::indexing_slicing`, not because this can fail.
    let Some(header) = bytes.get(offset..header_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let header_values = match parse_header(header) {
        Ok(values) => values,
        Err(err) => {
            return FrameAttempt::Invalid {
                message: err.to_string(),
            };
        }
    };
    let Ok(body_len) = usize::try_from(header_values.body_len) else {
        return FrameAttempt::Invalid {
            message: "WAL body length does not fit usize".to_string(),
        };
    };
    let Some(body_end) = header_end.checked_add(body_len) else {
        return FrameAttempt::Invalid {
            message: "WAL body end overflow".to_string(),
        };
    };
    let Some(body) = bytes.get(header_end..body_end) else {
        return FrameAttempt::TrailingPartial { remaining };
    };
    let expected = record_checksum(header_values.seq, header_values.body_len, body);
    if expected != header_values.checksum {
        return FrameAttempt::Invalid {
            message: format!("WAL checksum mismatch at byte offset {offset}"),
        };
    }
    match decode_envelope_file(body) {
        Ok(envelope) => FrameAttempt::Record {
            record: WalRecord {
                seq: header_values.seq,
                envelope,
            },
            next_offset: body_end,
        },
        Err(err) => FrameAttempt::Invalid {
            message: err.to_string(),
        },
    }
}

/// RFC 102 Stage 2: isolate-and-continue reading. A frame that fails to validate no longer aborts
/// replay -- its offset and error are recorded as a `Failed` outcome, and
/// `frame_resync::resync_to_next_magic` (RFC 102 Stage 3: extracted here, reused by `refs/log.rs`
/// and by the container read path rather than a third copy) finds the next candidate frame so every
/// subsequent sound record is still read. Corruption is therefore confined to the records it
/// actually damaged, matching amended constraint 5's blast-radius requirement (RFC 102 §3, §6.3a).
/// This never returns `Err` for a decode-level problem -- only for conditions that are not about
/// this WAL's own content, which is why the return type stays `Result` at all: none exist below,
/// `decode_records` cannot fail, kept fallible for API stability and because `parse_header`'s errors
/// are folded into `FrameAttempt::Invalid` rather than raised.
fn decode_records(bytes: &[u8]) -> Result<WalReplay> {
    let mut records = Vec::new();
    let mut record_outcomes = Vec::new();
    let mut offset = 0_usize;
    loop {
        match parse_frame_at(bytes, offset) {
            FrameAttempt::Record {
                record,
                next_offset,
            } => {
                record_outcomes.push(WalRecordOutcome {
                    offset,
                    status: WalRecordStatus::Evaluated,
                });
                records.push(record);
                offset = next_offset;
            }
            FrameAttempt::TrailingPartial { remaining } => {
                return Ok(WalReplay {
                    records,
                    trailing_partial_bytes: remaining,
                    record_outcomes,
                });
            }
            FrameAttempt::Invalid { message } => {
                record_outcomes.push(WalRecordOutcome {
                    offset,
                    status: WalRecordStatus::Failed { message },
                });
                match resync_to_next_magic(bytes, offset + 1, WAL_RECORD_MAGIC.as_slice()) {
                    Some(next) => offset = next,
                    None => {
                        return Ok(WalReplay {
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

struct WalHeader {
    seq: u64,
    body_len: u64,
    checksum: [u8; 32],
}

fn parse_header(header: &[u8]) -> Result<WalHeader> {
    let mut cursor = ByteCursor::new(header);
    let magic = cursor.read_array::<8>()?;
    if &magic != WAL_RECORD_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid WAL record magic".to_string(),
        ));
    }
    let version = cursor.read_u16()?;
    if version != WAL_RECORD_VERSION {
        return Err(PrikkError::UnsupportedFormatVersion(u32::from(version)));
    }
    let seq = cursor.read_u64()?;
    let body_len = cursor.read_u64()?;
    let checksum = cursor.read_array::<32>()?;
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in WAL header".to_string(),
        ));
    }
    Ok(WalHeader {
        seq,
        body_len,
        checksum,
    })
}

fn record_checksum(seq: u64, body_len: u64, body: &[u8]) -> [u8; 32] {
    let mut preimage = Vec::with_capacity(8 + 2 + 8 + 8 + body.len());
    preimage.extend_from_slice(WAL_RECORD_MAGIC);
    preimage.extend_from_slice(&WAL_RECORD_VERSION.to_be_bytes());
    preimage.extend_from_slice(&seq.to_be_bytes());
    preimage.extend_from_slice(&body_len.to_be_bytes());
    preimage.extend_from_slice(body);
    sha256(&preimage)
}

// DC-71: every test here sets up its scenario via real repository mutation (RepositoryLayout::init
// or equivalent), which is Linux-only; the module never compiles a non-Linux-meaningful test.
#[cfg(all(test, target_os = "linux"))]
mod tests;
