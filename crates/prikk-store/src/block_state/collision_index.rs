//! The path-collision policy, kept incrementally (0.59.0 P5a1b).
//!
//! `refuse_colliding_entries` asks one thing of a state: do two of its live paths share a canonical caseless key (`state_root.rs`,
//! `prikk_replay::validate_no_path_collisions`)? That is a function of the **multiset of keys**, so it can be kept as a map from key to how
//! many live paths have it, plus how many keys have two or more. A state's paths change only through the lifecycle state's mutators, which
//! journal each added and vacated path on request ([`prikk_replay::NodeLifecycleState::record_path_changes`]); applying a claim's journal
//! to the counts costs the claim's own changed paths, not the state's size. The batch's apply-only pass uses it to know, claim by claim,
//! exactly what 0.58.0's per-claim `refuse_colliding_entries` over every entry told it, without listing every entry per claim.

use std::collections::HashMap;

use prikk_object::canonical_caseless_key;
use prikk_replay::{NodeLifecycleState, PathChange};

/// How many live paths share each canonical caseless key, and how many keys are shared.
pub(crate) struct CollisionIndex {
    counts: HashMap<String, u32>,
    /// Keys with two or more live paths.
    shared: usize,
}

impl CollisionIndex {
    /// The index of `state`'s live paths as they are now.
    pub(crate) fn of_state(state: &NodeLifecycleState) -> Self {
        let mut index = Self {
            counts: HashMap::new(),
            shared: 0,
        };
        for path in state.live_paths() {
            index.add(path.as_str());
        }
        index
    }

    fn add(&mut self, path: &str) {
        let count = self.counts.entry(canonical_caseless_key(path)).or_insert(0);
        *count += 1;
        if *count == 2 {
            self.shared += 1;
        }
    }

    fn remove(&mut self, path: &str) {
        let key = canonical_caseless_key(path);
        if let Some(count) = self.counts.get_mut(&key) {
            if *count == 2 {
                self.shared = self.shared.saturating_sub(1);
            }
            *count -= 1;
            if *count == 0 {
                self.counts.remove(&key);
            }
        }
    }

    /// Apply one claim's journal, in the order the mutators made it.
    pub(crate) fn apply(&mut self, changes: &[PathChange]) {
        for change in changes {
            if change.added {
                self.add(change.path.as_str());
            } else {
                self.remove(change.path.as_str());
            }
        }
    }

    /// Whether two live paths share a key: `refuse_colliding_entries` would refuse the state.
    pub(crate) fn collides(&self) -> bool {
        self.shared > 0
    }
}

#[cfg(test)]
mod tests;
