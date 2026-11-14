//! The batch fold of `sync seal` (0.58.0 S2b): one running derivation through a batch, and the Block sealed with the root it produced.
//! Split out of `block_state.rs` (which it would have pushed past the 1,200-line limit); everything here is sealing-side, built on that
//! module's `derive_candidate`, `checkpoint_due` and `write_signed_block`.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockKind, BlockPayload, MerkleRoot, ObjectId, ObjectType};

use super::{
    CandidateStateDerivationError, StateAnchoring, checkpoint_due, derive_candidate,
    state_derivation_parent, write_signed_block,
};
use crate::lifecycle_cache::replay::{TextCache, apply_candidate_patches};
use crate::maintainer_signing::MaintainerSigner;
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::{ObjectReader, ObjectWriter};
use crate::snapshot::{CHECKPOINT_CADENCE, write_checkpoint_snapshot};
use crate::state_root::{
    StateRootEntry, compute_state_root, entries_from_state, refuse_colliding_entries,
};

/// How many blocks back from a new Block's parent the nearest snapshotted ancestor is: `Some(1)` when `parent` carries a snapshot,
/// `Some(2)` when its derivation parent does, and so on; `None` when none is within `CHECKPOINT_CADENCE - 1` ancestors or the line ends
/// first. [`checkpoint_due`] is exactly "this is `None`". The batch plan (0.58.0 S2b) reads it once for the batch's starting tip and
/// then tracks it block by block, so it knows which claims' Blocks are checkpoints.
pub(crate) fn nearest_snapshot_within_cadence(
    reader: &impl ObjectReader,
    parent: Option<ObjectId>,
) -> Result<Option<u32>> {
    let mut current = parent;
    let mut distance = 1_u32;
    while let Some(block_id) = current {
        if distance >= CHECKPOINT_CADENCE {
            return Ok(None);
        }
        let envelope = reader
            .read_typed(block_id, ObjectType::Block)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "checkpoint cadence: ancestor Block {block_id} is missing"
                ))
            })?;
        let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
        if payload.snapshot_blob_ref.is_some() {
            return Ok(Some(distance));
        }
        current = state_derivation_parent(&payload);
        distance += 1;
    }
    Ok(None)
}

/// One running derivation through a `sync seal` batch (0.58.0 S2b, RFC 159's mechanism with the batch's own state as an in-memory
/// anchor). The pre-batch state is derived **once**, anchored as RFC 159 already anchors `AnchorSite::SealFromAccepted`
/// ([`BatchFold::begin`]); each claim's patches are then applied to that one state in claim order ([`BatchFold::fold`]), and the root of
/// the state after each claim is the root that claim's Block will carry. **The rules of RFC 159 apply to it unchanged:** it is derived in
/// this process from verified inputs (the anchor-trust function admitted the starting snapshot, and every patch is a stored, signed
/// object); each claim's Block is signed over the root this fold produced and recorded replay-verified exactly as before; and a state is
/// never reused past a doubt -- the plan that holds the fold ends at the first claim whose fold fails or whose selection differs from the
/// real loop's, and every later claim derives from disk as it always did.
///
/// A `BatchFold` whose `fold` returned an error is partly applied and must be dropped; the caller stops folding there.
/// What one claim's fold produced.
pub(crate) struct FoldedStep {
    /// The root of the state after the claim's patches: the root its Block carries.
    pub(crate) root: MerkleRoot,
    /// The entries and text cache of that state, kept only when the claim's Block is a checkpoint (it carries a snapshot of its own
    /// state, written before the Block).
    pub(crate) snapshot_inputs: Option<(Vec<StateRootEntry>, TextCache)>,
}

pub(crate) struct BatchFold {
    state: NodeLifecycleState,
    text_cache: TextCache,
    /// Entries of the state after the last claim folded, kept for the one use past the roots: the in-the-way check's target manifest.
    last_entries: Vec<StateRootEntry>,
    /// Blocks whose roots the starting derivation confirmed by replay (the first sealed claim records them, as before).
    verified: Vec<ObjectId>,
}

impl BatchFold {
    /// Derive the state `parent` leaves (anchored as `anchoring` says), applying no patch.
    pub(crate) fn begin(
        layout: &crate::RepositoryLayout,
        reader: &impl ObjectReader,
        parent: Option<ObjectId>,
        anchoring: StateAnchoring,
    ) -> std::result::Result<Self, CandidateStateDerivationError> {
        let derived = derive_candidate(layout, reader, parent, &[], anchoring)?;
        Ok(Self {
            state: derived.state,
            text_cache: derived.text_cache,
            last_entries: derived.entries,
            verified: derived.verified,
        })
    }

    /// Apply `patch_ids` to the running state and return the state root of the result: what the Block sealed from these patches carries.
    pub(crate) fn fold(
        &mut self,
        reader: &impl ObjectReader,
        patch_ids: &[ObjectId],
        retain_for_snapshot: bool,
    ) -> std::result::Result<FoldedStep, CandidateStateDerivationError> {
        apply_candidate_patches(reader, &mut self.state, &mut self.text_cache, patch_ids)
            .map_err(CandidateStateDerivationError::Patch)?;
        // The same collision policy a sealed state asks (`derive_candidate`): a state being authored refuses a collision.
        let entries = entries_from_state(&self.state)
            .and_then(|entries| refuse_colliding_entries(&entries).map(|()| entries))
            .map_err(CandidateStateDerivationError::Lineage)?;
        let root = compute_state_root(&entries).map_err(CandidateStateDerivationError::Lineage)?;
        let snapshot_inputs =
            retain_for_snapshot.then(|| (entries.clone(), self.text_cache.clone()));
        self.last_entries = entries;
        Ok(FoldedStep {
            root,
            snapshot_inputs,
        })
    }

    /// The blocks the starting derivation verified.
    pub(crate) fn verified(&self) -> &[ObjectId] {
        &self.verified
    }

    /// The state after the last claim folded: the entries and the text cache (the target manifest of the in-the-way check).
    pub(crate) fn into_last_state(self) -> (Vec<StateRootEntry>, TextCache) {
        (self.last_entries, self.text_cache)
    }
}

/// Seal a `Normal` or `Root` Block on `parent` whose root the batch's running fold already produced. When a checkpoint is due the Block
/// carries a snapshot of its own state, written first from `step.snapshot_inputs`; if the fold did not keep them (it expected no
/// checkpoint here) this returns `None` having written nothing, and the caller seals that one claim from disk
/// ([`seal_block_classified`]) as it always did. Everything else is the same Block: same payload, same signature, same record. `step` is
/// what [`BatchFold::fold`] returned for exactly these `patch_ids` on top of `parent`'s state.
pub(crate) fn seal_block_with_folded_root(
    layout: &crate::RepositoryLayout,
    object_store: &mut (impl ObjectReader + ObjectWriter),
    parent: Option<ObjectId>,
    patch_ids: &[ObjectId],
    step: FoldedStep,
    signer: &impl MaintainerSigner,
    replay_verified: Vec<ObjectId>,
) -> Result<Option<ObjectId>> {
    let snapshot_blob_ref = if checkpoint_due(&*object_store, parent)? {
        let Some((entries, text_cache)) = step.snapshot_inputs else {
            return Ok(None);
        };
        Some(write_checkpoint_snapshot(
            object_store,
            entries,
            |node_id| text_cache.get(node_id).cloned(),
        )?)
    } else {
        None
    };
    let (parent_block_ids, kind) = match parent {
        None => (Vec::new(), BlockKind::Root),
        Some(parent) => (vec![parent], BlockKind::Normal),
    };
    write_signed_block(
        layout,
        object_store,
        BlockPayload {
            parent_block_ids,
            kind,
            patch_ids: patch_ids.to_vec(),
            state_merkle_root: step.root,
            snapshot_blob_ref,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
        signer,
        replay_verified,
    )
    .map(Some)
}
