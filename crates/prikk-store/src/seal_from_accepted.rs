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
    BlockLineage, CandidateStateDerivationError, StateAnchoring, derive_candidate,
    seal_block_classified,
};
use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{
    ContainerSlot, DEFAULT_ACTIVE_NAME, RepositoryLayout, persisted_object_types,
};
use crate::lifecycle_cache::replay::LifecycleReplayError;
use crate::lock::ActiveLock;
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::merge::execute::candidate_replay_manifest;
use crate::object_store::{ObjectReadSnapshot, ObjectReader, ObjectWriteSession};
use crate::patch_exchange::accepted_but_unsealed_patch_ids;
use crate::recognition_claim::{ClaimSignatureVerification, verify_claim_signature};
use crate::refs::{RefPublication, RefStore, validate_local_branch_ref};
use crate::trust::{GatedOperation, load_maintainer_trust_policy_or_empty, verify_signer_trusted};
use crate::wal::Wal;
use crate::worktree_materialize::{
    PendingCurrentBranchMaterialization, require_nothing_in_the_way,
};

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
    seal_from_accepted_claim_impl(layout, None, ref_name, claim_id, signer)
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
) -> Result<SealFromAcceptedOutcome> {
    seal_from_accepted_claim_impl(layout, Some(active_lock), ref_name, claim_id, signer)
}

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
    let unsealed: BTreeSet<ObjectId> = accepted_but_unsealed_patch_ids(layout)?
        .into_iter()
        .collect();
    let selected_patch_ids: Vec<ObjectId> = claim
        .patch_ids
        .iter()
        .copied()
        .filter(|patch_id| unsealed.contains(patch_id))
        .collect();
    if selected_patch_ids.is_empty() {
        return Ok(SealFromAcceptedOutcome::AlreadySealed {
            ref_name: canonical_ref,
            claim_id,
        });
    }

    // §3 item 4: if a stored claim overlaps this one's patch set and disagrees on their relative
    // order, refuse rather than guess. Checked only once we know there is real sealing work to do
    // -- ambiguity about an order that will not be applied is moot.
    refuse_if_order_ambiguous(layout, claim_id, &claim)?;

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

    // §4 and Phase D: seal the new Block under the receiver's own key, carrying the claim's own
    // order restricted to the unsealed subset, verbatim (D7, §6 row 11). The derivation is the last
    // check and nothing is written unless it passes; its failure keeps `LifecycleReplayError`'s own
    // variant alive to the classification point rather than flattening it into an undifferentiated
    // `PrikkError::Integrity` the way ordinary `seal_block` does.
    let mut object_store = ObjectWriteSession::open(layout)?;
    let block_id = seal_block_classified(
        layout,
        &mut object_store,
        BlockLineage::Linear { parent },
        &selected_patch_ids,
        signer,
        StateAnchoring::Anchored(AnchorSite::SealFromAccepted),
        |err| match err {
            CandidateStateDerivationError::Lineage(err) => err,
            CandidateStateDerivationError::Patch(err) => classify_patch_application_failure(err),
        },
    )?;

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

/// 0.56.0 step W3c-4: the pre-write in-the-way check for a whole `sync seal` batch onto the
/// current branch, called once from `prikk-cli`'s `run_seal` after
/// [`crate::require_worktree_clean_and_capture`] and before the batch's first claim is sealed --
/// the same placement `execute_merge`'s own pre-write check has (0.56.0 step W3c-3b), and the same
/// narrow-`pub`-function shape [`crate::require_nothing_in_the_way_for_published_ref`] already set
/// for `branch create --from`, since [`crate::worktree_materialize`]'s own `ReplayManifest`
/// parameter type stays `pub(crate)`.
///
/// **Why this replays the whole batch's own patches by id, concatenated, through one
/// [`derive_candidate`] call, rather than replaying the claimed blocks by the block ids the
/// claims themselves name:** a `RecognitionClaimPayload`'s `block_id` names a block on the
/// *sender's* repository, which `sync accept`'s own admission (`patch_exchange::accept`) never
/// writes here -- confirmed by reading that module: it decodes and admits `Patch`, `Blob`, author
/// key material, `RecognitionClaim` and `Tag` objects, never a `Block`. So there is no local
/// object that id could resolve to, and a block-id-addressed replay (a twin of
/// `replay_for_verified_worktree_write`, or `patch_replay::replay_block_chain` directly) is a dead
/// end for this producer specifically -- unlike `merge --into`/`branch create --from`, whose own
/// targets (`from_ref`'s tip, the merge's own candidate) are always blocks this repository already
/// holds. What *is* available, before any claim seals, is exactly what
/// [`seal_from_accepted_claim_impl`] itself computes per claim:
/// [`accepted_but_unsealed_patch_ids`] (the patches still eligible to be sealed) filtered against
/// each claim's own `patch_ids`, in the batch's own already-established order
/// (`order_claims_for_sealing`'s result, passed in as `ordered_claim_ids`). Concatenating every
/// claim's own filtered selection, removing each selection from the pool before considering the
/// next claim (mirroring what sealing the batch for real would do: a patch selected by claim N is
/// no longer "unsealed" when claim N+1 is considered), and calling [`derive_candidate`] once from
/// the pre-batch parent produces the identical final state N chained single-claim seals would --
/// confirmed by reading [`crate::lifecycle_cache::replay::apply_candidate_patches`], which folds a
/// flat patch-id slice onto a state with no dependency on block boundaries at all.
///
/// **This check only ever *skips itself*, never refuses on its own simulation's own behalf.**
/// `sync.rs`'s own module doc states §3 as a firm policy, not merely today's behaviour: "stop at
/// the first failure... no rollback -- each seal is an independent, legitimate act under the
/// receiver's own key," so an *earlier* claim's own valid seal must never be pre-empted by a
/// *later* claim's own failure, including a later claim whose patches would not apply cleanly
/// once an earlier one has. Combining every claim's own patches into one simulated derivation (as
/// this function does, for the file-collision check alone) would otherwise surface exactly that
/// kind of claim-vs-claim divergence *before* the batch even starts -- measured directly: two
/// senders independently creating the same new path from the same (absent) parent, sealed in one
/// batch onto an unborn current branch, used to refuse the whole batch here, reporting neither
/// claim sealed, where the real per-claim loop below would seal the first and fail only the
/// second (`rfc116_sync_cli.rs::row6_a_mid_batch_seal_failure_stops_reports_and_leaves_the_earlier_seal_intact`,
/// predates this round and must keep holding). So: if this function's own simulation cannot even
/// derive a combined state at all -- a claim that does not exist or does not decode, or patches
/// that do not fold together -- there is no manifest to check a file collision against, and this
/// returns `Ok(())` rather than propagating that failure as a refusal: the real, authoritative
/// per-claim loop (`seal_from_accepted_claim_in_batch`, called once per claim regardless of what
/// this function decided) re-derives the identical state per claim and reports the identical
/// failure at the exact claim and in the exact words it always has, with every earlier claim's
/// own seal already landed and kept. A genuine file-in-the-way collision, by contrast, is
/// orthogonal to claim-vs-claim ordering (the worktree is not party to any claim), so refusing on
/// *that* basis never pre-empts a legitimate earlier seal.
pub fn require_nothing_in_the_way_for_seal_batch(
    layout: &RepositoryLayout,
    pending: &PendingCurrentBranchMaterialization,
    ref_name: &str,
    ordered_claim_ids: &[ObjectId],
    action: &str,
) -> Result<()> {
    let Some(target_manifest) =
        simulate_seal_batch_target_manifest(layout, ref_name, ordered_claim_ids)
    else {
        return Ok(());
    };
    require_nothing_in_the_way(layout, pending, &target_manifest, action)
}

/// The simulation [`require_nothing_in_the_way_for_seal_batch`] needs, isolated so every failure
/// inside it can be turned into "no opinion" (`None`) rather than a hard error -- see that
/// function's own doc for why. Returns `None` on any `ObjectReadSnapshot`, claim-lookup,
/// claim-decode, or [`derive_candidate`]/[`candidate_replay_manifest`] failure; the real
/// per-claim loop re-reads and re-derives everything this touches anyway, moments later, so
/// nothing is silently lost -- it is reported there instead, correctly scoped to the one claim
/// that actually fails.
fn simulate_seal_batch_target_manifest(
    layout: &RepositoryLayout,
    ref_name: &str,
    ordered_claim_ids: &[ObjectId],
) -> Option<crate::patch_replay::ReplayManifest> {
    let read_snapshot = ObjectReadSnapshot::open(layout).ok()?;
    let ref_store = RefStore::new(layout.clone());
    let current = read_current_tip(&read_snapshot, &ref_store, ref_name).ok()?;
    let parent = current.as_ref().map(|tip| tip.target_block_id);

    let mut available: BTreeSet<ObjectId> = accepted_but_unsealed_patch_ids(layout)
        .ok()?
        .into_iter()
        .collect();
    let mut combined_patch_ids: Vec<ObjectId> = Vec::new();
    for &claim_id in ordered_claim_ids {
        let claim_envelope = read_snapshot
            .read_typed(claim_id, ObjectType::RecognitionClaim)
            .ok()??;
        let claim =
            RecognitionClaimPayload::decode_canonical(&claim_envelope.canonical_payload).ok()?;
        let selected: Vec<ObjectId> = claim
            .patch_ids
            .iter()
            .copied()
            .filter(|patch_id| available.contains(patch_id))
            .collect();
        for patch_id in &selected {
            available.remove(patch_id);
        }
        combined_patch_ids.extend(selected);
    }

    let candidate = derive_candidate(
        layout,
        &read_snapshot,
        parent,
        &combined_patch_ids,
        StateAnchoring::Anchored(AnchorSite::SealFromAccepted),
    )
    .ok()?;
    candidate_replay_manifest(&read_snapshot, candidate.entries, &candidate.text_cache).ok()
}

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
    let selected_set: BTreeSet<ObjectId> = selected.patch_ids.iter().copied().collect();
    for (other_id, other) in enumerate_stored_claims(layout)? {
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
fn enumerate_stored_claims(
    layout: &RepositoryLayout,
) -> Result<Vec<(ObjectId, RecognitionClaimPayload)>> {
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
