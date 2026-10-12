//! Verify tests (RFC 155 implementation Part V).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::type_complexity
)]

use std::io::Cursor;

use prikk_object::CanonicalEncode;

use crate::archive::export_archive;
use crate::archive::tests::{
    decode_manifest_for_test, insert_tail_bytes_into_section_for_test, seal_one_block,
};
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::{signed_text_blob_envelope, unique_temp_dir};
use crate::{FileObjectStore, ObjectWriter, RepositoryLayout};

use super::verify_archive;

/// Every regular file under `root`, by its path relative to `root` and its own byte length --
/// enough to prove "nothing written, nothing removed, nothing resized" without pulling in a hash
/// dependency this test-only helper does not need.
fn directory_fingerprint(root: &std::path::Path) -> Vec<(std::path::PathBuf, u64)> {
    fn walk(
        dir: &std::path::Path,
        root: &std::path::Path,
        out: &mut Vec<(std::path::PathBuf, u64)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(metadata) = entry.metadata() {
                let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
                out.push((relative, metadata.len()));
            }
        }
    }
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn build_one_block_archive() -> Vec<u8> {
    let dir = unique_temp_dir("archive-verify-fixture");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");
    let mut buffer = Vec::new();
    export_archive(&layout, &mut buffer).expect("export");
    buffer
}

/// The manifest's own `(offset, length)`, read from the end trailer the same way
/// `decode_manifest_for_test` does, duplicated here only for the raw byte offsets this module's own
/// corruption tests need (that function returns *decoded* content, not where it lives).
fn manifest_region(bytes: &[u8]) -> (usize, usize) {
    let end_trailer_start = bytes.len() - 56;
    let manifest_offset = u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    let manifest_length = u64::from_be_bytes(
        bytes[end_trailer_start + 16..end_trailer_start + 24]
            .try_into()
            .unwrap(),
    ) as usize;
    (manifest_offset, manifest_length)
}

/// The absolute byte offset of entry `index`'s own 50-byte record
/// (`kind(2) | offset(8) | length(8) | checksum(32)`) inside the manifest.
fn entry_record_offset(bytes: &[u8], index: usize) -> usize {
    let (manifest_offset, _) = manifest_region(bytes);
    let tool_version_len = u16::from_be_bytes(
        bytes[manifest_offset + 12..manifest_offset + 14]
            .try_into()
            .unwrap(),
    ) as usize;
    let first_entry = manifest_offset + 8 + 4 + 2 + tool_version_len + 8;
    first_entry + index * 50
}

/// This fixture's own `maintainer_signature()`/AUTHOR signature are both placeholders -- "a value,
/// not a signer" (`test_support`'s own doc comment): fixed bytes under a fixed `key_id`, never a
/// real Ed25519 signature over anything. No maintainer key is ever adopted in this fixture and no
/// author key material is ever recorded, so neither signature is by a key this archive carries --
/// §9.4's own N1 fix (amended 2026-10-12, review `rfc155-impl-IC-review-v1`): named *"not
/// checked,"* never refused on, since trust is the receiver's own (R2), not this archive's carried
/// record. Structure and closure, which do not depend on any of that, are both sound, and -- since
/// neither signature is damage -- so is the whole archive.
#[test]
fn verify_names_every_not_carried_signature_without_failing_structure_or_closure() {
    let bytes = build_one_block_archive();
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(report.structurally_sound);
    assert!(report.reference_closure_sound);
    assert!(report.damaged.is_empty());
    assert!(report.unresolvable_signatures.is_empty());
    assert!(
        report.is_internally_consistent(),
        "a signature by a key this archive does not carry must never refuse the whole archive \
         (N1): {report:?}"
    );
    assert!(
        report
            .not_checked_signatures
            .iter()
            .any(|line| line.contains("AUTHOR") && line.contains("not carried")),
        "not_checked_signatures: {:?}",
        report.not_checked_signatures
    );
    assert!(
        report
            .not_checked_signatures
            .iter()
            .any(|line| line.contains("not checked: signed by a key this archive does not carry")),
        "the MAINTAINER signature must be named not-checked too: {:?}",
        report.not_checked_signatures
    );
    assert!(
        report.note.is_some(),
        "N1: a signature by a key this archive does not carry must not stop the 'internally \
         consistent' note from printing: {:?}",
        report.note
    );
}

/// The genuinely positive case: a real `Ed25519MaintainerSigner`, adopted before export, signing a
/// real block for real -- confirming signature resolution actually succeeds when the material
/// genuinely matches, not only that it is named when it does not.
#[test]
fn verify_resolves_a_genuinely_signed_maintainer_signature() {
    let dir = unique_temp_dir("archive-verify-real-signature");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let signer = crate::Ed25519MaintainerSigner::from_seed("archive-verify-real", &[0x42; 32])
        .expect("signer");
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex).expect("adopt");

    let mut store = crate::FileObjectStore::new(layout.clone());
    let payload = prikk_object::BlockPayload {
        parent_block_ids: Vec::new(),
        kind: prikk_object::BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_object::MerkleRoot([0u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes().expect("encode block payload");
    let mut envelope =
        prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Block, 2, payload_bytes);
    let block_id = envelope.object_id();
    envelope
        .add_signature(
            crate::maintainer_signature(&signer, prikk_object::ObjectType::Block, block_id)
                .expect("sign for real"),
        )
        .expect("attach signature");
    store.write_object(&envelope).expect("write block");

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(report.structurally_sound);
    assert!(report.reference_closure_sound);
    assert!(
        !report
            .unresolvable_signatures
            .iter()
            .any(|line| line.contains(&block_id.to_string())),
        "the genuinely signed, genuinely adopted block must not be named as unresolvable: {:?}",
        report.unresolvable_signatures
    );
    assert!(
        report.signatures_checked >= 1,
        "the real maintainer signature must count toward signatures_checked"
    );
    assert!(
        report.is_internally_consistent(),
        "this fixture has nothing else wrong with it"
    );
    let note = report
        .note
        .expect("V3: a genuinely internally-consistent archive must carry the note");
    assert!(note.contains("internally consistent"), "note: {note}");
    assert!(
        note.contains("not trusted"),
        "the note must say it is not a trust claim, not merely avoid one word: {note}"
    );
    assert!(
        !note.contains("is trusted") && !note.starts_with("trusted"),
        "the note must never make an affirmative trust claim: {note}"
    );
}

/// N1's own other half (review `rfc155-impl-IC-review-v1`): a signature by a key this archive
/// **does** carry (adopted here), that nonetheless fails to verify, is damage -- never folded into
/// the "not checked" bucket the previous test exercises. Built by attaching a genuinely-computed
/// Ed25519 signature from a *different* signer, then relabeling its own `key_id` to the adopted
/// one -- a real signature, cryptographically sound under its own real key, that simply does not
/// verify under the key this archive carries for that id. The container's own checksum is computed
/// over exactly these (relabeled) bytes, so it matches; only the signature itself is wrong.
#[test]
fn verify_names_a_carried_key_signature_that_does_not_verify_as_damage() {
    let dir = unique_temp_dir("archive-verify-carried-invalid-signature");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let adopted_signer =
        crate::Ed25519MaintainerSigner::from_seed("archive-verify-carried", &[0x43; 32])
            .expect("signer");
    let public_key_hex: String = adopted_signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    crate::add_trusted_maintainer(&layout, adopted_signer.key_id(), &public_key_hex)
        .expect("adopt");
    let other_signer =
        crate::Ed25519MaintainerSigner::from_seed("archive-verify-wrong-signer", &[0x44; 32])
            .expect("a different signer");

    let mut store = crate::FileObjectStore::new(layout.clone());
    let payload = prikk_object::BlockPayload {
        parent_block_ids: Vec::new(),
        kind: prikk_object::BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: prikk_object::MerkleRoot([0u8; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let payload_bytes = payload.to_canonical_bytes().expect("encode block payload");
    let mut envelope =
        prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Block, 2, payload_bytes);
    let block_id = envelope.object_id();
    let mut wrong_signature =
        crate::maintainer_signature(&other_signer, prikk_object::ObjectType::Block, block_id)
            .expect("sign for real, with the wrong key");
    assert_ne!(
        wrong_signature.key_id,
        adopted_signer.key_id(),
        "the two signers must have distinct key ids, or this test proves nothing"
    );
    wrong_signature.key_id = adopted_signer.key_id().to_string();
    envelope
        .add_signature(wrong_signature)
        .expect("attach the relabeled signature");
    store.write_object(&envelope).expect("write block");

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(report.structurally_sound);
    assert!(report.reference_closure_sound);
    assert!(
        report.not_checked_signatures.is_empty(),
        "a carried key's own invalid signature must never be mistaken for a not-carried one: \
         {:?}",
        report.not_checked_signatures
    );
    assert!(
        report
            .unresolvable_signatures
            .iter()
            .any(|line| line.contains(&block_id.to_string())),
        "the carried-key signature that does not verify must be named as damage: {:?}",
        report.unresolvable_signatures
    );
    assert!(
        !report.is_internally_consistent(),
        "N1: a carried key's own invalid signature must still refuse the whole archive"
    );
}

/// §8's control, also a required test: "a byte flipped in any envelope makes `verify` fail and
/// names the object." Flips one byte inside the Block section's own record body (never its header
/// or trailer, which would instead look like a torn/foreign record) -- located precisely via the
/// manifest, not guessed from a fixed offset.
#[test]
fn verify_names_a_flipped_byte_in_an_envelope() {
    let mut bytes = build_one_block_archive();
    let (_, _, entries) = decode_manifest_for_test(&bytes);
    let block_entry = entries
        .iter()
        .find(|(kind, ..)| {
            *kind
                == crate::archive::ArchiveSectionKind::ObjectContainer(
                    prikk_object::ObjectType::Block,
                )
                .wire_code()
        })
        .expect("block section present");
    let (_, offset, length, _) = *block_entry;
    assert!(
        length > 60,
        "the sealed block's own record has real content to flip"
    );
    let body_start = offset as usize + 18;
    let flip_at = body_start + (length as usize / 2);
    bytes[flip_at] ^= 0xFF;

    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("a damaged archive still produces a report");
    assert!(!report.is_internally_consistent());
    assert!(!report.structurally_sound);
    assert!(
        report
            .damaged
            .iter()
            .any(|line| line.contains("block container")),
        "damaged: {:?}",
        report.damaged
    );
}

#[test]
fn verify_refuses_a_truncated_archive() {
    let bytes = build_one_block_archive();
    let truncated = bytes[..bytes.len() / 2].to_vec();
    let mut cursor = Cursor::new(truncated);
    let result = verify_archive(&mut cursor);
    assert!(
        result.is_err(),
        "a truncated archive has no trailer to find"
    );
}

/// V1a (review `rfc155-impl-V-review-v1`, finding V1): a bit flipped inside the manifest body,
/// isolated from V1b's own known-format check -- this flips an entry's own checksum byte (offset
/// and length are cross-checked by `validate_manifest_shape` already; the checksum field inside
/// each entry record was not, before this fix), never `repository_format` itself, so this test
/// proves the manifest checksum catches damage on its own, not merely as a side effect of the
/// format check also tripping on the same byte.
#[test]
fn verify_refuses_a_flipped_manifest_byte() {
    let mut bytes = build_one_block_archive();
    let first_entry_checksum_byte = entry_record_offset(&bytes, 0) + 18;
    bytes[first_entry_checksum_byte] ^= 0xFF;
    let mut cursor = Cursor::new(bytes);
    let err = verify_archive(&mut cursor).expect_err("a flipped manifest byte must refuse");
    assert!(
        err.to_string().contains("checksum"),
        "must name the manifest checksum, not merely fail some other way: {err}"
    );
}

/// V1b: a *legitimately re-encoded* manifest (correct checksum) declaring a `repository_format`
/// this build does not know -- proves the format check is independent of the checksum check, not a
/// side effect of it (a future build's own correctly-written archive must still be refused by name,
/// not silently misread).
#[test]
fn verify_refuses_an_unknown_repository_format() {
    let mut bytes = build_one_block_archive();
    crate::archive::tests::rewrite_manifest_repository_format_for_test(&mut bytes, 999);
    let mut cursor = Cursor::new(bytes);
    let err = verify_archive(&mut cursor).expect_err("an unknown repository format must refuse");
    let message = err.to_string();
    assert!(
        message.contains("999"),
        "must name the value it saw: {message}"
    );
    assert!(
        message.contains("repository format"),
        "must say what kind of value it is: {message}"
    );
}

#[test]
fn verify_refuses_an_unknown_section_kind() {
    let mut bytes = build_one_block_archive();
    let record_offset = entry_record_offset(&bytes, 0);
    bytes[record_offset] = 0xFF;
    bytes[record_offset + 1] = 0xFF; // kind 0xFFFF, outside the fixed allow-list
    // V1a's own checksum check runs before the section-kind check this test means to exercise --
    // recompute it so this test reaches that check, not the (also-now-true) checksum mismatch.
    crate::archive::tests::recompute_manifest_checksum_for_test(&mut bytes);
    let mut cursor = Cursor::new(bytes);
    let err = verify_archive(&mut cursor).expect_err("an unknown section kind must refuse");
    assert!(err.to_string().contains("unknown section kind"), "{err}");
}

#[test]
fn verify_refuses_a_duplicated_section_kind() {
    let mut bytes = build_one_block_archive();
    let first = entry_record_offset(&bytes, 0);
    let second = entry_record_offset(&bytes, 1);
    // Overwrite the second entry's own kind field with the first entry's kind code.
    bytes[second] = bytes[first];
    bytes[second + 1] = bytes[first + 1];
    crate::archive::tests::recompute_manifest_checksum_for_test(&mut bytes);
    let mut cursor = Cursor::new(bytes);
    let err = verify_archive(&mut cursor).expect_err("a duplicated section kind must refuse");
    assert!(err.to_string().contains("twice"), "{err}");
}

/// A section's kind is a fixed numeric code; this format carries no section *name* string for a
/// kind at all, so there is no wire-level string to misuse as a path. The one carried string this
/// module decodes (the manifest's own `tool_version`) is proven to round-trip as opaque data, never
/// consulted as a filesystem path -- a value shaped like a traversal attempt changes nothing about
/// how verify behaves.
#[test]
fn verify_never_treats_a_carried_string_as_a_path() {
    let mut bytes = build_one_block_archive();
    let (manifest_offset, _) = manifest_region(&bytes);
    let tool_version_len_offset = manifest_offset + 12;
    let old_len = u16::from_be_bytes(
        bytes[tool_version_len_offset..tool_version_len_offset + 2]
            .try_into()
            .unwrap(),
    ) as usize;
    // A short traversal fragment, built to fit within whatever this build's own
    // `CARGO_PKG_VERSION` string happens to be (never assumed longer than it really is).
    let traversal: Vec<u8> = b"../".iter().copied().cycle().take(old_len).collect();
    let value_start = tool_version_len_offset + 2;
    bytes[value_start..value_start + old_len].copy_from_slice(&traversal);
    crate::archive::tests::recompute_manifest_checksum_for_test(&mut bytes);

    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        report.tool_version.starts_with("../"),
        "the traversal-shaped string round-trips as plain data: {}",
        report.tool_version
    );
    // Nothing about verify's own outcome depends on this string's content -- the fixture is
    // otherwise undamaged, so the rest of the report is unaffected.
    assert!(report.structurally_sound);
}

#[test]
fn verify_lists_carried_maintainer_keys_as_carried_not_adopted() {
    let dir = unique_temp_dir("archive-verify-maintainer-keys");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");
    let (_, added) =
        crate::trust::add_trusted_maintainer(&layout, "verify-test-key", &"ab".repeat(32))
            .expect("adopt a maintainer key");
    assert!(added);

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert_eq!(report.maintainer_keys.len(), 1);
    assert_eq!(report.maintainer_keys[0].key_id, "verify-test-key");
    assert_eq!(
        report.maintainer_keys[0].public_key_hex,
        "ab".repeat(32),
        "the carried key's own public key round-trips"
    );
}

#[test]
fn verify_writes_nothing_on_any_exit_path() {
    let dir = unique_temp_dir("archive-verify-writes-nothing");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");
    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");

    let before = directory_fingerprint(&dir);

    let mut cursor = Cursor::new(bytes.clone());
    let _ = verify_archive(&mut cursor);

    // Also run it against a damaged copy, which exercises every refusal/damage path, not only the
    // clean one -- a write on the *failure* path is exactly the kind of bug "writes nothing, on any
    // exit path" exists to rule out.
    let mut damaged = bytes;
    let flip_at = damaged.len() / 3;
    damaged[flip_at] ^= 0xFF;
    let mut damaged_cursor = Cursor::new(damaged);
    let _ = verify_archive(&mut damaged_cursor);

    let after = directory_fingerprint(&dir);
    assert_eq!(
        before, after,
        "verify must not create, remove, or modify anything on disk"
    );
}

/// Required test, and the subject of the closure-pass control (see the report): a `Block` whose
/// own `patch_ids` names a patch id that was never written anywhere -- a genuine dangling
/// reference, built with real library calls, never hand-crafted bytes.
#[test]
fn verify_detects_a_genuinely_dangling_reference() {
    let dir = unique_temp_dir("archive-verify-dangling");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = crate::FileObjectStore::new(layout.clone());
    let missing_patch_id = prikk_object::ObjectId::from_bytes([0xAB; 32]);
    let root_block = crate::test_gates::test_support::signed_block(
        prikk_object::BlockKind::Root,
        Vec::new(),
        vec![missing_patch_id],
        None,
    );
    let root_block_id = object_store.write_object(&root_block).expect("write block");

    let ref_store = crate::RefStore::new(layout.clone());
    let ref_state = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        None,
        root_block_id,
        1,
    );
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        None,
        ref_state_id,
        root_block_id,
        1,
    );
    ref_store
        .publish(&crate::RefPublication {
            ref_name: "heads/main".to_string(),
            expected_previous_ref_state_id: None,
            ref_state,
            ref_update,
        })
        .expect("publish");

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        !report.reference_closure_sound,
        "a block naming a patch id nobody ever wrote must fail closure"
    );
    assert!(
        report
            .damaged
            .iter()
            .any(|line| line.contains(&missing_patch_id.to_string())),
        "damaged: {:?}",
        report.damaged
    );
}

/// Two real, unsigned-content `Blob` records in one container -- the minimum needed to test a
/// *middle* record's own damage with a sound record after it, never hand-assembled from scratch.
/// Both carry the same placeholder MAINTAINER signature `signed_text_blob_envelope` always does
/// (N1's own "not checked" bucket, never damage on its own), so the only signal in play for these
/// tests is each record's own structural soundness.
fn two_blob_archive() -> Vec<u8> {
    let dir = unique_temp_dir("archive-verify-two-blobs");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut store = FileObjectStore::new(layout.clone());
    store
        .write_object(&signed_text_blob_envelope(
            b"first blob, long enough to flip a body byte \
            without touching its own header or checksum fields",
        ))
        .expect("write first blob");
    store
        .write_object(&signed_text_blob_envelope(
            b"second blob, also long enough to survive a \
            flipped body byte and still be read back as one complete frame",
        ))
        .expect("write second blob");
    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    bytes
}

/// Flip one byte inside the `index`th `Blob` record's own body (never its header), then recompute
/// the section's own outer trailing checksum so it still matches what is now on disk -- isolating
/// the *per-record* damage this part's own fix names, from the section's outer checksum, which
/// would otherwise also (correctly, but less precisely) report the whole section as damaged.
fn flip_one_blob_record_body(bytes: &mut [u8], index: usize) {
    let (_, _, entries) = decode_manifest_for_test(bytes);
    let (_, offset, length, _) = *entries
        .iter()
        .find(|(kind, ..)| {
            *kind
                == crate::archive::ArchiveSectionKind::ObjectContainer(
                    prikk_object::ObjectType::Blob,
                )
                .wire_code()
        })
        .expect("blob section present");
    const SECTION_HEADER_LEN: u64 = 8 + 2 + 8;
    const CONTAINER_HEADER_LEN: u64 = 8 + 2 + 8 + 32;
    let mut cursor = offset + SECTION_HEADER_LEN;
    for seen in 0..=index {
        let header_at = cursor as usize;
        let body_len =
            u64::from_be_bytes(bytes[header_at + 10..header_at + 18].try_into().unwrap());
        if seen == index {
            let flip_at = header_at + CONTAINER_HEADER_LEN as usize;
            bytes[flip_at] ^= 0xFF;
            break;
        }
        cursor += CONTAINER_HEADER_LEN + body_len;
    }
    let section_header_start = offset as usize;
    let body_start = section_header_start + SECTION_HEADER_LEN as usize;
    let body_end = body_start + length as usize;
    let checksum = prikk_hash::sha256_parts(&[
        &bytes[section_header_start..section_header_start + 8],
        &bytes[section_header_start + 8..section_header_start + 10],
        &bytes[section_header_start + 10..body_start],
        &bytes[body_start..body_end],
    ]);
    bytes[body_end..body_end + 32].copy_from_slice(&checksum);
    // The manifest's own entry for this section carries its own copy of the same checksum
    // (`archive-format.md`'s own cross-check, I-E's own item) -- recomputed here too, so this test
    // isolates the per-record damage this part adds, not a manifest/trailer mismatch I-E's own
    // item would otherwise also catch first.
    let (_, _, manifest_entries) = decode_manifest_for_test(bytes);
    let entry_index = manifest_entries
        .iter()
        .position(|(kind, ..)| {
            *kind
                == crate::archive::ArchiveSectionKind::ObjectContainer(
                    prikk_object::ObjectType::Blob,
                )
                .wire_code()
        })
        .expect("blob entry present");
    let (_, _, manifest_offset) = (0u8, 0u8, {
        let end_trailer_start = bytes.len() - 56;
        u64::from_be_bytes(
            bytes[end_trailer_start + 8..end_trailer_start + 16]
                .try_into()
                .unwrap(),
        )
    });
    let manifest_body = &bytes[manifest_offset as usize..];
    let tool_version_len = u16::from_be_bytes(manifest_body[12..14].try_into().unwrap()) as usize;
    let first_entry = manifest_offset as usize + 14 + tool_version_len + 8;
    let entry_at = first_entry + entry_index * 50;
    bytes[entry_at + 18..entry_at + 50].copy_from_slice(&checksum);
    crate::archive::tests::recompute_manifest_checksum_for_test(bytes);
}

/// §9.1, amended 2026-10-12 (review `rfc155-impl-IC-review-v1`): "the archive classifies a
/// container's frames exactly as the store does" -- the full required test matrix, each shape
/// built on a real export, never hand-assembled. For every shape, the assertion is the one the
/// amended spec states: an *incomplete* frame (a short section-ending tail, however it reads) is
/// named but leaves the archive consistent and importable; a *complete* frame that fails its own
/// checksum is damage.
mod section_frame_classification {
    use super::*;

    fn assert_incomplete_tail_named_not_damage(bytes: Vec<u8>, min_tail_len: u64) {
        let mut cursor = Cursor::new(bytes);
        let report = verify_archive(&mut cursor).expect("verify");
        assert!(
            report.damaged.is_empty(),
            "an incomplete frame must never be named as damage: {:?}",
            report.damaged
        );
        assert!(
            report
                .interrupted_appends
                .iter()
                .any(|line| line.contains("block container")),
            "the tail must be named as an interrupted append: {:?}",
            report.interrupted_appends
        );
        let named_len: u64 = report
            .interrupted_appends
            .iter()
            .filter_map(|line| {
                line.split_whitespace()
                    .find_map(|word| word.parse::<u64>().ok())
            })
            .sum();
        assert!(
            named_len >= min_tail_len,
            "the named tail byte count must be at least the bytes actually appended: {named_len} \
             < {min_tail_len}"
        );
        assert!(
            report.is_internally_consistent(),
            "an interrupted append alone must leave the archive internally consistent: {report:?}"
        );
    }

    fn block_section_wire_code() -> u16 {
        crate::archive::ArchiveSectionKind::ObjectContainer(prikk_object::ObjectType::Block)
            .wire_code()
    }

    /// Shape 1: 16 raw bytes, shorter than one container record header (50 bytes) -- the
    /// shortest possible "a crash cut this append short" shape.
    #[test]
    fn sixteen_bytes_is_an_interrupted_append_not_damage() {
        let mut bytes = build_one_block_archive();
        insert_tail_bytes_into_section_for_test(&mut bytes, block_section_wire_code(), &[0u8; 16]);
        assert_incomplete_tail_named_not_damage(bytes, 16);
    }

    /// Shape 2: 49 zero bytes -- still shorter than one header, the boundary I-C's own first fix
    /// already handled correctly.
    #[test]
    fn forty_nine_zero_bytes_is_an_interrupted_append_not_damage() {
        let mut bytes = build_one_block_archive();
        insert_tail_bytes_into_section_for_test(&mut bytes, block_section_wire_code(), &[0u8; 49]);
        assert_incomplete_tail_named_not_damage(bytes, 49);
    }

    /// Shape 3, external review 023's own exact shape: 100 zero bytes -- **longer** than one
    /// header (50 bytes), so a naive loop reads it as two bogus zero-length "records" with the
    /// wrong magic and silently absorbs them, naming nothing at all. This is the real finding:
    /// the fix must stop at the first header whose magic/version does not match, not keep
    /// consuming headers past it.
    #[test]
    fn one_hundred_zero_bytes_is_an_interrupted_append_not_damage() {
        let mut bytes = build_one_block_archive();
        insert_tail_bytes_into_section_for_test(&mut bytes, block_section_wire_code(), &[0u8; 100]);
        assert_incomplete_tail_named_not_damage(bytes, 100);
    }

    /// Shape 4: 30 bytes of plain text, not zeros -- the same class, a different byte pattern.
    #[test]
    fn thirty_text_bytes_is_an_interrupted_append_not_damage() {
        let mut bytes = build_one_block_archive();
        insert_tail_bytes_into_section_for_test(
            &mut bytes,
            block_section_wire_code(),
            b"this is not a container record",
        );
        assert_incomplete_tail_named_not_damage(bytes, 30);
    }

    /// Shape 7: a 16 MiB tail -- the same shape, at a size that would matter if classification
    /// buffered the whole section (it does not: memory stays bounded by one frame).
    #[test]
    fn a_sixteen_mebibyte_tail_is_an_interrupted_append_not_damage() {
        let mut bytes = build_one_block_archive();
        let tail = vec![0u8; 16 * 1024 * 1024];
        insert_tail_bytes_into_section_for_test(&mut bytes, block_section_wire_code(), &tail);
        assert_incomplete_tail_named_not_damage(bytes, tail.len() as u64);
    }

    /// Shape 5: a complete *last* frame, one body byte flipped -- damage, named by offset, and the
    /// archive refuses on it, even though the section's own outer checksum is recomputed to still
    /// match (isolating the per-record signal this part adds).
    #[test]
    fn a_flipped_last_record_is_damage() {
        let mut bytes = two_blob_archive();
        flip_one_blob_record_body(&mut bytes, 1);
        let mut cursor = Cursor::new(bytes);
        let report = verify_archive(&mut cursor).expect("verify");
        assert!(
            report.interrupted_appends.is_empty(),
            "a flipped complete record must never read as a mere interrupted append: {:?}",
            report.interrupted_appends
        );
        assert!(
            report
                .damaged
                .iter()
                .any(|line| line.contains("blob container")
                    && line.contains("failed its own checksum")),
            "the flipped record must be named as damage by its own section: {:?}",
            report.damaged
        );
        assert!(!report.is_internally_consistent());
    }

    /// Shape 6: a complete *middle* frame flipped, with a sound frame after it -- damage is named,
    /// and the scan still finds the later sound record (the twin of `foundation::container`'s own
    /// isolate-and-continue reading).
    #[test]
    fn a_flipped_middle_record_is_damage_and_the_sound_record_after_it_still_reads() {
        let mut bytes = two_blob_archive();
        flip_one_blob_record_body(&mut bytes, 0);
        let mut cursor = Cursor::new(bytes);
        let mut decoded_second = false;
        let report = {
            // Re-run the same decode `verify_archive` uses internally, directly, to confirm the
            // second (sound) record is still reached -- `verify_archive` itself does not expose
            // which records it decoded, only the aggregate report.
            let (_, _, entries) = decode_manifest_for_test(&cursor.clone().into_inner());
            let entry = entries
                .iter()
                .find(|(kind, ..)| *kind == block_section_wire_code_for_blob())
                .expect("blob entry");
            let manifest_entry = crate::archive::ManifestEntry {
                kind: crate::archive::ArchiveSectionKind::from_wire_code(entry.0)
                    .expect("known kind"),
                offset: entry.1,
                length: entry.2,
                checksum: entry.3,
            };
            super::super::for_each_object_record(
                &mut cursor,
                &manifest_entry,
                prikk_object::ObjectType::Blob,
                |_body| {
                    decoded_second = true;
                    Ok(())
                },
            )
            .expect("read section");
            cursor.set_position(0);
            verify_archive(&mut cursor).expect("verify")
        };
        assert!(
            decoded_second,
            "the sound record after the flipped one must still be decoded"
        );
        assert!(
            report
                .damaged
                .iter()
                .any(|line| line.contains("blob container")
                    && line.contains("failed its own checksum")),
            "the flipped middle record must be named as damage: {:?}",
            report.damaged
        );
        assert!(!report.is_internally_consistent());
    }

    fn block_section_wire_code_for_blob() -> u16 {
        crate::archive::ArchiveSectionKind::ObjectContainer(prikk_object::ObjectType::Blob)
            .wire_code()
    }

    /// The twin, required by the handoff: a crafted tail and an honest one are the same bytes, so
    /// they must get the same outcome. Built by extracting the *real* trailing bytes a genuine,
    /// interrupted second write leaves in a live container (one blob written, then a second
    /// write's own record header started but never finished, then the process "crashes" --
    /// simulated here by truncating the live container file directly, the same shape a real crash
    /// mid-append leaves), and splicing that exact byte sequence into an otherwise-clean one-blob
    /// archive via the same crafting helper every other shape above uses.
    #[test]
    fn a_crafted_tail_and_a_genuine_crash_tail_are_the_same_bytes_and_get_the_same_outcome() {
        // The "crash": one real blob, then a second write that only gets its own header plus part
        // of its body on disk before the container file is truncated -- a real, physically short
        // container file, not a crafted archive.
        let crash_dir = unique_temp_dir("archive-verify-crash-tail-source");
        let crash_layout = RepositoryLayout::init(&crash_dir).expect("init");
        let mut crash_store = FileObjectStore::new(crash_layout.clone());
        crash_store
            .write_object(&signed_text_blob_envelope(
                b"the only blob that finishes writing",
            ))
            .expect("write first blob");
        let container_path = crash_layout.container_slot_path(
            prikk_object::ObjectType::Blob,
            crate::foundation::layout::ContainerSlot::A,
        );
        let full_len = std::fs::metadata(&container_path).expect("stat").len();
        crash_store
            .write_object(&signed_text_blob_envelope(
                b"a second blob whose own write never finishes, because the process is about to crash",
            ))
            .expect("write second blob");
        let grown_len = std::fs::metadata(&container_path).expect("stat").len();
        let second_record_bytes = grown_len - full_len;
        assert!(
            second_record_bytes > 32,
            "the second record must be long enough to truncate meaningfully"
        );
        let truncated_len = full_len + 24; // a genuine short read: part of the second header, no body.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&container_path)
            .expect("open container for truncation");
        file.set_len(truncated_len)
            .expect("truncate (simulated crash)");
        drop(file);

        let mut crash_bytes = Vec::new();
        export_archive(&crash_layout, &mut crash_bytes)
            .expect("export the crash-tailed repository");
        let crash_report = {
            let mut cursor = Cursor::new(crash_bytes.clone());
            verify_archive(&mut cursor).expect("verify the crash-tailed archive")
        };
        assert!(
            crash_report.damaged.is_empty(),
            "a genuine crash tail must never be named as damage: {:?}",
            crash_report.damaged
        );
        assert!(
            !crash_report.interrupted_appends.is_empty(),
            "a genuine crash tail must be named as an interrupted append"
        );

        // Extract the real trailing bytes the crash left (relative to the blob section's own
        // body start), then splice the identical bytes into an otherwise-clean one-blob archive.
        let (_, _, crash_entries) = decode_manifest_for_test(&crash_bytes);
        let (_, crash_offset, crash_length, _) = *crash_entries
            .iter()
            .find(|(kind, ..)| *kind == block_section_wire_code_for_blob())
            .expect("blob section present");
        let crash_body_start = crash_offset as usize + 18;
        let crash_tail_len = (truncated_len - full_len) as usize;
        let crash_tail_bytes = &crash_bytes[crash_body_start + crash_length as usize
            - crash_tail_len
            ..crash_body_start + crash_length as usize];

        let mut crafted_dir_bytes = {
            let dir = unique_temp_dir("archive-verify-crafted-twin");
            let layout = RepositoryLayout::init(&dir).expect("init");
            let mut store = FileObjectStore::new(layout.clone());
            store
                .write_object(&signed_text_blob_envelope(
                    b"the only blob that finishes writing",
                ))
                .expect("write the one real blob");
            let mut bytes = Vec::new();
            export_archive(&layout, &mut bytes).expect("export");
            bytes
        };
        insert_tail_bytes_into_section_for_test(
            &mut crafted_dir_bytes,
            block_section_wire_code_for_blob(),
            crash_tail_bytes,
        );
        let crafted_report = {
            let mut cursor = Cursor::new(crafted_dir_bytes);
            verify_archive(&mut cursor).expect("verify the crafted-twin archive")
        };
        assert_eq!(
            crash_report.damaged.len(),
            crafted_report.damaged.len(),
            "the crafted twin must agree with the genuine crash on damage"
        );
        assert_eq!(
            crash_report.is_internally_consistent(),
            crafted_report.is_internally_consistent(),
            "the same bytes must get the same consistency verdict regardless of how they got there"
        );
        assert!(
            !crafted_report.interrupted_appends.is_empty(),
            "the crafted twin must also read the spliced bytes as an interrupted append"
        );
    }
}
