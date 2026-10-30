//! 0.56.0 step 3d (A6): classify a patch-operations decode failure *before* it is folded into a
//! `String`, so a path this version's own path rule refuses (today: non-ASCII) is kept apart from
//! real corruption -- matching [`prikk_error::PrikkError::decode_failure_kind`], never rendered
//! text, which can be edited without anyone noticing the classification silently dropped. Split
//! out of `replay.rs` itself only to keep that file under this project's own line-count gate; this
//! is still `replay`'s own classifier, used by both of its patch-operations decode call sites.

use prikk_error::{DecodeFailureKind, PrikkError};
use prikk_object::ObjectId;

use super::LifecycleReplayError;

pub(super) fn classify_decode_failure(patch_id: ObjectId, err: PrikkError) -> LifecycleReplayError {
    let detail = err.to_string();
    if err.decode_failure_kind() == DecodeFailureKind::OutsideVersionRule {
        LifecycleReplayError::OutsideVersionRuleInLineage { patch_id, detail }
    } else {
        LifecycleReplayError::MalformedPatchInLineage { patch_id, detail }
    }
}
