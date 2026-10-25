//! Sync and exchange, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/sync_and_exchange/` for its own doc comment.

#[path = "sync_and_exchange/received.rs"]
mod received;
#[path = "sync_and_exchange/release_0_52_0_p3b_received_ref_failure_named.rs"]
mod release_0_52_0_p3b_received_ref_failure_named;
#[path = "sync_and_exchange/rfc116_sync_cli.rs"]
mod rfc116_sync_cli;
#[path = "sync_and_exchange/rfc132_absent_and_received_refs.rs"]
mod rfc132_absent_and_received_refs;
#[path = "sync_and_exchange/rfc146_unpublished_current_branch_listing.rs"]
mod rfc146_unpublished_current_branch_listing;
#[path = "sync_and_exchange/rfc147_unpublished_current_branch.rs"]
mod rfc147_unpublished_current_branch;
