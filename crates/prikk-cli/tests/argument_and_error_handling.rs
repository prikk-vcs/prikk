//! Argument and error handling, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/argument_and_error_handling/` for its own doc comment.

#[path = "argument_and_error_handling/rfc121_argument_hygiene.rs"]
mod rfc121_argument_hygiene;
#[path = "argument_and_error_handling/rfc121_epipe.rs"]
mod rfc121_epipe;
#[path = "argument_and_error_handling/rfc121_exit_code_contract.rs"]
mod rfc121_exit_code_contract;
#[path = "argument_and_error_handling/rfc123_message_in_log.rs"]
mod rfc123_message_in_log;
#[path = "argument_and_error_handling/rfc124_ignore_mechanism.rs"]
mod rfc124_ignore_mechanism;
#[path = "argument_and_error_handling/rfc132_part1_precondition_ownership.rs"]
mod rfc132_part1_precondition_ownership;
#[path = "argument_and_error_handling/rfc132_part2_precondition_prefix.rs"]
mod rfc132_part2_precondition_prefix;
#[path = "argument_and_error_handling/rfc147_authoring_refusal_field.rs"]
mod rfc147_authoring_refusal_field;
