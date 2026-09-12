//! Compatibility imports for the workspace-internal replay lifecycle substrate.

// RFC 149 §6c: waypoint widened with its item; a `pub(crate)` re-export blocks `lib.rs` (E0364).
pub use prikk_replay::NodeLifecycleState;
pub(crate) use prikk_replay::{LiveNode, NodeContent, Tombstone};

#[cfg(test)]
pub(crate) use prikk_replay::{ensure_node_id_nonzero, validate_kind_content_shape};
