//! Node lifecycle state for replay-derived semantic state.
//!
//! This index is **derived, rebuildable, and explicitly not a root of trust**
//! (FDD-02 §12): the authoritative artifacts are the persisted patch operations,
//! and this structure is reconstructed by replaying a block/patch sequence under
//! its parent-state context. It centralises the FDD-02 §12 / FDD-01 §7.2 node
//! rules so replay, inverse, rollback, and later semantic analysis paths cannot
//! diverge on them:
//!
//! - per-`CleanTree` live-node uniqueness — no two live nodes share a `node_id`;
//! - reintroduction of a currently-live `node_id` is rejected;
//! - reintroduction of a non-live (previously-seen) `node_id` is accepted only when
//!   it is **restoration-equivalent** to that node's latest deletion preimage
//!   (tombstone): same kind, same content payload, same mode where applicable, and
//!   same path. Non-liveness is necessary but **not** sufficient (DC-09a §4);
//! - `node_id` is preserved across `RenamePath`.
//!
//! This lifecycle substrate lives in `prikk-replay` as workspace-internal semantic
//! code. Repository lineage traversal, refs, WAL, object storage, cache persistence,
//! verification, doctor, and store-backed resolver construction remain in
//! `prikk-store`.

mod mutation;
mod query;
mod types;
mod validation;

pub use types::{LiveNode, NodeContent, NodeLifecycleState, PathChange, Tombstone};
pub use validation::{ensure_node_id_nonzero, validate_kind_content_shape};

#[cfg(test)]
mod tests;
