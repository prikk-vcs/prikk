//! Reading a point of history (RFC 157): the present leaf paths at a point, and one file's content there.
//!
//! **One replay, one entry.** Both readers take a resolved [`Point`], replay it once through the same
//! anchored read-only replay `checkout --patch-plan` uses, and describe each file through
//! [`entry_of`], whose text/binary decision is [`crate::patch_replay::classify_manifest_entry`] -- the
//! classification `checkout`'s content report makes. A listing and a single file's metadata therefore
//! cannot disagree.
//!
//! An upper-layer module (RFC 149): an operation built on replay and the point resolver.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::layout::RepositoryLayout;
use crate::patch_replay::{
    ManifestEncoding, ReplayManifestEntry, SnapshotAnchorFallback, classify_manifest_entry,
    replay_point_for_read_only_report,
};
use crate::point::Point;

/// What a listed path is (RFC 157 §3's `kind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PointEntryKind {
    /// A regular file.
    File,
}

impl PointEntryKind {
    /// The `kind` value in `tree-listing-v1` and `path-content-v1`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::File => "file",
        }
    }
}

/// How a file's content is encoded (RFC 157 §3's `encoding`): checkout's own classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PointEntryEncoding {
    /// A text file: its content is reconstructed by replay and has no stored object of its own.
    Text,
    /// A binary file, stored as a blob.
    Binary,
}

impl PointEntryEncoding {
    /// The `encoding` value in `tree-listing-v1` and `path-content-v1`.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Binary => "binary",
        }
    }
}

/// One present leaf path at a point (RFC 157 §3, one `tree-listing-v1` entry).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PointEntry {
    /// Repository-relative path.
    pub path: String,
    /// What the path is.
    pub kind: PointEntryKind,
    /// Text or binary.
    pub encoding: PointEntryEncoding,
    /// The full file mode, as `checkout` reports it (`33188`, `33261`).
    pub mode: u32,
    /// The exact size, in bytes, of the content at the point -- reconstructed for text.
    pub size: u64,
    /// The stored blob id, **for a binary file only**. Never for text, even when a checkpoint happens to
    /// store that content, so the listing does not change with snapshot cadence.
    pub content_id: Option<ObjectId>,
}

/// The present leaf paths at a point (RFC 157 §3, `tree-listing-v1`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TreeListing {
    /// The point exactly as named: a ref name, or a block id.
    pub point: String,
    /// The Block listed, or `None` for an unpublished current branch, which lists nothing.
    pub target_block_id: Option<ObjectId>,
    /// The `--prefix` exactly as given.
    pub prefix: Option<String>,
    /// Entries in canonical path order.
    pub entries: Vec<PointEntry>,
}

/// Describe one replayed file. The one place an entry's metadata is decided (module doc).
fn entry_of(entry: &ReplayManifestEntry) -> Result<PointEntry> {
    let (encoding, content_id) = match classify_manifest_entry(entry) {
        ManifestEncoding::Text => (PointEntryEncoding::Text, None),
        ManifestEncoding::Binary(blob_id) => (PointEntryEncoding::Binary, Some(blob_id)),
        ManifestEncoding::Opaque => {
            return Err(PrikkError::Integrity(format!(
                "{} has no text or binary kind after replay",
                entry.path.as_str()
            )));
        }
    };
    Ok(PointEntry {
        path: entry.path.as_str().to_string(),
        kind: PointEntryKind::File,
        encoding,
        mode: entry.mode,
        size: entry.bytes.len() as u64,
        content_id,
    })
}

/// Whether `path` lies at or under `prefix`, by **whole path components**: `src` matches `src` and
/// `src/main.rs`, never `srcx/a`. `prefix` carries no trailing `/`.
fn under_prefix(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// A `--prefix` as the component path it names: one trailing `/` is dropped (`src/` names `src`).
///
/// # Errors
///
/// `InvalidName` when what remains is not a valid repository-relative path.
pub fn parse_tree_prefix(prefix: &str) -> Result<String> {
    let trimmed = prefix.strip_suffix('/').unwrap_or(prefix);
    Ok(crate::path::RepoPath::parse(trimmed)?.as_str().to_string())
}

/// **List the present leaf paths at a point** (RFC 157 §3): one anchored replay, whole-tree whatever
/// `prefix` is, then the entries at or under `prefix` by whole path components. The snapshot this
/// read-only report could not anchor at, if any, is returned beside it (RFC 136 §10.3b.4).
///
/// # Errors
///
/// The replay fails -- an unsupported operation anywhere in the chain fails the whole call, never a
/// partial listing (RFC 140 §7b) -- or `prefix` is not a valid path.
pub fn list_tree_at_point_reporting_anchor(
    layout: &RepositoryLayout,
    point: &Point,
    prefix: Option<&str>,
) -> Result<(TreeListing, Option<SnapshotAnchorFallback>)> {
    let component_prefix = prefix.map(parse_tree_prefix).transpose()?;
    let (snapshot, fallback) = replay_point_for_read_only_report(layout, point)?;
    let mut entries = Vec::new();
    for entry in &snapshot.manifest.files {
        if component_prefix
            .as_deref()
            .is_some_and(|prefix| !under_prefix(entry.path.as_str(), prefix))
        {
            continue;
        }
        entries.push(entry_of(entry)?);
    }
    Ok((
        TreeListing {
            point: point.name.clone(),
            target_block_id: Some(snapshot.target_block_id),
            prefix: prefix.map(str::to_string),
            entries,
        },
        fallback,
    ))
}

/// One file's content at a point (RFC 157 §4), with the same metadata one [`TreeListing`] entry carries.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PathContent {
    /// The point exactly as named.
    pub point: String,
    /// The Block read.
    pub target_block_id: ObjectId,
    /// The path's own entry -- [`entry_of`]'s, the same one `tree` lists.
    pub entry: PointEntry,
    /// The content at the point: a text file's reconstructed bytes, or a binary file's stored bytes.
    pub bytes: Vec<u8>,
}

/// **Read one path's content at a point** (RFC 157 §4): one anchored replay, the path's own entry, and its
/// bytes -- **fully resolved before the caller writes anything**.
///
/// `max_bytes` is the caller's all-or-nothing bound (§4): content above it refuses, naming the size and the
/// bound, **before** any byte is handed back, so a caller that writes only what it receives cannot write a
/// partial file. The size compared is the entry's own exact size, which for a binary file is its stored
/// content's length and for a text file is the replay's reconstruction.
///
/// # Errors
///
/// `Precondition` when the path is not present at the point (a directory prefix is not a file, and is
/// refused the same way) or exceeds `max_bytes`; the replay's own failures otherwise -- an unsupported
/// operation, or a missing or non-recomputing blob, fails the whole call (RFC 157 §5a).
pub fn read_path_at_point_reporting_anchor(
    layout: &RepositoryLayout,
    point: &Point,
    path: &str,
    max_bytes: Option<u64>,
) -> Result<(PathContent, Option<SnapshotAnchorFallback>)> {
    let (snapshot, fallback) = replay_point_for_read_only_report(layout, point)?;
    let found = snapshot
        .manifest
        .files
        .iter()
        .find(|entry| entry.path.as_str() == path)
        .ok_or_else(|| {
            PrikkError::Precondition(format!("path {path} does not exist at {}", point.name))
        })?;
    let entry = entry_of(found)?;
    if let Some(bound) = max_bytes {
        if entry.size > bound {
            return Err(PrikkError::Precondition(format!(
                "{path} is {} bytes at {}, above the --max-bytes bound of {bound}; nothing was written",
                entry.size, point.name
            )));
        }
    }
    Ok((
        PathContent {
            point: point.name.clone(),
            target_block_id: snapshot.target_block_id,
            bytes: found.bytes.clone(),
            entry,
        },
        fallback,
    ))
}

/// The listing of an **unpublished current branch** (RFC 157 §3: a fresh repository lists nothing, with
/// exit 0 -- the refusal sweep's implicit-branch rule). An explicit absent ref refuses instead; that is
/// the caller's decision, made through [`crate::ref_resolution::require_existing_ref`].
#[must_use]
pub fn unpublished_branch_tree_listing(branch: &str, prefix: Option<&str>) -> TreeListing {
    TreeListing {
        point: branch.to_string(),
        target_block_id: None,
        prefix: prefix.map(str::to_string),
        entries: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::under_prefix;

    #[test]
    fn a_prefix_matches_whole_components_only() {
        assert!(under_prefix("src/main.rs", "src"));
        assert!(under_prefix("src", "src"));
        assert!(under_prefix("src/a/b.rs", "src/a"));
        assert!(!under_prefix("srcx/a", "src"));
        assert!(!under_prefix("src.rs", "src"));
        assert!(!under_prefix("sr", "src"));
    }
}
