use prikk_object::ObjectType;

use super::malformed_envelope;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{DEFAULT_ACTIVE_NAME, RepositoryLayout, Wal};

// RFC 103: `format1_diagnostics_are_byte_preserving_suppressed_and_deterministic` (which exercised
// `signature_diagnostics.rs`'s issues through an opened format-1 repository) is removed along with
// the rest of the format-1 scaffolding -- `RepositoryLayout::open` now refuses format-1 outright, so
// the fixture it needed can no longer exist. See `signature_diagnostics.rs::classify_signature_envelope`'s
// own doc for why that diagnostic layer is now provably unreachable through `verify_repository` at all,
// independent of this test's removal.

#[test]
fn strict_writers_do_not_admit_legacy_diagnostic_envelopes() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc39-no-read-write-promotion");
    let layout = RepositoryLayout::init(root.clone())?;
    let malformed = malformed_envelope(ObjectType::Patch, b"legacy", 63);
    assert!(malformed.validate().is_ok());
    assert!(malformed.validate_strict().is_err());
    assert!(
        Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
            .append_patch(&malformed)
            .is_err()
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
