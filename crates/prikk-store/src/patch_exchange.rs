//! RFC 115 Stage 3 (design-v1.md D1/D2/D5, §7/§8) — the patch-exchange artifact and accept path.
//!
//! **Representational, not frozen** (RFC 114 §3): this format carries objects whose identity is
//! already frozen, and carries no identity of its own. It may change in a later version with a
//! documented read path — the same asymmetry every repository-format and bundle-format transition
//! in this project already has (read what the past wrote, write only the present). That licence is
//! not permission to be careless: every byte still has a stated reason.
//!
//! **`PEXCH002` is emitted on export and accepted on import** (RFC 117 stage 3 bumped `PEXCH001` ->
//! `PEXCH002` to add a Tag section; `PEXCH001` is refused, not read -- see `artifact.rs`'s own doc).
//!
//! **Scope, stated plainly (handoff §1): this stage does not deliver a seal-from-accepted path.**
//! `seal` builds a block from the active WAL; an accepted patch is a written object that was never
//! in the WAL, and closing that gap is real design work this stage does not otherwise touch. What
//! lands here: receive patches, verify them, store them, and see exactly what you hold that is not
//! yet sealed. You cannot yet seal them.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload};

use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_set_digest::patch_ids_reachable_from_block;
use crate::recognition_claim::verify_claim_signature;
use crate::refs::{RefStore, resolve_ref_tip_block};
use crate::trust::load_maintainer_trust_policy_or_empty;

mod accept;
mod artifact;

pub use accept::{
    AcceptOptions, AcceptReport, ClaimSignatureVerification, accept_exchange_artifact,
};
pub use artifact::{
    DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_BYTES, DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT,
    DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES, ExchangeExportReport, export_exchange_artifact,
};

/// D2's derived query (design-v1.md §3, RFC 115 Stage 3 §5): **no new container, no stored pending
/// state.** "Accepted but unsealed" is computed, every time it's asked:
///
/// > patch objects present in this repository, minus the patch ids reachable from any block.
///
/// Enumerates stored patches the way `verify/objects.rs:163` enumerates objects
/// (`persisted_object_types()` → per-type container → `decode_container_records`, narrowed here to
/// `ObjectType::Patch` alone) and reuses Stage 1's `patch_ids_reachable_from_block` for the
/// subtrahend, over every local ref's tip -- the same ancestry walk `export_bundle` and Stage 1's
/// digest already share, not a second one.
pub fn accepted_but_unsealed_patch_ids(layout: &RepositoryLayout) -> Result<Vec<ObjectId>> {
    debug_assert!(
        persisted_object_types().contains(&ObjectType::Patch),
        "Patch must remain a persisted, containerized object type"
    );

    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("type-enumeration");
    let container_path = layout.container_slot_path(ObjectType::Patch, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let mut all_patch_ids: BTreeSet<ObjectId> = BTreeSet::new();
    if let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? {
        let replay = decode_container_records(ObjectType::Patch, &bytes)?;
        for record in replay.records {
            all_patch_ids.insert(record.envelope.object_id());
        }
    }

    let object_store = ObjectReadSnapshot::open(layout)?;
    let ref_store = RefStore::new(layout.clone());
    let mut reachable: BTreeSet<ObjectId> = BTreeSet::new();
    for pointer in ref_store.list_ref_pointers()? {
        let ref_state_envelope = object_store
            .read_typed(pointer.ref_state_id, ObjectType::RefState)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "ref {} names missing RefState {}",
                    pointer.ref_name, pointer.ref_state_id
                ))
            })?;
        let ref_state_payload = prikk_object::RefStatePayload::decode_canonical(
            &ref_state_envelope.canonical_payload,
            ref_state_envelope.schema_version,
        )?;
        let (tip_block_id, _tag_envelope) =
            resolve_ref_tip_block(&object_store, &ref_state_payload)?;
        reachable.extend(patch_ids_reachable_from_block(&object_store, tip_block_id)?);
    }

    Ok(all_patch_ids.difference(&reachable).copied().collect())
}

/// One recognition claim naming a pending patch (planeter letter 2026-10-21 §2; 0.56.0 step 1
/// part 1a): the claim's own id (content-addressed, not a payload field), the single block it
/// names (`RecognitionClaimPayload::block_id` -- **not** `parent_block_ids`, which mirrors that
/// named block's own parents and is used only for [`crate::order_claims_for_sealing`]'s batch
/// ordering; it is plural where the request's own "the parent block id the claim names" is
/// singular), and that claim's own MAINTAINER signature outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPatchClaim {
    /// The claim's own content-addressed id.
    pub claim_id: ObjectId,
    /// The block this claim says these patches are sealed into (`RecognitionClaimPayload::block_id`).
    pub parent_block_id: ObjectId,
    /// This claim's own MAINTAINER signature outcome -- reported, never gating (the same treatment
    /// `AcceptReport::claim_signature_outcomes` already gives it).
    pub signature: ClaimSignatureVerification,
}

/// One pending patch, with every recognition claim (held locally) that names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingPatchEntry {
    /// The pending patch's own id.
    pub patch_id: ObjectId,
    /// Every held claim naming `patch_id`, sorted by claim id. Empty when no held claim names
    /// this patch -- see [`pending_patches_with_claims`]'s own doc for when that happens.
    pub claims: Vec<PendingPatchClaim>,
}

/// `sync pending`'s own join (0.56.0 step 1 part 1a; planeter letter 2026-10-21 §2): every
/// accepted-but-unsealed patch, each with the recognition claim(s) naming it.
///
/// **One pass over each container, never a pass over claims per pending patch.** Reuses
/// [`accepted_but_unsealed_patch_ids`]'s own single scan of the `Patch` container (plus its
/// existing ancestry walk) for the pending set, then makes exactly one further scan of the
/// `RecognitionClaim` container -- the same container-walk shape [`crate::order_claims_for_sealing`]'s
/// own sibling listing (`seal_from_accepted.rs`'s private `enumerate_stored_claims`) uses, inlined
/// here rather than reused because this caller also needs each claim's own `ObjectEnvelope` (for
/// `verify_claim_signature`), which that function drops once it has computed a claim's id. Builds
/// the patch -> claims map in that single pass (for each claim, for each of its own `patch_ids`
/// that is in the pending set, append to that patch's own list) -- never re-scanned per patch.
///
/// **A patch no claim names appears with `claims: []`.** This can happen today: an exchange
/// artifact's patch and recognition-claim sections are independent (`accept_exchange_artifact`
/// writes whichever of `decoded.patches`/`decoded.claims` an artifact carries, with no requirement
/// that one names the other, in that same call or ever), so a patch accepted without ever naming
/// a claim -- or one named only by a claim this repository has not yet received -- is ordinary,
/// not an error.
///
/// **A claim's signature outcome is reported, never gated on.** `verify_claim_signature` can
/// still return `Err` -- not just the two reportable variants -- but only for a genuine forged
/// signature under a key this repository has *adopted*: a claim already held here passed this
/// same check once, under whatever trust policy held at accept time (`accept_exchange_artifact`
/// propagates that `Err` with `?`); what changes between then and a later `sync pending` call is
/// the trust policy itself, so a signature that was `Unverifiable` at accept time (its key not yet
/// adopted) can turn out, once that key *is* later adopted, to never have verified at all. That is
/// exactly the forged-claim-under-a-now-trusted-identity case `verify_claim_signature`'s own doc
/// calls "an integrity failure, not a trust question" -- every other caller of this function
/// propagates that `Err` as a refusal, and this one does too, rather than inventing a third,
/// undesigned JSON outcome for a case the type's own two variants deliberately do not carry.
pub fn pending_patches_with_claims(layout: &RepositoryLayout) -> Result<Vec<PendingPatchEntry>> {
    debug_assert!(
        persisted_object_types().contains(&ObjectType::RecognitionClaim),
        "RecognitionClaim must remain a persisted, containerized object type"
    );

    let pending_ids = accepted_but_unsealed_patch_ids(layout)?;
    let pending_set: BTreeSet<ObjectId> = pending_ids.iter().copied().collect();

    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("type-enumeration");
    let container_path = layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let trust_policy = load_maintainer_trust_policy_or_empty(layout)?;
    let mut claims_by_patch: BTreeMap<ObjectId, Vec<PendingPatchClaim>> = BTreeMap::new();
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
            let names_pending: Vec<ObjectId> = payload
                .patch_ids
                .iter()
                .copied()
                .filter(|patch_id| pending_set.contains(patch_id))
                .collect();
            if names_pending.is_empty() {
                continue;
            }
            // Only a claim naming at least one pending patch needs its signature checked here --
            // a claim naming no pending patch is irrelevant to this listing and must not be able
            // to refuse it on an unrelated signature problem.
            let signature = verify_claim_signature(&record.envelope, &trust_policy)?;
            for patch_id in names_pending {
                claims_by_patch
                    .entry(patch_id)
                    .or_default()
                    .push(PendingPatchClaim {
                        claim_id,
                        parent_block_id: payload.block_id,
                        signature: signature.clone(),
                    });
            }
        }
    }
    for claims in claims_by_patch.values_mut() {
        claims.sort_by_key(|claim| claim.claim_id);
    }

    Ok(pending_ids
        .into_iter()
        .map(|patch_id| PendingPatchEntry {
            claims: claims_by_patch.remove(&patch_id).unwrap_or_default(),
            patch_id,
        })
        .collect())
}

#[cfg(test)]
mod exchange_test_support;
#[cfg(test)]
mod tests;
