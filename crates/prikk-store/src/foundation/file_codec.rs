//! Persistent object-envelope file codec.

use prikk_error::{PrikkError, Result};
use prikk_object::{ObjectEnvelope, ObjectType, Signature, SignatureAlgorithm, SignerRole};

use crate::foundation::byte_cursor::ByteCursor;
use crate::foundation::fsutil::{len_to_u16, len_to_u32, len_to_u64};

const ENVELOPE_FILE_MAGIC: &[u8; 8] = b"POBJ0001";

/// Encode an envelope for object-store or WAL persistence.
pub(crate) fn encode_envelope_file(envelope: &ObjectEnvelope) -> Result<Vec<u8>> {
    envelope.validate_strict()?;
    encode_envelope_fields(envelope)
}

#[cfg(test)]
pub(crate) fn encode_envelope_file_structural(envelope: &ObjectEnvelope) -> Result<Vec<u8>> {
    envelope.validate()?;
    encode_envelope_fields(envelope)
}

fn encode_envelope_fields(envelope: &ObjectEnvelope) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    out.extend_from_slice(ENVELOPE_FILE_MAGIC);
    push_u16(&mut out, envelope.object_type.code());
    push_u32(&mut out, envelope.schema_version);
    push_bytes_u64(&mut out, &envelope.canonical_payload)?;
    push_u32(&mut out, len_to_u32(envelope.signatures.len())?);
    for signature in &envelope.signatures {
        push_u16(&mut out, signature.algorithm.code());
        push_u16(&mut out, signature.signer_role.code());
        push_string_u16(&mut out, &signature.key_id)?;
        push_u64(&mut out, signature.created_at);
        push_bytes_u32(&mut out, &signature.signature_bytes)?;
    }
    Ok(out)
}

/// Decode an envelope from object-store or WAL persistence.
pub(crate) fn decode_envelope_file(bytes: &[u8]) -> Result<ObjectEnvelope> {
    let mut cursor = ByteCursor::new(bytes);
    let magic = cursor.read_array::<8>()?;
    if &magic != ENVELOPE_FILE_MAGIC {
        return Err(PrikkError::MalformedData(
            "invalid object file magic".to_string(),
        ));
    }
    let object_type = ObjectType::from_code(cursor.read_u16()?)?;
    let schema_version = cursor.read_u32()?;
    let canonical_payload = cursor.read_bytes_u64()?;
    let signature_count = cursor.read_u32()?;
    let mut signatures = Vec::new();
    for _ in 0..signature_count {
        signatures.push(read_signature(&mut cursor)?);
    }
    if !cursor.is_finished() {
        return Err(PrikkError::MalformedData(
            "trailing bytes in object file".to_string(),
        ));
    }
    let envelope = ObjectEnvelope {
        object_type,
        schema_version,
        canonical_payload,
        signatures,
    };
    envelope.validate()?;
    Ok(envelope)
}

fn read_signature(cursor: &mut ByteCursor<'_>) -> Result<Signature> {
    let algorithm = SignatureAlgorithm::from_code(cursor.read_u16()?)?;
    let signer_role = SignerRole::from_code(cursor.read_u16()?)?;
    let key_id = cursor.read_string_u16()?;
    let created_at = cursor.read_u64()?;
    let signature_bytes = cursor.read_bytes_u32()?;
    Ok(Signature {
        algorithm,
        key_id,
        signature_bytes,
        created_at,
        signer_role,
    })
}

pub(crate) fn push_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn push_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

pub(crate) fn push_string_u16(out: &mut Vec<u8>, value: &str) -> Result<()> {
    let len = len_to_u16(value.len())?;
    push_u16(out, len);
    out.extend_from_slice(value.as_bytes());
    Ok(())
}

pub(crate) fn push_bytes_u32(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = len_to_u32(value.len())?;
    push_u32(out, len);
    out.extend_from_slice(value);
    Ok(())
}

pub(crate) fn push_bytes_u64(out: &mut Vec<u8>, value: &[u8]) -> Result<()> {
    let len = len_to_u64(value.len())?;
    push_u64(out, len);
    out.extend_from_slice(value);
    Ok(())
}

#[cfg(test)]
mod tests;
