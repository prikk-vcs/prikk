//! RFC 115 Stage 1 (design-v1.md §5, D4) — the patch-set digest: a canonical value over the set of
//! patch ids reachable from a ref, answering "are these two repositories the same?" at the level
//! where identity actually holds (RFC 115 §2.5-§2.7). Shaped like `state_root.rs`, not `id.rs`: a
//! comparison value, not a storable object -- a dedicated newtype, never an `ObjectId`, and no
//! `(object_type, schema_version)` pair.
//!
//! **RFC 117 T1: `PatchSetDigest` itself now lives in `prikk-object`**, re-exported here unchanged
//! (see this module's own `pub use` below) -- `TagPayload` carries one as its field 6, and
//! `prikk-object` cannot depend on `prikk-store`. Every function that *computes* a digest stays
//! here, the same split `MerkleRoot` (`prikk-object`) and `compute_state_root` (`prikk-store`)
//! already have.
//!
//! **Ref-kind support, per `RFC-115-stage-1-reachable-set-ruling-v1.md`:**
//! - `heads/*` (`RefKind::Branch`): `target_object_id` names the Block directly.
//! - `tags/*` (`RefKind::Tag`): `target_object_id` names a Tag object one hop away from the Block
//!   (`tag.rs`'s own model: "ref -> tag object -> block"); this module resolves that second hop
//!   itself, which `export_bundle` does not (`bundle-export-tag-ref-gap-v1.md`, a separate defect,
//!   not fixed here). The digest is the digest of the target Block's own patch-set closure only --
//!   the Tag object carries a name, an optional message and a signature, none of which are patches,
//!   and folding them in would make two repositories holding identical patches compare unequal over
//!   tag metadata, which inverts the whole purpose (ruling §2.2).
//! - `remotes/*` (received pointers): **refused explicitly**, not resolved. Not a difficulty --
//!   `received_index.rs`'s resolution is straightforward -- but the received namespace is precisely
//!   what patch-level exchange itself restructures (design D2/D5), and its digest semantics belong
//!   to Stage 3, not assumed here. Refusing by name check, before any `RefStore` lookup, so the
//!   error names the real reason rather than a misleading "ref does not exist" (ruling §2.3).
//! - Closed refs need no special case: `RefStatePayload.closed` gates further publication, it does
//!   not change what `target_object_id` names (ruling §2.4).

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use prikk_error::{PrikkError, Result};
use prikk_hash::sha256;
use prikk_object::{BlockPayload, ObjectId, ObjectType, RefStatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::merge::evidence::ancestors_inclusive;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::refs::{RefStore, resolve_ref_tip_block};

/// RFC 117 T1: the newtype itself now lives in `prikk-object` (`TagPayload` carries one, and
/// `prikk-object` cannot depend on this crate) -- re-exported here so every existing
/// `crate::patch_set_digest::PatchSetDigest` / `prikk_store::PatchSetDigest` path keeps resolving
/// to the same type, unchanged. Every function below that *computes* one stays here, matching
/// `MerkleRoot`/`compute_state_root`'s own split.
pub use prikk_object::PatchSetDigest;

const PATCH_SET_DIGEST_DOMAIN: &[u8] = b"PRIKK-PATCH-SET-DIGEST-v1";

/// Construct the exact preimage over an already-sorted, deduplicated, strictly-ascending slice of
/// patch ids. **Identity-bearing** (documented in `release-compatibility.md`'s frozen list): two
/// prikk versions must produce identical bytes over the same patch set, or the comparison this
/// digest exists for means nothing across an upgrade.
pub fn patch_set_digest_preimage(patch_ids: &[ObjectId]) -> Result<Vec<u8>> {
    if !prikk_object::canonical::is_strictly_sorted(patch_ids) {
        return Err(PrikkError::Integrity(
            "patch-set digest input is not strictly sorted and deduplicated".to_string(),
        ));
    }
    let count = u64::try_from(patch_ids.len())
        .map_err(|_| PrikkError::Integrity("patch-set digest count exceeds u64".to_string()))?;
    let mut preimage = Vec::with_capacity(PATCH_SET_DIGEST_DOMAIN.len() + 8 + patch_ids.len() * 32);
    preimage.extend_from_slice(PATCH_SET_DIGEST_DOMAIN);
    preimage.extend_from_slice(&count.to_be_bytes());
    for patch_id in patch_ids {
        preimage.extend_from_slice(patch_id.as_bytes());
    }
    Ok(preimage)
}

/// Compute the patch-set digest over an already-sorted, deduplicated slice of patch ids. The count
/// is hashed even when `patch_ids` is empty, so an empty set is distinguishable from a degenerate
/// one (matching `state_root.rs`'s own empty-case discipline, `compute_state_root`).
pub fn compute_patch_set_digest(patch_ids: &[ObjectId]) -> Result<PatchSetDigest> {
    Ok(PatchSetDigest(sha256(&patch_set_digest_preimage(
        patch_ids,
    )?)))
}

/// Every patch id reachable from `tip_block_id`'s ancestry, sorted and deduplicated -- the same
/// closure `export_bundle` walks (`bundle.rs:189,208-209`), narrowed to patch ids only (no blobs,
/// no attestations: Stage 1's scope is the patch set, nothing else, per the handoff's §6).
pub fn patch_ids_reachable_from_block(
    object_store: &impl ObjectReader,
    tip_block_id: ObjectId,
) -> Result<Vec<ObjectId>> {
    let ancestors = ancestors_inclusive(object_store, tip_block_id)?;
    let mut patch_ids: BTreeSet<ObjectId> = BTreeSet::new();
    for block in ancestors.values() {
        patch_ids.extend(block.patch_ids.iter().copied());
    }
    Ok(patch_ids.into_iter().collect())
}

/// Compute the patch-set digest for the closure reachable from `tip_block_id` directly -- the
/// block-rooted core, ref-resolution-agnostic, so a caller that has already resolved a ref through
/// any mechanism (`RefStore`, a received pointer, a future one) can reach the same computation
/// without this module re-deriving how to resolve it.
pub fn compute_patch_set_digest_from_block(
    object_store: &impl ObjectReader,
    tip_block_id: ObjectId,
) -> Result<PatchSetDigest> {
    compute_patch_set_digest(&patch_ids_reachable_from_block(object_store, tip_block_id)?)
}

/// RFC 117 T7: the digest and the count together, over one traversal -- `patch_count` is not new
/// information (`patch_set_digest_preimage` already hashes it), so a caller populating both of a
/// `TagPayload`'s field 6/7 should never pay for `patch_ids_reachable_from_block`'s own
/// `ancestors_inclusive` walk twice.
pub fn compute_patch_set_digest_and_count_from_block(
    object_store: &impl ObjectReader,
    tip_block_id: ObjectId,
) -> Result<(PatchSetDigest, u64)> {
    let patch_ids = patch_ids_reachable_from_block(object_store, tip_block_id)?;
    let count = crate::foundation::fsutil::len_to_u64(patch_ids.len())?;
    Ok((compute_patch_set_digest(&patch_ids)?, count))
}

/// Resolve `ref_name` to its target Block, per this module's own doc: `heads/*` directly, `tags/*`
/// through the Tag object's own `target_block_id` (the two-hop core itself is
/// `refs::resolve_ref_tip_block`, shared with `bundle.rs` and `patch_exchange.rs`). `remotes/*` is
/// refused explicitly before any `RefStore` lookup is attempted, so the refusal names the real
/// reason (ruling §2.3) rather than a misleading "ref does not exist".
fn resolve_ref_to_tip_block(
    layout: &RepositoryLayout,
    object_store: &impl ObjectReader,
    ref_name: &str,
) -> Result<ObjectId> {
    if ref_name.starts_with("remotes/") {
        return Err(PrikkError::Integrity(format!(
            "patch-set digest does not support received refs ({ref_name}) yet -- the received \
             namespace is what patch-level exchange itself restructures (RFC 115 design D2/D5); \
             its digest semantics belong to a later stage, not assumed here"
        )));
    }
    let ref_store = RefStore::new(layout.clone());
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(ref_name)? else {
        return Err(PrikkError::Integrity(format!(
            "ref {ref_name} does not exist, nothing to compute a patch-set digest for"
        )));
    };
    let ref_state_envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| PrikkError::Integrity(format!("missing RefState object: {ref_state_id}")))?;
    let ref_state_payload = RefStatePayload::decode_canonical(
        &ref_state_envelope.canonical_payload,
        ref_state_envelope.schema_version,
    )?;
    let (target_block_id, _tag_envelope) = resolve_ref_tip_block(object_store, &ref_state_payload)?;
    Ok(target_block_id)
}

/// The ref-rooted entry point, and the one two independent repositories can actually use: neither
/// side can name the other's Block id (that is the premise the digest exists to work around, RFC
/// 115 design §7), so a Block-rooted call alone would not serve the digest's own purpose.
pub fn compute_patch_set_digest_for_ref(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<PatchSetDigest> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let tip_block_id = resolve_ref_to_tip_block(layout, &object_store, ref_name)?;
    compute_patch_set_digest_from_block(&object_store, tip_block_id)
}

/// RFC 117 T2: the outcome of [`resolve_patch_set_digest`]. `NotHeld` is not an error -- the
/// ordinary "you have not synced that far yet" case -- and ambiguity is never a variant here:
/// design T2 rules more-than-one-match a refusal, so a caller cannot accidentally proceed on an
/// ambiguous answer by pattern-matching past it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PatchSetResolution {
    /// No local block reachable from any `heads/*`/`tags/*` ref has this patch set.
    NotHeld,
    /// Exactly one local block has this patch set.
    Resolved(ObjectId),
}

/// RFC 117 T2: resolve a patch-set digest to the local block it names, among every block reachable
/// from any local `heads/*`/`tags/*` ref (`remotes/*` excluded -- unsealed received history the
/// operator has not adopted, consistent with `resolve_ref_to_tip_block`'s own refusal of it).
///
/// **Single pass over the reachable block DAG, not a per-candidate re-walk** (§3): a naive
/// "for each candidate block, re-walk its ancestry" is O(blocks × closure), the same shape that made
/// `verify` roughly O(N³) before RFC 111. Instead this computes every candidate's patch-closure
/// *once*, in topological (parents-before-children) order -- the same Kahn's-algorithm scaffolding
/// `merge_evidence::topological_order` and `order_claims_for_sealing` already use for their own,
/// different orderings, restated here rather than shared (each has its own node/edge shape) --
/// accumulating each block's closure from its parents' already-computed closures plus its own
/// `patch_ids`. A parent's closure is **moved**, not cloned, into its child whenever that child is
/// its last remaining consumer (the ordinary single-parent, single-child case, i.e. most of any real
/// history) -- cloned only at genuine fan-out (a block with more than one child in the candidate
/// set, or a merge
/// block with more than one parent) -- and dropped entirely once every consumer has taken it. Peak
/// memory therefore tracks the DAG's width, not its length.
///
/// **RFC 117 T7: `patch_count` prunes before hashing, and is never trusted alone.** Stage 2 measured
/// this function at O(N²) for one long linear branch: every candidate's closure had to be *hashed in
/// full* to compare against the caller's opaque target, because `PatchSetDigest` alone reveals
/// nothing about the closure it summarizes -- no candidate could be skipped. `patch_count` is not new
/// information (`patch_set_digest_preimage` already hashes `DOMAIN ‖ count ‖ sorted ids`); it exposes
/// a fact the digest already commits to, cheaply enough to compare before hashing rather than after.
/// The comparison is inserted **into** the existing single pass below (`closure.len()` against the
/// caller's `patch_count`, immediately before the existing `compute_patch_set_digest` call) -- no
/// second traversal, no materializing every closure up front, and the move/clone-on-fan-out scheme
/// is untouched. In a linear history closure sizes are 1, 2, 3, … N, all distinct, so exactly one
/// candidate is ever hashed; O(N²) collapses to O(N log N) (RFC 117 stage 2a report has the
/// remeasured numbers). A branchy history prunes less completely but still enormously.
///
/// **The count is a hint that prunes, never an authority (design §9.4): a wrong `patch_count` can
/// only cause the right candidate to be skipped (→ `NotHeld`) or extra candidates to be hashed (→
/// slower) -- it can never produce a wrong resolution, because the digest still has to match.** The
/// same tried-not-trusted shape D6 §11.6 already established for a different object, one field over.
/// A tag whose count disagrees with its own digest is simply self-inconsistent and never resolves.
///
/// **Two or more matching blocks refuse, naming every one** -- never picked, never the ref tip,
/// never the newest. Ambiguity is reachable in production, not only in fixtures: since RFC 115
/// Stage 4, accepted patches are sealed locally, so two branches can seal the same accepted patch
/// set in a different order, giving the same patch-set digest and two distinct block ids (every
/// block has at least one patch -- `seal`/`merge_execute` both refuse an empty one -- so this can
/// only happen via a genuinely different patch order, not an accidentally-shared closure).
pub fn resolve_patch_set_digest(
    layout: &RepositoryLayout,
    digest: PatchSetDigest,
    patch_count: u64,
) -> Result<PatchSetResolution> {
    let object_store = ObjectReadSnapshot::open(layout)?;
    let ref_store = RefStore::new(layout.clone());

    // §2's candidate set: every block reachable from any local heads/*-or-tags/* ref. Reuses this
    // module's own `resolve_ref_to_tip_block` (not a fifth ref-tip resolution copy) and
    // `merge_evidence::ancestors_inclusive` (not a third traversal) -- `list_ref_pointers` itself
    // never names a `remotes/*` entry, since received pointers live in a wholly separate index
    // (`received_index.rs`), so no explicit exclusion is needed here beyond what both already do.
    let mut candidates: BTreeMap<ObjectId, BlockPayload> = BTreeMap::new();
    for pointer in ref_store.list_ref_pointers()? {
        let tip_block_id = resolve_ref_to_tip_block(layout, &object_store, &pointer.ref_name)?;
        candidates.extend(ancestors_inclusive(&object_store, tip_block_id)?);
    }

    // Build the forward (parent -> child) edges and each block's remaining-parent/remaining-child
    // counts. Every parent of a candidate block is itself a candidate: `ancestors_inclusive` walks
    // all the way to genesis for each tip, so the union above is already closed under "parent of".
    let mut remaining_parents: BTreeMap<ObjectId, usize> = BTreeMap::new();
    let mut children: BTreeMap<ObjectId, Vec<ObjectId>> = BTreeMap::new();
    for (&block_id, block) in &candidates {
        remaining_parents.insert(block_id, block.parent_block_ids.len());
        for &parent_id in &block.parent_block_ids {
            children.entry(parent_id).or_default().push(block_id);
        }
    }
    let mut remaining_children: BTreeMap<ObjectId, usize> = candidates
        .keys()
        .map(|&block_id| {
            let count = children.get(&block_id).map_or(0, Vec::len);
            (block_id, count)
        })
        .collect();

    let mut ready: Vec<ObjectId> = remaining_parents
        .iter()
        .filter(|&(_, &count)| count == 0)
        .map(|(&block_id, _)| block_id)
        .collect();
    ready.sort_unstable();
    let mut queue: VecDeque<ObjectId> = ready.into();

    let mut live_closures: BTreeMap<ObjectId, BTreeSet<ObjectId>> = BTreeMap::new();
    let mut matches: Vec<ObjectId> = Vec::new();

    while let Some(block_id) = queue.pop_front() {
        let block = candidates.get(&block_id).ok_or_else(|| {
            PrikkError::Integrity(
                "patch-set digest resolution lost a tracked candidate -- internal inconsistency"
                    .to_string(),
            )
        })?;

        let mut closure: BTreeSet<ObjectId> = BTreeSet::new();
        for &parent_id in &block.parent_block_ids {
            let parent_closure =
                take_parent_closure(parent_id, &mut live_closures, &mut remaining_children)?;
            if closure.is_empty() {
                // First parent (the overwhelmingly common single-parent case): move it in directly
                // rather than merging into an empty set.
                closure = parent_closure;
            } else {
                closure.extend(parent_closure);
            }
        }
        closure.extend(block.patch_ids.iter().copied());

        // RFC 117 T7 §9.2: the size is free -- this pass already built the set -- so try it before
        // paying for a hash. A mismatch here only ever means "not this candidate," never "not a
        // match despite matching," since a size match alone never enters `matches`: the digest
        // comparison below still has to agree too.
        if crate::foundation::fsutil::len_to_u64(closure.len())? == patch_count {
            let sorted: Vec<ObjectId> = closure.iter().copied().collect();
            if compute_patch_set_digest(&sorted)? == digest {
                matches.push(block_id);
            }
        }

        if remaining_children.get(&block_id).copied().unwrap_or(0) > 0 {
            live_closures.insert(block_id, closure);
        }

        for &child_id in children.get(&block_id).into_iter().flatten() {
            let entry = remaining_parents.get_mut(&child_id).ok_or_else(|| {
                PrikkError::Integrity(
                    "patch-set digest resolution lost a tracked child -- internal inconsistency"
                        .to_string(),
                )
            })?;
            *entry -= 1;
            if *entry == 0 {
                queue.push_back(child_id);
            }
        }
    }

    match matches.len() {
        0 => Ok(PatchSetResolution::NotHeld),
        1 => {
            let block_id = matches.pop().ok_or_else(|| {
                PrikkError::Integrity(
                    "patch-set digest resolution: exactly one match reported but none present -- \
                     internal inconsistency"
                        .to_string(),
                )
            })?;
            Ok(PatchSetResolution::Resolved(block_id))
        }
        _ => {
            matches.sort_unstable();
            let names = matches
                .iter()
                .map(ObjectId::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            Err(PrikkError::Integrity(format!(
                "patch-set digest resolves to {} distinct local blocks, refusing to pick: {names}",
                matches.len()
            )))
        }
    }
}

/// Take one parent's already-computed closure for a child now consuming it: **moved** out of
/// `live_closures` if this is the parent's last remaining child (the ordinary case), **cloned** if
/// other children still need it. Either way the parent's own remaining-child count is decremented,
/// and its stored closure is dropped once that count reaches zero.
fn take_parent_closure(
    parent_id: ObjectId,
    live_closures: &mut BTreeMap<ObjectId, BTreeSet<ObjectId>>,
    remaining_children: &mut BTreeMap<ObjectId, usize>,
) -> Result<BTreeSet<ObjectId>> {
    let count = remaining_children.get_mut(&parent_id).ok_or_else(|| {
        PrikkError::Integrity(
            "patch-set digest resolution lost a parent's remaining-child count -- internal \
             inconsistency"
                .to_string(),
        )
    })?;
    *count = count.checked_sub(1).ok_or_else(|| {
        PrikkError::Integrity(
            "patch-set digest resolution consumed a parent's closure more times than it has \
             children -- internal inconsistency"
                .to_string(),
        )
    })?;
    if *count == 0 {
        live_closures.remove(&parent_id).ok_or_else(|| {
            PrikkError::Integrity(format!(
                "patch-set digest resolution: parent {parent_id} has no live closure to take"
            ))
        })
    } else {
        live_closures.get(&parent_id).cloned().ok_or_else(|| {
            PrikkError::Integrity(format!(
                "patch-set digest resolution: parent {parent_id} has no live closure to clone"
            ))
        })
    }
}

#[cfg(test)]
mod tests;
