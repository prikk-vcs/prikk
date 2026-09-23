//! RFC 158 Stage A handoff §6 control 2: the shared reader itself, unit-tested over an in-memory
//! `Read` that counts bytes -- never by running a real `prikk` subprocess.

#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use super::*;

/// How much a [`CountingSource`] is prepared to supply before it ends. Far more than any bound
/// these tests use, so a reader that does not stop at `bound + 1` reads all of it -- but finite, so
/// that removing the streaming bound makes the assertion below *fail* rather than hang forever
/// (a control that goes red by never returning is a control that stalls CI, not one that reports).
const SOURCE_TOTAL: usize = 64 * 1024;

/// A `Read` that counts how many bytes it has actually yielded, standing in for a source that would
/// happily supply far more than any declared size claims -- exactly the "declared size lies" shape
/// control 2 exists for -- and then ends.
struct CountingSource {
    yielded: usize,
}

impl Read for CountingSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = buf.len().min(SOURCE_TOTAL - self.yielded);
        for byte in &mut buf[..n] {
            *byte = 0;
        }
        self.yielded += n;
        Ok(n)
    }
}

/// A declared size over the bound refuses before a single byte is read.
#[test]
fn a_declared_size_over_the_bound_reads_zero_bytes() {
    let mut source = CountingSource { yielded: 0 };
    let bound = SizeBound::fixed(1024, "a test artifact");
    let result = read_bounded("test artifact", "test", 1025, &mut source, &bound);
    assert!(
        result.is_err(),
        "a declared size one over the bound must refuse"
    );
    assert_eq!(
        source.yielded, 0,
        "refusing on the declared size must not touch the source at all"
    );
}

/// A declared size exactly at the bound is not refused on the declared-size check -- it is only
/// refused later if the source actually yields more than the bound while streaming.
#[test]
fn a_declared_size_exactly_at_the_bound_is_not_refused_on_declaration_alone() {
    struct ExactSource {
        remaining: usize,
    }
    impl Read for ExactSource {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.remaining);
            for byte in &mut buf[..n] {
                *byte = 0;
            }
            self.remaining -= n;
            Ok(n)
        }
    }
    let mut source = ExactSource { remaining: 4 };
    let bound = SizeBound::fixed(4, "a test artifact");
    let result = read_bounded("test artifact", "test", 4, &mut source, &bound);
    assert!(
        result.is_ok(),
        "a declared size exactly at the bound, matched by what the source yields, must not refuse"
    );
    assert_eq!(result.unwrap().len(), 4);
}

/// A source that yields more than its declared size is refused at `bound + 1` -- the streaming
/// check catches a lying declaration without ever reading the source's true, unbounded length.
#[test]
fn a_source_yielding_more_than_its_declared_size_is_refused_at_bound_plus_one() {
    let mut source = CountingSource { yielded: 0 };
    let bound = SizeBound::fixed(1024, "a test artifact");
    // Declares exactly the bound (a lie -- the source below has 64 KiB to give), so the
    // declared-size check passes and only the streaming check can catch it.
    let result = read_bounded("test artifact", "test", 1024, &mut source, &bound);
    assert!(
        result.is_err(),
        "a source that keeps yielding bytes past its declared size must be refused"
    );
    assert_eq!(
        source.yielded, 1025,
        "the reader must stop at exactly bound + 1 bytes, never reading the source's true \
         length"
    );
}

/// Addendum 1 item 1: a bound of `u64::MAX` -- 0.46.0's way to say "no practical limit" -- must read
/// a source that fits it. `bound + 1` overflowed: a panic in a debug build (where tests run), a wrap
/// to `take(0)` and an empty read in a release one. *Perturbed by hand: `saturating_add(1)` back to
/// `+ 1` panics here with "attempt to add with overflow" -- confirmed and reverted, see the report.*
#[test]
fn a_bound_of_u64_max_reads_a_source_that_fits_it() {
    let mut source = CountingSource { yielded: 0 };
    let bound = SizeBound::fixed(usize::MAX, "a test artifact");
    let result = read_bounded("test artifact", "test", 10, &mut source, &bound);
    assert_eq!(result.unwrap().len(), SOURCE_TOTAL);
}
