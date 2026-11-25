use prikk_object::NodeId;

use crate::path::RepoPath;

use super::{LiveNode, NodeLifecycleState, PathChange, Tombstone};

impl NodeLifecycleState {
    /// The live node for an identity, if any.
    pub fn live_node(&self, node_id: &NodeId) -> Option<&LiveNode> {
        self.live_by_id.get(node_id)
    }

    /// True if `node_id` is in the complete known-id set (live, tombstoned, or otherwise seen in
    /// this baseline). Fresh node-id minting rejects any candidate for which this holds, so a
    /// random draw can never alias an existing node identity (4.4a-1, erratum E2).
    pub fn contains_seen_node_id(&self, node_id: &NodeId) -> bool {
        self.seen_ids.contains(node_id)
    }

    /// Iterate the live clean-tree nodes as `(node_id, node)`. Used by worktree authoring to build
    /// the baseline `path → (node_id, kind, content)` view (4.4a-2).
    pub fn live_nodes(&self) -> impl Iterator<Item = (&NodeId, &LiveNode)> {
        self.live_by_id.iter()
    }

    /// The `node_id` currently live at a path, if any. (Production consumer is 4.4a-2 worktree
    /// authoring, which resolves an existing path to its node id here.)
    pub fn node_id_at(&self, path: &RepoPath) -> Option<NodeId> {
        self.path_to_id.get(path).copied()
    }

    /// The latest deletion preimage retained for an identity, if the node is tombstoned.
    pub fn latest_tombstone(&self, node_id: &NodeId) -> Option<&Tombstone> {
        self.latest_tombstone_by_id.get(node_id)
    }

    /// Iterate every tombstoned node as `(node_id, tombstone)`. Used to persist a lifecycle
    /// state's complete deletion history (DC-64's incremental baseline cache), which — per
    /// `create_node`/`delete_node`'s invariants — is exactly `seen_ids` minus the live set.
    pub fn tombstones(&self) -> impl Iterator<Item = (&NodeId, &Tombstone)> {
        self.latest_tombstone_by_id.iter()
    }

    /// Every live path, in path order.
    pub fn live_paths(&self) -> impl Iterator<Item = &RepoPath> {
        self.path_to_id.keys()
    }

    /// Start journaling the changes the mutators make to the set of live paths (an empty journal; a journal already running is emptied).
    /// A consumer that must judge the state's paths after every step reads [`Self::take_path_changes`] instead of listing them again.
    /// The journal is not part of the state's identity (`==` ignores it) and a clone carries it.
    pub fn record_path_changes(&mut self) {
        self.path_journal.0 = Some(Vec::new());
    }

    /// The path changes journaled since the last call, in the order they were made (the journal keeps running); empty when the state is
    /// not journaling.
    pub fn take_path_changes(&mut self) -> Vec<PathChange> {
        self.path_journal
            .0
            .as_mut()
            .map(std::mem::take)
            .unwrap_or_default()
    }

    /// Number of live nodes in the reconstructed clean tree.
    pub fn live_count(&self) -> usize {
        self.live_by_id.len()
    }
}
