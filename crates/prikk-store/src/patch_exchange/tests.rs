//! RFC 115 Stage 3 §5: `accepted_but_unsealed_patch_ids` -- the derived query, tested directly.

use prikk_error::Result;
use prikk_object::BlockKind;

use super::accepted_but_unsealed_patch_ids;
use crate::test_gates::test_support::{
    signed_block, signed_patch_blob_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};
use crate::{FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout};

#[test]
fn an_empty_repository_has_nothing_accepted_but_unsealed() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-empty");
    let layout = RepositoryLayout::init(root.clone())?;
    assert_eq!(accepted_but_unsealed_patch_ids(&layout)?, Vec::new());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_patch_sealed_into_a_ref_reachable_block_is_not_unsealed() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-sealed");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(BlockKind::Normal, vec![root_block_id], vec![patch_id], None);
    let child_block_id = objects.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;

    assert_eq!(accepted_but_unsealed_patch_ids(&layout)?, Vec::new());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_patch_written_but_reachable_from_no_ref_is_accepted_but_unsealed() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-orphan");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = objects.write_object(&patch)?;

    // No block, no ref -- exactly what an accept path (§1's scope cut) leaves behind: a Patch
    // object present in the store, reachable from nothing.
    assert_eq!(accepted_but_unsealed_patch_ids(&layout)?, vec![patch_id]);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn one_sealed_and_one_unsealed_patch_are_told_apart() -> Result<()> {
    let root = unique_temp_dir("pexch-unsealed-mixed");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    let sealed_patch = signed_patch_envelope();
    let sealed_patch_id = objects.write_object(&sealed_patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), Vec::new(), None);
    let root_block_id = objects.write_object(&root_block)?;
    let child_block = signed_block(
        BlockKind::Normal,
        vec![root_block_id],
        vec![sealed_patch_id],
        None,
    );
    let child_block_id = objects.write_object(&child_block)?;

    let ref_store = RefStore::new(layout.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, child_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update =
        signed_ref_update_envelope("heads/main", None, ref_state_id, child_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;

    // A second, distinct Patch object -- a different path and a different signer, so its id
    // genuinely differs from `sealed_patch_id` -- written directly, sealed into nothing.
    let signer = super::exchange_test_support::author_signer(0x61)?;
    let unsealed_blob = super::exchange_test_support::signed_blob_envelope(b"unsealed fixture\n")?;
    let unsealed_blob_id = objects.write_object(&unsealed_blob)?;
    let unsealed_patch = super::exchange_test_support::signed_author_patch_envelope(
        &signer,
        "unsealed.txt",
        0x62,
        unsealed_blob_id,
    )?;
    let unsealed_patch_id = objects.write_object(&unsealed_patch)?;
    assert_ne!(
        unsealed_patch_id, sealed_patch_id,
        "fixture sanity: distinct content"
    );

    assert_eq!(
        accepted_but_unsealed_patch_ids(&layout)?,
        vec![unsealed_patch_id]
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
