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

use prikk_error::{PrikkError, Result};

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

/// **RFC 160 §9 R2 -- no decode loop can stop advancing.** Every framed reader's resume point is a byte offset computed either from a
/// parsed frame's own length (`next_offset`, from `header_end + body_len`, both positive) or from [`resync_to_next_magic`], which
/// always searches from `offset + 1`. Both are strictly greater than the current offset **by construction** -- but "by construction"
/// is exactly the property a bug can break silently (RFC 160 F3's own perturbation did, unconditionally returning the buffer's end).
/// This turns the invariant into a checked one: every reader's loop advances through this call, so a computation that stops making
/// progress becomes an `Integrity` error at the exact place a spin would otherwise start, never a hang and never an unbounded
/// `Vec` of outcomes.
pub(crate) fn require_progress(reader: &'static str, offset: usize, next: usize) -> Result<usize> {
    if next <= offset {
        return Err(PrikkError::Integrity(format!(
            "{reader} decode did not advance past byte offset {offset} (computed next offset {next}); refusing rather than looping"
        )));
    }
    Ok(next)
}

/// **RFC 160 §9 R3 (the external review's M5): bytes hashed, tallied.** Every framed reader's checksum is `sha256(preimage)` where
/// `preimage` includes the frame's whole claimed body -- the quantity that made a torn tail packed with frame headers, each claiming a
/// body to the end of the file, quadratic: `sound_frame_after_partial` fully hashes every candidate's claimed body, and on that input
/// nearly every candidate claims nearly the whole remaining file. Every format's checksum function calls this instead of
/// `prikk_hash::sha256` directly, so the property tests can bound bytes hashed per input byte the same way P2 already bounds bytes read.
pub(crate) fn tallied_sha256(bytes: &[u8]) -> [u8; 32] {
    #[cfg(test)]
    hash_tally::record(bytes.len());
    prikk_hash::sha256(bytes)
}

/// **Test-only tally** (RFC 160 §9 R3 / the external review's M5): bytes passed to [`tallied_sha256`] on this thread since the last
/// [`reset`]. Mirrors `fsutil::anchored::read_tally` exactly, for the same reason: a property test can assert "this reader hashed at
/// most K times its input" only if something counts the hashing.
#[cfg(test)]
pub(crate) mod hash_tally {
    use std::cell::Cell;

    thread_local! {
        static BYTES: Cell<u64> = const { Cell::new(0) };
    }

    pub(super) fn record(len: usize) {
        BYTES.with(|total| total.set(total.get() + len as u64));
    }

    /// Bytes hashed by [`super::tallied_sha256`] on this thread since the last [`reset`].
    pub(crate) fn bytes_hashed() -> u64 {
        BYTES.with(Cell::get)
    }

    /// Zero this thread's tally.
    pub(crate) fn reset() {
        BYTES.with(|total| total.set(0));
    }
}
