//! RFC 155 §9: `PREPO001`, a whole repository, verbatim, in one file. Export is read-only and takes
//! one consistent view under every `LockableContainer` lock (§9.3); it refuses before writing
//! anything if a session holds queued, unsealed work, or a slotted container cannot be resolved
//! (§9.1). Verify and import are separate parts (RFC 155's implementation handoff, Parts V/I-A/I-B)
//! and are not built here.
//!
//! **Streamed on both ends, memory independent of repository size** (§9.2): every section is a
//! chunked copy of one existing on-disk file, hashed incrementally
//! ([`prikk_hash::IncrementalSha256`]), never buffered whole. The object containers and the ref log
//! travel **verbatim** -- their live slot's bytes, damage included (§9.1: "Damage in an object
//! container travels verbatim"); the three generation-aware families (the ref pointer index, the
//! received index, the trust policy) travel as their **resolved** content only -- no slot letters,
//! no generation log, exactly `init` would produce for a fresh slot `a` on import.
//!
//! **Section framing reuses the existing container frame's fields** (magic, length, body,
//! checksum), with the checksum moved to a *trailer* rather than a header: the exporter knows a
//! section's `body_len` from a stat, before reading a single body byte, so writing the header first
//! and the checksum last is what makes single-pass streaming possible. The checksum covers
//! `magic || kind || body_len || body`, fed to the hasher incrementally as the body streams through.
//!
//! **A section's kind is a fixed numeric code, never a carried path** (implementation handoff's own
//! security section): the importer and verifier (built in later parts) map each code to one of
//! [`ArchiveSectionKind`]'s fixed variants; nothing in this format ever names a filesystem path.

use std::io::Write;
use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};
use prikk_hash::IncrementalSha256;
use prikk_object::ObjectType;

use crate::foundation::fsutil::len_to_u64;
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::{
    ContainerSlot, LockableContainer, RepositoryFormat, RepositoryLayout, persisted_object_types,
};
use crate::lock::acquire_container_locks;
use crate::wal::Wal;

/// `PREPO001` -- the whole archive's own magic, embedding its version the same way `PBNDL00x`
/// does (the version is the trailing digits, not a separate field).
const ARCHIVE_MAGIC: &[u8; 8] = b"PREPO001";
/// One section frame's own magic, distinct from the archive's (so a resync scan, built in a later
/// part, can tell "start of the whole file" from "start of one section" apart).
const SECTION_MAGIC: &[u8; 8] = b"PRPOSEC1";
/// The trailing manifest's own magic.
const MANIFEST_MAGIC: &[u8; 8] = b"PRPOMAN1";
/// The fixed end trailer's own magic.
const END_TRAILER_MAGIC: &[u8; 8] = b"PRPOEND1";
/// The end trailer's fixed length: magic(8) + manifest_offset(8) + manifest_length(8).
const END_TRAILER_LEN: u64 = 24;
/// Chunk size for every streamed copy -- large enough to amortize one syscall per chunk, small
/// enough that peak memory never scales with a section's own length.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// One of the fixed, allow-listed section kinds `PREPO001` ever carries. Never derived from a
/// carried path -- the importer and verifier (later parts) switch on this directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveSectionKind {
    /// One object-type container (`Patch`, `Block`, `RefState`, `Tag`, `Attestation`, `Blob`,
    /// `RecognitionClaim`), live slot's bytes, verbatim.
    ObjectContainer(ObjectType),
    /// The ref log, slot `a`'s bytes, verbatim -- never compacted (D1/D3's own confirmed finding),
    /// so "verbatim" and "resolved" coincide for this one family.
    RefLog,
    /// The ref pointer index's resolved content (no slot letter, no generation log).
    RefPointerIndex,
    /// The received index's resolved content (no slot letter, no generation log).
    ReceivedIndex,
    /// The maintainer trust policy's resolved content (no slot letter, no generation log) --
    /// carried as an inert record (§9.1); never adopted on import.
    TrustPolicy,
    /// The adopted maintainer key material, verbatim -- carried as an inert record (§9.1); never
    /// adopted on import.
    MaintainerKeys,
    /// The author key material, verbatim -- carried and **recorded** on import (§9.1), through the
    /// existing DC-78 conflict-checked path.
    AuthorKeys,
}

impl ArchiveSectionKind {
    const fn wire_code(self) -> u16 {
        match self {
            Self::ObjectContainer(ObjectType::Patch) => 0,
            Self::ObjectContainer(ObjectType::Block) => 1,
            Self::ObjectContainer(ObjectType::RefState) => 2,
            Self::ObjectContainer(ObjectType::Tag) => 3,
            Self::ObjectContainer(ObjectType::Attestation) => 4,
            Self::ObjectContainer(ObjectType::Blob) => 5,
            Self::ObjectContainer(ObjectType::RecognitionClaim) => 6,
            // Every other `ObjectType` variant (`RefUpdate`, and anything added later) has no
            // archive section -- `persisted_object_types()` is the only source this module ever
            // iterates, so this arm is unreachable in practice; it must still resolve to a stable
            // code rather than panic, in case a future caller constructs this kind directly.
            Self::ObjectContainer(_) => 9999,
            Self::RefLog => 7,
            Self::RefPointerIndex => 8,
            Self::ReceivedIndex => 9,
            Self::TrustPolicy => 10,
            Self::MaintainerKeys => 11,
            Self::AuthorKeys => 12,
        }
    }

    /// A short, human-readable label for refusal/report text -- never a filesystem path.
    fn label(self) -> &'static str {
        match self {
            Self::ObjectContainer(ObjectType::Patch) => "patch container",
            Self::ObjectContainer(ObjectType::Block) => "block container",
            Self::ObjectContainer(ObjectType::RefState) => "ref-state container",
            Self::ObjectContainer(ObjectType::Tag) => "tag container",
            Self::ObjectContainer(ObjectType::Attestation) => "attestation container",
            Self::ObjectContainer(ObjectType::Blob) => "blob container",
            Self::ObjectContainer(ObjectType::RecognitionClaim) => "recognition-claim container",
            Self::ObjectContainer(_) => "object container",
            Self::RefLog => "ref log",
            Self::RefPointerIndex => "ref pointer index",
            Self::ReceivedIndex => "received index",
            Self::TrustPolicy => "trust policy",
            Self::MaintainerKeys => "maintainer keys",
            Self::AuthorKeys => "author keys",
        }
    }
}

/// One finished section's own manifest entry.
struct ManifestEntry {
    kind: ArchiveSectionKind,
    offset: u64,
    length: u64,
    checksum: [u8; 32],
}

/// Summary of an archive export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveExportReport {
    /// The on-disk repository format (`layout::RepositoryFormat`) that produced this archive.
    pub repository_format: u32,
    /// The exporting `prikk` build's own version (`CARGO_PKG_VERSION`).
    pub tool_version: String,
    /// Number of sections written (object containers present, plus the six non-object sections
    /// always written).
    pub section_count: usize,
    /// Total archive bytes written, including the manifest and end trailer.
    pub total_bytes: u64,
}

/// Export the whole repository at `layout` into `out`, streamed. Takes every `LockableContainer`
/// lock for the whole call (§9.3): a `seal`, `commit`, `compact`, or any other container writer
/// fails fast (`LockConflict`) for the duration, so the archive is the repository strictly before
/// or after any concurrent write, never a mix. Refuses before writing anything (§9.1) if a session
/// holds queued, unsealed work, or a slotted family's generation log cannot be resolved.
pub fn export_archive(
    layout: &RepositoryLayout,
    out: &mut dyn Write,
) -> Result<ArchiveExportReport> {
    let _locks = acquire_container_locks(
        layout,
        &[
            LockableContainer::ObjectStore,
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
            LockableContainer::TrustPolicy,
        ],
    )?;

    // Test seam, unreachable from production: runs once, with export's own five-lock hold still
    // in effect, so a test can prove a concurrent writer fails fast for the *whole* run -- not
    // merely that `acquire_container_locks` itself works, which `lock.rs`'s own tests already
    // cover.
    #[cfg(test)]
    if let Some(check) = AFTER_EXPORT_LOCKS_ACQUIRED.with(|slot| slot.borrow_mut().take()) {
        check();
    }

    refuse_on_queued_unsealed_work(layout)?;

    // Resolved **before** any byte is written -- an `Err` here (an ambiguous lost generation log,
    // or genuine interior damage in the resolved content) must leave the destination untouched.
    let pointer_index_source = resolve_pointer_index_source(layout)?;
    let received_index_source = resolve_received_index_source(layout)?;
    let trust_policy_source = resolve_trust_policy_source(layout)?;

    out.write_all(ARCHIVE_MAGIC)?;
    let mut offset: u64 = 8;
    let mut manifest = Vec::new();

    for object_type in persisted_object_types() {
        let path = layout.container_slot_path(object_type, ContainerSlot::A);
        let body_len = stat_len(&path)?;
        let entry = write_section(
            out,
            ArchiveSectionKind::ObjectContainer(object_type),
            &path,
            body_len,
            offset,
        )?;
        offset += section_total_len(entry.length)?;
        manifest.push(entry);
    }

    let ref_log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    let ref_log_len = stat_len(&ref_log_path)?;
    let entry = write_section(
        out,
        ArchiveSectionKind::RefLog,
        &ref_log_path,
        ref_log_len,
        offset,
    )?;
    offset += section_total_len(entry.length)?;
    manifest.push(entry);

    for (kind, (path, body_len)) in [
        (ArchiveSectionKind::RefPointerIndex, pointer_index_source),
        (ArchiveSectionKind::ReceivedIndex, received_index_source),
        (ArchiveSectionKind::TrustPolicy, trust_policy_source),
    ] {
        let entry = write_section(out, kind, &path, body_len, offset)?;
        offset += section_total_len(entry.length)?;
        manifest.push(entry);
    }

    let maintainer_keys_path = layout.trust_key_container_path();
    let maintainer_keys_len = stat_len(&maintainer_keys_path)?;
    let entry = write_section(
        out,
        ArchiveSectionKind::MaintainerKeys,
        &maintainer_keys_path,
        maintainer_keys_len,
        offset,
    )?;
    offset += section_total_len(entry.length)?;
    manifest.push(entry);

    let author_keys_path = layout.author_key_container_path();
    let author_keys_len = stat_len(&author_keys_path)?;
    let entry = write_section(
        out,
        ArchiveSectionKind::AuthorKeys,
        &author_keys_path,
        author_keys_len,
        offset,
    )?;
    offset += section_total_len(entry.length)?;
    manifest.push(entry);

    let repository_format = repository_format_number(layout.format());
    let tool_version = env!("CARGO_PKG_VERSION").to_string();
    let manifest_offset = offset;
    let manifest_length = write_manifest(out, &manifest, repository_format, &tool_version)?;
    write_end_trailer(out, manifest_offset, manifest_length)?;

    let total_bytes = manifest_offset + manifest_length + END_TRAILER_LEN;
    Ok(ArchiveExportReport {
        repository_format,
        tool_version,
        section_count: manifest.len(),
        total_bytes,
    })
}

/// §9.1: "a session holds queued, unsealed commits ... the message names the session, the count,
/// and `prikk seal`." Every active session, not only `default` -- a queued patch lives only in its
/// own session's WAL until that session's own `seal` runs, and an archive taken without it would
/// silently lose signed work no later read of the archive could recover.
fn refuse_on_queued_unsealed_work(layout: &RepositoryLayout) -> Result<()> {
    let mut queued = Vec::new();
    for name in layout.active_session_names()? {
        let replay = Wal::for_layout(layout, &name).replay()?;
        if !replay.records.is_empty() {
            queued.push((name.to_string_lossy().into_owned(), replay.records.len()));
        }
    }
    if queued.is_empty() {
        return Ok(());
    }
    let mut message = String::from(
        "export refuses: at least one active session holds queued, unsealed commits, which live \
         only in that session's own WAL and would be silently lost from the archive -- run \
         `prikk seal` for each named session first, then export again:",
    );
    for (name, count) in &queued {
        message.push_str(&format!(
            "\n  session {name:?}: {count} queued, unsealed commit(s)"
        ));
    }
    Err(PrikkError::Precondition(message))
}

/// Same damage/ambiguous text and decode/fold closures `verify.rs`'s own
/// `check_generation_log_deductions` uses for the ref pointer index -- copied, not shared via a
/// named constant, matching this codebase's own established practice for these three texts (see
/// `trust_index.rs`'s own doc note on the Q1 review: multiple literal copies already exist across
/// `verify.rs`/`compact.rs`/`repair_tails.rs`).
fn resolve_pointer_index_source(layout: &RepositoryLayout) -> Result<(PathBuf, u64)> {
    let slot = resolve_live_slot(
        layout,
        &layout.ref_pointer_index_generation_log_path(),
        &layout.ref_pointer_index_slot_path(ContainerSlot::A),
        &layout.ref_pointer_index_slot_path(ContainerSlot::B),
        "ref pointer index has a damaged entry; run `prikk doctor --rebuild-pointer-index \
         --plan-only`, then `prikk doctor --rebuild-pointer-index`",
        "ref pointer index's generation log is lost, and its two slots fit two different \
         histories; run `prikk doctor --rebuild-pointer-index --plan-only`, then `prikk doctor \
         --rebuild-pointer-index` -- the ref log decides, not either slot",
        crate::refs::decode_pointer_index_entries_for_resolver,
        crate::refs::fold_one_pointer_index_entry,
    )?;
    let replay = crate::refs::replay_pointer_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "ref pointer index has a damaged entry; run `prikk doctor --rebuild-pointer-index \
             --plan-only`, then `prikk doctor --rebuild-pointer-index`"
                .to_string(),
        ));
    }
    let path = layout.ref_pointer_index_slot_path(slot);
    let sound_len = sound_length(&path, replay.trailing_partial_bytes)?;
    Ok((path, sound_len))
}

fn resolve_received_index_source(layout: &RepositoryLayout) -> Result<(PathBuf, u64)> {
    let slot = resolve_live_slot(
        layout,
        &layout.received_index_generation_log_path(),
        &layout.received_index_slot_path(ContainerSlot::A),
        &layout.received_index_slot_path(ContainerSlot::B),
        "received-ref index has a damaged entry; no repair exists -- preserve the repository; \
         the way out is a copy of this repository's own `.prikk/` directory from a backup taken \
         before the damage",
        "the received index's generation log is lost, and its two slots fit two different \
         histories; run `prikk compact --received-index --keep-slot a|b --plan-only` to see \
         both slots and choose, or restore the repository's whole `.prikk/` from a backup \
         taken before the log was lost",
        crate::received::received_index::decode_received_index_entries_for_resolver,
        crate::received::received_index::fold_one_received_index_entry,
    )?;
    let replay = crate::received::received_index::replay_received_index(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "received-ref index has a damaged entry; no repair exists -- preserve the \
             repository; the way out is a copy of this repository's own `.prikk/` directory \
             from a backup taken before the damage"
                .to_string(),
        ));
    }
    let path = layout.received_index_slot_path(slot);
    let sound_len = sound_length(&path, replay.trailing_partial_bytes)?;
    Ok((path, sound_len))
}

fn resolve_trust_policy_source(layout: &RepositoryLayout) -> Result<(PathBuf, u64)> {
    let slot = resolve_live_slot(
        layout,
        &layout.trust_policy_generation_log_path(),
        &layout.trust_policy_container_slot_path(ContainerSlot::A),
        &layout.trust_policy_container_slot_path(ContainerSlot::B),
        "trust policy container has a damaged snapshot; no repair exists -- preserve the \
         repository; the way out is a copy of this repository's own `.prikk/` directory from a \
         backup taken before the damage, then re-apply every trust change made since that backup",
        "the trust policy's generation log is lost, and its two slots fit two different \
         histories (one trusts a key the other has revoked); run `prikk compact --trust-policy \
         --keep-slot a|b --plan-only` to see both slots and choose, or restore the repository's \
         whole `.prikk/` from a backup taken before the log was lost, then re-apply every trust \
         change made since that backup",
        crate::trust_index::decode_trust_policy_entries_for_resolver,
        crate::trust_index::fold_one_trust_policy_entry,
    )?;
    let replay = crate::trust_index::replay_trust_policy(layout)?;
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "trust policy container has a damaged snapshot; no repair exists -- preserve the \
             repository; the way out is a copy of this repository's own `.prikk/` directory \
             from a backup taken before the damage, then re-apply every trust change made since \
             that backup"
                .to_string(),
        ));
    }
    let path = layout.trust_policy_container_slot_path(slot);
    let sound_len = sound_length(&path, replay.trailing_partial_bytes)?;
    Ok((path, sound_len))
}

/// `trailing_partial_bytes` is "only meaningful when non-zero" (the three `*Replay` types' own
/// doc) -- `tail_offset` is not a reliable sound length in the common, no-torn-tail case. The sound
/// prefix is always `stat(path) - trailing_partial_bytes`, which is correct whether or not there is
/// a tail at all.
fn sound_length(path: &Path, trailing_partial_bytes: usize) -> Result<u64> {
    let total = stat_len(path)?;
    let trailing = len_to_u64(trailing_partial_bytes)?;
    total.checked_sub(trailing).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "{} is {total} bytes, shorter than its own replay's {trailing} trailing partial bytes",
            path.display()
        ))
    })
}

fn stat_len(path: &Path) -> Result<u64> {
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.len()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(0),
        Err(err) => Err(err.into()),
    }
}

/// `SECTION_MAGIC`(8) + `kind`(2) + `body_len`(8) + `checksum`(32).
const SECTION_HEADER_AND_TRAILER_LEN: u64 = 8 + 2 + 8 + 32;

fn section_total_len(body_len: u64) -> Result<u64> {
    body_len
        .checked_add(SECTION_HEADER_AND_TRAILER_LEN)
        .ok_or_else(|| PrikkError::Integrity("archive section length overflow".to_string()))
}

/// Write one section: header (magic, kind, `body_len` -- known from a stat, before any body byte is
/// read), then exactly `body_len` bytes streamed from `source` in fixed chunks, then the checksum
/// trailer. Single pass over `source`; `source`'s own content past `body_len` (if any -- a
/// resolved family's own torn tail, excluded by design) is never read.
fn write_section(
    out: &mut dyn Write,
    kind: ArchiveSectionKind,
    source: &Path,
    body_len: u64,
    offset: u64,
) -> Result<ManifestEntry> {
    let code = kind.wire_code();
    out.write_all(SECTION_MAGIC)?;
    out.write_all(&code.to_be_bytes())?;
    out.write_all(&body_len.to_be_bytes())?;

    let mut hasher = IncrementalSha256::new();
    hasher.update(SECTION_MAGIC);
    hasher.update(&code.to_be_bytes());
    hasher.update(&body_len.to_be_bytes());

    let written = stream_copy(source, body_len, out, &mut hasher)?;
    if written != body_len {
        return Err(PrikkError::Integrity(format!(
            "archive export: {} ended after {written} of {body_len} declared bytes -- it shrank \
             while the whole-repository lock was held, which should not be possible",
            kind.label()
        )));
    }

    let checksum = hasher.finalize();
    out.write_all(&checksum)?;
    Ok(ManifestEntry {
        kind,
        offset,
        length: body_len,
        checksum,
    })
}

/// Copy exactly `len` bytes of `source`, from its start, in `STREAM_CHUNK_BYTES` chunks, feeding
/// each chunk to `hasher` and `out`. Never opens more than one chunk buffer at a time, independent
/// of `len`. A source shorter than `len` returns the shorter count rather than erroring -- the
/// caller compares it against `len` itself (the one call site above), so a shrink mid-export is
/// reported with the section's own name, not a bare I/O error.
fn stream_copy(
    source: &Path,
    len: u64,
    out: &mut dyn Write,
    hasher: &mut IncrementalSha256,
) -> Result<u64> {
    use std::io::Read;
    let mut file = match std::fs::File::open(source) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let mut buffer = vec![0u8; STREAM_CHUNK_BYTES];
    let mut remaining = len;
    let mut written = 0u64;
    while remaining > 0 {
        let take = remaining.min(STREAM_CHUNK_BYTES as u64) as usize;
        let Some(slice) = buffer.get_mut(..take) else {
            return Err(PrikkError::Integrity(
                "archive export: chunk size exceeds the stream buffer".to_string(),
            ));
        };
        let read = file.read(slice)?;
        if read == 0 {
            break;
        }
        let Some(chunk) = buffer.get(..read) else {
            return Err(PrikkError::Integrity(
                "archive export: read more bytes than the stream buffer holds".to_string(),
            ));
        };
        hasher.update(chunk);
        out.write_all(chunk)?;
        written += read as u64;
        remaining -= read as u64;
    }
    Ok(written)
}

/// `MANIFEST_MAGIC`(8) + `repository_format`(4) + `tool_version_len`(2) + `tool_version` +
/// `count`(8) + repeated `{kind(2) | offset(8) | length(8) | checksum(32)}`.
fn write_manifest(
    out: &mut dyn Write,
    entries: &[ManifestEntry],
    repository_format: u32,
    tool_version: &str,
) -> Result<u64> {
    let mut body = Vec::new();
    body.extend_from_slice(MANIFEST_MAGIC);
    body.extend_from_slice(&repository_format.to_be_bytes());
    let tool_version_bytes = tool_version.as_bytes();
    let tool_version_len = u16::try_from(tool_version_bytes.len())
        .map_err(|_| PrikkError::Integrity("tool version string too long".to_string()))?;
    body.extend_from_slice(&tool_version_len.to_be_bytes());
    body.extend_from_slice(tool_version_bytes);
    body.extend_from_slice(&len_to_u64(entries.len())?.to_be_bytes());
    for entry in entries {
        body.extend_from_slice(&entry.kind.wire_code().to_be_bytes());
        body.extend_from_slice(&entry.offset.to_be_bytes());
        body.extend_from_slice(&entry.length.to_be_bytes());
        body.extend_from_slice(&entry.checksum);
    }
    out.write_all(&body)?;
    len_to_u64(body.len())
}

fn write_end_trailer(
    out: &mut dyn Write,
    manifest_offset: u64,
    manifest_length: u64,
) -> Result<()> {
    out.write_all(END_TRAILER_MAGIC)?;
    out.write_all(&manifest_offset.to_be_bytes())?;
    out.write_all(&manifest_length.to_be_bytes())?;
    Ok(())
}

const fn repository_format_number(format: RepositoryFormat) -> u32 {
    match format {
        RepositoryFormat::CurrentV6 => 6,
        RepositoryFormat::V7 => 7,
    }
}

#[cfg(test)]
thread_local! {
    static AFTER_EXPORT_LOCKS_ACQUIRED: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production: run `check` once, immediately after `export_archive`
/// takes its five container locks and before it reads or writes anything else.
#[cfg(test)]
pub(crate) fn after_export_locks_acquired_for_test(check: impl FnOnce() + 'static) {
    AFTER_EXPORT_LOCKS_ACQUIRED.with(|slot| *slot.borrow_mut() = Some(Box::new(check)));
}

#[cfg(test)]
mod tests;
