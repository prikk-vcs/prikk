//! RFC 116 stage 3 (design-v1.md §1.2 as amended, §4, §5; `stage-3-sender-side-handoff-v1.md`) --
//! the sender side: given a ref name and a have-list received from the other side, produce the
//! `PEXCH001` that closes their gap.
//!
//! **This is the first `RecognitionClaimPayload` producer in the project's history, and closes the
//! free-schema-amendment window D6 and N3 both used.** Every other module in this crate --
//! including this one's own `summary`/`have_list` siblings and `compute_sync_delta` -- only reads
//! or checks claims; this is the only place a claim is ever built and signed. After this merges,
//! any further change to `RecognitionClaimPayload` costs a second schema version with two
//! contracts carried forever (RFC 114).
//!
//! **One ref, one artifact** (design §1.2's amendment). No list of refs, no union artifact -- that
//! was the error the RFC 116 claim-design stress round caught before implementation.
//!
//! **Claims carry the block's `patch_ids` and `parent_block_ids` verbatim and complete, never
//! trimmed to the delta (§3).** Trimming would be tempting -- the receiver already holds some of
//! what a claimed block names -- but it is not optional:
//! - **A trimmed claim is a false statement about the block.** The claim asserts "block B contains
//!   these patches"; trimmed, it asserts something untrue.
//! - **It would break the receiver's own lie-detector.** `check_recognition_claim_consistency`
//!   compares a claim against a block the receiver holds by exact sequence equality (D6, N3). A
//!   trimmed claim about a block the receiver has would read `Contradicted` -- the receiver would
//!   correctly conclude the sender lied.
//! - **D7 already handles the overlap on the receiving side.** Patches the receiver has sealed are
//!   skipped there; absence is the only refusal. Trimming here would be solving, on the sending
//!   side, a problem D7 already solves correctly on the receiving one.
//! - No new disclosure results: every extra id is one the receiver already named in its own
//!   have-list.
//!
//! **The signer must be locally trusted** (`verify_signer_trusted`), checked before any claim is
//! signed. Signing a claim is not sealing and confers nothing on the receiver -- but a repository
//! should not emit signed assertions under a key it does not itself adopt. Secure by default,
//! stated here so a later reader does not remove the check as apparently redundant.
//!
//! **Absence is not a refusal (§4), carried forward from stage 2's review.** A ref the sender does
//! not hold, or a have-list naming a ref the receiver does not hold, both produce ordinary deltas
//! (empty and full-reachable-set respectively) through `compute_sync_delta`'s own existing
//! behaviour -- nothing here adds a new refusal on top. An empty delta is reported as
//! [`SyncArtifactOutcome::AlreadyInSync`], not an error and not a pointless artifact -- and, per
//! Stage 4's own no-op precedent, adopts no signer trust check either: nothing is signed, so
//! nothing needs to be.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RecognitionClaimPayload, RefKind,
    RefStatePayload, TagPayload,
};

use crate::foundation::layout::RepositoryLayout;
use crate::maintainer_signing::{MaintainerSigner, maintainer_signature};
use crate::merge_evidence::ancestors_inclusive;
use crate::object_store::{ObjectReadSnapshot, ObjectReader, ObjectWriteSession, ObjectWriter};
use crate::patch_exchange::{ExchangeExportReport, export_exchange_artifact};
use crate::refs::{RefStore, validate_local_branch_ref};
use crate::trust::{GatedOperation, verify_signer_trusted};

use super::have_list::{
    DEFAULT_HAVE_LIST_MAX_PATCH_COUNT, DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES, decode_have_list,
};
use super::{compute_sync_delta, resolve_branch_ref_tip};

/// Summary of a built `PEXCH001`, when the delta was non-empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncArtifactBuildReport {
    /// The ref this artifact closes the gap for.
    pub ref_name: String,
    /// Patch ids in the delta -- exactly what the artifact carries, per §5 row 1.
    pub delta_patch_count: usize,
    /// Claims built, one per block that contains any delta patch (§2/§3).
    pub claim_count: usize,
    /// Tags carried (RFC 117 stage 3 §2) -- every local tag whose target block lies within the
    /// synced ref's ancestry, regardless of whether the delta itself is empty (see this function's
    /// own doc for why tag-travel is not gated on a non-empty patch delta).
    pub tag_count: usize,
    /// The underlying `export_exchange_artifact` report -- patch/blob/author-key/claim/tag counts as
    /// that function already reports them.
    pub export_report: ExchangeExportReport,
}

/// The result of [`build_sync_artifact`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncArtifactOutcome {
    /// The delta was empty -- this ref is already in sync. No artifact was built, nothing was
    /// signed, and nothing was persisted (§4).
    AlreadyInSync {
        /// The ref that was already in sync.
        ref_name: String,
    },
    /// The delta was non-empty. `bytes` is the `PEXCH001` artifact carrying it.
    Artifact {
        /// What was built.
        report: SyncArtifactBuildReport,
        /// The encoded `PEXCH001` bytes.
        bytes: Vec<u8>,
    },
}

/// Build the `PEXCH002` exchange artifact that closes `ref_name`'s gap, from a have-list received
/// from the other side. See the module doc for why claims are never trimmed to the delta.
///
/// **RFC 117 stage 3 §2: tag-travel is not gated on a non-empty patch delta.** A tag can be created
/// after two repositories are otherwise already fully in sync on a ref's patches -- the ordinary case
/// once stage 3 lands, since `prikk tag create` is usually run well after the content it names has
/// already propagated. Gating tag inclusion on `delta_patch_ids` being non-empty (the shape this
/// function had before stage 3) would mean that ordinary case never travels the tag at all, only ever
/// a delta rebuilt for some unrelated later reason. So the ref tip's ancestry is now always resolved
/// and the qualifying-tags set is always computed; [`SyncArtifactOutcome::AlreadyInSync`] fires only
/// when **both** the patch delta and the qualifying-tag set are empty.
pub fn build_sync_artifact(
    layout: &RepositoryLayout,
    ref_name: &str,
    have_list_bytes: &[u8],
    signer: &impl MaintainerSigner,
) -> Result<SyncArtifactOutcome> {
    // §1 step 1: decode the have-list -- its own digest/list self-consistency check (stage 2,
    // §1.3) applies here unchanged.
    let have_list = decode_have_list(
        have_list_bytes,
        DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
        DEFAULT_HAVE_LIST_MAX_PATCH_COUNT,
    )?;
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    if have_list.ref_name != canonical_ref {
        return Err(PrikkError::Integrity(format!(
            "have-list names ref {}, but {canonical_ref} was requested",
            have_list.ref_name
        )));
    }

    // §1 step 2: stage 2's own function, unchanged. Handles both absence cases already (§4): a
    // sender-absent ref resolves to an empty reachable set (empty delta); a have-list naming a
    // receiver-absent ref (empty list) makes the delta the full reachable set.
    let delta_patch_ids = compute_sync_delta(layout, &have_list)?;

    // §2: resolve the ref tip's ancestry -- unconditionally now (RFC 117 stage 3's own doc comment
    // above explains why tag-travel cannot be gated on a non-empty delta). The same walk
    // `patch_ids_reachable_from_block` and bundle export already use. No second traversal, no
    // patch->block index.
    let read_snapshot = ObjectReadSnapshot::open(layout)?;
    let ref_store = RefStore::new(layout.clone());
    let tip_block_id = resolve_branch_ref_tip(&read_snapshot, &ref_store, &canonical_ref)?;
    if !delta_patch_ids.is_empty() && tip_block_id.is_none() {
        return Err(PrikkError::Integrity(format!(
            "ref {canonical_ref} has a non-empty delta but no local tip -- inconsistent state"
        )));
    }
    let ancestors = match tip_block_id {
        Some(id) => ancestors_inclusive(&read_snapshot, id)?,
        None => BTreeMap::new(),
    };

    // RFC 117 stage 3 §2: every local tag whose target block lies within that ancestry travels,
    // regardless of whether the receiver already holds it -- sending a tag it already has is
    // harmless (objects are content-addressed and accept is idempotent), so there is no attempt to
    // compute a tag-level delta the way there is for patches.
    let mut tag_ids: Vec<ObjectId> = Vec::new();
    for pointer in ref_store.list_ref_pointers()? {
        if !pointer.ref_name.starts_with("tags/") {
            continue;
        }
        let ref_state_envelope = read_snapshot
            .read_typed(pointer.ref_state_id, ObjectType::RefState)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "tag ref {} names missing RefState {}",
                    pointer.ref_name, pointer.ref_state_id
                ))
            })?;
        let ref_state_payload = RefStatePayload::decode_canonical(
            &ref_state_envelope.canonical_payload,
            ref_state_envelope.schema_version,
        )?;
        if ref_state_payload.kind != RefKind::Tag {
            continue;
        }
        let tag_id = ref_state_payload.target_object_id;
        let tag_envelope = read_snapshot
            .read_typed(tag_id, ObjectType::Tag)?
            .ok_or_else(|| PrikkError::Integrity(format!("missing Tag object: {tag_id}")))?;
        let tag_payload = TagPayload::decode_canonical(&tag_envelope.canonical_payload)?;
        if ancestors.contains_key(&tag_payload.target_block_id) {
            tag_ids.push(tag_id);
        }
    }
    tag_ids.sort_unstable();

    // §4: an empty delta and no travel-worthy tag means already in sync -- report it, build
    // nothing, sign nothing.
    if delta_patch_ids.is_empty() && tag_ids.is_empty() {
        return Ok(SyncArtifactOutcome::AlreadyInSync {
            ref_name: have_list.ref_name,
        });
    }

    // §3: the signer must be locally trusted before anything is signed. Checked here, after the
    // no-op return above and before any claim is built -- the no-op path performs no signing act,
    // so it needs no trust check, the same ordering Stage 4's `AlreadySealed` path already
    // established.
    verify_signer_trusted(layout, signer, GatedOperation::SyncBuild)?;

    let delta_set: BTreeSet<ObjectId> = delta_patch_ids.iter().copied().collect();
    let mut qualifying_block_ids: Vec<ObjectId> = ancestors
        .iter()
        .filter(|(_, block)| block.patch_ids.iter().any(|id| delta_set.contains(id)))
        .map(|(block_id, _)| *block_id)
        .collect();
    qualifying_block_ids.sort_unstable();

    // §3: build, sign, and persist one claim per qualifying block, verbatim and complete -- never
    // trimmed to the delta (module doc).
    let mut write_session = ObjectWriteSession::open(layout)?;
    let mut claim_ids = Vec::with_capacity(qualifying_block_ids.len());
    for block_id in &qualifying_block_ids {
        let block = ancestors.get(block_id).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "block {block_id} vanished from its own ancestry walk"
            ))
        })?;
        let claim_payload = RecognitionClaimPayload {
            block_id: *block_id,
            patch_ids: block.patch_ids.clone(),
            parent_block_ids: block.parent_block_ids.clone(),
        };
        let mut claim_envelope = ObjectEnvelope::unsigned(
            ObjectType::RecognitionClaim,
            1,
            claim_payload.to_canonical_bytes()?,
        );
        let claim_id = claim_envelope.object_id();
        claim_envelope.add_signature(maintainer_signature(
            signer,
            ObjectType::RecognitionClaim,
            claim_id,
        )?)?;
        let written_id = write_session.write_object(&claim_envelope)?;
        claim_ids.push(written_id);
    }

    // §1 step 5: now also carries `tag_ids` (RFC 117 stage 3 §2).
    let (export_report, bytes) =
        export_exchange_artifact(layout, &delta_patch_ids, &claim_ids, &tag_ids)?;

    Ok(SyncArtifactOutcome::Artifact {
        report: SyncArtifactBuildReport {
            ref_name: have_list.ref_name,
            delta_patch_count: delta_patch_ids.len(),
            claim_count: claim_ids.len(),
            tag_count: tag_ids.len(),
            export_report,
        },
        bytes,
    })
}

#[cfg(test)]
mod sender_test_support;
#[cfg(test)]
mod tests;
