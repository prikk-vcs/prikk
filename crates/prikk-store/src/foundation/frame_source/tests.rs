#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use super::*;

#[test]
fn slice_source_reads_in_range_and_refuses_out_of_range() {
    let bytes = b"0123456789";
    let mut source = SliceFrameSource::new(bytes);
    assert_eq!(source.total_len(), 10);
    assert_eq!(source.read_at(0, 4).unwrap(), Some(&b"0123"[..]));
    assert_eq!(source.read_at(6, 4).unwrap(), Some(&b"6789"[..]));
    assert_eq!(
        source.read_at(7, 4).unwrap(),
        None,
        "fewer than 4 bytes remain from 7"
    );
    assert_eq!(
        source.read_at(10, 1).unwrap(),
        None,
        "nothing remains at the very end"
    );
    assert_eq!(
        source.read_at(usize::MAX, 1).unwrap(),
        None,
        "an offset+len overflow must refuse, not panic or wrap"
    );
}

#[test]
fn slice_source_scan_for_magic_finds_every_occurrence_without_windowing() {
    let bytes = b"xxMAGICyyyMAGICz";
    let mut source = SliceFrameSource::new(bytes);
    assert_eq!(source.scan_for_magic(0, b"MAGIC").unwrap(), Some(2));
    assert_eq!(source.scan_for_magic(3, b"MAGIC").unwrap(), Some(10));
    assert_eq!(source.scan_for_magic(11, b"MAGIC").unwrap(), None);
}

/// A minimal, test-only [`FrameSource`] that never overrides [`FrameSource::scan_for_magic`], so a
/// call into it always exercises the default windowed body -- read a small window size via a
/// forced-small override is not possible without changing the production constant, so this test
/// instead confirms correctness across a magic that straddles a window boundary at the production
/// window size by using a byte string many times that size with the real `scan_for_magic` default.
struct DefaultOnlySource<'a> {
    bytes: &'a [u8],
}

impl FrameSource for DefaultOnlySource<'_> {
    fn total_len(&self) -> usize {
        self.bytes.len()
    }

    fn read_at(&mut self, offset: usize, len: usize) -> Result<Option<&[u8]>> {
        let Some(end) = offset.checked_add(len) else {
            return Ok(None);
        };
        Ok(self.bytes.get(offset..end))
    }
    // Deliberately does not override `scan_for_magic` -- exercises the trait's default body.
}

#[test]
fn default_windowed_scan_agrees_with_the_slice_override_on_a_magic_straddling_a_window_boundary() {
    // One window's worth of filler, then the magic split across where a window edge would fall if
    // the implementation failed to overlap windows by `magic.len() - 1`.
    let window = DEFAULT_SCAN_WINDOW_BYTES;
    let magic = b"PRIKKMAGIC";
    let mut bytes = vec![0u8; window - (magic.len() / 2)];
    bytes.extend_from_slice(magic);
    bytes.extend_from_slice(&[0u8; 64]);

    let mut default_source = DefaultOnlySource { bytes: &bytes };
    let mut slice_source = SliceFrameSource::new(&bytes);

    let via_default = default_source.scan_for_magic(0, magic).unwrap();
    let via_slice_override = slice_source.scan_for_magic(0, magic).unwrap();
    assert_eq!(via_default, via_slice_override);
    assert_eq!(via_default, Some(window - magic.len() / 2));
}

#[test]
fn default_windowed_scan_returns_none_past_the_end() {
    let bytes = vec![0u8; 10];
    let mut source = DefaultOnlySource { bytes: &bytes };
    assert_eq!(source.scan_for_magic(0, b"NOPE").unwrap(), None);
    assert_eq!(source.scan_for_magic(20, b"X").unwrap(), None);
}
