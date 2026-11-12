//! Changed-path commit index (DC-56).
//!
//! A rebuildable, non-authoritative cache under `cache_dir()` that lets `commit` skip reading a
//! worktree file's content when its size and mtime match what was last recorded for it —
//! [`CommitIndexEntry::matches_stat`] deliberately excludes mode from the trust condition, since a
//! permission change does not change content; `mode` is carried on the entry as plain bookkeeping,
//! not as a cache-validity input (DC-87 §3.3/§4.3 — it is also `None`-able at the source on a
//! platform with no observable POSIX mode, which the trust condition must stay indifferent to). Per
//! NFR-PERF-04 (`specs/prikk-non-functional-requirements-v1.1.md` §3 traceability row, "Caches are
//! rebuildable and never roots of trust"), this index is never authoritative for object identity: a
//! corrupt or absent file is always treated as an empty index, never a hard error, and any commit's
//! result is identical whether the index is warm, cold, or missing entirely — only the amount of
//! worktree content re-read differs.
//!
//! **Two further rules keep an entry from ever changing an answer.** `commit` trusts an entry unread
//! only to say a file is *unchanged* (a disagreeing "differs" is a miss, re-read and corrected --
//! `resolve_existing_file`), and records none for a file whose mtime is not older than the file
//! system's clock at the moment the commit took its lock ([`CommitIndex::set_racy_horizon`], git's
//! racy-entry rule). `prikk doctor --rebuild-caches` rebuilds the whole file.
//!
//! The validity rule this module implements — when an entry is trusted, what invalidates it, and
//! what bounds how often a rebuild occurs — is specified as a first-class document at
//! `rfcs/handoffs/DC-56-commit-full-tree-scan-compliance/cache-validity-specification-v1.md`. Read
//! that before changing the trust condition in [`CommitIndexEntry::matches_stat`].

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use prikk_error::Result;
use prikk_object::{BlobKind, CanonicalEncode, ObjectId};

use crate::foundation::fsutil::{
    RootFileStat, read_file_if_exists, stat_file_state_if_exists, write_file_atomically,
};
use crate::foundation::layout::RepositoryLayout;

const INDEX_FILE_NAME: &str = "commit-index.v1";
const INDEX_MAGIC: &str = "PRIKK-COMMIT-INDEX-V1";
const FIELD_COUNT: usize = 7;

/// One path's cached content state: the stat triple it was last computed against, and the resulting
/// content-addressed hash under the node kind that stat was read for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CommitIndexEntry {
    pub(crate) size: u64,
    pub(crate) mtime_secs: i64,
    pub(crate) mtime_nanos: u32,
    pub(crate) mode: u32,
    pub(crate) kind: BlobKind,
    pub(crate) content_hash: ObjectId,
}

impl CommitIndexEntry {
    /// True when a freshly observed stat exactly matches the state this entry's content hash was
    /// computed against — the sole condition under which the hash may be trusted without a read.
    /// This is the cache-validity specification's trust condition; see the module docs.
    pub(crate) fn matches_stat(&self, stat: &RootFileStat) -> bool {
        self.size == stat.size
            && self.mtime_secs == stat.mtime_secs
            && self.mtime_nanos == stat.mtime_nanos
    }
}

/// The full changed-path index: repository path -> last-known content state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct CommitIndex {
    entries: BTreeMap<String, CommitIndexEntry>,
    /// Git's racy-entry rule (0.57.0 step 0c): the file-system timestamp (seconds, nanoseconds)
    /// taken before this writer read any content. An entry whose file mtime is **not older** than
    /// it is not recorded -- see [`CommitIndex::set_racy_horizon`]. `None` records everything.
    racy_horizon: Option<(i64, u32)>,
}

impl CommitIndex {
    /// Load the persisted index, or an empty index if absent or unreadable. A corrupt or
    /// version-mismatched file is never a hard error: the index is rebuildable and never
    /// authoritative, so falling open to empty (forcing every path to be re-read once) is the
    /// correct failure mode, not a repository integrity issue.
    pub(crate) fn load(layout: &RepositoryLayout) -> Result<Self> {
        let relative = layout.repository_relative(&index_path(layout))?;
        let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
            return Ok(Self::default());
        };
        Ok(parse(&bytes).unwrap_or_default())
    }

    /// Persist the index through a durable atomic replace.
    pub(crate) fn save(&self, layout: &RepositoryLayout) -> Result<()> {
        let relative = layout.repository_relative(&index_path(layout))?;
        write_file_atomically(
            layout.repository_mutation_root(),
            &relative,
            &serialize(self),
        )
    }

    /// Look up a path's cached content state.
    pub(crate) fn get(&self, path: &str) -> Option<&CommitIndexEntry> {
        self.entries.get(path)
    }

    /// Arm the racy-entry rule for the records that follow (specification §5): an entry whose file
    /// mtime is not older than `horizon` is **not recorded**, and any older entry for the same path
    /// is dropped, so the next `commit` reads that file instead of trusting a stat that a rewrite in
    /// the same timestamp tick would reproduce. The horizon must come from the **file system's own
    /// clock**, taken before the first content read (see [`file_system_horizon`]): a file written
    /// before it has a strictly older stamp, and any rewrite after the read is stamped no earlier
    /// than the horizon, so it can never collide with a recorded mtime -- on a coarse clock too.
    pub(crate) fn set_racy_horizon(&mut self, horizon: (i64, u32)) {
        self.racy_horizon = Some(horizon);
    }

    /// Record or replace a path's content state (unless the racy-entry rule leaves it out).
    pub(crate) fn record(&mut self, path: String, entry: CommitIndexEntry) {
        if self
            .racy_horizon
            .is_some_and(|horizon| (entry.mtime_secs, entry.mtime_nanos) >= horizon)
        {
            self.entries.remove(&path);
            return;
        }
        self.entries.insert(path, entry);
    }

    /// Drop every entry whose path is no longer present in the worktree. Deleted or renamed-away
    /// paths must not linger: an unbounded index would grow forever, and a stale entry left behind
    /// risks a coincidental future stat match if a new, unrelated file is later created at the same
    /// path with the same size and mtime.
    pub(crate) fn retain_paths(&mut self, live_paths: &BTreeSet<String>) {
        self.entries.retain(|path, _| live_paths.contains(path));
    }

    /// Number of tracked paths. Used by tests and the deletion/rebuild evidence check.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn entries(&self) -> &BTreeMap<String, CommitIndexEntry> {
        &self.entries
    }
}

/// The file system's own clock now: the modification time of `held_lock`, a lock file this process
/// created (exclusively) before reading any content. Using a file the file system stamped, rather than
/// the system clock, is what makes the racy-entry rule hold on a coarse-clock file system (a kernel
/// tick, FAT's 2 s): both stamps come from the same clock. If it cannot be read, the horizon is the
/// earliest possible time, so **nothing** is recorded -- a cache that cannot prove an entry safe
/// leaves it out.
pub(crate) fn file_system_horizon(held_lock: &Path) -> (i64, u32) {
    let stamp = std::fs::metadata(held_lock)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok());
    stamp
        .and_then(|d| {
            i64::try_from(d.as_secs())
                .ok()
                .map(|s| (s, d.subsec_nanos()))
        })
        .unwrap_or((i64::MIN, 0))
}

/// What `prikk doctor --rebuild-caches` read and wrote for the commit-index (0.57.0 step 0c/1d).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct CommitIndexRebuildReport {
    /// Entries the index held before.
    pub entries_before: usize,
    /// Worktree files `commit` would scan: each is read once.
    pub paths_read: usize,
    /// Entries in the rebuilt index.
    pub entries_written: usize,
    /// Files read but not entered: too recently written to trust (the racy-entry rule), changed
    /// while being read, or content `commit` itself refuses.
    pub left_out: usize,
    /// Whether the index file was written (false for a plan).
    pub written: bool,
}

/// One path where the commit-index's recorded content hash disagrees with the worktree's actual
/// current content, despite the entry's stat still matching — the case the cache-validity
/// specification's §5/§6 exists to catch: a stat heuristic that was trusted but wrong (mtime
/// granularity, clock skew, a misbehaving filesystem).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitIndexDivergence {
    /// The repository-relative path with a disagreeing entry.
    pub path: String,
    /// The content hash the index recorded.
    pub recorded_hash: ObjectId,
    /// The content hash the file's actual current bytes produce.
    pub actual_hash: ObjectId,
}

/// What to do about a [`CommitIndexDivergence`], in the words `verify` and `doctor` both print: one
/// sentence, naming the one command that cures every shape of it.
pub const COMMIT_INDEX_DIVERGENCE_ADVICE: &str = "the commit-index is a cache and holds no \
history, so nothing recorded in the repository is wrong; run `prikk doctor --rebuild-caches` to \
rebuild it from the worktree (`--plan-only` first to see what it reads)";

/// Check every commit-index entry whose recorded stat still matches the worktree against the file's
/// actual current content. An entry whose stat no longer matches is not re-read here: that is the
/// ordinary, expected "edited but not yet committed" case, not divergence — the next `commit` will
/// already re-read it. See the cache-validity specification §6 before changing this check's scope.
pub(crate) fn verify_divergence(layout: &RepositoryLayout) -> Result<Vec<CommitIndexDivergence>> {
    let index = CommitIndex::load(layout)?;
    let mut divergences = Vec::new();
    for (path, entry) in index.entries() {
        let relative = Path::new(path);
        let Some(stat) = stat_file_state_if_exists(layout.worktree_mutation_root(), relative)?
        else {
            continue;
        };
        if !entry.matches_stat(&stat) {
            continue;
        }
        let Some(bytes) = read_file_if_exists(layout.worktree_mutation_root(), relative)? else {
            continue;
        };
        let actual_hash = content_hash(entry.kind, &bytes)?;
        if actual_hash != entry.content_hash {
            divergences.push(CommitIndexDivergence {
                path: path.clone(),
                recorded_hash: entry.content_hash,
                actual_hash,
            });
        }
    }
    Ok(divergences)
}

fn index_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.cache_dir().join(INDEX_FILE_NAME)
}

pub(crate) fn serialize(index: &CommitIndex) -> Vec<u8> {
    let mut out = String::new();
    out.push_str(INDEX_MAGIC);
    out.push('\n');
    for (path, entry) in &index.entries {
        out.push_str(path);
        out.push('\t');
        out.push_str(&entry.size.to_string());
        out.push('\t');
        out.push_str(&entry.mtime_secs.to_string());
        out.push('\t');
        out.push_str(&entry.mtime_nanos.to_string());
        out.push('\t');
        out.push_str(&entry.mode.to_string());
        out.push('\t');
        out.push_str(&entry.kind.code().to_string());
        out.push('\t');
        out.push_str(&entry.content_hash.to_hex());
        out.push('\n');
    }
    out.into_bytes()
}

/// Parse a persisted index. `None` on any structural problem — malformed magic, a line with the
/// wrong field count, or a field that fails to parse — so the caller can fail open to an empty
/// index rather than propagate a corrupt cache as a repository error. Repository paths are
/// ASCII-only with no control characters (`prikk_object::validate_repo_path`), so a plain tab/
/// newline-delimited line format never needs escaping.
pub(crate) fn parse(bytes: &[u8]) -> Option<CommitIndex> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()? != INDEX_MAGIC {
        return None;
    }
    let mut entries = BTreeMap::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() != FIELD_COUNT {
            return None;
        }
        let path = fields.first()?.to_string();
        let size: u64 = fields.get(1)?.parse().ok()?;
        let mtime_secs: i64 = fields.get(2)?.parse().ok()?;
        let mtime_nanos: u32 = fields.get(3)?.parse().ok()?;
        let mode: u32 = fields.get(4)?.parse().ok()?;
        let kind_code: u16 = fields.get(5)?.parse().ok()?;
        let kind = BlobKind::from_code(kind_code).ok()?;
        let content_hash: ObjectId = fields.get(6)?.parse().ok()?;
        entries.insert(
            path,
            CommitIndexEntry {
                size,
                mtime_secs,
                mtime_nanos,
                mode,
                kind,
                content_hash,
            },
        );
    }
    Some(CommitIndex {
        entries,
        racy_horizon: None,
    })
}

/// Compute the content hash a worktree file's bytes would produce as a blob of the given kind — the
/// same formula `node_authoring` uses for `EditText`/`ReplaceBinary`/`CreateFile` comparison, shared
/// here so the index and the authoring path can never compute two different hashes for the same
/// bytes.
pub(crate) fn content_hash(kind: BlobKind, bytes: &[u8]) -> Result<ObjectId> {
    let payload = prikk_object::BlobPayload::new(kind, bytes.to_vec());
    let canonical = payload.to_canonical_bytes()?;
    Ok(ObjectId::from_canonical_payload(
        prikk_object::ObjectType::Blob,
        1,
        &canonical,
    ))
}

#[cfg(test)]
mod tests;
