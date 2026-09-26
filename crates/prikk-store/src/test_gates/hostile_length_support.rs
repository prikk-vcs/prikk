//! Shared plumbing for RFC 160 P4's hostile-length damage suite (`test_gates/hostile_lengths.rs`, `refs/tests/hostile_lengths.rs`).
//!
//! **What a hostile length is.** Every on-disk record format here carries a length or a count read from the file itself: a frame's
//! `body_len`, a body's own string / list / payload length. Local files are not trusted input for a *size*: a damaged (or written)
//! record can say 2^62, and a reader that sizes a buffer from it does not return an error -- allocation failure **aborts**. So for
//! each format the suite sets the field to its maximum and demands that the decode is **refused** (no record decoded from it; an
//! error, a failed outcome, or a torn-tail remainder) and that the process is still alive to say so.
//!
//! **A matching checksum, where the format has one.** A frame whose header claims more bytes than remain is a torn tail and is never
//! checksummed, so the interesting hostile input for a body-internal field is a frame whose body carries the maximal field *and* whose
//! checksum matches -- the checksums are unkeyed, so anyone who can write the repository's files (or a faulty writer) can produce one.
//! [`frame_with_body`] rebuilds a record around a new body exactly as the format's own writer frames one, and each format's case first
//! checks that it reproduces the format's own framing byte for byte, so a case built with it tests the length and not the checksum.
//!
//! **Cases that could abort run in a child process** ([`isolated`]) with an address-space limit (Linux), so a regression fails one
//! test naming the case instead of killing the suite -- and the limit makes "asked for far more than the machine has" deterministic.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use std::process::Command;

/// What a decode of hostile bytes did, in the terms every format's replay can be reduced to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seen {
    /// Records decoded and accepted.
    pub(crate) records: usize,
    /// Bytes set aside as an incomplete final frame (a torn tail).
    pub(crate) trailing_partial_bytes: usize,
    /// Frames the decode reported as failed.
    pub(crate) failed: usize,
}

impl Seen {
    /// The decode **refused** the hostile input: nothing was accepted from it, and it was not silently ignored either.
    pub(crate) fn is_refusal(&self) -> bool {
        self.records == 0 && (self.trailing_partial_bytes > 0 || self.failed > 0)
    }
}

/// A `Result`-returning decode reduced to a refusal: an `Err` is one; an `Ok` must be a [`Seen`] refusal.
pub(crate) fn assert_refused<E: std::fmt::Debug>(label: &str, decoded: Result<Seen, E>) {
    match decoded {
        Err(_) => {}
        Ok(seen) => assert!(
            seen.is_refusal(),
            "{label}: a record claiming a hostile length was not refused: {seen:?}"
        ),
    }
}

/// Rebuild a record around `body` **with a matching checksum**, in the shape every frame format shares: `magic(8) version(2)
/// pre_len_fields length(8) checksum(32) body`, the checksum being `sha256(magic version pre_len_fields length body)`. `valid` is a
/// record of the format (its magic, version and pre-length fields are copied); `pre` is how many bytes sit between the version and the
/// length (0 for most, 8 for a WAL record's sequence, 32 for a ref container's ref key).
pub(crate) fn frame_with_body(valid: &[u8], pre: usize, body: &[u8]) -> Vec<u8> {
    let header = &valid[..10 + pre];
    let body_len = u64::try_from(body.len()).unwrap_or(u64::MAX).to_be_bytes();
    let mut preimage = header.to_vec();
    preimage.extend_from_slice(&body_len);
    preimage.extend_from_slice(body);
    let checksum = prikk_hash::sha256(&preimage);
    let mut out = header.to_vec();
    out.extend_from_slice(&body_len);
    out.extend_from_slice(&checksum);
    out.extend_from_slice(body);
    out
}

/// The body of a frame in the shared shape (see [`frame_with_body`]).
pub(crate) fn body_of(valid: &[u8], pre: usize) -> &[u8] {
    &valid[10 + pre + 8 + 32..]
}

/// `valid` with its length field set to `claimed` (the checksum left as it was: a frame claiming more bytes than remain is never
/// checksummed).
pub(crate) fn claiming(valid: &[u8], pre: usize, claimed: u64) -> Vec<u8> {
    let mut bytes = valid.to_vec();
    bytes[10 + pre..10 + pre + 8].copy_from_slice(&claimed.to_be_bytes());
    bytes
}

/// The lengths every frame's header is set to claim: the maximum, the edge where `header + length` overflows `usize`, and the
/// large round ones.
pub(crate) const CLAIMS: &[u64] = &[u64::MAX, u64::MAX - 49, 1 << 63, 1 << 62, 1 << 40, 1 << 32];

/// Run `body` in a child process (this test binary, re-executed on this one test) with an address-space limit, and fail with the
/// child's status if it did not exit cleanly. `case` is the test's own name within its module; `module` is `module_path!()`.
///
/// A regression that allocates the claimed size makes the **child** abort (SIGABRT) and this test fail naming the case; it never
/// takes the suite with it. On non-Linux targets there is no limit, so an allocation the machine can satisfy lazily does not
/// abort there: the decode's *outcome* is still asserted, and the abort detection is the Linux run's.
pub(crate) fn isolated(module: &str, case: &str, body: fn()) {
    if std::env::var("PRIKK_HOSTILE_CHILD").as_deref() == Ok(case) {
        limit_address_space();
        body();
        return;
    }
    let path = format!(
        "{}::{case}",
        module.strip_prefix("prikk_store::").unwrap_or(module)
    );
    let output = Command::new(std::env::current_exe().expect("this test binary"))
        .args(["--exact", &path, "--nocapture", "--test-threads=1"])
        .env("PRIKK_HOSTILE_CHILD", case)
        .output()
        .expect("spawning the child test process");
    assert!(
        output.status.success(),
        "{case}: the child process did not exit cleanly ({:?}) -- a hostile length was allocated or panicked\nstdout: {}\nstderr: {}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    // The child ran the case (a filter that matched nothing also "succeeds").
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "{case}: the child ran no test: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

#[cfg(target_os = "linux")]
fn limit_address_space() {
    use rustix::process::{Resource, Rlimit, setrlimit};
    // 4 GiB of address space: far above what a decode of a 100-byte record needs, far below what a claimed 2^32 elements or 2^62
    // bytes would ask for, so the claim fails to allocate deterministically instead of depending on the host's memory.
    let limit = Some(4 * 1024 * 1024 * 1024);
    let _ = setrlimit(
        Resource::As,
        Rlimit {
            current: limit,
            maximum: limit,
        },
    );
}

#[cfg(not(target_os = "linux"))]
fn limit_address_space() {}

/// Declare an isolated hostile-length test: `hostile_case!(name, function)`.
macro_rules! hostile_case {
    ($name:ident, $body:path) => {
        #[test]
        fn $name() {
            $crate::test_gates::hostile_length_support::isolated(
                module_path!(),
                stringify!($name),
                $body,
            );
        }
    };
}
pub(crate) use hostile_case;
