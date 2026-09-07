//! Received (imported) ref bookkeeping — DC-78 §D4's distinct `remotes/` namespace.
//!
//! A received ref is deliberately **not** a `refs/by-id/` pointer. `refs/by-id/`'s own consistency
//! check (a RefState object's embedded `ref_name` must equal its pointer's declared name — enforced
//! by `refs/verify/scan.rs`) cannot be satisfied by a renamed local identifier: the RefState object
//! keeps the *origin's own* embedded ref name (its content-addressed identity and signature would be
//! invalidated by editing it), so a pointer declaring `remotes/heads/main` could never agree with a
//! payload that says `heads/main`. Storing received refs under their own key, in their own small
//! index format, sidesteps that conflict entirely rather than patching the check to allow it — the
//! latter would be new verification-path surface, which §D6 rules out.
//!
//! This index is never read by `verify_repository`. Every object a received pointer leads to
//! (RefState, Block, Patch, Blob, Attestation) is an ordinary object-store entry, checked exactly
//! like any other by the existing type-based object scan — §D6's "no new verification path" holds
//! because there genuinely is none: this module only makes received tips *discoverable* by name.
//!
//! RFC 102 Stage 5, design-v1.md §14/Step 0 item 2: backed by `received_index.rs`'s shared, append-
//! only, last-entry-wins container (the refs container+pointer-index pattern, applied here because a
//! received ref's own name doesn't exist at `init` either — the same architecturally-forced shape
//! Stage 4 hit with refs). Replaced the old one-file-per-ref directory entirely, not layered on top of
//! it.
//!
//! RFC 131 §2.2a ruling (b): the `received` name family. This module's own bare name is the
//! family's, so it is the group's root; `received_index` is its child, below.

pub(crate) mod received_index;

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::foundation::layout::{RepositoryLayout, ref_name_key_bytes};
use crate::received::received_index::{
    ReceivedIndexEntry, append_received_index_entry, list_resolved_received_entries,
    lookup_received_index_entry,
};

/// One received ref pointer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceivedPointer {
    /// Local logical name, always `remotes/<origin ref name>`.
    pub ref_name: String,
    /// The received tip's RefState object id.
    pub ref_state_id: ObjectId,
}

/// Validate a received ref name: the reserved `remotes/` namespace, required rather than rejected
/// (the mirror image of `validate_local_branch_ref`/`validate_local_tag_ref`, which reject it).
pub fn validate_received_ref(ref_name: &str) -> Result<()> {
    if ref_name.is_empty() {
        return Err(PrikkError::InvalidName(
            "ref name must not be empty".to_string(),
        ));
    }
    if !ref_name.starts_with("remotes/") {
        return Err(PrikkError::InvalidName(format!(
            "ref {ref_name} is not a received ref; expected remotes/<name>"
        )));
    }
    let rest = &ref_name["remotes/".len()..];
    if rest.is_empty() {
        return Err(PrikkError::InvalidName(
            "received ref must include a name after remotes/".to_string(),
        ));
    }
    if ref_name.chars().any(|ch| ch == '\0' || ch.is_control()) {
        return Err(PrikkError::InvalidName(format!(
            "ref {ref_name} contains a forbidden control character"
        )));
    }
    if rest.starts_with('/') || rest.ends_with('/') || rest.contains("//") {
        return Err(PrikkError::InvalidName(format!(
            "received ref {ref_name} contains an empty path component"
        )));
    }
    if rest
        .split('/')
        .any(|component| component == "." || component == "..")
    {
        return Err(PrikkError::InvalidName(format!(
            "received ref {ref_name} contains a traversal component"
        )));
    }
    Ok(())
}

/// Write (or overwrite) a received ref's pointer. Each import replaces the prior received state for
/// that name outright — there is no CAS and no merge between two received histories under one name;
/// D4 explicitly leaves remote-tracking-ref semantics out of scope, so a re-import is simply "this is
/// what I have now," and turning that into real local history remains the operator's own deliberate
/// merge, using machinery that already exists. Never checks for an existing entry first — matching
/// `append_received_index_entry`'s own reasoning, "last entry wins" already makes a duplicate
/// harmless and there is no CAS to enforce here.
pub(crate) fn write_received_pointer(
    layout: &RepositoryLayout,
    ref_name: &str,
    ref_state_id: ObjectId,
) -> Result<()> {
    validate_received_ref(ref_name)?;
    append_received_index_entry(
        layout,
        &ReceivedIndexEntry {
            ref_name_key: ref_name_key_bytes(ref_name),
            ref_name: ref_name.to_string(),
            ref_state_id,
        },
    )
}

/// Read a received ref's current pointer, if one has been imported.
pub fn read_received_pointer(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<Option<ReceivedPointer>> {
    validate_received_ref(ref_name)?;
    let key = ref_name_key_bytes(ref_name);
    Ok(lookup_received_index_entry(layout, key)?.map(ReceivedPointer::from))
}

/// Enumerate every received ref pointer, sorted by name — the received-namespace counterpart of
/// `RefStore::list_ref_pointers`.
pub fn list_received_pointers(layout: &RepositoryLayout) -> Result<Vec<ReceivedPointer>> {
    let mut pointers: Vec<ReceivedPointer> = list_resolved_received_entries(layout)?
        .into_iter()
        .map(ReceivedPointer::from)
        .collect();
    pointers.sort_by(|left, right| left.ref_name.cmp(&right.ref_name));
    Ok(pointers)
}

impl From<ReceivedIndexEntry> for ReceivedPointer {
    fn from(entry: ReceivedIndexEntry) -> Self {
        Self {
            ref_name: entry.ref_name,
            ref_state_id: entry.ref_state_id,
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
