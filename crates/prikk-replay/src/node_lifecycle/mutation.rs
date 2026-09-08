use prikk_error::{PrikkError, Result};
use prikk_object::{NodeId, NodeKind, ObjectId};

use crate::path::RepoPath;

use super::{
    LiveNode, NodeContent, NodeLifecycleState, Tombstone, ensure_node_id_nonzero,
    validate_kind_content_shape, validation::ensure_restoration_equivalent,
};

impl NodeLifecycleState {
    /// Introduce a node (`CreateFile` / `CreateSymlink`).
    ///
    /// Rejects reuse of a currently-live `node_id`, rejects occupying an
    /// already-live path, and — for a non-live but previously-seen `node_id` —
    /// requires restoration-equivalence to that node's latest tombstone.
    pub fn create_node(&mut self, node_id: NodeId, node: LiveNode) -> Result<()> {
        // Fail closed at the central node-introduction boundary, matching seed_live_node /
        // seed_tombstone, rather than relying on decode/generator correctness.
        ensure_node_id_nonzero(node_id)?;
        // Erratum P1: fail closed at the central boundary on an inconsistent
        // kind/content discriminator, so no store path can seed a symlink-as-file
        // (or file-as-symlink) node.
        validate_kind_content_shape(node.kind, &node.content)?;
        if self.live_by_id.contains_key(&node_id) {
            return Err(PrikkError::Integrity(
                "cannot create a node whose node_id is already live (per-CleanTree uniqueness)"
                    .to_string(),
            ));
        }
        if self.path_to_id.contains_key(&node.path) {
            return Err(PrikkError::Integrity(format!(
                "cannot create node at already-occupied path {}",
                node.path.as_str()
            )));
        }
        if self.seen_ids.contains(&node_id) {
            // Non-live reintroduction: non-liveness alone is not sufficient.
            let tombstone = self.latest_tombstone_by_id.get(&node_id).ok_or_else(|| {
                PrikkError::Integrity(
                    "seen node_id has no recorded tombstone for restoration-equivalence"
                        .to_string(),
                )
            })?;
            ensure_restoration_equivalent(tombstone, &node)?;
        }
        // A restoration-equivalent reintroduction makes the node live again; its tombstone is
        // no longer the node's latest state, so clear it to keep live and tombstone disjoint
        // (no node_id may be both live and tombstoned). No-op for a fresh node_id.
        self.latest_tombstone_by_id.remove(&node_id);
        self.seen_ids.insert(node_id);
        self.path_to_id.insert(node.path.clone(), node_id);
        self.live_by_id.insert(node_id, node);
        Ok(())
    }

    /// Delete a live node (`DeleteNode`), recording its deletion preimage as the
    /// node's latest tombstone.
    pub fn delete_node(&mut self, node_id: NodeId) -> Result<LiveNode> {
        let node = self.live_by_id.remove(&node_id).ok_or_else(|| {
            PrikkError::Integrity("DeleteNode target node_id is not live".to_string())
        })?;
        // Erratum P2: live_by_id and path_to_id must stay in lockstep; refuse to
        // silently desynchronise if the path index does not point back at this node.
        match self.path_to_id.get(&node.path) {
            Some(id) if *id == node_id => {
                self.path_to_id.remove(&node.path);
            }
            Some(_) => {
                return Err(PrikkError::Integrity(
                    "path index points at a different node than the live node being deleted"
                        .to_string(),
                ));
            }
            None => {
                return Err(PrikkError::Integrity(
                    "live node has no path-index entry (internal inconsistency)".to_string(),
                ));
            }
        }
        self.latest_tombstone_by_id.insert(
            node_id,
            Tombstone {
                kind: node.kind,
                content: node.content.clone(),
                path: node.path.clone(),
            },
        );
        Ok(node)
    }

    /// Rename a live node, preserving its `node_id` (FDD-01 §7.2 / FDD-02 §12).
    pub fn rename_node(&mut self, node_id: NodeId, new_path: RepoPath) -> Result<()> {
        if let Some(existing) = self.path_to_id.get(&new_path) {
            if *existing != node_id {
                return Err(PrikkError::Integrity(format!(
                    "rename target path {} is occupied by another live node",
                    new_path.as_str()
                )));
            }
        }
        let node = self.live_by_id.get_mut(&node_id).ok_or_else(|| {
            PrikkError::Integrity("RenamePath target node_id is not live".to_string())
        })?;
        let old_path = node.path.clone();
        // Erratum P2-1: refuse to mutate if the path index does not point back at this
        // node, so a corrupted internal state is surfaced rather than silently healed.
        match self.path_to_id.get(&old_path) {
            Some(id) if *id == node_id => {}
            _ => {
                return Err(PrikkError::Integrity(
                    "path index does not point at the live node being renamed".to_string(),
                ));
            }
        }
        node.path = new_path.clone();
        self.path_to_id.remove(&old_path);
        self.path_to_id.insert(new_path, node_id);
        Ok(())
    }

    /// Delete a node, first verifying the persisted `DeleteNode` record's old-state assertion
    /// (path, kind, content/preimage) against the replayed live node (2c-2bR, P1-1). Exact replay
    /// must reject a record whose preimage disagrees with reality rather than tombstone from live
    /// state regardless. `expected` is the live node the record claims to delete.
    pub fn delete_node_checked(
        &mut self,
        node_id: NodeId,
        expected: &LiveNode,
    ) -> Result<LiveNode> {
        let live = self.live_by_id.get(&node_id).ok_or_else(|| {
            PrikkError::Integrity("DeleteNode target node_id is not live".to_string())
        })?;
        if live != expected {
            return Err(PrikkError::Integrity(
                "DeleteNode preimage (path/kind/content) does not match the replayed live node"
                    .to_string(),
            ));
        }
        self.delete_node(node_id)
    }

    /// Rename a node, first verifying the persisted `RenamePath` record's `old_path` against the
    /// replayed live node's current path (2c-2bR, P1-2), then renaming to `new_path`.
    pub fn rename_node_checked(
        &mut self,
        node_id: NodeId,
        expected_old_path: &RepoPath,
        new_path: RepoPath,
    ) -> Result<()> {
        let live = self.live_by_id.get(&node_id).ok_or_else(|| {
            PrikkError::Integrity("RenamePath target node_id is not live".to_string())
        })?;
        if live.path != *expected_old_path {
            return Err(PrikkError::Integrity(format!(
                "RenamePath old_path {} does not match the live path {}",
                expected_old_path.as_str(),
                live.path.as_str()
            )));
        }
        self.rename_node(node_id, new_path)
    }

    /// Rename a batch of live nodes together, resolving node before path (RFC 144 §4j / increment
    /// 2): a rename cycle (a swap) applied one operation at a time collides, since the second
    /// half's target is still occupied by the first half's own not-yet-moved node. Resolved by
    /// treating the whole batch as one unit: every `(node_id, old_path)` pair is validated against
    /// the state as it stood *before* this batch. This is what rejects a *chained* rename (`A→B`
    /// then `B→C` for the same node, per §4j.2's ruling that the intermediate path existed in no
    /// sealed state): the second pair's asserted `old_path` (`"b"`) is checked against node A's
    /// *pre-batch* live path (still `"a"`, since nothing has been applied yet), so it simply does
    /// not match. A destination or source node named twice with an old_path that *does* match the
    /// pre-batch state (e.g. `A: a→b` and `A: a→c` in the same batch) is a narrower malformed shape
    /// the old_path check alone would not catch, and is rejected separately, by name, before either
    /// check below ever runs. A destination currently occupied by a node *outside* this batch is a
    /// genuine, structural
    /// collision, reported (the exact same message `rename_node` already produces for a single
    /// rename) rather than silently overwritten; a destination occupied by a node *inside* the
    /// batch is not a collision at all -- that node's own path is being vacated by this same batch
    /// (§4k.3: checked against the batch's own source-node set, never by mutating `path_to_id` and
    /// re-reading it, so the check itself never observes an intermediate state).
    ///
    /// **Fail-atomic** (§4k.3): every check runs to completion before any mutation begins, so an
    /// `Err` leaves `self` byte-for-byte unchanged. This is `pub` on a published `prikk-replay`
    /// type -- unlike `patch_replay::apply::apply_rename_batch` (RFC 144 increment 1), whose
    /// `pub(super)` visibility and replay-local, discard-on-error state make its own non-atomicity
    /// safe by construction -- so a caller reaching this method has no way to know its receiver was
    /// left half-mutated on failure unless the method itself guarantees otherwise.
    ///
    /// Mirrors `prikk-store`'s `patch_replay::apply::apply_rename_batch` (RFC 144 increment 1) at
    /// the node-primary state this crate owns: no bytes move here (this state carries no content,
    /// only identity/path/metadata), so resolution is clearing and re-inserting `path_to_id`
    /// entries rather than moving byte buffers between map slots.
    pub fn rename_nodes_checked_batch(
        &mut self,
        renames: &[(NodeId, RepoPath, RepoPath)],
    ) -> Result<()> {
        if renames.is_empty() {
            return Ok(());
        }

        // Phase 0: validate every pair's own assertion against the pre-batch state, and reject a
        // batch that renames the same node twice or names the same destination twice.
        let mut sources_seen = std::collections::BTreeSet::new();
        let mut destinations_seen = std::collections::BTreeSet::new();
        for (node_id, old_path, new_path) in renames {
            let live = self.live_by_id.get(node_id).ok_or_else(|| {
                PrikkError::Integrity("RenamePath target node_id is not live".to_string())
            })?;
            if live.path != *old_path {
                return Err(PrikkError::Integrity(format!(
                    "RenamePath old_path {} does not match the live path {}",
                    old_path.as_str(),
                    live.path.as_str()
                )));
            }
            if !sources_seen.insert(*node_id) {
                return Err(PrikkError::Integrity(
                    "RenamePath renames the same node more than once in the same batch".to_string(),
                ));
            }
            if !destinations_seen.insert(new_path.clone()) {
                return Err(PrikkError::Integrity(
                    "RenamePath names the same destination path more than once in the same batch"
                        .to_string(),
                ));
            }
        }

        // Phase 1: check every destination for a genuine collision -- occupied by a live node this
        // batch does not itself rename. Checked against `sources_seen` (node identity), never by
        // mutating `path_to_id` first: a node inside the batch that currently occupies a
        // destination is, by Phase 0's own validation, about to vacate that exact path as part of
        // this same batch, so it is not a collision. No mutation has happened yet, so an error here
        // leaves `self` completely unchanged.
        for (_node_id, _old_path, new_path) in renames {
            if let Some(occupant) = self.path_to_id.get(new_path) {
                if !sources_seen.contains(occupant) {
                    return Err(PrikkError::Integrity(format!(
                        "rename target path {} is occupied by another live node",
                        new_path.as_str()
                    )));
                }
            }
        }

        // Phase 2: confirm the path index actually points back at each node being renamed
        // (internal consistency, matching `rename_node`'s own erratum P2-1 check) -- still
        // read-only, so this can still fail without leaving any partial state.
        for (node_id, old_path, _new_path) in renames {
            match self.path_to_id.get(old_path) {
                Some(id) if id == node_id => {}
                _ => {
                    return Err(PrikkError::Integrity(
                        "path index does not point at the live node being renamed".to_string(),
                    ));
                }
            }
        }

        // Phase 3: commit. No further failure is possible past this point. Every source is
        // cleared before any destination is claimed, so two nodes trading paths never observe an
        // intermediate collision with each other (about the map's own transient state during this
        // loop, not about failure -- Phase 1 already proved there is none left to find).
        for (_node_id, old_path, _new_path) in renames {
            self.path_to_id.remove(old_path);
        }
        for (node_id, _old_path, new_path) in renames {
            self.path_to_id.insert(new_path.clone(), *node_id);
            if let Some(node) = self.live_by_id.get_mut(node_id) {
                node.path = new_path.clone();
            }
        }
        Ok(())
    }

    /// Apply a `ChangePerm` to a live file node, preserving its `node_id` and path. The mode is
    /// recorded exactly (O1: the lifecycle index must carry post-mutation mode, since a later
    /// deletion's tombstone — and §10.2 `EntryHash` — bind it). Fails closed if the node is not
    /// live, is a symlink (whose mode is normatively zero), or if the stated `old_mode` does not
    /// match the replayed mode.
    pub fn change_file_mode(
        &mut self,
        node_id: NodeId,
        old_mode: u32,
        new_mode: u32,
    ) -> Result<()> {
        let node = self.live_by_id.get_mut(&node_id).ok_or_else(|| {
            PrikkError::Integrity("ChangePerm target node_id is not live".to_string())
        })?;
        match &mut node.content {
            NodeContent::File { mode, .. } => {
                if *mode != old_mode {
                    return Err(PrikkError::Integrity(format!(
                        "ChangePerm old_mode {old_mode:#o} does not match the live mode {:#o}",
                        *mode
                    )));
                }
                *mode = new_mode;
                Ok(())
            }
            NodeContent::Symlink { .. } => Err(PrikkError::Integrity(
                "ChangePerm cannot apply to a symlink node (mode is normatively zero)".to_string(),
            )),
        }
    }

    /// Apply a `ReplaceBinary` to a live binary-file node (2c-2c). Requires the node to be live,
    /// a `BinaryFile`, and to currently reference `old_blob_id`; replaces its blob with
    /// `new_blob_id` exactly, preserving mode. Text files are out of scope (that is `EditText`).
    pub fn replace_file_blob(
        &mut self,
        node_id: NodeId,
        old_blob_id: ObjectId,
        new_blob_id: ObjectId,
    ) -> Result<()> {
        let node = self.live_by_id.get_mut(&node_id).ok_or_else(|| {
            PrikkError::Integrity("ReplaceBinary target node_id is not live".to_string())
        })?;
        if node.kind != NodeKind::BinaryFile {
            return Err(PrikkError::Integrity(
                "ReplaceBinary target is not a binary-file node".to_string(),
            ));
        }
        match &mut node.content {
            NodeContent::File { blob_id, .. } => {
                if *blob_id != old_blob_id {
                    return Err(PrikkError::Integrity(
                        "ReplaceBinary old_blob_id does not match the live blob".to_string(),
                    ));
                }
                *blob_id = new_blob_id;
                Ok(())
            }
            NodeContent::Symlink { .. } => Err(PrikkError::Integrity(
                "ReplaceBinary cannot apply to a symlink node".to_string(),
            )),
        }
    }

    /// Set a live text-file node's content blob id (2c-2d). `EditText` replay derives a new
    /// full-text `BlobPayload(Text, …)` id and records it here, preserving `node_id`, path, and
    /// mode. The new bytes themselves live in the replay-local materialized-text cache, not the
    /// index. Fails closed if the node is absent or not a `TextFile`.
    pub fn set_text_blob(&mut self, node_id: NodeId, new_blob_id: ObjectId) -> Result<()> {
        let node = self.live_by_id.get_mut(&node_id).ok_or_else(|| {
            PrikkError::Integrity("EditText target node_id is not live".to_string())
        })?;
        if node.kind != NodeKind::TextFile {
            return Err(PrikkError::Integrity(
                "EditText target is not a text-file node".to_string(),
            ));
        }
        match &mut node.content {
            NodeContent::File { blob_id, .. } => {
                *blob_id = new_blob_id;
                Ok(())
            }
            NodeContent::Symlink { .. } => Err(PrikkError::Integrity(
                "EditText cannot apply to a symlink node".to_string(),
            )),
        }
    }

    /// Seed a live clean-tree node from a baseline cache (erratum P1, 4.4-2).
    ///
    /// Reuses the same gates as a fresh `create_node` so a cache cannot inject what
    /// an operation could not: rejects the all-zero `node_id`, an inconsistent
    /// kind/content shape, a duplicate live `node_id`, and an occupied path. (The
    /// symlink `normalized_mode == 0` rule is enforced at the cache-parse boundary,
    /// before a `NodeContent::Symlink` — which carries no mode — is constructed.)
    pub fn seed_live_node(&mut self, node_id: NodeId, node: LiveNode) -> Result<()> {
        ensure_node_id_nonzero(node_id)?;
        validate_kind_content_shape(node.kind, &node.content)?;
        if self.live_by_id.contains_key(&node_id) {
            return Err(PrikkError::Integrity(
                "baseline seeds a duplicate live node_id".to_string(),
            ));
        }
        if self.path_to_id.contains_key(&node.path) {
            return Err(PrikkError::Integrity(format!(
                "baseline seeds a duplicate live path {}",
                node.path.as_str()
            )));
        }
        self.seen_ids.insert(node_id);
        self.path_to_id.insert(node.path.clone(), node_id);
        self.live_by_id.insert(node_id, node);
        Ok(())
    }

    /// Seed the latest deletion preimage for a previously-seen, non-live node from a
    /// baseline lifecycle summary (erratum P1, 4.4-2). Records `seen_ids` so the
    /// historical-reintroduction rule applies across the snapshot boundary.
    pub fn seed_tombstone(&mut self, node_id: NodeId, tombstone: Tombstone) -> Result<()> {
        ensure_node_id_nonzero(node_id)?;
        validate_kind_content_shape(tombstone.kind, &tombstone.content)?;
        if self.live_by_id.contains_key(&node_id) {
            return Err(PrikkError::Integrity(
                "baseline seeds a tombstone for a currently-live node_id".to_string(),
            ));
        }
        self.seen_ids.insert(node_id);
        self.latest_tombstone_by_id.insert(node_id, tombstone);
        Ok(())
    }
}
