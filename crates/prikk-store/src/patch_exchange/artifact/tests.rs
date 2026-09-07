//! RFC 115 Stage 3 §3: `PEXCH002` structural encode/decode tests. Accept-path-level behaviour
//! (digest mismatch, closure, signatures, Phase D) is covered in `accept::tests`, not here -- this
//! module is about the artifact's own shape, matching how `bundle/tests.rs` keeps `decode_bundle`'s
//! shape tests separate from `import_bundle`'s behaviour tests.

#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::Result;
use prikk_object::{BlockKind, ObjectId, PatchSetDigest};

use super::{decode_exchange_artifact, export_exchange_artifact};
use crate::patch_exchange::exchange_test_support::{
    author_signer, maintainer_signer, signed_author_patch_envelope, signed_blob_envelope,
    signed_tag_envelope,
};
use crate::test_gates::test_support::{signed_block, unique_temp_dir};
use crate::{FileObjectStore, ObjectWriter, RepositoryLayout};

/// Build a repository holding one AUTHOR-signed patch (and its blob), returning the layout and the
/// patch's object id.
fn repo_with_one_patch(root_name: &str) -> Result<(RepositoryLayout, prikk_object::ObjectId)> {
    let root = unique_temp_dir(root_name);
    let layout = RepositoryLayout::init(root)?;
    let mut objects = FileObjectStore::new(layout.clone());
    let blob = signed_blob_envelope(b"artifact fixture\n")?;
    let blob_id = objects.write_object(&blob)?;
    let signer = author_signer(0x21)?;
    let patch = signed_author_patch_envelope(&signer, "artifact.txt", 0x22, blob_id)?;
    let patch_id = objects.write_object(&patch)?;
    Ok((layout, patch_id))
}

#[test]
fn export_then_decode_round_trips_every_section() -> Result<()> {
    let (layout, patch_id) = repo_with_one_patch("pexch-artifact-roundtrip")?;
    let (report, bytes) = export_exchange_artifact(&layout, &[patch_id], &[], &[])?;
    assert_eq!(report.patch_count, 1);
    assert_eq!(report.blob_count, 1);
    assert_eq!(report.claim_count, 0);
    assert_eq!(report.tag_count, 0);

    let decoded = decode_exchange_artifact(&bytes, 1_000)?;
    assert_eq!(decoded.patches.len(), 1);
    assert_eq!(decoded.patches[0].object_id(), patch_id);
    assert_eq!(decoded.blobs.len(), 1);
    assert_eq!(decoded.claims.len(), 0);
    assert_eq!(decoded.tags.len(), 0);

    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}

/// RFC 117 stage 3 §2: the new sixth section round-trips a real Tag object, independent of every
/// other section (zero patches, zero claims here -- tag-travel does not require either).
#[test]
fn export_then_decode_round_trips_the_tag_section() -> Result<()> {
    let root = unique_temp_dir("pexch-artifact-tag-roundtrip");
    let layout = RepositoryLayout::init(root)?;
    let mut store = FileObjectStore::new(layout.clone());
    let block = store.write_object(&signed_block(
        BlockKind::Root,
        Vec::new(),
        vec![ObjectId::from_bytes([0x31; 32])],
        None,
    ))?;
    let tag_signer = maintainer_signer(0x32)?;
    let tag = signed_tag_envelope(
        &tag_signer,
        "tags/artifact-roundtrip",
        block,
        PatchSetDigest([0x33; 32]),
        1,
    )?;
    let tag_id = store.write_object(&tag)?;

    let (report, bytes) = export_exchange_artifact(&layout, &[], &[], &[tag_id])?;
    assert_eq!(report.tag_count, 1);

    let decoded = decode_exchange_artifact(&bytes, 1_000)?;
    assert_eq!(decoded.tags.len(), 1);
    assert_eq!(decoded.tags[0].object_id(), tag_id);

    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}

#[test]
fn decode_rejects_wrong_magic() {
    let mut bytes = vec![0_u8; 40];
    bytes[..8].copy_from_slice(b"NOTPEXCH");
    assert!(decode_exchange_artifact(&bytes, 1_000).is_err());
}

/// RFC 117 stage 3 §2/§7 row 7: the retired `PEXCH001` magic is refused outright by the new reader
/// -- there is no dual-reader, and the refusal names the format, not a generic parse failure.
#[test]
fn decode_rejects_the_retired_pexch001_magic() {
    let mut bytes = vec![0_u8; 40];
    bytes[..8].copy_from_slice(b"PEXCH001");
    let error = decode_exchange_artifact(&bytes, 1_000).unwrap_err();
    assert!(
        error.to_string().contains("magic"),
        "expected a magic-naming refusal, got: {error}"
    );
}

#[test]
fn decode_rejects_a_declared_patch_count_over_the_configured_limit() -> Result<()> {
    let (layout, patch_id) = repo_with_one_patch("pexch-artifact-count-limit")?;
    let (_, bytes) = export_exchange_artifact(&layout, &[patch_id], &[], &[])?;
    // The artifact declares one patch; a limit of 0 must refuse on the declared count alone,
    // before any patch is decoded.
    let error = decode_exchange_artifact(&bytes, 0).unwrap_err();
    let message = error.to_string();
    assert!(
        message.contains("patches") && message.contains('0'),
        "expected a declared-count-over-limit refusal naming the section and the limit, got: \
         {message}"
    );
    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}

#[test]
fn decode_rejects_trailing_bytes() -> Result<()> {
    let (layout, patch_id) = repo_with_one_patch("pexch-artifact-trailing")?;
    let (_, mut bytes) = export_exchange_artifact(&layout, &[patch_id], &[], &[])?;
    bytes.push(0xAB);
    assert!(decode_exchange_artifact(&bytes, 1_000).is_err());
    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}

#[test]
fn export_refuses_a_duplicate_patch_id() -> Result<()> {
    let (layout, patch_id) = repo_with_one_patch("pexch-artifact-dup")?;
    let error = export_exchange_artifact(&layout, &[patch_id, patch_id], &[], &[]).unwrap_err();
    assert!(error.to_string().contains("more than once"));
    let _ = std::fs::remove_dir_all(layout.root());
    Ok(())
}
