//! Retained active-state evidence for interrupted ref publication diagnostics.

use prikk_error::{PrikkError, Result};
use prikk_object::{BlockPayload, ObjectType, RefStatePayload};

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
        // RFC 165 R4: `trust_is_valid` stays as its own, broader gate -- it answers "did the Objects
        // and trust-verification stages themselves run cleanly at all" (DC-95 Stage 2 Step 0's own
        // ruling: an accumulator's emptiness proves nothing unless its producer ran to completion),
        // a different question from "does *this* lead's own signature verify", which
        // `crate::ref_completion::plan_ref_completion`'s own condition (a) checks per ref. Both must
        // hold. The completion rule itself (conditions a, b [folded into its own lead
        // classification], c, d [only when the active WAL's retained metadata claims this exact
        // ref], e) now decides completable vs. divergent -- replacing this function's own narrower,
        // WAL-only check (`active_ref_matches`/`block_matches_wal`, below), which refused every
        // non-WAL-consuming publication (`branch create`, `tag create`, `merge`, `sync adopt-tag`)
        // unconditionally -- the DC-38 "seal only" limitation RFC 165 R4 exists to end. `verify`'s own
        // report and `prikk ref complete`'s own precondition now share the one table, exactly as the
        // handoff requires: a lead is never reported completable here and refused there, or the
        // reverse.
        if !trust_is_valid {
            mark_unproved(issue);
            continue;
        }
        match crate::ref_completion::plan_ref_completion(layout, ref_name)? {
            // 019 §5.2: name the way out directly in `verify`'s own message -- `verify`'s report
            // prints only `issue.code` and `issue.message`, never a separate recommendation the way
            // `doctor`'s own issues do, so the command has to be in the message itself.
            Ok(_plan) => {
                issue.message = format!("{}; run `prikk ref complete {ref_name}`", issue.message)
            }
            Err(_refusal) => mark_unproved(issue),
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
