//! RFC 117 stage 3 §3/§4 (`stage-3-tag-travel-and-adoption-handoff-v1.md`) -- what a repository does
//! with a Tag object once it has arrived via `accept_exchange_artifact`, short of adopting it.
//!
//! **T4: sync does not mint tags.** A received Tag object is stored and reportable (`accept.rs`
//! writes it and verifies its own MAINTAINER signature, never gating on the outcome -- the same
//! treatment a recognition claim already gets); this module is the separate, explicit act that turns
//! one into a local tag under the receiver's own key. **The sender's tag and the receiver's tag are
//! different objects with the same global identity** -- the same relationship their blocks already
//! have (RFC 115 design D2's block-identity argument, one object over): a locally re-signed Tag
//! carries a different `author_key_id` and therefore a different signature, so it is a different
//! `ObjectId`, even though its `patch_set_digest`/`patch_count` name the identical patch set.
//!
//! **"Received" has no new container.** Mirrors `patch_exchange.rs`'s own
//! `accepted_but_unsealed_patch_ids` precedent exactly, narrowed to a different object type: a
//! received tag is simply a `Tag` object present in this repository's store that no local `tags/*`
//! ref currently targets. A locally-created tag is always the target of its own `tags/*` `RefState`
//! by construction (`create_local_tag` writes both together, under one signer) -- so this is computed
//! fresh on every call, never persisted as its own pending-state container.

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload, Signature, SignatureAlgorithm, SignerRole, TagPayload,
};

use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::object_store::{ObjectReadSnapshot, ObjectReader, ObjectWriteSession, ObjectWriter};
use crate::patch_set_digest::{PatchSetDigest, PatchSetResolution, resolve_patch_set_digest};
use crate::refs::{RefPublication, RefStore, validate_local_tag_ref};
use crate::trust::{GatedOperation, MaintainerTrustPolicy, verify_signer_trusted};

/// The outcome of checking one received `Tag` object's own MAINTAINER signature. Shaped identically
/// to `ClaimSignatureVerification` and for the same reason (design T6 point 3, T3): **never gating**.
/// A tag naming a `key_id` this repository has not adopted still accepts -- it reads `Unverifiable`,
/// never `Sound`, and does not by itself refuse. Only a signature that fails to verify against a
/// `key_id` this repository *has* adopted refuses (a forged tag under a locally-trusted identity is
/// an integrity failure, not a trust question).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TagSignatureVerification {
    /// The signature verifies against a `key_id` this repository has adopted as a trusted
    /// maintainer.
    Sound {
        /// The MAINTAINER key id the signature named and verified against.
        key_id: String,
    },
    /// This repository has not adopted `key_id`, so the signature cannot be checked. Not a failure
    /// and not by itself a refusal -- see the type doc.
    Unverifiable {
        /// The MAINTAINER key id named, which this repository has not adopted.
        key_id: String,
    },
}

/// Verify `envelope`'s own MAINTAINER signature against `trust_policy`. The Tag-object counterpart of
/// `recognition_claim::verify_claim_signature` -- same shape, same reasoning, restated here because
/// the two object types carry no common signed-envelope verification helper (each is its own module
/// for its own reasons throughout this crate; see `AuthorSignatureVerification`'s own precedent for
/// why a third near-identical copy is not shared token-for-token). Refuses (`Err`) if `envelope`
/// carries no MAINTAINER signature at all, if the signature's algorithm is not Ed25519, or if it
/// fails to verify against an *adopted* key; reads `Unverifiable` rather than refusing when `key_id`
/// is simply not adopted.
pub(crate) fn verify_tag_signature(
    envelope: &ObjectEnvelope,
    trust_policy: &MaintainerTrustPolicy,
) -> Result<TagSignatureVerification> {
    let tag_id = envelope.object_id();
    let Some(signature) = envelope
        .signatures
        .iter()
        .find(|signature| signature.signer_role == SignerRole::Maintainer)
    else {
        return Err(PrikkError::Integrity(format!(
            "tag {tag_id} carries no MAINTAINER signature -- a tag is, by definition, signed by \
             its author's maintainer key"
        )));
    };
    if signature.algorithm != SignatureAlgorithm::Ed25519 {
        return Err(PrikkError::InvalidSignature(format!(
            "tag {tag_id} MAINTAINER signature is not Ed25519"
        )));
    }
    match trust_policy
        .keys
        .iter()
        .find(|adopted| adopted.key_id == signature.key_id)
    {
        None => Ok(TagSignatureVerification::Unverifiable {
            key_id: signature.key_id.clone(),
        }),
        Some(adopted) => {
            let preimage = Signature::signed_bytes(
                SignatureAlgorithm::Ed25519,
                envelope.object_type,
                tag_id,
                SignerRole::Maintainer,
                &signature.key_id,
            )?;
            if prikk_crypto::verify_ed25519(
                &adopted.public_key,
                &preimage,
                &signature.signature_bytes,
            )
            .is_err()
            {
                return Err(PrikkError::InvalidSignature(format!(
                    "tag {tag_id} MAINTAINER signature does not verify against adopted key {}",
                    signature.key_id
                )));
            }
            Ok(TagSignatureVerification::Sound {
                key_id: signature.key_id.clone(),
            })
        }
    }
}

/// Every `Tag` object present in this repository that no local `tags/*` ref currently targets --
/// "received but not (yet) adopted," computed fresh on every call. See the module doc for why this
/// carries no new persisted state.
pub fn received_tag_ids(layout: &RepositoryLayout) -> Result<Vec<ObjectId>> {
    debug_assert!(
        persisted_object_types().contains(&ObjectType::Tag),
        "Tag must remain a persisted, containerized object type"
    );
    let container_path = layout.container_slot_path(ObjectType::Tag, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let mut all_tag_ids: BTreeSet<ObjectId> = BTreeSet::new();
    if let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? {
        let replay = decode_container_records(ObjectType::Tag, &bytes)?;
        for record in replay.records {
            all_tag_ids.insert(record.envelope.object_id());
        }
    }

    let object_store = ObjectReadSnapshot::open(layout)?;
    let ref_store = RefStore::new(layout.clone());
    let mut locally_targeted: BTreeSet<ObjectId> = BTreeSet::new();
    for pointer in ref_store.list_ref_pointers()? {
        let ref_state_envelope = object_store
            .read_typed(pointer.ref_state_id, ObjectType::RefState)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "ref {} names missing RefState {}",
                    pointer.ref_name, pointer.ref_state_id
                ))
            })?;
        let ref_state_payload = RefStatePayload::decode_canonical(
            &ref_state_envelope.canonical_payload,
            ref_state_envelope.schema_version,
        )?;
        if ref_state_payload.kind == RefKind::Tag {
            locally_targeted.insert(ref_state_payload.target_object_id);
        }
    }

    Ok(all_tag_ids.difference(&locally_targeted).copied().collect())
}

/// One received tag's own name, signature outcome, and resolution state (`prikk sync tags`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedTagSummary {
    /// The received `Tag` object's own id.
    pub tag_id: ObjectId,
    /// The tag's own claimed name (`TagPayload.name`) -- untrusted, attacker-controlled data; never
    /// used to address a local ref without going through `validate_local_tag_ref` first (`adopt_tag`
    /// does exactly that).
    pub name: String,
    /// The tag's own MAINTAINER signature outcome, checked against this repository's *current*
    /// trust policy (live, not frozen at accept time -- policy can change between accept and query).
    pub signature_outcome: TagSignatureVerification,
    /// Whether the local repository currently holds a block with this tag's exact patch set.
    pub resolution: ReceivedTagResolution,
}

/// The resolution half of [`ReceivedTagSummary`] -- the same three states RFC 117 T2 already
/// distinguishes, restated here because [`resolve_patch_set_digest`] reports ambiguity as an `Err`
/// (a refusal to pick, appropriate for a caller about to act on it) where a listing command instead
/// wants it as a value to display alongside every other row, not a reason to abort the whole list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceivedTagResolution {
    /// Resolves to exactly this local block.
    Resolved(ObjectId),
    /// Not held locally yet -- ordinary, not a failure (T2).
    NotHeld,
    /// Two or more local blocks share this patch set; `detail` names them (`resolve_patch_set_digest`'s
    /// own refusal message, carried through unchanged).
    Ambiguous {
        /// The refusal detail, naming every candidate.
        detail: String,
    },
}

/// List every received tag (`received_tag_ids`) with its name, live signature outcome, and current
/// resolution state -- the read side of `prikk sync tags`.
pub fn list_received_tags(layout: &RepositoryLayout) -> Result<Vec<ReceivedTagSummary>> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let trust_policy = crate::trust::load_maintainer_trust_policy_or_empty(layout)?;
    let mut summaries = Vec::new();
    for tag_id in received_tag_ids(layout)? {
        let envelope = object_store
            .read_typed(tag_id, ObjectType::Tag)?
            .ok_or_else(|| PrikkError::Integrity(format!("missing Tag object: {tag_id}")))?;
        let payload = TagPayload::decode_canonical(&envelope.canonical_payload)?;
        let signature_outcome = verify_tag_signature(&envelope, &trust_policy)?;
        let resolution =
            match resolve_patch_set_digest(layout, payload.patch_set_digest, payload.patch_count) {
                Ok(PatchSetResolution::Resolved(block_id)) => {
                    ReceivedTagResolution::Resolved(block_id)
                }
                Ok(PatchSetResolution::NotHeld) => ReceivedTagResolution::NotHeld,
                Err(err) => ReceivedTagResolution::Ambiguous {
                    detail: err.to_string(),
                },
            };
        summaries.push(ReceivedTagSummary {
            tag_id,
            name: payload.name,
            signature_outcome,
            resolution,
        });
    }
    Ok(summaries)
}

/// A local tag freshly created by [`create_local_tag`] or [`adopt_tag`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTagCreation {
    /// The newly written `Tag` object's id.
    pub tag_object_id: ObjectId,
    /// The published `RefState` id.
    pub ref_state_id: ObjectId,
}

/// Create and publish a local, receiver-signed (or locally-signed) `Tag` at `requested_ref_name`,
/// naming `target_block_id` with the given `patch_set_digest`/`patch_count` -- the one place a
/// `Tag` object and its `tags/*` ref are ever written, so both `prikk tag create` (`prikk-cli`) and
/// [`adopt_tag`] go through this rather than each carrying its own copy of the write+publish shape.
/// Validates `requested_ref_name` itself (`validate_local_tag_ref`) rather than trusting a caller to
/// have already done so -- one caller's `requested_ref_name` is untrusted, attacker-controlled data
/// (a received tag's own claimed name), so this is the trust boundary, not a redundant re-check.
/// Refuses if a ref of that name already exists, the same "create-once" rule `prikk tag create`
/// already enforces.
#[allow(clippy::too_many_arguments)]
pub fn create_local_tag(
    layout: &RepositoryLayout,
    object_store: &mut ObjectWriteSession,
    requested_ref_name: &str,
    target_block_id: ObjectId,
    message: Option<String>,
    patch_set_digest: PatchSetDigest,
    patch_count: u64,
    signer: &impl MaintainerSigner,
) -> Result<LocalTagCreation> {
    layout.require_current_format()?;
    let canonical = validate_local_tag_ref(requested_ref_name)?;
    let ref_store = RefStore::new(layout.clone());
    if ref_store.read_current_ref_state_id(&canonical)?.is_some() {
        return Err(PrikkError::Integrity(format!(
            "tag {canonical} already exists"
        )));
    }

    let tag_payload = TagPayload {
        name: canonical.clone(),
        target_block_id,
        message,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
        patch_set_digest,
        patch_count,
    };
    let tag_envelope = signed_envelope(
        ObjectType::Tag,
        1,
        tag_payload.to_canonical_bytes()?,
        signer,
    )?;
    let tag_object_id = object_store.write_object(&tag_envelope)?;

    let ref_state_payload = RefStatePayload {
        ref_name: canonical.clone(),
        kind: RefKind::Tag,
        target_object_id: tag_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
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
        ref_name: canonical.clone(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: tag_object_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        signer,
    )?;
    let publication = RefPublication {
        ref_name: canonical,
        expected_previous_ref_state_id: None,
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    let published_ref_state_id = ref_store.publish_with_object_store(object_store, &publication)?;

    Ok(LocalTagCreation {
        tag_object_id,
        ref_state_id: published_ref_state_id,
    })
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

/// RFC 117 T4, the ruling this whole module exists to implement correctly: **adoption is the
/// receiver's own signed act, never conjured from someone else's assertion.** Resolves the one
/// received tag named `requested_name` (`validate_local_tag_ref`-canonicalized first, since a
/// received tag's own claimed name is untrusted) to a local block via stage 2's
/// `resolve_patch_set_digest`, then creates a **local** `Tag` naming that block, the identical
/// `patch_set_digest`/`patch_count`, signed under `signer`'s own key via [`create_local_tag`] -- the
/// ordinary tag-creation path, not a parallel one.
///
/// **Refuses, never picks:** no received tag by that name (0 candidates); more than one received tag
/// claims that name (ambiguous by name -- an implementation choice this module makes, distinct from
/// and in addition to T2's own patch-set ambiguity refusal, in the same "never picked, never
/// defaulted" spirit the rest of this arc keeps); the patch set is `NotHeld` locally; or the patch set
/// resolves to more than one local block (T2's own ambiguity refusal, propagated unchanged, already
/// naming every candidate).
///
/// **Does not gate on the received tag's own signature outcome.** `verify_tag_signature`'s result is
/// reported by `sync tags`/`accept`, never checked here -- the receiver's local tag is the receiver's
/// own assertion, verified against the receiver's own object store (the local block genuinely holds
/// this exact patch set); whether the *sender's* signature was `Sound` or `Unverifiable` says nothing
/// about that. `verify_signer_trusted` still gates, the same as `prikk tag create` -- that check is
/// about the *receiver's own* signer, not the received tag's.
pub fn adopt_tag(
    layout: &RepositoryLayout,
    requested_name: &str,
    signer: &impl MaintainerSigner,
) -> Result<LocalTagCreation> {
    let canonical = validate_local_tag_ref(requested_name)?;
    let object_store = ObjectReadSnapshot::open(layout)?;

    let mut matching: Vec<(ObjectId, TagPayload)> = Vec::new();
    for tag_id in received_tag_ids(layout)? {
        let envelope = object_store
            .read_typed(tag_id, ObjectType::Tag)?
            .ok_or_else(|| PrikkError::Integrity(format!("missing Tag object: {tag_id}")))?;
        let payload = TagPayload::decode_canonical(&envelope.canonical_payload)?;
        if payload.name == canonical {
            matching.push((tag_id, payload));
        }
    }

    let (source_tag_id, source_payload) = match matching.len() {
        0 => {
            return Err(PrikkError::Integrity(format!(
                "no received tag named {canonical} -- nothing to adopt"
            )));
        }
        1 => matching.pop().ok_or_else(|| {
            PrikkError::Integrity(
                "adopt_tag: exactly one match reported but none present -- internal inconsistency"
                    .to_string(),
            )
        })?,
        _ => {
            let ids = matching
                .iter()
                .map(|(id, _)| id.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(PrikkError::Integrity(format!(
                "{} received tags are named {canonical}, refusing to pick: {ids}",
                matching.len()
            )));
        }
    };

    let local_block_id = match resolve_patch_set_digest(
        layout,
        source_payload.patch_set_digest,
        source_payload.patch_count,
    )? {
        PatchSetResolution::NotHeld => {
            return Err(PrikkError::Integrity(format!(
                "received tag {canonical} ({source_tag_id})'s patch set is not held locally yet -- \
                 not enough of this repository's history has been synced to adopt it"
            )));
        }
        PatchSetResolution::Resolved(block_id) => block_id,
    };

    verify_signer_trusted(layout, signer, GatedOperation::SyncAdoptTag)?;

    let mut write_session = ObjectWriteSession::open(layout)?;
    create_local_tag(
        layout,
        &mut write_session,
        &canonical,
        local_block_id,
        source_payload.message,
        source_payload.patch_set_digest,
        source_payload.patch_count,
        signer,
    )
}

#[cfg(test)]
mod tests;
