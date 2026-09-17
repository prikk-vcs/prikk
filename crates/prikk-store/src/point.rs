//! A point in history (RFC 153 §2 and §7.1, RFC 157 §2): a ref, or a bare block id.
//!
//! The shape of a point and the checks that need nothing but its spelling. **Resolving** one reads refs
//! and the received-ref index, so [`crate::ref_resolution::resolve_point`] does that, in the upper layer
//! (RFC 149); this module depends on no other store module, so the lower-layer readers that take a
//! resolved point -- `checkout`, `patch_replay`, `worktree` -- can name it.

use prikk_error::{PrikkError, Result};
use prikk_object::ObjectId;

/// How a [`Point`] was named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PointKind {
    /// A local branch, `heads/…`.
    LocalBranch,
    /// A local tag, `tags/…`, resolved through its Tag object to the Block it names.
    Tag,
    /// A received ref, `remotes/…`.
    ReceivedRef,
    /// A bare block id: 64 lowercase hex characters.
    Block,
}

/// A point in history a read names (RFC 153 §2, RFC 157 §2): a ref, or a bare block id, resolved to the
/// Block whose state the read is of.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Point {
    /// The name exactly as given: a ref name, or the block id.
    pub name: String,
    /// How it was named.
    pub kind: PointKind,
    /// The Block the point resolves to.
    pub block_id: ObjectId,
}

/// Whether `name` is spelled as a bare block id: exactly 64 lowercase hex characters. A ref name never
/// is one -- every ref name starts `heads/`, `tags/` or `remotes/` -- so the two cannot collide.
#[must_use]
pub fn is_bare_block_id(name: &str) -> bool {
    name.len() == 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Whether `name` has the shape of a point at all: a ref name (`heads/…`, `tags/…`, `remotes/…`) or a
/// bare block id. A command refuses anything else as a usage error before resolving it.
#[must_use]
pub fn is_point_name(name: &str) -> bool {
    ["heads/", "tags/", "remotes/"]
        .iter()
        .any(|prefix| name.starts_with(prefix))
        || is_bare_block_id(name)
}

/// Refuse a bare block id given to a command that writes the worktree: the next `commit` authors against
/// a branch, and a block id names none (RFC 153 point-resolver handoff §2.4).
///
/// # Errors
///
/// `Precondition` when `name` is a bare block id.
pub fn refuse_block_point_for_worktree_write(name: &str, command: &str) -> Result<()> {
    if is_bare_block_id(name) {
        return Err(PrikkError::Precondition(format!(
            "{command} writes the worktree, which needs a branch: the next `commit` authors against \
             one, and a block id names no branch"
        )));
    }
    Ok(())
}
