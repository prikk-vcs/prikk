//! Production node-id minting (DC-09 Phase 4.4a-1).
//!
//! A `NodeId` is an opaque 32-byte **stable** node identity, minted once at node creation and
//! thereafter immutable: it must survive rename, edit, chmod, and binary replacement, and it is
//! part of the text `span_id` preimage. It is therefore **not** derived from path, content,
//! operation position, timestamp, or baseline state — it is a uniform 256-bit draw from the OS
//! CSPRNG.
//!
//! The boundary is deliberate (erratum E1): a [`NodeIdEntropySource`] produces *candidate bytes*,
//! and [`NodeIdGenerator`] is the **only** trusted minting authority — it enforces the fail-closed
//! invariants (nonzero via the canonical checked constructor, and no collision with any known
//! baseline node id) before returning a `NodeId`. Authoring must mint through `NodeIdGenerator`,
//! never treat a raw entropy source's output as certified.

use std::fmt;

use prikk_error::PrikkError;
use prikk_object::NodeId;

use crate::node::node_lifecycle::NodeLifecycleState;

/// Why fresh node-id minting failed. Structured so tests and later authoring can branch on the
/// cause (erratum E4); the fail-closed contract means none of these is ever swallowed or replaced
/// with a placeholder id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NodeIdMintError {
    /// The OS CSPRNG (or test source) could not supply entropy. Authoring must abort.
    EntropyUnavailable(String),
    /// Two consecutive all-zero draws (the reserved id). Astronomically unlikely from a healthy
    /// CSPRNG; treated as a fail-closed condition rather than retried unboundedly.
    ZeroNodeIdDraw,
    /// Two consecutive draws collided with a known baseline node id.
    NodeIdCollision(NodeId),
}

impl fmt::Display for NodeIdMintError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EntropyUnavailable(detail) => {
                write!(
                    f,
                    "node-id minting failed: OS entropy unavailable: {detail}"
                )
            }
            Self::ZeroNodeIdDraw => {
                write!(f, "node-id minting failed: repeated all-zero CSPRNG draw")
            }
            Self::NodeIdCollision(id) => {
                let hex: String = id.as_bytes().iter().map(|b| format!("{b:02x}")).collect();
                write!(
                    f,
                    "node-id minting failed: repeated draw collided with a known node id {hex}"
                )
            }
        }
    }
}

impl std::error::Error for NodeIdMintError {}

impl From<NodeIdMintError> for PrikkError {
    fn from(e: NodeIdMintError) -> Self {
        PrikkError::Integrity(e.to_string())
    }
}

/// Low-level source of *candidate* node-id bytes. **Not** the trusted minting API — it performs no
/// validation; the all-zero and collision checks live in [`NodeIdGenerator`]. Implementors fail
/// closed (returning [`NodeIdMintError::EntropyUnavailable`]) rather than producing weak bytes.
pub(crate) trait NodeIdEntropySource {
    fn fill_node_id_bytes(&mut self, out: &mut [u8; 32]) -> Result<(), NodeIdMintError>;
}

/// Production entropy: the OS CSPRNG via `getrandom`. Fail-closed — any error from the OS source
/// surfaces as [`NodeIdMintError::EntropyUnavailable`]; there is no weak/seeded fallback.
pub(crate) struct OsEntropySource;

impl NodeIdEntropySource for OsEntropySource {
    fn fill_node_id_bytes(&mut self, out: &mut [u8; 32]) -> Result<(), NodeIdMintError> {
        getrandom::fill(out).map_err(|e| NodeIdMintError::EntropyUnavailable(e.to_string()))
    }
}

/// The only trusted node-id minting authority. Wraps a candidate-byte source and enforces the
/// fail-closed invariants before returning a `NodeId`.
pub(crate) struct NodeIdGenerator<S> {
    source: S,
}

/// Why a single candidate draw was rejected (internal to the bounded retry).
enum Rejection {
    Zero,
    Collision(NodeId),
}

impl NodeIdGenerator<OsEntropySource> {
    /// Production generator backed by the OS CSPRNG.
    pub(crate) fn production() -> Self {
        Self {
            source: OsEntropySource,
        }
    }
}

impl<S: NodeIdEntropySource> NodeIdGenerator<S> {
    #[cfg(test)]
    pub(crate) fn with_source(source: S) -> Self {
        Self { source }
    }

    /// Draw one candidate and classify it: a valid fresh id, or a structured rejection. Propagates
    /// entropy failure. The id is constructed only through the canonical checked constructor
    /// [`NodeId::try_from_bytes`] (which rejects the reserved all-zero value).
    fn draw_candidate(
        &mut self,
        baseline: &NodeLifecycleState,
    ) -> Result<Result<NodeId, Rejection>, NodeIdMintError> {
        let mut bytes = [0_u8; 32];
        self.source.fill_node_id_bytes(&mut bytes)?;
        let candidate = match NodeId::try_from_bytes(bytes) {
            Ok(id) => id,
            Err(_) => return Ok(Err(Rejection::Zero)),
        };
        if baseline.contains_seen_node_id(&candidate) {
            return Ok(Err(Rejection::Collision(candidate)));
        }
        Ok(Ok(candidate))
    }

    /// Mint a fresh node id not present in `baseline`'s known-id set. Fail-closed and bounded
    /// (erratum E3): draw once; on an all-zero or colliding candidate, redraw exactly once; if the
    /// second draw also fails, return the structured cause rather than looping. Entropy failure on
    /// either draw aborts immediately.
    pub(crate) fn mint_fresh(
        &mut self,
        baseline: &NodeLifecycleState,
    ) -> Result<NodeId, NodeIdMintError> {
        if let Ok(id) = self.draw_candidate(baseline)? {
            return Ok(id);
        }
        match self.draw_candidate(baseline)? {
            Ok(id) => Ok(id),
            Err(Rejection::Zero) => Err(NodeIdMintError::ZeroNodeIdDraw),
            Err(Rejection::Collision(id)) => Err(NodeIdMintError::NodeIdCollision(id)),
        }
    }
}

#[cfg(test)]
mod tests;

/// Test-only scripted entropy: yields the given 32-byte candidates in order, then fails closed as an
/// entropy error. Shared across test modules (node-id minting and worktree authoring) so deterministic
/// fresh-id assignment can be driven from a fixed sequence.
#[cfg(test)]
pub(crate) struct SequenceEntropySource {
    queue: std::collections::VecDeque<[u8; 32]>,
}

#[cfg(test)]
impl SequenceEntropySource {
    pub(crate) fn new(candidates: &[[u8; 32]]) -> Self {
        Self {
            queue: candidates.iter().copied().collect(),
        }
    }
}

#[cfg(test)]
impl NodeIdEntropySource for SequenceEntropySource {
    fn fill_node_id_bytes(&mut self, out: &mut [u8; 32]) -> Result<(), NodeIdMintError> {
        match self.queue.pop_front() {
            Some(bytes) => {
                *out = bytes;
                Ok(())
            }
            None => Err(NodeIdMintError::EntropyUnavailable(
                "test sequence exhausted".to_string(),
            )),
        }
    }
}
