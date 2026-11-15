//! The batch of a `sync seal`: its plan (the fold of S2b and the reads of S3), and the pre-write in-the-way check that judges the state the
//! fold ends in. Split out of `seal_from_accepted.rs` (which the S3 reads would have pushed past the 1,200-line limit); everything here is
//! the batch-level half, built on that module's private per-claim helpers (`read_current_tip`, `ref_history`, `enumerate_stored_claims`,
//! `parent_reached`, ...), which a child module may use.

use prikk_error::Result;
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload};

use super::{
    RefHistory, claims_by_block, enumerate_stored_claims, parent_not_reached_refusal,
    parent_reached, read_current_tip, ref_history, refuse_if_order_ambiguous_among,
};
use crate::anchor_trust::AnchorSite;
use crate::block_state::StateAnchoring;
use crate::foundation::fsutil::stat_file_state_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::merge::execute::candidate_replay_manifest_wanting;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::refs::{RefStore, validate_local_branch_ref};
use crate::worktree_materialize::{
    PendingCurrentBranchMaterialization, require_nothing_in_the_way,
};

/// The batch, read and folded once (0.58.0 S2b and S3; RFC 159's mechanism with the batch's running state as an in-memory anchor).
///
/// **S2b, the fold.** [`SealBatchPlan::fold`] derives the pre-batch state once and applies each claim's patches to that one state in the
/// batch's order, producing the state root each claim's Block will carry and, at the end, the state the batch will publish -- the target of
/// the pre-write in-the-way check ([`require_nothing_in_the_way_for_seal_batch`]). The real loop ([`super::seal_from_accepted_claim_in_batch`])
/// consumes it claim by claim, so the batch is derived **once**.
///
/// **S3, the reads.** [`SealBatchPlan::open`] reads, once per batch, the two sets every per-claim check used to re-read: the stored
/// recognition claims (the order-ambiguity check and the parent check) and the ref's history (which patches are already sealed). They live
/// here, beside the fold's running state, as one reader each. The history is **extended** by each block the batch publishes, not re-walked;
/// the claims are **re-read when the claim container has changed** (see [`SealBatchPlan::refresh_claims`]).
///
/// **A plan never refuses and never decides.** It stops, with no opinion, at the first claim it cannot fold, or whose selection it
/// cannot reproduce; every claim from there on is sealed from disk exactly as before, with the same checks and the same messages. The
/// real loop also compares each claim's own selection with the plan's, abandons the plan's fold at the first difference, and compares the
/// ref's tip it reads with the tip the plan's history ends at, re-walking the history when they differ.
pub struct SealBatchPlan {
    /// The batch-scoped reads; `None` for a plan that could not be opened (every check then reads as it always did).
    pub(super) reads: Option<BatchReads>,
    steps: std::collections::VecDeque<PlannedStep>,
    /// The state after the last claim folded (its entries and text cache); `None` when no claim folded. The in-the-way check turns it
    /// into the replay manifest it judges, only when it runs: that manifest holds every file's bytes, which a batch onto a branch that is
    /// not the current one never needs.
    final_state: Option<(
        Vec<crate::state_root::StateRootEntry>,
        crate::lifecycle_cache::replay::TextCache,
    )>,
    /// Blocks the starting derivation verified, recorded with the first Block this plan seals.
    verified: Option<Vec<ObjectId>>,
}

/// What a batch reads once.
pub(super) struct BatchReads {
    canonical_ref: String,
    /// Every stored recognition claim, enumerated at `claims_version`.
    claims: Vec<(ObjectId, RecognitionClaimPayload)>,
    claims_version: Option<ClaimContainerVersion>,
    /// The ref's history as of `tip`, extended by each block this batch publishes ([`SealBatchPlan::note_published`]).
    history: RefHistory,
    pub(super) tip: Option<ObjectId>,
}

/// The claim container's size and modification time: what tells a cheap `stat` that the container is the one the batch read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClaimContainerVersion {
    size: u64,
    mtime_secs: i64,
    mtime_nanos: u32,
}

fn claim_container_version(layout: &RepositoryLayout) -> Result<Option<ClaimContainerVersion>> {
    let container_path = layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    Ok(
        stat_file_state_if_exists(layout.repository_mutation_root(), &relative)?.map(|stat| {
            ClaimContainerVersion {
                size: stat.size,
                mtime_secs: stat.mtime_secs,
                mtime_nanos: stat.mtime_nanos,
            }
        }),
    )
}

struct PlannedStep {
    /// The patches this claim seals (its own, less what the ref's history already holds), in the claim's order.
    selected: Vec<ObjectId>,
    /// The root of the state after them, and (for a checkpoint) what its snapshot is written from.
    folded: crate::block_state::batch_fold::FoldedStep,
}

impl SealBatchPlan {
    fn empty() -> Self {
        Self {
            reads: None,
            steps: std::collections::VecDeque::new(),
            final_state: None,
            verified: None,
        }
    }

    /// Read the batch's two sets once: the stored claims and `ref_name`'s history. Errors are the ones the first per-batch check of the
    /// parent relationship always raised (`validate_local_branch_ref`, an unreadable tip, a damaged claim container), at the same place.
    ///
    /// # Errors
    ///
    /// An invalid ref name, a ref whose RefState or Block cannot be read, a history that cannot be walked, or a claim container that
    /// cannot be decoded.
    pub fn open(layout: &RepositoryLayout, ref_name: &str) -> Result<Self> {
        let canonical_ref = validate_local_branch_ref(ref_name)?;
        let object_store = ObjectReadSnapshot::open(layout)?;
        let ref_store = RefStore::new(layout.clone());
        let tip = read_current_tip(&object_store, &ref_store, &canonical_ref)?
            .map(|tip| tip.target_block_id);
        let history = ref_history(&object_store, tip)?;
        // The version is read before the claims: a claim written in between is read now and read again at the next check.
        let claims_version = claim_container_version(layout)?;
        let claims = enumerate_stored_claims(layout)?;
        Ok(Self {
            reads: Some(BatchReads {
                canonical_ref,
                claims,
                claims_version,
                history,
                tip,
            }),
            ..Self::empty()
        })
    }

    /// Re-read the stored claims **only if the claim container is not the one the batch read.** A `sync accept` cannot add a claim while
    /// a batch holds the active lock (it takes it for its claim write), but `sync build` signs and stores claims for the repository's *own*
    /// blocks under the object-store lock alone, so a claim can appear mid-batch. A per-claim enumeration saw it at the next claim; so
    /// does this, for the cost of one `stat` per claim and one re-read per change. (Containers are append-only: an unchanged size and
    /// modification time is an unchanged container.)
    ///
    /// # Errors
    ///
    /// A claim container that cannot be statted or decoded.
    pub fn refresh_claims(&mut self, layout: &RepositoryLayout) -> Result<()> {
        let Some(reads) = self.reads.as_mut() else {
            return Ok(());
        };
        let version = claim_container_version(layout)?;
        if version != reads.claims_version {
            reads.claims = enumerate_stored_claims(layout)?;
            reads.claims_version = version;
        }
        Ok(())
    }

    /// The batch's stored claims, if it has them.
    pub(super) fn claims(&self) -> Option<&[(ObjectId, RecognitionClaimPayload)]> {
        self.reads.as_ref().map(|reads| reads.claims.as_slice())
    }

    /// The batch's history, if it ends at exactly `tip` (the ref's tip the caller just read). A different tip means something moved the
    /// ref: the caller walks the history itself.
    pub(super) fn history_at(&self, tip: Option<ObjectId>) -> Option<&RefHistory> {
        self.reads
            .as_ref()
            .filter(|reads| reads.tip == tip)
            .map(|reads| &reads.history)
    }

    /// A block this batch just published: it is the ref's tip now, and its patches and the block itself join the history.
    pub(super) fn note_published(&mut self, block_id: ObjectId, selected: &[ObjectId]) {
        if let Some(reads) = self.reads.as_mut() {
            reads.tip = Some(block_id);
            reads.history.blocks.insert(block_id);
            reads.history.patches.extend(selected.iter().copied());
        }
    }

    /// Refuse the whole batch, with nothing written, at the first claim that builds on a block the ref's history does not reach (0.57.0
    /// step 0a): the batch is judged claim by claim in order against the ref's own history, each claim's patches joining it for the
    /// claims after. A claim the real loop would refuse for another reason (it does not exist, a patch is missing) ends the judgement with
    /// no opinion: the loop reports that, as it always has. The per-claim check inside the seal itself stays, as the backstop. It reads
    /// the sets [`Self::open`] read.
    ///
    /// # Errors
    ///
    /// The `precondition not met` refusal naming the claim, the parent and a way out; or a read error.
    pub fn require_parents_reached(
        &self,
        layout: &RepositoryLayout,
        ordered_claim_ids: &[ObjectId],
    ) -> Result<()> {
        let Some(reads) = self.reads.as_ref() else {
            return Ok(());
        };
        let object_store = ObjectReadSnapshot::open(layout)?;
        let ref_store = RefStore::new(layout.clone());
        let mut history = RefHistory {
            blocks: reads.history.blocks.clone(),
            patches: reads.history.patches.clone(),
        };
        let by_block = claims_by_block(&reads.claims);
        for claim_id in ordered_claim_ids {
            let Some((_, claim)) = reads.claims.iter().find(|(id, _)| id == claim_id) else {
                return Ok(());
            };
            if !claim
                .patch_ids
                .iter()
                .all(|id| object_store.contains_object(ObjectType::Patch, *id))
            {
                return Ok(());
            }
            let selected: Vec<ObjectId> = claim
                .patch_ids
                .iter()
                .copied()
                .filter(|id| !history.patches.contains(id))
                .collect();
            if selected.is_empty() {
                continue;
            }
            for &parent_id in &claim.parent_block_ids {
                if !parent_reached(&object_store, &by_block, &history, parent_id)? {
                    return Err(parent_not_reached_refusal(
                        layout,
                        &object_store,
                        &ref_store,
                        &by_block,
                        &reads.canonical_ref,
                        reads.tip,
                        *claim_id,
                        parent_id,
                    ));
                }
            }
            history.patches.extend(selected);
        }
        Ok(())
    }

    /// The planned root for the claim whose own selection is `selected`, if the plan is still the plan: the next step's selection must be
    /// exactly this one. Any other selection abandons the plan's fold (every claim after it seals from disk).
    pub(super) fn take_step_for(
        &mut self,
        selected: &[ObjectId],
    ) -> Option<(crate::block_state::batch_fold::FoldedStep, Vec<ObjectId>)> {
        let matches = self
            .steps
            .front()
            .is_some_and(|step| step.selected == selected);
        if !matches {
            self.steps.clear();
            return None;
        }
        let step = self.steps.pop_front()?;
        Some((step.folded, self.verified.take().unwrap_or_default()))
    }

    /// How many claims are still planned (for the controls).
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn planned_claims(&self) -> usize {
        self.steps.len()
    }

    /// Fold the batch once, before anything is written. Stops with no opinion at the first doubt, and leaves the plan with the steps it
    /// has by then.
    pub fn fold(&mut self, layout: &RepositoryLayout, ordered_claim_ids: &[ObjectId]) {
        if self.fold_inner(layout, ordered_claim_ids).is_none() {
            self.steps.clear();
            self.final_state = None;
            self.verified = None;
        }
    }

    fn fold_inner(
        &mut self,
        layout: &RepositoryLayout,
        ordered_claim_ids: &[ObjectId],
    ) -> Option<()> {
        self.refresh_claims(layout).ok()?;
        let reads = self.reads.as_ref()?;
        let read_snapshot = ObjectReadSnapshot::open(layout).ok()?;
        let tip = reads.tip;
        // The ref's own history, as the real loop selects against it (`seal_from_accepted_claim_impl`): a claim's patches the history
        // already holds are skipped, and each claim's selection joins the history for the claims after it.
        let mut patches = reads.history.patches.clone();
        let mut fold = crate::block_state::batch_fold::BatchFold::begin(
            layout,
            &read_snapshot,
            tip,
            StateAnchoring::Anchored(AnchorSite::SealFromAccepted),
        )
        .ok()?;
        let verified = fold.verified().to_vec();
        // Which claims' Blocks are checkpoints (they carry a snapshot of their own state, so their fold keeps its entries): tracked from
        // the starting tip as `checkpoint_due` reads it -- the distance back to the nearest snapshot, `None` once none lies within the
        // cadence.
        let mut snapshot_distance =
            crate::block_state::batch_fold::nearest_snapshot_within_cadence(&read_snapshot, tip)
                .ok()?;
        let mut steps = std::collections::VecDeque::new();
        for &claim_id in ordered_claim_ids {
            let Some(claim_envelope) = read_snapshot
                .read_typed(claim_id, ObjectType::RecognitionClaim)
                .ok()?
            else {
                break;
            };
            let Ok(claim) =
                RecognitionClaimPayload::decode_canonical(&claim_envelope.canonical_payload)
            else {
                break;
            };
            // A claim naming a patch this repository does not hold is the real loop's refusal to make, at that claim.
            if !claim
                .patch_ids
                .iter()
                .all(|patch_id| read_snapshot.contains_object(ObjectType::Patch, *patch_id))
            {
                break;
            }
            let selected: Vec<ObjectId> = claim
                .patch_ids
                .iter()
                .copied()
                .filter(|patch_id| !patches.contains(patch_id))
                .collect();
            if selected.is_empty() {
                // `AlreadySealed`: no Block, no step.
                continue;
            }
            // The real loop refuses an order-ambiguous claim before it derives anything; the plan stops where it would.
            if refuse_if_order_ambiguous_among(&reads.claims, claim_id, &claim).is_err() {
                break;
            }
            let checkpoint = snapshot_distance.is_none();
            let Ok(folded) = fold.fold(&read_snapshot, &selected, checkpoint) else {
                break;
            };
            // The Block this claim will seal becomes the new tip: it carries a snapshot if it is a checkpoint, else the nearest snapshot
            // is one block further back (and out of reach once it is the cadence away).
            snapshot_distance = if checkpoint {
                Some(1)
            } else {
                snapshot_distance
                    .map(|distance| distance + 1)
                    .filter(|distance| *distance < crate::snapshot::CHECKPOINT_CADENCE)
            };
            patches.extend(selected.iter().copied());
            steps.push_back(PlannedStep { selected, folded });
        }
        self.final_state = if steps.is_empty() {
            None
        } else {
            Some(fold.into_last_state())
        };
        self.steps = steps;
        self.verified = Some(verified);
        Some(())
    }
}

/// Read the batch and fold it once, before anything is written: [`SealBatchPlan::open`] and [`SealBatchPlan::fold`], with a plan that
/// could not be opened being an empty one (every check then reads as it always did).
#[must_use]
pub fn plan_seal_batch(
    layout: &RepositoryLayout,
    ref_name: &str,
    ordered_claim_ids: &[ObjectId],
) -> SealBatchPlan {
    match SealBatchPlan::open(layout, ref_name) {
        Ok(mut plan) => {
            plan.fold(layout, ordered_claim_ids);
            plan
        }
        Err(_) => SealBatchPlan::empty(),
    }
}

/// 0.56.0 step W3c-4 / 0.58.0 S2b: the pre-write in-the-way check for a whole `sync seal` batch onto the current branch, called once from
/// `prikk-cli`'s `run_seal` after [`crate::require_worktree_clean_and_capture`] and before the batch's first claim is sealed. It judges
/// the state the batch will publish, which the plan ([`plan_seal_batch`]) folded once: the state after the last claim that folded, the
/// exact tip the real loop will publish and then materialize. If no claim folded there is no state to check, and this skips itself.
///
/// **This check only ever *skips itself*, never refuses on its own simulation's behalf.** `sync.rs`'s module doc states §3 as a firm
/// policy -- "stop at the first failure... no rollback" -- so an earlier claim's valid seal is never pre-empted by a later claim's failure.
/// A genuine file-in-the-way collision is orthogonal to claim-vs-claim ordering (the worktree is not party to any claim), so refusing on
/// that basis never pre-empts a legitimate earlier seal. Before 0.58.0 this simulated the batch itself, re-deriving the whole grown list
/// for every claim; it now reads the plan's one fold.
pub fn require_nothing_in_the_way_for_seal_batch(
    layout: &RepositoryLayout,
    pending: &PendingCurrentBranchMaterialization,
    plan: &SealBatchPlan,
    action: &str,
) -> Result<()> {
    let Some((entries, text_cache)) = plan.final_state.as_ref() else {
        return Ok(());
    };
    // The manifest holds every file's bytes, so it is built here, when the check runs, and not when the plan is made. A fold the manifest
    // cannot be built from (a symlink in the state) has no opinion, as the simulation had.
    let Ok(read_snapshot) = ObjectReadSnapshot::open(layout) else {
        return Ok(());
    };
    // Bytes only where the worktree already holds a regular file: that is the only place `plan_materialization` compares them, and this
    // check discards its plan. A path with nothing in the worktree (a fresh receiver's whole tree) reads no blob.
    let worktree = layout.worktree_mutation_root();
    let Ok(target_manifest) =
        candidate_replay_manifest_wanting(&read_snapshot, entries.clone(), text_cache, |path| {
            matches!(
                crate::foundation::fsutil::inspect_entry(worktree, std::path::Path::new(path)),
                Ok(Some(crate::foundation::fsutil::EntryKind::Regular)) | Err(_)
            )
        })
    else {
        return Ok(());
    };
    require_nothing_in_the_way(layout, pending, &target_manifest, action, None)
}
