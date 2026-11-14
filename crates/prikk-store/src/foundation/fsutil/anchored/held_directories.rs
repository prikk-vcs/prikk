//! Directories a writer keeps open for as long as it holds a lock (0.58.0 P2a).
//!
//! **What this is.** A table on a [`super::MutationRoot`], `relative directory -> retained handle`, filled and read only by the
//! walk (`directory::prepare_directory_required`, `directory::open_existing_directory_required`). The walk used to open every
//! component from the root, and sync its parent, for **every** write: 5 syncs per object under a held lock, 3 of them the walk's.
//! With a table, a directory is opened once, and its parent synced once, at its first use under a hold; every later write under
//! the hold uses the held handle.
//!
//! **A hold** lasts from a command's first lock to its last release. Every lock guard registers one ([`DirectoryHold`], returned by
//! `lock::acquire_lock_file`, so a lock cannot be taken without one), holds are counted, and the table is cleared when the count
//! reaches 0. With no hold the table is absent: `get` finds nothing and `remember` stores nothing, so a write outside a lock walks
//! exactly as before.
//!
//! **Why dropping the repeated syncs keeps the promise.** The first open under the hold synced the directory's parent, so the
//! directory's entry was durable before any write relied on it; a directory the hold's own walk created kept its created-parent
//! sync. Nothing under a hold removes or renames a directory below `.prikk` (audited, P2a report), so nothing changes that entry
//! afterwards. A handle is stored only after its parent sync succeeded.
//!
//! **Not here:** Windows. `WindowsAuthority` retains no handle between steps and has no directory sync, so a hold there is a
//! zero-sized token and there is no table.

use std::sync::Arc;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::collections::HashMap;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::path::{Path, PathBuf};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::{Mutex, MutexGuard, PoisonError};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use prikk_error::Result;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::directory::AnchoredDirectory;

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Default)]
struct State {
    holds: usize,
    directories: HashMap<PathBuf, AnchoredDirectory>,
}

/// The table, shared by every clone of one `MutationRoot`. One `Mutex`: two threads writing under one root share one table.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[derive(Default)]
pub(super) struct HeldDirectories {
    state: Mutex<State>,
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) struct HeldDirectories;

impl std::fmt::Debug for HeldDirectories {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("HeldDirectories")
            .finish_non_exhaustive()
    }
}

/// One registered hold. Dropping it releases the hold; the last release clears the table (closing its handles).
#[derive(Debug)]
pub(crate) struct DirectoryHold {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    held: Arc<HeldDirectories>,
}

impl HeldDirectories {
    pub(super) fn new() -> Arc<Self> {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            Arc::new(Self::default())
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            Arc::new(Self)
        }
    }

    /// Register a hold.
    pub(super) fn hold(self: &Arc<Self>) -> DirectoryHold {
        #[cfg(any(target_os = "linux", target_os = "macos"))]
        {
            self.lock().holds += 1;
            DirectoryHold {
                held: Arc::clone(self),
            }
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            DirectoryHold {}
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl HeldDirectories {
    /// A poisoned table is still a consistent map (every operation under the lock is one insert, one remove or one clear).
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A duplicate of the held handle for `key`, when a hold is open and `key` has been opened under it.
    pub(super) fn get(&self, key: &Path) -> Result<Option<AnchoredDirectory>> {
        let state = self.lock();
        if state.holds == 0 {
            return Ok(None);
        }
        state
            .directories
            .get(key)
            .map(AnchoredDirectory::duplicate)
            .transpose()
    }

    /// Keep a duplicate of `directory` as `key`'s held handle, when a hold is open. Called only after `directory`'s parent sync
    /// succeeded.
    pub(super) fn remember(&self, key: PathBuf, directory: &AnchoredDirectory) -> Result<()> {
        let mut state = self.lock();
        if state.holds > 0 {
            state.directories.insert(key, directory.duplicate()?);
        }
        Ok(())
    }

    /// Holds open and directories held, for the tests that assert the table is absent outside a hold.
    #[cfg(test)]
    pub(super) fn counts_for_test(&self) -> (usize, usize) {
        let state = self.lock();
        (state.holds, state.directories.len())
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
impl Drop for DirectoryHold {
    fn drop(&mut self) {
        let mut state = self.held.lock();
        state.holds = state.holds.saturating_sub(1);
        if state.holds == 0 {
            state.directories.clear();
        }
    }
}
