//! Replay against an **explicit recorded baseline** (0.56.0 step W2, R2), not a ref's live tip.
//!
//! Split out of `patch_replay.rs` itself purely to keep that file's own line count where it was
//! (`size-check`'s gate, not a style preference) -- this is one function, with no state or other
//! callers of its own.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::layout::RepositoryLayout;
use crate::node::node_lifecycle::NodeLifecycleState;
use crate::object_store::ObjectReader;
use crate::wal::WalReplay;

use super::FoldedWorktreeBaseline;
use super::read::single_parent_chain;

/// Replay lifecycle state as of an explicit recorded baseline, not whatever `ref_name` resolves to
/// live: the sealed state at `baseline` (or empty genesis when `None`), with the active WAL's
/// records whose `seq` is at most `wal_sequence_cap` folded on top. Every other baseline-resolving
/// function in `patch_replay.rs` derives its sealed side from a ref's *current* pointer
/// (`resolve_worktree_baseline`) -- deliberately, because "what does this ref mean right now" is
/// what `commit`/`worktree-status` want. This function answers a different question: "what did the
/// worktree's own record say it was last written to match," which can name a block the ref no
/// longer points at (a producer moved the ref since) -- exactly the case `branch_switch.rs`'s
/// `require_clean` and `node_authoring.rs`'s deletion-inference loop need when the record they are
/// comparing against is stale, so that "clean"/"held" is judged against what the worktree was
/// actually last materialized from, not against the ref's moved tip (which would wrongly read the
/// worktree as missing files it never had a chance to receive).
///
/// `queued_on_other_ref` in the returned [`FoldedWorktreeBaseline`] is always `None` here: this
/// function answers "what did this worktree match as of the record," not "who owns the active WAL
/// right now" -- a question this call is not trying to answer.
pub(crate) fn resolve_state_at_recorded_baseline(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    baseline: Option<ObjectId>,
    wal_sequence_cap: u64,
    active_replay: &WalReplay,
    text_cache: &mut crate::lifecycle_cache::replay::TextCache,
) -> Result<FoldedWorktreeBaseline> {
    let lineage = match baseline {
        Some(block) => {
            let chain = single_parent_chain(object_store, block)?;
            let horizon = *chain
                .first()
                .ok_or_else(|| PrikkError::Integrity(format!("Block {block} lineage is empty")))?;
            Some((block, horizon))
        }
        None => None,
    };
    let mut state = match lineage {
        Some((block, horizon)) => crate::lifecycle_cache::incremental::resolve_baseline_state_with(
            layout,
            object_store,
            block,
            horizon,
            crate::lifecycle_cache::incremental::CacheWrite::Never,
        )?
        .state()
        .clone(),
        None => NodeLifecycleState::new(),
    };
    let records: Vec<_> = active_replay
        .records
        .iter()
        .filter(|record| record.seq <= wal_sequence_cap)
        .cloned()
        .collect();
    if !records.is_empty() {
        crate::lifecycle_cache::replay::apply_queued_patch_envelopes(
            Some(layout),
            object_store,
            &records,
            &mut state,
            text_cache,
            lineage,
        )?;
    }
    Ok(FoldedWorktreeBaseline {
        state,
        lineage,
        queued_on_other_ref: None,
    })
}
