//! Lifecycle cache codec and importer (DC-09 Phase 4.4-2b.1).
//!
//! A persisted, **non-identity-bearing, rebuildable** accelerator for
//! `NodeLifecycleState` (FDD-02 §12; design v3). This module only decodes and
//! *structurally* validates a cache into a [`DecodedLifecycleCache`]; per design v3 §0
//! that type is **not validation authority** — it cannot seed an accept/reject `node_id`
//! reuse decision. Blob-kind verification (against a resolver), provenance-vs-baseline
//! staleness, and replay reconstruction/compare are later slices; until then there is no
//! type here that an identity decision can consume.
//!
//! Wire format: ascii magic `PRIKK-NODE-LIFECYCLE-CACHE-v1\0` followed by canonical
//! `FieldRecord` TLV (design v3 wire table). Deterministic, versioned, and validated
//! fail-closed: any structural or cross-set violation is rejected.
//!
//! **Split in two (DC-58).** Nearly all of this module's content — the codec, the
//! `DecodedLifecycleCache` → `ValidatedLifecycleCache` → `ComparedLifecycleCache` trust ladder,
//! and their field-level helpers — was already `#[cfg(test)]`-gated item-by-item before the
//! split, per the "later slices" note above: this scaffolding exists for validation, provenance,
//! and compare-against-replay, none of which is wired into production yet. That bulk moved to
//! `lifecycle_cache/cache_ladder.rs`, gated as a whole test-only module instead of per item, and
//! re-exported here (also `#[cfg(test)]`) so every path `lifecycle_cache::tests` already used via
//! `super::X` keeps resolving unchanged. What remains here is the genuinely production-only
//! surface: the resolver traits, the real store-backed resolver, the replay entry points, and
//! `ReplayDerivedLifecycleState`. No behaviour change.

use prikk_error::Result;
use prikk_object::{BlobKind, ObjectId};

use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::ObjectReader;

#[cfg(test)]
mod cache_ladder;

#[cfg(test)]
pub(crate) use cache_ladder::{
    BlockParentResolver, CACHE_SCHEMA_VERSION, CacheCertificationError, ComparedLifecycleCache,
    DecodedLifecycleCache, ParentPolicy, ValidatedLifecycleCache, certified_compared_cache,
    compute_window_hash,
};

/// Resolves a referenced blob's kind for file-entry verification. `Ok(None)` means the
/// blob is absent/unreadable, which makes the cache unusable (fail-closed). A real store
/// resolver is wired in the threading slice; this trait keeps verification testable and
/// keeps the codec module free of a store handle.
pub(crate) trait BlobKindResolver {
    fn blob_kind(&self, blob_id: &ObjectId) -> Result<Option<BlobKind>>;
}

/// Resolves a blob's kind and full content bytes, for `EditText` text materialization (2c-2d).
/// Returns `None` only when the blob is absent (fail-closed sentinel); a present non-Blob object
/// is an error.
pub(crate) trait BlobContentResolver {
    fn blob_content(&self, blob_id: &ObjectId) -> Result<Option<(BlobKind, Vec<u8>)>>;
}

/// Real store-backed implementations of the resolver traits (4.4-2c-1).
mod store_resolvers;

/// Explicit boundary (E1): authoritative store access enters the lifecycle trust ladder here.
pub(crate) use store_resolvers::StoreBackedResolver;

/// Authoritative lifecycle replay: lineage walker + dispatch skeleton (4.4-2c-2a).
pub(crate) mod replay;

/// Incremental baseline lifecycle-state cache, scoped to the commit path (DC-64).
pub(crate) mod incremental;

/// Authoritative replay-derived lifecycle state for a specific baseline (rung 3). It must be
/// produced **only** by authoritative replay over the actual walked single-parent chain; the
/// real producer is the threading slice. This type is the reference truth a cache is compared
/// against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplayDerivedLifecycleState {
    baseline_block_id: ObjectId,
    state: NodeLifecycleState,
}

impl ReplayDerivedLifecycleState {
    /// Wrap an authoritative replay-derived state, rejecting an internally inconsistent state
    /// (e.g. a node both live and tombstoned) so the compared rung cannot certify against a
    /// malformed reference.
    pub(crate) fn from_replay(
        baseline_block_id: ObjectId,
        state: NodeLifecycleState,
    ) -> Result<Self> {
        state.validate_internal_consistency()?;
        Ok(Self {
            baseline_block_id,
            state,
        })
    }

    /// Borrow the authoritative replay-derived lifecycle state (the only sanctioned baseline for
    /// node-addressed worktree authoring, 4.4a-2).
    pub(crate) fn state(&self) -> &NodeLifecycleState {
        &self.state
    }

    pub(crate) fn baseline_block_id(&self) -> ObjectId {
        self.baseline_block_id
    }
}

/// Rung-3 producer: authoritative replay-derived lifecycle state for `baseline_block_id`, with
/// `lineage_horizon_id` the claimed genesis. Runs the authoritative single-parent replay and wraps
/// it through `ReplayDerivedLifecycleState::from_replay`, which validates internal consistency
/// before the state can be used as the reference truth. This is the only sanctioned way to obtain
/// a `ReplayDerivedLifecycleState`.
pub(crate) fn replay_derived_state(
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    lineage_horizon_id: ObjectId,
) -> Result<ReplayDerivedLifecycleState> {
    let state = replay::replay_lineage(reader, baseline_block_id, lineage_horizon_id)?;
    ReplayDerivedLifecycleState::from_replay(baseline_block_id, state)
}

/// Materialize a `TextFile` node's current bytes by full replay (DC-65). A `TextFile` node's
/// `blob_id` after any `EditText` is a content identity, not necessarily a stored `Blob` object —
/// see `rfcs/handoffs/DC-65-text-edit-baseline-content/prerequisite-questions-v1.md` for the
/// invariant this implements. Returns `Ok(None)` if the node was never edited (nothing was
/// materialized for it — callers should already have a real stored blob to fall back to in that
/// case, since only `EditText` produces an unstored identity).
pub(crate) fn materialize_edited_text(
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    lineage_horizon_id: ObjectId,
    node_id: prikk_object::NodeId,
) -> Result<Option<Vec<u8>>> {
    let (_state, text_cache) = replay::replay_lineage_with_materialized_text(
        reader,
        baseline_block_id,
        lineage_horizon_id,
    )?;
    Ok(text_cache.get(&node_id).cloned())
}

#[cfg(test)]
mod tests;
