//! RFC 155 implementation handoff, Part I-A: `archive import`'s stages 0-2, the import journal,
//! and the writer-refusal interlock (§9.5). Stage 3 (ref landing, `--adopt`, RFC 156 identity
//! merge) is Part I-B's own, later work -- not built here. There is no public `prikk archive
//! import` command yet; that CLI surface is Part I-B's own deliverable too (§9.9 item 3's own
//! `prikk archive import <file> [--adopt]`). This part's only new CLI surface is
//! `prikk doctor --cancel-import <id> [--plan-only]`.
//!
//! **The journal is the first write** (§9.5): before it exists, stage 0's checks (the archive's own
//! verify, the author-key conflict check, the landing plan) have all already run, read-only, and a
//! refusal there writes nothing. Once it exists, every container lock it names is held until the
//! commit point, which empties it (keeping its name, RFC 168 §6's conservative choice). While it
//! exists, every ordinary repository writer refuses -- enforced structurally inside
//! [`crate::lock::ActiveLock::acquire`] itself (see that function's own doc comment), not by
//! per-call-site discipline, so a future writer this part's own author never anticipated is
//! covered automatically rather than silently missed.
//!
//! **Stage 0 reuses [`super::verify_archive`] directly, not a copy** (the handoff's own words, and
//! `verify.rs`'s own doc comment on why that is what makes Part I-A possible at all): a refused
//! import here is a refused `archive verify`, named identically. This module's own second pass
//! over the same archive (to decode and write what verify only checksums) reuses the same
//! low-level section decoders verify already uses ([`ManifestView`](super::verify), made visible to
//! this sibling module through their shared parent) rather than re-deriving the frame format a
//! second time -- see each reused function's own doc note in `verify.rs`.
//!
//! **Stage 1 never holds more than one object type's own records in memory at a time** (the
//! handoff's own prohibited-shortcut: "holding the whole archive or every object in memory").
//! Processed in [`crate::bundle::object_type_dependency_rank`]'s own cross-type order -- the same
//! ranking `bundle import` and `patch_exchange::accept` already use -- with same-kind topological
//! order applied per type via [`crate::bundle::objects_in_dependency_order`]. **An id already
//! present locally is left untouched, never merged**: `ObjectWriteSession::write_object`'s own
//! merge path (format 7, a stored id gaining another signer's signatures) assumes
//! `admit_carried_signatures`'s validation already ran -- that is RFC 156's identity merge, Part
//! I-B's own later work (§9.7), not available here. Skipping a colliding id outright is the safe
//! boundary until I-B adds it; nothing here ever calls the merge path.
//!
//! **Stage 2 records carried author keys through the unchanged DC-78 path**
//! (`check_author_key_conflict` at stage 0, `record_author_key_material` here) -- both already
//! idempotent, so a resume safely re-runs stage 2 from its start without double-recording anything.
//!
//! **A genuine ambiguity, resolved and named rather than guessed past** (RFC 155 §9.5's own text
//! lists, for the journal, "the length of every container import can append to: each object
//! container, the ref log, and the live slots of the pointer index and the received index" --
//! the author-key container is not in that list, even though stage 2 appends to it). Read
//! literally: cancel does not revert author-key records. This is very likely intentional, not an
//! omission -- `record_author_key_material` is commutative and idempotent (one `key_id` binds to
//! one public key, recorded at most once), so a half-finished stage 2 left behind by a cancelled
//! import is inert, never an inconsistent repository state the way a landed ref or a dangling
//! object reference would be. This module follows §9.5's literal text; the architect should
//! confirm this reading before Part I-B relies on it.

use std::io::{Read, Seek};
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectType};

use crate::author::author_key_index::{check_author_key_conflict, record_author_key_material};
use crate::bundle::{object_type_dependency_rank, objects_in_dependency_order};
use crate::foundation::file_codec::decode_envelope_file;
use crate::foundation::fsutil::{
    create_new_file_required, ensure_directory_required, read_file_if_exists,
    remove_file_if_present_required, truncate_existing_file_required,
};
use crate::foundation::layout::persisted_object_types;
use crate::foundation::layout::{ContainerSlot, LockableContainer, RepositoryLayout};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::object_store::{ObjectWriteSession, ObjectWriter};
use crate::received::received_index::decode_received_index_entries_for_resolver;
use crate::refs::decode_pointer_index_entries_for_resolver;

use super::ArchiveSectionKind;
use super::verify::{
    find_entry, for_each_object_record, read_author_material, read_end_trailer, read_manifest,
    read_section_body_fully,
};

/// Journal magic -- distinct from every other file this repository writes, so a resync or a
/// misread never confuses it with anything else.
const JOURNAL_MAGIC: &[u8; 8] = b"PRIMPJ01";
const JOURNAL_VERSION: u16 = 1;
/// DC-86 discipline: every length decoded from the journal is checked against one of these before
/// an allocation or a copy -- the journal is this repository's own write, never adversarial input
/// the way an archive file is, but a torn write from a real crash (the one case this bounds
/// against) must still refuse cleanly rather than read whatever garbage follows as a length.
const MAX_JOURNAL_STRING_BYTES: usize = 4096;
const MAX_JOURNAL_CONTAINER_COUNT: usize = 64;
const MAX_JOURNAL_LANDING_COUNT: usize = 10_000_000;

/// One container the journal names: its own stable label (for messages), its path (relative to the
/// repository root, `/`-separated, matching `recovery_log`'s own convention), and the length it
/// had before this import's own first write.
#[derive(Debug, Clone, PartialEq, Eq)]
struct JournaledContainer {
    label: String,
    relative_path: String,
    starting_length: u64,
    /// Which `LockableContainer` this file lives under -- `cancel_import`'s own cut needs this to
    /// take the object-store lock briefly, per cut, rather than for its whole run (see this
    /// module's own doc note on why the object-store lock is never held for a whole import run).
    kind: JournaledContainerKind,
}

/// Which `LockableContainer` a journaled file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JournaledContainerKind {
    ObjectStore,
    RefLog,
    RefPointerIndex,
    ReceivedIndex,
}

impl JournaledContainerKind {
    fn to_wire(self) -> u8 {
        match self {
            Self::ObjectStore => 0,
            Self::RefLog => 1,
            Self::RefPointerIndex => 2,
            Self::ReceivedIndex => 3,
        }
    }

    fn from_wire(code: u8) -> Result<Self> {
        match code {
            0 => Ok(Self::ObjectStore),
            1 => Ok(Self::RefLog),
            2 => Ok(Self::RefPointerIndex),
            3 => Ok(Self::ReceivedIndex),
            other => Err(PrikkError::Integrity(format!(
                "import journal names an unknown container kind {other}"
            ))),
        }
    }

    fn lockable(self) -> LockableContainer {
        match self {
            Self::ObjectStore => LockableContainer::ObjectStore,
            Self::RefLog => LockableContainer::RefLog,
            Self::RefPointerIndex => LockableContainer::RefPointerIndex,
            Self::ReceivedIndex => LockableContainer::ReceivedIndex,
        }
    }
}

/// One ref the archive carries, and where it would land if stage 3 ran today (§9.6's default --
/// Part I-B's own narrowed `--adopt` is not evaluated here). Recorded in the journal so a future
/// stage 3 (added by Part I-B, reading the same journal) has a plan to act on without re-deriving
/// it; Part I-A itself never writes any ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedRefLanding {
    /// The ref name as the archive's own resolved pointer/received index carries it (fully
    /// qualified, e.g. `heads/main`, `tags/v1.0`).
    pub ref_name: String,
    /// Where this ref would land today -- always under `remotes/` in Part I-A (`--adopt` is Part
    /// I-B's own work). **Not yet the final naming convention**: a received-index entry's own
    /// `ref_name` may already read as a remote-shaped name, in which case this nests it a second
    /// time -- flagged here, for Part I-B to settle before stage 3 ever executes a real landing.
    pub destination: String,
}

struct ImportJournal {
    archive_identity: [u8; 32],
    containers: Vec<JournaledContainer>,
    landing: Vec<PlannedRefLanding>,
}

/// The id `prikk doctor --cancel-import <id>` takes, and the one `doctor`'s own issue names.
/// Delegates to `foundation::import_journal_marker`'s own algorithm so the two can never disagree
/// (that module's own doc note explains why it exists as a separate, lower layer than this one).
fn journal_short_id(archive_identity: &[u8; 32]) -> String {
    crate::foundation::import_journal_marker::short_id(archive_identity)
}

/// Summary of a Part I-A import run (stages 0-2 only; no ref ever lands). `--format json`/the full
/// `prikk archive import` command are Part I-B's own deliverable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveImportReport {
    /// Whether this run started a fresh journal (`false`) or resumed one an earlier, interrupted
    /// run left behind (`true`).
    pub resumed: bool,
    /// Objects actually written in this run, per type -- an object already present locally is not
    /// counted here (it was skipped, not written; see this module's own doc note on why a
    /// colliding id is never merged in this part).
    pub written_object_counts: Vec<(ObjectType, usize)>,
    /// Author keys recorded in this run (idempotent; a key already on file is not counted again).
    pub recorded_author_key_count: usize,
    /// Every ref the archive carries, and where it would land -- planned, never executed, by this
    /// part.
    pub planned_landings: Vec<PlannedRefLanding>,
}

/// `doctor`'s own `PRIKK-DOCTOR-INTERRUPTED-IMPORT` issue reads this: `Some(id)` when a journal is
/// present (an import started and did not reach its commit point), `None` otherwise. Never writes
/// anything, never acquires a lock -- a plain read, safe to call from `doctor`'s own read-only
/// pass.
pub fn interrupted_import_short_id(layout: &RepositoryLayout) -> Result<Option<String>> {
    crate::foundation::import_journal_marker::short_id_if_present(layout)
}

/// Run Part I-A's own stages over `source`: stage 0's checks, the journal, stage 1's objects in
/// dependency order, and stage 2's author keys. Never lands a ref (Part I-B's stage 3). A refusal
/// at stage 0 writes nothing; past the journal, only I/O or a concurrent lock holder can stop this,
/// never a content decision (mirrors `import_bundle`'s own documented shape).
pub fn import_archive<R: Read + Seek>(
    layout: &RepositoryLayout,
    source: &mut R,
) -> Result<ArchiveImportReport> {
    // Stage 0, part 1: the archive's own verify, Part V's code, not a copy.
    let verify_report = super::verify_archive(source)?;
    if !verify_report.is_internally_consistent() {
        return Err(PrikkError::Integrity(format!(
            "archive is not internally consistent -- refusing the import; see `archive verify` \
             for the detail ({} unresolvable signature(s), {} damage line(s))",
            verify_report.unresolvable_signatures.len(),
            verify_report.damaged.len()
        )));
    }

    // Re-locate the manifest over the same, now-confirmed-consistent reader -- this module's own
    // second pass, not a second check (see this module's own doc comment).
    let (_total_len, manifest_offset, manifest_length, manifest_checksum) =
        read_end_trailer(source)?;
    let manifest = read_manifest(source, manifest_offset, manifest_length, manifest_checksum)?;
    let archive_identity = manifest_checksum;
    let author_material = read_author_material(source, &manifest)?;

    // RFC 155 §9.1: a conflict refuses the whole import before any write -- read-only, checked for
    // every carried (key_id, public_key) pair before stage 1 ever runs. One key_id binds to one
    // public key (DC-53), so checking every entry under a key_id (not just the first) catches a
    // conflict *within* the archive's own carried material too, not only against this repository.
    for entries in author_material.values() {
        for entry in entries {
            check_author_key_conflict(layout, &entry.key_id, entry.public_key)?;
        }
    }

    let existing_journal = read_journal(layout)?;
    let (journal, resumed) = match existing_journal {
        Some(journal) if journal.archive_identity == archive_identity => (journal, true),
        Some(other) => {
            let id = journal_short_id(&other.archive_identity);
            return Err(PrikkError::LockConflict(format!(
                "a different archive import is already in progress (id {id}); run the same \
                 import again, or `prikk doctor --cancel-import {id}`"
            )));
        }
        None => {
            let landing = compute_landing_plan(source, &manifest)?;
            let containers = compute_container_starting_lengths(layout)?;
            let journal = ImportJournal {
                archive_identity,
                containers,
                landing,
            };
            write_journal(layout, &journal)?;
            // Test seam, unreachable from production: a kill right after the journal's own first
            // write, before any container lock or any stage-1/2 byte -- the case table's own
            // "kill right after the journal" row.
            #[cfg(test)]
            if STOP_AFTER_JOURNAL_FOR_TEST.with(std::cell::Cell::take) {
                return Err(PrikkError::Integrity(
                    "import stopped for test: simulated kill right after the journal".to_string(),
                ));
            }
            (journal, false)
        }
    };

    // The ref-side container locks, held from here to the commit point (§9.5) -- acquired *after*
    // the journal exists for a fresh start (the journal write above needed none of them), and
    // unconditionally for a resume, since the prior run already held and released them when it
    // was killed. **Not `ObjectStore`**: `ObjectWriteSession::write_object` already takes that
    // lock itself, once per append (`object_store.rs`'s own `_object_store_lock`) -- holding it
    // here too would be this same process conflicting with its own per-append acquisition, not a
    // defense against anything. Nothing in stage 1/2 writes a ref, so holding these three is only
    // to keep a concurrent recovery verb (which bypasses the `ActiveLock` check) from moving them
    // out from under this run's own journaled starting lengths.
    let _container_locks = acquire_container_locks(
        layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
        ],
    )?;
    // Unconditionally the "allowing" variant: a fresh start has no journal yet (trivially passes
    // either way); a resume's own journal is already present, from this same import, and must not
    // refuse itself.
    let active_lock = ActiveLock::acquire_for_write_allowing_import_journal(
        layout,
        crate::foundation::layout::DEFAULT_ACTIVE_NAME,
    )?;

    // Stage 1: one object type at a time, in dependency order, never the whole archive's objects
    // in memory together.
    let mut object_store = ObjectWriteSession::open(layout)?;
    let mut object_types: Vec<ObjectType> = persisted_object_types().to_vec();
    object_types.sort_by_key(|object_type| object_type_dependency_rank(*object_type));
    let mut written_object_counts = Vec::with_capacity(object_types.len());
    for object_type in object_types {
        let entry = find_entry(&manifest, ArchiveSectionKind::ObjectContainer(object_type))?;
        let mut envelopes: Vec<ObjectEnvelope> = Vec::new();
        for_each_object_record(source, entry, object_type, |body| {
            envelopes.push(decode_envelope_file(body)?);
            Ok(())
        })?;
        let envelopes = objects_in_dependency_order(envelopes)?;
        let mut written = 0usize;
        for envelope in envelopes {
            let id = envelope.object_id();
            if object_store.contains_object(object_type, id)? {
                // Already present locally: left untouched, never merged (this module's own doc
                // note on why the merge path is out of scope for Part I-A).
                continue;
            }
            object_store.check_write(&envelope)?;
            object_store.write_object(&envelope)?;
            written = written.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("archive import written-object count overflow".to_string())
            })?;
        }
        written_object_counts.push((object_type, written));
    }

    // Test seam, unreachable from production: a kill right after stage 1's own last write, before
    // stage 2 or the commit point -- the case table's own "kill after the objects" row.
    #[cfg(test)]
    if STOP_AFTER_STAGE_1_FOR_TEST.with(std::cell::Cell::take) {
        return Err(PrikkError::Integrity(
            "import stopped for test: simulated kill after stage 1's objects".to_string(),
        ));
    }

    // Stage 2: carried author keys, through the unchanged DC-78 path -- idempotent (matches
    // `import_bundle`'s own counting: every carried entry attempted, whether or not it was already
    // on file), so a resume simply re-confirms what an earlier attempt may have already recorded.
    let mut recorded_author_key_count = 0usize;
    for entries in author_material.values() {
        for entry in entries {
            record_author_key_material(layout, &entry.key_id, entry.public_key, &active_lock)?;
            recorded_author_key_count =
                recorded_author_key_count.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity(
                        "archive import recorded-author-key count overflow".to_string(),
                    )
                })?;
        }
    }

    // The commit point: the journal is emptied, its name kept (RFC 168 §6's conservative choice on
    // Windows).
    empty_journal(layout)?;

    Ok(ArchiveImportReport {
        resumed,
        written_object_counts,
        recorded_author_key_count,
        planned_landings: journal.landing,
    })
}

fn compute_landing_plan(
    source: &mut (impl Read + Seek),
    manifest: &super::verify::ManifestView,
) -> Result<Vec<PlannedRefLanding>> {
    let mut landings = Vec::new();
    let pointer_entry = find_entry(manifest, ArchiveSectionKind::RefPointerIndex)?;
    let pointer_bytes = read_section_body_fully(source, pointer_entry)?;
    let decoded_pointers = decode_pointer_index_entries_for_resolver(&pointer_bytes)?;
    if decoded_pointers.damaged {
        return Err(PrikkError::Integrity(
            "archive's resolved ref-pointer-index section decodes with interior damage -- \
             refusing the import"
                .to_string(),
        ));
    }
    for entry in decoded_pointers.entries {
        landings.push(PlannedRefLanding {
            destination: format!("remotes/{}", entry.ref_name),
            ref_name: entry.ref_name,
        });
    }

    let received_entry = find_entry(manifest, ArchiveSectionKind::ReceivedIndex)?;
    let received_bytes = read_section_body_fully(source, received_entry)?;
    let decoded_received = decode_received_index_entries_for_resolver(&received_bytes)?;
    if decoded_received.damaged {
        return Err(PrikkError::Integrity(
            "archive's resolved received-index section decodes with interior damage -- refusing \
             the import"
                .to_string(),
        ));
    }
    for entry in decoded_received.entries {
        landings.push(PlannedRefLanding {
            destination: format!("remotes/{}", entry.ref_name),
            ref_name: entry.ref_name,
        });
    }
    Ok(landings)
}

fn relative_string(layout: &RepositoryLayout, path: &Path) -> Result<String> {
    Ok(layout
        .repository_relative(path)?
        .to_string_lossy()
        .replace('\\', "/"))
}

fn compute_container_starting_lengths(
    layout: &RepositoryLayout,
) -> Result<Vec<JournaledContainer>> {
    let mut containers = Vec::new();
    for object_type in persisted_object_types() {
        let path = layout.container_slot_path(object_type, ContainerSlot::A);
        containers.push(JournaledContainer {
            label: format!("{object_type} container"),
            relative_path: relative_string(layout, &path)?,
            starting_length: super::stat_len(&path)?,
            kind: JournaledContainerKind::ObjectStore,
        });
    }
    // `LockableContainer::ObjectStore` is every object container *and* the single shared
    // `index.container` beside them, taken together as one resource (`layout.rs`'s own doc note).
    // A write through the ordinary path appends to both; cancel must cut both back, or the index
    // keeps naming an object whose own container bytes were just truncated away.
    let index_path = layout.container_index_path();
    containers.push(JournaledContainer {
        label: "object index".to_string(),
        relative_path: relative_string(layout, &index_path)?,
        starting_length: super::stat_len(&index_path)?,
        kind: JournaledContainerKind::ObjectStore,
    });
    let ref_log_path = layout.ref_log_container_slot_path(ContainerSlot::A);
    containers.push(JournaledContainer {
        label: "ref log".to_string(),
        relative_path: relative_string(layout, &ref_log_path)?,
        starting_length: super::stat_len(&ref_log_path)?,
        kind: JournaledContainerKind::RefLog,
    });
    let (pointer_path, _) = super::resolve_pointer_index_source(layout)?;
    containers.push(JournaledContainer {
        label: "ref pointer index".to_string(),
        relative_path: relative_string(layout, &pointer_path)?,
        starting_length: super::stat_len(&pointer_path)?,
        kind: JournaledContainerKind::RefPointerIndex,
    });
    let (received_path, _) = super::resolve_received_index_source(layout)?;
    containers.push(JournaledContainer {
        label: "received index".to_string(),
        relative_path: relative_string(layout, &received_path)?,
        starting_length: super::stat_len(&received_path)?,
        kind: JournaledContainerKind::ReceivedIndex,
    });
    Ok(containers)
}

fn encode_journal(journal: &ImportJournal) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    body.extend_from_slice(JOURNAL_MAGIC);
    body.extend_from_slice(&JOURNAL_VERSION.to_be_bytes());
    body.extend_from_slice(&journal.archive_identity);
    let container_count = u16::try_from(journal.containers.len()).map_err(|_| {
        PrikkError::Integrity("import journal container count overflow".to_string())
    })?;
    body.extend_from_slice(&container_count.to_be_bytes());
    for container in &journal.containers {
        push_string(&mut body, &container.label)?;
        push_string(&mut body, &container.relative_path)?;
        body.extend_from_slice(&container.starting_length.to_be_bytes());
        body.push(container.kind.to_wire());
    }
    let landing_count = u32::try_from(journal.landing.len())
        .map_err(|_| PrikkError::Integrity("import journal landing count overflow".to_string()))?;
    body.extend_from_slice(&landing_count.to_be_bytes());
    for landing in &journal.landing {
        push_string(&mut body, &landing.ref_name)?;
        push_string(&mut body, &landing.destination)?;
    }
    let checksum = prikk_hash::sha256(&body);
    body.extend_from_slice(&checksum);
    Ok(body)
}

fn push_string(body: &mut Vec<u8>, value: &str) -> Result<()> {
    let bytes = value.as_bytes();
    let len = u16::try_from(bytes.len())
        .map_err(|_| PrikkError::Integrity("import journal string too long".to_string()))?;
    body.extend_from_slice(&len.to_be_bytes());
    body.extend_from_slice(bytes);
    Ok(())
}

struct ByteReader<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> ByteReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .offset
            .checked_add(len)
            .ok_or_else(|| PrikkError::Integrity("import journal offset overflow".to_string()))?;
        let slice = self
            .bytes
            .get(self.offset..end)
            .ok_or_else(|| PrikkError::Integrity("import journal is truncated".to_string()))?;
        self.offset = end;
        Ok(slice)
    }

    fn take_u8(&mut self) -> Result<u8> {
        self.take(1)?
            .first()
            .copied()
            .ok_or_else(|| PrikkError::Integrity("import journal is truncated".to_string()))
    }

    fn take_u16(&mut self) -> Result<u16> {
        Ok(u16::from_be_bytes(
            self.take(2)?.try_into().unwrap_or([0u8; 2]),
        ))
    }

    fn take_u32(&mut self) -> Result<u32> {
        Ok(u32::from_be_bytes(
            self.take(4)?.try_into().unwrap_or([0u8; 4]),
        ))
    }

    fn take_u64(&mut self) -> Result<u64> {
        Ok(u64::from_be_bytes(
            self.take(8)?.try_into().unwrap_or([0u8; 8]),
        ))
    }

    fn take_string(&mut self) -> Result<String> {
        let len = usize::from(self.take_u16()?);
        if len > MAX_JOURNAL_STRING_BYTES {
            return Err(PrikkError::ObjectOverBound {
                declared_bytes: len as u64,
                bound_bytes: MAX_JOURNAL_STRING_BYTES as u64,
            });
        }
        let bytes = self.take(len)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| {
            PrikkError::Integrity("import journal string is not valid UTF-8".to_string())
        })
    }
}

fn decode_journal(bytes: &[u8]) -> Result<ImportJournal> {
    if bytes.len() < 8 + 2 + 32 + 2 + 32 {
        return Err(PrikkError::Integrity(
            "import journal is too short to contain its own header and checksum".to_string(),
        ));
    }
    let checksum_offset = bytes.len() - 32;
    let (Some(body), Some(declared_checksum)) =
        (bytes.get(..checksum_offset), bytes.get(checksum_offset..))
    else {
        return Err(PrikkError::Integrity(
            "import journal checksum offset overflow".to_string(),
        ));
    };
    let actual_checksum = prikk_hash::sha256(body);
    if actual_checksum != declared_checksum {
        return Err(PrikkError::Integrity(
            "import journal failed its own checksum -- refusing to trust it; preserve the \
             repository and inspect it"
                .to_string(),
        ));
    }

    let mut reader = ByteReader::new(body);
    let magic = reader.take(8)?;
    if magic != JOURNAL_MAGIC {
        return Err(PrikkError::Integrity(
            "import journal has the wrong magic".to_string(),
        ));
    }
    let version = reader.take_u16()?;
    if version != JOURNAL_VERSION {
        return Err(PrikkError::Integrity(format!(
            "import journal version {version} is not one this build knows how to read"
        )));
    }
    let archive_identity: [u8; 32] = reader.take(32)?.try_into().unwrap_or([0u8; 32]);
    let container_count = usize::from(reader.take_u16()?);
    if container_count > MAX_JOURNAL_CONTAINER_COUNT {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: container_count as u64,
            bound_bytes: MAX_JOURNAL_CONTAINER_COUNT as u64,
        });
    }
    let mut containers = Vec::with_capacity(container_count);
    for _ in 0..container_count {
        let label = reader.take_string()?;
        let relative_path = reader.take_string()?;
        let starting_length = reader.take_u64()?;
        let kind = JournaledContainerKind::from_wire(reader.take_u8()?)?;
        containers.push(JournaledContainer {
            label,
            relative_path,
            starting_length,
            kind,
        });
    }
    let landing_count = reader.take_u32()? as usize;
    if landing_count > MAX_JOURNAL_LANDING_COUNT {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes: landing_count as u64,
            bound_bytes: MAX_JOURNAL_LANDING_COUNT as u64,
        });
    }
    let mut landing = Vec::with_capacity(landing_count);
    for _ in 0..landing_count {
        let ref_name = reader.take_string()?;
        let destination = reader.take_string()?;
        landing.push(PlannedRefLanding {
            ref_name,
            destination,
        });
    }
    Ok(ImportJournal {
        archive_identity,
        containers,
        landing,
    })
}

fn read_journal(layout: &RepositoryLayout) -> Result<Option<ImportJournal>> {
    let root = layout.repository_mutation_root();
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    let Some(bytes) = read_file_if_exists(root, &relative)? else {
        return Ok(None);
    };
    if bytes.is_empty() {
        // The commit point and cancel both empty the journal, keeping its name (RFC 168 §6) --
        // an empty file reads identically to "no import in progress".
        return Ok(None);
    }
    Ok(Some(decode_journal(&bytes)?))
}

fn write_journal(layout: &RepositoryLayout, journal: &ImportJournal) -> Result<()> {
    let root = layout.repository_mutation_root();
    let dir_relative = layout.repository_relative(&layout.prikk_dir().join("import"))?;
    ensure_directory_required(root, &dir_relative)?;
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    let bytes = encode_journal(journal)?;
    // The commit point and `cancel_import` both empty the journal rather than removing it (RFC
    // 168 §6's conservative choice on Windows) -- `read_journal` already treats an empty file
    // identically to an absent one, so by the time this function is reached (only from the `None`
    // match arm above, where `read_journal` just confirmed exactly that), any file here is either
    // absent or empty. Removing it first, best-effort, before the real exclusive create: a crash
    // between the two leaves "no file", which reads the same as "empty file" either way.
    remove_file_if_present_required(root, &relative)?;
    create_new_file_required(root, &relative, &bytes).map_err(|err| {
        if err.kind() == std::io::ErrorKind::AlreadyExists {
            PrikkError::LockConflict(
                "an import journal already exists -- a concurrent import is starting, or one \
                 crashed between creating it and this check; run `prikk doctor` to see which"
                    .to_string(),
            )
        } else {
            PrikkError::from(err)
        }
    })
}

fn empty_journal(layout: &RepositoryLayout) -> Result<()> {
    let root = layout.repository_mutation_root();
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    truncate_existing_file_required(root, &relative, 0)
}

/// `prikk doctor --cancel-import <id> [--plan-only]` (§9.5): for every container the journal
/// names, save the bytes past its journaled starting length (one RFC 168 recovery run, before any
/// truncate), truncate back, then empty the journal. Refuses if no journal is present, or if `id`
/// does not match the one present (naming the real id) -- the same "named, not guessed" discipline
/// `--recovery-restore <id>` already uses.
pub fn cancel_import(
    layout: &RepositoryLayout,
    id: &str,
    plan_only: bool,
) -> Result<CancelImportReport> {
    let Some(journal) = read_journal(layout)? else {
        return Err(PrikkError::Integrity(
            "no archive import journal is present -- nothing to cancel".to_string(),
        ));
    };
    let real_id = journal_short_id(&journal.archive_identity);
    if id != real_id {
        return Err(PrikkError::Integrity(format!(
            "no import with id {id} is present; the one present has id {real_id}"
        )));
    }

    // The ref-side container locks, held for the whole cancel run -- **not** `ObjectStore`, taken
    // instead per object-type container just below (see `import_archive`'s own doc note on why
    // holding it continuously would conflict with `write_object`'s own per-append acquisition;
    // cancel does not call `write_object`, but holding it here for the whole run is still
    // needlessly wide when a scoped acquisition per cut is just as correct and cheap).
    let _container_locks = acquire_container_locks(
        layout,
        &[
            LockableContainer::RefPointerIndex,
            LockableContainer::RefLog,
            LockableContainer::ReceivedIndex,
        ],
    )?;
    let _active_lock = ActiveLock::acquire_for_write_allowing_import_journal(
        layout,
        crate::foundation::layout::DEFAULT_ACTIVE_NAME,
    )?;

    let run = if plan_only {
        None
    } else {
        Some(crate::recovery_log::begin_run())
    };
    let root = layout.repository_mutation_root();
    let mut steps = Vec::with_capacity(journal.containers.len());
    for container in &journal.containers {
        let _object_store_lock = if container.kind == JournaledContainerKind::ObjectStore {
            Some(acquire_container_locks(
                layout,
                &[container.kind.lockable()],
            )?)
        } else {
            None
        };
        let relative = Path::new(&container.relative_path);
        // RFC 160 §3.1's whole-read guard: an object container or the ref log container is
        // store-growing, and this cancel's own save-before-truncate genuinely needs the whole
        // file (the saved copy is the way back) -- see `whole_read_guard::SCOPES`'s own
        // `archive-import-cancel-save` row, the same reasoning `pointer-index-rebuild-recovery-
        // save` already has for the pointer index.
        #[cfg(test)]
        let _whole_read_scope =
            crate::foundation::fsutil::whole_read_guard::declare("archive-import-cancel-save");
        let Some(bytes) = read_file_if_exists(root, relative)? else {
            steps.push(CancelImportStep {
                label: container.label.clone(),
                cut_bytes: 0,
                recovery: None,
            });
            continue;
        };
        let current_len = bytes.len() as u64;
        if current_len <= container.starting_length {
            steps.push(CancelImportStep {
                label: container.label.clone(),
                cut_bytes: 0,
                recovery: None,
            });
            continue;
        }
        let cut_bytes = current_len - container.starting_length;
        if plan_only {
            steps.push(CancelImportStep {
                label: container.label.clone(),
                cut_bytes,
                recovery: None,
            });
            continue;
        }
        let entry = crate::recovery_log::entry_for(
            layout,
            &container.relative_path,
            &bytes,
            container.starting_length,
            "cancel-import",
        )?;
        let recovery = crate::recovery_log::append(root, &entry)?;
        truncate_existing_file_required(root, relative, container.starting_length)?;
        steps.push(CancelImportStep {
            label: container.label.clone(),
            cut_bytes,
            recovery: Some(recovery),
        });
    }
    drop(run);

    if !plan_only {
        empty_journal(layout)?;
    }

    Ok(CancelImportReport {
        id: real_id,
        plan_only,
        steps,
    })
}

/// One container's own cancel outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelImportStep {
    /// The container's own stable label (matches `ArchiveImportReport`'s own labeling, and the
    /// journal's own).
    pub label: String,
    /// Bytes this step cut (or would cut, under `--plan-only`). `0` when the container was never
    /// appended to.
    pub cut_bytes: u64,
    /// The recovery-log entry this step wrote, before truncating -- `None` under `--plan-only`, or
    /// when there was nothing to cut.
    pub recovery: Option<crate::recovery_log::RecoveryRef>,
}

/// `prikk doctor --cancel-import`'s own report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CancelImportReport {
    /// The import's own id, as confirmed against the journal.
    pub id: String,
    /// Whether this was a preview (`--plan-only`): every step above describes what cancel would
    /// do, but wrote nothing.
    pub plan_only: bool,
    /// One outcome per journaled container, in the journal's own order.
    pub steps: Vec<CancelImportStep>,
}

#[cfg(test)]
thread_local! {
    static STOP_AFTER_JOURNAL_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static STOP_AFTER_STAGE_1_FOR_TEST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// stops right after the journal's own first write (the case table's own "kill right after the
/// journal" row), leaving the journal present and every stage after it un-run.
#[cfg(test)]
pub(crate) fn stop_after_journal_for_test() {
    STOP_AFTER_JOURNAL_FOR_TEST.with(|cell| cell.set(true));
}

/// Test seam, unreachable from production: the next call to [`import_archive`] on this thread
/// stops right after stage 1's own last object write (the case table's own "kill after the
/// objects" row), leaving the journal present and stage 2/the commit point un-run.
#[cfg(test)]
pub(crate) fn stop_after_stage_1_for_test() {
    STOP_AFTER_STAGE_1_FOR_TEST.with(|cell| cell.set(true));
}

#[cfg(test)]
mod tests;
