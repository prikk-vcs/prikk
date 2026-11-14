//! The way out of a finding, in one place (0.57.0 step 1e).
//!
//! **A reader never reads cleaner than the check it reports on, and every finding names a way out that is
//! true.** `prikk doctor` reports a finding as an issue with a recommendation; `prikk verify` prints the
//! same sentence under the same finding. Both read it from [`WayOut::sentence`], so the two can never say
//! different things about one finding, and a sentence is written once.
//!
//! Each sentence is one of three things, and says which:
//! 1. **a command that clears the finding.** Every such command was typed against the damage and `verify`
//!    read clean afterwards (`crates/prikk-cli/tests/verify_and_doctor/step1e_every_finding_names_a_true_way_out.rs`);
//! 2. **the sound-copy sequence, in full** -- [`SOUND_COPY`], typed once in the same test file: the new
//!    repository verifies clean and holds the history;
//! 3. **the honest sentence that no command repairs it in place**, followed by (2).
//!
//! A sentence that names a command that would not clear the finding is a defect, not a style choice: the
//! 1a ordering row first named `--discard-damaged-commits` and the test showed it does not clear that
//! finding.

use crate::verify::{ActiveWalMetadataStatus, ObjectItemStatus, RepositoryVerification};

/// The sound-copy sequence, in full. `setup` makes the new directory a repository (and adopts this
/// installation's maintainer key); `archive verify` reads the backup without a repository's help and
/// reports whether its sealing key is adopted; `archive import --adopt` lands the branch as a local branch
/// and writes its files. Only sealed history travels in a backup, so a commit still waiting for a `seal` is
/// made again in the new directory.
pub const SOUND_COPY: &str = "restore a sound copy of the sealed history: `prikk setup <new-dir>`, then, in \
<new-dir>, `prikk archive verify <backup>` and `prikk archive import <backup> --adopt` (or `prikk bundle \
import --input <bundle>` for one branch); the key that sealed the history must be adopted in <new-dir> \
(`archive verify` reports it). Only sealed history travels in a backup: a commit still waiting for a \
`prikk seal` is made again there";

/// One kind of finding and the way out of it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum WayOut {
    /// A verification stage did not complete.
    StageIncomplete,
    /// A stored object failed verification.
    StoredObject,
    /// A block's state root does not verify.
    BlockState,
    /// A ref pointer or log file, or a ref's own verification, failed.
    RefFile,
    /// A record of the active WAL failed verification.
    WalRecord,
    /// An object a sealed block, a queued patch or a ref references cannot be read.
    Connectivity,
    /// An appended file has interior damage (a damaged record that is not a tail).
    AppendedFileInterior,
    /// The active WAL holds records but its ref metadata is missing or malformed.
    ActiveMetadata,
    /// An acknowledged commit is damaged or gone, or the queue has a tail nothing can explain.
    AcknowledgedCommit,
    /// The queue's owner is not one its own acknowledgment history recognizes.
    OwnershipMismatch,
    /// A record before the last acknowledged one was substituted.
    SubstitutedRecord,
    /// A merge block's recorded baseline is not a common ancestor of its parents.
    MergeBaseline,
    /// The active WAL's sequence does not increase.
    WalOrdering,
    /// A received `remotes/…` pointer failed verification.
    ReceivedRef(String),
    /// A commit-index entry disagrees with its file.
    CommitIndex,
    /// The lifecycle-state cache disagrees with a replay.
    LifecycleCache,
    /// An adopted key's signature on a publication object does not verify, or no trusted key signed.
    PublicationTrust,
    /// A ref's pointer and log disagree, by the code `verify` gives that state.
    RefPublication {
        /// The `PRIKK-VERIFY-REF-*` code.
        code: String,
        /// The ref concerned, when `verify` names one.
        ref_name: Option<String>,
    },
}

impl WayOut {
    /// The sentence both `doctor` and `verify` print for this kind of finding.
    #[must_use]
    pub fn sentence(&self) -> String {
        match self {
            Self::StageIncomplete => format!(
                "the findings listed with this one name the cause and its command (for example `prikk doctor \
                 --rebuild-pointer-index` when the ref pointer index is damaged); when none does, no command \
                 repairs the damage in place: {SOUND_COPY}"
            ),
            Self::StoredObject => format!(
                "no command repairs a damaged stored object in place: {SOUND_COPY}"
            ),
            Self::BlockState => format!(
                "no command repairs a block whose state root does not verify in place: {SOUND_COPY}"
            ),
            Self::RefFile => format!(
                "run `prikk doctor --rebuild-pointer-index --plan-only`: when the ref log is sound and the pointer \
                 index is what is wrong, it re-derives the pointer index, and the same command without \
                 `--plan-only` clears this. When it says `nothing written` and this stays, the ref names an object \
                 that is damaged or missing (the stored-object finding listed with this one, if any, names it); \
                 when it refuses, the ref log itself is damaged. In both of those cases no command repairs it in \
                 place: {SOUND_COPY}"
            ),
            Self::WalRecord => format!(
                "run `prikk doctor --discard-damaged-commits --plan-only`: when the damaged record is a commit you \
                 were told had succeeded, it removes it (or declares it lost) and the same command without \
                 `--plan-only` clears this; when it refuses, no command repairs the queue in place, and the \
                 sealed history is untouched: {SOUND_COPY}"
            ),
            Self::Connectivity => format!(
                "an object that sealed history, a queued commit or a ref references cannot be read, and no \
                 command repairs that in place: {SOUND_COPY}"
            ),
            Self::AppendedFileInterior => format!(
                "no command repairs interior damage in place (`prikk doctor --repair-tails` refuses it rather \
                 than guess which bytes are safe to remove): {SOUND_COPY}"
            ),
            Self::ActiveMetadata | Self::OwnershipMismatch => {
                "run `prikk doctor --restore-queue-target --ref <ref> --plan-only`, naming the branch the queued \
                 commits belong to, then the same command without `--plan-only`: it gives the queue its owner \
                 back and clears this"
                    .to_string()
            }
            Self::AcknowledgedCommit => {
                "run `prikk doctor --discard-damaged-commits --plan-only`, then the same command without \
                 `--plan-only`: it removes the acknowledged commit the queue no longer holds soundly, or declares \
                 it lost, saving what it removed to the recovery log, and clears this. If this follows an \
                 interrupted `prikk uncommit`, running that same command again finishes it"
                    .to_string()
            }
            Self::SubstitutedRecord => format!(
                "no command repairs a substituted record in place, by design (a repair that rewrote it could not \
                 tell damage from tampering), and `prikk doctor --discard-damaged-commits` refuses it: {SOUND_COPY}"
            ),
            Self::MergeBaseline => format!(
                "no command rewrites signed history: {SOUND_COPY}"
            ),
            Self::WalOrdering => format!(
                "the sealed history is not affected, but no command repairs an out-of-order queue in place \
                 (`prikk doctor --discard-damaged-commits` does not clear it): {SOUND_COPY}"
            ),
            Self::ReceivedRef(name) => format!(
                "a received ref is a pointer to what another repository sent, not local history: run `prikk \
                 received remove {name} --plan-only`, then `prikk received remove {name}`, and receive it again \
                 if you still want it"
            ),
            Self::CommitIndex => crate::COMMIT_INDEX_DIVERGENCE_ADVICE.to_string(),
            Self::LifecycleCache => crate::lifecycle_cache_divergence_advice(),
            Self::RefPublication { code, ref_name } => match code.as_str() {
                "PRIKK-VERIFY-REF-POINTER-LEADS-LOG" => match ref_name.as_deref() {
                    Some(ref_name) => format!(
                        "run `prikk ref complete {ref_name}`; doctor does not sign or append"
                    ),
                    None => "run `prikk ref complete <ref>` for the affected ref; doctor does not sign or append"
                        .to_string(),
                },
                "PRIKK-VERIFY-REF-LEGACY-LOG-LEADS" | "PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING" => {
                    "run signer-backed `prikk seal --allow-no-audit` for the affected ref; doctor does not sign or \
                     append"
                        .to_string()
                }
                "PRIKK-VERIFY-REF-POINTER-MISSING" => format!(
                    "run signer-backed `prikk seal --allow-no-audit` for the affected ref only when the retained \
                     active state matches; otherwise no command repairs a missing ref pointer in place: {SOUND_COPY}"
                ),
                "PRIKK-VERIFY-REF-LEGACY-TIMESTAMP" => {
                    "treat the value as non-authoritative legacy data; do not normalize signed bytes in place"
                        .to_string()
                }
                "PRIKK-VERIFY-REF-DIVERGENCE" => format!(
                    "no command resolves a pointer/log divergence in place (a pointer rebuild would only reproduce \
                     it), and a signer-backed retry is not authorized without exact retained evidence: {SOUND_COPY}"
                ),
                _ => "the candidate is kept for inspection; doctor does not infer ownership or remove it".to_string(),
            },
            Self::PublicationTrust => {
                "configure trusted MAINTAINER keys with `prikk trust maintainer add --key-id <id> --public-key \
                 <hex>` and run `prikk verify` again; doctor will not auto-trust keys or repair signatures"
                    .to_string()
            }
        }
    }
}

/// The ways out `verify` prints, one per kind of finding it fails on, each with a short label for the
/// finding. The same kinds `doctor` reports, in the same words ([`WayOut::sentence`]).
#[must_use]
pub fn ways_out(report: &RepositoryVerification) -> Vec<(&'static str, WayOut)> {
    use crate::commit_boundary::classification::Verdict;
    let mut out: Vec<(&'static str, WayOut)> = Vec::new();
    if report.has_stage_failure() {
        out.push((
            "a verification stage did not complete",
            WayOut::StageIncomplete,
        ));
    }
    if report
        .object_outcomes
        .iter()
        .any(|o| matches!(o.status, ObjectItemStatus::Failed { .. }))
    {
        out.push(("a stored object failed", WayOut::StoredObject));
    }
    if report
        .block_state_outcomes
        .iter()
        .any(|o| o.status.is_failure())
    {
        out.push(("a block's state root failed", WayOut::BlockState));
    }
    if report
        .pointer_outcomes
        .iter()
        .chain(&report.log_outcomes)
        .any(|o| matches!(o.status, crate::refs::RefFileStatus::Failed { .. }))
        || report
            .ref_item_outcomes
            .iter()
            .any(|o| matches!(o.status, crate::refs::RefItemStatus::Failed { .. }))
    {
        out.push(("a ref file or ref failed", WayOut::RefFile));
    }
    if report
        .wal_record_outcomes
        .iter()
        .any(|o| matches!(o.status, crate::wal::WalRecordStatus::Failed { .. }))
    {
        out.push(("a WAL record failed", WayOut::WalRecord));
    }
    if !report.connectivity_issues.is_empty() {
        out.push((
            "an object is referenced but unreadable",
            WayOut::Connectivity,
        ));
    }
    if report
        .appended_file_tails
        .iter()
        .any(|status| status.interior_damage.is_some())
    {
        out.push((
            "an appended file has interior damage",
            WayOut::AppendedFileInterior,
        ));
    }
    if report
        .active_wal_metadata_status
        .as_ref()
        .is_some_and(ActiveWalMetadataStatus::has_integrity_issue)
    {
        out.push((
            "the active WAL has no usable ref metadata",
            WayOut::ActiveMetadata,
        ));
    }
    match &report.commit_witness_verdict {
        Some(
            Verdict::AcknowledgedDamage { .. }
            | Verdict::AcknowledgedLoss { .. }
            | Verdict::UnknownWithDamagedWitness,
        ) => out.push((
            "an acknowledged commit is damaged or gone",
            WayOut::AcknowledgedCommit,
        )),
        Some(Verdict::SubstitutedRecord { .. }) => {
            out.push(("a queued commit was substituted", WayOut::SubstitutedRecord));
        }
        Some(Verdict::OwnershipMissing) => {
            out.push((
                "the queue's owner is not recognized",
                WayOut::OwnershipMismatch,
            ));
        }
        _ => {}
    }
    if report.has_commit_witness_substituted_earlier_record() {
        out.push((
            "an earlier queued record was substituted",
            WayOut::SubstitutedRecord,
        ));
    }
    for issue in report
        .ref_publication_issues
        .iter()
        .filter(|issue| issue.blocking)
    {
        let kind = WayOut::RefPublication {
            code: issue.code.to_string(),
            ref_name: issue.ref_name.clone(),
        };
        if !out.iter().any(|(_, existing)| *existing == kind) {
            // 0.58.0 step 0b: a seal stopped after it published is not a disagreement (pointer and log agree); name the state.
            let label = if issue.code == "PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING" {
                "a seal stopped after it published, before it cleaned the queue"
            } else {
                "a ref's pointer and log disagree"
            };
            out.push((label, kind));
        }
    }
    if report.has_publication_trust_issues() {
        out.push((
            "a publication signature did not verify",
            WayOut::PublicationTrust,
        ));
    }
    if report.has_merge_baseline_divergence() {
        out.push(("a merge block's baseline is false", WayOut::MergeBaseline));
    }
    if report.has_active_wal_ordering_issue() {
        out.push(("the active WAL is out of order", WayOut::WalOrdering));
    }
    for outcome in &report.received_ref_item_outcomes {
        if let crate::refs::RefItemStatus::Failed { .. } = outcome.status {
            out.push((
                "a received ref failed",
                WayOut::ReceivedRef(outcome.ref_name.clone()),
            ));
        }
    }
    if report.has_commit_index_divergence() {
        out.push((
            "the commit-index disagrees with the worktree",
            WayOut::CommitIndex,
        ));
    }
    if report.has_lifecycle_cache_divergence() {
        out.push((
            "the lifecycle-state cache disagrees with a replay",
            WayOut::LifecycleCache,
        ));
    }
    out
}
