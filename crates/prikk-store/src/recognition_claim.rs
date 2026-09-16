//! RFC 115 Stage 2 (design-v1.md D3, §3), amended by §11 (D6) and by RFC 116 N3: what a receiver
//! may check about a `RecognitionClaim` against its own object store.
//!
//! **Must not**: `block_id`/`patch_ids`/`parent_block_ids` are never existence-checked. A claim is
//! verifiable with none of the objects it names present — that is the entire reason it is a claim
//! object and not a Block (D3). This module never refuses on absence.
//!
//! **Must**: if the receiver *does* hold the referenced block, **both** the claim's `patch_ids` and
//! its `parent_block_ids` must match that block's own, **in order** — a claim contradicting a block
//! already held is a detected lie, about whichever field disagreed.
//!
//! **The comparison is exact sequence equality, not set equality (D6; extended to
//! `parent_block_ids` by N3).** A block is content-addressed, so the same `block_id` names the same
//! canonical payload, therefore the same `patch_ids` sequence **and** the same `parent_block_ids`
//! sequence. An honest claim about a block the receiver genuinely holds therefore matches it in
//! order, always, on both fields — there is no honest way to name the right block and the wrong
//! patches, or the right block and the wrong parents. A differently-ordered (or differently-valued)
//! claim about a held block cannot arise from honesty; only from a lie or from a lossy claim
//! format. So sequence equality cannot produce a false accusation; it can only detect one — which
//! set equality, used before D6, structurally could not do, since order is exactly the information
//! a set discards. Neither side is sorted or deduplicated before comparing: `Block.patch_ids` and
//! `Block.parent_block_ids` are the free sequences the block itself carries, and
//! `RecognitionClaimPayload`'s own two fields mirror them verbatim by construction (the payload's
//! own decoder/encoder no longer accept anything else).
//!
//! **`Contradicted` names which field disagreed (N3 §4).** A parent mismatch reported through
//! `patch_ids`-shaped output would read as "your patches disagree" when the patches are fine — a
//! misleading diagnostic of exactly the class RFC 115 Stage 4's divergence-vs-corruption ruling
//! exists to prevent. A wrong explanation is worse than a vague one: it sends the reader somewhere
//! false.
//!
//! **[`order_claims_for_sealing`] (RFC 116 stage 5) is `parent_block_ids`'s actual purpose.** N3
//! added the field so a batch of claims spanning more than one block could be sealed in the right
//! order, derived from **signed** data rather than an artifact's own unsigned sequence or an
//! incidental id order. This is the first code that sorts by it.

use std::collections::{BTreeMap, VecDeque};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    BlockPayload, ObjectEnvelope, ObjectId, ObjectType, RecognitionClaimPayload, Signature,
    SignatureAlgorithm, SignerRole,
};

use crate::object_store::ObjectReader;
use crate::trust::MaintainerTrustPolicy;

/// Which field of a `RecognitionClaimPayload` a `RecognitionClaimConsistency::Contradicted`
/// outcome names as the one that disagreed with the held block (N3 §4) -- lets a caller
/// distinguish "the claim lies about which patches" from "the claim lies about which parents"
/// without parsing a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContradictedField {
    /// The claim's `patch_ids` disagree with the held block's own.
    PatchIds,
    /// The claim's `parent_block_ids` disagree with the held block's own.
    ParentBlockIds,
}

/// The outcome of checking a `RecognitionClaim` against the receiver's own store. Three states,
/// not a `bool` and not a `Result<()>` that would flatten "absent" into "fine" — `BlockAbsent` is
/// the expected case in real exchange and must not read as a degraded one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecognitionClaimConsistency {
    /// The referenced block is held, and both the claim's `patch_ids` and `parent_block_ids`
    /// match the block's own, in order.
    Consistent,
    /// The referenced block is not held. Expected, not a defect — the claim is still meaningful.
    BlockAbsent,
    /// The referenced block is held, and one of the claim's fields does **not** match the block's
    /// own, in order — a detected lie. `field` names which one; `patch_ids` is checked first, so a
    /// claim disagreeing on both fields is reported as a `PatchIds` contradiction.
    Contradicted {
        /// Which field disagreed.
        field: ContradictedField,
        /// That field's own value as the claim states it, verbatim.
        claimed: Vec<ObjectId>,
        /// That field's own value as the held block actually has it, verbatim.
        actual: Vec<ObjectId>,
    },
}

/// Check `claim` against `object_store`. See the module doc for why sequence equality, not set
/// equality, is the correct comparison under D6/N3's verbatim-order contract, and for why
/// `Contradicted` names the disagreeing field.
pub fn check_recognition_claim_consistency(
    object_store: &impl ObjectReader,
    claim: &RecognitionClaimPayload,
) -> Result<RecognitionClaimConsistency> {
    let Some(block_envelope) = object_store.read_typed(claim.block_id, ObjectType::Block)? else {
        return Ok(RecognitionClaimConsistency::BlockAbsent);
    };
    let block_payload = BlockPayload::decode_canonical(&block_envelope.canonical_payload)?;
    if block_payload.patch_ids != claim.patch_ids {
        return Ok(RecognitionClaimConsistency::Contradicted {
            field: ContradictedField::PatchIds,
            claimed: claim.patch_ids.clone(),
            actual: block_payload.patch_ids,
        });
    }
    if block_payload.parent_block_ids != claim.parent_block_ids {
        return Ok(RecognitionClaimConsistency::Contradicted {
            field: ContradictedField::ParentBlockIds,
            claimed: claim.parent_block_ids.clone(),
            actual: block_payload.parent_block_ids,
        });
    }
    Ok(RecognitionClaimConsistency::Consistent)
}

/// The outcome of checking one `RecognitionClaim`'s own MAINTAINER signature (Stage 3 handoff §4.2
/// item 8; reused by Stage 4 §3's "report the outcome alongside the result"). Shaped identically to
/// `AuthorSignatureVerification` and for the same reason: **never gating** (design D3) means a
/// claim naming a `key_id` this repository has not adopted still accepts -- it reads
/// `Unverifiable`, never `Sound`, and does not by itself refuse. Only a signature that fails to
/// verify against a `key_id` this repository *has* adopted refuses (a forged claim under a
/// locally-trusted identity is an integrity failure, not a trust question). There is no `Fails`
/// variant for the same reason `AuthorSignatureVerification` has none: that outcome is a genuine
/// refusal, propagated as an `Err`, not a value this type carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimSignatureVerification {
    /// The signature verifies against a `key_id` this repository has adopted as a trusted
    /// maintainer.
    Sound {
        /// The MAINTAINER key id the signature named and verified against.
        key_id: String,
    },
    /// This repository has not adopted `key_id`, so the signature cannot be checked. Not a
    /// failure and not by itself a refusal -- see the type doc.
    Unverifiable {
        /// The MAINTAINER key id named, which this repository has not adopted.
        key_id: String,
    },
}

/// Verify `envelope`'s own MAINTAINER signature against `trust_policy`. The single definition of
/// *how* a claim's signature is checked -- Stage 3's accept path and Stage 4's seal-from-accepted
/// path both call this rather than each carrying their own copy, the same reason
/// `verify_author_signature_against_material` was extracted for AUTHOR signatures. Refuses
/// (`Err`) if `envelope` carries no MAINTAINER signature at all, if the signature's algorithm is
/// not Ed25519, or if it fails to verify against an *adopted* key; reads `Unverifiable` rather
/// than refusing when `key_id` is simply not adopted -- see `ClaimSignatureVerification`'s own doc.
pub(crate) fn verify_claim_signature(
    envelope: &ObjectEnvelope,
    trust_policy: &MaintainerTrustPolicy,
) -> Result<ClaimSignatureVerification> {
    let claim_id = envelope.object_id();
    // RFC 156 Stage 1: every MAINTAINER signature, not only the first. Any that is not Ed25519, or
    // that names an adopted key and does not verify against it, refuses; the answer names the first
    // adopted signer that verified, or — when none is adopted — the first signer.
    let mut first_key_id: Option<String> = None;
    let mut first_sound: Option<String> = None;
    for signature in envelope
        .signatures
        .iter()
        .filter(|signature| signature.signer_role == SignerRole::Maintainer)
    {
        if first_key_id.is_none() {
            first_key_id = Some(signature.key_id.clone());
        }
        if signature.algorithm != SignatureAlgorithm::Ed25519 {
            return Err(PrikkError::InvalidSignature(format!(
                "recognition claim {claim_id} MAINTAINER signature is not Ed25519"
            )));
        }
        let Some(adopted) = trust_policy
            .keys
            .iter()
            .find(|adopted| adopted.key_id == signature.key_id)
        else {
            continue;
        };
        let preimage = Signature::signed_bytes(
            SignatureAlgorithm::Ed25519,
            envelope.object_type,
            claim_id,
            SignerRole::Maintainer,
            &signature.key_id,
        )?;
        if prikk_crypto::verify_ed25519(&adopted.public_key, &preimage, &signature.signature_bytes)
            .is_err()
        {
            return Err(PrikkError::InvalidSignature(format!(
                "recognition claim {claim_id} MAINTAINER signature does not verify against \
                 adopted key {}",
                signature.key_id
            )));
        }
        if first_sound.is_none() {
            first_sound = Some(signature.key_id.clone());
        }
    }
    let Some(first_key_id) = first_key_id else {
        return Err(PrikkError::Integrity(format!(
            "recognition claim {claim_id} carries no MAINTAINER signature -- a claim is, by \
             definition, signed by the sender's maintainer key"
        )));
    };
    Ok(match first_sound {
        Some(key_id) => ClaimSignatureVerification::Sound { key_id },
        None => ClaimSignatureVerification::Unverifiable {
            key_id: first_key_id,
        },
    })
}

/// Order a batch of recognition claims for sealing (RFC 116 stage 5, N3's field finally used):
/// **a claim's block is sealed after every claim in the same batch whose block is one of its
/// parents.** Kahn's algorithm over a graph built from each claim's own `block_id`/
/// `parent_block_ids` -- the same shape `merge_evidence.rs`'s own `topological_order` already
/// uses for a different node type, restated here because that function is `pub(crate)` to a
/// different graph (Block ids already held locally) and this one's nodes are claim ids decoded
/// from a batch that may include blocks this repository does not hold at all.
///
/// **Only intra-batch edges matter (§1.1).** A claim's `parent_block_ids` may name a block that is
/// not any claim in this batch's own `block_id` -- an already-sealed ancestor, or simply absent.
/// Such a parent is ignored for ordering, never refused on: refusing would break the ordinary
/// incremental case, where the true parent was sealed by a previous sync and is not part of this
/// batch at all.
///
/// **Deterministic (§1.1): ties are broken by the claim id itself**, the same node identity this
/// function returns an order over -- independent chains within one batch can interleave in more
/// than one valid order, and without a fixed tie-break two runs over the identical batch could
/// disagree, which would be untestable and would make two receivers diverge for no reason.
///
/// **A cycle is a refusal, and a security property, not a tidiness one (§1.1).** Blocks are
/// content-addressed and genuinely form a DAG, so an honest batch cannot contain one -- but a claim
/// is an assertion, not a fact, and a hostile sender can assert a cycle a receiver holding neither
/// block has no way to disprove on its own. The sort terminates (it is bounded by the batch size)
/// and refuses, naming every claim still unordered when the queue empties -- never loops, never
/// silently drops an edge to force progress.
pub fn order_claims_for_sealing(
    object_store: &impl ObjectReader,
    claim_ids: &[ObjectId],
) -> Result<Vec<ObjectId>> {
    let mut claims: BTreeMap<ObjectId, RecognitionClaimPayload> = BTreeMap::new();
    for &claim_id in claim_ids {
        let envelope = object_store
            .read_typed(claim_id, ObjectType::RecognitionClaim)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!("recognition claim {claim_id} does not exist"))
            })?;
        let payload = RecognitionClaimPayload::decode_canonical(&envelope.canonical_payload)?;
        claims.insert(claim_id, payload);
    }

    // Each batch block maps to exactly one claim -- two distinct claim objects naming the same
    // block_id would have to disagree on patch_ids or parent_block_ids to be distinct objects at
    // all (block_id alone does not determine a claim's identity), which makes at least one of them
    // a lie about that same block. Refuse rather than guess which one to believe for ordering.
    let mut block_to_claim: BTreeMap<ObjectId, ObjectId> = BTreeMap::new();
    for (&claim_id, payload) in &claims {
        if let Some(&existing) = block_to_claim.get(&payload.block_id) {
            return Err(PrikkError::Integrity(format!(
                "claims {existing} and {claim_id} both name block {} -- refusing to order an \
                 ambiguous batch",
                payload.block_id
            )));
        }
        block_to_claim.insert(payload.block_id, claim_id);
    }

    let mut remaining_parents: BTreeMap<ObjectId, usize> = BTreeMap::new();
    let mut children: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    for (&claim_id, payload) in &claims {
        let count = payload
            .parent_block_ids
            .iter()
            .filter(|parent_block_id| block_to_claim.contains_key(*parent_block_id))
            .count();
        remaining_parents.insert(claim_id, count);
        for parent_block_id in &payload.parent_block_ids {
            if let Some(&parent_claim_id) = block_to_claim.get(parent_block_id) {
                children.entry(parent_claim_id).or_default().push(claim_id);
            }
        }
    }

    // `remaining_parents`/`children` were both built by iterating `claims`, a `BTreeMap`, so every
    // per-parent `children` entry already accumulates in ascending claim-id order -- the initial
    // `ready` set below is sorted the same way `merge_evidence.rs`'s own precedent is, defensively,
    // even though a `BTreeMap` iterator already yields it in that order.
    let mut ready: Vec<ObjectId> = remaining_parents
        .iter()
        .filter(|(_, count)| **count == 0)
        .map(|(id, _)| *id)
        .collect();
    ready.sort_unstable();
    let mut queue: VecDeque<ObjectId> = ready.into();
    let mut order = Vec::with_capacity(claims.len());
    while let Some(claim_id) = queue.pop_front() {
        order.push(claim_id);
        for &child in children.get(&claim_id).into_iter().flatten() {
            let entry = remaining_parents.get_mut(&child).ok_or_else(|| {
                PrikkError::Integrity(
                    "claim ordering lost a tracked child -- internal inconsistency".to_string(),
                )
            })?;
            *entry -= 1;
            if *entry == 0 {
                queue.push_back(child);
            }
        }
    }

    if order.len() != claims.len() {
        let stuck: Vec<String> = remaining_parents
            .keys()
            .filter(|id| !order.contains(id))
            .map(ObjectId::to_string)
            .collect();
        return Err(PrikkError::Integrity(format!(
            "recognition claims for sealing contain a cycle -- refusing to order: {}",
            stuck.join(", ")
        )));
    }
    Ok(order)
}

#[cfg(test)]
mod tests;
