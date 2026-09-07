//! Checkout planning tests.

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope, ObjectType,
};

use crate::{
    CheckoutMaterialization, FileObjectStore, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, prepare_checkout_plan,
};

use crate::test_gates::test_support::{
    maintainer_signature, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};

#[test]
fn checkout_plan_reports_unpublished_ref() {
    let root = unique_temp_dir("checkout-unpublished");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let plan = prepare_checkout_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert!(!plan.has_target_block());
            assert_eq!(
                plan.materialization,
                CheckoutMaterialization::UnpublishedRef
            );
            assert_eq!(plan.patch_count, 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn checkout_plan_validates_current_block_and_patch_refs() {
    let root = unique_temp_dir("checkout-block");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut object_store = FileObjectStore::new(layout.clone());
        let patch = signed_patch_envelope();
        let patch_id = patch.object_id();
        assert!(object_store.write_object(&patch).is_ok());

        let block = signed_block_envelope(BlockKind::Normal, Vec::new(), vec![patch_id]);
        let block_id = block.object_id();
        assert!(object_store.write_object(&block).is_ok());

        let ref_store = RefStore::new(layout.clone());
        let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
        let ref_state_id = ref_state.object_id();
        let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
        let publication = RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        };
        assert!(ref_store.publish(&publication).is_ok());

        let plan = prepare_checkout_plan(&layout, "heads/main");
        assert!(plan.is_ok());
        if let Ok(plan) = plan {
            assert!(plan.has_target_block());
            assert_eq!(plan.block_id, Some(block_id));
            assert_eq!(plan.patch_count, 1);
            assert_eq!(
                plan.materialization,
                CheckoutMaterialization::RequiresPatchEngine
            );
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn checkout_plan_rejects_missing_patch_refs() {
    let root = unique_temp_dir("checkout-missing-patch");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut object_store = FileObjectStore::new(layout.clone());
        let missing_patch_id =
            prikk_object::ObjectId::from_canonical_payload(ObjectType::Patch, 1, b"missing patch");
        let block = signed_block_envelope(BlockKind::Normal, Vec::new(), vec![missing_patch_id]);
        let block_id = block.object_id();
        assert!(object_store.write_object(&block).is_ok());

        let ref_store = RefStore::new(layout.clone());
        let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
        let ref_state_id = ref_state.object_id();
        let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
        let publication = RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        };
        assert!(ref_store.publish(&publication).is_ok());

        let plan = prepare_checkout_plan(&layout, "heads/main");
        assert!(plan.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

fn signed_block_envelope(
    kind: BlockKind,
    parent_block_ids: Vec<prikk_object::ObjectId>,
    patch_ids: Vec<prikk_object::ObjectId>,
) -> ObjectEnvelope {
    let payload = BlockPayload {
        parent_block_ids,
        kind,
        patch_ids,
        state_merkle_root: MerkleRoot([0_u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes();
    assert!(payload_bytes.is_ok());
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload_bytes.unwrap_or_default());
    assert!(envelope.add_signature(maintainer_signature()).is_ok());
    envelope
}
