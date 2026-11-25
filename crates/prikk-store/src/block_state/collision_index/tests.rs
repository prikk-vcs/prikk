//! The collision index against the policy it replaces: after every step of generated sequences the index's answer equals
//! `validate_no_path_collisions` over every live path (what `refuse_colliding_entries` asks of a state's entries).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{NodeId, NodeKind, ObjectId};
use prikk_replay::{
    LiveNode, NodeContent, NodeLifecycleState, RepoPath, validate_no_path_collisions,
};

use super::CollisionIndex;

/// Pairs that are one name under the policy (case, Unicode case folding, normalization, the Kelvin sign), and names that are not.
const POOL: &[&str] = &[
    "a.txt",
    "A.TXT",
    "b/c",
    "B/C",
    "stra\u{df}e",
    "strasse",
    "\u{e9}",
    "e\u{301}",
    "\u{1e9e}.md",
    "ss.md",
    "\u{212a}",
    "k",
    "dir/x",
    "dir/X",
    "plain",
    "other",
];

struct Lcg(u64);

impl Lcg {
    fn next(&mut self, bound: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        usize::try_from(self.0 >> 33).unwrap() % bound
    }
}

fn node(path: &RepoPath) -> LiveNode {
    LiveNode {
        path: path.clone(),
        kind: NodeKind::TextFile,
        content: NodeContent::File {
            blob_id: ObjectId::from_bytes([7; 32]),
            mode: 0o100_644,
        },
    }
}

fn policy_collides(state: &NodeLifecycleState) -> bool {
    let paths: Vec<RepoPath> = state.live_paths().cloned().collect();
    validate_no_path_collisions(&paths).is_err()
}

/// A sequence of "claims" (one to three operations each: create, delete, rename, a rename batch) over the pool. After every claim the index,
/// which is told only the claim's journal, answers as the policy does over the whole state. The coverage counters pin that the sequences
/// reach collisions, heals and renames into collisions, so the equality is not over states that never collide.
#[test]
fn the_index_answers_as_the_policy_does_after_every_claim() {
    let pool: Vec<RepoPath> = POOL
        .iter()
        .filter_map(|p| RepoPath::parse(p).ok())
        .collect();
    assert!(pool.len() >= 12, "the pool parses: {}", pool.len());
    let (mut collided, mut healed, mut renamed_into, mut claims) =
        (0_usize, 0_usize, 0_usize, 0_usize);
    for seed in 0..300_u64 {
        let mut rng = Lcg(seed + 1);
        let mut state = NodeLifecycleState::new();
        let mut next_id = 1_u8;
        state.record_path_changes();
        let mut index = CollisionIndex::of_state(&state);
        let mut was_colliding = false;
        for _ in 0..40 {
            for _ in 0..=rng.next(3) {
                let live: Vec<NodeId> = state.live_nodes().map(|(id, _)| *id).collect();
                match rng.next(5) {
                    0 | 1 => {
                        let path = &pool[rng.next(pool.len())];
                        let id = NodeId::from_bytes([next_id; 32]);
                        if state.create_node(id, node(path)).is_ok() {
                            next_id = next_id.wrapping_add(1).max(1);
                        }
                    }
                    2 if !live.is_empty() => {
                        let id = live[rng.next(live.len())];
                        let _ = state.delete_node(id);
                    }
                    3 if !live.is_empty() => {
                        let id = live[rng.next(live.len())];
                        let to = pool[rng.next(pool.len())].clone();
                        let before = policy_collides(&state);
                        if state.rename_node(id, to).is_ok() && !before {
                            renamed_into += usize::from(policy_collides(&state));
                        }
                    }
                    4 if live.len() >= 2 => {
                        // A swap, as one batch.
                        let a = live[rng.next(live.len())];
                        let b = live[rng.next(live.len())];
                        if a != b {
                            let pa = state.live_node(&a).unwrap().path.clone();
                            let pb = state.live_node(&b).unwrap().path.clone();
                            let _ = state.rename_nodes_checked_batch(&[
                                (a, pa.clone(), pb.clone()),
                                (b, pb, pa),
                            ]);
                        }
                    }
                    _ => {}
                }
            }
            index.apply(&state.take_path_changes());
            let expected = policy_collides(&state);
            assert_eq!(index.collides(), expected, "seed {seed}");
            collided += usize::from(expected && !was_colliding);
            healed += usize::from(!expected && was_colliding);
            was_colliding = expected;
            claims += 1;
        }
        // The index rebuilt from the final state agrees with the one kept all along.
        assert_eq!(
            CollisionIndex::of_state(&state).collides(),
            index.collides(),
            "seed {seed}: rebuilt"
        );
    }
    assert!(
        collided >= 20 && healed >= 10 && renamed_into >= 5,
        "coverage: {collided} collided, {healed} healed, {renamed_into} renamed into a collision, of {claims} claims"
    );
}

/// A state that already collides is answered as such, and a claim that removes the collision heals it: what the 0.58.0 fold's first
/// claim saw over a starting state with a pair in it.
#[test]
fn a_starting_state_with_a_pair_collides_until_a_claim_removes_one() {
    let mut state = NodeLifecycleState::new();
    let (a, upper) = (
        RepoPath::parse("a.txt").unwrap(),
        RepoPath::parse("A.TXT").unwrap(),
    );
    state
        .seed_live_node(NodeId::from_bytes([1; 32]), node(&a))
        .unwrap();
    state
        .seed_live_node(NodeId::from_bytes([2; 32]), node(&upper))
        .unwrap();
    let mut index = CollisionIndex::of_state(&state);
    assert!(index.collides());
    state.record_path_changes();
    state.delete_node(NodeId::from_bytes([2; 32])).unwrap();
    index.apply(&state.take_path_changes());
    assert!(!index.collides());
}
