#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Workspace-internal replay and lifecycle semantics for Prikk.
//!
//! This crate is not a public API commitment. It holds two things, storage-free: the model of a
//! node's lifecycle ([`NodeLifecycleState`], [`LiveNode`], [`NodeContent`], [`Tombstone`]), and the
//! repository path rules ([`RepoPath`] and its validators, [`validate_repo_path`] and
//! [`validate_no_path_collisions`]). `prikk-store` depends on this crate: it builds on the lifecycle
//! types throughout (used internally, not re-exported) and re-exports the path types directly
//! (`prikk_store::{RepoPath, validate_repo_path, validate_no_path_collisions}`). Repository layout,
//! refs, WAL, object storage, verification, doctor, and store-backed resolver construction remain in
//! `prikk-store`.

mod node_lifecycle;
mod path;

pub use node_lifecycle::{
    LiveNode, NodeContent, NodeLifecycleState, Tombstone, ensure_node_id_nonzero,
    validate_kind_content_shape,
};
pub use path::{RepoPath, validate_no_path_collisions, validate_repo_path};
