//! Rename-declaration store tests.

#![allow(clippy::unwrap_used, clippy::indexing_slicing)]

use crate::RepositoryLayout;
use crate::rename_declaration::{
    clear_rename_declarations, read_rename_declarations, record_rename_declaration,
};
use crate::test_gates::test_support::unique_temp_dir;

#[test]
fn absent_store_reads_as_no_declarations() {
    let root = unique_temp_dir("rename-declaration-absent");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    assert!(read_rename_declarations(&layout).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_fresh_declaration_round_trips() {
    let root = unique_temp_dir("rename-declaration-fresh");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    record_rename_declaration(&layout, "a.txt", "b.txt").unwrap();
    let declarations = read_rename_declarations(&layout).unwrap();
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].old_path, "a.txt");
    assert_eq!(declarations[0].new_path, "b.txt");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_chained_declaration_collapses_to_the_net_move() {
    let root = unique_temp_dir("rename-declaration-chain");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    record_rename_declaration(&layout, "a.txt", "b.txt").unwrap();
    record_rename_declaration(&layout, "b.txt", "c.txt").unwrap();
    let declarations = read_rename_declarations(&layout).unwrap();
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].old_path, "a.txt");
    assert_eq!(declarations[0].new_path, "c.txt");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_round_trip_declaration_drops_entirely() {
    let root = unique_temp_dir("rename-declaration-round-trip");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    record_rename_declaration(&layout, "a.txt", "b.txt").unwrap();
    record_rename_declaration(&layout, "b.txt", "a.txt").unwrap();
    assert!(read_rename_declarations(&layout).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_second_declaration_for_the_same_source_replaces_the_first() {
    let root = unique_temp_dir("rename-declaration-replace");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    record_rename_declaration(&layout, "a.txt", "b.txt").unwrap();
    record_rename_declaration(&layout, "a.txt", "c.txt").unwrap();
    let declarations = read_rename_declarations(&layout).unwrap();
    assert_eq!(declarations.len(), 1);
    assert_eq!(declarations[0].old_path, "a.txt");
    assert_eq!(declarations[0].new_path, "c.txt");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn clear_empties_the_whole_store() {
    let root = unique_temp_dir("rename-declaration-clear");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    record_rename_declaration(&layout, "a.txt", "b.txt").unwrap();
    record_rename_declaration(&layout, "c.txt", "d.txt").unwrap();
    clear_rename_declarations(&layout).unwrap();
    assert!(read_rename_declarations(&layout).unwrap().is_empty());
    let _ = std::fs::remove_dir_all(root);
}
