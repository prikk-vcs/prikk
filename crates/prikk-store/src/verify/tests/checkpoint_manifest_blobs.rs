//! 0.56.0 P4: a reachable checkpoint's snapshot manifest reaches the content Blobs it names.
//!
//! A text file edited at every step has its content Blobs named by no Patch (an `EditText` names
//! none), only by the snapshot manifest a checkpoint Block carries. Before this, the walk's `Blob`
//! arm expanded nothing, so each checkpoint's content Blobs (every 64th block) read as unreached
//! objects, with the forgery-leftover warning, on an honest repository. These tests pin the step
//! itself (`expand_frontier`, the one function `verify` and `archive verify` both walk with); the
//! end-to-end shape (65 and 130 blocks, `verify`, `archive verify`) is
//! `crates/prikk-cli/tests/verify_and_doctor/p4_checkpoint_blobs_are_reached.rs`.

#![allow(clippy::expect_used)]

use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, NodeId, NodeKind, ObjectEnvelope, ObjectId, ObjectType,
};

use super::super::reachability::expand_frontier;
use crate::state_root::{StateRootContent, StateRootEntry};
use crate::{RepoPath, SnapshotManifest};

fn blob_id(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

fn entry(path: &str, node: u8, content: StateRootContent) -> StateRootEntry {
    StateRootEntry {
        path: RepoPath::parse(path).expect("path"),
        node_id: NodeId::from_bytes([node; 32]),
        kind: NodeKind::TextFile,
        mode: 0o100_644,
        content,
    }
}

fn blob_envelope(kind: BlobKind, content: Vec<u8>) -> ObjectEnvelope {
    ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        BlobPayload::new(kind, content)
            .to_canonical_bytes()
            .expect("encode blob"),
    )
}

/// The fix: a Snapshot Blob reaches every content Blob its entries name, in entry order.
#[test]
fn a_snapshot_manifest_reaches_the_content_blobs_it_names() {
    let manifest = SnapshotManifest {
        entries: vec![
            entry("a.txt", 1, StateRootContent::Blob(blob_id(0xA1))),
            entry("b.txt", 2, StateRootContent::Blob(blob_id(0xB2))),
        ],
    };
    let envelope = blob_envelope(BlobKind::Snapshot, manifest.encode().expect("encode"));
    assert_eq!(
        expand_frontier(&envelope),
        vec![blob_id(0xA1), blob_id(0xB2)],
        "both content Blobs of the checkpoint are reached through its manifest"
    );
}

/// A symlink entry names a target, not a Blob.
#[test]
fn a_symlink_entry_names_no_blob() {
    let mut link = entry("link", 3, StateRootContent::Symlink("a.txt".to_string()));
    link.kind = NodeKind::Symlink;
    link.mode = 0;
    let manifest = SnapshotManifest {
        entries: vec![
            entry("a.txt", 1, StateRootContent::Blob(blob_id(0xA1))),
            link,
        ],
    };
    let envelope = blob_envelope(BlobKind::Snapshot, manifest.encode().expect("encode"));
    assert_eq!(expand_frontier(&envelope), vec![blob_id(0xA1)]);
}

/// Only a Snapshot Blob names others: a text or binary content Blob reaches nothing, whatever its
/// bytes look like (here: a perfectly valid manifest stored as a Text Blob).
#[test]
fn a_content_blob_reaches_nothing_even_if_its_bytes_look_like_a_manifest() {
    let manifest = SnapshotManifest {
        entries: vec![entry("a.txt", 1, StateRootContent::Blob(blob_id(0xA1)))],
    };
    for kind in [BlobKind::Text, BlobKind::Binary] {
        let envelope = blob_envelope(kind, manifest.encode().expect("encode"));
        assert!(
            expand_frontier(&envelope).is_empty(),
            "{kind:?} Blobs name nothing"
        );
    }
}

/// A manifest that does not decode is treated as a Patch that does not decode is: the walk builds
/// no further past it (the manifest Blob itself stays reached, which the caller does before this
/// runs). Reporting the damage is the snapshot check's job, not the walk's.
#[test]
fn a_manifest_that_does_not_decode_expands_nothing() {
    let damaged = blob_envelope(
        BlobKind::Snapshot,
        b"PRIKK-SNAPSHOT-MANIFEST-v2\nnot a manifest".to_vec(),
    );
    assert!(expand_frontier(&damaged).is_empty());
    let wrong_magic = blob_envelope(BlobKind::Snapshot, b"not a manifest at all".to_vec());
    assert!(expand_frontier(&wrong_magic).is_empty());
}
