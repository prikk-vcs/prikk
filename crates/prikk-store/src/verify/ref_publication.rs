//! Retained active-state evidence for interrupted ref publication diagnostics.

use prikk_error::Result;

use super::ActiveWalMetadataStatus;
use crate::foundation::layout::RepositoryLayout;
use crate::refs::RefPublicationIssue;
use crate::wal::WalRecord;

/// `pub(crate)` (0.52.0 step 1 Part A round 3, A-5): `archive::export_archive`'s own narrow
/// blocking-ref-publication refusal calls this directly, on inputs it computes just as narrowly as
/// `verify_repository_with_options` does (the active WAL's own replay, its metadata status, and the
/// trust policy's own structural soundness) -- not a copy of this rule, sharing it. This module stays
/// private (`mod ref_publication;` in `verify.rs`); `verify.rs` re-exports this one function.
pub(crate) fn require_retained_evidence(
    layout: &RepositoryLayout,
    records: &[WalRecord],
    metadata: &ActiveWalMetadataStatus,
    trust_is_valid: bool,
    issues: &mut Vec<RefPublicationIssue>,
) -> Result<()> {
    // 0.58.0 F2: before anything else, a lead is either a publication in flight, a lead that has since gone, or interrupted.
    let mut gone: Vec<usize> = Vec::new();
    for (position, issue) in issues.iter_mut().enumerate() {
        if issue.code != "PRIKK-VERIFY-REF-POINTER-LEADS-LOG" {
            continue;
        }
        let Some(ref_name) = issue.ref_name.clone() else {
            continue;
        };
        // The first lead is read here (this stage's own first read, by identity); `decide` reads the holder, then the lead again.
        let Some(first) = crate::refs::in_flight::lead_now(layout, &ref_name)? else {
            gone.push(position);
            continue;
        };
        match crate::refs::in_flight::decide(layout, &ref_name, first, || {
            crate::refs::in_flight::lead_now(layout, &ref_name)
        })? {
            crate::refs::in_flight::Lead::Gone => gone.push(position),
            crate::refs::in_flight::Lead::InFlight { pid } => {
                issue.code = crate::refs::in_flight::IN_FLIGHT_CODE;
                issue.blocking = false;
                issue.message = in_flight_finding(&ref_name, pid);
            }
            crate::refs::in_flight::Lead::Interrupted {
                holder_note: Some(note),
            } => {
                issue.message = format!(
                    "{} (the lock it took is still on disk: its holder {note})",
                    issue.message
                );
            }
            crate::refs::in_flight::Lead::Interrupted { holder_note: None } => {}
        }
    }
    for position in gone.into_iter().rev() {
        issues.remove(position);
    }
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
            Err(crate::ref_completion::CompletionRefusal::PublicationInFlight(pid)) => {
                issue.code = crate::refs::in_flight::IN_FLIGHT_CODE;
                issue.blocking = false;
                issue.message = in_flight_finding(ref_name, Some(pid));
            }
            // 0.59.0 RS2c: the lead changed between the plan's two reads -- a publisher is running; never a divergence.
            Err(crate::ref_completion::CompletionRefusal::PublicationChanging) => {
                issue.code = crate::refs::in_flight::IN_FLIGHT_CODE;
                issue.blocking = false;
                issue.message = in_flight_finding(ref_name, None);
            }
            Err(_refusal) => mark_unproved(issue),
        }
    }
    add_incomplete_cleanup_issue(layout, records, metadata, issues)?;
    Ok(())
}

/// The in-flight finding for a queue that was still changing while it was read (0.59.0 RS2d): the existing id, `info`, the existing sentence.
pub(super) fn in_flight_issue(ref_name: &str, pid: u32) -> RefPublicationIssue {
    RefPublicationIssue {
        code: crate::refs::in_flight::IN_FLIGHT_CODE,
        ref_name: Some(ref_name.to_string()),
        message: in_flight_finding(ref_name, Some(pid)),
        blocking: false,
    }
}

/// [`in_flight_issue`] when a holder read may not have named a pid (the lock file changing during a stage is the only sign of the writer).
pub(super) fn in_flight_issue_for(ref_name: &str, pid: Option<u32>) -> RefPublicationIssue {
    RefPublicationIssue {
        code: crate::refs::in_flight::IN_FLIGHT_CODE,
        ref_name: Some(ref_name.to_string()),
        message: in_flight_finding(ref_name, pid),
        blocking: false,
    }
}

/// The finding for a publication that is running: the 0.58.0 sentence, with the pid when a holder read named one.
fn in_flight_finding(ref_name: &str, pid: Option<u32>) -> String {
    match pid {
        Some(pid) => format!(
            "a publication of {ref_name} is in flight (pid {pid}, from its lock); nothing to complete, run verify again when it finishes"
        ),
        None => format!(
            "a publication of {ref_name} is in flight; nothing to complete, run verify again when it finishes"
        ),
    }
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
    if crate::refs::publication::queue_already_sealed(layout, ref_name, records)?.is_some() {
        // 0.59.0 RS2c: a queue that still holds a settled publication is also a `seal` that is **running**, between its publish and its drain. The
        // active lock is asked (the holder, then the queue again, fresh): a queue that has drained is no finding; a holder that appears running is
        // in flight (the existing finding, `info`); anything else is today's finding.
        let publisher = crate::refs::in_flight::active_publisher(layout)?;
        let fresh =
            crate::wal::Wal::for_layout(layout, crate::foundation::layout::DEFAULT_ACTIVE_NAME)
                .replay()?;
        if crate::refs::publication::queue_already_sealed(layout, ref_name, &fresh.records)?
            .is_none()
        {
            return Ok(());
        }
        if let Some(pid) = publisher {
            issues.push(RefPublicationIssue {
                code: crate::refs::in_flight::IN_FLIGHT_CODE,
                ref_name: Some(ref_name.clone()),
                message: in_flight_finding(ref_name, Some(pid)),
                blocking: false,
            });
            return Ok(());
        }
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

fn mark_unproved(issue: &mut RefPublicationIssue) {
    issue.code = "PRIKK-VERIFY-REF-DIVERGENCE";
    issue.message =
        "pointer/log divergence is not proved by matching retained active state and trust"
            .to_string();
    issue.blocking = true;
}
