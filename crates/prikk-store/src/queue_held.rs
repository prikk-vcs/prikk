//! 0.59.0 steps W2 and W3: **no command that writes the worktree changes, on disk, a path that an active queue holds, whichever
//! ref owns the queue.**
//!
//! A command that writes the worktree writes **sealed** files (a branch's tip, a merge result, an adopted tip). The queue's
//! committed-but-unsealed work is not in them. Since 0.56.0 step W2f the files such a command may overwrite or delete are
//! judged against the worktree's **record**, which `commit --ref <x>` writes with `<x>`'s queue folded on top; so a queued new
//! file, edit, deletion or rename read as "the branch being left's own" and the command put sealed bytes over it (0.55.0
//! refused a queued edit, deletion or rename and kept a queued new file).
//!
//! The refusal, before the first write (before publish): the paths the queue holds, intersected with the paths the plan would
//! write over, create, delete or give a new mode, refuse. An empty intersection proceeds, so a queue that changes nothing the
//! command touches (a file created and deleted again, say) is not refused. One function serves `branch switch` (step W2: the
//! queue is the target's, refusal 2 having refused any other) and every producer that moves the current branch (step W3: `merge
//! --into`, `archive import --adopt`, `bundle import --adopt`, `branch create`), which reach it through
//! `worktree_materialize::require_nothing_in_the_way`.

use std::collections::BTreeSet;
use std::path::Path;

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::fsutil::stat_file_state_if_exists;
use crate::foundation::layout::RepositoryLayout;
use crate::patch_replay::ReplayManifestEntry;
use crate::wal::WalReplay;
use crate::worktree_marker::worktree_is_dirty;
use crate::worktree_status::{QueuedPatchEntry, QueuedPathResolution, enumerate_queued_patches};

/// How many paths the sentence names before `and N more`.
const PATHS_SHOWN: usize = 5;

/// What the plan would do to the disk: the paths it writes (over or new) or deletes, and the manifest entries it leaves with
/// their bytes, which a mode change can still touch.
pub(crate) struct PlannedChanges<'a> {
    pub(crate) written: Vec<&'a str>,
    pub(crate) deleted: Vec<&'a str>,
    pub(crate) unchanged: &'a [&'a ReplayManifestEntry],
}

/// What the queue is, as the caller already knows it from the WAL it replayed: how many commits, and the newest (the one
/// `uncommit` takes). `None` when the queue is empty, which is all a caller with no queue has to carry.
#[derive(Debug, Clone, Copy)]
pub(crate) struct QueueFacts {
    count: usize,
    newest: ObjectId,
}

impl QueueFacts {
    /// The facts of `replay`'s queue, or `None` when it holds no commit.
    pub(crate) fn of(replay: &WalReplay) -> Option<Self> {
        let newest = replay.records.last()?.envelope.object_id();
        Some(Self {
            count: replay.records.len(),
            newest,
        })
    }
}

/// Who is asking, for the sentence: the switch (the queue is the target's) or a producer moving the current branch.
pub(crate) enum Who<'a> {
    Switch {
        target: &'a str,
    },
    Producer {
        /// The participle phrase the producer's own refusals already use (`merging heads/a into heads/b`).
        action: &'a str,
        /// What is true of the repository at this point, when it is not "nothing was written" (`archive import --adopt`'s backstop).
        already_written: Option<&'a str>,
    },
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

/// Refuse, before any write, a command that would change a path the active queue holds.
pub(crate) fn refuse_when_the_queue_holds_a_changed_path(
    layout: &RepositoryLayout,
    queue: &QueueFacts,
    who: &Who<'_>,
    plan: &PlannedChanges<'_>,
) -> Result<()> {
    let mut changed: BTreeSet<&str> = plan.written.iter().chain(&plan.deleted).copied().collect();
    let patches = enumerate_queued_patches(layout)?;
    let (held, unnameable) = held_paths(&patches);
    // A mode the command would reset on a file it otherwise leaves is a change too; only a path the queue holds is looked at.
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
    let owner = match who {
        Who::Switch { target } => (*target).to_string(),
        Who::Producer { .. } => queue_owner(layout)?,
    };
    // While a switch or checkout is marked interrupted, `uncommit` refuses (the worktree is not verified), so it is not named.
    let marked = worktree_is_dirty(layout)?;
    Err(PrikkError::Precondition(refusal_sentence(
        who, &owner, queue, &hit, unnameable, marked,
    )))
}

/// The ref the active queue belongs to, which `seal --ref` takes.
fn queue_owner(layout: &RepositoryLayout) -> Result<String> {
    match crate::commit_boundary::active::read_active_ref_metadata(layout)? {
        crate::commit_boundary::active::ActiveRefMetadata::Valid(owner) => Ok(owner),
        crate::commit_boundary::active::ActiveRefMetadata::Missing => Err(PrikkError::Integrity(
            "active WAL has records but active ref metadata is missing".to_string(),
        )),
        crate::commit_boundary::active::ActiveRefMetadata::Invalid(reason) => {
            Err(PrikkError::Integrity(format!(
                "active WAL has records but active ref metadata is malformed: {reason}"
            )))
        }
    }
}

fn refusal_sentence(
    who: &Who<'_>,
    owner: &str,
    queue: &QueueFacts,
    hit: &[&str],
    unnameable: bool,
    marked: bool,
) -> String {
    let count = queue.count;
    let newest = queue.newest;
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
    // The switch's queue is the target's ("its queue"); a producer names the queue's owner.
    let (whose, subject) = match who {
        Who::Switch { .. } => ("its".to_string(), "the switch".to_string()),
        Who::Producer { action, .. } => (format!("{owner}'s"), (*action).to_string()),
    };
    let lead = if unnameable {
        format!(
            "{whose} queue holds {count} commit(s) with changes to paths it cannot name, and {subject} would overwrite or \
             delete {shown}{more} on disk"
        )
    } else {
        format!(
            "{whose} queue holds {count} commit(s) that change {shown}{more}, which {subject} would overwrite or delete on disk"
        )
    };
    let written = match who {
        Who::Producer {
            already_written: Some(text),
            ..
        } => (*text).to_string(),
        _ => "nothing was written".to_string(),
    };
    let ways_out = match (who, marked) {
        (Who::Switch { .. }, true) => format!(
            "`prikk seal --allow-no-audit --ref {owner}` publishes them and keeps your files; run the switch again after it"
        ),
        (Who::Switch { .. }, false) => format!(
            "`prikk seal --allow-no-audit --ref {owner}` publishes them and keeps your files, \
             or `prikk uncommit {newest}` takes the newest commit back and leaves your files"
        ),
        (Who::Producer { .. }, true) => format!(
            "`prikk seal --allow-no-audit --ref {owner}` publishes them to {owner}; run this command again after it"
        ),
        (Who::Producer { .. }, false) => format!(
            "`prikk seal --allow-no-audit --ref {owner}` publishes them to {owner}, \
             or `prikk uncommit {newest}` takes the newest commit back and leaves your files"
        ),
    };
    match who {
        Who::Switch { target } => {
            format!("refusing to switch to {target}: {lead} ({written}); {ways_out}")
        }
        Who::Producer { .. } => format!("refusing: {lead} ({written}); {ways_out}"),
    }
}

#[cfg(test)]
pub(crate) fn refusal_sentence_for_test(
    who: &Who<'_>,
    owner: &str,
    count: usize,
    newest: ObjectId,
    hit: &[&str],
    unnameable: bool,
    marked: bool,
) -> String {
    refusal_sentence(
        who,
        owner,
        &QueueFacts { count, newest },
        hit,
        unnameable,
        marked,
    )
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
