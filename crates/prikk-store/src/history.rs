//! Read-only sealed-history inspection helpers.
//!
//! PR-014 exposes a small history view built from the current RefState chain. It is intentionally
//! read-only and does not perform graph traversal beyond the published ref-state lineage.

use std::collections::HashSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockKind, BlockPayload, ObjectId, ObjectType, RefStatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_replay::decode::decode_patch_message;
use crate::refs::{RefStore, resolve_ref_tip_block};
use crate::rollback_verify::verify_rollback_patch_envelope;

/// Default number of history entries shown by the CLI.
pub const DEFAULT_HISTORY_LIMIT: usize = 20;

/// Read-only history view for a single ref.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefHistory {
    /// Human-readable ref name.
    pub ref_name: String,
    /// Entries ordered from newest to oldest.
    pub entries: Vec<HistoryEntry>,
}

impl RefHistory {
    /// Return true when the ref has no published history.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// One published RefState and its target Block summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    /// RefState object ID for this publication.
    pub ref_state_id: ObjectId,
    /// Target Block object ID.
    pub block_id: ObjectId,
    /// Monotonic ref update sequence.
    pub update_seq: u64,
    /// Previous RefState object ID, if any.
    pub previous_ref_state_id: Option<ObjectId>,
    /// Block kind.
    pub block_kind: BlockKind,
    /// Number of parent blocks referenced by the target Block.
    pub parent_count: usize,
    /// Number of patches referenced by the target Block.
    pub patch_count: usize,
    /// Number of required attestations attached to this RefState.
    pub required_attestation_count: usize,
    /// Number of rollback-marked Patch objects in the target Block.
    pub rollback_patch_count: usize,
    /// Whether this entry's target Block contains at least one rollback-marked Patch.
    pub is_rollback_block: bool,
    /// Messages of this block's own patches that carry one (RFC 123 §8), in `patch_ids` order.
    /// **Not one entry per patch** -- a schema 1/2/3 patch (or a genuinely absent one) contributes
    /// no entry at all, since absence of a message is not the same fact as an empty one (§8.4) and
    /// must not be displayed as if it were.
    pub patch_messages: Vec<PatchMessage>,
}

/// One patch's message, found while building a [`HistoryEntry`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchMessage {
    /// The Patch object carrying this message.
    pub patch_id: ObjectId,
    /// The message itself -- never empty (`decode_patch_message` refuses `Some("")`, mirroring
    /// `PatchPayload::validate`).
    pub message: String,
}

/// Load history for a ref, newest first.
///
/// The function follows `RefState.previous_ref_state_id` links and validates that every RefState
/// targets a persisted Block that decodes successfully. `limit == 0` means no entries are returned.
pub fn load_ref_history(
    layout: &RepositoryLayout,
    ref_name: &str,
    limit: usize,
) -> Result<RefHistory> {
    let ref_store = RefStore::new(layout.clone());
    let object_store = ObjectReadSnapshot::open(layout)?;
    let mut current = ref_store.read_current_ref_state_id(ref_name)?;
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    while let Some(ref_state_id) = current {
        if entries.len() >= limit {
            break;
        }
        if !seen.insert(ref_state_id) {
            return Err(PrikkError::Integrity(format!(
                "RefState chain for {ref_name} contains a cycle at {ref_state_id}"
            )));
        }
        let ref_state = read_ref_state(&object_store, ref_state_id, ref_name)?;
        // RFC 147 §3b: a `Tag` ref-state names a Tag object one hop from its Block. Resolving here
        // is what removes `read_block`'s "is Tag, expected Block" refusal for a valid tag ref --
        // that message stays for a genuinely wrong object type, which is all it ever meant to say.
        let (block_id, _tag_envelope) = resolve_ref_tip_block(&object_store, &ref_state)?;
        let block = read_block(&object_store, block_id)?;
        let rollback_patch_count =
            count_rollback_patches(&object_store, block_id, &block.patch_ids)?;
        let patch_messages = read_patch_messages(&object_store, block_id, &block.patch_ids)?;
        entries.push(HistoryEntry {
            ref_state_id,
            block_id,
            update_seq: ref_state.update_seq,
            previous_ref_state_id: ref_state.previous_ref_state_id,
            block_kind: block.kind,
            parent_count: block.parent_block_ids.len(),
            patch_count: block.patch_ids.len(),
            required_attestation_count: ref_state.required_attestation_ids.len(),
            rollback_patch_count,
            is_rollback_block: rollback_patch_count != 0,
            patch_messages,
        });
        current = ref_state.previous_ref_state_id;
    }

    Ok(RefHistory {
        ref_name: ref_name.to_string(),
        entries,
    })
}

/// Load history for a received ref (DC-78 ruling 4), newest first. Received refs have no ref-log
/// chain of their own (`received.rs`'s single-overwrite pointer) — this walks the same
/// `RefState.previous_ref_state_id` links as [`load_ref_history`], starting from the received tip.
/// Each RefState's *embedded* name is checked against the origin's own name (the `remotes/` prefix is
/// a local rename applied only to the received pointer, never written into the objects themselves —
/// exactly why received refs cannot use `refs/by-id/`'s pointer format, whose consistency check
/// requires the opposite: pointer name and embedded name to agree).
pub fn load_received_ref_history(
    layout: &RepositoryLayout,
    received_ref_name: &str,
    limit: usize,
) -> Result<RefHistory> {
    let Some(pointer) = crate::received::read_received_pointer(layout, received_ref_name)? else {
        return Ok(RefHistory {
            ref_name: received_ref_name.to_string(),
            entries: Vec::new(),
        });
    };
    let Some(origin_ref_name) = received_ref_name.strip_prefix("remotes/") else {
        return Err(PrikkError::InvalidName(format!(
            "{received_ref_name} is not a received ref"
        )));
    };
    let object_store = ObjectReadSnapshot::open(layout)?;
    let mut current = Some(pointer.ref_state_id);
    let mut entries = Vec::new();
    let mut seen = HashSet::new();

    while let Some(ref_state_id) = current {
        if entries.len() >= limit {
            break;
        }
        if !seen.insert(ref_state_id) {
            return Err(PrikkError::Integrity(format!(
                "RefState chain for {received_ref_name} contains a cycle at {ref_state_id}"
            )));
        }
        let ref_state = read_ref_state(&object_store, ref_state_id, origin_ref_name)?;
        // RFC 147 §3b, the received half: a received *tag* is reachable today (`bundle export
        // --ref tags/x` then `bundle import` records `remotes/tags/x`), so this path meets the same
        // Tag ref-state the local one does and resolves it the same way.
        let (block_id, _tag_envelope) = resolve_ref_tip_block(&object_store, &ref_state)?;
        let block = read_block(&object_store, block_id)?;
        let rollback_patch_count =
            count_rollback_patches(&object_store, block_id, &block.patch_ids)?;
        let patch_messages = read_patch_messages(&object_store, block_id, &block.patch_ids)?;
        entries.push(HistoryEntry {
            ref_state_id,
            block_id,
            update_seq: ref_state.update_seq,
            previous_ref_state_id: ref_state.previous_ref_state_id,
            block_kind: block.kind,
            parent_count: block.parent_block_ids.len(),
            patch_count: block.patch_ids.len(),
            required_attestation_count: ref_state.required_attestation_ids.len(),
            rollback_patch_count,
            is_rollback_block: rollback_patch_count != 0,
            patch_messages,
        });
        current = ref_state.previous_ref_state_id;
    }

    Ok(RefHistory {
        ref_name: received_ref_name.to_string(),
        entries,
    })
}

fn read_ref_state(
    object_store: &impl ObjectReader,
    ref_state_id: ObjectId,
    ref_name: &str,
) -> Result<RefStatePayload> {
    let Some(envelope) = object_store.read_object(ref_state_id)? else {
        return Err(PrikkError::Integrity(format!(
            "history RefState {ref_state_id} is missing"
        )));
    };
    if envelope.object_type != ObjectType::RefState {
        return Err(PrikkError::Integrity(format!(
            "history object {ref_state_id} is {}, expected RefState",
            envelope.object_type
        )));
    }
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if payload.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "history RefState {ref_state_id} name mismatch: expected {ref_name}, got {}",
            payload.ref_name
        )));
    }
    Ok(payload)
}

fn count_rollback_patches(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
    patch_ids: &[ObjectId],
) -> Result<usize> {
    let mut count = 0_usize;
    for patch_id in patch_ids {
        let Some(envelope) = object_store.read_typed(*patch_id, ObjectType::Patch)? else {
            return Err(PrikkError::Integrity(format!(
                "history Block {block_id} references missing Patch {patch_id}"
            )));
        };
        let context = format!("history Block {block_id} Patch {patch_id}");
        if verify_rollback_patch_envelope(&envelope, &context)? {
            count = count.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("history rollback patch count overflow".to_string())
            })?;
        }
    }
    Ok(count)
}

/// Collect this block's own patches' messages, in `patch_ids` order (RFC 123 §8, `prikk log`
/// display). A patch with no message (schema 1/2/3, or a genuinely absent RFC 113 import) is
/// omitted entirely -- not represented by an empty-message entry -- since absence and emptiness
/// must never read the same way (§8.4).
fn read_patch_messages(
    object_store: &impl ObjectReader,
    block_id: ObjectId,
    patch_ids: &[ObjectId],
) -> Result<Vec<PatchMessage>> {
    let mut messages = Vec::new();
    for patch_id in patch_ids {
        let Some(envelope) = object_store.read_typed(*patch_id, ObjectType::Patch)? else {
            return Err(PrikkError::Integrity(format!(
                "history Block {block_id} references missing Patch {patch_id}"
            )));
        };
        if let Some(message) =
            decode_patch_message(&envelope.canonical_payload, envelope.schema_version)?
        {
            messages.push(PatchMessage {
                patch_id: *patch_id,
                message,
            });
        }
    }
    Ok(messages)
}

fn read_block(object_store: &impl ObjectReader, block_id: ObjectId) -> Result<BlockPayload> {
    let Some(envelope) = object_store.read_object(block_id)? else {
        return Err(PrikkError::Integrity(format!(
            "history Block {block_id} is missing"
        )));
    };
    if envelope.object_type != ObjectType::Block {
        return Err(PrikkError::Integrity(format!(
            "history object {block_id} is {}, expected Block",
            envelope.object_type
        )));
    }
    BlockPayload::decode_canonical(&envelope.canonical_payload)
}

#[cfg(test)]
mod tests;
