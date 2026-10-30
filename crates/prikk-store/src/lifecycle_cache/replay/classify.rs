//! 0.56.0 step 3d (A6), corrected in 3d2: classify a patch-operations decode failure *before* it is
//! folded into a `String`, so a path this version's own path rule refuses (non-ASCII) is kept
//! apart from real corruption -- matching [`prikk_error::PrikkError::NonAsciiRepoPath`] directly,
//! never rendered text, which can be edited without anyone noticing the classification silently
//! dropped. Split out of `replay.rs` itself only to keep that file under this project's own
//! line-count gate; this is still `replay`'s own classifier, used by both of its patch-operations
//! decode call sites.
//!
//! 3d2: matching the variant directly (rather than calling
//! [`prikk_error::PrikkError::decode_failure_kind`] and re-deriving a message) is what lets the
//! offending path itself (`path`, below) reach [`LifecycleReplayError::OutsideVersionRuleInLineage`]
//! without first being folded into rendered text -- the same reason that type's own `detail: String`
//! field (3d's original) is gone: it was `err.to_string()`, the raw validator message, interpolated
//! verbatim into a trailing parenthesis every reader printed.

use prikk_error::PrikkError;
use prikk_object::ObjectId;

use super::LifecycleReplayError;

pub(super) fn classify_decode_failure(patch_id: ObjectId, err: PrikkError) -> LifecycleReplayError {
    match err {
        PrikkError::NonAsciiRepoPath { path } => {
            LifecycleReplayError::OutsideVersionRuleInLineage { patch_id, path }
        }
        other => LifecycleReplayError::MalformedPatchInLineage {
            patch_id,
            detail: other.to_string(),
        },
    }
}
