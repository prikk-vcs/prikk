//! RFC 155 Part I-A: the minimal, foundation-layer read of "is an archive import journal present,
//! and what is its short id" -- used by `lock.rs` itself (to refuse an ordinary writer while one
//! is present), which must not depend on `archive` (a feature module built on top of `lock`, not
//! the other way around: boundary-check's own layer rule caught the cycle a first version of this
//! part created, naming it `archive -> lock`).
//!
//! **The full journal** (its container list, its landing plan, its own checksum verification) is
//! `archive::import`'s own job, over its own richer, versioned wire format. This reads only the
//! journal's fixed-position archive-identity field -- present at the same offset regardless of
//! what the rest of the format ever grows into -- matching `archive::import`'s own short-id
//! algorithm exactly, so the two can never disagree on what one journal's "id" is.

use prikk_error::{PrikkError, Result};

use super::fsutil::read_file_if_exists;
use super::layout::RepositoryLayout;

/// Magic(8) + version(2), before the archive-identity field.
const MAGIC_AND_VERSION_LEN: usize = 8 + 2;
const IDENTITY_LEN: usize = 32;

/// `Some(short id)` when a non-empty journal is present; `None` otherwise -- absent, or emptied at
/// the commit point or by `cancel_import`, both of which read identically to "no import in
/// progress".
pub(crate) fn short_id_if_present(layout: &RepositoryLayout) -> Result<Option<String>> {
    let root = layout.repository_mutation_root();
    let relative = layout.repository_relative(&layout.import_journal_path())?;
    let Some(bytes) = read_file_if_exists(root, &relative)? else {
        return Ok(None);
    };
    if bytes.is_empty() {
        return Ok(None);
    }
    let identity = bytes
        .get(MAGIC_AND_VERSION_LEN..MAGIC_AND_VERSION_LEN + IDENTITY_LEN)
        .ok_or_else(|| {
            PrikkError::Integrity("import journal is too short to name its own id".to_string())
        })?;
    Ok(Some(short_id(identity)))
}

/// The id both this module and `archive::import` print: the first 16 hex characters of the
/// archive's own manifest checksum, matching `recovery_log::ID_LEN`'s own convention for a short,
/// printable id.
pub(crate) fn short_id(archive_identity: &[u8]) -> String {
    prikk_hash::to_hex(archive_identity)
        .chars()
        .take(16)
        .collect()
}

/// Called from inside [`crate::lock::ActiveLock::acquire`] itself for the default session -- see
/// that function's own doc comment for why this is the structural half of RFC 155 §9.5's "every
/// repository writer refuses" rule.
pub(crate) fn refuse_while_present(layout: &RepositoryLayout) -> Result<()> {
    let Some(id) = short_id_if_present(layout)? else {
        return Ok(());
    };
    Err(PrikkError::LockConflict(format!(
        "an archive import did not finish (id {id}); every repository writer refuses while its \
         journal is present -- run the same import again (it resumes), or `prikk doctor \
         --cancel-import {id}`"
    )))
}
