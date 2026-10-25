//! Format and compat, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/format_and_compat/` for its own doc comment.

#[path = "format_and_compat/rfc136_anchored_text.rs"]
mod rfc136_anchored_text;
#[path = "format_and_compat/rfc136_derivation_gate.rs"]
mod rfc136_derivation_gate;
#[path = "format_and_compat/rfc136_seal_block_byte_identity.rs"]
mod rfc136_seal_block_byte_identity;
#[path = "format_and_compat/rfc136_verified_record.rs"]
mod rfc136_verified_record;
#[path = "format_and_compat/rfc156_format_7.rs"]
mod rfc156_format_7;
