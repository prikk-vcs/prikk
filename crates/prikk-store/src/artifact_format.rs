//! What an exchange artifact's declared repository format means to the repository that receives it
//! (0.57.0 U4).
//!
//! A bundle or an archive declares its **sending repository's** format (U0 ruling Q1: O(1) at export; deciding
//! whether any path is non-ASCII would mean decoding all of history). Two rules follow, and this module is the one
//! place that states them, for both artifacts:
//!
//! 1. **A number this build does not know is refused.** The archive has always done this; the bundle's
//!    `repository_format` was informational until now, so a bundle could declare anything. It is not damage (a different
//!    version of prikk may have written it), so it reads as outside this version's rules: `UnknownArtifactFormat`.
//! 2. **A receiver that cannot hold non-ASCII paths refuses an artifact from a repository that can** (format 8 and
//!    later), before the first write, naming both formats and the command that moves the receiver
//!    (`prikk format upgrade`). It is a precondition -- the caller can fix it -- not damage and not a malformed file.
//!    Format 6 and 7 artifacts are unaffected: nothing else differs between formats 6, 7 and 8 on the wire.
//!
//! Only rule 2 needs a receiving repository, so `verify` (which has none) applies rule 1 alone.

use prikk_error::{PrikkError, Result};

use crate::foundation::layout::RepositoryFormat;

/// Every format a real export can write: the inverse of [`RepositoryFormat::number`], enumerated rather than
/// re-guessed as a numeric range, so a future variant must be added here deliberately.
const KNOWN: [RepositoryFormat; 3] = [
    RepositoryFormat::CurrentV6,
    RepositoryFormat::V7,
    RepositoryFormat::V8,
];

/// Every `repository_format` number a real export can ever write.
pub(crate) fn known_repository_format_numbers() -> [u32; 3] {
    KNOWN.map(RepositoryFormat::number)
}

/// Which artifact is being read, for the sentence only.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ArtifactKind {
    Archive,
    Bundle,
    /// A `sync` exchange artifact (`sync accept`). It declares no repository format, so only the content rule applies.
    Sync,
}

impl ArtifactKind {
    const fn noun(self) -> &'static str {
        match self {
            Self::Archive => "archive",
            Self::Bundle => "bundle",
            Self::Sync => "sync artifact",
        }
    }
}

/// Rule 1: refuse a declared number this build does not know how to read. Not damage (the file's checksum was sound; a
/// different version of prikk may have written it), so [`PrikkError::UnknownArtifactFormat`] and not `MalformedData`: the
/// sentence and the `refusal-v1` id (`outside-version-rule`) agree.
pub(crate) fn require_known_declared_format(kind: ArtifactKind, declared: u32) -> Result<()> {
    if known_repository_format_numbers().contains(&declared) {
        return Ok(());
    }
    Err(PrikkError::UnknownArtifactFormat {
        artifact: kind.noun(),
        declared,
    })
}

/// Rule 2: refuse an artifact that declares a format able to hold non-ASCII paths when `receiver` cannot.
///
/// `declared` must already have passed [`require_known_declared_format`]; an unknown number is not this rule's.
pub(crate) fn require_receiver_can_hold(
    kind: ArtifactKind,
    declared: u32,
    receiver: RepositoryFormat,
) -> Result<()> {
    let first_non_ascii_format = RepositoryFormat::V8;
    if declared < first_non_ascii_format.number() || receiver.admits_non_ascii_paths() {
        return Ok(());
    }
    let have = receiver.number();
    let want = first_non_ascii_format.number();
    let noun = kind.noun();
    Err(PrikkError::RepositoryFormatTooOld {
        artifact: noun,
        declared,
        have,
        message: format!(
            "this {noun} declares repository format {declared}, and this repository is format {have}; a format-{declared} \
             repository may hold non-ASCII paths, which format {have} cannot. Run `prikk format upgrade` to move this \
             repository to format {want} (a prikk older than this one cannot open a format-{want} repository), then \
             import again; nothing was written"
        ),
    })
}

/// Rule 3, **the content half** of "a format-6 or 7 repository refuses non-ASCII" (U4 review, Q2): an artifact that
/// *carries* a non-ASCII path is refused by a receiver that cannot hold one, whatever format the artifact declares -- a
/// declaration of 6 or 7 from the 0.55.0 hole, or no declaration at all (`sync`). The declared-format half (rule 2) catches
/// every artifact from a format-8 repository; this catches the one declaring less than it carries. A named path, the
/// command, the cost, and that nothing was written: the same shape as rule 2, the same `repository-format-too-old` id.
///
/// `operations` are one patch's already-decoded operations. Called by every importer that has a receiving repository
/// (`bundle import`, `bundle preview`, `archive import`, `sync accept`), before any write; the receiver-less `verify`
/// commands do not call it.
pub(crate) fn require_receiver_can_hold_paths(
    kind: ArtifactKind,
    operations: &[crate::patch_replay::decode::DecodedPatchOperation],
    receiver: RepositoryFormat,
) -> Result<()> {
    if receiver.admits_non_ascii_paths() {
        return Ok(());
    }
    let first_non_ascii_format = RepositoryFormat::V8;
    let noun = kind.noun();
    let have = receiver.number();
    let want = first_non_ascii_format.number();
    for operation in operations {
        if let Some(path) = operation
            .kind
            .paths()
            .into_iter()
            .find(|path| !path.is_ascii())
        {
            return Err(PrikkError::RepositoryFormatTooOld {
                artifact: noun,
                declared: want,
                have,
                message: format!(
                    "this {noun} carries the non-ASCII path {path}, and this repository is format {have}, which cannot hold \
                     non-ASCII paths. Run `prikk format upgrade` to move this repository to format {want} (a prikk older \
                     than this one cannot open a format-{want} repository), then import again; nothing was written"
                ),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
