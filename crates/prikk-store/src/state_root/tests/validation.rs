use prikk_object::{NodeId, NodeKind, ObjectId};

use super::super::{StateRootContent, compute_state_root, state_leaf_preimage};
use super::vectors::{binary_entry, symlink_entry, text_entry};
use crate::path::RepoPath;

#[test]
fn rejects_noncanonical_order_collisions_and_duplicate_node_ids() -> prikk_error::Result<()> {
    let mut reversed = vec![binary_entry()?, text_entry()?];
    assert!(compute_state_root(&reversed).is_err());

    let [first, second] = reversed.as_mut_slice() else {
        unreachable!("fixture has exactly two entries");
    };
    first.path = RepoPath::parse("A.txt")?;
    second.path = RepoPath::parse("a.txt")?;
    // 0.57.0 U3b: two different paths that are one name under this version's collision rule are **policy**, not integrity. The
    // state root is still computed (it is what `verify` re-derives a sealed block through, and a later Unicode version must not
    // make valid history fail), and the collision is refused where a state is authored or written.
    assert!(compute_state_root(&reversed).is_ok());
    assert!(crate::state_root::refuse_colliding_entries(&reversed).is_err());
    assert!(crate::state_root::first_collision(&reversed).is_some());
    // The same path twice is integrity, and stays a refusal everywhere.
    let mut twice = reversed.clone();
    if let Some(entry) = twice.get_mut(1) {
        entry.path = RepoPath::parse("A.txt")?;
    }
    assert!(compute_state_root(&twice).is_err());

    let mut duplicate_node = vec![text_entry()?, binary_entry()?];
    let [first, second] = duplicate_node.as_mut_slice() else {
        unreachable!("fixture has exactly two entries");
    };
    second.node_id = first.node_id;
    assert!(compute_state_root(&duplicate_node).is_err());
    Ok(())
}

#[test]
fn rejects_zero_node_kind_content_and_mode_disagreement() -> prikk_error::Result<()> {
    let mut entry = text_entry()?;
    entry.node_id = NodeId::from_bytes([0; 32]);
    assert!(state_leaf_preimage(&entry).is_err());

    let mut entry = text_entry()?;
    entry.mode = 0o100600;
    assert!(state_leaf_preimage(&entry).is_err());

    let mut entry = text_entry()?;
    entry.kind = NodeKind::Symlink;
    assert!(state_leaf_preimage(&entry).is_err());

    let mut entry = symlink_entry()?;
    entry.mode = 0o100644;
    assert!(state_leaf_preimage(&entry).is_err());
    Ok(())
}

#[test]
fn opaque_utf8_symlink_targets_remain_identity_bytes() -> prikk_error::Result<()> {
    let mut empty = symlink_entry()?;
    empty.content = StateRootContent::Symlink(String::new());
    let mut control = symlink_entry()?;
    control.node_id = NodeId::from_bytes([4; 32]);
    control.path = RepoPath::parse("other-link")?;
    control.content = StateRootContent::Symlink("/tmp/../x\n".to_string());
    assert!(state_leaf_preimage(&empty)?.ends_with(&0_u64.to_be_bytes()));
    assert!(compute_state_root(&[empty, control]).is_ok());
    Ok(())
}

#[test]
fn every_committed_field_changes_the_root() -> prikk_error::Result<()> {
    let baseline = compute_state_root(&[text_entry()?])?;
    let mut variants = Vec::new();

    let mut path = text_entry()?;
    path.path = RepoPath::parse("b.txt")?;
    variants.push(path);
    let mut node = text_entry()?;
    node.node_id = NodeId::from_bytes([9; 32]);
    variants.push(node);
    let mut kind = text_entry()?;
    kind.kind = NodeKind::BinaryFile;
    variants.push(kind);
    let mut mode = text_entry()?;
    mode.mode = 0o100755;
    variants.push(mode);
    let mut content = text_entry()?;
    content.content = StateRootContent::Blob(ObjectId::from_bytes([0x44; 32]));
    variants.push(content);

    for variant in variants {
        assert_ne!(compute_state_root(&[variant])?, baseline);
    }
    Ok(())
}
