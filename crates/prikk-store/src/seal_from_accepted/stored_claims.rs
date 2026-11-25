//! The stored recognition claims a `sync seal` batch needs, read as a **subset** (0.59.0 P5a3).
//!
//! A batch used to decode every stored claim, keep them all and index them three ways (`enumerate_stored_claims`, `ClaimsIndex::build`): about 1 KB
//! per stored claim for the whole life of the batch, in a repository that may hold far more accepted claims than the batch seals. The two questions
//! the batch asks of the stored claims need much less (P5a §3, proved from the code of each):
//!
//! - **The order-ambiguity check** ([`super::refuse_if_order_ambiguous_over`]) skips every claim whose patches do not overlap the claim being sealed, so
//!   it reads **only the stored claims that name a patch of a batch claim**, in stored order. Those are the *kept* claims, with the batch's own claims
//!   (a claim that names no patch is skipped by the check but its payload is still what `require_parents_reached` reads).
//! - **The parent check** ([`super::parent_reached_visiting`]) reads a claim **only through the block it is about**, for a parent that is neither in the
//!   ref's history nor a held Block: so what it needs of the stored claims is **`block_id` -> the claim about it**, for every stored claim (the compact
//!   index below), and that claim's payload **on demand**, by its offset in the claim container, only for such a parent.
//!
//! **What is kept:** for every stored claim one [`BlockClaim`] (its id and where its record is) under its block; for the kept claims, their payloads (the
//! two `Vec<ObjectId>` shrunk to their length) and the same patch/id indexes the batch always had ([`ClaimsIndex`]). **What is read:** one streaming pass
//! over the container (frame by frame, never the whole decoded set), re-run when the container's size or modification time changes, as before.
//!
//! **Meaning is kept.** A claim held as several records (format 7) is one claim: **the first record of an id is the one kept**, as the full read did, and
//! the first claim *for a block* is the one the block resolves to. A claim that is not in the subset is fetched whole when its payload is needed, so the
//! parent check decides what it decided; and a claim being sealed whose patches are **not** all in the batch's patch set (the plan was told other claims)
//! is checked against the whole container, so the order-ambiguity check never decides over a subset that could miss a claim it needs.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::hash::{BuildHasherDefault, Hasher};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload};

use super::ClaimRef;
use super::batch::ClaimsIndex;
use crate::foundation::container::{
    ContainerRecordStatus, decode_container_records, decode_frames_with,
};
use crate::foundation::frame_source::SliceFrameSource;
use crate::foundation::fsutil::{read_file_if_exists, read_file_range_if_exists};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout, persisted_object_types};

/// The batch a read is for: its claims' ids. (The patches they name are read from the claim container itself, in the read's first pass.)
#[derive(Debug, Clone)]
pub(super) struct BatchScope {
    claim_ids: BTreeSet<ObjectId>,
}

impl BatchScope {
    pub(super) fn of(claim_ids: &[ObjectId]) -> Self {
        Self {
            claim_ids: claim_ids.iter().copied().collect(),
        }
    }
}

/// Where a stored claim's record is in the claim container, and whose claim it is.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BlockClaim {
    pub(crate) claim_id: ObjectId,
    pub(super) offset: u64,
    len: u32,
    /// The claim's position among the kept payloads; [`NOT_KEPT`] when its payload is read from its record when asked for.
    kept: u32,
}

/// `BlockClaim::kept` for a claim whose payload is not held.
const NOT_KEPT: u32 = u32::MAX;

/// A hasher for keys that are already hashes (an `ObjectId` is a digest): its first eight bytes are the hash. A parent check looks a block up once per
/// step of a chain, so the lookup's cost is the batch's cost; this one is a handful of instructions.
#[derive(Default)]
pub(super) struct IdHasher(u64);

impl Hasher for IdHasher {
    fn finish(&self) -> u64 {
        self.0
    }

    fn write(&mut self, bytes: &[u8]) {
        for chunk in bytes.chunks(8) {
            let mut word = [0_u8; 8];
            for (to, from) in word.iter_mut().zip(chunk) {
                *to = *from;
            }
            self.0 = (self.0 ^ u64::from_le_bytes(word)).rotate_left(5);
        }
    }
}

/// What a batch holds of the stored claims.
pub(crate) struct StoredClaims {
    /// Every stored claim, by the block it is about: the **first** claim (the first record of each id, in stored order) for a block. One entry per stored
    /// claim and nothing else (its id and where its record is).
    by_block: HashMap<ObjectId, BlockClaim, BuildHasherDefault<IdHasher>>,
    /// The claims the batch's checks read as payloads, in stored order (all of them when the read was not scoped).
    kept: Vec<(ObjectId, RecognitionClaimPayload)>,
    /// `by_patch` and `by_id` over `kept`; its `by_block` is empty (`by_block` above covers every claim).
    kept_index: ClaimsIndex,
    /// The patches the subset covers (the patches of the batch's claims); `None` when every claim is kept.
    covered: Option<BTreeSet<ObjectId>>,
    /// Payloads fetched by offset (a parent claim outside the subset), so a chain walked again is not read again.
    fetched: std::cell::RefCell<BTreeMap<ObjectId, std::rc::Rc<RecognitionClaimPayload>>>,
}

impl StoredClaims {
    /// Read the stored claims once, as a stream over the container's bytes. `scope` `Some`: a first pass finds the patches of the batch's own claims, and a
    /// second keeps the payloads of the claims that name one of them (and the batch's own); `None` keeps every claim (what a caller with no batch to scope
    /// to gets: the whole set, as before). Neither pass holds the decoded set.
    pub(super) fn read(layout: &RepositoryLayout, scope: Option<&BatchScope>) -> Result<Self> {
        #[cfg(test)]
        super::BATCH_READS.with(|reads| reads.set((reads.get().0 + 1, reads.get().1)));
        debug_assert!(
            persisted_object_types().contains(&ObjectType::RecognitionClaim),
            "RecognitionClaim must remain a persisted, containerized object type"
        );
        #[cfg(test)]
        let _whole_read_scope =
            crate::foundation::fsutil::whole_read_guard::declare("type-enumeration");
        let relative = layout.repository_relative(
            &layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A),
        )?;
        let bytes =
            read_file_if_exists(layout.repository_mutation_root(), &relative)?.unwrap_or_default();

        // Pass 1: the patches of the batch's own claims, decoding only those records.
        let covered = match scope {
            None => None,
            Some(scope) => {
                let mut patches = BTreeSet::new();
                let mut source = SliceFrameSource::new(&bytes);
                decode_frames_with(ObjectType::RecognitionClaim, &mut source, None, |record| {
                    if scope.claim_ids.contains(&record.envelope.object_id()) {
                        let claim = RecognitionClaimPayload::decode_canonical(
                            &record.envelope.canonical_payload,
                        )?;
                        patches.extend(claim.patch_ids);
                    }
                    Ok(())
                })?;
                Some(patches)
            }
        };

        // Pass 2: every record, in order. The payload is kept for the claims the batch's checks read; the claim's block and id are kept for all.
        // RFC 156 §5b: one entry per claim **id**, not per record -- a format-7 repository may hold several records for one claim, and a claim listed
        // twice would enter a `sync seal` batch twice. Records of one id carry the same payload (it is what the id hashes), so **the first record is kept**.
        let mut seen_ids: BTreeSet<ObjectId> = BTreeSet::new();
        let mut blocks: Vec<Option<(ObjectId, ObjectId, u32)>> = Vec::new();
        let mut kept: Vec<(ObjectId, RecognitionClaimPayload)> = Vec::new();
        let mut outcomes: Vec<(usize, usize)> = Vec::new();
        {
            let mut source = SliceFrameSource::new(&bytes);
            let summary =
                decode_frames_with(ObjectType::RecognitionClaim, &mut source, None, |record| {
                    let claim_id = record.envelope.object_id();
                    if !seen_ids.insert(claim_id) {
                        blocks.push(None);
                        return Ok(());
                    }
                    let mut payload = RecognitionClaimPayload::decode_canonical(
                        &record.envelope.canonical_payload,
                    )?;
                    let keep = match (scope, &covered) {
                        (Some(scope), Some(patches)) => {
                            scope.claim_ids.contains(&claim_id)
                                || payload.patch_ids.iter().any(|id| patches.contains(id))
                        }
                        _ => true,
                    };
                    let kept_at = if keep {
                        u32::try_from(kept.len()).unwrap_or(NOT_KEPT)
                    } else {
                        NOT_KEPT
                    };
                    blocks.push(Some((payload.block_id, claim_id, kept_at)));
                    if keep {
                        payload.patch_ids.shrink_to_fit();
                        payload.parent_block_ids.shrink_to_fit();
                        kept.push((claim_id, payload));
                    }
                    Ok(())
                })?;
            outcomes.extend(summary.record_outcomes.into_iter().filter_map(|outcome| {
                match outcome.status {
                    ContainerRecordStatus::Evaluated { frame_len, .. } => {
                        Some((outcome.offset, frame_len))
                    }
                    ContainerRecordStatus::Failed { .. } => None,
                }
            }));
        }
        drop(seen_ids);
        if outcomes.len() != blocks.len() {
            return Err(PrikkError::Integrity(
                "recognition claim container: the frames read and the records decoded disagree"
                    .to_string(),
            ));
        }
        let mut by_block: HashMap<ObjectId, BlockClaim, BuildHasherDefault<IdHasher>> =
            HashMap::with_capacity_and_hasher(blocks.len(), BuildHasherDefault::default());
        for (entry, (offset, len)) in blocks.into_iter().zip(outcomes) {
            let Some((block_id, claim_id, kept_at)) = entry else {
                continue;
            };
            // The first claim for a block: stored order, the first one wins.
            by_block.entry(block_id).or_insert(BlockClaim {
                claim_id,
                offset: offset as u64,
                kept: kept_at,
                len: u32::try_from(len).map_err(|_| {
                    PrikkError::Integrity(
                        "a recognition claim record is longer than the batch's index holds"
                            .to_string(),
                    )
                })?,
            });
        }
        let kept_index = ClaimsIndex::build_without_blocks(&kept);
        Ok(Self {
            by_block,
            kept,
            kept_index,
            covered,
            fetched: std::cell::RefCell::new(BTreeMap::new()),
        })
    }

    pub(super) fn block_claim(&self, block: &ObjectId) -> Option<&BlockClaim> {
        self.by_block.get(block)
    }

    /// Whether the subset holds every stored claim naming any of `patch_ids`: always when every claim was kept, else when all of them are patches of the
    /// batch the read was for.
    pub(super) fn covers(&self, patch_ids: &[ObjectId]) -> bool {
        match &self.covered {
            None => true,
            Some(patches) => patch_ids.iter().all(|id| patches.contains(id)),
        }
    }

    /// The kept claims, in stored order.
    pub(super) fn kept(&self) -> &[(ObjectId, RecognitionClaimPayload)] {
        &self.kept
    }

    /// `by_patch` / `by_id` over the kept claims.
    pub(super) fn kept_index(&self) -> &ClaimsIndex {
        &self.kept_index
    }

    /// The kept claim with this id: its payload (a batch claim is always kept).
    pub(super) fn kept_claim(&self, claim_id: &ObjectId) -> Option<&RecognitionClaimPayload> {
        let position = *self.kept_index.by_id.get(claim_id)?;
        self.kept.get(position).map(|(_, payload)| payload)
    }

    /// The claim stored for `block` (the first), if any: its id, and its payload -- borrowed from the subset when it is there, else read from its record.
    pub(super) fn claim_for_block(
        &self,
        layout: &RepositoryLayout,
        block: &ObjectId,
    ) -> Result<Option<(ObjectId, ClaimRef<'_>)>> {
        let Some(at) = self.block_claim(block) else {
            return Ok(None);
        };
        if at.kept != NOT_KEPT {
            if let Some((_, payload)) = self.kept.get(at.kept as usize) {
                return Ok(Some((at.claim_id, ClaimRef::Borrowed(payload))));
            }
        }
        if let Some(payload) = self.fetched.borrow().get(&at.claim_id) {
            return Ok(Some((
                at.claim_id,
                ClaimRef::Shared(std::rc::Rc::clone(payload)),
            )));
        }
        let payload = std::rc::Rc::new(Self::fetch(layout, at)?);
        self.fetched
            .borrow_mut()
            .insert(at.claim_id, std::rc::Rc::clone(&payload));
        Ok(Some((at.claim_id, ClaimRef::Shared(payload))))
    }

    /// One claim's payload from its own record in the container (a range read of that one frame).
    fn fetch(layout: &RepositoryLayout, at: &BlockClaim) -> Result<RecognitionClaimPayload> {
        let relative = layout.repository_relative(
            &layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A),
        )?;
        let bytes = read_file_range_if_exists(
            layout.repository_mutation_root(),
            &relative,
            at.offset,
            at.len as usize,
        )?
        .ok_or_else(|| {
            PrikkError::Integrity(
                "recognition claim container disappeared while sealing".to_string(),
            )
        })?;
        let replay = decode_container_records(ObjectType::RecognitionClaim, &bytes)?;
        let [record] = replay.records.as_slice() else {
            return Err(PrikkError::Integrity(format!(
                "recognition claim {} is not where the batch read it (the claim container changed under the batch)",
                at.claim_id
            )));
        };
        if record.envelope.object_id() != at.claim_id {
            return Err(PrikkError::Integrity(format!(
                "recognition claim {} is not where the batch read it (the claim container changed under the batch)",
                at.claim_id
            )));
        }
        RecognitionClaimPayload::decode_canonical(&record.envelope.canonical_payload)
    }
}

#[cfg(test)]
mod tests;
