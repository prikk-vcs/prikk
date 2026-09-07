//! Received-ref pointer storage tests. Wire-level round-trip/totality coverage for the underlying
//! container format lives in `received_index/tests.rs` now -- this file tests only `received.rs`'s
//! own public behavior (validation, write/read/list through the container).

use prikk_object::ObjectId;

use crate::received::{list_received_pointers, read_received_pointer, validate_received_ref};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{RepositoryLayout, received};

fn fake_object_id(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

#[test]
fn validate_received_ref_requires_the_remotes_prefix() {
    assert!(validate_received_ref("remotes/heads/main").is_ok());
    assert!(validate_received_ref("heads/main").is_err());
    assert!(validate_received_ref("tags/v1").is_err());
    assert!(validate_received_ref("remotes/").is_err());
    assert!(validate_received_ref("").is_err());
    assert!(validate_received_ref("remotes/../heads/main").is_err());
}

#[test]
fn write_then_read_round_trips() -> prikk_error::Result<()> {
    let root = unique_temp_dir("received-round-trip");
    let layout = RepositoryLayout::init(root.clone())?;
    let id = fake_object_id(0x11);

    assert!(read_received_pointer(&layout, "remotes/heads/main")?.is_none());
    received::write_received_pointer(&layout, "remotes/heads/main", id)?;
    let loaded = read_received_pointer(&layout, "remotes/heads/main")?;
    assert!(loaded.is_some());
    if let Some(loaded) = loaded {
        assert_eq!(loaded.ref_name, "remotes/heads/main");
        assert_eq!(loaded.ref_state_id, id);
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_second_import_replaces_the_first() -> prikk_error::Result<()> {
    let root = unique_temp_dir("received-replace");
    let layout = RepositoryLayout::init(root.clone())?;
    received::write_received_pointer(&layout, "remotes/heads/main", fake_object_id(0x22))?;
    received::write_received_pointer(&layout, "remotes/heads/main", fake_object_id(0x33))?;

    let loaded = read_received_pointer(&layout, "remotes/heads/main")?;
    assert_eq!(
        loaded.map(|pointer| pointer.ref_state_id),
        Some(fake_object_id(0x33))
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn list_is_empty_before_any_import_and_sorted_after() -> prikk_error::Result<()> {
    let root = unique_temp_dir("received-list");
    let layout = RepositoryLayout::init(root.clone())?;
    assert!(list_received_pointers(&layout)?.is_empty());

    received::write_received_pointer(&layout, "remotes/heads/z", fake_object_id(0x44))?;
    received::write_received_pointer(&layout, "remotes/heads/a", fake_object_id(0x55))?;
    let listed = list_received_pointers(&layout)?;
    let names: Vec<&str> = listed
        .iter()
        .map(|pointer| pointer.ref_name.as_str())
        .collect();
    assert_eq!(names, vec!["remotes/heads/a", "remotes/heads/z"]);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
