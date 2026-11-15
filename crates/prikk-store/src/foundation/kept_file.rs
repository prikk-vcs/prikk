//! The one routine a kept decode of an append-only container shares (0.58.0 S4, S5): the object index, the ref pointer index and the ref log.
//!
//! A kept decode is brought up to date by reading **only what was appended** -- but only after proving the file is the one the decode
//! came from. This module is the proof, and nothing else: one `stat` of the file's identity and size, one positioned read of
//! `[known - canary.len(), size)`, a second `stat` that must name the same identity, and the canary (the last record the decode ended with)
//! compared with the file's bytes at its offset. What the appended bytes *mean* -- the index's frames, a pointer entry, a ref-log record --
//! is the caller's decoder.
//!
//! **Residual, stated (S4a §3).** A truncate to below `known_length` followed by a regrowth whose records between the cut and `known_length`
//! differ *and* whose record at `known_length` is byte-identical to the one the caller decoded would not be seen. That needs a second writer
//! to rebuild the prefix and re-create our last record byte for byte at its offset, which the append protocol does not produce (offsets are
//! container offsets). A full checksum of the prefix is O(file) per check and gives the reads back.

use std::path::Path;

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{
    FileIdentity, MutationRoot, len_to_u64, read_file_range_if_exists, stat_file_identity_if_exists,
};

/// A file as one `stat` shows it: its identity where the platform has one (Linux and macOS) and its size; a missing file is `(None, 0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileState {
    pub(crate) identity: Option<FileIdentity>,
    pub(crate) size: u64,
}

/// One `stat` of the file at `relative`.
pub(crate) fn stat_file(root: &MutationRoot, relative: &Path) -> Result<FileState> {
    Ok(stat_file_identity_if_exists(root, relative)?.map_or(
        FileState {
            identity: None,
            size: 0,
        },
        |stat| FileState {
            identity: stat.identity,
            size: stat.size,
        },
    ))
}

/// What [`read_verified_tail`] found.
pub(crate) enum VerifiedBytes {
    /// The file is no longer the one the caller's copy was decoded from (a different identity after the read than before it), is shorter
    /// than the copy's extent, or its bytes at the copy's last record are not the ones the copy decoded (the canary). The caller
    /// re-reads the file whole.
    NotTheSameFile,
    /// The same file, and these bytes appended after `start_offset` (the canary already checked and removed).
    Tail(Vec<u8>),
}

/// Read what was appended to the file since `start_offset` **and prove it is the file the caller's decoded copy came from**.
/// `observed` is the caller's own `stat` of the file just before; `canary` is the last record the copy decoded, `canary.len()` bytes ending
/// at `start_offset` (empty: no canary).
///
/// Where the platform has no file identity (`observed.identity` is `None`, Windows) the two `stat`s agree trivially and the read is the
/// bounded tail read it was before S4; nothing is *kept* without an identity (the caller's rule).
pub(crate) fn read_verified_tail(
    root: &MutationRoot,
    relative: &Path,
    start_offset: u64,
    observed: FileState,
    canary: &[u8],
) -> Result<VerifiedBytes> {
    let canary_len = len_to_u64(canary.len())?;
    if observed.size < start_offset || start_offset < canary_len {
        return Ok(VerifiedBytes::NotTheSameFile);
    }
    let read_from = start_offset - canary_len;
    let wanted = usize::try_from(observed.size - read_from)
        .map_err(|_| PrikkError::Integrity("file tail length exceeds usize".to_string()))?;
    let mut bytes = if wanted == 0 {
        Vec::new()
    } else {
        match read_file_range_if_exists(root, relative, read_from, wanted)? {
            Some(bytes) => bytes,
            None => return Ok(VerifiedBytes::NotTheSameFile),
        }
    };
    if bytes.len() != wanted || stat_file(root, relative)?.identity != observed.identity {
        return Ok(VerifiedBytes::NotTheSameFile);
    }
    if bytes.get(..canary.len()) != Some(canary) {
        return Ok(VerifiedBytes::NotTheSameFile);
    }
    Ok(VerifiedBytes::Tail(bytes.split_off(canary.len())))
}
