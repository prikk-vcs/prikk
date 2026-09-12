//! Active-session commit helpers.
//!
//! This module is the narrow boundary between higher-level commit construction and the
//! durable active WAL. It owns lock acquisition for the default active session and appends only
//! already-constructed, signed patch envelopes. It also owns the local ref-name metadata that makes a
//! non-empty active WAL unambiguously belong to one target ref.
//!
//! RFC 102 Stage 5, design-v1.md §14.5/§14.6: the ref-name metadata file is pre-allocated at `init`
//! (`layout.rs`) and never removed again -- "cleared" now means truncated to empty, "set" means
//! truncated-then-appended, both `atomic_replace`-free. `write_active_ref_metadata` truncates
//! internally rather than trusting caller discipline: it is `pub` API, and a bare append would let a
//! second call silently concatenate two ref names into one file rather than replacing it.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectEnvelope;

use crate::foundation::fsutil::{
    append_file_required, read_file_if_exists, truncate_file_empty_required,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::refs::{ensure_no_incomplete_publication, validate_local_branch_ref};
use crate::wal::Wal;

/// Result of appending a patch envelope to the active session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveCommitResult {
    /// WAL sequence assigned to the appended patch envelope.
    pub wal_sequence: u64,
}

/// Active-WAL ref metadata read result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveRefMetadata {
    /// Metadata file is absent.
    Missing,
    /// Metadata file contains a valid canonical local branch ref.
    Valid(String),
    /// Metadata file exists but is malformed or not a valid local branch ref.
    Invalid(String),
}

/// Default active-session handle.
#[derive(Debug, Clone)]
pub struct ActiveSession {
    layout: RepositoryLayout,
}

impl ActiveSession {
    /// Create an active-session handle for a repository layout.
    #[must_use]
    pub fn new(layout: RepositoryLayout) -> Self {
        Self { layout }
    }

    /// Append one signed patch envelope while holding the active-session lock. `active_patch_limit`
    /// is DC-57's hard block (NFR-PERF-02): once the active WAL already holds this many patches, no
    /// more may be appended — see `node_authoring.rs::author_inner`'s identical check, the one other
    /// authoring path this definition must also hold for.
    pub fn append_patch(
        &self,
        envelope: &ObjectEnvelope,
        active_patch_limit: usize,
    ) -> Result<ActiveCommitResult> {
        self.layout.require_current_format()?;
        let _lock = ActiveLock::acquire(&self.layout, DEFAULT_ACTIVE_NAME)?;
        ensure_no_incomplete_publication(&self.layout)?;
        let wal = Wal::for_layout(&self.layout, DEFAULT_ACTIVE_NAME);
        let replay = wal.replay()?;
        if replay.trailing_partial_bytes != 0 {
            return Err(PrikkError::Integrity(format!(
                "active WAL has {} trailing partial bytes; run doctor before appending",
                replay.trailing_partial_bytes
            )));
        }
        // RFC 102 Stage 2: `replay.records` below (the active-patch-limit count, and the
        // empty-vs-non-empty branch) silently omits a damaged record rather than erroring now.
        if replay.has_item_failure() {
            return Err(PrikkError::Integrity(
                "active WAL has a damaged record; run doctor before appending".to_string(),
            ));
        }
        if crate::commit_boundary::worktree_patch::active_patch_limit_exceeded(
            replay.records.len(),
            active_patch_limit,
        ) {
            // RFC 132 part 2: a full active-patch queue is a caller precondition, not a lock --
            // nothing is held and no other writer is racing this one; waiting does not help, only
            // sealing does.
            return Err(PrikkError::Precondition(format!(
                "active WAL has {} queued patches, at or above the configured limit \
                 ({active_patch_limit}); run doctor or seal before appending again",
                replay.records.len()
            )));
        }
        if replay.records.is_empty() {
            prepare_empty_active_ref_for_append(&self.layout, "heads/main")?;
        } else {
            // DC-66: a non-empty active WAL now queues rather than refusing outright; ownership must
            // still be unambiguous — see `node_authoring.rs::author_inner`'s identical guard change.
            require_active_ref_for_non_empty_wal(&self.layout, "heads/main")?;
        }
        let wal_sequence = wal.append_patch(envelope)?;
        Ok(ActiveCommitResult { wal_sequence })
    }
}

/// Read active-WAL ref metadata for `name`, without mutating it.
///
/// RFC 108 increment 3c: generalized from the `default`-only `read_active_ref_metadata`, the same
/// wrapper shape increment 1 used for `default_active_dir`/`default_queue_wal_path` -- a new
/// parameterized function, with the existing zero-argument one reimplemented in terms of it, so
/// every one of `read_active_ref_metadata`'s own callers needs no edit. `pub(crate)` for now: the
/// only caller that needs a non-default name is `doctor.rs`'s own per-active-session reporting,
/// inside this crate; nothing outside it constructs a second active session yet either.
pub(crate) fn read_active_ref_metadata_for(
    layout: &RepositoryLayout,
    name: impl AsRef<std::path::Path>,
) -> Result<ActiveRefMetadata> {
    let relative = layout.repository_relative(&layout.active_ref_name_path(name))?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(ActiveRefMetadata::Missing);
    };
    // RFC 102 Stage 5, design-v1.md §14.6: the file is pre-allocated at `init` and never removed, so
    // "no active session" is now represented by empty content as well as (pre-migration) absence --
    // both read as `Missing`. Empty content can only be the cleared state; a real ref name is never
    // zero bytes (`validate_local_branch_ref` rejects an empty string).
    if bytes.is_empty() {
        return Ok(ActiveRefMetadata::Missing);
    }
    let text = match std::str::from_utf8(&bytes) {
        Ok(text) => text,
        Err(err) => {
            return Ok(ActiveRefMetadata::Invalid(format!(
                "active ref metadata is not UTF-8: {err}"
            )));
        }
    };
    match validate_local_branch_ref(text) {
        Ok(canonical) => Ok(ActiveRefMetadata::Valid(canonical)),
        Err(err) => Ok(ActiveRefMetadata::Invalid(err.to_string())),
    }
}

/// Read active-WAL ref metadata without mutating it.
pub fn read_active_ref_metadata(layout: &RepositoryLayout) -> Result<ActiveRefMetadata> {
    read_active_ref_metadata_for(layout, DEFAULT_ACTIVE_NAME)
}

/// Write active-WAL ref metadata, replacing whatever was there before. `pub` API, so the
/// replace-semantics contract is enforced structurally rather than by caller discipline (design-v1.md
/// §14.6's condition): truncates to empty, then appends the canonical ref name, so a second call can
/// never concatenate two names into one file the way a bare append would.
///
/// **`default`-only, deliberately, unlike the read side (RFC 108 increment 3c).** Generalizing the
/// read path had a real, named caller waiting (`doctor.rs`'s per-active reporting); generalizing this
/// write path does not -- nothing appends to or seals a non-default active's WAL yet, so a
/// parameterized writer here would be dead surface the day it landed (this project's own
/// dead-surface-consolidation history is why that is worth avoiding, not merely untidy). **What would
/// justify generalizing it**: a caller that actually appends to or clears a non-default active's WAL
/// -- RFC 108 increment 3d (per-active repair) is the first candidate, and it is the one to
/// generalize this alongside, not before.
pub fn write_active_ref_metadata(layout: &RepositoryLayout, ref_name: &str) -> Result<String> {
    layout.require_current_format()?;
    let canonical = validate_local_branch_ref(ref_name)?;
    let relative = layout.repository_relative(&layout.default_active_ref_name_path())?;
    truncate_file_empty_required(layout.repository_mutation_root(), &relative)?;
    append_file_required(
        layout.repository_mutation_root(),
        &relative,
        canonical.as_bytes(),
    )?;
    Ok(canonical)
}

/// Clear active-WAL ref metadata and fsync the active-session directory. Returns whether there was
/// non-empty content to clear (the pre-migration "did a file exist to remove" contract, now answered
/// by content rather than presence -- the file itself is permanent from `init` onward).
///
/// **`default`-only, for the same reason `write_active_ref_metadata` above is** -- no caller clears a
/// non-default active's metadata yet.
pub fn remove_active_ref_metadata(layout: &RepositoryLayout) -> Result<bool> {
    layout.require_current_format()?;
    remove_active_ref_metadata_authorized(layout)
}

fn remove_active_ref_metadata_authorized(layout: &RepositoryLayout) -> Result<bool> {
    let relative = layout.repository_relative(&layout.default_active_ref_name_path())?;
    let had_content = !read_file_if_exists(layout.repository_mutation_root(), &relative)?
        .unwrap_or_default()
        .is_empty();
    truncate_file_empty_required(layout.repository_mutation_root(), &relative)?;
    Ok(had_content)
}

/// Drain a fully published active WAL and remove its ownership metadata under the active lock.
pub fn finish_active_publication_cleanup(
    layout: &RepositoryLayout,
    active_lock: &ActiveLock,
) -> Result<()> {
    layout.require_current_format()?;
    active_lock.require_layout(layout)?;
    Wal::for_layout(layout, DEFAULT_ACTIVE_NAME).truncate_empty()?;
    remove_active_ref_metadata_authorized(layout)?;
    Ok(())
}

/// Prepare active ref metadata for the first WAL append.
///
/// Caller must hold the active-session lock and must call this only after replay has proven that the
/// active WAL has no records and no trailing partial bytes.
///
/// RFC 102 Stage 5, design-v1.md §14.6: no longer branches on the metadata's prior state --
/// `write_active_ref_metadata` truncates before it appends, so any stale `Valid`/`Invalid` debris left
/// over from a fully-drained-but-uncleared session is replaced unconditionally, the same as the
/// `Missing` case. The pre-clear-then-write two-step this function used to perform is now internal to
/// `write_active_ref_metadata` itself; the crash window between clear and write moved, it did not grow.
pub(crate) fn prepare_empty_active_ref_for_append(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<String> {
    write_active_ref_metadata(layout, ref_name)
}

/// Who owns the active WAL for a non-empty session, the read-only question
/// [`require_active_ref_for_non_empty_wal`] asserts an answer to (RFC 132 part 1). **A value, not
/// a `Result`**: "owned by a different ref" is a legitimate answer to an ownership question, not a
/// failure. Returning it as `Err(PrikkError::Precondition(_))` forced `prikk branch close` to
/// un-file the answer by variant (`Err(PrikkError::Precondition(_)) => {}`) -- a match broad
/// enough that any *other* `Precondition` reaching that same call site would have been silently
/// treated the same way, the exact defect this part exists to close before part 2 adds six more
/// `Precondition` sites elsewhere in the crate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActiveRefOwnership {
    /// The active WAL is owned by the ref that was asked about.
    Owned,
    /// The active WAL is owned by a different ref, named here.
    OwnedByOther(String),
}

/// Answer which ref owns the active WAL for a non-empty session. Missing or malformed active-ref
/// metadata is still a hard error here, not an ownership answer -- an active WAL that has records
/// but cannot say whose they are is an integrity condition regardless of which ref asked.
pub fn active_ref_ownership(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<ActiveRefOwnership> {
    let expected = validate_local_branch_ref(ref_name)?;
    match read_active_ref_metadata(layout)? {
        ActiveRefMetadata::Valid(actual) if actual == expected => Ok(ActiveRefOwnership::Owned),
        ActiveRefMetadata::Valid(actual) => Ok(ActiveRefOwnership::OwnedByOther(actual)),
        ActiveRefMetadata::Missing => Err(PrikkError::Integrity(
            "active WAL has records but active ref metadata is missing".to_string(),
        )),
        ActiveRefMetadata::Invalid(reason) => Err(PrikkError::Integrity(format!(
            "active WAL has records but active ref metadata is malformed: {reason}"
        ))),
    }
}

/// Validate active ref metadata for a non-empty active WAL -- the assertion form of
/// [`active_ref_ownership`], written in terms of it rather than re-deriving the same comparison,
/// for the two callers (`ActiveSession::append_patch`, `node_authoring::author_inner`) that want a
/// plain `?` refusal and use the returned ref name for nothing but confirming it matched.
pub fn require_active_ref_for_non_empty_wal(
    layout: &RepositoryLayout,
    ref_name: &str,
) -> Result<String> {
    match active_ref_ownership(layout, ref_name)? {
        ActiveRefOwnership::Owned => Ok(ref_name.to_string()),
        // RFC 132's Precondition variant: this is an ownership mismatch, not a lock -- nothing is
        // held and no other process is racing this one, so `LockConflict`'s "another writer may be
        // active" was never true here.
        ActiveRefOwnership::OwnedByOther(actual) => Err(PrikkError::Precondition(format!(
            "active WAL is owned by {actual}; requested ref {ref_name}"
        ))),
    }
}

// DC-71: every test here sets up its scenario via real repository mutation (RepositoryLayout::init
// or equivalent), which is Linux-only; the module never compiles a non-Linux-meaningful test.
#[cfg(all(test, target_os = "linux"))]
mod tests;
