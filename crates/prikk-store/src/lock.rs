//! Simple file locks for active-session and ref writers.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{
    MutationRoot, create_new_file_required, remove_file_cleanup_best_effort,
};
use crate::foundation::layout::{LockableContainer, RepositoryLayout};

/// Active session lock acquired before mutating an active WAL tail.
#[derive(Debug)]
pub struct ActiveLock {
    path: PathBuf,
    relative: PathBuf,
    mutation_root: MutationRoot,
}

impl ActiveLock {
    /// Acquire a lock through exclusive file creation, for the active session named `name` (RFC
    /// 108 increment 1: previously hardcoded to `"default"`; generalizes the same way
    /// `RefLock::acquire` below already takes `ref_name`).
    ///
    /// RFC 108 increment 3a: `name` widened from `&str` to `impl AsRef<Path>`, matching
    /// `wal.rs::Wal::for_layout`. No other change needed here -- `relative` was already **derived**
    /// from `path` via `repository_relative`, never reconstructed from `name` the way `Wal` used to.
    ///
    /// RFC 155 Part I-A2: refuses while an archive import's journal is present, for **every**
    /// session name, not only the default one (corrected from Part I-A's own first cut, which
    /// checked `is_default` here -- review `rfc155-impl-IA-review-v1`, finding IA1(b): RFC 108's
    /// other sessions can append objects too, and a cancel would otherwise cut their work out from
    /// under them with no refusal having warned them off first). Confirmed from source that nothing
    /// relies on the old, narrower behavior: every production writer that takes this lock --
    /// `commit`, `seal`, `trust maintainer add`, `bundle import`, and the rest -- passes
    /// `DEFAULT_ACTIVE_NAME` unconditionally; the only non-default callers anywhere in the
    /// workspace are RFC 108's own multi-session tests, none of which runs while an import journal
    /// is present. This is the structural half of §9.5's rule ("every repository writer refuses"):
    /// every caller of this function, present and future, is covered with no per-call-site
    /// enumeration to remember, because this is -- by this module's own established doc note on
    /// `ensure_write_state` -- "the one call site every writer passes through."
    /// `Self::acquire_allowing_import_journal` is the named, narrow exception, used only by the
    /// import itself (so a resume can re-acquire this lock while its own journal is present), the
    /// cancel, and doctor's own recovery verbs.
    pub fn acquire(layout: &RepositoryLayout, name: impl AsRef<Path>) -> Result<Self> {
        Self::acquire_checked(layout, name, true)
    }

    /// As [`Self::acquire`], but never refuses for an archive import's own journal -- the named
    /// exception [`Self::acquire`]'s own doc comment describes. Used only by: `archive::import`'s
    /// own lock acquisition (a resume must be able to re-acquire this lock while its own journal is
    /// still present), `archive::import::cancel_import`, and doctor's recovery verbs
    /// (`repair_tails`/`plan_repair_tails`, `discard_damaged_commits`, `restore_queue_target`,
    /// `repair_repository`'s per-active loop, and `recovery_restore`/`recovery_clear` via
    /// `recovery_log::repair_locks`) -- enumerated here, from source, the same way RFC 163
    /// enumerated its own write-side tail-guard call sites, so this doc comment's own list is a
    /// checkable claim, not a label.
    pub(crate) fn acquire_allowing_import_journal(
        layout: &RepositoryLayout,
        name: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::acquire_checked(layout, name, false)
    }

    fn acquire_checked(
        layout: &RepositoryLayout,
        name: impl AsRef<Path>,
        check_import_journal: bool,
    ) -> Result<Self> {
        if check_import_journal {
            crate::foundation::import_journal_marker::refuse_while_present(layout)?;
        }
        let path = layout.active_lock_path(name);
        let relative = layout.repository_relative(&path)?;
        let mutation_root = layout.repository_mutation_root().clone();
        acquire_lock_file(&mutation_root, &relative, &path, "active")?;
        Ok(Self {
            path,
            relative,
            mutation_root,
        })
    }

    /// The lock a writer takes. RFC 168 §3.1, §3.3: a repository created before the recovery log and the witness gets them here,
    /// at its first write. A plan-only path takes [`Self::acquire`] instead and creates nothing (RFC 168 F6). Created after the lock
    /// is held and returned as the guard, so a failure here releases the lock on drop.
    pub fn acquire_for_write(layout: &RepositoryLayout, name: impl AsRef<Path>) -> Result<Self> {
        let is_default = name.as_ref() == Path::new(crate::foundation::layout::DEFAULT_ACTIVE_NAME);
        let lock = Self::acquire(layout, name)?;
        if is_default {
            layout.ensure_write_state()?;
        }
        Ok(lock)
    }

    /// As [`Self::acquire_for_write`], built on [`Self::acquire_allowing_import_journal`] instead of
    /// [`Self::acquire`] -- doctor's own recovery verbs that also need `ensure_write_state` run
    /// (`repair_tails`, `repair_repository`'s per-active loop) use this one.
    pub(crate) fn acquire_for_write_allowing_import_journal(
        layout: &RepositoryLayout,
        name: impl AsRef<Path>,
    ) -> Result<Self> {
        let is_default = name.as_ref() == Path::new(crate::foundation::layout::DEFAULT_ACTIVE_NAME);
        let lock = Self::acquire_allowing_import_journal(layout, name)?;
        if is_default {
            layout.ensure_write_state()?;
        }
        Ok(lock)
    }

    /// Return lock file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn require_layout(&self, layout: &RepositoryLayout) -> Result<()> {
        if self
            .mutation_root
            .same_authority(layout.repository_mutation_root())
        {
            return Ok(());
        }
        Err(PrikkError::LockConflict(
            "active lock belongs to a different repository authority".to_string(),
        ))
    }
}

impl Drop for ActiveLock {
    fn drop(&mut self) {
        remove_file_cleanup_best_effort(&self.mutation_root, &self.relative);
    }
}

/// Ref-specific lock acquired before publishing one ref pointer.
#[derive(Debug)]
pub struct RefLock {
    path: PathBuf,
    relative: PathBuf,
    mutation_root: MutationRoot,
}

impl RefLock {
    /// Acquire a ref lock through exclusive file creation.
    pub fn acquire(layout: &RepositoryLayout, ref_name: &str) -> Result<Self> {
        let path = layout.ref_lock_path(ref_name);
        let relative = layout.repository_relative(&path)?;
        let mutation_root = layout.repository_mutation_root().clone();
        acquire_lock_file(&mutation_root, &relative, &path, "ref")?;
        Ok(Self {
            path,
            relative,
            mutation_root,
        })
    }

    /// Return lock file path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for RefLock {
    fn drop(&mut self) {
        remove_file_cleanup_best_effort(&self.mutation_root, &self.relative);
    }
}

/// One held lock on a `LockableContainer`. Never constructed directly outside
/// `acquire_container_locks` -- the sorted-order guarantee that helper provides is only real if
/// nothing can acquire a container lock any other way.
#[derive(Debug)]
struct ContainerLockHandle {
    /// RFC 155 Part I-A2: which container this handle actually holds -- read back by
    /// [`ContainerLockGuard::holds`], so a caller that needs to *prove* a lock is held (rather
    /// than simply trusting its own bookkeeping) has something to check against.
    kind: LockableContainer,
    relative: PathBuf,
    mutation_root: MutationRoot,
}

impl Drop for ContainerLockHandle {
    fn drop(&mut self) {
        remove_file_cleanup_best_effort(&self.mutation_root, &self.relative);
    }
}

/// RAII guard for one or more container locks, acquired together by `acquire_container_locks` and
/// released when dropped. Held for its `Drop` effect, not read from -- the same shape
/// `ActiveLock`/`RefLock` already use.
#[derive(Debug)]
pub struct ContainerLockGuard {
    handles: Vec<ContainerLockHandle>,
}

impl ContainerLockGuard {
    /// Whether this guard actually holds `container`'s own lock -- RFC 155 Part I-A2's own need:
    /// `object_store`'s under-held-lock append path (import's stage 1, which holds `ObjectStore`
    /// for its whole run rather than once per append) checks this before appending, rather than
    /// trusting a caller's claim that it already holds the lock it is about to rely on.
    pub(crate) fn holds(&self, container: LockableContainer) -> bool {
        self.handles.iter().any(|handle| handle.kind == container)
    }
}

/// Acquire every lock in `containers`, sorted into `LockableContainer`'s fixed `Ord` before any file
/// is created (design-v1.md §15.7's deadlock ruling: a single acquisition helper that sorts the
/// caller's requested set, not per-call-site ordering discipline) -- so two call sites that each
/// request `{RefPointerIndex, RefLog}` always acquire them in the same order regardless of which order
/// their own arguments list them in.
///
/// If any acquisition in the sorted sequence fails -- most commonly `LockConflict`, a concurrent
/// writer or the compactor already holding a later container in the order -- every lock already
/// acquired during this call is released before the error returns: `handles` (built incrementally,
/// `?`-propagating on failure) simply drops here, and each already-acquired `ContainerLockHandle`'s
/// own `Drop` releases it. A partial, leaked hold on early-failure is exactly the wedge shape this
/// stage's stale-lock recovery work exists to stop introducing more of, so this path must never leave
/// one behind.
///
/// RFC 155 Part I-B2: refuses while an archive import's journal is present, mirroring
/// [`ActiveLock::acquire`]'s own check (review `rfc155-impl-IB-review-v1`, finding IB1(b)) --
/// `ActiveLock::acquire` alone was not "the one call site every writer passes through" after all:
/// `compact --pointer-index`/`--received-index`/`--trust-policy`, `doctor --rebuild-pointer-index`,
/// and `doctor --repair-index`/`--repair-pointer-index-tail` each take only a container lock, with
/// no `ActiveLock` at all, so none of them passed through Part I-A2's own fix. [`acquire_container_locks`]
/// is the one function every one of those, and every ordinary writer that also holds `ActiveLock`
/// first, passes through -- so this is now the actual "one call site," not `ActiveLock::acquire`
/// alone. `acquire_container_locks_allowing_import_journal` is the named, narrow exception, used
/// only by the import itself, the cancel, and doctor's own recovery verbs (`repair_tails`/
/// `plan_repair_tails`, `recovery_log::repair_locks` backing `--recovery-restore`/`--recovery-clear`)
/// -- enumerated here, from source, the same way `ActiveLock::acquire_allowing_import_journal`'s own
/// doc comment does for its list. `doctor --repair-index`/`--repair-pointer-index-tail` are
/// deliberately **not** exempted: unlike `repair_tails`/`--recovery-restore`/`--recovery-clear`,
/// neither is part of the import's own resume/cancel flow (the interrupted-import issue only ever
/// names resume or `--cancel-import`), and rebuilding the object index or truncating a pointer-index
/// tail mid-import is exactly the "another writer mutates a journaled container while the journal
/// still names its old length" hazard this part exists to close -- the same reasoning that keeps
/// `compact`/`--rebuild-pointer-index` refusing too.
pub fn acquire_container_locks(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
) -> Result<ContainerLockGuard> {
    acquire_container_locks_checked(layout, containers, true)
}

/// As [`acquire_container_locks`], but never refuses for an archive import's own journal -- the
/// named exception that function's own doc comment describes. Used only by: `archive::import`'s own
/// lock acquisition (a resume must be able to re-acquire these locks while its own journal is still
/// present), `archive::import::cancel_import`, `doctor::repair_tails`/`plan_repair_tails`, and
/// `recovery_log::repair_locks` (backing `--recovery-restore`/`--recovery-clear`).
pub(crate) fn acquire_container_locks_allowing_import_journal(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
) -> Result<ContainerLockGuard> {
    acquire_container_locks_checked(layout, containers, false)
}

fn acquire_container_locks_checked(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
    check_import_journal: bool,
) -> Result<ContainerLockGuard> {
    if check_import_journal {
        crate::foundation::import_journal_marker::refuse_while_present(layout)?;
    }
    let mut sorted = containers.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    let mutation_root = layout.repository_mutation_root().clone();
    let mut handles = Vec::with_capacity(sorted.len());
    for container in sorted {
        let path = layout.lockable_container_lock_path(container);
        let relative = layout.repository_relative(&path)?;
        acquire_lock_file(
            &mutation_root,
            &relative,
            &path,
            container_lock_kind(container),
        )?;
        handles.push(ContainerLockHandle {
            kind: container,
            relative,
            mutation_root: mutation_root.clone(),
        });
    }
    Ok(ContainerLockGuard { handles })
}

fn container_lock_kind(container: LockableContainer) -> &'static str {
    match container {
        LockableContainer::RefPointerIndex => "container:ref-pointer-index",
        LockableContainer::RefLog => "container:ref-log",
        LockableContainer::ReceivedIndex => "container:received-index",
        LockableContainer::TrustPolicy => "container:trust-policy",
        LockableContainer::ObjectStore => "container:object-store",
    }
}

fn acquire_lock_file(
    mutation_root: &MutationRoot,
    relative: &Path,
    path: &Path,
    kind: &str,
) -> Result<()> {
    let body = lock_body(kind);
    match create_new_file_required(mutation_root, relative, body.as_bytes()) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => Err(
            PrikkError::LockConflict(format!("{kind} lock already exists: {}", path.display())),
        ),
        Err(err) => Err(err.into()),
    }
}

fn lock_body(kind: &str) -> String {
    format!(
        "pid={}\nkind={kind}\nnote=PR-007 lock has no stale-lock stealing yet\n",
        std::process::id()
    )
}

// Repository mutation is implemented on Linux, macOS and Windows (`platform-support.md`); the
// tests that need a Linux-only or directory-sync mechanism carry their own narrower gate.
#[cfg(test)]
mod tests;
