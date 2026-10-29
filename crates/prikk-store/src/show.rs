//! `prikk show` (RFC 142): what a block or patch changed.
//!
//! Content comes straight from the patch payloads (RFC 142 §3 — no replay needed): `EditText`
//! carries its own before/after span text, and every other kind carries its own path, mode, or
//! blob reference inline. Only the three node-addressed kinds (`EditText`, `ChangePerm`,
//! `ReplaceBinary`) carry no path and need resolving: at a sealed block's own patch, against that
//! patch's own state -- the block's starting state (its parent's, via
//! `merge_evidence::mainline_or_sole_parent` and `lifecycle_state_at`, RFC 142 §3/§4), folded with
//! the block's own patches in canonical order **up to and including that patch** (stikk 020); at a
//! queued patch, against the folded worktree baseline truncated at that patch (the
//! queued-patch-paths handoff) -- the same per-operation fold either way (`apply_state_effect`),
//! through two different drivers (`apply_patch_ids_with_materialize_fallback` for a sealed block,
//! `apply_queued_patch_envelopes` for the queue), sharing one materialize-and-retry fallback
//! (DC-65) for an `EditText` whose preimage text an earlier edit produced but never separately
//! stored (0.54.0 Part 5c -- a sealed block's own fold did not share this fallback before, and
//! failed outright from the second consecutive edit of a file onward -- the first edit's own
//! preimage is always a real, stored `CreateFile` blob, never unstored, so only the *second* edit
//! onward ever needs this fallback at all; measured directly by reverting the fix and re-running
//! `show::tests::show_resolves_every_block_in_a_chain_of_five_consecutive_text_edits`, not assumed),
//! one replay per invocation, not one per operation, and identical before and after `seal` by
//! construction. Only a **bare
//! sealed patch id**, with no block or queue to resolve against, reports every node-addressed
//! operation unresolved.

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlobKind, BlobPayload, BlockPayload, NodeId, ObjectEnvelope, ObjectId, ObjectType,
    RefStatePayload,
};

use crate::RepositoryLayout;
use crate::author::author_signing::{author_key_ids, require_author_key_id};
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lifecycle_cache::replay::{TextCache, apply_one_patch_with_materialize_fallback};
use crate::merge::evidence::{
    ancestors_inclusive, lifecycle_state_at, lineage_horizon, mainline_or_sole_parent,
};
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation, decode_patch_operations,
};
use crate::patch_set_digest::patch_ids_pending_excluding_received;
use crate::received::list_received_pointers;
use crate::recognition_claim::claims_naming_patch;
use crate::refs::{RefStore, resolve_ref_tip_block};
use crate::wal::Wal;

/// `show`'s own classification of one patch (0.56.0 step 1b): `queued` (this repository's own
/// active WAL), `pending` (accepted via `sync accept`, not reachable from any tip this repository
/// holds -- the identical membership `patch_ids_pending_excluding_received` already computes for
/// `sync pending`), or `sealed` (reached from a block, via this repository's own local ref history
/// or a received pointer's).
///
/// **Precedence, when more than one could apply:** `pending` and `sealed` cannot both hold --
/// `patch_ids_pending_excluding_received` is defined as "accepted, minus reachable from any tip,"
/// so membership in it already proves non-membership in the sealed closure. `queued` and `pending`
/// cannot both hold under ordinary operation either -- a queued patch has no object-store `Patch`
/// object yet at all (`accept_exchange_artifact` writes one directly, `seal` is what first turns a
/// WAL record into one), so the two sets are populated from disjoint sources. `queued` and
/// `sealed` *can* genuinely, transiently overlap, across the real crash window between a block's
/// own write/publish and the active WAL's later truncation (`finish_active_publication_cleanup`
/// truncates only after the block and ref are written). `show()`'s own existing structure already
/// decides this case, for free: it reads the real object first (`object_store.read_object`,
/// covering both `pending` and `sealed`) and only falls back to the WAL scan when that read finds
/// nothing -- so `sealed`/`pending` already win over `queued` by construction, with no additional
/// priority check needed here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PatchState {
    /// Found only in this repository's own active WAL -- committed but not yet sealed.
    Queued,
    /// Accepted (`sync accept`) and not reachable from any tip this repository holds, its own or
    /// received -- the same membership `sync pending` itself reports.
    Pending,
    /// Reached from a block, via this repository's own local ref history or a received pointer's.
    Sealed,
}

/// What `show` resolved a node-addressed operation's path to. RFC 140's "never fatal" rule
/// applies unchanged (RFC 142 §3, control 3): an unresolved node id is reported, not an error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowPathResolution {
    /// A repository-relative path, read from the payload or resolved against the target block's or
    /// queued patch's lifecycle state.
    Path(String),
    /// A node-addressed operation whose node id is not live at the target — hex-encoded, matching
    /// `prikk_hash::to_hex` (RFC 140's own format). Reachable when **one patch** both edits and
    /// deletes the same node before the node is deleted for good (a later patch in the same block
    /// or queue no longer makes an earlier patch's own edit unresolved -- each patch resolves
    /// against its own state, stikk 020), and whenever the target is a bare sealed patch id with no
    /// block or queue to resolve against.
    Unresolved {
        /// Hex-encoded node id that failed to resolve.
        node_id: String,
        /// Why no path was resolved -- `None` for the fold-completed-but-not-live case above
        /// (unchanged from before 0.56.0 step 1b: the node id and the operation sequence already
        /// say everything there is to say). `Some` for every case where no fold was even
        /// attempted: a bare sealed patch id shown directly (`show`'s own scope, RFC 142 §1 --
        /// never searches for a patch's containing block), or a pending patch whose own path
        /// resolution (0.56.0 step 1b) could not find a usable claim. **Never a guess**: when
        /// `Some`, no path was computed at all, not an uncertain one.
        reason: Option<String>,
    },
}

/// A blob's content, rendered only when it is text — RFC 142 §7 refuses binary rendering.
///
/// RFC 142 §6a/§6b: a blob a patch payload names is a content-addressed *id*, not a promise the
/// object store holds it (§3a's correction — DC-65 leaves a text node's pre-edit blob id
/// deliberately unbacked once the node has been edited without a fresh blob write). `show` is a
/// read surface, not `verify`: an *absent* blob (`Ok(None)`) is ambiguous at this layer — the
/// overwhelmingly common cause is DC-65's by-design gap, and `show` cannot tell that case apart
/// from any other blob that simply never made it into the store without replaying the algebra to
/// decide what *should* be stored, a verification path this RFC's own §7 refuses to add — so
/// absence alone degrades to this variant rather than failing the command. **A real error does
/// not degrade**: the object store affirmatively reporting damage (a content-hash mismatch, a type
/// disagreement, a malformed payload, a `SNAPSHOT`-kind blob) is not ambiguous the way absence is,
/// and propagates as a `PrikkError` instead (§6b). A `doctor`-level surface wanting to separate
/// DC-65's specific cause from ordinary absence would need to independently check whether this
/// exact node's lifecycle includes an `EditText` since the blob was last live -- evidence `show`
/// does not gather because nothing here needs it otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowBlobContent {
    /// `BlobKind::Text` — the blob's own bytes, verbatim.
    Text(Vec<u8>),
    /// `BlobKind::Binary` — id and declared size only, never content.
    Binary {
        /// The blob's own object id.
        blob_id: ObjectId,
        /// Declared content size in bytes.
        size: u64,
    },
    /// The blob id is absent from the object store — named and machine-branchable, carrying the
    /// id that failed, never an omitted field or an empty value a consumer could mistake for real
    /// empty content. A real error (wrong type, malformed payload, content-hash mismatch) is not
    /// this variant: it propagates instead (§6b).
    Unavailable {
        /// The blob id that could not be read.
        blob_id: ObjectId,
    },
}

/// `DeleteNode`'s discriminated preimage, content-resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowDeletePreimage {
    /// File or binary node: the deleted blob's content (rendered per [`ShowBlobContent`]).
    File(ShowBlobContent),
    /// Symlink node: the deleted target, already inline in the operation.
    Symlink {
        /// The symlink's former target.
        old_target: String,
    },
}

/// What one operation changed, RFC 142 §2/§6: `EditText` renders its own before/after span,
/// never a synthesized line diff — the span is the truth, not a reconstructed file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShowOperationContent {
    /// A new file's initial content.
    CreateFile {
        /// The new blob's content.
        content: ShowBlobContent,
        /// Mode bits.
        mode: u32,
    },
    /// A deleted node's preimage.
    DeleteNode {
        /// What the node held before deletion.
        preimage: ShowDeletePreimage,
    },
    /// A span-anchored text edit: the exact bytes replaced and their replacement (RFC 134 — a
    /// content-anchored span, not a line range).
    EditText {
        /// The span's bytes before this edit.
        old_span_text: Vec<u8>,
        /// The span's bytes after this edit.
        replacement_text: Vec<u8>,
    },
    /// A binary blob replacement. Each side is [`ShowBlobContent`] the same way `CreateFile`'s
    /// content is: id and declared size only, since binary content is never rendered (RFC 142
    /// §7) — reusing the one degradation idiom rather than a second, `Option`-shaped one for this
    /// operation alone (RFC 142 §6a's follow-up: "follow the local precedent").
    ReplaceBinary {
        /// The replaced blob.
        old: ShowBlobContent,
        /// The new blob.
        new: ShowBlobContent,
    },
    /// A path rename; both endpoints are already in [`ShowOperation::paths`]. RFC 144 §4o.6a's
    /// honesty invariant: a signed `RenamePath` asserts *this signer* rewrote `old` to `new`, so
    /// the asserting signer must be recoverable in the same answer, not a fact a surface can
    /// choose to drop -- this variant cannot be constructed without it.
    RenamePath {
        /// The patch's AUTHOR signature key id -- the signer who asserted this rename. Not a
        /// trust or verification judgement (that is a separate, local question); carries identity
        /// only. **The first AUTHOR signer in canonical order** (key id bytes) when the patch carries
        /// several (RFC 156): its meaning is unchanged, and `author_key_ids` names every signer.
        author_key_id: String,
        /// Every AUTHOR signer's key id, in canonical order; `author_key_id` is its first entry.
        author_key_ids: Vec<String>,
    },
    /// A mode change.
    ChangePerm {
        /// Mode before the change.
        old_mode: u32,
        /// Mode after the change.
        new_mode: u32,
    },
    /// A new symlink node.
    CreateSymlink {
        /// The symlink's target.
        target: String,
    },
}

/// One operation `show` renders.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowOperation {
    /// Stable kind label, matching `QueuedOperationEntry::kind`'s own vocabulary (RFC 140):
    /// `create-file`, `delete-node`, `edit-text`, `rename-path`, `change-perm`, `create-symlink`,
    /// or `replace-binary`.
    pub kind: &'static str,
    /// The path(s) this operation affects, in payload order (two for `rename-path`, one
    /// otherwise).
    pub paths: Vec<ShowPathResolution>,
    /// What this operation changed.
    pub content: ShowOperationContent,
}

/// One patch's own operations, in canonical (`op_seq`) order.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowPatch {
    /// This patch's own signed object id.
    pub patch_id: ObjectId,
    /// This patch's operations.
    pub operations: Vec<ShowOperation>,
    /// `true` when the patch was found in the active WAL -- committed but not yet sealed -- rather
    /// than in the object store (RFC 142 §7a). Kept unchanged alongside [`PatchState`] (0.56.0
    /// step 1b is additive): `state == PatchState::Queued` always agrees with `queued == true`.
    pub queued: bool,
    /// This patch's own classification (0.56.0 step 1b) -- see [`PatchState`]'s own doc.
    pub state: PatchState,
    /// The recognition claim that resolved this pending patch's own node-addressed path, when one
    /// did (0.56.0 step 1b2, R4). `Some(claim_id)` only for a pending patch whose path was
    /// actually resolved through that claim; `None` for every other case -- queued, sealed (via a
    /// block or via a bare id), and every *unresolved* pending-patch case, including one skipped
    /// for an invalid signature or a fold failure. A tool can look up that claim id's own
    /// signature outcome in `sync pending`'s own `sync-pending-v1` listing.
    pub resolved_through_claim_id: Option<ObjectId>,
}

/// `prikk show <block-id|patch-id>` (RFC 142): read the target, decode its patch(es), resolve
/// node-addressed paths against **that patch's own state** when the target is a block -- the
/// block's starting state (its parent's, or empty for a root), folded with the block's own
/// patches in canonical order up to and including that patch (stikk 020) -- or a queued patch's
/// own truncated folded baseline when it is one (the queued-patch-paths handoff): the block's
/// starting state comes from the one shared derivation (`block_starting_state`, built on
/// `lifecycle_state_at`/`mainline_or_sole_parent`), and each patch's own path resolves through the
/// same per-operation fold (`apply_state_effect`) either form uses, so a patch renders identically
/// before and after `seal` by construction, not by two copies agreeing. A block's output is the
/// union of its patches in canonical (`patch_ids`)
/// order (control 5). **Only a bare sealed patch id** has no context to resolve node-addressed
/// operations against -- those are reported unresolved rather than fatal, the same mechanism §3
/// already requires for the edit-then-delete-in-one-patch case, applied here for a different
/// reason (no target block at all, not a node missing from one). RFC 142 §1: a patch id alone
/// carries no block context, and
/// searching for a block that contains it would make the answer depend on what else happens to be
/// sealed -- this is deliberate, not a gap the queued case's fix also closes.
pub fn show(layout: &RepositoryLayout, id: ObjectId) -> Result<Vec<ShowPatch>> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    // 0.56.0 step 1b2, R2: computed once per command, shared by every branch below, rather than
    // once per patch or only in the bare-Patch branch -- `state` is a property of the patch in
    // this repository, the same whichever id names it. A `BTreeSet` (the same idiom
    // `patch_exchange.rs`'s own `pending_patches_with_claims` already uses) so repeated membership
    // checks in the block loop below are not `O(n)` each.
    let pending_set: BTreeSet<ObjectId> = patch_ids_pending_excluding_received(layout)?
        .into_iter()
        .collect();
    let Some(envelope) = object_store.read_object(id)? else {
        return show_queued_patch(layout, &object_store, id);
    };
    match envelope.object_type {
        ObjectType::Block => {
            let block = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
            let (mut state, parent_id) = block_starting_state(&object_store, id, &block)?;
            // 0.54.0 Part 5c: the block's own sealed lineage, for the materialize-and-retry
            // fallback below -- `None` for a root block (no parent, so no earlier sealed lineage
            // to materialize an unstored `EditText` result from at all).
            let sealed_lineage = match parent_id {
                Some(parent_id) => Some((parent_id, lineage_horizon(&object_store, parent_id)?)),
                None => None,
            };
            let mut text_cache = TextCache::new();
            let mut patches = Vec::with_capacity(block.patch_ids.len());
            for patch_id in &block.patch_ids {
                apply_one_patch_with_materialize_fallback(
                    Some(layout),
                    &object_store,
                    *patch_id,
                    &mut state,
                    &mut text_cache,
                    sealed_lineage,
                )?;
                // 0.56.0 step 1b2, R2: this patch may be reachable from no tip this repository
                // holds (its own or received) even though it is sealed into *this* block -- a
                // `received remove` on the only pointer that ever reached this block leaves the
                // block itself still readable by its own id (RFC 142 §1's bare-id read has no
                // reachability check at all), but no longer reachable from any tip. The same
                // membership the bare-Patch branch already used decides this, not a hardcoded
                // `Sealed`.
                // 0.56.0 step 1b2, R2: this patch may be reachable from no tip this repository
                // holds (its own or received) even though it is sealed into *this* block -- a
                // `received remove` on the only pointer that ever reached this block leaves the
                // block itself still readable by its own id (RFC 142 §1's bare-id read has no
                // reachability check at all), but no longer reachable from any tip. The same
                // membership the bare-Patch branch already used decides this, not a hardcoded
                // `Sealed`.
                let patch_state = if pending_set.contains(patch_id) {
                    PatchState::Pending
                } else {
                    PatchState::Sealed
                };
                patches.push(show_patch(
                    &object_store,
                    *patch_id,
                    Some(&state),
                    None,
                    patch_state,
                    None,
                )?);
            }
            Ok(patches)
        }
        ObjectType::Patch => {
            if pending_set.contains(&id) {
                let (lifecycle, reason, resolved_through_claim_id) =
                    match resolve_pending_patch_lifecycle(layout, &object_store, id)? {
                        PendingPathResolution::Resolved(claim_id, state) => {
                            (Some(state), None, Some(claim_id))
                        }
                        PendingPathResolution::Unresolved(reason) => (None, Some(reason), None),
                    };
                Ok(vec![show_patch(
                    &object_store,
                    id,
                    lifecycle.as_ref(),
                    reason.as_deref(),
                    PatchState::Pending,
                    resolved_through_claim_id,
                )?])
            } else {
                Ok(vec![show_patch(
                    &object_store,
                    id,
                    None,
                    Some(
                        "`show` does not search for the block that sealed a patch shown by its \
                         own id; `prikk log` or `prikk tree` finds it",
                    ),
                    PatchState::Sealed,
                    None,
                )?])
            }
        }
        other => Err(PrikkError::UnsupportedObjectType(format!(
            "show requires a Block or Patch id; {id} is {other}"
        ))),
    }
}

/// The starting state a block's own patches fold onto, not the fold itself -- `show()`'s own loop
/// applies the block's patches afterward, one at a time, via
/// `apply_one_patch_with_materialize_fallback` (RFC 142, stikk 020; 0.54.0 Part 5c). The mainline
/// parent's state for a `Merge` block, the sole parent's for `Normal`, empty for `Root` -- the
/// identical policy `lineage_horizon` already walks, shared through [`mainline_or_sole_parent`]
/// rather than re-derived. This is not a second implementation of block-state derivation:
/// `lifecycle_state_at(block_id)` already derives a block's own end state internally, in one pass
/// over this same parent chain; calling it on the *parent* here gives everything up to but not
/// including this block, a narrower call to the existing one, not a new one.
///
/// Also returns the parent id itself (`None` for a `Root` block) -- `show()`'s own caller needs it
/// to compute the block's own `sealed_lineage` (0.54.0 Part 5c: `(parent_id,
/// lineage_horizon(parent_id))`), the materialize-and-retry fallback's own input. Returning it here
/// rather than re-deriving it a second time keeps `mainline_or_sole_parent(block)` called exactly
/// once for this block.
fn block_starting_state(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
    block: &BlockPayload,
) -> Result<(NodeLifecycleState, Option<ObjectId>)> {
    match mainline_or_sole_parent(block) {
        Some(None) => Ok((NodeLifecycleState::new(), None)),
        Some(Some(parent_id)) => Ok((
            lifecycle_state_at(object_store, parent_id)?.state().clone(),
            Some(parent_id),
        )),
        None => Err(PrikkError::UnsupportedObjectType(format!(
            "show requires a block whose state is its parent's state plus its own patches; \
             block {block_id} has {} parents",
            block.parent_block_ids.len()
        ))),
    }
}

/// One patch's own render context, besides its envelope/content -- bundled into one small `Copy`
/// value (0.56.0 step 1b2) so [`patch_from_envelope`] stays under clippy's `too_many_arguments`
/// threshold once `resolved_through_claim_id` joined `lifecycle`/`unresolved_reason`/`queued`/
/// `state`, the same reason `lifecycle_cache/replay.rs`'s own `MaterializeFallbackContext` bundles
/// `layout`/`sealed_lineage` there.
#[derive(Clone, Copy)]
struct PatchRenderContext<'a> {
    lifecycle: Option<&'a NodeLifecycleState>,
    unresolved_reason: Option<&'a str>,
    queued: bool,
    state: PatchState,
    resolved_through_claim_id: Option<ObjectId>,
}

fn show_patch(
    object_store: &impl ObjectReader,
    patch_id: ObjectId,
    lifecycle: Option<&NodeLifecycleState>,
    unresolved_reason: Option<&str>,
    state: PatchState,
    resolved_through_claim_id: Option<ObjectId>,
) -> Result<ShowPatch> {
    let envelope = object_store
        .read_typed(patch_id, ObjectType::Patch)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Patch {patch_id}")))?;
    patch_from_envelope(
        object_store,
        &envelope,
        patch_id,
        PatchRenderContext {
            lifecycle,
            unresolved_reason,
            queued: false,
            state,
            resolved_through_claim_id,
        },
    )
}

/// RFC 142 §7a, on stikk's letter 011: an id the object store does not hold may be a patch that is
/// committed but not yet sealed -- `status --format json` prints exactly those ids. Looked up in the
/// same active-WAL records the queue enumeration reads, and rendered as it will render once sealed.
///
/// **An id found in neither place is `Precondition`**: a user-supplied id that resolves nowhere is
/// caller-fixable, not damage (RFC 132; RFC 147 §2d). An object a *ref* names and the store lacks is a
/// different path, `verify`'s, and stays `Integrity` there.
///
/// **Node-addressed operations resolve** (the queued-patch-paths handoff, RFC 142): against the folded
/// baseline -- the sealed tip with the queue folded on top, exactly as `commit` and `worktree-status`
/// derive it -- truncated after this patch, so the state is the one *at* this patch, not the queue's
/// end. A patch therefore renders identically before and after `seal`, apart from `queued`.
fn show_queued_patch(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    id: ObjectId,
) -> Result<Vec<ShowPatch>> {
    let replay = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).replay()?;
    let Some(index) = replay.records.iter().position(|record| {
        record.envelope.object_type == ObjectType::Patch && record.envelope.object_id() == id
    }) else {
        return Err(PrikkError::Precondition(format!(
            "no object {id} in the object store or the active WAL; `prikk status --format json` \
             lists queued patch ids, `prikk log` sealed ones"
        )));
    };
    let lifecycle = queued_patch_lifecycle_state(layout, object_store, &replay, index)?;
    let record = replay.records.get(index).ok_or_else(|| {
        PrikkError::Integrity(format!(
            "queued patch at index {index} vanished from the active WAL"
        ))
    })?;
    Ok(vec![patch_from_envelope(
        object_store,
        &record.envelope,
        id,
        PatchRenderContext {
            lifecycle: Some(&lifecycle),
            unresolved_reason: None,
            queued: true,
            state: PatchState::Queued,
            resolved_through_claim_id: None,
        },
    )?])
}

/// The folded baseline state at one queued patch, for [`show_queued_patch`]. The active WAL holding
/// this exact patch record is what makes `read_active_ref_metadata` meaningful here: a record found in
/// it already means the WAL is non-empty, so `Missing`/`Invalid` ownership metadata at this point is
/// the WAL's own damage, not a state this function degrades -- the same classification
/// `resolve_folded_worktree_baseline_up_to_queued_patch` would give it internally, surfaced here before
/// it, only to name the ref for the call.
fn queued_patch_lifecycle_state(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    active_replay: &crate::wal::WalReplay,
    patch_index: usize,
) -> Result<NodeLifecycleState> {
    let ref_name = match crate::read_active_ref_metadata(layout)? {
        crate::ActiveRefMetadata::Valid(ref_name) => ref_name,
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
    let baseline =
        crate::patch_replay::resolve_folded_worktree_baseline_up_to_queued_patch_with_own_cache(
            layout,
            object_store,
            &ref_name,
            active_replay,
            patch_index,
        )?;
    Ok(baseline.state)
}

/// One patch's operations from its envelope, wherever the envelope was read from.
fn patch_from_envelope(
    object_store: &impl ObjectReader,
    envelope: &ObjectEnvelope,
    patch_id: ObjectId,
    context: PatchRenderContext<'_>,
) -> Result<ShowPatch> {
    let operations = decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?;
    let operations = operations
        .iter()
        .map(|operation| {
            show_operation(
                object_store,
                envelope,
                operation,
                context.lifecycle,
                context.unresolved_reason,
            )
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(ShowPatch {
        patch_id,
        operations,
        queued: context.queued,
        state: context.state,
        resolved_through_claim_id: context.resolved_through_claim_id,
    })
}

/// RFC 142 §6b: fallible again, unlike round 2's infallible shape -- three of the seven kinds may
/// now propagate a real `PrikkError` (an unreadable blob's *error* case, not its absence), and a
/// `Result` at this layer is the cleanest way to let that reach `show_patch`'s own `?` rather than
/// inventing a second signalling path alongside [`ShowBlobContent::Unavailable`].
fn show_operation(
    object_store: &impl ObjectReader,
    envelope: &ObjectEnvelope,
    operation: &DecodedPatchOperation,
    lifecycle: Option<&NodeLifecycleState>,
    unresolved_reason: Option<&str>,
) -> Result<ShowOperation> {
    Ok(match &operation.kind {
        DecodedOperationKind::CreateFile {
            path,
            blob_id,
            mode,
            ..
        } => {
            let content = show_blob_content(object_store, *blob_id)?;
            ShowOperation {
                kind: "create-file",
                paths: vec![ShowPathResolution::Path(path.clone())],
                content: ShowOperationContent::CreateFile {
                    content,
                    mode: *mode,
                },
            }
        }
        DecodedOperationKind::DeleteNode { path, preimage, .. } => {
            let preimage = match preimage {
                DecodedDeletePreimage::File { old_blob_id, .. } => {
                    ShowDeletePreimage::File(show_blob_content(object_store, *old_blob_id)?)
                }
                DecodedDeletePreimage::Symlink { old_target } => ShowDeletePreimage::Symlink {
                    old_target: old_target.clone(),
                },
            };
            ShowOperation {
                kind: "delete-node",
                paths: vec![ShowPathResolution::Path(path.clone())],
                content: ShowOperationContent::DeleteNode { preimage },
            }
        }
        DecodedOperationKind::EditText {
            node_id,
            old_span_text,
            replacement_text,
            ..
        } => ShowOperation {
            kind: "edit-text",
            paths: vec![resolve_node_path(*node_id, lifecycle, unresolved_reason)],
            content: ShowOperationContent::EditText {
                old_span_text: old_span_text.clone(),
                replacement_text: replacement_text.clone(),
            },
        },
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => ShowOperation {
            kind: "replace-binary",
            paths: vec![resolve_node_path(*node_id, lifecycle, unresolved_reason)],
            content: ShowOperationContent::ReplaceBinary {
                old: show_blob_content(object_store, *old_blob_id)?,
                new: show_blob_content(object_store, *new_blob_id)?,
            },
        },
        DecodedOperationKind::RenamePath {
            old_path, new_path, ..
        } => ShowOperation {
            kind: "rename-path",
            paths: vec![
                ShowPathResolution::Path(old_path.clone()),
                ShowPathResolution::Path(new_path.clone()),
            ],
            content: ShowOperationContent::RenamePath {
                author_key_id: require_author_key_id(envelope)?,
                author_key_ids: author_key_ids(envelope),
            },
        },
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => ShowOperation {
            kind: "change-perm",
            paths: vec![resolve_node_path(*node_id, lifecycle, unresolved_reason)],
            content: ShowOperationContent::ChangePerm {
                old_mode: *old_mode,
                new_mode: *new_mode,
            },
        },
        DecodedOperationKind::CreateSymlink { path, target, .. } => ShowOperation {
            kind: "create-symlink",
            paths: vec![ShowPathResolution::Path(path.clone())],
            content: ShowOperationContent::CreateSymlink {
                target: target.clone(),
            },
        },
    })
}

/// `unresolved_reason` is consulted only when `lifecycle` itself is `None` -- no fold was even
/// attempted, so a bare node id alone would say nothing about why. When `lifecycle` is `Some` but
/// the node is not live in it, the reason stays `None` (unchanged from before 0.56.0 step 1b): a
/// fold that completed fine but found the node not live already says everything relevant through
/// the node id and the operation sequence around it.
fn resolve_node_path(
    node_id: NodeId,
    lifecycle: Option<&NodeLifecycleState>,
    unresolved_reason: Option<&str>,
) -> ShowPathResolution {
    if let Some(state) = lifecycle {
        if let Some(node) = state.live_node(&node_id) {
            return ShowPathResolution::Path(node.path.as_str().to_string());
        }
        return ShowPathResolution::Unresolved {
            node_id: prikk_hash::to_hex(node_id.as_bytes()),
            reason: None,
        };
    }
    ShowPathResolution::Unresolved {
        node_id: prikk_hash::to_hex(node_id.as_bytes()),
        reason: unresolved_reason.map(str::to_string),
    }
}

/// RFC 142 §6b: keeps the `Option` the object store hands out instead of collapsing it into a bare
/// `Result` the way round 2's own `read_blob` did -- that collapse was the structural cause of
/// treating a content-hash-verified corrupt object the same as a merely-absent one. `Ok(None)` is
/// the only case this function's caller may degrade; every `Err` here is the store affirmatively
/// reporting damage (`object_store.rs`'s own id-recomputation check among them) and must reach the
/// caller as one.
fn read_blob(object_store: &impl ObjectReader, blob_id: ObjectId) -> Result<Option<BlobPayload>> {
    let Some(envelope) = object_store.read_typed(blob_id, ObjectType::Blob)? else {
        return Ok(None);
    };
    BlobPayload::decode_canonical(&envelope.canonical_payload).map(Some)
}

/// RFC 142 §6a/§6b: a blob id a patch payload names is a reference, not a guarantee the object
/// store holds it (§3a) -- absence (`Ok(None)`) degrades to [`ShowBlobContent::Unavailable`], the
/// same "report, do not fail" treatment [`resolve_node_path`] already gives an unresolvable node
/// id, because DC-65 makes a text node's pre-edit blob id absent by design and `show` cannot tell
/// that case apart from a lost object by absence alone. **An `Err` is different**: the object store
/// affirmatively reporting damage (a content-hash mismatch, a type disagreement, a malformed
/// payload, an `Io` failure) is not "unbacked by design" under any cause this crate knows, so it
/// propagates rather than degrading -- round 1 called an intact repository damaged; degrading every
/// `Err` here would have called a damaged one intact, which is worse because it is quieter. A
/// `SNAPSHOT`-kind blob is the same: no file-content operation legitimately names one, so decoding
/// one here is itself an integrity problem, not an absence.
fn show_blob_content(
    object_store: &impl ObjectReader,
    blob_id: ObjectId,
) -> Result<ShowBlobContent> {
    match read_blob(object_store, blob_id)? {
        None => Ok(ShowBlobContent::Unavailable { blob_id }),
        Some(blob) => match blob.blob_kind {
            BlobKind::Text => Ok(ShowBlobContent::Text(blob.content)),
            BlobKind::Binary => Ok(ShowBlobContent::Binary {
                blob_id,
                size: blob.declared_size,
            }),
            BlobKind::Snapshot => Err(PrikkError::Integrity(format!(
                "content blob {blob_id} is a SNAPSHOT blob, not file content"
            ))),
        },
    }
}

/// A pending patch's own path resolution, through the claim(s) naming it (0.56.0 step 1b; step
/// 1b2, R4: the resolving claim's own id is now carried too).
enum PendingPathResolution {
    /// A usable claim resolved: its own id, and the folded state to resolve this patch's own
    /// node-addressed operations against.
    Resolved(ObjectId, NodeLifecycleState),
    /// No claim resolved -- the most specific reason among every claim tried (0.56.0 step 1b2,
    /// R4's own priority order: see [`UnresolvedReasonPriority`]), or the one and only reason,
    /// when every claim failed the same way.
    Unresolved(String),
}

/// How specific an unresolved reason is, among every claim [`resolve_pending_patch_lifecycle`]
/// tried for one pending patch (0.56.0 step 1b2, R4, ruled explicitly): when several claims fail
/// for different reasons, the *most specific* one is reported, not simply whichever was tried
/// last. Declaration order is priority order -- `derive(Ord)` ranks an earlier variant lower, and
/// [`record_if_more_specific`] keeps the lowest-ranked (most specific) reason seen, so this order
/// *is* the ranking: an invalid signature (the claim itself cannot be trusted at all) outranks a
/// fold failure (the claim is trusted enough to try, but its own patches do not fit its own
/// declared parent), which outranks a chain (the claim is internally sound, only its parent is not
/// yet realized), which outranks a merge (the claim is internally sound, there is simply no
/// mainline to pick). Ties (two claims failing for the identical reason) are broken by claim-id
/// order, since claims are already tried in that order and a reason already recorded at a given
/// rank is never displaced by another at the same rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum UnresolvedReasonPriority {
    /// The claim's own MAINTAINER signature is invalid -- skipped before any fold is attempted.
    InvalidSignature,
    /// The claim's own patches do not fold against its own declared parent.
    FoldFailure,
    /// The claim's one declared parent is itself only pending -- an unrealized chain.
    Chain,
    /// The claim declares more than one parent, with no mainline to choose.
    Merge,
}

/// Record `reason` at `priority` in `best`, keeping whichever of the two is more specific (the
/// lower-ranked [`UnresolvedReasonPriority`]) -- see that type's own doc for why declaration order
/// is priority order, and why this never displaces a reason already recorded at the same rank
/// (claim-id order is preserved that way, since claims are visited in that order).
fn record_if_more_specific(
    best: &mut Option<(UnresolvedReasonPriority, String)>,
    priority: UnresolvedReasonPriority,
    reason: String,
) {
    let replace = match best {
        None => true,
        Some((existing_priority, _)) => priority < *existing_priority,
    };
    if replace {
        *best = Some((priority, reason));
    }
}

/// Resolve a **pending** patch's own node-addressed operations against the claim that names it
/// (0.56.0 step 1 part 1b; planeter letter 2026-10-21 §2's own "parent block id the claim names";
/// step 1b2, R4: a reader must not refuse because of a claim, and must say which claim resolved
/// the path).
///
/// **"The claim's parent block" is `RecognitionClaimPayload::parent_block_ids`, not `block_id`.**
/// `block_id` is the block the claim says these patches *would* be sealed into -- the block it
/// recognizes (`target_block_id`, in `extract-report-v1`'s and `sync-pending-v1`'s own naming). It
/// cannot be what this task means: for a `Consistent` claim (the honest, unmodified kind),
/// `block_id`'s own `patch_ids` equal the claim's own `patch_ids` verbatim
/// (`check_recognition_claim_consistency`), so the moment `block_id` is itself reachable from any
/// tip, those same patch ids are already inside the sealed closure
/// `patch_ids_pending_excluding_received` excludes -- the patch would already read `sealed`, never
/// `pending`. "The claim's parent block, sealed or received" has to mean the *pre-existing*
/// historical anchor the prospective block would attach onto: `parent_block_ids`, the named
/// block's own parents.
///
/// Tries every held claim naming this patch (0.56.0 step 1a already proved more than one can), in
/// claim id order (`claims_naming_patch`'s own order -- the same convention `claims[]` uses
/// elsewhere), and uses the first that resolves:
/// - **An invalid signature skips the claim before any fold is attempted** (R4): a claim whose own
///   MAINTAINER signature `claims_naming_patch` reports invalid (the same case `sync-pending-v1`
///   reports as `"invalid"`, "the one value a tool must treat as an alarm") must not be allowed to
///   choose the path `show` prints.
/// - `parent_block_ids` empty (a root claim): folds from an empty [`NodeLifecycleState`], the same
///   starting point [`block_starting_state`]'s own `Root` arm already uses.
/// - `parent_block_ids` exactly one, and that one block id is itself sealed or received (checked
///   against the same local-ref-tip/received-pointer-tip union [`patch_ids_pending_excluding_received`]
///   already walks, narrowed here to block-id membership via [`ancestors_inclusive`] instead of
///   patch-id accumulation): folds from that block's own end state
///   ([`lifecycle_state_at`], the same call [`block_starting_state`] makes for a real block's own
///   parent).
/// - `parent_block_ids` exactly one, but not itself sealed or received: this claim is part of an
///   unrealized chain -- skipped, named as the reason if no other claim does better.
/// - `parent_block_ids` with more than one entry (a merge-shaped claim): no `mainline_parent_id`
///   exists on `RecognitionClaimPayload` to pick one from (unlike `BlockPayload`) -- skipped, named
///   as its own distinct reason.
/// - **A fold failure becomes that claim's own reason, not a propagated `Err`** (R4): the fold
///   itself is the same [`apply_one_patch_with_materialize_fallback`] driver `show()`'s own block
///   loop already uses, over the claim's own `patch_ids` (its stated order, not a real block's)
///   instead of `block.patch_ids`, stopping once the target patch has been applied -- but an
///   untrusted claim's own declared parent not actually fitting its patches (an `EditText` naming
///   a node absent there, for example) must not make the whole `show <patch>` call refuse; the
///   next claim (if any) is tried instead, the same "readers report, gates refuse" rule 1a's R1
///   already established for `sync pending`'s own signature handling.
///
/// **When several claims fail, the most specific reason is reported** (R4, ruled explicitly), not
/// simply whichever was tried last -- see [`UnresolvedReasonPriority`]'s own doc for the order and
/// [`record_if_more_specific`] for how it is kept.
fn resolve_pending_patch_lifecycle(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    patch_id: ObjectId,
) -> Result<PendingPathResolution> {
    let claims = claims_naming_patch(layout, patch_id)?;
    if claims.is_empty() {
        return Ok(PendingPathResolution::Unresolved(
            "no held recognition claim names this pending patch yet".to_string(),
        ));
    }

    let mut best_reason: Option<(UnresolvedReasonPriority, String)> = None;
    for claim in &claims {
        if let Some(reason) = &claim.invalid_signature_reason {
            record_if_more_specific(
                &mut best_reason,
                UnresolvedReasonPriority::InvalidSignature,
                format!(
                    "the claim naming this patch has an invalid MAINTAINER signature, so it is \
                     skipped rather than used to resolve a path: {reason}"
                ),
            );
            continue;
        }
        let anchor = match claim.payload.parent_block_ids.as_slice() {
            [] => None,
            [parent] => {
                if block_is_sealed_or_received(layout, object_store, *parent)? {
                    Some(*parent)
                } else {
                    record_if_more_specific(
                        &mut best_reason,
                        UnresolvedReasonPriority::Chain,
                        format!(
                            "the claim naming this patch names parent block {parent}, which is \
                             not itself sealed or received yet -- it is still part of an \
                             unrealized chain of claims"
                        ),
                    );
                    continue;
                }
            }
            _ => {
                record_if_more_specific(
                    &mut best_reason,
                    UnresolvedReasonPriority::Merge,
                    "the claim naming this patch names more than one parent block, with no \
                     mainline to choose"
                        .to_string(),
                );
                continue;
            }
        };
        let mut state = match anchor {
            Some(anchor_id) => lifecycle_state_at(object_store, anchor_id)?.state().clone(),
            None => NodeLifecycleState::new(),
        };
        let sealed_lineage = match anchor {
            Some(anchor_id) => Some((anchor_id, lineage_horizon(object_store, anchor_id)?)),
            None => None,
        };
        let mut text_cache = TextCache::new();
        let mut fold_failed = false;
        for claim_patch_id in &claim.payload.patch_ids {
            if let Err(err) = apply_one_patch_with_materialize_fallback(
                Some(layout),
                object_store,
                *claim_patch_id,
                &mut state,
                &mut text_cache,
                sealed_lineage,
            ) {
                record_if_more_specific(
                    &mut best_reason,
                    UnresolvedReasonPriority::FoldFailure,
                    format!(
                        "the claim naming this patch does not fold against its own declared \
                         parent: {err}"
                    ),
                );
                fold_failed = true;
                break;
            }
            if *claim_patch_id == patch_id {
                return Ok(PendingPathResolution::Resolved(claim.claim_id, state));
            }
        }
        debug_assert!(
            fold_failed,
            "claims_naming_patch only returns a claim whose own patch_ids contains patch_id; \
             the loop above must either resolve or record a fold failure before falling through"
        );
    }
    let (_, reason) = best_reason.ok_or_else(|| {
        PrikkError::Integrity(
            "resolve_pending_patch_lifecycle tried every claim naming this patch without \
             reaching a resolution or recording a reason -- internal inconsistency"
                .to_string(),
        )
    })?;
    Ok(PendingPathResolution::Unresolved(reason))
}

/// Resolve one ref/received pointer's own `RefState` to its tip block id -- the identical
/// resolution [`crate::patch_exchange::accepted_but_unsealed_patch_ids`]'s own per-pointer loop
/// already performs. Duplicated here at a few lines, rather than reaching into that module's
/// private helper and bumping its visibility, to keep this file's own tip walk self-contained for
/// a block-id membership question [`crate::patch_exchange`] has no existing caller for.
fn resolve_tip_block_id(
    object_store: &impl ObjectReader,
    pointer_name: &str,
    ref_state_id: ObjectId,
) -> Result<ObjectId> {
    let ref_state_envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {pointer_name} names missing RefState {ref_state_id}"
            ))
        })?;
    let ref_state_payload = RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )?;
    let (tip_block_id, _tag_envelope) = resolve_ref_tip_block(object_store, &ref_state_payload)?;
    Ok(tip_block_id)
}

/// Is `block_id` itself sealed (reachable from a local ref tip) or received (reachable from a
/// received pointer's tip)? No existing predicate answers this -- `patch_ids_reachable_from_block`
/// and its callers only ever accumulate *patch* ids from an ancestry walk, discarding the ancestor
/// *block* ids [`ancestors_inclusive`] already computes along the way. This reuses that same walk,
/// over the same tip union [`patch_ids_pending_excluding_received`] already walks for patch ids,
/// and checks `block_id`'s own membership in the accumulated block-id keys instead.
fn block_is_sealed_or_received(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<bool> {
    let ref_store = RefStore::new(layout.clone());
    for pointer in ref_store.list_ref_pointers()? {
        let tip = resolve_tip_block_id(object_store, &pointer.ref_name, pointer.ref_state_id)?;
        if ancestors_inclusive(object_store, tip)?.contains_key(&block_id) {
            return Ok(true);
        }
    }
    for pointer in list_received_pointers(layout)? {
        let tip = resolve_tip_block_id(object_store, &pointer.ref_name, pointer.ref_state_id)?;
        if ancestors_inclusive(object_store, tip)?.contains_key(&block_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests;
