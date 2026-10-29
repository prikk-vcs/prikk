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
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload, SignerRole};

use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_set_digest::patch_ids_reachable_from_block;
use crate::received::list_received_pointers;
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
        reachable.extend(reachable_patch_ids_from_ref_state(
            &object_store,
            &pointer.ref_name,
            pointer.ref_state_id,
        )?);
    }

    Ok(all_patch_ids.difference(&reachable).copied().collect())
}

/// Resolve one ref pointer's tip block to its own reachable patch ids: `RefState` read -> decode
/// -> [`resolve_ref_tip_block`] -> [`patch_ids_reachable_from_block`]. Shared by
/// [`accepted_but_unsealed_patch_ids`]'s own loop over local ref pointers and
/// [`patch_ids_pending_excluding_received`]'s loop over received pointers -- identical resolution,
/// differing only in which pointer kind calls it (`pointer_name` is only ever used for the error
/// message, so a `remotes/...` name reads naturally too).
fn reachable_patch_ids_from_ref_state(
    object_store: &ObjectReadSnapshot,
    pointer_name: &str,
    ref_state_id: ObjectId,
) -> Result<Vec<ObjectId>> {
    let ref_state_envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {pointer_name} names missing RefState {ref_state_id}"
            ))
        })?;
    let ref_state_payload = prikk_object::RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )?;
    let (tip_block_id, _tag_envelope) = resolve_ref_tip_block(object_store, &ref_state_payload)?;
    patch_ids_reachable_from_block(object_store, tip_block_id)
}

/// `sync pending`'s own view (0.56.0 step 1a2; owner ruling 2026-10-29, "Exclude received
/// history"): every patch [`accepted_but_unsealed_patch_ids`] reports, minus every one also
/// reachable from a **received** pointer's own tip block (`archive import` without a successful
/// `--adopt`, landing via `land_as_received_pointer` into the received-index -- confirmed in step
/// 1a's own report, §4). A patch the sender's own history already sealed, even one this repository
/// has not adopted, is not something an operator still needs to act on.
///
/// **Built as its own subtraction, not a change to [`accepted_but_unsealed_patch_ids`] itself.**
/// That function has a third caller, `seal_from_accepted_claim` (a gate, not a listing), which
/// keeps its own exact prior behavior -- see this round's own report for why sealing does not get
/// this exclusion too.
///
/// Set algebra: `(A \ X) \ Y = A \ (X ∪ Y)`. `accepted_but_unsealed_patch_ids` already computed
/// `A \ X`; this adds exactly one further pass, over received pointers (typically few), never a
/// second scan of the `Patch` container. Filtering an already-sorted `Vec` by exclusion preserves
/// its order, so the result stays in the same patch-id order the raw query already returns.
pub fn patch_ids_pending_excluding_received(layout: &RepositoryLayout) -> Result<Vec<ObjectId>> {
    let pending = accepted_but_unsealed_patch_ids(layout)?;
    if pending.is_empty() {
        return Ok(pending);
    }

    let object_store = ObjectReadSnapshot::open(layout)?;
    let mut received_reachable: BTreeSet<ObjectId> = BTreeSet::new();
    for pointer in list_received_pointers(layout)? {
        received_reachable.extend(reachable_patch_ids_from_ref_state(
            &object_store,
            &pointer.ref_name,
            pointer.ref_state_id,
        )?);
    }
    if received_reachable.is_empty() {
        return Ok(pending);
    }

    Ok(pending
        .into_iter()
        .filter(|patch_id| !received_reachable.contains(patch_id))
        .collect())
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
    /// This claim's own MAINTAINER signature outcome -- reported, never gating. See
    /// [`PendingClaimSignatureOutcome`]'s own doc for why this is not plain
    /// [`ClaimSignatureVerification`].
    pub signature: PendingClaimSignatureOutcome,
}

/// `sync pending`'s own reporting of a claim's MAINTAINER signature outcome (0.56.0 step 1a2,
/// owner ruling "a reader reports a failed signature; it does not refuse the listing"). Carries
/// [`ClaimSignatureVerification`]'s own two outcomes unchanged, plus a third this reader alone
/// adds for the one case that type deliberately does not carry -- a signature that fails to
/// verify against a key this repository has *adopted*, i.e. a detected forgery.
/// [`ClaimSignatureVerification`] itself keeps exactly its own two variants: every gate
/// (`accept_exchange_artifact`, `seal_from_accepted_claim`) still refuses on that case via
/// `verify_claim_signature`'s `Err`, unchanged. This mapping exists only here, in the reader,
/// because a listing must show the operator *which* claim failed, not hide the whole listing
/// behind one forged claim's refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingClaimSignatureOutcome {
    /// Mirrors [`ClaimSignatureVerification::Sound`].
    Sound {
        /// The MAINTAINER key id the signature named and verified against.
        key_id: String,
    },
    /// Mirrors [`ClaimSignatureVerification::Unverifiable`].
    Unverifiable {
        /// The MAINTAINER key id named, which this repository has not adopted.
        key_id: String,
    },
    /// `verify_claim_signature` returned `Err`: a signature naming an *adopted* key that failed
    /// to verify (a detected forgery), a MAINTAINER signature whose algorithm is not Ed25519, or
    /// no MAINTAINER-role signature present at all. `key_id` is the first MAINTAINER-role
    /// signature's own key id, when the claim's envelope carries one at all -- `None` only for
    /// the last case, where there is no signature to name a key from. `reason` is the refusal's
    /// own display text. **The one value a tool consuming this listing must treat as an alarm.**
    Invalid {
        /// The first MAINTAINER-role signature's key id, or `None` if the claim carries no
        /// MAINTAINER-role signature at all.
        key_id: Option<String>,
        /// `verify_claim_signature`'s own refusal text.
        reason: String,
    },
}

impl From<ClaimSignatureVerification> for PendingClaimSignatureOutcome {
    fn from(value: ClaimSignatureVerification) -> Self {
        match value {
            ClaimSignatureVerification::Sound { key_id } => Self::Sound { key_id },
            ClaimSignatureVerification::Unverifiable { key_id } => Self::Unverifiable { key_id },
        }
    }
}

/// The first MAINTAINER-role signature's own key id in `envelope`, if any -- used only to name a
/// `key_id` for [`PendingClaimSignatureOutcome::Invalid`] when `verify_claim_signature` itself
/// returns `Err` without a structured key id to reuse. Mirrors the same filter
/// `verify_claim_signature`'s own first pass over `envelope.signatures` applies.
fn first_maintainer_key_id(envelope: &prikk_object::ObjectEnvelope) -> Option<String> {
    envelope
        .signatures
        .iter()
        .find(|signature| signature.signer_role == SignerRole::Maintainer)
        .map(|signature| signature.key_id.clone())
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
/// patch [`patch_ids_pending_excluding_received`] reports, each with the recognition claim(s)
/// naming it.
///
/// **One pass over each container, never a pass over claims per pending patch.** Reuses
/// [`patch_ids_pending_excluding_received`]'s own scan (which itself reuses
/// [`accepted_but_unsealed_patch_ids`]'s single scan of the `Patch` container plus its ancestry
/// walks) for the pending set, then makes exactly one further scan of the `RecognitionClaim`
/// container -- the same container-walk shape [`crate::order_claims_for_sealing`]'s own sibling
/// listing (`seal_from_accepted.rs`'s private `enumerate_stored_claims`) uses, inlined here rather
/// than reused because this caller also needs each claim's own `ObjectEnvelope` (for
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
/// **A claim's signature outcome is reported, never gated on -- see [`PendingClaimSignatureOutcome`]'s
/// own doc.** `verify_claim_signature`'s `Err` (a genuine forged signature under an adopted key, a
/// non-Ed25519 MAINTAINER signature, or none at all) is mapped to
/// [`PendingClaimSignatureOutcome::Invalid`] for that one claim, not propagated -- an unrelated
/// forged claim must not blind the whole listing to every other pending patch.
pub fn pending_patches_with_claims(layout: &RepositoryLayout) -> Result<Vec<PendingPatchEntry>> {
    debug_assert!(
        persisted_object_types().contains(&ObjectType::RecognitionClaim),
        "RecognitionClaim must remain a persisted, containerized object type"
    );

    let pending_ids = patch_ids_pending_excluding_received(layout)?;
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
            // a claim naming no pending patch is irrelevant to this listing. A signature failure
            // is reported for this one claim (`Invalid`), never propagated: a forged claim must
            // not blind the listing to every other pending patch (0.56.0 step 1a2, R1).
            let signature = match verify_claim_signature(&record.envelope, &trust_policy) {
                Ok(outcome) => PendingClaimSignatureOutcome::from(outcome),
                Err(err) => PendingClaimSignatureOutcome::Invalid {
                    key_id: first_maintainer_key_id(&record.envelope),
                    reason: err.to_string(),
                },
            };
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
