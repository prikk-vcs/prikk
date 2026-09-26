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

/// **The one rule for a partial frame** (RFC 160 F3, "a torn tail is a prefix of one frame"). A reader that reaches a frame whose
/// header claims more bytes than remain has two possible explanations: the harmless remnant of an interrupted append (a **prefix
/// of one well-formed frame**, and nothing else), or a damaged length in front of records that are still sound. Before this rule
/// every framed reader took the first explanation unconditionally, so a sound record after the damage was swallowed into the
/// "tail" -- and `doctor --repair-wal-tail` then truncated it away.
///
/// A torn tail is the **last** thing in the file. So if a **sound** frame starts anywhere in the remainder -- `sound_frame_at`
/// says the format's own full parse (magic, valid header, a body that passes its checksum, an admissible record) accepts one
/// there; a magic match alone is not enough, since payloads can contain magic bytes -- then the partial frame is damage and is
/// returned with the offset to resume at. **Ambiguity resolves to damage**: a sound frame embedded in the payload of a genuinely
/// torn frame reads as damage here. The cost of a false "damage" is a refused repair and a manual step; the cost of a false "tail"
/// is lost data.
///
/// `offset` is where the partial frame starts. Returns `None` when nothing in the remainder is a sound frame: a true tail.
pub(crate) fn sound_frame_after_partial(
    bytes: &[u8],
    offset: usize,
    magic: &[u8],
    sound_frame_at: impl Fn(usize) -> bool,
) -> Option<usize> {
    let mut from = offset.checked_add(1)?;
    while let Some(candidate) = resync_to_next_magic(bytes, from, magic) {
        if sound_frame_at(candidate) {
            return Some(candidate);
        }
        from = candidate.checked_add(1)?;
    }
    None
}

/// The message every framed reader gives a partial frame that is damage (`reported_*` are the offsets as the reader names them: the
/// file's, for a reader decoding a tail of one).
pub(crate) fn partial_before_sound_frame_message(
    reported_offset: usize,
    reported_next: usize,
) -> String {
    format!(
        "frame at byte offset {reported_offset} claims more bytes than remain, but a sound frame starts at byte offset {reported_next}: \
         damage, not a torn tail"
    )
}

#[cfg(test)]
mod tests;
