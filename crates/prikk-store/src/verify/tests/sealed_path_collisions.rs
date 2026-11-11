//! 0.57.0 U3b: `verify` judges a sealed block's **integrity**, not today's collision rule.
//!
//! A later Unicode version (or a crate upgrade) can make two old paths collide that did not. If `verify`'s re-derivation of a
//! block's state root asked that rule, valid sealed history would fail as a state-root mismatch, which reads as damage -- the one
//! thing ruling C3 forbids. So the state root is re-derived without the collision rule (a real mismatch is still a failure), and
//! a sealed **tip** whose paths collide under this version's rule is one warning, naming both paths and the way out. The sites
//! that write the disk (`seal`, `extract`, every checkout, `branch switch`) keep refusing.
//!
//! No real input shows the tightening today (the crates are pinned), so the test **arms a stricter rule**:
//! `fold_one_more_pair_for_test('b', 'a')` makes `b.txt` and `a.txt` one name. History is sealed under the real rule, then judged
//! by the stricter one.
//!
//! | step | rule | `verify` | `extract` of the tip |
//! |---|---|---|---|
//! | tip holds `a.txt`, `b.txt` | real | no collision | ok |
//! | the same tip | **stricter** | passes, **one warning** naming both | **refuses** (`case-collision`) |
//! | a child tip renames `b.txt` to `c.txt` | stricter | passes, **no warning** (the history's collision is no one's to fix) | ok |

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId, ObjectType,
    Operation, OperationKind, PatchPayload, PatchPurpose, RenamePath,
};

use crate::point::{Point, PointKind};
use crate::test_gates::test_support::{
    dummy_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir, write_blob,
};
use crate::{
    FileObjectStore, ObjectReadSnapshot, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    prepare_extract_plan, verify_repository,
};

fn patch(store: &mut FileObjectStore, operations: Vec<OperationKind>) -> ObjectId {
    let payload = PatchPayload {
        operations: operations
            .into_iter()
            .enumerate()
            .map(|(index, kind)| Operation {
                op_seq: u32::try_from(index + 1).unwrap(),
                op_id: None,
                preconditions: Vec::new(),
                kind,
            })
            .collect(),
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    envelope.add_signature(dummy_signature()).unwrap();
    store.write_object(&envelope).unwrap()
}

fn node(seed: u8) -> NodeId {
    NodeId::from_bytes([seed; 32])
}

/// A block over `parent` applying `patch_id`, claiming the state root its patch derives.
fn block(store: &mut FileObjectStore, parent: Option<ObjectId>, patch_id: ObjectId) -> ObjectId {
    let state = crate::derive_next_state_root(&*store, parent, &[patch_id]).unwrap();
    let envelope = signed_block_with_state_root(
        if parent.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        },
        parent.into_iter().collect(),
        vec![patch_id],
        None,
        state,
    );
    store.write_object(&envelope).unwrap()
}

/// Publish `block_id` on `heads/main`, after `previous` (the ref state it replaces, if any). Returns the new ref state's id.
fn publish(
    layout: &RepositoryLayout,
    previous: Option<ObjectId>,
    block_id: ObjectId,
    seq: u64,
) -> ObjectId {
    let ref_state = signed_ref_state_envelope("heads/main", previous, block_id, seq);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", previous, ref_state_id, block_id, seq);
    RefStore::new(layout.clone())
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: previous,
            ref_state,
            ref_update,
        })
        .unwrap();
    ref_state_id
}

fn extract_refuses_as_a_collision(layout: &RepositoryLayout, block_id: ObjectId) -> bool {
    let reader = ObjectReadSnapshot::open(layout).unwrap();
    let point = Point {
        name: block_id.to_string(),
        kind: PointKind::Block,
        block_id,
    };
    prepare_extract_plan(layout, &reader, &point, false)
        .is_err_and(|err| err.kind == Some(crate::ExtractRefusalKind::CaseCollision))
}

fn extract_succeeds(layout: &RepositoryLayout, block_id: ObjectId) -> bool {
    let reader = ObjectReadSnapshot::open(layout).unwrap();
    let point = Point {
        name: block_id.to_string(),
        kind: PointKind::Block,
        block_id,
    };
    prepare_extract_plan(layout, &reader, &point, false).is_ok()
}

#[test]
fn a_collision_under_a_stricter_rule_is_a_warning_in_verify_while_extract_still_refuses() {
    let root = unique_temp_dir("u3b-sealed-collision");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let mut store = FileObjectStore::new(layout.clone());
    let a_blob = write_blob(&mut store, b"a\n").unwrap();
    let b_blob = write_blob(&mut store, b"b\n").unwrap();
    let create_both = patch(
        &mut store,
        vec![
            OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: node(1),
                blob_id: a_blob,
                mode: 0o100_644,
            }),
            OperationKind::CreateFile(CreateFile {
                path: "b.txt".to_string(),
                node_id: node(2),
                blob_id: b_blob,
                mode: 0o100_644,
            }),
        ],
    );
    let tip = block(&mut store, None, create_both);
    let tip_ref_state = publish(&layout, None, tip, 1);

    // 1. The real rule: nothing to say, and extract works.
    let report = verify_repository(&layout).unwrap();
    assert!(
        report.sealed_path_collisions.is_empty(),
        "{:?}",
        report.sealed_path_collisions
    );
    assert!(extract_succeeds(&layout, tip));

    // 2. A stricter rule: valid history is a warning, never a failure; the writer refuses.
    {
        let _stricter = prikk_object::fold_one_more_pair_for_test('b', 'a');
        let report = verify_repository(&layout).unwrap();
        assert!(
            !report.has_item_failure(),
            "a stricter collision rule must not turn a valid state root into a failure: {report:?}"
        );
        assert_eq!(
            report.sealed_path_collisions.len(),
            1,
            "{:?}",
            report.sealed_path_collisions
        );
        let finding = &report.sealed_path_collisions[0];
        assert_eq!(
            (
                finding.block_id,
                finding.first_path.as_str(),
                finding.second_path.as_str(),
                finding.blocks
            ),
            (tip, "a.txt", "b.txt", 1)
        );
        let said = finding.warning();
        assert!(
            said.contains("a.txt and b.txt")
                && said.contains("`extract`, `checkout` and `branch switch` will refuse")
                && said.contains("prikk mv b.txt <new name>")
                && said.contains("`commit` and `seal`"),
            "{said}"
        );
        assert!(
            extract_refuses_as_a_collision(&layout, tip),
            "extract protects the disk"
        );
    }

    // 3. The way out: a child tip renames `b.txt`. The stricter rule is still armed; nothing is left to warn about or refuse.
    let rename = patch(
        &mut store,
        vec![OperationKind::RenamePath(RenamePath {
            node_id: node(2),
            old_path: "b.txt".to_string(),
            new_path: "c.txt".to_string(),
        })],
    );
    let child = block(&mut store, Some(tip), rename);
    publish(&layout, Some(tip_ref_state), child, 2);
    {
        let _stricter = prikk_object::fold_one_more_pair_for_test('b', 'a');
        let report = verify_repository(&layout).unwrap();
        assert!(!report.has_item_failure(), "{report:?}");
        assert!(
            report.sealed_path_collisions.is_empty(),
            "the renamed tip clears the warning; the earlier block is history: {:?}",
            report.sealed_path_collisions
        );
        assert!(
            extract_succeeds(&layout, child),
            "and extract of the renamed tip works"
        );
        assert!(
            extract_refuses_as_a_collision(&layout, tip),
            "the old block is still what it was"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
