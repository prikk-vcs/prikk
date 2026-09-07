//! Shared fixtures for `sender::tests`. Unlike `sync_negotiation::sync_test_support`'s bare
//! `ObjectId` patch literals (fine for `summary`/`have_list`, which only ever read a Block's own
//! `patch_ids` field), building a real `PEXCH001` needs real Patch and Blob objects for
//! `export_exchange_artifact` to read -- modelled directly on `seal_from_accepted/tests.rs`'s own
//! fixture shapes (`write_blob`, `create_file_patch`, WAL-then-seal).

use prikk_error::Result;
use prikk_object::{
    BlobKind, BlobPayload, CanonicalEncode, CreateFile, NodeId, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
};

use crate::author::author_signing::author_signature;
use crate::maintainer_signing::{
    Ed25519MaintainerSigner, MaintainerSigner as _, maintainer_signature,
};
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::sync_negotiation::sync_test_support::fresh_repo;
use crate::wal::Wal;
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, FileObjectStore, ObjectWriter, RepositoryLayout,
    add_trusted_maintainer,
};

pub(super) use crate::sync_negotiation::sync_test_support::cleanup;

pub(super) fn repo(name: &str) -> Result<RepositoryLayout> {
    fresh_repo(name)
}

pub(super) fn maintainer_signer(seed: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(format!("sender-maintainer-{seed}"), &[seed; 32])
}

pub(super) fn author_signer(seed: u8) -> Result<Ed25519AuthorSigner> {
    Ed25519AuthorSigner::from_seed(format!("sender-author-{seed}"), &[seed; 32])
}

pub(super) fn adopt(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) -> Result<()> {
    let public_key_hex: String = signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    add_trusted_maintainer(layout, signer.key_id(), &public_key_hex)?;
    Ok(())
}

pub(super) fn write_blob(objects: &mut FileObjectStore, content: &[u8]) -> Result<ObjectId> {
    let signer = maintainer_signer(0xFF)?;
    let payload = BlobPayload::new(BlobKind::Text, content.to_vec());
    let mut envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(&signer, ObjectType::Blob, id)?)?;
    objects.write_object(&envelope)
}

/// A standalone, AUTHOR-signed `CreateFile` patch -- not yet written anywhere.
pub(super) fn create_file_patch(
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

/// Queue `patches` into the active WAL, in order, and seal them as one new Block onto `ref_name` --
/// mirrors `seal_active_no_audit`'s own "new content" path (`simulate_one_seal`'s own doc).
/// `simulate_one_seal` itself returns the published RefState id, not the Block id, so this resolves
/// the ref's new tip afterward and returns the sealed Block id.
pub(super) fn seal_patches_onto(
    layout: &RepositoryLayout,
    ref_name: &str,
    patches: &[ObjectEnvelope],
    signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    // `simulate_one_seal`'s own publication path requires the active-WAL ref-name metadata to
    // name the same ref being sealed (`refs/evidence.rs::validate_signer_backed_recovery`) -- the
    // bookkeeping the real `ActiveSession::append_patch` commit path maintains, which the raw
    // `Wal::append_patch` below deliberately bypasses (it only appends bytes).
    crate::active::write_active_ref_metadata(layout, ref_name)?;
    let wal = Wal::for_layout(layout, DEFAULT_ACTIVE_NAME);
    for patch in patches {
        wal.append_patch(patch)?;
    }
    simulate_one_seal(layout, ref_name, signer)?;
    let ref_store = crate::RefStore::new(layout.clone());
    let objects = FileObjectStore::new(layout.clone());
    let ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)?
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("ref state missing after seal".to_string())
        })?;
    let envelope = crate::object_store::ObjectReader::read_typed(
        &objects,
        ref_state_id,
        ObjectType::RefState,
    )?
    .ok_or_else(|| prikk_error::PrikkError::Integrity("ref state envelope missing".to_string()))?;
    let payload = prikk_object::RefStatePayload::decode_canonical(
        &envelope.canonical_payload,
        envelope.schema_version,
    )?;
    Ok(payload.target_object_id)
}
