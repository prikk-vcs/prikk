//! RFC 144 §4j / increment 2 — control 2, the real deliverable: the seal path
//! ([`derive_next_state_root`]) and the read path (`patch_replay`) must reach the *same* verdict
//! over rename-containing history. Five shapes, each checked through both paths against the exact
//! same operations.
//!
//! `derive_next_state_root` works over any `impl ObjectReader`, but `patch_replay`'s own
//! `prepare_patch_replay_plan` requires a real `&RepositoryLayout` (it opens its own
//! `ObjectReadSnapshot`), so this uses `FileObjectStore` + `RefStore` throughout -- the same
//! raw-patch-then-seal shape `patch_replay`'s own increment-1 fixtures
//! (`test_gates::test_support::rename_history`) established, generalized to five shapes and to
//! comparing both verdicts rather than asserting one.

#![allow(clippy::expect_used, clippy::unwrap_used)]

use prikk_object::{
    BlockKind, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectType, Operation,
    OperationKind, PatchPayload, PatchPurpose, RenamePath,
};

use crate::test_gates::test_support::{
    dummy_signature, signed_block_with_state_root, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir, write_blob,
};
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    derive_next_state_root, prepare_patch_replay_plan,
};

/// One `CreateFile` to author, before blob writing: `run_shape` writes the blob into the shape's
/// own store and fills in `blob_id` at that point, so no id ever needs to be computed against a
/// different store than the one the block is actually sealed from.
struct FileToCreate {
    path: &'static str,
    node_seed: u8,
}

enum TestOperation {
    Create(FileToCreate),
    Rename {
        node_seed: u8,
        old_path: &'static str,
        new_path: &'static str,
    },
}

/// One shape's root-block files (always succeeds -- no rename ever appears here) and its test-block
/// operations (the shape under test).
struct Shape {
    name: &'static str,
    root_files: Vec<FileToCreate>,
    test_operations: Vec<TestOperation>,
    /// What both paths must agree on: `true` = both must accept, `false` = both must reject.
    expect_accept: bool,
}

fn renumbered(mut operations: Vec<Operation>) -> Vec<Operation> {
    for (index, operation) in operations.iter_mut().enumerate() {
        operation.op_seq = (index + 1) as u32;
    }
    operations
}

fn create_operation(store: &mut FileObjectStore, file: &FileToCreate) -> Operation {
    let blob_id = write_blob(store, format!("{} content\n", file.path).as_bytes()).unwrap();
    Operation {
        op_seq: 0, // renumbered by the caller
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: file.path.to_string(),
            node_id: NodeId::from_bytes([file.node_seed; 32]),
            blob_id,
            mode: 0o100644,
        }),
    }
}

fn test_operation(store: &mut FileObjectStore, operation: &TestOperation) -> Operation {
    match operation {
        TestOperation::Create(file) => create_operation(store, file),
        TestOperation::Rename {
            node_seed,
            old_path,
            new_path,
        } => Operation {
            op_seq: 0,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::RenamePath(RenamePath {
                node_id: NodeId::from_bytes([*node_seed; 32]),
                old_path: old_path.to_string(),
                new_path: new_path.to_string(),
            }),
        },
    }
}

/// Run one shape end to end: seal the root, derive the test block's state root (the seal-path
/// verdict), seal the test block regardless of whether that derivation succeeded (a placeholder
/// root when it did not -- `patch_replay`'s read path never checks a block's own state root, an
/// increment-1 finding this reuses rather than re-deriving), publish both, then run
/// `prepare_patch_replay_plan` (the read-path verdict). Returns `(seal_ok, replay_ok)`.
fn run_shape(shape: &Shape) -> (bool, bool) {
    let root = unique_temp_dir(&format!("rfc144-inc2-convergence-{}", shape.name));
    let layout = RepositoryLayout::init(root.clone()).expect("layout init");
    let mut object_store = FileObjectStore::new(layout.clone());

    let root_operations = renumbered(
        shape
            .root_files
            .iter()
            .map(|file| create_operation(&mut object_store, file))
            .collect(),
    );
    let root_payload = PatchPayload {
        operations: root_operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut root_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        root_payload.to_canonical_bytes().unwrap(),
    );
    root_patch.add_signature(dummy_signature()).unwrap();
    let root_patch_id = object_store.write_object(&root_patch).unwrap();
    let root_state = derive_next_state_root(&object_store, None, &[root_patch_id])
        .expect("root block's own operations never contain a rename; must always derive");
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![root_patch_id],
        None,
        root_state,
    );
    let root_block_id = object_store.write_object(&root_block).unwrap();

    let test_operations = renumbered(
        shape
            .test_operations
            .iter()
            .map(|operation| test_operation(&mut object_store, operation))
            .collect(),
    );
    let test_payload = PatchPayload {
        operations: test_operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut test_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        test_payload.to_canonical_bytes().unwrap(),
    );
    test_patch.add_signature(dummy_signature()).unwrap();
    let test_patch_id = object_store.write_object(&test_patch).unwrap();

    let seal_result = derive_next_state_root(&object_store, Some(root_block_id), &[test_patch_id]);
    let seal_ok = seal_result.is_ok();
    let test_state = seal_result.unwrap_or(root_state);
    let test_block = signed_block_with_state_root(
        BlockKind::Normal,
        vec![root_block_id],
        vec![test_patch_id],
        None,
        test_state,
    );
    let test_block_id = object_store.write_object(&test_block).unwrap();

    let ref_store = RefStore::new(layout.clone());
    let root_ref_state = signed_ref_state_envelope("heads/main", None, root_block_id, 1);
    let root_ref_state_id = root_ref_state.object_id();
    let root_ref_update =
        signed_ref_update_envelope("heads/main", None, root_ref_state_id, root_block_id, 1);
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state: root_ref_state,
            ref_update: root_ref_update,
        })
        .unwrap();

    let test_ref_state =
        signed_ref_state_envelope("heads/main", Some(root_ref_state_id), test_block_id, 2);
    let test_ref_state_id = test_ref_state.object_id();
    let test_ref_update = signed_ref_update_envelope(
        "heads/main",
        Some(root_ref_state_id),
        test_ref_state_id,
        test_block_id,
        2,
    );
    ref_store
        .publish(&RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: Some(root_ref_state_id),
            ref_state: test_ref_state,
            ref_update: test_ref_update,
        })
        .unwrap();

    let replay_ok = prepare_patch_replay_plan(&layout, "heads/main").is_ok();

    let _ = std::fs::remove_dir_all(root);
    (seal_ok, replay_ok)
}

/// Control 2: the five-shape convergence table. Built and asserted as one table, not five separate
/// tests, so a failure reports the whole picture (which shapes disagreed) at once.
#[test]
fn seal_path_and_read_path_agree_on_every_shape() {
    let shapes = vec![
        Shape {
            name: "plain-rename",
            root_files: vec![FileToCreate {
                path: "a.txt",
                node_seed: 0x01,
            }],
            test_operations: vec![TestOperation::Rename {
                node_seed: 0x01,
                old_path: "a.txt",
                new_path: "b.txt",
            }],
            expect_accept: true,
        },
        Shape {
            name: "two-node-swap",
            root_files: vec![
                FileToCreate {
                    path: "a.txt",
                    node_seed: 0x02,
                },
                FileToCreate {
                    path: "b.txt",
                    node_seed: 0x03,
                },
            ],
            test_operations: vec![
                TestOperation::Rename {
                    node_seed: 0x02,
                    old_path: "a.txt",
                    new_path: "b.txt",
                },
                TestOperation::Rename {
                    node_seed: 0x03,
                    old_path: "b.txt",
                    new_path: "a.txt",
                },
            ],
            expect_accept: true,
        },
        Shape {
            name: "chained-rename",
            root_files: vec![FileToCreate {
                path: "a.txt",
                node_seed: 0x04,
            }],
            test_operations: vec![
                TestOperation::Rename {
                    node_seed: 0x04,
                    old_path: "a.txt",
                    new_path: "b.txt",
                },
                TestOperation::Rename {
                    node_seed: 0x04,
                    old_path: "b.txt",
                    new_path: "c.txt",
                },
            ],
            expect_accept: false,
        },
        Shape {
            name: "genuine-collision",
            root_files: vec![
                FileToCreate {
                    path: "a.txt",
                    node_seed: 0x05,
                },
                FileToCreate {
                    path: "c.txt",
                    node_seed: 0x06,
                },
            ],
            test_operations: vec![TestOperation::Rename {
                node_seed: 0x05,
                old_path: "a.txt",
                new_path: "c.txt",
            }],
            expect_accept: false,
        },
        Shape {
            name: "rename-free",
            root_files: vec![FileToCreate {
                path: "a.txt",
                node_seed: 0x07,
            }],
            test_operations: vec![TestOperation::Create(FileToCreate {
                path: "d.txt",
                node_seed: 0x08,
            })],
            expect_accept: true,
        },
    ];

    let mut table = Vec::new();
    let mut disagreements = Vec::new();
    for shape in &shapes {
        let (seal_ok, replay_ok) = run_shape(shape);
        table.push(format!(
            "{:<18} seal={:<5} replay={:<5} expected={:<5}",
            shape.name, seal_ok, replay_ok, shape.expect_accept
        ));
        if seal_ok != replay_ok {
            disagreements.push(format!(
                "{}: seal_ok={seal_ok}, replay_ok={replay_ok} -- DISAGREE",
                shape.name
            ));
        }
        if seal_ok != shape.expect_accept {
            disagreements.push(format!(
                "{}: seal_ok={seal_ok}, expected {} -- WRONG VERDICT",
                shape.name, shape.expect_accept
            ));
        }
    }

    assert!(
        disagreements.is_empty(),
        "convergence table:\n{}\ndisagreements:\n{}",
        table.join("\n"),
        disagreements.join("\n")
    );
}

/// Control 5: a genuine collision still fails at seal time, for the same reason it did before this
/// round -- there is no valid post-state to derive a root from. Same fixture as the convergence
/// table's own "genuine-collision" shape; this test additionally names the exact error, which the
/// table alone does not.
#[test]
fn genuine_collision_fails_at_seal_time_with_the_expected_error() {
    let root = unique_temp_dir("rfc144-inc2-control5-collision");
    let layout = RepositoryLayout::init(root.clone()).expect("layout init");
    let mut object_store = FileObjectStore::new(layout.clone());

    let root_operations = renumbered(vec![
        create_operation(
            &mut object_store,
            &FileToCreate {
                path: "a.txt",
                node_seed: 0x09,
            },
        ),
        create_operation(
            &mut object_store,
            &FileToCreate {
                path: "c.txt",
                node_seed: 0x0a,
            },
        ),
    ]);
    let root_payload = PatchPayload {
        operations: root_operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut root_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        root_payload.to_canonical_bytes().unwrap(),
    );
    root_patch.add_signature(dummy_signature()).unwrap();
    let root_patch_id = object_store.write_object(&root_patch).unwrap();
    let root_state = derive_next_state_root(&object_store, None, &[root_patch_id]).unwrap();
    let root_block = signed_block_with_state_root(
        BlockKind::Root,
        Vec::new(),
        vec![root_patch_id],
        None,
        root_state,
    );
    let root_block_id = object_store.write_object(&root_block).unwrap();

    let collision_operations = renumbered(vec![test_operation(
        &mut object_store,
        &TestOperation::Rename {
            node_seed: 0x09,
            old_path: "a.txt",
            new_path: "c.txt",
        },
    )]);
    let collision_payload = PatchPayload {
        operations: collision_operations,
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut collision_patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        collision_payload.to_canonical_bytes().unwrap(),
    );
    collision_patch.add_signature(dummy_signature()).unwrap();
    let collision_patch_id = object_store.write_object(&collision_patch).unwrap();

    let err = derive_next_state_root(&object_store, Some(root_block_id), &[collision_patch_id])
        .expect_err("a genuine collision must fail at seal time, not silently derive a root");
    let message = format!("{err}");
    assert!(
        message.contains("c.txt") && message.contains("occupied"),
        "error must name the occupied path: {message}"
    );

    let _ = std::fs::remove_dir_all(root);
}
