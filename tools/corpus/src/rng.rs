//! A deliberate duplicate of the `SplitMix64` generator in
//! `crates/prikk-cli/tests/dc59_commit_benchmark.rs` and `dc92_lineage_replay_benchmark.rs`, which is
//! itself a deliberate duplicate of the one in `crates/prikk-hash/src/tests/hash_differential.rs`
//! (RFC 139 §5's own precedent: "the existing harnesses already do this ... documents why the copy
//! exists," rather than inventing a third/fourth generator). None of those are reachable from this
//! crate -- two are private test modules of other crates, the third is `#[cfg(test)]`-only source in
//! a different crate entirely -- so this is a fourth independent copy of reviewed material, not a new
//! invented one.

/// A small, fast, non-cryptographic PRNG. Used only to make corpus content and the planner's
/// sampling decisions reproducible from a recorded seed (RFC 139 §5) -- never for anything
/// security-sensitive.
pub struct SplitMix64(u64);

impl SplitMix64 {
    /// Seed a new generator.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    /// Draw the next pseudo-random `u64`.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Fill `buf` with pseudo-random bytes.
    pub fn fill_bytes(&mut self, buf: &mut [u8]) {
        let mut chunks = buf.chunks_exact_mut(8);
        for chunk in &mut chunks {
            chunk.copy_from_slice(&self.next_u64().to_le_bytes());
        }
        let remainder = chunks.into_remainder();
        if !remainder.is_empty() {
            let extra = self.next_u64().to_le_bytes();
            for (dst, byte) in remainder.iter_mut().zip(extra) {
                *dst = byte;
            }
        }
    }

    /// Draw a value in `0..bound` without modulo bias analysis -- this generator is a measurement
    /// tool, not a cryptographic or fairness-critical one, so the small modulo bias of `next_u64() %
    /// bound` is accepted deliberately rather than implementing rejection sampling. `bound == 0`
    /// returns `0` rather than dividing by zero; callers must not call this with an empty population.
    pub fn below(&mut self, bound: u64) -> u64 {
        if bound == 0 {
            0
        } else {
            self.next_u64() % bound
        }
    }
}

/// Deterministic, printable-ASCII pseudo-random content of `len` bytes, independently seeded from
/// `seed`. Matches `dc59_commit_benchmark.rs`'s own content recipe (`b'a' + byte % 26`) rather than
/// raw bytes, so generated files are always valid UTF-8 text -- which matters here specifically:
/// `crates/prikk-store/src/worktree_patch/node_authoring.rs` fixes a node's text/binary kind at
/// creation and never transitions it ("existing-node kind is authoritative, no text<->binary
/// transition"), so every path this corpus creates must stay authorable as `EditText` for its whole
/// life, and non-UTF-8 content at creation would instead mint a `ReplaceBinary`-only node.
#[must_use]
pub fn generate_bytes(seed: u64, len: u64) -> Vec<u8> {
    let mut rng = SplitMix64::new(seed);
    let len = usize::try_from(len).unwrap_or(usize::MAX);
    let mut buf = vec![0_u8; len];
    rng.fill_bytes(&mut buf);
    for byte in &mut buf {
        *byte = b'a' + (*byte % 26);
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_seed_reproduces_the_same_bytes() {
        assert_eq!(generate_bytes(42, 64), generate_bytes(42, 64));
    }

    #[test]
    fn different_seeds_reproduce_different_bytes() {
        assert_ne!(generate_bytes(42, 64), generate_bytes(43, 64));
    }

    #[test]
    fn generated_bytes_are_always_printable_ascii_lowercase() {
        let bytes = generate_bytes(7, 1000);
        assert!(bytes.iter().all(|byte| byte.is_ascii_lowercase()));
    }

    #[test]
    fn below_zero_bound_returns_zero_rather_than_panicking() {
        let mut rng = SplitMix64::new(1);
        assert_eq!(rng.below(0), 0);
    }
}
