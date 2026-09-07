use prikk_object::{ObjectEnvelope, ObjectId, ObjectType};

use super::malformed_envelope;
use crate::foundation::file_codec::{
    decode_envelope_file, encode_envelope_file, encode_envelope_file_structural,
};
use crate::refs::append_log_record_for_signature_test;
use crate::test_gates::test_support::{
    sample_object_id, signed_ref_state_envelope, signed_ref_update_envelope, unique_temp_dir,
};
use crate::{
    AuthorSigner, DEFAULT_ACTIVE_NAME, FileObjectStore, MaintainerSigner, MemoryObjectStore,
    ObjectWriter, RefPublication, RefStore, RepositoryLayout, Wal, author_signature,
    maintainer_signature,
};

const LENGTHS: [usize; 5] = [0, 1, 63, 64, 65];

struct LengthSigner {
    length: usize,
}

impl AuthorSigner for LengthSigner {
    fn key_id(&self) -> &str {
        "length-author"
    }

    fn sign(&self, _preimage: &[u8]) -> prikk_error::Result<Vec<u8>> {
        Ok(vec![1; self.length])
    }

    fn public_key_bytes(&self) -> [u8; 32] {
        [0; 32]
    }
}

impl MaintainerSigner for LengthSigner {
    fn key_id(&self) -> &str {
        "length-maintainer"
    }

    fn sign(&self, _preimage: &[u8]) -> prikk_error::Result<Vec<u8>> {
        Ok(vec![1; self.length])
    }

    fn public_key_bytes(&self) -> [u8; 32] {
        [0; 32]
    }
}

fn set_signature_length(envelope: &mut ObjectEnvelope, length: usize) -> prikk_error::Result<()> {
    let signature = envelope.signatures.first_mut().ok_or_else(|| {
        prikk_error::PrikkError::Integrity("test envelope is unsigned".to_string())
    })?;
    signature.signature_bytes.resize(length, 1);
    Ok(())
}

#[test]
fn codec_shape_matrix_covers_structural_strict_and_simulated_format2() -> prikk_error::Result<()> {
    for length in LENGTHS {
        let envelope = malformed_envelope(ObjectType::Blob, b"shape", length);
        assert_eq!(
            encode_envelope_file(&envelope).is_ok(),
            length == 64,
            "strict codec length {length}"
        );
        let structural = encode_envelope_file_structural(&envelope);
        assert_eq!(
            structural.is_ok(),
            length != 0,
            "structural codec length {length}"
        );
        if let Ok(bytes) = structural {
            let decoded = decode_envelope_file(&bytes)?;
            assert_eq!(decoded, envelope);
            assert_eq!(
                decoded.validate_strict().is_ok(),
                length == 64,
                "simulated format-2 length {length}"
            );
        }
    }
    Ok(())
}

#[test]
fn object_writer_shape_matrix_rejects_before_mutation() -> prikk_error::Result<()> {
    for length in LENGTHS {
        let root = unique_temp_dir(&format!("dc39-object-shape-{length}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let envelope = malformed_envelope(ObjectType::Blob, &[length as u8], length);
        let object_id = envelope.object_id();
        let mut memory = MemoryObjectStore::new();
        let mut file = FileObjectStore::new(layout.clone());

        assert_eq!(memory.write_object(&envelope).is_ok(), length == 64);
        assert_eq!(memory.len(), usize::from(length == 64));
        assert_eq!(file.write_object(&envelope).is_ok(), length == 64);
        assert_eq!(
            file.contains_object(ObjectType::Blob, object_id),
            length == 64
        );

        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn wal_and_ref_log_shape_matrix_reject_before_mutation() -> prikk_error::Result<()> {
    for length in LENGTHS {
        let root = unique_temp_dir(&format!("dc39-log-shape-{length}"));
        let layout = RepositoryLayout::init(root.clone())?;
        let patch = malformed_envelope(ObjectType::Patch, &[length as u8], length);
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        assert_eq!(wal.append_patch(&patch).is_ok(), length == 64);
        // RFC 102 Stage 1: the WAL file now exists from `init` onward (created empty), so success is
        // proven by a replayed record existing, not by the file's existence.
        assert_eq!(
            wal.replay().is_ok_and(|replay| !replay.records.is_empty()),
            length == 64
        );

        let target = sample_object_id("target");
        let state = sample_object_id("state");
        let mut update = signed_ref_update_envelope("heads/main", None, state, target, 1);
        set_signature_length(&mut update, length)?;
        assert_eq!(
            append_log_record_for_signature_test(&layout, "heads/main", &update).is_ok(),
            length == 64
        );
        assert_eq!(
            RefStore::new(layout.clone())
                .replay_log("heads/main")?
                .records
                .len(),
            usize::from(length == 64)
        );

        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

#[test]
fn publication_shape_matrix_covers_both_governed_envelopes() -> prikk_error::Result<()> {
    for mutate_state in [true, false] {
        for length in LENGTHS {
            let root = unique_temp_dir(&format!("dc39-publication-shape-{mutate_state}-{length}"));
            let layout = RepositoryLayout::init(root.clone())?;
            let target = sample_object_id("target");
            let mut state = signed_ref_state_envelope("heads/main", None, target, 1);
            let state_id = state.object_id();
            let mut update = signed_ref_update_envelope("heads/main", None, state_id, target, 1);
            if mutate_state {
                set_signature_length(&mut state, length)?;
            } else {
                set_signature_length(&mut update, length)?;
            }
            let publication = RefPublication {
                ref_name: "heads/main".to_string(),
                expected_previous_ref_state_id: None,
                ref_state: state,
                ref_update: update,
            };

            assert_eq!(
                RefStore::new(layout.clone()).publish(&publication).is_ok(),
                length == 64,
                "state={mutate_state}, length={length}"
            );
            assert_eq!(
                FileObjectStore::new(layout.clone())
                    .contains_object(ObjectType::RefState, state_id),
                length == 64
            );
            assert_eq!(
                RefStore::new(layout.clone())
                    .read_current_ref_state_id("heads/main")?
                    .is_some(),
                length == 64
            );
            assert_eq!(
                RefStore::new(layout.clone())
                    .replay_log("heads/main")?
                    .records
                    .len(),
                usize::from(length == 64)
            );

            let _ = std::fs::remove_dir_all(root);
        }
    }
    Ok(())
}

#[test]
fn signer_provider_shape_matrix_rejects_malformed_output() {
    let object_id = ObjectId::from_bytes([7; 32]);
    for length in LENGTHS {
        let signer = LengthSigner { length };
        let author = author_signature(&signer, object_id);
        let maintainer = maintainer_signature(&signer, ObjectType::RefState, object_id);
        assert_eq!(author.is_ok(), length == 64, "AUTHOR length {length}");
        assert_eq!(
            maintainer.is_ok(),
            length == 64,
            "MAINTAINER length {length}"
        );
    }
}
