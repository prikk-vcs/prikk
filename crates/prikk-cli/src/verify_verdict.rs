//! RFC 118 stage 5: `prikk verify`'s blocking-condition declaration.
//!
//! `RepositoryVerification` has no method meaning "would `prikk verify` refuse this repository" --
//! the authoritative answer used to live only as an else-if chain, hand-written, inside
//! `main.rs::run_verify`. `--format json` needs the identical answer in a machine-readable shape,
//! and two independently hand-written lists that happen to agree today is exactly the defect RFC
//! 118 exists to remove: they could silently diverge the next time either one is edited alone.
//!
//! [`VERDICT_CONDITIONS`] is the single declaration both consumers read: `main.rs::run_verify`'s
//! exit chain (which keeps its own short-circuiting, first-match-wins behaviour and distinct
//! per-condition message, unchanged from before this stage) and `output::verification`'s
//! `--format json` emitter (which reports every condition currently true, not only the first, since
//! a CI job reading structured output benefits from the complete picture a single terse exit message
//! cannot carry). Removing an entry here removes it from both at once, by construction -- there is
//! no second list to forget.
//!
//! **Not exhaustive over every predicate `RepositoryVerification` exposes, deliberately.**
//! `has_trailing_partial_wal` and `has_active_wal_metadata_warning` are excluded on purpose: both
//! are warning-severity findings, already surfaced as `println!("warning: ...")` lines in
//! `print_verify_report` rather than contributing to its `Err` chain, matching the warning/error
//! severity split `doctor`'s own report already draws (`DoctorSeverity::Warning` vs `Error`). A
//! repository with only these findings verifies cleanly today, and this declaration does not change
//! that.

use prikk_store::RepositoryVerification;

/// One condition under which `prikk verify` refuses a repository (exit nonzero, JSON `ok: false`).
pub(crate) struct VerdictCondition {
    /// Stable, kebab-case identifier -- the JSON key naming this condition when it is true. An
    /// external interface once stage 5 ships, the same footing `VerificationStage::label()` is on:
    /// renaming one is a breaking change to any tool that reads `--format json`.
    pub(crate) id: &'static str,
    /// The exact sentence `prikk verify`'s exit chain has always used for this condition,
    /// unchanged by this stage -- carried into the JSON verbatim rather than re-derived, so a
    /// human reading either form sees the identical claim.
    pub(crate) message: &'static str,
    /// Whether this condition is currently true of `report`.
    pub(crate) check: fn(&RepositoryVerification) -> bool,
    /// RFC 136 §10.3b.1: whether this condition, when true, keeps the provisional-worktree marker set.
    /// A condition about objects, the WAL, refs or block state blocks clearing, because it leaves open
    /// whether replay produces the state a snapshot claimed. Each row says why it is marked as it is.
    pub(crate) blocks_provisional_clear: bool,
}

/// Declared in exit-chain order: `main.rs::run_verify` reports the *first* true condition (its
/// existing short-circuiting behaviour, unchanged), so this order is load-bearing for which single
/// message a human sees on the command line, even though `--format json` reports every true
/// condition regardless of position.
pub(crate) const VERDICT_CONDITIONS: &[VerdictCondition] = &[
    VerdictCondition {
        id: "stage-failure",
        message: "repository verification did not complete every stage; see stage outcomes above",
        check: RepositoryVerification::has_stage_failure,
        // A stage that did not complete may be the one that replays blocks.
        blocks_provisional_clear: true,
    },
    VerdictCondition {
        id: "item-failure",
        message: "repository verification found at least one failed object, block, or ref; see \
                   item outcomes above",
        check: RepositoryVerification::has_item_failure,
        // A failed object, block or ref: objects, block state, refs.
        blocks_provisional_clear: true,
    },
    VerdictCondition {
        id: "active-wal-metadata-integrity",
        message: "repository has active-WAL metadata integrity issues",
        check: RepositoryVerification::has_active_wal_metadata_integrity_issue,
        // The WAL.
        blocks_provisional_clear: true,
    },
    VerdictCondition {
        id: "blocking-ref-publication",
        message: "repository has interrupted or divergent ref publication state",
        check: RepositoryVerification::has_blocking_ref_publication_issues,
        // Refs: an interrupted or divergent publication leaves which tip was replayed open.
        blocks_provisional_clear: true,
    },
    VerdictCondition {
        id: "publication-trust",
        message: "repository has publication-trust issues",
        check: RepositoryVerification::has_publication_trust_issues,
        // Ruled (RFC 136 §10.3b.1): who signed says nothing about whether replay equals the snapshot.
        blocks_provisional_clear: false,
    },
    VerdictCondition {
        id: "commit-index-divergence",
        message: "commit-index cache disagrees with the worktree for at least one path",
        check: RepositoryVerification::has_commit_index_divergence,
        // A worktree cache compared with the worktree, not history. A worktree just written from a
        // snapshot can disagree with it by construction, so counting it would keep the marker forever.
        blocks_provisional_clear: false,
    },
    VerdictCondition {
        id: "lifecycle-cache-divergence",
        message: "lifecycle-state cache disagrees with an independent replay",
        check: RepositoryVerification::has_lifecycle_cache_divergence,
        // Counted, conservatively: the lifecycle cache holds replay-derived block state, and a
        // disagreement with an independent replay is a block-state question left open.
        blocks_provisional_clear: true,
    },
    VerdictCondition {
        id: "active-wal-ordering",
        message: "active WAL contains an out-of-order or duplicate queued patch sequence",
        check: RepositoryVerification::has_active_wal_ordering_issue,
        // The WAL.
        blocks_provisional_clear: true,
    },
    VerdictCondition {
        id: "merge-baseline-divergence",
        message: "a merge block's recorded baseline is not a common ancestor of its parents",
        check: RepositoryVerification::has_merge_baseline_divergence,
        // Block state: a merge block's recorded baseline.
        blocks_provisional_clear: true,
    },
];

/// The first true condition, in declared order -- `main.rs::run_verify`'s exit chain reduces to
/// this one call, preserving its exact prior short-circuiting behaviour and message.
pub(crate) fn first_true_condition(
    report: &RepositoryVerification,
) -> Option<&'static VerdictCondition> {
    VERDICT_CONDITIONS
        .iter()
        .find(|condition| (condition.check)(report))
}

/// Every currently-true condition that keeps the provisional-worktree marker set (RFC 136 §10.3b.1).
pub(crate) fn provisional_clear_blockers(
    report: &RepositoryVerification,
) -> Vec<&'static VerdictCondition> {
    VERDICT_CONDITIONS
        .iter()
        .filter(|condition| condition.blocks_provisional_clear && (condition.check)(report))
        .collect()
}

/// Every currently-true condition, in declared order -- what `--format json` reports, since a
/// structured consumer benefits from the complete picture a single terse exit message cannot carry.
pub(crate) fn all_true_conditions(
    report: &RepositoryVerification,
) -> Vec<&'static VerdictCondition> {
    VERDICT_CONDITIONS
        .iter()
        .filter(|condition| (condition.check)(report))
        .collect()
}
