//! Merge, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/merge/` for its own doc comment.

#[path = "merge/dc74_merge_execution.rs"]
mod dc74_merge_execution;
#[path = "merge/dc75_merge_block_lineage.rs"]
mod dc75_merge_block_lineage;
#[path = "merge/dc75_merge_fold_execution.rs"]
mod dc75_merge_fold_execution;
#[path = "merge/dc75_merge_two_edits.rs"]
mod dc75_merge_two_edits;
#[path = "merge/dc85_merge_from_received_ref.rs"]
mod dc85_merge_from_received_ref;
#[path = "merge/merge_evidence.rs"]
mod merge_evidence;
#[path = "merge_evidence_support/mod.rs"]
mod merge_evidence_support;
#[path = "merge/merge_plan.rs"]
mod merge_plan;
#[path = "merge/merge_refusal_classes.rs"]
mod merge_refusal_classes;
#[path = "merge/merge_with_renames.rs"]
mod merge_with_renames;
#[path = "merge/rfc126_aud08_merge_update_seq_overflow.rs"]
mod rfc126_aud08_merge_update_seq_overflow;
#[path = "merge/rfc144_increment1_rename_replay.rs"]
mod rfc144_increment1_rename_replay;
#[path = "merge/rfc156_collaboration_merges.rs"]
mod rfc156_collaboration_merges;
#[path = "support/mod.rs"]
mod support;
