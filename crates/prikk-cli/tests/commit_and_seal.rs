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
#[path = "support/mod.rs"]
mod support;
