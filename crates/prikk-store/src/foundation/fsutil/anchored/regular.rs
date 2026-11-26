//! Nonblocking final-entry opens and same-handle validation.

use std::path::Path;

use prikk_error::{PrikkError, Result};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::{failpoints, io_error};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fd::{AsFd, OwnedFd};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use rustix::fs::{self, FileType, Mode, OFlags};

/// Platform-neutral path decomposition, shared by every backend that resolves a relative mutation
/// path one component at a time (Unix's fd-anchored walk, Windows' path-anchored walk).
pub(super) fn required_parent(path: &Path) -> Result<&Path> {
    path.parent().ok_or_else(|| PrikkError::Io {
        kind: None,
        context: "relative mutation path has no parent".to_string(),
    })
}

/// See `required_parent`.
pub(super) fn required_file_name(path: &Path) -> Result<&std::ffi::OsStr> {
    path.file_name().ok_or_else(|| PrikkError::Io {
        kind: None,
        context: "relative mutation path has no file name".to_string(),
    })
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_new_regular(
    directory: impl AsFd,
    name: &std::ffi::OsStr,
) -> rustix::io::Result<OwnedFd> {
    failpoints::required_open().map_err(|_| rustix::io::Errno::IO)?;
    fs::openat(
        directory,
        name,
        OFlags::WRONLY
            | OFlags::CREATE
            | OFlags::EXCL
            | OFlags::NONBLOCK
            | OFlags::NOFOLLOW
            | OFlags::CLOEXEC,
        Mode::from_raw_mode(0o600),
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_existing_regular(
    directory: impl AsFd,
    name: &std::ffi::OsStr,
    flags: OFlags,
) -> Result<OwnedFd> {
    failpoints::required_open()?;
    let fd = fs::openat(
        directory,
        name,
        flags | OFlags::NONBLOCK | OFlags::NOFOLLOW | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(io_error)?;
    validate_regular(&fd)?;
    Ok(fd)
}

/// RFC 102 Stage 5, design-v1.md §14.3/§14.5: requires an existing file rather than falling back to
/// creation on `EEXIST`, matching `open_existing_regular`'s own use by `durable_truncate`
/// (`anchored/linux.rs:63-64`). A create-on-append is a new-directory-entry event on every append --
/// harmless on Linux (a wasted `EEXIST` syscall) but the exact gap this RFC exists to close on
/// Windows, and it silently repaired an interrupted `init` (RFC's own §14.2 finding) into an
/// undetectable one. Safe with zero behavior change on every path that exists today: every production
/// `append_file_required` caller targets a name `RepositoryLayout::init` already created via
/// `create_empty_file_once`, which routes through `create_new_file_required` -- a separate primitive
/// that never calls this function -- so nothing here has ever depended on the fallback for
/// correctness.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn open_append_regular(directory: impl AsFd, name: &std::ffi::OsStr) -> Result<OwnedFd> {
    open_existing_regular(directory, name, OFlags::WRONLY | OFlags::APPEND)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn validate_regular(fd: impl AsFd) -> Result<()> {
    let stat = fs::fstat(fd).map_err(io_error)?;
    if FileType::from_raw_mode(stat.st_mode).is_file() {
        return Ok(());
    }
    Err(PrikkError::Integrity(
        "mutation target is not a regular file".to_string(),
    ))
}

/// Append `head` then `body` to `file` (opened `O_APPEND`) with `writev`, so the two pieces are one write and neither is copied into
/// a buffer with the other. **The same bytes reach the file as `write_all` of `head` then `body` concatenated**; only the copy is gone
/// (0.59.0 P5b1a). See [`append_pieces`] for the loop.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(super) fn append_all_vectored(file: &std::fs::File, head: &[u8], body: &[u8]) -> Result<()> {
    append_pieces(head, body, |pieces| rustix::io::writev(file, pieces))
}

/// The loop of [`append_all_vectored`], over any `write` that takes the two pieces as `writev` does and says how many bytes it took.
/// **Loops on a short write as `write_all` does:** after a partial write what is left of the two pieces is written, in order (a piece
/// the write finished is dropped, a piece it half-finished is cut at the byte it reached); an interrupted call is retried; a call that
/// takes nothing from a non-empty record is an error, as `write_all`'s is. Separate from the descriptor so a test can make every
/// write short and can stop the write at any byte.
#[cfg(any(target_os = "linux", target_os = "macos"))]
pub(crate) fn append_pieces(
    head: &[u8],
    body: &[u8],
    mut write: impl FnMut(&[std::io::IoSlice<'_>]) -> rustix::io::Result<usize>,
) -> Result<()> {
    use std::io::IoSlice;
    let mut head = head;
    let mut body = body;
    while !head.is_empty() || !body.is_empty() {
        match write(&[IoSlice::new(head), IoSlice::new(body)]) {
            Ok(0) => {
                return Err(PrikkError::from(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "failed to write whole buffer",
                )));
            }
            Ok(written) => {
                let from_head = written.min(head.len());
                head = head.get(from_head..).unwrap_or_default();
                body = body.get(written - from_head..).unwrap_or_default();
            }
            Err(rustix::io::Errno::INTR) => {}
            Err(error) => return Err(io_error(error)),
        }
    }
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
#[allow(clippy::expect_used, clippy::indexing_slicing)]
mod tests {
    use super::append_pieces;
    use rustix::io::Errno;
    use std::cell::RefCell;

    const HEAD: &[u8] = b"HEADER-0123456789";
    const BODY: &[u8] = b"the body of the record, longer than the header";

    /// A `write` that takes at most `most` bytes a call, from the two pieces in order, as a short `writev` does.
    fn sink(
        most: usize,
        out: &RefCell<Vec<u8>>,
    ) -> impl FnMut(&[std::io::IoSlice<'_>]) -> rustix::io::Result<usize> + '_ {
        move |pieces| {
            let mut taken = 0;
            for piece in pieces {
                let room = most - taken;
                let n = piece.len().min(room);
                out.borrow_mut().extend_from_slice(&piece[..n]);
                taken += n;
                if n < piece.len() {
                    break;
                }
            }
            Ok(taken)
        }
    }

    fn whole(head: &[u8], body: &[u8]) -> Vec<u8> {
        [head, body].concat()
    }

    /// Every size of short write, from one byte a call up past the whole record, delivers the bytes of `head` then `body`, each once.
    /// **Perturb:** the body offset after a partial write taken from the call's own count (not the bytes past the header) puts bytes
    /// twice or out of order here.
    #[test]
    fn short_writes_of_every_size_deliver_the_two_pieces_whole_and_in_order() {
        for (head, body) in [(HEAD, BODY), (b"".as_slice(), BODY), (HEAD, b"".as_slice())] {
            for most in 1..=(head.len() + body.len() + 2) {
                let out = RefCell::new(Vec::new());
                append_pieces(head, body, sink(most, &out)).expect("the append");
                assert_eq!(out.into_inner(), whole(head, body), "{most} bytes a call");
            }
        }
    }

    /// A write that stops at byte `c` and then fails leaves the first `c` bytes of the record and no others: a torn tail is a **prefix**,
    /// at every cut, including the cut at the join of the two pieces.
    #[test]
    fn a_write_stopped_at_any_byte_leaves_exactly_the_prefix() {
        let record = whole(HEAD, BODY);
        for cut in 0..=record.len() {
            for most in [1, 5, record.len()] {
                let out = RefCell::new(Vec::new());
                let mut inner = sink(most, &out);
                let mut taken = 0;
                let result = append_pieces(HEAD, BODY, |pieces| {
                    let allowed = cut - taken;
                    if allowed == 0 {
                        return Err(Errno::IO);
                    }
                    // The cut: hand `inner` only as many bytes as remain before it.
                    let mut room = allowed;
                    let mut limited = Vec::new();
                    for piece in pieces {
                        let n = piece.len().min(room);
                        limited.push(&piece[..n]);
                        room -= n;
                    }
                    let slices: Vec<std::io::IoSlice<'_>> = limited
                        .iter()
                        .map(|part| std::io::IoSlice::new(part))
                        .collect();
                    let n = inner(&slices)?;
                    taken += n;
                    Ok(n)
                });
                assert_eq!(
                    result.is_ok(),
                    cut == record.len(),
                    "cut {cut}, {most} a call"
                );
                drop(inner);
                assert_eq!(out.into_inner(), record[..cut], "cut {cut}, {most} a call");
            }
        }
    }

    /// An interrupted call (`EINTR`) is retried and writes nothing; a call that takes nothing from a record that is not empty is an
    /// error, as `write_all`'s is, and is not a loop.
    #[test]
    fn an_interrupted_call_is_retried_and_a_zero_write_is_an_error() {
        let out = RefCell::new(Vec::new());
        let mut interrupts = 3;
        let mut inner = sink(4, &out);
        append_pieces(HEAD, BODY, |pieces| {
            if interrupts > 0 {
                interrupts -= 1;
                return Err(Errno::INTR);
            }
            inner(pieces)
        })
        .expect("the append");
        drop(inner);
        assert_eq!(out.into_inner(), whole(HEAD, BODY));

        let error = append_pieces(HEAD, BODY, |_| Ok(0)).expect_err("a zero write is an error");
        assert!(
            error.to_string().contains("failed to write whole buffer"),
            "{error}"
        );
        // An empty record takes no call at all.
        append_pieces(b"", b"", |_| panic!("no write for an empty record"))
            .expect("an empty record");
    }
}
