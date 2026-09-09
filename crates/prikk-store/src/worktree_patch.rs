//! Worktree-to-patch authoring.
//!
//! Turns worktree changes into a node-addressed, role-bound Ed25519 AUTHOR-signed Patch envelope
//! against a replay-derived baseline and appends it to the active WAL. The change-detection,
//! operation-mapping, minting, mode-normalization, and canonical-ordering logic lives in
//! [`node_authoring`]; signing goes through the injected [`crate::AuthorSigner`] boundary (no
//! placeholder signer). Existing paths resolve to their persisted `node_id`; fresh nodes are minted
//! in canonical create order; text edits go through the shared `text_span` module. Rename authoring
//! (RFC 144 §4o) is declaration-based, never inferred from content -- see `node_authoring`'s own
//! consumption of `rename_declaration`'s live store. Symlink authoring remains out of scope.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

use crate::author::author_signing::AuthorSigner;
use crate::foundation::layout::RepositoryLayout;
use crate::node::node_id_gen::NodeIdGenerator;

mod node_authoring;

/// Result of authoring and appending a node-addressed patch from worktree changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePatchCommitReport {
    /// Baseline ref used to classify changes.
    pub ref_name: String,
    /// Patch object ID appended to the active WAL.
    pub patch_id: ObjectId,
    /// WAL sequence assigned to the patch envelope.
    pub wal_sequence: u64,
    /// Number of patch operations emitted.
    pub operation_count: usize,
    /// Number of Blob object references written or reused for operation payloads.
    pub referenced_blob_count: usize,
    /// Number of `EditText` operations emitted (text-file modifications).
    pub text_edit_count: usize,
    /// Operation summaries in emitted order.
    pub changes: Vec<WorktreePatchOperationSummary>,
}

/// Summary of one generated patch operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorktreePatchOperationSummary {
    /// Repository-relative path.
    pub path: String,
    /// Generated operation kind.
    pub operation: WorktreePatchOperationKind,
}

/// Generated operation kind for CLI/reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorktreePatchOperationKind {
    /// A new file will be represented as `CreateFile`.
    CreateFile,
    /// A missing tracked file will be represented as `DeleteFile`.
    DeleteFile,
    /// A modified tracked file will be represented as `ReplaceBinary`.
    ReplaceBinary,
    /// A modified UTF-8 text file is represented as an arbitrary-span `EditText`.
    EditText,
    /// A regular file whose normalized mode changed is represented as `ChangePerm`.
    ChangePerm,
    /// A declared move (RFC 144 §4o) is represented as `RenamePath`. `path` on the report carries
    /// both sides as `"old_path -> new_path"`, since a rename is the one kind this report's
    /// single-path summary shape must describe two paths for.
    RenamePath,
}

impl WorktreePatchOperationKind {
    /// Stable CLI label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::CreateFile => "create-file",
            Self::DeleteFile => "delete-file",
            Self::ReplaceBinary => "replace-binary",
            Self::EditText => "edit-text",
            Self::ChangePerm => "change-perm",
            Self::RenamePath => "rename-path",
        }
    }
}

/// DC-57 default hard block on active (queued, unsealed) patches — NFR-PERF-02's default 1000,
/// overridable per invocation via `PRIKK_ACTIVE_PATCH_LIMIT` at the CLI boundary, never persisted.
pub const DEFAULT_ACTIVE_PATCH_LIMIT: usize = 1000;

/// DC-57 (NFR-PERF-02): true when the active WAL already holds `active_patch_limit` or more queued
/// patches, so appending one more must be refused. "Active patches" has exactly one definition — the
/// active WAL's record count — and this is the one comparison every authoring path
/// (`node_authoring.rs::author_inner`, `active.rs::ActiveSession::append_patch`) calls, rather than
/// each reimplementing it. `>=`, not `>`: once the queue already holds the limit, no more may join it.
#[must_use]
pub(crate) const fn active_patch_limit_exceeded(
    current_count: usize,
    active_patch_limit: usize,
) -> bool {
    current_count >= active_patch_limit
}

#[cfg(test)]
mod threshold_tests {
    use super::active_patch_limit_exceeded;

    /// DC-57 criterion 5: the literal boundary values named in the RFC, tested directly against the
    /// one shared comparison — this is the pure-arithmetic half of the boundary proof; the
    /// integration half (proving it is actually wired into `author_inner` before any write, with a
    /// small scaled limit) lives in `worktree_patch/tests.rs`.
    #[test]
    fn boundary_values_match_the_rfc() {
        assert!(!active_patch_limit_exceeded(799, 800));
        assert!(active_patch_limit_exceeded(800, 800));
        assert!(!active_patch_limit_exceeded(999, 1000));
        assert!(active_patch_limit_exceeded(1000, 1000));
        assert!(active_patch_limit_exceeded(1001, 1000));
    }
}

/// Options for authoring a node-addressed patch from worktree changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorktreePatchCommitOptions {
    /// Retained for API compatibility. Existing-node `NodeKind` is now authoritative for the
    /// modified-file mapping (text files author `EditText`, binary files author `ReplaceBinary`),
    /// so this flag no longer drives kind selection and is a no-op.
    pub prefer_text_edits: bool,
    /// DC-57 (NFR-PERF-02): the active-patch count — the active WAL's record count, the one
    /// definition every authoring path uses — must be strictly less than this before `author_inner`
    /// does anything else. Defaults to [`DEFAULT_ACTIVE_PATCH_LIMIT`]; the CLI overrides it from
    /// `PRIKK_ACTIVE_PATCH_LIMIT`, failing closed on a malformed value rather than silently keeping
    /// the default.
    pub active_patch_limit: usize,
}

impl WorktreePatchCommitOptions {
    /// Default options (kind-driven mapping; `prefer_text_edits` is a no-op, see the field).
    #[must_use]
    pub const fn file_level() -> Self {
        Self {
            prefer_text_edits: false,
            active_patch_limit: DEFAULT_ACTIVE_PATCH_LIMIT,
        }
    }

    /// Options with `prefer_text_edits` set; retained for API compatibility (no-op, see the field).
    #[must_use]
    pub const fn prefer_text_edits() -> Self {
        Self {
            prefer_text_edits: true,
            active_patch_limit: DEFAULT_ACTIVE_PATCH_LIMIT,
        }
    }

    /// Override the active-patch hard-block limit (DC-57). The CLI calls this with a value parsed
    /// from `PRIKK_ACTIVE_PATCH_LIMIT`; everything else keeps the default.
    #[must_use]
    pub const fn with_active_patch_limit(mut self, limit: usize) -> Self {
        self.active_patch_limit = limit;
        self
    }
}

impl Default for WorktreePatchCommitOptions {
    fn default() -> Self {
        Self::file_level()
    }
}

/// Author a node-addressed patch from worktree changes against the replay-derived baseline, sign it
/// with a real role-bound Ed25519 AUTHOR signature from the injected `signer`, and append it to the
/// active WAL (DC-09 Phase 4.4a, R1). Existing paths resolve to their persisted `node_id`; fresh
/// nodes are minted through the production `NodeIdGenerator`; text edits go through the shared
/// `text_span` module. There is no placeholder signing path.
pub fn commit_worktree_changes_signed(
    layout: &RepositoryLayout,
    ref_name: &str,
    message: &str,
    options: WorktreePatchCommitOptions,
    signer: &impl AuthorSigner,
) -> Result<WorktreePatchCommitReport> {
    layout.require_current_format()?;
    let mut generator = NodeIdGenerator::production();
    node_authoring::author_worktree_patch(
        layout,
        ref_name,
        message,
        options,
        &mut generator,
        signer,
    )
}

/// Test-only entry that injects a deterministic node-id generator and an explicit signer so authoring
/// is reproducible.
#[cfg(test)]
pub(crate) fn commit_worktree_changes_with_generator<S, A>(
    layout: &RepositoryLayout,
    ref_name: &str,
    message: &str,
    options: WorktreePatchCommitOptions,
    generator: &mut NodeIdGenerator<S>,
    signer: &A,
) -> Result<WorktreePatchCommitReport>
where
    S: crate::node::node_id_gen::NodeIdEntropySource,
    A: AuthorSigner,
{
    node_authoring::author_worktree_patch(layout, ref_name, message, options, generator, signer)
}

/// Assign a contiguous `op_seq` (1-based) for the operation at `index`. Used by node-addressed
/// worktree authoring.
pub(crate) fn next_op_seq(index: usize) -> Result<u32> {
    let next = index
        .checked_add(1)
        .ok_or_else(|| PrikkError::CanonicalEncoding("operation count overflow".to_string()))?;
    u32::try_from(next)
        .map_err(|_| PrikkError::CanonicalEncoding("operation count exceeds u32".to_string()))
}

#[cfg(test)]
mod tests;
