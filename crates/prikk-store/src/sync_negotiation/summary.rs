//! RFC 116 stage 2 (design-v1.md N1; handoff §1, §2): the `PSYNCSU1` sync summary -- one message,
//! every local `heads/*` ref, each with its own [`PatchSetDigest`] and patch count. Answers "are we
//! the same?" without moving a single patch id (design §1.1): the steady-state case, two
//! repositories already in sync, costs a few hundred bytes rather than 32 bytes per patch.
//!
//! **Branches only.** `remotes/*` never appears: those pointers live in the received-index, not the
//! ordinary ref-pointer index [`RefStore::list_ref_pointers`] enumerates. `tags/*` **is** filtered
//! out explicitly and deliberately, not by oversight -- see the parent module doc.
//!
//! **Representational, not frozen** (RFC 114 §3): carries no identity of its own.
//!
//! **Constructs no `RecognitionClaimPayload`** (handoff §6) -- only ref names, counts, and digests.

use std::collections::{BTreeMap, BTreeSet};

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectType, RefKind, RefStatePayload};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::file_codec::{push_string_u16, push_u64};
use crate::foundation::fsutil::len_to_u64;
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::{ObjectReadSnapshot, ObjectReader};
use crate::patch_set_digest::{
    PatchSetDigest, compute_patch_set_digest, compute_patch_set_digest_for_ref,
    patch_ids_reachable_from_block,
};
use crate::refs::RefStore;

const SYNC_SUMMARY_MAGIC: &[u8; 8] = b"PSYNCSU1";

/// DC-86 bound on the summary's declared ref count, checked before the section is allocated.
pub const DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT: usize = 100_000;

/// DC-86 bound on the summary's total encoded byte length, checked before decoding starts.
pub const DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES: usize = 16 * 1024 * 1024;

/// One ref's own entry in a decoded sync summary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncSummaryRefEntry {
    /// The branch ref's name, e.g. `heads/main`.
    pub ref_name: String,
    /// That ref's own patch-set digest, over its full reachable closure.
    pub digest: PatchSetDigest,
    /// That ref's own reachable patch count -- informational only; the digest already commits to
    /// the exact set, not merely its size, so this is never re-verified against it.
    pub patch_count: u64,
}

/// Build a `PSYNCSU1` sync summary covering every local `heads/*` ref, in
/// [`RefStore::list_ref_pointers`]'s own sorted-by-name order. `remotes/*` and `tags/*` are
/// excluded -- see the module doc. A repository with no `heads/*` ref at all still encodes validly,
/// as a summary declaring zero refs.
pub fn build_sync_summary(layout: &RepositoryLayout) -> Result<Vec<u8>> {
    let ref_store = RefStore::new(layout.clone());
    let object_store = ObjectReadSnapshot::open(layout)?;
    let mut entries: Vec<(String, PatchSetDigest, u64)> = Vec::new();
    for pointer in ref_store.list_ref_pointers()? {
        if !pointer.ref_name.starts_with("heads/") {
            continue;
        }
        let envelope = object_store
            .read_typed(pointer.ref_state_id, ObjectType::RefState)?
            .ok_or_else(|| {
                PrikkError::Integrity(format!(
                    "ref {} names missing RefState {}",
                    pointer.ref_name, pointer.ref_state_id
                ))
            })?;
        let payload = RefStatePayload::decode_canonical(
            &envelope.canonical_payload,
            envelope.schema_version,
        )?;
        if payload.kind != RefKind::Branch {
            return Err(PrikkError::Integrity(format!(
                "ref {} is under heads/ but its RefState kind is not Branch",
                pointer.ref_name
            )));
        }
        let patch_ids = patch_ids_reachable_from_block(&object_store, payload.target_object_id)?;
        let digest = compute_patch_set_digest(&patch_ids)?;
        let patch_count = len_to_u64(patch_ids.len())?;
        entries.push((pointer.ref_name, digest, patch_count));
    }

    let mut out = Vec::new();
    out.extend_from_slice(SYNC_SUMMARY_MAGIC);
    push_u64(&mut out, len_to_u64(entries.len())?);
    for (ref_name, digest, patch_count) in &entries {
        push_string_u16(&mut out, ref_name)?;
        out.extend_from_slice(&digest.0);
        push_u64(&mut out, *patch_count);
    }
    Ok(out)
}

/// Decode a `PSYNCSU1` sync summary structurally. Bounds the total byte length before touching the
/// bytes at all, then the declared ref count before allocating, the same DC-86 shape
/// `decode_exchange_artifact` follows. Performs no cross-entry checks and no comparison against
/// this repository's own refs -- see [`compare_sync_summary`] for that.
pub fn decode_sync_summary(
    bytes: &[u8],
    max_total_bytes: usize,
    max_ref_count: usize,
) -> Result<Vec<SyncSummaryRefEntry>> {
    if bytes.len() > max_total_bytes {
        return Err(PrikkError::MalformedData(format!(
            "sync summary is {} bytes, over the configured limit of {max_total_bytes} bytes",
            bytes.len()
        )));
    }
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_array::<8>()?;
    if &magic != SYNC_SUMMARY_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid sync summary magic".to_string(),
        ));
    }
    let ref_count = cursor.read_u64()?;
    if ref_count > len_to_u64(max_ref_count)? {
        return Err(PrikkError::MalformedData(format!(
            "sync summary declares {ref_count} refs, over the configured limit of {max_ref_count}"
        )));
    }
    let mut entries = Vec::new();
    for _ in 0..ref_count {
        let ref_name = cursor.read_string_u16()?;
        let digest = PatchSetDigest(cursor.read_array::<32>()?);
        let patch_count = cursor.read_u64()?;
        entries.push(SyncSummaryRefEntry {
            ref_name,
            digest,
            patch_count,
        });
    }
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in sync summary".to_string(),
        ));
    }
    Ok(entries)
}

/// One ref's own comparison state, from [`compare_sync_summary`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncRefComparisonState {
    /// Both sides have this ref, and their digests agree.
    InSync,
    /// Both sides have this ref, but their digests disagree.
    Differs,
    /// Only the remote summary names this ref -- this repository does not hold it.
    RemoteOnly,
    /// Only this repository holds this ref -- the remote summary does not name it.
    LocalOnly,
}

impl SyncRefComparisonState {
    /// Stable, lowercase, hyphenated name -- used for CLI output and test assertions alike, so
    /// there is exactly one spelling to keep in sync.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::InSync => "in-sync",
            Self::Differs => "differs",
            Self::RemoteOnly => "remote-only",
            Self::LocalOnly => "local-only",
        }
    }
}

/// One ref's own comparison result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncRefComparison {
    /// The branch ref's name.
    pub ref_name: String,
    /// How this repository's own state compares to `remote`'s.
    pub state: SyncRefComparisonState,
}

/// Compare this repository's own `heads/*` refs against a remote summary's entries (handoff §2).
/// **None of the four states is a refusal** -- an asymmetric ref set (one side names a ref the
/// other does not hold) is ordinary, ruled by design §5 item 6 / N5 item 6 and carried forward
/// through every stage since: a receiver-absent have-list is empty, not a refusal (stage 2/3); a
/// sender-absent ref reports `AlreadyInSync`, not a refusal (stage 3). This function is the same
/// principle applied to the summary's own comparison, which stage 2's review flagged as pinned by
/// a passing test and by no control.
///
/// Local digests are computed directly per ref via [`compute_patch_set_digest_for_ref`] -- not by
/// building and decoding this repository's own summary and diffing the two decoded lists, which
/// would compute a digest for every local ref whether or not it is even being compared. Branches
/// only, matching this module's own scope.
pub fn compare_sync_summary(
    layout: &RepositoryLayout,
    remote: &[SyncSummaryRefEntry],
) -> Result<Vec<SyncRefComparison>> {
    let ref_store = RefStore::new(layout.clone());
    let mut local_ref_names: BTreeSet<String> = BTreeSet::new();
    for pointer in ref_store.list_ref_pointers()? {
        if pointer.ref_name.starts_with("heads/") {
            local_ref_names.insert(pointer.ref_name);
        }
    }
    let remote_digests: BTreeMap<&str, PatchSetDigest> = remote
        .iter()
        .map(|entry| (entry.ref_name.as_str(), entry.digest))
        .collect();

    let mut all_ref_names: BTreeSet<&str> = local_ref_names.iter().map(String::as_str).collect();
    all_ref_names.extend(remote_digests.keys().copied());

    let mut comparisons = Vec::with_capacity(all_ref_names.len());
    for ref_name in all_ref_names {
        let is_local = local_ref_names.contains(ref_name);
        let state = match (is_local, remote_digests.get(ref_name)) {
            (true, Some(remote_digest)) => {
                let local_digest = compute_patch_set_digest_for_ref(layout, ref_name)?;
                if &local_digest == remote_digest {
                    SyncRefComparisonState::InSync
                } else {
                    SyncRefComparisonState::Differs
                }
            }
            (true, None) => SyncRefComparisonState::LocalOnly,
            (false, Some(_)) => SyncRefComparisonState::RemoteOnly,
            (false, None) => unreachable!(
                "ref_name is drawn from local_ref_names or remote_digests, so at least one holds it"
            ),
        };
        comparisons.push(SyncRefComparison {
            ref_name: ref_name.to_string(),
            state,
        });
    }
    Ok(comparisons)
}

#[cfg(test)]
mod tests;
