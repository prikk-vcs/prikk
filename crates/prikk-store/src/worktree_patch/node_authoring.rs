//! Node-addressed worktree authoring (DC-09 Phase 4.4a-2a / 4.4a-2aR / 4.4a-2b).
//!
//! Turns worktree changes into node-addressed §9.3 operations against a baseline reconstructed from
//! authoritative replay. Existing paths resolve to their persisted `node_id` from the replay-derived
//! lifecycle state; fresh nodes are minted through [`NodeIdGenerator`] in canonical create order;
//! text edits compute all span identity through the shared [`crate::text_span`] module. Identity-bearing
//! policies: existing-node kind is authoritative (no text↔binary transition), operation order is
//! canonical, and fresh `node_id` assignment is deterministic with respect to canonical create order.
//! File modes are normalized through the single [`normalize_file_mode`] rule and drive both
//! `CreateFile.mode` (4.4a-2aR) and existing-node `ChangePerm` detection (4.4a-2b).
//!
//! Rename authoring (RFC 144 §4o) is declaration-based, never inferred from content: a live
//! declaration recorded by `prikk mv` is consumed here against this same worktree/baseline snapshot,
//! authoring a `RenamePath` (plus any simultaneous content/mode change on the same node) for a
//! confirmed move, a plain `DeleteNode` for one whose destination was itself deleted, or nothing at
//! all for a round trip -- see the consumption block inside `author_inner`.
//!
//! Out of scope (unchanged): symlink authoring (fails closed until FDD-04 §5.4a).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::Path;

mod worktree_files;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, ChangePerm, CreateFile, DeleteNode, DeleteNodePreimage,
    EditText, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind,
    PATCH_MESSAGE_SCHEMA, PatchPayload, PatchPurpose, RenamePath, ReplaceBinary,
};

use crate::active::{prepare_empty_active_ref_for_append, require_active_ref_for_non_empty_wal};
use crate::author::author_signing::AuthorSigner;
use crate::commit_index::{self, CommitIndex, CommitIndexEntry};
use crate::foundation::fsutil::{RootFileStat, read_file_if_exists};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::node::node_id_gen::{NodeIdEntropySource, NodeIdGenerator};
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState};
use crate::object_store::{ObjectReader, ObjectWriteSession, ObjectWriter};
use crate::patch_replay::resolve_folded_worktree_baseline;
use crate::path::RepoPath;
use crate::rename_declaration::{clear_rename_declarations, read_rename_declarations};
use crate::text_span;
use crate::wal::Wal;
use crate::worktree_marker::worktree_is_dirty;
use crate::worktree_patch::{
    WorktreePatchCommitOptions, WorktreePatchCommitReport, WorktreePatchOperationKind,
    WorktreePatchOperationSummary, next_op_seq,
};
use crate::{
    ActiveRefMetadata, read_active_ref_metadata, remove_active_ref_metadata,
    validate_local_branch_ref,
};

use worktree_files::{WorktreeFileMeta, enumerate_worktree_files};

/// Canonical mode recorded for a created regular file with no executable bit, and the default on
/// platforms without an executable-bit source (4.4a-2aR, ratified rule).
const REGULAR_FILE_MODE: u32 = 0o100_644;
/// Canonical mode for a created regular file with an executable bit (4.4a-2aR, ratified rule).
const EXECUTABLE_FILE_MODE: u32 = 0o100_755;

/// Structured authoring failure. Kept structured internally (review E2/E3/E4) and flattened into
/// [`PrikkError`] only at the public command boundary.
#[derive(Debug)]
pub(crate) enum AuthorError {
    /// A changed existing path does not resolve to a live node id in the replay-derived baseline
    /// (e.g. a snapshot-only baseline, which carries no node identity). Fails closed; never minted.
    NodeIdentityUnavailable(String),
    /// New worktree bytes would change an existing node's kind (text↔binary). Out of scope.
    UnsupportedKindTransition(String),
    /// Symlink authoring is out of scope until FDD-04 §5.4a static target validation.
    UnsupportedSymlinkAuthoring(String),
    /// A live rename declaration (RFC 144 §4o.2) does not match this worktree snapshot: either the
    /// declared source is back on disk (a contradiction §3 requires refusing outright, naming the
    /// declaration), or the declared destination is already occupied by a different tracked node
    /// this commit does not also move or delete (a collision `patch_replay`'s own batch-rename
    /// check would refuse at seal time -- authoring must not emit what replay would reject).
    DeclarationContradicted(String),
    /// Fresh node-id minting failed (propagated without flattening).
    Mint(crate::node::node_id_gen::NodeIdMintError),
    /// An underlying store/encoding error.
    Store(PrikkError),
}

impl fmt::Display for AuthorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NodeIdentityUnavailable(detail) => {
                write!(f, "worktree authoring: node identity unavailable: {detail}")
            }
            Self::UnsupportedKindTransition(detail) => {
                write!(
                    f,
                    "worktree authoring: unsupported kind transition: {detail}"
                )
            }
            Self::UnsupportedSymlinkAuthoring(detail) => {
                write!(
                    f,
                    "worktree authoring: unsupported symlink authoring: {detail}"
                )
            }
            Self::DeclarationContradicted(detail) => {
                write!(
                    f,
                    "worktree authoring: rename declaration refused: {detail}"
                )
            }
            Self::Mint(e) => write!(f, "worktree authoring: {e}"),
            Self::Store(e) => write!(f, "worktree authoring: {e}"),
        }
    }
}

impl From<AuthorError> for PrikkError {
    fn from(e: AuthorError) -> Self {
        match e {
            AuthorError::Store(inner) => inner,
            AuthorError::Mint(inner) => inner.into(),
            // RFC 132's Precondition variant: a worktree that disagrees with a live declaration, or
            // a declared destination already occupied by an untouched tracked node, is a caller
            // precondition -- nothing is held and no other writer is racing this one; waiting does
            // not help, only changing the worktree or the declaration does.
            AuthorError::DeclarationContradicted(detail) => PrikkError::Precondition(detail),
            other => PrikkError::Integrity(other.to_string()),
        }
    }
}

impl From<PrikkError> for AuthorError {
    fn from(e: PrikkError) -> Self {
        AuthorError::Store(e)
    }
}

/// A baseline live file node, resolved from the replay-derived state.
struct BaselineFile {
    node_id: NodeId,
    kind: NodeKind,
    blob_id: ObjectId,
    mode: u32,
}

/// A planned, fully-resolved operation prior to canonical ordering / `op_seq` assignment.
struct PlannedOp {
    kind: OperationKind,
    /// Sort key: canonical repo path for the op's primary node.
    path: String,
    /// Sort key tiebreak: the node id the op addresses.
    node_id: NodeId,
    /// Report label.
    summary_kind: WorktreePatchOperationKind,
    /// Blob references this op writes or reuses (for the report's `referenced_blob_count`).
    blob_refs: usize,
}

/// Rank operations by kind for deterministic ordering (review v2 §4):
/// `DeleteNode` < `CreateFile` < `ChangePerm` < `ReplaceBinary` < `EditText`.
fn kind_rank(kind: &OperationKind) -> u8 {
    match kind {
        OperationKind::DeleteNode(_) => 0,
        OperationKind::CreateFile(_) => 1,
        OperationKind::ChangePerm(_) => 2,
        OperationKind::ReplaceBinary(_) => 3,
        OperationKind::EditText(_) => 4,
        OperationKind::CreateSymlink(_) | OperationKind::RenamePath(_) => 5,
    }
}

/// Author a node-addressed patch from worktree changes and append it to the active WAL.
///
/// The `generator` is injected (review E2): production passes `NodeIdGenerator::production()`; tests
/// pass a deterministic generator. State comes only from `replay_derived_state` (review E3); the
/// snapshot manifest is never consulted as identity authority.
pub(crate) fn author_worktree_patch<S: NodeIdEntropySource, A: AuthorSigner>(
    layout: &RepositoryLayout,
    ref_name: &str,
    message: &str,
    options: WorktreePatchCommitOptions,
    generator: &mut NodeIdGenerator<S>,
    signer: &A,
) -> Result<WorktreePatchCommitReport> {
    if message.trim().is_empty() {
        return Err(PrikkError::InvalidName(
            "commit message must not be empty".to_string(),
        ));
    }
    author_inner(
        layout,
        ref_name,
        message,
        options.active_patch_limit,
        generator,
        signer,
    )
    .map_err(PrikkError::from)
}

fn author_inner<S: NodeIdEntropySource, A: AuthorSigner>(
    layout: &RepositoryLayout,
    ref_name: &str,
    message: &str,
    active_patch_limit: usize,
    generator: &mut NodeIdGenerator<S>,
    signer: &A,
) -> std::result::Result<WorktreePatchCommitReport, AuthorError> {
    let canonical_ref = validate_local_branch_ref(ref_name).map_err(AuthorError::Store)?;
    // 4.4bR2: hold the active-session lock across the entire critical section — the active-WAL
    // emptiness/genesis guard, patch authoring, and the final WAL append — so guard and append are
    // one atomic step. `ActiveLock::acquire` is fail-fast (exclusive create), so a concurrent commit
    // either loses the lock here (LockConflict) or, if it runs after this releases, sees the appended
    // record and fails the "seal first" guard. Released on return (RAII). The append below uses the
    // raw WAL under this held lock (not `ActiveSession::append_patch`, which would re-acquire).
    let active_lock =
        ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME).map_err(AuthorError::Store)?;
    crate::refs::ensure_no_incomplete_publication(layout).map_err(AuthorError::Store)?;

    // RFC 102 Stage 1: a dirty worktree marker means a prior materialization call did not complete
    // durably -- this function's own deletion-inference loop (below) cannot distinguish "the user
    // deleted this file" from "this file's name never survived a crash," so it must not run at all
    // until the worktree has been re-verified against its baseline. Checked before any worktree
    // read below, not only before the deletion loop specifically, since nothing here re-verifies.
    if worktree_is_dirty(layout).map_err(AuthorError::Store)? {
        return Err(AuthorError::Store(PrikkError::Integrity(
            "worktree materialization was interrupted; the worktree must be re-verified against \
             its baseline before committing (re-run checkout materialization to complete it)"
                .to_string(),
        )));
    }

    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let active_replay = wal.replay().map_err(AuthorError::Store)?;
    if active_replay.trailing_partial_bytes != 0 {
        return Err(AuthorError::Store(PrikkError::InvalidName(format!(
            "active WAL has {} trailing partial bytes; run `prikk doctor --repair-wal-tail` \
             before committing",
            active_replay.trailing_partial_bytes
        ))));
    }
    // RFC 102 Stage 2: a damaged record no longer makes `replay()` itself return `Err` -- refuse
    // explicitly, since `active_replay.records` below (the active-patch-limit count, and the
    // empty-vs-non-empty branch just past it) is computed from the surviving records alone and
    // would silently under-count a damaged one out of existence otherwise.
    if active_replay.has_item_failure() {
        return Err(AuthorError::Store(PrikkError::Integrity(
            "active WAL has a damaged record; run doctor before committing".to_string(),
        )));
    }
    // DC-57 (NFR-PERF-02): the hard block fires here — before any ref-metadata write, baseline
    // resolution, blob write, or WAL append below. "Active patches" has exactly one definition and
    // computation site: the active WAL's record count, read once above. `>=` (not `>`) is deliberate:
    // once the queue already holds `active_patch_limit` patches, no more may be added; the boundary
    // tests are 799/800/999/1000/1001 against the count *before* this commit's own patch.
    if crate::worktree_patch::active_patch_limit_exceeded(
        active_replay.records.len(),
        active_patch_limit,
    ) {
        // RFC 132 part 2: a full active-patch queue is a caller precondition, not a lock -- nothing
        // is held and no other writer is racing this one; waiting does not help, only sealing does.
        return Err(AuthorError::Store(PrikkError::Precondition(format!(
            "active WAL has {} queued patches, at or above the configured limit ({active_patch_limit}); \
             run `prikk seal` before committing again",
            active_replay.records.len()
        ))));
    }
    if active_replay.records.is_empty() {
        match read_active_ref_metadata(layout).map_err(AuthorError::Store)? {
            ActiveRefMetadata::Missing => {}
            ActiveRefMetadata::Valid(_) | ActiveRefMetadata::Invalid(_) => {
                remove_active_ref_metadata(layout).map_err(AuthorError::Store)?;
            }
        }
    } else {
        // DC-66: a non-empty active WAL no longer refuses outright — it queues. Ownership must still
        // be unambiguous: every queued patch belongs to exactly one ref, and this guard's semantics
        // are unchanged by N (see `rfcs/handoffs/DC-66-multi-commit-queuing/prerequisite-questions-v1.md`
        // §2). A mismatch, or missing/malformed metadata, still fails closed exactly as before this
        // increment; only a *matching* queue now falls through to author against it instead of
        // rejecting.
        require_active_ref_for_non_empty_wal(layout, &canonical_ref).map_err(AuthorError::Store)?;
    }

    // RFC 111 §6.1 Stage 2: `author_worktree_patch` never publishes a ref itself (that happens at
    // seal time, a different operation) -- confirmed by grep, no `.publish(`/
    // `finish_interrupted_publication`/`RefPublication` anywhere in this file -- so it needs no
    // ref-publication threading, only the plain object-write-session swap.
    let mut object_store = ObjectWriteSession::open(layout).map_err(AuthorError::Store)?;

    // Baseline node lifecycle state from authoritative replay only (E3), with any already-queued
    // (unsealed) patches folded on top (DC-66) -- the single derivation `worktree-status` now
    // shares (RFC 122, `replay-baseline-handoff-v1.md` §3). DC-64's incremental cache and DC-65's
    // text-materialization fallback are both inside this call, unchanged. `queue_text_cache` is
    // declared here, not inside the shared call, because `plan_edit_text` below reuses whatever
    // folding already materialized into it.
    let mut queue_text_cache = crate::lifecycle_cache::replay::TextCache::new();
    let resolved = resolve_folded_worktree_baseline(
        layout,
        &object_store,
        ref_name,
        &active_replay,
        &mut queue_text_cache,
    )?;
    // DC-65: `plan_edit_text` needs these to materialize a text node's current bytes when its
    // baseline `blob_id` is a content identity rather than a stored object (any node whose most
    // recent operation was an `EditText`). Only ever consulted when `baseline_files` is non-empty,
    // which implies `Published` — see `plan_edit_text`'s own fail-closed check on the `None` case.
    let (lineage_baseline_block_id, lineage_horizon_id) = match resolved.lineage {
        Some((baseline_block, horizon)) => (Some(baseline_block), Some(horizon)),
        None => (None, None),
    };
    let baseline_state: NodeLifecycleState = resolved.state;

    // Baseline file view: path -> (node_id, kind, blob_id, mode). Symlink nodes are tracked so a
    // change touching one can fail closed.
    let mut baseline_files: BTreeMap<String, BaselineFile> = BTreeMap::new();
    let mut baseline_symlinks: BTreeMap<String, NodeId> = BTreeMap::new();
    for (node_id, node) in baseline_state.live_nodes() {
        match &node.content {
            NodeContent::File { blob_id, mode } => {
                baseline_files.insert(
                    node.path.as_str().to_string(),
                    BaselineFile {
                        node_id: *node_id,
                        kind: node.kind,
                        blob_id: *blob_id,
                        mode: *mode,
                    },
                );
            }
            NodeContent::Symlink { .. } => {
                baseline_symlinks.insert(node.path.as_str().to_string(), *node_id);
            }
        }
    }

    // E3: distinguish a snapshot-only baseline (path-keyed, no node identity) from a genuinely
    // empty node repo. An empty node state with a snapshot blob reference means the only identity
    // authority available is the path-keyed snapshot manifest, which Option A excludes — fail closed
    // rather than treat every snapshot-tracked file as untracked and mint fresh ids for it. This can
    // only arise for a published baseline; a genesis baseline has no block and no snapshot.
    if let Some((baseline_block, _horizon)) = resolved.lineage {
        if baseline_files.is_empty()
            && baseline_symlinks.is_empty()
            && baseline_block_has_snapshot_ref(&object_store, baseline_block)?
        {
            return Err(AuthorError::NodeIdentityUnavailable(
                "baseline is snapshot-derived and carries no node identity; \
                 a node-addressed baseline is required for worktree authoring"
                    .to_string(),
            ));
        }
    }

    // RFC 124 §4.4: every already-tracked path (files and symlinks together), so the ignore-aware
    // walk below can never hide one of them from `worktree` -- doing so would make the
    // deletion-inference loop further down read a still-present file as deleted.
    let tracked_paths: BTreeSet<String> = baseline_files
        .keys()
        .cloned()
        .chain(baseline_symlinks.keys().cloned())
        .collect();

    // Worktree view: path -> metadata for regular files (symlinks/non-regular fail closed). Content
    // is read on demand (below) so an unchanged file is never opened — DC-56.
    let worktree = enumerate_worktree_files(layout, &tracked_paths)?;

    // RFC 144 §4o.2/§3: consume every live rename declaration against this same worktree/baseline
    // snapshot, before either per-path loop below ever sees a rename's old or new path -- both need
    // the exclusion sets built here first (`renamed_away`, `rename_targets`), and the dedicated
    // rename-authoring loop further down needs `confirmed_renames`.
    //
    // Control 6's two-node swap is why every check below is evaluated against the *whole batch* of
    // live declarations, not one declaration in isolation: after `a -> tmp -> ... -> c` and
    // `c -> a` both land on disk, `old_path` for the first declaration (`a`) is genuinely present in
    // the worktree again -- but as the *second* declaration's own landing spot, not because the
    // original node never moved. Checking a single declaration's `old_path`/`new_path` against the
    // worktree without also asking "is this path claimed elsewhere in this same batch" would refuse
    // a real swap as a false contradiction (source-checked) or a false collision (destination-
    // checked) -- exactly the two-node case increments 1 and 2 built `rename_nodes_checked_batch`'s
    // own same-batch tolerance for.
    let live_declarations = read_rename_declarations(layout).map_err(AuthorError::Store)?;
    let declared_old_paths: BTreeSet<&str> = live_declarations
        .iter()
        .map(|declaration| declaration.old_path.as_str())
        .collect();
    let declared_new_paths: BTreeSet<&str> = live_declarations
        .iter()
        .map(|declaration| declaration.new_path.as_str())
        .collect();
    let mut renamed_away: BTreeSet<String> = BTreeSet::new();
    let mut confirmed_renames: Vec<(String, String)> = Vec::new();
    for declaration in &live_declarations {
        let old_path = &declaration.old_path;
        let new_path = &declaration.new_path;
        // A declaration whose old_path was never a tracked baseline node (e.g. declared against a
        // path that was itself never committed) is vacuous. Judgment call, not RFC-specified -- the
        // six required controls do not exercise this: drop silently, letting the ordinary loops
        // below handle whatever the worktree actually holds at old_path/new_path.
        let Some(base) = baseline_files.get(old_path) else {
            continue;
        };
        if worktree.contains_key(old_path.as_str())
            && !declared_new_paths.contains(old_path.as_str())
        {
            // The worktree contradicts a live declaration -- source is back, and no other live
            // declaration claims to have landed here (the swap's own tolerance above does not apply).
            // §3: refuse the whole commit, naming the declaration, rather than silently dropping a
            // human assertion.
            return Err(AuthorError::DeclarationContradicted(format!(
                "{old_path} -> {new_path}: the source is present in the worktree again; the \
                 declared move was not completed on disk. Run `prikk mv` again, or move {new_path} \
                 back to {old_path} to clear the declaration before committing"
            )));
        }
        if !worktree.contains_key(new_path.as_str()) {
            // Nets to deletion (§3 corollary 2): neither path is present. The ordinary deletion loop
            // below already authors a plain DeleteNode for old_path unassisted; nothing to claim.
            continue;
        }
        // Confirmed: old_path is a baseline node, and its declared destination is present. Refuse a
        // destination already occupied by a different, untouched tracked node that this same batch
        // does not also vacate -- authoring must not emit what `rename_nodes_checked_batch` would
        // refuse at seal time (§3's own principle, applied to a collision as much as to a chain).
        if let Some(occupant) = baseline_files.get(new_path) {
            if occupant.node_id != base.node_id && !declared_old_paths.contains(new_path.as_str()) {
                return Err(AuthorError::DeclarationContradicted(format!(
                    "{old_path} -> {new_path}: the destination is already occupied by a different \
                     tracked node that this commit does not also move or delete"
                )));
            }
        }
        renamed_away.insert(old_path.clone());
        confirmed_renames.push((old_path.clone(), new_path.clone()));
    }
    let rename_targets: BTreeSet<String> = confirmed_renames
        .iter()
        .map(|(_, new_path)| new_path.clone())
        .collect();

    // DC-56 changed-path index: per-path (size, mtime, mode) -> last-known content hash, so an
    // unchanged file's content read can be skipped. Rebuildable and never authoritative (NFR-PERF-04)
    // — a missing or corrupt index loads as empty and simply costs one full read per path, exactly as
    // the first commit against a repository already does. See the cache-validity specification at
    // `rfcs/handoffs/DC-56-commit-full-tree-scan-compliance/cache-validity-specification-v1.md`.
    let mut commit_index = CommitIndex::load(layout).map_err(AuthorError::Store)?;

    // Working state for same-session duplicate prevention (E1): clone the baseline; each fresh node
    // is inserted immediately after minting so the next mint sees it via contains_seen_node_id.
    let mut working_state = baseline_state.clone();

    let mut planned: Vec<PlannedOp> = Vec::new();

    // Fresh creates are collected first, then minted in canonical path order (E1) — so path->node_id
    // assignment is independent of worktree traversal order. Each carries its normalized mode.
    let mut create_candidates: Vec<(String, Vec<u8>, u32)> = Vec::new();

    for (path, meta) in &worktree {
        if rename_targets.contains(path) {
            // Claimed by the dedicated rename-authoring block below -- never a plain create, and
            // never re-inspected here even if it also happens to be a baseline path (the occupied-
            // destination check above already refused that case).
            continue;
        }
        if let Some(base) = baseline_files.get(path) {
            // Existing node: kind is authoritative (E4); compare in that kind, never reclassify.
            match base.kind {
                NodeKind::TextFile => {
                    let resolved = resolve_existing_file(
                        layout,
                        &mut commit_index,
                        path,
                        meta,
                        base.mode,
                        BlobKind::Text,
                    )?;
                    if resolved.content_hash != base.blob_id {
                        let bytes = match resolved.bytes {
                            Some(bytes) => bytes,
                            None => read_existing_file_bytes(layout, path, BlobKind::Text)?,
                        };
                        planned.push(plan_edit_text(
                            &object_store,
                            base,
                            &bytes,
                            path,
                            lineage_baseline_block_id,
                            lineage_horizon_id,
                            &queue_text_cache,
                        )?);
                    }
                }
                NodeKind::BinaryFile => {
                    let resolved = resolve_existing_file(
                        layout,
                        &mut commit_index,
                        path,
                        meta,
                        base.mode,
                        BlobKind::Binary,
                    )?;
                    if resolved.content_hash != base.blob_id {
                        let bytes = match resolved.bytes {
                            Some(bytes) => bytes,
                            None => read_existing_file_bytes(layout, path, BlobKind::Binary)?,
                        };
                        planned.push(plan_replace_binary(&mut object_store, base, &bytes, path)?);
                    }
                }
                NodeKind::Symlink => {
                    return Err(AuthorError::UnsupportedSymlinkAuthoring(format!(
                        "{path}: symlink node modification is out of scope"
                    )));
                }
            }
            // Mode-change detection (4.4a-2b), independent of content, for regular file nodes only
            // (symlink nodes never reach here — they live in `baseline_symlinks`). The canonical
            // operation sort places `ChangePerm` before any `ReplaceBinary`/`EditText` content op.
            if let Some(op) = plan_mode_change_if_observed(base, meta.mode, path) {
                planned.push(op);
            }
        } else if baseline_symlinks.contains_key(path) {
            return Err(AuthorError::UnsupportedSymlinkAuthoring(format!(
                "{path}: symlink node modification is out of scope"
            )));
        } else {
            // A path with no baseline node is genuinely new; there is nothing a cache could have
            // recorded for it yet, so it is always read. The read is cached anyway so an unchanged
            // re-commit of the same content (once this path is itself a baseline node) can skip it.
            let bytes = read_worktree_file_bytes(layout, path)?;
            let (blob_kind, _node_kind) = classify_new(&bytes);
            let content_hash =
                commit_index::content_hash(blob_kind, &bytes).map_err(AuthorError::Store)?;
            // No baseline node exists yet, so there is no existing recorded mode to carry forward —
            // unlike the existing-node case above. A platform with no observable POSIX mode
            // (DC-87 §3.3/§4.3) creates the file non-executable by default; this is a missing
            // capability, not silent corruption of something previously right (see
            // `docs/src/reference/platform-support.md`).
            let resolved_mode = meta.mode.unwrap_or(REGULAR_FILE_MODE);
            commit_index.record(
                path.clone(),
                CommitIndexEntry {
                    size: meta.size,
                    mtime_secs: meta.mtime_secs,
                    mtime_nanos: meta.mtime_nanos,
                    mode: resolved_mode,
                    kind: blob_kind,
                    content_hash,
                },
            );
            create_candidates.push((path.clone(), bytes, resolved_mode));
        }
    }

    // RFC 144 §4o: author each confirmed rename's RenamePath, plus any simultaneous content or mode
    // change on the same node -- safe to combine, because ChangePerm/EditText/ReplaceBinary are all
    // node-addressed (no path field), so their relative order against RenamePath never depends on
    // which path is "current" when a patch is applied. Run before the deletions loop below (and
    // before commit_index is persisted, further down) so the fresh `commit_index.record` calls made
    // here land in the same save.
    //
    // Deliberately does not call `resolve_existing_file` (this file's other two content-comparison
    // sites both do): its cache is keyed by *path*, on the trust condition "this path's own content
    // is unchanged since the entry was recorded" -- true for every other caller, which only ever
    // reads a path that has held the same node across commits. `new_path` here just changed which
    // node occupies it; any prior entry at that key describes new_path's *former* occupant, not the
    // node being evaluated now, so a stat-coincidence hit (found directly: two files this round's
    // own control 6 wrote nanoseconds apart landed in the same commit_index mtime bucket in this
    // environment) reads as a false content mismatch against `base.blob_id`, which then sends a
    // provably-unchanged rename through `plan_edit_text` and trips its own "unchanged text" guard.
    // Always reading for real avoids the whole class, and the explicit `commit_index.record` below
    // still refreshes -- rather than leaves stale -- whatever entry `new_path` carried in before.
    for (old_path, new_path) in &confirmed_renames {
        let base = baseline_files.get(old_path).ok_or_else(|| {
            AuthorError::Store(PrikkError::Integrity(format!(
                "{old_path}: confirmed rename lost its baseline node between resolution and \
                 authoring"
            )))
        })?;
        planned.push(PlannedOp {
            kind: OperationKind::RenamePath(RenamePath {
                node_id: base.node_id,
                old_path: old_path.clone(),
                new_path: new_path.clone(),
            }),
            // Deliberately not just `old_path`: `RenamePath` is the only kind at this rank
            // (`CreateSymlink` authoring always fails closed before reaching this point), so the
            // sort-key role this field otherwise plays never actually discriminates between two
            // rename ops here -- carrying both sides instead makes the CLI's per-change report
            // (which reuses this same field) show the move, not just its source.
            path: format!("{old_path} -> {new_path}"),
            node_id: base.node_id,
            summary_kind: WorktreePatchOperationKind::RenamePath,
            blob_refs: 0,
        });
        let meta = worktree.get(new_path).ok_or_else(|| {
            AuthorError::Store(PrikkError::Integrity(format!(
                "{new_path}: confirmed rename lost its destination between resolution and \
                 authoring"
            )))
        })?;
        let blob_kind = match base.kind {
            NodeKind::TextFile => BlobKind::Text,
            NodeKind::BinaryFile => BlobKind::Binary,
            NodeKind::Symlink => {
                return Err(AuthorError::UnsupportedSymlinkAuthoring(format!(
                    "{old_path} -> {new_path}: symlink node modification is out of scope"
                )));
            }
        };
        let bytes = read_existing_file_bytes(layout, new_path, blob_kind)?;
        let content_hash =
            commit_index::content_hash(blob_kind, &bytes).map_err(AuthorError::Store)?;
        let resolved_mode = meta.mode.unwrap_or(base.mode);
        commit_index.record(
            new_path.clone(),
            CommitIndexEntry {
                size: meta.size,
                mtime_secs: meta.mtime_secs,
                mtime_nanos: meta.mtime_nanos,
                mode: resolved_mode,
                kind: blob_kind,
                content_hash,
            },
        );
        if content_hash != base.blob_id {
            match base.kind {
                NodeKind::TextFile => {
                    planned.push(plan_edit_text(
                        &object_store,
                        base,
                        &bytes,
                        new_path,
                        lineage_baseline_block_id,
                        lineage_horizon_id,
                        &queue_text_cache,
                    )?);
                }
                NodeKind::BinaryFile => {
                    planned.push(plan_replace_binary(
                        &mut object_store,
                        base,
                        &bytes,
                        new_path,
                    )?);
                }
                NodeKind::Symlink => unreachable!("symlinks already refused above"),
            }
        }
        if let Some(op) = plan_mode_change_if_observed(base, meta.mode, new_path) {
            planned.push(op);
        }
    }

    // Deletions: baseline files absent from the worktree.
    for (path, base) in &baseline_files {
        if !worktree.contains_key(path) {
            if renamed_away.contains(path) {
                // Claimed above by a confirmed rename's own RenamePath -- not a plain deletion.
                continue;
            }
            planned.push(plan_delete(base, path));
        }
    }
    for path in baseline_symlinks.keys() {
        if !worktree.contains_key(path) {
            return Err(AuthorError::UnsupportedSymlinkAuthoring(format!(
                "{path}: symlink node deletion is out of scope"
            )));
        }
    }

    // Persist the refreshed index: prune paths no longer in the worktree (so a future unrelated file
    // reusing the same path can never inherit a stale entry), then write through durably. Done
    // regardless of whether this commit finds any change to make (`planned` may still be empty below)
    // — the scan already paid for whatever reads happened, so the cache should keep the benefit.
    let live_paths: BTreeSet<String> = worktree.keys().cloned().collect();
    commit_index.retain_paths(&live_paths);
    commit_index.save(layout).map_err(AuthorError::Store)?;

    // E1: mint fresh ids in canonical path order, inserting into the working state immediately.
    create_candidates.sort_by(|a, b| a.0.cmp(&b.0));
    for (path, bytes, mode) in &create_candidates {
        let repo_path = RepoPath::parse(path).map_err(AuthorError::Store)?;
        let (blob_kind, node_kind) = classify_new(bytes);
        let blob_id = write_content_blob(&mut object_store, blob_kind, bytes)?;
        let node_id = generator
            .mint_fresh(&working_state)
            .map_err(AuthorError::Mint)?;
        // Insert into the working state so a subsequent mint cannot reuse this id (E1).
        working_state
            .create_node(
                node_id,
                LiveNode {
                    path: repo_path,
                    kind: node_kind,
                    content: NodeContent::File {
                        blob_id,
                        mode: *mode,
                    },
                },
            )
            .map_err(AuthorError::Store)?;
        planned.push(PlannedOp {
            kind: OperationKind::CreateFile(CreateFile {
                path: path.clone(),
                node_id,
                blob_id,
                mode: *mode,
            }),
            path: path.clone(),
            node_id,
            summary_kind: WorktreePatchOperationKind::CreateFile,
            blob_refs: 1,
        });
    }

    if planned.is_empty() {
        // RFC 132's Precondition variant: an empty change set names no path and involves no name
        // validation at all -- `InvalidName`'s own doc ("a path-like name failed Prikk path/ref
        // validation") never applied here.
        return Err(AuthorError::Store(PrikkError::Precondition(
            "worktree has no node-addressed changes to commit".to_string(),
        )));
    }

    // Canonical operation ordering (review v2 §4): kind rank, then path bytes, then node_id bytes.
    planned.sort_by(|a, b| {
        kind_rank(&a.kind)
            .cmp(&kind_rank(&b.kind))
            .then_with(|| a.path.as_bytes().cmp(b.path.as_bytes()))
            .then_with(|| a.node_id.as_bytes().cmp(b.node_id.as_bytes()))
    });

    // Assemble the patch payload with contiguous op_seq from 1.
    let mut operations = Vec::with_capacity(planned.len());
    let mut referenced_blob_count = 0_usize;
    let mut text_edit_count = 0_usize;
    let mut summaries = Vec::with_capacity(planned.len());
    for (index, op) in planned.into_iter().enumerate() {
        let op_seq = next_op_seq(index).map_err(AuthorError::Store)?;
        referenced_blob_count += op.blob_refs;
        if matches!(op.kind, OperationKind::EditText(_)) {
            text_edit_count += 1;
        }
        summaries.push(WorktreePatchOperationSummary {
            path: op.path,
            operation: op.summary_kind,
        });
        operations.push(Operation {
            op_seq,
            op_id: None,
            preconditions: Vec::new(),
            kind: op.kind,
        });
    }

    let operation_count = operations.len();
    let patch_payload = PatchPayload {
        operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        // RFC 123 §8: `-m` is mandatory here (checked in `author_worktree_patch` above), so this
        // is always `Some`; stored exactly as given, not `trim()`'d, matching `tag create`'s own
        // -m/--message precedent (`prikk-cli/src/tag.rs`).
        message: Some(message.to_string()),
    };
    patch_payload.validate().map_err(AuthorError::Store)?;
    // RFC 134 §8 / RFC 123 §8.6: every EditText this function authors carries v2 anchor-length
    // fields (tags 10/11), and every commit carries a message, so every newly authored patch is
    // minted at PATCH_MESSAGE_SCHEMA -- unconditionally, even for patches with no EditText
    // operation, since a schema only ever *permits* its own fields, never requires them, and every
    // earlier authoring path already only ever wrote a lower schema (each stays frozen for those
    // bytes, forever).
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        PATCH_MESSAGE_SCHEMA,
        patch_payload
            .to_canonical_bytes()
            .map_err(AuthorError::Store)?,
    );
    let patch_id = patch.object_id();
    // R1: real role-bound Ed25519 AUTHOR signature over the unsigned patch object id.
    let signature = crate::author::author_signing::author_signature(signer, patch_id)
        .map_err(AuthorError::Store)?;
    patch.add_signature(signature).map_err(AuthorError::Store)?;
    // DC-53 Stage 1: record this signer's key material now, while it is still available -- the
    // signer is the only party that ever holds it, and Ed25519 signatures are not
    // public-key-recoverable, so `verify` has nothing to check this Patch's signature against
    // later unless it is captured here.
    crate::author::author_key_index::record_author_key_material(
        layout,
        signer.key_id(),
        signer.public_key_bytes(),
        &active_lock,
    )
    .map_err(AuthorError::Store)?;

    prepare_empty_active_ref_for_append(layout, &canonical_ref).map_err(AuthorError::Store)?;
    let wal_sequence = wal.append_patch(&patch).map_err(AuthorError::Store)?;

    // RFC 144 §4o.2: "cleared when the commit that consumes it is queued -- not when sealed." The
    // loop above resolves every live declaration into exactly one of confirmed-rename,
    // nets-to-deletion, or vacuous -- a contradiction returns early, before this point, and nothing
    // else leaves a declaration unresolved -- so a successful append here means the whole live set
    // was just consumed and the store is cleared unconditionally, not entry by entry.
    if !live_declarations.is_empty() {
        clear_rename_declarations(layout).map_err(AuthorError::Store)?;
    }

    Ok(WorktreePatchCommitReport {
        ref_name: canonical_ref,
        patch_id,
        wal_sequence,
        operation_count,
        referenced_blob_count,
        text_edit_count,
        changes: summaries,
    })
}

/// True if the baseline block carries a snapshot blob reference. Used to reject a snapshot-only
/// baseline (review E3) while still allowing a genuinely empty node repo to create its first file.
fn baseline_block_has_snapshot_ref(
    object_store: &impl ObjectReader,
    baseline_block: ObjectId,
) -> std::result::Result<bool, AuthorError> {
    let envelope = object_store
        .read_typed(baseline_block, ObjectType::Block)
        .map_err(AuthorError::Store)?
        .ok_or_else(|| {
            AuthorError::Store(PrikkError::Integrity(format!(
                "baseline Block {baseline_block} is missing"
            )))
        })?;
    let block = prikk_object::BlockPayload::decode_canonical(&envelope.canonical_payload)
        .map_err(AuthorError::Store)?;
    Ok(block.snapshot_blob_ref.is_some())
}

/// Classify a *new* file's blob/node kind by UTF-8 validity (existing nodes are never reclassified).
fn classify_new(bytes: &[u8]) -> (BlobKind, NodeKind) {
    if std::str::from_utf8(bytes).is_ok() {
        (BlobKind::Text, NodeKind::TextFile)
    } else {
        (BlobKind::Binary, NodeKind::BinaryFile)
    }
}

/// Plan an arbitrary-span `EditText` for a modified existing `TextFile`, with all span identity computed
/// through the shared `text_span` module (no authoring-local span logic).
fn plan_edit_text(
    object_store: &impl ObjectReader,
    base: &BaselineFile,
    new_bytes: &[u8],
    path: &str,
    lineage_baseline_block_id: Option<ObjectId>,
    lineage_horizon_id: Option<ObjectId>,
    queue_text_cache: &crate::lifecycle_cache::replay::TextCache,
) -> std::result::Result<PlannedOp, AuthorError> {
    let old_text = current_text_for_node(
        object_store,
        base,
        path,
        lineage_baseline_block_id,
        lineage_horizon_id,
        queue_text_cache,
    )?;
    let span = text_span::plan_authored_text_span(&old_text, new_bytes, base.node_id)
        .map_err(|err| AuthorError::Store(PrikkError::Integrity(format!("EditText: {err}"))))?
        .ok_or_else(|| {
            AuthorError::Store(PrikkError::Integrity(
                "EditText requested for unchanged text".to_string(),
            ))
        })?;
    Ok(PlannedOp {
        kind: OperationKind::EditText(EditText {
            node_id: base.node_id,
            span_id: span.span_id,
            old_span_hash: span.old_span_hash,
            left_anchor_hash: span.left_anchor_hash,
            right_anchor_hash: span.right_anchor_hash,
            replacement_text: span.replacement_text,
            presentation_hint_line: None,
            presentation_hint_column: None,
            old_span_text: span.old_span_text,
            left_anchor_len: Some(span.left_anchor_len),
            right_anchor_len: Some(span.right_anchor_len),
        }),
        path: path.to_string(),
        node_id: base.node_id,
        summary_kind: WorktreePatchOperationKind::EditText,
        blob_refs: 0,
    })
}
fn plan_replace_binary(
    object_store: &mut impl ObjectWriter,
    base: &BaselineFile,
    new_bytes: &[u8],
    path: &str,
) -> std::result::Result<PlannedOp, AuthorError> {
    let new_blob_id = write_content_blob(object_store, BlobKind::Binary, new_bytes)?;
    Ok(PlannedOp {
        kind: OperationKind::ReplaceBinary(ReplaceBinary {
            node_id: base.node_id,
            old_blob_id: base.blob_id,
            new_blob_id,
        }),
        path: path.to_string(),
        node_id: base.node_id,
        summary_kind: WorktreePatchOperationKind::ReplaceBinary,
        blob_refs: 2,
    })
}

/// Decide whether an existing regular file node's worktree mode changed (4.4a-2b), and plan a
/// `ChangePerm` if so. The worktree mode is normalized through the same `normalize_file_mode` rule
/// as `CreateFile` (N2).
///
/// `observed_mode` is `None` on a platform with no observable POSIX mode (DC-87 §3.3/§4.3).
/// Declining to compare there is deliberate: it is choosing not to detect a signal the platform
/// structurally cannot produce, not claiming to have set something and not doing it (contrast with
/// `set_permission_bits`'s "returns `Ok` is not evidence" standard, which governs a different claim).
/// The existing recorded mode (`base.mode`) carries forward untouched — see `mode_change_tests`.
fn plan_mode_change_if_observed(
    base: &BaselineFile,
    observed_mode: Option<u32>,
    path: &str,
) -> Option<PlannedOp> {
    let observed_mode = observed_mode?;
    if observed_mode == base.mode {
        return None;
    }
    Some(plan_change_perm(base, observed_mode, path))
}

/// Plan a `ChangePerm` for an existing regular file node whose normalized worktree mode differs from
/// its replay-derived baseline mode (4.4a-2b). `old_mode` is the baseline mode; `new_mode` is the
/// normalized worktree mode.
fn plan_change_perm(base: &BaselineFile, new_mode: u32, path: &str) -> PlannedOp {
    PlannedOp {
        kind: OperationKind::ChangePerm(ChangePerm {
            node_id: base.node_id,
            old_mode: base.mode,
            new_mode,
        }),
        path: path.to_string(),
        node_id: base.node_id,
        summary_kind: WorktreePatchOperationKind::ChangePerm,
        blob_refs: 0,
    }
}

/// Plan a `DeleteNode` for a baseline file absent from the worktree.
fn plan_delete(base: &BaselineFile, path: &str) -> PlannedOp {
    PlannedOp {
        kind: OperationKind::DeleteNode(DeleteNode {
            path: path.to_string(),
            node_id: base.node_id,
            old_node_kind: base.kind,
            preimage: DeleteNodePreimage::File {
                old_blob_id: base.blob_id,
                old_mode: base.mode,
            },
        }),
        path: path.to_string(),
        node_id: base.node_id,
        summary_kind: WorktreePatchOperationKind::DeleteFile,
        blob_refs: 0,
    }
}

/// An existing node's resolved current content hash (DC-56). `bytes` is `Some` only when a real
/// read happened to produce it — a cache hit that still matches the baseline (the common,
/// unchanged case) never populates it.
struct ExistingFileResolution {
    content_hash: ObjectId,
    bytes: Option<Vec<u8>>,
}

/// Resolve an existing node's current content hash against the commit-index cache, reading the file
/// only when the cache cannot vouch for it unread. See the cache-validity specification for the
/// trust condition this implements (`CommitIndexEntry::matches_stat`).
fn resolve_existing_file(
    layout: &RepositoryLayout,
    commit_index: &mut CommitIndex,
    path: &str,
    meta: &WorktreeFileMeta,
    base_mode: u32,
    blob_kind: BlobKind,
) -> std::result::Result<ExistingFileResolution, AuthorError> {
    let stat = RootFileStat {
        size: meta.size,
        mtime_secs: meta.mtime_secs,
        mtime_nanos: meta.mtime_nanos,
        mode: meta.mode,
    };
    if let Some(cached) = commit_index.get(path) {
        if cached.kind == blob_kind && cached.matches_stat(&stat) {
            return Ok(ExistingFileResolution {
                content_hash: cached.content_hash,
                bytes: None,
            });
        }
    }
    let bytes = read_existing_file_bytes(layout, path, blob_kind)?;
    let content_hash = commit_index::content_hash(blob_kind, &bytes).map_err(AuthorError::Store)?;
    // `meta.mode` is `None` on a platform with no observable POSIX mode (DC-87 §3.3/§4.3); the cache
    // entry records whatever mode was actually resolved for this node — `base_mode`, carried forward
    // untouched — rather than a raw `Option`, since `CommitIndexEntry.mode` is plain bookkeeping,
    // never part of `matches_stat`'s trust condition.
    commit_index.record(
        path.to_string(),
        CommitIndexEntry {
            size: meta.size,
            mtime_secs: meta.mtime_secs,
            mtime_nanos: meta.mtime_nanos,
            mode: meta.mode.unwrap_or(base_mode),
            kind: blob_kind,
            content_hash,
        },
    );
    Ok(ExistingFileResolution {
        content_hash,
        bytes: Some(bytes),
    })
}

/// Read an existing node's current worktree bytes, enforcing the same rule the content comparison
/// always assumed: an existing `TextFile` node must remain valid UTF-8 (E4, existing-node kind is
/// authoritative, never reclassified).
fn read_existing_file_bytes(
    layout: &RepositoryLayout,
    path: &str,
    blob_kind: BlobKind,
) -> std::result::Result<Vec<u8>, AuthorError> {
    let bytes = read_worktree_file_bytes(layout, path)?;
    if matches!(blob_kind, BlobKind::Text) && std::str::from_utf8(&bytes).is_err() {
        return Err(AuthorError::UnsupportedKindTransition(format!(
            "{path}: existing TextFile cannot accept non-UTF-8 content"
        )));
    }
    Ok(bytes)
}

/// Read a worktree regular file's current bytes.
fn read_worktree_file_bytes(
    layout: &RepositoryLayout,
    path: &str,
) -> std::result::Result<Vec<u8>, AuthorError> {
    read_file_if_exists(layout.worktree_mutation_root(), Path::new(path))
        .map_err(AuthorError::Store)?
        .ok_or_else(|| {
            AuthorError::Store(PrikkError::Io {
                kind: None,
                context: format!("worktree entry disappeared: {path}"),
            })
        })
}

fn write_content_blob(
    object_store: &mut impl ObjectWriter,
    kind: BlobKind,
    bytes: &[u8],
) -> std::result::Result<ObjectId, AuthorError> {
    let payload = BlobPayload::new(kind, bytes.to_vec());
    let canonical = payload.to_canonical_bytes().map_err(AuthorError::Store)?;
    let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, canonical);
    // RFC 111 §6.1 Stage 2: no longer clones the store to get a mutable local (`FileObjectStore`
    // tolerated that -- stateless, so a clone was harmless -- but cloning `ObjectWriteSession` would
    // fork its in-memory snapshot, silently discarding this write from the caller's own copy). Takes
    // `object_store` mutably instead, threaded all the way up to where the session is opened.
    object_store
        .write_object(&envelope)
        .map_err(AuthorError::Store)
}

/// Read a text node's baseline content from a directly-stored `Blob`, when one exists. `Ok(None)`
/// means no such object is stored — expected whenever the node's most recent operation was an
/// `EditText` (DC-65: its `blob_id` is a content identity, never a stored object), not an error by
/// itself; the caller falls back to replay-based materialization.
fn read_file_blob_bytes_if_present(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> std::result::Result<Option<Vec<u8>>, AuthorError> {
    let Some(envelope) = object_store
        .read_object(blob_id)
        .map_err(AuthorError::Store)?
    else {
        return Ok(None);
    };
    crate::blob_access::decode_file_content_blob(&envelope.canonical_payload)
        .map_err(AuthorError::Store)
        .map(Some)
}

/// Materialize a `TextFile` node's current baseline bytes (DC-65, extended by DC-66). Checks the
/// queue's own text cache first — a node created or edited earlier in the same not-yet-sealed queue
/// has its true current text there, not in a stored blob or in sealed lineage (see
/// `rfcs/handoffs/DC-66-multi-commit-queuing/queuing-baseline-design-v1.md`). Empty when no queue
/// exists, so this check always misses and behaviour is unchanged from before DC-66. Otherwise tries
/// the stored blob — correct and cheap for a node that has never been edited, since `CreateFile`
/// always writes a real `Blob`. Falling through to replay-based materialization is expected, not
/// exceptional, for any node whose most recent *sealed* operation was an `EditText`; see the invariant
/// document at `rfcs/handoffs/DC-65-text-edit-baseline-content/prerequisite-questions-v1.md`.
fn current_text_for_node(
    object_store: &impl ObjectReader,
    base: &BaselineFile,
    path: &str,
    lineage_baseline_block_id: Option<ObjectId>,
    lineage_horizon_id: Option<ObjectId>,
    queue_text_cache: &crate::lifecycle_cache::replay::TextCache,
) -> std::result::Result<Vec<u8>, AuthorError> {
    if let Some(text) = queue_text_cache.get(&base.node_id) {
        return Ok(text.clone());
    }
    if let Some(bytes) = read_file_blob_bytes_if_present(object_store, base.blob_id)? {
        return Ok(bytes);
    }
    let (baseline_block_id, horizon_id) = match (lineage_baseline_block_id, lineage_horizon_id) {
        (Some(baseline_block_id), Some(horizon_id)) => (baseline_block_id, horizon_id),
        _ => {
            return Err(AuthorError::Store(PrikkError::Integrity(format!(
                "{path}: text node baseline blob {} is missing and no lineage is available to \
                 materialize it (an existing node implies a published baseline)",
                base.blob_id
            ))));
        }
    };
    crate::lifecycle_cache::materialize_edited_text(
        object_store,
        baseline_block_id,
        horizon_id,
        base.node_id,
    )
    .map_err(AuthorError::Store)?
    .ok_or_else(|| {
        AuthorError::Store(PrikkError::Integrity(format!(
            "{path}: text node baseline blob {} is missing and could not be materialized from \
             its edit history",
            base.blob_id
        )))
    })
}

/// DC-87 §3.3/§4.3: an unobservable worktree mode must never be mistaken for a real, unchanged one,
/// and must never fire `ChangePerm` — exercised directly here (not through a real filesystem walk)
/// so this protection is verified on every platform's CI, not only a future Windows job's.
#[cfg(test)]
mod mode_change_tests {
    #![allow(clippy::expect_used)]

    use prikk_object::{NodeId, NodeKind, ObjectId, ObjectType};

    use super::{
        BaselineFile, EXECUTABLE_FILE_MODE, REGULAR_FILE_MODE, plan_mode_change_if_observed,
    };

    fn sample_baseline(mode: u32) -> BaselineFile {
        BaselineFile {
            node_id: NodeId::from_bytes([0x42; 32]),
            kind: NodeKind::TextFile,
            blob_id: ObjectId::from_canonical_payload(ObjectType::Blob, 1, b"mode-change-tests"),
            mode,
        }
    }

    #[test]
    fn unobserved_mode_never_plans_a_change_perm_even_when_baseline_differs() {
        // The platform cannot observe a mode at all (DC-87 §3.3/§4.3) — the baseline is executable,
        // so a real `REGULAR_FILE_MODE` observation would have fired `ChangePerm`; `None` must not.
        let base = sample_baseline(EXECUTABLE_FILE_MODE);
        assert!(plan_mode_change_if_observed(&base, None, "bin/tool").is_none());
    }

    #[test]
    fn unobserved_mode_never_plans_a_change_perm_when_baseline_is_regular() {
        let base = sample_baseline(REGULAR_FILE_MODE);
        assert!(plan_mode_change_if_observed(&base, None, "src/lib.rs").is_none());
    }

    #[test]
    fn observed_mode_matching_baseline_plans_no_change() {
        let base = sample_baseline(REGULAR_FILE_MODE);
        assert!(
            plan_mode_change_if_observed(&base, Some(REGULAR_FILE_MODE), "src/lib.rs").is_none()
        );
    }

    #[test]
    fn observed_mode_differing_from_baseline_plans_a_change_perm() {
        let base = sample_baseline(REGULAR_FILE_MODE);
        let op = plan_mode_change_if_observed(&base, Some(EXECUTABLE_FILE_MODE), "bin/tool")
            .expect("a real observed change must be planned");
        assert_eq!(op.path, "bin/tool");
        assert_eq!(op.node_id, base.node_id);
    }
}
