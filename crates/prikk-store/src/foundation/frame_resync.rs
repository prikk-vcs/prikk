//! Shared byte-wise resync scan for isolate-and-continue frame readers (RFC 102 Stage 2).
//!
//! `wal.rs` and `refs/log.rs` each independently implemented this scan in Stage 2, byte-for-byte
//! identical apart from which magic constant they matched -- reasonable at the time (Stage 2 earned
//! the behaviour against two formats already in production), but RFC 102 Stage 3's own handoff is
//! explicit that its container read path must **reuse** this reader, not write a third copy. This is
//! that extraction: the one piece of logic every isolate-and-continue reader in this codebase needs,
//! parameterized only by the magic bytes a given frame format uses.
//!
//! Never uses any field from a failed frame to decide where to resume -- a corrupted length cannot
//! push the scan past a sound record, because the resume point is never derived from it. On a
//! rejected false-positive candidate (the magic appearing inside corrupted body bytes), the caller is
//! expected to advance by exactly one byte and call this again from there -- this function only finds
//! the *next* magic from `start`, it does not know whether a caller's full-frame validation at that
//! offset will succeed.

/// Scan `bytes` byte-wise from `start` for the next occurrence of `magic`. Returns `None` once fewer
/// than `magic.len()` bytes remain -- nothing further to find.
pub(crate) fn resync_to_next_magic(bytes: &[u8], start: usize, magic: &[u8]) -> Option<usize> {
    let magic_len = magic.len();
    let mut cursor = start;
    while cursor
        .checked_add(magic_len)
        .is_some_and(|end| end <= bytes.len())
    {
        if bytes.get(cursor..cursor + magic_len) == Some(magic) {
            return Some(cursor);
        }
        cursor += 1;
    }
    None
}

#[cfg(test)]
mod tests;
