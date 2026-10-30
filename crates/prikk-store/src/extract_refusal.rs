//! `prikk extract`'s own refusal classification (0.54.0 step 3 Part 5d).
//!
//! `prikk-cli`'s own `refusal-v1` document (`crates/prikk-cli/src/extract.rs`) needs to tell apart
//! several conditions that [`prepare_extract_plan`](crate::extract::prepare_extract_plan)/
//! [`prepare_extract_plan_for_claim`](crate::extract::prepare_extract_plan_for_claim) can all raise
//! as the identical [`prikk_error::PrikkError`] variant (most of them `Precondition`) -- the same
//! problem `PrikkError::IncompletePublication` exists apart from `PrikkError::Precondition` to
//! solve, rather than recovering the distinction later by matching the message's own words
//! (`prikk-error/src/lib.rs`'s own doc: "an edit to the text could silently drop the
//! classification"). [`ExtractError`] carries the classification out of this crate's own
//! extract-building functions, computed at the exact point each condition is raised, never
//! recovered afterward from text.
//!
//! **Confined to `extract`.** No other command's refusal is classified this way (yet) -- `extract`
//! is this project's first machine-readable refusal document, designed as a precedent other
//! commands adopt when next touched, not a retrofit onto every existing refusal today.
//!
//! **`#[non_exhaustive]` since 0.55.0.** [`ExtractRefusalKind`] gained a sixth member
//! (`ClaimParentNotSealed`) in its second release, after shipping exhaustive in 0.54.0 --
//! refusal ids keep coming (0.56.0 is already known to add a rollback refusal), so a library
//! caller's own exhaustive `match` must carry a wildcard arm to keep compiling across a release
//! that adds one. `refusal-v1`'s own "exhaustive by construction" design is still true of the
//! wire-level id strings themselves; it is this Rust enum's own compile-time exhaustiveness that
//! cannot be promised release to release.

use prikk_error::PrikkError;

/// Which of `no-checkpoint-in-reach`'s own three reasons applied (0.54.0 step 3 Part 5d): the
/// first that applies, checked in priority order, across the whole admitted-anchor search within
/// reach (`patch_replay::extract::search_admitted_anchor`).
///
/// Deliberately not `#[non_exhaustive]`: new in this same release cycle, with exactly the three
/// reasons the admitted-anchor search's own priority order can produce, and growing it is this
/// crate's own change to make alongside the match in `prikk-cli` that reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoCheckpointReason {
    /// A recorded checkpoint was found within reach, but its signer is not a key this repository
    /// has adopted -- the most specific, actionable reason: the remedy is to adopt the key.
    SignerNotAdopted,
    /// A block carrying a snapshot was found within reach, but none was ever recorded by
    /// `verify`/`seal` (or one was recorded but failed some other admission check) -- the remedy
    /// is to run `verify`.
    NotRecorded,
    /// No block within reach carried a snapshot at all -- the remedy is `--allow-full-replay`.
    NoCheckpoint,
}

/// What kind of refusal one of `extract`'s own plan-building functions raised, carried alongside
/// the ordinary [`PrikkError`] so a caller can classify it without reading its message. Four of
/// `extract`'s eleven documented `refusal-v1` ids are classified here, at the exact point each is
/// raised; the rest are either raised directly by the CLI itself (unambiguous at their own call
/// site -- `over-max-bytes`, `over-max-files`, `destination-exists`, `temporary-sibling-exists`),
/// come from the point resolver unchanged (`point-not-found`), or fall back to a classification of
/// the underlying [`PrikkError`]'s own variant (`damaged-repository`, `io-failure`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ExtractRefusalKind {
    /// `claim-not-found`: no such claim.
    ClaimNotFound,
    /// `not-a-claim`: the id names another object type.
    NotAClaim {
        /// What was expected (`RecognitionClaim`).
        expected: String,
        /// What the stored object's own type actually is.
        actual: String,
    },
    /// `claim-has-several-parents`: a merge claim.
    ClaimHasSeveralParents {
        /// How many parents the claim declared.
        count: usize,
    },
    /// `claim-parent-not-sealed`: the claim's own declared parent Block was never sealed or
    /// received into this repository -- this repository is not damaged, the parent just has not
    /// arrived yet (0.55.0 step 1 item 2, external review 035 §3.3). Boxed: two full `ObjectId`s
    /// (64 bytes) would otherwise make this the largest variant, pushing `ExtractError` (returned
    /// by value on every `Result::Err` this module's functions raise) past
    /// `clippy::result_large_err`'s threshold for what is, in practice, one of this enum's rarest
    /// constructions.
    ClaimParentNotSealed(Box<ClaimParentNotSealedInfo>),
    /// `case-collision`: two of the point's own resolved paths collide under ASCII case folding.
    CaseCollision,
    /// `no-checkpoint-in-reach`: the pre-walk bound, carrying its own `reason` sub-classification.
    NoCheckpointInReach(NoCheckpointReason),
    /// `claim-signature-invalid` (0.56.0 step 3e, D2): an `invalid` MAINTAINER signature at some
    /// link in the chain -- the claim named directly on the command line included. No signature at
    /// all is `invalid` too (`verify_claim_signature`'s own classification).
    ClaimSignatureInvalid(Box<ClaimSignatureInvalidInfo>),
    /// `claim-does-not-fold` (0.56.0 step 3e, D4): some link's own `patch_ids` do not apply onto the
    /// previous link's end state (or, for the root link, onto the chain's own anchor). Replaces
    /// what used to fall through, unclassified, to `damaged-repository` -- an untrusted chain's own
    /// claim not fitting where it says it goes is not this repository being damaged.
    ClaimDoesNotFold(Box<ClaimDoesNotFoldInfo>),
    /// `claim-chain-shared-block` (0.56.0 step 3e, D1): two or more held claims recognize the same
    /// block the walk visits, and either none or more than one of them has a `Sound` signature.
    ClaimChainSharedBlock(Box<ClaimChainSharedBlockInfo>),
    /// `claim-chain-cycle` (0.56.0 step 3e, D3): the walk would revisit a block it has already
    /// visited earlier in the same chain.
    ClaimChainCycle(Box<ClaimChainCycleInfo>),
}

/// [`ExtractRefusalKind::ClaimParentNotSealed`]'s own payload, boxed there for the reason that
/// variant's own doc comment gives.
///
/// Release 0.55.0 prep: `#[non_exhaustive]`, matching its siblings
/// ([`ArchiveUnreferencedRemnant`](crate::ArchiveUnreferencedRemnant),
/// [`UnreferencedRemnant`](crate::UnreferencedRemnant)) -- a future field (for example, more
/// context on why the parent is not sealed) must not break an existing exhaustive match.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaimParentNotSealedInfo {
    /// The claim being extracted.
    pub claim: prikk_object::ObjectId,
    /// The claim's own declared parent, confirmed absent from this repository's object store.
    pub parent: prikk_object::ObjectId,
}

/// [`ExtractRefusalKind::ClaimSignatureInvalid`]'s own payload, boxed for the same reason
/// [`ClaimParentNotSealedInfo`] is.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaimSignatureInvalidInfo {
    /// The link whose own MAINTAINER signature is invalid (or absent).
    pub claim_id: prikk_object::ObjectId,
}

/// [`ExtractRefusalKind::ClaimDoesNotFold`]'s own payload.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaimDoesNotFoldInfo {
    /// The link whose own `patch_ids` did not apply.
    pub claim_id: prikk_object::ObjectId,
}

/// One candidate's own signature outcome, as reported in [`ClaimChainSharedBlockInfo::candidates`]
/// -- a simplified, wire-friendly three-way tag (`extract-report-v1`/`refusal-v1` carry the same
/// three strings `sync-pending-v1` already does: `"sound"`, `"unverifiable"`, `"invalid"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainSignatureOutcomeKind {
    /// The signature verifies against a key this repository has adopted.
    Sound,
    /// The signature names a key this repository has not adopted -- not checked, not a failure.
    Unverifiable,
    /// The signature failed to verify against an adopted key, is not Ed25519, or is absent.
    Invalid,
}

/// One candidate claim named in a [`ExtractRefusalKind::ClaimChainSharedBlock`] refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ChainCandidateSignature {
    /// The candidate's own id.
    pub claim_id: prikk_object::ObjectId,
    /// Its own signature outcome.
    pub outcome: ChainSignatureOutcomeKind,
    /// The MAINTAINER key id named, when there is one to name.
    pub key_id: Option<String>,
}

/// [`ExtractRefusalKind::ClaimChainSharedBlock`]'s own payload (D1): every candidate claim that
/// recognizes `block_id`, with its own signature outcome, so an operator can see exactly why the
/// walk refused rather than picked one.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaimChainSharedBlockInfo {
    /// The block two or more held claims recognize.
    pub block_id: prikk_object::ObjectId,
    /// Every candidate claim recognizing it, sorted by claim id.
    pub candidates: Vec<ChainCandidateSignature>,
}

/// [`ExtractRefusalKind::ClaimChainCycle`]'s own payload (D3): every claim id the walk found in the
/// loop, oldest-in-the-chain first.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ClaimChainCycleInfo {
    /// Every claim id the walk found forming the loop.
    pub claim_ids: Vec<prikk_object::ObjectId>,
}

/// [`prepare_extract_plan`](crate::extract::prepare_extract_plan)/
/// [`prepare_extract_plan_for_claim`](crate::extract::prepare_extract_plan_for_claim)'s own error:
/// the ordinary [`PrikkError`] (`Display`'s text is exactly what stderr already printed before this
/// round -- this type changes nothing about it), paired with a classification when the raise site
/// already knows one. `kind: None` means "not specially classified here" -- the CLI's own fallback
/// then classifies by `error`'s own `PrikkError` variant (`Io` -> `io-failure`, everything else
/// that names a persisted-data problem -> `damaged-repository`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExtractError {
    /// The refusal kind, when the raise site already knows it.
    pub kind: Option<ExtractRefusalKind>,
    /// The ordinary error.
    pub error: PrikkError,
}

impl ExtractError {
    /// Build a classified error: `error`'s own `Display` text is unchanged, `kind` is attached
    /// alongside it.
    pub(crate) fn classified(kind: ExtractRefusalKind, error: PrikkError) -> Self {
        Self {
            kind: Some(kind),
            error,
        }
    }
}

impl std::fmt::Display for ExtractError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Display::fmt(&self.error, f)
    }
}

impl std::error::Error for ExtractError {}

/// An error not otherwise classified here propagates with `kind: None` -- the ordinary case for
/// every call this module's own functions make into the rest of this crate (`read_block`,
/// `read_patch`, `decode_patch_operations`, ...), via `?`.
impl From<PrikkError> for ExtractError {
    fn from(error: PrikkError) -> Self {
        Self { kind: None, error }
    }
}

/// The inverse: a caller that only wants the ordinary error (a test's own `.to_string()`, or
/// `?` into a function that still returns `prikk_error::Result`) gets exactly what it always did.
impl From<ExtractError> for PrikkError {
    fn from(value: ExtractError) -> Self {
        value.error
    }
}

#[cfg(test)]
mod tests;
