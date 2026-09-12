//! Container-backed object store (RFC 102 Stage 3). Public API (`FileObjectStore`, `ObjectReader`,
//! `ObjectWriter`) is unchanged from the loose-file implementation this replaces -- every other call
//! site in the workspace uses only that trait interface, so none of them needed to change. Only the
//! internals moved: reads and writes now go through `index.rs`'s lookup/write-protocol functions,
//! which target `container.rs`'s per-type container files instead of one file per object.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType};

use crate::foundation::index::{
    self, IndexEntry, WriteDecision, append_object_to_container, decide_write_outcome,
    lookup_object_location, read_object_envelope_at,
};
use crate::foundation::layout::{LockableContainer, RepositoryLayout};
use crate::lock::acquire_container_locks;

/// Read-only object access boundary.
pub trait ObjectReader {
    /// Read an object by ID.
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>>;

    /// Read and require a specific object type. Default-implemented in terms of `read_object` alone
    /// (RFC 111 §6.1: every implementor -- `FileObjectStore`, `ObjectReadSnapshot`,
    /// `ObjectWriteSession`, `MemoryObjectStore` -- gets this for free, and any function generic over
    /// `impl ObjectReader` can call it without depending on a concrete type). One body, not one per
    /// implementor (Stage 1 review v1 §3): every concrete type's own `read_typed` delegates here too.
    fn read_typed(&self, id: ObjectId, object_type: ObjectType) -> Result<Option<ObjectEnvelope>> {
        let Some(envelope) = self.read_object(id)? else {
            return Ok(None);
        };
        if envelope.object_type != object_type {
            return Err(PrikkError::ObjectTypeMismatch {
                expected: object_type.to_string(),
                actual: envelope.object_type.to_string(),
            });
        }
        Ok(Some(envelope))
    }
}

/// Write object boundary.
pub trait ObjectWriter {
    /// Write an object envelope after validation.
    fn write_object(&mut self, envelope: &ObjectEnvelope) -> Result<ObjectId>;
}

/// File-backed object store.
#[derive(Debug, Clone)]
pub struct FileObjectStore {
    layout: RepositoryLayout,
}

impl FileObjectStore {
    /// Create a file object store for a repository layout.
    #[must_use]
    pub fn new(layout: RepositoryLayout) -> Self {
        Self { layout }
    }

    /// Return the repository layout.
    #[must_use]
    pub fn layout(&self) -> &RepositoryLayout {
        &self.layout
    }

    /// Return true if an object with this id and type is indexed.
    #[must_use]
    pub fn contains_object(&self, object_type: ObjectType, id: ObjectId) -> bool {
        if object_type == ObjectType::RefUpdate {
            return false;
        }
        matches!(
            lookup_object_location(&self.layout, id),
            Ok(Some(entry)) if entry.object_type == object_type
        )
    }
}

impl ObjectReader for FileObjectStore {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        let Some(entry) = lookup_object_location(&self.layout, id)? else {
            return Ok(None);
        };
        read_object_at_entry(&self.layout, &entry, id)
    }
}

impl ObjectWriter for FileObjectStore {
    fn write_object(&mut self, envelope: &ObjectEnvelope) -> Result<ObjectId> {
        if envelope.object_type == ObjectType::RefUpdate {
            return Err(PrikkError::UnsupportedObjectType(
                "RefUpdate is stored inline in ref logs for v1".to_string(),
            ));
        }
        self.layout.validate_format()?;
        crate::format::validate_object_envelope(self.layout.format(), envelope)?;
        // The write protocol (design §5, handoff §3) lives in `index.rs`, not here: append the
        // object record to its container and make it durable, then and only then append the index
        // entry. Stated at that call site too, not only here. The idempotency decision itself (RFC
        // 111 §6.1 addendum, C2) is `index::decide_write_outcome`, shared verbatim with
        // `ObjectWriteSession` below -- only where its `existing` lookup comes from differs: this
        // type always re-decodes the whole index (unchanged cost, a safe default for any call site
        // not migrated to a snapshot-backed type).
        let existing = lookup_object_location(&self.layout, envelope.object_id())?;
        match decide_write_outcome(
            &self.layout,
            envelope.object_type,
            envelope,
            existing.as_ref(),
        )? {
            WriteDecision::AlreadyPresent(id) => Ok(id),
            WriteDecision::New => {
                append_object_under_lock(&self.layout, envelope.object_type, envelope)
                    .map(|entry| entry.object_id)
            }
        }
    }
}

/// Append one object under the object-store lock — **the only production route to
/// [`append_object_to_container`]**, and the whole of RFC 102's 2026-09-12 ruling.
///
/// The lock spans the entire append: the container length read, the container append, and the index
/// append. That span is the requirement, not an implementation detail — `offset` is derived from the
/// length *before* the append and written to the index *after* it, so two writers that read the same
/// length both record the same offset and one index entry ends up pointing at the other's record.
/// `O_APPEND` keeps the container bytes intact either way, which is why the damage surfaced as an
/// unreadable index entry rather than a torn container.
///
/// **Measured before this lock existed**: twelve concurrent `prikk tag create` in one repository left
/// two index entries claiming one offset and `prikk verify` failing — two runs of four here, four of
/// four on the reviewer's machine. Ordinary commands: `commit` holds `ActiveLock`, while `tag
/// create`, `merge` and `sync build` held nothing at all, so nothing serialised them.
///
/// **Taken here, not inside `append_object_to_container`**, because `foundation` is the bottom layer
/// and must not depend on `crate::lock` — the coupling gate rejected `foundation -> lock` when the
/// acquisition was placed there. Taken here, not per caller, because the eleven object-append call
/// sites across `commit`, `seal`, `merge`, `bundle import`, `sync accept/build/seal`, `tag create`
/// and ref publication all funnel through `ObjectWriter::write_object` — five of them holding no
/// lock whatsoever.
///
/// **This lock is a leaf: nothing is acquired while it is held**, which is what makes it safe to take
/// inside a call `publish_ref` already makes while holding `RefLock` plus the ref container locks.
/// The nesting is one-directional and no inversion is expressible. It is fail-fast like every other
/// lock here, so two overlapping object appends now produce one visible `lock conflict` instead of a
/// silently wrong index.
fn append_object_under_lock(
    layout: &RepositoryLayout,
    object_type: ObjectType,
    envelope: &ObjectEnvelope,
) -> Result<IndexEntry> {
    let _object_store_lock = acquire_container_locks(layout, &[LockableContainer::ObjectStore])?;
    append_object_to_container(layout, object_type, envelope)
}

/// Read validation shared by every reader below (`FileObjectStore`, `ObjectReadSnapshot`,
/// `ObjectWriteSession`): the index is trusted for *location*, but the bytes found there are always
/// checked against the id actually asked for by recomputing it from the decoded content -- free,
/// since decoding already happened. A mismatch is reported, never silently accepted and never a
/// fallback to scanning ("one seek", design §12/§10.3).
fn read_object_at_entry(
    layout: &RepositoryLayout,
    entry: &IndexEntry,
    id: ObjectId,
) -> Result<Option<ObjectEnvelope>> {
    let envelope = read_object_envelope_at(layout, entry)?;
    let computed = envelope.object_id();
    if computed != id {
        return Err(PrikkError::Integrity(format!(
            "index entry for {id} resolves to an envelope with computed id {computed}"
        )));
    }
    if envelope.object_type != entry.object_type {
        return Err(PrikkError::Integrity(format!(
            "index entry for {id} names type {}, envelope decoded as {}",
            entry.object_type, envelope.object_type
        )));
    }
    crate::format::validate_read_schema(layout.format(), &envelope)?;
    Ok(Some(envelope))
}

/// A decoded object-index snapshot, taken once. Backs both `ObjectReadSnapshot` and
/// `ObjectWriteSession` (RFC 111 §6.1) -- the read logic (lookup, then decode at a known offset) is
/// identical between them, so it exists here once rather than twice. Has no public API of its own;
/// both public types below wrap it.
struct IndexSnapshot {
    entries: Vec<IndexEntry>,
    /// The object index's byte length as of the last time `entries` was known-current --
    /// `bytes.len() - trailing_partial_bytes` from whichever decode produced `entries`, never the
    /// raw stat size (RFC 111 §6.1 addendum §3.2: a torn trailing write must not be counted as
    /// decoded).
    known_length: u64,
}

impl IndexSnapshot {
    fn open(layout: &RepositoryLayout) -> Result<Self> {
        let (replay, known_length) = index::replay_index_with_extent(layout)?;
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(
                "object index has a damaged entry; run doctor before reading".to_string(),
            ));
        }
        Ok(Self {
            entries: replay.entries,
            known_length,
        })
    }

    /// Same last-entry-wins semantics `lookup_object_location` already has, preserved verbatim.
    fn lookup(&self, id: ObjectId) -> Option<&IndexEntry> {
        self.entries
            .iter()
            .rev()
            .find(|entry| entry.object_id == id)
    }

    /// Re-stat only; decode only if the stat disagrees with what this snapshot already knows. Every
    /// write decision calls this first (RFC 111 §6.1 addendum, C1) -- it is what makes a stale
    /// idempotency decision structurally impossible regardless of *what* wrote the new bytes: a
    /// nested unmediated writer in the same process (`refs/publication.rs`'s current shape), a call
    /// site not yet migrated to a snapshot-backed type, or a genuinely separate process. All three
    /// grow the index file, and this catches every one the same way, because it checks the one fact
    /// that is true regardless of cause. The common case -- nothing else wrote -- costs one stat, no
    /// decode. `ObjectReadSnapshot` never calls this: a reader's staleness is already accepted and
    /// bounded (RFC 111 Q3/Q4), so charging every read a stat here would buy nothing.
    fn ensure_current(&mut self, layout: &RepositoryLayout) -> Result<()> {
        let relative = layout.repository_relative(&layout.container_index_path())?;
        let current_length = crate::foundation::fsutil::stat_file_state_if_exists(
            layout.repository_mutation_root(),
            &relative,
        )?
        .map_or(0, |stat| stat.size);
        if current_length == self.known_length {
            return Ok(());
        }
        if current_length < self.known_length {
            // The object index is append-only and must never shrink (it is not one of the four
            // compactable containers). A shorter file than this snapshot last knew means either the
            // file was rebuilt out from under an open session or something is badly wrong -- fail
            // closed rather than decode from an offset past the new end (RFC 111 §6.1 addendum §3.1).
            return Err(PrikkError::Integrity(format!(
                "object index shrank from {} to {current_length} bytes since it was last read; \
                 the object index is append-only and must never shrink -- run doctor",
                self.known_length
            )));
        }
        let (tail, new_extent) = index::replay_index_tail_with_extent(layout, self.known_length)?;
        if tail.has_item_failure() {
            return Err(PrikkError::Integrity(
                "object index has a damaged entry; run doctor before reading".to_string(),
            ));
        }
        self.entries.extend(tail.entries);
        self.known_length = new_extent;
        Ok(())
    }
}

/// Read-only object access for one operation's lifetime (RFC 111 §6.1). Takes one decoded index
/// snapshot at construction and never re-decodes -- correct because a reader never writes, so it can
/// never observe its *own* write as missing the way a writer holding a stale snapshot could (RFC 111
/// Q3). A snapshot taken here may miss an object a concurrent writer appends after construction; that
/// is `verify`'s own already-documented point-in-time semantics, unchanged by this type (RFC 111 Q4).
pub struct ObjectReadSnapshot {
    layout: RepositoryLayout,
    snapshot: IndexSnapshot,
}

impl ObjectReadSnapshot {
    /// Open a read-only snapshot of `layout`'s object index, decoding it exactly once.
    pub fn open(layout: &RepositoryLayout) -> Result<Self> {
        Ok(Self {
            layout: layout.clone(),
            snapshot: IndexSnapshot::open(layout)?,
        })
    }

    /// Return true if an object with this id and type is indexed, as of when this snapshot was
    /// taken.
    #[must_use]
    pub fn contains_object(&self, object_type: ObjectType, id: ObjectId) -> bool {
        if object_type == ObjectType::RefUpdate {
            return false;
        }
        matches!(self.snapshot.lookup(id), Some(entry) if entry.object_type == object_type)
    }
}

impl ObjectReader for ObjectReadSnapshot {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        let Some(entry) = self.snapshot.lookup(id) else {
            return Ok(None);
        };
        read_object_at_entry(&self.layout, entry, id)
    }
}

/// Read-write object access for one writing operation's lifetime (RFC 111 §6.1). Holds the same kind
/// of in-memory index snapshot `ObjectReadSnapshot` does, but every write decision first calls
/// `IndexSnapshot::ensure_current` (see its own doc), and every successful write calls it again
/// afterward instead of computing what changed itself (Stage 1 review v1, B1) -- `ensure_current`'s
/// own tail-decode is the only thing ever allowed to grow `entries`/`known_length`, whether what it
/// finds is this session's own write, a concurrent one, or both.
pub struct ObjectWriteSession {
    layout: RepositoryLayout,
    snapshot: IndexSnapshot,
}

impl ObjectWriteSession {
    /// Open a read-write session over `layout`'s object index, decoding it exactly once.
    pub fn open(layout: &RepositoryLayout) -> Result<Self> {
        Ok(Self {
            layout: layout.clone(),
            snapshot: IndexSnapshot::open(layout)?,
        })
    }

    /// Return true if an object with this id and type is indexed, refreshing the snapshot first if
    /// something else has grown the index since it was last known-current.
    pub fn contains_object(&mut self, object_type: ObjectType, id: ObjectId) -> Result<bool> {
        if object_type == ObjectType::RefUpdate {
            return Ok(false);
        }
        self.snapshot.ensure_current(&self.layout)?;
        Ok(matches!(self.snapshot.lookup(id), Some(entry) if entry.object_type == object_type))
    }
}

impl ObjectReader for ObjectWriteSession {
    fn read_object(&self, id: ObjectId) -> Result<Option<ObjectEnvelope>> {
        let Some(entry) = self.snapshot.lookup(id) else {
            return Ok(None);
        };
        read_object_at_entry(&self.layout, entry, id)
    }
}

impl ObjectWriter for ObjectWriteSession {
    fn write_object(&mut self, envelope: &ObjectEnvelope) -> Result<ObjectId> {
        if envelope.object_type == ObjectType::RefUpdate {
            return Err(PrikkError::UnsupportedObjectType(
                "RefUpdate is stored inline in ref logs for v1".to_string(),
            ));
        }
        self.layout.validate_format()?;
        crate::format::validate_object_envelope(self.layout.format(), envelope)?;
        self.snapshot.ensure_current(&self.layout)?;
        let existing = self.snapshot.lookup(envelope.object_id());
        match decide_write_outcome(&self.layout, envelope.object_type, envelope, existing)? {
            WriteDecision::AlreadyPresent(id) => Ok(id),
            WriteDecision::New => {
                let object_id = envelope.object_id();
                append_object_under_lock(&self.layout, envelope.object_type, envelope)?;
                // Do not trust the append's own return value for what changed (RFC 111 Stage 1
                // review v1, B1): re-derive by re-checking freshness through the same primitive
                // every other read decision uses. `ensure_current`'s tail-decode is frame-aligned
                // by construction and correctly picks up this write, a concurrent writer's, or
                // both -- a stat taken immediately after only this call's own append cannot tell
                // those apart.
                self.snapshot.ensure_current(&self.layout)?;
                Ok(object_id)
            }
        }
    }
}

// DC-97 correction of the comment this replaced: the Linux/macOS-only reasoning was true when
// written (DC-71/DC-81, before DC-87 made Windows a mutating platform) and nobody revisited it once
// Windows mutation shipped -- found only by DC-97's own G5 investigation, back when this module's
// now-deleted `tests::immutable` still made the claimed Windows evidence for G5. `publish_immutable`
// and its tests are gone entirely as of DC-98 (G5 retired, zero production callers). What remains
// here is gated the same way regardless: `RepositoryLayout::init` and real repository mutation are
// not Linux/macOS-only, so what is still unix-only inside this module (failpoints, symlinks, FIFOs)
// is gated per-test/per-file instead of by one blanket gate.
#[cfg(test)]
impl ObjectWriteSession {
    /// The session's own current view of the object index's byte extent -- exposed only so tests can
    /// assert it lands on the true file length rather than a value this type accumulated itself (RFC
    /// 111 §6.1 addendum §3.3, the load-bearing assertion the design review named explicitly).
    pub(crate) fn known_index_length_for_test(&self) -> u64 {
        self.snapshot.known_length
    }

    /// The session's own current view of how many index entries it holds -- exposed only for tests.
    pub(crate) fn entry_count_for_test(&self) -> usize {
        self.snapshot.entries.len()
    }
}

#[cfg(all(
    test,
    any(target_os = "linux", target_os = "macos", target_os = "windows")
))]
mod tests;
