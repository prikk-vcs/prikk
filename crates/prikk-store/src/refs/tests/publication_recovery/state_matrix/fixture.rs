//! Trusted persisted-state construction for the DC-38 outcome matrix.

use std::io::Write;
use std::path::Path;

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload,
};

use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test};
use crate::foundation::layout::ContainerSlot;
use crate::test_gates::test_support::{
    rollback_patch_blob_envelope, rollback_patch_envelope, signed_patch_blob_envelope,
    signed_patch_envelope, unique_temp_dir,
};
use crate::{
    DEFAULT_ACTIVE_NAME, Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader,
    ObjectWriter, RefPublication, RefStore, RepositoryLayout, Wal, add_trusted_maintainer,
    derive_next_state_root, maintainer_signature, write_active_ref_metadata,
};

#[derive(Debug, Clone, Copy)]
pub(super) enum PersistedState {
    Candidate,
    PointerLeading,
    PartialTail,
    CompleteCleanup,
    LegacyLogLeading,
    Divergence,
    EmptyWalMetadata,
}

pub(super) struct Fixture {
    pub(super) root: std::path::PathBuf,
    pub(super) layout: RepositoryLayout,
    pub(super) publication: RefPublication,
}

impl Fixture {
    pub(super) fn new(state: PersistedState) -> prikk_error::Result<Self> {
        let root = unique_temp_dir("dc38-outcome-matrix");
        let layout = RepositoryLayout::init(root.clone())?;
        let signer = Ed25519MaintainerSigner::from_seed("matrix-maintainer", &[0x61; 32])?;
        let public_key = signer
            .public_key_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        add_trusted_maintainer(&layout, signer.key_id(), &public_key)?;
        let patch = if matches!(state, PersistedState::LegacyLogLeading) {
            rollback_patch_envelope()
        } else {
            signed_patch_envelope()
        };
        let publication = if matches!(state, PersistedState::LegacyLogLeading) {
            existing_ref_publication(&layout, &signer, &patch)?
        } else {
            root_publication(&layout, &signer, &patch)?
        };
        construct_state(&layout, state, &publication)?;
        if matches!(state, PersistedState::EmptyWalMetadata) {
            write_active_ref_metadata(&layout, "heads/topic")?;
        } else {
            write_active_ref_metadata(&layout, "heads/main")?;
            Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&patch)?;
        }
        Ok(Self {
            root,
            layout,
            publication,
        })
    }

    pub(super) fn state_bytes(&self) -> prikk_error::Result<Vec<Option<Vec<u8>>>> {
        [
            self.layout.ref_tmp_path("heads/main"),
            self.layout.ref_pointer_index_slot_path(ContainerSlot::A),
            self.layout.ref_log_container_slot_path(ContainerSlot::A),
            self.layout.default_queue_wal_path(),
            self.layout.default_active_ref_name_path(),
        ]
        .iter()
        .map(|path| read_optional(path))
        .collect()
    }

    pub(super) fn remove(self) {
        let _ = std::fs::remove_dir_all(self.root);
    }
}

fn construct_state(
    layout: &RepositoryLayout,
    state: PersistedState,
    publication: &RefPublication,
) -> prikk_error::Result<()> {
    let store = RefStore::new(layout.clone());
    match state {
        PersistedState::Candidate => {
            // RFC 102 Stage 4, design-v1.md §13.9: no real code path can leave a file under
            // `refs/tmp/` anymore (the candidate-write-then-promote mechanism is gone), so this is
            // no longer reachable through any real crash -- planted directly to keep exercising
            // `candidate_issues`'s own dormant scan, kept per the ruling rather than retired.
            let path = layout.ref_tmp_path("heads/main");
            std::fs::write(path, b"candidate")?;
        }
        PersistedState::PointerLeading => {
            // RFC 102 Stage 4: the candidate/promote mechanism `PromotionDestinationSync`
            // instrumented is gone; failing the fourth `AppendWrite` (after lock-acquire,
            // object-container-append, object-index-append, and pointer-index-append) lands the
            // interruption on the log-container-append instead, leaving the pointer committed and
            // the log not yet appended -- the same state this variant is named for.
            fail_after_for_test(TestFailPoint::AppendWrite, 3);
            assert!(store.publish(publication).is_err());
        }
        PersistedState::PartialTail => {
            // RFC 102 Stage 4: one more `AppendWrite` precedes the log append than in Stage 3
            // (the pointer-index append), so skip 3, not 2, to land on it. This is the ref's
            // first-ever publish, so the interrupted log append leaves *no* real record behind to
            // duplicate -- encode the pending update directly instead.
            fail_after_for_test(TestFailPoint::AppendWrite, 3);
            assert!(store.publish(publication).is_err());
            super::super::super::super::append_torn_ref_log_tail_for_test(
                layout,
                crate::foundation::layout::ref_name_key_bytes("heads/main"),
                &publication.ref_update,
            )?;
        }
        PersistedState::CompleteCleanup => {
            store.publish(publication)?;
        }
        PersistedState::LegacyLogLeading => {
            FileObjectStore::new(layout.clone()).write_object(&publication.ref_state)?;
            super::super::super::super::append_log_record_for_signature_test(
                layout,
                "heads/main",
                &publication.ref_update,
            )?;
        }
        PersistedState::Divergence => {
            store.publish(publication)?;
            // `heads/main` is the only ref in this fixture, so the whole container is exactly its
            // own subsequence -- duplicating the whole file duplicates only this ref's own records.
            let container_path = layout.ref_log_container_slot_path(ContainerSlot::A);
            let bytes = std::fs::read(&container_path)?;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&container_path)?
                .write_all(&bytes)?;
        }
        PersistedState::EmptyWalMetadata => {}
    }
    Ok(())
}

fn root_publication(
    layout: &RepositoryLayout,
    signer: &impl MaintainerSigner,
    patch: &ObjectEnvelope,
) -> prikk_error::Result<RefPublication> {
    publication(layout, signer, patch, None, 1)
}

fn existing_ref_publication(
    layout: &RepositoryLayout,
    signer: &impl MaintainerSigner,
    patch: &ObjectEnvelope,
) -> prikk_error::Result<RefPublication> {
    let baseline = publication(layout, signer, &signed_patch_envelope(), None, 1)?;
    RefStore::new(layout.clone()).publish(&baseline)?;
    publication(
        layout,
        signer,
        patch,
        Some(baseline.ref_state.object_id()),
        2,
    )
}

fn publication(
    layout: &RepositoryLayout,
    signer: &impl MaintainerSigner,
    patch: &ObjectEnvelope,
    previous: Option<prikk_object::ObjectId>,
    sequence: u64,
) -> prikk_error::Result<RefPublication> {
    let mut objects = FileObjectStore::new(layout.clone());
    objects.write_object(&signed_patch_blob_envelope())?;
    objects.write_object(&rollback_patch_blob_envelope())?;
    let patch_id = objects.write_object(patch)?;
    let parent = match previous {
        Some(previous_ref_state_id) => {
            let envelope = objects
                .read_typed(previous_ref_state_id, ObjectType::RefState)?
                .ok_or_else(|| {
                    prikk_error::PrikkError::Integrity(
                        "fixture previous RefState is missing".to_string(),
                    )
                })?;
            Some(
                RefStatePayload::decode_canonical(
                    &envelope.canonical_payload,
                    envelope.schema_version,
                )?
                .target_object_id,
            )
        }
        None => None,
    };
    let block_payload = BlockPayload {
        parent_block_ids: parent.into_iter().collect(),
        kind: if parent.is_some() {
            BlockKind::Normal
        } else {
            BlockKind::Root
        },
        patch_ids: vec![patch_id],
        state_merkle_root: derive_next_state_root(&objects, parent, &[patch_id])?,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let block = signed_publication(
        ObjectType::Block,
        block_payload.to_canonical_bytes()?,
        signer,
    )?;
    let target = objects.write_object(&block)?;
    let state_payload = RefStatePayload {
        ref_name: "heads/main".to_string(),
        kind: RefKind::Branch,
        target_object_id: target,
        update_seq: sequence,
        previous_ref_state_id: previous,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state = signed_publication(
        ObjectType::RefState,
        state_payload.to_canonical_bytes()?,
        signer,
    )?;
    let state_id = ref_state.object_id();
    let update_payload = RefUpdatePayload {
        ref_name: "heads/main".to_string(),
        old_ref_state_id: previous,
        new_ref_state_id: state_id,
        new_target_object_id: target,
        update_seq: sequence,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    Ok(RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: previous,
        ref_update: signed_publication(
            ObjectType::RefUpdate,
            update_payload.to_canonical_bytes()?,
            signer,
        )?,
        ref_state,
    })
}

fn signed_publication(
    object_type: ObjectType,
    canonical_payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> prikk_error::Result<ObjectEnvelope> {
    let schema_version = if object_type == ObjectType::Block {
        2
    } else {
        1
    };
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    envelope.add_signature(maintainer_signature(
        signer,
        object_type,
        envelope.object_id(),
    )?)?;
    Ok(envelope)
}

fn read_optional(path: &Path) -> prikk_error::Result<Option<Vec<u8>>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}
