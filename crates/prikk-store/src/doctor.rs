//! Repository doctor diagnostics and narrowly-scoped repair helpers.
//!
//! Doctor repairs are deliberately conservative. Mutating repair is opt-in and limited to
//! incomplete active-WAL tail truncation. `--repair-main-ref` remains a recognized input but has no
//! implemented repair and is always refused.

use prikk_error::{PrikkError, Result};

use crate::block_state::BlockStateStatus;
use crate::commit_boundary::classification::{Verdict, classify};
use crate::commit_boundary::witness::read_witness;
use crate::foundation::fsutil::{EntryKind, inspect_entry};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryLayout};
use crate::lock::{ActiveLock, acquire_container_locks};
use crate::refs::{RefFileStatus, RefItemStatus};
use crate::verify::{
    ActiveWalMetadataStatus, ObjectItemStatus, RepositoryVerification, StageOutcome, StageStatus,
    VerificationStage, active_ref_metadata_from_wal_metadata_status, classify_active_wal_metadata,
    verify_repository,
};
use crate::wal::{Wal, WalRecordStatus, WalRepair};

mod discard_damaged_commits;
mod repair_tails;
mod restore_queue_target;
pub use discard_damaged_commits::{
    DiscardDamagedCommitsPlan, discard_damaged_commits, plan_discard_damaged_commits,
};
pub use repair_tails::{RepairTailsFileOutcome, RepairTailsReport, repair_tails};
pub use restore_queue_target::{
    RestoreQueueTargetPlan, plan_restore_queue_target, restore_queue_target,
};

/// Severity assigned to a doctor diagnostic issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoctorSeverity {
    /// Informational diagnostic that does not require user action.
    Info,
    /// Warning diagnostic that may require attention but does not prove corruption.
    Warning,
    /// Error diagnostic that blocks repository health.
    Error,
}

impl DoctorSeverity {
    /// Return a stable lower-case label for CLI output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Error => "error",
        }
    }
}

/// One doctor diagnostic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorIssue {
    /// Stable diagnostic code.
    pub code: &'static str,
    /// Diagnostic severity.
    pub severity: DoctorSeverity,
    /// Human-readable explanation.
    pub message: String,
    /// Suggested next action.
    pub recommendation: String,
    /// Which active session this issue is about, if it is about exactly one (RFC 108 increment
    /// 3d). `None` means repository-wide -- the issue is not specific to any one active session's
    /// WAL or metadata, and blocks `repair_repository` for every active, exactly as every issue did
    /// before this field existed. **This default is what makes the field safe to add**: every call
    /// site that does not opt in via `for_active_session` keeps today's meaning unchanged. Only this
    /// arc's own per-active issues (`ACTIVE-SESSION-*`, `default`'s own `ACTIVE-REF-METADATA-*`, and
    /// the handful of stage/record outcomes that are inherently about one active's WAL) opt in.
    pub active_session: Option<std::ffi::OsString>,
}

impl DoctorIssue {
    /// Construct an informational diagnostic. Repository-wide (`active_session: None`) unless
    /// `for_active_session` is chained onto the result.
    #[must_use]
    pub fn info(
        code: &'static str,
        message: impl Into<String>,
        recommendation: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: DoctorSeverity::Info,
            message: message.into(),
            recommendation: recommendation.into(),
            active_session: None,
        }
    }

    /// Construct a warning diagnostic. Repository-wide (`active_session: None`) unless
    /// `for_active_session` is chained onto the result.
    #[must_use]
    pub fn warning(
        code: &'static str,
        message: impl Into<String>,
        recommendation: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: DoctorSeverity::Warning,
            message: message.into(),
            recommendation: recommendation.into(),
            active_session: None,
        }
    }

    /// Construct an error diagnostic. Repository-wide (`active_session: None`) unless
    /// `for_active_session` is chained onto the result.
    #[must_use]
    pub fn error(
        code: &'static str,
        message: impl Into<String>,
        recommendation: impl Into<String>,
    ) -> Self {
        Self {
            code,
            severity: DoctorSeverity::Error,
            message: message.into(),
            recommendation: recommendation.into(),
            active_session: None,
        }
    }

    /// Attribute this issue to one active session's own WAL/metadata rather than the whole
    /// repository (RFC 108 increment 3d, §3.1's adjudication: a builder step chained onto the
    /// existing `::error`/`::warning`/`::info` constructors, not a fourth parameter on each of
    /// them or a parallel set of constructors -- every one of this crate's existing call sites
    /// (repository-wide) needed zero edits, and only the handful of genuinely per-active call
    /// sites gained one chained call each).
    #[must_use]
    pub fn for_active_session(mut self, name: impl Into<std::ffi::OsString>) -> Self {
        self.active_session = Some(name.into());
        self
    }
}

/// Doctor report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorReport {
    /// Repository verification summary, when verification completed.
    pub verification: Option<RepositoryVerification>,
    /// Diagnostics produced by doctor.
    pub issues: Vec<DoctorIssue>,
}

impl DoctorReport {
    /// Return true if no error-severity issue was found.
    #[must_use]
    pub fn is_healthy(&self) -> bool {
        !self
            .issues
            .iter()
            .any(|issue| issue.severity == DoctorSeverity::Error)
    }

    /// Count issues with a given severity.
    #[must_use]
    pub fn count_by_severity(&self, severity: DoctorSeverity) -> usize {
        self.issues
            .iter()
            .filter(|issue| issue.severity == severity)
            .count()
    }
}

/// Opt-in repair switches for doctor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DoctorRepairOptions {
    /// Truncate incomplete trailing bytes from the active WAL after verification confirms that the
    /// prefix is valid.
    pub truncate_wal_tail: bool,
    /// Request `heads/main` ref reconstruction. Always refused -- no repair is implemented.
    pub reconstruct_main_ref: bool,
}

impl DoctorRepairOptions {
    /// Return options that perform no repair.
    #[must_use]
    pub const fn none() -> Self {
        Self {
            truncate_wal_tail: false,
            reconstruct_main_ref: false,
        }
    }

    /// Return options that enable only safe active-WAL tail truncation.
    #[must_use]
    pub const fn truncate_wal_tail() -> Self {
        Self {
            truncate_wal_tail: true,
            reconstruct_main_ref: false,
        }
    }

    /// Return options that request `heads/main` ref reconstruction. Always refused -- no repair is
    /// implemented.
    #[must_use]
    pub const fn reconstruct_main_ref() -> Self {
        Self {
            truncate_wal_tail: false,
            reconstruct_main_ref: true,
        }
    }
}

/// Outcome of attempting to repair one active session, as part of `repair_repository`'s per-active
/// pass (RFC 108 increment 3d).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveSessionRepairStatus {
    /// The repair ran against this active session (or no repair was requested; `WalRepair`'s own
    /// three fields are all zero/empty in that case, same shape `repair_repository` always used
    /// when `truncate_wal_tail` was `false`).
    Repaired(WalRepair),
    /// Not attempted -- either this active session had its own blocking `DoctorIssue`
    /// (`before.issues` named it specifically, not repository-wide), or its active-session lock
    /// could not be acquired (someone else is writing to it right now). **Not a failure of the
    /// run** (RFC 108 §D3.3/§2.1): every other eligible active session is still attempted.
    Skipped {
        /// Why this active session was not repaired.
        reason: String,
    },
}

/// One active session's own repair outcome, named (RFC 108 increment 3d) -- `repair_repository`'s
/// per-active pass produces one of these per entry in `RepositoryLayout::active_session_names`, in
/// that same sorted order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSessionRepairOutcome {
    /// Which active session this outcome is about.
    pub active_session: std::ffi::OsString,
    /// What happened when repair reached it.
    pub status: ActiveSessionRepairStatus,
}

/// Report returned by an opt-in doctor repair run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DoctorRepairReport {
    /// Doctor report before any repair action.
    pub before: DoctorReport,
    /// `default`'s own WAL repair summary -- unchanged in type and meaning from before RFC 108
    /// increment 3d existed: every field is zero/empty when no repair ran (whether because none was
    /// requested, `default` had its own blocking issue, or `default`'s lock could not be acquired).
    /// **A zero here is not proof `default` was actually repaired** -- the same "zero is a claim"
    /// caution this project applies elsewhere (`prikk-cli`'s own `format_count`): check
    /// `active_repairs`' own entry for `default` for the real answer, including *why* if it was
    /// skipped. Kept, unchanged, so every existing caller of this one field needs no edit.
    pub wal_repair: WalRepair,
    /// One outcome per active session on disk, in `RepositoryLayout::active_session_names`'s sorted
    /// order (RFC 108 increment 3d) -- including `default`, whose entry `wal_repair` above
    /// duplicates for callers that have not moved to this richer field yet.
    pub active_repairs: Vec<ActiveSessionRepairOutcome>,
    /// Doctor report after repair action.
    pub after: DoctorReport,
}

/// Report every required directory that is missing or occupied by something other than a
/// directory, sourced from `RepositoryLayout::required_directories` -- the same inventory `init`
/// itself creates from, never a second hand-typed list of paths.
///
/// **This is the "some surface must say a required directory is missing" half of the
/// recovery-listing-tolerance follow-up.** `unlock`'s per-ref lock listing and `verify`'s
/// ref-candidate-debris scan both now tolerate their own required directory being absent rather
/// than erroring -- correct for keeping those two commands usable on a damaged repository, but on
/// its own it would convert a loud failure into a silent one (a repository missing `refs/locks`
/// would report "no locks held", confidently and wrongly, with nothing anywhere saying the
/// directory is gone). `doctor` is where an operator actually looks to ask "what is wrong with this
/// repository," so this check runs here, independently of whatever any individual listing chose to
/// tolerate -- it does not depend on `unlock` or `verify` having been run at all.
fn push_missing_required_directory_issues(
    layout: &RepositoryLayout,
    issues: &mut Vec<DoctorIssue>,
) {
    for dir in layout.required_directories() {
        let relative = match layout.repository_relative(&dir) {
            Ok(relative) => relative,
            Err(err) => {
                issues.push(DoctorIssue::error(
                    "PRIKK-DOCTOR-REQUIRED-DIRECTORY-UNREADABLE",
                    format!(
                        "could not resolve required directory {}: {err}",
                        dir.display()
                    ),
                    "preserve the repository and inspect its layout before attempting repair",
                ));
                continue;
            }
        };
        match inspect_entry(layout.repository_mutation_root(), &relative) {
            Ok(None) => issues.push(DoctorIssue::error(
                "PRIKK-DOCTOR-MISSING-REQUIRED-DIRECTORY",
                format!("required directory is missing: {}", dir.display()),
                "recreate the directory matching a fresh `init`'s own layout, or restore the \
                 repository from backup; doctor does not create required directories automatically",
            )),
            Ok(Some(EntryKind::Directory)) => {}
            Ok(Some(_)) => issues.push(DoctorIssue::error(
                "PRIKK-DOCTOR-REQUIRED-DIRECTORY-WRONG-TYPE",
                format!(
                    "required directory location is occupied by something other than a directory: {}",
                    dir.display()
                ),
                "preserve the repository for inspection; doctor does not replace unexpected \
                 filesystem entries",
            )),
            Err(err) => issues.push(DoctorIssue::error(
                "PRIKK-DOCTOR-REQUIRED-DIRECTORY-UNREADABLE",
                format!("could not check required directory {}: {err}", dir.display()),
                "preserve the repository and inspect its layout before attempting repair",
            )),
        }
    }
}

/// Report each **non-default** active session's WAL replay state (a trailing partial tail, or a WAL
/// that fails to replay at all) *and* ref-name metadata state, reaching parity with what
/// `verification.trailing_partial_wal_bytes`/`verification.active_wal_metadata_status` give
/// `default`. `default`'s own reporting stays exactly where it already was -- sourced from those two
/// `verification` fields and the `WalReplay` stage outcome below, all untouched by this function --
/// so this is new coverage, not a second reader of a fact doctor already had for `default`.
///
/// RFC 108 §D3.3/§D3.4, increment 3b: with a second active session on disk, doctor was previously
/// silent about its WAL entirely -- the same silent-hole shape increment 2 closed for `unlock`'s
/// per-ref-lock listing and `verify`'s ref-candidate-debris scan. `default`'s WAL replay failure is
/// already caught by `verification`'s own `WalReplay` stage outcome (relayed into a
/// `PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE` error below); a non-default active has no stage watching
/// it, so this function is the only thing that will report one failing to replay at all.
///
/// **Adjudication (increment 3b handoff §2.2, unchanged by 3c): doctor does not re-read `default`'s
/// WAL or metadata here.** The alternative -- one uniform loop over every active including
/// `default`, ignoring what `verification` already computed -- was rejected: `default`'s path is
/// already correct and already tested (existing assertions pin its exact behaviour), so re-deriving
/// it a second way here would be duplication with no coverage gained, the "two readers of one fact"
/// shape this project keeps punishing. Skipping `default` costs a small asymmetry (its issues and a
/// non-default one's use different `DoctorIssue` codes), not a coverage gap -- WAL replay state *and*
/// ref-name metadata state are now covered for every active session, by exactly one path each.
///
/// **Increment 3c closed the gap increment 3b recorded here**: this function now calls
/// `classify_active_wal_metadata` -- the exact same classification `verification` runs for `default`,
/// widened (increment 3c) to take a name -- rather than re-deriving the six-arm match a second time.
/// Parity is by construction, not by separately matching the same intent: both arms increment 3b's
/// review asked about (`MissingForNonEmptyWal` and `InvalidForNonEmptyWal`) are covered, because both
/// come from the one shared function `default`'s own path already calls.
fn push_non_default_active_session_wal_issues(
    layout: &RepositoryLayout,
    issues: &mut Vec<DoctorIssue>,
) {
    let Ok(names) = layout.active_session_names() else {
        // Absence or a wrong-type occupant at `active/` itself is already reported by
        // `push_missing_required_directory_issues` above -- nothing further to add here.
        return;
    };
    for name in names {
        if name.to_str() == Some(DEFAULT_ACTIVE_NAME) {
            continue;
        }
        match Wal::for_layout(layout, &name).replay() {
            Ok(replay) => {
                if replay.trailing_partial_bytes != 0 {
                    issues.push(
                        DoctorIssue::warning(
                            "PRIKK-DOCTOR-ACTIVE-SESSION-WAL-TRAILING-PARTIAL",
                            format!(
                                "active session {name:?} has {} trailing byte(s) that look like \
                                 an incomplete final record",
                                replay.trailing_partial_bytes
                            ),
                            "run `prikk doctor --repair-wal-tail` to truncate only this active \
                             session's incomplete final WAL bytes",
                        )
                        .for_active_session(name.clone()),
                    );
                }
                match classify_active_wal_metadata(layout, &name, replay.records.is_empty()) {
                    Ok(status) => {
                        push_active_session_ref_metadata_issue(&name, &status, issues);
                        let owning_ref = active_ref_metadata_from_wal_metadata_status(&status);
                        match read_witness(layout, &name)
                            .and_then(|witness| classify(layout, &replay, &owning_ref, &witness))
                        {
                            Ok(verdict) => {
                                push_active_session_commit_witness_issue(
                                    &name, &verdict, &status, issues,
                                );
                            }
                            Err(error) => {
                                issues.push(
                                    DoctorIssue::error(
                                        "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-UNREADABLE",
                                        format!(
                                            "active session {name:?}'s own acknowledgment history \
                                             failed to read: {error}"
                                        ),
                                        "preserve the repository and inspect the active session's \
                                         own acknowledgment history before attempting repair",
                                    )
                                    .for_active_session(name.clone()),
                                );
                            }
                        }
                    }
                    Err(error) => {
                        issues.push(
                            DoctorIssue::error(
                                "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-UNREADABLE",
                                format!(
                                    "active session {name:?}'s ref metadata failed to read: {error}"
                                ),
                                "preserve the repository and inspect the active session's ref \
                                 metadata before attempting repair",
                            )
                            .for_active_session(name.clone()),
                        );
                    }
                }
            }
            Err(error) => {
                issues.push(
                    DoctorIssue::error(
                        "PRIKK-DOCTOR-ACTIVE-SESSION-WAL-UNREADABLE",
                        format!("active session {name:?}'s WAL failed to read: {error}"),
                        "preserve the repository and inspect the active session's WAL before \
                         attempting repair",
                    )
                    .for_active_session(name.clone()),
                );
            }
        }
    }
}

/// The non-default counterpart of `add_active_wal_metadata_issues` below -- same six-arm intent
/// (an error for WAL records with missing/malformed metadata, a warning for stale/malformed debris
/// on an empty WAL, silence for the two healthy arms), reusing the exact `ActiveWalMetadataStatus`
/// `classify_active_wal_metadata` already produces rather than re-deriving it. Distinct `DoctorIssue`
/// codes from `default`'s (`ACTIVE-SESSION-REF-METADATA-*` vs. `ACTIVE-REF-METADATA-*`), matching
/// increment 3b's own `ACTIVE-SESSION-WAL-*` convention -- and the message must name the active
/// session, since `default`'s equivalent message never needed to distinguish one active from another.
fn push_active_session_ref_metadata_issue(
    name: &std::ffi::OsStr,
    status: &ActiveWalMetadataStatus,
    issues: &mut Vec<DoctorIssue>,
) {
    match status {
        ActiveWalMetadataStatus::MissingForNonEmptyWal => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MISSING",
                format!("active session {name:?} has WAL records but its ref metadata is missing"),
                "preserve the repository and inspect the active session's WAL before attempting \
                 repair",
            )
            .for_active_session(name.to_os_string()),
        ),
        ActiveWalMetadataStatus::InvalidForNonEmptyWal { reason } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MALFORMED",
                format!(
                    "active session {name:?} has WAL records but its ref metadata is malformed: \
                     {reason}"
                ),
                "preserve the repository and inspect the active session's WAL before attempting \
                 repair",
            )
            .for_active_session(name.to_os_string()),
        ),
        ActiveWalMetadataStatus::ValidForEmptyWal { ref_name } => issues.push(
            DoctorIssue::warning(
                "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-DEBRIS",
                format!(
                    "active session {name:?}'s WAL is empty but stale ref metadata remains for \
                     {ref_name}"
                ),
                "no repair is required; the next guarded append will replace stale metadata",
            )
            .for_active_session(name.to_os_string()),
        ),
        ActiveWalMetadataStatus::InvalidForEmptyWal { reason } => issues.push(
            DoctorIssue::warning(
                "PRIKK-DOCTOR-ACTIVE-SESSION-REF-METADATA-MALFORMED-DEBRIS",
                format!(
                    "active session {name:?}'s WAL is empty but malformed ref metadata remains: \
                     {reason}"
                ),
                "no repair is required; the next guarded append will replace stale metadata",
            )
            .for_active_session(name.to_os_string()),
        ),
        ActiveWalMetadataStatus::MissingForEmptyWal
        | ActiveWalMetadataStatus::ValidForNonEmptyWal { .. } => {}
    }
}

/// The non-default counterpart of `add_commit_witness_issues` below -- same nine-arm intent, same
/// `DoctorIssue` severities, reusing the exact `Verdict` `classify` already produced for this active
/// session rather than re-deriving it. Distinct codes from `default`'s
/// (`ACTIVE-SESSION-COMMIT-WITNESS-*` vs. `COMMIT-WITNESS-*`), matching `push_active_session_ref_
/// metadata_issue`'s own convention -- and the message must name the active session.
fn push_active_session_commit_witness_issue(
    name: &std::ffi::OsStr,
    verdict: &Verdict,
    ref_metadata_status: &ActiveWalMetadataStatus,
    issues: &mut Vec<DoctorIssue>,
) {
    match verdict {
        // RFC 166 §13 item 16: same reasoning as `add_commit_witness_issues`'s own arm -- silent in
        // `status`, a note here only.
        Verdict::NoWitness { stale: true, .. } => issues.push(
            DoctorIssue::info(
                "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-STALE",
                format!(
                    "active session {name:?}'s own acknowledgment history is stale (an older \
                     binary's own seal drained the queue it covered)"
                ),
                "no action is required; the next commit or `prikk doctor --repair-tails` replaces \
                 it",
            )
            .for_active_session(name.to_os_string()),
        ),
        Verdict::NoWitness { stale: false, .. }
        | Verdict::Healthy { .. }
        | Verdict::Pending { .. }
        | Verdict::CrashTail { .. } => {}
        Verdict::AcknowledgedDamage { witnessed_seq } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-ACKNOWLEDGED-DAMAGE",
                format!(
                    "active session {name:?} has a queued commit you were told had succeeded \
                     (sequence {witnessed_seq}) that is now damaged"
                ),
                "preserve the repository; it was already acknowledged, and it cannot be removed \
                 as a crash leftover",
            )
            .for_active_session(name.to_os_string()),
        ),
        Verdict::AcknowledgedLoss { witnessed_seq } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-ACKNOWLEDGED-LOSS",
                format!(
                    "active session {name:?} has a queued commit you were told had succeeded \
                     (sequence {witnessed_seq}) that is no longer present at all"
                ),
                "preserve the repository; it was already acknowledged, and it cannot be removed \
                 as a crash leftover",
            )
            .for_active_session(name.to_os_string()),
        ),
        Verdict::SubstitutedRecord { witnessed_seq } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-SUBSTITUTED-RECORD",
                format!(
                    "sequence {witnessed_seq} in active session {name:?}'s WAL does not match the \
                     queued commit it was told had succeeded"
                ),
                "preserve the repository; this is not a crash shape, and a copy is the way out",
            )
            .for_active_session(name.to_os_string()),
        ),
        Verdict::UnknownWithDamagedWitness => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-UNKNOWN",
                format!(
                    "active session {name:?}'s WAL has an unexplained tail, and its own \
                     acknowledgment history is unreadable, so the tail cannot be shown to be a \
                     crash leftover"
                ),
                "preserve the repository; it cannot be removed as a crash leftover without being \
                 able to confirm whether it was acknowledged",
            )
            .for_active_session(name.to_os_string()),
        ),
        Verdict::WitnessDamaged => issues.push(
            DoctorIssue::warning(
                "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-DAMAGED",
                format!(
                    "active session {name:?}'s own acknowledgment history is unreadable, but its \
                     active WAL is wholly sound"
                ),
                "run `prikk doctor --repair-tails` to rebuild it",
            )
            .for_active_session(name.to_os_string()),
        ),
        // Same reasoning as `add_commit_witness_issues`'s own `OwnershipMissing` arm: a plain missing
        // or malformed owner is already reported, precisely, by `push_active_session_ref_metadata_
        // issue` above (via `ref_metadata_status`); this fires only for D6's genuine new case, where
        // ownership is validly present and still disagrees with the witness.
        Verdict::OwnershipMissing => {
            if matches!(
                ref_metadata_status,
                ActiveWalMetadataStatus::ValidForNonEmptyWal { .. }
            ) {
                issues.push(
                    DoctorIssue::error(
                        "PRIKK-DOCTOR-ACTIVE-SESSION-COMMIT-WITNESS-OWNERSHIP-MISSING",
                        format!(
                            "active session {name:?}'s ref metadata names an owner its own \
                             acknowledgment history does not recognize as its own"
                        ),
                        "preserve the repository and inspect the active session's WAL before \
                         attempting repair",
                    )
                    .for_active_session(name.to_os_string()),
                );
            }
        }
    }
}

/// Return `Some(DEFAULT_ACTIVE_NAME)` when `outcome` is about a stage that reads `default`'s own
/// active WAL and nothing else, `None` when it does not (RFC 108 increment 3d, §2.2).
///
/// **Not a new classification -- a fixed fact about `verify_repository`'s own pipeline, already
/// documented at each stage's own call site in `verify.rs`.** `verify_repository_with_options`
/// hardcodes exactly one `Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)` call; `WalReplay` reads it
/// directly, and `WalPersistence`/`RollbackDrafts`/`WalRecordSchema`/`ActiveWalMetadata`/`WalOrdering`
/// each have `default`'s own `replay` as their **only** real dependency (`verify.rs`'s own
/// `not_evaluated(_, VerificationStage::WalReplay)` calls for all five). `CommitWitness` (RFC 166 D3)
/// also reads only `default`'s own replay and ref-name metadata -- its `NotEvaluated` names whichever
/// of `WalReplay`/`ActiveWalMetadata` blocked it first, never a stage outside this group, so it stays
/// `default`-scoped the same as the other six regardless of which of the two it names. A failure or
/// non-evaluation in any of these seven can only ever mean `default`'s own WAL, ref-name metadata, or
/// commit witness is the problem -- `second`'s state cannot cause it, since nothing here ever reads
/// `second`.
///
/// `PublicationReclassification` is the one genuinely mixed stage: `verify.rs`'s own comment above
/// its call site says its `NotEvaluated` names whichever of `WalReplay`/`Refs`/`ActiveWalMetadata`
/// blocked it first, and `Refs` is repository-wide sealed-history verification, not WAL-scoped.
/// `blocked_by` already carries that answer -- read it, rather than inventing a second way to derive
/// it: attribute only when it names one of the two WAL-scoped upstream stages. A `Failed` outcome
/// here (every real dependency evaluated, but the cross-check between WAL records and ref evidence
/// itself failed) stays repository-wide, deliberately: the failure could stem from either half, and
/// treating a genuinely ambiguous cause as repository-wide is the conservative reading `repair`
/// should default to, not a gap.
///
/// Every other stage (`Objects`, `Refs`, `RefUpdateSchemaTrust`, `CommitIndex`, `LifecycleCache`,
/// `ReceivedRefs`, `LocalTagTrust`) is independent of the active WAL entirely -- `verify.rs`'s own
/// comments for `CommitIndex`/`LifecycleCache` say so explicitly ("No upstream stage dependency").
/// **No wildcard arm**: an eighteenth `VerificationStage` variant must not silently fall through
/// either side of this match -- it fails to compile until a real decision is recorded here, the same
/// defense `verification_stages!`'s own macro already gives `ALL`/`label()`.
fn active_session_owning_stage_outcome(outcome: &StageOutcome) -> Option<&'static str> {
    match outcome.stage {
        VerificationStage::WalReplay
        | VerificationStage::WalPersistence
        | VerificationStage::RollbackDrafts
        | VerificationStage::WalRecordSchema
        | VerificationStage::ActiveWalMetadata
        | VerificationStage::CommitWitness
        | VerificationStage::WalOrdering => Some(DEFAULT_ACTIVE_NAME),
        VerificationStage::PublicationReclassification => match &outcome.status {
            StageStatus::NotEvaluated {
                blocked_by: VerificationStage::WalReplay | VerificationStage::ActiveWalMetadata,
            } => Some(DEFAULT_ACTIVE_NAME),
            _ => None,
        },
        VerificationStage::Objects
        | VerificationStage::Refs
        | VerificationStage::RefUpdateSchemaTrust
        | VerificationStage::CommitIndex
        | VerificationStage::LifecycleCache
        | VerificationStage::ReceivedRefs
        | VerificationStage::LocalTagTrust => None,
        // RFC 162: unlike the six WAL-scoped stages above, this one reads *every* active session's
        // own WAL (not only `default`'s), so a failure cannot be attributed to one session alone --
        // repository-wide, the same conservative reading `PublicationReclassification`'s ambiguous
        // case already uses.
        VerificationStage::ObjectConnectivity => None,
        // RFC 164 Rule B: reads seven standalone, repository-wide files, none of them scoped to any
        // active session's own WAL.
        VerificationStage::AppendedFileTails => None,
    }
}

/// RFC 151 §2.1: a current-branch pointer no default can resolve -- malformed, or naming a branch
/// that does not exist or is closed. A **warning**, not an error: the repository is intact and
/// every command still works with `--ref` given explicitly; only the default is unusable.
fn push_current_branch_issue(layout: &RepositoryLayout, issues: &mut Vec<DoctorIssue>) {
    if let Err(err) = crate::refs::current_branch(layout) {
        issues.push(DoctorIssue::warning(
            "PRIKK-DOCTOR-CURRENT-BRANCH",
            err.to_string(),
            "run `prikk branch switch heads/<name>` to a branch that exists and is open, or \
             `prikk branch create` the branch the pointer names; `--ref` given explicitly still \
             works meanwhile",
        ));
    }
}

/// RFC 136 §10.3b.2: `doctor` knows the provisional-worktree marker. A warning, not an error: the
/// repository is not damaged, and `prikk verify` is the route that clears it.
fn push_provisional_worktree_issue(layout: &RepositoryLayout, issues: &mut Vec<DoctorIssue>) {
    match crate::worktree_marker::provisional_worktree(layout) {
        Ok(None) => {}
        Ok(Some(provisional)) => issues.push(DoctorIssue::warning(
            "PRIKK-DOCTOR-PROVISIONAL-WORKTREE",
            format!(
                "the worktree was materialized from the snapshot of Block {} on {} and is not \
                 replay-verified",
                provisional.block_id, provisional.ref_name
            ),
            "run `prikk verify`; until it passes, commit, mv, seal, merge, sync accept, sync seal, \
             rollback-draft and branch switch refuse",
        )),
        Err(err) => issues.push(DoctorIssue::warning(
            "PRIKK-DOCTOR-PROVISIONAL-WORKTREE",
            format!("the provisional-worktree marker could not be read: {err}"),
            "run `prikk verify`",
        )),
    }
}

/// Checkout-refusal round §2.4: `doctor` reports a checkout or branch switch that stopped part-way (the
/// RFC 102 dirty marker), naming the two routes that clear it. A warning: nothing is damaged.
fn push_interrupted_materialization_issue(
    layout: &RepositoryLayout,
    issues: &mut Vec<DoctorIssue>,
) {
    match crate::worktree_marker::worktree_materialization_interrupted(layout) {
        Ok(false) => {}
        Ok(true) => {
            let branch = crate::refs::current_branch(layout)
                .unwrap_or_else(|_| "<the current branch>".to_string());
            issues.push(DoctorIssue::warning(
                "PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION",
                "a checkout or branch switch stopped part-way, so the worktree is not verified \
                 against its baseline and `commit` refuses",
                format!(
                    "move aside any file the stopped checkout named, then run `prikk checkout \
                     --patch-materialize --ref {branch}` or `prikk branch switch {branch}`; either \
                     clears it"
                ),
            ));
        }
        Err(err) => issues.push(DoctorIssue::warning(
            "PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION",
            format!("the worktree dirty marker could not be read: {err}"),
            "run `prikk checkout --patch-materialize --ref <the current branch>`",
        )),
    }
}

/// Run doctor diagnostics for a repository layout.
#[must_use]
pub fn doctor_repository(layout: &RepositoryLayout) -> DoctorReport {
    let mut issues = Vec::new();
    push_missing_required_directory_issues(layout, &mut issues);
    push_non_default_active_session_wal_issues(layout, &mut issues);
    push_current_branch_issue(layout, &mut issues);
    push_provisional_worktree_issue(layout, &mut issues);
    push_interrupted_materialization_issue(layout, &mut issues);
    match verify_repository(layout) {
        Ok(verification) => {
            issues.push(DoctorIssue::info(
                "PRIKK-DOCTOR-VERIFY-OK",
                "repository structural verification scan completed",
                "review the remaining diagnostics before deciding whether action is required",
            ));
            // DC-95 Stage 2 Level 1: a stage that failed or could not evaluate is blocking by
            // construction (severity derives from the stage outcome itself, not a per-field decision
            // here) -- this is what preserves `repair_repository`'s refusal gate now that
            // `verify_repository` no longer aborts on the first hard error.
            for outcome in &verification.stage_outcomes {
                let message = match &outcome.status {
                    StageStatus::Evaluated => continue,
                    StageStatus::Failed { message } => {
                        format!("verification stage {} failed: {message}", outcome.stage)
                    }
                    StageStatus::NotEvaluated { blocked_by } => {
                        format!(
                            "verification stage {} could not run because stage {blocked_by} did not evaluate",
                            outcome.stage
                        )
                    }
                    StageStatus::Halted { after } => {
                        format!(
                            "verification stage {} was not attempted because stage {after} failed and halted the walk (--stop-on-first-error)",
                            outcome.stage
                        )
                    }
                };
                let issue = DoctorIssue::error(
                    "PRIKK-DOCTOR-VERIFY-STAGE-INCOMPLETE",
                    message,
                    "preserve the repository and inspect the failing stage before attempting repair",
                );
                issues.push(match active_session_owning_stage_outcome(outcome) {
                    Some(name) => issue.for_active_session(name),
                    None => issue,
                });
            }
            // DC-95 Stage 2 Level 2: item containment means the `Objects` stage above can be
            // `Evaluated` even when one of its items individually failed -- these two loops are what
            // preserve `repair_repository`'s refusal gate at item granularity, the same way the loop
            // above preserves it at stage granularity.
            for outcome in &verification.object_outcomes {
                if let ObjectItemStatus::Failed { message } = &outcome.status {
                    issues.push(DoctorIssue::error(
                        "PRIKK-DOCTOR-VERIFY-OBJECT-INCOMPLETE",
                        format!(
                            "object {} ({}) failed verification: {message}",
                            outcome.path.display(),
                            outcome.object_type
                        ),
                        "preserve the repository and inspect the failing object before attempting repair",
                    ));
                }
            }
            for outcome in &verification.block_state_outcomes {
                let message = match &outcome.status {
                    BlockStateStatus::Verified => continue,
                    BlockStateStatus::Failed { message } => {
                        format!(
                            "Block {} state-root verification failed: {message}",
                            outcome.block_id
                        )
                    }
                    BlockStateStatus::NotEvaluated { blocked_by } => {
                        format!(
                            "Block {} state root could not be verified because its state-derivation \
                             parent {blocked_by} did not evaluate",
                            outcome.block_id
                        )
                    }
                };
                issues.push(DoctorIssue::error(
                    "PRIKK-DOCTOR-VERIFY-BLOCK-STATE-INCOMPLETE",
                    message,
                    "preserve the repository and inspect the failing block before attempting repair",
                ));
            }
            // DC-95 Stage 2 Level 2 (refs half): same reasoning as the two loops above, one level
            // in for `verify_refs`'s own items -- a single ref's pointer file, log file, or
            // classification failing no longer fails the whole `Refs` stage.
            for outcome in verification
                .pointer_outcomes
                .iter()
                .chain(&verification.log_outcomes)
            {
                if let RefFileStatus::Failed { message } = &outcome.status {
                    issues.push(DoctorIssue::error(
                        "PRIKK-DOCTOR-VERIFY-REF-FILE-INCOMPLETE",
                        format!("ref file {} failed verification: {message}", outcome.path.display()),
                        "preserve the repository and inspect the failing ref file before attempting repair",
                    ));
                }
            }
            for outcome in &verification.ref_item_outcomes {
                if let RefItemStatus::Failed { message } = &outcome.status {
                    issues.push(DoctorIssue::error(
                        "PRIKK-DOCTOR-VERIFY-REF-ITEM-INCOMPLETE",
                        format!("ref {} failed verification: {message}", outcome.ref_name),
                        "preserve the repository and inspect the failing ref before attempting repair",
                    ));
                }
            }
            // RFC 102 Stage 2: isolate-and-continue reading means a damaged WAL record no longer
            // fails the whole `WalReplay` stage -- same shape as the two ref loops above, one level
            // in for the WAL's own records.
            // RFC 108 increment 3d: `wal_record_outcomes` is built entirely from `default`'s own
            // replay (`verify.rs`'s single hardcoded `Wal::for_layout(layout, DEFAULT_ACTIVE_NAME)`)
            // -- a damaged record here can only ever be `default`'s, on the same footing as
            // `active_session_owning_stage_outcome`'s reasoning for the stage-outcome loop above.
            for outcome in &verification.wal_record_outcomes {
                if let WalRecordStatus::Failed { message } = &outcome.status {
                    issues.push(
                        DoctorIssue::error(
                            "PRIKK-DOCTOR-VERIFY-WAL-RECORD-INCOMPLETE",
                            format!(
                                "WAL record at offset {} failed verification: {message}{}",
                                outcome.offset,
                                sound_after_damage_note(
                                    &verification.wal_record_outcomes,
                                    outcome.offset
                                )
                            ),
                            "preserve the repository and inspect the failing WAL record before \
                             attempting repair",
                        )
                        .for_active_session(DEFAULT_ACTIVE_NAME),
                    );
                }
            }
            if verification
                .trailing_partial_wal_bytes
                .is_some_and(|n| n != 0)
            {
                issues.push(
                    DoctorIssue::warning(
                        "PRIKK-DOCTOR-WAL-TRAILING-PARTIAL",
                        format!(
                            concat!(
                                "active WAL has {} trailing byte(s) that look like an incomplete ",
                                "final record"
                            ),
                            verification.trailing_partial_wal_bytes.unwrap_or_default()
                        ),
                        "run `prikk doctor --repair-wal-tail` to truncate only the incomplete \
                         final WAL bytes",
                    )
                    .for_active_session(DEFAULT_ACTIVE_NAME),
                );
            }
            for issue in &verification.publication_trust_issues {
                issues.push(DoctorIssue::error(
                    issue.code,
                    issue.message.clone(),
                    "configure trusted MAINTAINER keys and re-run verification; doctor will not \
                     auto-trust keys or repair signatures",
                ));
            }
            for issue in &verification.signature_envelope_issues {
                issues.push(DoctorIssue::warning(
                    issue.code,
                    format!("{}: {}", issue.source, issue.message),
                    "preserve the format-1 bytes for inspection; do not normalize or reuse the envelope for mutation",
                ));
            }
            // Addendum 1 fix 1: I2 applies to `doctor` too -- a connectivity failure `verify` reports
            // must also keep `doctor` from reading clean, the same way every other item-level failure
            // above already does.
            for issue in &verification.connectivity_issues {
                issues.push(
                    DoctorIssue::error(
                        "PRIKK-DOCTOR-OBJECT-CONNECTIVITY",
                        format!(
                            "{} {} cannot be read ({}), referenced by {}",
                            issue.object_type,
                            issue.object_id,
                            issue.problem,
                            issue.referencing_work
                        ),
                        "preserve the repository and inspect the referenced object before \
                         attempting repair; doctor does not repair a missing or unreadable object",
                    )
                    .for_active_session(issue.active_session.clone()),
                );
            }
            // Addendum 1 fix 2: a frame that does not parse is not automatically harmless -- while
            // connectivity above reports a missing or unreadable object of this same container's
            // type, the frame may be holding exactly that object, so it is named rather than the
            // frame being called an unreferenced remnant.
            for append in &verification.object_interrupted_appends {
                let possibly_missing: Vec<_> = verification
                    .connectivity_issues
                    .iter()
                    .filter(|issue| issue.object_type == append.object_type)
                    .collect();
                if possibly_missing.is_empty() {
                    issues.push(DoctorIssue::warning(
                        "PRIKK-DOCTOR-OBJECT-INTERRUPTED-APPEND",
                        format!(
                            "object container {:?} has a frame at {} that does not parse ({})",
                            append.object_type,
                            append.path.display(),
                            append.message
                        ),
                        "no repair is required: the container record is made durable before its \
                         index entry, so a frame nothing still needs was never committed (an \
                         interrupted append, and whatever was written after it)",
                    ));
                } else {
                    let missing_ids = possibly_missing
                        .iter()
                        .map(|issue| issue.object_id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    issues.push(DoctorIssue::warning(
                        "PRIKK-DOCTOR-OBJECT-INTERRUPTED-APPEND",
                        format!(
                            "object container {:?} has a frame at {} that does not parse ({}); \
                             connectivity reports {} missing {:?} object(s) ({missing_ids}) that \
                             this frame may hold -- do not treat it as a harmless remnant",
                            append.object_type,
                            append.path.display(),
                            append.message,
                            possibly_missing.len(),
                            append.object_type
                        ),
                        "preserve the repository; the connectivity finding above names what needs \
                         repair, not this frame directly",
                    ));
                }
            }
            // Addendum 1 fix 3: the object index is a pure cache (rule 1), so its own damage is never
            // a failure -- but it must not be silent either.
            if verification
                .trailing_partial_object_index_bytes
                .is_some_and(|n| n != 0)
            {
                issues.push(DoctorIssue::warning(
                    "PRIKK-DOCTOR-OBJECT-INDEX-TRAILING-PARTIAL",
                    format!(
                        "object index has {} trailing byte(s) that look like an incomplete final \
                         record",
                        verification
                            .trailing_partial_object_index_bytes
                            .unwrap_or_default()
                    ),
                    "no repair is required: readers already rescan the containers in memory when \
                     the index is damaged (rule 1); run `prikk doctor --repair-index` to rebuild it \
                     on disk if desired",
                ));
            }
            if verification.object_index_interior_damage == Some(true) {
                issues.push(DoctorIssue::warning(
                    "PRIKK-DOCTOR-OBJECT-INDEX-INTERIOR-DAMAGE",
                    "object index has an interior record that failed to decode",
                    "no repair is required: readers already rescan the containers in memory when \
                     the index is damaged (rule 1); run `prikk doctor --repair-index` to rebuild it \
                     on disk if desired",
                ));
            }
            if verification
                .trailing_partial_pointer_index_bytes
                .is_some_and(|n| n != 0)
            {
                issues.push(DoctorIssue::warning(
                    "PRIKK-DOCTOR-POINTER-INDEX-TRAILING-PARTIAL",
                    format!(
                        "ref pointer index has {} trailing byte(s) that look like an incomplete \
                         final record",
                        verification
                            .trailing_partial_pointer_index_bytes
                            .unwrap_or_default()
                    ),
                    "run `prikk doctor --repair-pointer-index-tail` to truncate only the \
                     incomplete final pointer-index bytes",
                ));
            }
            // RFC 164 Rule B: one Rule-A file's own tail (a warning) or interior damage (an error) --
            // the same split every other framed file above already gets, extended by Rule A to these
            // seven files' own shape.
            for status in &verification.appended_file_tails {
                if status.trailing_partial_bytes != 0 {
                    issues.push(DoctorIssue::warning(
                        "PRIKK-DOCTOR-APPENDED-FILE-TRAILING-PARTIAL",
                        format!(
                            "{} has {} trailing byte(s) at offset {} that look like an incomplete \
                             final record",
                            status.label, status.trailing_partial_bytes, status.tail_offset
                        ),
                        "run `prikk doctor --repair-tails` to truncate only the incomplete final \
                         bytes",
                    ));
                }
                if let Some(message) = &status.interior_damage {
                    issues.push(DoctorIssue::error(
                        "PRIKK-DOCTOR-APPENDED-FILE-INTERIOR-DAMAGE",
                        format!("{}: {message}", status.label),
                        "preserve the repository for manual recovery; `--repair-tails` refuses on \
                         interior damage rather than guessing which bytes are safe to remove",
                    ));
                }
            }
            // RFC 164 Addendum 1 (N7): an object container's own short tail, reported (never
            // repaired -- Rule B only makes these report, per the review's ruling).
            for status in &verification.object_container_tails {
                if status.trailing_partial_bytes != 0 {
                    issues.push(DoctorIssue::warning(
                        "PRIKK-DOCTOR-OBJECT-CONTAINER-TRAILING-PARTIAL",
                        format!(
                            "{} container has {} trailing byte(s) that look like an incomplete final record",
                            status.object_type, status.trailing_partial_bytes
                        ),
                        "no automated repair; connectivity separately reports whether anything \
                         still needs the object this frame may be holding",
                    ));
                }
            }
            // RFC 164 Rule E: a stored object's own dangling reference, when the object making it
            // is not itself reachable from committed state -- never repaired or removed in 0.49.0.
            for remnant in &verification.unreferenced_remnants {
                issues.push(DoctorIssue::warning(
                    "PRIKK-DOCTOR-UNREFERENCED-REMNANT",
                    // RFC 165 R6: the same canonical form `verify.rs::ensure_object_exists` already
                    // uses for a missing reference -- `missing_role` already names what kind of thing
                    // is missing ("parent block", "block patch", …), so the owner/missing object
                    // types this used to print separately added nothing a reader couldn't already
                    // get from the role.
                    format!(
                        "object {} references missing {role} {}",
                        remnant.owner_object_id,
                        remnant.missing_object_id,
                        role = remnant.missing_role
                    ),
                    "re-run the import if you still have the bundle; otherwise it is harmless",
                ));
            }
            for path in &verification.object_temp_paths {
                let name = path
                    .file_name()
                    .and_then(|value| value.to_str())
                    .unwrap_or("<non-UTF-8 object temp>");
                issues.push(DoctorIssue::warning(
                    "PRIKK-DOCTOR-OBJECT-TEMP-DEBRIS",
                    format!("non-authoritative object publication temp remains: {name}"),
                    "preserve it for inspection; doctor does not infer ownership or remove object temps",
                ));
            }
            for issue in &verification.ref_publication_issues {
                // RFC 165 R4: a completable lead (`POINTER-LEADS-LOG`) now names `ref complete`
                // directly, since `verify`'s own `require_retained_evidence` only keeps that code
                // when `ref_completion::plan_ref_completion` agrees the lead is completable -- "the
                // same table" the handoff requires. `LEGACY-LOG-LEADS` is confirmed dead (format-1
                // only, never pushed by `classify_ref_state`) and stays grouped with the unrelated
                // `ACTIVE-CLEANUP-PENDING` rather than being reworked for a case that cannot occur.
                // `DIVERGENCE`'s own text still does not name the rebuild (R5, `prikk doctor
                // --rebuild-pointer-index`): that command does not exist yet (round 2 U4); naming it
                // here would promise a flag doctor cannot run.
                let recommendation = match issue.code {
                    "PRIKK-VERIFY-REF-POINTER-LEADS-LOG" => match issue.ref_name.as_deref() {
                        Some(ref_name) => format!(
                            "run `prikk ref complete {ref_name}`; doctor does not sign or append"
                        ),
                        None => "run `prikk ref complete <ref>` for the affected ref; doctor does not sign or append".to_string(),
                    },
                    "PRIKK-VERIFY-REF-LEGACY-LOG-LEADS" | "PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING" => {
                        "run signer-backed `prikk seal --allow-no-audit` for the affected ref; doctor does not sign or append".to_string()
                    }
                    "PRIKK-VERIFY-REF-POINTER-MISSING" => {
                        "preserve the repository; use signer-backed seal retry only with matching retained active state, otherwise restore from backup".to_string()
                    }
                    "PRIKK-VERIFY-REF-LEGACY-TIMESTAMP" => {
                        "treat the value as non-authoritative legacy data; do not normalize signed bytes in place".to_string()
                    }
                    "PRIKK-VERIFY-REF-DIVERGENCE" => {
                        "preserve the repository for manual recovery; signer-backed retry is not authorized without exact retained evidence".to_string()
                    }
                    _ => {
                        "preserve the candidate for inspection; doctor does not infer ownership or remove it".to_string()
                    }
                };
                let doctor_issue = if issue.blocking {
                    DoctorIssue::error(issue.code, issue.message.clone(), recommendation)
                } else {
                    DoctorIssue::warning(issue.code, issue.message.clone(), recommendation)
                };
                issues.push(doctor_issue);
            }
            add_active_wal_metadata_issues(&verification, &mut issues);
            add_commit_witness_issues(&verification, &mut issues);
            DoctorReport {
                verification: Some(verification),
                issues,
            }
        }
        Err(error) => {
            issues.push(issue_for_verification_error(error));
            DoctorReport {
                verification: None,
                issues,
            }
        }
    }
}

fn empty_wal_repair() -> WalRepair {
    WalRepair {
        preserved_records: 0,
        truncated_bytes: 0,
        preserved_patch_ids: Vec::new(),
        recovery_file: None,
        complete_records_removed: 0,
    }
}

/// Rebuild the object index from the containers and install it atomically —
/// `prikk doctor --repair-index`.
///
/// RFC 102's repair round. Separate from [`repair_repository`] rather than a third
/// `DoctorRepairOptions` flag: that function's whole body is per-active-session WAL work, taking one
/// `ActiveLock` at a time, and an index rebuild shares none of it — it touches no active session, no
/// WAL, and no ref. Folding it in would mean one report type describing two unrelated repairs and an
/// exit rule ("every skip is a failure") that was reasoned about for WAL tails only.
///
/// Reads and writes the index only. The containers are the source of truth here and are never
/// touched, which is asserted by test rather than merely intended.
///
/// **Holds [`LockableContainer::ObjectStore`] for the whole repair.** A repair is a *writer* to
/// `index.container` like any other, and the first version of this verb took no lock — measured
/// afterwards, forty `doctor --repair-index` runs launched beside forty `tag create` runs left
/// `verify` failing in four rounds of six: the repair scans, a concurrent writer appends, and the
/// repair then installs a rebuilt index that predates the append, discarding it. Acquired before the
/// first read, so the scan and the install are one exclusive region — a lock taken only around the
/// install would still write a stale scan.
///
/// Acquired here rather than in `foundation::index`, for the same reason
/// `append_object_under_lock` sits in `object_store.rs`: `foundation` is the bottom layer and must
/// not reach `crate::lock`. Nothing else is acquired while it is held, so it stays a leaf.
///
/// Three consequences, all of them ordinary lock behaviour and none of them silent:
///
/// - a writer (`commit`, `seal`, `tag create`, …) arriving during a repair is refused with
///   `lock conflict`;
/// - a repair arriving during a write is refused the same way;
/// - a stale `objects.lock` — left by a failed acquisition, see `concurrency-locking.md` — refuses
///   the repair with the message naming `prikk unlock`, which is the one case where the operator
///   must act before the repair can run.
pub fn repair_object_index(
    layout: &RepositoryLayout,
) -> Result<crate::foundation::index::IndexRepairReport> {
    layout.require_current_format()?;
    let _object_store_lock = acquire_container_locks(layout, &[LockableContainer::ObjectStore])?;
    crate::foundation::index::repair_index_from_containers(layout)
}

/// Safely truncate an incomplete trailing pointer-index record, if one exists —
/// `prikk doctor --repair-pointer-index-tail` (RFC 162 rule 3).
///
/// Separate from [`repair_repository`] for the same reason [`repair_object_index`] is: this touches no
/// active session, no WAL, and no object container -- only the ref-pointer-index container, under its
/// own lock. **Holds [`LockableContainer::RefPointerIndex`] for the whole repair**, matching
/// `compact_ref_pointer_index`'s own lock discipline (`compact.rs`): the read (to compute the tail) and
/// the write (to truncate it) are one exclusive region, so a concurrent publish cannot append between
/// them.
pub fn repair_pointer_index_tail(
    layout: &RepositoryLayout,
) -> Result<crate::refs::PointerIndexRepair> {
    layout.require_current_format()?;
    let _pointer_index_lock =
        acquire_container_locks(layout, &[LockableContainer::RefPointerIndex])?;
    crate::refs::truncate_pointer_index_trailing_partial(layout)
}

/// Run an explicitly requested, narrow repair action, now per-active-session (RFC 108 §D3.3,
/// increment 3d).
///
/// **§3.2's gate**: `before`'s issues are partitioned by `active_session`. Any `Error` with
/// `active_session: None` is repository-wide (a required directory missing, sealed-history/object/
/// ref verification, an incomplete publication) and refuses the **whole** run outright, exactly as
/// every refusal did before this increment -- proceeding over genuine repository-wide damage would
/// be a worse bug than the one this increment fixes (§4). An `Error` with `active_session:
/// Some(name)` refuses only *that* active session's own repair; every other eligible active session
/// is still attempted.
///
/// **§3.3's per-active pass**: iterates `RepositoryLayout::active_session_names` (already sorted,
/// so the order is deterministic), acquiring **one** active session's lock at a time and releasing
/// it (via `ActiveLock`'s own `Drop`) before moving to the next -- never two at once (§2.1: holding
/// N locks simultaneously reintroduces exactly the coupling this increment removes, and the
/// requirement is symmetric, so `default` is looped like every other name rather than
/// pre-acquired specially). A lock that cannot be acquired, or a repair action that itself fails
/// (e.g. `truncate_trailing_partial`'s own refusal on a damaged record, §4 -- unweakened, just no
/// longer able to fail the whole run) is recorded as `Skipped` with the reason, never propagated:
/// "nothing could be repaired" must never be the answer because one active session is busy or
/// broken (§2.1).
pub fn repair_repository(
    layout: &RepositoryLayout,
    options: DoctorRepairOptions,
) -> Result<DoctorRepairReport> {
    layout.require_current_format()?;
    if options.reconstruct_main_ref {
        return Err(PrikkError::Integrity(
            "--repair-main-ref has no implemented repair; doctor cannot reconstruct a missing \
             heads/main ref"
                .to_string(),
        ));
    }
    crate::refs::ensure_no_incomplete_publication(layout)?;
    let before = doctor_repository(layout);
    if before
        .issues
        .iter()
        .any(|issue| issue.severity == DoctorSeverity::Error && issue.active_session.is_none())
    {
        // RFC 108 increment 3b's message correction still applies (this may be a doctor-level
        // check, not a `verify` finding) -- now also naming that a *repository-wide* issue is what
        // refused everything, since increment 3d gives an active-scoped issue a narrower gate.
        return Err(PrikkError::Integrity(
            "doctor repair refused because the repository has a repository-wide issue; see `prikk \
             doctor`'s own issue list for which check reported it -- not necessarily a `verify` \
             finding, and not scoped to any one active session"
                .to_string(),
        ));
    }

    let mut active_repairs = Vec::new();
    for name in layout.active_session_names()? {
        if let Some(reason) = before.issues.iter().find_map(|issue| {
            (issue.severity == DoctorSeverity::Error
                && issue.active_session.as_deref() == Some(name.as_os_str()))
            .then(|| {
                format!(
                    "this active session has its own blocking issue: {}",
                    issue.message
                )
            })
        }) {
            active_repairs.push(ActiveSessionRepairOutcome {
                active_session: name,
                status: ActiveSessionRepairStatus::Skipped { reason },
            });
            continue;
        }
        let lock = match ActiveLock::acquire(layout, &name) {
            Ok(lock) => lock,
            Err(error) => {
                active_repairs.push(ActiveSessionRepairOutcome {
                    active_session: name,
                    status: ActiveSessionRepairStatus::Skipped {
                        reason: format!("could not acquire its active-session lock: {error}"),
                    },
                });
                continue;
            }
        };
        let repair_result = if options.truncate_wal_tail {
            Wal::for_layout(layout, &name).truncate_trailing_partial()
        } else {
            Ok(empty_wal_repair())
        };
        drop(lock);
        active_repairs.push(ActiveSessionRepairOutcome {
            active_session: name,
            status: match repair_result {
                Ok(wal_repair) => ActiveSessionRepairStatus::Repaired(wal_repair),
                Err(error) => ActiveSessionRepairStatus::Skipped {
                    reason: format!("repair attempt failed: {error}"),
                },
            },
        });
    }

    // `wal_repair` keeps its exact prior type and meaning (`default`'s own outcome) -- see its own
    // doc comment on `DoctorRepairReport` for what a zero here does and does not prove now that
    // `Ok` is reachable even when `default` itself was skipped.
    let wal_repair = active_repairs
        .iter()
        .find(|outcome| outcome.active_session.to_str() == Some(DEFAULT_ACTIVE_NAME))
        .map(|outcome| match &outcome.status {
            ActiveSessionRepairStatus::Repaired(wal_repair) => wal_repair.clone(),
            ActiveSessionRepairStatus::Skipped { .. } => empty_wal_repair(),
        })
        .unwrap_or_else(empty_wal_repair);

    let after = doctor_repository(layout);
    Ok(DoctorRepairReport {
        before,
        wal_repair,
        active_repairs,
        after,
    })
}

fn add_active_wal_metadata_issues(
    verification: &RepositoryVerification,
    issues: &mut Vec<DoctorIssue>,
) {
    // `None` (the active-WAL-metadata stage did not evaluate) is already surfaced, more precisely, by
    // the stage-outcome loop above this function's own call site.
    let Some(status) = &verification.active_wal_metadata_status else {
        return;
    };
    // RFC 108 increment 3d, §2.2: these are exactly as active-scoped as the non-default issues
    // above -- only their code names differ, for historical reasons (they predate this arc). Same
    // code, same severity, same message as before this increment (§4) -- only `.for_active_session`
    // is new, so `repair_repository`'s new per-active gate treats a `default`-only metadata problem
    // as `default`'s own, not repository-wide.
    match status {
        ActiveWalMetadataStatus::MissingForNonEmptyWal => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MISSING",
                "active WAL has records but active ref metadata is missing",
                "run `prikk doctor --restore-queue-target --ref <ref>` to give the queue its \
                 owner back",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        ActiveWalMetadataStatus::InvalidForNonEmptyWal { reason } => {
            issues.push(
                DoctorIssue::error(
                    "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED",
                    format!("active WAL has records but active ref metadata is malformed: {reason}"),
                    "run `prikk doctor --restore-queue-target --ref <ref>` to give the queue its \
                     owner back",
                )
                .for_active_session(DEFAULT_ACTIVE_NAME),
            );
        }
        ActiveWalMetadataStatus::ValidForEmptyWal { ref_name } => issues.push(
            DoctorIssue::warning(
                "PRIKK-DOCTOR-ACTIVE-REF-METADATA-DEBRIS",
                format!("active WAL is empty but stale ref metadata remains for {ref_name}"),
                "no repair is required; the next guarded active-WAL append will replace stale metadata",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        ActiveWalMetadataStatus::InvalidForEmptyWal { reason } => issues.push(
            DoctorIssue::warning(
                "PRIKK-DOCTOR-ACTIVE-REF-METADATA-MALFORMED-DEBRIS",
                format!("active WAL is empty but malformed ref metadata remains: {reason}"),
                "no repair is required; the next guarded active-WAL append will replace stale metadata",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        ActiveWalMetadataStatus::MissingForEmptyWal
        | ActiveWalMetadataStatus::ValidForNonEmptyWal { .. } => {}
    }
}

/// RFC 166 D3/D6, round 1: rows 4, 5, 6, 7 and 9 of §5 name no recovery verb yet
/// (`--discard-damaged-commits`/`--restore-queue-target` are round 2, D5) -- each recommendation says
/// only that the commit was acknowledged and cannot be removed as a crash leftover, never a verb that
/// does not exist. Row 8 is the one warning, rebuilt by `--repair-tails` (§13 item 5). Rows `NoWitness`,
/// `Healthy`, `Pending` and `CrashTail` push nothing -- `NoWitness` is 0.48.0's own unchanged rule 3
/// (never worse, C2), and the other three are the ordinary, unremarkable cases the verdict table's own
/// "0" column names.
fn add_commit_witness_issues(verification: &RepositoryVerification, issues: &mut Vec<DoctorIssue>) {
    // `None` (the stage did not evaluate) is already surfaced, more precisely, by the stage-outcome
    // loop above this function's own call site -- same reasoning as `add_active_wal_metadata_issues`.
    let Some(verdict) = &verification.commit_witness_verdict else {
        return;
    };
    match verdict {
        // RFC 166 §13 item 16: a stale witness (a drain an older binary's own seal left behind,
        // connectivity-confirmed) is silent in `status` -- same as a genuinely absent one -- but
        // `doctor` alone notes it, since the next commit or `--repair-tails` replaces it.
        Verdict::NoWitness { stale: true, .. } => issues.push(
            DoctorIssue::info(
                "PRIKK-DOCTOR-COMMIT-WITNESS-STALE",
                "this session's own acknowledgment history is stale (an older binary's own seal \
                 drained the queue it covered)",
                "no action is required; the next commit or `prikk doctor --repair-tails` replaces \
                 it",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        Verdict::NoWitness { stale: false, .. }
        | Verdict::Healthy { .. }
        | Verdict::Pending { .. }
        | Verdict::CrashTail { .. } => {}
        Verdict::AcknowledgedDamage { witnessed_seq } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-DAMAGE",
                format!("a queued commit you were told had succeeded (sequence {witnessed_seq}) is damaged"),
                "run `prikk doctor --discard-damaged-commits` to remove it; it was already acknowledged, so it cannot be removed as a crash leftover",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        Verdict::AcknowledgedLoss { witnessed_seq } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-COMMIT-WITNESS-ACKNOWLEDGED-LOSS",
                format!("a queued commit you were told had succeeded (sequence {witnessed_seq}) is no longer present at all"),
                "run `prikk doctor --discard-damaged-commits` to declare it lost; it was already acknowledged, so it cannot be removed as a crash leftover",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        Verdict::SubstitutedRecord { witnessed_seq } => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-COMMIT-WITNESS-SUBSTITUTED-RECORD",
                format!("sequence {witnessed_seq} does not match the queued commit you were told had succeeded"),
                "preserve the repository; this is not a crash shape, and a copy is the way out",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        Verdict::UnknownWithDamagedWitness => issues.push(
            DoctorIssue::error(
                "PRIKK-DOCTOR-COMMIT-WITNESS-UNKNOWN",
                "the queue has an unexplained tail, and this session's own acknowledgment history is unreadable, so the tail cannot be shown to be a crash leftover",
                "run `prikk doctor --discard-damaged-commits` to discard it; it cannot be removed as a crash leftover without being able to confirm whether it was acknowledged",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        Verdict::WitnessDamaged => issues.push(
            DoctorIssue::warning(
                "PRIKK-DOCTOR-COMMIT-WITNESS-DAMAGED",
                "this session's own acknowledgment history is unreadable, but the active WAL it covers is wholly sound",
                "run `prikk doctor --repair-tails` to rebuild it",
            )
            .for_active_session(DEFAULT_ACTIVE_NAME),
        ),
        // D6's mismatch case only: when ownership is missing or malformed outright (not merely
        // disagreeing with the witness), `add_active_wal_metadata_issues` above already reports it,
        // precisely, as `ACTIVE-REF-METADATA-MISSING`/`MALFORMED` -- a second, less specific code for
        // the exact same root cause would be two independently derived answers to one question, the
        // defect class `classify`/`VERDICT_CONDITIONS` exist to remove, not add. This fires only when
        // ownership is validly present (`ValidForNonEmptyWal`) and still disagrees with the witness --
        // the one case `add_active_wal_metadata_issues` has no way to see at all.
        Verdict::OwnershipMissing => {
            if matches!(
                verification.active_wal_metadata_status,
                Some(ActiveWalMetadataStatus::ValidForNonEmptyWal { .. })
            ) {
                issues.push(
                    DoctorIssue::error(
                        "PRIKK-DOCTOR-COMMIT-WITNESS-OWNERSHIP-MISSING",
                        "the active ref metadata names an owner this session's own acknowledgment \
                         history does not recognize as its own",
                        "run `prikk doctor --restore-queue-target --ref <ref>` to give the queue \
                         its owner back",
                    )
                    .for_active_session(DEFAULT_ACTIVE_NAME),
                );
            }
        }
    }
}

/// `; N sound record(s) follow it and are intact; --repair-wal-tail will not touch them` -- what a damaged WAL record leaves standing,
/// so the operator sees that the repair verb (which truncates only a torn tail) is not the answer (RFC 160 F3).
fn sound_after_damage_note(outcomes: &[crate::wal::WalRecordOutcome], offset: usize) -> String {
    let sound_after = outcomes
        .iter()
        .filter(|outcome| {
            outcome.offset > offset && matches!(outcome.status, WalRecordStatus::Evaluated)
        })
        .count();
    if sound_after == 0 {
        return String::new();
    }
    format!(
        "; {sound_after} sound record(s) follow it and are intact, and `--repair-wal-tail` (which truncates only a torn tail) will not touch them"
    )
}

fn issue_for_verification_error(error: PrikkError) -> DoctorIssue {
    DoctorIssue::error(
        "PRIKK-DOCTOR-VERIFY-ERROR",
        format!("repository verification failed: {error}"),
        "do not run seal or publish operations; preserve the repository and inspect the \
         failing path before attempting repair",
    )
}

#[cfg(test)]
mod tests;
