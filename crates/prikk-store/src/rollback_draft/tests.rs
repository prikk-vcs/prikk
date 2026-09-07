//! Rollback draft append tests.

use std::cell::Cell;

use prikk_crypto::verify_ed25519;
use prikk_object::{ObjectType, PatchPurpose, Signature, SignatureAlgorithm, SignerRole};

use crate::{
    AuthorSigner, DEFAULT_ACTIVE_NAME, Ed25519AuthorSigner, FileObjectStore, ObjectWriter,
    RefPublication, RefStore, RepositoryLayout, Wal, append_rollback_draft,
};

use crate::test_gates::test_support::{
    publish_snapshot_then_patch_block, publish_text_create_then_edit_block,
    publish_text_edit_then_unsupported_rename_path_block,
};
use crate::test_gates::test_support::{
    signed_empty_block_envelope, signed_patch_envelope, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
};

fn test_signer() -> Ed25519AuthorSigner {
    match Ed25519AuthorSigner::from_seed("rollback-author-key", &[9_u8; 32]) {
        Ok(signer) => signer,
        Err(error) => panic!("test author signer should be constructible: {error}"),
    }
}

struct AdvancingSigner {
    inner: Ed25519AuthorSigner,
    layout: RepositoryLayout,
    advanced: Cell<bool>,
}

impl AdvancingSigner {
    fn new(layout: RepositoryLayout) -> Self {
        Self {
            inner: test_signer(),
            layout,
            advanced: Cell::new(false),
        }
    }
}

impl AuthorSigner for AdvancingSigner {
    fn key_id(&self) -> &str {
        self.inner.key_id()
    }

    fn sign(&self, preimage: &[u8]) -> prikk_error::Result<Vec<u8>> {
        if !self.advanced.replace(true) {
            advance_main_ref(&self.layout)?;
        }
        self.inner.sign(preimage)
    }

    fn public_key_bytes(&self) -> [u8; prikk_crypto::ED25519_KEY_LEN] {
        self.inner.public_key_bytes()
    }
}

fn advance_main_ref(layout: &RepositoryLayout) -> prikk_error::Result<()> {
    let mut object_store = FileObjectStore::new(layout.clone());
    let block = signed_empty_block_envelope();
    let target = object_store.write_object(&block)?;
    let store = RefStore::new(layout.clone());
    let previous = store.read_current_ref_state_id("heads/main")?;
    let ref_state = signed_ref_state_envelope("heads/main", previous, target, 3);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", previous, ref_state_id, target, 3);
    store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: previous,
        ref_state,
        ref_update,
    })?;
    Ok(())
}

#[test]
fn rollback_draft_appends_inverse_patch_to_empty_active_wal() {
    let root = unique_temp_dir("rollback-draft-file-ops");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_snapshot_then_patch_block(&layout);
        assert!(result.is_ok());
        let signer = test_signer();
        let report =
            append_rollback_draft(&layout, "heads/main", "rollback supported ops", &signer);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.ref_name, "heads/main");
            assert_eq!(report.wal_sequence, 1);
            assert_eq!(report.author_key_id, "rollback-author-key");
            assert_eq!(report.inverse_operation_count, 2);
            assert_eq!(report.preview_change_count, 2);
            let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
            let replay = wal.replay();
            assert!(replay.is_ok());
            if let Ok(replay) = replay {
                assert_eq!(replay.records.len(), 1);
                let first = replay.records.first();
                assert!(first.is_some());
                if let Some(first) = first {
                    assert_eq!(first.envelope.object_type, ObjectType::Patch);
                    assert_eq!(first.envelope.object_id(), report.inverse_patch_id);
                    assert!(!first.envelope.signatures.is_empty());
                    assert!(!first.envelope.canonical_payload.is_empty());
                    assert_eq!(
                        PatchPurpose::decode_from_patch_payload(&first.envelope.canonical_payload),
                        Ok(PatchPurpose::RollbackDraft)
                    );
                    let signature = first
                        .envelope
                        .signatures
                        .iter()
                        .find(|signature| signature.signer_role == SignerRole::Author);
                    assert!(signature.is_some());
                    if let Some(signature) = signature {
                        assert_eq!(signature.algorithm, SignatureAlgorithm::Ed25519);
                        assert_eq!(signature.key_id, "rollback-author-key");
                        let preimage = match Signature::signed_bytes(
                            SignatureAlgorithm::Ed25519,
                            ObjectType::Patch,
                            first.envelope.object_id(),
                            SignerRole::Author,
                            &signature.key_id,
                        ) {
                            Ok(preimage) => preimage,
                            Err(error) => panic!("test signature preimage should build: {error}"),
                        };
                        assert!(
                            verify_ed25519(
                                &signer.public_key_bytes(),
                                &preimage,
                                &signature.signature_bytes
                            )
                            .is_ok()
                        );
                    }
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rollback_draft_refuses_candidate_debris_before_planning_or_wal_mutation()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("rollback-draft-candidate-guard");
    let layout = RepositoryLayout::init(root.clone())?;
    let candidate = layout.ref_tmp_path("heads/main");
    let parent = candidate
        .parent()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("candidate has no parent".to_string()))?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&candidate, b"candidate")?;

    let error = append_rollback_draft(&layout, "heads/main", "blocked", &test_signer())
        .err()
        .ok_or_else(|| {
            prikk_error::PrikkError::Integrity("rollback draft unexpectedly ran".to_string())
        })?;
    assert!(
        error
            .to_string()
            .contains("repository mutation is blocked by incomplete ref publication")
    );
    assert_eq!(std::fs::read(&candidate)?, b"candidate");
    assert!(
        Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME)
            .replay()?
            .records
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn rollback_draft_appends_arbitrary_span_text_inverse() {
    let root = unique_temp_dir("rollback-draft-edit-text");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_text_create_then_edit_block(&layout, b"alpha beta\n", b"alpha BETA\n");
        assert!(result.is_ok());
        let signer = test_signer();
        let report = append_rollback_draft(&layout, "heads/main", "rollback text", &signer);
        assert!(report.is_ok());
        if let Ok(report) = report {
            assert_eq!(report.inverse_operation_count, 2);
            assert_eq!(
                report
                    .operations
                    .first()
                    .map(|operation| operation.kind.as_str()),
                Some("edit-text")
            );
            let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
            let replay = wal.replay();
            assert!(replay.is_ok());
            if let Ok(replay) = replay {
                assert_eq!(replay.records.len(), 1);
                let record = replay.records.first();
                assert!(record.is_some());
                if let Some(record) = record {
                    assert_eq!(
                        PatchPurpose::decode_from_patch_payload(&record.envelope.canonical_payload),
                        Ok(PatchPurpose::RollbackDraft)
                    );
                }
            }
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rollback_draft_rejects_ref_change_between_planning_and_append() {
    let root = unique_temp_dir("rollback-draft-stale-tip");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_snapshot_then_patch_block(&layout);
        assert!(result.is_ok());
        let signer = AdvancingSigner::new(layout.clone());
        let report = append_rollback_draft(&layout, "heads/main", "rollback stale tip", &signer);
        assert!(report.is_err());
        if let Err(error) = report {
            assert!(error.to_string().contains("target ref changed"));
            // RFC 132 follow-up: this is a genuine lock conflict, not a caller precondition -- another
            // writer raced this rollback-draft between planning and append, and retrying is exactly
            // the right response, unlike the six sites this taxonomy round moved off `LockConflict`.
            assert!(
                matches!(error, prikk_error::PrikkError::LockConflict(_)),
                "unexpected error variant: {error:?}"
            );
            assert!(
                error.to_string().starts_with("lock conflict:"),
                "unexpected error: {error}"
            );
        }
        let replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).replay();
        assert!(replay.is_ok());
        if let Ok(replay) = replay {
            assert_eq!(replay.records.len(), 0);
        }
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rollback_draft_refuses_non_empty_active_wal() {
    let root = unique_temp_dir("rollback-draft-non-empty-wal");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_snapshot_then_patch_block(&layout);
        assert!(result.is_ok());
        let wal = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME);
        let append = wal.append_patch(&signed_patch_envelope());
        assert!(append.is_ok());
        let signer = test_signer();
        let report =
            append_rollback_draft(&layout, "heads/main", "rollback with pending work", &signer);
        assert!(report.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn rollback_draft_fails_closed_on_supported_text_plus_unsupported_operation() {
    let root = unique_temp_dir("rollback-draft-edit-text-unsupported");
    let layout = RepositoryLayout::init(root.clone());
    assert!(layout.is_ok());
    if let Ok(layout) = layout {
        let result = publish_text_edit_then_unsupported_rename_path_block(&layout);
        assert!(result.is_ok());
        let signer = test_signer();
        let report = append_rollback_draft(&layout, "heads/main", "rollback unsupported", &signer);
        assert!(report.is_err());
    }
    let _ = std::fs::remove_dir_all(root);
}
