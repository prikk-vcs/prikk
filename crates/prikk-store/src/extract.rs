//! `prikk extract` (0.54.0 step 3 Part 2, design review v2 ruling 2): the files at a point, written
//! into a brand-new, external, history-free directory. An upper-layer module (RFC 149, the same role
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
use crate::extract_refusal::{ExtractError, ExtractRefusalKind};
use crate::foundation::layout::RepositoryLayout;
use crate::object_store::ObjectReader;
use crate::patch_replay::extract::{ExtractContent, replay_for_extract, replay_for_extract_claim};
use crate::patch_replay::read::single_parent_chain;
use crate::path::{RepoPath, validate_no_path_collisions};
use crate::point::Point;

/// Where one planned file's content comes from.
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
    /// Files to write, in canonical path order, with no two colliding under ASCII case folding.
    pub entries: Vec<ExtractPlanEntry>,
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
        entries,
    })
}

/// **`prikk extract --claim <claim-id>`** (0.54.0 step 3 Part 3): the same [`ExtractPlan`] shape, built
/// from a `RecognitionClaim`'s own declared parent and patches instead of a resolved [`Point`]'s block
/// chain -- a preview of what the claim would produce if its own patches were sealed onto its own
/// declared parent, never sealing anything itself.
///
/// **The parent-resolution question, answered plainly rather than guessed**: a `RecognitionClaimPayload`
/// (`prikk_object::payload::recognition_claim`) carries `parent_block_ids` verbatim from the real Block
/// it describes -- zero entries for a root, one for an ordinary block, or **two** for a claim built from
/// an ancestor that happens to be a `Merge` block (confirmed: `sync_negotiation::sender` builds a
/// claim's own `parent_block_ids` directly from `block.parent_block_ids.clone()`). The claim itself
/// carries no `BlockKind` or `mainline_parent_id` field to say which of two parents is mainline, and the
/// one real production consumer of a claim's own lineage concept (`seal_from_accepted_claim`) does not
/// use `parent_block_ids` for this at all -- it anchors on *this repository's own current ref tip*
/// instead, which is the wrong parent for a read-only preview of the claim's own declared lineage
/// (this function's own job). Without a trustworthy way to name a merge claim's own mainline parent from
/// the claim object alone, **a claim with more than one declared parent is refused by name** rather than
/// guessed at (sorted-first, declared-first, or any other silent pick) -- this is the honest answer the
/// architect asked for, not a placeholder for one.
///
/// # Errors
///
/// `claim_id` does not name a `RecognitionClaim`, or it does not decode; it declares more than one
/// parent; the parent's own lineage does not resolve, or the claim's own patches do not apply cleanly to
/// it; an entry names a symlink; two of the claimed state's own paths collide under ASCII case
/// folding (`validate_no_path_collisions`, exactly as the ref/block-id path already enforces it -- named
/// here because an unsealed claim's own patches have never passed through `seal`'s own collision check,
/// unlike most sealed history this command otherwise reads); or (0.54.0 step 3 Part 3b) the claimed
/// parent's own chain reaches back past the trusted distance bound with no admitted checkpoint in
/// reach and `allow_full_replay` is `false` -- the identical bound the ref/block-id path already
/// enforces, naming the distance.
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

    let parent = match claim.parent_block_ids.as_slice() {
        [] => None,
        [single] => Some(*single),
        many => {
            return Err(ExtractError::classified(
                ExtractRefusalKind::ClaimHasSeveralParents { count: many.len() },
                PrikkError::Precondition(format!(
                    "recognition claim {claim_id} declares {} parents; a merge claim's own \
                     mainline parent cannot be named from the claim object alone, so this is \
                     refused rather than guessed -- extract --claim does not yet support a claim \
                     about a merge block",
                    many.len()
                )),
            ));
        }
    };

    let outcome =
        replay_for_extract_claim(layout, reader, parent, &claim.patch_ids, allow_full_replay)?;

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
        point: claim_id.to_string(),
        target_block_id: claim.block_id,
        provisional: outcome.provisional,
        claim_parent_block_id: parent,
        entries,
    })
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
