//! RFC 165 R5: one whole-container read of the ref log, for the rebuild -- every sound record,
//! decoded, in file order (the rebuild's own source of truth), plus the same damage/tail status
//! [`super::ref_log_tail_status`] reports, from that single read rather than a second one.
//! `container` is private to `refs`, so this is the boundary-respecting way a sibling module
//! (`pointer_rebuild.rs`) reaches it: a plain, public type (`RefUpdatePayload` is already
//! `prikk_object`'s own, not `refs`-private) out of a `refs`-internal read, the same shape
//! `ref_log_tail_status` already uses. Split out of `refs.rs` itself (size-check: one function and
//! its result type, no reason to keep them inline) rather than declared, since there is nothing
//! about this pair that needs the rest of `refs.rs` beside it.

use prikk_error::Result;
use prikk_object::RefUpdatePayload;

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
