//! Verify tests (RFC 155 implementation Part V).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::type_complexity
)]

use std::collections::HashMap;
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

/// External review 024's own finding: starting from a real export of the fixture above (one
/// genuinely signed, genuinely adopted maintainer signature, `signatures_checked == 1`), stripping
/// the archive's own carried `MaintainerKeys`/`TrustPolicy` sections to empty -- real byte surgery
/// on a real export, never a hand-assembled archive -- turns that same signature into *"not
/// checked"*, per §9.4 (R2: trust is the receiver's). This is the rule's own correct consequence,
/// not a defect, but a report that only ever printed `signatures checked: N` read identically
/// whether 0 of 1 or 1 of 1 signatures were actually checked. Both counts, and the note explaining
/// what a nonzero `not_checked` count means, must appear together.
#[test]
fn verify_shows_both_signature_counts_when_the_key_section_is_stripped() {
    let dir = unique_temp_dir("archive-verify-stripped-key-section");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let signer = crate::Ed25519MaintainerSigner::from_seed("archive-verify-stripped", &[0x53; 32])
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

    // Confirm the baseline first: this real export genuinely checks the signature before any
    // stripping, or the rest of this test would be proving nothing.
    {
        let mut cursor = Cursor::new(bytes.clone());
        let before = verify_archive(&mut cursor).expect("verify before stripping");
        assert_eq!(
            before.signatures_checked, 1,
            "baseline: checked, before: {before:?}"
        );
        assert!(
            before.not_checked_signatures.is_empty(),
            "baseline: nothing not-checked yet: {before:?}"
        );
    }

    let maintainer_keys_wire_code = crate::archive::ArchiveSectionKind::MaintainerKeys.wire_code();
    let trust_policy_wire_code = crate::archive::ArchiveSectionKind::TrustPolicy.wire_code();
    crate::archive::tests::replace_section_body_for_test(
        &mut bytes,
        maintainer_keys_wire_code,
        &[],
    );
    crate::archive::tests::replace_section_body_for_test(&mut bytes, trust_policy_wire_code, &[]);

    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify after stripping the key section");
    assert_eq!(
        report.signatures_checked, 0,
        "with the key section stripped, nothing can be checked against carried material: \
         {report:?}"
    );
    assert_eq!(
        report.not_checked_signatures.len(),
        1,
        "the one real signature must now read as not checked, not vanish: {report:?}"
    );
    assert!(
        report.is_internally_consistent(),
        "a stripped key section is not damage -- trust is the receiver's own decision: {report:?}"
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

/// I-E's own aside (RFC 155 §9.4): each section's own on-disk trailing checksum must equal what
/// the manifest itself recorded for that section, or it is damage -- a *legitimately re-encoded*
/// manifest (its own outer checksum recomputed to match) that nonetheless disagrees with one
/// section's own real, untouched trailing bytes. The exporter always writes the identical value to
/// both places, so this divergence can only mean the manifest and the section were not produced
/// together (e.g. one section's own bytes were swapped in from elsewhere, a shape each
/// individually self-consistent, but the carried manifest no longer names them correctly).
#[test]
fn verify_refuses_a_manifest_entry_checksum_that_disagrees_with_its_own_section() {
    let mut bytes = build_one_block_archive();
    let checksum_offset = entry_record_offset(&bytes, 0) + 18;
    bytes[checksum_offset] ^= 0xFF;
    crate::archive::tests::recompute_manifest_checksum_for_test(&mut bytes);
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        !report.is_internally_consistent(),
        "a manifest entry checksum disagreeing with its own section must not be consistent: \
         {report:?}"
    );
    assert!(
        report
            .damaged
            .iter()
            .any(|line| line.contains("checksum") || line.contains("truncated")),
        "must name the section's own checksum problem: {:?}",
        report.damaged
    );
}

/// I-E item 3 (review `rfc155-impl-IC-review-v1`): the section-count bound's own refusal named no
/// subject at all (*"declares 20000 entries, over the bound of 10000"*), borrowing wording that
/// could not tell a user what was actually counted. The whole message is asserted, not merely
/// that some refusal happened.
#[test]
fn verify_names_the_subject_of_a_manifest_section_count_over_its_bound() {
    let mut bytes = build_one_block_archive();
    crate::archive::tests::rewrite_manifest_entry_count_for_test(&mut bytes, 20_000);
    let mut cursor = Cursor::new(bytes);
    let err = verify_archive(&mut cursor).expect_err("an over-bound section count must refuse");
    assert_eq!(
        err.to_string(),
        "the archive manifest declares 20000 sections, over the bound of 10000",
        "the whole message must name the subject, not only that a count was over bound"
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

/// 0.52.0 step 0, Part C1: a Block that no carried ref reaches, whose own `patch_ids` names a patch
/// id nobody ever wrote -- the same shape `verify_repository_detects_every_missing_referenced_
/// object_as_a_remnant` (`verify/tests.rs`) already proves is a harmless remnant for a *live*
/// repository (RFC 164 Rule E, §5's own standing example: a killed `bundle import` leaves exactly
/// this). The archive's own export/verify/import must agree with the store, not refuse an archive
/// of a repository whose own `prikk verify` already exits 0 over this.
#[test]
fn verify_names_an_unreferenced_remnant_instead_of_refusing_an_archive_over_it() {
    let dir = unique_temp_dir("archive-verify-remnant");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    let missing_patch_id = prikk_object::ObjectId::from_bytes([0xCD; 32]);
    // Never published by any ref, received pointer, or queued patch -- by construction unreachable,
    // the same way the store's own mirror test builds it.
    let orphan_block = crate::test_gates::test_support::signed_block(
        prikk_object::BlockKind::Root,
        Vec::new(),
        vec![missing_patch_id],
        None,
    );
    let orphan_block_id = object_store
        .write_object(&orphan_block)
        .expect("write block");

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        report.is_internally_consistent(),
        "an unreachable block's own missing reference must not refuse the whole archive: {report:?}"
    );
    assert!(
        report.damaged.is_empty(),
        "a remnant is never damage: {:?}",
        report.damaged
    );
    assert!(
        report.unreferenced_remnants.iter().any(|remnant| {
            remnant.owner_object_id == orphan_block_id
                && remnant.missing_object_id == missing_patch_id
        }),
        "the remnant must name both the owning block and what it lacks: {:?}",
        report.unreferenced_remnants
    );
}

/// 0.52.0 step 0, Part C1 round 2, control (a): before this round, `classify_references` required
/// `previous_ref_state_id` only for a *received* ref's own current tip (C1-1) -- a local ref's own
/// chain was never checked at all, even though the archive carries the `RefLog` section that would
/// catch it (`stream_opaque_section` only checksums it). `RefStore::publish` itself cross-checks a
/// publication's own fields against its own CAS precondition, so an honest `publish` call can never
/// produce this shape -- this builds it the way a crafted or damaged archive would carry it instead:
/// two real, signed RefState/RefUpdate object pairs, written directly (never through `publish`, which
/// this specific shape cannot pass), with the ref-log's own second record naming a previous id that
/// is not the first record's own id. The fix (sharing `refs::verify::scan::verify_update` over the
/// archive's own carried `RefLog` section, through `ArchiveObjectReader`) catches it as a
/// chain/sequence divergence -- the identical check `refs::verify::scan::validate_log_replay` already
/// runs for a *live* repository's own ref log.
#[test]
fn control_a_a_local_tip_naming_a_missing_previous_ref_state_is_damage() {
    let dir = unique_temp_dir("c1r2-control-a");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    let root_block = crate::test_gates::test_support::signed_block(
        prikk_object::BlockKind::Root,
        Vec::new(),
        Vec::new(),
        None,
    );
    let root_block_id = object_store.write_object(&root_block).expect("write block");

    let ref_state_1 = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        None,
        root_block_id,
        1,
    );
    let ref_state_1_id = ref_state_1.object_id();
    object_store
        .write_object(&ref_state_1)
        .expect("write ref state 1");
    let ref_update_1 = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        None,
        ref_state_1_id,
        root_block_id,
        1,
    );

    // The second record's own declared `old_ref_state_id` names an id that is not `ref_state_1_id`
    // -- nothing the honest chain above ever produced. An honest `RefStore::publish` call would
    // refuse this (its own CAS precondition must equal the pointer's actual current tip); appending
    // it directly to the ref-log container simulates what a crafted or damaged archive could still
    // carry.
    let fabricated_previous = prikk_object::ObjectId::from_bytes([0xEE; 32]);
    let ref_state_2 = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        Some(fabricated_previous),
        root_block_id,
        2,
    );
    let ref_state_2_id = ref_state_2.object_id();
    object_store
        .write_object(&ref_state_2)
        .expect("write ref state 2");
    let ref_update_2 = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        Some(fabricated_previous),
        ref_state_2_id,
        root_block_id,
        2,
    );

    let ref_name_key = crate::foundation::layout::ref_name_key_bytes("heads/main");
    let log_path = layout.ref_log_container_slot_path(crate::foundation::layout::ContainerSlot::A);
    let mut log_bytes = Vec::new();
    log_bytes.extend_from_slice(
        &crate::refs::encode_ref_container_record_for_test(ref_name_key, &ref_update_1)
            .expect("encode record 1"),
    );
    log_bytes.extend_from_slice(
        &crate::refs::encode_ref_container_record_for_test(ref_name_key, &ref_update_2)
            .expect("encode record 2"),
    );
    std::fs::write(&log_path, &log_bytes).expect("write ref log container");
    crate::refs::write_ref_pointer_candidate_for_test(&layout, "heads/main", ref_state_2_id)
        .expect("write pointer entry");

    let mut bytes = Vec::new();
    export_archive(&layout, &mut bytes).expect("export");
    let mut cursor = Cursor::new(bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        !report.reference_closure_sound,
        "a local chain whose second record names a fabricated previous id must be damage: \
         {report:?}"
    );
    assert!(
        report.damaged.iter().any(|line| line.contains("diverges")),
        "damaged must name the chain divergence: {:?}",
        report.damaged
    );
}

/// 0.52.0 step 0, Part C1 round 2, control (b): round 1's own `classify_references` judged an
/// `Attestation` against `required_attestation_ids` only if the `RefState` section had already
/// streamed while pass 2 was *still in progress* (C1-2) -- the real exporter always writes RefState
/// (wire code 2) before Attestation (wire code 4), so this was dormant, never caught by a test.
/// `validate_manifest_shape` (`archive/verify.rs:1541-1559`) requires every manifest entry's own
/// `offset` to equal the running sum of every section before it, in list order -- so a well-formed
/// archive's manifest entries are *always* listed in the same order their sections physically sit in
/// the file, and a genuinely adversarial reorder would mean physically relocating section bytes, not
/// just swapping two manifest entries (which `validate_manifest_shape` itself then refuses as
/// self-disagreeing, confirmed empirically against this exact case). The fix instead removes the
/// order-dependence structurally: pass 2 now only builds `id_to_envelope` (one entry per carried
/// object, regardless of which section it came from or when), and every ref check -- including
/// `ensure_required_attestations_present` -- runs in a separate pass *after* pass 2 completes, against
/// the finished map, through [`super::ArchiveObjectReader`]. This test proves that function directly,
/// built from a map whose own construction order is exactly backwards from the real exporter's own
/// section order (Attestation inserted before RefState) -- a `HashMap` carries no order at all, so
/// this is the only way to state "this does not depend on order" as a reproducible test rather than
/// an unverified claim about the exporter's own section order never changing.
#[test]
fn control_b_a_required_attestation_whose_target_is_missing_is_damage_regardless_of_insertion_order()
 {
    let missing_attestation_target = prikk_object::ObjectId::from_bytes([0xFA; 32]);
    let attestation_envelope = {
        let payload = prikk_object::AttestationPayload {
            target_block_id: missing_attestation_target,
            policy_version: "v1".to_string(),
            plugin_set_hash: vec![0u8; 32],
            results: Vec::new(),
            status: prikk_object::AttestationStatus::Pass,
            created_at: 0,
            is_reproducible_offline: false,
        };
        let bytes = payload.to_canonical_bytes().expect("encode attestation");
        let mut envelope =
            prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Attestation, 1, bytes);
        envelope
            .add_signature(crate::test_gates::test_support::maintainer_signature())
            .expect("sign");
        envelope
    };
    let attestation_id = attestation_envelope.object_id();

    let root_block = crate::test_gates::test_support::signed_block(
        prikk_object::BlockKind::Root,
        Vec::new(),
        Vec::new(),
        None,
    );
    let root_block_id = root_block.object_id();

    let ref_state = {
        let payload = prikk_object::RefStatePayload {
            ref_name: "heads/main".to_string(),
            kind: prikk_object::RefKind::Branch,
            target_object_id: root_block_id,
            update_seq: 1,
            previous_ref_state_id: None,
            required_attestation_ids: vec![attestation_id],
            closed: false,
        };
        let bytes = payload.to_canonical_bytes().expect("encode ref state");
        let mut envelope =
            prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::RefState, 1, bytes);
        envelope
            .add_signature(crate::test_gates::test_support::maintainer_signature())
            .expect("sign");
        envelope
    };
    let pointer_entry = crate::refs::PointerIndexEntry {
        ref_name_key: crate::foundation::layout::ref_name_key_bytes("heads/main"),
        ref_name: "heads/main".to_string(),
        ref_state_id: ref_state.object_id(),
    };

    // Inserted in the exact opposite order from the real exporter's own section order (Attestation,
    // then RefState, then Block) -- a `HashMap` is unordered regardless, which is itself the point:
    // nothing about this check can depend on insertion order.
    let mut envelopes = HashMap::new();
    envelopes.insert(attestation_id, attestation_envelope);
    envelopes.insert(ref_state.object_id(), ref_state);
    envelopes.insert(root_block_id, root_block);
    let reader = super::ArchiveObjectReader {
        envelopes: &envelopes,
    };

    let outcome = crate::refs::verify::verify_one_pointer_entry(&reader, &pointer_entry);
    assert!(
        outcome.is_err(),
        "a required attestation whose own target is missing must be damage, regardless of which \
         order the archive's objects were decoded in: {outcome:?}"
    );
}

/// 0.52.0 step 0, Part C1 round 2, control (c): the store's own `ensure_ref_target_valid` is
/// kind-aware -- a `Branch` tip must resolve to a `Block` object specifically, by a *typed* read.
/// Round 1's own `require_reference` checked only `ids.contains(&target)`, an untyped id-set
/// membership test that cannot tell a `Block` from any other carried object (C1-3).
#[test]
fn control_c_a_branch_targeting_a_carried_non_block_is_damage() {
    let dir = unique_temp_dir("c1r2-control-c");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    let blob_envelope = signed_text_blob_envelope(b"not a block\n");
    let blob_id = object_store
        .write_object(&blob_envelope)
        .expect("write blob");

    let ref_state =
        crate::test_gates::test_support::signed_ref_state_envelope("heads/main", None, blob_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        None,
        ref_state_id,
        blob_id,
        1,
    );
    crate::RefStore::new(layout.clone())
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
        "a Branch targeting a carried Blob, not a Block, must be damage: {report:?}"
    );
}

/// 0.52.0 step 0, Part C1 round 2, control (e): the perturbation half of controls (a)-(c) above --
/// proving the test fixture itself is sensitive to *which* check runs, not merely a passing shape
/// (the same "a control that cannot fail is not a control" discipline `bundle_preview_write_nothing_
/// control_can_fail` already applies). Reuses control (c)'s exact fixture: a Branch RefState whose
/// carried target is a `Blob`, not a `Block`. The real, typed, shared check
/// (`ensure_ref_target_valid`) correctly classifies this as damage. An untyped existence check over
/// the same id set -- the shape round 1's own `require_reference` used, and the shape a second,
/// re-derived copy of this check would plausibly still use -- would have wrongly accepted it: the
/// blob is, after all, a carried object. Simulated directly here (not by editing
/// `ensure_ref_target_valid` itself, which the assertion above already proves correct), the same way
/// the bundle preview control's own perturbation is simulated rather than introduced into
/// production code.
#[test]
fn control_e_perturbing_the_shared_check_to_an_untyped_one_would_wrongly_pass() {
    let dir = unique_temp_dir("c1r2-control-e");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    let blob_envelope = signed_text_blob_envelope(b"not a block\n");
    let blob_id = object_store
        .write_object(&blob_envelope)
        .expect("write blob");

    let ref_state =
        crate::test_gates::test_support::signed_ref_state_envelope("heads/main", None, blob_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        None,
        ref_state_id,
        blob_id,
        1,
    );
    crate::RefStore::new(layout.clone())
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
        "the real, typed, shared check must still catch this: {report:?}"
    );

    // The perturbation: an untyped `ids.contains(&target)` check, as round 1's own
    // `require_reference` performed it, over the identical carried id set this archive holds.
    let mut untyped_id_set = std::collections::HashSet::new();
    untyped_id_set.insert(blob_id);
    assert!(
        untyped_id_set.contains(&blob_id),
        "an untyped, re-derived copy of the old check would have wrongly accepted this archive -- \
         proving why this part shares the real, typed function instead of re-deriving one"
    );
}

/// 0.52.0 step 0, Part C1 round 2, control (d): the strict side. The store's own `ensure_ref_target_
/// valid`/`ensure_required_attestations_present` are only ever reached *from a ref's own current
/// tip* -- a `Tag` object no ref reaches is never read at all, and a received ref's own superseded
/// (non-tip) `RefState` is never re-checked once a newer one lands (`reduce_received_index_entries`
/// keeps only the latest per name). Both are carried, both name a missing target, and neither is
/// damage -- matching the store's own rule, not a stricter one the archive invents.
#[test]
fn control_d_an_orphan_tag_and_a_superseded_received_target_are_not_damage() {
    let dir = unique_temp_dir("c1r2-control-d-tag");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    let missing_target = prikk_object::ObjectId::from_bytes([0x7A; 32]);
    let digest = crate::patch_set_digest::compute_patch_set_digest(&[]).expect("digest");
    let tag_payload = prikk_object::TagPayload {
        name: "release".to_string(),
        target_block_id: missing_target,
        message: None,
        created_at: 0,
        author_key_id: "maintainer-key".to_string(),
        patch_set_digest: digest,
        patch_count: 0,
    };
    let bytes = tag_payload.to_canonical_bytes().expect("encode tag");
    let mut tag_envelope =
        prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Tag, 1, bytes);
    tag_envelope
        .add_signature(crate::test_gates::test_support::maintainer_signature())
        .expect("sign");
    object_store.write_object(&tag_envelope).expect("write tag");

    let mut archive_bytes = Vec::new();
    export_archive(&layout, &mut archive_bytes).expect("export");
    let mut cursor = Cursor::new(archive_bytes);
    let report = verify_archive(&mut cursor).expect("verify");
    assert!(
        report.reference_closure_sound,
        "an orphan Tag no ref reaches must not be damage even with a missing target: {report:?}"
    );
    assert!(report.damaged.is_empty(), "damaged: {:?}", report.damaged);

    let dir2 = unique_temp_dir("c1r2-control-d-received");
    let layout2 = RepositoryLayout::init(&dir2).expect("init");
    let mut object_store2 = FileObjectStore::new(layout2.clone());
    let root_block = crate::test_gates::test_support::signed_block(
        prikk_object::BlockKind::Root,
        Vec::new(),
        Vec::new(),
        None,
    );
    let root_block_id = object_store2
        .write_object(&root_block)
        .expect("write block");
    let missing_target2 = prikk_object::ObjectId::from_bytes([0x7B; 32]);
    let superseded = crate::test_gates::test_support::signed_ref_state_envelope(
        "remotes/origin/heads/main",
        None,
        missing_target2,
        1,
    );
    let superseded_id = superseded.object_id();
    object_store2
        .write_object(&superseded)
        .expect("write superseded ref state");
    crate::received::write_received_pointer(&layout2, "remotes/origin/heads/main", superseded_id)
        .expect("write received pointer");

    let tip = crate::test_gates::test_support::signed_ref_state_envelope(
        "remotes/origin/heads/main",
        Some(superseded_id),
        root_block_id,
        2,
    );
    let tip_id = tip.object_id();
    object_store2
        .write_object(&tip)
        .expect("write tip ref state");
    crate::received::write_received_pointer(&layout2, "remotes/origin/heads/main", tip_id)
        .expect("write received pointer");

    let mut archive_bytes2 = Vec::new();
    export_archive(&layout2, &mut archive_bytes2).expect("export");
    let mut cursor2 = Cursor::new(archive_bytes2);
    let report2 = verify_archive(&mut cursor2).expect("verify");
    assert!(
        report2.reference_closure_sound,
        "a superseded received RefState's own missing target must not be damage: {report2:?}"
    );
    assert!(report2.damaged.is_empty(), "damaged: {:?}", report2.damaged);
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

    /// `_min_tail_len` is kept at every call site for documentation (the shape each test builds),
    /// but is no longer asserted as a byte count parsed from message text: I-D2's own resync means
    /// an all-garbage tail *longer* than one container record header (the 100-zero-byte and 16 MiB
    /// shapes) is reached via the `Invalid` path (a bad magic/version at the garbage's own start),
    /// whose own bare resync finds nothing before the section's end -- the real store's own
    /// `decode_container_records` reports this with **no** aggregate trailing-byte-count message
    /// at all for that specific path (confirmed from source: `FrameAttempt::Invalid`'s own `None`
    /// branch returns `trailing_partial_bytes: 0`, unlike `TrailingPartial`'s own `NoSoundFrame`
    /// branch, which does report a real count) -- only the per-offset "invalid magic" message,
    /// which names *where*, not *how many bytes*. A byte-count assertion here would therefore be
    /// asserting a live-store quirk this report deliberately does not reproduce as a false
    /// positive. What every shape *does* guarantee, checked below: nothing is damage, something is
    /// named, and the archive is still internally consistent.
    fn assert_incomplete_tail_named_not_damage(bytes: Vec<u8>, _min_tail_len: u64) {
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
        // External review 024: the *classification* (`damaged` empty, above) was already right,
        // but one call site's own printed text still said "damage" -- borrowed verbatim from
        // `foundation::frame_resync::partial_before_sound_frame_message`, written for several
        // live-repository readers that do treat this shape as damage in their own context. The
        // text a user reads must agree with the classification it describes, never contradict it.
        for line in &report.interrupted_appends {
            assert!(
                !line.contains("damage"),
                "an interrupted-append line must never say \"damage\": {line:?}"
            );
        }
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

    /// RFC 155 Part I-D2: the exact shape review `rfc155-impl-ID-review-v1` reproduced against
    /// Part I-D's own code and found broken -- a torn frame (a crash mid-append) that a later,
    /// genuine write overran. The source repository's own container read resyncs past the torn
    /// bytes and still finds the second write; Part I-D's own archive walk did not, and lost the
    /// second blob entirely. Built from real writes, a direct append of a real-shaped torn header
    /// (never through `write_object`, which cannot leave a torn frame on its own), then a real
    /// second write -- not a hand-assembled archive.
    #[test]
    fn an_overrun_torn_frame_is_an_interrupted_append_and_the_object_after_it_still_imports() {
        let dir = unique_temp_dir("archive-verify-overrun-source");
        let layout = RepositoryLayout::init(&dir).expect("init");
        let mut store = FileObjectStore::new(layout.clone());
        store
            .write_object(&signed_text_blob_envelope(
                b"the first blob, which finishes writing",
            ))
            .expect("write first blob");

        let container_path = layout.container_slot_path(
            prikk_object::ObjectType::Blob,
            crate::foundation::layout::ContainerSlot::A,
        );
        {
            use std::io::Write as _;
            let magic =
                crate::foundation::container::container_magic(prikk_object::ObjectType::Blob)
                    .expect("magic");
            let mut torn = Vec::new();
            torn.extend_from_slice(magic);
            torn.extend_from_slice(&1u16.to_be_bytes()); // CONTAINER_VERSION
            torn.extend_from_slice(&999u64.to_be_bytes()); // a claimed body length far longer than what follows
            torn.extend_from_slice(&[0xABu8; 32]); // a checksum never reached, since the body never finishes
            torn.extend_from_slice(&[0u8; 10]); // only 10 of the claimed body bytes actually arrive
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&container_path)
                .expect("open container for append");
            file.write_all(&torn).expect("append the torn frame");
        }
        // Containers are append-only: the store has no idea the file now ends in torn bytes, and
        // appends its own next record right after them -- the new record's own header starts
        // inside the range the torn header's own (never-validated) length claims.
        let second = store
            .write_object(&signed_text_blob_envelope(
                b"the second blob, written after a crash the store never saw",
            ))
            .expect("write second blob");

        let mut bytes = Vec::new();
        export_archive(&layout, &mut bytes).expect("export");
        let report = {
            let mut cursor = Cursor::new(bytes.clone());
            verify_archive(&mut cursor).expect("verify")
        };
        assert!(
            report.damaged.is_empty(),
            "the overrun torn frame must never be named as damage: {:?}",
            report.damaged
        );
        assert!(
            !report.interrupted_appends.is_empty(),
            "the overrun torn frame must be named as an interrupted append"
        );
        // External review 024's own exact finding, on this exact shape: the line used to read
        // "... but a sound frame starts at byte offset M: damage, not a torn tail" --
        // `foundation::frame_resync::partial_before_sound_frame_message`'s own prose, leaked
        // through unchanged. It must now read as an interrupted append, in the archive's own
        // words, never the store-reader-facing word "damage".
        for line in &report.interrupted_appends {
            assert!(
                !line.contains("damage"),
                "the reviewer's own overrun shape must never print \"damage\": {line:?}"
            );
            assert!(
                line.contains("interrupted append")
                    && line.contains("claims more bytes than remain")
                    && line.contains("a sound frame starts at byte offset")
                    && line.ends_with("not imported"),
                "the overrun line must read as an interrupted append, in the archive's own \
                 words: {line:?}"
            );
        }
        assert!(
            report.is_internally_consistent(),
            "an overrun torn frame alone must leave the archive internally consistent: {report:?}"
        );

        let dest_dir = unique_temp_dir("archive-verify-overrun-dest");
        let dest_layout = RepositoryLayout::init(&dest_dir).expect("init");
        let mut cursor = Cursor::new(bytes);
        crate::archive::import_archive(
            &dest_layout,
            &mut cursor,
            &crate::archive::ArchiveImportOptions::default(),
        )
        .expect("import must succeed: the source's own verify only ever warns about this shape");
        assert!(
            crate::FileObjectStore::new(dest_layout.clone())
                .contains_object(prikk_object::ObjectType::Blob, second),
            "the second blob, written after the overrun, must not be lost to import"
        );
    }
}

/// The differential test the architect's own ruling requires (review `rfc155-impl-ID2-review-v1`):
/// a streamed re-derivation of `foundation::container`'s own classification rule is acceptable only
/// when held equal to that real decoder by a test, not merely by inspection. Every shape below
/// builds a section's own record-area bytes, runs the real `decode_container_records` over them
/// (a test can afford the whole-buffer read; production code never does) and the archive's own
/// streamed `for_each_object_record` over the identical bytes wrapped in a minimal one-section
/// buffer, and asserts the two agree on every sound frame's own offset and object id, every
/// complete failure's own offset, and every interrupted append's own offset.
mod differential_against_the_store {
    use std::io::Cursor;

    use prikk_object::ObjectType;

    use crate::foundation::container::{
        ContainerRecordStatus, container_magic, decode_container_records, record_checksum,
    };
    use crate::test_gates::test_support::signed_text_blob_envelope;

    use super::super::for_each_object_record;
    use super::super::{ArchiveSectionKind, ManifestEntry, SECTION_MAGIC};

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum RealClass {
        Sound,
        Damage,
        Interrupted,
    }

    /// The real store's own ground-truth classification, by offset, over raw record-area bytes --
    /// exactly what `foundation::container::decode_container_records` already returns, read
    /// directly rather than through any archive-side wrapper.
    fn real_classification(object_type: ObjectType, bytes: &[u8]) -> Vec<(usize, RealClass)> {
        let replay = decode_container_records(object_type, bytes).expect("store decode");
        let mut classified: Vec<(usize, RealClass)> = replay
            .record_outcomes
            .iter()
            .map(|outcome| {
                let class = match &outcome.status {
                    ContainerRecordStatus::Evaluated { .. } => RealClass::Sound,
                    ContainerRecordStatus::Failed { complete: true, .. } => RealClass::Damage,
                    ContainerRecordStatus::Failed {
                        complete: false, ..
                    } => RealClass::Interrupted,
                };
                (outcome.offset, class)
            })
            .collect();
        // `decode_container_records`'s own `TrailingPartial`/`NoSoundFrame` path returns *without*
        // pushing a `Failed` entry at all -- the trailing bytes live only in its own separate
        // `trailing_partial_bytes` field. Fold it in here as the equivalent interrupted-append
        // offset, or every shape that ends in a genuine, unresyncable tail (most of them) would
        // read as "the store found nothing to say," which is not true.
        if replay.trailing_partial_bytes > 0 {
            classified.push((
                bytes.len() - replay.trailing_partial_bytes,
                RealClass::Interrupted,
            ));
        }
        classified
    }

    /// Every numeral substring in `message` -- used to recover the byte offset each archive-side
    /// message names (every one of this module's own messages names at least one), without
    /// depending on exact wording.
    fn numbers_in(message: &str) -> Vec<usize> {
        let mut numbers = Vec::new();
        let mut current = String::new();
        for ch in message.chars() {
            if ch.is_ascii_digit() {
                current.push(ch);
            } else if !current.is_empty() {
                if let Ok(value) = current.parse() {
                    numbers.push(value);
                }
                current.clear();
            }
        }
        if !current.is_empty() {
            if let Ok(value) = current.parse() {
                numbers.push(value);
            }
        }
        numbers
    }

    /// Wrap raw record-area `bytes` in the minimal one-section framing `for_each_object_record`
    /// itself expects (`archive.rs`'s own shape: magic, kind, length, the bytes verbatim, a
    /// trailing checksum), and run it -- the archive's own streamed classification, over the
    /// identical bytes `real_classification` above just read directly.
    /// `Err` only for [`prikk_error::PrikkError::ObjectOverBound`] -- I-E item 0(c): after item
    /// 0(a)'s reorder, `parse_frame_at_streamed` raises this **only** once a candidate's claimed
    /// body is already known to fit entirely within the section's own real, present bytes (the
    /// past-the-end check runs first) and *still* exceeds the 256 MiB per-object bound. That is
    /// the one legitimate divergence: `decode_container_records` never applies this bound at all
    /// (it trusts its own local container file; the bound exists only because the archive is
    /// untrusted input), so it would read a record this large; the archive correctly refuses it
    /// (DC-86). No other call path in this module can produce this error, so this `Err` arm is
    /// already exactly this one shape, not a broad catch-all -- [`assert_held_equal_to_the_store`]
    /// treats it as "nothing further to compare." Any `ObjectOverBound` arising from a claim that
    /// does *not* fit the section would be a real differential-test failure; before item 0(a)'s
    /// fix such a claim could never reach here at all (the bound was checked before the
    /// past-the-end test), which is exactly why this `Err` arm was unreachable, and therefore
    /// dead, prior to this part.
    fn archive_classification(
        object_type: ObjectType,
        bytes: &[u8],
    ) -> Result<(Vec<(usize, RealClass)>, Vec<prikk_object::ObjectId>), prikk_error::PrikkError>
    {
        let kind = ArchiveSectionKind::ObjectContainer(object_type);
        let wire_code = kind.wire_code().to_be_bytes();
        let length = (bytes.len() as u64).to_be_bytes();
        let checksum = prikk_hash::sha256_parts(&[SECTION_MAGIC, &wire_code, &length, bytes]);
        let mut buffer = Vec::new();
        buffer.extend_from_slice(SECTION_MAGIC);
        buffer.extend_from_slice(&wire_code);
        buffer.extend_from_slice(&length);
        buffer.extend_from_slice(bytes);
        buffer.extend_from_slice(&checksum);
        let entry = ManifestEntry {
            kind,
            offset: 0,
            length: bytes.len() as u64,
            checksum,
        };
        let mut cursor = Cursor::new(buffer);
        let mut sound = Vec::new();
        let outcome = for_each_object_record(&mut cursor, &entry, object_type, |body| {
            if let Ok(envelope) = crate::foundation::file_codec::decode_envelope_file(body) {
                sound.push(envelope.object_id());
            }
            Ok(())
        })?;
        let mut classified = Vec::new();
        for message in &outcome.record_damage {
            if let Some(&offset) = numbers_in(message).first() {
                classified.push((offset, RealClass::Damage));
            }
        }
        // `outcome.tail_len` is the one case tracked as a separate, structured field rather than a
        // message (mirroring `decode_container_records`'s own `TrailingPartial`/`NoSoundFrame` arm,
        // which pushes no `record_outcomes` entry at all) -- fold it in at the offset it starts.
        if outcome.tail_len > 0 {
            classified.push((
                bytes.len() - usize::try_from(outcome.tail_len).unwrap_or(usize::MAX),
                RealClass::Interrupted,
            ));
        }
        for message in &outcome.interrupted_appends {
            if let Some(&offset) = numbers_in(message).first() {
                classified.push((offset, RealClass::Interrupted));
            }
        }
        Ok((classified, sound))
    }

    /// Assert the archive's own streamed walk and the real store's own decoder agree on every
    /// offset classified as damage or an interrupted append, over the identical bytes.
    fn assert_held_equal_to_the_store(object_type: ObjectType, bytes: &[u8]) {
        let real = real_classification(object_type, bytes);
        let (archive_classified, archive_sound) = match archive_classification(object_type, bytes) {
            Ok(result) => result,
            Err(prikk_error::PrikkError::ObjectOverBound { .. }) => return,
            Err(err) => panic!("archive streamed classification: {err}"),
        };
        let real_replay = decode_container_records(object_type, bytes).expect("store decode");
        let real_sound: std::collections::BTreeSet<_> = real_replay
            .records
            .iter()
            .map(|record| record.envelope.object_id())
            .collect();
        let archive_sound_set: std::collections::BTreeSet<_> = archive_sound.into_iter().collect();
        assert_eq!(
            real_sound, archive_sound_set,
            "the set of sound object ids must agree with the store"
        );

        let real_damage: std::collections::BTreeSet<usize> = real
            .iter()
            .filter(|(_, class)| *class == RealClass::Damage)
            .map(|(offset, _)| *offset)
            .collect();
        let real_interrupted: std::collections::BTreeSet<usize> = real
            .iter()
            .filter(|(_, class)| *class == RealClass::Interrupted)
            .map(|(offset, _)| *offset)
            .collect();
        let archive_damage: std::collections::BTreeSet<usize> = archive_classified
            .iter()
            .filter(|(_, class)| *class == RealClass::Damage)
            .map(|(offset, _)| *offset)
            .collect();
        let archive_interrupted: std::collections::BTreeSet<usize> = archive_classified
            .iter()
            .filter(|(_, class)| *class == RealClass::Interrupted)
            .map(|(offset, _)| *offset)
            .collect();

        assert_eq!(
            real_damage, archive_damage,
            "damage offsets must agree with the store -- real: {real:?}, archive: {archive_classified:?}"
        );
        assert_eq!(
            real_interrupted, archive_interrupted,
            "interrupted-append offsets must agree with the store -- real: {real:?}, archive: {archive_classified:?}"
        );
    }

    fn one_real_blob_record() -> Vec<u8> {
        let magic = container_magic(ObjectType::Blob).expect("magic");
        let envelope = signed_text_blob_envelope(b"one real blob record, long enough to matter");
        let body = crate::foundation::file_codec::encode_envelope_file_structural(&envelope)
            .expect("encode");
        let checksum = record_checksum(magic, body.len() as u64, &body);
        let mut record = Vec::new();
        record.extend_from_slice(magic);
        record.extend_from_slice(&1u16.to_be_bytes());
        record.extend_from_slice(&(body.len() as u64).to_be_bytes());
        record.extend_from_slice(&checksum);
        record.extend_from_slice(&body);
        record
    }

    #[test]
    fn agrees_on_a_clean_single_record() {
        assert_held_equal_to_the_store(ObjectType::Blob, &one_real_blob_record());
    }

    #[test]
    fn agrees_on_forty_nine_zero_bytes() {
        let mut bytes = one_real_blob_record();
        bytes.extend(std::iter::repeat_n(0u8, 49));
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }

    #[test]
    fn agrees_on_one_hundred_zero_bytes() {
        let mut bytes = one_real_blob_record();
        bytes.extend(std::iter::repeat_n(0u8, 100));
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }

    #[test]
    fn agrees_on_a_bad_magic_in_the_middle_with_sound_frames_after() {
        let mut bytes = one_real_blob_record();
        bytes.extend(std::iter::repeat_n(0xABu8, 64)); // longer than one header, wrong magic
        bytes.extend(one_real_blob_record());
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }

    #[test]
    fn agrees_on_a_torn_frame_claiming_past_the_end_with_sound_frames_after() {
        // A header claiming a huge body, but the section ends almost immediately after it --
        // then a real sound record genuinely starts somewhere inside that claimed range.
        let magic = container_magic(ObjectType::Blob).expect("magic");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(magic);
        bytes.extend_from_slice(&1u16.to_be_bytes());
        bytes.extend_from_slice(&1_000_000u64.to_be_bytes()); // claims way more than remains
        bytes.extend_from_slice(&[0u8; 32]); // checksum, irrelevant -- body never reaches this far
        bytes.extend_from_slice(&[0xCDu8; 20]); // a few garbage bytes standing in for a torn start
        bytes.extend(one_real_blob_record()); // the later, genuine write
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }

    #[test]
    fn agrees_on_a_checksum_mismatch_whose_next_sound_frame_starts_after_its_claimed_end() {
        // A complete, correctly-bounded record whose checksum is wrong (not an overrun -- the
        // claimed end lines up exactly where the next real record starts).
        let mut bad = one_real_blob_record();
        let last_byte = bad.len() - 1;
        bad[last_byte] ^= 0xFF; // corrupt the checksum field itself -- body still "fits"
        let mut bytes = bad;
        bytes.extend(one_real_blob_record());
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }

    /// The review's own overrun shape, built exactly as the probe did: a real header plus 10 of
    /// its own claimed body bytes (a genuine crash mid-append), then a second real write whose own
    /// frame starts inside the first one's own claimed range.
    #[test]
    fn agrees_on_the_reviews_own_overrun_shape() {
        let magic = container_magic(ObjectType::Blob).expect("magic");
        let real = one_real_blob_record();
        // A header claiming the same body length as the real record above, but only its own first
        // 10 declared body bytes are actually present before the "crash" -- then the real record
        // (unrelated content) starts immediately after those 10 bytes, landing inside the torn
        // header's own claimed range.
        let body_len = (real.len() - crate::foundation::container::CONTAINER_HEADER_LEN) as u64;
        let mut torn = Vec::new();
        torn.extend_from_slice(magic);
        torn.extend_from_slice(&1u16.to_be_bytes());
        torn.extend_from_slice(&body_len.to_be_bytes());
        torn.extend_from_slice(&[0u8; 32]);
        torn.extend_from_slice(&[0x11u8; 10]);
        let mut bytes = torn;
        bytes.extend(real);
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }

    #[test]
    fn control_dropping_resync_makes_the_overrun_shape_disagree() {
        // Not a real control on production code (that would need a temporary code edit, done by
        // hand and reverted -- see the report). This test instead documents, as a differential
        // check in its own right, what the OLD (pre-I-D2) behavior would have disagreed on: the
        // old reader stopped at the first unsound frame and called everything after it one flat
        // tail, never finding the second record's own sound frame at all. Confirms the *current*
        // code does find it (the real control, run by hand, is in the report).
        let magic = container_magic(ObjectType::Blob).expect("magic");
        let real = one_real_blob_record();
        let body_len = (real.len() - crate::foundation::container::CONTAINER_HEADER_LEN) as u64;
        let mut torn = Vec::new();
        torn.extend_from_slice(magic);
        torn.extend_from_slice(&1u16.to_be_bytes());
        torn.extend_from_slice(&body_len.to_be_bytes());
        torn.extend_from_slice(&[0u8; 32]);
        torn.extend_from_slice(&[0x11u8; 10]);
        let mut bytes = torn;
        bytes.extend(real);
        let (_classified, sound) =
            archive_classification(ObjectType::Blob, &bytes).expect("archive classification");
        assert_eq!(
            sound.len(),
            1,
            "the second record's own sound frame must still be found after the torn one"
        );
    }

    /// Random byte flips, truncations, insertions, and torn-frame-shaped insertions over a real
    /// multi-record container, comparing the archive's own streamed walk against the store's own
    /// decoder on every case -- this project's own established mutation strategy
    /// (`test_gates::framed_decoder_fuzz`'s `mutation`/`apply`, already shared with `whole_file_
    /// cache_fuzz`), reused here rather than a fourth copy of the same shapes.
    fn multi_record_container() -> Vec<u8> {
        let mut bytes = one_real_blob_record();
        bytes.extend(one_real_blob_record());
        bytes.extend(one_real_blob_record());
        bytes
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(200))]
        #[test]
        fn proptest_random_corruption_agrees_with_the_store(
            mutation in crate::test_gates::framed_decoder_fuzz::mutation(),
        ) {
            let valid = multi_record_container();
            let mutated = crate::test_gates::framed_decoder_fuzz::apply(&valid, &mutation);
            assert_held_equal_to_the_store(ObjectType::Blob, &mutated);
        }
    }

    /// A torn-frame-shaped insertion specifically (real magic, a bogus length, no real body) --
    /// the one shape `framed_decoder_fuzz`'s own general mutation set does not construct on its
    /// own (its `Insert` variant inserts arbitrary bytes, not specifically a plausible-looking
    /// header), so this part's own overrun-adjacent concern gets its own small property too.
    ///
    /// I-E item 0(b): `at` and `claimed_len` were previously restricted to record boundaries and
    /// under 10 MB, on the reasoning that a real crash or append can only ever happen at an
    /// existing record's own boundary. The review's own finding: the archive is untrusted input,
    /// so a writer of *any* bytes is in scope, not only a realistic crash shape -- and that
    /// restriction is exactly what hid I-E item 0(a)'s own bug (a torn header spliced into the
    /// *middle* of a record, corrupting its own length field to read as a huge number). Both
    /// restrictions are removed: `at` ranges over every byte offset in the fixture, and
    /// `claimed_len` ranges over all of `u64`.
    #[test]
    fn proptest_torn_frame_shaped_insertion_agrees_with_the_store() {
        use proptest::prelude::*;
        let magic = container_magic(ObjectType::Blob).expect("magic");
        proptest!(ProptestConfig::with_cases(200), |(at: usize, claimed_len: u64)| {
            let mut bytes = multi_record_container();
            assert_eq!(bytes.len(), 774, "sanity: this fixture's own three records are 258 bytes each");
            let at = at % (bytes.len() + 1);
            let mut torn = Vec::new();
            torn.extend_from_slice(magic);
            torn.extend_from_slice(&1u16.to_be_bytes());
            torn.extend_from_slice(&claimed_len.to_be_bytes());
            torn.extend_from_slice(&[0u8; 32]);
            bytes.splice(at..at, torn);
            assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
        });
    }

    /// 5,000 cases of each property above, run once by hand and reported in the I-E report rather
    /// than committed as the default case count (the committed tests above keep their own
    /// established, cheap default so ordinary `cargo test` stays fast) -- the architect's own
    /// scratch-worktree run that found and fixed item 0(a)'s bug used this count.
    #[test]
    #[ignore = "run deliberately: 5,000 cases of each differential proptest, reported by hand"]
    fn proptest_long_run_five_thousand_cases_each() {
        use proptest::prelude::*;
        let magic = container_magic(ObjectType::Blob).expect("magic");
        proptest!(ProptestConfig::with_cases(5_000), |(mutation in crate::test_gates::framed_decoder_fuzz::mutation())| {
            let valid = multi_record_container();
            let mutated = crate::test_gates::framed_decoder_fuzz::apply(&valid, &mutation);
            assert_held_equal_to_the_store(ObjectType::Blob, &mutated);
        });
        proptest!(ProptestConfig::with_cases(5_000), |(at: usize, claimed_len: u64)| {
            let mut bytes = multi_record_container();
            let at = at % (bytes.len() + 1);
            let mut torn = Vec::new();
            torn.extend_from_slice(magic);
            torn.extend_from_slice(&1u16.to_be_bytes());
            torn.extend_from_slice(&claimed_len.to_be_bytes());
            torn.extend_from_slice(&[0u8; 32]);
            bytes.splice(at..at, torn);
            assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
        });
    }

    /// The review's own minimal failing case, named and deterministic rather than left to the
    /// proptest to rediscover: a torn header with `claimed_len = 0` spliced at byte offset 10,
    /// inside the first record's own header -- this corrupts that record's own length field to
    /// read as a huge, bound-exceeding number. Before I-E item 0(a)'s fix, the archive wrongly
    /// resynced to the next *magic* occurrence (the torn header itself) and called the result
    /// damage; the fix resyncs to the next genuinely *sound* frame, agreeing with the store.
    #[test]
    fn the_reviews_own_minimal_case_offset_ten_claimed_len_zero_agrees_with_the_store() {
        let magic = container_magic(ObjectType::Blob).expect("magic");
        let mut bytes = multi_record_container();
        let mut torn = Vec::new();
        torn.extend_from_slice(magic);
        torn.extend_from_slice(&1u16.to_be_bytes());
        torn.extend_from_slice(&0u64.to_be_bytes());
        torn.extend_from_slice(&[0u8; 32]);
        bytes.splice(10..10, torn);
        assert_held_equal_to_the_store(ObjectType::Blob, &bytes);
    }
}
