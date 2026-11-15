//! RFC 115 Stage 4 (`stage-4-seal-from-accepted-handoff-v1.md`) — sealing what you accepted.
//!
//! **One claim, one block (§1).** A `RecognitionClaim` already describes exactly one block: its
//! `block_id`, and since the D6 order amendment its `patch_ids` verbatim, in the sequence the
//! sender's own block actually sealed them in. This module reconstructs a *local equivalent* of
//! that block -- the same patches, in the same order, on the receiver's own parent, sealed under
//! the receiver's own maintainer key.
//!
//! **The resulting block id will not equal the claim's `block_id`, and that is correct, not a
//! failure.** A different parent yields a different state root yields a different id. RFC 115
//! §2.4-§2.7 already rules that blocks diverge between repositories by design, and that this loses
//! nothing, because identity lives at the patch level, not the block level. If you are looking at
//! this module because two block ids disagree, that disagreement is not the bug.
//!
//! **The order is a hint that must be tried, never a fact that is trusted (D6 §11.6).** A claim
//! never gates admission and never confers trust; sealing under it does not change that. The
//! receiver applies the claimed order and either it produces a valid state -- which the receiver
//! then seals under their **own** key, `verify_signer_trusted` unchanged and still gating -- or it
//! does not, and the seal is refused. A hostile or simply wrong order cannot forge a state; it can
//! only produce a different valid application or none at all. Consequently an `Unverifiable` claim
//! (§3) may still supply an order -- refusing on that would make this path useless on first
//! contact, where no maintainer key material travels in a `PEXCH001` artifact and every claim is
//! therefore permanently `Unverifiable`.
//!
//! **Divergence is not corruption (§4).** This is the first place prikk applies patches that were
//! not authored against the state they are being applied to. Every other lifecycle-replay caller
//! serves sealed-history replay, where a patch definitionally applied cleanly when authored; an
//! accepted-but-unsealed patch failing to apply here is an ordinary divergence between two
//! histories that moved differently, not a broken repository, and must be reported as such --
//! see [`classify_patch_application_failure`].
//!
//! **A sealed block may carry fewer patches than the claim names (D7, design-v1.md §12).** For
//! every patch a claim names, its state in this repository is one of three: sealed (its effect is
//! already in this repository's state -- skip it), present-and-unsealed (seal it -- this is the
//! work), or absent (refuse the whole claim). Only the unsealed subset is applied and sealed, in
//! the claim's own order restricted to that subset -- never sorted, never deduplicated. Skipping an
//! already-sealed patch does not skip its effect; re-applying it would be wrong. Blocks already
//! diverge between repositories by design (§2.4-§2.7 above); state converges. If you are looking at
//! this module because a sealed block's `patch_ids` does not equal the claim's own, that is not a
//! bug.
//!
//! **"Already sealed" and "builds on" are judged against the target ref's own history (0.57.0
//! step 0a).** A patch is skipped as already sealed only when a block reachable from the target
//! ref's current tip carries it; a block some *other* ref sealed does not count, so the answer
//! never says a ref holds something it does not. And a claim whose `parent_block_ids` name a block
//! the target ref's tip does not reach is refused before anything is written
//! ([`parent_not_reached_refusal`]): sealing it anyway puts the patch on whatever the tip happens
//! to be and silently drops the history the claim was made on (measured on 0.56.0: a branch
//! carried to a repository that did not hold it came out with only its own file). "Reaches" is
//! judged at the patch level, the level this module's identity lives at, because block ids differ
//! between repositories by design: a parent counts as reached when its id is in the tip's history,
//! or when every patch of its own ancestry that this repository can name (from the block itself
//! when held, else from the claims held for it) is already in the tip's history. A parent that
//! is neither a held block nor the subject of a held claim cannot be judged and is not reached.
//!
//! **Out of scope (§7):** resolving divergence (refuse and report; that is `merge`'s job),
//! transport, and CLI wiring.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RecognitionClaimPayload, RefKind,
    RefStatePayload, RefUpdatePayload,
};

use crate::anchor_trust::AnchorSite;
use crate::block_state::{
    BlockLineage, CandidateStateDerivationError, StateAnchoring, seal_block_classified,
};
use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{
    ContainerSlot, DEFAULT_ACTIVE_NAME, RepositoryLayout, persisted_object_types,
};
use crate::lifecycle_cache::replay::LifecycleReplayError;
use crate::lock::ActiveLock;
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::merge::evidence::ancestors_inclusive;
use crate::object_store::{ObjectReadSnapshot, ObjectReader, ObjectWriteSession};
use crate::recognition_claim::{ClaimSignatureVerification, verify_claim_signature};
use crate::refs::{RefPublication, RefStore, validate_local_branch_ref};
use crate::trust::{GatedOperation, load_maintainer_trust_policy_or_empty, verify_signer_trusted};
use crate::wal::Wal;

/// The result of [`seal_from_accepted_claim`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SealFromAcceptedOutcome {
    /// A new Block was sealed under the receiver's own key, from the claim's patches in the
    /// claim's own verbatim order.
    Sealed {
        /// The ref advanced.
        ref_name: String,
        /// The newly sealed Block's id. Will not equal the claim's own `block_id` -- see the
        /// module doc.
        block_id: ObjectId,
        /// The newly published RefState's id.
        ref_state_id: ObjectId,
        /// Patches sealed into the new block, in the order they were applied -- possibly fewer
        /// than the claim named, when some of its patches were already sealed here (D7).
        patch_count: usize,
        /// The selected claim's own MAINTAINER signature outcome (design D6 §11.6: reported, never
        /// gating -- an `Unverifiable` claim can still have supplied the order just sealed).
        claim_signature_outcome: ClaimSignatureVerification,
    },
    /// Every patch the named claim describes was already sealed. No new block was written --
    /// replaying an already-completed seal is a no-op success (§3.3, §6 row 9), not an error.
    AlreadySealed {
        /// The ref that was named.
        ref_name: String,
        /// The claim that named already-sealed patches.
        claim_id: ObjectId,
    },
}

/// Seal the patches named by the `RecognitionClaim` at `claim_id` into `ref_name`, under `signer`.
/// See the module doc for the shape of what this does and does not guarantee. Acquires its own
/// `ActiveLock` for the one claim, held only for this call -- see
/// [`seal_from_accepted_claim_in_batch`] for the batch shape that holds one lock across several
/// calls instead.
pub fn seal_from_accepted_claim(
    layout: &RepositoryLayout,
    ref_name: &str,
    claim_id: ObjectId,
    signer: &impl MaintainerSigner,
) -> Result<SealFromAcceptedOutcome> {
    seal_from_accepted_claim_impl(layout, None, ref_name, claim_id, signer, None)
}

/// 0.56.0 step W3c-4: as [`seal_from_accepted_claim`], but for one claim inside a `sync seal`
/// batch that already holds `active_lock` for the *whole* batch (`prikk-cli`'s
/// `run_seal`'s `SealClaims::Batch` arm) -- `ActiveLock::acquire_for_write` is not reentrant (an
/// exclusive-file-creation lock), so a batch cannot call the single-claim function once per claim
/// the way it did before this round without releasing and re-acquiring the lock between every
/// claim, which is exactly the window the batch's own clean-check-once/marker-once/
/// materialize-once R3 shape must not have. `active_lock` is a witness only -- this function reads
/// nothing from it and does not drop it; the caller's own guard keeps the lock held across the
/// whole batch, including this call.
pub fn seal_from_accepted_claim_in_batch(
    layout: &RepositoryLayout,
    active_lock: &ActiveLock,
    ref_name: &str,
    claim_id: ObjectId,
    signer: &impl MaintainerSigner,
    plan: &mut SealBatchPlan,
) -> Result<SealFromAcceptedOutcome> {
    seal_from_accepted_claim_impl(
        layout,
        Some(active_lock),
        ref_name,
        claim_id,
        signer,
        Some(plan),
    )
}

/// S2a (0.58.0): a crash point for the real-kill tests of `sync seal`. With `prikk-store`'s `test-support` feature (which no shipped
/// build enables) and `PRIKK_TEST_ABORT_AT=<point>:<k>` set, the process aborts, with no cleanup (a `SIGKILL`'s disk state), at `point`
/// (`before-block`, `after-block`, `after-publish`) of the k-th claim this process seals. In every other build it is an empty function.
#[cfg(feature = "test-support")]
fn crash_point(point: &str) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    static CLAIM: AtomicUsize = AtomicUsize::new(0);
    let Ok(spec) = std::env::var("PRIKK_TEST_ABORT_AT") else {
        return;
    };
    let Some((want, k)) = spec.split_once(':') else {
        return;
    };
    if point == "before-block" {
        CLAIM.fetch_add(1, Ordering::SeqCst);
    }
    if want == point && k.parse::<usize>().ok() == Some(CLAIM.load(Ordering::SeqCst)) {
        std::process::abort();
    }
}

/// The shipped form: nothing.
#[cfg(not(feature = "test-support"))]
#[inline(always)]
fn crash_point(_point: &str) {}

/// Shared body for [`seal_from_accepted_claim`] and [`seal_from_accepted_claim_in_batch`]: every
/// check and the seal itself, unchanged from before this round, except that the `ActiveLock` is
/// either acquired here (`held_lock` is `None`, the single-claim entry point's own shape) or
/// supplied by the caller (`held_lock` is `Some`, the batch shape) -- the one thing this round
/// needed to change about this function's own lock scope.
fn seal_from_accepted_claim_impl(
    layout: &RepositoryLayout,
    held_lock: Option<&ActiveLock>,
    ref_name: &str,
    claim_id: ObjectId,
    signer: &impl MaintainerSigner,
    mut plan: Option<&mut SealBatchPlan>,
) -> Result<SealFromAcceptedOutcome> {
    layout.require_current_format()?;
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    crate::refs::ensure_no_incomplete_publication(layout).map_err(|error| {
        // 019 §5.2: name `ref complete` only when the refusal is a genuine N3 lead.
        if matches!(&error, PrikkError::IncompletePublication(_)) {
            crate::ref_completion::incomplete_publication_refusal_naming_any_lead(layout, None)
                .unwrap_or(error)
        } else {
            error
        }
    })?;
    // RFC 136 §10.3b.3: the derivation gate, before any write.
    crate::worktree_marker::ensure_worktree_replay_verified(layout)?;

    // §3 item 1: the named claim must exist and decode.
    let read_snapshot = ObjectReadSnapshot::open(layout)?;
    let claim_envelope = read_snapshot
        .read_typed(claim_id, ObjectType::RecognitionClaim)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!("recognition claim {claim_id} does not exist"))
        })?;
    let claim = RecognitionClaimPayload::decode_canonical(&claim_envelope.canonical_payload)?;

    // §3 item 2: every named patch must be present. No partial application -- the same rule
    // Stage 3's §8.4 closure check enforces.
    for &patch_id in &claim.patch_ids {
        if !read_snapshot.contains_object(ObjectType::Patch, patch_id) {
            return Err(PrikkError::Integrity(format!(
                "recognition claim {claim_id} names patch {patch_id}, which does not exist -- \
                 refusing the whole seal, no partial application"
            )));
        }
    }

    // §3 item 3 (D7, design-v1.md §12): a named patch already sealed here has its effect already
    // in this repository's state -- skip it, do not refuse the claim over it. Only the unsealed
    // subset is the work, kept in the claim's own verbatim order (D6) by filtering in place --
    // never sorted, never deduplicated; restricting a total order to a subset is well-defined. An
    // empty subset is the degenerate case: every named patch is already sealed, so this is a no-op
    // success, not an error (§8.7/§6 row 9).
    //
    // 0.57.0 step 0a: "sealed" means sealed in *the target ref's own history* (blocks reachable
    // from its current tip), not in any block this repository holds -- a patch another ref sealed
    // is still work for this one. This first read is the cheap, lock-free answer for the no-op
    // case; the authoritative selection is made again under the lock below, against the tip read
    // there.
    {
        let ref_store = RefStore::new(layout.clone());
        let tip = read_current_tip(&read_snapshot, &ref_store, &canonical_ref)?
            .map(|tip| tip.target_block_id);
        // 0.58.0 S3: a batch holds the ref's history already, ending at the tip it expects; a different tip (something moved the ref)
        // is walked here, as every claim used to be.
        let walked;
        let history = match plan.as_deref().and_then(|plan| plan.history_at(tip)) {
            Some(history) => history,
            None => {
                walked = ref_history(&read_snapshot, tip)?;
                &walked
            }
        };
        if claim
            .patch_ids
            .iter()
            .all(|patch_id| history.patches.contains(patch_id))
        {
            return Ok(SealFromAcceptedOutcome::AlreadySealed {
                ref_name: canonical_ref,
                claim_id,
            });
        }
    }

    // §3 item 4: if a stored claim overlaps this one's patch set and disagrees on their relative
    // order, refuse rather than guess. Checked only once we know there is real sealing work to do
    // -- ambiguity about an order that will not be applied is moot.
    // 0.58.0 S3: the batch's stored claims (re-read only if the claim container changed since it read them), else the container as ever.
    match plan.as_deref_mut() {
        Some(plan) if plan.reads.is_some() => {
            plan.refresh_claims(layout)?;
            plan.refuse_if_order_ambiguous(claim_id, &claim)?;
        }
        _ => refuse_if_order_ambiguous(layout, claim_id, &claim)?,
    }

    // §6 row 1: sealing requires a locally trusted signer. Checked before any lock or WAL read --
    // an untrusted signer must refuse cheaply, the same fail-fast shape Stage 3 gives its own
    // early checks.
    verify_signer_trusted(layout, signer, GatedOperation::SyncSeal)?;

    let trust_policy = load_maintainer_trust_policy_or_empty(layout)?;
    let claim_signature_outcome = verify_claim_signature(&claim_envelope, &trust_policy)?;

    let owned_lock = match held_lock {
        Some(_) => None,
        None => Some(ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?),
    };
    crate::refs::ensure_no_incomplete_publication(layout).map_err(|error| {
        // 019 §5.2: name `ref complete` only when the refusal is a genuine N3 lead.
        if matches!(&error, PrikkError::IncompletePublication(_)) {
            crate::ref_completion::incomplete_publication_refusal_naming_any_lead(layout, None)
                .unwrap_or(error)
        } else {
            error
        }
    })?;

    // §5: the active WAL must be empty. Sealing accepted patches advances the branch tip; locally
    // queued WAL patches were composed against the *old* tip, and since DC-66 a queue chains
    // baselines, so advancing the tip underneath them invalidates assumptions they were built on --
    // the same reasoning `rollback_draft` already applies for exactly this class of reason.
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    // RFC 166 round 2 §0: the commit-witness classification speaks first, before the older
    // tail/damage checks below. `owning_ref` is read fresh, not assumed `Missing` -- this function's
    // own "WAL must be empty" requirement is checked separately, below, and a non-empty WAL here can
    // genuinely have valid ownership (e.g. unrelated local queued work); assuming `Missing` would
    // misfire row 9 (`OwnershipMissing`) on exactly that case.
    let owning_ref_now = crate::commit_boundary::active::read_active_ref_metadata(layout)?;
    {
        let witness = crate::commit_boundary::witness::read_witness(layout, DEFAULT_ACTIVE_NAME)?;
        let verdict = crate::commit_boundary::classification::classify(
            layout,
            &replay,
            &owning_ref_now,
            &witness,
        )?;
        if let Some(reason) = crate::commit_boundary::classification::write_refusal_reason(
            &verdict,
            Some(
                crate::commit_boundary::classification::RestoreRefusalContext {
                    layout,
                    witness: &witness,
                    queued_count: replay.records.len(),
                },
            ),
        ) {
            return Err(PrikkError::Integrity(reason));
        }
    }
    if replay.trailing_partial_bytes != 0 {
        return Err(PrikkError::Integrity(format!(
            "active WAL has {} trailing partial bytes; run verify/doctor before sealing from \
             an accepted claim",
            replay.trailing_partial_bytes
        )));
    }
    if replay.has_item_failure() {
        return Err(PrikkError::Integrity(
            "active WAL has a damaged record; run verify/doctor before sealing from an accepted \
             claim"
                .to_string(),
        ));
    }
    if !replay.records.is_empty() {
        // RFC 132 part 2: a non-empty active WAL is a caller precondition here, not a lock --
        // nothing is held and no other writer is racing this one; the caller must seal or discard
        // local work first, not wait.
        return Err(PrikkError::Precondition(
            "sealing from an accepted claim requires an empty active WAL -- seal or discard \
             local work first"
                .to_string(),
        ));
    }

    let ref_store = RefStore::new(layout.clone());
    // RFC 164 Rule D: the pointer index's own tail check, before the new Block write just below
    // (`seal_block_classified`) -- previously this ref's own pointer-index tail was not checked
    // until `publish_with_object_store`'s own, later `Ready`-branch-only call, by which point the
    // new Block was already durably written, for nothing. `read_current_tip` below already reads
    // the pointer index too (and so already refuses on damage via `has_item_failure()`); this call
    // adds the tail half.
    ref_store.ensure_pointer_index_has_no_tail(&canonical_ref)?;
    let current = read_current_tip(&read_snapshot, &ref_store, &canonical_ref)?;
    let parent = current.as_ref().map(|tip| tip.target_block_id);
    // 0.58.0 S3: the batch's history, when it ends at the tip just read; the claims, refreshed if the container changed.
    if !claim.parent_block_ids.is_empty() {
        if let Some(plan) = plan.as_deref_mut() {
            plan.refresh_claims(layout)?;
        }
    }
    let selected_patch_ids: Vec<ObjectId> = {
        let walked;
        let history = match plan.as_deref().and_then(|plan| plan.history_at(parent)) {
            Some(history) => history,
            None => {
                walked = ref_history(&read_snapshot, parent)?;
                &walked
            }
        };
        let selected: Vec<ObjectId> = claim
            .patch_ids
            .iter()
            .copied()
            .filter(|patch_id| !history.patches.contains(patch_id))
            .collect();
        if selected.is_empty() {
            // Another writer sealed it between the first read and the lock: nothing left to do.
            return Ok(SealFromAcceptedOutcome::AlreadySealed {
                ref_name: canonical_ref,
                claim_id,
            });
        }
        // 0.57.0 step 0a: refuse before any write when the claim builds on history this ref's tip
        // does not reach (see the module doc).
        if !claim.parent_block_ids.is_empty() {
            let enumerated;
            let by_block = match plan.as_deref().and_then(SealBatchPlan::claims_by_block) {
                Some(by_block) => by_block,
                None => {
                    enumerated = enumerate_stored_claims(layout)?;
                    claims_by_block(&enumerated)
                }
            };
            for &parent_id in &claim.parent_block_ids {
                if !parent_reached(&read_snapshot, &by_block, history, parent_id)? {
                    return Err(parent_not_reached_refusal(
                        layout,
                        &read_snapshot,
                        &ref_store,
                        &by_block,
                        &canonical_ref,
                        parent,
                        claim_id,
                        parent_id,
                    ));
                }
            }
        }
        selected
    };

    // §4 and Phase D: seal the new Block under the receiver's own key, carrying the claim's own
    // order restricted to the unsealed subset, verbatim (D7, §6 row 11). The derivation is the last
    // check and nothing is written unless it passes; its failure keeps `LifecycleReplayError`'s own
    // variant alive to the classification point rather than flattening it into an undifferentiated
    // `PrikkError::Integrity` the way ordinary `seal_block` does.
    crash_point("before-block");
    let mut object_store = ObjectWriteSession::open(layout)?;
    // 0.58.0 S2b: the root this claim's Block carries is the one the batch's single fold produced, when the plan still holds it (its next
    // step's selection is exactly this claim's) and no checkpoint is due (that Block carries a snapshot of its own state, which the plan
    // does not hold). Otherwise this claim is sealed from disk exactly as it was before, with the same checks and the same messages.
    let planned = plan
        .as_deref_mut()
        .and_then(|plan| plan.take_step_for(&selected_patch_ids));
    let folded_block = match planned {
        Some((root, verified)) => crate::block_state::batch_fold::seal_block_with_folded_root(
            layout,
            &mut object_store,
            parent,
            &selected_patch_ids,
            root,
            signer,
            verified,
        )?,
        None => None,
    };
    let block_id = match folded_block {
        Some(block_id) => block_id,
        None => seal_block_classified(
            layout,
            &mut object_store,
            BlockLineage::Linear { parent },
            &selected_patch_ids,
            signer,
            StateAnchoring::Anchored(AnchorSite::SealFromAccepted),
            |err| match err {
                CandidateStateDerivationError::Lineage(err) => err,
                CandidateStateDerivationError::Patch(err) => {
                    classify_patch_application_failure(err)
                }
            },
        )?,
    };

    crash_point("after-block");
    // AUD-08: untested here, deliberately -- unlike `seal`/`execute_merge`, this function's own
    // `ensure_no_incomplete_publication(layout)` call above (line 108) already refuses the only way
    // to make `current.update_seq` reach `u64::MAX` (repointing `canonical_ref` at a RefState the
    // real ref-log cannot support), so a test proving this arm would have to route around that
    // pre-existing, unrelated check -- exactly the "controls that pass for the wrong reason" shape
    // this project has been burned by before. The three sites share one fix for one reason
    // (`update_seq + 1` should never silently wrap), not one reachability story.
    let update_seq = match current.as_ref() {
        Some(tip) => tip.update_seq.checked_add(1).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "seal-from-accepted-claim {canonical_ref}: ref-state update_seq overflow"
            ))
        })?,
        None => 1,
    };
    let previous_ref_state_id = current.as_ref().map(|tip| tip.ref_state_id);
    let ref_state_payload = RefStatePayload {
        ref_name: canonical_ref.clone(),
        kind: RefKind::Branch,
        target_object_id: block_id,
        update_seq,
        previous_ref_state_id,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
        signer,
    )?;
    let ref_state_id = ref_state_envelope.object_id();

    let ref_update_payload = RefUpdatePayload {
        ref_name: canonical_ref.clone(),
        old_ref_state_id: previous_ref_state_id,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        signer,
    )?;

    let published_ref_state_id = ref_store.publish_with_object_store(
        &mut object_store,
        &RefPublication {
            ref_name: canonical_ref.clone(),
            expected_previous_ref_state_id: previous_ref_state_id,
            ref_state: ref_state_envelope,
            ref_update: ref_update_envelope,
        },
    )?;
    crash_point("after-publish");
    // 0.58.0 S3: the Block just published is the ref's tip now, and its patches and the Block itself join the batch's history.
    if let Some(plan) = plan {
        plan.note_published(block_id, &selected_patch_ids);
    }
    drop(owned_lock);

    Ok(SealFromAcceptedOutcome::Sealed {
        ref_name: canonical_ref,
        block_id,
        ref_state_id: published_ref_state_id,
        patch_count: selected_patch_ids.len(),
        claim_signature_outcome,
    })
}

/// §4's ruled classification, applied to the one `LifecycleReplayError` variant
/// [`derive_next_state_for_candidate`] can actually surface from applying candidate patches
/// (the lineage-walk variants are impossible here -- they belong to
/// `CandidateStateDerivationError::Lineage`, already handled by the caller before this function is
/// reached -- but the match stays exhaustive over every variant so the classification is total, not
/// contingent on which variants are reachable today).
fn classify_patch_application_failure(error: LifecycleReplayError) -> PrikkError {
    match error {
        LifecycleReplayError::InconsistentLifecycleEffect { .. }
        | LifecycleReplayError::TextSpanResolutionFailed { .. } => PrikkError::Integrity(format!(
            "seal refused: divergence -- an accepted patch did not apply cleanly to this \
             repository's own tip ({error}); the two histories moved differently, nothing is \
             corrupt, and resolving this is merge's job, not this operation's"
        )),
        LifecycleReplayError::MissingBlockInLineage { .. }
        | LifecycleReplayError::UnreadableBlockInLineage { .. }
        | LifecycleReplayError::MergeLineageUnsupported { .. }
        | LifecycleReplayError::LineageCycle { .. }
        | LifecycleReplayError::HorizonNotInLineage { .. }
        | LifecycleReplayError::MalformedPatchInLineage { .. }
        // 0.56.0 step 3d (A6) added this variant for the six readers it names (verify, doctor, show,
        // tree, extract, commit); this candidate-patch-application path is not one of them and is out
        // of that round's scope, so it is bucketed here unchanged rather than re-litigated.
        | LifecycleReplayError::OutsideVersionRuleInLineage { .. }
        | LifecycleReplayError::MissingBlobForLifecycleEffect { .. } => PrikkError::Integrity(
            format!("seal refused: integrity -- this repository's own state is broken ({error})"),
        ),
    }
}

mod batch;
pub use batch::{SealBatchPlan, plan_seal_batch, require_nothing_in_the_way_for_seal_batch};

struct CurrentTip {
    ref_state_id: ObjectId,
    target_block_id: ObjectId,
    update_seq: u64,
}

/// `None` when `ref_name` is not yet published -- the genesis case, mirroring `seal`'s own
/// `current: Option<CurrentRefState>`. Does not attempt seal's own interrupted-publication recovery
/// dance; `ensure_no_incomplete_publication` above already refuses a repository in that state
/// before this is reached.
fn read_current_tip(
    object_store: &impl ObjectReader,
    ref_store: &RefStore,
    ref_name: &str,
) -> Result<Option<CurrentTip>> {
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(ref_name)? else {
        return Ok(None);
    };
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {ref_name} points to missing RefState {ref_state_id}"
            ))
        })?;
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if payload.ref_name != ref_name {
        return Err(PrikkError::Integrity(format!(
            "RefState name mismatch for {ref_name}: got {}",
            payload.ref_name
        )));
    }
    if object_store
        .read_typed(payload.target_object_id, ObjectType::Block)?
        .is_none()
    {
        return Err(PrikkError::Integrity(format!(
            "ref {ref_name} targets missing block {}",
            payload.target_object_id
        )));
    }
    Ok(Some(CurrentTip {
        ref_state_id,
        target_block_id: payload.target_object_id,
        update_seq: payload.update_seq,
    }))
}

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, object_type, object_id)?)?;
    Ok(envelope)
}

/// §3 item 4: refuse if any *other* stored claim names a patch set overlapping `selected`'s and
/// disagrees on the relative order of the overlap. Scoped to claims relevant to the one being
/// sealed, not an all-pairs check across every stored claim -- an unrelated pair of claims
/// disagreeing about patches this operation is not touching is not this operation's ambiguity to
/// refuse on.
fn refuse_if_order_ambiguous(
    layout: &RepositoryLayout,
    selected_claim_id: ObjectId,
    selected: &RecognitionClaimPayload,
) -> Result<()> {
    refuse_if_order_ambiguous_among(
        &enumerate_stored_claims(layout)?,
        selected_claim_id,
        selected,
    )
}

/// [`refuse_if_order_ambiguous`] against a set of stored claims the caller already holds (a batch reads them once, 0.58.0 S3).
fn refuse_if_order_ambiguous_among(
    claims: &[(ObjectId, RecognitionClaimPayload)],
    selected_claim_id: ObjectId,
    selected: &RecognitionClaimPayload,
) -> Result<()> {
    refuse_if_order_ambiguous_over(claims.iter(), selected_claim_id, selected)
}

/// The same check over the stored claims that name at least one of `selected`'s patches, found through the batch's index (0.58.0 S6): the
/// claims are walked in their stored order, as the whole set is, so the first disagreeing claim -- the one the refusal names -- is the same.
pub(super) fn refuse_if_order_ambiguous_indexed(
    claims: &[(ObjectId, RecognitionClaimPayload)],
    by_patch: &BTreeMap<ObjectId, Vec<usize>>,
    selected_claim_id: ObjectId,
    selected: &RecognitionClaimPayload,
) -> Result<()> {
    let positions: BTreeSet<usize> = selected
        .patch_ids
        .iter()
        .filter_map(|patch_id| by_patch.get(patch_id))
        .flatten()
        .copied()
        .collect();
    refuse_if_order_ambiguous_over(
        positions
            .into_iter()
            .filter_map(|position| claims.get(position)),
        selected_claim_id,
        selected,
    )
}

fn refuse_if_order_ambiguous_over<'a>(
    claims: impl Iterator<Item = &'a (ObjectId, RecognitionClaimPayload)>,
    selected_claim_id: ObjectId,
    selected: &RecognitionClaimPayload,
) -> Result<()> {
    let selected_set: BTreeSet<ObjectId> = selected.patch_ids.iter().copied().collect();
    for (other_id, other) in claims {
        let other_id = *other_id;
        if other_id == selected_claim_id {
            continue;
        }
        let overlaps = other.patch_ids.iter().any(|id| selected_set.contains(id));
        if !overlaps {
            continue;
        }
        if !orders_agree_on_overlap(&selected.patch_ids, &other.patch_ids) {
            return Err(PrikkError::Integrity(format!(
                "recognition claims {selected_claim_id} and {other_id} name overlapping patches \
                 but disagree on their relative order -- refusing rather than guessing which is \
                 right"
            )));
        }
    }
    Ok(())
}

/// Whether `a` and `b` agree on the relative order of every patch id they both name. Walks `a` in
/// `a`'s own order, checking that the subsequence of ids also present in `b` is non-decreasing in
/// `b`'s own index order -- symmetric by construction (the same pairwise relation holds regardless
/// of which side is walked). A duplicated id within one side is compared by its first occurrence,
/// the conservative choice for a field D6 permits duplicates in but no real construction path
/// (`seal`'s own WAL-to-block path, this module's own) ever produces one for.
fn orders_agree_on_overlap(a: &[ObjectId], b: &[ObjectId]) -> bool {
    let mut b_index: BTreeMap<ObjectId, usize> = BTreeMap::new();
    for (index, id) in b.iter().enumerate() {
        b_index.entry(*id).or_insert(index);
    }
    let mut last_seen: Option<usize> = None;
    for id in a {
        if let Some(&index) = b_index.get(id) {
            if let Some(last) = last_seen {
                if index < last {
                    return false;
                }
            }
            last_seen = Some(index);
        }
    }
    true
}

/// Enumerate every stored `RecognitionClaim`, decoded, the same container-walk shape
/// `accepted_but_unsealed_patch_ids` (`patch_exchange.rs`) already uses for `Patch` --
/// `persisted_object_types()` -> container -> `decode_container_records`, narrowed here to
/// `ObjectType::RecognitionClaim`.
/// What one ref's own history holds: the blocks reachable from its tip (inclusive) and every patch
/// those blocks carry. `None` (an unborn ref) is the empty history.
struct RefHistory {
    blocks: BTreeSet<ObjectId>,
    patches: BTreeSet<ObjectId>,
}

fn ref_history(object_store: &impl ObjectReader, tip: Option<ObjectId>) -> Result<RefHistory> {
    #[cfg(test)]
    BATCH_READS.with(|reads| reads.set((reads.get().0, reads.get().1 + 1)));
    let mut history = RefHistory {
        blocks: BTreeSet::new(),
        patches: BTreeSet::new(),
    };
    if let Some(tip) = tip {
        for (block_id, block) in ancestors_inclusive(object_store, tip)? {
            history.blocks.insert(block_id);
            history.patches.extend(block.patch_ids.iter().copied());
        }
    }
    Ok(history)
}

/// The held claims keyed by the block they are about (the first claim for a block is kept: claims
/// about one block carry the same patches and parents).
fn claims_by_block(claims: &[(ObjectId, RecognitionClaimPayload)]) -> ClaimsByBlock<'_> {
    ClaimsByBlock {
        claims,
        index: std::borrow::Cow::Owned(batch::ClaimsIndex::build(claims).by_block),
    }
}

/// A block id -> (the id of a held claim about it, that claim's payload): the held claims and each block's position among them. A batch
/// builds the positions once per claim set (0.58.0 S6, [`batch::ClaimsIndex`]) and lends them; any other caller builds its own.
pub(crate) struct ClaimsByBlock<'a> {
    claims: &'a [(ObjectId, RecognitionClaimPayload)],
    index: std::borrow::Cow<'a, BTreeMap<ObjectId, usize>>,
}

impl<'a> ClaimsByBlock<'a> {
    pub(crate) fn lent(
        claims: &'a [(ObjectId, RecognitionClaimPayload)],
        index: &'a BTreeMap<ObjectId, usize>,
    ) -> Self {
        Self {
            claims,
            index: std::borrow::Cow::Borrowed(index),
        }
    }

    fn get(&self, block: &ObjectId) -> Option<(ObjectId, &'a RecognitionClaimPayload)> {
        let (claim_id, claim) = self.claims.get(*self.index.get(block)?)?;
        Some((*claim_id, claim))
    }
}

/// Whether `parent` -- a block id a claim names -- is reached by `history`. See the module doc:
/// its id is in the history, or every patch of its own ancestry this repository can name is
/// already in the history (judged from the held block when there is one, else from the claims held
/// for it, recursively). A block that is neither held nor the subject of a held claim cannot be
/// judged, and is not reached.
fn parent_reached(
    object_store: &impl ObjectReader,
    by_block: &ClaimsByBlock<'_>,
    history: &RefHistory,
    parent: ObjectId,
) -> Result<bool> {
    parent_reached_visiting(
        object_store,
        by_block,
        history,
        parent,
        &mut BTreeSet::new(),
    )
}

fn parent_reached_visiting(
    object_store: &impl ObjectReader,
    by_block: &ClaimsByBlock<'_>,
    history: &RefHistory,
    parent: ObjectId,
    visiting: &mut BTreeSet<ObjectId>,
) -> Result<bool> {
    if history.blocks.contains(&parent) {
        return Ok(true);
    }
    if object_store.has_object(parent, ObjectType::Block)? {
        let ancestry = ancestors_inclusive(object_store, parent)?;
        return Ok(ancestry.values().all(|block| {
            block
                .patch_ids
                .iter()
                .all(|id| history.patches.contains(id))
        }));
    }
    let Some((_, claim)) = by_block.get(&parent) else {
        return Ok(false);
    };
    if !visiting.insert(parent) {
        return Ok(false);
    }
    let mut reached = claim
        .patch_ids
        .iter()
        .all(|id| history.patches.contains(id));
    if reached {
        for &grandparent in &claim.parent_block_ids {
            if !parent_reached_visiting(object_store, by_block, history, grandparent, visiting)? {
                reached = false;
                break;
            }
        }
    }
    visiting.remove(&parent);
    Ok(reached)
}

/// The refusal for a claim that builds on `parent_id`, which `target_ref`'s tip does not reach.
/// A precondition (exit `1`, `error: precondition not met:`), never a new `refusal-v1` id: `sync
/// seal` has no JSON surface. It names the claim, the parent and a way out the user can type: when
/// a local branch reaches the parent, `branch create` from it; when none does, the commands that
/// carry the branch the parent belongs to first.
#[allow(clippy::too_many_arguments)]
fn parent_not_reached_refusal(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_store: &RefStore,
    by_block: &ClaimsByBlock<'_>,
    target_ref: &str,
    tip: Option<ObjectId>,
    claim_id: ObjectId,
    parent_id: ObjectId,
) -> PrikkError {
    let _ = layout;
    let state = match tip {
        Some(tip) => format!("is at block {tip}"),
        None => "has no history yet".to_string(),
    };
    let carrier = local_branch_reaching(object_store, ref_store, by_block, target_ref, parent_id);
    // When no local branch holds the history but the claims for it are already accepted here, the
    // cheaper way out is to seal those claims first, in lineage order.
    let held_ancestry = match (&carrier, tip) {
        (None, tip) => ref_history(object_store, tip).ok().and_then(|history| {
            held_unsealed_ancestry(object_store, by_block, &history, parent_id)
        }),
        _ => None,
    };
    let way_out = match (&carrier, tip) {
        (Some(branch), None) => {
            format!("run `prikk branch create {target_ref} --from {branch}`, then seal again")
        }
        (Some(branch), Some(_)) => format!(
            "{target_ref} does not contain that history (it is behind it, or on another line); \
             to carry this claim on the history it was made on, run `prikk branch create \
             <new-branch> --from {branch}` and seal onto <new-branch>"
        ),
        (None, _) if held_ancestry.is_some() => {
            let ancestry = held_ancestry.unwrap_or_default();
            let steps: Vec<String> = ancestry
                .iter()
                .map(|claim| format!("`prikk sync seal {target_ref} --claim {claim}`"))
                .collect();
            format!(
                "the claims for the history it builds on are accepted here but not sealed on \
                 {target_ref}: seal them first, in this order: {}; then run this command again \
                 (or, if you still have the claims file `sync accept --claims-out` wrote, `prikk \
                 sync seal {target_ref} --claims <that file>` seals them in order along with this \
                 one)",
                steps.join(", then ")
            )
        }
        (None, _) => format!(
            "block {parent_id} is not held on any local branch here. Carry the branch it \
             belongs to first: in this repository run `prikk sync have <that-branch> --output \
             have.bin`; in the repository the claims came from run `prikk sync build \
             <that-branch> --have have.bin --output art.bin`; back here run `prikk sync accept \
             art.bin --claims-out claims.txt` and `prikk sync seal <that-branch> --claims \
             claims.txt`; then run `prikk branch create {target_ref} --from <that-branch>` and \
             seal again"
        ),
    };
    PrikkError::Precondition(format!(
        "sync seal {target_ref}: claim {claim_id} builds on block {parent_id}, which {target_ref} \
         does not reach ({target_ref} {state}); sealing it here would drop the history it was \
         made on -- refusing, nothing was written. To carry it: {way_out}"
    ))
}

/// The held claims, root first, whose sealing onto a ref with `history` would make `parent` reached:
/// the claim about `parent` and, before it, the claims about each of its ancestors that `history`
/// does not reach (a depth-first walk over the claims' own `parent_block_ids`, ancestors pushed
/// before the claim that builds on them). `None` when some block of that ancestry is neither
/// reached nor the subject of a held claim: the claims alone cannot supply it, and the caller falls
/// back to naming the carry.
fn held_unsealed_ancestry(
    object_store: &impl ObjectReader,
    by_block: &ClaimsByBlock<'_>,
    history: &RefHistory,
    parent: ObjectId,
) -> Option<Vec<ObjectId>> {
    fn visit(
        object_store: &impl ObjectReader,
        by_block: &ClaimsByBlock<'_>,
        history: &RefHistory,
        block: ObjectId,
        seen: &mut BTreeSet<ObjectId>,
        out: &mut Vec<ObjectId>,
    ) -> Option<()> {
        if parent_reached(object_store, by_block, history, block).ok()? {
            return Some(());
        }
        if !seen.insert(block) {
            return Some(());
        }
        let (claim_id, claim) = by_block.get(&block)?;
        for &ancestor in &claim.parent_block_ids {
            visit(object_store, by_block, history, ancestor, seen, out)?;
        }
        out.push(claim_id);
        Some(())
    }
    let mut out = Vec::new();
    visit(
        object_store,
        by_block,
        history,
        parent,
        &mut BTreeSet::new(),
        &mut out,
    )?;
    Some(out)
}

/// The first local branch (other than `target_ref`) whose history reaches `parent_id`, if any.
fn local_branch_reaching(
    object_store: &impl ObjectReader,
    ref_store: &RefStore,
    by_block: &ClaimsByBlock<'_>,
    target_ref: &str,
    parent_id: ObjectId,
) -> Option<String> {
    let pointers = ref_store.list_ref_pointers().ok()?;
    for pointer in pointers {
        if !pointer.ref_name.starts_with("heads/") || pointer.ref_name == target_ref {
            continue;
        }
        let Ok(Some(tip)) = read_current_tip(object_store, ref_store, &pointer.ref_name) else {
            continue;
        };
        let Ok(history) = ref_history(object_store, Some(tip.target_block_id)) else {
            continue;
        };
        if let Ok(true) = parent_reached(object_store, by_block, &history, parent_id) {
            return Some(pointer.ref_name);
        }
    }
    None
}

#[cfg(test)]
thread_local! {
    /// `(whole-container enumerations of the stored claims, walks of a ref's history)` on this thread: the controls that hold "a batch
    /// reads each once" (0.58.0 S3) count them.
    static BATCH_READS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
}

/// `(claim enumerations, history walks)` on this thread so far (`cfg(test)`).
#[cfg(test)]
pub(crate) fn batch_reads_for_test() -> (usize, usize) {
    BATCH_READS.with(std::cell::Cell::get)
}

fn enumerate_stored_claims(
    layout: &RepositoryLayout,
) -> Result<Vec<(ObjectId, RecognitionClaimPayload)>> {
    #[cfg(test)]
    BATCH_READS.with(|reads| reads.set((reads.get().0 + 1, reads.get().1)));
    debug_assert!(
        persisted_object_types().contains(&ObjectType::RecognitionClaim),
        "RecognitionClaim must remain a persisted, containerized object type"
    );

    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("type-enumeration");
    let container_path = layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let mut claims: Vec<(ObjectId, RecognitionClaimPayload)> = Vec::new();
    // RFC 156 §5b: one entry per claim **id**, not per record — a format-7 repository may hold several
    // records for one claim, and a claim listed twice would enter a `sync seal` batch twice. Records of
    // one id carry the same payload (it is what the id hashes), so the first position is kept.
    let mut seen: BTreeSet<ObjectId> = BTreeSet::new();
    if let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? {
        let replay = decode_container_records(ObjectType::RecognitionClaim, &bytes)?;
        for record in replay.records {
            let claim_id = record.envelope.object_id();
            if !seen.insert(claim_id) {
                continue;
            }
            let payload =
                RecognitionClaimPayload::decode_canonical(&record.envelope.canonical_payload)?;
            claims.push((claim_id, payload));
        }
    }
    Ok(claims)
}

#[cfg(test)]
mod tests;
