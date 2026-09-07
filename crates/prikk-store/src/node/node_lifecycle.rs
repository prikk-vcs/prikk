//! Compatibility imports for the workspace-internal replay lifecycle substrate.

pub(crate) use prikk_replay::{LiveNode, NodeContent, NodeLifecycleState, Tombstone};

#[cfg(test)]
pub(crate) use prikk_replay::{ensure_node_id_nonzero, validate_kind_content_shape};
