//! The batch of a `sync seal`: its plan (the fold of S2b and the reads of S3), and the pre-write in-the-way check that judges the state the
//! fold ends in. Split out of `seal_from_accepted.rs` (which the S3 reads would have pushed past the 1,200-line limit); everything here is
//! the batch-level half, built on that module's private per-claim helpers (`read_current_tip`, `ref_history`, `enumerate_stored_claims`,
//! `parent_reached`, ...), which a child module may use.

use prikk_error::Result;
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload};

use super::{
    ClaimsByBlock, RefHistory, enumerate_stored_claims, parent_not_reached_refusal, parent_reached,
    read_current_tip, ref_history, refuse_if_order_ambiguous_indexed,
};
use crate::anchor_trust::AnchorSite;
use crate::block_state::StateAnchoring;
use crate::foundation::fsutil::stat_file_state_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::merge::execute::candidate_replay_manifest_wanting;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::refs::{RefStore, validate_local_branch_ref};
use crate::worktree_materialize::{
    PendingCurrentBranchMaterialization, require_nothing_in_the_way_or_queued,
};

/// The batch, read and folded once (0.58.0 S2b and S3; RFC 159's mechanism with the batch's running state as an in-memory anchor).
///
/// **S2b, the fold (0.59.0 P5a1: one state at a time).** [`SealBatchPlan::fold`] derives the pre-batch state once and **selects** each
/// claim's patches (its own, less what the ref's history holds) in the batch's order. On the current branch it also runs an **apply-only**
/// pass over a copy of that state (no entries, no root), because the pre-write in-the-way check
/// ([`require_nothing_in_the_way_for_seal_batch`]) judges the state the batch will publish; on any other branch that pass is skipped, since
/// its only product is that state. The real loop ([`super::seal_from_accepted_claim_in_batch`]) advances a **second, running fold** one
/// claim at a time and seals each claim with the root it produces, writing a checkpoint's snapshot from that claim's state alone, so the
/// batch holds one state, never one per checkpoint (0.58.0 held them all: about 0.81 x claims^2 bytes in a tree that gains a path per claim).
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
    /// The fold the loop advances, one claim per [`Self::take_step_for`]; `None` when no claim is planned.
    running: Option<crate::block_state::batch_fold::BatchFold>,
    /// The state after the last claim that applied (its entries and text cache); `None` when no claim did, and **always `None` for a
    /// batch onto a branch that is not the current one** (the in-the-way check does not run there). The check turns it into the replay
    /// manifest it judges, only when it runs: that manifest holds every file's bytes.
    final_state: Option<FinalState>,
    /// Blocks the starting derivation verified, recorded with the first Block this plan seals.
    verified: Option<Vec<ObjectId>>,
    /// While the plan is alive the replay-verified record is not rewritten per sealed Block: the batch's ids are written once, when this
    /// ends ([`Self::finish_records`], or the plan's drop). 0.58.0 S6.
    records: Option<crate::verified_blocks::DeferredRecords>,
    /// Claims sealed with the root the plan's running fold produced, and claims sealed from disk with a plan present (`cfg(test)` and
    /// `test-support`): a batch whose claims all fold asserts the second is zero.
    #[cfg(any(test, feature = "test-support"))]
    sealed_counts: (usize, usize),
}

/// What a batch reads once.
pub(super) struct BatchReads {
    canonical_ref: String,
    /// Every stored recognition claim, enumerated at `claims_version`.
    claims: Vec<(ObjectId, RecognitionClaimPayload)>,
    /// The claims' positions by block, by patch and by id, built with `claims` (0.58.0 S6) and rebuilt only when they are re-read.
    index: ClaimsIndex,
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

/// Where each stored claim is, by block (the first claim for a block), by patch (every claim naming it, in stored order) and by id (the first
/// record of an id). Built once per claim set: a batch used to rebuild the by-block map and walk every stored claim for every claim it sealed.
pub(crate) struct ClaimsIndex {
    pub(crate) by_block: std::collections::BTreeMap<ObjectId, usize>,
    pub(super) by_patch: std::collections::BTreeMap<ObjectId, Vec<usize>>,
    pub(super) by_id: std::collections::BTreeMap<ObjectId, usize>,
}

#[cfg(test)]
std::thread_local! {
    static CLAIMS_INDEX_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// How many times a claims index was built on this thread (`cfg(test)`): the pin that a batch builds it once per claim set.
#[cfg(test)]
pub(crate) fn claims_index_builds_for_test() -> usize {
    CLAIMS_INDEX_BUILDS.with(std::cell::Cell::get)
}

impl ClaimsIndex {
    pub(crate) fn build(claims: &[(ObjectId, RecognitionClaimPayload)]) -> Self {
        #[cfg(test)]
        CLAIMS_INDEX_BUILDS.with(|builds| builds.set(builds.get() + 1));
        let mut by_block = std::collections::BTreeMap::new();
        let mut by_patch: std::collections::BTreeMap<ObjectId, Vec<usize>> =
            std::collections::BTreeMap::new();
        let mut by_id = std::collections::BTreeMap::new();
        for (position, (claim_id, claim)) in claims.iter().enumerate() {
            by_block.entry(claim.block_id).or_insert(position);
            by_id.entry(*claim_id).or_insert(position);
            for patch_id in &claim.patch_ids {
                let positions = by_patch.entry(*patch_id).or_default();
                if positions.last() != Some(&position) {
                    positions.push(position);
                }
            }
        }
        Self {
            by_block,
            by_patch,
            by_id,
        }
    }
}

/// The entries and text cache of a state: what the in-the-way check turns into the manifest it judges.
type FinalState = (
    Vec<crate::state_root::StateRootEntry>,
    crate::lifecycle_cache::replay::TextCache,
);

struct PlannedStep {
    /// The patches this claim seals (its own, less what the ref's history already holds), in the claim's order.
    selected: Vec<ObjectId>,
    /// This claim's Block is a checkpoint: it carries a snapshot of its own state, which the running fold keeps for it.
    checkpoint: bool,
}

impl SealBatchPlan {
    fn empty() -> Self {
        Self {
            reads: None,
            steps: std::collections::VecDeque::new(),
            running: None,
            final_state: None,
            verified: None,
            records: None,
            #[cfg(any(test, feature = "test-support"))]
            sealed_counts: (0, 0),
        }
    }

    /// Write the batch's replay-verified ids now (once) instead of when the plan is dropped. The batch calls it right after its last claim,
    /// before the worktree is written; a plan dropped on an error path writes them there.
    pub fn finish_records(&mut self) {
        self.records = None;
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
        let snapshot = ObjectReadSnapshot::open(layout)?;
        // 0.59.0 RS2b: the pointer is read after the snapshot opened: the ids it names are looked up through the fresh-on-miss reader.
        let object_store = crate::refs::PointerFreshReader::over(layout, &snapshot);
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
                index: ClaimsIndex::build(&claims),
                claims,
                claims_version,
                history,
                tip,
            }),
            records: Some(crate::verified_blocks::defer_records(layout)),
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
            reads.index = ClaimsIndex::build(&reads.claims);
            reads.claims_version = version;
        }
        Ok(())
    }

    /// The stored claims keyed by block, from the batch's index (built once per claim set).
    pub(super) fn claims_by_block(&self) -> Option<ClaimsByBlock<'_>> {
        self.reads
            .as_ref()
            .map(|reads| ClaimsByBlock::lent(&reads.claims, &reads.index.by_block))
    }

    /// The order-ambiguity check of `claim` against the batch's stored claims, through the index.
    pub(super) fn refuse_if_order_ambiguous(
        &self,
        claim_id: ObjectId,
        claim: &RecognitionClaimPayload,
    ) -> Result<()> {
        let Some(reads) = self.reads.as_ref() else {
            return Ok(());
        };
        refuse_if_order_ambiguous_indexed(&reads.claims, &reads.index.by_patch, claim_id, claim)
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
        let by_block = ClaimsByBlock::lent(&reads.claims, &reads.index.by_block);
        for claim_id in ordered_claim_ids {
            let Some((_, claim)) = reads
                .index
                .by_id
                .get(claim_id)
                .and_then(|&position| reads.claims.get(position))
            else {
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

    /// The root for the claim whose own selection is `selected`, if the plan is still the plan: the next step's selection must be exactly
    /// this one. Any other selection, and any claim the running fold cannot apply, abandons the plan (every claim after it seals from
    /// disk). **The running fold advances here**, one claim, reading the patches through `reader`.
    pub(super) fn take_step_for(
        &mut self,
        selected: &[ObjectId],
        reader: &impl ObjectReader,
    ) -> Option<(crate::block_state::batch_fold::FoldedStep, Vec<ObjectId>)> {
        let matches = self
            .steps
            .front()
            .is_some_and(|step| step.selected == selected);
        if !matches {
            self.abandon();
            return None;
        }
        let step = self.steps.pop_front()?;
        let folded = self
            .running
            .as_mut()
            .and_then(|running| running.fold(reader, &step.selected, step.checkpoint).ok());
        let Some(folded) = folded else {
            self.abandon();
            return None;
        };
        if self.steps.is_empty() {
            self.abandon();
        }
        Some((folded, self.verified.take().unwrap_or_default()))
    }

    /// Drop what the plan holds for the claims still to come: the steps (and their buffer, which popping does not shrink) and the
    /// running fold.
    fn abandon(&mut self) {
        self.steps = std::collections::VecDeque::new();
        self.running = None;
    }

    /// How many claims are still planned (for the controls).
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn planned_claims(&self) -> usize {
        self.steps.len()
    }

    /// How many claims this plan sealed from its running fold, and how many it sealed from disk (`cfg(test)` and `test-support`).
    #[cfg(any(test, feature = "test-support"))]
    #[must_use]
    pub fn sealed_counts(&self) -> (usize, usize) {
        self.sealed_counts
    }

    /// How many entries the state the in-the-way check will judge has (`cfg(test)`).
    #[cfg(test)]
    pub(super) fn final_state_entries(&self) -> Option<usize> {
        self.final_state.as_ref().map(|(entries, _)| entries.len())
    }

    /// The capacity the plan's steps buffer holds (`cfg(test)`): a popped `VecDeque` keeps its buffer, so a finished plan must hold none.
    #[cfg(test)]
    pub(super) fn steps_capacity(&self) -> usize {
        self.steps.capacity()
    }

    /// Count one claim sealed with the plan present (`planned`: from the running fold's root).
    #[cfg(any(test, feature = "test-support"))]
    pub(super) fn note_sealed(&mut self, planned: bool) {
        if planned {
            self.sealed_counts.0 += 1;
        } else {
            self.sealed_counts.1 += 1;
        }
    }

    /// Select the batch's claims once and, when `current_branch` (the in-the-way check will run), apply them once to a copy of the
    /// pre-batch state, before anything is written. Stops with no opinion at the first doubt, and leaves the plan with the steps it has by
    /// then.
    pub fn fold(
        &mut self,
        layout: &RepositoryLayout,
        ordered_claim_ids: &[ObjectId],
        current_branch: bool,
    ) {
        if self
            .fold_inner(layout, ordered_claim_ids, current_branch)
            .is_none()
        {
            self.abandon();
            self.final_state = None;
            self.verified = None;
        }
    }

    fn fold_inner(
        &mut self,
        layout: &RepositoryLayout,
        ordered_claim_ids: &[ObjectId],
        current_branch: bool,
    ) -> Option<()> {
        self.refresh_claims(layout).ok()?;
        let reads = self.reads.as_ref()?;
        let read_snapshot = ObjectReadSnapshot::open(layout).ok()?;
        let tip = reads.tip;
        // The ref's own history, as the real loop selects against it (`seal_from_accepted_claim_impl`): a claim's patches the history
        // already holds are skipped, and each claim's selection joins the history for the claims after it.
        let mut patches = reads.history.patches.clone();
        let fold = crate::block_state::batch_fold::BatchFold::begin(
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
            if refuse_if_order_ambiguous_indexed(
                &reads.claims,
                &reads.index.by_patch,
                claim_id,
                &claim,
            )
            .is_err()
            {
                break;
            }
            let checkpoint = snapshot_distance.is_none();
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
            steps.push_back(PlannedStep {
                selected,
                checkpoint,
            });
        }
        if steps.is_empty() {
            self.final_state = None;
            self.verified = Some(verified);
            return Some(());
        }
        if current_branch {
            // The running fold starts from a copy; the original runs ahead, apply-only, to the state the in-the-way check judges.
            let running = fold.fork();
            let (kept, final_state) = judged_state(layout, &read_snapshot, tip, fold, &steps)?;
            steps.truncate(kept);
            self.final_state = final_state;
            self.running = Some(running);
        } else {
            self.final_state = None;
            self.running = Some(fold);
        }
        self.steps = steps;
        self.verified = Some(verified);
        Some(())
    }
}

/// The state the in-the-way check judges, and how many of `steps` the plan keeps: **the state after the last claim that folded, and every claim
/// that folded**, as the 0.58.0 fold judged it. The quick way is one apply-only pass over every claim and one entries derivation with the
/// collision policy at the end. If a claim does not apply, or the final state collides, the quick way cannot say which claim to stop at, so
/// the same claims are folded again the exact way (each one's entries and collision policy, keeping the last good state): that is the 0.58.0
/// fold, run only for a batch with a claim that would not seal.
fn judged_state(
    layout: &RepositoryLayout,
    read_snapshot: &ObjectReadSnapshot,
    tip: Option<ObjectId>,
    mut fold: crate::block_state::batch_fold::BatchFold,
    steps: &std::collections::VecDeque<PlannedStep>,
) -> Option<(usize, Option<FinalState>)> {
    let applied = steps
        .iter()
        .try_for_each(|step| fold.apply(read_snapshot, &step.selected));
    if applied.is_ok() {
        if let Ok(final_state) = fold.into_final_state() {
            return Some((steps.len(), Some(final_state)));
        }
    }
    let mut exact = crate::block_state::batch_fold::BatchFold::begin(
        layout,
        read_snapshot,
        tip,
        StateAnchoring::Anchored(AnchorSite::SealFromAccepted),
    )
    .ok()?;
    let mut kept = 0;
    let mut last = None;
    for step in steps {
        let Ok(folded) = exact.fold(read_snapshot, &step.selected, true) else {
            break;
        };
        last = folded.snapshot_inputs.map(|state| state.into_parts());
        kept += 1;
    }
    Some((kept, last))
}

/// Read the batch and fold it once, before anything is written: [`SealBatchPlan::open`] and [`SealBatchPlan::fold`], with a plan that
/// could not be opened being an empty one (every check then reads as it always did).
#[must_use]
pub fn plan_seal_batch(
    layout: &RepositoryLayout,
    ref_name: &str,
    ordered_claim_ids: &[ObjectId],
    current_branch: bool,
) -> SealBatchPlan {
    match SealBatchPlan::open(layout, ref_name) {
        Ok(mut plan) => {
            plan.fold(layout, ordered_claim_ids, current_branch);
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
    // The batch refuses any non-empty active queue itself (`requires an empty active WAL`), so the queue rule is not asked here.
    require_nothing_in_the_way_or_queued(layout, pending, &target_manifest, action, None, false)
}
