//! A `FORMAT` marker that is absent or empty, and the one sentence every command prints for it (0.59.0 step K4).
//!
//! Two states look alike from the marker and differ in everything else (`init_set.rs`): an `init` that did not finish (`.prikk` holds only what a
//! killed `init` leaves: `prikk init` finishes it) and a repository that lost its marker (the format number is gone: `prikk format upgrade` verifies
//! it and writes format 8, or a sound copy is restored). Before K4 the first was `required root-relative file is absent: FORMAT`, the empty marker a
//! killed `init` could leave was `holds "", which is not a format number`, and neither named a command.

use std::path::Path;

use prikk_error::{PrikkError, Result};

use super::RepositoryLayout;
use crate::foundation::fsutil::{MutationRoot, read_file_range_if_exists};

/// The sound-copy sequence, in full: the way out of every state no command repairs in place. Defined here, in the foundation layer, because the layout's
/// own refusal names it and a layer below `refs` cannot read `refs`; `refs::SOUND_COPY` and `way_out::SOUND_COPY` are this value.
pub(crate) const SOUND_COPY: &str = "restore a sound copy of the sealed history: `prikk setup <new-dir>`, then, in \
<new-dir>, `prikk archive verify <backup>` and `prikk archive import <backup> --adopt` (or `prikk bundle \
import --input <bundle>` for one branch); the key that sealed the history must be adopted in <new-dir> \
(`archive verify` reports it). Only sealed history travels in a backup: a commit still waiting for a \
`prikk seal` is made again there";

/// What `.prikk/FORMAT` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MarkerState {
    /// There is no file.
    Absent,
    /// The file holds no byte.
    Empty,
    /// The file holds bytes (a format, or something this binary does not read).
    Written,
}

impl MarkerState {
    pub(crate) const fn is_lost(self) -> bool {
        matches!(self, Self::Absent | Self::Empty)
    }

    const fn words(self) -> &'static str {
        match self {
            Self::Absent => "absent",
            Self::Empty => "empty",
            Self::Written => "written",
        }
    }
}

/// What the marker's bytes (`None`: no file) say about whether it is lost.
pub(super) const fn state_of(bytes: Option<&[u8]>) -> MarkerState {
    match bytes {
        None => MarkerState::Absent,
        Some([]) => MarkerState::Empty,
        Some(_) => MarkerState::Written,
    }
}

/// Read what the marker holds, one byte at most (a regular file only: a directory or link where it should be is read as written, and refused by the
/// format read as it always was).
pub(super) fn marker_state(root: &MutationRoot) -> Result<MarkerState> {
    Ok(
        match read_file_range_if_exists(root, Path::new("FORMAT"), 0, 1)? {
            None => MarkerState::Absent,
            Some(bytes) if bytes.is_empty() => MarkerState::Empty,
            Some(_) => MarkerState::Written,
        },
    )
}

impl RepositoryLayout {
    /// The refusal for a lost marker: the sentence of the state `.prikk` is in. Every command that opens a layout prints it.
    pub(crate) fn lost_marker_refusal(&self, state: MarkerState) -> Result<PrikkError> {
        let words = state.words();
        Ok(if self.holds_only_a_killed_inits_state()? {
            PrikkError::Precondition(format!(
                "the format marker .prikk/FORMAT is {words}: an `init` did not finish here: `prikk init` finishes it"
            ))
        } else {
            PrikkError::Integrity(format!(
                "the format marker is lost (.prikk/FORMAT is {words}): \
                 `prikk format upgrade` verifies the repository and writes format 8 (prikk before 0.57.0 does not open format 8); or {SOUND_COPY}"
            ))
        })
    }
}
