//! RFC 116 stage 2 (design-v1.md N1, §1.3; handoff §1, §4): the `PSYNCHV1` have-list -- one ref,
//! its declared [`PatchSetDigest`], and the full patch-id list the digest is over. Sent receiver ->
//! sender (design §1.2 step 2), so the sender can compute the delta ([`super::compute_sync_delta`]).
//!
//! **Representational, not frozen** (RFC 114 §3): carries no identity of its own.
//!
//! **Self-consistency (§1.3): the digest is recomputed over the decoded list and checked, never
//! trusted from the wire.** [`compute_patch_set_digest`] already refuses a list that is not
//! sorted-and-deduplicated, so a truncated or reordered list either fails there or fails the digest
//! comparison below it -- no separate sortedness check is needed here.
//!
//! **Constructs no `RecognitionClaimPayload`** (handoff §6) -- only patch ids.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RefKind, RefStatePayload};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{push_string_u16, push_u64};
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_set_digest::{
    PatchSetDigest, compute_patch_set_digest, patch_ids_reachable_from_block,
};
use crate::refs::{RefStore, validate_local_branch_ref};

const HAVE_LIST_MAGIC: &[u8; 8] = b"PSYNCHV1";

/// DC-86 bound on the have-list's declared patch count, checked before the list is allocated.
pub const DEFAULT_HAVE_LIST_MAX_PATCH_COUNT: usize = 100_000;

/// DC-86 bound on the have-list's total encoded byte length, checked before decoding starts.
pub const DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES: usize = 64 * 1024 * 1024;

/// A decoded, self-consistency-checked `PSYNCHV1` have-list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HaveList {
    /// The one ref this have-list is about.
    pub ref_name: String,
    /// The declared digest -- verified to match `patch_ids` at decode time.
    pub digest: PatchSetDigest,
    /// Every patch id the sender of this message already holds for `ref_name`, sorted and
    /// deduplicated (the shape [`compute_patch_set_digest`]'s input requires).
    pub patch_ids: Vec<ObjectId>,
}

/// Build a `PSYNCHV1` have-list for `ref_name`, from this repository's own current state.
///
/// **A ref this repository does not hold locally encodes an empty list, not a refusal** (design §5
/// item 6 / N5 item 6): the receiver of a summary entry it has no local counterpart for still needs
/// to say "I have nothing under this name" so the sender can compute a delta that is everything.
pub fn build_have_list(layout: &RepositoryLayout, ref_name: &str) -> Result<Vec<u8>> {
    let canonical_ref = validate_local_branch_ref(ref_name)?;
    let ref_store = RefStore::new(layout.clone());
    let object_store = ObjectReadSnapshot::open(layout)?;
    let patch_ids = match ref_store.read_current_ref_state_id(&canonical_ref)? {
        Some(ref_state_id) => {
            let envelope = object_store
                .read_typed(ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    PrikkError::Integrity(format!(
                        "ref {canonical_ref} names missing RefState {ref_state_id}"
                    ))
                })?;
            let payload = RefStatePayload::decode_canonical(
                &envelope.canonical_payload,
                envelope.schema_version,
            )?;
            if payload.kind != RefKind::Branch {
                return Err(PrikkError::Integrity(format!(
                    "ref {canonical_ref} is under heads/ but its RefState kind is not Branch"
                )));
            }
            patch_ids_reachable_from_block(&object_store, payload.target_object_id)?
        }
        None => Vec::new(),
    };
    let digest = compute_patch_set_digest(&patch_ids)?;

    let mut out = Vec::new();
    out.extend_from_slice(HAVE_LIST_MAGIC);
    push_string_u16(&mut out, &canonical_ref)?;
    out.extend_from_slice(&digest.0);
    push_u64(&mut out, len_to_u64(patch_ids.len())?);
    for patch_id in &patch_ids {
        out.extend_from_slice(patch_id.as_bytes());
    }
    Ok(out)
}

/// Decode and self-consistency-check a `PSYNCHV1` have-list (§1.3). Bounds the total byte length
/// before touching the bytes at all, then the declared patch count before allocating the list --
/// the same DC-86 shape `decode_exchange_artifact` follows.
pub fn decode_have_list(
    bytes: &[u8],
    max_total_bytes: usize,
    max_patch_count: usize,
) -> Result<HaveList> {
    if bytes.len() > max_total_bytes {
        return Err(PrikkError::MalformedData(format!(
            "have-list is {} bytes, over the configured limit of {max_total_bytes} bytes",
            bytes.len()
        )));
    }
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_array::<8>()?;
    if &magic != HAVE_LIST_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid have-list magic".to_string(),
        ));
    }
    let ref_name = cursor.read_string_u16()?;
    let declared_digest = PatchSetDigest(cursor.read_array::<32>()?);
    let patch_count = cursor.read_u64()?;
    if patch_count > len_to_u64(max_patch_count)? {
        return Err(PrikkError::MalformedData(format!(
            "have-list declares {patch_count} patch ids, over the configured limit of \
             {max_patch_count}"
        )));
    }
    let mut patch_ids = Vec::new();
    for _ in 0..patch_count {
        patch_ids.push(ObjectId::from_bytes(cursor.read_array::<32>()?));
    }
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in have-list".to_string(),
        ));
    }

    // §1.3: recompute the digest over the list actually received and refuse on mismatch --
    // `compute_patch_set_digest` already refuses an unsorted or duplicate-bearing list, so a
    // truncated or reordered list fails there before the comparison below even runs.
    let recomputed_digest = compute_patch_set_digest(&patch_ids)?;
    if recomputed_digest != declared_digest {
        return Err(PrikkError::Integrity(
            "have-list's declared patch-set digest does not match its own carried list".to_string(),
        ));
    }

    Ok(HaveList {
        ref_name,
        digest: declared_digest,
        patch_ids,
    })
}

#[cfg(test)]
mod tests;
