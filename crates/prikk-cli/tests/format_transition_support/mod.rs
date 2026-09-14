use std::error::Error;
use std::path::Path;

use prikk_object::{
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, ChangePerm, CreateFile,
    DeleteNode, DeleteNodePreimage, MerkleRoot, NodeId, NodeKind, ObjectEnvelope, ObjectId,
    ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose, RefKind, RefStatePayload,
    RefUpdatePayload, Signature, SignatureAlgorithm, SignerRole,
};
use prikk_store::{
    AuthorSigner, Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout,
    SnapshotManifest, StateRootContent, StateRootEntry, add_trusted_maintainer, author_signature,
    maintainer_signature, write_active_ref_metadata,
};

mod codec;

pub(crate) type TestResult<T = ()> = Result<T, Box<dyn Error>>;

pub(crate) const MAINTAINER_KEY_ID: &str = "legacy-maintainer";
pub(crate) const MAINTAINER_SEED_HEX: &str =
    "3535353535353535353535353535353535353535353535353535353535353535";

#[derive(Debug, Clone, Copy)]
pub(crate) enum ActiveFixture {
    RollbackDraft,
    InterruptedPublication,
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum StrictFailure {
    MalformedLength,
    Duplicate,
    InvertedOrder,
}

mod fixture;

pub(crate) use fixture::{build_current_format_strict_wal_fixture, build_legacy_fixture};
