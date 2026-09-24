//! Minimal patch replay planning for supported file-level operations.
//!
//! PR-024 keeps a deliberately narrow replay boundary. It can reconstruct an in-memory snapshot
//! manifest by walking a single-parent block chain and applying `CreateFile`, `DeleteNode`,
//! `EditText`, `ReplaceBinary`, `ChangePerm`, and — RFC 144 increment 1 — `RenamePath` operations
//! (DC-73 wired the middle three; see `apply.rs` and `decode.rs::ensure_apply_supported`).
//! Consecutive `RenamePath` operations within one patch are resolved together, node before path,
//! by `apply::apply_rename_batch` rather than one at a time (RFC 144 §4h.7) — see that function's
//! own doc comment for why. Renames still have no authoring path (`node_authoring.rs` never
//! produces `RenamePath`; that is increment 3, gated on this one landing correctly) and symlink
//! authoring is refused outright, so symlink apply stays deferred pending an authoring path, not
//! the node model.
//!
//! **Split across four modules** (DC-58, RFC 144 §4m): this file keeps the public API and baseline
//! resolution; `read.rs` holds object-store reading helpers (block-chain walking, blob/patch/
//! snapshot loading); `apply.rs` holds the per-operation state-fold logic; `preview.rs` (RFC 144
//! §4m) reuses both to answer *would applying a bundle's own new operations onto a ref's current
//! state conflict* -- a narrower question than `patch_algebra`'s own confluence/witness machinery
//! answers (see `preview.rs`'s own doc comment for why that narrower question is the right one for
//! a bundle preview), not full merge algebra, which remains `patch_algebra`'s and later
//! increments' own scope. `decode.rs` (pre-existing) is unchanged.

use std::collections::BTreeMap;

pub(crate) mod anchor;
pub(crate) mod apply;
pub(crate) mod decode;
pub(crate) mod read;

pub use anchor::SnapshotAnchorFallback;

use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, NodeKind, ObjectId};

use crate::foundation::layout::RepositoryLayout;
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::path::RepoPath;
use crate::point::Point;
use crate::refs::RefStore;
use crate::validate_local_branch_ref;
use crate::wal::WalReplay;

use apply::{apply_decoded_operation, apply_rename_batch};
use decode::DecodedOperationKind;
use read::{files_to_replay_manifest, single_parent_chain};

/// Read-only result of replaying supported patch operations to an in-memory snapshot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchReplayPlan {
    /// Ref used as the checkout target.
    pub ref_name: String,
    /// Target block ID.
    pub target_block_id: ObjectId,
    /// Number of blocks replayed from oldest to newest.
    pub block_count: usize,
    /// Number of patch objects replayed.
    pub patch_count: usize,
    /// Number of supported operations applied.
    pub applied_operation_count: usize,
    /// Number of files in the resulting manifest.
    pub file_count: usize,
    /// Total content bytes in the resulting manifest.
    pub total_content_bytes: u64,
    /// Repository-relative paths in the resulting manifest.
    pub paths: Vec<String>,
}

/// Replay the supported operation subset for a ref without writing the worktree.
pub fn prepare_patch_replay_plan(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<PatchReplayPlan> {
    Ok(prepare_patch_replay_plan_reporting_anchor(layout, ref_name)?.0)
}

/// [`prepare_patch_replay_plan`], plus the snapshot this read-only report could not anchor at, if any
/// (RFC 136 §10.3b.4): the plan is unchanged either way, and a caller prints the fallback on stderr.
pub fn prepare_patch_replay_plan_reporting_anchor(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(PatchReplayPlan, Option<SnapshotAnchorFallback>)> {
    let (snapshot, fallback) = replay_for_read_only_report(layout, ref_name)?;
    Ok((patch_replay_plan_from(snapshot), fallback))
}

/// [`prepare_patch_replay_plan_reporting_anchor`] at a resolved [`Point`] -- a ref, or a bare block id
/// (RFC 153 §7.1). `ref_name` in the plan is the point's name as given.
///
/// # Errors
///
/// The replay fails.
pub fn prepare_patch_replay_plan_at_point_reporting_anchor(
    layout: &RepositoryLayout,
    point: &Point,
) -> Result<(PatchReplayPlan, Option<SnapshotAnchorFallback>)> {
    let (snapshot, fallback) = replay_point_for_read_only_report(layout, point)?;
    Ok((patch_replay_plan_from(snapshot), fallback))
}

fn patch_replay_plan_from(snapshot: PatchReplaySnapshot) -> PatchReplayPlan {
    let paths = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| entry.path.as_str().to_string())
        .collect();
    PatchReplayPlan {
        ref_name: snapshot.ref_name,
        target_block_id: snapshot.target_block_id,
        block_count: snapshot.block_count,
        patch_count: snapshot.patch_count,
        applied_operation_count: snapshot.applied_operation_count,
        file_count: snapshot.manifest.files.len(),
        total_content_bytes: snapshot.manifest.total_content_bytes(),
        paths,
    }
}

/// RFC 143 §6: the replay's own structural coverage, exposed so a consumer can tell "the complete
/// state at this point" from "what the supported subset reconstructs" without parsing prose.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanCoverage {
    /// Operation kinds actually applied while reconstructing this point, in stable
    /// (`ShowOperation::kind`-style: `"create-file"`, `"edit-text"`, ...) form -- a subset of the
    /// five kinds this replay mechanism knows how to apply at all. Never includes `"rename-path"`
    /// or `"create-symlink"`: those, and a symlink `"delete-node"`, are refused outright
    /// (`ensure_apply_supported`) rather than silently applied and omitted, so a *successful*
    /// response is never a partial one that went unreported (RFC 143 §6b: the unsupported-kind
    /// case is an `Err`, propagated by `prepare_patch_plan_content_report` below, not folded into
    /// this field).
    pub applied_operation_kinds: Vec<&'static str>,
    /// Always `"single-parent"`: this replay mechanism walks one parent only at a block with more
    /// than one (a well-formed `Merge` block's own mainline, never the other sides). A future
    /// merge-aware replay would report something else here; today there is only one value.
    pub walk: &'static str,
}

/// One path's content, RFC 143 §5: binary is never rendered, exactly as `show` (RFC 142 §7)
/// already refuses to render it -- one project, one answer to that question.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchPlanContent {
    /// A text file's own bytes, verbatim.
    Text(Vec<u8>),
    /// A binary file: id and declared size only, never content.
    Binary {
        /// The blob backing this file's current content.
        blob_id: ObjectId,
        /// Content length in bytes.
        size: u64,
    },
    /// Content the replayed window cannot classify as text or binary: a path seeded by a snapshot
    /// boundary and never subsequently touched by a node-addressed operation in the replayed
    /// window (see `ReplayManifestEntry`'s own doc for why -- `SnapshotEntry` itself carries no
    /// kind field to fall back on). Real bytes exist and their length is reported; rendering them
    /// as text would risk emitting non-UTF-8 bytes into a JSON string on a guess this crate has no
    /// basis for, and reporting them as binary would claim an identity (`blob_id`) this code does
    /// not actually have for them.
    Opaque {
        /// Content length in bytes.
        size: u64,
    },
}

/// Content at one requested path, at the point [`PatchPlanContentReport`] resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanContentEntry {
    /// Repository-relative path, exactly as requested.
    pub path: String,
    /// Mode bits.
    pub mode: u32,
    /// The content itself.
    pub content: PatchPlanContent,
}

/// RFC 143 §5: content at a replayed point, for exactly the requested paths -- never the whole
/// tree by default. An empty `requested_paths` yields metadata only: `entries` and `not_found` are
/// both empty, `coverage` is still populated. Deliberately not `ReplayManifest`/
/// `ReplayManifestEntry` widened to `pub`: RFC 131 applies, and this narrow, purpose-built type is
/// the minimum this feature needs -- the `show` round's own restraint (one narrow function,
/// nothing else widened) is the standard matched here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchPlanContentReport {
    /// Ref used as the checkout target.
    pub ref_name: String,
    /// Target block ID.
    pub target_block_id: ObjectId,
    /// The replay's own structural coverage.
    pub coverage: PatchPlanCoverage,
    /// Content for every requested path found in the resulting manifest, in the order requested.
    pub entries: Vec<PatchPlanContentEntry>,
    /// Requested paths absent from the resulting manifest -- never existed, or removed by this
    /// point in history. Degraded, not an error: absence is the RFC 140 §7b / RFC 142 §6b case,
    /// distinguished from a real error, which still propagates via this function's own `Result`.
    pub not_found: Vec<String>,
}

/// Replay the supported operation subset for a ref and report content at exactly the requested
/// paths (RFC 143 §5) -- read-only, same as [`prepare_patch_replay_plan`]. An unsupported
/// operation anywhere in the walked chain still fails the whole call (`Err`; RFC 143 §6b/§6 names
/// this the third instance of RFC 140 §7b's rule): this never degrades a real error into a
/// partial-but-successful response, only absence (a requested path the manifest does not contain)
/// degrades, into `not_found`.
pub fn prepare_patch_plan_content_report(
    layout: &RepositoryLayout,
    ref_name: &str,
    requested_paths: &[String],
) -> Result<PatchPlanContentReport> {
    Ok(prepare_patch_plan_content_report_reporting_anchor(layout, ref_name, requested_paths)?.0)
}

/// [`prepare_patch_plan_content_report`], plus the snapshot this read-only report could not anchor at,
/// if any (RFC 136 §10.3b.4).
pub fn prepare_patch_plan_content_report_reporting_anchor(
    layout: &RepositoryLayout,
    ref_name: &str,
    requested_paths: &[String],
) -> Result<(PatchPlanContentReport, Option<SnapshotAnchorFallback>)> {
    let (snapshot, fallback) = replay_for_read_only_report(layout, ref_name)?;
    Ok((content_report_from(snapshot, requested_paths), fallback))
}

/// [`prepare_patch_plan_content_report_reporting_anchor`] at a resolved [`Point`] -- a ref, or a bare
/// block id (RFC 153 §7.1). `ref_name` in the report is the point's name as given.
///
/// # Errors
///
/// The replay fails, including on an unsupported operation anywhere in the walked chain.
pub fn prepare_patch_plan_content_report_at_point_reporting_anchor(
    layout: &RepositoryLayout,
    point: &Point,
    requested_paths: &[String],
) -> Result<(PatchPlanContentReport, Option<SnapshotAnchorFallback>)> {
    let (snapshot, fallback) = replay_point_for_read_only_report(layout, point)?;
    Ok((content_report_from(snapshot, requested_paths), fallback))
}

/// How a replayed entry's content is classified: **the one classification** `checkout --patch-plan`'s
/// content report and RFC 157's `tree`/`cat` share, so they cannot disagree on whether a file is text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ManifestEncoding {
    /// A text node: its bytes are the reconstructed content.
    Text,
    /// A binary node, with the blob that stores its current content.
    Binary(ObjectId),
    /// Neither: a kind replay did not track. Nothing produces it since RFC 136 §10.1a (see
    /// [`ReplayManifestEntry::kind`]).
    Opaque,
}

/// Classify one replayed entry (RFC 143 §5; RFC 157 §3's `encoding`).
pub(crate) fn classify_manifest_entry(entry: &ReplayManifestEntry) -> ManifestEncoding {
    match (entry.kind, entry.blob_id) {
        (Some(NodeKind::BinaryFile), Some(blob_id)) => ManifestEncoding::Binary(blob_id),
        (Some(NodeKind::TextFile), _) => ManifestEncoding::Text,
        _ => ManifestEncoding::Opaque,
    }
}

fn content_report_from(
    snapshot: PatchReplaySnapshot,
    requested_paths: &[String],
) -> PatchPlanContentReport {
    let by_path: BTreeMap<&str, &ReplayManifestEntry> = snapshot
        .manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    let mut entries = Vec::with_capacity(requested_paths.len());
    let mut not_found = Vec::new();
    for requested in requested_paths {
        match by_path.get(requested.as_str()) {
            Some(entry) => {
                let size = entry.bytes.len() as u64;
                let content = match classify_manifest_entry(entry) {
                    ManifestEncoding::Binary(blob_id) => PatchPlanContent::Binary { blob_id, size },
                    ManifestEncoding::Text => PatchPlanContent::Text(entry.bytes.clone()),
                    ManifestEncoding::Opaque => PatchPlanContent::Opaque { size },
                };
                entries.push(PatchPlanContentEntry {
                    path: requested.clone(),
                    mode: entry.mode,
                    content,
                });
            }
            None => not_found.push(requested.clone()),
        }
    }
    PatchPlanContentReport {
        ref_name: snapshot.ref_name,
        target_block_id: snapshot.target_block_id,
        coverage: PatchPlanCoverage {
            applied_operation_kinds: snapshot.applied_operation_kinds.into_iter().collect(),
            walk: "single-parent",
        },
        entries,
        not_found,
    }
}

/// One file entry in a replay-derived manifest, carrying the mode bits `CreateFile`/`ChangePerm`
/// recorded (DC-73) and the node kind replay tracked. It exists only in memory, built by replaying
/// operations or by seeding from a snapshot's state entries (RFC 136 §10.1a), and never crosses the
/// object-format boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct ReplayManifestEntry {
    /// The node this path is, which **survives a rename**: the same node at a different path is a
    /// declared `RenamePath`, and nothing else moves a node. `prikk diff` pairs a deleted path with an added
    /// one by this id -- never by similarity of content (RFC 153 §3). Always `Some` in a replayed manifest;
    /// `None` only for a **worktree** file no commit has authored yet, which has no node.
    pub(crate) node_id: Option<NodeId>,
    /// Validated repository-relative path.
    pub(crate) path: RepoPath,
    /// File content bytes.
    pub(crate) bytes: Vec<u8>,
    /// Mode bits, as recorded by the most recent `CreateFile`/`ChangePerm` for this node.
    pub(crate) mode: u32,
    /// This entry's node kind. **Always `Some`** since RFC 136 §10.1a: a snapshot seeds live nodes
    /// with their real kinds, so every path in a replay manifest has one, and a path without one is
    /// refused as `Integrity` (`read::files_to_replay_manifest`). The field stays an `Option` because
    /// RFC 143's `patch-plan-content-v1` report keeps its `opaque` state; nothing produces it now.
    /// Never `Symlink`: a symlink node never enters `files`/`live_nodes` at all, since both
    /// `DeleteNode(symlink)` and `CreateSymlink` are refused by `ensure_apply_supported` before
    /// application, and a snapshot holding one is refused by `snapshot::load_block_snapshot`.
    pub(crate) kind: Option<NodeKind>,
    /// The blob id backing this entry's *current* content -- `Some` only when `kind` is
    /// `Some(NodeKind::BinaryFile)` (see [`super::apply::ReplayLiveNode`]'s own doc for why a
    /// text node's tracked id would not be trustworthy here even if carried through).
    pub(crate) blob_id: Option<ObjectId>,
}

/// Replay-derived manifest, sorted by path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct ReplayManifest {
    /// File entries, sorted by path.
    pub(crate) files: Vec<ReplayManifestEntry>,
}

impl ReplayManifest {
    pub(crate) fn total_content_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|entry| entry.bytes.len() as u64)
            .sum()
    }
}

/// In-memory replay result used by patch checkout materialization.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct PatchReplaySnapshot {
    /// Ref used as the replay target.
    pub(crate) ref_name: String,
    /// Target block ID.
    pub(crate) target_block_id: ObjectId,
    /// Number of blocks replayed from oldest to newest.
    pub(crate) block_count: usize,
    /// Number of patch objects replayed.
    pub(crate) patch_count: usize,
    /// Number of supported operations applied.
    pub(crate) applied_operation_count: usize,
    /// Distinct kinds of operation actually applied in this replay window (RFC 143 §6) -- a
    /// subset of the five `ensure_apply_supported` admits, since a given repository's own history
    /// may never exercise all five.
    pub(crate) applied_operation_kinds: std::collections::BTreeSet<&'static str>,
    /// Resulting file manifest, mode-aware (DC-73).
    pub(crate) manifest: ReplayManifest,
    /// Files explicitly removed by replayed patches and still absent in the final manifest.
    pub(crate) deleted_files: Vec<PatchReplayDeletedFile>,
    /// Blocks applied operation by operation, after any anchor (RFC 136).
    pub(crate) replayed_block_count: usize,
}

/// A file explicitly deleted while replaying the supported patch subset.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) struct PatchReplayDeletedFile {
    /// Validated repository-relative path that was removed.
    pub(crate) path: RepoPath,
    /// Blob ID recorded as the delete precondition.
    pub(crate) old_blob_id: ObjectId,
    /// The deleted node's kind, which with `old_blob_id` names the exact bytes an opt-in destructive
    /// delete requires the worktree file to still hold. Recorded instead of the bytes themselves so a
    /// read-only report can recover it by decoding history (RFC 136 increment 2a): an edited text's
    /// post-edit content id is never stored (DC-65), but it is in the `DeleteNode` record.
    pub(crate) old_node_kind: NodeKind,
}

/// Apply a decoded operation sequence -- already concatenated across whatever patch(es) produced
/// it -- onto existing `files`/`live_nodes`/`deleted_files` state, using the exact per-operation
/// dispatch `replay_supported_patch_chain`'s own loop uses: consecutive `RenamePath` operations
/// batched into one run (RFC 144 §4h.7/§4j; see `apply::apply_rename_batch`'s own doc comment),
/// everything else applied one at a time via `apply_decoded_operation`. Returns the count and the
/// distinct applied-kind labels, matching the bookkeeping the chain-walking loop keeps.
///
/// Extracted as its own reusable primitive (not merely inlined in the loop below) because the
/// bundle-impact preview needs the identical dispatch applied to a *different* operation source: a
/// bundle's own candidate operations, applied onto a copy of the local ref's current replayed
/// state rather than operations read patch-by-patch while walking a block chain -- "one replay of
/// current state, plus the bundle's own patches applied in memory," in the design's own words.
pub(crate) fn apply_operation_sequence(
    object_store: &impl ObjectReader,
    files: &mut BTreeMap<String, Vec<u8>>,
    live_nodes: &mut BTreeMap<NodeId, apply::ReplayLiveNode>,
    deleted_files: &mut BTreeMap<String, PatchReplayDeletedFile>,
    capture: &mut apply::DeletedContentCapture,
    operations: Vec<decode::DecodedPatchOperation>,
) -> Result<(usize, std::collections::BTreeSet<&'static str>)> {
    let mut applied_operation_count = 0_usize;
    let mut applied_operation_kinds = std::collections::BTreeSet::new();
    let mut operations = operations.into_iter().peekable();
    while let Some(first) = operations.next() {
        if matches!(first.kind, DecodedOperationKind::RenamePath { .. }) {
            let mut run = vec![first];
            while let Some(next) = operations
                .next_if(|next| matches!(next.kind, DecodedOperationKind::RenamePath { .. }))
            {
                run.push(next);
            }
            // Captured before the run is consumed below, and only kept if the batch apply that
            // follows actually succeeds -- same rule as the single-operation path below.
            let run_len = run.len();
            apply_rename_batch(files, live_nodes, run)?;
            applied_operation_kinds.insert("rename-path");
            applied_operation_count += run_len;
        } else {
            // Captured before the operation is consumed below, and only kept if the apply that
            // follows actually succeeds -- a kind `ensure_apply_supported` admits can still fail
            // apply's own further validation (a real Integrity error), and that must propagate
            // rather than being recorded as "covered".
            let kind_label = decode::applied_operation_kind_label(&first.kind);
            apply_decoded_operation(
                object_store,
                files,
                live_nodes,
                deleted_files,
                capture,
                first,
            )?;
            applied_operation_kinds.insert(kind_label);
            applied_operation_count += 1;
        }
    }
    Ok((applied_operation_count, applied_operation_kinds))
}

/// Replay the supported operation subset into a validated in-memory manifest, from genesis. Rollback
/// preview uses this: a snapshot never anchors it (RFC 136 §10.3a ruling 3).
pub(crate) fn replay_supported_patch_chain(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<PatchReplaySnapshot> {
    Ok(replay_ref_chain(layout, ref_name, anchor::Anchoring::Never)?.0)
}

/// DC-78 v2: the content of the deletions whose preimage Blobs `wanted` names, derived by replay.
///
/// **Why anyone needs this.** A text file whose content only ever arrived as `EditText` spans has no
/// stored content Blob (DC-65), so when it is deleted the deletion's preimage names an id nothing
/// holds. Exporters must still carry that Blob -- every released prikk requires it -- and the
/// rollback seal must store it, because the inverse of the deletion is a `CreateFile` naming it.
///
/// **One derivation, two callers** (the v2 ruling): replay already materializes exactly these bytes,
/// and [`anchor::replay_chain_capturing`] records them at the moment it applies the deletion. Replay's
/// own `ensure_blob_matches_node_kind` has already checked that they hash to the id the patch names,
/// so a mismatch is an `Integrity` error from the replay itself, never a silently wrong Blob.
///
/// **Anchored first** (RFC 136): the anchored pass starts at the nearest valid snapshot, which is
/// enough whenever the deletion is after it. Anything still wanted is then derived by a full replay,
/// because an anchor's skipped prefix never materializes its bytes. Content from a snapshot is safe
/// here even though a snapshot is not replay-verified: the id check above is what makes it so.
///
/// **A merge history does derive**, and both shapes are measured
/// (`dc78_export_after_deleting_an_edited_file.rs`). `single_parent_chain` follows a `Merge` block's
/// mainline parent, so a mainline deletion is reached like any other; a deletion that happened only on
/// the merged-in side is reached too, because the merge block adopts those patches and the mainline walk
/// applies them. What remains unreachable is a block shape this walk cannot follow at all — a non-`Merge`
/// block with several parents, or a `Merge` with no valid mainline — reported as `unsupported_chain`
/// rather than silently empty.
///
/// **A chain failure is not "nothing to derive"** (review v2, finding 2): only an unsupported *shape*
/// falls back to an empty result; a cycle or an unreadable block propagates, so corruption is never
/// reported as a missing Blob.
pub(crate) fn derive_deleted_content(
    reader: &impl ObjectReader,
    tip_block_id: ObjectId,
    wanted: &std::collections::BTreeSet<ObjectId>,
) -> Result<DerivedDeletedContent> {
    if wanted.is_empty() {
        return Ok(DerivedDeletedContent::default());
    }
    let chain = match read::single_parent_chain(reader, tip_block_id) {
        Ok(chain) => chain,
        // The one shape this walk legitimately cannot follow. Reported, not swallowed: the caller says
        // it in the refusal, so a user reads "this shape was not walked" rather than only a blob id.
        Err(PrikkError::UnsupportedObjectType(detail)) => {
            return Ok(DerivedDeletedContent {
                found: BTreeMap::new(),
                unsupported_chain: Some(detail),
            });
        }
        Err(err) => return Err(err),
    };
    let anchored = anchor::replay_chain_capturing(
        reader,
        &chain,
        anchor::Anchoring::ReadOnlyReport,
        apply::DeletedContentCapture::for_ids(wanted.clone()),
    )?;
    if anchored.capture.is_satisfied() {
        return Ok(DerivedDeletedContent {
            found: anchored.capture.into_found(),
            unsupported_chain: None,
        });
    }
    let mut found = anchored.capture.into_found();
    let remaining: std::collections::BTreeSet<ObjectId> = wanted
        .iter()
        .copied()
        .filter(|id| !found.contains_key(id))
        .collect();
    let full = anchor::replay_chain_capturing(
        reader,
        &chain,
        anchor::Anchoring::Never,
        apply::DeletedContentCapture::for_ids(remaining),
    )?;
    found.extend(full.capture.into_found());
    Ok(DerivedDeletedContent {
        found,
        unsupported_chain: None,
    })
}

/// What [`derive_deleted_content`] produced: the content it derived, and the block-chain shape it could
/// not walk, if that is why something is missing.
#[derive(Debug, Default)]
pub(crate) struct DerivedDeletedContent {
    pub(crate) found: BTreeMap<ObjectId, (prikk_object::NodeKind, Vec<u8>)>,
    pub(crate) unsupported_chain: Option<String>,
}

/// The same replay for a **read-only report**, which may start at the nearest snapshot that passes the
/// loader (RFC 136 §10.3c ruling 1). History fields still cover the whole chain (`anchor.rs`). Never
/// call this from a path that writes the worktree or history.
pub(crate) fn replay_for_read_only_report(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(PatchReplaySnapshot, Option<SnapshotAnchorFallback>)> {
    replay_ref_chain(layout, ref_name, anchor::Anchoring::ReadOnlyReport)
}

/// [`replay_for_read_only_report`] at a resolved [`Point`]: a ref or a bare block id, through the same
/// [`replay_block_chain`].
pub(crate) fn replay_point_for_read_only_report(
    layout: &RepositoryLayout,
    point: &Point,
) -> Result<(PatchReplaySnapshot, Option<SnapshotAnchorFallback>)> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    replay_block_chain(
        &object_store,
        &point.name,
        point.block_id,
        anchor::Anchoring::ReadOnlyReport,
    )
}

/// **Two points, one read snapshot** (RFC 153 §2: "holds the same read snapshot both replays use"):
/// [`replay_point_for_read_only_report`] for each of `left` and `right`, over a single
/// [`ObjectReadSnapshot`], so both sides are read from one consistent view of the store and the index is
/// decoded once. Each side keeps its own anchoring and its own fallback.
pub(crate) fn replay_two_points_for_read_only_report(
    layout: &RepositoryLayout,
    left: &Point,
    right: &Point,
) -> Result<[(PatchReplaySnapshot, Option<SnapshotAnchorFallback>); 2]> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let replay = |point: &Point| {
        replay_block_chain(
            &object_store,
            &point.name,
            point.block_id,
            anchor::Anchoring::ReadOnlyReport,
        )
    };
    Ok([replay(left)?, replay(right)?])
}

/// Test-support instrument (RFC 153 point-resolver handoff §3.3): how many blocks the read-only replay of
/// `point` applied operation by operation -- the whole chain, or only those after its anchor. The same
/// replay `checkout --patch-plan` runs. Never in a shipped build.
///
/// # Errors
///
/// The replay fails.
#[cfg(feature = "test-support")]
pub fn replayed_block_count_at_point_for_test_support(
    layout: &RepositoryLayout,
    point: &Point,
) -> Result<usize> {
    Ok(replay_point_for_read_only_report(layout, point)?
        .0
        .replayed_block_count)
}

/// The replay for a **worktree write** (RFC 136 §10.3c ruling 2): it may start only at a snapshot whose
/// Block is in this repository's replay-verified record and that passes the loader; otherwise it
/// replays from genesis. A missing or damaged record is the empty set, so the replay runs in full.
pub(crate) fn replay_for_verified_worktree_write(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(PatchReplaySnapshot, Option<SnapshotAnchorFallback>)> {
    let verified = crate::verified_blocks::load_verified_blocks(layout);
    replay_ref_chain(
        layout,
        ref_name,
        anchor::Anchoring::VerifiedWorktreeWrite(&verified),
    )
}

fn replay_ref_chain(
    layout: &RepositoryLayout,
    ref_name: &str,
    anchoring: anchor::Anchoring<'_>,
) -> Result<(PatchReplaySnapshot, Option<SnapshotAnchorFallback>)> {
    // RFC 111 §6.1: safe as a read-only snapshot because every production caller
    // (`patch_checkout.rs`, `rollback_preview.rs`) only reads -- neither ever writes an object.
    // If a future caller reaches this from a writing operation, confirm its own write happens
    // after this function returns (Stage 1 review v1 §4) before assuming this stays safe.
    let object_store = ObjectReadSnapshot::open(layout)?;
    let target_block_id = crate::refs::read_current_ref_tip_block(layout, &object_store, ref_name)?;
    replay_block_chain(&object_store, ref_name, target_block_id, anchoring)
}

/// **The one replay of a point** (RFC 153 point-resolver handoff §2.3): the state at `target_block_id`,
/// by the single-parent walk from it and the same anchoring every caller uses (RFC 136). The ref-addressed
/// [`replay_ref_chain`] reads its ref's tip and calls this; a bare block id comes here directly.
/// `point_name` is carried into the result as given.
fn replay_block_chain(
    object_store: &impl ObjectReader,
    point_name: &str,
    target_block_id: ObjectId,
    anchoring: anchor::Anchoring<'_>,
) -> Result<(PatchReplaySnapshot, Option<SnapshotAnchorFallback>)> {
    let block_ids = single_parent_chain(object_store, target_block_id)?;
    let chain = anchor::replay_chain(object_store, &block_ids, anchoring)?;
    Ok((
        PatchReplaySnapshot {
            ref_name: point_name.to_string(),
            target_block_id,
            block_count: block_ids.len(),
            patch_count: chain.patch_count,
            applied_operation_count: chain.applied_operation_count,
            applied_operation_kinds: chain.applied_operation_kinds,
            manifest: files_to_replay_manifest(chain.files, &chain.live_nodes)?,
            deleted_files: chain.deleted_files.into_values().collect(),
            replayed_block_count: chain.replayed_block_count,
        },
        chain.fallback,
    ))
}

/// Resolve the node-addressed lineage bounds for a ref: the current target block (baseline) and the
/// lineage genesis (horizon). Worktree authoring (4.4a-2) supplies these to `replay_derived_state`
/// so the baseline node lifecycle state is reconstructed from authoritative node-addressed history,
/// never from a snapshot manifest. For a well-formed `Merge` block (DC-75), `single_parent_chain`
/// follows its mainline parent only; it fails closed only on a malformed multi-parent block (a
/// non-`Merge` block with more than one parent, or a `Merge` block with a missing or invalid
/// mainline) — see `read::mainline_or_sole_parent`.
pub(crate) fn resolve_node_lineage_bounds(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(ObjectId, ObjectId)> {
    // RFC 111 §6.1: safe as a read-only snapshot even though this is reached from a writing
    // operation (`node_authoring.rs::author_worktree_patch`, via `resolve_worktree_baseline`) --
    // this function's own read completes and the snapshot is dropped entirely before that
    // caller's own writes begin later in the same call (Stage 1 review v1 §4). If this call is
    // ever hoisted to live across that caller's writes, or a new caller writes before calling
    // this, that guarantee breaks silently -- check this comment still describes reality before
    // assuming it's still safe.
    let object_store = ObjectReadSnapshot::open(layout)?;
    let baseline = crate::refs::read_current_ref_tip_block(layout, &object_store, ref_name)?;
    let chain = single_parent_chain(&object_store, baseline)?;
    let horizon = *chain
        .first()
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} lineage is empty")))?;
    Ok((baseline, horizon))
}

/// Baseline context for worktree authoring: either a published node-addressed lineage, or a genesis
/// (first-commit) context with no baseline at all.
pub(crate) enum WorktreeBaseline {
    /// The ref is published; author against replay-derived node lifecycle state.
    Published {
        /// Current target block (baseline).
        baseline_block: ObjectId,
        /// Lineage genesis (horizon).
        horizon: ObjectId,
    },
    /// The ref has never been published; author against an empty baseline (all `CreateFile`).
    Genesis,
}

/// Decide whether worktree authoring runs against a published lineage or a genesis (first-commit)
/// context (DC-09 4.4b). Genesis is selected **only** when the target ref has never been published:
/// the ref pointer is absent **and** the ref log is readable and empty. A missing pointer with any
/// ref-log history, or an unreadable/partial ref log, is treated as corruption - not genesis - and
/// fails closed with preserve/restore guidance (design §4, review E2). The active-WAL guard (review E1) is the
/// authoring caller's responsibility and is enforced there.
pub(crate) fn resolve_worktree_baseline(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<WorktreeBaseline> {
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    let ref_store = RefStore::new(layout.clone());
    if ref_store
        .read_current_ref_state_id(&canonical_ref)?
        .is_some()
    {
        let (baseline_block, horizon) = resolve_node_lineage_bounds(layout, &canonical_ref)?;
        return Ok(WorktreeBaseline::Published {
            baseline_block,
            horizon,
        });
    }
    // Pointer absent: genesis only if the log is readable and empty; otherwise corruption.
    // An absent log is decoded as an empty log by `RefStore::replay_log`; unreadable, malformed, or
    // partial logs remain corruption, not genesis.
    let log = ref_store.replay_log(&canonical_ref).map_err(|err| {
        PrikkError::Integrity(format!(
            "ref {canonical_ref} log is unreadable; run `prikk doctor` before committing ({err})"
        ))
    })?;
    if log.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} pointer is missing and its log has trailing partial bytes; \
             run `prikk doctor` (this is not a genesis repository)"
        )));
    }
    // RFC 102 Stage 2: checked before the emptiness check below -- a damaged sole record would
    // otherwise read as `log.records.is_empty()`, and the worst possible misclassification here is
    // exactly that: authoring against an empty (Genesis) baseline for a ref that actually has
    // history.
    if log.has_item_failure() {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} pointer is missing and its log has a damaged record; \
             run `prikk doctor` (this is not a genesis repository)"
        )));
    }
    if !log.records.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} pointer is missing but ref-log history exists; \
             preserve the repository and restore from backup (this is not a genesis repository)"
        )));
    }
    Ok(WorktreeBaseline::Genesis)
}

/// Test-support instrument (warm-cache `commit` anomaly measurement, RFC 136): which rung `commit` on
/// `ref_name` would take to its baseline state now, and why — the ref resolved exactly as
/// [`resolve_folded_worktree_baseline`] resolves it. Read-only; never in a shipped build.
///
/// # Errors
///
/// Resolving the ref's baseline, or opening a read snapshot, fails.
#[cfg(feature = "test-support")]
pub fn baseline_cache_rung_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<crate::lifecycle_cache::incremental::BaselineCacheRung> {
    let WorktreeBaseline::Published {
        baseline_block,
        horizon,
    } = resolve_worktree_baseline(layout, ref_name)?
    else {
        return Ok(crate::lifecycle_cache::incremental::BaselineCacheRung::Genesis);
    };
    let reader = crate::object_store::ObjectReadSnapshot::open(layout)?;
    Ok(
        crate::lifecycle_cache::incremental::baseline_cache_rung_at_for_test_support(
            layout,
            &reader,
            baseline_block,
            horizon,
        ),
    )
}

/// Test-support instruments for RFC 136 increment 2c's design round: the ref-resolving entry points to the
/// probes in `lifecycle_cache` (which take ids, so that module names neither `refs` nor this one). Read-only
/// except the ladder walk, which refreshes the cache under `layout` -- give it a copy. Never in a shipped build.
///
/// `ref_name`'s lineage, oldest first, with a read snapshot.
#[cfg(feature = "test-support")]
fn ref_chain_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<(crate::object_store::ObjectReadSnapshot, Vec<ObjectId>)> {
    let reader = crate::object_store::ObjectReadSnapshot::open(layout)?;
    let tip = crate::refs::read_current_ref_tip_block(layout, &reader, ref_name)?;
    let chain = single_parent_chain(&reader, tip)?;
    if chain.is_empty() {
        return Err(PrikkError::Integrity(format!(
            "ref {ref_name} lineage is empty"
        )));
    }
    Ok((reader, chain))
}

/// Block `block_number` (1 is the ref's first block) of `ref_name`'s lineage and its horizon.
#[cfg(feature = "test-support")]
fn ref_block_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    block_number: usize,
) -> Result<(crate::object_store::ObjectReadSnapshot, ObjectId, ObjectId)> {
    let (reader, chain) = ref_chain_for_test_support(layout, ref_name)?;
    let horizon = *chain
        .first()
        .ok_or_else(|| PrikkError::Integrity(format!("ref {ref_name} lineage is empty")))?;
    let block_id = *block_number
        .checked_sub(1)
        .and_then(|index| chain.get(index))
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {ref_name} has {} blocks, not {block_number}",
                chain.len()
            ))
        })?;
    Ok((reader, block_id, horizon))
}

/// Every block of `ref_name`, oldest first, through the real baseline ladder over a cache that starts absent
/// and is refreshed after each step -- what a corpus build's successive `commit`s do -- naming per tip the rung
/// taken, the header left and (with `compare`) whether the state returned equals full replay's in every field.
///
/// # Errors
///
/// The ref does not resolve, or a step fails.
#[cfg(feature = "test-support")]
pub fn ladder_walk_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    compare: bool,
) -> Result<Vec<crate::lifecycle_cache::incremental::LadderTip>> {
    let (reader, chain) = ref_chain_for_test_support(layout, ref_name)?;
    crate::lifecycle_cache::incremental::ladder_walk_over_chain_for_test_support(
        layout, &reader, &chain, compare,
    )
}

/// The size, with the DC-64 codec, of the lifecycle state at block `block_number` of `ref_name`.
///
/// # Errors
///
/// The store cannot be read, or the lineage does not replay.
#[cfg(feature = "test-support")]
pub fn lifecycle_state_shape_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    block_number: usize,
) -> Result<crate::lifecycle_cache::incremental::LifecycleStateShape> {
    let (reader, block_id, horizon) = ref_block_for_test_support(layout, ref_name, block_number)?;
    crate::lifecycle_cache::incremental::lifecycle_state_shape_at_for_test_support(
        &reader, block_id, horizon,
    )
}

/// Where a full replay of block `block_number` of `ref_name` spends its time (RFC 136 2c option (i)).
///
/// # Errors
///
/// The store cannot be read, the lineage does not replay, or the timed fold disagrees with the product's.
#[cfg(feature = "test-support")]
pub fn replay_time_split_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    block_number: usize,
) -> Result<crate::lifecycle_cache::replay::ReplayTimeSplit> {
    let (reader, block_id, horizon) = ref_block_for_test_support(layout, ref_name, block_number)?;
    crate::lifecycle_cache::replay::replay_time_split_for_test_support(&reader, block_id, horizon)
}

/// What an id-only walk derives of the history fields at block `block_number` of `ref_name`, against full
/// replay (RFC 136 2c option (i)).
///
/// # Errors
///
/// The store cannot be read, or the lineage does not decode or replay.
#[cfg(feature = "test-support")]
pub fn id_only_history_for_test_support(
    layout: &RepositoryLayout,
    ref_name: &str,
    block_number: usize,
) -> Result<crate::lifecycle_cache::replay::IdOnlyHistory> {
    let (reader, block_id, horizon) = ref_block_for_test_support(layout, ref_name, block_number)?;
    crate::lifecycle_cache::replay::id_only_history_for_test_support(&reader, block_id, horizon)
}

/// The baseline lifecycle state a worktree operation should compare or author against: the sealed
/// baseline (or an empty genesis state), with any already-queued (unsealed) patches for this ref
/// folded on top (DC-66) exactly as `commit` folds them.
#[non_exhaustive]
pub(crate) struct FoldedWorktreeBaseline {
    /// Baseline lifecycle state, with the active queue folded on top when it belongs to this ref.
    pub(crate) state: NodeLifecycleState,
    /// `Some((baseline_block, horizon))` when the ref is published; `None` for a genesis baseline.
    pub(crate) lineage: Option<(ObjectId, ObjectId)>,
    /// `Some(other_ref)` when the active WAL is non-empty but owned by a ref other than the one
    /// requested — correctly *not* folded into `state` (another ref's queue is not part of this
    /// ref's baseline), but a caller may still want to say so: that queue is real, committed work,
    /// not a stray file, even though it plays no part in this baseline (RFC 122
    /// `replay-baseline-handoff-v2-amendment.md` §4).
    pub(crate) queued_on_other_ref: Option<String>,
}

/// [`resolve_folded_worktree_baseline`] with a text cache of its own, for a caller that has no later
/// use for one (RFC 151's `branch_switch`) -- so that caller depends on this module alone rather than
/// also reaching into `lifecycle_cache::replay` for the cache type.
pub(crate) fn resolve_folded_worktree_baseline_with_own_cache(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    active_replay: &WalReplay,
) -> Result<FoldedWorktreeBaseline> {
    let mut text_cache = crate::lifecycle_cache::replay::TextCache::new();
    resolve_folded_worktree_baseline(
        layout,
        object_store,
        ref_name,
        active_replay,
        &mut text_cache,
    )
}

/// **The single derivation every worktree-comparing command uses** (RFC 122 §3,
/// `replay-baseline-handoff-v1.md`): `commit` (`node_authoring.rs`) and `worktree-status`
/// (`worktree_status.rs`) both call this rather than each reconstructing baseline state their own
/// way — a second implementation that happens to agree today is the defect RFC 122 fixes, one
/// release later. `active_replay` is a parameter, not read here, because `commit` already has its
/// own copy in scope for its own, authoring-specific checks (WAL tail/damage/patch-limit) and
/// reading it twice would cost a second WAL replay for no reason; `worktree-status` has no such
/// copy yet and reads one itself before calling this.
///
/// **Folding decision, deliberately not `require_active_ref_for_non_empty_wal`'s own decision**:
/// that function refuses outright when the active WAL belongs to a different ref, which is correct
/// for `commit` (it is about to append to that WAL and single-active-session ownership must be
/// unambiguous before it does) but wrong for a read-only query about one specific ref — a queue
/// that belongs to some *other* ref is simply irrelevant to this ref's status, not an error. Only
/// genuinely ambiguous ownership (non-empty WAL, no readable owner) is refused here, with the same
/// classification and wording `require_active_ref_for_non_empty_wal` already uses for that case —
/// by the time `commit` reaches this function it has already called that stricter check itself
/// (`author_inner`, before baseline resolution begins), so a non-empty queue there is always
/// already known to belong to this ref, and this function's own ownership check is a harmless,
/// always-true re-confirmation for that caller, not a behavior change.
///
/// `text_cache` is a parameter, not created here, because `commit` (`node_authoring.rs`) reuses its
/// own cache afterward for `plan_edit_text`'s own text materialization — folding into a cache the
/// caller keeps, not a throwaway internal to this call, so that later reuse still sees what folding
/// already materialized. `worktree-status` has no further use for it and passes a fresh, empty one.
pub(crate) fn resolve_folded_worktree_baseline(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    active_replay: &WalReplay,
    text_cache: &mut crate::lifecycle_cache::replay::TextCache,
) -> Result<FoldedWorktreeBaseline> {
    resolve_folded_worktree_baseline_with(
        layout,
        object_store,
        ref_name,
        &active_replay.records,
        text_cache,
        crate::lifecycle_cache::incremental::CacheWrite::Refresh,
    )
}

/// [`resolve_folded_worktree_baseline`] for a report that must **write nothing**: the same derivation, the same
/// answer, and the rebuildable baseline cache is read when it helps and never refreshed (`prikk diff`, RFC 153
/// §6.2).
pub(crate) fn resolve_folded_worktree_baseline_without_cache_write(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    active_replay: &WalReplay,
    text_cache: &mut crate::lifecycle_cache::replay::TextCache,
) -> Result<FoldedWorktreeBaseline> {
    resolve_folded_worktree_baseline_with(
        layout,
        object_store,
        ref_name,
        &active_replay.records,
        text_cache,
        crate::lifecycle_cache::incremental::CacheWrite::Never,
    )
}

/// [`resolve_folded_worktree_baseline_without_cache_write`], truncated after one queued patch (RFC 142,
/// the queued-patch-paths handoff): the sealed baseline with only the records **up to and including**
/// `patch_index` folded on top -- the state *at* that patch, the same way `lifecycle_state_at` is the
/// state at a sealed block, so a patch's own `show` renders identically before and after `seal` (apart
/// from `queued`). Read-only (`CacheWrite::Never`), the same as the untruncated form: `show` must not
/// become a writer any more than `diff` may.
///
/// # Errors
///
/// Whatever the untruncated form can: the sealed baseline cannot be derived, or the active WAL's queue
/// does not belong to `ref_name` (`Integrity` -- a caller that found `patch_index` in this exact WAL
/// already knows which ref that is, from the same read `read_active_ref_metadata` would give here).
/// `Integrity` too when `patch_index` is out of range for `active_replay` -- a caller found it in this
/// exact replay, so this is the replay changing under the caller, not a usage error.
pub(crate) fn resolve_folded_worktree_baseline_up_to_queued_patch(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    active_replay: &WalReplay,
    text_cache: &mut crate::lifecycle_cache::replay::TextCache,
    patch_index: usize,
) -> Result<FoldedWorktreeBaseline> {
    let records = active_replay.records.get(..=patch_index).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "queued patch index {patch_index} is out of range for {} active WAL records",
            active_replay.records.len()
        ))
    })?;
    resolve_folded_worktree_baseline_with(
        layout,
        object_store,
        ref_name,
        records,
        text_cache,
        crate::lifecycle_cache::incremental::CacheWrite::Never,
    )
}

/// [`resolve_folded_worktree_baseline_up_to_queued_patch`] with a text cache of its own, for `show`
/// (`show.rs`), which has no later use for one -- the same reason
/// [`resolve_folded_worktree_baseline_with_own_cache`] exists, so that `show` depends on this module
/// alone rather than also reaching into `lifecycle_cache::replay` for the cache type (the coupling
/// gate's own module-boundary rule).
pub(crate) fn resolve_folded_worktree_baseline_up_to_queued_patch_with_own_cache(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    active_replay: &WalReplay,
    patch_index: usize,
) -> Result<FoldedWorktreeBaseline> {
    let mut text_cache = crate::lifecycle_cache::replay::TextCache::new();
    resolve_folded_worktree_baseline_up_to_queued_patch(
        layout,
        object_store,
        ref_name,
        active_replay,
        &mut text_cache,
        patch_index,
    )
}

fn resolve_folded_worktree_baseline_with(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
    records: &[crate::wal::WalRecord],
    text_cache: &mut crate::lifecycle_cache::replay::TextCache,
    cache_write: crate::lifecycle_cache::incremental::CacheWrite,
) -> Result<FoldedWorktreeBaseline> {
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    let baseline = resolve_worktree_baseline(layout, &canonical_ref)?;
    let lineage = match &baseline {
        WorktreeBaseline::Published {
            baseline_block,
            horizon,
        } => Some((*baseline_block, *horizon)),
        WorktreeBaseline::Genesis => None,
    };
    let mut state = match &baseline {
        WorktreeBaseline::Published {
            baseline_block,
            horizon,
        } => crate::lifecycle_cache::incremental::resolve_baseline_state_with(
            layout,
            object_store,
            *baseline_block,
            *horizon,
            cache_write,
        )?
        .state()
        .clone(),
        WorktreeBaseline::Genesis => NodeLifecycleState::new(),
    };

    let mut queued_on_other_ref = None;
    if !records.is_empty() {
        match crate::read_active_ref_metadata(layout)? {
            crate::ActiveRefMetadata::Valid(actual) if actual == canonical_ref => {
                crate::lifecycle_cache::replay::apply_queued_patch_envelopes(
                    object_store,
                    records,
                    &mut state,
                    text_cache,
                    lineage,
                )?;
            }
            crate::ActiveRefMetadata::Valid(other) => {
                queued_on_other_ref = Some(other);
            }
            crate::ActiveRefMetadata::Missing => {
                return Err(PrikkError::Integrity(
                    "active WAL has records but active ref metadata is missing".to_string(),
                ));
            }
            crate::ActiveRefMetadata::Invalid(reason) => {
                return Err(PrikkError::Integrity(format!(
                    "active WAL has records but active ref metadata is malformed: {reason}"
                )));
            }
        };
    }

    Ok(FoldedWorktreeBaseline {
        state,
        lineage,
        queued_on_other_ref,
    })
}

#[cfg(test)]
pub(crate) mod tests;
