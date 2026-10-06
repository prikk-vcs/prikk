#![forbid(unsafe_code)]
#![warn(missing_docs)]

//! Hash primitives used by Prikk.
//!
//! `sha256` runs on the audited `sha2` crate. Prikk originally shipped a first-party SHA-256
//! implementation for the initial source drop; DC-50 concluded the ROI no longer favoured
//! maintaining it (identity-bearing maintenance cost, a measured ~5.8x throughput gap, and no
//! remaining supply-chain benefit once `sha2` was already trusted for Ed25519 signing via
//! `ed25519-dalek`), and DC-55 performed the swap after an equivalence campaign proved it changed
//! no existing identity. The outgoing implementation is retained test-only, frozen, as an
//! independent differential reference — see `tests::frozen_outgoing`.
//!
//! `#![forbid(unsafe_code)]` remains true of this crate's own source, but hashing now happens
//! inside `sha2`, whose accelerated backends use `unsafe` internally for CPU-specific
//! instructions. This crate no longer provides an unsafe-free guarantee for hashing by itself.

use sha2::{Digest, Sha256};

/// A 32-byte SHA-256 digest.
pub type Sha256Digest = [u8; 32];

/// Compute SHA-256 for a byte slice.
#[must_use]
pub fn sha256(input: &[u8]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    hasher.update(input);
    hasher.finalize().into()
}

/// SHA-256 over several byte slices in order, without concatenating them: the same digest as [`sha256`] of their
/// concatenation. Used where a checksum covers a header and a body that are held apart, so the body is hashed in place.
#[must_use]
pub fn sha256_parts(parts: &[&[u8]]) -> Sha256Digest {
    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// Convert bytes to lowercase hex.
#[must_use]
pub fn to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(hex_char(byte >> 4));
        out.push(hex_char(byte & 0x0f));
    }
    out
}

fn hex_char(value: u8) -> char {
    match value {
        0..=9 => char::from(b'0' + value),
        10..=15 => char::from(b'a' + (value - 10)),
        _ => '?',
    }
}

#[cfg(test)]
mod tests;
