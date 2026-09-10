//! RFC 131 §6d.2's first grouping under the lifted §6a prohibition: `active` (active-session/
//! ref-metadata) and `worktree_patch` (worktree patch authoring) -- two co-designed layers each
//! checking the half of a commit boundary the other owns (`active` asks `worktree_patch` whether
//! the active-patch queue is full; `worktree_patch`'s node authoring asks `active` to prepare or
//! validate the active-ref metadata it needs before authoring). Grouped for the real, mutual
//! dependency between them, not because they were easiest to move.

pub(crate) mod active;
pub(crate) mod worktree_patch;
