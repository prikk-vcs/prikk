//! Store decode coverage for the FDD-03 §9.3 operation records: node_id
//! validation (all-zero rejection through `try_from_bytes`), field types, the
//! DeleteNode discriminator (text/binary file, symlink, and rejection of illegal
//! field combinations), the §9.2 operation-kind oneof, and the decode→apply
//! boundary for node-addressed kinds (each well-formed kind decodes into its
//! typed variant; application of still-deferred kinds is gated as
//! `UnsupportedObjectType` by `ensure_apply_supported`, erratum P1) — exercised on
//! the read path, not just the write-side encoder validators.
#![allow(clippy::expect_used)]

use prikk_error::PrikkError;
use prikk_object::{
    CanonicalEncode, ChangePerm, CreateFile, CreateSymlink, DeleteNode, DeleteNodePreimage,
    EditText, NodeId, NodeKind, ObjectId, Operation, OperationKind, PatchPayload, PatchPurpose,
    RenamePath, ReplaceBinary, text_span_hash,
};

use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, decode_patch_operations, ensure_apply_supported,
};

/// Wrap one operation in a single-op patch payload and return canonical bytes.
fn patch_bytes(kind: OperationKind) -> Vec<u8> {
    let patch = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind,
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    patch.to_canonical_bytes().expect("patch encodes")
}

/// Minimal TLV field emitter for crafting malformed records the canonical
/// encoder would refuse to produce.
fn tlv(out: &mut Vec<u8>, tag: u16, value_type: u8, value: &[u8]) {
    out.extend_from_slice(&tag.to_be_bytes());
    out.push(value_type);
    out.extend_from_slice(&(value.len() as u64).to_be_bytes());
    out.extend_from_slice(value);
}

/// Compose patch bytes carrying one operation whose tag-11 delete_node record is
/// supplied raw, so discriminator negatives can be tested on the read path.
fn patch_with_raw_delete_node(delete_node_record: &[u8]) -> Vec<u8> {
    let mut operation = Vec::new();
    tlv(&mut operation, 1, 0x03, &1_u32.to_be_bytes()); // op_seq u32
    tlv(&mut operation, 11, 0x20, delete_node_record); // delete_node record
    let mut patch = Vec::new();
    tlv(&mut patch, 1, 0x21, &operation); // operations record_list_item
    patch
}

#[test]
fn decode_create_file_round_trips_node_bearing_fields() {
    let bytes = patch_bytes(OperationKind::CreateFile(CreateFile {
        path: "a.txt".to_string(),
        node_id: NodeId::from_bytes([0x42; 32]),
        blob_id: ObjectId::from_bytes([0x11; 32]),
        mode: 0o100_644,
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    assert_eq!(ops.len(), 1);
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::CreateFile {
            path,
            node_id,
            blob_id,
            mode,
        } => {
            assert_eq!(path, "a.txt");
            assert_eq!(node_id.as_bytes(), &[0x42; 32]);
            assert_eq!(blob_id, &ObjectId::from_bytes([0x11; 32]));
            assert_eq!(*mode, 0o100_644);
        }
        other => panic!("expected CreateFile, got {other:?}"),
    }
}

#[test]
fn decode_delete_node_text_and_binary_file_kinds() {
    for kind in [NodeKind::TextFile, NodeKind::BinaryFile] {
        let bytes = patch_bytes(OperationKind::DeleteNode(DeleteNode {
            path: "a.txt".to_string(),
            node_id: NodeId::from_bytes([0x43; 32]),
            old_node_kind: kind,
            preimage: DeleteNodePreimage::File {
                old_blob_id: ObjectId::from_bytes([0x11; 32]),
                old_mode: 0o100_644,
            },
        }));
        let ops = decode_patch_operations(&bytes, 1).expect("decodes");
        match &ops.first().expect("one operation").kind {
            DecodedOperationKind::DeleteNode {
                preimage:
                    DecodedDeletePreimage::File {
                        old_node_kind,
                        old_blob_id,
                        old_mode,
                    },
                ..
            } => {
                assert_eq!(*old_node_kind, kind);
                assert_eq!(old_blob_id, &ObjectId::from_bytes([0x11; 32]));
                assert_eq!(*old_mode, 0o100_644);
            }
            other => panic!("expected DeleteNode, got {other:?}"),
        }
    }
}

/// Compose patch bytes carrying one operation whose tag-10 create_file record is
/// supplied raw, so node_id negatives can be tested on the read path (the object
/// encoder now refuses to produce an all-zero node_id).
fn patch_with_raw_create_file(create_file_record: &[u8]) -> Vec<u8> {
    let mut operation = Vec::new();
    tlv(&mut operation, 1, 0x03, &1_u32.to_be_bytes()); // op_seq u32
    tlv(&mut operation, 10, 0x20, create_file_record); // create_file record
    let mut patch = Vec::new();
    tlv(&mut patch, 1, 0x21, &operation); // operations record_list_item
    patch
}

#[test]
fn decode_rejects_all_zero_node_id_in_create_file() {
    // The object encoder now rejects all-zero node_id, so this must be crafted raw.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"a.txt"); // path repo_path
    tlv(&mut record, 2, 0x11, &[0x00; 32]); // node_id all-zero (illegal)
    tlv(&mut record, 3, 0x12, &[0x11; 32]); // blob_id object_id
    tlv(&mut record, 4, 0x03, &0o100_644_u32.to_be_bytes()); // mode
    let bytes = patch_with_raw_create_file(&record);
    assert!(decode_patch_operations(&bytes, 1).is_err());
}

#[test]
fn decode_rejects_all_zero_node_id_in_delete_node() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"a.txt");
    tlv(&mut record, 2, 0x11, &[0x00; 32]); // node_id all-zero (illegal)
    tlv(
        &mut record,
        3,
        0x05,
        &NodeKind::TextFile.code().to_be_bytes(),
    );
    tlv(&mut record, 4, 0x12, &[0x11; 32]);
    tlv(&mut record, 6, 0x03, &0o100_644_u32.to_be_bytes());
    let bytes = patch_with_raw_delete_node(&record);
    assert!(decode_patch_operations(&bytes, 1).is_err());
}

#[test]
fn decode_symlink_delete_node_then_apply_unsupported() {
    let bytes = patch_bytes(OperationKind::DeleteNode(DeleteNode {
        path: "link".to_string(),
        node_id: NodeId::from_bytes([0x44; 32]),
        old_node_kind: NodeKind::Symlink,
        preimage: DeleteNodePreimage::Symlink {
            old_target: "t.txt".to_string(),
        },
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::DeleteNode {
            path,
            node_id,
            preimage: DecodedDeletePreimage::Symlink { old_target },
        } => {
            assert_eq!(path, "link");
            assert_eq!(node_id.as_bytes(), &[0x44; 32]);
            assert_eq!(old_target, "t.txt");
        }
        other => panic!("expected symlink DeleteNode, got {other:?}"),
    }
    let err = ensure_apply_supported(ops.first().expect("one operation")).expect_err("deferred");
    assert!(matches!(err, PrikkError::UnsupportedObjectType(_)));
}

#[test]
fn decode_rejects_file_kind_carrying_old_target() {
    // Illegal combination: TextFile kind with an old_target field present.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"a.txt"); // path repo_path
    tlv(&mut record, 2, 0x11, &[0x45; 32]); // node_id bytes
    tlv(
        &mut record,
        3,
        0x05,
        &NodeKind::TextFile.code().to_be_bytes(),
    ); // enum_u16
    tlv(&mut record, 4, 0x12, &[0x11; 32]); // old_blob_id object_id
    tlv(&mut record, 5, 0x10, b"target"); // old_target (illegal for file)
    tlv(&mut record, 6, 0x03, &0o100_644_u32.to_be_bytes()); // old_mode
    let bytes = patch_with_raw_delete_node(&record);
    assert!(decode_patch_operations(&bytes, 1).is_err());
}

#[test]
fn decode_rejects_file_kind_missing_old_blob_id() {
    // Illegal: file kind without the required old_blob_id.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"a.txt");
    tlv(&mut record, 2, 0x11, &[0x46; 32]);
    tlv(
        &mut record,
        3,
        0x05,
        &NodeKind::BinaryFile.code().to_be_bytes(),
    );
    tlv(&mut record, 6, 0x03, &0o100_644_u32.to_be_bytes());
    let bytes = patch_with_raw_delete_node(&record);
    assert!(decode_patch_operations(&bytes, 1).is_err());
}

#[test]
fn decode_rejects_symlink_kind_carrying_blob_and_mode() {
    // Illegal: symlink kind with file-only fields.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"link");
    tlv(&mut record, 2, 0x11, &[0x47; 32]);
    tlv(
        &mut record,
        3,
        0x05,
        &NodeKind::Symlink.code().to_be_bytes(),
    );
    tlv(&mut record, 4, 0x12, &[0x11; 32]); // old_blob_id (illegal for symlink)
    tlv(&mut record, 6, 0x03, &0o100_644_u32.to_be_bytes()); // old_mode (illegal)
    let bytes = patch_with_raw_delete_node(&record);
    assert!(decode_patch_operations(&bytes, 1).is_err());
}

/// Compose patch bytes carrying one operation whose tag-12 edit_text record is
/// supplied raw, so §9.3 EditText validator negatives can be tested on the read
/// path (the object encoder refuses to produce invalid records).
fn patch_with_raw_edit_text(edit_text_record: &[u8]) -> Vec<u8> {
    let mut operation = Vec::new();
    tlv(&mut operation, 1, 0x03, &1_u32.to_be_bytes()); // op_seq u32
    tlv(&mut operation, 12, 0x20, edit_text_record); // edit_text record
    let mut patch = Vec::new();
    tlv(&mut patch, 1, 0x21, &operation); // operations record_list_item
    patch
}

/// A valid §9.3 EditText record body (tags 1-6, 9; no hints).
fn valid_edit_text_record() -> Vec<u8> {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]); // node_id
    tlv(&mut record, 2, 0x11, &[0x10; 32]); // span_id
    tlv(&mut record, 3, 0x11, &text_span_hash(b"old")); // old_span_hash
    tlv(&mut record, 4, 0x11, &[0x11; 32]); // left_anchor_hash
    tlv(&mut record, 5, 0x11, &[0x12; 32]); // right_anchor_hash
    tlv(&mut record, 6, 0x11, b"new"); // replacement_text
    tlv(&mut record, 9, 0x11, b"old"); // old_span_text
    record
}

#[test]
fn decode_valid_edit_text_is_apply_supported() {
    // A well-formed §9.3 EditText is reconciled, validated, and admitted by the
    // DC-12 apply-supported gate. Runtime replay may still fail closed on missing
    // node identity, stale anchors, or text preconditions.
    let bytes = patch_with_raw_edit_text(&valid_edit_text_record());
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    ensure_apply_supported(ops.first().expect("one operation")).expect("supported");
}

#[test]
fn decode_edit_text_via_object_encoder_is_supported() {
    // Encoded through the real object writer: proves the §9.3 encoder and the
    // decoder agree on the wire shape end to end.
    let bytes = patch_bytes(OperationKind::EditText(EditText {
        node_id: NodeId::from_bytes([0x22; 32]),
        span_id: [0x10; 32],
        old_span_hash: text_span_hash(b"old"),
        left_anchor_hash: [0x11; 32],
        right_anchor_hash: [0x12; 32],
        replacement_text: b"new".to_vec(),
        presentation_hint_line: None,
        presentation_hint_column: None,
        old_span_text: b"old".to_vec(),
        left_anchor_len: None,
        right_anchor_len: None,
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::EditText {
            node_id,
            span_id,
            old_span_hash,
            left_anchor_hash,
            right_anchor_hash,
            replacement_text,
            old_span_text,
            ..
        } => {
            assert_eq!(node_id.as_bytes(), &[0x22; 32]);
            assert_eq!(span_id, &[0x10; 32]);
            assert_eq!(old_span_hash, &text_span_hash(b"old"));
            assert_eq!(left_anchor_hash, &[0x11; 32]);
            assert_eq!(right_anchor_hash, &[0x12; 32]);
            assert_eq!(replacement_text.as_slice(), &b"new"[..]);
            assert_eq!(old_span_text.as_slice(), &b"old"[..]);
        }
        other => panic!("expected EditText, got {other:?}"),
    }
    ensure_apply_supported(ops.first().expect("one operation")).expect("supported");
}

#[test]
fn decode_rejects_edit_text_hash_binding_violation() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x11, &[0x10; 32]);
    tlv(&mut record, 3, 0x11, &[0x00; 32]); // wrong: != SHA-256(old_span_text)
    tlv(&mut record, 4, 0x11, &[0x11; 32]);
    tlv(&mut record, 5, 0x11, &[0x12; 32]);
    tlv(&mut record, 6, 0x11, b"new");
    tlv(&mut record, 9, 0x11, b"old");
    let bytes = patch_with_raw_edit_text(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_edit_text_non_utf8_old_span_text() {
    let bad: &[u8] = &[0xff, 0xfe];
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x11, &[0x10; 32]);
    tlv(&mut record, 3, 0x11, &text_span_hash(bad)); // hash binds, but not UTF-8
    tlv(&mut record, 4, 0x11, &[0x11; 32]);
    tlv(&mut record, 5, 0x11, &[0x12; 32]);
    tlv(&mut record, 6, 0x11, b"new");
    tlv(&mut record, 9, 0x11, bad);
    let bytes = patch_with_raw_edit_text(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_edit_text_non_utf8_replacement_text() {
    // FDD-03 §9.3 names both span fields; the read path must reject non-UTF-8
    // replacement_text too (P2-2).
    let bad: &[u8] = &[0xff, 0xfe];
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x11, &[0x10; 32]);
    tlv(&mut record, 3, 0x11, &text_span_hash(b"old"));
    tlv(&mut record, 4, 0x11, &[0x11; 32]);
    tlv(&mut record, 5, 0x11, &[0x12; 32]);
    tlv(&mut record, 6, 0x11, bad); // replacement_text not UTF-8
    tlv(&mut record, 9, 0x11, b"old");
    let bytes = patch_with_raw_edit_text(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_edit_text_all_zero_node_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x00; 32]); // node_id all-zero (illegal)
    tlv(&mut record, 2, 0x11, &[0x10; 32]);
    tlv(&mut record, 3, 0x11, &text_span_hash(b"old"));
    tlv(&mut record, 4, 0x11, &[0x11; 32]);
    tlv(&mut record, 5, 0x11, &[0x12; 32]);
    tlv(&mut record, 6, 0x11, b"new");
    tlv(&mut record, 9, 0x11, b"old");
    let bytes = patch_with_raw_edit_text(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

/// Compose patch bytes carrying one operation with two operation-kind records, to
/// exercise the FDD-03 §9.2 oneof read-side rejection. The bodies are empty: the
/// duplicate-kind check fires before any kind body is decoded.
fn patch_with_two_kind_records(tag_a: u16, tag_b: u16) -> Vec<u8> {
    let mut operation = Vec::new();
    tlv(&mut operation, 1, 0x03, &1_u32.to_be_bytes()); // op_seq u32
    tlv(&mut operation, tag_a, 0x20, b""); // first kind record
    tlv(&mut operation, tag_b, 0x20, b""); // second kind record (oneof violation)
    let mut patch = Vec::new();
    tlv(&mut patch, 1, 0x21, &operation);
    patch
}

#[test]
fn decode_rejects_operation_with_create_and_delete_kinds() {
    let bytes = patch_with_two_kind_records(10, 11);
    let err = decode_patch_operations(&bytes, 1).expect_err("oneof");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_operation_with_create_and_replace_kinds() {
    let bytes = patch_with_two_kind_records(10, 16);
    let err = decode_patch_operations(&bytes, 1).expect_err("oneof");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_operation_with_create_and_edit_text_kinds() {
    // Must be rejected as a malformed oneof, NOT merely as an unsupported EditText.
    let bytes = patch_with_two_kind_records(10, 12);
    let err = decode_patch_operations(&bytes, 1).expect_err("oneof");
    assert!(
        matches!(err, PrikkError::MalformedData(_)),
        "create+edittext must be a oneof violation, got {err:?}"
    );
}

fn patch_with_raw_replace_binary(replace_binary_record: &[u8]) -> Vec<u8> {
    let mut operation = Vec::new();
    tlv(&mut operation, 1, 0x03, &1_u32.to_be_bytes()); // op_seq u32
    tlv(&mut operation, 16, 0x20, replace_binary_record); // replace_binary record
    let mut patch = Vec::new();
    tlv(&mut patch, 1, 0x21, &operation); // operations record_list_item
    patch
}

fn valid_replace_binary_record() -> Vec<u8> {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]); // node_id bytes (nonzero)
    tlv(&mut record, 2, 0x12, &[0x11; 32]); // old_blob_id object_id
    tlv(&mut record, 3, 0x12, &[0x33; 32]); // new_blob_id object_id
    record
}

#[test]
fn decode_valid_replace_binary_is_apply_supported() {
    // §9.3 ReplaceBinary is node-addressed; DC-73 wired its apply, so a valid record is both
    // decoded and admitted by `ensure_apply_supported`, not deferred.
    let bytes = patch_with_raw_replace_binary(&valid_replace_binary_record());
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    ensure_apply_supported(ops.first().expect("one operation")).expect("DC-73 apply-supported");
}

#[test]
fn decode_replace_binary_via_object_encoder_is_apply_supported() {
    // End-to-end through the real §9.3 object writer (encoder/decoder agree on wire).
    let bytes = patch_bytes(OperationKind::ReplaceBinary(ReplaceBinary {
        node_id: NodeId::from_bytes([0x22; 32]),
        old_blob_id: ObjectId::from_bytes([0x11; 32]),
        new_blob_id: ObjectId::from_bytes([0x33; 32]),
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::ReplaceBinary {
            node_id,
            old_blob_id,
            new_blob_id,
        } => {
            assert_eq!(node_id.as_bytes(), &[0x22; 32]);
            assert_eq!(old_blob_id, &ObjectId::from_bytes([0x11; 32]));
            assert_eq!(new_blob_id, &ObjectId::from_bytes([0x33; 32]));
        }
        other => panic!("expected ReplaceBinary, got {other:?}"),
    }
    ensure_apply_supported(ops.first().expect("one operation")).expect("DC-73 apply-supported");
}

#[test]
fn decode_rejects_replace_binary_all_zero_node_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x00; 32]); // node_id all-zero (illegal)
    tlv(&mut record, 2, 0x12, &[0x11; 32]);
    tlv(&mut record, 3, 0x12, &[0x33; 32]);
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_replace_binary_blob_id_wrong_wire() {
    // §9.3 requires old/new blob ids be object_id (0x12); bytes (0x11) must reject.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x11, &[0x11; 32]); // old_blob_id as bytes, not object_id
    tlv(&mut record, 3, 0x12, &[0x33; 32]);
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

/// Wrap an arbitrary operation-kind record (by its oneof tag) into patch bytes.
fn patch_with_raw_op(op_tag: u16, record: &[u8]) -> Vec<u8> {
    let mut operation = Vec::new();
    tlv(&mut operation, 1, 0x03, &1_u32.to_be_bytes()); // op_seq u32
    tlv(&mut operation, op_tag, 0x20, record);
    let mut patch = Vec::new();
    tlv(&mut patch, 1, 0x21, &operation);
    patch
}

// ---- RenamePath (tag 13) §9.3 read-side ----

#[test]
fn decode_valid_rename_path_is_validated_then_apply_supported() {
    // RFC 144 increment 1: a well-formed §9.3 RenamePath is reconciled, validated, and now
    // admitted by the apply-supported gate -- routing to `apply::apply_rename_batch` is the
    // caller's job (`patch_replay.rs`'s own replay loop), not this gate's.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]); // node_id
    tlv(&mut record, 2, 0x13, b"a.txt"); // old_path repo_path
    tlv(&mut record, 3, 0x13, b"b.txt"); // new_path repo_path
    let bytes = patch_with_raw_op(13, &record);
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    ensure_apply_supported(ops.first().expect("one operation")).expect("supported");
}

#[test]
fn decode_rejects_rename_path_all_zero_node_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x00; 32]);
    tlv(&mut record, 2, 0x13, b"a.txt");
    tlv(&mut record, 3, 0x13, b"b.txt");
    let bytes = patch_with_raw_op(13, &record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rename_path_via_object_encoder_is_apply_supported() {
    // End-to-end through the real §9.3 object writer (encoder/decoder agree on wire). RFC 144
    // increment 1: RenamePath is now admitted by the apply-supported gate.
    let bytes = patch_bytes(OperationKind::RenamePath(RenamePath {
        node_id: NodeId::from_bytes([0x22; 32]),
        old_path: "a.txt".to_string(),
        new_path: "b.txt".to_string(),
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::RenamePath {
            node_id,
            old_path,
            new_path,
        } => {
            assert_eq!(node_id.as_bytes(), &[0x22; 32]);
            assert_eq!(old_path, "a.txt");
            assert_eq!(new_path, "b.txt");
        }
        other => panic!("expected RenamePath, got {other:?}"),
    }
    ensure_apply_supported(ops.first().expect("one operation")).expect("supported");
}

// ---- ChangePerm (tag 14) §9.3 read-side ----

#[test]
fn decode_valid_change_perm_is_apply_supported() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x03, &0o100_644_u32.to_be_bytes());
    tlv(&mut record, 3, 0x03, &0o100_755_u32.to_be_bytes());
    let bytes = patch_with_raw_op(14, &record);
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    ensure_apply_supported(ops.first().expect("one operation")).expect("DC-73 apply-supported");
}

#[test]
fn decode_rejects_change_perm_all_zero_node_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x00; 32]);
    tlv(&mut record, 2, 0x03, &0o100_644_u32.to_be_bytes());
    tlv(&mut record, 3, 0x03, &0o100_755_u32.to_be_bytes());
    let bytes = patch_with_raw_op(14, &record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_change_perm_via_object_encoder_is_apply_supported() {
    let bytes = patch_bytes(OperationKind::ChangePerm(ChangePerm {
        node_id: NodeId::from_bytes([0x22; 32]),
        old_mode: 0o100_644,
        new_mode: 0o100_755,
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::ChangePerm {
            node_id,
            old_mode,
            new_mode,
        } => {
            assert_eq!(node_id.as_bytes(), &[0x22; 32]);
            assert_eq!(*old_mode, 0o100_644);
            assert_eq!(*new_mode, 0o100_755);
        }
        other => panic!("expected ChangePerm, got {other:?}"),
    }
    ensure_apply_supported(ops.first().expect("one operation")).expect("DC-73 apply-supported");
}

// ---- CreateSymlink (tag 15) §9.3 read-side ----

#[test]
fn decode_valid_create_symlink_is_validated_then_unsupported() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"link"); // path repo_path
    tlv(&mut record, 2, 0x11, &[0x22; 32]); // node_id
    tlv(&mut record, 3, 0x10, b"target.txt"); // target utf8_string
    let bytes = patch_with_raw_op(15, &record);
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    let err = ensure_apply_supported(ops.first().expect("one operation")).expect_err("deferred");
    assert!(
        matches!(err, PrikkError::UnsupportedObjectType(_)),
        "{err:?}"
    );
}

#[test]
fn decode_rejects_create_symlink_all_zero_node_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"link");
    tlv(&mut record, 2, 0x11, &[0x00; 32]);
    tlv(&mut record, 3, 0x10, b"target.txt");
    let bytes = patch_with_raw_op(15, &record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_create_symlink_target_wrong_wire() {
    // target is utf8_string (0x10); bytes (0x11) must reject.
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x13, b"link");
    tlv(&mut record, 2, 0x11, &[0x22; 32]);
    tlv(&mut record, 3, 0x11, b"target.txt"); // wrong wire
    let bytes = patch_with_raw_op(15, &record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_create_symlink_via_object_encoder_is_unsupported() {
    let bytes = patch_bytes(OperationKind::CreateSymlink(CreateSymlink {
        path: "link".to_string(),
        node_id: NodeId::from_bytes([0x22; 32]),
        target: "target.txt".to_string(),
    }));
    let ops = decode_patch_operations(&bytes, 1).expect("decodes");
    match &ops.first().expect("one operation").kind {
        DecodedOperationKind::CreateSymlink {
            path,
            node_id,
            target,
        } => {
            assert_eq!(path, "link");
            assert_eq!(node_id.as_bytes(), &[0x22; 32]);
            assert_eq!(target, "target.txt");
        }
        other => panic!("expected CreateSymlink, got {other:?}"),
    }
    let err = ensure_apply_supported(ops.first().expect("one operation")).expect_err("deferred");
    assert!(
        matches!(err, PrikkError::UnsupportedObjectType(_)),
        "{err:?}"
    );
}

// ---- ReplaceBinary decoder errata (P2): missing-field + wrong-wire negatives ----

#[test]
fn decode_rejects_replace_binary_missing_node_id() {
    let mut record = Vec::new();
    tlv(&mut record, 2, 0x12, &[0x11; 32]);
    tlv(&mut record, 3, 0x12, &[0x33; 32]);
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_replace_binary_missing_old_blob_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 3, 0x12, &[0x33; 32]);
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_replace_binary_missing_new_blob_id() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x12, &[0x11; 32]);
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_replace_binary_new_blob_id_wrong_wire() {
    let mut record = Vec::new();
    tlv(&mut record, 1, 0x11, &[0x22; 32]);
    tlv(&mut record, 2, 0x12, &[0x11; 32]);
    tlv(&mut record, 3, 0x11, &[0x33; 32]); // new_blob_id as bytes, not object_id
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_replace_binary_unknown_tag() {
    let mut record = valid_replace_binary_record();
    tlv(&mut record, 7, 0x03, &1_u32.to_be_bytes()); // unknown tag in ReplaceBinary
    let bytes = patch_with_raw_replace_binary(&record);
    let err = decode_patch_operations(&bytes, 1).expect_err("malformed");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_operation_with_create_and_rename_kinds() {
    // Oneof must still be enforced now that tag 13 is body-parsed (claim happens
    // before dispatch, so the second kind is a oneof violation, not a RenamePath
    // unsupported result).
    let bytes = patch_with_two_kind_records(10, 13);
    let err = decode_patch_operations(&bytes, 1).expect_err("oneof");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

// ---- op_seq canonical invariants (FDD-03 §9.2.1) ----

/// A valid CreateFile record body keyed by a distinct nonzero node byte.
fn create_file_record_for_seq(node: u8) -> Vec<u8> {
    let mut r = Vec::new();
    tlv(&mut r, 1, 0x13, format!("f{node}.txt").as_bytes()); // path repo_path
    tlv(&mut r, 2, 0x11, &[node; 32]); // node_id (nonzero)
    tlv(&mut r, 3, 0x12, &[0x11; 32]); // blob_id object_id
    tlv(&mut r, 4, 0x03, &0o100_644_u32.to_be_bytes()); // mode u32
    r
}

/// Build patch bytes whose operations carry the given (op_seq, node) pairs in the
/// given physical order, each a CreateFile.
fn patch_with_operation_seqs(seqs: &[(u32, u8)]) -> Vec<u8> {
    let mut patch = Vec::new();
    for (op_seq, node) in seqs {
        let mut op = Vec::new();
        tlv(&mut op, 1, 0x03, &op_seq.to_be_bytes()); // op_seq u32
        tlv(&mut op, 10, 0x20, &create_file_record_for_seq(*node)); // CreateFile
        tlv(&mut patch, 1, 0x21, &op);
    }
    patch
}

#[test]
fn decode_accepts_contiguous_one_based_op_seq() {
    // Physical order 1,2,3 with op_seq 1,2,3 is the single valid encoding.
    let bytes = patch_with_operation_seqs(&[(1, 1), (2, 2), (3, 3)]);
    let ops = decode_patch_operations(&bytes, 1).expect("valid op_seq accepts");
    assert_eq!(ops.len(), 3);
}

#[test]
fn decode_rejects_op_seq_not_starting_at_one() {
    let bytes = patch_with_operation_seqs(&[(2, 1), (3, 2)]);
    let err = decode_patch_operations(&bytes, 1).expect_err("first op_seq != 1");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_op_seq_gap() {
    let bytes = patch_with_operation_seqs(&[(1, 1), (3, 2)]);
    let err = decode_patch_operations(&bytes, 1).expect_err("op_seq gap");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_op_seq_duplicate() {
    let bytes = patch_with_operation_seqs(&[(1, 1), (1, 2)]);
    let err = decode_patch_operations(&bytes, 1).expect_err("duplicate op_seq");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}

#[test]
fn decode_rejects_op_seq_physical_order_mismatch() {
    // Physical order carries op_seq 2 then 1; ascending order would be 1 then 2.
    let bytes = patch_with_operation_seqs(&[(2, 1), (1, 2)]);
    let err = decode_patch_operations(&bytes, 1).expect_err("order mismatch");
    assert!(matches!(err, PrikkError::MalformedData(_)), "{err:?}");
}
