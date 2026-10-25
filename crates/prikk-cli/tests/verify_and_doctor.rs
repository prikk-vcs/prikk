//! Verify and doctor, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/verify_and_doctor/` for its own doc comment.

#[path = "verify_and_doctor/complete_damaged_record_is_not_interrupted_append.rs"]
mod complete_damaged_record_is_not_interrupted_append;
#[path = "verify_and_doctor/damaged_container_header.rs"]
mod damaged_container_header;
#[path = "verify_and_doctor/interrupted_append.rs"]
mod interrupted_append;
#[path = "verify_and_doctor/rfc102_doctor_repair_index.rs"]
mod rfc102_doctor_repair_index;
#[path = "verify_and_doctor/rfc102_object_store_lock_instrument.rs"]
mod rfc102_object_store_lock_instrument;
#[path = "verify_and_doctor/rfc102_part_e_lost_generation_log.rs"]
mod rfc102_part_e_lost_generation_log;
#[path = "verify_and_doctor/rfc102_stage6_compact.rs"]
mod rfc102_stage6_compact;
#[path = "verify_and_doctor/rfc102_stage6_unlock.rs"]
mod rfc102_stage6_unlock;
#[path = "verify_and_doctor/rfc108_increment3d_repair_cli.rs"]
mod rfc108_increment3d_repair_cli;
#[path = "verify_and_doctor/rfc118_stage5_verify_json.rs"]
mod rfc118_stage5_verify_json;
#[path = "verify_and_doctor/rfc156_stage0_superseding_record_compatibility.rs"]
mod rfc156_stage0_superseding_record_compatibility;
#[path = "verify_and_doctor/rfc156_verify_checks_every_signature.rs"]
mod rfc156_verify_checks_every_signature;
#[path = "verify_and_doctor/rfc158_incoming_bound.rs"]
mod rfc158_incoming_bound;
#[path = "verify_and_doctor/rfc162_recovery_matrix.rs"]
mod rfc162_recovery_matrix;
#[path = "verify_and_doctor/rfc163_generation_log.rs"]
mod rfc163_generation_log;
#[path = "verify_and_doctor/rfc163_n6_trailing_partial_suppressed.rs"]
mod rfc163_n6_trailing_partial_suppressed;
#[path = "verify_and_doctor/rfc163_write_never_buries_a_crash_state.rs"]
mod rfc163_write_never_buries_a_crash_state;
#[path = "verify_and_doctor/rfc164_a6_item4_repair_tails_plan_only.rs"]
mod rfc164_a6_item4_repair_tails_plan_only;
#[path = "verify_and_doctor/rfc164_addendum1_matrix.rs"]
mod rfc164_addendum1_matrix;
#[path = "verify_and_doctor/rfc164_addendum3_all_files_reader.rs"]
mod rfc164_addendum3_all_files_reader;
#[path = "verify_and_doctor/rfc164_rule_d_refuse_before_first_write.rs"]
mod rfc164_rule_d_refuse_before_first_write;
#[path = "verify_and_doctor/rfc165_addendum1_wedge_and_entry_points.rs"]
mod rfc165_addendum1_wedge_and_entry_points;
// The file itself keeps its own name; the `mod` identifier is adjusted to snake_case
// (`rustc`'s `nonstandard_style` lint, `-D warnings` under the `clippy*` gates).
#[path = "verify_and_doctor/rfc165_step1_partA_ref_complete_directly.rs"]
mod rfc165_step1_part_a_ref_complete_directly;
#[path = "verify_and_doctor/rfc165_step1_partC_keep_slot.rs"]
mod rfc165_step1_part_c_keep_slot;
#[path = "verify_and_doctor/rfc166_a6_item3_no_witness_classification_named.rs"]
mod rfc166_a6_item3_no_witness_classification_named;
#[path = "verify_and_doctor/rfc166_round2_refusals_speak_first.rs"]
mod rfc166_round2_refusals_speak_first;
#[path = "verify_and_doctor/rfc168_recovery_log.rs"]
mod rfc168_recovery_log;
#[path = "verify_and_doctor/torn_tail_is_one_frame.rs"]
mod torn_tail_is_one_frame;
