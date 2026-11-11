#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Object identity and canonical payload definitions for Prikk.
//!
//! This crate deliberately does **not** use protobuf bytes for object identity. It implements a
//! small deterministic canonical encoder seed that can be extended as FDD-03 matures.

pub mod canonical;
pub mod envelope;
pub mod id;
pub mod path;
pub mod payload;
pub mod signature;

#[cfg(test)]
mod object_type_table_binding_gate;
#[cfg(test)]
mod vectors;

pub use canonical::{CanonicalEncode, CanonicalWriter, WireType};
pub use envelope::{ObjectEnvelope, SignatureEnvelopeIssues};
pub use id::{OBJECT_ID_DOMAIN, ObjectId, ObjectType};
#[cfg(feature = "test-support")]
pub use path::{StricterRuleGuard, fold_one_more_pair_for_test};
pub use path::{
    ascii_fold, canonical_caseless_key, escape_non_ascii, first_refused_character,
    is_windows_reserved_name, non_nfc_spelling, refusal_reason, validate_repo_path,
};
pub use payload::*;
pub use signature::{ED25519_SIGNATURE_LEN, Signature, SignatureAlgorithm, SignerRole};
