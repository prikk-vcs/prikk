//! Joint ref pointer and ref-log verification.

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{EntryKind, list_directory_tolerating_absence};
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReadSnapshot;
use crate::signature_diagnostics::{
    SignatureEnvelopeIssue, SignatureEnvelopeSource, classify_signature_envelope,
};

mod scan;

pub(crate) use scan::ensure_ref_target_valid;
pub use scan::{RefFileOutcome, RefFileStatus};

use scan::{LogState, PointerState, read_logs, read_pointers};

/// One recognized interrupted-publication or local-debris condition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefPublicationIssue {
    /// Stable diagnostic code.
    pub code: &'static str,
    /// Ref name when the condition belongs to one ref.
    pub ref_name: Option<String>,
    /// Human-readable diagnosis without host paths.
    pub message: String,
    /// Whether verification must return non-zero and unrelated mutation must remain blocked.
    pub blocking: bool,
}

/// Outcome of attempting to classify one ref by name (DC-95 Stage 2 Level 2), after its pointer
/// and/or log file (whichever exist) were themselves read. No `NotEvaluated` distinct from
/// `Failed`: unlike Level 1's stages or Phase B's blocks, a ref has no *peer* ref it depends on --
/// its own pointer/log files are its own data, the same footing as an object's own file in
/// `verify_objects` Phase A -- so a failure attributable to this ref, whether from its own file
/// read or from `classify_ref_state` itself, is `Failed`, not a dependency-graph claim about
/// another item.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefItemStatus {
    /// This ref's own classification (or its file reads, if either failed) resolved cleanly.
    Evaluated,
    /// This ref's pointer read, log read, or `classify_ref_state` call itself failed. Carries
    /// whichever failure applies -- a ref whose *own* pointer or log entry did not read is reported
    /// through that entry's real failure message, not reinterpreted as "pointer/log absent" (see
    /// `verify_refs`'s own cross-referencing by `ref_name_key_bytes`, RFC 102 Stage 4).
    Failed {
        /// The error that applies to this ref.
        message: String,
    },
}

/// One ref's resolved outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefItemOutcome {
    /// The ref's human-readable name.
    pub ref_name: String,
    /// How this ref's own verification resolved.
    pub status: RefItemStatus,
}

/// Ref verification counters and publication-state issues.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RefVerification {
    pub pointer_count: usize,
    pub log_record_count: usize,
    pub ref_update_envelopes: Vec<prikk_object::ObjectEnvelope>,
    pub publication_issues: Vec<RefPublicationIssue>,
    pub signature_envelope_issues: Vec<SignatureEnvelopeIssue>,
    /// One outcome per pointer file scanned under `refs/by-id/`, in scan order (DC-95 Stage 2
    /// Level 2).
    pub pointer_outcomes: Vec<RefFileOutcome>,
    /// One outcome per log file scanned under `refs/logs/`, in scan order. A log file that is
    /// legitimately empty with no trailing bytes is not an item at all (nothing to report), same
    /// as the pre-Level-2 behavior of skipping it entirely.
    pub log_outcomes: Vec<RefFileOutcome>,
    /// One outcome per ref name reached via a successfully-read pointer or log (DC-95 Stage 2
    /// Level 2). A ref whose own pointer/log file failed to read is still included here -- see
    /// `RefItemStatus::Failed`'s own doc -- so no ref name known to exist is silently absent.
    pub ref_item_outcomes: Vec<RefItemOutcome>,
}

impl RefVerification {
    /// Return true when any pointer file, log file, or ref-name classification failed (DC-95
    /// Stage 2 Level 2). Item containment means `verify_refs` itself now returns `Ok` for these
    /// cases -- callers that need "is this repository's ref state fully sound," not just "did the
    /// scan run at all," must check this alongside any hard `Err`.
    pub(crate) fn has_item_failure(&self) -> bool {
        self.pointer_outcomes
            .iter()
            .any(|outcome| matches!(outcome.status, RefFileStatus::Failed { .. }))
            || self
                .log_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, RefFileStatus::Failed { .. }))
            || self
                .ref_item_outcomes
                .iter()
                .any(|outcome| matches!(outcome.status, RefItemStatus::Failed { .. }))
    }
}

pub(crate) fn verify_refs(layout: &RepositoryLayout) -> Result<RefVerification> {
    // RFC 111 §6.1: `verify_refs` is read-only (never calls `write_object`), so it takes its own
    // decoded index snapshot here rather than sharing `verify_repository_with_options`'s -- they are
    // two separate top-level constructions today (this one predates this change), and unifying them
    // into one shared snapshot across the whole `verify` run is a further optimization this RFC does
    // not require: RFC 111's own gate measures decode *count*, not construction count, and each of
    // these two snapshots is still exactly one decode regardless of repository size.
    let objects = ObjectReadSnapshot::open(layout)?;
    let (pointers, pointer_failures_by_key, pointer_outcomes) = read_pointers(layout, &objects)?;
    let (logs, log_record_count, ref_log_envelopes, log_failures_by_key, log_outcomes) =
        read_logs(layout, &objects, &pointers)?;
    // DC-95 Stage 2 Level 2 handoff §7 Q4, ruled: stays a whole-set precheck. RFC 103: with format-1
    // retired, this is no longer "a format-2 repository contaminated by format-1 records" -- it is
    // simply malformed data, and the check is unconditional rather than format-gated. Still a claim
    // about the whole repository's history, not a per-ref defect, so still deliberately not
    // contained to the one ref that happens to carry it.
    if logs.values().any(|state| state.has_legacy_timestamp) {
        return Err(PrikkError::Integrity(
            "format-2 RefUpdate requires created_at == 0".to_string(),
        ));
    }
    let mut ref_update_envelopes = Vec::with_capacity(ref_log_envelopes.len());
    let mut signature_envelope_issues = Vec::new();
    for record in ref_log_envelopes {
        signature_envelope_issues.extend(classify_signature_envelope(
            &record.envelope,
            SignatureEnvelopeSource::RefLog {
                ref_name: record.ref_name,
                sequence: record.sequence,
                object_id: record.envelope.object_id(),
            },
        )?);
        ref_update_envelopes.push(record.envelope);
    }
    let mut names = BTreeSet::new();
    names.extend(pointers.keys().cloned());
    names.extend(logs.keys().cloned());
    let mut publication_issues = candidate_issues(layout)?;
    let mut ref_item_outcomes = Vec::with_capacity(names.len());
    for ref_name in names {
        let pointer = pointers.get(&ref_name);
        let log = logs.get(&ref_name);
        // DC-95 Stage 2 Level 2: a ref reached only through its log (or only through its pointer)
        // might have a pointer (or log) that genuinely does not exist -- or might have one that
        // exists but failed to read, which `read_pointers`/`read_logs` recorded as its own
        // `ref_name_key -> message` entry rather than silently omitting it. These are different
        // facts: the first is legitimate business logic `classify_ref_state`'s own match arms
        // already handle; the second must not be reinterpreted as the first. RFC 102 Stage 4:
        // cross-reference by `ref_name_key_bytes(ref_name)`, not by a per-ref file path -- a shared
        // container has no such path, and `RefFileOutcome::path` is a display-only container
        // locator now (see its own doc), not a stable key a failed entry can be found by.
        let ref_name_key = crate::foundation::layout::ref_name_key_bytes(&ref_name);
        let pointer_failure = pointer
            .is_none()
            .then(|| pointer_failures_by_key.get(&ref_name_key).cloned());
        let log_failure = log
            .is_none()
            .then(|| log_failures_by_key.get(&ref_name_key).cloned());
        if let Some(message) = pointer_failure.flatten().or(log_failure.flatten()) {
            ref_item_outcomes.push(RefItemOutcome {
                ref_name,
                status: RefItemStatus::Failed { message },
            });
            continue;
        }
        // This ref's own classification is caught here, at the item boundary, rather than
        // propagated -- every other ref is still attempted.
        match classify_ref_state(&ref_name, pointer, log, &mut publication_issues) {
            Ok(()) => ref_item_outcomes.push(RefItemOutcome {
                ref_name,
                status: RefItemStatus::Evaluated,
            }),
            Err(err) => ref_item_outcomes.push(RefItemOutcome {
                ref_name,
                status: RefItemStatus::Failed {
                    message: err.to_string(),
                },
            }),
        }
    }
    Ok(RefVerification {
        pointer_count: pointers.len(),
        log_record_count,
        ref_update_envelopes,
        publication_issues,
        signature_envelope_issues,
        pointer_outcomes,
        log_outcomes,
        ref_item_outcomes,
    })
}

/// A code this function pushes is not necessarily the code `verify_repository` reports:
/// `POINTER-LEADS-LOG` is piped through `ref_publication::require_retained_evidence` afterward,
/// which overwrites it in place -- code, message, and blocking flag -- to
/// `PRIKK-VERIFY-REF-DIVERGENCE` unless retained active-WAL evidence (matching ref, valid trust, and
/// a target `Block` whose `patch_ids` match the queued WAL records) proves the divergence is a
/// genuinely interrupted publication rather than an unexplained one. RFC 103: the two format-1-only
/// codes this function used to choose between here (`LEGACY-LOG-LEADS`, `POINTER-MISSING`) and their
/// format-2 `DIVERGENCE` counterparts are gone along with format-1 itself -- both conditions now
/// report `DIVERGENCE` unconditionally, which is what they always resolved to under format-2 and
/// what `LEGACY-LOG-LEADS` was already downstream-redundant with (DC-95 Stage 1 round 10).
fn classify_ref_state(
    ref_name: &str,
    pointer: Option<&PointerState>,
    log: Option<&LogState>,
    issues: &mut Vec<RefPublicationIssue>,
) -> Result<()> {
    match (pointer, log) {
        (Some(pointer), Some(log)) if Some(pointer.id) == log.tip => {
            if log.trailing_partial_bytes != 0 {
                return Err(PrikkError::Integrity(format!(
                    "ref {ref_name} has an incomplete log tail without a pointer lead"
                )));
            }
            Ok(())
        }
        (Some(pointer), log)
            if pointer.payload.previous_ref_state_id == log.and_then(|state| state.tip)
                && pointer.payload.update_seq == next_log_sequence(log)? =>
        {
            let partial = log.map_or(0, |state| state.trailing_partial_bytes);
            issues.push(blocking_issue(
                "PRIKK-VERIFY-REF-POINTER-LEADS-LOG",
                ref_name,
                if partial == 0 {
                    "authoritative pointer leads committed ref log by one transition".to_string()
                } else {
                    format!(
                        "authoritative pointer leads ref log by one transition with {partial} incomplete trailing byte(s)"
                    )
                },
            ));
            Ok(())
        }
        (Some(pointer), Some(log)) if log.previous_tip == Some(pointer.id) => {
            issues.push(blocking_issue(
                "PRIKK-VERIFY-REF-DIVERGENCE",
                ref_name,
                "format-2 ref log leads the authoritative pointer".to_string(),
            ));
            Ok(())
        }
        (None, Some(log)) if log.record_count == 1 && log.previous_tip.is_none() => {
            issues.push(blocking_issue(
                "PRIKK-VERIFY-REF-DIVERGENCE",
                ref_name,
                "format-2 ref pointer is missing while committed log history exists".to_string(),
            ));
            Ok(())
        }
        (None, None) => Ok(()),
        _ => Err(PrikkError::Integrity(format!(
            "unexplained pointer/log divergence for ref {ref_name}"
        ))),
    }
}

fn next_log_sequence(log: Option<&LogState>) -> Result<u64> {
    u64::try_from(log.map_or(0, |state| state.record_count))
        .ok()
        .and_then(|value| value.checked_add(1))
        .ok_or_else(|| PrikkError::Integrity("ref-log sequence overflow".to_string()))
}

/// **Tolerates a missing `refs/tmp` directory rather than erroring** (recovery-listing-tolerance
/// follow-up): a bare `list_directory` call here used to fail this whole scan -- and, through
/// `verify_refs`'s `Err` being caught into a `Failed` stage outcome, halt every downstream
/// verification stage under `--stop-on-first-error` -- over one missing debris-scan directory. The
/// absence itself is reported by `doctor`'s own sweep over `RepositoryLayout::required_directories`,
/// not here, so this stays a pure scan rather than growing a second way to say "something is
/// missing."
fn candidate_issues(layout: &RepositoryLayout) -> Result<Vec<RefPublicationIssue>> {
    let mut issues = Vec::new();
    let dir = layout.refs_dir().join("tmp");
    let relative = layout.repository_relative(&dir)?;
    let Some(entries) = list_directory_tolerating_absence(
        layout.repository_mutation_root(),
        &dir,
        &relative,
        "the ref candidate directory",
    )?
    else {
        return Ok(issues);
    };
    for entry in entries {
        if entry.kind == EntryKind::Regular {
            issues.push(RefPublicationIssue {
                code: "PRIKK-VERIFY-REF-CANDIDATE-DEBRIS",
                ref_name: None,
                message: "non-authoritative ref pointer candidate remains".to_string(),
                blocking: false,
            });
        } else {
            return Err(PrikkError::Integrity(
                "unexpected non-file in ref candidate directory".to_string(),
            ));
        }
    }
    Ok(issues)
}

fn blocking_issue(code: &'static str, ref_name: &str, message: String) -> RefPublicationIssue {
    RefPublicationIssue {
        code,
        ref_name: Some(ref_name.to_string()),
        message,
        blocking: true,
    }
}
