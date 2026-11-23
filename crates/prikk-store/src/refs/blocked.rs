//! 0.59.0 step G2a: **which state the write-path precondition refused** (a read-only classification, for the sentence that names the way out).
//!
//! The precondition ([`super::ensure_publication_precondition_with_replay`]) answers one `IncompletePublication` for several genuine
//! states. A caller that knows the state can name the command that clears it (or say plainly that none does); the upper layer builds those
//! sentences (they name `SOUND_COPY` and `ref complete`), so this module only **classifies**, in the order the precondition checks:
//! a damaged pointer index, a log record carrying a time this version never writes, a pointer naming a RefState the object index lacks,
//! candidate debris. A lead (a pointer that disagrees with the log) and a settled queue are classified by the callers that already own those
//! questions; anything this answers `None` to is the last arm.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RefUpdatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReadSnapshot;

/// The sound-copy sequence, in full: the way out of every state no command repairs in place. Defined here (a leaf both `way_out` and `ref_completion` read)
/// so that `ref_completion` names it without an edge into `way_out`, which would close a cycle through `verify`. `way_out::SOUND_COPY` is this value.
pub(crate) const SOUND_COPY: &str = "restore a sound copy of the sealed history: `prikk setup <new-dir>`, then, in \
<new-dir>, `prikk archive verify <backup>` and `prikk archive import <backup> --adopt` (or `prikk bundle \
import --input <bundle>` for one branch); the key that sealed the history must be adopted in <new-dir> \
(`archive verify` reports it). Only sealed history travels in a backup: a commit still waiting for a \
`prikk seal` is made again there";

/// The state the precondition refused, when this module can tell.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PublicationBlock {
    /// A pointer-index record failed: `doctor --rebuild-pointer-index` re-derives the index from the ref log.
    PointerIndexDamaged,
    /// A ref-log record carries a non-zero `created_at`: no writer of this version produces one.
    LegacyTimestamp,
    /// The newest pointer of `ref_name` names a RefState the object index does not hold.
    MissingRefState {
        /// The ref.
        ref_name: String,
        /// The RefState it names.
        ref_state_id: ObjectId,
    },
    /// A ref-pointer candidate is left in `refs/tmp`.
    CandidateDebris,
}

/// Classify the block, `None` when it is none of the four. Read-only; whole reads of the pointer index, the ref log and the object index
/// (the error path of a refusal, not a hot path).
///
/// # Errors
///
/// A container that cannot be read at all.
pub(crate) fn classify_publication_block(
    layout: &RepositoryLayout,
) -> Result<Option<PublicationBlock>> {
    // The same whole-container read the precondition and `mismatched_lead_candidate` make for the same reason (the error path of a refusal, once).
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let pointer_replay = super::replay_pointer_index(layout)?;
    if pointer_replay.has_item_failure() {
        return Ok(Some(PublicationBlock::PointerIndexDamaged));
    }
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    if let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )? {
        let discovery = super::decode_ref_container_records(&bytes)?;
        if !discovery.has_damage() {
            for record in &discovery.records {
                let update =
                    RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
                if update.created_at != 0 {
                    return Ok(Some(PublicationBlock::LegacyTimestamp));
                }
            }
        }
    }
    let objects = ObjectReadSnapshot::open(layout)?;
    let mut newest: std::collections::BTreeMap<[u8; 32], (String, ObjectId)> =
        std::collections::BTreeMap::new();
    for entry in &pointer_replay.entries {
        newest.insert(
            entry.ref_name_key,
            (entry.ref_name.clone(), entry.ref_state_id),
        );
    }
    for (ref_name, ref_state_id) in newest.into_values() {
        if !objects.contains_object(ObjectType::RefState, ref_state_id) {
            return Ok(Some(PublicationBlock::MissingRefState {
                ref_name,
                ref_state_id,
            }));
        }
    }
    if !super::verify::candidate_issues(layout)?.is_empty() {
        return Ok(Some(PublicationBlock::CandidateDebris));
    }
    Ok(None)
}

/// Candidate debris keeps the text it had until step G2b decides what clears it (0.59.0 G2a: "candidate debris keeps today's text").
pub(crate) fn candidate_debris_refusal() -> PrikkError {
    PrikkError::IncompletePublication(
        "repository mutation is blocked by incomplete ref publication; run verify/doctor and use signer-backed seal retry"
            .to_string(),
    )
}
