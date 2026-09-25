//! A snapshot anchor that could not be used, and the one place such findings are collected (RFC 136 §10.3b.4,
//! increment 2c).
//!
//! **Two users, and a third** (RFC 159's state anchors, [`AnchorUse::State`]). A read-only report (`checkout --patch-plan`, `diff --from`, `tree`, ...) names a snapshot that
//! failed the loader and returns the finding beside its result. The baseline reconstruction of 2c
//! (`lifecycle_cache::anchored_text`) takes *text* from a **verified** anchor, and when that anchor's manifest
//! fails validation, or the text it yields fails its hash, the command falls back to a full replay (the answer
//! stays correct) but says so: an integrity signal is never swallowed.
//!
//! **Why a collector for the second.** Those findings arise deep inside `commit`, `worktree-status`, `merge-evidence`
//! and `diff`, whose result types carry no place for one. A per-thread list, drained once per command by the CLI
//! (`take_anchor_fallbacks`), reaches every command without widening a dozen signatures. It holds only findings
//! (a block id and a sentence), deduplicated, and never influences a result: a caller that ignores it loses the
//! message and nothing else.

use std::cell::RefCell;
use std::fmt;

use prikk_object::ObjectId;

/// What the snapshot was being used for when it failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnchorUse {
    /// A read-only report replayed from it (§10.3b.4).
    Report,
    /// A baseline reconstruction took a text from it (2c).
    Text,
    /// A signature would rest on its **state**: `seal`, `merge`, seal-from-accepted, or a worktree write (RFC 159
    /// §8.2). Named when a *recorded* anchor's manifest fails validation or its maintainer signature does not verify
    /// against an adopted key.
    State,
}

/// A snapshot an anchored read could not use because it failed validation, or because what it yielded failed
/// its own check (RFC 136 §10.3b.4, 2c). The work was done from genesis instead, so the output is unchanged.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotAnchorFallback {
    /// The block whose snapshot could not be used.
    pub block_id: ObjectId,
    /// The finding.
    pub finding: String,
    pub(crate) purpose: AnchorUse,
}

impl SnapshotAnchorFallback {
    /// A read-only report's finding (§10.3b.4).
    pub(crate) fn for_report(block_id: ObjectId, finding: String) -> Self {
        Self {
            block_id,
            finding,
            purpose: AnchorUse::Report,
        }
    }

    /// A baseline text's finding (2c).
    pub(crate) fn for_text(block_id: ObjectId, finding: String) -> Self {
        Self {
            block_id,
            finding,
            purpose: AnchorUse::Text,
        }
    }

    /// A state anchor's finding (RFC 159 §8.2).
    pub(crate) fn for_state(block_id: ObjectId, finding: String) -> Self {
        Self {
            block_id,
            finding,
            purpose: AnchorUse::State,
        }
    }
}

impl fmt::Display for SnapshotAnchorFallback {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.purpose {
            AnchorUse::Report => write!(
                f,
                "warning: the snapshot of Block {} failed validation ({}); this report replayed the whole \
                 history instead -- run `prikk verify`",
                self.block_id, self.finding
            ),
            AnchorUse::Text => write!(
                f,
                "warning: the verified snapshot of Block {} could not supply a file's text ({}); this \
                 command replayed the whole history instead -- run `prikk verify`",
                self.block_id, self.finding
            ),
            AnchorUse::State => write!(
                f,
                "warning: the recorded snapshot of Block {} was not used as an anchor ({}); this \
                 command derived its state from the whole history instead -- run `prikk verify`",
                self.block_id, self.finding
            ),
        }
    }
}

thread_local! {
    static PENDING: RefCell<Vec<SnapshotAnchorFallback>> = const { RefCell::new(Vec::new()) };
}

/// Note a finding for the CLI to name. Identical findings are kept once.
pub(crate) fn record_anchor_fallback(fallback: SnapshotAnchorFallback) {
    PENDING.with(|pending| {
        let mut pending = pending.borrow_mut();
        if !pending.contains(&fallback) {
            pending.push(fallback);
        }
    });
}

/// The findings noted on this thread since the last call, oldest first. The CLI calls it once, after a
/// command, and prints each on stderr (`warn_anchor_fallbacks`).
///
/// **A library caller must drain it after each operation it wants findings for** (any store call that reads a
/// baseline or a file's text: a commit, a status, a merge-evidence, a diff): findings accumulate per thread until
/// taken, and the list is deduplicated but not bounded by anything else. **It never affects a result.** A finding
/// records that a verified snapshot could not be used and the answer came from a full replay instead; the answer
/// is the same either way, so a caller that never drains loses the message and nothing else.
#[must_use]
pub fn take_anchor_fallbacks() -> Vec<SnapshotAnchorFallback> {
    PENDING.with(|pending| std::mem::take(&mut *pending.borrow_mut()))
}
