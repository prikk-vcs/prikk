//! Extract and read, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/extract_and_read/` for its own doc comment.

#[path = "extract_and_read/extract.rs"]
mod extract;
#[path = "extract_and_read/rfc140_status_json_and_enumeration.rs"]
mod rfc140_status_json_and_enumeration;
#[path = "extract_and_read/rfc142_queued_patch_paths.rs"]
mod rfc142_queued_patch_paths;
#[path = "extract_and_read/rfc142_queued_patches.rs"]
mod rfc142_queued_patches;
#[path = "extract_and_read/rfc142_show.rs"]
mod rfc142_show;
#[path = "extract_and_read/rfc142_show_block_64_patch_cost.rs"]
mod rfc142_show_block_64_patch_cost;
#[path = "extract_and_read/rfc142_show_merge_block_per_patch.rs"]
mod rfc142_show_merge_block_per_patch;
#[path = "extract_and_read/rfc143_content_at_a_point.rs"]
mod rfc143_content_at_a_point;
#[path = "extract_and_read/rfc146_listing_json.rs"]
mod rfc146_listing_json;
#[path = "extract_and_read/rfc153_diff.rs"]
mod rfc153_diff;
#[path = "extract_and_read/rfc153_diff_worktree.rs"]
mod rfc153_diff_worktree;
#[path = "extract_and_read/rfc153_point_resolver.rs"]
mod rfc153_point_resolver;
#[path = "extract_and_read/rfc156_show_every_signer.rs"]
mod rfc156_show_every_signer;
#[path = "extract_and_read/rfc157_cat.rs"]
mod rfc157_cat;
#[path = "extract_and_read/rfc157_tree.rs"]
mod rfc157_tree;
#[path = "extract_and_read/show_fix_block_reading_command_sweep.rs"]
mod show_fix_block_reading_command_sweep;
#[path = "extract_and_read/step3f_claim_chain_report.rs"]
mod step3f_claim_chain_report;
#[path = "support/mod.rs"]
mod support;
