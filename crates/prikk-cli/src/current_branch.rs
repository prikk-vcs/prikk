//! RFC 151 §2.2: the one place the CLI reads the current-branch pointer.
//!
//! Every `--ref` default and every "current branch" a command displays comes through here, so a
//! command cannot quietly keep the old `heads/main` constant, and the pointer has exactly two
//! readers in the product: this module and `doctor` (a test over both production trees holds it).
//! `--ref` given explicitly never reaches the pointer at all.

use prikk_store::{RepositoryLayout, current_branch};

use crate::commands::CliError;

/// The ref a command runs against: `explicit` when given, otherwise the current branch. A pointer
/// that cannot be resolved refuses with the store's own `Precondition` message.
pub(crate) fn resolve_ref(
    layout: &RepositoryLayout,
    explicit: Option<String>,
) -> std::result::Result<String, CliError> {
    match explicit {
        Some(ref_name) => Ok(ref_name),
        None => current_branch(layout).map_err(|err| CliError::from(err.to_string())),
    }
}

/// The current branch for display beside a report, or `None` when the pointer cannot be resolved.
/// Display never refuses: a command run with `--ref` must not fail because the default it did not
/// use is broken (`doctor` reports that).
pub(crate) fn displayed_current_branch(layout: &RepositoryLayout) -> Option<String> {
    current_branch(layout).ok()
}

/// Whether `name` is the repository's current branch, and it has never been published (RFC 147 §2i): an
/// explicit `--ref` naming it must answer exactly as leaving `--ref` off does, instead of the resolver's
/// ordinary refusal or block-requiring path. `false`, never an error, when the pointer cannot be
/// resolved -- the same "display never refuses" rule as [`displayed_current_branch`], since `name` came
/// from `--ref` either way and must not fail because a default it did not use is broken.
pub(crate) fn is_unpublished_current_branch(
    layout: &RepositoryLayout,
    name: &str,
) -> std::result::Result<bool, CliError> {
    if current_branch(layout).ok().as_deref() != Some(name) {
        return Ok(false);
    }
    prikk_store::is_unpublished_local_branch(layout, name).map_err(|err| err.to_string().into())
}
