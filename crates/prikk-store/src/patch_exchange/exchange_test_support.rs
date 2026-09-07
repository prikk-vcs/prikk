//! Shared fixtures for `patch_exchange`'s own test modules (`artifact::tests`, `accept::tests`,
//! `tests`). Unlike `crate::test_gates::test_support`'s fixed dummy signatures, these build **real** Ed25519
//! signatures -- the accept path's own security properties (§7) are about signature verification,
//! so a fixture that cannot fail to verify would not exercise the thing being tested.

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, PatchSetDigest,
    RecognitionClaimPayload, TagPayload,
};

use crate::author::author_signing::author_signature;
use crate::maintainer_signing::maintainer_signature;
use crate::{Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner};

pub(super) fn author_signer(discriminant: u8) -> Result<Ed25519AuthorSigner> {
    Ed25519AuthorSigner::from_seed(format!("pexch-author-{discriminant}"), &[discriminant; 32])
}

pub(super) fn maintainer_signer(discriminant: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(
        format!("pexch-maintainer-{discriminant}"),
        &[discriminant; 32],
    )
}

pub(super) fn public_key_hex(signer: &Ed25519MaintainerSigner) -> String {
    signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A signed Blob envelope with `content`, signed by an arbitrary fixed maintainer key -- the accept
/// path never checks a Blob's own signature, only its presence, so which key signs it is immaterial.
pub(super) fn signed_blob_envelope(content: &[u8]) -> Result<ObjectEnvelope> {
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    let signer = maintainer_signer(0xFF)?;
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(&signer, ObjectType::Blob, id)?)?;
    Ok(envelope)
}

/// A standalone, AUTHOR-signed Patch envelope: one `CreateFile` operation naming `blob_id`. No
/// Block, no ref -- patch-exchange operates at the patch level alone (§0).
pub(super) fn signed_author_patch_envelope(
    signer: &Ed25519AuthorSigner,
    path: &str,
    node_seed: u8,
    blob_id: ObjectId,
) -> Result<ObjectEnvelope> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: path.to_string(),
                node_id: NodeId::from_bytes([node_seed; 32]),
                blob_id,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(author_signature(signer, id)?)?;
    Ok(envelope)
}

/// A MAINTAINER-signed `RecognitionClaim` envelope naming `block_id`/`patch_ids`.
/// `parent_block_ids` is always empty -- none of Stage 3's own fixtures need a multi-block claim;
/// see `seal_from_accepted`'s own test support for one that does.
pub(super) fn signed_claim_envelope(
    signer: &Ed25519MaintainerSigner,
    block_id: ObjectId,
    patch_ids: Vec<ObjectId>,
) -> Result<ObjectEnvelope> {
    let payload = RecognitionClaimPayload {
        block_id,
        patch_ids,
        parent_block_ids: Vec::new(),
    };
    let mut envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes()?,
    );
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(
        signer,
        ObjectType::RecognitionClaim,
        id,
    )?)?;
    Ok(envelope)
}

/// A MAINTAINER-signed `Tag` envelope naming `target_block_id`/`patch_set_digest`/`patch_count`
/// (RFC 117 stage 3). `name` is passed through verbatim, untrusted -- callers exercising the
/// "attacker-controlled name" properties pass whatever they need to.
pub(super) fn signed_tag_envelope(
    signer: &Ed25519MaintainerSigner,
    name: &str,
    target_block_id: ObjectId,
    patch_set_digest: PatchSetDigest,
    patch_count: u64,
) -> Result<ObjectEnvelope> {
    let payload = TagPayload {
        name: name.to_string(),
        target_block_id,
        message: None,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
        patch_set_digest,
        patch_count,
    };
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Tag, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, ObjectType::Tag, id)?)?;
    Ok(envelope)
}

/// Re-encode a `PEXCH002` artifact's bytes, replacing whichever sections are `Some(..)` and
/// preserving the rest **including the original declared digest untouched** -- for §7's negative
/// controls, which need to construct an artifact whose byte-level shape a real export could never
/// produce (a missing referenced blob, a truncated patch list under an unchanged declared digest)
/// without hand-editing raw bytes. Built from the same low-level primitives `encode_bundle`/
/// `decode_bundle` already use, not a parallel ad hoc format.
pub(super) fn reencode_artifact(
    bytes: &[u8],
    patches: Option<Vec<ObjectEnvelope>>,
    blobs: Option<Vec<ObjectEnvelope>>,
    claims: Option<Vec<ObjectEnvelope>>,
    tags: Option<Vec<ObjectEnvelope>>,
) -> Result<Vec<u8>> {
    use crate::foundation::file_codec::{encode_envelope_file, push_bytes_u64, push_u64};

    let decoded = crate::patch_exchange::artifact::decode_exchange_artifact(bytes, 10_000_000)?;
    let patches = patches.unwrap_or(decoded.patches);
    let blobs = blobs.unwrap_or(decoded.blobs);
    let claims = claims.unwrap_or(decoded.claims);
    let tags = tags.unwrap_or(decoded.tags);

    let mut out = Vec::new();
    out.extend_from_slice(b"PEXCH002");
    out.extend_from_slice(&decoded.declared_digest.0);
    push_u64(&mut out, patches.len() as u64);
    for envelope in &patches {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, blobs.len() as u64);
    for envelope in &blobs {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, decoded.author_keys.len() as u64);
    for entry in &decoded.author_keys {
        push_bytes_u64(&mut out, entry.key_id.as_bytes())?;
        out.extend_from_slice(&entry.public_key);
    }
    push_u64(&mut out, claims.len() as u64);
    for envelope in &claims {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    push_u64(&mut out, tags.len() as u64);
    for envelope in &tags {
        push_bytes_u64(&mut out, &encode_envelope_file(envelope)?)?;
    }
    Ok(out)
}
