use std::collections::{BTreeMap, BTreeSet};

use prikk_object::{NodeId, NodeKind, ObjectId};

use crate::path::RepoPath;

/// Content-payload identity of a node (FDD-03 §10.2 `node_payload`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeContent {
    /// Text or binary file: blob identity plus mode bits.
    File {
        /// Canonical blob object id carried by this file node.
        blob_id: ObjectId,
        /// File mode bits carried by this file node.
        mode: u32,
    },
    /// Symlink: target string (mode is normatively `u32be(0)`, FDD-03 §10.2).
    Symlink {
        /// Link target carried by this symlink node.
        target: String,
    },
}

/// A node currently live in the reconstructed clean tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveNode {
    /// Repository-relative path occupied by the live node.
    pub path: RepoPath,
    /// Canonical node kind.
    pub kind: NodeKind,
    /// Content identity carried by the node.
    pub content: NodeContent,
}

/// The latest deletion preimage retained per node for restoration-equivalence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tombstone {
    /// Canonical node kind at deletion time.
    pub kind: NodeKind,
    /// Content identity at deletion time.
    pub content: NodeContent,
    /// Repository-relative path occupied at deletion time.
    pub path: RepoPath,
}

/// One change to the set of live paths, as the state's mutators make it (see [`NodeLifecycleState::record_path_changes`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathChange {
    /// The path that became occupied (`added`) or vacated.
    pub path: RepoPath,
    /// `true` when a live node now occupies `path`, `false` when one stopped.
    pub added: bool,
}

/// The journal of path changes a state keeps when asked to. **Never part of a state's identity:** two states are equal whatever each one
/// has journaled, and a state that does not journal holds nothing.
#[derive(Debug, Default, Clone)]
pub(super) struct PathJournal(pub(super) Option<Vec<PathChange>>);

impl PartialEq for PathJournal {
    fn eq(&self, _other: &Self) -> bool {
        true
    }
}

impl Eq for PathJournal {}

/// Replay-derived node lifecycle index. Not a root of trust (FDD-02 §12).
///
/// Live-node uniqueness is structural: `live_by_id` is keyed by `node_id`, and
/// every introduction path rejects a currently-live `node_id`, so no reconstructed
/// clean tree can hold two live nodes sharing an identity.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct NodeLifecycleState {
    pub(super) live_by_id: BTreeMap<NodeId, LiveNode>,
    pub(super) path_to_id: BTreeMap<RepoPath, NodeId>,
    pub(super) latest_tombstone_by_id: BTreeMap<NodeId, Tombstone>,
    pub(super) seen_ids: BTreeSet<NodeId>,
    pub(super) path_journal: PathJournal,
}

impl NodeLifecycleState {
    /// An empty lifecycle state (genesis parent-state context).
    pub fn new() -> Self {
        Self::default()
    }
}
