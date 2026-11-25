//! What `init` creates, in one list, and the question that list answers: **does this `.prikk` hold only what a killed `init` leaves?**
//!
//! 0.59.0 step K4. `init` used to be a run of `create_empty_file_once` calls; the list below is that run, in the same order, as data. `init` walks
//! it to create, and [`RepositoryLayout::holds_only_a_killed_inits_state`] walks it to ask, so the two cannot drift: a name added to `init` is a
//! name added here, and a test compares the list with what a real `init` leaves on disk.
//!
//! **Why the question exists.** A repository whose `FORMAT` marker is absent or empty is either an `init` that did not finish (nothing stored, so
//! `init` may finish it and write the marker) or a repository that lost its marker (a number nobody can read back: `init` must not label it, and
//! `prikk format upgrade` is the verb that proves it and writes the marker). **Unpopulated** is the first and nothing else: every entry of
//! `.prikk` is one `init` creates, each file is empty or holds exactly the bytes `init` writes there, and there is no other file or directory.
//! Anything else, an unknown name included, is populated.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use prikk_error::Result;

use super::{
    CURRENT_FORMAT_VERSION, ContainerSlot, RECOVERY_LOG_RELATIVE, RepositoryLayout,
    persisted_object_types,
};
use crate::foundation::fsutil::{EntryKind, list_directory, read_file_range_if_exists};

/// The name `FORMAT` is written under before it is renamed into place (`write_file_atomically` appends `.tmp.<pid>.<random>`).
pub(super) const FORMAT_TEMP_PREFIX: &str = "FORMAT.tmp.";

/// The bytes `init` writes into the current-branch pointer.
const CURRENT_BRANCH_AT_INIT: &[u8] = b"heads/main\n";

/// One thing `init` creates, named relative to `.prikk`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InitEntry {
    /// A directory.
    Directory(PathBuf),
    /// A file, created exclusively with these bytes (empty for every file but the current-branch pointer).
    File { path: PathBuf, bytes: &'static [u8] },
}

impl RepositoryLayout {
    /// Everything `init` creates except `FORMAT` (written last, by the caller) and the worktree baseline (recorded by the CLI after `init`), in the
    /// order it is created.
    pub(super) fn init_entries(&self) -> Result<Vec<InitEntry>> {
        let mut entries = Vec::new();
        for dir in self.required_repository_directories()? {
            entries.push(InitEntry::Directory(dir));
        }
        fn empty(
            entries: &mut Vec<InitEntry>,
            layout: &RepositoryLayout,
            path: PathBuf,
        ) -> Result<()> {
            entries.push(InitEntry::File {
                path: layout.repository_relative(&path)?,
                bytes: &[],
            });
            Ok(())
        }
        empty(
            &mut entries,
            self,
            self.worktree_unclean_shutdown_marker_path(),
        )?;
        empty(&mut entries, self, self.provisional_worktree_marker_path())?;
        empty(&mut entries, self, self.default_queue_wal_path())?;
        empty(&mut entries, self, self.default_active_ref_name_path())?;
        // The recovery log and the default session's witness (`ensure_write_state`, which every later writer also calls).
        entries.push(InitEntry::Directory(PathBuf::from("recovery")));
        empty(
            &mut entries,
            self,
            self.prikk_dir().join(RECOVERY_LOG_RELATIVE),
        )?;
        empty(
            &mut entries,
            self,
            self.active_session_dir(super::DEFAULT_ACTIVE_NAME)
                .join("witness"),
        )?;
        empty(&mut entries, self, self.default_declarations_path())?;
        entries.push(InitEntry::File {
            path: self.repository_relative(&self.current_branch_path())?,
            bytes: CURRENT_BRANCH_AT_INIT,
        });
        for object_type in persisted_object_types() {
            empty(
                &mut entries,
                self,
                self.container_slot_path(object_type, ContainerSlot::A),
            )?;
            empty(
                &mut entries,
                self,
                self.container_slot_path(object_type, ContainerSlot::B),
            )?;
        }
        empty(&mut entries, self, self.container_index_path())?;
        empty(&mut entries, self, self.container_generation_log_path())?;
        empty(
            &mut entries,
            self,
            self.ref_log_container_slot_path(ContainerSlot::A),
        )?;
        empty(
            &mut entries,
            self,
            self.ref_log_container_slot_path(ContainerSlot::B),
        )?;
        empty(
            &mut entries,
            self,
            self.ref_pointer_index_slot_path(ContainerSlot::A),
        )?;
        empty(
            &mut entries,
            self,
            self.ref_pointer_index_slot_path(ContainerSlot::B),
        )?;
        empty(
            &mut entries,
            self,
            self.ref_pointer_index_generation_log_path(),
        )?;
        empty(
            &mut entries,
            self,
            self.received_index_slot_path(ContainerSlot::A),
        )?;
        empty(
            &mut entries,
            self,
            self.received_index_slot_path(ContainerSlot::B),
        )?;
        empty(
            &mut entries,
            self,
            self.received_index_generation_log_path(),
        )?;
        empty(&mut entries, self, self.trust_key_container_path())?;
        empty(&mut entries, self, self.author_key_container_path())?;
        empty(
            &mut entries,
            self,
            self.trust_policy_container_slot_path(ContainerSlot::A),
        )?;
        empty(
            &mut entries,
            self,
            self.trust_policy_container_slot_path(ContainerSlot::B),
        )?;
        empty(&mut entries, self, self.trust_policy_generation_log_path())?;
        Ok(entries)
    }

    /// `true` when `.prikk` holds only what a killed `init` leaves (see the module doc). Reads no file whole: a populated container is read for one
    /// byte past what `init` writes there, and the walk stops at the first entry that is not `init`'s.
    pub(crate) fn holds_only_a_killed_inits_state(&self) -> Result<bool> {
        self.holds_only_inits_entries(false)
    }

    /// `true` when `.prikk` holds nothing yet but what `init` creates **and the worktree baseline `prikk init` records after it** (0.59.0 K6): a repository
    /// no one has adopted a key in, committed to, or sealed. What `prikk setup` may finish rather than refuse.
    pub(crate) fn holds_nothing_yet_checked(&self) -> Result<bool> {
        self.holds_only_inits_entries(true)
    }

    fn holds_only_inits_entries(&self, with_baseline: bool) -> Result<bool> {
        let mut expected: BTreeMap<PathBuf, InitEntry> = BTreeMap::new();
        for entry in self.init_entries()? {
            let path = match &entry {
                InitEntry::Directory(path) | InitEntry::File { path, .. } => path.clone(),
            };
            expected.insert(path, entry);
        }
        self.directory_holds_only_inits_entries(Path::new(""), &expected, with_baseline)
    }

    fn directory_holds_only_inits_entries(
        &self,
        relative: &Path,
        expected: &BTreeMap<PathBuf, InitEntry>,
        with_baseline: bool,
    ) -> Result<bool> {
        let root = self.repository_mutation_root();
        for entry in list_directory(root, relative)? {
            let name = entry.name.to_string_lossy().into_owned();
            let child = relative.join(&entry.name);
            if relative.as_os_str().is_empty() {
                // The marker itself is the caller's question. Its temporary is `init`'s own, holding at most a prefix of what it writes.
                if name == "FORMAT" && entry.kind == EntryKind::Regular {
                    continue;
                }
                if with_baseline && name == "worktree.baseline" && entry.kind == EntryKind::Regular
                {
                    continue;
                }
                if name.starts_with(FORMAT_TEMP_PREFIX) && entry.kind == EntryKind::Regular {
                    let held = read_file_range_if_exists(
                        root,
                        &child,
                        0,
                        CURRENT_FORMAT_VERSION.len() + 1,
                    )?
                    .unwrap_or_default();
                    if CURRENT_FORMAT_VERSION.starts_with(&held) {
                        continue;
                    }
                    return Ok(false);
                }
            }
            match (expected.get(&child), entry.kind) {
                (Some(InitEntry::Directory(_)), EntryKind::Directory) => {
                    if !self.directory_holds_only_inits_entries(&child, expected, with_baseline)? {
                        return Ok(false);
                    }
                }
                (Some(InitEntry::File { bytes, .. }), EntryKind::Regular) => {
                    let held = read_file_range_if_exists(root, &child, 0, bytes.len() + 1)?
                        .unwrap_or_default();
                    if !(held.is_empty() || held.as_slice() == *bytes) {
                        return Ok(false);
                    }
                }
                _ => return Ok(false),
            }
        }
        Ok(true)
    }
}
