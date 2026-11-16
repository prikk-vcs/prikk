//! The root-checking walk of one lineage, for the callers that record what it confirmed (0.58.0 R2).
//!
//! `seal` walks a parent's lineage (`resolved_parent_state`: [`super::validate_v2_lineage`] then [`super::verify_v2_lineage_roots`]) and
//! records what the walk confirmed in `verified_blocks`; a repository that received its history has not walked it, so nothing anchors
//! (R1's cause). This is the same walk -- the same lineage collection, the same per-Block check ([`super::confirm_block_root`]) -- but it
//! keeps **one** state instead of one per Block, stops at the first Block that fails and says which, and hands back the tip's state and
//! text cache. It is used by the adopting imports (they record the ids) and by the commit's miss (it records the ids and takes the text).
//! Neither runs it beside the replay it replaces.

use prikk_error::PrikkError;
use prikk_object::ObjectId;

use super::{LineageStateMemo, collect_v2_lineage, confirm_block_root};
use crate::lifecycle_cache::replay::TextCache;
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::ObjectReader;

/// What walking a lineage confirmed.
#[derive(Debug)]
pub(crate) struct LineageConfirmation {
    /// Every Block whose state root the walk recomputed and found equal to the root it signs, oldest first. A failure leaves the
    /// Blocks before the failing one here and nothing from it on.
    pub(crate) confirmed: Vec<ObjectId>,
    /// The tip's state and text cache; `Some` only when the whole lineage was confirmed.
    pub(crate) tip: Option<(NodeLifecycleState, TextCache)>,
    /// The Block the walk stopped at, and why. `None` when the whole lineage was confirmed.
    pub(crate) failure: Option<(ObjectId, PrikkError)>,
}

#[cfg(test)]
std::thread_local! {
    static CONFIRM_LINEAGE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Walks this thread has made (`cfg(test)`): the control that holds "a second text miss anchors, it does not walk again".
#[cfg(test)]
pub(crate) fn confirm_lineage_calls_for_test() -> usize {
    CONFIRM_LINEAGE_CALLS.with(std::cell::Cell::get)
}

/// Walk `tip`'s state-derivation lineage (the mainline of a merge) from genesis, checking every root. Never writes.
pub(crate) fn confirm_lineage(reader: &impl ObjectReader, tip: ObjectId) -> LineageConfirmation {
    #[cfg(test)]
    CONFIRM_LINEAGE_CALLS.with(|calls| calls.set(calls.get() + 1));
    let lineage = match collect_v2_lineage(reader, tip, &LineageStateMemo::new()) {
        Ok(lineage) => lineage,
        // A Block the walk cannot read or accept stops it before any root is checked; the Blocks older than it are not reached.
        Err(failure) => {
            return LineageConfirmation {
                confirmed: Vec::new(),
                tip: None,
                failure: Some(failure),
            };
        }
    };
    let mut state = NodeLifecycleState::new();
    let mut text_cache = TextCache::new();
    let mut confirmed = Vec::with_capacity(lineage.len());
    for (block_id, payload) in lineage.iter().rev() {
        if let Err(err) =
            confirm_block_root(reader, *block_id, payload, &mut state, &mut text_cache)
        {
            return LineageConfirmation {
                confirmed,
                tip: None,
                failure: Some((*block_id, err)),
            };
        }
        confirmed.push(*block_id);
    }
    LineageConfirmation {
        confirmed,
        tip: Some((state, text_cache)),
        failure: None,
    }
}
