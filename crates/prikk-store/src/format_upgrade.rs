//! `prikk format upgrade`: move a format-6 repository to format 7 in place (RFC 156 §5b; RFC 114 §5.2a).
//!
//! Format 7 is format 6 plus "an id may hold several records; the last is authoritative". Nothing
//! stored differs, so the upgrade **carries everything by moving nothing**: it proves the repository
//! verifies, then changes the one-line `FORMAT` marker. It is explicit and never automatic, idempotent
//! on format 7, and has no inverse — there is no downgrade.
//!
//! **The lock.** `ActiveLock`, held from reading the marker through writing it: every writer that can
//! store objects under this repository's author or maintainer keys — `commit`, `seal`, `merge`,
//! `sync accept`/`seal`, `bundle import`, `rollback-draft` — takes it, so none runs between the
//! verification and the marker change. Writers that take only ref locks (`branch create`, `tag create`)
//! write nothing a format-6 repository could not already hold, so they cannot invalidate what was
//! verified.

use std::path::Path;

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::overwrite_in_place_required;
use crate::foundation::layout::{
    CURRENT_FORMAT_VERSION, DEFAULT_ACTIVE_NAME, RepositoryFormat, RepositoryLayout,
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
    /// The marker was changed.
    Upgraded {
        /// The format it was at.
        from: u32,
        /// The format it is at now.
        to: u32,
    },
}

/// Upgrade `layout`'s repository to format 7.
///
/// `verdict` decides whether the verification passed. It is the caller's, not a second definition here:
/// the CLI passes `prikk verify`'s own blocking-condition declaration, so an upgrade passes exactly when
/// `prikk verify` would exit 0. It returns the reason a failing verification fails.
///
/// Refuses, writing nothing, when the lock is held, the marker is neither 6 nor 7, or the verification
/// does not pass.
pub fn upgrade_repository_format(
    layout: &RepositoryLayout,
    verdict: impl FnOnce(&RepositoryVerification) -> std::result::Result<(), String>,
) -> Result<FormatUpgradeOutcome> {
    let _lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    // The marker as it is now, under the lock — not as it was when `layout` was opened.
    let current = RepositoryLayout::open(layout.root().to_path_buf())?;
    match current.format() {
        RepositoryFormat::V7 => {
            return Ok(FormatUpgradeOutcome::AlreadyCurrent {
                format: RepositoryFormat::V7.number(),
            });
        }
        RepositoryFormat::CurrentV6 => {}
    }

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

    // RFC 168 §3.3: the marker is a one-byte overwrite of its own `6\n`, flushed. A one-byte write does not tear, and a
    // later format-7 write cannot outlive a marker that reverts, because the marker is never renamed.
    overwrite_in_place_required(
        current.repository_mutation_root(),
        Path::new("FORMAT"),
        0,
        CURRENT_FORMAT_VERSION.get(..1).unwrap_or_default(),
    )?;
    let upgraded = RepositoryLayout::open(layout.root().to_path_buf())?;
    if upgraded.format() != RepositoryFormat::V7 {
        return Err(PrikkError::Integrity(format!(
            "the format marker was written but reads format {} afterwards",
            upgraded.format().number()
        )));
    }
    Ok(FormatUpgradeOutcome::Upgraded {
        from: RepositoryFormat::CurrentV6.number(),
        to: RepositoryFormat::V7.number(),
    })
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
