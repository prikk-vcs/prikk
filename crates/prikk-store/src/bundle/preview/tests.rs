use super::*;

/// The higher-level tests in `crates/prikk-store/src/bundle/tests/preview.rs` exercise
/// `preview_impact` end to end (real repositories, real bundles); this file is reserved for
/// primitives narrow enough not to need one. `find_lowest_common_ancestors` is the one piece of
/// genuinely new graph logic this module adds (this module's own doc comment), so it gets a direct
/// unit test rather than relying only on the end-to-end coverage to exercise it.
#[test]
fn find_lowest_common_ancestors_of_a_single_shared_tip_is_that_tip() {
    let tip = ObjectId::from_bytes([0x01; 32]);
    let mut ancestors = BTreeMap::new();
    ancestors.insert(
        tip,
        BlockPayload {
            parent_block_ids: Vec::new(),
            kind: prikk_object::BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: prikk_object::MerkleRoot([0; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    );
    let found = find_lowest_common_ancestors(&ancestors, &ancestors);
    assert_eq!(found, vec![tip]);
}

/// A linear chain shared by both sides: the *nearest* shared block (not genesis) is the only
/// maximal element, even though genesis is also in the intersection.
#[test]
fn find_lowest_common_ancestors_prefers_the_nearest_shared_block() {
    let genesis = ObjectId::from_bytes([0x01; 32]);
    let middle = ObjectId::from_bytes([0x02; 32]);
    let mut ancestors = BTreeMap::new();
    ancestors.insert(
        genesis,
        BlockPayload {
            parent_block_ids: Vec::new(),
            kind: prikk_object::BlockKind::Root,
            patch_ids: Vec::new(),
            state_merkle_root: prikk_object::MerkleRoot([0; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    );
    ancestors.insert(
        middle,
        BlockPayload {
            parent_block_ids: vec![genesis],
            kind: prikk_object::BlockKind::Normal,
            patch_ids: Vec::new(),
            state_merkle_root: prikk_object::MerkleRoot([0; 32]),
            snapshot_blob_ref: None,
            mainline_parent_id: None,
            merge_baseline_block_id: None,
        },
    );
    let found = find_lowest_common_ancestors(&ancestors, &ancestors);
    assert_eq!(
        found,
        vec![middle],
        "genesis is a common ancestor too, but it is dominated by middle, which is also shared"
    );
}
