#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Storage crate for Prikk repositories.
//!
//! This crate provides persistent layout, object storage, WAL durability, deeper read-only
//! repository verification, initial ref-state/ref-log publication primitives, a narrow
//! active-session append API, opt-in safe doctor repairs, conservative snapshot materialization,
//! read-only worktree status, minimal worktree-to-patch draft generation, supported patch replay
//! planning and materialization, explicit opt-in deletion of patch-removed files, deterministic
//! arbitrary-span text edit replay and generation from worktree changes, explicit unborn local branch
//! genesis through active-WAL ref ownership,
//! read-only inverse planning for the supported patch subset, non-mutating rollback preview,
//! conservative rollback draft append to an empty active WAL, rollback draft verification, sealed
//! rollback block classification, and an internal patch-algebra foundation. Production confluence,
//! plugin execution, and remote sync remain separate increments.

mod active;
// RFC 131 §2.2a ruling (b): the `author` name family (author_key_index, author_signing).
mod author;
mod blob_access;
mod block_state;
mod bundle;
mod checkout;
mod commit_index;
mod compact;
mod doctor;
// RFC 131 §2 / RFC 130 §2.3's "foundation" grouping: layout, fsutil, byte_cursor, file_codec,
// frame_resync, container, index, generation -- a wide, one-directional base, grouped by role.
mod format;
mod foundation;
mod history;
mod ignore;
mod lifecycle_cache;
mod lock;
mod maintainer_signing;
mod memory_store;
mod merge_evidence;
mod merge_execute;
// Production node-id minting (DC-09 Phase 4.4a-1), consumed by node-addressed worktree authoring
// (4.4a-2) for fresh-node creation. RFC 131 §2.2a ruling (b): the `node` name family (node_id_gen,
// node_lifecycle).
mod node;
mod object_store;
// Patch algebra foundation and evidence contract (DC-16/DC-21), now production-compiled through the
// DC-22 read-only merge-evidence store boundary. RFC 131 §2.2a: the `patch` name family is NOT
// grouped -- measured to introduce new coupling-graph cycles (patch<->merge, patch<->tag_travel);
// see the report.
mod patch_algebra;
mod patch_checkout;
mod patch_exchange;
mod patch_inverse;
mod patch_replay;
mod patch_set_digest;
mod path;
// RFC 131 §2.2a ruling (b): the `received` name family (received, received_index).
mod received;
mod recognition_claim;
mod refs;
mod rename_declaration;
mod rfc111_seal_simulation;
// RFC 131 §2.2a: the `rollback` name family is NOT grouped -- measured to introduce a new
// verify<->rollback coupling-graph cycle via a shared `patch` dependency; see the report.
mod rollback_draft;
mod rollback_preview;
mod rollback_verify;
mod seal_from_accepted;
// RFC 142: `prikk show` -- what a block or patch changed.
mod show;
mod signature_diagnostics;
mod snapshot;
mod state_root;
mod sync_negotiation;
mod tag_travel;
mod text_span;
mod trust;
mod trust_index;
mod unlock;
mod verify;
mod wal;
// RFC 131 §2.2a: the `worktree` name family is NOT grouped -- measured to reconnect into the
// coupling gate's own SCC (worktree_patch -> worktree -> active/patch_replay/lifecycle_cache ->
// ... -> active); see the report. `worktree_patch` is one of §2's constrained modules regardless.
mod worktree;
mod worktree_marker;
mod worktree_patch;
mod worktree_status;

// RFC 131 §2.2a ruling (a): these eight were already one contiguous #[cfg(test)] run above --
// gates, evidence harnesses and shared fixtures, not production code -- and are now one directory
// (`test_gates/`) rather than eight top-level entries. `rfc111_seal_simulation` is production and
// stays where it was.
#[cfg(test)]
mod test_gates;

pub use active::{
    ActiveCommitResult, ActiveRefMetadata, ActiveRefOwnership, ActiveSession, active_ref_ownership,
    finish_active_publication_cleanup, read_active_ref_metadata, remove_active_ref_metadata,
    require_active_ref_for_non_empty_wal, write_active_ref_metadata,
};
pub use author::author_signing::{AuthorSigner, Ed25519AuthorSigner, author_signature};
pub use block_state::{
    BlockStateOutcome, BlockStateStatus, derive_next_state_root, validate_block_v2_shape,
};
pub use bundle::{
    BundleExportReport, BundleImportOptions, BundleImportReport, BundleManifest,
    BundlePreviewConflict, BundlePreviewConnectivity, BundlePreviewEffect, BundlePreviewEffectKind,
    BundlePreviewReport, BundleScope, BundleVerifyReport, DEFAULT_BUNDLE_MAX_OBJECT_COUNT,
    DEFAULT_BUNDLE_MAX_TOTAL_BYTES, export_bundle, import_bundle, preview_bundle, verify_bundle,
};
pub use checkout::{
    CheckoutMaterialization, CheckoutPlan, DEFAULT_CHECKOUT_REF, SnapshotCheckoutPlan,
    prepare_checkout_plan, prepare_snapshot_checkout_plan,
};
pub use commit_index::CommitIndexDivergence;
pub use compact::{
    CompactionReport, compact_received_index, compact_ref_pointer_index, compact_trust_policy,
    plan_compact_received_index, plan_compact_ref_pointer_index, plan_compact_trust_policy,
};
pub use doctor::{
    ActiveSessionRepairOutcome, ActiveSessionRepairStatus, DoctorIssue, DoctorRepairOptions,
    DoctorRepairReport, DoctorReport, DoctorSeverity, doctor_repository, repair_repository,
};
pub use foundation::layout::{
    ContainerSlot, DEFAULT_ACTIVE_NAME, LockableContainer, RepositoryFormat, RepositoryLayout,
};
pub use history::{
    DEFAULT_HISTORY_LIMIT, HistoryEntry, RefHistory, load_received_ref_history, load_ref_history,
};
pub use lifecycle_cache::incremental::LifecycleCacheDivergence;
pub use lock::{ActiveLock, ContainerLockGuard, RefLock, acquire_container_locks};
pub use maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner, maintainer_signature};
pub use memory_store::MemoryObjectStore;
pub use merge_evidence::{
    MergeEvidenceDisplay, MergeEvidenceDisplayItem, MergeEvidenceDisplayOperation,
    MergeEvidenceDisplaySelector, MergeEvidenceTarget, MergePlanDisplay, prepare_merge_evidence,
    prepare_merge_plan,
};
pub use merge_execute::{MergeExecutionReport, execute_merge};
pub use object_store::{
    FileObjectStore, ObjectReadSnapshot, ObjectReader, ObjectWriteSession, ObjectWriter,
};
pub use patch_checkout::{
    PatchDeletionConflict, PatchDeletionPlan, PatchMaterializationReport,
    materialize_patch_checkout, materialize_patch_checkout_with_deletions,
    plan_patch_checkout_deletions,
};
pub use patch_exchange::{
    AcceptOptions, AcceptReport, ClaimSignatureVerification,
    DEFAULT_EXCHANGE_ARTIFACT_MAX_OBJECT_COUNT, DEFAULT_EXCHANGE_ARTIFACT_MAX_TOTAL_BYTES,
    ExchangeExportReport, accept_exchange_artifact, accepted_but_unsealed_patch_ids,
    export_exchange_artifact,
};
pub use patch_inverse::{
    PatchInverseOperationKind, PatchInverseOperationSummary, PatchInversePlan,
    prepare_patch_inverse_plan,
};
pub use patch_replay::{
    PatchPlanContent, PatchPlanContentEntry, PatchPlanContentReport, PatchPlanCoverage,
    PatchReplayPlan, prepare_patch_plan_content_report, prepare_patch_replay_plan,
};
pub use patch_set_digest::{
    PatchSetDigest, PatchSetResolution, compute_patch_set_digest,
    compute_patch_set_digest_and_count_from_block, compute_patch_set_digest_for_ref,
    compute_patch_set_digest_from_block, patch_ids_reachable_from_block, patch_set_digest_preimage,
    resolve_patch_set_digest,
};
pub use path::{RepoPath, validate_no_path_collisions, validate_repo_path};
pub use received::{
    ReceivedPointer, list_received_pointers, read_received_pointer, validate_received_ref,
};
pub use recognition_claim::{
    RecognitionClaimConsistency, check_recognition_claim_consistency, order_claims_for_sealing,
};
pub use refs::{
    RefFileOutcome, RefFileStatus, RefItemOutcome, RefItemStatus, RefLogRecord, RefLogReplay,
    RefPointerSummary, RefPublication, RefPublicationIssue, RefRecoveryCandidate, RefStore,
    validate_local_branch_ref, validate_local_tag_ref,
};
#[cfg(feature = "test-support")]
pub use refs::{
    force_ref_pointer_to_arbitrary_state_for_test_support,
    remove_ref_pointer_entry_for_test_support,
};
pub use rename_declaration::{
    RenameDeclaration, clear_rename_declarations, read_rename_declarations,
    record_rename_declaration,
};
#[cfg(feature = "test-support")]
pub use rfc111_seal_simulation::simulate_one_seal_for_test_support;
pub use rollback_draft::{RollbackDraftReport, append_rollback_draft};
pub use rollback_preview::{
    RollbackPreviewChange, RollbackPreviewChangeKind, RollbackPreviewPlan, prepare_rollback_preview,
};
pub use rollback_verify::{RollbackDraftVerification, verify_active_rollback_draft};
pub use seal_from_accepted::{SealFromAcceptedOutcome, seal_from_accepted_claim};
pub use show::{
    ShowBlobContent, ShowDeletePreimage, ShowOperation, ShowOperationContent, ShowPatch,
    ShowPathResolution, show,
};
pub use signature_diagnostics::{SignatureEnvelopeIssue, SignatureEnvelopeSource};
pub use snapshot::{SnapshotEntry, SnapshotManifest};
pub use state_root::{
    StateRootContent, StateRootEntry, compute_state_root, state_leaf_hash, state_leaf_preimage,
};
pub use sync_negotiation::{
    DEFAULT_HAVE_LIST_MAX_PATCH_COUNT, DEFAULT_HAVE_LIST_MAX_TOTAL_BYTES,
    DEFAULT_SYNC_SUMMARY_MAX_REF_COUNT, DEFAULT_SYNC_SUMMARY_MAX_TOTAL_BYTES, HaveList,
    SyncArtifactBuildReport, SyncArtifactOutcome, SyncRefComparison, SyncRefComparisonState,
    SyncSummaryRefEntry, build_have_list, build_sync_artifact, build_sync_summary,
    compare_sync_summary, compute_sync_delta, decode_have_list, decode_sync_summary,
};
pub use tag_travel::{
    LocalTagCreation, ReceivedTagResolution, ReceivedTagSummary, TagSignatureVerification,
    adopt_tag, create_local_tag, list_received_tags, received_tag_ids,
};
pub use trust::{
    AdoptedMaintainerKey, GatedOperation, MaintainerTrustPolicy, PublicationTrustIssue,
    add_trusted_maintainer, load_maintainer_trust_policy, load_maintainer_trust_policy_or_empty,
    remove_trusted_maintainer, verify_signer_trusted, verify_trusted_publication_envelope,
};
pub use unlock::{HeldLock, PidLiveness, clear_lock, find_held_lock, list_held_locks};
pub use verify::{
    ActiveWalMetadataStatus, ActiveWalOrderingIssue, AuthorSignatureVerification,
    BlockSealVerification, ObjectItemOutcome, ObjectItemStatus, ObjectVerification,
    RepositoryVerification, StageOutcome, StageStatus, VerificationStage, VerifyOptions,
    verify_repository, verify_repository_with_options,
};
pub use wal::{Wal, WalRecord, WalRepair, WalReplay};
pub use worktree::{SnapshotMaterializationReport, materialize_snapshot_checkout};
pub use worktree_patch::{
    DEFAULT_ACTIVE_PATCH_LIMIT, WorktreePatchCommitOptions, WorktreePatchCommitReport,
    WorktreePatchOperationKind, WorktreePatchOperationSummary, commit_worktree_changes_signed,
};
pub use worktree_status::{
    QueuedOperationEntry, QueuedPatchEntry, QueuedPathResolution, WorktreeChange,
    WorktreeChangeKind, WorktreeStatusReport, enumerate_queued_patches, worktree_status,
};
