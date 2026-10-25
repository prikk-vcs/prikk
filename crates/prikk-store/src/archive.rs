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

mod import;
mod verify;

pub use import::{
    ArchiveImportOptions, ArchiveImportReport, CancelImportReport, CancelImportStep, LandingOrigin,
    LandingOutcome, PlannedRefLanding, cancel_import, import_archive, interrupted_import_short_id,
};
pub use verify::{
    ArchiveUnreferencedRemnant, ArchiveVerifyReport, CarriedMaintainerKeySummary, verify_archive,
};

use prikk_error::{PrikkError, Result};
use prikk_hash::IncrementalSha256;
use prikk_object::ObjectType;

use crate::bundle::DEFAULT_BUNDLE_MAX_OBJECT_BYTES;
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::generation::resolve_live_slot;
use crate::foundation::layout::{
    ContainerSlot, DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryFormat, RepositoryLayout,
    persisted_object_types,
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
/// The end trailer's fixed length: magic(8) + manifest_offset(8) + manifest_length(8) +
/// manifest_checksum(32). V2's own fix (review `rfc155-impl-V-review-v1`, finding V1): the manifest
/// itself was not checksummed anywhere, so a flipped byte inside it (the `repository_format` field,
/// confirmed) passed `archive verify` as consistent. The checksum lives in the trailer, not the
/// manifest body, because it must cover the *whole* manifest body -- a checksum inside the thing it
/// checksums cannot cover its own bytes.
const END_TRAILER_LEN: u64 = 56;
/// Chunk size for every streamed copy -- large enough to amortize one syscall per chunk, small
/// enough that peak memory never scales with a section's own length.
const STREAM_CHUNK_BYTES: usize = 64 * 1024;

/// One of the fixed, allow-listed section kinds `PREPO001` ever carries. Never derived from a
/// carried path -- the importer and verifier (later parts) switch on this directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
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

    /// The inverse of [`Self::wire_code`] -- `None` for any code this format does not define,
    /// which [`verify::verify_archive`]'s own manifest validation treats as a refusal, never a
    /// silent skip (the implementation handoff's own security section: "an unknown ... section is a
    /// refusal naming it").
    fn from_wire_code(code: u16) -> Option<Self> {
        Some(match code {
            0 => Self::ObjectContainer(ObjectType::Patch),
            1 => Self::ObjectContainer(ObjectType::Block),
            2 => Self::ObjectContainer(ObjectType::RefState),
            3 => Self::ObjectContainer(ObjectType::Tag),
            4 => Self::ObjectContainer(ObjectType::Attestation),
            5 => Self::ObjectContainer(ObjectType::Blob),
            6 => Self::ObjectContainer(ObjectType::RecognitionClaim),
            7 => Self::RefLog,
            8 => Self::RefPointerIndex,
            9 => Self::ReceivedIndex,
            10 => Self::TrustPolicy,
            11 => Self::MaintainerKeys,
            12 => Self::AuthorKeys,
            _ => return None,
        })
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

/// Summary of an archive export. Part E's own review: *"'sections: 13' says nothing about whether
/// the backup holds the user's work."* This reports what a user can actually check against `branch
/// list`/their own expectation -- objects by type, and refs by namespace -- not an internal framing
/// detail.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ArchiveExportReport {
    /// The on-disk repository format (`layout::RepositoryFormat`) that produced this archive.
    pub repository_format: u32,
    /// The exporting `prikk` build's own version (`CARGO_PKG_VERSION`).
    pub tool_version: String,
    /// Sound record count per object type, in `persisted_object_types()`'s own order. Counted by
    /// skipping each record's own declared body length via seek, never by reading it -- reading
    /// every object's bytes just to count them would reintroduce the unbounded-memory shape this
    /// design otherwise avoids.
    pub object_counts: Vec<(ObjectType, usize)>,
    /// Local branch count (`heads/*`), from the resolved pointer index -- the same list `branch
    /// list` reads.
    pub local_branches: usize,
    /// Tag count (`tags/*`), from the same resolved pointer index.
    pub tags: usize,
    /// Received-ref count, from the resolved received index.
    pub received_refs: usize,
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
    refuse_on_over_bound_object(layout)?;
    refuse_on_blocking_ref_publication(layout)?;

    export_archive_body(layout, out)
}

/// 0.52.0 step 1 Part A, item 2: a test-only seam so `archive::verify::tests`' own
/// `*_is_damage_in_the_archive_too` controls can still produce archive bytes carrying a blocking
/// pointer<->log state, now that [`export_archive`] itself refuses one -- proving the archive's own
/// closure pass still catches this state too, as defense in depth for an archive written by an
/// older build or assembled by some other means, not only the export path this part now guards.
/// Identical to [`export_archive`] except it skips [`refuse_on_blocking_ref_publication`]; every
/// other precondition and the whole write itself are shared, not re-derived.
#[cfg(test)]
pub(crate) fn export_archive_bypassing_blocking_ref_publication_for_test(
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
    refuse_on_queued_unsealed_work(layout)?;
    refuse_on_over_bound_object(layout)?;
    export_archive_body(layout, out)
}

fn export_archive_body(
    layout: &RepositoryLayout,
    out: &mut dyn Write,
) -> Result<ArchiveExportReport> {
    // Resolved **before** any byte is written -- an `Err` here (an ambiguous lost generation log,
    // or genuine interior damage in the resolved content) must leave the destination untouched.
    let pointer_index_source = resolve_pointer_index_source(layout)?;
    let received_index_source = resolve_received_index_source(layout)?;
    let trust_policy_source = resolve_trust_policy_source(layout)?;

    out.write_all(ARCHIVE_MAGIC)?;
    let mut offset: u64 = 8;
    let mut manifest = Vec::new();

    let mut object_counts = Vec::new();
    for object_type in persisted_object_types() {
        let path = layout.container_slot_path(object_type, ContainerSlot::A);
        let body_len = stat_len(&path)?;
        object_counts.push((
            object_type,
            count_object_records(&path, object_type, body_len)?,
        ));
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
    let (manifest_length, manifest_checksum) =
        write_manifest(out, &manifest, repository_format, &tool_version)?;
    write_end_trailer(out, manifest_offset, manifest_length, manifest_checksum)?;

    let total_bytes = manifest_offset + manifest_length + END_TRAILER_LEN;
    let ref_pointers = crate::refs::RefStore::new(layout.clone()).list_ref_pointers()?;
    let local_branches = ref_pointers
        .iter()
        .filter(|pointer| pointer.ref_name.starts_with("heads/"))
        .count();
    let tags = ref_pointers
        .iter()
        .filter(|pointer| pointer.ref_name.starts_with("tags/"))
        .count();
    let received_refs = crate::received::list_received_pointers(layout)?.len();
    Ok(ArchiveExportReport {
        repository_format,
        tool_version,
        object_counts,
        local_branches,
        tags,
        received_refs,
        total_bytes,
    })
}

/// Count sound records in one object-type container by skipping each record's own declared body
/// (seek, never a read) -- the report's own object count needs no more than this, and reading every
/// body just to count it would reintroduce exactly the unbounded-memory shape this design otherwise
/// avoids. Stops counting (best-effort) at the first record whose header does not match, the same
/// "a torn tail is not content" reasoning `write_section`'s own sound-length resolution already
/// uses elsewhere in this module.
fn count_object_records(path: &Path, object_type: ObjectType, sound_len: u64) -> Result<usize> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };
    let magic = crate::foundation::container::container_magic(object_type)?;
    let mut remaining = sound_len;
    let mut count = 0usize;
    while remaining >= crate::foundation::container::CONTAINER_HEADER_LEN as u64 {
        let mut header = [0u8; crate::foundation::container::CONTAINER_HEADER_LEN];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        remaining -= crate::foundation::container::CONTAINER_HEADER_LEN as u64;
        let record_magic: [u8; 8] = header[0..8].try_into().unwrap_or([0u8; 8]);
        let body_len = u64::from_be_bytes(header[10..18].try_into().unwrap_or([0u8; 8]));
        if record_magic != *magic || body_len > remaining {
            break;
        }
        if file
            .seek(SeekFrom::Current(
                i64::try_from(body_len).unwrap_or(i64::MAX),
            ))
            .is_err()
        {
            break;
        }
        remaining -= body_len;
        count += 1;
    }
    Ok(count)
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

/// 0.52.0 step 1 Part A, item 1 (external review 023): refuse before writing anything if any
/// carried object declares a body over the same per-object bound `archive import`/`archive verify`
/// already enforce when reading one back (`DEFAULT_BUNDLE_MAX_OBJECT_BYTES`,
/// `archive/verify.rs:907`) -- otherwise export succeeds today and only that later read refuses,
/// after the user already has a file they believe is a backup. Seek-only, the same scanning shape
/// `count_object_records` (below) already uses to report sound record counts: a record's own
/// declared `body_len` is read from its header and compared to the bound before any body byte is
/// read, so an over-bound record is named by its type and position, never by reading the oversized
/// body itself.
fn refuse_on_over_bound_object(layout: &RepositoryLayout) -> Result<()> {
    let bound = object_over_bound_threshold();
    for object_type in persisted_object_types() {
        let path = layout.container_slot_path(object_type, ContainerSlot::A);
        let sound_len = stat_len(&path)?;
        if let Some(position) = first_over_bound_record(&path, object_type, sound_len, bound)? {
            return Err(PrikkError::Precondition(format!(
                "export refuses: the {} holds a record (position {position}) declaring more than \
                 the per-object bound of {bound} bytes -- `archive import` would refuse it too; \
                 this repository cannot be archived as is",
                ArchiveSectionKind::ObjectContainer(object_type).label()
            )));
        }
    }
    Ok(())
}

/// 0.52.0 step 1 Part A round 2 (A-2): the real per-object bound, except on this thread when a test
/// has armed [`lower_object_over_bound_threshold_for_test`] -- so an end-to-end "nothing written"
/// test (including one driven from `prikk-cli`'s own test binary, through its real
/// `durable_output` write wrapper) can exercise the real refusal path without building a real
/// 257 MiB object. `feature = "test-support"`, not `cfg(test)`: `prikk-cli`'s own tests build
/// `prikk-store` as an ordinary dependency with that feature enabled (its own `Cargo.toml`), not as
/// `prikk-store`'s own `cfg(test)` build, the same reason `anchor_uses_for_test_support` and this
/// crate's other cross-crate test seams are gated the same way. Production code never arms this,
/// and this function itself is always compiled, always returning the real bound outside tests.
fn object_over_bound_threshold() -> u64 {
    #[cfg(feature = "test-support")]
    {
        if let Some(bound) = OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST.with(|cell| *cell.borrow()) {
            return bound;
        }
    }
    DEFAULT_BUNDLE_MAX_OBJECT_BYTES as u64
}

#[cfg(feature = "test-support")]
std::thread_local! {
    static OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST: std::cell::RefCell<Option<u64>> =
        const { std::cell::RefCell::new(None) };
}

/// Arm a lowered per-object bound for `refuse_on_over_bound_object`, on this thread only, until
/// [`clear_object_over_bound_threshold_for_test`] runs. Re-exported at the crate root under
/// `feature = "test-support"` so `prikk-cli`'s own tests can call it.
#[cfg(feature = "test-support")]
pub fn lower_object_over_bound_threshold_for_test(bound: u64) {
    OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST.with(|cell| *cell.borrow_mut() = Some(bound));
}

/// Disarm whatever [`lower_object_over_bound_threshold_for_test`] last armed on this thread.
#[cfg(feature = "test-support")]
pub fn clear_object_over_bound_threshold_for_test() {
    OBJECT_OVER_BOUND_THRESHOLD_FOR_TEST.with(|cell| *cell.borrow_mut() = None);
}

/// The zero-based position of the first record in one object-type container whose own declared
/// body length exceeds `bound`, or `None` if every sound record is within it. Never reads a body:
/// the same seek-only walk `count_object_records` uses, with one extra comparison per record.
fn first_over_bound_record(
    path: &Path,
    object_type: ObjectType,
    sound_len: u64,
    bound: u64,
) -> Result<Option<usize>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let magic = crate::foundation::container::container_magic(object_type)?;
    let mut remaining = sound_len;
    let mut position = 0usize;
    while remaining >= crate::foundation::container::CONTAINER_HEADER_LEN as u64 {
        let mut header = [0u8; crate::foundation::container::CONTAINER_HEADER_LEN];
        if file.read_exact(&mut header).is_err() {
            break;
        }
        remaining -= crate::foundation::container::CONTAINER_HEADER_LEN as u64;
        let record_magic: [u8; 8] = header[0..8].try_into().unwrap_or([0u8; 8]);
        let body_len = u64::from_be_bytes(header[10..18].try_into().unwrap_or([0u8; 8]));
        if record_magic != *magic || body_len > remaining {
            break;
        }
        if body_len > bound {
            return Ok(Some(position));
        }
        if file
            .seek(SeekFrom::Current(
                i64::try_from(body_len).unwrap_or(i64::MAX),
            ))
            .is_err()
        {
            break;
        }
        remaining -= body_len;
        position += 1;
    }
    Ok(None)
}

/// 0.52.0 step 1 Part A, item 2 (C1 review v3, N2); round 2 (A-1); round 3 (A-5): refuse before
/// writing anything if the source repository sits in a ref-publication state the store's own
/// `verify` already blocks on (`PRIKK-VERIFY-REF-POINTER-LEADS-LOG`/`-DIVERGENCE`) -- shares the
/// real classification and post-processing rule, never a re-derivation of either, and **never the
/// whole `verify_repository`**: round 2 called it directly, and the architect measured the
/// regression that caused -- `verify_repository`'s own `Objects` stage walks every persisted object
/// across every type (`verify::objects::verify_objects`), O(store); at 1 GiB of blob content, export
/// went from 12 MiB/0.8s to 2.18 GiB/5.1s peak. RFC 155 bounds export's own memory by the largest
/// object, not the store.
///
/// **The narrow path, each input as cheap as `verify_repository_with_options` itself makes it:**
/// - the issues themselves: `classify_live_pointer_log_issues` (restored from round 1, removed by
///   round 2 when it switched to the whole verify) -- the identical `read_pointers`/`read_logs`/
///   `classify_ref_items` construction `verify_refs_with` runs, O(refs);
/// - `records`: the active WAL's own replay. By this point `refuse_on_queued_unsealed_work` has
///   already refused on any session holding queued work, so the default session's own WAL is empty
///   or nearly so -- a second, cheap read of a near-empty file, not a re-derivation of the refusal
///   itself;
/// - `metadata`: `crate::verify::classify_active_wal_metadata`, the identical function and call
///   shape `verify_repository_with_options` uses (`DEFAULT_ACTIVE_NAME`, `records.is_empty()`);
/// - `trust_is_valid`: **not** `verify_repository`'s own `objects_evaluated && trust_verifier.issues.
///   is_empty()` (an O(store) accumulator across every object's own trust check) -- that answers a
///   broader question this narrow path does not need an answer to. `ref_publication.rs`'s own doc
///   on `trust_is_valid` names what it actually gates: whether `ref_completion::plan_ref_completion`
///   even gets a chance to run for a lead (if false, `mark_unproved` fires without calling it at
///   all). `plan_ref_completion` is itself a complete, self-contained, read-only rule (confirmed by
///   reading it): condition (a) re-verifies the *specific* leading RefState's own signature against
///   the *current* trust policy (`load_maintainer_trust_policy`/`verify_trusted_publication_
///   envelope`), independently of anything this function computes -- and `prikk ref complete`'s own
///   real write calls this same `plan_ref_completion` again, fresh, before ever touching the store.
///   So a `trust_is_valid` that is too optimistic here never bypasses a real safety check: at worst
///   it lets `plan_ref_completion` run when the broader, whole-store question would have said not to
///   bother, and `plan_ref_completion`'s own fresh condition (a) still refuses if that specific
///   lead's own signature does not verify. The narrow, correct, and cheap question this path asks
///   instead is **whether a trust policy actually exists and replays without damage** --
///   `crate::trust_index::read_current_trust_policy_snapshot(layout)` (already `pub(crate)`,
///   already O(trust-key-container): it is `replay_trust_policy` plus the one fact this narrow path
///   needs, `Ok(None)` meaning no maintainer key was ever adopted here), `.is_some()`. Found
///   empirically, not assumed: `replay_trust_policy(layout)?.has_item_failure()` alone reads `false`
///   for an empty, never-adopted policy too (zero entries is not damage), which let
///   `plan_ref_completion` run and hard-fail on `load_maintainer_trust_policy`'s own "no maintainer
///   key is adopted" precondition rather than this function cleanly marking the issue unproved --
///   caught by `export_refuses_a_source_with_an_unprovable_ref_divergence`, which builds exactly
///   this shape. A genuinely damaged trust-policy container still propagates as a real `Err` here
///   (the same `?` any other precondition in this function already uses), consistent with
///   `resolve_trust_policy_source` refusing on the identical condition later in this same function.
fn refuse_on_blocking_ref_publication(layout: &RepositoryLayout) -> Result<()> {
    let ref_log = crate::refs::read_and_decode_ref_log(layout, "verify-scan");
    let mut issues = crate::refs::verify::classify_live_pointer_log_issues(layout, &ref_log)?;

    let records = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)
        .replay()?
        .records;
    let metadata = crate::verify::classify_active_wal_metadata(
        layout,
        DEFAULT_ACTIVE_NAME,
        records.is_empty(),
    )?;
    // `read_current_trust_policy_snapshot` is `replay_trust_policy` plus the one extra fact this
    // narrow path actually needs: `Ok(None)` means no maintainer key was ever adopted here -- a
    // normal, common repository state, not damage (it already fails closed on real damage via its
    // own `has_item_failure` check, propagated by `?` below exactly as `resolve_trust_policy_source`
    // already does later in this same function). Found empirically: `replay_trust_policy(layout)?.
    // has_item_failure()` alone reads `false` for an empty, never-adopted policy too, which let
    // `plan_ref_completion` run and hard-fail on `load_maintainer_trust_policy`'s own "no maintainer
    // key is adopted" precondition instead of this function cleanly marking the issue unproved.
    let trust_is_valid = crate::trust_index::read_current_trust_policy_snapshot(layout)?.is_some();
    crate::verify::require_retained_evidence(
        layout,
        &records,
        &metadata,
        trust_is_valid,
        &mut issues,
    )?;

    let blocking: Vec<_> = issues.iter().filter(|issue| issue.blocking).collect();
    if blocking.is_empty() {
        return Ok(());
    }
    let mut message = String::from(
        "export refuses: at least one ref has a publication state this repository's own `verify` \
         already blocks on, which an archive taken now would carry and then refuse on import -- \
         see the remedy named below for each ref, then export again:",
    );
    for issue in blocking {
        let ref_name = issue.ref_name.as_deref().unwrap_or("(repository-wide)");
        message.push_str(&format!(
            "\n  ref {ref_name}: {} ({})",
            issue.message, issue.code
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
/// `count`(8) + repeated `{kind(2) | offset(8) | length(8) | checksum(32)}`. Returns the body's own
/// length and its SHA-256 (over every byte written here, in order) -- the end trailer carries the
/// checksum, since the manifest body cannot checksum itself.
fn write_manifest(
    out: &mut dyn Write,
    entries: &[ManifestEntry],
    repository_format: u32,
    tool_version: &str,
) -> Result<(u64, [u8; 32])> {
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
    let checksum = prikk_hash::sha256(&body);
    Ok((len_to_u64(body.len())?, checksum))
}

fn write_end_trailer(
    out: &mut dyn Write,
    manifest_offset: u64,
    manifest_length: u64,
    manifest_checksum: [u8; 32],
) -> Result<()> {
    out.write_all(END_TRAILER_MAGIC)?;
    out.write_all(&manifest_offset.to_be_bytes())?;
    out.write_all(&manifest_length.to_be_bytes())?;
    out.write_all(&manifest_checksum)?;
    Ok(())
}

const fn repository_format_number(format: RepositoryFormat) -> u32 {
    match format {
        RepositoryFormat::CurrentV6 => 6,
        RepositoryFormat::V7 => 7,
    }
}

/// Every `repository_format` number a real export can ever write -- the inverse of
/// [`repository_format_number`], enumerated the same way rather than re-guessed as a numeric range.
/// `verify::validate_manifest_shape` refuses, naming the value, when a carried archive declares
/// anything else (V2's own fix, review `rfc155-impl-V-review-v1` finding V1b): import would
/// otherwise act on a format number nobody actually wrote.
pub(crate) fn known_repository_format_numbers() -> [u32; 2] {
    [
        repository_format_number(RepositoryFormat::CurrentV6),
        repository_format_number(RepositoryFormat::V7),
    ]
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
pub(crate) mod tests;
