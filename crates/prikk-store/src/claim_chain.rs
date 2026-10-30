//! The claim chain walk (0.56.0 step 3e, design review v1's own 2B ruling, D1-D4): walking backward
//! from one named `RecognitionClaim` through `parent_block_ids` -> "which other held claim, if any,
//! recognizes that block" -> that claim's own `parent_block_ids` -> ... until the walk reaches a
//! parent that is a **reachable** block (sealed or received), structurally checking every link along
//! the way (a shared-block collision, a cycle, a merge claim, a missing link) without yet deciding
//! whether any link's own signature outcome is good enough to use -- that is each caller's own
//! policy (`extract` refuses on an invalid link anywhere in the chain; `show`, from 0.56.0 step 3f
//! onward, skips one instead), not this module's.
//!
//! **One walk, two policies** (the ruling's own words): this module only ever answers "what is the
//! chain, and where does it structurally fail" -- it never folds a patch and never decides whether an
//! `Unverifiable` or `Invalid` link is acceptable. `extract.rs` is this round's only caller; `show.rs`
//! is 3f's.
//!
//! **Built for `show.rs` to call next, without restructuring**: `walk_claim_chain` takes an already
//! -decoded starting claim (id, payload, signature outcome) rather than reading it itself, because
//! `show`'s own entry point (`claims_naming_patch`, searching by *patch* id) already has every
//! candidate decoded before it would call this; re-reading it here would be a second decode of the
//! same object for no reason.
//!
//! **[`block_is_sealed_or_received`] moved here from `show.rs`** (0.56.0 step 1b built it there for
//! its own one-hop case) -- the identical reachability question this walk's own base case needs,
//! not duplicated a second time. `show.rs`'s own single call site now calls this module's copy.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::Result;
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload, RefStatePayload};

use crate::foundation::container::decode_container_records;
use crate::foundation::fsutil::read_file_if_exists;
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::merge::evidence::ancestors_inclusive;
use crate::object_store::ObjectReader;
use crate::received::list_received_pointers;
use crate::recognition_claim::{ClaimChainSignatureOutcome, classify_claim_signature};
use crate::refs::{RefStore, resolve_ref_tip_block};
use crate::trust::load_maintainer_trust_policy_or_empty;

/// One link in a walked chain, root (oldest) to foot (the claim named by the caller).
#[derive(Debug, Clone)]
pub(crate) struct ClaimChainLink {
    pub(crate) claim_id: ObjectId,
    /// `RecognitionClaimPayload::block_id` -- the block this link recognizes.
    pub(crate) target_block_id: ObjectId,
    /// `RecognitionClaimPayload::parent_block_ids`, verbatim (for the report; the walk itself only
    /// ever uses this when it is empty or has exactly one entry -- more than one already refused
    /// before a link is ever pushed, see [`ClaimChainWalkError::SeveralParents`]).
    pub(crate) parent_block_ids: Vec<ObjectId>,
    /// `RecognitionClaimPayload::patch_ids`, verbatim -- the one thing a fold needs from this link
    /// that the report does not otherwise carry.
    pub(crate) patch_ids: Vec<ObjectId>,
    pub(crate) signature: ClaimChainSignatureOutcome,
    /// D1's own exception, surfaced: when this link was chosen because it was the *one* `Sound`
    /// candidate among two or more held claims recognizing [`Self::target_block_id`], every other
    /// candidate is named here (id and outcome) for visibility -- "use it, and list the others in
    /// the report." Empty whenever this link was the only candidate at its own hop (the ordinary
    /// case), never populated for the chain's own starting link (D1 only ever concerns a *parent*
    /// hop the walk resolves, never the caller's own already-chosen starting claim).
    pub(crate) other_candidates: Vec<(ObjectId, ClaimChainSignatureOutcome)>,
}

/// Where the walk stopped: a real, reachable Block, or genesis (the root link's own
/// `parent_block_ids` was empty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClaimChainAnchor {
    Genesis,
    Block(ObjectId),
}

/// The walk's own result: every link, root to foot, and where it stopped.
#[derive(Debug, Clone)]
pub(crate) struct ClaimChain {
    pub(crate) links: Vec<ClaimChainLink>,
    pub(crate) anchor: ClaimChainAnchor,
}

/// A structural failure the walk itself detects (D1/D3; D4's merge/parent cases) -- distinct from an
/// ordinary [`prikk_error::PrikkError`] (an I/O failure, a malformed object): every one of these is a
/// property of the *claims themselves*, not of this repository's own storage.
#[derive(Debug, Clone)]
pub(crate) enum ClaimChainWalkError {
    /// A merge-shaped claim (more than one declared parent) at `claim_id`, anywhere in the chain --
    /// `RecognitionClaimPayload` carries no `mainline_parent_id` at any position to pick one from.
    SeveralParents { claim_id: ObjectId, count: usize },
    /// `claim_id`'s own declared parent `parent` is neither a reachable block nor recognized by any
    /// held claim -- the walk could not take its next step.
    ParentNotSealed {
        claim_id: ObjectId,
        parent: ObjectId,
    },
    /// D1: two or more held claims recognize `block_id`, and either none or more than one of them is
    /// `Sound` -- refused rather than guessed. Every candidate is named, with its own outcome.
    SharedBlock {
        block_id: ObjectId,
        candidates: Vec<(ObjectId, ClaimChainSignatureOutcome)>,
    },
    /// The walk was about to revisit a block it has already visited earlier in this same chain --
    /// naming every claim id seen in the loop, oldest (already in the chain) to the one that would
    /// have closed it.
    Cycle { claim_ids: Vec<ObjectId> },
}

/// Is `block_id` itself sealed (reachable from a local ref tip) or received (reachable from a
/// received pointer's tip)? Moved here, verbatim, from `show.rs` (0.56.0 step 1b built it for that
/// module's own one-hop case; this walk's own base case needs the identical question, so it lives in
/// one place rather than two).
pub(crate) fn block_is_sealed_or_received(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    block_id: ObjectId,
) -> Result<bool> {
    let ref_store = RefStore::new(layout.clone());
    for pointer in ref_store.list_ref_pointers()? {
        let tip = resolve_tip_block_id(object_store, &pointer.ref_name, pointer.ref_state_id)?;
        if ancestors_inclusive(object_store, tip)?.contains_key(&block_id) {
            return Ok(true);
        }
    }
    for pointer in list_received_pointers(layout)? {
        let tip = resolve_tip_block_id(object_store, &pointer.ref_name, pointer.ref_state_id)?;
        if ancestors_inclusive(object_store, tip)?.contains_key(&block_id) {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Resolve one ref/received pointer's own `RefState` to its tip block id -- moved alongside
/// [`block_is_sealed_or_received`], its only caller, for the identical reason.
fn resolve_tip_block_id(
    object_store: &impl ObjectReader,
    pointer_name: &str,
    ref_state_id: ObjectId,
) -> Result<ObjectId> {
    let ref_state_envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity(format!(
                "ref {pointer_name} names missing RefState {ref_state_id}"
            ))
        })?;
    let ref_state_payload = RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )?;
    let (tip_block_id, _tag_envelope) = resolve_ref_tip_block(object_store, &ref_state_payload)?;
    Ok(tip_block_id)
}

/// One held claim recognizing some block, as the chain walk's own candidate map carries it --
/// decoded once, up front, in [`candidates_by_recognized_block`]'s one container scan.
#[derive(Debug, Clone)]
struct ClaimChainCandidate {
    claim_id: ObjectId,
    payload: RecognitionClaimPayload,
    signature: ClaimChainSignatureOutcome,
}

/// Build `block_id -> every held claim recognizing it` in one scan of the `RecognitionClaim`
/// container (the design report's own §1a recommendation, reused verbatim: computed once, at the
/// start of the walk, never rescanned per hop -- the same container-walk idiom
/// `claims_naming_patch`/`pending_patches_with_claims` already use, filtering on `payload.block_id`
/// instead of `payload.patch_ids`/a patch-id membership test).
fn candidates_by_recognized_block(
    layout: &RepositoryLayout,
) -> Result<BTreeMap<ObjectId, Vec<ClaimChainCandidate>>> {
    #[cfg(test)]
    let _whole_read_scope =
        crate::foundation::fsutil::whole_read_guard::declare("type-enumeration");
    let container_path = layout.container_slot_path(ObjectType::RecognitionClaim, ContainerSlot::A);
    let relative = layout.repository_relative(&container_path)?;
    let trust_policy = load_maintainer_trust_policy_or_empty(layout)?;
    let mut by_block: BTreeMap<ObjectId, Vec<ClaimChainCandidate>> = BTreeMap::new();
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
            let signature = classify_claim_signature(&record.envelope, &trust_policy);
            by_block
                .entry(payload.block_id)
                .or_default()
                .push(ClaimChainCandidate {
                    claim_id,
                    payload,
                    signature,
                });
        }
    }
    for candidates in by_block.values_mut() {
        candidates.sort_by_key(|candidate| candidate.claim_id);
    }
    Ok(by_block)
}

/// How many of `candidates` are `Sound` -- D1's own tie-break test.
fn sound_count(candidates: &[ClaimChainCandidate]) -> usize {
    candidates
        .iter()
        .filter(|candidate| {
            matches!(
                candidate.signature,
                ClaimChainSignatureOutcome::Sound { .. }
            )
        })
        .count()
}

/// **Walk backward from an already-decoded starting claim** (D4): `start_claim_id`/`start_payload`/
/// `start_signature` are the caller's own claim -- `extract --claim <id>` the one it was given
/// directly, a future `show` caller one of `claims_naming_patch`'s own candidates -- decoded once by
/// the caller, not re-read here.
///
/// Returns the structural result: `Ok(Ok(chain))` for a chain the walk could fully build (every
/// link's own signature outcome is still carried, unjudged -- the caller decides whether an
/// `Invalid`/`Unverifiable` link anywhere in it is acceptable), or `Ok(Err(reason))` for a structural
/// failure (D1's shared-block collision, D3's cycle, a merge claim at any link, or a dead end with
/// no reachable block and no claim recognizing it). An `Err` at the outer level is an ordinary
/// [`prikk_error::PrikkError`] -- a container or object this walk needed to read did not decode, or a
/// repository-level I/O failure.
///
/// **Only blocks the walk actually visits are checked for a shared-block collision** (D1): the whole
/// `RecognitionClaim` container is scanned once up front to build the candidate map, but a block the
/// walk never has reason to resolve a parent against -- because the chain never reaches it -- never
/// triggers this refusal, even if two or more held claims do collide on it elsewhere in the store.
pub(crate) fn walk_claim_chain(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    start_claim_id: ObjectId,
    start_payload: RecognitionClaimPayload,
    start_signature: ClaimChainSignatureOutcome,
) -> Result<std::result::Result<ClaimChain, ClaimChainWalkError>> {
    let by_block = candidates_by_recognized_block(layout)?;

    let mut links = vec![ClaimChainLink {
        claim_id: start_claim_id,
        target_block_id: start_payload.block_id,
        parent_block_ids: start_payload.parent_block_ids.clone(),
        patch_ids: start_payload.patch_ids.clone(),
        signature: start_signature,
        other_candidates: Vec::new(),
    }];
    let mut visited: BTreeSet<ObjectId> = BTreeSet::new();
    visited.insert(start_payload.block_id);
    let mut current_parents = start_payload.parent_block_ids;

    loop {
        let parent = match current_parents.as_slice() {
            [] => {
                links.reverse();
                return Ok(Ok(ClaimChain {
                    links,
                    anchor: ClaimChainAnchor::Genesis,
                }));
            }
            [single] => *single,
            many => {
                // Safe: `links` always has at least the starting link.
                let claim_id = links
                    .last()
                    .map(|link| link.claim_id)
                    .unwrap_or(start_claim_id);
                return Ok(Err(ClaimChainWalkError::SeveralParents {
                    claim_id,
                    count: many.len(),
                }));
            }
        };

        if block_is_sealed_or_received(layout, reader, parent)? {
            links.reverse();
            return Ok(Ok(ClaimChain {
                links,
                anchor: ClaimChainAnchor::Block(parent),
            }));
        }

        if visited.contains(&parent) {
            let mut claim_ids: Vec<ObjectId> = links.iter().map(|link| link.claim_id).collect();
            // Name the candidate(s) that would have closed the loop too, when exactly one
            // unambiguously would have (the general shared-block case under a cycle is reported as
            // a cycle regardless -- D1's own ambiguity machinery is not re-run here).
            if let Some(candidates) = by_block.get(&parent) {
                claim_ids.extend(candidates.iter().map(|candidate| candidate.claim_id));
            }
            return Ok(Err(ClaimChainWalkError::Cycle { claim_ids }));
        }

        let candidates = match by_block.get(&parent) {
            Some(candidates) => candidates.clone(),
            None => Vec::new(),
        };
        let (chosen, other_candidates) = match candidates.len() {
            0 => {
                let claim_id = links
                    .last()
                    .map(|link| link.claim_id)
                    .unwrap_or(start_claim_id);
                return Ok(Err(ClaimChainWalkError::ParentNotSealed {
                    claim_id,
                    parent,
                }));
            }
            1 => {
                let chosen = candidates
                    .into_iter()
                    .next()
                    .unwrap_or_else(|| unreachable!("candidates.len() == 1 just checked"));
                (chosen, Vec::new())
            }
            _ => {
                if sound_count(&candidates) == 1 {
                    let chosen_id = candidates
                        .iter()
                        .find(|candidate| {
                            matches!(
                                candidate.signature,
                                ClaimChainSignatureOutcome::Sound { .. }
                            )
                        })
                        .map(|candidate| candidate.claim_id)
                        .unwrap_or_else(|| unreachable!("sound_count == 1 just checked"));
                    let other_candidates = candidates
                        .iter()
                        .filter(|candidate| candidate.claim_id != chosen_id)
                        .map(|candidate| (candidate.claim_id, candidate.signature.clone()))
                        .collect();
                    let chosen = candidates
                        .into_iter()
                        .find(|candidate| candidate.claim_id == chosen_id)
                        .unwrap_or_else(|| unreachable!("chosen_id was just found in candidates"));
                    (chosen, other_candidates)
                } else {
                    let reported = candidates
                        .iter()
                        .map(|candidate| (candidate.claim_id, candidate.signature.clone()))
                        .collect();
                    return Ok(Err(ClaimChainWalkError::SharedBlock {
                        block_id: parent,
                        candidates: reported,
                    }));
                }
            }
        };

        visited.insert(parent);
        current_parents = chosen.payload.parent_block_ids.clone();
        links.push(ClaimChainLink {
            claim_id: chosen.claim_id,
            target_block_id: chosen.payload.block_id,
            parent_block_ids: chosen.payload.parent_block_ids.clone(),
            patch_ids: chosen.payload.patch_ids.clone(),
            signature: chosen.signature,
            other_candidates,
        });
    }
}

#[cfg(test)]
mod tests;
