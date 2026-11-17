//! The seekable, framed input both streaming importers read: `bundle import`/`preview`/`verify` and `sync accept` (0.58.0 P3).
//!
//! **Hoisted, not rewritten** out of `bundle.rs` (0.58.0 P3b, ruling 4): the source type and the small readers a length-prefixed
//! frame format needs -- big-endian `u64`, a length-prefixed byte string bounded by a caller's maximum, one frame body whose declared
//! length is checked against the per-object bound *before* it is read, and the seek-only walk over a section's frames. Both formats
//! lay their objects out as `u64 length || envelope file`, and both put the author keys *after* the frames they are needed for, so
//! both reach them the same way (`skip_object_frames`) and then seek back. One reader, so the two importers cannot drift apart on
//! what a frame is.

use std::io::{Read, Seek, SeekFrom};

use prikk_error::{PrikkError, Result};

use crate::foundation::fsutil::len_to_u64;

/// 0.55.0 step 3 Part 3b-i2 (Q2): what `bundle import` reads from -- an open regular file's own
/// handle, seekable in place, or (for a non-regular input: a FIFO, a pipe) the existing fully
/// buffered fallback wrapped in a `Cursor` so both branches satisfy the identical `Read + Seek`
/// interface pass 1 and pass 2 both need. Built by `prikk-cli`'s `bounded_read` module (which
/// decides, from the open handle's own metadata, which variant to construct) -- `prikk-store` has
/// no notion of a CLI bound or a path, and only ever reads from whichever variant it is handed.
///
/// Release 0.55.0 prep: deliberately not `#[non_exhaustive]`. A library caller of `import_bundle`/
/// `verify_bundle`/`preview_bundle` needs to construct a variant directly -- its own open `File`,
/// or its own buffered bytes -- and this crate provides no separate constructor function for
/// either; `#[non_exhaustive]` would block exactly that construction. The two variants are also a
/// complete classification of what a `Read + Seek` source over a bundle can be (seekable, or not);
/// a third kind would be a visible, deliberate addition worth its own match-arm update at every
/// call site, not something to hide behind a forward-compatibility attribute.
pub enum BundleSource {
    /// A regular file, already opened by the caller. Pass 1 still enforces the real per-object and
    /// per-import bounds against the bytes it actually reads, independently of any declared size
    /// (RFC 158 Stage A's own "metadata can lie" rule) -- this variant carries no promise beyond
    /// "seekable."
    File(std::fs::File),
    /// A non-regular input, already read fully into memory and bounded by the caller -- wrapped so
    /// it exposes the identical `Read + Seek` surface the file-backed variant does.
    Buffered(std::io::Cursor<Vec<u8>>),
}

impl Read for BundleSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Self::File(file) => file.read(buf),
            Self::Buffered(cursor) => cursor.read(buf),
        }
    }
}

impl Seek for BundleSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        match self {
            Self::File(file) => file.seek(pos),
            Self::Buffered(cursor) => cursor.seek(pos),
        }
    }
}

pub(crate) fn u64_to_usize(value: u64) -> Result<usize> {
    usize::try_from(value)
        .map_err(|_| PrikkError::MalformedData("u64 length does not fit usize".to_string()))
}

pub(crate) fn stream_position_usize(source: &mut BundleSource) -> Result<usize> {
    let position = source.stream_position().map_err(|err| {
        PrikkError::Integrity(format!(
            "bundle import: failed to read the input's current position: {err}"
        ))
    })?;
    u64_to_usize(position)
}

pub(crate) fn read_exact_from(source: &mut BundleSource, len: usize) -> Result<Vec<u8>> {
    let mut buf = vec![0_u8; len];
    source
        .read_exact(&mut buf)
        .map_err(|err| PrikkError::MalformedData(format!("unexpected end of bundle: {err}")))?;
    Ok(buf)
}

pub(crate) fn read_array_from<const N: usize>(source: &mut BundleSource) -> Result<[u8; N]> {
    let bytes = read_exact_from(source, N)?;
    let mut out = [0_u8; N];
    out.copy_from_slice(&bytes);
    Ok(out)
}

pub(crate) fn read_u64_from(source: &mut BundleSource) -> Result<u64> {
    Ok(u64::from_be_bytes(read_array_from::<8>(source)?))
}

/// A u64-length-prefixed byte string, read from the stream -- the streaming counterpart of
/// `ByteCursor::read_bytes_u64`, bounded by `max_len` (the bundle's own declared total size: no
/// legitimate ref name, author key id, or manifest string can exceed the bundle that carries it).
pub(crate) fn read_bytes_u64_from(source: &mut BundleSource, max_len: usize) -> Result<Vec<u8>> {
    let len = read_u64_from(source)?;
    if len > len_to_u64(max_len)? {
        return Err(PrikkError::MalformedData(format!(
            "bundle declares a {len}-byte field, over the bundle's own declared total size"
        )));
    }
    read_exact_from(source, u64_to_usize(len)?)
}

/// One object frame's declared length and body, read separately -- 0.55.0 step 3 Part 3b-ii2
/// (N1): pass 1 used to concatenate the 8-byte length prefix and the body into one combined
/// `frame` buffer solely so R3's digest could be computed over "the length-prefixed frame exactly
/// as read" in a single `sha256` call. `sha256_parts` already exists for exactly this split --
/// "a checksum covers a header and a body that are held apart" -- so this reads only `body`
/// (never a second, prefix-prepended copy of it), and the caller hashes the prefix and body as
/// two parts instead. Refuses on the length prefix, before the body is read, exactly as before.
pub(crate) fn read_frame_body_from(
    source: &mut BundleSource,
    max_object_bytes: usize,
) -> Result<(u64, Vec<u8>)> {
    let declared_bytes = read_u64_from(source)?;
    let bound_bytes = len_to_u64(max_object_bytes)?;
    if declared_bytes > bound_bytes {
        return Err(PrikkError::ObjectOverBound {
            declared_bytes,
            bound_bytes,
        });
    }
    let body = read_exact_from(source, u64_to_usize(declared_bytes)?)?;
    Ok((declared_bytes, body))
}

/// 0.55.0 step 3 Part 3b-i2: the author-key section comes *after* every object frame in the wire
/// format (`encode_bundle`'s own section order), but RFC 156 §4 admission needs that section's own
/// transported material available *while* streaming the object frames, to verify a signature a
/// newly-carried key_id introduces. Skipped here without reading a single frame's body: each
/// frame's own length prefix says exactly how far to seek forward, so this walk costs `count` seeks
/// and transfers no object content -- landing exactly at the author-key section's own first byte.
pub(crate) fn skip_object_frames(
    source: &mut BundleSource,
    count: u64,
    max_object_bytes: usize,
) -> Result<()> {
    let bound_bytes = len_to_u64(max_object_bytes)?;
    for _ in 0..count {
        let declared_bytes = read_u64_from(source)?;
        if declared_bytes > bound_bytes {
            return Err(PrikkError::ObjectOverBound {
                declared_bytes,
                bound_bytes,
            });
        }
        let offset = i64::try_from(declared_bytes).map_err(|_| {
            PrikkError::MalformedData("u64 length does not fit a seek offset".to_string())
        })?;
        source.seek(SeekFrom::Current(offset)).map_err(|err| {
            PrikkError::Integrity(format!(
                "bundle import: failed to skip an object frame: {err}"
            ))
        })?;
    }
    Ok(())
}
