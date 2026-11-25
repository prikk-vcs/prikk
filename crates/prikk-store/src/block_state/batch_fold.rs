//! The batch fold of `sync seal` (0.58.0 S2b): one running derivation through a batch, and the Block sealed with the root it produced.
//! Split out of `block_state.rs` (which it would have pushed past the 1,200-line limit); everything here is sealing-side, built on that
//! module's `derive_candidate`, `checkpoint_due` and `write_signed_block`.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockKind, BlockPayload, MerkleRoot, ObjectId, ObjectType};

use super::collision_index::CollisionIndex;
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

/// What one claim's fold produced.
pub(crate) struct FoldedStep {
    /// The root of the state after the claim's patches: the root its Block carries.
    pub(crate) root: MerkleRoot,
    /// The entries and text cache of that state, kept only when the claim's Block is a checkpoint (it carries a snapshot of its own
    /// state, written before the Block).
    pub(crate) snapshot_inputs: Option<RetainedState>,
}

/// A checkpoint's state, held from the claim's fold until its snapshot is written. **At most one is alive at a time** (0.59.0 P5a1): the
/// running fold produces it at the claim whose Block is a checkpoint, the seal consumes it before the next claim is folded, and the plan
/// keeps none. (0.58.0 kept every checkpoint's state from the up-front fold until its claim was sealed, which in a tree that gains a path
/// per claim is about 0.81 x claims^2 bytes.)
pub(crate) struct RetainedState {
    entries: Vec<StateRootEntry>,
    text_cache: TextCache,
}

#[cfg(test)]
std::thread_local! {
    static RETAINED_LIVE: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static RETAINED_PEAK: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// A control (`cfg(test)`): the last entry of every checkpoint snapshot is left out, so a test can see that its comparison can fail.
    static DROP_SNAPSHOT_ENTRY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// The most checkpoint states alive at once on this thread since the last call (`cfg(test)`): the pin that a batch holds one at a time.
#[cfg(test)]
pub(crate) fn retained_states_peak_for_test() -> usize {
    RETAINED_PEAK.with(|peak| peak.replace(RETAINED_LIVE.with(std::cell::Cell::get)))
}

/// Run `body` with the snapshot-entry control on (`cfg(test)`).
#[cfg(test)]
pub(crate) fn with_snapshot_entry_dropped_for_test<T>(body: impl FnOnce() -> T) -> T {
    DROP_SNAPSHOT_ENTRY.with(|flag| flag.set(true));
    let result = body();
    DROP_SNAPSHOT_ENTRY.with(|flag| flag.set(false));
    result
}

impl RetainedState {
    fn new(entries: Vec<StateRootEntry>, text_cache: TextCache) -> Self {
        #[cfg(test)]
        RETAINED_LIVE.with(|live| {
            live.set(live.get() + 1);
            RETAINED_PEAK.with(|peak| peak.set(peak.get().max(live.get())));
        });
        Self {
            entries,
            text_cache,
        }
    }

    /// The entries and the text cache, leaving this empty.
    pub(crate) fn into_parts(mut self) -> (Vec<StateRootEntry>, TextCache) {
        (
            std::mem::take(&mut self.entries),
            std::mem::take(&mut self.text_cache),
        )
    }
}

#[cfg(test)]
impl Drop for RetainedState {
    fn drop(&mut self) {
        RETAINED_LIVE.with(|live| live.set(live.get().saturating_sub(1)));
    }
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
/// **0.59.0 P5a1: the fold advances with the loop.** The plan holds the fold and the loop advances it one claim at a time, so the
/// batch holds one state (and, at a checkpoint, that one claim's snapshot inputs), not one per checkpoint. The same fold also exists
/// in a second form, [`BatchFold::apply`]-only, run once up front on the current branch to reach the state the in-the-way check judges.
///
/// A `BatchFold` whose `fold` or `apply` returned an error is partly applied and must be dropped; the caller stops there.
pub(crate) struct BatchFold {
    state: NodeLifecycleState,
    text_cache: TextCache,
    /// Kept only by [`BatchFold::track_collisions`]: what the state's paths collide as, claim by claim.
    collisions: Option<CollisionIndex>,
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
            collisions: None,
            verified: derived.verified,
        })
    }

    /// A second fold at the same state, for the loop to advance while this one runs ahead: the state and text cache are copied, the
    /// verified ids stay with the original.
    pub(crate) fn fork(&self) -> Self {
        Self {
            state: self.state.clone(),
            text_cache: self.text_cache.clone(),
            collisions: None,
            verified: Vec::new(),
        }
    }

    /// Apply `patch_ids` to the running state and nothing else: no entries, no collision check, no root.
    pub(crate) fn apply(
        &mut self,
        reader: &impl ObjectReader,
        patch_ids: &[ObjectId],
    ) -> std::result::Result<(), CandidateStateDerivationError> {
        apply_candidate_patches(reader, &mut self.state, &mut self.text_cache, patch_ids)
            .map_err(CandidateStateDerivationError::Patch)
    }

    /// Start keeping the collision policy's answer incrementally ([`BatchFold::apply_checked`]): the index of the state's paths as they
    /// are now, and a journal of the paths each later claim adds or vacates.
    pub(crate) fn track_collisions(&mut self) {
        self.collisions = Some(CollisionIndex::of_state(&self.state));
        self.state.record_path_changes();
    }

    /// [`BatchFold::apply`], and whether the state it leaves collides (`refuse_colliding_entries` would refuse it): the answer 0.58.0's fold
    /// got by listing every entry after every claim, here from the claim's own changed paths. Needs [`BatchFold::track_collisions`].
    pub(crate) fn apply_checked(
        &mut self,
        reader: &impl ObjectReader,
        patch_ids: &[ObjectId],
    ) -> std::result::Result<bool, CandidateStateDerivationError> {
        self.apply(reader, patch_ids)?;
        let changes = self.state.take_path_changes();
        Ok(match self.collisions.as_mut() {
            Some(index) => {
                index.apply(&changes);
                index.collides()
            }
            // Not tracking: ask the state itself (listing every path), as a fold without the index must.
            None => CollisionIndex::of_state(&self.state).collides(),
        })
    }

    /// Apply `patch_ids` to the running state and return the state root of the result: what the Block sealed from these patches carries.
    pub(crate) fn fold(
        &mut self,
        reader: &impl ObjectReader,
        patch_ids: &[ObjectId],
        retain_for_snapshot: bool,
    ) -> std::result::Result<FoldedStep, CandidateStateDerivationError> {
        self.apply(reader, patch_ids)?;
        // The same collision policy a sealed state asks (`derive_candidate`): a state being authored refuses a collision.
        let entries = entries_from_state(&self.state)
            .and_then(|entries| refuse_colliding_entries(&entries).map(|()| entries))
            .map_err(CandidateStateDerivationError::Lineage)?;
        let root = compute_state_root(&entries).map_err(CandidateStateDerivationError::Lineage)?;
        let snapshot_inputs = retain_for_snapshot.then(|| {
            #[cfg(test)]
            let entries = if DROP_SNAPSHOT_ENTRY.with(std::cell::Cell::get) {
                let mut entries = entries;
                entries.pop();
                entries
            } else {
                entries
            };
            RetainedState::new(entries, self.text_cache.clone())
        });
        Ok(FoldedStep {
            root,
            snapshot_inputs,
        })
    }

    /// The blocks the starting derivation verified.
    pub(crate) fn verified(&self) -> &[ObjectId] {
        &self.verified
    }

    /// The entries and the text cache of the state as it stands (the target manifest of the in-the-way check), derived once, with the
    /// collision policy of [`BatchFold::fold`].
    pub(crate) fn into_final_state(
        self,
    ) -> std::result::Result<(Vec<StateRootEntry>, TextCache), CandidateStateDerivationError> {
        let entries = entries_from_state(&self.state)
            .and_then(|entries| refuse_colliding_entries(&entries).map(|()| entries))
            .map_err(CandidateStateDerivationError::Lineage)?;
        Ok((entries, self.text_cache))
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
        let Some(retained) = step.snapshot_inputs else {
            return Ok(None);
        };
        let (entries, text_cache) = retained.into_parts();
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
