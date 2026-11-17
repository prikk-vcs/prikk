//! 0.58.0 P3b: the streamed accept's own properties -- one decoded object at a time, the second pass refusing a frame that is not the
//! one the first pass validated, and the order refusals keep for an artifact with more than one fault.
//!
//! **Red controls** (each shown red when named): *keep the decoded objects* -- hold every `LiveDecoded` to the end of pass 1 and
//! `an_accept_holds_one_decoded_object_at_a_time` fails; *skip the digest comparison in pass 2* -- `a_frame_changed_between_the_passes_…`
//! fails; *check signatures before the closure* -- `a_missing_blob_is_refused_before_a_bad_signature` fails.

#![allow(clippy::indexing_slicing, clippy::unwrap_used)]

use std::io::Write as _;

use prikk_error::Result;
use prikk_object::{ObjectId, ObjectType};

use super::{
    AcceptOptions, accept_exchange_artifact, accept_exchange_artifact_streaming,
    between_accept_passes_for_test, peak_decoded_bytes_for_test,
};
use crate::author::author_key_index::record_author_key_material;
use crate::author::author_signing::AuthorSigner as _;
use crate::bundle::BundleSource;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::lock::ActiveLock;
use crate::patch_exchange::exchange_test_support::{
    author_signer, reencode_artifact, signed_author_patch_envelope, signed_blob_envelope,
};
use crate::patch_exchange::export_exchange_artifact;
use crate::test_gates::test_support::{repository_bytes, unique_temp_dir};
use crate::{FileObjectStore, ObjectReadSnapshot, ObjectWriter, RepositoryLayout};

const BLOB_BYTES: usize = 40_000;

fn fresh_repo(name: &str) -> Result<RepositoryLayout> {
    RepositoryLayout::init(unique_temp_dir(name))
}

/// An artifact of `count` patches, each with its own `BLOB_BYTES`-byte blob, the signer's key material carried. Returns the bytes and the
/// patch and blob ids in artifact order.
fn artifact_of(
    name: &str,
    count: u8,
    corrupt_signatures: bool,
) -> Result<(Vec<u8>, Vec<ObjectId>, Vec<ObjectId>)> {
    let signer = author_signer(0x60)?;
    let sender = fresh_repo(name)?;
    let mut objects = FileObjectStore::new(sender.clone());
    let (mut patch_ids, mut blob_ids) = (Vec::new(), Vec::new());
    for index in 0..count {
        let blob = signed_blob_envelope(&vec![index.wrapping_add(1); BLOB_BYTES])?;
        let blob_id = objects.write_object(&blob)?;
        let mut patch = signed_author_patch_envelope(
            &signer,
            &format!("stream-{index}.txt"),
            0x61 + index,
            blob_id,
        )?;
        if corrupt_signatures {
            patch.signatures[0].signature_bytes[0] ^= 0x01;
        }
        patch_ids.push(objects.write_object(&patch)?);
        blob_ids.push(blob_id);
    }
    let lock = ActiveLock::acquire(&sender, DEFAULT_ACTIVE_NAME)?;
    record_author_key_material(&sender, signer.key_id(), signer.public_key_bytes(), &lock)?;
    drop(lock);
    let (_, bytes) = export_exchange_artifact(&sender, &patch_ids, &[], &[], None)?;
    let _ = std::fs::remove_dir_all(sender.root());
    Ok((bytes, patch_ids, blob_ids))
}

/// The artifact's four objects' payloads are never alive together: the high-water mark of decoded payload bytes over a whole accept is one
/// object's, not the artifact's. **Red control:** keep every decoded object's `LiveDecoded` to the end of pass 1 and this goes red.
#[test]
fn an_accept_holds_one_decoded_object_at_a_time() -> Result<()> {
    let (bytes, _, _) = artifact_of("pexch-stream-live-sender", 4, false)?;
    let receiver = fresh_repo("pexch-stream-live-receiver")?;
    let (report, peak) = peak_decoded_bytes_for_test(|| {
        accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())
    });
    let report = report?;
    assert_eq!(report.blob_count, 4);
    assert_eq!(report.written_object_count, 8);
    // One blob payload plus a little framing; two would be 80,000, all four 160,000.
    assert!(
        (BLOB_BYTES..2 * BLOB_BYTES).contains(&peak),
        "peak decoded payload bytes {peak}: one object, not the artifact"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// Write `bytes` to a file and open it as the seekable source a CLI accept holds.
fn open_as_file(bytes: &[u8]) -> Result<(std::path::PathBuf, BundleSource)> {
    let path = std::env::temp_dir().join(format!(
        "pexch-stream-{}-{}.bin",
        std::process::id(),
        unique_temp_dir("pexch-stream-file")
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("x")
    ));
    std::fs::write(&path, bytes).map_err(prikk_error::PrikkError::from)?;
    let file = std::fs::File::open(&path).map_err(prikk_error::PrikkError::from)?;
    Ok((path, BundleSource::File(file)))
}

/// Row 12. A frame that is not the one pass 1 validated is refused before it is written. Here the **first** object written (a patch) is
/// changed under the open file: nothing is written. **Red control:** drop pass 2's digest comparison and the changed frame is written (or
/// fails later, elsewhere) and this goes red.
#[test]
fn a_frame_changed_between_the_passes_is_refused_before_it_is_written() -> Result<()> {
    let (bytes, patch_ids, _) = artifact_of("pexch-stream-row12-sender", 2, false)?;
    let receiver = fresh_repo("pexch-stream-row12-receiver")?;
    let before = repository_bytes(&receiver)?;
    let (path, mut source) = open_as_file(&bytes)?;

    // The first patch's frame starts after magic (8), digest (32) and the patch count (8): flip a byte in its last 8 bytes.
    let first_frame_len = u64::from_be_bytes(bytes[48..56].try_into().unwrap()) as usize;
    let target = 56 + first_frame_len - 1;
    let changed = path.clone();
    between_accept_passes_for_test(move || {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&changed)
            .unwrap();
        let mut one = [0_u8; 1];
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(target as u64)).unwrap();
        std::io::Read::read_exact(&mut file, &mut one).unwrap();
        one[0] ^= 0xFF;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(target as u64)).unwrap();
        file.write_all(&one).unwrap();
    });
    let error = accept_exchange_artifact_streaming(
        &receiver,
        &mut source,
        &AcceptOptions::default_limits(),
    )
    .unwrap_err();
    let said = error.to_string();
    assert!(
        said.contains("changed between validation and writing") && said.contains("0 of 4"),
        "{said}"
    );
    assert_eq!(
        repository_bytes(&receiver)?,
        before,
        "nothing was written, not even a lock file is left"
    );

    // The same file, restored, completes it: nothing was half-done.
    std::fs::write(&path, &bytes).map_err(prikk_error::PrikkError::from)?;
    let file = std::fs::File::open(&path).map_err(prikk_error::PrikkError::from)?;
    let report = accept_exchange_artifact_streaming(
        &receiver,
        &mut BundleSource::File(file),
        &AcceptOptions::default_limits(),
    )?;
    assert_eq!(report.written_object_count, 4);
    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(snapshot.contains_object(ObjectType::Patch, patch_ids[0]));
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// The case table's one new state: a **later** object changed under the open file. The objects before it stay (content-addressed; nothing
/// points at them as accepted), no key material, claim or tag is written, the refusal says how many were written, and the same accept on
/// the right file finishes the rest -- a kill's state, not a new one.
#[test]
fn a_later_frame_changed_between_the_passes_leaves_a_kills_state_and_a_rerun_finishes() -> Result<()>
{
    let (bytes, patch_ids, blob_ids) = artifact_of("pexch-stream-late-sender", 2, false)?;
    let receiver = fresh_repo("pexch-stream-late-receiver")?;
    let (path, mut source) = open_as_file(&bytes)?;

    // The last blob's frame ends the blob section; the author-key section follows. Flip the last byte of that blob's frame: find it as
    // the frame that begins after the patches and the first blob.
    let mut at = 48_usize;
    for _ in 0..2 {
        at += 8 + u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
    }
    at += 8; // the blob count
    at += 8 + u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
    let second_blob_len = u64::from_be_bytes(bytes[at..at + 8].try_into().unwrap()) as usize;
    let target = at + 8 + second_blob_len - 1;
    let changed = path.clone();
    between_accept_passes_for_test(move || {
        let mut bytes = std::fs::read(&changed).unwrap();
        bytes[target] ^= 0xFF;
        std::fs::write(&changed, bytes).unwrap();
    });
    let error = accept_exchange_artifact_streaming(
        &receiver,
        &mut source,
        &AcceptOptions::default_limits(),
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("changed between validation and writing"),
        "{error}"
    );
    let snapshot = ObjectReadSnapshot::open(&receiver)?;
    assert!(
        snapshot.contains_object(ObjectType::Patch, patch_ids[0])
            && snapshot.contains_object(ObjectType::Patch, patch_ids[1])
            && snapshot.contains_object(ObjectType::Blob, blob_ids[0]),
        "the objects before the changed frame were written"
    );
    assert!(!snapshot.contains_object(ObjectType::Blob, blob_ids[1]));
    assert!(
        crate::author::author_key_index::lookup_author_key_entries(
            &receiver,
            author_signer(0x60)?.key_id()
        )?
        .is_empty(),
        "no author key material was recorded"
    );

    std::fs::write(&path, &bytes).map_err(prikk_error::PrikkError::from)?;
    let file = std::fs::File::open(&path).map_err(prikk_error::PrikkError::from)?;
    let report = accept_exchange_artifact_streaming(
        &receiver,
        &mut BundleSource::File(file),
        &AcceptOptions::default_limits(),
    )?;
    assert_eq!(report.written_object_count, 1, "only the missing blob");
    assert_eq!(report.recorded_author_key_count, 1);
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// A file cut short between the passes cannot be re-read: the refusal says so, before the object is written.
#[test]
fn a_file_truncated_between_the_passes_cannot_be_reread() -> Result<()> {
    let (bytes, _, _) = artifact_of("pexch-stream-cut-sender", 2, false)?;
    let receiver = fresh_repo("pexch-stream-cut-receiver")?;
    let (path, mut source) = open_as_file(&bytes)?;
    let cut = path.clone();
    between_accept_passes_for_test(move || {
        std::fs::OpenOptions::new()
            .write(true)
            .open(&cut)
            .unwrap()
            .set_len(60)
            .unwrap();
    });
    let error = accept_exchange_artifact_streaming(
        &receiver,
        &mut source,
        &AcceptOptions::default_limits(),
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("could not be re-read"),
        "{error}"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}

/// The order refusals keep for an artifact with several faults (P3b ruling 2): closure completeness before the signature check. An
/// artifact whose patch names a missing blob **and** carries a corrupt signature says the blob is missing, as the whole-artifact accept
/// did. **Red control:** check signatures first and this goes red.
#[test]
fn a_missing_blob_is_refused_before_a_bad_signature() -> Result<()> {
    let (bytes, _, _) = artifact_of("pexch-stream-order-sender", 1, true)?;
    let mutated = reencode_artifact(&bytes, None, Some(Vec::new()), None, None)?;
    let receiver = fresh_repo("pexch-stream-order-receiver")?;
    let error = accept_exchange_artifact(&receiver, &mutated, &AcceptOptions::default_limits())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("neither carried by this artifact"),
        "the closure refusal comes first: {error}"
    );
    // And with the blob present the signature is what refuses.
    let receiver_two = fresh_repo("pexch-stream-order-receiver-two")?;
    let error = accept_exchange_artifact(&receiver_two, &bytes, &AcceptOptions::default_limits())
        .unwrap_err();
    assert!(
        !error
            .to_string()
            .contains("neither carried by this artifact"),
        "{error}"
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    let _ = std::fs::remove_dir_all(receiver_two.root());
    Ok(())
}

/// `decode_exchange_artifact` (the whole decode into owned envelopes) is fixture code: an accept, whatever it carries, never calls it.
/// **Red control:** have `accept` decode the artifact whole and this goes red.
#[test]
fn an_accept_never_decodes_the_artifact_whole() -> Result<()> {
    let (bytes, _, _) = artifact_of("pexch-stream-whole-sender", 2, false)?;
    let receiver = fresh_repo("pexch-stream-whole-receiver")?;
    let before = crate::patch_exchange::artifact::whole_decodes_for_test();
    accept_exchange_artifact(&receiver, &bytes, &AcceptOptions::default_limits())?;
    assert_eq!(
        crate::patch_exchange::artifact::whole_decodes_for_test(),
        before,
        "accept took the artifact apart whole"
    );
    // The control is live: the fixture helper does count.
    reencode_artifact(&bytes, None, None, None, None)?;
    assert_eq!(
        crate::patch_exchange::artifact::whole_decodes_for_test(),
        before + 1
    );
    let _ = std::fs::remove_dir_all(receiver.root());
    Ok(())
}
