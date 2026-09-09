//! Live rename declarations (RFC 144 §4o.2): `prikk mv`'s durable intent store.
//!
//! A declaration is an assertion about the *next* patch, not a mutation in its own right -- `prikk
//! mv` records one (and, for the "old gone, new exists" worktree state, that is the command's
//! entire effect); `commit` consumes every live declaration when it queues a patch, whether that
//! declaration nets to a `RenamePath`, a plain `DeleteNode`, or nothing at all (see
//! `worktree_patch::node_authoring`'s own consumption logic). Because a commit that succeeds always
//! resolves every live declaration one way or another, "cleared when the commit that consumes it is
//! queued" reduces to "the whole store is emptied on a successful commit" -- there is no partial-set
//! removal to implement.
//!
//! Stored as one `old_path\tnew_path` line per declaration, sibling to `queue.wal`/`ref-name` under
//! `.prikk/active/<name>/declarations`, replaced wholesale (truncate, then append the full new
//! content) on every write -- the same structural-replace discipline `active.rs`'s
//! `write_active_ref_metadata` uses for the same reason: this is `pub` surface, so a second call
//! must not be able to silently concatenate onto a first. A tab or newline can never appear inside a
//! valid `RepoPath` (`validate_repo_path` rejects every control byte), so this format needs no
//! escaping.
//!
//! `record_rename_declaration` holds the same `ActiveLock` `commit`'s own critical section holds
//! across its read-modify-write -- `author_inner` clears this store while still holding that lock
//! (RFC 144 §4o.2: cleared when the commit that consumes it is queued), so without this a `prikk mv`
//! racing a `commit` could read the pre-clear set, lose the race, and write it straight back,
//! resurrecting a declaration that commit had just consumed.

use std::collections::BTreeMap;

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::{
    append_file_required, create_new_file_required, read_file_if_exists,
    truncate_file_empty_required,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryLayout};
use crate::lock::ActiveLock;

/// One live declared rename: the net move from `old_path` to `new_path`, already collapsed through
/// any intermediate chain (`record_rename_declaration`'s own doc explains the collapse).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenameDeclaration {
    /// Repository-relative source path at the last sealed baseline.
    pub old_path: String,
    /// Repository-relative destination path this declaration currently targets.
    pub new_path: String,
}

/// What `record_rename_declaration` actually did (RFC 144 §4p.2: disclosure, not just effect --
/// `prikk mv` prints one of these, so a round trip that nets to no move is never silent about it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeclarationRecordOutcome {
    /// A declaration is now live, naming the net move (fresh, chain-extended, or replacing a prior
    /// unrelated one for the same `old_path`).
    Recorded {
        /// Repository-relative source path.
        old_path: String,
        /// Repository-relative destination path.
        new_path: String,
    },
    /// This call completed a round trip back to `origin` -- the earlier declaration was dropped
    /// rather than stored as a no-op `origin -> origin`. `via` is this call's own `old_path`.
    NetsToNoMove {
        /// The path the whole chain started from and returned to.
        origin: String,
        /// This call's own `old_path` -- the intermediate hop the chain returned from.
        via: String,
    },
}

/// Read every live rename declaration, in `old_path` order. An absent file reads the same as an
/// empty one -- both mean "no live declarations," matching `ActiveRefMetadata`'s own
/// absence-is-`Missing` convention (`active.rs`) -- a repository initialized before this store
/// existed has no file on disk yet, and this read path must keep working against one unassisted.
/// Unlocked: a plain read, same as `read_active_ref_metadata`'s own unlocked read of `ref-name`.
pub fn read_rename_declarations(layout: &RepositoryLayout) -> Result<Vec<RenameDeclaration>> {
    Ok(read_declarations_map(layout)?
        .into_iter()
        .map(|(old_path, new_path)| RenameDeclaration { old_path, new_path })
        .collect())
}

fn read_declarations_map(layout: &RepositoryLayout) -> Result<BTreeMap<String, String>> {
    let relative = layout.repository_relative(&layout.default_declarations_path())?;
    let Some(bytes) = read_file_if_exists(layout.repository_mutation_root(), &relative)? else {
        return Ok(BTreeMap::new());
    };
    if bytes.is_empty() {
        return Ok(BTreeMap::new());
    }
    let text = std::str::from_utf8(&bytes).map_err(|err| {
        PrikkError::Integrity(format!("rename-declaration store is not UTF-8: {err}"))
    })?;
    let mut map = BTreeMap::new();
    for line in text.lines() {
        let mut parts = line.splitn(2, '\t');
        let (Some(old_path), Some(new_path)) = (parts.next(), parts.next()) else {
            return Err(PrikkError::Integrity(format!(
                "rename-declaration store has a malformed line: {line:?}"
            )));
        };
        map.insert(old_path.to_string(), new_path.to_string());
    }
    Ok(map)
}

fn write_declarations_map(layout: &RepositoryLayout, map: &BTreeMap<String, String>) -> Result<()> {
    layout.require_current_format()?;
    let relative = layout.repository_relative(&layout.default_declarations_path())?;
    // A repository initialized before this file existed has no `declarations` on disk yet (`init`
    // now pre-allocates it for every new repository, the same as `ref-name`, but this write path
    // must not require a re-`init` or a separate migration step to work against an older one) --
    // `truncate_file_empty_required`/`append_file_required` below both require the file to already
    // exist. Idempotent, matching `layout.rs`'s own `create_empty_file_once` -- both this function's
    // own callers already serialize against each other under `ActiveLock`, so no second writer can
    // observe this check and the create it guards as anything but atomic.
    if read_file_if_exists(layout.repository_mutation_root(), &relative)?.is_none() {
        create_new_file_required(layout.repository_mutation_root(), &relative, &[])?;
    }
    let mut content = String::new();
    for (old_path, new_path) in map {
        content.push_str(old_path);
        content.push('\t');
        content.push_str(new_path);
        content.push('\n');
    }
    truncate_file_empty_required(layout.repository_mutation_root(), &relative)?;
    if !content.is_empty() {
        append_file_required(
            layout.repository_mutation_root(),
            &relative,
            content.as_bytes(),
        )?;
    }
    Ok(())
}

/// Record a declared move from `old_path` to `new_path`, collapsing a chain to its net effect
/// (RFC 144 §4o.2/§3, quoting §4j.2: "its only honest meaning is the net move").
///
/// If an existing live declaration's own `new_path` equals this call's `old_path` (i.e. `old_path`
/// is the current landing spot of an earlier declared move), that declaration is extended in place
/// to the net move instead of a second, independent declaration being recorded -- `mv a b` then
/// `mv b c` leaves exactly one declaration, `a -> c`. If the net result is a full round trip
/// (`origin == new_path`, e.g. `mv a b` then `mv b a`), the declaration is dropped entirely rather
/// than stored as a no-op `a -> a`.
///
/// Otherwise this is either a fresh declaration or a second declaration naming an `old_path` that
/// already had a distinct, unrelated live declaration (only reachable when that declaration's own
/// destination is unrelated to this call's `new_path` -- see the caller's own four-worktree-state
/// check, which requires `old_path` to be absent from disk either way). Judgment call, not
/// RFC-specified: the newest declaration for a given `old_path` replaces the previous one, the same
/// "last stated intent wins" rule an ordinary key-value overwrite already gives for free.
pub fn record_rename_declaration(
    layout: &RepositoryLayout,
    old_path: &str,
    new_path: &str,
) -> Result<DeclarationRecordOutcome> {
    // Held across the whole read-modify-write -- see this module's own doc comment for the race
    // this closes (a concurrent `commit` clearing the store between this call's read and write).
    let _lock = ActiveLock::acquire(layout, DEFAULT_ACTIVE_NAME)?;
    let mut map = read_declarations_map(layout)?;
    let chained_origin = map
        .iter()
        .find(|(_, dest)| dest.as_str() == old_path)
        .map(|(origin, _)| origin.clone());
    let outcome = if let Some(origin) = chained_origin {
        map.remove(&origin);
        if origin == new_path {
            DeclarationRecordOutcome::NetsToNoMove {
                origin,
                via: old_path.to_string(),
            }
        } else {
            map.insert(origin.clone(), new_path.to_string());
            DeclarationRecordOutcome::Recorded {
                old_path: origin,
                new_path: new_path.to_string(),
            }
        }
    } else {
        map.insert(old_path.to_string(), new_path.to_string());
        DeclarationRecordOutcome::Recorded {
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
        }
    };
    write_declarations_map(layout, &map)?;
    Ok(outcome)
}

/// Clear every live rename declaration. Called once, after a commit that consumed the whole live
/// set has successfully appended its patch to the active WAL -- see this module's own doc comment
/// for why there is no partial-set removal.
pub fn clear_rename_declarations(layout: &RepositoryLayout) -> Result<()> {
    write_declarations_map(layout, &BTreeMap::new())
}

#[cfg(all(test, target_os = "linux"))]
mod tests;
