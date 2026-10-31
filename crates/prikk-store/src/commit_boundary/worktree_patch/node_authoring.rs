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
mod worktree_read;

// The read-only view `prikk diff` reads the worktree through (RFC 153 §6.2): built from the same parts
// `author_inner` uses, and it writes nothing.
pub(crate) use worktree_read::{Retain, read_worktree_for_diff};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, ChangePerm, CreateFile, DeleteNode, DeleteNodePreimage,
    EditText, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind,
    PATCH_MESSAGE_SCHEMA, PatchPayload, PatchPurpose, RenamePath, ReplaceBinary,
};

use crate::author::author_signing::AuthorSigner;
use crate::commit_boundary::active::{
    prepare_empty_active_ref_for_append, require_active_ref_for_non_empty_wal,
};
use crate::commit_boundary::worktree_patch::{
    DeclarationDisclosure, DeclarationDisclosureReason, MOVE_HINT_SUMMARY_THRESHOLD,
    MoveHintCandidate, MoveHints, WorktreePatchCommitOptions, WorktreePatchCommitReport,
    WorktreePatchOperationKind, WorktreePatchOperationSummary, next_op_seq,
};
use crate::commit_index::{self, CommitIndex, CommitIndexEntry};
use crate::foundation::fsutil::{EntryKind, RootFileStat, read_file_if_exists};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryLayout};
use crate::lifecycle_cache::replay::TextCache;
use crate::lock::{ActiveLock, ContainerLockGuard, acquire_container_locks};
use crate::node::node_id_gen::{NodeIdEntropySource, NodeIdGenerator};
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState};
use crate::object_store::{ObjectReader, ObjectWriteSession};
use crate::patch_replay::resolve_folded_worktree_baseline;
use crate::path::{RepoPath, authoring_path_error};
use crate::rename_declaration::{clear_rename_declarations, read_rename_declarations};
use crate::text_span;
use crate::worktree_marker::worktree_is_dirty;
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
pub(in crate::commit_boundary) enum AuthorError {
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
            // RFC 147 §2e(c): a symlink in the worktree and a text/binary kind change are both
            // states the caller can fix and nothing else can -- no lock is held, no retry helps,
            // and nothing is damaged. `worktree-status` now reports them *before* the commit, which
            // makes classifying them as damage doubly wrong: the tool said the commit would refuse.
            AuthorError::UnsupportedSymlinkAuthoring(detail)
            | AuthorError::UnsupportedKindTransition(detail) => PrikkError::Precondition(detail),
        }
    }
}

/// What a worktree entry looks like to an authoring decision -- the only shape facts the rule below
/// depends on. Two constructors, because the two callers legitimately hold different things: the
/// authoring walk sees `fsutil`'s root-scoped `EntryKind` (it must never touch `std::fs` directly),
/// and `worktree_status` holds a `symlink_metadata` it already read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub(crate) enum WorktreeEntryShape {
    /// An ordinary regular file: authorable.
    Regular,
    /// A symlink, by `symlink_metadata` -- never followed.
    Symlink,
    /// Present, but neither a regular file nor a symlink (FIFO, socket, device).
    Other,
}

impl WorktreeEntryShape {
    pub(crate) fn from_symlink_metadata(metadata: &std::fs::Metadata) -> Self {
        if metadata.file_type().is_symlink() {
            Self::Symlink
        } else if metadata.is_file() {
            Self::Regular
        } else {
            Self::Other
        }
    }

    pub(in crate::commit_boundary) const fn from_entry_kind(kind: EntryKind) -> Self {
        match kind {
            EntryKind::Regular => Self::Regular,
            EntryKind::Symlink => Self::Symlink,
            EntryKind::Directory | EntryKind::Other => Self::Other,
        }
    }
}

/// **The authoring refusal rule, in one place** (RFC 147 §2e(a)): given what the baseline says this
/// path is (if it tracks it at all) and what the worktree entry now looks like, the error `commit`
/// will raise for it -- or `None` when `commit` will author it.
///
/// **Shape is tested before baseline kind, because that is the order `commit` actually refuses in**:
/// the worktree walk (`worktree_files.rs`) meets a symlink and fails closed before node authoring
/// ever consults the baseline, so a tracked *file* replaced by a symlink refuses as a worktree
/// symlink, not as a symlink node. Verified against the binary rather than read off the call graph.
///
/// Content-dependent refusals are deliberately out of reach here:
/// [`AuthorError::UnsupportedKindTransition`] needs the bytes, and a status scan that read every
/// file to predict it would stop being a status scan.
pub(in crate::commit_boundary) fn authoring_refusal(
    path: &str,
    baseline_kind: Option<NodeKind>,
    shape: WorktreeEntryShape,
) -> Option<AuthorError> {
    match shape {
        WorktreeEntryShape::Symlink => Some(AuthorError::UnsupportedSymlinkAuthoring(format!(
            "{path}: worktree symlink authoring is out of scope"
        ))),
        WorktreeEntryShape::Other => Some(AuthorError::Store(PrikkError::InvalidName(format!(
            "{path}: worktree entry is not a regular file"
        )))),
        WorktreeEntryShape::Regular => match baseline_kind {
            Some(NodeKind::Symlink) => Some(AuthorError::UnsupportedSymlinkAuthoring(format!(
                "{path}: symlink node modification is out of scope"
            ))),
            _ => None,
        },
    }
}

/// `authoring_refusal` rendered exactly as `commit` will render it, for readers outside
/// `commit_boundary`.
///
/// `worktree_status` cannot name `AuthorError` -- it is `pub(in crate::commit_boundary)` (RFC 131
/// §6d.3) and stays that way. It gets the same decision through the same function.
///
/// **Rendered through `PrikkError`, not through `AuthorError`'s own `Display`.** §2e(c) reclassifies
/// these refusals, and `From<AuthorError> for PrikkError` carries the *detail* across for a
/// `Precondition` rather than the whole `AuthorError` line -- so the two forms had genuinely
/// different text the moment the class changed. Converting here is what makes the agreement
/// mechanical instead of coincidental: `commit`'s stderr is `"error: "` followed by exactly this
/// string, which is what the status-and-commit-agree test asserts.
pub(crate) fn authoring_refusal_reason(
    path: &str,
    baseline_kind: Option<NodeKind>,
    shape: WorktreeEntryShape,
) -> Option<String> {
    authoring_refusal(path, baseline_kind, shape)
        .map(|refusal| PrikkError::from(refusal).to_string())
}

impl From<PrikkError> for AuthorError {
    fn from(e: PrikkError) -> Self {
        AuthorError::Store(e)
    }
}

/// A baseline live file node, resolved from the replay-derived state. Derives `PartialEq`/`Eq`/
/// `Copy` (0.56.0 step W2b) so [`BaselinePathState`]'s own derived `PartialEq` can compare one
/// baseline's entry for a path against another's, field for field, with no hand-picked subset of
/// fields and no second comparison written by hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BaselineFile {
    node_id: NodeId,
    kind: NodeKind,
    blob_id: ObjectId,
    mode: u32,
}

/// A baseline's own split, as [`split_baseline`] produces it: files by path, symlinks by path.
/// Named (0.56.0 step W2b, `clippy::type_complexity`) because it is now also threaded through
/// [`path_is_stale`]'s own signature, not only held locally.
type BaselineSplit = (BTreeMap<String, BaselineFile>, BTreeMap<String, NodeId>);

/// A baseline's live nodes as authoring sees them: files by path, and symlink nodes by path (tracked so a change
/// touching one can fail closed). **The one split** `commit` and `prikk diff`'s worktree read both use.
fn split_baseline(state: &NodeLifecycleState) -> BaselineSplit {
    let mut files: BTreeMap<String, BaselineFile> = BTreeMap::new();
    let mut symlinks: BTreeMap<String, NodeId> = BTreeMap::new();
    for (node_id, node) in state.live_nodes() {
        match &node.content {
            NodeContent::File { blob_id, mode } => {
                files.insert(
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
                symlinks.insert(node.path.as_str().to_string(), *node_id);
            }
        }
    }
    (files, symlinks)
}

/// A path's state as a baseline sees it: presence, kind, the node it belongs to, its content, and
/// its mode -- the five dimensions 0.56.0 step W2b's own staleness rule names, read off the same
/// `BaselineFile`/symlink maps [`split_baseline`] already produces for the live baseline and (as
/// of this round) the recorded one too. [`BaselineFile`] derives `PartialEq`, so this type's own
/// derived one compares every field, never a hand-picked subset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BaselinePathState {
    File(BaselineFile),
    Symlink(NodeId),
    Absent,
}

impl BaselinePathState {
    fn of(
        path: &str,
        files: &BTreeMap<String, BaselineFile>,
        symlinks: &BTreeMap<String, NodeId>,
    ) -> Self {
        if let Some(file) = files.get(path) {
            Self::File(*file)
        } else if let Some(node_id) = symlinks.get(path) {
            Self::Symlink(*node_id)
        } else {
            Self::Absent
        }
    }
}

/// 0.56.0 step W2b: whether `path` is **stale** -- its state in the live baseline disagrees with
/// its state in this worktree's own recorded one, in content, mode, kind, presence, or the node it
/// belongs to (the ruling's own five dimensions -- exactly what [`BaselinePathState`]'s derived
/// `PartialEq` compares). `recorded` is `None` when this worktree has no record at all: R1's own
/// migration rule, preserved exactly here too -- a path is never stale against a baseline that was
/// never recorded, so a repository that predates this round, or one an older binary wrote to,
/// behaves exactly as it did before W2 and before W2b both.
///
/// **This is the one rule that replaces W2's own deletion-only "never held" check** -- the ruling's
/// own instruction was not to add a second check beside it. Every per-path authoring site below
/// (creates, edits, mode changes, a confirmed rename's own source node, and the deletion-inference
/// loop) calls this same function, at the exact point each one has already decided "there is
/// something to author here" (the worktree's current entry for `path` differs from the live
/// baseline's). A path that is stale but for which nothing would be authored -- the worktree
/// already holds the live state -- is never even asked about, because nothing below calls this
/// function except from inside a branch that is about to plan an operation. That is exactly the
/// ruling's own "unless the worktree already holds the live state for it," characterized once,
/// mechanically, rather than re-derived at each call site.
///
/// **`require_concrete_record` is the one asymmetry W2b's own measurement found necessary, and
/// 0.56.0 step W2c re-checked it empirically against the corrected, ref-gate-free rule rather
/// than assuming the ref gate's removal made it moot.** W2c's own ruling asked exactly this
/// question: with `recorded_names_canonical_ref` gone (see the call sites below -- staleness is
/// now judged in states, never in which ref the record names), does `dc67_ordinary_use_
/// conformance::sequence_06_branch_commit_both_close_one_keep_committing_other` still need the
/// asymmetry, or does the corrected rule make the record's "silence" about a path never arise?
/// **Measured, not assumed: removing this parameter entirely (making every site, including the
/// deletion loop, use a plain symmetric `!=`) was tried and makes `sequence_06` fail**, refusing
/// `commit --ref heads/main`'s own round-2 `main.txt` edit with "the worktree holds content for
/// main.txt, but it does not match heads/main's current state" -- even though `main.txt` was
/// never touched by anything other than `heads/main`'s own prior, legitimate commits. The cause
/// is unchanged by the ref-gate's removal: alternating `commit`s between `heads/main` and
/// `heads/topic` from one physical worktree leaves the record naming whichever ref committed
/// last, and `heads/topic`'s own history never mentions `main.txt` at all (`Absent` in the
/// recorded split) -- a plain `!=` against `main.txt`'s real, concrete live entry on `heads/main`
/// reads as "stale," not because any producer silently moved anything, but merely because the
/// record's one slot currently describes a disjoint-history ref that was simply never asked
/// about this path. **A recorded baseline that is silent about a path can never contradict that
/// path's live state** -- there is no claim to contradict -- so `require_concrete_record: true`
/// (every site but the deletion loop) returns `false` outright when the record has no entry for
/// `path` at all, letting the ordinary multi-ref authoring idiom (W2's own deviation #4) through
/// untouched; this is the ruling's own "case where the symmetric rule refuses something that is
/// not a hazard," demonstrated by running the suite, not argued from the shape of the rule. The
/// deletion loop alone keeps `require_concrete_record: false`, the original, *symmetric*
/// comparison: there, a recorded baseline silent about a path the worktree is missing **is**
/// exactly the hazard (W2's own five cases, and W2b's case 2 and case 5) -- the one direction
/// where "no claim" must still refuse, because nothing else explains the live tip having gained
/// content this worktree was never shown. Kept, not removed; see
/// `.git-exclude/review-request/0.56.0-step-3w2c-report.md` for the full empirical record.
///
/// **0.56.0 step W2c re-derivation:** this refinement no longer coexists with a ref-name gate
/// (W2b's `recorded_names_canonical_ref`, removed this round) -- it stands alone now, and was
/// re-verified empirically, not assumed, to still be necessary on its own: flipping
/// `require_concrete_record` to `false` at every non-deletion site (a fully symmetric rule
/// everywhere) and re-running `dc67_ordinary_use_conformance::sequence_06_branch_commit_both_
/// close_one_keep_committing_other` reproduces the exact same failure the original W2b round
/// measured -- `main.txt`'s own, legitimate next edit onto `heads/main`, committed right after a
/// commit onto `heads/topic` moved the record there, refuses with "the worktree holds content
/// for, but it does not match heads/main's current state... main.txt", even though no producer
/// touched anything. Removing the ref-name gate does not retire this hazard: the gate answered a
/// *different* question ("does the record's claim even apply to this ref") than this refinement
/// answers ("is a recorded *absence* a claim at all"), and a record can be silent about a path
/// for an entirely mundane reason -- it currently names a sibling ref whose own history simply
/// never mentions that path -- independent of whether that sibling's *concrete* entries (if any)
/// for other paths are still compared in full (which, after W2c, they now always are). So
/// `require_concrete_record` is sufficient and correct standing alone: it is the one place "the
/// record has nothing to say about this path" is distinguished from "the record says something
/// that disagrees," and only the latter is ever evidence of staleness for a would-author site.
fn path_is_stale(
    path: &str,
    recorded: Option<&BaselineSplit>,
    live_files: &BTreeMap<String, BaselineFile>,
    live_symlinks: &BTreeMap<String, NodeId>,
    require_concrete_record: bool,
) -> bool {
    let Some((recorded_files, recorded_symlinks)) = recorded else {
        return false;
    };
    let recorded_state = BaselinePathState::of(path, recorded_files, recorded_symlinks);
    if require_concrete_record && recorded_state == BaselinePathState::Absent {
        return false;
    }
    BaselinePathState::of(path, live_files, live_symlinks) != recorded_state
}

/// Why a stale path's authoring was refused (0.56.0 step W2b) -- collected across every per-path
/// site above into one refusal, named before anything is written to the active WAL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StalePathReason {
    /// Not present in the worktree at all. W2's own original (and, per the ruling, now the special
    /// case this general rule reduces to) shape: "worktree absent, live present."
    AbsentFromWorktree,
    /// Present in the worktree, with content/mode/kind that does not match the live baseline --
    /// authoring now would either revert a producer's change (the worktree holds the record's old
    /// state) or record a conflict as an ordinary edit (the worktree holds a third state). Both
    /// refuse the same way; the ruling's own words are "either way: refuse."
    PresentButStale,
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
pub(in crate::commit_boundary) fn author_worktree_patch<S: NodeIdEntropySource, A: AuthorSigner>(
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
        ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME).map_err(AuthorError::Store)?;
    // 0.54.0 step 1 round 2: the precondition decodes the active WAL itself, at the same point it
    // always has (its own fifth and last sequential check, after ref-log damage, agreement,
    // object-index and candidate-debris) -- round 1 decoded here first, which let a decode failure
    // (an I/O error, or a newer schema) outrun one of those four earlier, more specific refusals.
    // The slot below receives that one decode, shared by everything `active_replay` feeds further
    // down (the tail/damage check, the active-patch limit, the RFC 166 witness classification, the
    // worktree baseline fold) -- one decode under the held lock, not two, taken at the original
    // point, not moved ahead of it.
    let mut active_replay_slot: Option<crate::wal::WalReplay> = None;
    crate::refs::ensure_publication_precondition_with_replay(
        layout,
        None,
        false,
        Some(&mut active_replay_slot),
    )
    .map_err(AuthorError::Store)?;
    let active_replay = active_replay_slot.ok_or_else(|| {
        AuthorError::Store(PrikkError::Integrity(
            "ensure_publication_precondition_with_replay returned Ok(()) without filling the \
             replay slot"
                .to_string(),
        ))
    })?;
    // RFC 136 §10.3b.3: the derivation gate, before any write.
    crate::worktree_marker::ensure_worktree_replay_verified(layout).map_err(AuthorError::Store)?;

    // RFC 102 Stage 1: a dirty worktree marker means a prior materialization call did not complete
    // durably -- this function's own deletion-inference loop (below) cannot distinguish "the user
    // deleted this file" from "this file's name never survived a crash," so it must not run at all
    // until the worktree has been re-verified against its baseline. Checked before any worktree
    // read below, not only before the deletion loop specifically, since nothing here re-verifies.
    if worktree_is_dirty(layout).map_err(AuthorError::Store)? {
        // Checkout-refusal round §2.3: a user state with a way out, so `Precondition`, and the route
        // it names is one that clears the marker.
        return Err(AuthorError::Store(PrikkError::Precondition(format!(
            "worktree materialization was interrupted, so the worktree is not verified against its \
             baseline and nothing can be committed; {}",
            crate::worktree_marker::dirty_marker_route(layout).map_err(AuthorError::Store)?
        ))));
    }

    // 0.54.0 step 1 round 2: `active_replay` is the single decode the precondition above took and
    // handed back through the slot -- not re-decoded here.
    // RFC 166 round 2 §0 (review v1's own required fix): the commit-witness classification speaks
    // *first*, before the older tail/damage checks below -- round 1 wired it in after ownership was
    // already confirmed, which let the tail check's own generic text answer first for rows 4-7, 9 and
    // 10, naming `--repair-wal-tail` as if it could help when it cannot. `owning_ref_now` is a single
    // fresh read, reused by the empty-queue branch below instead of a second one. `commit_witness_
    // verdict` is kept past this block -- §13 item 13 reads it again below, at the "no node-addressed
    // changes" refusal, to tell an interrupted-but-durable retry apart from a genuinely empty worktree.
    let owning_ref_now = read_active_ref_metadata(layout).map_err(AuthorError::Store)?;
    let commit_witness_verdict = {
        let witness = crate::commit_boundary::witness::read_witness(layout, DEFAULT_ACTIVE_NAME)
            .map_err(AuthorError::Store)?;
        let verdict = crate::commit_boundary::classification::classify(
            layout,
            &active_replay,
            &owning_ref_now,
            &witness,
        )
        .map_err(AuthorError::Store)?;
        if let Some(reason) = crate::commit_boundary::classification::write_refusal_reason(
            &verdict,
            Some(
                crate::commit_boundary::classification::RestoreRefusalContext {
                    layout,
                    witness: &witness,
                    queued_count: active_replay.records.len(),
                },
            ),
        ) {
            return Err(AuthorError::Store(PrikkError::Integrity(reason)));
        }
        verdict
    };
    // Row 3 only past this point (round 2 §0 item 2): a genuine crash tail the classification above
    // already confirmed is not acknowledged damage. `--repair-wal-tail` is the correct way out here.
    if active_replay.trailing_partial_bytes != 0 {
        return Err(AuthorError::Store(PrikkError::Integrity(format!(
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
        return Err(AuthorError::Store(
            active_replay.damaged_record_error("committing"),
        ));
    }
    // DC-57 (NFR-PERF-02): the hard block fires here — before any ref-metadata write, baseline
    // resolution, blob write, or WAL append below. "Active patches" has exactly one definition and
    // computation site: the active WAL's record count, read once above. `>=` (not `>`) is deliberate:
    // once the queue already holds `active_patch_limit` patches, no more may be added; the boundary
    // tests are 799/800/999/1000/1001 against the count *before* this commit's own patch.
    if crate::commit_boundary::worktree_patch::active_patch_limit_exceeded(
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
        match owning_ref_now {
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

    // RFC 164 Rule D: this signer's own author-key check, moved here from its own previous position
    // immediately before the WAL append (near the end of this function, where `record_author_key_
    // material` still runs and re-checks, harmless and cheap under the same held `active_lock`) --
    // before this fix, a new author's very first commit already had a durable blob, an object-index
    // entry, and updated caches by the time this check ran, so a refusal here did not leave the tree
    // as it found it. Read-only (`check_author_key_conflict` records nothing); the actual append, if
    // any, still happens only at `record_author_key_material`'s own call site below, under the same
    // `active_lock` held continuously since the top of this function, so nothing else can append to
    // the author-key container between this check and that one.
    crate::author::author_key_index::check_author_key_conflict(
        layout,
        signer.key_id(),
        signer.public_key_bytes(),
    )
    .map_err(AuthorError::Store)?;

    // RFC 111 §6.1 Stage 2: `author_worktree_patch` never publishes a ref itself (that happens at
    // seal time, a different operation) -- confirmed by grep, no `.publish(`/
    // `finish_interrupted_publication`/`RefPublication` anywhere in this file -- so it needs no
    // ref-publication threading, only the plain object-write-session swap.
    let mut object_store = ObjectWriteSession::open(layout).map_err(AuthorError::Store)?;
    // 0.55.0 step 3 Part 2: held once across both blob-writing loops below
    // (`write_object_under_held_lock`, the shape `archive import` already uses), instead of
    // acquired and released once per written file. `ObjectStore` is a leaf
    // (`object_store.rs:237`-`241`) and declared last in `LockableContainer`'s own order; nothing
    // else is acquired anywhere else in this function (only `ActiveLock`, above, already held), so
    // holding it here changes nothing about the documented nesting. Dropped explicitly once both
    // loops are done, below.
    let object_store_lock = acquire_container_locks(layout, &[LockableContainer::ObjectStore])
        .map_err(AuthorError::Store)?;

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

    // 0.56.0 step W2b: the recorded baseline's own full file/symlink split, not just a set of path
    // names (W2's own original, narrower shape) -- staleness is a per-path *state* comparison
    // (content, mode, kind, presence, and the node a path belongs to), not a presence check alone,
    // so [`split_baseline`] -- the exact function that already turns the live baseline into
    // `baseline_files`/`baseline_symlinks` below -- is reused here against the recorded replay too.
    // One derivation for "what a baseline's live nodes look like," not two.
    //
    // Consulted *per path*, by every per-path authoring site below (creates, edits, mode changes,
    // a confirmed rename's own source node, and the deletion-inference loop), through
    // [`path_is_stale`] -- not as a blanket ref-level gate here. A blanket "does the recorded
    // identity match `canonical_ref`'s live one" gate (tried first, in an earlier round of this
    // same change, W2's own deviation #4) refused real, legitimate usage this codebase's own test
    // suite already relies on: committing onto a ref other than the one the worktree was last
    // recorded against is completely safe whenever *that ref's own baseline* has nothing stale
    // relative to the record for the paths that actually matter -- `dc74_merge_execution`'s
    // fixtures author directly onto a sibling branch (`commit --ref heads/topic` while the worktree
    // is recorded against `heads/main`) purely to stage history, never switching the worktree, and
    // nothing is lost because topic's own baseline at that point has nothing stale for this
    // worktree. `None` when there is no record (R1's own migration-safety rule: every path is then
    // never stale, so this round behaves exactly as before W2 for a repository with no record).
    //
    // 0.56.0 step W2c: W2b's own further refinement, gating `path_is_stale` on whether the record
    // happened to name `canonical_ref` by name (`recorded_names_canonical_ref`), is removed. It
    // reopened the hazard for an edit: `commit --ref heads/feature` from a worktree recorded
    // against `heads/main`, where `heads/feature` itself diverged on a path this worktree still
    // holds the pre-divergence bytes for, silently reverted `heads/feature`'s own sealed change,
    // because the gate switched the whole check off whenever the record named a different ref.
    // The rule is stated in states, never in ref names, and is enforced unconditionally below --
    // the legitimate multi-ref idiom above survives only on the paths where the record and the
    // live state actually agree, never merely because the record names a different ref.
    let recorded_identity =
        crate::worktree_baseline::read_worktree_baseline(layout).map_err(AuthorError::Store)?;
    let recorded_baseline: Option<BaselineSplit> = match &recorded_identity {
        None => None,
        Some(recorded) => {
            // A fresh, throwaway cache -- never `queue_text_cache`, which `plan_edit_text` below
            // reuses for the *target* ref's own folding; sharing it here would let a node that
            // exists in both the recorded and the target state, with different text, poison the
            // other's cached materialization.
            let mut recorded_text_cache = crate::lifecycle_cache::replay::TextCache::new();
            let recorded_state = crate::patch_replay::resolve_state_at_recorded_baseline(
                layout,
                &object_store,
                recorded.baseline,
                recorded.wal_sequence,
                &active_replay,
                &mut recorded_text_cache,
            )
            .map_err(AuthorError::Store)?;
            Some(split_baseline(&recorded_state.state))
        }
    };
    let baseline_state: NodeLifecycleState = resolved.state;

    // Baseline file view: path -> (node_id, kind, blob_id, mode). Symlink nodes are tracked so a
    // change touching one can fail closed.
    let (baseline_files, baseline_symlinks) = split_baseline(&baseline_state);

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
    let mut renamed_away: BTreeSet<String> = BTreeSet::new();
    let mut confirmed_renames: Vec<(String, String)> = Vec::new();
    // RFC 144 §4p.2: every declaration that resolves to something other than the rename it asserted
    // gets one disclosure entry -- the outcome was always correct, the silence about it was the gap.
    let mut declaration_disclosures: Vec<DeclarationDisclosure> = Vec::new();

    // RFC 147 §2f: one classifier, obeyed here and reported by `worktree-status`, so a report can
    // never say "rename" about a declaration this commit refuses. The whole batch is evaluated
    // together (the two-node swap tolerance lives inside it), and this loop only acts on the answers.
    let outcomes = crate::declaration_resolution::resolve_declarations(
        layout,
        &live_declarations,
        |path| {
            baseline_files
                .get(path)
                .map(|base| crate::declaration_resolution::DeclaredSource {
                    node_id: base.node_id,
                    kind: base.kind,
                    blob_id: base.blob_id,
                    mode: base.mode,
                })
        },
        &crate::ignore::IgnoreRules::load(layout).map_err(AuthorError::Store)?,
        &tracked_paths,
    )
    .map_err(AuthorError::Store)?;
    for outcome in &outcomes {
        match &outcome.resolution {
            // §3: refuse the whole commit, naming the declaration, rather than silently dropping a
            // human assertion. The first refusal in store order is the one reported, verbatim.
            crate::declaration_resolution::DeclarationResolution::Refused(message) => {
                return Err(AuthorError::DeclarationContradicted(message.clone()));
            }
            crate::declaration_resolution::DeclarationResolution::NeverTracked => {
                declaration_disclosures.push(DeclarationDisclosure {
                    old_path: outcome.old_path.clone(),
                    new_path: outcome.new_path.clone(),
                    resolution: DeclarationDisclosureReason::NeverTracked,
                });
            }
            crate::declaration_resolution::DeclarationResolution::Deletion => {
                declaration_disclosures.push(DeclarationDisclosure {
                    old_path: outcome.old_path.clone(),
                    new_path: outcome.new_path.clone(),
                    resolution: DeclarationDisclosureReason::DestinationDeleted,
                });
            }
            crate::declaration_resolution::DeclarationResolution::DeletionDirectory => {
                declaration_disclosures.push(DeclarationDisclosure {
                    old_path: outcome.old_path.clone(),
                    new_path: outcome.new_path.clone(),
                    resolution: DeclarationDisclosureReason::DestinationIsDirectory,
                });
            }
            crate::declaration_resolution::DeclarationResolution::DeletionIgnored => {
                declaration_disclosures.push(DeclarationDisclosure {
                    old_path: outcome.old_path.clone(),
                    new_path: outcome.new_path.clone(),
                    resolution: DeclarationDisclosureReason::DestinationIgnored,
                });
            }
            crate::declaration_resolution::DeclarationResolution::Rename { .. } => {
                renamed_away.insert(outcome.old_path.clone());
                confirmed_renames.push((outcome.old_path.clone(), outcome.new_path.clone()));
            }
        }
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

    // 0.56.0 step W2b: every path found stale-and-about-to-be-authored, across every per-path site
    // below -- the one collection point the generalized rule feeds, named in full before anything
    // is written (see the refusal built from it, after the deletion loop further down).
    let mut stale_paths: BTreeMap<String, StalePathReason> = BTreeMap::new();

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
        // 0.56.0 step W2c: computed once per path, consulted only from inside a branch that has
        // already decided something would be authored for `path` -- never unconditionally.
        // **No longer gated on which ref the record names** (W2b's own `recorded_names_canonical_
        // ref` removed): the rule is stated in states, never in ref names -- for every path where
        // the record disagrees with `canonical_ref`'s own live state, the worktree must already
        // hold that live state, or the commit refuses, *regardless* of which ref the record
        // itself names. The legitimate multi-ref idiom (committing onto a sibling while the
        // record still names another ref) survives only on the paths where the record and the
        // live state actually agree -- shared, undiverged history -- not, as W2b had it, on every
        // path merely because the record names a different ref. See `path_is_stale`'s own doc for
        // the measured hazard this closes (a cross-ref edit reverting a diverged change).
        let is_stale = path_is_stale(
            path,
            recorded_baseline.as_ref(),
            &baseline_files,
            &baseline_symlinks,
            true,
        );
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
                        if is_stale {
                            stale_paths.insert(path.clone(), StalePathReason::PresentButStale);
                            continue;
                        }
                        let bytes = match resolved.bytes {
                            Some(bytes) => bytes,
                            None => read_existing_file_bytes(layout, path, BlobKind::Text)?,
                        };
                        planned.push(plan_edit_text(
                            layout,
                            &object_store,
                            base,
                            &bytes,
                            path,
                            (lineage_baseline_block_id, lineage_horizon_id),
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
                        if is_stale {
                            stale_paths.insert(path.clone(), StalePathReason::PresentButStale);
                            continue;
                        }
                        let bytes = match resolved.bytes {
                            Some(bytes) => bytes,
                            None => read_existing_file_bytes(layout, path, BlobKind::Binary)?,
                        };
                        planned.push(plan_replace_binary(
                            &mut object_store,
                            &object_store_lock,
                            base,
                            &bytes,
                            path,
                        )?);
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
                if is_stale {
                    stale_paths.insert(path.clone(), StalePathReason::PresentButStale);
                } else {
                    planned.push(op);
                }
            }
        } else if baseline_symlinks.contains_key(path) {
            return Err(AuthorError::UnsupportedSymlinkAuthoring(format!(
                "{path}: symlink node modification is out of scope"
            )));
        } else {
            // 0.56.0 step W2b: no baseline node tracks `path` in the *live* baseline, but the
            // worktree still holds a file there -- about to be authored as a fresh `CreateFile`.
            // When the record disagrees (it tracked a node at this path, now gone from the live
            // baseline -- a producer's deletion this worktree was never told about), authoring the
            // create would re-materialize stale content under a brand-new node id instead of
            // refusing -- exactly the "producer's deletion" shape the ruling names by reasoning.
            if is_stale {
                stale_paths.insert(path.clone(), StalePathReason::PresentButStale);
                continue;
            }
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
        // 0.56.0 step W2b: the node a declared rename moves is itself checked against the record,
        // at its *source* path -- the ruling's own "the node it belongs to" dimension, for a
        // rename specifically. Refusing here (rather than letting `renamed_away.contains(old_path)`
        // exempt it from the deletion loop below, as W2's own version did unconditionally) is what
        // keeps a stale rename from bypassing the check by being claimed early as "not a plain
        // deletion." Skips the whole triplet (RenamePath, and any simultaneous content/mode change
        // on the same node) for this pair -- one node, one staleness verdict.
        // 0.56.0 step W2c: no longer gated on which ref the record names, same reason as the main
        // loop above.
        if path_is_stale(
            old_path,
            recorded_baseline.as_ref(),
            &baseline_files,
            &baseline_symlinks,
            true,
        ) {
            stale_paths.insert(old_path.clone(), StalePathReason::PresentButStale);
            continue;
        }
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
                        layout,
                        &object_store,
                        base,
                        &bytes,
                        new_path,
                        (lineage_baseline_block_id, lineage_horizon_id),
                        &queue_text_cache,
                    )?);
                }
                NodeKind::BinaryFile => {
                    planned.push(plan_replace_binary(
                        &mut object_store,
                        &object_store_lock,
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
    //
    // 0.56.0 step W2b: a candidate absent from the worktree *and* stale against this worktree's
    // own recorded baseline (when one exists) was never materialized here at all -- its absence
    // cannot be the user's deletion, because there was never a chance to delete it. This is the
    // special case `path_is_stale`'s general rule reduces to ("worktree absent, live present") --
    // the same function every other per-path site above already calls, not a second check.
    // `require_concrete_record: false` -- the one site that keeps the *symmetric* comparison: a
    // record silent about `path` (no concrete claim at all) is exactly W2's own original hazard
    // here (every one of its five cases, and this round's case 2 and case 5), because nothing else
    // explains the live tip having gained a path this worktree was never shown.
    for (path, base) in &baseline_files {
        if !worktree.contains_key(path) {
            if renamed_away.contains(path) {
                // Claimed above by a confirmed rename's own RenamePath -- not a plain deletion.
                continue;
            }
            if path_is_stale(
                path,
                recorded_baseline.as_ref(),
                &baseline_files,
                &baseline_symlinks,
                false,
            ) {
                stale_paths.insert(path.clone(), StalePathReason::AbsentFromWorktree);
                continue;
            }
            planned.push(plan_delete(base, path));
        }
    }
    // 0.56.0 step W2b: one refusal, naming every path collected as stale-and-about-to-be-authored
    // across every per-path site above (creates, edits, mode changes, renames, and the deletion
    // loop just above) -- before anything is written. `AbsentFromWorktree` keeps W2's own original
    // wording verbatim (including "never were"); `PresentButStale` is the generalization this round
    // adds (an edit, mode change, create or rename that would revert a producer's change or record
    // a conflict as an ordinary one).
    if !stale_paths.is_empty() {
        let recorded_desc = match &recorded_identity {
            Some(recorded) => format!(
                "this worktree was last written from {}",
                crate::worktree_baseline::describe_identity(
                    recorded.baseline,
                    recorded.wal_sequence
                )
            ),
            // Unreachable in practice: `recorded_baseline` is only `Some` (so `stale_paths` can be
            // non-empty) when `recorded_identity` is also `Some` -- kept as a real sentence, not an
            // `unreachable!()`, since a refusal message must never panic to compose.
            None => "this worktree has no recorded baseline".to_string(),
        };
        let mut absent: Vec<&str> = Vec::new();
        let mut present: Vec<&str> = Vec::new();
        for (path, reason) in &stale_paths {
            match reason {
                StalePathReason::AbsentFromWorktree => absent.push(path.as_str()),
                StalePathReason::PresentButStale => present.push(path.as_str()),
            }
        }
        let mut clauses = Vec::new();
        if !absent.is_empty() {
            clauses.push(format!(
                "{} path(s) the new state has are not in this worktree, and never were: {}",
                absent.len(),
                absent.join(", ")
            ));
        }
        if !present.is_empty() {
            clauses.push(format!(
                "{} path(s) the worktree holds content for, but it does not match {canonical_ref}'s \
                 current state, so authoring now would either revert that change or record a \
                 conflict as an ordinary one: {}",
                present.len(),
                present.join(", ")
            ));
        }
        return Err(AuthorError::Store(PrikkError::Precondition(format!(
            "refusing to commit: {canonical_ref} is now at {}, and {recorded_desc}; {}; run \
             `prikk branch switch <the current branch>` (`prikk status` names the current \
             branch) to bring the worktree up to date, then commit again",
            crate::worktree_baseline::describe_identity(lineage_baseline_block_id, 0),
            clauses.join("; ")
        ))));
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
        let repo_path = RepoPath::parse(path)
            .map_err(authoring_path_error)
            .map_err(AuthorError::Store)?;
        let (blob_kind, node_kind) = classify_new(bytes);
        let blob_id = write_content_blob(&mut object_store, &object_store_lock, blob_kind, bytes)?;
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
    // Every blob write for this commit is behind us; nothing past this point touches the object
    // store, so the lock is released here rather than riding along to the end of the function.
    drop(object_store_lock);

    if planned.is_empty() {
        // RFC 166 §13 item 13: a `Pending` commit witness means the active WAL already durably
        // holds a sound, unacknowledged record -- exactly what a commit killed right after its own
        // durable append (but before reporting success) leaves behind. Retrying that same commit
        // reaches here with nothing left to plan, since the worktree's own baseline already folds
        // the queued patch in -- "no node-addressed changes" is true but misleading in that specific
        // case, and indistinguishable from an older prikk having written it instead; the text says
        // so honestly rather than picking one.
        if matches!(
            commit_witness_verdict,
            crate::commit_boundary::classification::Verdict::Pending { .. }
        ) {
            return Err(AuthorError::Store(PrikkError::Precondition(
                "a queued commit was already written but not confirmed, either because a \
                 previous command was interrupted after its own durable write or because an \
                 older prikk wrote it; there is nothing left to commit"
                    .to_string(),
            )));
        }
        // RFC 132's Precondition variant: an empty change set names no path and involves no name
        // validation at all -- `InvalidName`'s own doc ("a path-like name failed Prikk path/ref
        // validation") never applied here.
        return Err(AuthorError::Store(PrikkError::Precondition(
            "worktree has no node-addressed changes to commit".to_string(),
        )));
    }

    // RFC 144 §4o.4: read-only, over `planned` exactly as it stands right now -- authors nothing,
    // and must run before anything below could be mistaken for a reason to change it.
    let move_hints = plan_move_hints(&planned, &renamed_away, &rename_targets)?;

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

    // RFC 166 D1: `ref-name` is written only by a session's first commit. Ownership cannot have
    // changed on a later one -- a non-empty WAL already proves it (the ownership check above, which
    // ran against the unmodified file, already confirmed it names this same ref). Rewriting it on
    // every commit was §1.6: a crash between this write's own truncate and its own append left a
    // non-empty WAL with no durable owner, on any commit after the first. `ActiveSession::append_patch`
    // already had this shape; this is the one call site that did not.
    if active_replay.records.is_empty() {
        prepare_empty_active_ref_for_append(layout, &canonical_ref).map_err(AuthorError::Store)?;
    }
    let wal_sequence = crate::commit_boundary::witness::append_patch_and_witness(
        layout,
        DEFAULT_ACTIVE_NAME,
        &canonical_ref,
        &patch,
    )
    .map_err(AuthorError::Store)?;

    // RFC 144 §4o.2: "cleared when the commit that consumes it is queued -- not when sealed." The
    // loop above resolves every live declaration into exactly one of confirmed-rename,
    // nets-to-deletion, or vacuous -- a contradiction returns early, before this point, and nothing
    // else leaves a declaration unresolved -- so a successful append here means the whole live set
    // was just consumed and the store is cleared unconditionally, not entry by entry.
    if !live_declarations.is_empty() {
        clear_rename_declarations(layout).map_err(AuthorError::Store)?;
    }

    // 0.56.0 step W2 (R1): the worktree is now "the queued fold" at this new sequence -- the sealed
    // baseline this commit authored against (`lineage_baseline_block_id`, unchanged by `commit`
    // itself) with the active WAL folded up to the sequence this append just returned. Costs no
    // replay: this is exactly what this call already knows, the moment it knows it.
    crate::worktree_baseline::record_worktree_baseline(
        layout,
        &canonical_ref,
        lineage_baseline_block_id,
        wal_sequence,
    )
    .map_err(AuthorError::Store)?;

    Ok(WorktreePatchCommitReport {
        ref_name: canonical_ref,
        patch_id,
        wal_sequence,
        operation_count,
        referenced_blob_count,
        text_edit_count,
        changes: summaries,
        declaration_disclosures,
        move_hints,
    })
}

/// RFC 144 §4o.4: compute this commit's own move-hint candidates from `planned` as already
/// assembled -- read-only, authors nothing, and does not consult `planned` again after this point.
/// Exact content equality is the whole signal (§4o.4's own scope note rules similarity scoring
/// out): two operations are a candidate pair only when a `DeleteNode`'s preimage blob and a
/// `CreateFile`'s new blob are byte-identical, `blob_id` equality standing in for content equality
/// since `BlobPayload`'s canonical encoding (`commit_index::content_hash`'s own doc) is a pure
/// function of `(kind, bytes)`.
fn plan_move_hints(
    planned: &[PlannedOp],
    renamed_away: &BTreeSet<String>,
    rename_targets: &BTreeSet<String>,
) -> std::result::Result<MoveHints, AuthorError> {
    let mut deleted_by_blob: BTreeMap<ObjectId, Vec<&str>> = BTreeMap::new();
    let mut created_by_blob: BTreeMap<ObjectId, Vec<&str>> = BTreeMap::new();
    for op in planned {
        match &op.kind {
            OperationKind::DeleteNode(delete) => {
                if let DeleteNodePreimage::File { old_blob_id, .. } = &delete.preimage {
                    deleted_by_blob
                        .entry(*old_blob_id)
                        .or_default()
                        .push(delete.path.as_str());
                }
            }
            OperationKind::CreateFile(create) => {
                created_by_blob
                    .entry(create.blob_id)
                    .or_default()
                    .push(create.path.as_str());
            }
            _ => {}
        }
    }

    // Zero-byte content is excluded explicitly (§4o.4 case 2): every empty file shares one of these
    // two blob ids, a real repository holds several unrelated ones, and a lone 1:1 pair of them is
    // not caught by the ambiguity check below (that check only fires on more than one candidate per
    // side) -- confirmed by working through exactly that case rather than assumed.
    let empty_text_blob_id =
        commit_index::content_hash(BlobKind::Text, &[]).map_err(AuthorError::Store)?;
    let empty_binary_blob_id =
        commit_index::content_hash(BlobKind::Binary, &[]).map_err(AuthorError::Store)?;

    let mut candidates = Vec::new();
    for (blob_id, deleted_paths) in &deleted_by_blob {
        if *blob_id == empty_text_blob_id || *blob_id == empty_binary_blob_id {
            continue;
        }
        let Some(created_paths) = created_by_blob.get(blob_id) else {
            continue;
        };
        // Ambiguous pairing (§4o.4 case 1): more than one candidate on either side means which
        // moved to which is unknowable -- no hint for this content at all, rather than a guess.
        if deleted_paths.len() != 1 || created_paths.len() != 1 {
            continue;
        }
        let (Some(&old_path), Some(&new_path)) = (deleted_paths.first(), created_paths.first())
        else {
            continue;
        };
        // §4o.4 case 3: a declared move authors a RenamePath, never a DeleteNode/CreateFile pair --
        // asserted here, not assumed, since `renamed_away`/`rename_targets` are exactly the sets
        // the loops above used to keep a confirmed rename's own paths out of the generic
        // deletion/create loops in the first place.
        debug_assert!(
            !renamed_away.contains(old_path) && !rename_targets.contains(new_path),
            "a declared move must never reach the move-hint candidate scan: {old_path} -> {new_path}"
        );
        candidates.push(MoveHintCandidate {
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
        });
    }
    candidates.sort_by(|a, b| a.old_path.cmp(&b.old_path));

    Ok(if candidates.is_empty() {
        MoveHints::None
    } else if candidates.len() > MOVE_HINT_SUMMARY_THRESHOLD {
        // §4o.4 case 4: a genuine mass reorganisation prints one summary line, not a wall of them.
        MoveHints::Summary {
            count: candidates.len(),
        }
    } else {
        MoveHints::Pairs(candidates)
    })
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
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    base: &BaselineFile,
    new_bytes: &[u8],
    path: &str,
    (lineage_baseline_block_id, lineage_horizon_id): (Option<ObjectId>, Option<ObjectId>),
    queue_text_cache: &crate::lifecycle_cache::replay::TextCache,
) -> std::result::Result<PlannedOp, AuthorError> {
    let old_text = current_text_for_node(
        layout,
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
    object_store: &mut ObjectWriteSession,
    object_store_lock: &ContainerLockGuard,
    base: &BaselineFile,
    new_bytes: &[u8],
    path: &str,
) -> std::result::Result<PlannedOp, AuthorError> {
    let new_blob_id =
        write_content_blob(object_store, object_store_lock, BlobKind::Binary, new_bytes)?;
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
    match existing_content_refusal(path, blob_kind, &bytes) {
        Some(refusal) => Err(refusal),
        None => Ok(bytes),
    }
}

/// **The existing-content rule, in one place**: an existing `TextFile` node must stay valid UTF-8 (E4, the
/// node's kind is authoritative and never reclassified). `commit` refuses with this; `prikk diff` names the path
/// with the same words instead of failing on it.
fn existing_content_refusal(path: &str, blob_kind: BlobKind, bytes: &[u8]) -> Option<AuthorError> {
    if matches!(blob_kind, BlobKind::Text) && std::str::from_utf8(bytes).is_err() {
        return Some(AuthorError::UnsupportedKindTransition(format!(
            "{path}: existing TextFile cannot accept non-UTF-8 content"
        )));
    }
    None
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
    object_store: &mut ObjectWriteSession,
    object_store_lock: &ContainerLockGuard,
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
    // 0.55.0 step 3 Part 2: `write_object_under_held_lock`, not `write_object` -- the caller already
    // holds `ObjectStore` for the whole commit, once, not once per blob.
    object_store
        .write_object_under_held_lock(&envelope, object_store_lock)
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
    layout: &RepositoryLayout,
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
    // RFC 136 increment 2c: from the nearest replay-verified anchor when there is one (and it yields a text
    // hashing to the content id this baseline names), otherwise a full replay, as before.
    crate::lifecycle_cache::materialize_edited_text_anchored(
        layout,
        object_store,
        baseline_block_id,
        horizon_id,
        base.node_id,
        base.blob_id,
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
