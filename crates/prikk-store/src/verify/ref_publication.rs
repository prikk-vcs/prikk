//! Retained active-state evidence for interrupted ref publication diagnostics.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectType, RefStatePayload, RefUpdatePayload};

use super::ActiveWalMetadataStatus;
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{FileObjectStore, ObjectReader};
use crate::refs::{RefPublicationIssue, RefStore};
use crate::wal::WalRecord;

pub(super) fn require_retained_evidence(
    layout: &RepositoryLayout,
    records: &[WalRecord],
    metadata: &ActiveWalMetadataStatus,
    trust_is_valid: bool,
    issues: &mut Vec<RefPublicationIssue>,
) -> Result<()> {
    for issue in issues.iter_mut() {
        if !matches!(
            issue.code,
            "PRIKK-VERIFY-REF-POINTER-LEADS-LOG"
                | "PRIKK-VERIFY-REF-LEGACY-LOG-LEADS"
                | "PRIKK-VERIFY-REF-POINTER-MISSING"
        ) {
            continue;
        }
        let Some(ref_name) = issue.ref_name.as_deref() else {
            mark_unproved(issue);
            continue;
        };
        if !trust_is_valid || !active_ref_matches(metadata, ref_name) {
            mark_unproved(issue);
            continue;
        }
        let target = interrupted_target(layout, ref_name, issue.code)?;
        if !block_matches_wal(layout, target, records)? {
            mark_unproved(issue);
        }
    }
    add_incomplete_cleanup_issue(layout, records, metadata, issues)?;
    Ok(())
}

fn add_incomplete_cleanup_issue(
    layout: &RepositoryLayout,
    records: &[WalRecord],
    metadata: &ActiveWalMetadataStatus,
    issues: &mut Vec<RefPublicationIssue>,
) -> Result<()> {
    let ActiveWalMetadataStatus::ValidForNonEmptyWal { ref_name } = metadata else {
        return Ok(());
    };
    if issues
        .iter()
        .any(|issue| issue.ref_name.as_deref() == Some(ref_name))
    {
        return Ok(());
    }
    let store = RefStore::new(layout.clone());
    let Some(state_id) = store.read_current_ref_state_id(ref_name)? else {
        return Ok(());
    };
    let objects = FileObjectStore::new(layout.clone());
    let state = objects
        .read_typed(state_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {state_id}")))?;
    let target = RefStatePayload::decode_canonical(&state.canonical_payload, state.schema_version)?
        .target_object_id;
    if block_matches_wal(layout, target, records)? {
        issues.push(RefPublicationIssue {
            code: "PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING",
            ref_name: Some(ref_name.clone()),
            message: "pointer and log agree but matching active publication state remains"
                .to_string(),
            blocking: true,
        });
    }
    Ok(())
}

fn active_ref_matches(metadata: &ActiveWalMetadataStatus, ref_name: &str) -> bool {
    matches!(
        metadata,
        ActiveWalMetadataStatus::ValidForNonEmptyWal { ref_name: active } if active == ref_name
    )
}

fn interrupted_target(
    layout: &RepositoryLayout,
    ref_name: &str,
    issue_code: &str,
) -> Result<prikk_object::ObjectId> {
    let store = RefStore::new(layout.clone());
    let state_id = if issue_code == "PRIKK-VERIFY-REF-POINTER-LEADS-LOG" {
        store.read_current_ref_state_id(ref_name)?.ok_or_else(|| {
            PrikkError::Integrity(format!("interrupted ref {ref_name} has no pointer"))
        })?
    } else {
        let replay = store.replay_log(ref_name)?;
        // RFC 102 Stage 2: a damaged record silently missing from `replay.records` could make
        // `.last()` below resolve to a stale earlier record instead of the true (but corrupted)
        // tip, misidentifying the interrupted-publication target this evidence check is proving.
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(format!(
                "interrupted ref {ref_name} log has a damaged record"
            )));
        }
        let record = replay.records.last().ok_or_else(|| {
            PrikkError::Integrity(format!("interrupted ref {ref_name} has no log record"))
        })?;
        RefUpdatePayload::decode_canonical(&record.envelope.canonical_payload)?.new_ref_state_id
    };
    let objects = FileObjectStore::new(layout.clone());
    let state = objects
        .read_typed(state_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {state_id}")))?;
    Ok(
        RefStatePayload::decode_canonical(&state.canonical_payload, state.schema_version)?
            .target_object_id,
    )
}

fn block_matches_wal(
    layout: &RepositoryLayout,
    target: prikk_object::ObjectId,
    records: &[WalRecord],
) -> Result<bool> {
    if records.is_empty() {
        return Ok(false);
    }
    let objects = FileObjectStore::new(layout.clone());
    let block = objects
        .read_typed(target, ObjectType::Block)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing Block object: {target}")))?;
    let payload = BlockPayload::decode_canonical(&block.canonical_payload)?;
    Ok(payload.patch_ids
        == records
            .iter()
            .map(|record| record.envelope.object_id())
            .collect::<Vec<_>>())
}

fn mark_unproved(issue: &mut RefPublicationIssue) {
    issue.code = "PRIKK-VERIFY-REF-DIVERGENCE";
    issue.message =
        "pointer/log divergence is not proved by matching retained active state and trust"
            .to_string();
    issue.blocking = true;
}
