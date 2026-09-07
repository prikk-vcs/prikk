//! Commit-index cache tests.

#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;

use prikk_object::{BlobKind, CanonicalEncode, ObjectId};

use crate::RepositoryLayout;
use crate::foundation::fsutil::RootFileStat;
use crate::test_gates::test_support::unique_temp_dir;

use super::{CommitIndex, CommitIndexEntry, content_hash};

fn sample_entry(seed: u8) -> CommitIndexEntry {
    let mut bytes = [0_u8; 32];
    bytes[0] = seed;
    CommitIndexEntry {
        size: 42,
        mtime_secs: 1_700_000_000,
        mtime_nanos: 123_456,
        mode: 0o100_644,
        kind: BlobKind::Text,
        content_hash: ObjectId::from_bytes(bytes),
    }
}

#[test]
fn empty_index_loads_from_a_missing_file() {
    let root = unique_temp_dir("commit-index-missing");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let index = CommitIndex::load(&layout);
        assert!(index.is_ok());
        if let Ok(index) = index {
            assert_eq!(index.len(), 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn saved_index_round_trips_through_load() {
    let root = unique_temp_dir("commit-index-roundtrip");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let mut index = CommitIndex::default();
        index.record("a/one.txt".to_string(), sample_entry(1));
        index.record("two.bin".to_string(), sample_entry(2));
        assert!(index.save(&layout).is_ok());

        let reloaded = CommitIndex::load(&layout);
        assert!(reloaded.is_ok());
        if let Ok(reloaded) = reloaded {
            assert_eq!(reloaded.entries(), index.entries());
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_corrupt_index_file_loads_as_empty_not_an_error() {
    let root = unique_temp_dir("commit-index-corrupt");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let relative = layout.repository_relative(&layout.cache_dir().join("commit-index.v1"));
        assert!(relative.is_ok());
        if let Ok(relative) = relative {
            let write = crate::foundation::fsutil::write_file_atomically(
                layout.repository_mutation_root(),
                &relative,
                b"not a valid commit index at all",
            );
            assert!(write.is_ok());
        }

        let index = CommitIndex::load(&layout);
        assert!(index.is_ok());
        if let Ok(index) = index {
            assert_eq!(index.len(), 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn retain_paths_drops_entries_absent_from_the_worktree() {
    let mut index = CommitIndex::default();
    index.record("keep.txt".to_string(), sample_entry(1));
    index.record("gone.txt".to_string(), sample_entry(2));

    let mut live = BTreeSet::new();
    live.insert("keep.txt".to_string());
    index.retain_paths(&live);

    assert_eq!(index.len(), 1);
    assert!(index.get("keep.txt").is_some());
    assert!(index.get("gone.txt").is_none());
}

#[test]
fn matches_stat_requires_size_and_mtime_equality() {
    let entry = sample_entry(1);
    let same = RootFileStat {
        size: entry.size,
        mtime_secs: entry.mtime_secs,
        mtime_nanos: entry.mtime_nanos,
        mode: Some(entry.mode),
    };
    assert!(entry.matches_stat(&same));

    let different_size = RootFileStat {
        size: entry.size + 1,
        ..same
    };
    assert!(!entry.matches_stat(&different_size));

    let different_mtime = RootFileStat {
        mtime_nanos: entry.mtime_nanos + 1,
        ..same
    };
    assert!(!entry.matches_stat(&different_mtime));
}

#[test]
fn content_hash_matches_the_blob_payload_formula_for_its_kind() {
    let bytes = b"hello prikk";
    let hash = content_hash(BlobKind::Text, bytes);
    assert!(hash.is_ok());
    if let Ok(hash) = hash {
        let payload = prikk_object::BlobPayload::new(BlobKind::Text, bytes.to_vec());
        let canonical = payload.to_canonical_bytes();
        assert!(canonical.is_ok());
        if let Ok(canonical) = canonical {
            let expected =
                ObjectId::from_canonical_payload(prikk_object::ObjectType::Blob, 1, &canonical);
            assert_eq!(hash, expected);
        }
    }

    let text_hash = content_hash(BlobKind::Text, bytes);
    let binary_hash = content_hash(BlobKind::Binary, bytes);
    assert!(text_hash.is_ok() && binary_hash.is_ok());
    if let (Ok(text_hash), Ok(binary_hash)) = (text_hash, binary_hash) {
        assert_ne!(text_hash, binary_hash);
    }
}
