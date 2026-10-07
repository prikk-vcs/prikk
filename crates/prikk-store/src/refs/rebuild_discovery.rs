//! RFC 165 R5: one whole-container read of the ref log, for the rebuild -- every sound record,
//! decoded, in file order (the rebuild's own source of truth), plus the same damage/tail status
//! [`super::ref_log_tail_status`] reports, from that single read rather than a second one.
//! `container` is private to `refs`, so this is the boundary-respecting way a sibling module
//! (`pointer_rebuild.rs`) reaches it: a plain, public type (`RefUpdatePayload` is already
//! `prikk_object`'s own, not `refs`-private) out of a `refs`-internal read, the same shape
//! `ref_log_tail_status` already uses. Split out of `refs.rs` itself (size-check: these functions
//! and their result types, no reason to keep them inline) rather than declared, since there is
//! nothing about them that needs the rest of `refs.rs` beside it.
//!
//! [`mismatched_lead_candidate`] joined this module for the same reason, not because it is part of
//! the rebuild: `refs.rs` had no room left under the size limit once it grew a `tail_offset` field.

use prikk_error::Result;
use prikk_object::{ObjectId, RefUpdatePayload};

use crate::foundation::layout::RepositoryLayout;

use super::container;

pub(crate) struct RefLogDiscoveryForRebuild {
    pub(crate) records: Vec<RefUpdatePayload>,
    pub(crate) trailing_partial_bytes: usize,
    /// The tail's own byte offset (019 §5.2: the rebuild's refusal must name where the tail is, not a
    /// placeholder). `0` when `trailing_partial_bytes` is `0` too, so the pair is meaningless.
    pub(crate) tail_offset: usize,
    pub(crate) interior_damage: Option<String>,
}

pub(crate) fn decode_ref_log_for_rebuild(
    layout: &RepositoryLayout,
) -> Result<RefLogDiscoveryForRebuild> {
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("pointer-index-rebuild");
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )?
    else {
        return Ok(RefLogDiscoveryForRebuild {
            records: Vec::new(),
            trailing_partial_bytes: 0,
            tail_offset: 0,
            interior_damage: None,
        });
    };
    let discovery = container::decode_ref_container_records(&bytes)?;
    let records: Result<Vec<RefUpdatePayload>> = discovery
        .records
        .iter()
        .map(|record| RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload))
        .collect();
    let records = records?;
    if let Some(tail) = container::ref_log_container_tail(&bytes, &discovery) {
        return Ok(RefLogDiscoveryForRebuild {
            records,
            trailing_partial_bytes: tail.len,
            tail_offset: tail.offset,
            interior_damage: None,
        });
    }
    let interior_damage =
        discovery
            .record_outcomes
            .iter()
            .find_map(|outcome| match &outcome.status {
                container::RefContainerRecordStatus::Failed { message, .. } => {
                    Some(message.clone())
                }
                container::RefContainerRecordStatus::Evaluated => None,
            });
    Ok(RefLogDiscoveryForRebuild {
        records,
        trailing_partial_bytes: 0,
        tail_offset: 0,
        interior_damage,
    })
}

/// RFC 165 R4/019 §5.2: the first ref (other than `exclude_ref_name`), in key order, whose pointer
/// disagrees with the ref log -- read the same way `super::ensure_publication_precondition` does,
/// as a second, deliberate whole-container read on the refusal path only (never the hot path), for
/// an upper-layer caller to classify against RFC 165 R4's own rule, which `refs` itself must not
/// depend on (`ref_completion` is upper; this module is lower -- RFC 149's layer rule). `None` when
/// every ref agrees, which is also the right answer when `ensure_publication_precondition` failed
/// for an unrelated reason (damage, a missing object, candidate debris, pending cleanup): those
/// checks run only once this exact agreement walk finds nothing, so replaying it here agrees with
/// it by construction.
pub(crate) fn mismatched_lead_candidate(
    layout: &RepositoryLayout,
    exclude_ref_name: Option<&str>,
) -> Result<Option<(String, ObjectId)>> {
    use std::collections::{BTreeMap, BTreeSet};

    // The same whole-container read `ensure_publication_precondition` makes for the same reason,
    // replayed a second time, only on the refusal path (RFC 160 report v1 §F1 already covers this
    // category under `ref-log-replay`; this is not a new one).
    #[cfg(test)]
    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");

    let excluded_key = exclude_ref_name.map(crate::foundation::layout::ref_name_key_bytes);
    let pointer_replay = super::replay_pointer_index(layout)?;
    if pointer_replay.has_item_failure() {
        return Ok(None);
    }
    let mut newest_pointer: BTreeMap<[u8; 32], ObjectId> = BTreeMap::new();
    let mut names: BTreeMap<[u8; 32], String> = BTreeMap::new();
    for entry in &pointer_replay.entries {
        newest_pointer.insert(entry.ref_name_key, entry.ref_state_id);
        names.insert(entry.ref_name_key, entry.ref_name.clone());
    }
    let relative = layout.repository_relative(
        &layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A),
    )?;
    let mut newest_log: BTreeMap<[u8; 32], ObjectId> = BTreeMap::new();
    if let Some(bytes) = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )? {
        let discovery = container::decode_ref_container_records(&bytes)?;
        if discovery.has_damage() {
            return Ok(None);
        }
        for record in &discovery.records {
            let update = RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?;
            if update.created_at != 0 {
                return Ok(None);
            }
            names.insert(record.ref_name_key, update.ref_name.clone());
            newest_log.insert(record.ref_name_key, update.new_ref_state_id);
        }
    }
    let mut keys: BTreeSet<[u8; 32]> = BTreeSet::new();
    keys.extend(newest_pointer.keys().copied());
    keys.extend(newest_log.keys().copied());
    for key in keys {
        if Some(key) == excluded_key {
            continue;
        }
        let pointer_id = newest_pointer.get(&key).copied();
        let log_id = newest_log.get(&key).copied();
        if pointer_id != log_id {
            if let (Some(leading_id), Some(name)) = (pointer_id, names.get(&key)) {
                return Ok(Some((name.clone(), leading_id)));
            }
        }
    }
    Ok(None)
}
