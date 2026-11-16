//! The history this repository has confirmed by replay, and records (0.58.0 R2): what an adopting import confirmed about the history it
//! landed (option (i)), and the commit's miss that walks instead of replaying (option (ii)). Below, both.
//!
//! # The adopting import
//!
//! A repository that received a branch has never replayed it, so its `verified_blocks` record is empty and nothing can anchor: every
//! later `extract` refuses and every text-editing `commit` replays the whole lineage (R1 measured 2.0× the content per commit; the
//! readers, R2's first step). `archive import --adopt` and `bundle import --adopt` therefore walk the adopted tip's lineage once,
//! **with the check `verify` and `seal` make** ([`crate::block_state::confirm_lineage`]), and record the Blocks it confirmed in that
//! same record -- no second one.
//!
//! **Best effort, and never a refusal.** The objects and the ref are already written when this runs; a refusal here would be the 0.44.0
//! defect (an import that wrote and then said no). Failing to record leaves fewer entries, and the commit's miss records them later. A
//! Block that does **not** replay to the root it signs -- a Block an adopted key signed, and a real finding -- is the one thing this
//! reports: [`AdoptedHistoryNote`], one stderr line from the CLI naming the Block and `prikk verify`. The Blocks before it are recorded;
//! it and everything after it are not.
//!
//! **What it records is "this machine recomputed the root", not trust.** `anchor_trust` still requires an adopted-key signature and a
//! valid manifest at every use, so the record never widens what an anchor may be.

use prikk_error::Result;
use prikk_object::{ObjectId, RefStatePayload};

use crate::block_state::confirm_lineage::confirm_lineage;
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::verified_blocks::record_verified_blocks;

/// The imported history did not replay cleanly: the first Block whose recomputed state root differs from the root it signs, or that the
/// walk could not read or accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdoptedHistoryNote {
    /// The Block the walk stopped at.
    pub block_id: ObjectId,
}

impl std::fmt::Display for AdoptedHistoryNote {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "the imported history did not replay cleanly at block {}; run `prikk verify`",
            self.block_id
        )
    }
}

/// Walk the lineage of the Block `landed_ref_state_id` (the adopted ref's tip RefState) names, and record what it confirms.
pub(crate) fn record_adopted_history(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    landed_ref_state_id: ObjectId,
) -> Option<AdoptedHistoryNote> {
    let tip = tip_block(reader, landed_ref_state_id).ok()?;
    record_lineage(layout, reader, tip)
}

/// [`record_adopted_history`] from the tip Block.
fn record_lineage(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    tip: ObjectId,
) -> Option<AdoptedHistoryNote> {
    // A recorded Block was recorded with its whole lineage confirmed, so an import run again over what is already recorded walks nothing.
    if crate::verified_blocks::load_verified_blocks(layout).contains(&tip) {
        return None;
    }
    let walked = confirm_lineage(reader, tip);
    record_verified_blocks(layout, walked.confirmed);
    walked
        .failure
        .map(|(block_id, _)| AdoptedHistoryNote { block_id })
}

fn tip_block(reader: &impl ObjectReader, ref_state_id: ObjectId) -> Result<ObjectId> {
    let envelope = reader.read_object(ref_state_id)?.ok_or_else(|| {
        prikk_error::PrikkError::Integrity(format!("RefState {ref_state_id} is missing"))
    })?;
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    Ok(crate::refs::resolve_ref_tip_block(reader, &payload)?.0)
}

/// [`crate::lifecycle_cache::materialize_edited_text_anchored`] for the one caller that may record (`commit`'s authoring of a text edit), 0.58.0 R2 option (ii).
/// The anchor first; and on a miss -- a repository whose record does not cover this lineage, such as one that received its history --
/// **the root-checking walk replaces the unrecorded replay, never runs beside it**: the walk is the check `verify` and `seal` make
/// ([`crate::block_state::confirm_lineage::confirm_lineage`]), it yields the same text cache, and the Blocks it confirmed are recorded in
/// `verified_blocks` (the one record), so the next text miss anchors. The unrecorded replay ([`crate::lifecycle_cache::materialize_edited_text`]) checks no roots
/// and so never records.
///
/// A lineage that does not replay is **refused here, by Block, naming `prikk verify`, and nothing is recorded**: the old replay would have
/// authored an edit on top of history `seal` and `verify` refuse. A walk that did confirm every root but did not yield a text hashing to
/// `expected_blob_id`, or whose genesis is not the horizon, answers `None` (the caller's own "could not be materialized"), after recording.
pub(crate) fn materialize_edited_text_recording(
    layout: &crate::foundation::layout::RepositoryLayout,
    reader: &impl ObjectReader,
    baseline_block_id: ObjectId,
    lineage_horizon_id: ObjectId,
    node_id: prikk_object::NodeId,
    expected_blob_id: ObjectId,
) -> Result<Option<Vec<u8>>> {
    let wanted = std::collections::BTreeMap::from([(node_id, expected_blob_id)]);
    if let Some(mut texts) = crate::lifecycle_cache::anchored_text::anchored_texts(
        layout,
        reader,
        baseline_block_id,
        &wanted,
    ) {
        if let Some(text) = texts.remove(&node_id) {
            return Ok(Some(text));
        }
    }
    let walked = crate::block_state::confirm_lineage::confirm_lineage(reader, baseline_block_id);
    if let Some((block_id, err)) = walked.failure {
        return Err(prikk_error::PrikkError::Integrity(format!(
            "the history does not replay cleanly at block {block_id} ({err}); run `prikk verify`"
        )));
    }
    let genesis_is_horizon = walked.confirmed.first() == Some(&lineage_horizon_id);
    crate::verified_blocks::record_verified_blocks(layout, walked.confirmed);
    let Some((_, text_cache)) = walked.tip else {
        return Ok(None);
    };
    Ok(text_cache
        .get(&node_id)
        .filter(|text| {
            genesis_is_horizon
                && crate::text_span::text_blob_id(text).ok() == Some(expected_blob_id)
        })
        .cloned())
}

#[cfg(test)]
mod tests;
