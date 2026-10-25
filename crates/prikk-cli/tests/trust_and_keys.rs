//! Trust and keys, grouped (0.55.0 G2 Part B). Each module below was
//! its own top-level test binary before this round; the file is unchanged, only its location
//! and this grouping file's `mod` line are new. See the module's own file under
//! `tests/trust_and_keys/` for its own doc comment.

#[path = "trust_and_keys/dc78_seal_provenance.rs"]
mod dc78_seal_provenance;
#[path = "trust_and_keys/dc78_trust_adoption.rs"]
mod dc78_trust_adoption;
#[path = "trust_and_keys/distinct_default_key_ids.rs"]
mod distinct_default_key_ids;
#[path = "trust_and_keys/rfc132_trust_precondition_sites.rs"]
mod rfc132_trust_precondition_sites;
#[path = "trust_and_keys/rfc135_key_and_setup.rs"]
mod rfc135_key_and_setup;
#[path = "trust_and_keys/rfc138_trust_read_surface.rs"]
mod rfc138_trust_read_surface;
#[path = "trust_and_keys/rfc148_key_directory.rs"]
mod rfc148_key_directory;
#[path = "trust_and_keys/rfc150_key_status.rs"]
mod rfc150_key_status;
#[path = "support/mod.rs"]
mod support;
