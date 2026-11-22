//! 0.59.0 step W2: **a switch never changes, on disk, a path the target's queue holds.**
//!
//! `switch_branch` writes the target's *sealed* files. The branch's own queued commits are not in them, and since 0.56.0 step
//! W2f the files the switch may overwrite or delete are judged against the worktree's **record**, which `commit --ref <target>`
//! writes with the queue folded on top. So a queued new file, edit, deletion or rename read as "the branch being left's own"
//! and the switch put the sealed bytes over them (0.55.0 refused the edit, deletion and rename, and kept the new file).
//!
//! The refusal, before any write: when the active queue is owned by the target (refusal 2 has already refused one owned by
//! another ref), the paths the queue holds, intersected with the paths the plan would write over, create, delete or give a new
//! mode, refuse. An empty intersection proceeds, so a queue that changes nothing the switch touches (a file created and deleted
//! again, say) switches as before.

use std::collections::BTreeSet;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::fsutil::stat_file_state_if_exists;
use crate::foundation::layout::RepositoryLayout;
use crate::patch_replay::ReplayManifestEntry;
use crate::wal::WalReplay;
use crate::worktree_status::{QueuedPatchEntry, QueuedPathResolution, enumerate_queued_patches};

/// How many paths the sentence names before `and N more`.
const PATHS_SHOWN: usize = 5;

/// What the plan would do to the disk: the paths it writes (over or new) or deletes, and the manifest entries it leaves with
/// their bytes, which a mode change can still touch.
pub(super) struct PlannedChanges<'a> {
    pub(super) written: Vec<&'a str>,
    pub(super) deleted: Vec<&'a str>,
    pub(super) unchanged: &'a [&'a ReplayManifestEntry],
}

/// Every path a queued operation names (a rename's old and new), and whether any operation's path could not be named.
fn held_paths(queue: &[QueuedPatchEntry]) -> (BTreeSet<&str>, bool) {
    let mut held = BTreeSet::new();
    let mut unnameable = false;
    for operation in queue.iter().flat_map(|patch| &patch.operations) {
        for path in &operation.paths {
            match path {
                QueuedPathResolution::Path(path) => {
                    held.insert(path.as_str());
                }
                QueuedPathResolution::Unresolved { .. } => unnameable = true,
            }
        }
    }
    (held, unnameable)
}

/// The paths the switch would change that the queue holds: an operation whose path cannot be named holds every one of them.
fn changed_and_held<'a>(
    changed: &BTreeSet<&'a str>,
    held: &BTreeSet<&str>,
    unnameable: bool,
) -> Vec<&'a str> {
    if unnameable {
        changed.iter().copied().collect()
    } else {
        changed
            .iter()
            .copied()
            .filter(|path| held.contains(path))
            .collect()
    }
}

/// Refuse, before any write, a switch to `target` that would change a path `target`'s queue holds.
pub(super) fn refuse_when_the_queue_holds_a_changed_path(
    layout: &RepositoryLayout,
    target: &str,
    active_replay: &WalReplay,
    plan: &PlannedChanges<'_>,
) -> Result<()> {
    let Some(newest) = active_replay.records.last() else {
        return Ok(());
    };
    let mut changed: BTreeSet<&str> = plan.written.iter().chain(&plan.deleted).copied().collect();
    let queue = enumerate_queued_patches(layout)?;
    let (held, unnameable) = held_paths(&queue);
    // A mode the switch would reset on a file it otherwise leaves is a change too; only a path the queue holds is looked at.
    let root = layout.worktree_mutation_root();
    for entry in plan.unchanged {
        let path = entry.path.as_str();
        if !unnameable && !held.contains(path) {
            continue;
        }
        let mode = stat_file_state_if_exists(root, Path::new(path))?
            .and_then(|stat| stat.mode)
            .map(|mode| mode & 0o7777);
        if mode != Some(entry.mode & 0o7777) {
            changed.insert(path);
        }
    }
    let hit = changed_and_held(&changed, &held, unnameable);
    if hit.is_empty() {
        return Ok(());
    }
    let newest_id: ObjectId = newest.envelope.object_id();
    let count = active_replay.records.len();
    // While a switch or checkout is marked interrupted, `uncommit` refuses (the worktree is not verified), so it is not named.
    let marked = crate::worktree_marker::worktree_is_dirty(layout)?;
    Err(PrikkError::Precondition(refusal_sentence(
        target, count, newest_id, &hit, unnameable, marked,
    )))
}

fn refusal_sentence(
    target: &str,
    count: usize,
    newest: ObjectId,
    hit: &[&str],
    unnameable: bool,
    marked: bool,
) -> String {
    let shown = hit
        .iter()
        .take(PATHS_SHOWN)
        .copied()
        .collect::<Vec<_>>()
        .join(", ");
    let more = match hit.len().saturating_sub(PATHS_SHOWN) {
        0 => String::new(),
        n => format!(" and {n} more"),
    };
    let lead = if unnameable {
        format!(
            "its queue holds {count} commit(s) with changes to paths it cannot name, and the switch would overwrite or \
             delete {shown}{more} on disk"
        )
    } else {
        format!(
            "its queue holds {count} commit(s) that change {shown}{more}, which the switch would overwrite or delete on disk"
        )
    };
    let ways_out = if marked {
        format!(
            "`prikk seal --allow-no-audit --ref {target}` publishes them and keeps your files; run the switch again after it"
        )
    } else {
        format!(
            "`prikk seal --allow-no-audit --ref {target}` publishes them and keeps your files, \
             or `prikk uncommit {newest}` takes the newest commit back and leaves your files"
        )
    };
    format!("refusing to switch to {target}: {lead} (nothing was written); {ways_out}")
}

#[cfg(test)]
pub(super) fn refusal_sentence_for_test(
    target: &str,
    count: usize,
    newest: ObjectId,
    hit: &[&str],
    unnameable: bool,
    marked: bool,
) -> String {
    refusal_sentence(target, count, newest, hit, unnameable, marked)
}

#[cfg(test)]
#[allow(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::worktree_status::QueuedOperationContent;
    use crate::worktree_status::QueuedOperationEntry;

    fn patch(operations: Vec<(&'static str, Vec<QueuedPathResolution>)>) -> QueuedPatchEntry {
        QueuedPatchEntry {
            patch_id: ObjectId::from_bytes([1; 32]),
            message: None,
            operations: operations
                .into_iter()
                .map(|(kind, paths)| QueuedOperationEntry {
                    kind,
                    paths,
                    content: QueuedOperationContent::Other,
                })
                .collect(),
        }
    }

    fn path(text: &str) -> QueuedPathResolution {
        QueuedPathResolution::Path(text.to_string())
    }

    #[test]
    fn a_rename_holds_both_its_paths() {
        let queue = [patch(vec![("rename-path", vec![path("a"), path("b")])])];
        let (held, unnameable) = held_paths(&queue);
        assert_eq!(
            (held.into_iter().collect::<Vec<_>>(), unnameable),
            (vec!["a", "b"], false)
        );
    }

    #[test]
    fn an_operation_whose_path_cannot_be_named_holds_every_changed_path() {
        let queue = [patch(vec![
            ("create-file", vec![path("x")]),
            (
                "edit-text",
                vec![QueuedPathResolution::Unresolved {
                    node_id: "n".to_string(),
                }],
            ),
        ])];
        let (held, unnameable) = held_paths(&queue);
        assert!(unnameable);
        let changed: BTreeSet<&str> = ["y", "z"].into_iter().collect();
        assert_eq!(
            changed_and_held(&changed, &held, unnameable),
            vec!["y", "z"]
        );
        assert_eq!(changed_and_held(&changed, &held, false), Vec::<&str>::new());
        let changed: BTreeSet<&str> = ["x", "y"].into_iter().collect();
        assert_eq!(changed_and_held(&changed, &held, false), vec!["x"]);
    }
}
