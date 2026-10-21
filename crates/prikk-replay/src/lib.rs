#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Workspace-internal replay and lifecycle semantics for Prikk.
//!
//! This crate is not a public API commitment. It owns semantic replay/lifecycle substrate types
//! while repository layout, refs, WAL, object storage, verification, doctor, and store-backed
//! resolver construction remain in `prikk-store`.

mod node_lifecycle;
mod path;

pub use node_lifecycle::{
    LiveNode, NodeContent, NodeLifecycleState, Tombstone, ensure_node_id_nonzero,
    validate_kind_content_shape,
};
pub use path::{RepoPath, validate_no_path_collisions, validate_repo_path};
