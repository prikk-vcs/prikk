//! Codec/importer tests for the lifecycle cache (DC-09 4.4-2b.1): round-trip, magic,
//! schema/policy provenance, file/symlink discriminator, ordering/duplicate/cross-set
//! rejection, all-zero rejection, record vs record_list_item, and the window hash.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::{
    CACHE_SCHEMA_VERSION as CACHE_SCHEMA_VERSION_TEST, DecodedLifecycleCache, ParentPolicy,
    compute_window_hash,
};

use prikk_object::{CanonicalWriter, NodeId, NodeKind, ObjectId, WireType};

use crate::node::node_lifecycle::{LiveNode, NodeContent, Tombstone};
use crate::path::RepoPath;

const MAGIC: &[u8] = b"PRIKK-NODE-LIFECYCLE-CACHE-v1\0";

fn oid(byte: u8) -> ObjectId {
    ObjectId::from_bytes([byte; 32])
}

fn nid(byte: u8) -> NodeId {
    NodeId::from_bytes([byte; 32])
}

fn rp(value: &str) -> RepoPath {
    RepoPath::parse(value).expect("valid path")
}

fn file_live(path: &str, id: u8, blob: u8, mode: u32) -> (NodeId, LiveNode) {
    (
        nid(id),
        LiveNode {
            path: rp(path),
            kind: NodeKind::TextFile,
            content: NodeContent::File {
                blob_id: oid(blob),
                mode,
            },
        },
    )
}

fn file_tomb(path: &str, id: u8, blob: u8, mode: u32) -> (NodeId, Tombstone) {
    (
        nid(id),
        Tombstone {
            kind: NodeKind::TextFile,
            content: NodeContent::File {
                blob_id: oid(blob),
                mode,
            },
            path: rp(path),
        },
    )
}

fn seen_bytes(ids: &[u8]) -> Vec<NodeId> {
    ids.iter().map(|b| nid(*b)).collect()
}

/// A valid base cache: live {0x11 a.txt, 0x22 b.txt}, tombstone {0x33}, seen union.
fn valid_cache() -> DecodedLifecycleCache {
    DecodedLifecycleCache {
        schema_version: CACHE_SCHEMA_VERSION_TEST,
        baseline_block_id: oid(0xb0),
        lineage_horizon_id: oid(0xa0),
        parent_policy: ParentPolicy::SingleParent,
        replay_window_hash: compute_window_hash(&[oid(0xa0), oid(0xb0)]),
        snapshot_blob_id: Some(oid(0x5a)),
        live_entries: vec![
            file_live("a.txt", 0x11, 0xaa, 0o100_644),
            file_live("b.txt", 0x22, 0xbb, 0o100_644),
        ],
        tombstones: vec![file_tomb("c.txt", 0x33, 0xcc, 0o100_644)],
        seen_ids: seen_bytes(&[0x11, 0x22, 0x33]),
    }
}

#[test]
fn round_trip_file_and_symlink_and_tombstone() {
    let mut cache = valid_cache();
    // add a symlink live entry (sorted path) + adjust seen union
    cache.live_entries.push((
        nid(0x44),
        LiveNode {
            path: rp("d.link"),
            kind: NodeKind::Symlink,
            content: NodeContent::Symlink {
                target: "target.txt".to_string(),
            },
        },
    ));
    cache.seen_ids = seen_bytes(&[0x11, 0x22, 0x33, 0x44]);
    let bytes = cache.encode().expect("encode");
    assert!(bytes.starts_with(MAGIC));
    let decoded = DecodedLifecycleCache::decode(&bytes).expect("decode");
    assert_eq!(decoded, cache);
}

#[test]
fn wrong_magic_rejected_before_tlv() {
    let bytes = valid_cache().encode().expect("encode");
    let mut corrupt = bytes.clone();
    corrupt[0] = b'X';
    let err = DecodedLifecycleCache::decode(&corrupt).expect_err("wrong magic");
    assert!(format!("{err:?}").contains("magic"));
}

#[test]
fn unknown_schema_version_rejected() {
    let mut cache = valid_cache();
    cache.schema_version = 999;
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("schema");
    assert!(format!("{err:?}").contains("schema_version"));
}

#[test]
fn merge_parent_policy_fails_closed() {
    let mut cache = valid_cache();
    cache.parent_policy = ParentPolicy::Dc13MergeAware;
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("merge policy");
    assert!(format!("{err:?}").contains("single_parent"));
}

#[test]
fn duplicate_live_node_id_rejected() {
    let mut cache = valid_cache();
    cache.live_entries[1].0 = nid(0x11); // duplicate id
    cache.seen_ids = seen_bytes(&[0x11, 0x33]);
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("dup live id");
    assert!(format!("{err:?}").contains("duplicate live node_id"));
}

#[test]
fn unsorted_live_paths_rejected() {
    let mut cache = valid_cache();
    cache.live_entries.swap(0, 1); // b.txt before a.txt
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("unsorted");
    assert!(format!("{err:?}").contains("sorted by repo_path"));
}

#[test]
fn seen_not_equal_union_rejected() {
    let mut cache = valid_cache();
    cache.seen_ids = seen_bytes(&[0x11, 0x22]); // missing 0x33 tombstone
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("seen union");
    assert!(format!("{err:?}").contains("live ∪ tombstoned"));
}

#[test]
fn id_both_live_and_tombstoned_rejected() {
    let mut cache = valid_cache();
    cache.tombstones = vec![file_tomb("c.txt", 0x11, 0xcc, 0o100_644)]; // 0x11 also live
    cache.seen_ids = seen_bytes(&[0x11, 0x22]);
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("both sets");
    assert!(format!("{err:?}").contains("both live and tombstone"));
}

#[test]
fn all_zero_seen_id_rejected() {
    let mut cache = valid_cache();
    cache.live_entries = vec![file_live("a.txt", 0x00, 0xaa, 0o100_644)];
    cache.tombstones = vec![];
    cache.seen_ids = vec![NodeId::from_bytes([0; 32])];
    let bytes = cache.encode_unchecked().expect("encode");
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("all-zero");
    assert!(format!("{err:?}").contains("node id") || format!("{err:?}").contains("zero"));
}

// ---- crafted structural negatives (bytes the normal encoder won't produce) ----

fn craft(live: &[(WireType, Vec<u8>)], tombs: &[(WireType, Vec<u8>)], seen: &[u8]) -> Vec<u8> {
    let mut w = CanonicalWriter::new();
    w.field_u32(1, CACHE_SCHEMA_VERSION_TEST).unwrap();
    w.field_object_id(2, &oid(0xb0)).unwrap();
    w.field_object_id(3, &oid(0xa0)).unwrap();
    w.field_enum_u16(4, 1).unwrap();
    w.field_bytes(5, &[0u8; 32]).unwrap();
    for (wire, rec) in live {
        w.field_raw(10, *wire, rec).unwrap();
    }
    for (wire, rec) in tombs {
        w.field_raw(11, *wire, rec).unwrap();
    }
    w.field_bytes(12, seen).unwrap();
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&w.finish());
    out
}

fn file_record(path: &str, id: u8, blob: u8, mode: u32) -> Vec<u8> {
    let mut w = CanonicalWriter::new();
    w.field_repo_path(1, path).unwrap();
    w.field_bytes(2, nid(id).as_bytes()).unwrap();
    w.field_enum_u16(3, NodeKind::TextFile.code()).unwrap();
    w.field_object_id(4, &oid(blob)).unwrap();
    w.field_u32(5, mode).unwrap();
    w.finish()
}

#[test]
fn tag10_encoded_as_record_rejected() {
    // tag 10 must be record_list_item (0x21); a plain record (0x20) is rejected.
    let bytes = craft(
        &[(
            WireType::Record,
            file_record("a.txt", 0x11, 0xaa, 0o100_644),
        )],
        &[],
        nid(0x11).as_bytes(),
    );
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("record not record_list_item");
    assert!(format!("{err:?}").contains("wire type"));
}

#[test]
fn symlink_record_with_mode_field_rejected() {
    let mut w = CanonicalWriter::new();
    w.field_repo_path(1, "d.link").unwrap();
    w.field_bytes(2, nid(0x44).as_bytes()).unwrap();
    w.field_enum_u16(3, NodeKind::Symlink.code()).unwrap();
    w.field_u32(5, 0).unwrap(); // forbidden field 5 (even zero)
    w.field_string(6, "target.txt").unwrap();
    let rec = w.finish();
    let bytes = craft(
        &[(WireType::RecordListItem, rec)],
        &[],
        nid(0x44).as_bytes(),
    );
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("symlink field 5");
    assert!(format!("{err:?}").contains("must not carry blob_id or normalized_mode"));
}

#[test]
fn file_record_missing_blob_rejected() {
    let mut w = CanonicalWriter::new();
    w.field_repo_path(1, "a.txt").unwrap();
    w.field_bytes(2, nid(0x11).as_bytes()).unwrap();
    w.field_enum_u16(3, NodeKind::TextFile.code()).unwrap();
    w.field_u32(5, 0o100_644).unwrap(); // mode but no blob_id
    let rec = w.finish();
    let bytes = craft(
        &[(WireType::RecordListItem, rec)],
        &[],
        nid(0x11).as_bytes(),
    );
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("missing blob");
    assert!(format!("{err:?}").contains("missing blob_id"));
}

#[test]
fn seen_ids_wrong_length_rejected() {
    let bytes = craft(&[], &[], &[0u8; 31]); // not a multiple of 32
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("seen length");
    assert!(format!("{err:?}").contains("multiple of 32"));
}

// ---- window hash ----

#[test]
fn window_hash_is_deterministic_domain_separated_and_order_sensitive() {
    let a = compute_window_hash(&[oid(0x01), oid(0x02)]);
    let b = compute_window_hash(&[oid(0x01), oid(0x02)]);
    assert_eq!(a, b, "deterministic");
    let reordered = compute_window_hash(&[oid(0x02), oid(0x01)]);
    assert_ne!(a, reordered, "order-sensitive");
    let empty = compute_window_hash(&[]);
    assert_ne!(a, empty, "count participates");
    // domain separation: not equal to a bare hash of the concatenated ids
    let mut bare = Vec::new();
    bare.extend_from_slice(oid(0x01).as_bytes());
    bare.extend_from_slice(oid(0x02).as_bytes());
    assert_ne!(a, prikk_hash::sha256(&bare));
}

// ---- erratum P1: production encode validates ----

#[test]
fn production_encode_validates_before_writing() {
    assert!(valid_cache().encode().is_ok());

    let mut unsorted = valid_cache();
    unsorted.live_entries.swap(0, 1);
    assert!(
        unsorted.encode().is_err(),
        "unsorted live must not be persistable"
    );

    let mut seen_mismatch = valid_cache();
    seen_mismatch.seen_ids = seen_bytes(&[0x11, 0x22]); // missing tombstone id
    assert!(
        seen_mismatch.encode().is_err(),
        "seen mismatch must not be persistable"
    );

    let mut merge = valid_cache();
    merge.parent_policy = ParentPolicy::Dc13MergeAware;
    assert!(
        merge.encode().is_err(),
        "merge policy must not be persistable"
    );
}

// ---- erratum P2-1: non-canonical TLV tag order rejected ----

fn raw_field(tag: u16, wire: WireType, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&tag.to_be_bytes());
    out.push(wire as u8);
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
    out
}

#[test]
fn header_field_out_of_order_rejected() {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(raw_field(2, WireType::ObjectId, oid(0xb0).as_bytes())); // baseline first
    bytes.extend(raw_field(
        1,
        WireType::U32,
        &CACHE_SCHEMA_VERSION_TEST.to_be_bytes(),
    )); // schema after
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("header order");
    assert!(format!("{err:?}").contains("TLV tag order"));
}

#[test]
fn node_record_field_out_of_order_rejected() {
    let mut rec = Vec::new();
    rec.extend(raw_field(2, WireType::Bytes, nid(0x11).as_bytes())); // node_id first
    rec.extend(raw_field(1, WireType::RepoPath, b"a.txt")); // path after
    rec.extend(raw_field(
        3,
        WireType::EnumU16,
        &NodeKind::TextFile.code().to_be_bytes(),
    ));
    rec.extend(raw_field(4, WireType::ObjectId, oid(0xaa).as_bytes()));
    rec.extend(raw_field(5, WireType::U32, &0o100_644u32.to_be_bytes()));
    let bytes = craft(
        &[(WireType::RecordListItem, rec)],
        &[],
        nid(0x11).as_bytes(),
    );
    let err = DecodedLifecycleCache::decode(&bytes).expect_err("record order");
    assert!(format!("{err:?}").contains("TLV tag order"));
}

// ---- erratum 4.4-2b.2R: production encode validates node-record shape ----

#[test]
fn encode_rejects_all_zero_live_node_id() {
    let mut cache = valid_cache();
    cache.live_entries = vec![file_live("a.txt", 0x00, 0xaa, 0o100_644)];
    cache.tombstones = vec![];
    cache.seen_ids = seen_bytes(&[0x00]);
    assert!(cache.encode().is_err());
}

#[test]
fn encode_rejects_all_zero_tombstone_node_id() {
    let mut cache = valid_cache();
    cache.live_entries = vec![];
    cache.tombstones = vec![file_tomb("c.txt", 0x00, 0xcc, 0o100_644)];
    cache.seen_ids = seen_bytes(&[0x00]);
    assert!(cache.encode().is_err());
}

#[test]
fn encode_rejects_all_zero_seen_id() {
    let mut cache = valid_cache();
    cache.live_entries = vec![];
    cache.tombstones = vec![];
    cache.seen_ids = vec![NodeId::from_bytes([0; 32])];
    assert!(cache.encode().is_err());
}

#[test]
fn encode_rejects_file_kind_with_symlink_content() {
    let mut cache = valid_cache();
    cache.live_entries = vec![(
        nid(0x11),
        LiveNode {
            path: rp("a.txt"),
            kind: NodeKind::TextFile,
            content: NodeContent::Symlink {
                target: "x".to_string(),
            },
        },
    )];
    cache.tombstones = vec![];
    cache.seen_ids = seen_bytes(&[0x11]);
    assert!(cache.encode().is_err());
}

#[test]
fn encode_rejects_symlink_kind_with_file_content() {
    let mut cache = valid_cache();
    cache.live_entries = vec![(
        nid(0x11),
        LiveNode {
            path: rp("a.txt"),
            kind: NodeKind::Symlink,
            content: NodeContent::File {
                blob_id: oid(0xaa),
                mode: 0o100_644,
            },
        },
    )];
    cache.tombstones = vec![];
    cache.seen_ids = seen_bytes(&[0x11]);
    assert!(cache.encode().is_err());
}

// ---- N1: explicit tombstone kind/content production-encode negatives ----

#[test]
fn encode_rejects_tombstone_file_kind_with_symlink_content() {
    let mut cache = valid_cache();
    cache.tombstones = vec![(
        nid(0x33),
        Tombstone {
            kind: NodeKind::BinaryFile,
            content: NodeContent::Symlink {
                target: "x".to_string(),
            },
            path: rp("c.txt"),
        },
    )];
    assert!(cache.encode().is_err());
}

#[test]
fn encode_rejects_tombstone_symlink_kind_with_file_content() {
    let mut cache = valid_cache();
    cache.tombstones = vec![(
        nid(0x33),
        Tombstone {
            kind: NodeKind::Symlink,
            content: NodeContent::File {
                blob_id: oid(0xcc),
                mode: 0o100_644,
            },
            path: rp("c.txt"),
        },
    )];
    assert!(cache.encode().is_err());
}

// ---- 4.4-2b.2 step 1: blob-kind verification -> ValidatedLifecycleCache ----

use super::{BlobKindResolver, ValidatedLifecycleCache};
use prikk_object::BlobKind;
use std::collections::BTreeMap;

struct MapResolver(BTreeMap<[u8; 32], BlobKind>);

impl BlobKindResolver for MapResolver {
    fn blob_kind(&self, blob_id: &ObjectId) -> prikk_error::Result<Option<BlobKind>> {
        Ok(self.0.get(blob_id.as_bytes()).copied())
    }
}

fn resolver(pairs: &[(u8, BlobKind)]) -> MapResolver {
    MapResolver(pairs.iter().map(|(b, k)| ([*b; 32], *k)).collect())
}

fn decoded_valid() -> DecodedLifecycleCache {
    DecodedLifecycleCache::decode(&valid_cache().encode().expect("encode")).expect("decode")
}

fn decoded_of(cache: &DecodedLifecycleCache) -> DecodedLifecycleCache {
    DecodedLifecycleCache::decode(&cache.encode().expect("encode")).expect("decode")
}

#[test]
fn validated_accepts_matching_blob_kinds() {
    // valid_cache: live blobs 0xaa,0xbb (TextFile) + tombstone blob 0xcc (TextFile)
    let res = resolver(&[
        (0xaa, BlobKind::Text),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Text),
    ]);
    assert!(ValidatedLifecycleCache::from_decoded(decoded_valid(), &res, &genesis_chain()).is_ok());
}

#[test]
fn validated_rejects_blob_kind_mismatch() {
    // 0xaa is referenced by a TextFile entry but the blob is Binary
    let res = resolver(&[
        (0xaa, BlobKind::Binary),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Text),
    ]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &res, &genesis_chain())
        .expect_err("mismatch");
    assert!(format!("{err:?}").contains("disagrees with referenced"));
}

#[test]
fn validated_rejects_missing_blob() {
    // 0xbb absent -> fail closed
    let res = resolver(&[(0xaa, BlobKind::Text), (0xcc, BlobKind::Text)]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &res, &genesis_chain())
        .expect_err("missing");
    assert!(format!("{err:?}").contains("blob required for kind"));
}

// ---- step-1 P1: from_decoded re-validates structurally (no caller-discipline trust) ----

#[test]
fn validated_rejects_structurally_invalid_decoded_even_if_blobs_resolve() {
    // Directly constructed (not via decode) with a seen_ids ∪ violation; blob kinds resolve fine.
    let mut invalid = valid_cache();
    invalid.seen_ids = seen_bytes(&[0x11, 0x22, 0x33, 0x44]); // 0x44 not in live ∪ tombstoned
    let res = resolver(&[
        (0xaa, BlobKind::Text),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Text),
    ]);
    let err = ValidatedLifecycleCache::from_decoded(invalid, &res, &genesis_chain())
        .expect_err("structural first");
    assert!(format!("{err:?}").contains("live ∪ tombstoned"));
}

#[test]
fn validated_accepts_binary_file_kind() {
    let mut cache = valid_cache();
    // make a.txt a BinaryFile referencing 0xaa
    cache.live_entries[0] = (
        nid(0x11),
        LiveNode {
            path: rp("a.txt"),
            kind: NodeKind::BinaryFile,
            content: NodeContent::File {
                blob_id: oid(0xaa),
                mode: 0o100_644,
            },
        },
    );
    let res = resolver(&[
        (0xaa, BlobKind::Binary),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Text),
    ]);
    assert!(
        ValidatedLifecycleCache::from_decoded(decoded_of(&cache), &res, &genesis_chain()).is_ok()
    );
}

#[test]
fn validated_rejects_snapshot_blob_for_file_entry() {
    let res = resolver(&[
        (0xaa, BlobKind::Snapshot),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Text),
    ]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &res, &genesis_chain())
        .expect_err("snapshot");
    assert!(format!("{err:?}").contains("SNAPSHOT"));
}

#[test]
fn validated_rejects_tombstone_blob_kind_mismatch() {
    // tombstone c.txt (TextFile) blob 0xcc resolves as Binary
    let res = resolver(&[
        (0xaa, BlobKind::Text),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Binary),
    ]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &res, &genesis_chain())
        .expect_err("tombstone mismatch");
    assert!(format!("{err:?}").contains("disagrees with referenced"));
}

struct ErrResolver;
impl BlobKindResolver for ErrResolver {
    fn blob_kind(&self, _blob_id: &ObjectId) -> prikk_error::Result<Option<BlobKind>> {
        Err(prikk_error::PrikkError::Integrity(
            "resolver boom".to_string(),
        ))
    }
}

#[test]
fn validated_propagates_resolver_error_fail_closed() {
    let err =
        ValidatedLifecycleCache::from_decoded(decoded_valid(), &ErrResolver, &genesis_chain())
            .expect_err("resolver error");
    assert!(format!("{err:?}").contains("resolver boom"));
}

// ---- 4.4-2b.2 step 2: walked-chain provenance ----

use super::BlockParentResolver;

struct BlockMapResolver(BTreeMap<[u8; 32], Vec<ObjectId>>);

impl BlockParentResolver for BlockMapResolver {
    fn parent_block_ids(&self, block_id: &ObjectId) -> prikk_error::Result<Vec<ObjectId>> {
        Ok(self.0.get(block_id.as_bytes()).cloned().unwrap_or_default())
    }
}

/// Build a parent map: each `(block, parents)` pair maps a block byte to its parents.
fn parents(pairs: &[(u8, &[u8])]) -> BlockMapResolver {
    let mut map = BTreeMap::new();
    for (block, ps) in pairs {
        map.insert([*block; 32], ps.iter().map(|p| oid(*p)).collect());
    }
    BlockMapResolver(map)
}

/// The chain matching `valid_cache()`: baseline 0xb0 -> horizon 0xa0 (genesis).
fn genesis_chain() -> BlockMapResolver {
    parents(&[(0xb0, &[0xa0]), (0xa0, &[])])
}

fn full_resolver() -> MapResolver {
    resolver(&[
        (0xaa, BlobKind::Text),
        (0xbb, BlobKind::Text),
        (0xcc, BlobKind::Text),
    ])
}

#[test]
fn provenance_accepts_matching_walked_chain() {
    assert!(
        ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &genesis_chain())
            .is_ok()
    );
}

#[test]
fn provenance_rejects_window_hash_mismatch() {
    // baseline -> 0x77 -> genesis: a different chain, so the recomputed hash won't match.
    let chain = parents(&[(0xb0, &[0x77]), (0x77, &[0xa0]), (0xa0, &[])]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &chain)
        .expect_err("hash mismatch");
    assert!(format!("{err:?}").contains("does not match the walked"));
}

#[test]
fn provenance_rejects_merge_block() {
    // `BlockParentResolver`/`ResolverLineage` (this trust-ladder scaffold, test-only, not
    // production per DC-75's investigation) carries no `BlockKind` at all -- it cannot distinguish
    // a legitimate `Merge` block's mainline parent from a malformed multi-parent block, so it still
    // fails closed on any >1-parent block exactly as before DC-75. Message text updated to match
    // `LifecycleReplayError::MergeLineageUnsupported`'s DC-75 wording (`replay.rs`); behavior
    // unchanged.
    let chain = parents(&[(0xb0, &[0xa0, 0xc0]), (0xa0, &[]), (0xc0, &[])]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &chain)
        .expect_err("merge");
    assert!(format!("{err:?}").contains("no valid mainline parent"));
}

#[test]
fn provenance_rejects_horizon_not_genesis() {
    // horizon 0xa0 has a parent -> not genesis -> inadequate horizon in v1.
    let chain = parents(&[(0xb0, &[0xa0]), (0xa0, &[0x90]), (0x90, &[])]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &chain)
        .expect_err("not genesis");
    assert!(format!("{err:?}").contains("claimed horizon"));
}

#[test]
fn provenance_rejects_cycle() {
    let chain = parents(&[(0xb0, &[0x77]), (0x77, &[0xb0])]); // 0xb0 -> 0x77 -> 0xb0
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &chain)
        .expect_err("cycle");
    assert!(format!("{err:?}").contains("cycle"));
}

#[test]
fn provenance_rejects_genesis_before_horizon() {
    // baseline hits genesis (no parents) before reaching the claimed horizon 0xa0.
    let chain = parents(&[(0xb0, &[])]);
    let err = ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &chain)
        .expect_err("early genesis");
    assert!(format!("{err:?}").contains("claimed horizon"));
}

// ---- 4.4-2b.2 step 3/4: replay-derived + compared rungs ----

use super::{ComparedLifecycleCache, ReplayDerivedLifecycleState};
use crate::node::node_lifecycle::NodeLifecycleState;

fn validated() -> ValidatedLifecycleCache {
    ValidatedLifecycleCache::from_decoded(decoded_valid(), &full_resolver(), &genesis_chain())
        .expect("validated")
}

/// Build an authoritative replay reference by seeding a state (simulating replay output).
fn replay_state(
    baseline: u8,
    live: &[(u8, &str, u8)],
    tombs: &[(u8, &str, u8)],
) -> ReplayDerivedLifecycleState {
    let mut state = NodeLifecycleState::new();
    for (id, path, blob) in live {
        state
            .seed_live_node(
                nid(*id),
                LiveNode {
                    path: rp(path),
                    kind: NodeKind::TextFile,
                    content: NodeContent::File {
                        blob_id: oid(*blob),
                        mode: 0o100_644,
                    },
                },
            )
            .unwrap();
    }
    for (id, path, blob) in tombs {
        state
            .seed_tombstone(
                nid(*id),
                Tombstone {
                    kind: NodeKind::TextFile,
                    content: NodeContent::File {
                        blob_id: oid(*blob),
                        mode: 0o100_644,
                    },
                    path: rp(path),
                },
            )
            .unwrap();
    }
    ReplayDerivedLifecycleState::from_replay(oid(baseline), state).expect("consistent replay state")
}

fn matching_replay() -> ReplayDerivedLifecycleState {
    replay_state(
        0xb0,
        &[(0x11, "a.txt", 0xaa), (0x22, "b.txt", 0xbb)],
        &[(0x33, "c.txt", 0xcc)],
    )
}

#[test]
fn compared_accepts_cache_matching_replay() {
    assert!(
        ComparedLifecycleCache::from_validated_and_replay(validated(), &matching_replay()).is_ok()
    );
}

#[test]
fn compared_rejects_false_tombstone() {
    // Cache claims node 0x33 tombstoned with blob 0xcc; authoritative replay has 0xdd.
    // Provenance is correct (same baseline/window) but the tombstone content is false.
    let replay = replay_state(
        0xb0,
        &[(0x11, "a.txt", 0xaa), (0x22, "b.txt", 0xbb)],
        &[(0x33, "c.txt", 0xdd)],
    );
    let err = ComparedLifecycleCache::from_validated_and_replay(validated(), &replay)
        .expect_err("false tombstone");
    assert!(matches!(
        err,
        super::CacheCertificationError::ContentMismatch
    ));
    assert!(format!("{err}").contains("disagree with authoritative replay"));
}

#[test]
fn compared_rejects_baseline_mismatch() {
    let replay = replay_state(
        0xff,
        &[(0x11, "a.txt", 0xaa), (0x22, "b.txt", 0xbb)],
        &[(0x33, "c.txt", 0xcc)],
    );
    let err = ComparedLifecycleCache::from_validated_and_replay(validated(), &replay)
        .expect_err("baseline mismatch");
    assert!(matches!(
        err,
        super::CacheCertificationError::BaselineMismatch { .. }
    ));
    assert!(format!("{err}").contains("baseline mismatch"));
}

#[test]
fn from_decoded_for_baseline_accepts_matching() {
    assert!(
        ValidatedLifecycleCache::from_decoded_for_baseline(
            decoded_valid(),
            oid(0xb0),
            &full_resolver(),
            &genesis_chain()
        )
        .is_ok()
    );
}

#[test]
fn from_decoded_for_baseline_rejects_wrong_baseline() {
    let err = ValidatedLifecycleCache::from_decoded_for_baseline(
        decoded_valid(),
        oid(0xee),
        &full_resolver(),
        &genesis_chain(),
    )
    .expect_err("wrong baseline");
    assert!(format!("{err:?}").contains("intended baseline"));
}

#[test]
fn compared_accepts_post_restore_baseline() {
    // Authoritative replay: create N -> delete N -> restore N => N live, no tombstone.
    let mut state = NodeLifecycleState::new();
    let live = LiveNode {
        path: rp("a.txt"),
        kind: NodeKind::TextFile,
        content: NodeContent::File {
            blob_id: oid(0xaa),
            mode: 0o100_644,
        },
    };
    state.seed_live_node(nid(0x11), live.clone()).unwrap();
    // simulate the post-restore authoritative state directly: live, no tombstone, seen={N}
    let replay =
        ReplayDerivedLifecycleState::from_replay(oid(0xb0), state).expect("consistent replay");

    // Cache for the same baseline: N live, no tombstone, seen = {N}.
    let cache = DecodedLifecycleCache {
        schema_version: CACHE_SCHEMA_VERSION_TEST,
        baseline_block_id: oid(0xb0),
        lineage_horizon_id: oid(0xa0),
        parent_policy: ParentPolicy::SingleParent,
        replay_window_hash: compute_window_hash(&[oid(0xa0), oid(0xb0)]),
        snapshot_blob_id: None,
        live_entries: vec![file_live("a.txt", 0x11, 0xaa, 0o100_644)],
        tombstones: vec![],
        seen_ids: seen_bytes(&[0x11]),
    };
    let validated = ValidatedLifecycleCache::from_decoded(
        decoded_of(&cache),
        &resolver(&[(0xaa, BlobKind::Text)]),
        &genesis_chain(),
    )
    .expect("validated");
    assert!(ComparedLifecycleCache::from_validated_and_replay(validated, &replay).is_ok());
}
