//! `prikk diff` between two points (RFC 153 §3, as amended by §7): what changed from one state to another.
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

use crate::foundation::layout::RepositoryLayout;
use crate::line_diff::unified_hunks;
use crate::patch_replay::{
    ReplayManifestEntry, SnapshotAnchorFallback, replay_two_points_for_read_only_report,
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

/// One side of the comparison, as it was named and as it resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct DiffPoint {
    /// The point exactly as named: a ref name, or a block id.
    pub point: String,
    /// The Block it resolved to.
    pub target_block_id: ObjectId,
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
    /// Paths the comparison could not read and names instead. **Always empty between two points**; the
    /// worktree form fills it (RFC 147 §3f).
    pub unsupported_paths: Vec<String>,
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

fn hunks_between(
    left: Option<&ReplayManifestEntry>,
    right: Option<&ReplayManifestEntry>,
) -> Result<Vec<String>> {
    let left_text = match left {
        Some(entry) => text_of(entry)?,
        None => "",
    };
    let right_text = match right {
        Some(entry) => text_of(entry)?,
        None => "",
    };
    Ok(unified_hunks(left_text, right_text))
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
            let (status, hunks) = if same_content {
                (DiffStatus::Mode, Vec::new())
            } else if from.encoding == PointEntryEncoding::Binary
                || to.encoding == PointEntryEncoding::Binary
            {
                (DiffStatus::Binary, Vec::new())
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
            }))
        }
        Candidate::Added(right) => Ok(Some(DiffEntry {
            path: right.path.as_str().to_string(),
            status: DiffStatus::Added,
            from: None,
            to: Some(entry_of(right)?),
            hunks: if text(right)? {
                hunks_between(None, Some(right))?
            } else {
                Vec::new()
            },
        })),
        Candidate::Deleted(left) => Ok(Some(DiffEntry {
            path: left.path.as_str().to_string(),
            status: DiffStatus::Deleted,
            from: Some(entry_of(left)?),
            to: None,
            hunks: if text(left)? {
                hunks_between(Some(left), None)?
            } else {
                Vec::new()
            },
        })),
        Candidate::Renamed(left, right) => {
            let both_text = text(left)? && text(right)?;
            Ok(Some(DiffEntry {
                path: right.path.as_str().to_string(),
                status: DiffStatus::Renamed,
                from: Some(entry_of(left)?),
                to: Some(entry_of(right)?),
                hunks: if both_text && left.bytes != right.bytes {
                    hunks_between(Some(left), Some(right))?
                } else {
                    Vec::new()
                },
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

    // A vanished path, by the node it was: only the same node arriving elsewhere is a rename.
    let mut vanished: BTreeMap<NodeId, &ReplayManifestEntry> = left
        .iter()
        .filter(|entry| !right_by_path.contains_key(entry.path.as_str()))
        .map(|entry| (entry.node_id, entry))
        .collect();
    let mut renamed_nodes: BTreeSet<NodeId> = BTreeSet::new();

    let mut candidates: BTreeMap<&str, Candidate<'_>> = BTreeMap::new();
    for entry in right {
        let path = entry.path.as_str();
        match left_by_path.get(path) {
            Some(before) => {
                candidates.insert(path, Candidate::Both(before, entry));
            }
            None => match vanished.remove(&entry.node_id) {
                Some(before) => {
                    renamed_nodes.insert(entry.node_id);
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
        if !right_by_path.contains_key(path) && !renamed_nodes.contains(&entry.node_id) {
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
            from: DiffPoint {
                point: from.name.clone(),
                target_block_id: left.target_block_id,
            },
            to: DiffPoint {
                point: to.name.clone(),
                target_block_id: right.target_block_id,
            },
            entries,
            unsupported_paths: Vec::new(),
        },
        left_fallback.into_iter().chain(right_fallback).collect(),
    ))
}
