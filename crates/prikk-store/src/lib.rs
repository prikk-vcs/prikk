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

// RFC 131 §2.2a ruling (b): the `author` name family (author_key_index, author_signing).
mod author;
mod blob_access;
mod block_state;
mod bundle;
mod checkout;
// RFC 131 §6d.2's first grouping under the lifted §6a prohibition: `active` and `worktree_patch`,
// two co-designed layers each checking the half of a commit boundary the other owns.
mod commit_boundary;
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
mod merge;
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
// RFC 131 §6f, grouping increment 1: the three `rollback_*` modules became `rollback::{draft,
// preview, verify}`. §2.2a had refused this grouping for a `verify <-> rollback` cycle; qualified
// node naming removed the collapse that created it, re-measured before and after the move. Root
// export names below are unchanged -- this is grouping, not an API change.
mod rollback;
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
// ... -> active); see the report. `worktree_patch` itself moved to `commit_boundary` at §6d.2,
// grouped with `active` specifically, not with this family.
mod worktree;
mod worktree_marker;
mod worktree_status;

// RFC 131 §2.2a ruling (a): these eight were already one contiguous #[cfg(test)] run above --
// gates, evidence harnesses and shared fixtures, not production code -- and are now one directory
// (`test_gates/`) rather than eight top-level entries. `rfc111_seal_simulation` is production and
// stays where it was.
#[cfg(test)]
mod test_gates;

pub use author::author_key_index::{AuthorKeyBinding, author_key_binding};
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
pub use commit_boundary::active::{
    ActiveCommitResult, ActiveRefMetadata, ActiveRefOwnership, ActiveSession, active_ref_ownership,
    finish_active_publication_cleanup, read_active_ref_metadata, remove_active_ref_metadata,
    require_active_ref_for_non_empty_wal, write_active_ref_metadata,
};
pub use commit_boundary::worktree_patch::{
    DEFAULT_ACTIVE_PATCH_LIMIT, DeclarationDisclosure, DeclarationDisclosureReason,
    MOVE_HINT_SUMMARY_THRESHOLD, MoveHintCandidate, MoveHints, WorktreePatchCommitOptions,
    WorktreePatchCommitReport, WorktreePatchOperationKind, WorktreePatchOperationSummary,
    commit_worktree_changes_signed,
};
pub use commit_index::CommitIndexDivergence;
pub use compact::{
    CompactionReport, compact_received_index, compact_ref_pointer_index, compact_trust_policy,
    plan_compact_received_index, plan_compact_ref_pointer_index, plan_compact_trust_policy,
};
pub use doctor::{
    ActiveSessionRepairOutcome, ActiveSessionRepairStatus, DoctorIssue, DoctorRepairOptions,
    DoctorRepairReport, DoctorReport, DoctorSeverity, doctor_repository, repair_object_index,
    repair_repository,
};
pub use foundation::index::IndexRepairReport;
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
pub use merge::evidence::{
    MergeEvidenceDisplay, MergeEvidenceDisplayItem, MergeEvidenceDisplayOperation,
    MergeEvidenceDisplayOperationContent, MergeEvidenceDisplaySelector, MergeEvidenceTarget,
    MergePlanDisplay, prepare_merge_evidence, prepare_merge_plan,
};
pub use merge::execute::{MergeExecutionReport, execute_merge};
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
    resolve_ref_tip_block, validate_local_branch_ref, validate_local_tag_ref,
};
#[cfg(feature = "test-support")]
pub use refs::{
    force_ref_pointer_to_arbitrary_state_for_test_support,
    remove_ref_pointer_entry_for_test_support,
};
// RFC 149 increment 1: the seven `refs` helpers the operations layer's own tests reach. They are
// test-support by every measure -- no surface's production path touches one -- so they go behind the
// feature rather than into the operations-layer contract below, and never become public API.
#[cfg(feature = "test-support")]
pub use refs::{
    append_log_record_for_signature_test, append_torn_ref_log_tail_for_test,
    encode_log_record_for_test, encode_ref_container_record_for_test,
    remove_pointer_entries_for_test, write_ref_pointer_candidate_for_test,
    write_ref_pointer_entry_with_explicit_key_for_test,
};
pub use rename_declaration::{
    DeclarationRecordOutcome, RenameDeclaration, clear_rename_declarations,
    read_rename_declarations, record_rename_declaration,
};
#[cfg(feature = "test-support")]
pub use rfc111_seal_simulation::simulate_one_seal_for_test_support;
pub use rollback::draft::{RollbackDraftReport, append_rollback_draft};
pub use rollback::preview::{
    RollbackPreviewChange, RollbackPreviewChangeKind, RollbackPreviewPlan, prepare_rollback_preview,
};
pub use rollback::verify::{RollbackDraftVerification, verify_active_rollback_draft};
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
pub use worktree_status::{
    QueuedOperationContent, QueuedOperationEntry, QueuedPatchEntry, QueuedPathResolution,
    WorktreeChange, WorktreeChangeKind, WorktreeStatusReport, enumerate_queued_patches,
    worktree_status,
};

// ---------------------------------------------------------------------------------------------
// Operations-layer contract (RFC 149 §5.2b)
//
// **One named set, not a drift.** These are the items the operations layer -- the 26 modules RFC 149
// moves to `prikk-operations` -- reaches inside this crate's core (`commit_boundary`,
// `lifecycle_cache`, `patch_replay`, `refs`, `trust`). They are `pub` because that layer will live in
// another crate, and they are listed here, together, so that the cost of the cut is one reviewable
// block rather than a `pub` scattered across thirteen files.
//
// Derived from the step-2b census, not from what happened to compile: every entry is an item a
// surface module actually references today, resolved to its declaration. `trust` appears nowhere
// below because everything the surfaces reach in it was already public.
//
// **Adding to this block is a decision, not a fix.** An item that needs `pub` and is not here means
// the census missed a dependency; say so and measure it rather than appending quietly.
//
// **`#[non_exhaustive]` on every enum and struct among them** (RFC 147 ruling 2), applied at the
// declaration: a consumer in another crate reads these, it does not construct or exhaustively match
// them, and the first time it wants to is a conversation rather than a silent break.
//
// One item the census named is **not** here: `lifecycle_cache::replay::LifecycleReplayError`. Its
// `TextSpanResolutionFailed` variant carries a `text_span::TextSpanResolutionFailure`, which is
// infrastructure that stays in `prikk-store`; exposing the error would expose that type too --
// a 42nd item, beyond the 41 the owner ruled on. RFC 149 §5's hard stop, reported rather than
// widened.
pub use commit_boundary::active::{
    prepare_empty_active_ref_for_append, read_active_ref_metadata_for,
};
pub use commit_boundary::worktree_patch::{WorktreeEntryShape, authoring_refusal_reason};
pub use lifecycle_cache::incremental::verify_divergence;
pub use lifecycle_cache::replay::TextCache;
pub use lifecycle_cache::{
    ReplayDerivedLifecycleState, materialize_edited_text, replay_derived_state,
};
pub use patch_replay::apply::ReplayLiveNode;
pub use patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation, decode_patch_message,
    decode_patch_operations, decode_patch_parent_ids, ensure_apply_supported,
};
pub use patch_replay::read::{load_snapshot_files, read_block, read_patch, single_parent_chain};
pub use patch_replay::{
    PatchReplayDeletedFile, ReplayManifest, ReplayManifestEntry, apply_operation_sequence,
};
pub use refs::{
    PointerIndexEntry, encode_pointer_index_record, ensure_no_incomplete_publication,
    ensure_ref_target_valid, read_current_ref_tip_block,
};
