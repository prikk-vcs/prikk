//! Divergent complete prefixes must never authorize partial-tail truncation.

use std::io::Write;

use prikk_object::RefUpdatePayload;

use super::root_publication;
use crate::foundation::fsutil::{TestFailPoint, fail_after_for_test};
use crate::foundation::layout::ContainerSlot;
use crate::test_gates::test_support::{
    sample_object_id, signed_ref_update_envelope, unique_temp_dir,
};
use crate::{RefStore, RepositoryLayout};

#[derive(Clone, Copy)]
enum Divergence {
    WrongRef,
    WrongOldLink,
    WrongSequence,
    DuplicateRecord,
    ByteDifferentExpected,
}

#[test]
fn divergent_complete_prefix_with_partial_tail_is_preserved_byte_for_byte()
-> prikk_error::Result<()> {
    for divergence in [
        Divergence::WrongRef,
        Divergence::WrongOldLink,
        Divergence::WrongSequence,
        Divergence::DuplicateRecord,
        Divergence::ByteDifferentExpected,
    ] {
        let root = unique_temp_dir("dc38-divergent-partial-tail");
        let layout = RepositoryLayout::init(root.clone())?;
        let publication = root_publication(&layout, "heads/main")?;
        let store = RefStore::new(layout.clone());
        // RFC 102 Stage 4: the candidate/promote mechanism `PromotionDestinationSync` instrumented
        // is gone; the equivalent interruption today is failing the *fourth* `AppendWrite` of a
        // publish -- lock-acquire, object-container-append, object-index-append, and
        // pointer-index-append all precede the log-container-append this test needs to interrupt.
        fail_after_for_test(TestFailPoint::AppendWrite, 3);
        assert!(store.publish(&publication).is_err());

        let proposed = publication.ref_state.object_id();
        let target = RefUpdatePayload::decode_canonical(&publication.ref_update.canonical_payload)?
            .new_target_object_id;
        let divergent = match divergence {
            Divergence::WrongRef => {
                signed_ref_update_envelope("heads/topic", None, proposed, target, 1)
            }
            Divergence::WrongOldLink => signed_ref_update_envelope(
                "heads/main",
                Some(sample_object_id("wrong-old-link")),
                proposed,
                target,
                1,
            ),
            Divergence::WrongSequence => {
                signed_ref_update_envelope("heads/main", None, proposed, target, 2)
            }
            Divergence::DuplicateRecord => publication.ref_update.clone(),
            Divergence::ByteDifferentExpected => {
                let mut envelope = publication.ref_update.clone();
                let byte = envelope
                    .signatures
                    .first_mut()
                    .and_then(|signature| signature.signature_bytes.first_mut())
                    .ok_or_else(|| {
                        prikk_error::PrikkError::Integrity(
                            "expected RefUpdate signature bytes".to_string(),
                        )
                    })?;
                *byte ^= 0xff;
                envelope
            }
        };
        super::super::super::append_log_record_for_signature_test(
            &layout,
            "heads/main",
            &divergent,
        )?;
        let container_path = layout.ref_log_container_slot_path(ContainerSlot::A);
        if matches!(divergence, Divergence::DuplicateRecord) {
            // `heads/main` is the only ref in this fixture, so the whole container is exactly its
            // own subsequence -- duplicating the whole file duplicates only this ref's own records.
            let bytes = std::fs::read(&container_path)?;
            std::fs::OpenOptions::new()
                .append(true)
                .open(&container_path)?
                .write_all(&bytes)?;
        }
        super::super::super::append_torn_ref_log_tail_for_test(
            &layout,
            crate::foundation::layout::ref_name_key_bytes("heads/main"),
            &divergent,
        )?;
        let before = std::fs::read(&container_path)?;

        assert!(
            store
                .finish_interrupted_publication_for_test(&publication)
                .is_err()
        );
        assert_eq!(std::fs::read(&container_path)?, before);
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}
