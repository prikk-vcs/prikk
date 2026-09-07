//! RFC 115 Stage 3 handoff §4: the accept path. Implements Phase A-D in exactly the handoff's own
//! order -- see each phase's comment below for the corresponding numbered item. §4.1's invariant
//! governs the whole shape: **a refused exchange leaves nothing behind.** Every check that can fail
//! runs before any write; Phase D is reached only once nothing earlier could still refuse for a
//! reason attributable to the artifact's content.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType, RecognitionClaimPayload, SignerRole};

use crate::author::author_key_index::{
    check_author_key_conflict, lookup_author_key_entries, record_author_key_material,
    verify_author_signature_against_material,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::object_store::{ObjectReadSnapshot, ObjectWriteSession, ObjectWriter};
use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations, decode_patch_parent_ids,
};
use crate::patch_set_digest::compute_patch_set_digest;
use crate::recognition_claim::{check_recognition_claim_consistency, verify_claim_signature};
use crate::tag_travel::verify_tag_signature;
use crate::trust::load_maintainer_trust_policy_or_empty;
use crate::verify::AuthorSignatureVerification;

pub use crate::recognition_claim::ClaimSignatureVerification;
pub use crate::tag_travel::TagSignatureVerification;

use super::artifact::{
    DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT, DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES,
    decode_exchange_artifact,
};

/// DC-86 resource bound for [`accept_exchange_artifact`], checked before any decode or write --
/// the same shape `BundleImportOptions` gives `import_bundle`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AcceptOptions {
    /// Maximum any one of the artifact's four declared counts (patches, blobs, author keys, claims)
    /// may be. Refused before the section it governs is decoded.
    pub max_object_count: usize,
    /// Maximum encoded byte length the artifact may have. Refused before decoding starts at all.
    pub max_total_bytes: usize,
}

impl AcceptOptions {
    /// [`DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT`] and
    /// [`DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES`].
    #[must_use]
    pub const fn default_limits() -> Self {
        Self {
            max_object_count: DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT,
            max_total_bytes: DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES,
        }
    }

    /// Override the maximum declared count.
    #[must_use]
    pub const fn with_max_object_count(mut self, max_object_count: usize) -> Self {
        self.max_object_count = max_object_count;
        self
    }

    /// Override the maximum total encoded byte length.
    #[must_use]
    pub const fn with_max_total_bytes(mut self, max_total_bytes: usize) -> Self {
        self.max_total_bytes = max_total_bytes;
        self
    }
}

/// Summary of an exchange-artifact accept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptReport {
    /// Patches the artifact carried.
    pub patch_count: usize,
    /// Blobs the artifact carried.
    pub blob_count: usize,
    /// Recognition claims the artifact carried.
    pub claim_count: usize,
    /// Tag objects the artifact carried (RFC 117 stage 3 §2/§3).
    pub tag_count: usize,
    /// Patch, blob, claim, and tag objects that did not already exist in this repository's object store
    /// before this accept -- content-addressed, so a replayed accept (§4.3) reports zero here.
    pub written_object_count: usize,
    /// AUTHOR key entries the artifact carried and this accept recorded locally. Zero on a replayed
    /// accept, the same continuity-only semantics `BundleImportReport::recorded_author_key_count`
    /// already has.
    pub recorded_author_key_count: usize,
    /// One outcome per carried patch that has an AUTHOR-role signature at all; a patch with none is
    /// simply absent from this list, mirroring `RepositoryVerification`'s own
    /// `author_verification: Option<..>` precedent.
    pub author_signature_outcomes: Vec<(ObjectId, AuthorSignatureVerification)>,
    /// One outcome per carried claim's own MAINTAINER signature.
    pub claim_signature_outcomes: Vec<(ObjectId, ClaimSignatureVerification)>,
    /// One outcome per carried tag's own MAINTAINER signature (RFC 117 stage 3 §3) -- reported,
    /// never gating, the same treatment `claim_signature_outcomes` gets.
    pub tag_signature_outcomes: Vec<(ObjectId, TagSignatureVerification)>,
}

/// Accept a `PEXCH002` exchange artifact (handoff §4). Writes patches, blobs, recognition claims,
/// and Tag objects (RFC 117 stage 3 §3) -- and records AUTHOR key material -- **only** once every
/// fallible check has already passed. Never touches a ref, a Block, or the received namespace: this
/// is patch-level exchange (§0's "the unit is the patch"), and Stage 3 does not extend the accept
/// path into sealing (§1's scope cut) or into tag adoption (RFC 117 T4 -- a received Tag is stored
/// and reportable, never adopted; see `tag_travel::adopt_tag` for the separate, explicit act that
/// does adopt one).
pub fn accept_exchange_artifact(
    layout: &RepositoryLayout,
    bytes: &[u8],
    options: &AcceptOptions,
) -> Result<AcceptReport> {
    // Phase A, item 1: total byte length, before any decoding.
    if bytes.len() > options.max_total_bytes {
        return Err(PrikkError::MalformedData(format!(
            "patch-exchange artifact is {} bytes, over the configured limit of {} bytes",
            bytes.len(),
            options.max_total_bytes
        )));
    }

    // Phase A item 2 (each declared count against `max_object_count`) and Phase B item 3 (decode
    // all sections) both happen inside `decode_exchange_artifact` -- the same split `decode_bundle`
    // keeps between its own caller-checked total-byte bound and its own declared-count bounds.
    let decoded = decode_exchange_artifact(bytes, options.max_object_count)?;

    // Phase B item 4: recompute the patch-set digest over the decoded patches; refuse on mismatch.
    let mut decoded_patch_ids: Vec<ObjectId> = decoded
        .patches
        .iter()
        .map(ObjectEnvelope::object_id)
        .collect();
    decoded_patch_ids.sort_unstable();
    decoded_patch_ids.dedup();
    let recomputed_digest = compute_patch_set_digest(&decoded_patch_ids)?;
    if recomputed_digest != decoded.declared_digest {
        return Err(PrikkError::Integrity(
            "patch-exchange artifact's declared patch-set digest does not match its own decoded \
             patches -- refusing before any signature work"
                .to_string(),
        ));
    }

    // Phase B item 5: artifact-internal author-key conflict -- two different public keys for one
    // `key_id` within the artifact itself refuses the whole import (`import_bundle` learned this the
    // hard way; same check, same reason).
    let mut artifact_key_ids: BTreeMap<&str, [u8; 32]> = BTreeMap::new();
    for entry in &decoded.author_keys {
        match artifact_key_ids.get(entry.key_id.as_str()) {
            Some(existing) if *existing != entry.public_key => {
                return Err(PrikkError::MalformedData(format!(
                    "patch-exchange artifact's author-key section carries two different public \
                     keys for key_id {} -- refusing the whole exchange",
                    entry.key_id
                )));
            }
            Some(_) => {}
            None => {
                artifact_key_ids.insert(&entry.key_id, entry.public_key);
            }
        }
    }

    // Phase B item 5b: artifact-versus-repository conflict, read-only, as a cheap early refusal
    // before any signature work. Does **not** replace Phase D's check under the lock -- that one is
    // authoritative, because check-then-act without the lock is a race.
    for (&key_id, &public_key) in &artifact_key_ids {
        check_author_key_conflict(layout, key_id, public_key)?;
    }

    // Phase B item 6: closure completeness. Every blob a carried patch's operations reference must
    // be present -- in the artifact or already in this repository. `parent_patch_ids` is always
    // empty today; check it anyway and refuse if it is ever non-empty, because the day it stops
    // being empty this is the code that must not silently ignore it.
    //
    // Deliberately schema-blind, and deliberately runs before `decode_patch_operations` below --
    // Patch schema 2 handoff v2 amendment §3: this shadows `decode_patch_operations`'s own
    // schema-2-and-above refusal of a present tag 2, for both schemas, since it refuses a non-empty
    // value at *any* schema, including schema 1 where the field is legal-but-must-be-empty. It is
    // broader, not redundant -- do not remove it as a "simplification", and do not reorder these two
    // checks.
    let read_snapshot = ObjectReadSnapshot::open(layout)?;
    let artifact_blob_ids: BTreeSet<ObjectId> = decoded
        .blobs
        .iter()
        .map(ObjectEnvelope::object_id)
        .collect();
    for envelope in &decoded.patches {
        let parent_patch_ids = decode_patch_parent_ids(&envelope.canonical_payload)?;
        if !parent_patch_ids.is_empty() {
            return Err(PrikkError::Integrity(format!(
                "patch {} carries a non-empty parent_patch_ids -- this field is always empty \
                 today and there is nothing defined to walk there yet; refusing rather than \
                 silently ignoring it",
                envelope.object_id()
            )));
        }
        for operation in
            decode_patch_operations(&envelope.canonical_payload, envelope.schema_version)?
        {
            for blob_id in referenced_blob_ids(&operation.kind) {
                if !artifact_blob_ids.contains(&blob_id)
                    && !read_snapshot.contains_object(ObjectType::Blob, blob_id)
                {
                    return Err(PrikkError::Integrity(format!(
                        "patch {} references blob {blob_id}, which is neither carried by this \
                         artifact nor already present in this repository -- refusing the whole \
                         exchange, no partial apply",
                        envelope.object_id()
                    )));
                }
            }
        }
    }

    // Phase C item 7: every carried patch's AUTHOR signature, verified against the union of this
    // repository's already-recorded material and the artifact's own transported material for that
    // `key_id` -- the shared core the handoff's §4.2 item 7 rules must be reused, not duplicated.
    let mut author_signature_outcomes = Vec::with_capacity(decoded.patches.len());
    for envelope in &decoded.patches {
        let Some(signature) = envelope
            .signatures
            .iter()
            .find(|signature| signature.signer_role == SignerRole::Author)
        else {
            continue;
        };
        let mut candidates = lookup_author_key_entries(layout, &signature.key_id)?;
        candidates.extend(
            decoded
                .author_keys
                .iter()
                .filter(|entry| entry.key_id == signature.key_id)
                .cloned(),
        );
        let Some((key_id, verifies)) =
            verify_author_signature_against_material(envelope, &candidates)?
        else {
            continue;
        };
        let outcome = if verifies {
            AuthorSignatureVerification::Sound { key_id }
        } else {
            AuthorSignatureVerification::Unverifiable { key_id }
        };
        author_signature_outcomes.push((envelope.object_id(), outcome));
    }

    // Phase C items 8-9: every claim's own MAINTAINER signature, then every claim against blocks
    // this repository already holds.
    let trust_policy = load_maintainer_trust_policy_or_empty(layout)?;
    let mut claim_signature_outcomes = Vec::with_capacity(decoded.claims.len());
    for envelope in &decoded.claims {
        let claim_id = envelope.object_id();
        let outcome = verify_claim_signature(envelope, &trust_policy)?;
        claim_signature_outcomes.push((claim_id, outcome));

        let payload = RecognitionClaimPayload::decode_canonical(&envelope.canonical_payload)?;
        match check_recognition_claim_consistency(&read_snapshot, &payload)? {
            crate::recognition_claim::RecognitionClaimConsistency::Contradicted { .. } => {
                return Err(PrikkError::Integrity(format!(
                    "recognition claim {claim_id} contradicts a block this repository already \
                     holds -- refusing the whole exchange"
                )));
            }
            crate::recognition_claim::RecognitionClaimConsistency::Consistent
            | crate::recognition_claim::RecognitionClaimConsistency::BlockAbsent => {}
        }
    }

    // RFC 117 stage 3 §3: every carried tag's own MAINTAINER signature, reported and never gating --
    // the same treatment claims get (Phase C items 8-9, immediately above). No consistency check
    // against a held block: a Tag's own identity is its `patch_set_digest`/`patch_count`, which say
    // nothing about which local block (if any) currently matches -- that is `resolve_patch_set_digest`
    // and `sync tags`'s job, not accept's (T2/T4 keep resolution and adoption out of this path).
    let mut tag_signature_outcomes = Vec::with_capacity(decoded.tags.len());
    for envelope in &decoded.tags {
        let tag_id = envelope.object_id();
        let outcome = verify_tag_signature(envelope, &trust_policy)?;
        tag_signature_outcomes.push((tag_id, outcome));
    }

    // Phase D item 10 (patches and blobs only -- see the claim-write note below): write the patch
    // and blob objects. Content-addressed and idempotent -- a replayed accept (§4.3) writes nothing
    // new here.
    let mut object_store = ObjectWriteSession::open(layout)?;
    let mut written_object_count = 0_usize;
    for envelope in decoded.patches.iter().chain(decoded.blobs.iter()) {
        let id = envelope.object_id();
        if !object_store.contains_object(envelope.object_type, id)? {
            written_object_count = written_object_count.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("exchange accept written-object count overflow".to_string())
            })?;
        }
        object_store.write_object(envelope)?;
    }

    // Phase D item 11: under a single `ActiveLock`, validate every entry against this repository's
    // material, then record every entry -- never check-then-record one entry at a time
    // (`multi-key-import-partial-write-v1.md`). `import_bundle` already does this correctly; same
    // structure, copied.
    let mut recorded_author_key_count = 0_usize;
    {
        let active_lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
        for (&key_id, &public_key) in &artifact_key_ids {
            check_author_key_conflict(layout, key_id, public_key)?;
        }
        for entry in &decoded.author_keys {
            record_author_key_material(layout, &entry.key_id, entry.public_key, &active_lock)?;
            recorded_author_key_count =
                recorded_author_key_count.checked_add(1).ok_or_else(|| {
                    PrikkError::Integrity(
                        "exchange accept recorded-author-key count overflow".to_string(),
                    )
                })?;
        }
    }

    // Claims and tags are written last, only after item 11 has fully succeeded. Design §8.1 names
    // claims separately from ordinary objects -- "no key material, and no claim, may be recorded
    // from an exchange that failed" -- unlike patches and blobs, which §8.1 explicitly allows to
    // survive a failed exchange (content-addressed and harmless). Writing them earlier, alongside
    // patches and blobs, would leave one behind if the author-key record step above failed after an
    // earlier write -- caught in review (`RFC-115-stage-3-exchange-artifact-review-v1.md` §2) as
    // reachable, if narrow: a concurrent writer between Phase B's read-only conflict check and this
    // lock, or an I/O error during `record_author_key_material`. RFC 117 stage 3 §5 row 6 puts a Tag
    // object on the same terms explicitly: **a refused exchange records no tag.**
    for envelope in decoded.claims.iter().chain(decoded.tags.iter()) {
        let id = envelope.object_id();
        if !object_store.contains_object(envelope.object_type, id)? {
            written_object_count = written_object_count.checked_add(1).ok_or_else(|| {
                PrikkError::Integrity("exchange accept written-object count overflow".to_string())
            })?;
        }
        object_store.write_object(envelope)?;
    }

    Ok(AcceptReport {
        patch_count: decoded.patches.len(),
        blob_count: decoded.blobs.len(),
        claim_count: decoded.claims.len(),
        tag_count: decoded.tags.len(),
        written_object_count,
        recorded_author_key_count,
        author_signature_outcomes,
        claim_signature_outcomes,
        tag_signature_outcomes,
    })
}

/// Every blob id one decoded operation references -- the same three kinds `export_exchange_artifact`
/// (`artifact.rs`) and `export_bundle` (`bundle.rs`) each scan for, restated here for the accept
/// path's own closure check.
fn referenced_blob_ids(kind: &DecodedOperationKind) -> Vec<ObjectId> {
    match kind {
        DecodedOperationKind::CreateFile { blob_id, .. } => vec![*blob_id],
        DecodedOperationKind::ReplaceBinary {
            old_blob_id,
            new_blob_id,
            ..
        } => vec![*old_blob_id, *new_blob_id],
        DecodedOperationKind::DeleteNode {
            preimage: DecodedDeletePreimage::File { old_blob_id, .. },
            ..
        } => vec![*old_blob_id],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests;
