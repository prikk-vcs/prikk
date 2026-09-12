//! Authoritative lifecycle replay — lineage walker and state-effect interpreter
//! (DC-09 Phase 4.4-2c-2a..2c-2b).
//!
//! Walks the v1 single-parent block lineage from a baseline back to a genesis horizon over the
//! **real** object store, failing closed on every malformed-lineage condition, then applies each
//! block's patch operations in apply order (oldest first) into a [`NodeLifecycleState`].
//!
//! Per the O1 ruling, replay must apply every lifecycle-affecting state effect *exactly* or fail
//! closed. As of 2c-2d **all** lifecycle-affecting operations have exact effects: `CreateFile`,
//! `CreateSymlink`, `DeleteNode`, `RenamePath`, `ChangePerm`, `ReplaceBinary`, and `EditText` (the
//! last derives a new full-text `BlobPayload(Text, …)` content id by materializing the edited
//! text).

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use prikk_error::PrikkError;
use prikk_object::{
    BlobKind, BlockKind, BlockPayload, NodeId, NodeKind, ObjectId, ObjectType, text_span_hash,
};

use super::{BlobContentResolver, BlobKindResolver, StoreBackedResolver};
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState};
use crate::object_store::ObjectReader;
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations,
};
use crate::path::RepoPath;
use crate::text_span::{self, TextSpanResolutionFailure};

/// Replay-local materialized text for edited text nodes, keyed by `node_id`. Transient to a replay
/// pass; never part of the persisted lifecycle index (which stores only `blob_id` + `mode`).
pub type TextCache = BTreeMap<NodeId, Vec<u8>>;

/// Structured lifecycle-replay error taxonomy (carry-forward P2-3).
///
/// These are the classes a replay / fallback caller branches on.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LifecycleReplayError {
    /// A block referenced by the lineage walk is absent. Never treated as genesis (P2-1).
    MissingBlockInLineage {
        /// The block the lineage walk expected and did not find.
        block_id: ObjectId,
    },
    /// A block object exists but cannot be read as a `Block` (wrong type, decode failure).
    UnreadableBlockInLineage {
        /// The block that exists but would not decode.
        block_id: ObjectId,
        /// What decoding said.
        detail: String,
    },
    /// A block in the lineage window has more parents than this walk can follow: either a
    /// non-`Merge` block with more than one parent, or a `Merge` block whose `mainline_parent_id`
    /// is missing or does not name one of its own `parent_block_ids` (DC-75).
    MergeLineageUnsupported {
        /// The block whose parentage the walk cannot follow.
        block_id: ObjectId,
        /// How many parents it declares.
        parent_count: usize,
    },
    /// The single-parent walk revisited a block — a cycle in a store that should be a DAG.
    LineageCycle {
        /// The block reached a second time.
        block_id: ObjectId,
    },
    /// The walk reached genesis, but genesis is not the claimed horizon (v1 adequate-horizon).
    HorizonNotInLineage {
        /// The horizon that was claimed and not met.
        horizon_id: ObjectId,
    },
    /// A decoded operation could not be applied to the replayed state: the target node is not
    /// live, a path is occupied, restoration-equivalence failed, or a stated old-state field
    /// (mode/path) disagrees with the replayed reality. Distinct from a decode failure.
    InconsistentLifecycleEffect {
        /// Which state the operation disagreed with, and how.
        detail: String,
    },
    /// A patch referenced by a block is missing or cannot be decoded.
    MalformedPatchInLineage {
        /// The patch a block referenced.
        patch_id: ObjectId,
        /// Why it could not be read.
        detail: String,
    },
    /// A blob required to resolve a lifecycle state effect is absent. (2c-2b onward.)
    MissingBlobForLifecycleEffect {
        /// The blob an effect needed.
        blob_id: ObjectId,
    },
    /// An `EditText` span could not be uniquely localized in the replayed text during sealed-history
    /// replay. This is an integrity failure (the sealed edit applied cleanly when authored), not a
    /// user/merge conflict.
    TextSpanResolutionFailed {
        /// The node whose text was being replayed.
        node_id: NodeId,
        /// The span identity that could not be localized.
        span_id: [u8; 32],
        /// Which way the localization failed -- absent, ambiguous, or anchored elsewhere.
        reason: TextSpanResolutionFailure,
    },
}

impl fmt::Display for LifecycleReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingBlockInLineage { block_id } => write!(
                f,
                "lifecycle replay: block {block_id} is missing and cannot be treated as genesis"
            ),
            Self::UnreadableBlockInLineage { block_id, detail } => {
                write!(
                    f,
                    "lifecycle replay: block {block_id} is unreadable ({detail})"
                )
            }
            Self::MergeLineageUnsupported {
                block_id,
                parent_count,
            } => write!(
                f,
                "lifecycle replay: block {block_id} has {parent_count} parents and no valid \
                 mainline parent to derive state from"
            ),
            Self::LineageCycle { block_id } => {
                write!(f, "lifecycle replay: cycle detected at block {block_id}")
            }
            Self::HorizonNotInLineage { horizon_id } => write!(
                f,
                "lifecycle replay: walk reached genesis without crossing the claimed horizon \
                 {horizon_id}"
            ),
            Self::InconsistentLifecycleEffect { detail } => write!(
                f,
                "lifecycle replay: operation could not be applied to the replayed state ({detail})"
            ),
            Self::MalformedPatchInLineage { patch_id, detail } => {
                write!(
                    f,
                    "lifecycle replay: patch {patch_id} is malformed ({detail})"
                )
            }
            Self::MissingBlobForLifecycleEffect { blob_id } => {
                write!(
                    f,
                    "lifecycle replay: blob {blob_id} required for a state effect is missing"
                )
            }
            Self::TextSpanResolutionFailed {
                node_id,
                span_id,
                reason,
            } => {
                write!(f, "lifecycle replay: EditText span on node ")?;
                for byte in node_id.as_bytes() {
                    write!(f, "{byte:02x}")?;
                }
                write!(f, " (span_id ")?;
                for byte in span_id {
                    write!(f, "{byte:02x}")?;
                }
                write!(f, ") could not be localized: {reason}")
            }
        }
    }
}

impl From<LifecycleReplayError> for PrikkError {
    fn from(value: LifecycleReplayError) -> Self {
        Self::Integrity(value.to_string())
    }
}

/// Read a `Block` payload, distinguishing absence from unreadability so the walk can fail closed
/// with the correct structured class. `pub(crate)`: DC-64's incremental baseline cache
/// (`lifecycle_cache/incremental.rs`) reads a single candidate block directly, without walking the
/// full lineage, to decide whether it is a single-parent child of a cached predecessor.
pub(in crate::lifecycle_cache) fn read_block(
    reader: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<BlockPayload, LifecycleReplayError> {
    let envelope = reader.read_object(block_id).map_err(|e| {
        LifecycleReplayError::UnreadableBlockInLineage {
            block_id,
            detail: e.to_string(),
        }
    })?;
    let Some(envelope) = envelope else {
        return Err(LifecycleReplayError::MissingBlockInLineage { block_id });
    };
    if envelope.object_type != ObjectType::Block {
        return Err(LifecycleReplayError::UnreadableBlockInLineage {
            block_id,
            detail: format!("object is not a Block ({} found)", envelope.object_type),
        });
    }
    BlockPayload::decode_canonical(&envelope.canonical_payload).map_err(|e| {
        LifecycleReplayError::UnreadableBlockInLineage {
            block_id,
            detail: e.to_string(),
        }
    })
}

/// One walked block: its id and decoded payload, retained so replay does not re-read.
type WalkedBlock = (ObjectId, BlockPayload);

/// Source of lineage blocks for walking. Each implementor reads a block **once** and yields its
/// own `Block` view (the full payload for replay; just the parent ids for provenance), failing
/// closed on missing, unreadable, or non-Block objects. This is the **single** seam through which
/// both authoritative replay and cache provenance read the lineage, so the two cannot drift on
/// which blocks are in the window or in what order — and, since the walk returns what it read, on a
/// file-backed store replay never re-reads a block whose contents could have changed (E4).
pub(in crate::lifecycle_cache) trait LineageBlockReader {
    /// What a single lineage-block read yields to the caller.
    type Block;
    /// Read one lineage block exactly once.
    fn read_lineage_block(&self, block_id: ObjectId) -> Result<Self::Block, LifecycleReplayError>;
    /// The parent ids the walk should see for a read block, drives the single-parent walk rule.
    /// For a `Merge` block (DC-75) with a validly recorded mainline parent, this is `[mainline]` —
    /// state derivation and replay follow the mainline only, never the secondary parent, so the walk
    /// never needs to change shape for `Merge`. A `Merge` block with a missing or invalid mainline
    /// parent falls through to its raw (two-element) parent list, so the existing `>1 parent` guard
    /// below fails closed on it exactly as it would on any other malformed multi-parent block.
    fn parents_of(block: &Self::Block) -> Vec<ObjectId>;
}

/// Reader-backed lineage source: reads and **retains** the full block payload, preserving the
/// missing/unreadable distinction. Replay applies patches from the payload returned by the walk,
/// so no block is read a second time.
struct ReaderLineage<'a, R: ObjectReader>(&'a R);

impl<R: ObjectReader> LineageBlockReader for ReaderLineage<'_, R> {
    type Block = BlockPayload;

    fn read_lineage_block(&self, block_id: ObjectId) -> Result<BlockPayload, LifecycleReplayError> {
        read_block(self.0, block_id)
    }

    fn parents_of(block: &BlockPayload) -> Vec<ObjectId> {
        if block.kind == BlockKind::Merge {
            if let Some(mainline) = block.mainline_parent_id {
                if block.parent_block_ids.contains(&mainline) {
                    return vec![mainline];
                }
            }
            // Missing or invalid mainline: fall through to the raw (two-element) list below, so
            // the walker's existing `>1 parent` guard fails closed on it, unchanged.
        }
        block.parent_block_ids.clone()
    }
}

/// Canonical v1 single-parent lineage walk — the **single source of truth** for the lineage
/// window. From `baseline`, follow single parents (as `R::parents_of` reduces them) to repository
/// genesis (a block with no parents), which MUST equal `horizon`. Returns the chain in apply order
/// (genesis/horizon first … baseline last), each entry paired with the `Block` the walk read for
/// it (so replay does not re-read). Fails closed on more than one parent (per `R::parents_of`'s own
/// reduction), a cycle, or a genesis that is not the claimed horizon.
///
/// **Whether a well-formed `Merge` block is walked or refused depends entirely on the caller's own
/// `R: LineageBlockReader`, not on this function.** `ReaderLineage` (authoritative replay, below)
/// reduces a well-formed `Merge` to `[mainline]` before this walk ever sees more than one parent —
/// only a malformed `Merge` (missing/invalid mainline) fails closed here. `ResolverLineage`
/// (`cache_ladder.rs`, operational-provenance verification) returns every parent verbatim, so
/// *any* `Merge` block — well-formed or not — fails closed for that caller: lifecycle-cache
/// provenance does not yet support merge-aware baselines (`ParentPolicy::Dc13MergeAware` is
/// reserved for this, unused in v1). Read this function's own behaviour from the `R` its caller
/// supplies, never assume one caller's shape from another's.
pub(in crate::lifecycle_cache) fn walk_single_parent_chain<R: LineageBlockReader>(
    blocks: &R,
    baseline: ObjectId,
    horizon: ObjectId,
) -> Result<Vec<(ObjectId, R::Block)>, LifecycleReplayError> {
    walk_single_parent_chain_inner(blocks, baseline, Some(horizon))
}

fn walk_single_parent_chain_inner<R: LineageBlockReader>(
    blocks: &R,
    baseline: ObjectId,
    expected_horizon: Option<ObjectId>,
) -> Result<Vec<(ObjectId, R::Block)>, LifecycleReplayError> {
    let mut chain: Vec<(ObjectId, R::Block)> = Vec::new();
    let mut visited: BTreeSet<ObjectId> = BTreeSet::new();
    let mut current = baseline;

    loop {
        if !visited.insert(current) {
            return Err(LifecycleReplayError::LineageCycle { block_id: current });
        }
        let block = blocks.read_lineage_block(current)?;
        let next = match R::parents_of(&block).as_slice() {
            [] => None,
            [parent] => Some(*parent),
            other => {
                return Err(LifecycleReplayError::MergeLineageUnsupported {
                    block_id: current,
                    parent_count: other.len(),
                });
            }
        };
        chain.push((current, block));
        match next {
            None => {
                if expected_horizon.is_some_and(|horizon| current != horizon) {
                    return Err(LifecycleReplayError::HorizonNotInLineage {
                        horizon_id: expected_horizon.unwrap_or(current),
                    });
                }
                break;
            }
            Some(parent) => current = parent,
        }
    }

    chain.reverse();
    Ok(chain)
}

/// Walk the v1 single-parent lineage from `baseline` back to the genesis `horizon`, returning the
/// chain in **apply order** with each block's payload — read exactly once by the walk and retained
/// for patch application. Built on the shared [`walk_single_parent_chain`]; performs no second
/// block read (E4).
fn walk_lineage(
    reader: &impl ObjectReader,
    baseline: ObjectId,
    horizon: ObjectId,
) -> Result<Vec<WalkedBlock>, LifecycleReplayError> {
    walk_single_parent_chain(&ReaderLineage(reader), baseline, horizon)
}

/// Replay the lineage from `baseline` back to `horizon`, applying each operation's lifecycle
/// state effect into a [`NodeLifecycleState`]. Returns the reconstructed state.
///
/// **This is not yet `ReplayDerivedLifecycleState`**; the producer
/// (`super::replay_derived_state`) wraps it through `ReplayDerivedLifecycleState::from_replay`.
/// Every operation kind has an exact effect; blob kinds and text content are resolved through the
/// real store-backed [`StoreBackedResolver`].
pub(in crate::lifecycle_cache) fn replay_lineage(
    reader: &impl ObjectReader,
    baseline: ObjectId,
    horizon: ObjectId,
) -> Result<NodeLifecycleState, LifecycleReplayError> {
    let chain = walk_lineage(reader, baseline, horizon)?;
    let (state, _text_cache) = replay_chain_with_appended_patches(reader, &chain, &[], false)?;
    Ok(state)
}

/// Full replay of the lineage, additionally returning the materialized-text cache accumulated along
/// the way (DC-65). A `TextFile` node's `blob_id` after any `EditText` is a content identity, not
/// necessarily a stored object (see the DC-65 invariant document); a caller that needs the node's
/// actual current bytes materializes them here, from the diff chain, exactly as this replay pass
/// already does internally to apply later `EditText` operations against the same node.
pub(in crate::lifecycle_cache) fn replay_lineage_with_materialized_text(
    reader: &impl ObjectReader,
    baseline: ObjectId,
    horizon: ObjectId,
) -> Result<(NodeLifecycleState, TextCache), LifecycleReplayError> {
    let chain = walk_lineage(reader, baseline, horizon)?;
    replay_chain_with_appended_patches(reader, &chain, &[], false)
}

/// Apply exactly one already-read block's patches to an existing lifecycle state (DC-64
/// incremental baseline reconstruction). Calls the *identical* `apply_patch_ids`/
/// `apply_state_effect` functions full replay uses — a `TextCache` is created fresh, which is safe
/// because it is a same-pass memoization only (`TextCache`'s doc comment): a miss always falls back
/// to reading the node's actual current blob content, so an empty cache changes nothing about
/// correctness. This is not a second implementation of the fold; it is the same fold's tail,
/// executed in a separate process invocation from the rest of the lineage.
pub(in crate::lifecycle_cache) fn apply_one_block(
    reader: &impl ObjectReader,
    block: &BlockPayload,
    state: &mut NodeLifecycleState,
    require_schema_one: bool,
) -> Result<(), LifecycleReplayError> {
    let blob_resolver = if require_schema_one {
        StoreBackedResolver::new_format2(reader)
    } else {
        StoreBackedResolver::new(reader)
    };
    let mut text_cache = TextCache::new();
    apply_patch_ids(
        reader,
        &block.patch_ids,
        &blob_resolver,
        state,
        &mut text_cache,
        require_schema_one,
    )
}

/// Apply exactly one already-read block's patches to an existing lifecycle state **and** an
/// existing, externally-carried `TextCache` (DC-92). Unlike `apply_one_block`, which creates a
/// fresh cache per call — correct only when the caller processes one block in isolation — this
/// variant is for a caller replaying **several separate blocks in sequence** and needing text
/// materialization to survive between them, exactly as a single continuous full replay would
/// provide. Without this, a node's `TextFile` content identity produced by an *earlier* block's
/// `EditText` (a content identity, not necessarily a stored object — see the DC-65 invariant
/// document) would be unreachable once that earlier call's own local cache was discarded, and a
/// later `EditText` against the same node would fail looking for a blob that was never stored.
/// `crate::lifecycle_cache::incremental`'s own one-block step hits exactly this gap and falls
/// back to full replay rather than solving it (see its module doc); this function is DC-92's
/// solution for the case where blocks are visited **in order**, so there is a real cache to carry.
pub fn apply_one_block_with_text_cache(
    reader: &impl ObjectReader,
    block: &BlockPayload,
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
) -> Result<(), LifecycleReplayError> {
    let blob_resolver = StoreBackedResolver::new_format2(reader);
    apply_patch_ids(
        reader,
        &block.patch_ids,
        &blob_resolver,
        state,
        text_cache,
        true,
    )
}

/// Apply a raw candidate patch id list — not yet a sealed `BlockPayload`, since it names the patches
/// a *proposed* block would carry — onto an existing lifecycle state and carried `TextCache`
/// (DC-92). The tail-only counterpart to [`apply_one_block_with_text_cache`] for the one caller
/// that has a patch list but no block payload yet: `derive_next_state_root`'s own final step,
/// deriving the state a new block's own transition would produce before that block has been signed
/// or persisted. Takes the same carried `text_cache` its parent's lineage resolution accumulated,
/// for the identical reason `apply_one_block_with_text_cache` needs it.
pub fn apply_candidate_patches(
    reader: &impl ObjectReader,
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
    patch_ids: &[ObjectId],
) -> Result<(), LifecycleReplayError> {
    let blob_resolver = StoreBackedResolver::new_format2(reader);
    apply_patch_ids(reader, patch_ids, &blob_resolver, state, text_cache, true)
}

/// Fold a sequence of not-yet-sealed queued patch envelopes (DC-66) onto an already-resolved
/// lifecycle state, using the same [`apply_state_effect`] fold every other replay path uses.
///
/// Unlike [`apply_patch_ids`], the envelopes are supplied directly from `crate::wal::WalRecord`
/// rather than read from the object store by id — `persist_wal_patches`
/// (`crates/prikk-cli/src/seal/support.rs`) only writes queued patches as durable objects at `seal`
/// time, so a WAL record sitting in an unsealed queue has no object to read yet.
///
/// `text_cache` starts empty for this fold, exactly like [`apply_one_block`]'s per-block cache
/// (DC-64/DC-65 §9a). Unlike DC-64's incremental step, there is no "fall back to full replay" escape
/// available here — the unsealed portion of a queue has no independent replay path of its own to fall
/// back to. Instead, on `MissingBlobForLifecycleEffect` for an `EditText` operation, the target node's
/// *sealed* content is materialized once via `materialize_edited_text` (the same DC-65 mechanism
/// `plan_edit_text` already uses to resolve an unstored `EditText` result) to seed the cache, and the
/// operation is retried exactly once. `apply_edit_text` only mutates `state`/`text_cache` after it has
/// successfully resolved the current text, so a failed first attempt leaves both untouched and the
/// retry is safe. `sealed_lineage` is `None` for a `Genesis` baseline — reachable only when every node
/// in this queue was created within the queue itself (there is no sealed history to consult), so the
/// fallback is never exercised in that case; if it somehow were, failing closed is correct.
pub(crate) fn apply_queued_patch_envelopes(
    reader: &impl ObjectReader,
    records: &[crate::wal::WalRecord],
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
    sealed_lineage: Option<(ObjectId, ObjectId)>,
) -> prikk_error::Result<()> {
    let blob_resolver = StoreBackedResolver::new(reader);
    for record in records {
        let patch_id = record.envelope.object_id();
        let operations = read_patch_operations_from_envelope(&record.envelope, patch_id)?;
        let mut iter = operations.iter().peekable();
        while let Some(operation) = iter.next() {
            if matches!(operation.kind, DecodedOperationKind::RenamePath { .. }) {
                // RFC 144 §4j / increment 2: resolve the whole run together, node before path --
                // never one RenamePath at a time (see `effect::apply_rename_run`'s own doc comment).
                let run = effect::collect_rename_run(&mut iter, operation)?;
                effect::apply_rename_run(state, &run)?;
                continue;
            }
            match apply_state_effect(state, text_cache, &operation.kind, &blob_resolver) {
                Ok(()) => {}
                Err(LifecycleReplayError::MissingBlobForLifecycleEffect { blob_id }) => {
                    let DecodedOperationKind::EditText { node_id, .. } = &operation.kind else {
                        return Err(LifecycleReplayError::MissingBlobForLifecycleEffect {
                            blob_id,
                        }
                        .into());
                    };
                    let Some((baseline_block_id, horizon_id)) = sealed_lineage else {
                        return Err(PrikkError::Integrity(format!(
                            "queued patch {patch_id} edits node {node_id:?} whose content blob \
                             {blob_id} is missing, and no sealed lineage exists to materialize it \
                             from"
                        )));
                    };
                    let text = super::materialize_edited_text(
                        reader,
                        baseline_block_id,
                        horizon_id,
                        *node_id,
                    )?
                    .ok_or_else(|| {
                        PrikkError::Integrity(format!(
                            "queued patch {patch_id} edits node {node_id:?} whose content blob \
                             {blob_id} is missing and could not be materialized from sealed history"
                        ))
                    })?;
                    text_cache.insert(*node_id, text);
                    apply_state_effect(state, text_cache, &operation.kind, &blob_resolver)?;
                }
                Err(other) => return Err(other.into()),
            }
        }
    }
    Ok(())
}

/// Read and decode a patch's operations directly from an in-memory envelope, mapping every failure to
/// `MalformedPatchInLineage`. Unlike [`read_patch_operations`], no object-store read is involved —
/// used for queued WAL records, which are not yet durable objects.
fn read_patch_operations_from_envelope(
    envelope: &prikk_object::ObjectEnvelope,
    patch_id: ObjectId,
) -> Result<Vec<crate::patch_replay::decode::DecodedPatchOperation>, LifecycleReplayError> {
    if envelope.object_type != ObjectType::Patch {
        return Err(LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: format!("object is not a Patch ({} found)", envelope.object_type),
        });
    }
    decode_patch_operations(&envelope.canonical_payload, envelope.schema_version).map_err(|e| {
        LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: e.to_string(),
        }
    })
}

fn replay_chain_with_appended_patches(
    reader: &impl ObjectReader,
    chain: &[WalkedBlock],
    appended_patch_ids: &[ObjectId],
    require_schema_one: bool,
) -> Result<(NodeLifecycleState, TextCache), LifecycleReplayError> {
    let blob_resolver = if require_schema_one {
        StoreBackedResolver::new_format2(reader)
    } else {
        StoreBackedResolver::new(reader)
    };
    let mut state = NodeLifecycleState::new();
    let mut text_cache = TextCache::new();
    for (_block_id, block) in chain {
        apply_patch_ids(
            reader,
            &block.patch_ids,
            &blob_resolver,
            &mut state,
            &mut text_cache,
            require_schema_one,
        )?;
    }
    apply_patch_ids(
        reader,
        appended_patch_ids,
        &blob_resolver,
        &mut state,
        &mut text_cache,
        require_schema_one,
    )?;
    Ok((state, text_cache))
}

fn apply_patch_ids<R: BlobKindResolver + BlobContentResolver>(
    reader: &impl ObjectReader,
    patch_ids: &[ObjectId],
    blob_resolver: &R,
    state: &mut NodeLifecycleState,
    text_cache: &mut TextCache,
    require_schema_one: bool,
) -> Result<(), LifecycleReplayError> {
    for patch_id in patch_ids {
        let operations = read_patch_operations(reader, *patch_id, require_schema_one)?;
        let mut iter = operations.iter().peekable();
        while let Some(operation) = iter.next() {
            if matches!(operation.kind, DecodedOperationKind::RenamePath { .. }) {
                // RFC 144 §4j / increment 2: resolve the whole run together, node before path --
                // never one RenamePath at a time (see `effect::apply_rename_run`'s own doc comment).
                let run = effect::collect_rename_run(&mut iter, operation)?;
                effect::apply_rename_run(state, &run)?;
                continue;
            }
            apply_state_effect(state, text_cache, &operation.kind, blob_resolver)?;
        }
    }
    Ok(())
}

/// Read and decode a patch's operations, mapping every failure to `MalformedPatchInLineage`.
fn read_patch_operations(
    reader: &impl ObjectReader,
    patch_id: ObjectId,
    require_schema_one: bool,
) -> Result<Vec<crate::patch_replay::decode::DecodedPatchOperation>, LifecycleReplayError> {
    let envelope = reader.read_object(patch_id).map_err(|e| {
        LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: e.to_string(),
        }
    })?;
    let Some(envelope) = envelope else {
        return Err(LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: "patch object is missing".to_string(),
        });
    };
    if envelope.object_type != ObjectType::Patch {
        return Err(LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: format!("object is not a Patch ({} found)", envelope.object_type),
        });
    }
    if require_schema_one {
        // Patch schema 2 handoff v2 amendment §2: read the authoritative admitted-schema table
        // `format.rs::validate_format2_schema` owns, rather than a second, hand-maintained `!= 1`
        // check -- state derivation does not itself depend on Patch envelope schema (only the
        // decoded operations reach it, and a schema-2 Patch decodes to the identical operations a
        // schema-1 one would), so this exists only to keep an out-of-band envelope from reaching a
        // decoder that assumes format-2 admission already held.
        let accepted = crate::format::admitted_schemas(envelope.object_type).unwrap_or(&[]);
        if !accepted.contains(&envelope.schema_version) {
            return Err(LifecycleReplayError::MalformedPatchInLineage {
                patch_id,
                detail: format!(
                    "format-2 Patch does not accept envelope schema {} (accepted: {accepted:?})",
                    envelope.schema_version
                ),
            });
        }
    }
    decode_patch_operations(&envelope.canonical_payload, envelope.schema_version).map_err(|e| {
        LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: e.to_string(),
        }
    })
}

mod effect;
use effect::apply_state_effect;

#[cfg(test)]
mod tests;
