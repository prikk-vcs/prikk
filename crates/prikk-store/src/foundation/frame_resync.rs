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

/// RFC 167 D1: an explicit, per-decode work budget, passed through the call rather than tracked in
/// a thread-local counter live in every build (the design round's prototype used the latter; the
/// accepted design corrects it -- `rfc167-design-round-review-v1`, ruling 1). One instance is
/// created per decode call (`for_input`, at the input's own length) and shared by `&mut` reference
/// into both placements the design round measured as each insufficient alone:
/// - **narrow**, inside [`sound_frame_after_partial_budgeted`]'s own loop, between candidates;
/// - **broad**, in each reader's own outer decode loop, charging that reader's ordinary per-frame
///   checksum cost too -- the cost a `never_a_tail`-shaped candidate pays without ever reaching the
///   narrow placement at all.
///
/// Both placements charge the *same* counter, so either one alone still sees the other's cost.
#[derive(Debug)]
pub(crate) struct ScanBudget {
    limit: u64,
    spent: u64,
}

/// RFC 167 D1: 8x the input length -- a constant, not a config key (a key would add a surface and a
/// way to misconfigure a safety check). RFC 167 §6 item 2 ("the honest margin"): 8x must leave at
/// least a 4x margin over the largest bytes-hashed-to-input ratio seen on honest data anywhere in
/// the suite, the RFC 133 corpus, `matrix.py`, and a magic-dense repository -- measured in this
/// round's own report, not re-derived here.
pub(crate) const SCAN_BUDGET_MULTIPLE: u64 = 8;

impl ScanBudget {
    /// A fresh budget for one decode call over an input of `input_len` bytes. `.max(1)` so a
    /// zero-length input still has a (trivial) budget rather than none at all.
    pub(crate) fn for_input(input_len: usize) -> Self {
        Self {
            limit: SCAN_BUDGET_MULTIPLE * (input_len as u64).max(1),
            spent: 0,
        }
    }

    /// Record `n` more bytes hashed against this budget. Saturating: a budget this far over its
    /// limit is already exceeded, and nothing downstream needs the exact overshoot.
    pub(crate) fn charge(&mut self, n: u64) {
        self.spent = self.spent.saturating_add(n);
    }

    /// Whether this budget's limit has been passed. Checked after charging, never before -- the
    /// charge that crosses the limit is itself allowed to complete (it is already paid for), and
    /// only the *next* candidate is refused.
    pub(crate) fn exceeded(&self) -> bool {
        self.spent > self.limit
    }
}

/// RFC 167 D2: the third outcome a budgeted scan can reach, alongside finding a sound frame or
/// finding none. **Every reader treats `Undetermined` as damage, never as a tail** (C3) -- the scan
/// could not rule out a sound frame hidden in the remainder, and ambiguity resolves to damage, the
/// same rule [`sound_frame_after_partial`] already applies to a frame it *did* finish checking.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoundedScan {
    /// A sound frame starts at this offset: damage, with the offset to resume at (mirrors
    /// [`sound_frame_after_partial`]'s `Some`).
    Sound(usize),
    /// Nothing sound anywhere in the remainder: a true tail (mirrors [`sound_frame_after_partial`]'s
    /// `None`).
    NoSoundFrame,
    /// The budget ran out before every candidate could be checked. Never a tail.
    Undetermined,
}

/// The budgeted sibling of [`sound_frame_after_partial`] (RFC 167 D1/D2): identical scan, except
/// each candidate's check is charged against `budget` (the narrow placement) and a budget already
/// exceeded when a candidate is reached stops the scan with [`BoundedScan::Undetermined`] rather
/// than continuing. `sound_frame_at` takes the same `&mut ScanBudget` explicitly, not by closure
/// capture, so a reader's own full-frame check (which hashes a candidate's claimed body) can charge
/// the identical counter the broad placement, in that reader's own outer loop, also charges.
pub(crate) fn sound_frame_after_partial_budgeted(
    bytes: &[u8],
    offset: usize,
    magic: &[u8],
    budget: &mut ScanBudget,
    mut sound_frame_at: impl FnMut(usize, &mut ScanBudget) -> bool,
) -> BoundedScan {
    let Some(mut from) = offset.checked_add(1) else {
        return BoundedScan::NoSoundFrame;
    };
    while let Some(candidate) = resync_to_next_magic(bytes, from, magic) {
        if budget.exceeded() {
            return BoundedScan::Undetermined;
        }
        if sound_frame_at(candidate, budget) {
            return BoundedScan::Sound(candidate);
        }
        from = match candidate.checked_add(1) {
            Some(next) => next,
            None => return BoundedScan::NoSoundFrame,
        };
    }
    BoundedScan::NoSoundFrame
}

/// RFC 167 D2: the one coherent message every reader gives a scan the budget cut short, before that
/// reader's own wrapper (a hard failure, or -- for object containers and the ref container, RFC 164
/// Rule E -- "a harmless remnant" once reachability says nothing needs it) adds its own verdict. This
/// text alone never says "damage": the two readers with a remnant classification would otherwise read
/// as self-contradictory ("damage ... a harmless remnant, not damage"), exactly what the design
/// round's own prototype said and the review rejected (`rfc167-design-round-review-v1`, "What I
/// verified").
pub(crate) fn scan_budget_exceeded_message(reported_offset: usize) -> String {
    format!(
        "the bytes after byte offset {reported_offset} look like many frame headers; prikk stopped checking after {SCAN_BUDGET_MULTIPLE}x the file's size"
    )
}

/// Like [`scan_budget_exceeded_message`], for the four readers with no remnant classification (the
/// WAL, trust policy, received index, the pointer index): every `Undetermined` there is an
/// unconditional hard failure, so the message says so itself instead of leaving it to a wrapper that
/// does not exist for these formats.
pub(crate) fn scan_budget_exceeded_as_damage_message(reported_offset: usize) -> String {
    format!(
        "{} and treats this as damage, not a torn tail",
        scan_budget_exceeded_message(reported_offset)
    )
}

/// RFC 164 §9.2: **the checksum decides.** §9 decided whether the bytes at a tail candidate are a
/// *complete* record (never a tail, whatever its shape) from the header's own magic, version, and
/// length fields -- but any one of those three can itself be the single corrupted byte a full write
/// left behind, and a flipped header field is no more a crash's own signature than a flipped body byte
/// is. Tried before any of those fields is trusted: recomputes the checksum with the format's own real
/// magic and version constants (`checksum_of` already bakes them in, never the stored, possibly-
/// corrupted bytes at this offset) over two candidate bodies -- the one the stored length claims, and
/// the one that runs to the end of the file (catching a corrupted length field itself) -- and compares
/// against the checksum bytes stored at their fixed offset, read directly regardless of whether the
/// rest of the header parses at all. A match either way means the record was fully written; only the
/// checksum's own bytes (or the body) still being wrong falls through to genuine damage, and nothing
/// sound anywhere in the remainder falls through to a genuine tail, exactly as before this rule.
///
/// **Excluded by construction, not by a special case**: a torn prefix (fewer bytes than one full
/// header) never reaches this -- every caller only calls it once a full header's worth of bytes is
/// confirmed present, the same precondition `sound_frame_after_partial`'s own callers already check.
/// The WAL keeps RFC 162 rule 3 unchanged (RFC 164 §9.2): its own decode loop does not call this.
///
/// `header_len` is the format's fixed header length; the checksum is assumed to be its trailing 32
/// bytes and the body length its preceding 8 (magic(8) + version(2) + body_len(8) + checksum(32) is
/// every format's own shared layout, confirmed against each module's `*_HEADER_LEN` constant).
/// Returns the record's own total length (header + body) from `offset` when a checksum-verified
/// interpretation exists, `None` otherwise.
///
/// RFC 167 D1: charges `budget` with each candidate body's length right before hashing it (up to
/// two charges per call -- the stored length, and the to-EOF length). The four readers that call
/// this and are immune to M5 (generation logs, trust keys, author keys) pass a budget too, purely to
/// satisfy this signature; they never check it for `exceeded()`, and RFC 167 D2 leaves them
/// unchanged.
pub(crate) fn complete_by_checksum(
    bytes: &[u8],
    offset: usize,
    header_len: usize,
    budget: &mut ScanBudget,
    checksum_of: impl Fn(u64, &[u8]) -> [u8; 32],
) -> Option<usize> {
    let header_end = offset.checked_add(header_len)?;
    if header_end > bytes.len() {
        return None;
    }
    let checksum_start = header_end.checked_sub(32)?;
    let stored_checksum: [u8; 32] = bytes.get(checksum_start..header_end)?.try_into().ok()?;
    let body_len_start = checksum_start.checked_sub(8)?;
    let claimed_body_len = bytes
        .get(body_len_start..checksum_start)
        .and_then(|slice| slice.try_into().ok())
        .map(u64::from_be_bytes);

    // Candidate 1: the stored length, whatever it claims -- catches a corrupted magic or version byte
    // alone, with the length field itself untouched.
    if let Some(claimed) = claimed_body_len {
        if let Ok(claimed_usize) = usize::try_from(claimed) {
            if let Some(body_end) = header_end.checked_add(claimed_usize) {
                if let Some(body) = bytes.get(header_end..body_end) {
                    budget.charge((body.len() + header_len) as u64);
                    if checksum_of(claimed, body) == stored_checksum {
                        return Some(body_end);
                    }
                }
            }
        }
    }

    // Candidate 2: the length to the end of the file -- catches a corrupted length field itself, on
    // the last record in the file (the only place a positional tail candidate ever arises).
    let to_eof_len = bytes.len().saturating_sub(header_end);
    if let Some(body_to_eof) = bytes.get(header_end..) {
        budget.charge((body_to_eof.len() + header_len) as u64);
        if checksum_of(to_eof_len as u64, body_to_eof) == stored_checksum {
            return Some(bytes.len());
        }
    }
    None
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

/// [`tallied_sha256`] over several slices in order, hashed in place: the bytes are counted once each, as if concatenated.
pub(crate) fn tallied_sha256_parts(parts: &[&[u8]]) -> [u8; 32] {
    #[cfg(test)]
    hash_tally::record(parts.iter().map(|part| part.len()).sum());
    prikk_hash::sha256_parts(parts)
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
