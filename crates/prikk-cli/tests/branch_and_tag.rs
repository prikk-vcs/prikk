//! Branch and tag, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/branch_and_tag/` for its own doc comment.

// `mod` identifiers cannot start with a digit; the file itself keeps its own name (and its own
// `#![path]`-free filename cited by nothing else), only the module identifier a `mod` declaration
// needs is adjusted.
#[path = "branch_and_tag/checkout_refusal_writes_nothing.rs"]
mod checkout_refusal_writes_nothing;
#[path = "branch_and_tag/0_53_0_step1_checkout_unpublished_branch_wording.rs"]
mod checkout_unpublished_branch_wording_0_53_0;
#[path = "branch_and_tag/dc60_branch_management.rs"]
mod dc60_branch_management;
#[path = "branch_and_tag/dc61_branch_closure.rs"]
mod dc61_branch_closure;
#[path = "branch_and_tag/dc63_tag_surface.rs"]
mod dc63_tag_surface;
#[path = "branch_and_tag/rfc117_stage3_tag_travel_cli.rs"]
mod rfc117_stage3_tag_travel_cli;
#[path = "branch_and_tag/rfc147_tag_ref_resolution.rs"]
mod rfc147_tag_ref_resolution;
#[path = "branch_and_tag/rfc151_branch_switch.rs"]
mod rfc151_branch_switch;
#[path = "branch_and_tag/rfc151_current_branch.rs"]
mod rfc151_current_branch;
#[path = "support/mod.rs"]
mod support;
