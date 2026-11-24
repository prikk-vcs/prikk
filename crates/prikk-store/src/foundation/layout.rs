//! Repository layout paths and initialization.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};
use prikk_hash::{sha256, to_hex};
use prikk_object::{ObjectId, ObjectType, is_windows_reserved_name};

use crate::foundation::fsutil::{
    EntryKind, MutationRoot, create_new_file_required, ensure_directory_required, inspect_entry,
    list_directory, read_file_if_exists, read_file_range_if_exists, write_file_atomically,
};

const REPO_DIR: &str = ".prikk";
/// The one active-session name in use today (RFC 108 increment 1). `.prikk/active/<name>/`
/// already supports any name; every current caller of `Wal::for_layout`/`ActiveLock::acquire`
/// passes this constant rather than a re-typed literal, so a second active session -- a later
/// increment's own CLI-surface decision -- needs no caller here to change when it exists.
pub const DEFAULT_ACTIVE_NAME: &str = "default";
/// Format 6: still read and written (RFC 156 §5b) — a format-6 repository stays format 6 until
/// `prikk format upgrade` changes its marker.
pub(crate) const FORMAT_6_VERSION: &[u8] = b"6\n";
/// Format 7 (RFC 156 §5b): still read and written -- a format-7 repository stays format 7 until
/// `prikk format upgrade` changes its marker.
pub(crate) const FORMAT_7_VERSION: &[u8] = b"7\n";
/// The format new repositories are created at. Visible at `pub(crate)` so `format_stability_gate.rs`'s
/// pinning test can assert this byte form and `CURRENT_FORMAT_VERSION_NUMERIC` against literal expected
/// values side by side.
pub(crate) const CURRENT_FORMAT_VERSION: &[u8] = b"8\n";
/// RFC 168 §3.1: the recovery log's path under `.prikk/`. Named here, not in `recovery_log`, so the layout layer does not depend on
/// the recovery module that depends on it.
pub(crate) const RECOVERY_LOG_RELATIVE: &str = "recovery/log";
/// Numeric companion to `CURRENT_FORMAT_VERSION`, for `format_stability_gate.rs`'s own range
/// arithmetic (RFC 114 §4, Gate B layer 1). Kept as an independent literal, not parsed from
/// `CURRENT_FORMAT_VERSION` at compile or test time -- see
/// `format_stability_gate::current_format_version_byte_and_numeric_forms_agree` for why.
#[cfg(test)]
pub(crate) const CURRENT_FORMAT_VERSION_NUMERIC: u32 = 8;

/// Repository format selected by the authoritative `.prikk/FORMAT` marker.
///
/// RFC 103 retired format 1; RFC 102 Stage 3 retired format 2 the same way, RFC 102 Stage 4 did the
/// same again for format 3, RFC 102 Stage 5 did it once more for format 4, and RFC 102 Stage 6 does it
/// again for format 5 -- rejected at open (`read_repository_format`), not merely unsupported for
/// mutation, no variant naming it here. **This bump was sought and decided by the owner explicitly**
/// (design-v1.md §14.7, 2026-08-15) and Stage 6 follows the same precedent (design-v1.md §15.6):
/// Stage 6 Step 1 adds a B slot and a generation log for each of the three compacting containers, new
/// names `durable_append`'s strictness makes unsafe to leave undetected in an older repository. The
/// single remaining variant is kept as an enum rather than collapsed away, per design-v1.md §12.1's
/// own note: `require_current_format`'s disk re-read is a real runtime check (RFC 103 Increment B was
/// abandoned specifically because of it), so the enum's *shape* still carries meaning and is not free
/// to simplify away.
///
/// **"Format 2"/"format 3"/"format 4"/"format 5"/"format 6" here name the on-disk repository layout**
/// (loose objects/refs vs. RFC 102's containers) **-- a different axis from DC-40's "format-2" wire
/// schema** (`block_state.rs`, `state_root.rs`, `format.rs`'s Block/Patch shape and Merkle rules),
/// which no RFC 102 stage touches and which keeps its own "format-2" name regardless of what this
/// enum's current variant is called (design §8: "format-2's rejection of the ahead-log state" is
/// explicitly unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepositoryFormat {
    /// Format 6: RFC 102 Stage 6 Step 1's generation-aware index containers (ref pointer index,
    /// received-ref index, trust policy container), layered on every prior stage's container work,
    /// still writable under the unchanged DC-40 schema and state-root rules. **One record per object
    /// id.** (The name predates format 7; it is format 6, and is still read and written.)
    CurrentV6,
    /// Format 7 (RFC 156 §5b): format 6 plus **"an id may hold several records; the last is
    /// authoritative"**. Nothing else differs, so every format-6 repository is already a valid format-7
    /// repository, and `prikk format upgrade` changes only the marker. Older binaries refuse it at open.
    V7,
    /// Format 8 (0.57.0, Unicode repository paths): format 7 plus **"a repository path may be non-ASCII"**.
    /// Nothing stored differs, so every format-7 repository is already a valid format-8 repository, and
    /// `prikk format upgrade` changes only the marker. Its job is to cut off older binaries cleanly: one
    /// of them meeting such a path reads damage or silently admits it (measured, 0.55.0), where a
    /// marker they do not know is refused at open, uniformly, before any command's own logic runs.
    /// New repositories are created at format 8; an ASCII-only repository never needs to move.
    V8,
}

impl RepositoryFormat {
    /// The number the `FORMAT` marker holds.
    #[must_use]
    pub const fn number(self) -> u32 {
        match self {
            Self::CurrentV6 => 6,
            Self::V7 => 7,
            Self::V8 => 8,
        }
    }

    /// Whether this format permits several records for one object id (RFC 156 §5b).
    #[must_use]
    pub const fn holds_several_records_per_id(self) -> bool {
        matches!(self, Self::V7 | Self::V8)
    }

    /// Whether a repository path may be non-ASCII to **author** in this format (0.57.0). Authoring is
    /// where the format matters: a format-6 or format-7 repository refuses, naming `prikk format
    /// upgrade`, because writing such a path there would hand older binaries history they misread.
    #[must_use]
    pub const fn admits_non_ascii_paths(self) -> bool {
        matches!(self, Self::V8)
    }
}

/// One container's pre-allocated alternate slot (RFC's §3.2 compaction requirement: a fixed A/B pair
/// of names, never a rotated/new name). Object and ref-log containers keep `B` reserved-but-unused
/// forever, per design-v1.md §15.2 -- object compaction has no data model to target and the ref log
/// must never be compacted (DC-38/DC-69). The three genuine compaction targets (ref pointer index,
/// received-ref index, trust policy container -- design-v1.md §15.1) got their own `A`/`B` slots in
/// Stage 6 Step 1; `B` is written only once Stage 6 Step 2's compactor exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerSlot {
    /// The slot every write targets until compaction (Stage 6 Step 2) exists.
    A,
    /// The alternate slot compaction publishes to. Unused by object/ref-log containers, which never
    /// compact; unused by the three Step-1-generation-aware containers until Step 2 lands.
    B,
}

impl ContainerSlot {
    /// Return a stable lower-case label used in the container's file name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::A => "a",
            Self::B => "b",
        }
    }

    /// Return the other slot -- the compactor's own "which slot am I retiring into" question
    /// (design-v1.md §15.6/§4: compaction always writes the slot that is *not* currently live).
    #[must_use]
    pub const fn other(self) -> Self {
        match self {
            Self::A => Self::B,
            Self::B => Self::A,
        }
    }
}

/// One of the four containers RFC 102 Stage 6 Step 2 locks against concurrent writer/compactor races
/// (design-v1.md §15.8, ruled **wide** by the project owner over the developer's own narrower lean):
/// the three genuine compaction targets, plus the ref log, whose own tearing exposure predates RFC 102
/// and is not caused by compaction, but is fixed here because the exclusion machinery being built for
/// compaction closes it for free. `trust_key_container` is deliberately absent -- it never compacts,
/// and stays protected by the unchanged, repository-wide `ActiveLock` alone, the same as before this
/// stage.
///
/// `derive(Ord)` on a fieldless enum compares by declaration order, which **is** the one fixed total
/// lock order every multi-container acquisition sorts into (`lock::acquire_container_locks`,
/// design-v1.md §15.7's deadlock ruling) -- no call site can express an inverted order even by
/// accident, because sorting is structural, not a discipline to remember.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LockableContainer {
    /// `refs/containers/pointer-index-{a,b}.container` -- Stage 6 Step 1's own generation-aware
    /// target.
    RefPointerIndex,
    /// `refs/containers/log-{a,b}.container` -- never compacted (DC-38/DC-69), but shares the ref
    /// pointer index's write path (`publication.rs`) and the same unserialized-appender exposure.
    RefLog,
    /// `refs/containers/received-index-{a,b}.container` -- Stage 6 Step 1's own generation-aware
    /// target, and the container whose investigation (`bundle.rs:270`'s `import_bundle`, no lock at
    /// all) is what surfaced this whole ruling.
    ReceivedIndex,
    /// `trust/policy-{a,b}.container` -- Stage 6 Step 1's own generation-aware target. Already
    /// incidentally protected today by `ActiveLock` (`trust.rs:88,144`); gains its own dedicated lock
    /// here anyway, per the owner's decision that the lock is container-scoped, not repository-wide
    /// (design-v1.md §15.7 decision 2) -- so a `prikk compact` run on this container never contends
    /// with unrelated `ActiveLock` holders (a `commit`, a `seal`) that never touch it.
    TrustPolicy,
    /// `containers/` -- every object container **and** the single `index.container` beside them,
    /// taken together as one resource (RFC 102 §, ruled 2026-09-12).
    ///
    /// One lock for all of them, not one per object type, because the index is a *single shared
    /// file*: a `commit` appending a Patch and a `tag create` appending a Tag write to different
    /// object containers but to the same index, so per-type locks would not serialise them.
    ///
    /// **Declared last, and that is load-bearing.** This lock is a leaf -- `append_object_to_container`
    /// acquires and releases it around one append, and nothing is ever acquired while it is held --
    /// so it can take part in no cycle. Sorting last additionally means that if a future caller ever
    /// *did* request it alongside another container in one `acquire_container_locks` call, it would
    /// still be taken after them, matching the nesting `publish_ref` already performs by holding
    /// `RefLock` + `{RefPointerIndex, RefLog}` across an object write.
    ObjectStore,
}

impl LockableContainer {
    /// Every variant, in the declared total order -- the one enumeration `prikk unlock` (and anything
    /// else that needs to sweep every container lock) should use, rather than hand-rolling the list
    /// and risking it drift from the enum's own declaration.
    pub const ALL: [Self; 5] = [
        Self::RefPointerIndex,
        Self::RefLog,
        Self::ReceivedIndex,
        Self::TrustPolicy,
        Self::ObjectStore,
    ];
}

/// Repository layout paths.
#[derive(Debug, Clone)]
pub struct RepositoryLayout {
    root: PathBuf,
    prikk_dir: PathBuf,
    worktree_mutation: MutationRoot,
    repository_mutation: MutationRoot,
    format: RepositoryFormat,
    /// `prikk format upgrade` over a lost marker (0.59.0 step K4): the marker was absent or empty on a populated repository when this layout was
    /// opened, and the layout stands for format 8 until the marker is written. Only `open_allowing_lost_marker` sets it.
    marker_lost: bool,
}

impl PartialEq for RepositoryLayout {
    fn eq(&self, other: &Self) -> bool {
        self.root == other.root && self.prikk_dir == other.prikk_dir
    }
}

impl Eq for RepositoryLayout {}

impl RepositoryLayout {
    /// Create a layout for a working tree root.
    pub fn new(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let prikk_dir = root.join(REPO_DIR);
        // RFC 132 refusal sweep, rule 5: a path holding no repository is a precondition, decided here,
        // once, for every command that opens one. Only a missing `.prikk` (or a missing root) is this;
        // any other I/O failure below stays `Io`.
        if let Err(err) = std::fs::symlink_metadata(&prikk_dir) {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(PrikkError::Precondition(format!(
                    "no prikk repository at {}",
                    root.display()
                )));
            }
        }
        let worktree_mutation = MutationRoot::open(&root)?;
        // 0.58.0 P1, rule 5: this is the one fsync a read-only command (`status`, `verify`, `log`) makes, and it is a DIRECTORY sync: it exists on Linux and
        // macOS only (a Windows directory has no sync), so on Windows a reader makes none. `open_root` walks `.prikk` with
        // `open_validated_child`, which syncs the PARENT (the worktree root) of every directory it opens, so that a directory entry an
        // earlier run created (`init`'s `mkdir`, whose own parent sync may not have happened before a crash) is durable before this run
        // builds on it. A reader does not need that; the writers that share this open do (nothing else ever syncs the worktree root's
        // entry for `.prikk`), and this function cannot tell which it is serving. Removing the sync for readers is a read-only open,
        // a new API; it is the architect's to rule (P1 report), not a one-line deletion.
        let repository_mutation = worktree_mutation.open_root(Path::new(REPO_DIR))?;
        Self::opened(
            root,
            prikk_dir,
            worktree_mutation,
            repository_mutation,
            false,
        )
    }

    /// `new`'s second half: read the marker, or refuse with the sentence of a lost one. `lost_marker_is_a_format_8_layout` is `prikk format
    /// upgrade`'s one exception: a populated repository whose marker is lost opens as format 8, flagged, so the verification it runs reads it as it
    /// will be labelled.
    fn opened(
        root: PathBuf,
        prikk_dir: PathBuf,
        worktree_mutation: MutationRoot,
        repository_mutation: MutationRoot,
        lost_marker_is_a_format_8_layout: bool,
    ) -> Result<Self> {
        // One read of the marker serves both questions: is it lost, and what does it say.
        let bytes = read_file_if_exists(&repository_mutation, Path::new("FORMAT"))?;
        let state = lost_marker::state_of(bytes.as_deref());
        let mut layout = Self {
            root,
            prikk_dir,
            worktree_mutation,
            repository_mutation,
            format: RepositoryFormat::V8,
            marker_lost: false,
        };
        if state.is_lost() {
            if lost_marker_is_a_format_8_layout && !layout.holds_only_a_killed_inits_state()? {
                layout.marker_lost = true;
                return Ok(layout);
            }
            return Err(layout.lost_marker_refusal(state)?);
        }
        layout.format = parse_repository_format(bytes.as_deref().unwrap_or_default())?;
        Ok(layout)
    }

    /// Open for the two commands a lost marker's way out runs through: `prikk format upgrade` (which writes the marker) and `prikk unlock` (which clears
    /// the lock a killed `format upgrade` left, and which that refusal names). A populated repository whose marker is lost opens as format 8, flagged
    /// (`marker_is_lost`); an unpopulated `.prikk` still refuses, naming `prikk init`. No other command opens one.
    ///
    /// # Errors
    ///
    /// No repository at `root`; an unpopulated `.prikk` whose marker is absent or empty; a marker this version does not read.
    pub fn open_allowing_lost_marker(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let prikk_dir = root.join(REPO_DIR);
        if let Err(err) = std::fs::symlink_metadata(&prikk_dir) {
            if err.kind() == std::io::ErrorKind::NotFound {
                return Err(PrikkError::Precondition(format!(
                    "no prikk repository at {}",
                    root.display()
                )));
            }
        }
        let worktree_mutation = MutationRoot::open(&root)?;
        let repository_mutation = worktree_mutation.open_root(Path::new(REPO_DIR))?;
        Self::opened(
            root,
            prikk_dir,
            worktree_mutation,
            repository_mutation,
            true,
        )
    }

    /// Whether this layout was opened over a lost marker (see `open_allowing_lost_marker`).
    pub(crate) const fn marker_is_lost(&self) -> bool {
        self.marker_lost
    }

    /// Initialize a repository layout on disk.
    pub fn init(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        let prikk_dir = root.join(REPO_DIR);
        let worktree_mutation = MutationRoot::open(&root)?;
        let repository_mutation = worktree_mutation.ensure_root(Path::new(REPO_DIR))?;
        let state = lost_marker::marker_state(&repository_mutation)?;
        let existing = if state.is_lost() {
            None
        } else {
            read_file_if_exists(&repository_mutation, Path::new("FORMAT"))?
        };
        if let Some(version) = &existing {
            if version.as_slice() != CURRENT_FORMAT_VERSION
                && version.as_slice() != FORMAT_7_VERSION
                && version.as_slice() != FORMAT_6_VERSION
            {
                // RFC 102 Stage 3, design-v1.md §12.1: this refusal inherited the format-2 -> 3 bump;
                // Stage 4 carried it forward for format-3 -> 4, Stage 5 did the same for format-4 ->
                // 5, and Stage 6 does it again for format-5 -> 6 (design-v1.md §15.6). Audited, not
                // just the constant swapped: unlike `read_repository_format`'s own rejection (reached
                // via `open`), this fires only on a redundant `init` against an already-initialized
                // repository of some other format, so it stays terse and points at `open` (any other
                // command) for the detailed migration message rather than duplicating it here.
                return Err(PrikkError::Integrity(
                    "refusing to initialize an existing Prikk repository that is none of formats 6, \
                     7 and 8 (open it with any other command for a detailed unsupported-format \
                     message)"
                        .to_string(),
                ));
            }
        }
        // An existing repository keeps its own format — re-running `init` never upgrades one (RFC 156
        // §5b: the upgrade is explicit). A new one is created at format 8.
        let format = if existing.as_deref() == Some(FORMAT_6_VERSION) {
            RepositoryFormat::CurrentV6
        } else if existing.as_deref() == Some(FORMAT_7_VERSION) {
            RepositoryFormat::V7
        } else {
            RepositoryFormat::V8
        };
        let layout = Self {
            root,
            prikk_dir,
            worktree_mutation,
            repository_mutation,
            format,
            marker_lost: false,
        };
        // 0.59.0 step K4: `init` never labels a repository it did not make. An absent or empty marker over anything but what a killed `init` leaves is
        // a repository whose format number is lost, and `prikk format upgrade` is the verb that proves it and writes the marker. Nothing is written first.
        if state.is_lost() && !layout.holds_only_a_killed_inits_state()? {
            return Err(layout.lost_marker_refusal(state)?);
        }
        // Every name `init` creates is one entry of `init_entries`, in the order it is created (`init_set.rs`): that list is also what the question above
        // reads, so the two cannot drift. Each is created once and never clobbered: a missing file and an idempotent re-`init` on an already-initialized
        // repository must not overwrite either (RFC 102 Stage 1). Every file is allocated here and nowhere else (RFC 102 Stage 3 acceptance criterion 1;
        // Stage 4's ref-log and pointer-index containers; Stage 6's compaction targets; DC-53's author-key container: a repository initialized before any
        // of them simply has no such file and reads it as empty). The current-branch pointer is `heads/main` (RFC 151 §2.1), only when absent: a
        // re-`init` never moves a branch someone switched to.
        for entry in layout.init_entries()? {
            match entry {
                init_set::InitEntry::Directory(dir) => {
                    ensure_directory_required(layout.repository_mutation_root(), &dir)?;
                }
                init_set::InitEntry::File { path, bytes: [] } => {
                    create_empty_file_once(&layout, &layout.prikk_dir.join(path))?;
                }
                init_set::InitEntry::File { path, bytes } => {
                    if read_file_if_exists(layout.repository_mutation_root(), &path)?.is_none() {
                        create_new_file_required(layout.repository_mutation_root(), &path, bytes)?;
                    }
                }
            }
        }
        // RFC 102 Stage 5, design-v1.md §14.2: written last, once every container/marker/WAL name above is confirmed present. `FORMAT`'s presence is what
        // certifies `init` completed: written first, a crash between it and the containers left a repository that read as a valid, empty one with every
        // container absent, and nothing detected it. Written last, an interrupted `init` leaves it absent, a re-`init` re-enters this body, and every
        // creation above is idempotent.
        //
        // 0.59.0 step K4: through a temporary file and a rename, not `create_exclusive` (create, then write). A kill between the create and the write
        // left an EMPTY marker, which every command refused and none could clear (`real_kill_sweep`, ordinal 221 of 228). Now a kill leaves the old marker
        // (none) or the new, and at most a `FORMAT.tmp.*` that the question above accepts. **Nothing else writes the marker while this runs:** no other
        // command opens a repository whose marker is lost (`new` refuses), `prikk format upgrade` writes it only over a POPULATED repository and `init`
        // only over an unpopulated one, and a second `init` writes the same two bytes through its own temporary, so a rename over an `init`'s rename is
        // the same file. The rename replaces the empty marker of a killed pre-0.59.0 `init`.
        if lost_marker::marker_state(layout.repository_mutation_root())?.is_lost() {
            write_file_atomically(
                layout.repository_mutation_root(),
                Path::new("FORMAT"),
                CURRENT_FORMAT_VERSION,
            )?;
        }
        Ok(layout)
    }

    /// Open an existing repository layout.
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        Self::new(root)
    }

    /// Return the repository format selected when this layout was opened.
    #[must_use]
    pub const fn format(&self) -> RepositoryFormat {
        self.format
    }

    pub(crate) fn validate_format(&self) -> Result<()> {
        // `prikk format upgrade` over a lost marker (K4) runs its verification before the marker exists: the layout stands for format 8 while it is absent.
        if self.marker_lost && lost_marker::marker_state(self.repository_mutation_root())?.is_lost()
        {
            return Ok(());
        }
        let format = read_repository_format(self.repository_mutation_root())?;
        if format != self.format {
            // The marker changed under an open layout — `prikk format upgrade` ran meanwhile. Name
            // both versions rather than a placeholder number (RFC 156 §5b).
            return Err(PrikkError::Precondition(format!(
                "the repository format changed from {} to {} while this command ran; run it again",
                self.format.number(),
                format.number()
            )));
        }
        Ok(())
    }

    /// Refuse mutation unless the marker still names the format this layout opened with. Formats 6 and
    /// 7 are both current (RFC 156 §5b); every older format is already refused at open.
    pub fn require_current_format(&self) -> Result<()> {
        self.validate_format()
    }

    /// Return the working tree root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Return the `.prikk` directory.
    #[must_use]
    pub fn prikk_dir(&self) -> &Path {
        &self.prikk_dir
    }

    /// Return the repository format marker path.
    #[must_use]
    pub fn format_path(&self) -> PathBuf {
        self.prikk_dir.join("FORMAT")
    }

    /// Return the unclean-shutdown worktree marker path (RFC 102 Stage 1). Created empty at `init`;
    /// non-empty means worktree materialization was interrupted and commit-authoring must refuse to
    /// infer deletion from absence until the worktree is re-verified against its baseline. Always
    /// updated by append/truncate (`fsutil::append_file_required`/`truncate_file_empty_required`),
    /// never by `atomic_replace` -- RFC 102 §3's correction: `atomic_replace` renames over the
    /// destination unconditionally, which is a new-name event whose Windows durability is DC-87
    /// §3.4's still-open question, exactly the gap this marker exists to close.
    #[must_use]
    pub fn worktree_unclean_shutdown_marker_path(&self) -> PathBuf {
        self.prikk_dir.join("worktree.marker")
    }

    /// Return the provisional-worktree marker path (RFC 136 §10.3b.2), beside the unclean-shutdown
    /// marker and on the same pattern: created empty at `init`, set by appending a record naming the
    /// ref and snapshot block `checkout --snapshot-materialize` wrote from, and cleared only by
    /// `prikk verify` truncating it to empty. Non-empty means the worktree is not replay-verified, and
    /// the derivation gate refuses every command that turns worktree content into history.
    #[must_use]
    pub fn provisional_worktree_marker_path(&self) -> PathBuf {
        self.prikk_dir.join("worktree.provisional")
    }

    /// Return the object root directory.
    ///
    /// Dead-surface consolidation: no longer in `required_directories()` (RFC-format-3 repositories
    /// write containers, not loose object files, so a fresh repository never gets this tree). Kept
    /// `pub(crate)`, not removed, because `verify/objects.rs`'s `scan_loose_file_temp_debris` still
    /// reads it on every `verify` -- a dormant, format-3-only-relevant diagnostic (design-v1.md §12.3
    /// item 3) whose own retirement was explicitly reserved for an RFC-level act, not a side effect of
    /// this consolidation. `inspect_entry` tolerates the tree's absence, so a repository that never had
    /// it (or an old one that did) both verify cleanly.
    #[must_use]
    pub(crate) fn objects_dir(&self) -> PathBuf {
        self.prikk_dir.join("objects")
    }

    /// Return the active-session root directory.
    #[must_use]
    pub fn active_dir(&self) -> PathBuf {
        self.prikk_dir.join("active")
    }

    /// Return the active-session directory for `name`.
    ///
    /// RFC 108 increment 1: `.prikk/active/<name>/` was already the on-disk shape; only the
    /// choice of name was hardcoded. This method is the layout's own generalization of it,
    /// kept `pub(crate)` for now since nothing outside this crate constructs a second active
    /// session yet -- that is a later increment's CLI-surface decision, not this one's.
    ///
    /// RFC 108 increment 2 review: widened from `&str` to `impl AsRef<Path>` so a name obtained
    /// from `active_session_names()` -- an `OsString`, which may not be valid UTF-8 on a POSIX
    /// filesystem -- can be used to **derive** this path directly, rather than forcing a caller to
    /// **reconstruct** it through a lossy `String` round-trip first. Every existing caller passes a
    /// `&str` literal (`DEFAULT_ACTIVE_NAME`), which still satisfies this bound unchanged.
    #[must_use]
    pub(crate) fn active_session_dir(&self, name: impl AsRef<Path>) -> PathBuf {
        self.active_dir().join(name)
    }

    /// Return the active WAL path for `name`. See `active_session_dir`'s doc for why this takes
    /// `impl AsRef<Path>` rather than `&str`.
    #[must_use]
    pub(crate) fn active_queue_wal_path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.active_session_dir(name).join("queue.wal")
    }

    /// Return the active lock path for `name`. See `active_session_dir`'s doc for why this takes
    /// `impl AsRef<Path>` rather than `&str`.
    #[must_use]
    pub(crate) fn active_lock_path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.active_session_dir(name).join("active.lock")
    }

    /// Return the active-session directory for `name`, relative to the repository root -- the
    /// `repository_relative`-equivalent of `active_session_dir`, but derived directly from the same
    /// components rather than computed by stripping a prefix off the absolute path.
    ///
    /// RFC 108 increment 3a: `wal.rs::Wal::for_layout` used to rebuild its own relative path via
    /// `format!("active/{name}/queue.wal")` -- a reconstruction that cannot accept a non-UTF-8
    /// `name` at all (`format!` requires `Display`), unlike `lock.rs::ActiveLock::acquire`, which
    /// already derives its relative path from its own absolute one via `repository_relative`. This
    /// method gives `Wal::for_layout` the same derive-not-reconstruct shape without making it
    /// fallible: `active_session_dir` is built by joining `name` onto `self.prikk_dir` in the first
    /// place, so stripping that exact prefix back off can never fail and always yields exactly
    /// `Path::new("active").join(name)` -- asserted equal to `repository_relative`'s own output for
    /// the same absolute path, not merely believed to be, by this file's own pinning test.
    #[must_use]
    pub(crate) fn active_session_relative_dir(&self, name: impl AsRef<Path>) -> PathBuf {
        Path::new("active").join(name)
    }

    /// Return the active WAL path for `name`, relative to the repository root. See
    /// `active_session_relative_dir`'s own doc for why this is infallible and byte-exact.
    #[must_use]
    pub(crate) fn active_queue_wal_relative_path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.active_session_relative_dir(name).join("queue.wal")
    }

    /// Return the active-session names currently present on disk (RFC 108 increment 2) -- the
    /// read-back counterpart of `active_session_dir`'s own construction authority, sited here
    /// rather than in `active.rs` for the same reason: `active.rs`'s own module doc describes it
    /// as the *operational* boundary for one active session's WAL/lock/ref-metadata lifecycle,
    /// never a discovery registry, while this layout already owns the shape `active/<name>/`
    /// enumerates over.
    ///
    /// **A missing `active/` directory returns an empty list, not an error.** `unlock` is a
    /// recovery surface, run precisely when a repository may not be fully valid --
    /// `required_directories` guarantees this directory on a *valid* repository, which is not the
    /// guarantee this caller can rely on. Checked with `inspect_entry` first (the same
    /// absence-tolerant pattern `verify/objects.rs::scan_loose_file_temp_debris` and
    /// `patch_checkout.rs`'s deleted-file handling already use), not assumed from
    /// `list_directory`'s own doc comment: `list_directory` itself errors outright on an absent
    /// directory (confirmed directly against its `PosixReader` implementation, not inferred), so
    /// a caller that skipped this check would make `prikk unlock` fail on exactly the damaged
    /// repository it exists to recover.
    ///
    /// **Sorted by raw name bytes** for deterministic output across platforms --
    /// `scan_loose_file_temp_debris` established this exact key already; directory listing order
    /// is not guaranteed and differs by filesystem, so an unsorted result would make output from
    /// this method platform-dependent with no `#[cfg(target_os)]` anywhere to show it.
    ///
    /// **Returns `OsString`, not `String`.** A review of the first version of this method found it
    /// returning `String` via `to_string_lossy()`, and the one consumer (`unlock::list_held_locks`)
    /// then reconstructing a path from that lossy string with `active_lock_path(&name)` -- for any
    /// session directory whose name is not valid UTF-8 (valid on POSIX filesystems), the
    /// reconstructed path does not match the path on disk, so `read_lock_if_present` finds nothing
    /// and a held lock is silently dropped from the report. That is the exact defect this increment
    /// exists to prevent, reintroduced one layer down. The per-ref lock loop already gets this
    /// right (`ref_locks_dir.join(&entry.name)`, the `OsString` straight from the listing, never
    /// round-tripped through `String`) -- this method now follows that same neighbour. Every caller
    /// of `active_session_dir`/`active_queue_wal_path`/`active_lock_path` **derives** its path from
    /// this value directly; none may re-`to_string_lossy()` it first.
    pub(crate) fn active_session_names(&self) -> Result<Vec<std::ffi::OsString>> {
        let active_dir = self.active_dir();
        let relative = self.repository_relative(&active_dir)?;
        match inspect_entry(self.repository_mutation_root(), &relative)? {
            None => return Ok(Vec::new()),
            Some(EntryKind::Directory) => {}
            Some(_) => {
                return Err(PrikkError::Integrity(format!(
                    "unexpected non-directory where the active-session root should be: {}",
                    active_dir.display()
                )));
            }
        }
        let mut entries = list_directory(self.repository_mutation_root(), &relative)?;
        entries.sort_by(|left, right| {
            left.name
                .as_encoded_bytes()
                .cmp(right.name.as_encoded_bytes())
        });
        Ok(entries
            .into_iter()
            .filter(|entry| entry.kind == EntryKind::Directory)
            .map(|entry| entry.name)
            .collect())
    }

    /// Return the default active-session directory.
    #[must_use]
    pub fn default_active_dir(&self) -> PathBuf {
        self.active_session_dir(DEFAULT_ACTIVE_NAME)
    }

    /// Return the default active WAL path.
    #[must_use]
    pub fn default_queue_wal_path(&self) -> PathBuf {
        self.active_queue_wal_path(DEFAULT_ACTIVE_NAME)
    }

    /// Return the default active lock path.
    #[must_use]
    pub fn default_active_lock_path(&self) -> PathBuf {
        self.active_lock_path(DEFAULT_ACTIVE_NAME)
    }

    /// Return the active-session ref-name metadata path for `name`. See `active_session_dir`'s doc
    /// for why this takes `impl AsRef<Path>` rather than `&str` (RFC 108 increment 3c: the same
    /// `OsString`-from-`active_session_names()` reach every other `active_*` accessor already
    /// supports).
    #[must_use]
    pub(crate) fn active_ref_name_path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.active_session_dir(name).join("ref-name")
    }

    /// Return the default active-session ref-name metadata path.
    #[must_use]
    pub fn default_active_ref_name_path(&self) -> PathBuf {
        self.active_ref_name_path(DEFAULT_ACTIVE_NAME)
    }

    /// Return the active-session live rename-declaration store path for `name` (RFC 144 §4o.2) --
    /// a sibling of `queue.wal`/`ref-name`, on the same `active/<name>/` shape, pre-allocated at
    /// `init` the same way. See `active_session_dir`'s doc for why this takes `impl AsRef<Path>`
    /// rather than `&str`.
    #[must_use]
    pub(crate) fn active_declarations_path(&self, name: impl AsRef<Path>) -> PathBuf {
        self.active_session_dir(name).join("declarations")
    }

    /// Return the default active-session live rename-declaration store path.
    #[must_use]
    pub fn default_declarations_path(&self) -> PathBuf {
        self.active_declarations_path(DEFAULT_ACTIVE_NAME)
    }

    /// Return the current-branch pointer path (RFC 151 §2.1): `.prikk/current-branch`, one local
    /// branch ref name and a newline. Read by `refs::current_branch`, never by anything that decides
    /// trust.
    #[must_use]
    pub fn current_branch_path(&self) -> PathBuf {
        self.prikk_dir.join("current-branch")
    }

    /// Return the ref root directory.
    #[must_use]
    pub fn refs_dir(&self) -> PathBuf {
        self.prikk_dir.join("refs")
    }

    /// Return the cache directory.
    #[must_use]
    pub fn cache_dir(&self) -> PathBuf {
        self.prikk_dir.join("cache")
    }

    /// Return the container root directory (RFC 102 Stage 3, design-v1.md §2).
    #[must_use]
    pub fn containers_dir(&self) -> PathBuf {
        self.prikk_dir.join("containers")
    }

    /// Return the container directory for a persisted object type.
    #[must_use]
    pub fn container_type_dir(&self, object_type: ObjectType) -> PathBuf {
        self.containers_dir()
            .join(object_type_directory_name(object_type))
    }

    /// Return one object type's container file for a given slot. **Every slot's name is allocated
    /// at `init`, including B, even though Stage 3 only ever writes A** -- compaction (Stage 6, not
    /// authorized) is what would ever target B; the RFC's §3.2 fixed-name-set requirement applies to
    /// the whole RFC, not per stage, so the name exists now regardless of when it is first used.
    #[must_use]
    pub fn container_slot_path(&self, object_type: ObjectType, slot: ContainerSlot) -> PathBuf {
        self.container_type_dir(object_type)
            .join(format!("{}.container", slot.as_str()))
    }

    /// Return the object index's container path. Single file, no A/B slot -- design-v1.md §4 /
    /// RFC 102's own §6.7 answer #2: the index's publication shape is plain append-only ("A/B" for an
    /// index reduces to "append-only wearing an A/B costume, not a second option" once forced through
    /// this codebase's real primitives), so unlike the six object-type containers it needs only one
    /// name.
    #[must_use]
    pub fn container_index_path(&self) -> PathBuf {
        self.containers_dir().join("index.container")
    }

    /// Return the small, fixed-name compaction generation log (design-v1.md §4: "compaction publishes
    /// by appending a generation record to a small fixed-name log; readers take the last complete
    /// generation record"). **Reserved, not used, by Stage 3** -- its name must still be allocated at
    /// `init` because compaction (Stage 6) is not authorized to create any name later. Absent any
    /// generation record (the only state Stage 3 ever produces), every container type's slot A is
    /// live by construction -- there is nothing for an empty log to disambiguate yet.
    #[must_use]
    pub fn container_generation_log_path(&self) -> PathBuf {
        self.containers_dir().join("generations.log")
    }

    /// Return the ref-container root directory (RFC 102 Stage 4). Kept under `refs/`, sibling to the
    /// now-vestigial `by-id/`/`logs/`/`tmp/`/`locks/` directories, rather than under the object
    /// `containers/` tree -- ref containers are not object containers and the two are never confused
    /// for the same purpose.
    #[must_use]
    pub fn refs_containers_dir(&self) -> PathBuf {
        self.refs_dir().join("containers")
    }

    /// Return the shared ref-log container file for a given slot (Step 0 §13.2: one container holds
    /// every ref's log records, forced by acceptance criterion 1 -- ref names do not exist at `init`,
    /// so a per-ref container is architecturally impossible). **Both slots allocated at `init`**,
    /// matching Stage 3's own A/B convention exactly (`container_slot_path`'s own doc comment) --
    /// Stage 4 only ever writes `A`.
    #[must_use]
    pub fn ref_log_container_slot_path(&self, slot: ContainerSlot) -> PathBuf {
        self.refs_containers_dir()
            .join(format!("log-{}.container", slot.as_str()))
    }

    /// RFC 168 §3.1 and §3.3: the recovery log and the default session's witness exist before a write relies on them. `init`
    /// creates them; a repository created earlier gets them at its first write, through `ActiveLock::acquire` for the default
    /// session (the one call site every writer passes through). An absent file is created exclusively: the one new-name case
    /// (RFC 168 §6, residuals (b) and (c)).
    pub(crate) fn ensure_write_state(&self) -> Result<()> {
        ensure_directory_required(self.repository_mutation_root(), Path::new("recovery"))?;
        create_empty_file_once(self, &self.prikk_dir().join(RECOVERY_LOG_RELATIVE))?;
        create_empty_file_once(
            self,
            &self.active_session_dir(DEFAULT_ACTIVE_NAME).join("witness"),
        )
    }

    /// Return the ref-pointer-index container path for a given slot (RFC 102 Stage 6 Step 1,
    /// design-v1.md §15.6: this is one of the three genuine compaction targets -- `ref_pointer_index`
    /// is last-entry-wins, and every ref update strands the previous entry, §15.1's own finding. `A`/`B`
    /// slots mirror `container_slot_path`'s own naming shape). Reads and writes resolve which slot is
    /// live through `generation.rs`'s resolver; Step 1 always resolves `A` because no generation record
    /// has ever been written -- see `ref_pointer_index_generation_log_path`.
    #[must_use]
    pub fn ref_pointer_index_slot_path(&self, slot: ContainerSlot) -> PathBuf {
        self.refs_containers_dir()
            .join(format!("pointer-index-{}.container", slot.as_str()))
    }

    /// Return the ref-pointer-index generation log path (RFC 102 Stage 6 Step 1, design-v1.md §15.6
    /// item 3/§4: readers take the last complete generation record; empty until Step 2's compactor
    /// ever writes one, at which point `A` stops being the unconditional answer). Its own name, not
    /// the pre-existing `container_generation_log_path()` -- that name was allocated for object-
    /// container compaction, which §15.1 establishes will never happen under the current content-
    /// addressed, no-GC data model, and a shared log across independently-compacting containers would
    /// let one corrupt record take down slot resolution for all of them at once (§15.6's own
    /// blast-radius reasoning).
    #[must_use]
    pub fn ref_pointer_index_generation_log_path(&self) -> PathBuf {
        self.refs_containers_dir()
            .join("pointer-index-generation.log")
    }

    /// Return the received-ref-index container path for a given slot (RFC 102 Stage 6 Step 1,
    /// design-v1.md §15.6: the second of the three genuine compaction targets, same last-entry-wins
    /// shape as the ref pointer index). Formerly `received_index_path`, single-name -- RFC 102 Stage 5,
    /// design-v1.md §14.1/Step 0 item 2's own reasoning for why `received.rs` belongs on the refs
    /// container+pointer-index pattern is unaffected by gaining a slot; only the publication shape
    /// (single-name vs. resolver-selected) changed.
    #[must_use]
    pub fn received_index_slot_path(&self, slot: ContainerSlot) -> PathBuf {
        self.refs_containers_dir()
            .join(format!("received-index-{}.container", slot.as_str()))
    }

    /// Return the received-ref-index generation log path. Its own name, for the same blast-radius
    /// reason `ref_pointer_index_generation_log_path` has its own.
    #[must_use]
    pub fn received_index_generation_log_path(&self) -> PathBuf {
        self.refs_containers_dir()
            .join("received-index-generation.log")
    }

    /// Return all required directories for layout creation.
    ///
    /// Dead-surface consolidation: `objects/` and its six type subdirectories, `refs/by-id/`,
    /// `refs/logs/`, and `quarantine/` are no longer created here -- nothing in a format-3 repository
    /// writes into any of them (containers replaced loose objects and per-ref files years ago). This is
    /// not a format change: `required_directories()` is consulted only here, at `init`; nothing
    /// validates it at `open`, so an existing repository keeps its now-unused directories harmlessly,
    /// and only a newly initialized one has fewer. `refs/tmp/` stays -- `refs/verify.rs`'s
    /// `candidate_issues` still scans it on every `verify`.
    #[must_use]
    pub fn required_directories(&self) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        dirs.push(self.containers_dir());
        for object_type in persisted_object_types() {
            dirs.push(self.container_type_dir(object_type));
        }
        dirs.push(self.active_dir());
        dirs.push(self.default_active_dir());
        dirs.push(self.refs_dir());
        dirs.push(self.refs_dir().join("locks"));
        dirs.push(self.refs_dir().join("tmp"));
        dirs.push(self.refs_containers_dir());
        dirs.push(self.trust_dir());
        dirs.push(self.cache_dir());
        dirs
    }

    pub(crate) fn repository_mutation_root(&self) -> &MutationRoot {
        &self.repository_mutation
    }

    pub(crate) fn worktree_mutation_root(&self) -> &MutationRoot {
        &self.worktree_mutation
    }

    pub(crate) fn repository_relative(&self, path: &Path) -> Result<PathBuf> {
        path.strip_prefix(&self.prikk_dir)
            .map(Path::to_path_buf)
            .map_err(|_| PrikkError::Io {
                kind: None,
                context: "path is outside repository mutation authority".to_string(),
            })
    }

    pub(crate) fn required_repository_directories(&self) -> Result<Vec<PathBuf>> {
        self.required_directories()
            .into_iter()
            .map(|path| self.repository_relative(&path))
            .collect()
    }

    /// Return the object directory for a persisted object type.
    ///
    /// `pub(crate)`, not public: its only reader is `verify/objects.rs`'s
    /// `scan_loose_file_temp_debris`, a dormant format-3-only diagnostic (see `objects_dir`'s own doc).
    #[must_use]
    pub(crate) fn object_type_dir(&self, object_type: ObjectType) -> PathBuf {
        self.objects_dir()
            .join(object_type_directory_name(object_type))
    }

    /// Return the storage path for a persisted object ID and type.
    ///
    /// No production caller by design, not by omission: this addresses the pre-container, format-1/2
    /// loose-object layout, which nothing in a live format-3 repository writes or reads. Kept `pub`
    /// specifically so `prikk-cli`'s format-transition fixtures (`tests/format_transition_support/`, a
    /// different crate) can construct legacy-shaped repositories without duplicating the private
    /// hex-prefixing scheme (`hex_prefix`, `layout.rs`) this method wraps.
    #[must_use]
    pub fn object_path(&self, object_type: ObjectType, id: ObjectId) -> PathBuf {
        let hex = id.to_hex();
        let prefix = hex_prefix(&hex);
        self.object_type_dir(object_type)
            .join(prefix)
            .join(format!("{hex}.pobj"))
    }

    /// Return the flat ref pointer path for a human-readable ref name.
    ///
    /// No production caller by design: this addresses the pre-container, format-1/2 flat-pointer-file
    /// layout containers replaced. Kept `pub` so `prikk-cli`'s format-transition fixtures (a different
    /// crate) can construct legacy-shaped repositories without reimplementing the private
    /// `ref_name_storage_key` hash this method wraps -- that helper is `pub(crate)`, unreachable from
    /// outside this crate.
    #[must_use]
    pub fn ref_pointer_path(&self, ref_name: &str) -> PathBuf {
        self.refs_dir()
            .join("by-id")
            .join(format!("{}.ref", ref_name_storage_key(ref_name)))
    }

    /// Return the ref log path for a human-readable ref name.
    ///
    /// No production caller by design, for the same reason as `ref_pointer_path`: a legacy-format path
    /// builder kept `pub` for `prikk-cli`'s cross-crate format-transition fixtures.
    #[must_use]
    pub fn ref_log_path(&self, ref_name: &str) -> PathBuf {
        self.refs_dir()
            .join("logs")
            .join(format!("{}.log", ref_name_storage_key(ref_name)))
    }

    /// Return the ref lock path for a human-readable ref name.
    #[must_use]
    pub fn ref_lock_path(&self, ref_name: &str) -> PathBuf {
        self.refs_dir()
            .join("locks")
            .join(format!("{}.lock", ref_name_storage_key(ref_name)))
    }

    /// Return the ref temporary candidate path for a human-readable ref name.
    ///
    /// No production writer since Stage 4 removed the candidate-write-then-promote mechanism, but this
    /// is not dead the way `ref_pointer_path`/`ref_log_path` are: `refs/tmp/` itself stays in
    /// `required_directories()` because `refs/verify.rs`'s `candidate_issues` scans it on every
    /// `verify`, and this accessor is what several of that scan's own regression tests
    /// (`refs/tests/publication_recovery/candidate_cleanup.rs`) use to construct debris inside it.
    #[must_use]
    pub fn ref_tmp_path(&self, ref_name: &str) -> PathBuf {
        self.refs_dir()
            .join("tmp")
            .join(format!("{}.tmp", ref_name_storage_key(ref_name)))
    }

    /// Return the publication trust-store directory.
    #[must_use]
    pub fn trust_dir(&self) -> PathBuf {
        self.prikk_dir.join("trust")
    }

    /// Return the trust key-material container path (RFC 102 Stage 5, design-v1.md §14/§14.9).
    /// Replaces the one-file-per-key-id `trust/keys/maintainer/*.pub` directory entirely -- format 5
    /// rejects every repository old enough to have one, so no repository this code can open ever
    /// contains that directory's contents (§14.9 §3's own reasoning, applied here as it was to
    /// `refs/received/`, not Stage 4's "keep, dead" precedent).
    #[must_use]
    pub fn trust_key_container_path(&self) -> PathBuf {
        self.trust_dir().join("keys.container")
    }

    /// Return the AUTHOR key-material container path (DC-53 Stage 1,
    /// `.git-exclude/reviewed/DC-53-stage-1-report-ruling-v1.md` §5). Its own container, not a
    /// third role folded into `trust_key_container_path` -- that one is MAINTAINER key material
    /// with a policy layered over it; this one is material only, populated by the authoring path
    /// rather than an adoption command (`author_key_index.rs`'s own module doc). A repository
    /// initialized before this container existed has no such file, which `author_key_index.rs`
    /// reads identically to an empty one, not a structural defect.
    #[must_use]
    pub fn author_key_container_path(&self) -> PathBuf {
        self.trust_dir().join("author-keys.container")
    }

    /// Return the archive import journal's path (RFC 155 §9.5, Part I-A). Created lazily, by the
    /// first import a repository ever runs -- not at `init`, since most repositories never import
    /// an archive. Its own directory, not a bare file directly under `.prikk/`, so a future
    /// per-run artifact (if one is ever needed) has somewhere to live without a second `init`-time
    /// allocation; today there is exactly one journal at a time, matching §9.5's own "untouched or
    /// complete" model, which never describes two imports in flight together.
    #[must_use]
    pub(crate) fn import_journal_path(&self) -> PathBuf {
        self.prikk_dir.join("import").join("journal")
    }

    /// Return the trust policy container path for a given slot (RFC 102 Stage 5, design-v1.md
    /// §14/§14.9, gaining a slot in Stage 6 Step 1, design-v1.md §15.6 -- the third of the three
    /// genuine compaction targets: one complete snapshot appended per `add`/`remove`, every earlier
    /// snapshot dead, §15.1's own finding). Each append is a **complete snapshot** of the adopted key
    /// id list, not an incremental log entry -- see `trust_index.rs`'s own module doc for why that is
    /// what makes revocation representable without a tombstone record; that property is unaffected by
    /// gaining a slot.
    #[must_use]
    pub fn trust_policy_container_slot_path(&self, slot: ContainerSlot) -> PathBuf {
        self.trust_dir()
            .join(format!("policy-{}.container", slot.as_str()))
    }

    /// Return the trust-policy generation log path. Its own name, for the same blast-radius reason
    /// `ref_pointer_index_generation_log_path` has its own -- and distinct from `trust_key_container_
    /// path`, which is **not** one of the three compacting containers and gains no slot: TOFU history
    /// must persist across removal (`trust.rs:77`), which compacting the key container would break.
    #[must_use]
    pub fn trust_policy_generation_log_path(&self) -> PathBuf {
        self.trust_dir().join("policy-generation.log")
    }

    /// Return the lock file path for one of the `LockableContainer`s
    /// (design-v1.md §15.8). Ephemeral, like every other lock file in this codebase
    /// (`ActiveLock`/`RefLock`): created on acquire, removed on release, never pre-allocated at
    /// `init` -- criterion 2's "every name created at `init`" obligation is about durability-bearing
    /// container names, not transient mutual-exclusion markers, and `ActiveLock`/`RefLock` already
    /// establish that a lock file is exempt from it.
    #[must_use]
    pub fn lockable_container_lock_path(&self, container: LockableContainer) -> PathBuf {
        match container {
            LockableContainer::RefPointerIndex => {
                self.refs_containers_dir().join("pointer-index.lock")
            }
            LockableContainer::RefLog => self.refs_containers_dir().join("log.lock"),
            LockableContainer::ReceivedIndex => {
                self.refs_containers_dir().join("received-index.lock")
            }
            LockableContainer::TrustPolicy => self.trust_dir().join("policy.lock"),
            LockableContainer::ObjectStore => self.containers_dir().join("objects.lock"),
        }
    }
}

/// Validate a maintainer key id's storage safety: ASCII alphanumeric/`-`/`_` only, and not a
/// Windows-reserved device stem. Split out from the retired `maintainer_trust_key_path` (which paired
/// this check with building a per-key-id file path that no longer exists under the container model) --
/// the validation itself is unchanged and still required before a key id is accepted.
pub(crate) fn validate_maintainer_key_id_storage_safety(key_id: &str) -> Result<()> {
    if key_id.is_empty()
        || !key_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        return Err(PrikkError::InvalidName(
            "maintainer key id is not storage-safe".to_string(),
        ));
    }
    // DC-72: the allowlist above is character-shape only and does not exclude Windows-reserved
    // device stems (`CON`, `PRN`, ...) — `CON` is all ASCII-alphanumeric and would otherwise
    // pass. Checked regardless of host OS, matching `RepoPath`'s equivalent rule.
    if is_windows_reserved_name(key_id) {
        return Err(PrikkError::InvalidName(format!(
            "maintainer key id is a Windows reserved device name: {key_id}"
        )));
    }
    Ok(())
}

/// Create `path` empty if it does not already exist. Idempotent, so a retried or re-run `init`
/// against an already-initialized repository never clobbers it -- the same rule RFC 102 Stage 1
/// established for the worktree marker and the active WAL, now shared by every `init`-time file this
/// layout creates.
/// RFC 168 F12: a concurrent first write may create the same name first. `AlreadyExists` is that outcome, not a failure.
pub(crate) fn create_if_absent(
    layout: &RepositoryLayout,
    relative: &Path,
    bytes: &[u8],
) -> Result<()> {
    match create_new_file_required(layout.repository_mutation_root(), relative, bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn create_empty_file_once(layout: &RepositoryLayout, path: &Path) -> Result<()> {
    let relative = layout.repository_relative(path)?;
    // An existence check, not a read: a zero-length ranged read opens exactly as the whole read did (same refusals, same failpoint)
    // and reads no byte. It used to read the whole file, so `init` over an existing repository read every container and the index
    // whole (RFC 160 P1's inventory found it; the whole-read guard is red if it is put back).
    if read_file_range_if_exists(layout.repository_mutation_root(), &relative, 0, 0)?.is_none() {
        create_if_absent(layout, &relative, &[])?;
    }
    Ok(())
}

mod format_marker;
mod init_set;
mod lost_marker;
use format_marker::{parse_repository_format, read_repository_format};
pub(crate) use lost_marker::SOUND_COPY;

#[cfg(test)]
mod lost_marker_tests;
#[cfg(test)]
mod tests;

/// Return persisted object types. RefUpdate is log-inline in v1 and is intentionally absent.
#[must_use]
pub fn persisted_object_types() -> [ObjectType; 7] {
    [
        ObjectType::Patch,
        ObjectType::Block,
        ObjectType::RefState,
        ObjectType::Tag,
        ObjectType::Attestation,
        ObjectType::Blob,
        ObjectType::RecognitionClaim,
    ]
}

/// Return a stable directory name for an object type.
#[must_use]
pub fn object_type_directory_name(object_type: ObjectType) -> &'static str {
    match object_type {
        ObjectType::Patch => "patch",
        ObjectType::Block => "block",
        ObjectType::RefState => "ref-state",
        ObjectType::Tag => "tag",
        ObjectType::Attestation => "attestation",
        ObjectType::Blob => "blob",
        ObjectType::RecognitionClaim => "recognition-claim",
        ObjectType::RefUpdate => "ref-update-inline-only",
        // New FDD-03 §3 types. Full storage-layout placement (`cache/block-summary/`,
        // `refs/recovery/`) is reconciled in the FDD-02 layout phase; these names keep the
        // mapper exhaustive without creating directories yet.
        ObjectType::BlockSummaryCache => "block-summary-cache-rebuildable",
        ObjectType::RecoveryNote => "recovery-note-inline-only",
    }
}

fn hex_prefix(hex: &str) -> String {
    hex.chars().take(2).collect()
}

pub(crate) fn ref_name_storage_key(ref_name: &str) -> String {
    to_hex(&ref_name_key_bytes(ref_name))
}

/// The raw 32-byte form of [`ref_name_storage_key`] (RFC 102 Stage 4, Step 0 §13.4 / design-v1.md
/// §13.4's ruling): a fixed-width key already used to name every ref pointer/log file today, reused
/// as the ref-pointer-index's own key rather than inventing a second one -- the "new key shape"
/// objection Step 0 raised dissolved specifically because this already existed.
#[must_use]
pub(crate) fn ref_name_key_bytes(ref_name: &str) -> [u8; 32] {
    sha256(ref_name.as_bytes())
}
