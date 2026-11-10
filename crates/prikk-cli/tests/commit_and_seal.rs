//! Commit and seal, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/commit_and_seal/` for its own doc comment.

#[path = "commit_and_seal/commit_witness_read_only_commands.rs"]
mod commit_witness_read_only_commands;
#[path = "commit_and_seal/dc56_commit_index.rs"]
mod dc56_commit_index;
#[path = "commit_and_seal/dc59_commit_benchmark.rs"]
mod dc59_commit_benchmark;
#[path = "commit_and_seal/dc65_text_edit_baseline.rs"]
mod dc65_text_edit_baseline;
#[path = "commit_and_seal/dc66_multi_commit_queuing.rs"]
mod dc66_multi_commit_queuing;
#[path = "commit_and_seal/dc67_ordinary_use_conformance.rs"]
mod dc67_ordinary_use_conformance;
#[path = "commit_and_seal/dc92_lineage_replay_benchmark.rs"]
mod dc92_lineage_replay_benchmark;
#[path = "commit_and_seal/genesis_end_to_end.rs"]
mod genesis_end_to_end;
#[path = "commit_and_seal/helper_uniqueness.rs"]
mod helper_uniqueness;
#[path = "commit_and_seal/p2b_stale_commit_way_out_every_shape.rs"]
mod p2b_stale_commit_way_out_every_shape;
#[path = "commit_and_seal/p3b_worktree_record_naming_a_missing_block.rs"]
mod p3b_worktree_record_naming_a_missing_block;
#[path = "commit_and_seal/rfc111_seal_drift_guard.rs"]
mod rfc111_seal_drift_guard;
#[path = "commit_and_seal/rfc121_aud10_signer_ordering.rs"]
mod rfc121_aud10_signer_ordering;
#[path = "commit_and_seal/rfc126_aud08_seal_update_seq_overflow.rs"]
mod rfc126_aud08_seal_update_seq_overflow;
#[path = "commit_and_seal/seal_recovery.rs"]
mod seal_recovery;
#[path = "commit_and_seal/seal_trust_gate.rs"]
mod seal_trust_gate;
#[path = "commit_and_seal/step3_s5b_uncommit.rs"]
mod step3_s5b_uncommit;
#[path = "commit_and_seal/step3e_t2_non_ascii_authoring.rs"]
mod step3e_t2_non_ascii_authoring;
#[path = "commit_and_seal/step3w2_worktree_baseline.rs"]
mod step3w2_worktree_baseline;
#[path = "commit_and_seal/step3w2b_stale_authoring.rs"]
mod step3w2b_stale_authoring;
#[path = "commit_and_seal/step3w2c_cross_ref_and_fold_resync.rs"]
mod step3w2c_cross_ref_and_fold_resync;
#[path = "commit_and_seal/step3w2d_symmetric_record_silence.rs"]
mod step3w2d_symmetric_record_silence;
#[path = "commit_and_seal/step3w2f_record_outlives_pointer.rs"]
mod step3w2f_record_outlives_pointer;
#[path = "commit_and_seal/step3w3_0_refusal_names_a_way_out_that_works.rs"]
mod step3w3_0_refusal_names_a_way_out_that_works;
#[path = "commit_and_seal/step3w3_r4_queue_ownership.rs"]
mod step3w3_r4_queue_ownership;
#[path = "commit_and_seal/step3w3b_merge_into_current_branch.rs"]
mod step3w3b_merge_into_current_branch;
#[path = "commit_and_seal/step3w3b_rollback_draft_output_sentence.rs"]
mod step3w3b_rollback_draft_output_sentence;
#[path = "commit_and_seal/step3w3c1_merge_refusal_writes_nothing.rs"]
mod step3w3c1_merge_refusal_writes_nothing;
#[path = "commit_and_seal/step3w3c2_branch_create_from_current_branch.rs"]
mod step3w3c2_branch_create_from_current_branch;
#[path = "commit_and_seal/step3w3c3_in_the_way_before_publish.rs"]
mod step3w3c3_in_the_way_before_publish;
#[path = "commit_and_seal/step3w3c3b_merge_in_the_way_before_publish.rs"]
mod step3w3c3b_merge_in_the_way_before_publish;
#[path = "commit_and_seal/step3w3c4_sync_seal_current_branch.rs"]
mod step3w3c4_sync_seal_current_branch;
#[path = "commit_and_seal/step3w3c5_archive_import_adopt_current_branch.rs"]
mod step3w3c5_archive_import_adopt_current_branch;
#[path = "support/mod.rs"]
mod support;
