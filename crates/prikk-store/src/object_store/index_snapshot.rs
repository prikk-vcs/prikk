//! The decoded object-index snapshot behind `ObjectReadSnapshot` and `ObjectWriteSession` (RFC 111 §6.1), and -- 0.58.0 S4 -- the one copy of
//! it a hold keeps.
//!
//! **What is kept.** While a command holds any lock (a `DirectoryHold` on the repository's `MutationRoot`, the P2a mechanism), the
//! decoded index outlives the snapshot that made it: `MutationRoot::keep_value` stores a [`KeptIndex`], and the next
//! `IndexSnapshot::open` under the same hold refreshes it instead of reading the whole file again. `sync seal` opened four snapshots per
//! claim and so read the whole index four times per claim (S4a, `probe-groups-250.txt`); with the kept copy it reads the file once and
//! the appended tail after that. **Outside a hold nothing is kept** (`keep_value` stores nothing and the hold's last release drops the
//! value), and `open` is the whole decode it has always been.
//!
//! **Never stale.** The index is append-only, but the hold spans object-store lock releases, so another process (a `sync build`, a `tag
//! create`) can append between two of this command's lookups, and a repair can replace the file. Every `open` and every `ensure_current`
//! therefore starts from one `stat`: the file's `(device, inode)` and size.
//! - the same file at the size the copy decoded: nothing to read;
//! - the same file, longer: read **only the appended tail**, together with the copy's last record (the canary), and decode the tail;
//! - a different file (another inode: `doctor --repair-index` installs by rename), a shorter file, a canary that is not the bytes the copy
//!   decoded, or a file replaced between the `stat` and the read: **the copy is dropped and the file is read whole**, today's path.
//!
//! **Rule 3 is unchanged.** A tail with an item failure or a trailing partial record is never added to the copy; the copy is dropped and
//! the call takes the path a fresh `open` always took (a reader scans the containers in memory, a writer repairs the index on disk under
//! its lock first). A copy is made only from a clean decode in file order.
//!
//! **Residual, stated (S4a §3).** A truncate to below `known_length` followed by a regrowth whose records between the cut and
//! `known_length` differ *and* whose record at `known_length` is byte-identical to the one we decoded would not be seen. That needs a second
//! writer to rebuild the prefix and re-create our last record byte for byte at its offset, which the append protocol does not produce
//! (offsets are container offsets). A full checksum of the prefix is O(index) per check and gives the reads back.
//!
//! **Who else rewrites the index** (S4a §3): `repair_index_from_containers` (`doctor --repair-index`, and a writer's own rebuild under its
//! lock) installs a new file by rename -- a new inode, seen by the identity; `archive import`'s journaled cancel truncates in place -- seen
//! by the size, or by the canary if the file regrows past `known_length` before the next check; nothing compacts the object index.
//!
//! **Sharing.** The data is an `Arc<IndexData>`: a snapshot owns a reference, so a reader keeps its point in time (RFC 111 Q4) while the
//! kept copy moves on. An extension takes the kept copy's reference away first and appends through `Arc::make_mut`, which copies only if a
//! longer-lived snapshot still holds the data.
//!
//! **Platform.** Linux and macOS only (the held directories' `cfg`): the identity is `(device, inode)`, which `std` cannot give on Windows.
//! Without an identity `open` keeps nothing and every open decodes in full, exactly as before.

use std::collections::HashMap;
use std::sync::Arc;

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use super::{current_index_file_length, extend_entries_and_by_id, index_position_map};
use crate::foundation::fsutil::{FileIdentity, Kept};
use crate::foundation::index::{self, IndexEntry, VerifiedTail};
use crate::foundation::layout::RepositoryLayout;

/// The decoded entries and the id -> last position map, shared by `Arc`.
#[derive(Clone, Default)]
pub(super) struct IndexData {
    entries: Vec<IndexEntry>,
    /// 0.55.0 step 3 Part 1 (2C, ruling C1): id -> position in `entries`, kept alongside it on every path that changes `entries` -- a full
    /// rebuild on `open`/a replace, an incremental extend on the one true append path. `entries` itself is kept exactly as it was (never
    /// removed): its own forward order is what makes "last entry wins" the natural result of inserting into this map in that same order.
    by_id: HashMap<ObjectId, u32>,
}

/// What a hold keeps: the data, and what it was decoded from.
struct KeptIndex {
    data: Arc<IndexData>,
    known_length: u64,
    identity: FileIdentity,
    /// The last record the data decoded, as the file holds it (`encode_index_record` of the last entry; empty for an empty index).
    canary: Vec<u8>,
}

/// A decoded object-index snapshot. Has no public API of its own; `ObjectReadSnapshot` and `ObjectWriteSession` wrap it.
pub(super) struct IndexSnapshot {
    data: Arc<IndexData>,
    /// The object index's byte length as of the last time `data` was known-current -- `bytes.len() - trailing_partial_bytes` from whichever
    /// decode produced it, never the raw stat size (RFC 111 §6.1 addendum §3.2: a torn trailing write must not be counted as decoded).
    known_length: u64,
    /// The file `data` came from, where the platform has an identity and it was the same file before and after the read; `None` otherwise,
    /// which `ensure_current` reads as "not known to be the same file".
    identity: Option<FileIdentity>,
    /// `data.entries` are the file's records in file order (a clean decode), not a container scan: only such data is kept.
    file_order: bool,
}

impl IndexSnapshot {
    /// RFC 162 rule 1: a reader never refuses while the containers are sound. Never called from inside `append_object_under_lock`'s hold
    /// (every production caller opens a snapshot before attempting any write), and never persists anything on damage -- see
    /// `resolve_object_location_entries`'s own doc for why an unlocked path must not have a hidden write side effect (`verify`'s read-only
    /// contract, `import_bundle`'s all-or-nothing refusal). Scans the containers in memory instead.
    ///
    /// Under a hold, the kept copy is refreshed instead (see the module doc); anything it cannot prove falls through to [`Self::load`].
    pub(super) fn open(layout: &RepositoryLayout) -> Result<Self> {
        if let Some(snapshot) = Self::from_kept(layout)? {
            return Ok(snapshot);
        }
        let snapshot = Self::load(layout)?;
        snapshot.keep(layout);
        Ok(snapshot)
    }

    /// The whole decode, and what `open` always did.
    ///
    /// 0.50.0 step 1, A5: a trailing-partial tail with no `Failed` item used to be read as "nothing to rebuild for," the same oversight
    /// `resolve_object_location_entries`'s fix carries the full account of. The most-recently-written object's index entry is exactly the one a
    /// crash leaves torn, so `replay.entries` alone silently omits it. The tail case keeps `known_length` at the sound prefix's own extent
    /// (`replay_index_with_extent`'s own, excluding the torn bytes) rather than the raw stat size used for interior damage below, so a later
    /// `ensure_current_locked` finds the tail still visible and rebuilds the index on disk before a write appends behind it (rule 1's writer
    /// half).
    fn load(layout: &RepositoryLayout) -> Result<Self> {
        let before = index::stat_index_file(layout)?;
        let (replay, known_length) = index::replay_index_with_extent(layout)?;
        let identity = stable_identity(layout, before.identity)?;
        if replay.has_item_failure() {
            let entries = index::rebuild_index_from_containers(layout)?;
            return Ok(Self {
                data: Arc::new(data_of(entries)?),
                known_length: current_index_file_length(layout)?,
                identity,
                file_order: false,
            });
        }
        if replay.trailing_partial_bytes != 0 {
            let entries = index::rebuild_index_from_containers(layout)?;
            return Ok(Self {
                data: Arc::new(data_of(entries)?),
                known_length,
                identity,
                file_order: false,
            });
        }
        Ok(Self {
            data: Arc::new(data_of(replay.entries)?),
            known_length,
            identity,
            file_order: true,
        })
    }

    /// The kept copy, brought up to date by reading only what was appended; `None` (the copy is gone) when nothing was kept or it cannot
    /// be proven current. Costs one `stat`, one positioned read of the tail and the canary, and a second `stat`.
    fn from_kept(layout: &RepositoryLayout) -> Result<Option<Self>> {
        let Some(kept) = take_kept(layout) else {
            return Ok(None);
        };
        let observed = index::stat_index_file(layout)?;
        if observed.identity != Some(kept.identity) || observed.size < kept.known_length {
            return Ok(None);
        }
        let VerifiedTail::Tail(tail, extent) =
            index::replay_index_tail_verified(layout, kept.known_length, observed, &kept.canary)?
        else {
            return Ok(None);
        };
        // Rule 3: never add a damaged or torn tail to the copy; the caller's `load` applies the damaged-index rule.
        if tail.has_item_failure() || tail.trailing_partial_bytes != 0 {
            return Ok(None);
        }
        let mut snapshot = Self {
            data: kept.data,
            known_length: kept.known_length,
            identity: Some(kept.identity),
            file_order: true,
        };
        snapshot.extend(tail.entries, extent)?;
        snapshot.keep(layout);
        Ok(Some(snapshot))
    }

    /// Append `tail` entries (a clean tail decode ending at `extent`). Copies the data only if another snapshot still holds it.
    fn extend(&mut self, tail: Vec<IndexEntry>, extent: u64) -> Result<()> {
        if !tail.is_empty() {
            let data = Arc::make_mut(&mut self.data);
            // 0.55.0 step 3 Part 1 (2C, ruling C1): the one true incremental path.
            extend_entries_and_by_id(&mut data.entries, &mut data.by_id, tail)?;
        }
        self.known_length = extent;
        Ok(())
    }

    /// Make this snapshot's data the hold's kept copy, if a hold is open and the data is a clean decode of a known file.
    fn keep(&self, layout: &RepositoryLayout) {
        let root = layout.repository_mutation_root();
        if !self.file_order || !root.hold_is_open() {
            return;
        }
        let Some(identity) = self.identity else {
            return;
        };
        let canary = match self.data.entries.last() {
            Some(last) => match index::encode_index_record(last) {
                Ok(bytes) => bytes,
                Err(_) => return,
            },
            None => Vec::new(),
        };
        let kept: Kept = Arc::new(KeptIndex {
            data: Arc::clone(&self.data),
            known_length: self.known_length,
            identity,
            canary,
        });
        root.keep_value(kept);
    }

    /// Give back the kept copy's reference to this snapshot's data, so that an extension does not copy it. A kept value that is not this
    /// snapshot's data is put back.
    fn release_kept(&self, layout: &RepositoryLayout) {
        let root = layout.repository_mutation_root();
        let Some(value) = root.take_kept_value() else {
            return;
        };
        let same = value
            .clone()
            .downcast::<KeptIndex>()
            .is_ok_and(|kept| Arc::ptr_eq(&kept.data, &self.data));
        if !same {
            root.keep_value(value);
        }
    }

    /// Last-entry-wins, as the index's reverse search it replaced (`lookup_object_location`, removed in 0.58.0 P2a) had it --
    /// 0.55.0 step 3 Part 1 (2C, ruling C1) replaced the reverse linear scan with an O(1) lookup into `by_id`, which already names whichever
    /// position is last-written for `id`.
    pub(super) fn lookup(&self, id: ObjectId) -> Option<&IndexEntry> {
        // In range by construction, as `resolve_cached`'s own comment explains; `.get()` is used anyway to satisfy
        // `clippy::indexing_slicing`.
        self.data
            .by_id
            .get(&id)
            .and_then(|&position| self.data.entries.get(position as usize))
    }

    /// Re-stat only; decode only if the stat disagrees with what this snapshot already knows. Every write decision calls this first (RFC 111
    /// §6.1 addendum, C1) -- it is what makes a stale idempotency decision structurally impossible regardless of *what* wrote the new
    /// bytes: a nested unmediated writer in the same process (`refs/publication.rs`'s current shape), a call site not yet migrated to a
    /// snapshot-backed type, or a genuinely separate process. All three grow the index file, and this catches every one the same way,
    /// because it checks the one fact that is true regardless of cause. The common case -- nothing else wrote -- costs one stat, no decode.
    /// `ObjectReadSnapshot` never calls this: a reader's staleness is already accepted and bounded (RFC 111 Q3/Q4), so charging every read a
    /// stat here would buy nothing.
    pub(super) fn ensure_current(&mut self, layout: &RepositoryLayout) -> Result<()> {
        self.ensure_current_impl(layout, false)
    }

    /// Like [`Self::ensure_current`], but for use only from inside `append_object_under_lock`'s hold -- never acquires the object-store lock
    /// itself. This is `write_object`'s own re-check under the lock (RFC 156 Stage 2b); RFC 162 rule 1's writer half rides along with it: if
    /// the index is found damaged here, it is rebuilt before the write decision is made, so a writer never appends behind damage.
    pub(super) fn ensure_current_locked(&mut self, layout: &RepositoryLayout) -> Result<()> {
        self.ensure_current_impl(layout, true)
    }

    fn ensure_current_impl(
        &mut self,
        layout: &RepositoryLayout,
        already_locked: bool,
    ) -> Result<()> {
        let observed = index::stat_index_file(layout)?;
        let same_file = observed.identity == self.identity;
        if same_file && observed.size == self.known_length {
            return Ok(());
        }
        if !same_file {
            // 0.58.0 S4: another file now stands at the index's path (`doctor --repair-index` installs by rename, a writer's own rebuild the
            // same), or this snapshot never knew which file it read. Today's check compared the length only, and an equal-length rebuild
            // went unseen. Read it whole, as a fresh open does.
            return self.reload(layout, already_locked);
        }
        if observed.size < self.known_length {
            // RFC 162 rule 1: the object index is a pure cache and can be rebuilt in place (`doctor --repair-index`, or a writer's own
            // rebuild-before-append), which can legitimately shrink it -- a rebuild holds exactly the sound entries a container scan finds,
            // discarding whatever stale, duplicate, or damaged bytes the old file carried. Reload fully and accept it only if the shorter file
            // is now clean; otherwise this is still the genuine "something is badly wrong" case the original check existed for (RFC 111
            // §6.1 addendum §3.1).
            let (fresh, fresh_length) = index::replay_index_with_extent(layout)?;
            if fresh.has_item_failure() {
                return Err(PrikkError::Integrity(format!(
                    "object index shrank from {} to {} bytes since it was last read, \
                     and is still damaged; the object index only shrinks through a clean rebuild -- \
                     run doctor",
                    self.known_length, observed.size
                )));
            }
            self.data = Arc::new(data_of(fresh.entries)?);
            self.known_length = fresh_length;
            self.identity = stable_identity(layout, observed.identity)?;
            self.file_order = true;
            self.keep(layout);
            return Ok(());
        }
        // The same file, longer. The kept copy's reference to this data goes first, so that the extension below does not copy it.
        self.release_kept(layout);
        let VerifiedTail::Tail(tail, new_extent) =
            index::replay_index_tail_verified(layout, self.known_length, observed, &[])?
        else {
            return self.reload(layout, already_locked);
        };
        // RFC 162 rule 1: "a write never buries a torn index tail." A locked caller is about to append behind whatever is at the file's
        // current end; a trailing partial there, even with no `Failed` item, must trigger the same rebuild an item failure does, or the
        // append would sandwich the tail into interior damage. An unlocked caller (a plain re-check, not about to append) has no such
        // hazard: extending past a trailing partial is exactly what a tail decode already does correctly.
        let must_rebuild =
            tail.has_item_failure() || (already_locked && tail.trailing_partial_bytes != 0);
        if !must_rebuild {
            self.extend(tail.entries, new_extent)?;
            self.keep(layout);
            return Ok(());
        }
        // RFC 162 rule 1: rebuild rather than refuse. A rebuild (or a scan standing in for one) reorders the whole file relative to a tail
        // decode's own assumptions (containers are walked in type order, not write order), so discard this snapshot's entries/known_length
        // entirely rather than trying to extend them. `already_locked` decides whether that rebuild may be persisted: only
        // `ensure_current_locked`'s caller, inside `append_object_under_lock`'s hold, may -- every unlocked caller scans in memory instead,
        // for the same reason `open` does.
        if already_locked {
            self.repair_and_load(layout)
        } else {
            self.data = Arc::new(data_of(index::rebuild_index_from_containers(layout)?)?);
            self.known_length = current_index_file_length(layout)?;
            self.identity = observed.identity;
            self.file_order = false;
            Ok(())
        }
    }

    /// A fresh read of a file that is not the one this snapshot knows (or shorter than a tail can prove). A reader's reload scans in
    /// memory on damage, as `open` does; a writer under its lock repairs the index on disk first (rule 1's writer half), then reads it.
    fn reload(&mut self, layout: &RepositoryLayout, already_locked: bool) -> Result<()> {
        let fresh = Self::load(layout)?;
        if already_locked && !fresh.file_order {
            return self.repair_and_load(layout);
        }
        *self = fresh;
        self.keep(layout);
        Ok(())
    }

    /// Rule 1's writer half: repair the index on disk (the object-store lock the caller holds makes it safe to persist), then read it.
    fn repair_and_load(&mut self, layout: &RepositoryLayout) -> Result<()> {
        index::repair_index_from_containers(layout, |lost| {
            crate::recovery_log::save_lost_ids(layout, lost)
        })?;
        *self = Self::load(layout)?;
        self.keep(layout);
        Ok(())
    }

    /// The snapshot's own view of the index's byte extent -- for tests.
    #[cfg(test)]
    pub(super) fn known_length(&self) -> u64 {
        self.known_length
    }

    /// How many entries the snapshot holds -- for tests.
    #[cfg(test)]
    pub(super) fn entry_count(&self) -> usize {
        self.data.entries.len()
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
impl IndexData {
    /// The heap the data holds: the entries' capacity and the map's (the key, the value and one control byte per slot), the layout's
    /// own sizes, not an allocator's count.
    pub(super) fn heap_bytes(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<IndexEntry>()
            + self.by_id.capacity() * (std::mem::size_of::<(ObjectId, u32)>() + 1)
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
impl IndexSnapshot {
    pub(super) fn data_heap_bytes_for_test(&self) -> usize {
        self.data.heap_bytes()
    }
}

fn data_of(entries: Vec<IndexEntry>) -> Result<IndexData> {
    let by_id = index_position_map(&entries)?;
    Ok(IndexData { entries, by_id })
}

/// The kept index of this hold, taken out of the root (the caller owns it until it keeps again), if one was kept.
fn take_kept(layout: &RepositoryLayout) -> Option<KeptIndex> {
    let value = layout.repository_mutation_root().take_kept_value()?;
    let kept = value.downcast::<KeptIndex>().ok()?;
    Some(Arc::try_unwrap(kept).unwrap_or_else(|shared| KeptIndex {
        data: Arc::clone(&shared.data),
        known_length: shared.known_length,
        identity: shared.identity,
        canary: shared.canary.clone(),
    }))
}

/// `before` if the index file is still that file after a read, else `None`: a snapshot that cannot say which file it read is reloaded by
/// the next `ensure_current` and kept by nobody.
fn stable_identity(
    layout: &RepositoryLayout,
    before: Option<FileIdentity>,
) -> Result<Option<FileIdentity>> {
    let after = index::stat_index_file(layout)?.identity;
    Ok(if after == before { before } else { None })
}
