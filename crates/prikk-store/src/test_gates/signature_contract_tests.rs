mod admission;
mod diagnostics;
mod read_admission;
mod shape_matrix;
mod vectors;

use prikk_object::{ObjectEnvelope, ObjectType, Signature, SignatureAlgorithm, SignerRole};

fn signature(key_id: &str, byte: u8) -> Signature {
    Signature {
        algorithm: SignatureAlgorithm::Ed25519,
        key_id: key_id.to_string(),
        signature_bytes: vec![byte; 64],
        created_at: 0,
        signer_role: SignerRole::Author,
    }
}

fn malformed_envelope(object_type: ObjectType, payload: &[u8], length: usize) -> ObjectEnvelope {
    let mut malformed = signature("author", 1);
    malformed.signature_bytes.resize(length, 1);
    ObjectEnvelope {
        object_type,
        schema_version: 1,
        canonical_payload: payload.to_vec(),
        signatures: vec![malformed],
    }
}
