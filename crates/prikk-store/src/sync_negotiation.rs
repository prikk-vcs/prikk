//! RFC 116 stage 2 (design-v1.md N1, N2, N4; `stage-2-negotiation-artifacts-handoff-v1.md`) — the
//! negotiation artifacts and the delta computation. **Nothing here touches the network.**
//! `prikk-store` stays bytes-in, bytes-out (RFC 116 ruling 2); every function in this module and
//! its submodules is a pure read or a pure computation over already-decoded bytes.
//!
//! **Two artifacts, one computation, one sender:**
//! - [`build_sync_summary`]/[`decode_sync_summary`] (`summary.rs`): `PSYNCSU1`, one message
//!   covering every local `heads/*` ref -- name, [`patch_set_digest::PatchSetDigest`], patch count.
//!   32 bytes plus a name per ref (design §1.1): cheap enough that two repositories already in sync
//!   can find out for a few hundred bytes, never a patch-id list.
//! - [`build_have_list`]/[`decode_have_list`] (`have_list.rs`): `PSYNCHV1`, one ref's own full
//!   patch-id list plus a digest the receiver of the message recomputes and checks (§1.3) --
//!   self-consistent by construction, never trusted from the wire.
//! - [`compute_sync_delta`] (N4): `reachable(sender's own ref) ∖ have_list.patch_ids`, sorted.
//! - [`build_sync_artifact`] (stage 3, `sender.rs`): given a ref and a have-list, produces the
//!   `PEXCH001` that closes the gap -- see `sender.rs`'s own module doc for why this is the one
//!   and only place in the crate that ever constructs a `RecognitionClaimPayload`.
//!
//! **Representational, not frozen** (RFC 114 §3, restated per artifact): both formats carry objects
//! whose identity is already frozen (`ObjectId`, [`patch_set_digest::PatchSetDigest`]) and carry no
//! identity of their own.
//!
//! **Unsigned by design (N2).** A lying negotiation message can only cause a wrong delta -- more
//! patches sent than needed (wasteful, harmless: the receiver deduplicates by content address) or
//! fewer (the receiver's next summary comparison says so). Every byte that finally arrives is still
//! verified by Stage 3's accept path against material it carries, and Stage 4 seals only under the
//! receiver's own key. Signing these would imply the negotiation itself is trust-bearing; it is
//! not, and implying otherwise is worse than leaving it plain. **Negotiation does disclose
//! metadata** -- a have-list reveals which patch ids a repository holds, never their content; this
//! is the one property to document, not defend against (N2).
//!
//! **Branches only (§2 ruling).** `remotes/*` never appears: those pointers live in
//! [`crate::received`]'s own index, not the ordinary ref-pointer index [`crate::refs::RefStore`]
//! enumerates, so there is nothing to filter there. `tags/*` **is** enumerable through the ordinary
//! index and is excluded deliberately, not by oversight: [`crate::seal_from_accepted_claim`]
//! requires [`crate::refs::validate_local_branch_ref`] -- a tag cannot be sealed onto -- so
//! including tags here would report differences nothing downstream can act on. Tag sync is its own
//! question, not answered here.
//!
//! **This top-level module, `summary.rs`, and `have_list.rs` construct no
//! [`prikk_object::RecognitionClaimPayload`].** `sender.rs` is the sole exception, deliberately:
//! it is stage 3's sender side, the first claim producer in the project's history, and closes the
//! free-schema-amendment window D6 and N3 both used (RFC 114).

use std::collections::BTreeSet;

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RefKind, RefStatePayload};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_set_digest::patch_ids_reachable_from_block;
use crate::refs::{RefStore, validate_local_branch_ref};

mod have_list;
mod sender;
mod summary;

pub use have_list::{
    DEFAULT_HAVE_LIST_MAX_PATCH_COUNT, DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES, HaveList,
    build_have_list, decode_have_list,
};
pub use sender::{SyncArtifactBuildReport, SyncArtifactOutcome, build_sync_artifact};
pub use summary::{
    DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT, DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES, SyncRefComparison,
    SyncRefComparisonState, SyncSummaryRefEntry, build_sync_summary, compare_sync_summary,
    decode_sync_summary,
};

/// N4 (design-v1.md §4): the delta for the one ref `have_list` names -- every patch id reachable
/// from this repository's own tip for that ref, minus what `have_list` says the other side already
/// has. **Returned sorted** (§3's own instruction), following naturally from
/// [`patch_ids_reachable_from_block`]'s own `BTreeSet`-derived order. Does not build a `PEXCH001`
/// artifact -- see [`build_sync_artifact`] for that -- and constructs no `RecognitionClaimPayload`,
/// only a `Vec<ObjectId>`.
///
/// **A ref this repository does not hold produces the full reachable set as the delta, not a
/// refusal** (design §5 item 6 / N5 item 6): an absent local ref behaves as an empty local reach
/// set, so the delta is simply everything the other side is missing -- the correct outcome when the
/// other side has never seen this ref at all, not a special case.
pub fn compute_sync_delta(
    layout: &RepositoryLayout,
    have_list: &HaveList,
) -> Result<Vec<ObjectId>> {
    let canonical_ref = validate_local_branch_ref(&have_list.ref_name)?;
    let object_store = ObjectReadSnapshot::open(layout)?;
    let ref_store = RefStore::new(layout.clone());
    let sender_patch_ids = match resolve_branch_ref_tip(&object_store, &ref_store, &canonical_ref)?
    {
        Some(tip_block_id) => patch_ids_reachable_from_block(&object_store, tip_block_id)?,
        None => Vec::new(),
    };
    let receiver_patch_ids: BTreeSet<ObjectId> = have_list.patch_ids.iter().copied().collect();
    Ok(sender_patch_ids
        .into_iter()
        .filter(|patch_id| !receiver_patch_ids.contains(patch_id))
        .collect())
}

/// Resolve `ref_name` (already validated as a local branch ref shape) to its target Block id, or
/// `None` if this repository does not hold the ref at all. No tag hop -- branches only, per this
/// module's own scope (§2).
fn resolve_branch_ref_tip(
    object_store: &impl ObjectReader,
    ref_store: &RefStore,
    ref_name: &str,
) -> Result<Option<ObjectId>> {
    let Some(ref_state_id) = ref_store.read_current_ref_state_id(ref_name)? else {
        return Ok(None);
    };
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)?
        .ok_or_else(|| {
            PrikkError::Integrity(format!(
                "ref {ref_name} names missing RefState {ref_state_id}"
            ))
        })?;
    let payload =
        RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)?;
    if payload.kind != RefKind::Branch {
        return Err(PrikkError::Integrity(format!(
            "ref {ref_name} is under heads/ but its RefState kind is not Branch"
        )));
    }
    Ok(Some(payload.target_object_id))
}

#[cfg(test)]
mod sync_test_support;
#[cfg(test)]
mod tests;
