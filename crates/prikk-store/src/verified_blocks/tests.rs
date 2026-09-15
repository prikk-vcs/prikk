//! The record's own format controls (RFC 136 increment 2b §3, "record failure is safe").

use prikk_object::ObjectId;

use super::{RECORD_MAGIC, encode, load_verified_blocks, record_path, record_verified_blocks};
use crate::RepositoryLayout;
use crate::test_gates::test_support::unique_temp_dir;

fn id(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

#[test]
fn the_record_round_trips_and_only_grows() -> prikk_error::Result<()> {
    let root = unique_temp_dir("verified-blocks-round-trip");
    let layout = RepositoryLayout::init(root.clone())?;
    assert!(
        load_verified_blocks(&layout).is_empty(),
        "absent reads empty"
    );
    record_verified_blocks(&layout, [id(1), id(2)]);
    record_verified_blocks(&layout, [id(2), id(3)]);
    assert_eq!(
        load_verified_blocks(&layout)
            .into_iter()
            .collect::<Vec<_>>(),
        vec![id(1), id(2), id(3)]
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A flipped byte, a truncation, and another `prikk-store` version each read as the empty set, with no
/// error.
#[test]
fn a_damaged_or_foreign_record_reads_empty() -> prikk_error::Result<()> {
    let root = unique_temp_dir("verified-blocks-damaged");
    let layout = RepositoryLayout::init(root.clone())?;
    record_verified_blocks(&layout, [id(1), id(2)]);
    let path = record_path(&layout);
    let good = std::fs::read(&path)?;
    assert_eq!(load_verified_blocks(&layout).len(), 2, "fixture sanity");

    let mut flipped = good.clone();
    if let Some(byte) = flipped.last_mut() {
        *byte ^= 0x01;
    }
    std::fs::write(&path, &flipped)?;
    assert!(
        load_verified_blocks(&layout).is_empty(),
        "a flipped byte reads empty"
    );

    std::fs::write(
        &path,
        good.get(..good.len().saturating_sub(7)).unwrap_or_default(),
    )?;
    assert!(
        load_verified_blocks(&layout).is_empty(),
        "a truncation reads empty"
    );

    // The same set under another crate version, with a valid checksum, so only the version differs.
    let mut foreign = good.clone();
    let version = env!("CARGO_PKG_VERSION").as_bytes();
    let start = RECORD_MAGIC.len() + 32 + 4 + 2;
    if let Some(byte) = foreign.get_mut(start + version.len() - 1) {
        *byte = if *byte == b'9' { b'8' } else { b'9' };
    }
    let checksum = prikk_hash::sha256(foreign.get(RECORD_MAGIC.len() + 32..).unwrap_or_default());
    if let Some(slot) = foreign.get_mut(RECORD_MAGIC.len()..RECORD_MAGIC.len() + 32) {
        slot.copy_from_slice(&checksum);
    }
    std::fs::write(&path, &foreign)?;
    assert!(
        load_verified_blocks(&layout).is_empty(),
        "another version reads empty"
    );

    std::fs::write(&path, encode(&[id(9)].into_iter().collect()))?;
    assert_eq!(
        load_verified_blocks(&layout).len(),
        1,
        "a good record reads again"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 136 increment 2b §1: only operations that have just confirmed roots by replay write the record.
/// `bundle import`, `sync accept`, `branch create`, `checkout`, `status` and `worktree-status` cannot add
/// an id because nothing but `block_state.rs` (seal) and `verify.rs` calls the writer.
#[test]
fn only_seal_and_verify_write_the_record() {
    const WRITERS: &[&str] = &["block_state.rs", "verify.rs"];
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut stack = vec![src.clone()];
    let mut callers = Vec::new();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if path.extension().and_then(|e| e.to_str()) != Some("rs")
                || name == "tests.rs"
                || name == "verified_blocks.rs"
                || path.components().any(|c| c.as_os_str() == "tests")
            {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            if text.lines().any(|line| {
                !line.trim_start().starts_with("//") && line.contains("record_verified_blocks(")
            }) {
                callers.push(
                    path.strip_prefix(&src)
                        .unwrap_or(&path)
                        .display()
                        .to_string(),
                );
            }
        }
    }
    callers.sort();
    assert_eq!(callers, WRITERS, "the record's writers");
}
