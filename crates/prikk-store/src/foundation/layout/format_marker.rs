//! Reading the repository's `.prikk/FORMAT` marker (split out of `layout.rs` when format 8 pushed it past the file-size limit).
//!
//! One function turns the marker's bytes into a [`RepositoryFormat`], or into the refusal that names what
//! the marker held: a format withdrawn long ago, one this binary does not know, or text that is not a
//! number. It is the one place the set of formats a binary reads is written down.

use std::path::Path;

use prikk_error::{PrikkError, Result};

use super::{CURRENT_FORMAT_VERSION, FORMAT_6_VERSION, FORMAT_7_VERSION, RepositoryFormat};
use crate::foundation::fsutil::{MutationRoot, read_file_required};

const LEGACY_FORMAT_VERSION: &[u8] = b"1\n";
const LEGACY_FORMAT_2_VERSION: &[u8] = b"2\n";
const LEGACY_FORMAT_3_VERSION: &[u8] = b"3\n";
const LEGACY_FORMAT_4_VERSION: &[u8] = b"4\n";
const LEGACY_FORMAT_5_VERSION: &[u8] = b"5\n";

pub(super) fn read_repository_format(root: &MutationRoot) -> Result<RepositoryFormat> {
    let version = read_file_required(root, Path::new("FORMAT"))?;
    match version.as_slice() {
        // RFC 114 §5.3 (owner decision): formats 1-5 are not supported. A real released version can
        // still read this one -- 0.19.0 and earlier, confirmed against the release record -- so the
        // message must not claim otherwise; it states withdrawal, not absence.
        LEGACY_FORMAT_VERSION => Err(PrikkError::Integrity(
            "this repository uses format 1, which prikk no longer supports (this version \
             requires format 6, 7 or 8). format-1 support was removed after 0.19.0; migration from \
             format 1 is not supported."
                .to_string(),
        )),
        // RFC 102 Stage 3, design-v1.md §12.1 (owner decision): bump to format 3, reject format-2 at
        // open, no dual-layout bridge. Format 2 was current through the 0.19.0 release (the same
        // release format-1's own rejection message above already points at), so it is the last
        // release able to open a format-2 repository -- established from the release record
        // (`CHANGELOG.md`, `git tag`), not guessed.
        // RFC 114 §5.3: same as format 1's arm -- 0.19.0 genuinely reads format 2 (confirmed against
        // the release record), so this states withdrawal, not absence of a reader.
        LEGACY_FORMAT_2_VERSION => Err(PrikkError::Integrity(
            "this repository uses format 2, which prikk no longer supports (this version \
             requires format 6, 7 or 8). format-2 support was removed after 0.19.0; migration from \
             format 2 is not supported."
                .to_string(),
        )),
        // RFC 102 Stage 4: bump to format 4, reject format-3 at open, no dual-layout bridge --
        // applying Stage 3's own already-ruled policy (design-v1.md §12.1), not a fresh decision (see
        // `RepositoryFormat`'s own doc comment). Unlike format 2, format 3 was never itself the
        // subject of a tagged release (Stage 3 was still pending three-platform CI when Stage 4
        // began) -- no specific "removed after X.Y.Z" version is named here, since none can be
        // verified from the release record yet; naming one would be guessing, which this project's
        // own discipline for these messages does not do.
        // RFC 114 §5.3: unlike formats 1-2, no released version ever shipped format 3 -- the message
        // says so plainly rather than offering a step the product cannot honour.
        LEGACY_FORMAT_3_VERSION => Err(PrikkError::Integrity(
            "this repository uses format 3, which prikk no longer supports (this version \
             requires format 6, 7 or 8). format 3 was never used in a released prikk version; there is \
             no supported migration path."
                .to_string(),
        )),
        // RFC 102 Stage 5, design-v1.md §14.7 (owner decision): bump to format 5, reject format-4 at
        // open, no dual-layout bridge -- format 3's precedent, not format 2's. No release was ever
        // tagged at format 4 either (Stage 4 merged and Stage 5 began before any tag), verified against
        // the release record (`CHANGELOG.md`, `git tag`) rather than assumed from format 3's own
        // no-version precedent -- naming one anyway would be guessing, which this project's own
        // discipline for these messages does not do.
        // RFC 114 §5.3: same as format 3's arm -- format 4 never shipped in a released version either.
        LEGACY_FORMAT_4_VERSION => Err(PrikkError::Integrity(
            "this repository uses format 4, which prikk no longer supports (this version \
             requires format 6, 7 or 8). format 4 was never used in a released prikk version; there is \
             no supported migration path."
                .to_string(),
        )),
        // RFC 102 Stage 6, design-v1.md §15.6 (owner decision): bump to format 6, reject format-5 at
        // open, no dual-layout bridge -- the same precedent again. No release was ever tagged at
        // format 5 either (still 0.19.0, format 2, verified against `git tag` fresh rather than
        // assumed from the format-4 arm's own finding), so no version is named here for the same
        // reason.
        // RFC 114 §5.3: same as formats 3-4's arms -- format 5 never shipped in a released version.
        LEGACY_FORMAT_5_VERSION => Err(PrikkError::Integrity(
            "this repository uses format 5, which prikk no longer supports (this version \
             requires format 6, 7 or 8). format 5 was never used in a released prikk version; there is \
             no supported migration path."
                .to_string(),
        )),
        FORMAT_6_VERSION => Ok(RepositoryFormat::CurrentV6),
        FORMAT_7_VERSION => Ok(RepositoryFormat::V7),
        CURRENT_FORMAT_VERSION => Ok(RepositoryFormat::V8),
        other => Err(unsupported_format_marker(other)),
    }
}

/// A `FORMAT` marker this binary does not read, **naming what it holds** (RFC 156 §5b): a number is
/// reported as that version, anything else is quoted. 0.44.0 and earlier said `unsupported format
/// version: 0` whatever the marker held.
fn unsupported_format_marker(marker: &[u8]) -> PrikkError {
    let text = String::from_utf8_lossy(marker);
    let trimmed = text.trim_end_matches('\n');
    match trimmed.parse::<u32>() {
        Ok(version) => PrikkError::UnsupportedFormatVersion(version),
        Err(_) => PrikkError::Integrity(format!(
            "the repository format marker .prikk/FORMAT holds {trimmed:?}, which is not a format \
             number this version reads (it reads formats 6, 7 and 8)"
        )),
    }
}
