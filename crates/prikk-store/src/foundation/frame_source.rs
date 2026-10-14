//! `FrameSource`: one byte-range read abstraction a framed decoder can be written against once,
//! whether its bytes are already resident (a slice) or must be fetched a window at a time (a
//! seekable stream, RFC 155 §9.4's archive verify). 0.52.0 step 2 (RFC 155 §9.4, amended
//! 2026-10-12: *"Until one primitive serves both (0.52.0), a differential test holds the two
//! equal"*) -- `foundation::container`'s own decode loop is the first, and so far only, caller;
//! this module holds the abstraction itself, not the container format's own decisions.
//!
//! **Scoped to the object container format alone**, not a generalization of
//! `frame_resync::resync_to_next_magic`/`sound_frame_after_partial_budgeted` for every framed
//! reader in this crate. The WAL and the ref log already share those two functions directly, over
//! `&[u8]`, and neither ever needs a streamed variant (both are always read whole) -- widening
//! their signatures to a generic source for one caller that does not need it would cost every
//! existing reader a conversion for nothing gained. This primitive's own generic resync (see
//! [`FrameSource::scan_for_magic`]'s default body) still calls the real, unchanged
//! `resync_to_next_magic` per window, the same way the archive's own pre-2A
//! `resync_to_next_magic_streamed` already did -- the byte-search algorithm was already shared;
//! only the window I/O around it is unified here.

use prikk_error::Result;

use super::frame_resync::resync_to_next_magic;

/// One window's worth of bytes read by the default, windowed [`FrameSource::scan_for_magic`] --
/// large enough that a sound frame is found in one window the overwhelming majority of the time,
/// small enough that a streamed source never buffers more than this much at once.
const DEFAULT_SCAN_WINDOW_BYTES: usize = 1024 * 1024;

/// A position-addressable byte source a framed decoder reads from: a slice already resident in
/// memory ([`SliceFrameSource`]), or a `Read + Seek` window over an archive section (RFC 155 §9.4,
/// wired in 0.52.0 step 2 Part 2B). Every method takes `&mut self` even though the slice
/// implementation never mutates anything through it -- a streamed implementation needs to `seek`
/// its own cursor and fill its own scratch buffer, and one shared signature is what lets both
/// implementations satisfy the same generic decoder.
pub(crate) trait FrameSource {
    /// This source's total length in bytes -- known up front for both implementations (a slice's
    /// own `len()`, or an archive section's own manifest-recorded length), never discovered by
    /// reading to the end.
    fn total_len(&self) -> usize;

    /// Borrow exactly `len` bytes starting at `offset`, or `Ok(None)` when fewer than `len` bytes
    /// remain from `offset` -- the same short-read signal `bytes.get(offset..offset+len)` already
    /// gives the slice path today, so a caller's "not enough bytes left" branch needs no change
    /// moving from one implementation to the other. `Err` only for a genuine I/O failure on a
    /// streamed source; the slice path can never produce one.
    ///
    /// **Never allocates `len` bytes before confirming they exist.** A streamed implementation
    /// must size its own scratch buffer from bytes actually read (or from a length this source's
    /// own caller has independently bounded, e.g. DC-86's per-object cap on an untrusted archive),
    /// never from an unvalidated header field alone -- the hostile-length suite (`test_gates::
    /// hostile_lengths`, P4) demands exactly this of every framed reader, and would catch a
    /// regression here.
    fn read_at(&mut self, offset: usize, len: usize) -> Result<Option<&[u8]>>;

    /// Find the next occurrence of `magic` at or after `start`, or `Ok(None)` once fewer than
    /// `magic.len()` bytes remain anywhere in the source. The default implementation windows
    /// through [`read_at`](Self::read_at) in [`DEFAULT_SCAN_WINDOW_BYTES`]-sized, overlapping
    /// chunks (the archive's own pre-2A `resync_to_next_magic_streamed` shape), calling the real
    /// `resync_to_next_magic` on each window unchanged. [`SliceFrameSource`] overrides this to call
    /// `resync_to_next_magic` once, over the whole remaining slice, with no windowing loop at all
    /// -- the store's own commit-path cost this step must not add to.
    fn scan_for_magic(&mut self, start: usize, magic: &[u8]) -> Result<Option<usize>> {
        default_scan_for_magic(self, start, magic)
    }
}

/// [`FrameSource::scan_for_magic`]'s default, windowed body -- a free function (not a method with a
/// default body calling itself) so [`SliceFrameSource`]'s own override can still reach it if a
/// future caller ever wants the windowed behavior deliberately, without a name collision.
fn default_scan_for_magic<S: FrameSource + ?Sized>(
    source: &mut S,
    start: usize,
    magic: &[u8],
) -> Result<Option<usize>> {
    let total_len = source.total_len();
    let overlap = magic.len().saturating_sub(1);
    let mut window_start = start;
    loop {
        if window_start >= total_len {
            return Ok(None);
        }
        let remaining = total_len - window_start;
        let read_len = remaining.min(DEFAULT_SCAN_WINDOW_BYTES);
        if read_len < magic.len() {
            return Ok(None);
        }
        let Some(window) = source.read_at(window_start, read_len)? else {
            return Ok(None);
        };
        if let Some(found) = resync_to_next_magic(window, 0, magic) {
            return Ok(Some(window_start + found));
        }
        let advance = read_len.saturating_sub(overlap);
        if advance == 0 {
            return Ok(None);
        }
        window_start += advance;
    }
}

/// A [`FrameSource`] over bytes already resident in memory -- the store's own container reads,
/// zero-copy: every [`read_at`](FrameSource::read_at) is a direct `bytes.get(range)`, and
/// [`scan_for_magic`](FrameSource::scan_for_magic) is overridden to scan the whole remaining slice
/// in one real `resync_to_next_magic` call, with no windowing loop (the default body's own loop
/// overhead, trivial as it is, is still overhead this format's commit-path reader must not pay).
pub(crate) struct SliceFrameSource<'a> {
    bytes: &'a [u8],
}

impl<'a> SliceFrameSource<'a> {
    pub(crate) fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }
}

impl FrameSource for SliceFrameSource<'_> {
    fn total_len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&mut self, offset: usize, len: usize) -> Result<Option<&[u8]>> {
        let Some(end) = offset.checked_add(len) else {
            return Ok(None);
        };
        Ok(self.bytes.get(offset..end))
    }

    fn scan_for_magic(&mut self, start: usize, magic: &[u8]) -> Result<Option<usize>> {
        Ok(resync_to_next_magic(self.bytes, start, magic))
    }
}

#[cfg(test)]
mod tests;
