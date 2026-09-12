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
    pub fn acquire(layout: &RepositoryLayout, name: impl AsRef<Path>) -> Result<Self> {
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
    _handles: Vec<ContainerLockHandle>,
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
pub fn acquire_container_locks(
    layout: &RepositoryLayout,
    containers: &[LockableContainer],
) -> Result<ContainerLockGuard> {
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
            relative,
            mutation_root: mutation_root.clone(),
        });
    }
    Ok(ContainerLockGuard { _handles: handles })
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

// DC-71: every test here sets up its scenario via real repository mutation (RepositoryLayout::init
// or equivalent), which is Linux-only; the module never compiles a non-Linux-meaningful test.
#[cfg(all(test, target_os = "linux"))]
mod tests;
