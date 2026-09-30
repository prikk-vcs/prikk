//! RFC 164 Rule C: `repair_tails` -- one repair for every tail Rule A defines.

#![allow(clippy::indexing_slicing, clippy::expect_used, clippy::unwrap_used)]

use super::repair_tails;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout, add_trusted_maintainer};

/// A fresh repository with one adopted maintainer -- real trust-key and trust-policy content, so
/// there is something to repair (or, in the clean case, to confirm untouched).
fn repo_with_trust_content(tag: &str) -> RepositoryLayout {
    let layout = RepositoryLayout::init(unique_temp_dir(tag)).expect("init");
    let maintainer =
        Ed25519MaintainerSigner::from_seed("rfc164-repair-tails-maintainer", &[0x85; 32])
            .expect("signer");
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )
    .expect("adopt maintainer");
    layout
}

#[test]
fn a_clean_repository_reports_every_file_untouched() {
    let layout = repo_with_trust_content("rfc164-repair-tails-clean");
    let report = repair_tails(&layout).expect("repair_tails");
    assert_eq!(
        report.files.len(),
        9,
        "the WAL, the pointer index, and the seven Rule-A files"
    );
    for file in &report.files {
        assert_eq!(
            file.truncated_bytes, 0,
            "{}: a clean repository has nothing to repair",
            file.label
        );
        assert!(file.recovery_file.is_none(), "{}", file.label);
    }
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn a_tail_on_trust_keys_is_repaired_and_recovered() {
    let layout = repo_with_trust_content("rfc164-repair-tails-trust-keys-tail");
    let path = layout.trust_key_container_path();
    let before = std::fs::read(&path).unwrap();
    let mut with_tail = before.clone();
    with_tail.extend(vec![0_u8; 100]);
    std::fs::write(&path, &with_tail).unwrap();

    let report = repair_tails(&layout).expect("repair_tails");
    let row = report
        .files
        .iter()
        .find(|file| file.label == "trust keys")
        .expect("trust keys row");
    assert_eq!(row.truncated_bytes, 100);
    let recovery_file = row.recovery_file.as_ref().expect("recovery file recorded");
    let recovery_bytes =
        std::fs::read(layout.prikk_dir().join(recovery_file)).expect("read recovery file");
    assert_eq!(
        recovery_bytes,
        vec![0_u8; 100],
        "exactly the removed bytes, nothing else"
    );

    let after = std::fs::read(&path).unwrap();
    assert_eq!(after, before, "truncated back to exactly the sound prefix");

    // Every other file untouched.
    for file in &report.files {
        if file.label != "trust keys" {
            assert_eq!(
                file.truncated_bytes, 0,
                "{}: unaffected by trust keys' own tail",
                file.label
            );
        }
    }
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn interior_damage_on_one_file_refuses_and_touches_nothing() {
    let layout = repo_with_trust_content("rfc164-repair-tails-interior-damage");
    let path = layout.trust_key_container_path();
    let before_trust_keys = std::fs::read(&path).unwrap();

    // Addendum 1 control 4 ("--repair-tails touching files before its check must redden its
    // rows"): the pointer index also carries a genuine, unrelated TAIL at the same time trust keys
    // carries interior damage. All-or-nothing means this tail must survive the refusal
    // byte-for-byte -- if the all-or-nothing check ever moved after the truncation loop, this tail
    // would be gone even though the repair as a whole still reports an error.
    let pointer_index_path = layout.ref_pointer_index_slot_path(crate::ContainerSlot::A);
    let sound_pointer_entry = crate::refs::PointerIndexEntry {
        ref_name_key: [0x22; 32],
        ref_name: "heads/rfc164-repair-tails-control4".to_string(),
        ref_state_id: prikk_object::ObjectId::from_bytes([0x33; 32]),
    };
    let mut pointer_index_with_tail =
        crate::refs::encode_pointer_index_record(&sound_pointer_entry).expect("encode");
    pointer_index_with_tail.extend(vec![0_u8; 40]);
    std::fs::write(&pointer_index_path, &pointer_index_with_tail).unwrap();

    // Garbage with nothing sound after it would be a tail (RFC 164 Rule A); a genuinely sound
    // trust-key record after the garbage is what keeps this interior damage -- the same shape
    // `hostile_lengths.rs`/`rfc138_trust_read_surface.rs` construct at the CLI level, built here
    // directly via the crate's own (pub(crate)) encoder, since this is a store-internal test.
    let mut damaged = before_trust_keys.clone();
    damaged.extend(vec![0xAB_u8; 200]);
    let sound_entry = crate::trust_index::TrustKeyEntry {
        key_id: "rfc164-repair-tails-sound-entry".to_string(),
        public_key: [0x11; 32],
    };
    damaged.extend(crate::trust_index::encode_trust_key_record(&sound_entry).expect("encode"));
    std::fs::write(&path, &damaged).unwrap();

    let report = repair_tails(&layout);
    assert!(
        report.is_err(),
        "interior damage on trust keys must refuse the whole repair"
    );
    let message = report.unwrap_err().to_string();
    assert!(message.contains("trust keys"), "{message}");

    let after_trust_keys = std::fs::read(&path).unwrap();
    assert_eq!(
        after_trust_keys, damaged,
        "the damaged file itself is untouched"
    );
    let after_pointer_index = std::fs::read(&pointer_index_path).unwrap();
    assert_eq!(
        after_pointer_index, pointer_index_with_tail,
        "an unrelated file's own tail must survive the refusal byte-for-byte -- \
         the all-or-nothing check must run before any truncation, not after"
    );
    let _ = std::fs::remove_dir_all(layout.root());
}

#[test]
fn a_second_run_after_repair_is_idempotent() {
    let layout = repo_with_trust_content("rfc164-repair-tails-idempotent");
    let path = layout.trust_key_container_path();
    let mut with_tail = std::fs::read(&path).unwrap();
    with_tail.extend(vec![0_u8; 30]);
    std::fs::write(&path, &with_tail).unwrap();

    let first = repair_tails(&layout).expect("first repair");
    assert_eq!(
        first
            .files
            .iter()
            .find(|file| file.label == "trust keys")
            .unwrap()
            .truncated_bytes,
        30
    );

    let second = repair_tails(&layout).expect("second repair");
    for file in &second.files {
        assert_eq!(
            file.truncated_bytes, 0,
            "{}: a second run has nothing left to repair",
            file.label
        );
    }
    let _ = std::fs::remove_dir_all(layout.root());
}
