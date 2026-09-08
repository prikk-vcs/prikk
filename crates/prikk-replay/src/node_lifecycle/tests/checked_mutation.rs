use super::*;

#[test]
fn create_node_rejects_all_zero_node_id() {
    let mut state = NodeLifecycleState::new();
    let err = state
        .create_node(nid(0x00), file_node("a.txt", 0xaa, 0o100_644))
        .expect_err("all-zero node_id");
    assert!(format!("{err:?}").contains("zero"));
}

#[test]
fn change_file_mode_updates_mode_exactly() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create");
    state
        .change_file_mode(nid(1), 0o100_644, 0o100_755)
        .expect("chmod");
    match &state.live_node(&nid(1)).expect("live").content {
        NodeContent::File { mode, .. } => assert_eq!(*mode, 0o100_755),
        other => panic!("expected file content, got {other:?}"),
    }
}

#[test]
fn change_file_mode_rejects_old_mode_mismatch() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create");
    assert!(
        state
            .change_file_mode(nid(1), 0o100_600, 0o100_755)
            .is_err()
    );
}

#[test]
fn change_file_mode_rejects_symlink() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(2), symlink_node("link", "target"))
        .expect("create symlink");
    assert!(state.change_file_mode(nid(2), 0, 0o100_755).is_err());
}

#[test]
fn change_file_mode_rejects_dead_node() {
    let mut state = NodeLifecycleState::new();
    assert!(
        state
            .change_file_mode(nid(9), 0o100_644, 0o100_755)
            .is_err()
    );
}

#[test]
fn delete_node_checked_rejects_preimage_mismatch() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create");
    // Expected node claims a different mode than the live node.
    let wrong = file_node("a.txt", 7, 0o100_600);
    assert!(state.delete_node_checked(nid(1), &wrong).is_err());
    // The node must remain live after a rejected delete.
    assert!(state.live_node(&nid(1)).is_some());
}

#[test]
fn delete_node_checked_accepts_exact_preimage() {
    let mut state = NodeLifecycleState::new();
    let node = file_node("a.txt", 7, 0o100_644);
    state.create_node(nid(1), node.clone()).expect("create");
    let deleted = state.delete_node_checked(nid(1), &node).expect("delete");
    assert_eq!(deleted, node);
    assert!(state.live_node(&nid(1)).is_none());
}

#[test]
fn rename_node_checked_rejects_old_path_mismatch() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create");
    assert!(
        state
            .rename_node_checked(nid(1), &path("wrong.txt"), path("b.txt"))
            .is_err()
    );
    // Path index unchanged after rejection.
    assert_eq!(state.node_id_at(&path("a.txt")), Some(nid(1)));
}

#[test]
fn rename_node_checked_accepts_correct_old_path() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create");
    state
        .rename_node_checked(nid(1), &path("a.txt"), path("b.txt"))
        .expect("rename");
    assert_eq!(state.node_id_at(&path("b.txt")), Some(nid(1)));
    assert_eq!(state.node_id_at(&path("a.txt")), None);
}

/// RFC 144 §4j.2's own control: a chained rename (`A→B` then `B→C`, same node twice) resolves the
/// batch as one unit, so the swap case is not confused with the chain case.
#[test]
fn rename_nodes_checked_batch_resolves_a_two_node_swap() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create a");
    state
        .create_node(nid(2), file_node("b.txt", 8, 0o100_644))
        .expect("create b");

    state
        .rename_nodes_checked_batch(&[
            (nid(1), path("a.txt"), path("b.txt")),
            (nid(2), path("b.txt"), path("a.txt")),
        ])
        .expect("a genuine swap must resolve, not collide with itself");

    assert_eq!(state.node_id_at(&path("b.txt")), Some(nid(1)));
    assert_eq!(state.node_id_at(&path("a.txt")), Some(nid(2)));
}

/// §4j.2: renaming the same node twice within one batch (`A→B` then `B→C`) is malformed -- the
/// intermediate path existed in no sealed state, so its only honest meaning is the net move.
/// Rejected by Phase 0's own `old_path`-against-*pre-batch*-state check, not by the
/// `sources_seen` dedup: the second pair asserts `old_path = "b.txt"`, but Phase 0 never applies
/// the first pair before checking the second, so node 1's live path is still `"a.txt"` when the
/// second pair is checked -- the assertion simply does not match reality. (`sources_seen` guards a
/// narrower shape: the *same* asserted `old_path` claimed twice for one node, e.g. `A: a→b` and
/// `A: a→c` in the same batch -- both would pass the pre-batch check individually.)
#[test]
fn rename_nodes_checked_batch_rejects_a_chained_rename() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create a");
    let before = state.clone();

    let err = state
        .rename_nodes_checked_batch(&[
            (nid(1), path("a.txt"), path("b.txt")),
            (nid(1), path("b.txt"), path("c.txt")),
        ])
        .expect_err("the same node renamed twice in one batch must be rejected");
    assert!(
        format!("{err:?}").contains("does not match the live path"),
        "{err:?}"
    );
    assert_eq!(
        state, before,
        "a rejected batch must leave the receiver completely unchanged"
    );
}

/// The narrower shape `rename_nodes_checked_batch_rejects_a_chained_rename`'s own doc comment
/// names: two operations both asserting the *same* (true, pre-batch) `old_path` for one node, with
/// different destinations. This is what `sources_seen` itself exists to catch -- the `old_path`
/// check alone would pass both individually.
#[test]
fn rename_nodes_checked_batch_rejects_the_same_node_renamed_twice_from_the_same_old_path() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create a");
    let before = state.clone();

    let err = state
        .rename_nodes_checked_batch(&[
            (nid(1), path("a.txt"), path("b.txt")),
            (nid(1), path("a.txt"), path("c.txt")),
        ])
        .expect_err("the same node named twice from the same old_path must be rejected");
    assert!(format!("{err:?}").contains("more than once"), "{err:?}");
    assert_eq!(
        state, before,
        "a rejected batch must leave the receiver completely unchanged"
    );
}

/// RFC 144 §4k.3: a batch that fails partway (here, its second pair collides with a live node
/// outside the batch) must leave the receiver **completely** unchanged -- not just "the destination
/// was never inserted" but "the source was never removed either." Checked by cloning the state
/// before the call and asserting full equality after, rather than checking individual fields, so a
/// future regression anywhere in the method (not just the two fields this test happens to think to
/// check) would be caught.
#[test]
fn rename_nodes_checked_batch_leaves_state_untouched_on_collision() {
    let mut state = NodeLifecycleState::new();
    state
        .create_node(nid(1), file_node("a.txt", 7, 0o100_644))
        .expect("create a");
    state
        .create_node(nid(2), file_node("b.txt", 8, 0o100_644))
        .expect("create b");
    state
        .create_node(nid(3), file_node("e.txt", 9, 0o100_644))
        .expect("create e");
    let before = state.clone();

    // First pair (A: a.txt -> b.txt) is a genuine swap partner of node 2 and would resolve fine on
    // its own; the second pair (node 2: b.txt -> e.txt) collides with node 3, which is not part of
    // this batch at all. The whole batch must fail, and neither pair's effect may be visible.
    let err = state
        .rename_nodes_checked_batch(&[
            (nid(1), path("a.txt"), path("b.txt")),
            (nid(2), path("b.txt"), path("e.txt")),
        ])
        .expect_err("e.txt is occupied by node 3, outside the batch");
    assert!(
        format!("{err:?}").contains("occupied by another live node"),
        "{err:?}"
    );

    assert_eq!(
        state, before,
        "a failed batch must leave the receiver byte-for-byte unchanged, including the first \
         pair's own source path, even though checking it in isolation would have succeeded"
    );
}
