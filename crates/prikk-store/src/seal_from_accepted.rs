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
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType,
    RecognitionClaimPayload, RefKind, RefStatePayload, RefUpdatePayload,
};

use crate::block_state::{CandidateStateDerivationError, derive_next_state_root_for_candidate};
use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{
    ContainerSlot, DEFAULT_ACTIVE_NAME, RepositoryLayout, persisted_object_types,
};
use crate::lifecycle_cache::replay::LifecycleReplayError;
use crate::lock::ActiveLock;
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::object_store::{ObjectReadSnapshot, ObjectReader, ObjectWriteSession, ObjectWriter};
use crate::patch_exchange::accepted_but_unsealed_patch_ids;
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
/// See the module doc for the shape of what this does and does not guarantee.
pub fn seal_from_accepted_claim(
    layout: &RepositoryLayout,
    ref_name: &str,
    claim_id: ObjectId,
    signer: &impl MaintainerSigner,
) -> Result<SealFromAcceptedOutcome> {
    layout.require_current_format()?;
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    crate::refs::ensure_no_incomplete_publication(layout)?;

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

    let active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    crate::refs::ensure_no_incomplete_publication(layout)?;

    // §5: the active WAL must be empty. Sealing accepted patches advances the branch tip; locally
    // queued WAL patches were composed against the *old* tip, and since DC-66 a queue chains
    // baselines, so advancing the tip underneath them invalidates assumptions they were built on --
    // the same reasoning `rollback_draft` already applies for exactly this class of reason.
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
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
    let current = read_current_tip(&read_snapshot, &ref_store, &canonical_ref)?;
    let parent = current.as_ref().map(|tip| tip.target_block_id);

    // §4: derive the new state, keeping `LifecycleReplayError`'s own variant alive to the
    // classification point rather than letting `?` flatten it into an undifferentiated
    // `PrikkError::Integrity` the way ordinary `derive_next_state_root` does.
    let state_merkle_root =
        match derive_next_state_root_for_candidate(&read_snapshot, parent, &selected_patch_ids) {
            Ok(root) => root,
            Err(CandidateStateDerivationError::Lineage(err)) => return Err(err),
            Err(CandidateStateDerivationError::Patch(err)) => {
                return Err(classify_patch_application_failure(err));
            }
        };

    // Phase D: write nothing until every check above has passed. Build and sign the new Block,
    // sealed under the receiver's own key, carrying the claim's own order restricted to the
    // unsealed subset, verbatim (D7, §6 row 11).
    let block_payload = BlockPayload {
        parent_block_ids: parent.into_iter().collect(),
        kind: if current.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        },
        patch_ids: selected_patch_ids.clone(),
        state_merkle_root,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_envelope = signed_envelope(
        ObjectType::Block,
        2,
        block_payload.to_canonical_bytes()?,
        signer,
    )?;
    let block_id = block_envelope.object_id();

    let mut object_store = ObjectWriteSession::open(layout)?;
    object_store.write_object(&block_envelope)?;

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
    drop(active_lock);

    Ok(SealFromAcceptedOutcome::Sealed {
        ref_name: canonical_ref,
        block_id,
        ref_state_id: published_ref_state_id,
        patch_count: selected_patch_ids.len(),
        claim_signature_outcome,
    })
}

/// §4's ruled classification, applied to the one `LifecycleReplayError` variant
/// [`derive_next_state_root_for_candidate`] can actually surface from applying candidate patches
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
        | LifecycleReplayError::MissingBlobForLifecycleEffect { .. } => PrikkError::Integrity(
            format!("seal refused: integrity -- this repository's own state is broken ({error})"),
        ),
    }
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
    let container_path = layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let mut claims = Vec::new();
    if let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? {
        let replay = decode_container_records(ObjectType::RecognitionClaim, &bytes)?;
        for record in replay.records {
            let payload =
                RecognitionClaimPayload::decode_canonical(&record.envelope.canonical_payload)?;
            claims.push((record.envelope.object_id(), payload));
        }
    }
    Ok(claims)
}

#[cfg(test)]
mod tests;
