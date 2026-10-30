//! `prikk extract` (0.54.0 step 3 Part 2, design review v2 ruling 2): the files at a point, written
//! into a new, history-free directory that does not exist yet -- never checked against this
//! repository's own tree. An upper-layer module (RFC 149, the same role
//! `point_reading.rs` plays for `cat`/`tree`): built on the point resolver and
//! [`crate::patch_replay::extract`]'s own streamed replay.
//!
//! **The one anchoring rule this command uses, stated once more for the reader of this file**:
//! `extract` anchors only where a writer would (`AnchorTrust::admit`, under its own
//! [`crate::anchor_trust::AnchorSite::Extract`]) -- never at any snapshot that merely "passes the
//! loader" the way `cat`/`tree`'s own `Anchoring::ReadOnlyReport` does. Past the admitted-anchor
//! distance bound, with no admitted anchor in reach, this refuses by name before reading a single
//! patch unless `allow_full_replay` is set.
//!
//! **Streaming, for the caller**: [`ExtractPlan`] never holds a streamed file's bytes -- only its
//! Blob id ([`ExtractPlanContent::Blob`]) -- so building the plan costs no more than this module's
//! own replay does (module doc, `patch_replay/extract.rs`). A caller writes each file one at a time,
//! calling [`read_extract_blob_content`] only when it is actually about to write that one file, over
//! the **same** `reader` the plan itself was built from, so both phases read one consistent view of
//! the store.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectId, ObjectType, RecognitionClaimPayload};

use crate::anchor_trust::AnchorTrust;
use crate::claim_chain::{ClaimChain, ClaimChainAnchor, ClaimChainWalkError, walk_claim_chain};
use crate::extract_refusal::{
    ChainCandidateSignature, ChainSignatureOutcomeKind, ClaimChainCycleInfo,
    ClaimChainSharedBlockInfo, ClaimDoesNotFoldInfo, ClaimParentNotSealedInfo,
    ClaimSignatureInvalidInfo, ExtractError, ExtractRefusalKind,
};
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::patch_replay::extract::{
    ExtractChainFoldError, ExtractContent, replay_for_extract, replay_for_extract_chain,
};
use crate::patch_replay::read::single_parent_chain;
use crate::path::{RepoPath, validate_no_path_collisions};
use crate::point::Point;
use crate::recognition_claim::{ClaimChainSignatureOutcome, classify_claim_signature};
use crate::trust::load_maintainer_trust_policy_or_empty;

/// Where one planned file's content comes from.
///
/// Deliberately not `#[non_exhaustive]`: new in this same release cycle, with exactly the two
/// shapes a planned file's content can take (streamed from a stored Blob, or already resident),
/// and growing it is this crate's own change to make alongside the match in `prikk-cli` that
/// writes each entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtractPlanContent {
    /// Stream straight from this Blob id, read fresh with [`read_extract_blob_content`] only when
    /// the caller is about to write this one file.
    Blob(ObjectId),
    /// Already resident (a text file edited since the admitted anchor, or since genesis).
    Bytes(Vec<u8>),
}

/// One file `extract` will write, at its final (possibly renamed) path.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExtractPlanEntry {
    /// Validated repository-relative path, already checked for collisions against every other
    /// entry in the same plan (case-insensitively).
    pub path: RepoPath,
    /// Mode bits.
    pub mode: u32,
    /// The content itself.
    pub content: ExtractPlanContent,
}

/// What `extract` will write at a point.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExtractPlan {
    /// The point exactly as named: a ref name, a block id, or (0.54.0 step 3 Part 3) a claim id.
    pub point: String,
    /// The Block read.
    pub target_block_id: ObjectId,
    /// Whether a real admitted anchor was used (`false`) or the replay fell back to a full replay
    /// from genesis (`true`) -- worded for the CLI exactly as `checkout --snapshot-materialize`
    /// words its own `provisional:` line.
    pub provisional: bool,
    /// (0.54.0 step 3 Part 5d) `claim`'s own declared parent -- `None` for a root claim or a
    /// ref/block-id point (`prepare_extract_plan` always sets this to `None`), `Some(parent)` for a
    /// claim with exactly one declared parent (`prepare_extract_plan_for_claim`'s own already-computed
    /// `parent` local). **Not the same thing as [`Self::target_block_id`]**: for a claim,
    /// `target_block_id` is the block the claim *recognizes* (`claim.block_id`); this is the block the
    /// claim's own patches would be sealed *onto* -- no command printed this before this round.
    pub claim_parent_block_id: Option<ObjectId>,
    /// (0.56.0 step 3e, D1-D4) The whole walked chain, root (oldest) to foot (the named claim) --
    /// empty for a ref/block-id point, which names no claim at all. **`claim_parent_block_id`'s own
    /// meaning does not change**: it stays the foot claim's own immediate declared parent; this
    /// field is the additive information for the rest of the chain. Every link here always has a
    /// `Sound` or `Unverifiable` signature outcome -- an `Invalid` link anywhere refuses the whole
    /// command before a plan is ever built (D2), so one never appears here.
    pub chain: Vec<ExtractChainLink>,
    /// The weakest signature outcome across the whole chain -- `None` for a ref/block-id point
    /// (`chain` is empty), otherwise `Some` and never [`ChainSignatureOutcomeKind::Invalid`], for
    /// the same reason [`Self::chain`]'s own doc gives.
    pub chain_signature: Option<ChainSignatureOutcomeKind>,
    /// Files to write, in canonical path order, with no two colliding under ASCII case folding.
    pub entries: Vec<ExtractPlanEntry>,
}

/// One link of [`ExtractPlan::chain`] (0.56.0 step 3e): the identical field names
/// `sync-pending-v1`'s own `claims[]` entries already use (`PendingPatchClaim`), reused verbatim
/// rather than invented a second time for the same two concepts.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ExtractChainLink {
    /// The claim's own content-addressed id.
    pub claim_id: ObjectId,
    /// The block this claim recognizes (`RecognitionClaimPayload::block_id`).
    pub target_block_id: ObjectId,
    /// The block(s) this claim declares as its own parents -- `[]` for a root claim, one entry
    /// ordinarily (more than one already refused the whole command before a link is ever pushed
    /// here, D4).
    pub parent_block_ids: Vec<ObjectId>,
    /// This link's own MAINTAINER signature outcome.
    pub signature_outcome: ChainSignatureOutcomeKind,
    /// The MAINTAINER key id named, when there is one to name.
    pub signature_key_id: Option<String>,
    /// D1's own exception, surfaced: non-empty only when this link was chosen because it was the
    /// one `Sound` candidate among two or more held claims recognizing [`Self::target_block_id`] --
    /// every other candidate, named with its own outcome, for visibility.
    pub other_candidates: Vec<ChainCandidateSignature>,
}

/// **Build the plan for `point`** (RFC 157-style, extended to extract's own admitted-anchor rule):
/// the pre-walk and the streamed replay (`patch_replay::extract::replay_for_extract`), then
/// the same case-collision check every write safety checklist in this project reuses
/// ([`validate_no_path_collisions`]), over the point's own resolved paths, before the caller writes a
/// single file.
///
/// `reader` is a parameter, not opened here, so a caller can read each file's content afterward
/// (streaming, one at a time) over the identical read-only view this plan was built from
/// ([`read_extract_blob_content`]).
///
/// # Errors
///
/// `Precondition` (`no-checkpoint-in-reach`) when no admitted anchor exists within the trusted
/// distance bound and `allow_full_replay` is `false` (naming the distance), or `InvalidName`
/// (`case-collision`) when two of the point's own paths collide under ASCII case folding;
/// otherwise, the replay's own failures (an unsupported operation, a missing object, or an
/// integrity mismatch), carried with no classification (`ExtractError::kind` is `None`).
pub fn prepare_extract_plan(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    point: &Point,
    allow_full_replay: bool,
) -> std::result::Result<ExtractPlan, ExtractError> {
    let block_ids = single_parent_chain(reader, point.block_id)?;
    let trust = AnchorTrust::load(layout);
    let outcome = replay_for_extract(reader, &block_ids, &trust, allow_full_replay)?;

    let paths = outcome
        .files
        .iter()
        .map(|file| RepoPath::parse(&file.path))
        .collect::<Result<Vec<_>>>()?;
    validate_no_path_collisions(&paths)
        .map_err(|err| ExtractError::classified(ExtractRefusalKind::CaseCollision, err))?;

    let entries = outcome
        .files
        .into_iter()
        .zip(paths)
        .map(|(file, path)| ExtractPlanEntry {
            path,
            mode: file.mode,
            content: match file.content {
                ExtractContent::Blob(blob_id) => ExtractPlanContent::Blob(blob_id),
                ExtractContent::Resident(bytes) => ExtractPlanContent::Bytes(bytes),
            },
        })
        .collect();

    Ok(ExtractPlan {
        point: point.name.clone(),
        target_block_id: point.block_id,
        provisional: outcome.provisional,
        claim_parent_block_id: None,
        chain: Vec::new(),
        chain_signature: None,
        entries,
    })
}

/// **`prikk extract --claim <claim-id>`** (0.54.0 step 3 Part 3; 0.56.0 step 3e, D1-D4: now a full
/// chain walk, not a single hop): the same [`ExtractPlan`] shape, built from a walked chain of
/// `RecognitionClaim`s instead of a resolved [`Point`]'s block chain -- a preview of what the named
/// claim's own patches would produce if sealed, walking back through every unsealed ancestor claim
/// the ruling's own D1-D4 govern, never sealing anything itself.
///
/// **The walk** (`crate::claim_chain::walk_claim_chain`) answers every structural question: which
/// claim, if any, recognizes a given unsealed parent block (D4); what to do when more than one held
/// claim recognizes the same block (D1); a cycle (D3); a merge-shaped claim at any link, not only
/// the one named here (D4, extending the single-hop refusal this function always had). This
/// function's own job is the two things that remain caller policy for `extract` specifically (D2,
/// D3's bound, D4's refuse-before-writing): refuse on any `Invalid` signature anywhere in the chain
/// (unlike `show`, from 0.56.0 step 3f), check the length bound, fold every link in order
/// (`replay_for_extract_chain`), and report the whole chain additively.
///
/// # Errors
///
/// `claim_id` does not name a `RecognitionClaim`, or it does not decode; a merge-shaped claim
/// anywhere in the chain; two or more held claims recognize the same block the walk visits with no
/// single trusted candidate (`claim-chain-shared-block`); a cycle (`claim-chain-cycle`); a parent
/// that is neither reachable nor recognized by any held claim (`claim-parent-not-sealed`); an
/// `Invalid` MAINTAINER signature anywhere in the chain (`claim-signature-invalid`); a link's own
/// patches do not fold onto the state before it (`claim-does-not-fold`); two of the claimed state's
/// own paths collide under ASCII case folding; or the chain's own total length (the real anchor's
/// own distance plus the chain's hops) reaches past the trusted distance bound with no admitted
/// checkpoint in reach and `allow_full_replay` is `false`.
pub fn prepare_extract_plan_for_claim(
    layout: &RepositoryLayout,
    reader: &impl ObjectReader,
    claim_id: ObjectId,
    allow_full_replay: bool,
) -> std::result::Result<ExtractPlan, ExtractError> {
    // 0.54.0 step 3 Part 5d, item 4: this used to be `PrikkError::Integrity` (an unknown claim id
    // is a caller precondition, not stored damage) and let `PrikkError::ObjectTypeMismatch` bubble
    // straight through from `read_typed`'s own shared default (`object_store.rs`'s own
    // `read_typed`, untouched by this round -- every other caller still gets the generic mismatch).
    // Both are now `Precondition`, classified here, at the one call site that can tell them apart.
    let envelope = match reader.read_typed(claim_id, ObjectType::RecognitionClaim) {
        Ok(Some(envelope)) => envelope,
        Ok(None) => {
            return Err(ExtractError::classified(
                ExtractRefusalKind::ClaimNotFound,
                PrikkError::Precondition(format!("recognition claim {claim_id} does not exist")),
            ));
        }
        Err(PrikkError::ObjectTypeMismatch { expected, actual }) => {
            return Err(ExtractError::classified(
                ExtractRefusalKind::NotAClaim {
                    expected: expected.clone(),
                    actual: actual.clone(),
                },
                PrikkError::Precondition(format!(
                    "{claim_id} is not a recognition claim (expected {expected}, got {actual}) -- \
                     extract --claim takes a RecognitionClaim id"
                )),
            ));
        }
        Err(err) => return Err(err.into()),
    };
    let claim = RecognitionClaimPayload::decode_canonical(&envelope.canonical_payload)?;

    let trust_policy = load_maintainer_trust_policy_or_empty(layout)?;
    let start_signature = classify_claim_signature(&envelope, &trust_policy);

    let chain = match walk_claim_chain(layout, reader, claim_id, claim, start_signature)? {
        Ok(chain) => chain,
        Err(walk_error) => return Err(classify_claim_chain_walk_error(walk_error)),
    };

    // D2: an `Invalid` link anywhere refuses the whole chain, the claim named on the command line
    // included -- unlike `show` (0.56.0 step 3f), which skips one instead.
    if let Some(invalid) = chain
        .links
        .iter()
        .find(|link| matches!(link.signature, ClaimChainSignatureOutcome::Invalid { .. }))
    {
        let invalid_claim_id = invalid.claim_id;
        return Err(ExtractError::classified(
            ExtractRefusalKind::ClaimSignatureInvalid(Box::new(ClaimSignatureInvalidInfo {
                claim_id: invalid_claim_id,
            })),
            PrikkError::Precondition(format!(
                "recognition claim {invalid_claim_id} does not carry a usable MAINTAINER \
                 signature -- a forged or unsigned claim is refused, not extracted"
            )),
        ));
    }

    let anchor = match chain.anchor {
        ClaimChainAnchor::Genesis => None,
        ClaimChainAnchor::Block(block_id) => Some(block_id),
    };
    let hop_count = chain.links.len() - 1;
    let link_patch_ids: Vec<Vec<ObjectId>> = chain
        .links
        .iter()
        .map(|link| link.patch_ids.clone())
        .collect();

    let outcome = replay_for_extract_chain(
        layout,
        reader,
        anchor,
        hop_count,
        &link_patch_ids,
        allow_full_replay,
    )
    .map_err(|err| classify_extract_chain_fold_error(err, &chain))?;

    let paths = outcome
        .files
        .iter()
        .map(|file| RepoPath::parse(&file.path))
        .collect::<Result<Vec<_>>>()?;
    validate_no_path_collisions(&paths)
        .map_err(|err| ExtractError::classified(ExtractRefusalKind::CaseCollision, err))?;

    let entries = outcome
        .files
        .into_iter()
        .zip(paths)
        .map(|(file, path)| ExtractPlanEntry {
            path,
            mode: file.mode,
            content: match file.content {
                ExtractContent::Blob(blob_id) => ExtractPlanContent::Blob(blob_id),
                ExtractContent::Resident(bytes) => ExtractPlanContent::Bytes(bytes),
            },
        })
        .collect();

    // Safe: `walk_claim_chain` always returns at least one link (the starting claim itself).
    let foot = chain
        .links
        .last()
        .ok_or_else(|| PrikkError::Integrity("claim chain walk produced no links".to_string()))?;
    let claim_parent_block_id = foot.parent_block_ids.first().copied();
    let target_block_id = foot.target_block_id;

    let chain_report: Vec<ExtractChainLink> = chain
        .links
        .iter()
        .map(|link| {
            let (signature_outcome, signature_key_id) = chain_signature_report(&link.signature);
            let other_candidates = link
                .other_candidates
                .iter()
                .map(|(id, outcome)| {
                    let (outcome_kind, key_id) = chain_signature_report(outcome);
                    ChainCandidateSignature {
                        claim_id: *id,
                        outcome: outcome_kind,
                        key_id,
                    }
                })
                .collect();
            ExtractChainLink {
                claim_id: link.claim_id,
                target_block_id: link.target_block_id,
                parent_block_ids: link.parent_block_ids.clone(),
                signature_outcome,
                signature_key_id,
                other_candidates,
            }
        })
        .collect();
    let chain_signature = Some(weakest_chain_signature(&chain_report));

    Ok(ExtractPlan {
        point: claim_id.to_string(),
        target_block_id,
        provisional: outcome.provisional,
        claim_parent_block_id,
        chain: chain_report,
        chain_signature,
        entries,
    })
}

/// A single claim's own signature outcome, as [`ExtractChainLink`] reports it -- `Invalid` never
/// reaches this conversion (it already refused the whole command above).
fn chain_signature_report(
    outcome: &ClaimChainSignatureOutcome,
) -> (ChainSignatureOutcomeKind, Option<String>) {
    match outcome {
        ClaimChainSignatureOutcome::Sound { key_id } => {
            (ChainSignatureOutcomeKind::Sound, Some(key_id.clone()))
        }
        ClaimChainSignatureOutcome::Unverifiable { key_id } => (
            ChainSignatureOutcomeKind::Unverifiable,
            Some(key_id.clone()),
        ),
        ClaimChainSignatureOutcome::Invalid { key_id, .. } => {
            (ChainSignatureOutcomeKind::Invalid, key_id.clone())
        }
    }
}

/// D2's own `chain_signature`: the weakest outcome across the whole reported chain. `Invalid` never
/// appears in `links` by the time this is called (see [`ExtractPlan::chain`]'s own doc), so this
/// only ever chooses between `Sound` and `Unverifiable`; `Sound` is the fallback for an (unreachable
/// in practice) empty chain.
fn weakest_chain_signature(links: &[ExtractChainLink]) -> ChainSignatureOutcomeKind {
    if links
        .iter()
        .any(|link| link.signature_outcome == ChainSignatureOutcomeKind::Unverifiable)
    {
        ChainSignatureOutcomeKind::Unverifiable
    } else {
        ChainSignatureOutcomeKind::Sound
    }
}

/// One word for a signature outcome, for a refusal's own human-readable text.
fn signature_outcome_word(outcome: &ClaimChainSignatureOutcome) -> &'static str {
    match outcome {
        ClaimChainSignatureOutcome::Sound { .. } => "sound",
        ClaimChainSignatureOutcome::Unverifiable { .. } => "unverifiable",
        ClaimChainSignatureOutcome::Invalid { .. } => "invalid",
    }
}

/// [`ClaimChainWalkError`] -> [`ExtractError`] (D1, D3, D4's merge/parent cases): the walk's own
/// structural failures, classified for `extract`'s own `refusal-v1` document.
fn classify_claim_chain_walk_error(error: ClaimChainWalkError) -> ExtractError {
    match error {
        ClaimChainWalkError::SeveralParents { claim_id, count } => ExtractError::classified(
            ExtractRefusalKind::ClaimHasSeveralParents { count },
            PrikkError::Precondition(format!(
                "recognition claim {claim_id} declares {count} parents; a merge claim's own \
                 mainline parent cannot be named from the claim object alone, so this is refused \
                 rather than guessed -- extract --claim does not yet support a claim about a \
                 merge block"
            )),
        ),
        ClaimChainWalkError::ParentNotSealed { claim_id, parent } => ExtractError::classified(
            ExtractRefusalKind::ClaimParentNotSealed(Box::new(ClaimParentNotSealedInfo {
                claim: claim_id,
                parent,
            })),
            PrikkError::Precondition(format!(
                "recognition claim {claim_id} declares parent {parent}, which is not in this \
                 repository yet -- seal or receive the parent before extracting this claim"
            )),
        ),
        ClaimChainWalkError::SharedBlock {
            block_id,
            candidates,
        } => {
            let described: Vec<String> = candidates
                .iter()
                .map(|(id, outcome)| format!("{id} ({})", signature_outcome_word(outcome)))
                .collect();
            let reported: Vec<ChainCandidateSignature> = candidates
                .iter()
                .map(|(id, outcome)| {
                    let (outcome_kind, key_id) = chain_signature_report(outcome);
                    ChainCandidateSignature {
                        claim_id: *id,
                        outcome: outcome_kind,
                        key_id,
                    }
                })
                .collect();
            ExtractError::classified(
                ExtractRefusalKind::ClaimChainSharedBlock(Box::new(ClaimChainSharedBlockInfo {
                    block_id,
                    candidates: reported,
                })),
                PrikkError::Precondition(format!(
                    "block {block_id} is recognized by more than one held recognition claim, \
                     with no single trusted candidate to choose: {} -- refusing rather than \
                     guessing which one is true",
                    described.join(", ")
                )),
            )
        }
        ClaimChainWalkError::Cycle { claim_ids } => {
            let described: Vec<String> = claim_ids.iter().map(ObjectId::to_string).collect();
            ExtractError::classified(
                ExtractRefusalKind::ClaimChainCycle(Box::new(ClaimChainCycleInfo { claim_ids })),
                PrikkError::Precondition(format!(
                    "this claim chain loops back on itself: {}",
                    described.join(" -> ")
                )),
            )
        }
    }
}

/// [`ExtractChainFoldError`] -> [`ExtractError`] (D3's bound, D4's fold failure): `chain` is this
/// function's own just-walked chain, so `Fold`'s own `link_index` can be named by claim id.
fn classify_extract_chain_fold_error(
    error: ExtractChainFoldError,
    chain: &ClaimChain,
) -> ExtractError {
    match error {
        ExtractChainFoldError::Bound(err) | ExtractChainFoldError::Collision(err) => err,
        ExtractChainFoldError::Lineage(err) | ExtractChainFoldError::Other(err) => err.into(),
        ExtractChainFoldError::Fold { link_index, err } => {
            let named_claim_id = chain
                .links
                .get(link_index)
                .or_else(|| chain.links.last())
                .map(|link| link.claim_id);
            let Some(claim_id) = named_claim_id else {
                return PrikkError::Integrity(
                    "claim chain fold failure named a link index out of range -- internal \
                     inconsistency"
                        .to_string(),
                )
                .into();
            };
            ExtractError::classified(
                ExtractRefusalKind::ClaimDoesNotFold(Box::new(ClaimDoesNotFoldInfo { claim_id })),
                PrikkError::Integrity(format!(
                    "recognition claim {claim_id} refused: its own patches do not apply cleanly \
                     onto the state before it ({err})"
                )),
            )
        }
    }
}

/// Read one Blob's content, fresh, for `extract`'s own per-file streaming write
/// ([`ExtractPlanContent::Blob`]). A thin, crate-boundary wrapper: the decoding itself is the same
/// one every patch-replay reader already uses.
///
/// # Errors
///
/// The Blob is missing, is not a file-content Blob, or does not decode.
pub fn read_extract_blob_content(reader: &impl ObjectReader, blob_id: ObjectId) -> Result<Vec<u8>> {
    Ok(crate::patch_replay::read::read_blob_bytes_with_kind(reader, blob_id)?.1)
}

/// Rename `from` onto `to`, refusing with the platform's own no-replace default if `to` already
/// exists, rather than silently replacing it (`prikk extract`'s own write-safety ruling, 0.54.0 step
/// 3 Part 2, design round 2 R-A4). `to`'s parent directory must already exist.
///
/// This is the one all-or-nothing boundary between `extract`'s own temporary sibling directory and
/// its real target: every file has already been written into the sibling by the time this is
/// called, so this single rename either makes the whole write visible atomically or fails leaving
/// neither the sibling renamed nor the target touched.
///
/// Placed here, in `prikk-store`, rather than in `prikk-cli` alongside `durable_output.rs`'s own
/// single-file write: this crate already carries both platform dependencies this needs (`rustix` for
/// Linux/macOS, `prikk-ffi` for Windows), and adding either as a fresh dependency of `prikk-cli`
/// would duplicate what is already here for no reason.
///
/// # Errors
///
/// `to` already exists, or the rename otherwise fails.
pub fn rename_new_directory_no_replace(
    from: &std::path::Path,
    to: &std::path::Path,
) -> std::io::Result<()> {
    rename_no_replace_platform(from, to)
}

/// `renameat2(RENAME_NOREPLACE)` on Linux, `renamex_np(RENAME_EXCL)` on macOS -- one call site covers
/// both (design round 2, R-A4): `rustix::fs::renameat_with` is `cfg(any(apple, linux_kernel,
/// target_os = "redox"))`, and `RenameFlags::NOREPLACE` maps to the right flag on each. `CWD` as both
/// directory arguments: `from`/`to` are always absolute (the caller's own target/sibling paths), and
/// an absolute path argument ignores its paired directory fd under POSIX `*at` semantics either way.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn rename_no_replace_platform(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    rustix::fs::renameat_with(
        rustix::fs::CWD,
        from,
        rustix::fs::CWD,
        to,
        rustix::fs::RenameFlags::NOREPLACE,
    )
    .map_err(std::io::Error::from)
}

/// `MoveFileExW` without `MOVEFILE_REPLACE_EXISTING` -- the platform's own no-replace default
/// (design round 2 ruling 4), pinned by `prikk-ffi`'s own Windows test rather than assumed from
/// documentation alone.
#[cfg(windows)]
fn rename_no_replace_platform(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    prikk_ffi::rename_no_replace(from, to)
}

/// No mechanism is wired for any other target. Refused by name rather than silently falling back to
/// a replacing rename, which would defeat the whole write-safety rule this function exists to
/// enforce.
#[cfg(not(any(target_os = "linux", target_os = "macos", windows)))]
fn rename_no_replace_platform(
    _from: &std::path::Path,
    _to: &std::path::Path,
) -> std::io::Result<()> {
    Err(std::io::Error::other(
        "no-replace rename has no implementation on this platform",
    ))
}

#[cfg(test)]
mod tests;
