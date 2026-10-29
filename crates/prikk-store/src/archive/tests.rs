//! Export tests (RFC 155 implementation Part E).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::type_complexity
)]

use std::io::Cursor;

use prikk_object::{BlockKind, ObjectId, ObjectType};

use crate::archive::{ArchiveSectionKind, export_archive};
use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test};
use crate::foundation::layout::{ContainerSlot, DEFAULT_ACTIVE_NAME};
use crate::lock::ActiveLock;
use crate::test_gates::test_support::{
    signed_block, signed_patch_envelope, signed_ref_state_envelope, signed_ref_update_envelope,
    unique_temp_dir,
};
use crate::wal::Wal;
use crate::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectWriter, RefPublication,
    RefStore, RepositoryLayout, add_trusted_maintainer, derive_next_state_root,
    maintainer_signature,
};

/// Seal a one-block `heads/main` into `layout`, returning the tip Block id -- the minimum history
/// that gives every object container at least one real record. `pub(crate)`, not private:
/// `archive::verify`'s own tests need it beyond this module's own file. Its signatures are
/// deliberate placeholders that never resolve (`test_support`'s own doc comment: "a value, not a
/// signer") -- `test_gates::archive_decoder_fuzz` used to share this fixture too, until V2's own
/// fix found that choice makes `is_internally_consistent` false even unmutated, which makes that
/// module's whole fuzz property vacuous; it now builds its own genuinely-consistent fixture
/// instead.
pub(crate) fn seal_one_block(layout: &RepositoryLayout) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    // `signed_patch_envelope`'s own `CreateFile` operation names this blob's id -- writing it here
    // is what makes this fixture's reference closure genuinely sound (RFC 155 implementation Part
    // V's own closure check, added after this fixture, found the pre-existing gap: the blob was
    // never actually written).
    object_store.write_object(&crate::test_gates::test_support::signed_patch_blob_envelope())?;
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;
    let root_block = signed_block(BlockKind::Root, Vec::new(), vec![patch_id], None);
    let root_block_id = object_store.write_object(&root_block)?;

    let ref_store = RefStore::new(layout.clone());
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
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(root_block_id)
}

/// 0.56.0 step 3c (design review v2, ruling A5): like [`seal_one_block`], but the sealed `Patch`'s
/// own canonical payload is truncated by one byte **after** being encoded by the ordinary,
/// validating `PatchPayload::to_canonical_bytes()` path -- the envelope itself (its checksum, its
/// placeholder signature, the frame it is written in) is computed fresh over these already-
/// truncated bytes, so nothing at the framing/signature layer is disturbed; only
/// `decode_patch_operations`'s own semantic parse of the payload's own operation TLV fails. This
/// is deliberately **not** the Unicode-path shape 2C v2/A6 found (that is 0.56.0 step 3d's own,
/// later scope, and this round must not classify it) -- any decode failure proves A5's rule, and a
/// truncated record is the cheapest one to construct by hand, the same way 2C v2's own fixtures
/// were built directly rather than through the ordinary, validating CLI pipeline. `pub(crate)`:
/// both `archive::import`'s and `archive::verify`'s own test modules need it.
pub(crate) fn seal_one_block_with_an_undecodable_patch(
    layout: &RepositoryLayout,
) -> prikk_error::Result<ObjectId> {
    let mut object_store = FileObjectStore::new(layout.clone());
    object_store.write_object(&crate::test_gates::test_support::signed_patch_blob_envelope())?;

    let mut patch = signed_patch_envelope();
    // Rebuild with a one-byte-shorter payload, signed fresh over exactly those bytes -- see this
    // function's own doc comment above for why this is safe at every layer except the one this
    // test means to exercise.
    let mut truncated_payload = patch.canonical_payload.clone();
    truncated_payload.pop();
    patch = prikk_object::ObjectEnvelope::unsigned(ObjectType::Patch, 1, truncated_payload);
    patch.add_signature(crate::test_gates::test_support::rollback_author_signature())?;
    let patch_id = object_store.write_object(&patch)?;

    let root_block = signed_block(BlockKind::Root, Vec::new(), vec![patch_id], None);
    let root_block_id = object_store.write_object(&root_block)?;

    let ref_store = RefStore::new(layout.clone());
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
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    Ok(root_block_id)
}

/// Decode one manifest entry's own `(kind, offset, length, checksum)`, test-only -- the real
/// decoder is Part V's job. Returns `(repository_format, tool_version, entries)`. `pub(in
/// crate::archive)`: `archive::verify`'s own tests need to locate a specific section's own byte
/// range to corrupt it precisely, not guess an offset.
pub(in crate::archive) fn decode_manifest_for_test(
    bytes: &[u8],
) -> (u32, String, Vec<(u16, u64, u64, [u8; 32])>) {
    let end_trailer_start = bytes.len() - 56;
    assert_eq!(
        &bytes[end_trailer_start..end_trailer_start + 8],
        b"PRPOEND1"
    );
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
    let manifest = &bytes[manifest_offset..manifest_offset + manifest_length];
    assert_eq!(&manifest[0..8], b"PRPOMAN1");
    let repository_format = u32::from_be_bytes(manifest[8..12].try_into().unwrap());
    let tool_version_len = u16::from_be_bytes(manifest[12..14].try_into().unwrap()) as usize;
    let tool_version = String::from_utf8(manifest[14..14 + tool_version_len].to_vec()).unwrap();
    let mut cursor = 14 + tool_version_len;
    let count = u64::from_be_bytes(manifest[cursor..cursor + 8].try_into().unwrap()) as usize;
    cursor += 8;
    let mut entries = Vec::with_capacity(count);
    for _ in 0..count {
        let kind = u16::from_be_bytes(manifest[cursor..cursor + 2].try_into().unwrap());
        let offset = u64::from_be_bytes(manifest[cursor + 2..cursor + 10].try_into().unwrap());
        let length = u64::from_be_bytes(manifest[cursor + 10..cursor + 18].try_into().unwrap());
        let checksum: [u8; 32] = manifest[cursor + 18..cursor + 50].try_into().unwrap();
        entries.push((kind, offset, length, checksum));
        cursor += 50;
    }
    (repository_format, tool_version, entries)
}

/// Read one section's own body bytes back out of an exported archive, by its manifest entry --
/// test-only; re-verifies the section's own checksum the same way a real verifier will.
fn read_section_body_for_test(
    bytes: &[u8],
    offset: u64,
    length: u64,
    checksum: [u8; 32],
) -> Vec<u8> {
    let offset = offset as usize;
    let header_len = 8 + 2 + 8;
    let body_start = offset + header_len;
    let body_end = body_start + length as usize;
    assert_eq!(&bytes[offset..offset + 8], b"PRPOSEC1", "section magic");
    let body = bytes[body_start..body_end].to_vec();
    let trailer = &bytes[body_end..body_end + 32];
    let recomputed = prikk_hash::sha256_parts(&[
        &bytes[offset..offset + 8],
        &bytes[offset + 8..offset + 10],
        &bytes[offset + 10..offset + 18],
        &body,
    ]);
    assert_eq!(&recomputed, checksum.as_slice(), "section own checksum");
    assert_eq!(trailer, checksum.as_slice(), "trailer matches manifest");
    body
}

/// Rewrite the manifest's own `repository_format` field to `new_format`, then recompute and
/// rewrite the end trailer's manifest checksum to match -- a *legitimately re-encoded* manifest
/// declaring a different format, not a corrupted one. This is what Part V2's own "unknown format"
/// test needs: a build that genuinely writes a format this one does not know about still has a
/// manifest whose checksum is internally correct, so the format check (V1b) must refuse on its own,
/// never merely as a side effect of the checksum check (V1a) catching a byte it did not expect to
/// change. `pub(in crate::archive)`: `archive::verify`'s own tests build this scenario.
pub(in crate::archive) fn rewrite_manifest_repository_format_for_test(
    bytes: &mut [u8],
    new_format: u32,
) {
    let end_trailer_start = bytes.len() - 56;
    let manifest_offset = u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    bytes[manifest_offset + 8..manifest_offset + 12].copy_from_slice(&new_format.to_be_bytes());
    recompute_manifest_checksum_for_test(bytes);
}

/// Rewrite the manifest's own entry `count` field to `new_count`, then recompute and rewrite the
/// end trailer's manifest checksum to match -- a *legitimately re-encoded* manifest declaring a
/// count this build must refuse (I-E item 3), not a corrupted one. The manifest's own layout
/// (`write_manifest`'s own order, `decode_manifest_bytes`'s own read order): magic(8) +
/// repository_format(4) + tool_version_len(2) + tool_version + count(8) -- `count`'s own offset
/// depends on `tool_version`'s own length, so it is located by walking the same fields
/// `decode_manifest_bytes` reads, not assumed at a fixed offset.
pub(in crate::archive) fn rewrite_manifest_entry_count_for_test(bytes: &mut [u8], new_count: u64) {
    let end_trailer_start = bytes.len() - 56;
    let manifest_offset = u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    ) as usize;
    let tool_version_len_offset = manifest_offset + 8 + 4;
    let tool_version_len = u16::from_be_bytes(
        bytes[tool_version_len_offset..tool_version_len_offset + 2]
            .try_into()
            .unwrap(),
    ) as usize;
    let count_offset = tool_version_len_offset + 2 + tool_version_len;
    bytes[count_offset..count_offset + 8].copy_from_slice(&new_count.to_be_bytes());
    recompute_manifest_checksum_for_test(bytes);
}

/// Recompute the end trailer's manifest checksum from `bytes`'s own current manifest body, and
/// rewrite it in place -- for a test that deliberately edits manifest bytes to reach some *other*
/// refusal (an unknown/duplicated section kind, a traversal-shaped string) and needs the checksum
/// check (V1a) to pass so that other refusal is actually what's being exercised, not merely the
/// checksum mismatch every unrecomputed edit would now trigger first.
pub(in crate::archive) fn recompute_manifest_checksum_for_test(bytes: &mut [u8]) {
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
    let body = bytes[manifest_offset..manifest_offset + manifest_length].to_vec();
    let checksum = prikk_hash::sha256(&body);
    bytes[end_trailer_start + 24..end_trailer_start + 56].copy_from_slice(&checksum);
}

/// Insert `extra` bytes right before the named section's own trailing checksum -- inside its own
/// declared length, after its last real record -- and fix up everything that insertion shifts:
/// every manifest entry whose own offset comes after the insertion point (by `extra.len()`), the
/// target section's own declared `length` and recomputed outer checksum, the end trailer's own
/// `manifest_offset`, and the manifest's own checksum. `pub(in crate::archive)`: RFC 155 Part I-D's
/// own test matrix builds every "extra bytes inside a section" shape this way, on a real export,
/// never by hand-assembling a whole archive from scratch.
pub(in crate::archive) fn insert_tail_bytes_into_section_for_test(
    bytes: &mut Vec<u8>,
    section_kind: u16,
    extra: &[u8],
) {
    let (_, _, entries) = decode_manifest_for_test(bytes);
    let (_, target_offset, target_length, _) = *entries
        .iter()
        .find(|(kind, ..)| *kind == section_kind)
        .expect("section present");
    const SECTION_HEADER_LEN: u64 = 8 + 2 + 8;
    let insert_at = (target_offset + SECTION_HEADER_LEN + target_length) as usize;

    bytes.splice(insert_at..insert_at, extra.iter().copied());
    let shift = extra.len() as u64;

    // Every manifest entry whose own section comes after the one just grown moves down by `shift`
    // bytes -- sections are laid out in increasing offset order, so this is every entry whose
    // offset exceeds the insertion point, which is every entry after the target including the
    // target's own is not shifted (it starts before `insert_at`).
    let end_trailer_start = bytes.len() - 56;
    let manifest_offset = u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    );
    let new_manifest_offset = manifest_offset + shift;
    bytes[end_trailer_start + 8..end_trailer_start + 16]
        .copy_from_slice(&new_manifest_offset.to_be_bytes());

    let manifest = &bytes[new_manifest_offset as usize..];
    let tool_version_len = u16::from_be_bytes(manifest[12..14].try_into().unwrap()) as usize;
    let count_at = 14 + tool_version_len;
    let count = u64::from_be_bytes(manifest[count_at..count_at + 8].try_into().unwrap()) as usize;
    let first_entry = new_manifest_offset as usize + count_at + 8;
    for index in 0..count {
        let entry_at = first_entry + index * 50;
        let entry_offset =
            u64::from_be_bytes(bytes[entry_at + 2..entry_at + 10].try_into().unwrap());
        if entry_offset == target_offset {
            let new_length = target_length + shift;
            bytes[entry_at + 10..entry_at + 18].copy_from_slice(&new_length.to_be_bytes());
            let section_header_start = target_offset as usize;
            // The section's own on-disk header also carries its own length (checked against the
            // manifest entry's copy by every reader) -- both must agree, or `header_ok` refuses.
            bytes[section_header_start + 10..section_header_start + 18]
                .copy_from_slice(&new_length.to_be_bytes());
            let body_start = section_header_start + SECTION_HEADER_LEN as usize;
            let body_end = body_start + new_length as usize;
            let checksum = prikk_hash::sha256_parts(&[
                &bytes[section_header_start..section_header_start + 8],
                &bytes[section_header_start + 8..section_header_start + 10],
                &bytes[section_header_start + 10..body_start],
                &bytes[body_start..body_end],
            ]);
            bytes[entry_at + 18..entry_at + 50].copy_from_slice(&checksum);
            bytes[body_end..body_end + 32].copy_from_slice(&checksum);
        } else if entry_offset > target_offset {
            let shifted = entry_offset + shift;
            bytes[entry_at + 2..entry_at + 10].copy_from_slice(&shifted.to_be_bytes());
        }
    }
    recompute_manifest_checksum_for_test(bytes);
}

/// Replace the named section's own body outright with `new_body` (any length, including empty) --
/// unlike [`insert_tail_bytes_into_section_for_test`], which only ever grows a section, this can
/// shrink one. Fixes up everything a length change shifts: every manifest entry whose own offset
/// comes after the target (by the signed delta), the target's own declared `length` and
/// recomputed checksum (over the real new body, both in its own section header and its own
/// manifest entry), the end trailer's own `manifest_offset`, and the manifest's own checksum.
/// `pub(in crate::archive)`: external review 024's own "strip the carried key section" shape is
/// built this way, on a real export, never by hand-assembling a whole archive from scratch.
pub(in crate::archive) fn replace_section_body_for_test(
    bytes: &mut Vec<u8>,
    section_kind: u16,
    new_body: &[u8],
) {
    let (_, _, entries) = decode_manifest_for_test(bytes);
    let (_, target_offset, target_length, _) = *entries
        .iter()
        .find(|(kind, ..)| *kind == section_kind)
        .expect("section present");
    const SECTION_HEADER_LEN: u64 = 8 + 2 + 8;
    let body_start = (target_offset + SECTION_HEADER_LEN) as usize;
    let old_body_end = body_start + target_length as usize;
    // The 32-byte trailing checksum right after the old body moves with the splice automatically;
    // only the body itself is replaced.
    bytes.splice(body_start..old_body_end, new_body.iter().copied());
    let shift = new_body.len() as i64 - target_length as i64;

    let end_trailer_start = bytes.len() - 56;
    let manifest_offset = u64::from_be_bytes(
        bytes[end_trailer_start + 8..end_trailer_start + 16]
            .try_into()
            .unwrap(),
    );
    let new_manifest_offset = (manifest_offset as i64 + shift) as u64;
    bytes[end_trailer_start + 8..end_trailer_start + 16]
        .copy_from_slice(&new_manifest_offset.to_be_bytes());

    let manifest = &bytes[new_manifest_offset as usize..];
    let tool_version_len = u16::from_be_bytes(manifest[12..14].try_into().unwrap()) as usize;
    let count_at = 14 + tool_version_len;
    let count = u64::from_be_bytes(manifest[count_at..count_at + 8].try_into().unwrap()) as usize;
    let first_entry = new_manifest_offset as usize + count_at + 8;
    let new_length = new_body.len() as u64;
    let new_checksum = {
        let header = &bytes[target_offset as usize..target_offset as usize + 10];
        let length_field = new_length.to_be_bytes();
        let new_body_start = target_offset as usize + SECTION_HEADER_LEN as usize;
        let new_body_end = new_body_start + new_body.len();
        prikk_hash::sha256_parts(&[header, &length_field, &bytes[new_body_start..new_body_end]])
    };
    for index in 0..count {
        let entry_at = first_entry + index * 50;
        let entry_offset =
            u64::from_be_bytes(bytes[entry_at + 2..entry_at + 10].try_into().unwrap());
        if entry_offset == target_offset {
            bytes[entry_at + 10..entry_at + 18].copy_from_slice(&new_length.to_be_bytes());
            bytes[entry_at + 18..entry_at + 50].copy_from_slice(&new_checksum);
            let section_header_start = target_offset as usize;
            bytes[section_header_start + 10..section_header_start + 18]
                .copy_from_slice(&new_length.to_be_bytes());
            let new_body_start = section_header_start + SECTION_HEADER_LEN as usize;
            let new_body_end = new_body_start + new_body.len();
            bytes[new_body_end..new_body_end + 32].copy_from_slice(&new_checksum);
        } else if entry_offset > target_offset {
            let shifted = (entry_offset as i64 + shift) as u64;
            bytes[entry_at + 2..entry_at + 10].copy_from_slice(&shifted.to_be_bytes());
        }
    }
    recompute_manifest_checksum_for_test(bytes);
}

#[test]
fn export_archive_round_trips_a_small_history() {
    let dir = unique_temp_dir("archive-export-basic");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");

    let mut buffer = Vec::new();
    let report = export_archive(&layout, &mut buffer).expect("export");
    assert_eq!(report.repository_format, 7);
    assert_eq!(
        report
            .object_counts
            .iter()
            .find(|(object_type, _)| *object_type == ObjectType::Block)
            .map(|(_, count)| *count),
        Some(1),
        "the one sealed block is counted"
    );
    assert_eq!(
        report
            .object_counts
            .iter()
            .find(|(object_type, _)| *object_type == ObjectType::Patch)
            .map(|(_, count)| *count),
        Some(1),
        "the one sealed patch is counted"
    );
    assert_eq!(
        report.local_branches, 1,
        "heads/main is counted as a local branch"
    );
    assert_eq!(report.tags, 0);
    assert_eq!(report.received_refs, 0);
    assert_eq!(report.total_bytes as usize, buffer.len());
    assert_eq!(&buffer[0..8], b"PREPO001");

    let (_, _, entries) = decode_manifest_for_test(&buffer);
    assert_eq!(entries.len(), 13);
    let block_entry = entries
        .iter()
        .find(|(kind, ..)| {
            *kind == ArchiveSectionKind::ObjectContainer(ObjectType::Block).wire_code()
        })
        .expect("block section present");
    let body = read_section_body_for_test(&buffer, block_entry.1, block_entry.2, block_entry.3);
    assert!(
        !body.is_empty(),
        "the sealed block's own container section is non-empty"
    );

    let pointer_entry = entries
        .iter()
        .find(|(kind, ..)| *kind == ArchiveSectionKind::RefPointerIndex.wire_code())
        .expect("pointer-index section present");
    let pointer_body =
        read_section_body_for_test(&buffer, pointer_entry.1, pointer_entry.2, pointer_entry.3);
    assert!(
        !pointer_body.is_empty(),
        "heads/main's own pointer entry is carried"
    );
}

#[test]
fn export_refuses_queued_unsealed_work() {
    let dir = unique_temp_dir("archive-export-queued");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let _active_lock =
        ActiveLock::acquire_for_write(&layout, DEFAULT_ACTIVE_NAME).expect("active lock");
    let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
    wal.append_patch(&signed_patch_envelope())
        .expect("queue a patch");
    drop(_active_lock);

    let mut buffer = Vec::new();
    let err = export_archive(&layout, &mut buffer).expect_err("must refuse");
    let message = err.to_string();
    assert!(message.contains("queued, unsealed commits"), "{message}");
    assert!(message.contains("`prikk seal`"), "{message}");
    assert!(buffer.is_empty(), "a refused export writes nothing");

    // Follow the message: seal, then export must succeed.
    seal_queued_patch(&layout).expect("sealing the queued patch");
    let mut buffer2 = Vec::new();
    export_archive(&layout, &mut buffer2).expect("export succeeds once sealed");
}

/// 0.52.0 step 1 Part A, item 1 (external review 023): the scanning logic `export_archive` calls
/// before writing anything, exercised directly against a small, test-chosen bound -- building a
/// real over-256-MiB object just to prove the comparison works would be pure cost for no more
/// signal. Shown red on the code before this part (there was no such function at all, so every
/// record passed, however large its own declared body) and green after.
#[test]
fn first_over_bound_record_finds_a_record_whose_declared_body_exceeds_the_bound() {
    let dir = unique_temp_dir("archive-export-over-bound-record");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    object_store
        .write_object(&signed_patch_envelope())
        .expect("write a real patch record");
    let path = layout.container_slot_path(ObjectType::Patch, ContainerSlot::A);
    let sound_len = std::fs::metadata(&path).expect("stat").len();
    assert_eq!(
        super::first_over_bound_record(&path, ObjectType::Patch, sound_len, u64::MAX)
            .expect("scan"),
        None,
        "an enormous bound is never exceeded"
    );
    assert_eq!(
        super::first_over_bound_record(&path, ObjectType::Patch, sound_len, 0).expect("scan"),
        Some(0),
        "a near-zero bound is exceeded by any real record"
    );
}

/// 0.52.0 step 1 Part A, item 2 (C1 review v3, N2); round 2 (A-1): the same `PointerLeading` crash
/// shape `refs::tests::publication_recovery::state_matrix`'s own fixture names (interrupting a root
/// publish's 4th `AppendWrite` -- after the object-container, object-index and pointer-index
/// appends -- lands the pointer committed and the ref log not yet appended), built with
/// `test_gates::test_support`'s own fixed, non-cryptographic placeholder signature (`"maintainer-
/// key"`, never trusted via `add_trusted_maintainer` here) rather than a real signer. **This is the
/// genuine divergence case, found empirically in round 2**: with no trusted maintainer and no
/// matching active-WAL evidence of any kind, `require_retained_evidence`'s own `trust_is_valid` gate
/// is false, so `mark_unproved` renames the issue to `PRIKK-VERIFY-REF-DIVERGENCE` -- the refusal
/// must **not** send the user to `prikk ref complete`, since that command would simply refuse here
/// too. Shown red on the code before this part (there was no such refusal at all, so this source
/// exported successfully) and green after.
#[test]
fn export_refuses_a_source_with_an_unprovable_ref_divergence() {
    let dir = unique_temp_dir("archive-export-ref-divergence");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let mut object_store = FileObjectStore::new(layout.clone());
    let block_id = object_store
        .write_object(&signed_block(BlockKind::Root, Vec::new(), Vec::new(), None))
        .expect("write a real target block");
    let ref_state = signed_ref_state_envelope("heads/main", None, block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, block_id, 1);
    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    };

    fail_after_for_test(TestFailPoint::AppendWrite, 3);
    assert!(
        RefStore::new(layout.clone()).publish(&publication).is_err(),
        "the failpoint must interrupt this publish"
    );

    let mut buffer = Vec::new();
    let err = export_archive(&layout, &mut buffer).expect_err("must refuse");
    let message = err.to_string();
    assert!(
        !message.contains("ref complete"),
        "a non-completable divergence must not name `ref complete`: {message}"
    );
    assert!(message.contains("PRIKK-VERIFY-REF-DIVERGENCE"), "{message}");
    assert!(buffer.is_empty(), "a refused export writes nothing");
}

/// 0.52.0 step 1 Part A round 2 (A-1): the companion to the divergence test above -- a genuinely
/// **completable** `POINTER-LEADS-LOG` lead, proved by a real `Ed25519MaintainerSigner` trusted via
/// `add_trusted_maintainer` rather than `test_support`'s fixed placeholder signature. **Built as a
/// non-WAL-consuming lead deliberately, not a `seal`'s own WAL-backed one**: a real `seal`-consumed
/// lead's own retained evidence is `ActiveWalMetadataStatus::ValidForNonEmptyWal` -- the active WAL
/// *still holding* the queued records that built the Block -- which is exactly the same condition
/// `refuse_on_queued_unsealed_work` (run earlier in `export_archive`, unconditionally) already
/// refuses on; a `seal`-shaped completable lead can therefore never reach this part's own new
/// refusal through `archive export` at all, since the older check wins first. RFC 165 R4's own
/// design note (`ref_publication.rs`'s `require_retained_evidence`) says precisely this: conditions
/// (a)/(c)/(e) alone complete a lead for a **non-WAL-consuming** publication (`branch create`,
/// `tag create`, `merge`, `sync adopt-tag`); condition (d), the WAL-evidence match, applies "only
/// when the active WAL's retained metadata claims this exact ref" -- never touched here, so it does
/// not apply, and the lead is still genuinely completable from the pointer/log/signature state
/// alone. The refusal must name `prikk ref complete <ref>`, and running that command for real must
/// then let export through.
#[test]
fn export_refuses_a_completable_pointer_lead_naming_ref_complete_and_then_succeeds() {
    let dir = unique_temp_dir("archive-export-pointer-lead-completable");
    let layout = RepositoryLayout::init(&dir).expect("init");
    let signer = Ed25519MaintainerSigner::from_seed("round-2-maintainer", &[0x72; 32])
        .expect("build signer");
    let public_key_hex = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    add_trusted_maintainer(&layout, signer.key_id(), &public_key_hex)
        .expect("trust the maintainer");

    let mut object_store = FileObjectStore::new(layout.clone());
    object_store
        .write_object(&crate::test_gates::test_support::signed_patch_blob_envelope())
        .expect("write the patch's own referenced blob");
    let patch = signed_patch_envelope();
    let patch_id = object_store
        .write_object(&patch)
        .expect("write the patch object");
    let block_payload = prikk_object::BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: vec![patch_id],
        state_merkle_root: derive_next_state_root(&object_store, None, &[patch_id])
            .expect("derive state root"),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block_bytes =
        prikk_object::CanonicalEncode::to_canonical_bytes(&block_payload).expect("encode block");
    let mut block_envelope =
        prikk_object::ObjectEnvelope::unsigned(ObjectType::Block, 2, block_bytes);
    block_envelope
        .add_signature(
            maintainer_signature(&signer, ObjectType::Block, block_envelope.object_id())
                .expect("sign block"),
        )
        .expect("attach block signature");
    let block_id = object_store
        .write_object(&block_envelope)
        .expect("write the block");

    let state_payload = prikk_object::RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: prikk_object::RefKind::Branch,
        target_object_id: block_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let state_bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&state_payload)
        .expect("encode ref state");
    let mut ref_state =
        prikk_object::ObjectEnvelope::unsigned(ObjectType::RefState, 1, state_bytes);
    let ref_state_id_for_sig = ref_state.object_id();
    ref_state
        .add_signature(
            maintainer_signature(&signer, ObjectType::RefState, ref_state_id_for_sig)
                .expect("sign ref state"),
        )
        .expect("attach ref state signature");
    let ref_state_id = ref_state.object_id();

    let update_payload = prikk_object::RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: block_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let update_bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&update_payload)
        .expect("encode ref update");
    let mut ref_update =
        prikk_object::ObjectEnvelope::unsigned(ObjectType::RefUpdate, 1, update_bytes);
    let ref_update_id_for_sig = ref_update.object_id();
    ref_update
        .add_signature(
            maintainer_signature(&signer, ObjectType::RefUpdate, ref_update_id_for_sig)
                .expect("sign ref update"),
        )
        .expect("attach ref update signature");

    let publication = RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    };

    fail_after_for_test(TestFailPoint::AppendWrite, 3);
    assert!(
        RefStore::new(layout.clone()).publish(&publication).is_err(),
        "the failpoint must interrupt this publish"
    );

    let mut buffer = Vec::new();
    let err = export_archive(&layout, &mut buffer).expect_err("must refuse");
    let message = err.to_string();
    assert!(
        message.contains("`prikk ref complete heads/main`"),
        "a completable lead must name `ref complete`: {message}"
    );
    assert!(message.contains("POINTER-LEADS-LOG"), "{message}");
    assert!(buffer.is_empty(), "a refused export writes nothing");

    let plan = crate::ref_completion::plan_ref_completion(&layout, "heads/main")
        .expect("plan_ref_completion must not hard-fail")
        .expect("the lead must actually be completable");
    let active_lock =
        ActiveLock::acquire_for_write(&layout, DEFAULT_ACTIVE_NAME).expect("acquire active lock");
    let mut completion_store = FileObjectStore::new(layout.clone());
    crate::ref_completion::complete_ref_publication(
        &layout,
        &mut completion_store,
        &active_lock,
        &plan,
        &signer,
    )
    .expect("ref complete must succeed");
    drop(active_lock);

    let mut buffer_after = Vec::new();
    export_archive(&layout, &mut buffer_after)
        .expect("export must succeed once the lead is completed");
    assert!(
        !buffer_after.is_empty(),
        "a successful export must write something"
    );
}

/// Drain the active WAL exactly as `prikk seal` would -- the minimum needed to make the queued-work
/// refusal test's own "then export succeeding" half real, without pulling in the CLI crate.
fn seal_queued_patch(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let active_lock = ActiveLock::acquire_for_write(layout, DEFAULT_ACTIVE_NAME)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    let replay = wal.replay()?;
    let mut object_store = FileObjectStore::new(layout.clone());
    let mut patch_ids = Vec::new();
    for record in &replay.records {
        patch_ids.push(object_store.write_object(&record.envelope)?);
    }
    let root_block = signed_block(BlockKind::Root, Vec::new(), patch_ids, None);
    let root_block_id = object_store.write_object(&root_block)?;
    let ref_store = RefStore::new(layout.clone());
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
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;
    crate::finish_active_publication_cleanup(layout, &active_lock)?;
    Ok(())
}

/// D3's own closed gap: export from a repository whose pointer-index live slot is `b`, through
/// ordinary compaction -- never `--keep-slot` (that path is for the lost-log recovery case, not a
/// healthy repository), matching the real command sequence `compact --pointer-index` runs.
#[test]
fn export_carries_resolved_content_when_live_slot_is_b() {
    let dir = unique_temp_dir("archive-export-slot-b");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal 1");
    advance_ref_once_more(&layout).expect("seal 2, so compaction has something to reclaim");

    let before = crate::compact::compact_ref_pointer_index(&layout)
        .expect("ordinary pointer-index compaction");
    assert!(
        before.entries_after < before.entries_before,
        "compaction must have reclaimed at least one stale record"
    );

    let slot_b_path = layout.ref_pointer_index_slot_path(ContainerSlot::B);
    assert!(
        std::fs::metadata(&slot_b_path)
            .map(|m| m.len())
            .unwrap_or(0)
            > 0,
        "slot b must now hold the live content"
    );

    let mut buffer = Vec::new();
    export_archive(&layout, &mut buffer).expect("export after compaction");
    let (_, _, entries) = decode_manifest_for_test(&buffer);
    let pointer_entry = entries
        .iter()
        .find(|(kind, ..)| *kind == ArchiveSectionKind::RefPointerIndex.wire_code())
        .expect("pointer-index section present");
    let body =
        read_section_body_for_test(&buffer, pointer_entry.1, pointer_entry.2, pointer_entry.3);

    // The ground truth: `RefStore` resolves the live content the same way `branch list` would.
    let ref_store = RefStore::new(layout.clone());
    let live_tip = ref_store
        .read_current_ref_state_id("heads/main")
        .expect("read current ref state")
        .expect("heads/main exists");

    // The exported body must decode to exactly one live entry naming the same current tip --
    // never slot a's own stale, reclaimed-away bytes (slot a is longer than the live content: it
    // still holds every record compaction just reclaimed).
    let decoded =
        crate::refs::decode_pointer_index_records(&body).expect("decode exported pointer index");
    assert_eq!(
        decoded.entries.len(),
        1,
        "compaction reduced heads/main to one live record"
    );
    assert_eq!(
        decoded.entries[0].ref_state_id, live_tip,
        "the exported, resolved content must name the same tip RefStore itself reports live"
    );

    let slot_a_len = std::fs::metadata(layout.ref_pointer_index_slot_path(ContainerSlot::A))
        .expect("slot a exists")
        .len();
    assert_ne!(
        body.len() as u64,
        slot_a_len,
        "the exported body must not simply be slot a's own (longer, stale) bytes"
    );
}

fn advance_ref_once_more(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let ref_store = RefStore::new(layout.clone());
    let current = ref_store
        .read_current_ref_state_id("heads/main")?
        .expect("heads/main exists");
    let mut object_store = FileObjectStore::new(layout.clone());
    let patch = signed_patch_envelope();
    let patch_id = object_store.write_object(&patch)?;
    let block = signed_block(BlockKind::Normal, vec![current], vec![patch_id], None);
    let block_id = object_store.write_object(&block)?;
    let ref_state = crate::test_gates::test_support::signed_ref_state_envelope(
        "heads/main",
        Some(current),
        block_id,
        2,
    );
    let ref_state_id = ref_state.object_id();
    let ref_update = crate::test_gates::test_support::signed_ref_update_envelope(
        "heads/main",
        Some(current),
        ref_state_id,
        block_id,
        2,
    );
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: Some(current),
        ref_state,
        ref_update,
    })?;
    Ok(())
}

/// §8's control: export holds every `LockableContainer` lock for its **whole run** (not merely
/// while acquiring them), so a concurrent `seal`-shaped writer (here, a direct object-store append
/// plus a ref publish, the same primitives `seal` itself uses) must fail fast rather than
/// interleave. Calls `export_archive` itself, via the test seam
/// [`crate::archive::after_export_locks_acquired_for_test`], so this proves `export_archive`'s own
/// behaviour -- not merely that `acquire_container_locks` works, which `lock.rs`'s own tests already
/// cover. **This is the real control**: perturbed (the seam moved past the locks, simulating an
/// early release) in the report's own control run, shown red, then reverted here.
#[test]
fn export_blocks_a_concurrent_seal_for_its_whole_run() {
    let dir = unique_temp_dir("archive-export-concurrent-seal");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");

    let probe_layout = layout.clone();
    crate::archive::after_export_locks_acquired_for_test(move || {
        let err = advance_ref_once_more(&probe_layout).expect_err(
            "a concurrent seal-shaped write must fail fast while export holds its locks",
        );
        assert!(
            matches!(err, prikk_error::PrikkError::LockConflict(_)),
            "expected a lock conflict, got: {err}"
        );
    });

    let mut buffer = Vec::new();
    export_archive(&layout, &mut buffer).expect("export itself still succeeds");
}

#[test]
fn verify_section_bodies_stream_without_loading_file_into_a_cursor() {
    // A cheap sanity check that `export_archive` accepts any `Write`, not only `Vec<u8>` --
    // `std::io::Cursor` over a fixed buffer exercises the same `dyn Write` path a real file would.
    let dir = unique_temp_dir("archive-export-generic-writer");
    let layout = RepositoryLayout::init(&dir).expect("init");
    seal_one_block(&layout).expect("seal");
    let mut cursor = Cursor::new(Vec::new());
    export_archive(&layout, &mut cursor).expect("export into a Cursor<Vec<u8>>");
    assert!(!cursor.into_inner().is_empty());
}
