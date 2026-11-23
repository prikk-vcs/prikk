//! 0.59.0 step G2d: `prikk doctor --remove-ref-candidates [--plan-only]` -- remove the files left in `refs/tmp`, saving each first.
//!
//! **What a candidate is.** Until RFC 102 Stage 4 (2026-08) a publication wrote the pointer's next value to `refs/tmp/<key>.tmp` and promoted it onto
//! the per-ref pointer file; a crash left the candidate. Nothing writes one now (`design-v1.md` §13.3: *"`refs/tmp/` stops being written,
//! unconditionally"*), `verify` calls one *non-authoritative*, and no code reads its bytes. The write-path precondition still refused on one because
//! RFC 165 R2 kept the check for a test-suite tie. The writers no longer refuse (`refs::verify::candidate_issues` callers); this verb is how the
//! warning `PRIKK-VERIFY-REF-CANDIDATE-DEBRIS` is cleared.
//!
//! **A repair verb is a writer, so it takes what a writer takes:** the active lock, then the `RefLock` of every ref a decodable candidate names, in
//! sorted order, refusing at a running holder with the in-flight sentence (it never waits). Under those locks it **reads again** (the plan it acted on
//! is the one it prints), **saves each file's bytes durably in the recovery log** (`Kind::Remove`, framed as a version-2 entry so a prikk that does not
//! know it counts it as written by a newer prikk and lists and restores every other entry as before), **then removes the file**, then syncs the
//! directory (`remove_file_if_present_required` does). A crash between the save and the removal leaves the file and a saved copy; a run again saves
//! again and removes. `--recovery-restore <run>` recreates a removed file, only where nothing is.
//!
//! **Every regular file in `refs/tmp` is removed**, decodable as a legacy pointer candidate or not: nothing else lives there. **An empty directory** is
//! removed too (0.59.0 step G2e; no bytes, so nothing is saved). **Any other entry** (a directory that holds something, a symlink) refuses before
//! anything is touched, and says no prikk command removes it: prikk never wrote it.

use std::collections::BTreeSet;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::fsutil::{
    EntryKind, list_directory_tolerating_absence, read_file_if_exists,
    remove_empty_directory_if_present_required, remove_file_if_present_required,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::{ActiveLock, RefLock};

/// What a file in `refs/tmp` is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefCandidateKind {
    /// A legacy ref pointer candidate (`PREFPTR1`, the ref name, the RefState id).
    LegacyPointer {
        /// The ref it named.
        ref_name: String,
        /// The RefState it named.
        ref_state_id: ObjectId,
    },
    /// A regular file that is not a ref pointer candidate (nothing that prikk writes).
    NotACandidate,
    /// A directory that holds nothing (0.59.0 step G2e): removed with no bytes saved, since there are none.
    EmptyDirectory,
}

/// What the verb did, or would do, to one file.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum RefCandidateAction {
    /// `--plan-only`: the file would be saved and removed.
    WouldRemove,
    /// The file was saved in the recovery log and removed.
    Removed,
    /// The file was gone when the run came to it (it vanished between the read and the removal): nothing to do.
    AlreadyAbsent,
}

/// One file of `refs/tmp`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RefCandidateOutcome {
    /// The file's name in `refs/tmp`.
    pub name: String,
    /// What it is.
    pub kind: RefCandidateKind,
    /// How many bytes it holds.
    pub bytes: u64,
    /// What was, or would be, done.
    pub action: RefCandidateAction,
    /// The recovery log entry that holds its bytes (`None` for a plan, and for a file already absent).
    pub recovery: Option<crate::recovery_log::RecoveryRef>,
}

/// The report of `--remove-ref-candidates` (and of its `--plan-only`): one row per file, sorted by name; empty when `refs/tmp` holds none.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct RemoveRefCandidatesReport {
    /// One outcome per file.
    pub candidates: Vec<RefCandidateOutcome>,
}

/// `prikk doctor --remove-ref-candidates`: save, then remove, every file in `refs/tmp`, and remove an empty directory there.
///
/// # Errors
///
/// An entry in `refs/tmp` that is neither a regular file nor an empty directory (no prikk command removes it: prikk never wrote it); another command publishing a ref a candidate names (or holding the active lock); a file that cannot be read,
/// saved or removed.
pub fn remove_ref_candidates(layout: &RepositoryLayout) -> Result<RemoveRefCandidatesReport> {
    run(layout, false)
}

/// `--remove-ref-candidates --plan-only`: the same read, the same locks, the same refusals, nothing written.
///
/// # Errors
///
/// As [`remove_ref_candidates`].
pub fn plan_remove_ref_candidates(layout: &RepositoryLayout) -> Result<RemoveRefCandidatesReport> {
    run(layout, true)
}

struct Found {
    name: String,
    bytes: Vec<u8>,
    kind: RefCandidateKind,
}

fn read_candidates(layout: &RepositoryLayout) -> Result<Vec<Found>> {
    let dir = layout.refs_dir().join("tmp");
    let relative = layout.repository_relative(&dir)?;
    let root = layout.repository_mutation_root();
    let Some(entries) =
        list_directory_tolerating_absence(root, &dir, &relative, "the ref candidate directory")?
    else {
        return Ok(Vec::new());
    };
    // Every entry that is neither a file nor an empty directory refuses first, before anything is read for removal.
    let non_files = crate::refs::ref_tmp_non_files(layout)?;
    if let Some((name, _)) = non_files
        .iter()
        .find(|(_, kind)| *kind == crate::refs::RefTmpEntryKind::NotPrikks)
    {
        return Err(PrikkError::Integrity(format!(
            "unexpected non-file in ref candidate directory: refs/tmp/{name}; no prikk command removes it: prikk never wrote it, and nothing in prikk \
             reads it. Look at what it holds, then remove it yourself; this command touches nothing in refs/tmp while it is there"
        )));
    }
    let mut found = Vec::new();
    for entry in entries {
        let name = entry.name.to_string_lossy().into_owned();
        if entry.kind == EntryKind::Directory {
            found.push(Found {
                name,
                bytes: Vec::new(),
                kind: RefCandidateKind::EmptyDirectory,
            });
            continue;
        }
        let Some(bytes) = read_file_if_exists(root, &relative.join(&entry.name))? else {
            continue;
        };
        let kind = match crate::refs::decode_legacy_pointer_candidate(&bytes) {
            Some((ref_name, ref_state_id)) => RefCandidateKind::LegacyPointer {
                ref_name,
                ref_state_id,
            },
            None => RefCandidateKind::NotACandidate,
        };
        found.push(Found { name, bytes, kind });
    }
    Ok(found)
}

fn run(layout: &RepositoryLayout, plan_only: bool) -> Result<RemoveRefCandidatesReport> {
    layout.require_current_format()?;
    let _run = crate::recovery_log::begin_run();
    // Another command's publication is never waited for: a running holder of the active lock or of a named ref's lock is the in-flight sentence.
    if let Some(pid) = crate::refs::in_flight::active_publisher(layout)? {
        return Err(PrikkError::Precondition(format!(
            "{}; run this command again when it finishes",
            crate::refs::in_flight::in_flight_sentence("the active queue", Some(pid))
        )));
    }
    let _active_lock =
        ActiveLock::acquire_for_write_allowing_import_journal(layout, DEFAULT_ACTIVE_NAME)?;
    // The first read names the refs whose locks are taken; the read under the locks is the one acted on.
    let named: BTreeSet<String> = read_candidates(layout)?
        .into_iter()
        .filter_map(|found| match found.kind {
            RefCandidateKind::LegacyPointer { ref_name, .. } => Some(ref_name),
            RefCandidateKind::NotACandidate | RefCandidateKind::EmptyDirectory => None,
        })
        .collect();
    let mut ref_locks = Vec::new();
    for ref_name in &named {
        if crate::refs::in_flight::ref_publisher_running(layout, ref_name)? {
            return Err(PrikkError::Precondition(format!(
                "{}; run this command again when it finishes",
                crate::refs::in_flight::in_flight_sentence(ref_name, None)
            )));
        }
        ref_locks.push(RefLock::acquire(layout, ref_name)?);
    }
    let found = read_candidates(layout)?;
    let root = layout.repository_mutation_root();
    let mut candidates = Vec::with_capacity(found.len());
    for file in found {
        let source = format!("refs/tmp/{}", file.name);
        let bytes = file.bytes.len() as u64;
        if plan_only {
            candidates.push(RefCandidateOutcome {
                name: file.name,
                kind: file.kind,
                bytes,
                action: RefCandidateAction::WouldRemove,
                recovery: None,
            });
            continue;
        }
        if file.kind == RefCandidateKind::EmptyDirectory {
            // No bytes, so nothing to save. `rmdir` refuses a directory that filled since it was read, and that refusal is the run's.
            #[cfg(test)]
            crate::refs::view_hooks::run("remove-empty-directory-before");
            let removed = remove_empty_directory_if_present_required(root, Path::new(&source))?;
            candidates.push(RefCandidateOutcome {
                name: file.name,
                kind: file.kind,
                bytes,
                action: if removed {
                    RefCandidateAction::Removed
                } else {
                    RefCandidateAction::AlreadyAbsent
                },
                recovery: None,
            });
            continue;
        }
        let recovery = crate::recovery_log::save_remove(
            layout,
            &source,
            &file.bytes,
            "remove-ref-candidates",
        )?;
        #[cfg(test)]
        crate::refs::view_hooks::run("remove-candidate-after-save");
        let removed = remove_file_if_present_required(root, Path::new(&source))?;
        candidates.push(RefCandidateOutcome {
            name: file.name,
            kind: file.kind,
            bytes,
            action: if removed {
                RefCandidateAction::Removed
            } else {
                RefCandidateAction::AlreadyAbsent
            },
            recovery: Some(recovery),
        });
    }
    drop(ref_locks);
    Ok(RemoveRefCandidatesReport { candidates })
}

#[cfg(test)]
mod tests;
