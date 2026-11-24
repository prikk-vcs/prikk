//! `prikk format upgrade`: move a format-6 or format-7 repository to the current format, 8, in place
//! (RFC 156 §5b; RFC 114 §5.2a; 0.57.0 Unicode repository paths).
//!
//! Format 7 is format 6 plus "an id may hold several records; the last is authoritative"; format 8 is
//! format 7 plus "a repository path may be non-ASCII". Nothing stored differs between them, so the
//! upgrade **carries everything by moving nothing**: it proves the repository verifies, then changes
//! the one-line `FORMAT` marker, in one step from either older format. It is explicit and never
//! automatic, idempotent on format 8, and has no inverse — there is no downgrade.
//!
//! **The lock.** `ActiveLock`, held from reading the marker through writing it: every writer that can
//! store objects under this repository's author or maintainer keys — `commit`, `seal`, `merge`,
//! `sync accept`/`seal`, `bundle import`, `rollback-draft` — takes it, so none runs between the
//! verification and the marker change. Writers that take only ref locks (`branch create`, `tag create`)
//! write nothing a format-6 repository could not already hold, so they cannot invalidate what was
//! verified.

use std::path::{Path, PathBuf};

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{overwrite_in_place_required, write_file_atomically};
use crate::foundation::layout::{
    CURRENT_FORMAT_VERSION, DEFAULT_ACTIVE_NAME, RepositoryFormat, RepositoryLayout, SOUND_COPY,
};
use crate::lock::ActiveLock;
use crate::verify::{RepositoryVerification, verify_repository};

/// What `prikk format upgrade` did.
#[non_exhaustive]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatUpgradeOutcome {
    /// The repository was already at the current format; nothing was written.
    AlreadyCurrent {
        /// The format it is at.
        format: u32,
    },
    /// The marker was lost (absent or empty) on a populated repository, and is written again: the repository verified, and it is format 8 now. The
    /// number it held is not known, so none is claimed (0.59.0 step K4).
    MarkerRestored {
        /// The format it is at now.
        to: u32,
    },
    /// The marker was changed.
    Upgraded {
        /// The format it was at.
        from: u32,
        /// The format it is at now.
        to: u32,
    },
}

/// Upgrade `layout`'s repository to the current format (8), from 6 or 7.
///
/// `verdict` decides whether the verification passed. It is the caller's, not a second definition here:
/// the CLI passes `prikk verify`'s own blocking-condition declaration, so an upgrade passes exactly when
/// `prikk verify` would exit 0. It returns the reason a failing verification fails.
///
/// Refuses, writing nothing, when the lock is held, the marker is not 6, 7 or 8, or the verification
/// does not pass.
pub fn upgrade_repository_format(
    layout: &RepositoryLayout,
    verdict: impl FnOnce(&RepositoryVerification) -> std::result::Result<(), String>,
) -> Result<FormatUpgradeOutcome> {
    let _lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    // The marker as it is now, under the lock — not as it was when `layout` was opened.
    let current = RepositoryLayout::open_allowing_lost_marker(layout.root().to_path_buf())?;
    if current.marker_is_lost() {
        return restore_lost_marker(&current, verdict);
    }
    let from = match current.format() {
        RepositoryFormat::V8 => {
            return Ok(FormatUpgradeOutcome::AlreadyCurrent {
                format: RepositoryFormat::V8.number(),
            });
        }
        older @ (RepositoryFormat::CurrentV6 | RepositoryFormat::V7) => older.number(),
    };

    let report = verify_repository(&current)?;
    verdict(&report).map_err(|reason| {
        PrikkError::Precondition(format!(
            "refusing to upgrade the repository format: verification did not pass ({reason}); run \
             `prikk verify`, fix what it reports, then upgrade -- nothing was changed"
        ))
    })?;

    #[cfg(test)]
    if let Some(change) = BEFORE_MARKER_WRITE.with(|slot| slot.borrow_mut().take()) {
        change();
    }

    // RFC 168 §3.3: the marker is a one-byte overwrite of its own `6\n` or `7\n`, flushed. A one-byte write does not
    // tear, and a later format-8 write cannot outlive a marker that reverts, because the marker is never renamed.
    overwrite_in_place_required(
        current.repository_mutation_root(),
        Path::new("FORMAT"),
        0,
        CURRENT_FORMAT_VERSION.get(..1).unwrap_or_default(),
    )?;
    let upgraded = RepositoryLayout::open(layout.root().to_path_buf())?;
    if upgraded.format() != RepositoryFormat::V8 {
        return Err(PrikkError::Integrity(format!(
            "the format marker was written but reads format {} afterwards",
            upgraded.format().number()
        )));
    }
    Ok(FormatUpgradeOutcome::Upgraded {
        from,
        to: RepositoryFormat::V8.number(),
    })
}

/// `prikk format upgrade` over a marker that is absent or empty on a **populated** repository (0.59.0 step K4; an unpopulated `.prikk` is an `init` that did
/// not finish, and opening it refuses naming `prikk init`). The earlier number is lost, so none is claimed: the repository must verify, and the marker is
/// written as format 8, which holds everything formats 6 and 7 hold. Written through a temporary file and a rename, because there is no marker to overwrite:
/// a kill leaves the marker absent or empty as it was, or format 8, never a part of it.
fn restore_lost_marker(
    current: &RepositoryLayout,
    verdict: impl FnOnce(&RepositoryVerification) -> std::result::Result<(), String>,
) -> Result<FormatUpgradeOutcome> {
    let report = verify_repository(current)?;
    verdict(&report).map_err(|reason| {
        PrikkError::Precondition(format!(
            "refusing to restore the format marker: verification did not pass ({reason}), and `prikk verify` cannot run without the marker; nothing \
             was changed; {SOUND_COPY}"
        ))
    })?;

    #[cfg(test)]
    if let Some(change) = BEFORE_MARKER_WRITE.with(|slot| slot.borrow_mut().take()) {
        change();
    }

    write_file_atomically(
        current.repository_mutation_root(),
        Path::new("FORMAT"),
        CURRENT_FORMAT_VERSION,
    )?;
    let restored = RepositoryLayout::open(current.root().to_path_buf())?;
    if restored.format() != RepositoryFormat::V8 {
        return Err(PrikkError::Integrity(format!(
            "the format marker was written but reads format {} afterwards",
            restored.format().number()
        )));
    }
    Ok(FormatUpgradeOutcome::MarkerRestored {
        to: RepositoryFormat::V8.number(),
    })
}

/// `prikk format upgrade [path]`: open `root` (a populated repository with a lost marker included) and upgrade it.
///
/// # Errors
///
/// As [`upgrade_repository_format`]; an unpopulated `.prikk` with no marker refuses, naming `prikk init`.
pub fn upgrade_repository_format_in(
    root: impl Into<PathBuf>,
    verdict: impl FnOnce(&RepositoryVerification) -> std::result::Result<(), String>,
) -> Result<FormatUpgradeOutcome> {
    let layout = RepositoryLayout::open_allowing_lost_marker(root)?;
    upgrade_repository_format(&layout, verdict)
}

#[cfg(test)]
thread_local! {
    static BEFORE_MARKER_WRITE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

/// Test seam, unreachable from production: run `change` once, after verification passes and before the
/// marker is written — while the upgrade holds its lock.
#[cfg(test)]
pub(crate) fn before_marker_write_for_test(change: impl FnOnce() + 'static) {
    BEFORE_MARKER_WRITE.with(|slot| *slot.borrow_mut() = Some(Box::new(change)));
}

#[cfg(test)]
mod tests;
