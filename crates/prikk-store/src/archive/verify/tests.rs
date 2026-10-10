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
use crate::archive::tests::{decode_manifest_for_test, seal_one_block};
use crate::maintainer_signing::MaintainerSigner;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{ObjectWriter, RepositoryLayout};

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
    let end_trailer_start = bytes.len() - 24;
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
/// author key material is ever recorded, so every signature on it is **correctly** unresolvable --
/// this is exactly R2's "a signature it cannot check is reported, not skipped," demonstrated on the
/// plainest fixture available, not an error in the test. Structure and closure, which do not depend
/// on any of that, are both sound.
#[test]
fn verify_names_every_unresolvable_signature_without_failing_structure_or_closure() {
    let bytes = build_one_block_archive();
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(report.structurally_sound);
    assert!(report.reference_closure_sound);
    assert!(report.damaged.is_empty());
    assert!(
        !report.is_internally_consistent(),
        "a placeholder signature with no adopted key and no recorded author material must not \
         read as internally consistent"
    );
    assert!(
        report
            .unresolvable_signatures
            .iter()
            .any(|line| line.contains("AUTHOR") && line.contains("no recorded material")),
        "unresolvable_signatures: {:?}",
        report.unresolvable_signatures
    );
    assert!(
        report.note.contains("internally consistent"),
        "note: {}",
        report.note
    );
    assert!(
        report.note.contains("not trusted"),
        "the note must say it is not a trust claim, not merely avoid one word: {}",
        report.note
    );
    assert!(
        !report.note.contains("is trusted") && !report.note.starts_with("trusted"),
        "the note must never make an affirmative trust claim: {}",
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

#[test]
fn verify_refuses_an_unknown_section_kind() {
    let mut bytes = build_one_block_archive();
    let record_offset = entry_record_offset(&bytes, 0);
    bytes[record_offset] = 0xFF;
    bytes[record_offset + 1] = 0xFF; // kind 0xFFFF, outside the fixed allow-list
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
