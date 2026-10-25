//! Harness self test, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/harness_self_test/` for its own doc comment.

#[path = "support/mod.rs"]
mod support;
#[path = "harness_self_test/support_binary_identity.rs"]
mod support_binary_identity;
