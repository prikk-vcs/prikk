//! **The one anchor-trust function** (RFC 159 §8.2, ruled 2026-09-26).
//!
//! A snapshot's *state* may reach a signature -- a sealed block, a merge block, a block sealed from an accepted claim,
//! or a worktree that a later `commit` will author against -- only through [`AnchorTrust::admit`], and `admit` returns a
//! usable anchor only if **all four** hold:
//!
//! 1. the block is **in the replay-verified record** (`verified_blocks`): this repository confirmed its root by replay;
//! 2. its **maintainer signature verifies against a key in this repository's adopted maintainer trust policy**
//!    (`trust::verify_trusted_publication_envelope`, the check `verify` and `seal` already use for a published block);
//! 3. its snapshot **passes `validate_snapshot_manifest`** (the manifest recomputes to the block's signed
//!    `state_merkle_root`, and every content blob it names is present);
//! 4. it is **at most [`MAX_ANCHOR_DISTANCE`] blocks back** from the block the caller is deriving from, which is what
//!    bounds the fold after it (`CHECKPOINT_CADENCE - 1`: a healthy lineage never needs more).
//!
//! Anything else is no anchor, and the caller runs today's full derivation. **A rebuildable local file therefore only
//! chooses *which* authenticated anchor to use; it never makes one.** Forging an anchor takes a trusted maintainer who
//! signed a wrong root *and* write access to `.prikk/cache/` (the residual RFC 159 §8.2 states).
//!
//! **Failure is classified, not folded** (the 2c pattern). An anchor that is simply not usable is **silent**: not
//! recorded, not near enough, no snapshot, a signer this repository has not adopted, an unreadable block. A **recorded**
//! anchor whose manifest fails validation, or whose signature does not verify against an adopted key, is an integrity
//! signal: [`Admission::Signal`] carries the finding, and the caller names it (`record_anchor_fallback`, or the finding a
//! replay already returns) and runs the full derivation.
//!
//! **Two users, one rule.** Stack A (`block_state::anchored_parent`: `seal`, `merge`, seal-from-accepted) and RFC 136
//! increment 2b's anchored worktree writes (`patch_replay::anchor`'s `VerifiedWorktreeWrite`, and
//! `worktree::materialize_snapshot_checkout`'s provisional marker). 2c's anchored *text* is not one of them: it is
//! hash-checked against the content id the caller expects, so it certifies itself. A read-only report
//! (`Anchoring::ReadOnlyReport`) shows state as its block signs it and is not one of them either (§10.3c ruling 1).
//!
//! **Which site consulted it** is a name, counted under `cfg(any(test, feature = "test-support"))` so each call site's
//! control can show that removing the anchoring *there* turns *its* test red and no other.

use std::collections::BTreeSet;

use prikk_object::{BlockPayload, ObjectId};

use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::snapshot::{CHECKPOINT_CADENCE, SnapshotManifest, validate_snapshot_manifest};
use crate::trust::{
    MaintainerTrustPolicy, load_maintainer_trust_policy_or_empty,
    verify_trusted_publication_envelope,
};
use crate::verified_blocks::load_verified_blocks;

/// The most blocks between an anchor and the block a derivation starts from. A block sealed on a parent this far
/// behind its nearest snapshot is itself a checkpoint (`checkpoint_due`), so a healthy lineage never needs more.
pub(crate) const MAX_ANCHOR_DISTANCE: usize = CHECKPOINT_CADENCE as usize - 1;

/// The stable issue code `verify_trusted_publication_envelope` gives a signature by an adopted key that does not verify.
const INVALID_SIGNATURE: &str = "PRIKK-TRUST-PUBLICATION-INVALID-SIGNATURE";

/// Where a snapshot's state is about to be used. A name only: the rule is the same at every site.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AnchorSite {
    /// `prikk seal` (`prikk-cli`'s `seal_active_no_audit`, through the public `seal_block`).
    Seal,
    /// `prikk merge` (`merge::execute`).
    Merge,
    /// `prikk sync seal` (`seal_from_accepted`).
    SealFromAccepted,
    /// An anchored worktree write: `checkout --patch-materialize*` and `branch switch`
    /// (`patch_replay::replay_for_verified_worktree_write`).
    WorktreeWrite,
    /// Whether `checkout --snapshot-materialize` may skip the provisional marker (`worktree.rs`).
    SnapshotMaterialize,
}

impl AnchorSite {
    #[cfg(any(test, feature = "test-support"))]
    fn name(self) -> &'static str {
        match self {
            Self::Seal => "seal",
            Self::Merge => "merge",
            Self::SealFromAccepted => "seal-from-accepted",
            Self::WorktreeWrite => "worktree-write",
            Self::SnapshotMaterialize => "snapshot-materialize",
        }
    }
}

/// What the anchor-trust function answered for one block.
#[derive(Debug)]
pub(crate) enum Admission {
    /// All four conditions hold. The validated manifest, so the caller does not validate it a second time.
    Usable(SnapshotManifest),
    /// Not an anchor, and **silently**: the caller looks further back (within the distance bound) or runs the full
    /// derivation.
    NotUsable,
    /// A **recorded** anchor that fails validation or whose signature does not verify against an adopted key: an
    /// integrity signal. The caller names the finding and runs the full derivation; it does not look further back.
    Signal(String),
}

/// The record and the adopted-key policy, loaded once for one derivation. A missing or unreadable record is the empty
/// set, and an unreadable or empty policy adopts nothing: either way there is no anchor and the full derivation runs.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct AnchorTrust {
    verified: BTreeSet<ObjectId>,
    policy: MaintainerTrustPolicy,
}

impl AnchorTrust {
    /// Load the record and the policy.
    pub(crate) fn load(layout: &RepositoryLayout) -> Self {
        Self {
            verified: load_verified_blocks(layout),
            policy: load_maintainer_trust_policy_or_empty(layout)
                .unwrap_or(MaintainerTrustPolicy { keys: Vec::new() }),
        }
    }

    /// Whether the record names anything at all (a caller with an empty record can skip its search).
    pub(crate) fn has_record(&self) -> bool {
        !self.verified.is_empty()
    }

    /// Whether `block_id` is in the record. The record alone never makes an anchor; a caller uses this only to decide
    /// whether reading a block further is worth it, never to *use* one.
    #[cfg(test)]
    pub(crate) fn records(&self, block_id: &ObjectId) -> bool {
        self.verified.contains(block_id)
    }

    /// **The function.** `distance` is how many blocks back from the block the caller derives from `block_id` is
    /// (0 for that block itself). The conditions are checked cheapest first; a condition that fails before the block
    /// is known to be *recorded* is silent, one that fails after is a signal only where §8.2 says so.
    pub(crate) fn admit(
        &self,
        reader: &impl ObjectReader,
        block_id: ObjectId,
        distance: usize,
        site: AnchorSite,
    ) -> Admission {
        // 4. Near enough.
        if distance > MAX_ANCHOR_DISTANCE {
            return Admission::NotUsable;
        }
        // 1. In the record.
        if !self.verified.contains(&block_id) {
            return Admission::NotUsable;
        }
        // 2. Signed by an adopted maintainer key. An unreadable or non-Block object is simply not usable: the full
        //    derivation reads it too and reports what is wrong.
        let Ok(Some(envelope)) = reader.read_object(block_id) else {
            return Admission::NotUsable;
        };
        if envelope.object_type != prikk_object::ObjectType::Block {
            return Admission::NotUsable;
        }
        match verify_trusted_publication_envelope(&self.policy, &envelope) {
            Ok(_adopted_key_id) => {}
            Err(issue) if issue.code == INVALID_SIGNATURE => {
                return Admission::Signal(issue.message);
            }
            // Untrusted: no signature by a key this repository has adopted. Adoption is local, so this is not damage.
            Err(_) => return Admission::NotUsable,
        }
        // 3. The snapshot passes the loader.
        let Ok(payload) = BlockPayload::decode_canonical(&envelope.canonical_payload) else {
            return Admission::Signal(format!("Block {block_id} does not decode"));
        };
        match validate_snapshot_manifest(reader, block_id, &payload) {
            Ok(Some(manifest)) => {
                count_use(site);
                Admission::Usable(manifest)
            }
            Ok(None) => Admission::NotUsable,
            Err(err) => Admission::Signal(err.to_string()),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
std::thread_local! {
    static USES: std::cell::RefCell<std::collections::BTreeMap<&'static str, usize>> =
        const { std::cell::RefCell::new(std::collections::BTreeMap::new()) };
}

#[cfg(any(test, feature = "test-support"))]
fn count_use(site: AnchorSite) {
    USES.with(|uses| *uses.borrow_mut().entry(site.name()).or_insert(0) += 1);
}

#[cfg(not(any(test, feature = "test-support")))]
fn count_use(_site: AnchorSite) {}

/// How many times this thread's anchor-trust function admitted an anchor for `site` (`"seal"`, `"merge"`,
/// `"seal-from-accepted"`, `"worktree-write"` or `"snapshot-materialize"`), since the thread started or the last
/// [`reset_anchor_uses_for_test_support`]. The per-call-site controls read it; nothing in a shipped build does.
#[cfg(any(test, feature = "test-support"))]
#[must_use]
pub fn anchor_uses_for_test_support(site: &str) -> usize {
    USES.with(|uses| uses.borrow().get(site).copied().unwrap_or(0))
}

/// Zero this thread's per-site anchor counts.
#[cfg(any(test, feature = "test-support"))]
pub fn reset_anchor_uses_for_test_support() {
    USES.with(|uses| uses.borrow_mut().clear());
}

#[cfg(test)]
mod tests;
