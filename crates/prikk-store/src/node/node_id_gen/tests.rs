#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use prikk_object::{NodeId, NodeKind, ObjectId};

use super::{NodeIdEntropySource, NodeIdGenerator, NodeIdMintError, SequenceEntropySource};
use crate::node::node_lifecycle::{LiveNode, NodeContent, NodeLifecycleState};
use crate::path::RepoPath;

/// An entropy source that always fails (simulates OS CSPRNG unavailability).
struct FailingEntropySource;

impl NodeIdEntropySource for FailingEntropySource {
    fn fill_node_id_bytes(&mut self, _out: &mut [u8; 32]) -> Result<(), NodeIdMintError> {
        Err(NodeIdMintError::EntropyUnavailable(
            "simulated failure".to_string(),
        ))
    }
}

fn bytes(b: u8) -> [u8; 32] {
    [b; 32]
}

const ZERO: [u8; 32] = [0_u8; 32];

fn empty_baseline() -> NodeLifecycleState {
    NodeLifecycleState::new()
}

fn baseline_with(id: NodeId) -> NodeLifecycleState {
    let mut state = NodeLifecycleState::new();
    state
        .seed_live_node(
            id,
            LiveNode {
                path: RepoPath::parse("a.txt").unwrap(),
                kind: NodeKind::TextFile,
                content: NodeContent::File {
                    blob_id: ObjectId::from_bytes([0x33; 32]),
                    mode: 0o100_644,
                },
            },
        )
        .expect("seed baseline");
    state
}

#[test]
fn deterministic_source_emits_expected_nonzero_ids() {
    let mut generator =
        NodeIdGenerator::with_source(SequenceEntropySource::new(&[bytes(0x11), bytes(0x22)]));
    let baseline = empty_baseline();
    let first = generator.mint_fresh(&baseline).expect("mint 1");
    let second = generator.mint_fresh(&baseline).expect("mint 2");
    assert_eq!(first, NodeId::from_bytes(bytes(0x11)));
    assert_eq!(second, NodeId::from_bytes(bytes(0x22)));
    assert!(!first.is_zero() && !second.is_zero());
}

#[test]
fn entropy_failure_returns_structured_error_and_no_placeholder() {
    let mut generator = NodeIdGenerator::with_source(FailingEntropySource);
    let err = generator
        .mint_fresh(&empty_baseline())
        .expect_err("entropy failure");
    assert!(matches!(err, NodeIdMintError::EntropyUnavailable(_)));
}

#[test]
fn all_zero_candidate_is_rejected_and_redrawn() {
    // First draw is the reserved all-zero id; the generator redraws and returns the nonzero id.
    let mut generator =
        NodeIdGenerator::with_source(SequenceEntropySource::new(&[ZERO, bytes(0x44)]));
    let id = generator
        .mint_fresh(&empty_baseline())
        .expect("redraw past zero");
    assert_eq!(id, NodeId::from_bytes(bytes(0x44)));
}

#[test]
fn repeated_all_zero_fails_closed() {
    let mut generator = NodeIdGenerator::with_source(SequenceEntropySource::new(&[ZERO, ZERO]));
    let err = generator
        .mint_fresh(&empty_baseline())
        .expect_err("zero twice");
    assert_eq!(err, NodeIdMintError::ZeroNodeIdDraw);
}

#[test]
fn collision_with_known_baseline_is_rejected_and_redrawn() {
    let known = NodeId::from_bytes(bytes(0x55));
    let baseline = baseline_with(known);
    // First draw collides with the known id; the generator redraws and returns the fresh id.
    let mut generator =
        NodeIdGenerator::with_source(SequenceEntropySource::new(&[bytes(0x55), bytes(0x66)]));
    let id = generator
        .mint_fresh(&baseline)
        .expect("redraw past collision");
    assert_eq!(id, NodeId::from_bytes(bytes(0x66)));
}

#[test]
fn repeated_collision_fails_closed() {
    let known = NodeId::from_bytes(bytes(0x77));
    let baseline = baseline_with(known);
    let mut generator =
        NodeIdGenerator::with_source(SequenceEntropySource::new(&[bytes(0x77), bytes(0x77)]));
    let err = generator
        .mint_fresh(&baseline)
        .expect_err("collision twice");
    assert_eq!(err, NodeIdMintError::NodeIdCollision(known));
}

#[test]
fn minted_id_is_nonzero_checked_construction() {
    // The only path to a NodeId is through NodeId::try_from_bytes inside the generator, which
    // rejects the reserved all-zero value; a minted id is therefore always nonzero.
    let mut generator = NodeIdGenerator::with_source(SequenceEntropySource::new(&[bytes(0x88)]));
    let id = generator.mint_fresh(&empty_baseline()).expect("mint");
    assert!(!id.is_zero());
}
