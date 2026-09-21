//! `prikk diff` (RFC 153 §3, as amended by §6a and §7): what changed from one state to another, where a state is
//! a point in history or the worktree.
//!
//! **The worktree side is what `commit` would author** (RFC 153 §7.2): it is read by
//! [`crate::commit_boundary::worktree_patch::read_worktree_for_diff`], which is built from `commit`'s own parts
//! and writes nothing, so a new file is `added` (there is no `untracked` bucket), an ignored path never appears,
//! and a path `commit` refuses is named in [`DiffReport::unsupported_paths`] with `commit`'s own words and no
//! content. The left side of a bare diff is the current branch's tip **with its queued commits folded on top**,
//! because that is the baseline `commit` authors against.
//!
//! **One replay, twice, and `tree`'s own entry.** Each point is resolved by the shared resolver, then
//! replayed through the same anchored, block-addressed, read-only replay `checkout --patch-plan` uses, both
//! over **one read snapshot** ([`crate::patch_replay::replay_two_points_for_read_only_report`]). Every side of
//! every entry is described by [`crate::point_reading::entry_of`] -- the function `tree` and `cat` use -- so
//! this report cannot disagree with them about whether a file is text, or what mode it has.
//!
//! **Classified by path first, then renames.** A path on one side only is `added` or `deleted`; a path on
//! both is `modified`, `mode` or `binary` (or unchanged, and omitted). Only then is a vanished path and a new
//! path merged into `renamed` -- and **only when they are the same node** ([`ReplayManifestEntry::node_id`]),
//! which only a declared `RenamePath` produces. A file deleted and another created with the same bytes is a
//! `deleted` and an `added`: nothing here infers a rename from similarity (RFC 153 §3). Because the path-level
//! description is always true of the two states, a rename that cannot be told cleanly (its old path is taken
//! by another node, say) degrades to `added`/`deleted`/`modified` and never to a wrong claim.
//!
//! **Text is compared as lines** by [`crate::line_diff`]; **binary content is never rendered**: a binary entry
//! carries ids and sizes, and the caller names `prikk cat` as the way to read either side.
//!
//! An upper-layer module (RFC 149): an operation built on replay and the point resolver.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, ObjectId};

use crate::commit_boundary::worktree_patch::{Retain, read_worktree_for_diff};
use crate::foundation::layout::RepositoryLayout;
use crate::line_diff::unified_hunks;
use crate::patch_replay::{
    ReplayManifestEntry, SnapshotAnchorFallback, replay_point_for_read_only_report,
    replay_two_points_for_read_only_report,
};
use crate::point::Point;
use crate::point_reading::{PointEntry, PointEntryEncoding, entry_of};

/// What happened to one path between the two points (RFC 153 §3). One status per entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DiffStatus {
    /// The path is on the right side only.
    Added,
    /// The path is on the left side only.
    Deleted,
    /// A text file whose content differs.
    Modified,
    /// A **declared** rename: the same node at a different path.
    Renamed,
    /// The content is the same and the mode is not.
    Mode,
    /// A binary file whose content differs: ids and sizes only, never bytes.
    Binary,
    /// A symlink whose target differs. **Unreachable today** (RFC 153 §7.4): replay applies no symlink
    /// operation, so a history holding one fails the call before it can be classified. The status stays in
    /// the schema for the day replay does.
    Symlink,
}

impl DiffStatus {
    /// The `status` value in `diff-report-v1`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Added => "added",
            Self::Deleted => "deleted",
            Self::Modified => "modified",
            Self::Renamed => "renamed",
            Self::Mode => "mode",
            Self::Binary => "binary",
            Self::Symlink => "symlink",
        }
    }
}

/// The name the worktree side of a comparison goes by, in prose and in `diff-report-v1`.
pub const WORKTREE_POINT: &str = "worktree";

/// One side of the comparison, as it was named and as it resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffPoint {
    /// The point exactly as named: a ref name, or a block id -- or [`WORKTREE_POINT`] for the worktree. (No ref
    /// or block id can be spelled `worktree`, so the name is unambiguous.)
    pub point: String,
    /// The Block it resolved to. `None` for the worktree, and for the branch of a fresh repository, which is
    /// not published: its left side is the empty state.
    pub target_block_id: Option<ObjectId>,
    /// Whether this side is the worktree.
    pub worktree: bool,
    /// **Only for the implicit left side of a bare diff** (the current branch's tip): how many already-queued,
    /// unsealed commits were folded on top of `target_block_id`, because `commit` authors against the tip *and*
    /// its queue and a report that hid them would contradict the commit it describes. `None` for every side that
    /// is a sealed point.
    pub queued_patches: Option<usize>,
}

impl DiffPoint {
    fn sealed(point: &Point, target: ObjectId) -> Self {
        Self {
            point: point.name.clone(),
            target_block_id: Some(target),
            worktree: false,
            queued_patches: None,
        }
    }

    fn worktree() -> Self {
        Self {
            point: WORKTREE_POINT.to_string(),
            target_block_id: None,
            worktree: true,
            queued_patches: None,
        }
    }
}

/// One path that differs between the two points.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffEntry {
    /// The entry's path: the path on the right side, or on the left for a deletion. For a rename it is the
    /// **new** path, and the old one is `from.path`.
    pub path: String,
    /// What happened to it.
    pub status: DiffStatus,
    /// The left side, when the path existed there. **The same description `tree` gives the file**, so its
    /// `content_id` is present for a binary file only.
    pub from: Option<PointEntry>,
    /// The right side, when the path exists there.
    pub to: Option<PointEntry>,
    /// The unified hunks, each rendered as text from its `@@` line, for a text entry whose content differs.
    /// Empty for a binary, mode-only or content-identical entry.
    pub hunks: Vec<String>,
    /// Whether the hunks render a **shortest** edit script (RFC 153 §6a C). `false` only when the line diff's
    /// work bound engaged for this entry: the hunks are then still valid -- applying them reproduces the right
    /// side byte for byte -- but larger than the shortest. `true` for an entry with no hunks, which has no
    /// script to be shorter than.
    pub minimal: bool,
}

/// A worktree path `commit` refuses, named with the refusal `commit` prints and no content (RFC 147 §3f,
/// RFC 153 §7.2).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UnsupportedPath {
    /// The path as the worktree spells it.
    pub path: String,
    /// The refusal `commit` prints for it.
    pub refusal: String,
}

/// The whole comparison (RFC 153 §3, `diff-report-v1`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffReport {
    /// The left point.
    pub from: DiffPoint,
    /// The right point.
    pub to: DiffPoint,
    /// The differing paths, in canonical path order. Empty when the two states are the same.
    pub entries: Vec<DiffEntry>,
    /// Worktree paths `commit` refuses, named instead of compared, in path order. **Always empty between two
    /// points**; a path listed here is never also an entry.
    pub unsupported_paths: Vec<UnsupportedPath>,
}

/// What one path is, before it is described.
enum Candidate<'a> {
    Both(&'a ReplayManifestEntry, &'a ReplayManifestEntry),
    Added(&'a ReplayManifestEntry),
    Deleted(&'a ReplayManifestEntry),
    Renamed(&'a ReplayManifestEntry, &'a ReplayManifestEntry),
}

/// A text entry's content. A node recorded as text is valid UTF-8 by construction (`commit` classifies it
/// so), so this failing is damage, and the whole call fails rather than rendering a lossy conversion that
/// would no longer apply.
fn text_of(entry: &ReplayManifestEntry) -> Result<&str> {
    std::str::from_utf8(&entry.bytes).map_err(|_| {
        PrikkError::Integrity(format!(
            "{} is recorded as text, but its content at this point is not valid UTF-8",
            entry.path.as_str()
        ))
    })
}

/// The hunks between two entries' text, and whether they render a shortest script.
fn hunks_between(
    left: Option<&ReplayManifestEntry>,
    right: Option<&ReplayManifestEntry>,
) -> Result<(Vec<String>, bool)> {
    let left_text = match left {
        Some(entry) => text_of(entry)?,
        None => "",
    };
    let right_text = match right {
        Some(entry) => text_of(entry)?,
        None => "",
    };
    let rendered = unified_hunks(left_text, right_text);
    Ok((rendered.hunks, rendered.minimal))
}

/// No script at all: a binary, mode-only or content-identical entry, which is trivially as short as it can be.
fn no_hunks() -> (Vec<String>, bool) {
    (Vec::new(), true)
}

/// Describe one candidate, or `None` when the path is the same on both sides.
fn describe(candidate: &Candidate<'_>) -> Result<Option<DiffEntry>> {
    let text = |entry: &ReplayManifestEntry| {
        Ok::<_, PrikkError>(entry_of(entry)?.encoding == PointEntryEncoding::Text)
    };
    match candidate {
        Candidate::Both(left, right) => {
            let (from, to) = (entry_of(left)?, entry_of(right)?);
            let same_content = left.bytes == right.bytes && from.encoding == to.encoding;
            if same_content && left.mode == right.mode {
                return Ok(None);
            }
            let (status, (hunks, minimal)) = if same_content {
                (DiffStatus::Mode, no_hunks())
            } else if from.encoding == PointEntryEncoding::Binary
                || to.encoding == PointEntryEncoding::Binary
            {
                (DiffStatus::Binary, no_hunks())
            } else {
                (
                    DiffStatus::Modified,
                    hunks_between(Some(left), Some(right))?,
                )
            };
            Ok(Some(DiffEntry {
                path: right.path.as_str().to_string(),
                status,
                from: Some(from),
                to: Some(to),
                hunks,
                minimal,
            }))
        }
        Candidate::Added(right) => {
            let (hunks, minimal) = if text(right)? {
                hunks_between(None, Some(right))?
            } else {
                no_hunks()
            };
            Ok(Some(DiffEntry {
                path: right.path.as_str().to_string(),
                status: DiffStatus::Added,
                from: None,
                to: Some(entry_of(right)?),
                hunks,
                minimal,
            }))
        }
        Candidate::Deleted(left) => {
            let (hunks, minimal) = if text(left)? {
                hunks_between(Some(left), None)?
            } else {
                no_hunks()
            };
            Ok(Some(DiffEntry {
                path: left.path.as_str().to_string(),
                status: DiffStatus::Deleted,
                from: Some(entry_of(left)?),
                to: None,
                hunks,
                minimal,
            }))
        }
        Candidate::Renamed(left, right) => {
            let both_text = text(left)? && text(right)?;
            let (hunks, minimal) = if both_text && left.bytes != right.bytes {
                hunks_between(Some(left), Some(right))?
            } else {
                no_hunks()
            };
            Ok(Some(DiffEntry {
                path: right.path.as_str().to_string(),
                status: DiffStatus::Renamed,
                from: Some(entry_of(left)?),
                to: Some(entry_of(right)?),
                hunks,
                minimal,
            }))
        }
    }
}

/// Whether `filter` (exact paths; empty means everything) selects this candidate: by its own path, or, for a
/// rename, by the path it came from.
fn selected(filter: &[String], path: &str, renamed_from: Option<&str>) -> bool {
    filter.is_empty()
        || filter
            .iter()
            .any(|wanted| wanted == path || renamed_from == Some(wanted.as_str()))
}

/// Compare two replayed manifests (each sorted by path): the entries that differ, in canonical path order.
fn diff_manifests(
    left: &[ReplayManifestEntry],
    right: &[ReplayManifestEntry],
    filter: &[String],
) -> Result<Vec<DiffEntry>> {
    let left_by_path: BTreeMap<&str, &ReplayManifestEntry> = left
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let right_by_path: BTreeMap<&str, &ReplayManifestEntry> = right
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();

    // A vanished path, by the node it was: only the same node arriving elsewhere is a rename. A file with no
    // node (a worktree file no commit has authored) cannot be one side of a rename.
    let mut vanished: BTreeMap<NodeId, &ReplayManifestEntry> = left
        .iter()
        .filter(|entry| !right_by_path.contains_key(entry.path.as_str()))
        .filter_map(|entry| entry.node_id.map(|node_id| (node_id, entry)))
        .collect();
    let mut renamed_nodes: BTreeSet<NodeId> = BTreeSet::new();

    let mut candidates: BTreeMap<&str, Candidate<'_>> = BTreeMap::new();
    for entry in right {
        let path = entry.path.as_str();
        match left_by_path.get(path) {
            Some(before) => {
                candidates.insert(path, Candidate::Both(before, entry));
            }
            None => match entry.node_id.and_then(|node_id| vanished.remove(&node_id)) {
                Some(before) => {
                    renamed_nodes.extend(entry.node_id);
                    candidates.insert(path, Candidate::Renamed(before, entry));
                }
                None => {
                    candidates.insert(path, Candidate::Added(entry));
                }
            },
        }
    }
    for entry in left {
        let path = entry.path.as_str();
        let renamed = entry
            .node_id
            .is_some_and(|node_id| renamed_nodes.contains(&node_id));
        if !right_by_path.contains_key(path) && !renamed {
            candidates.insert(path, Candidate::Deleted(entry));
        }
    }

    let mut entries = Vec::new();
    for (path, candidate) in &candidates {
        let renamed_from = match candidate {
            Candidate::Renamed(before, _) => Some(before.path.as_str()),
            _ => None,
        };
        if !selected(filter, path, renamed_from) {
            continue;
        }
        if let Some(entry) = describe(candidate)? {
            entries.push(entry);
        }
    }
    Ok(entries)
}

/// **Compare two points** (RFC 153 §2 and §3): the paths that differ between `from` and `to`, with a unified
/// rendering of each text change. `paths` (exact, root-relative; empty means all) filters the **output** --
/// both replays are whole-tree either way, and the cost does not shrink.
///
/// Both points come from the shared resolver, so a received ref is read and a bare block id is a point.
/// Read-only, over one read snapshot. Any snapshot a replay could not anchor at is returned beside the
/// report (RFC 136 §10.3b.4).
///
/// # Errors
///
/// Either replay fails -- an unsupported operation, or a blob that is missing or does not recompute, in
/// **either** chain fails the whole call (RFC 157 §5a), never a partial comparison -- or a text entry's
/// content is not valid UTF-8.
pub fn diff_points_reporting_anchor(
    layout: &RepositoryLayout,
    from: &Point,
    to: &Point,
    paths: &[String],
) -> Result<(DiffReport, Vec<SnapshotAnchorFallback>)> {
    let [(left, left_fallback), (right, right_fallback)] =
        replay_two_points_for_read_only_report(layout, from, to)?;
    let entries = diff_manifests(&left.manifest.files, &right.manifest.files, paths)?;
    Ok((
        DiffReport {
            from: DiffPoint::sealed(from, left.target_block_id),
            to: DiffPoint::sealed(to, right.target_block_id),
            entries,
            unsupported_paths: Vec::new(),
        },
        left_fallback.into_iter().chain(right_fallback).collect(),
    ))
}

/// **Compare a point with the worktree** (RFC 153 §2, §6.2, §7.2, §7.3): what a bare `prikk diff` shows.
///
/// `from` is a point to compare against, or `None` for the **current branch's tip with its queued commits
/// folded on top** -- the baseline `commit` authors against, so that `commit` followed by this is empty again. An
/// unpublished branch (a fresh repository) has the empty state on that side: every file `commit` would author is
/// `added`. `branch` names the current branch either way: the worktree is read against *its* baseline (node
/// identity, file kinds, live rename declarations), because that is what `commit` would author it against.
///
/// **Read-only** (the worktree is read by `read_worktree_for_diff` in the commit boundary): no lock beyond the
/// read snapshot, no write, and the dirty and provisional worktree markers, which gate history, do not gate a
/// look. `paths` (exact; empty means everything) filters the output, both entries and unsupported paths.
///
/// # Errors
///
/// The point's replay fails (an unsupported operation or a damaged blob in the chain fails the whole call), the
/// baseline cannot be derived, a live rename declaration is one `commit` refuses (the call refuses with
/// `commit`'s words), or the worktree cannot be read.
pub fn diff_worktree_reporting_anchor(
    layout: &RepositoryLayout,
    branch: &str,
    from: Option<&Point>,
    paths: &[String],
) -> Result<(DiffReport, Vec<SnapshotAnchorFallback>)> {
    let (left_point, entries, unsupported, fallback) = match from {
        None => {
            let read = read_worktree_for_diff(layout, branch, Retain::ChangedFromBaseline)?;
            let entries = diff_manifests(&read.baseline, &read.worktree, paths)?;
            let point = DiffPoint {
                point: branch.to_string(),
                target_block_id: read.tip_block_id,
                worktree: false,
                queued_patches: Some(read.queued_patches),
            };
            (point, entries, read.unsupported, None)
        }
        Some(point) => {
            let (snapshot, fallback) = replay_point_for_read_only_report(layout, point)?;
            let read = read_worktree_for_diff(layout, branch, Retain::Everything)?;
            let entries = diff_manifests(&snapshot.manifest.files, &read.worktree, paths)?;
            let left = DiffPoint::sealed(point, snapshot.target_block_id);
            (left, entries, read.unsupported, fallback)
        }
    };
    // A path `commit` refuses is named once, as unsupported, and is never also an entry (a tracked file replaced
    // by a symlink is not `deleted`).
    let refused: BTreeSet<&str> = unsupported.iter().map(|u| u.path.as_str()).collect();
    let entries: Vec<DiffEntry> = entries
        .into_iter()
        .filter(|entry| {
            !refused.contains(entry.path.as_str())
                && !entry
                    .from
                    .as_ref()
                    .is_some_and(|side| refused.contains(side.path.as_str()))
        })
        .collect();
    let unsupported_paths = unsupported
        .into_iter()
        .filter(|u| paths.is_empty() || paths.contains(&u.path))
        .map(|u| UnsupportedPath {
            path: u.path,
            refusal: u.refusal,
        })
        .collect();
    Ok((
        DiffReport {
            from: left_point,
            to: DiffPoint::worktree(),
            entries,
            unsupported_paths,
        },
        fallback.into_iter().collect(),
    ))
}
