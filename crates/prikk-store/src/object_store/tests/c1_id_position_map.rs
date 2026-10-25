//! 0.55.0 step 3 Part 1 (2C): the id-to-position map (`IndexSnapshot`/`LookupCache`'s own `by_id`)
//! must agree with the reverse linear scan it replaced, in every shape that scan ever had to
//! handle -- a repeated id (ruling C3: "last entry wins" across a format-7 signature merge, both
//! before and after `rebuild_index_from_containers`'s own type-then-container reordering), and an
//! arbitrary sequence of batched extends and wholesale rebuilds (ruling C1).
//!
//! `write_session_catches_up_after_a_nested_unmediated_writer` (this module's parent file) already
//! exercises the one true incremental-extend branch end to end and would go red on its own if `by_id`
//! were not updated there -- it is the closest thing to a "skip the map update in extend" red
//! control this round has against real production code, and is named here rather than duplicated.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::HashMap;

use proptest::prelude::*;

use prikk_object::{BlobKind, BlobPayload, CanonicalEncode, ObjectId, ObjectType};

use crate::foundation::index::{IndexEntry, rebuild_index_from_containers};
use crate::foundation::layout::{ContainerSlot, RepositoryLayout};
use crate::object_store::{checked_index_position, index_position_map};
use crate::test_gates::test_support::{maintainer_signature, unique_temp_dir};
use crate::{
    FileObjectStore, ObjectReader, ObjectWriter, append_superseding_record_for_test_support,
};

/// A second MAINTAINER-role fixture signature, distinct from `maintainer_signature()`'s -- a
/// superseding record (RFC 156's own signature merge) is the one production shape that writes two
/// index records for the same id, so this is what builds it.
fn second_maintainer_signature() -> prikk_object::Signature {
    prikk_object::Signature {
        algorithm: prikk_object::SignatureAlgorithm::Ed25519,
        key_id: "maintainer-key-2".to_string(),
        signature_bytes: vec![6; 64],
        created_at: 9,
        signer_role: prikk_object::SignerRole::Maintainer,
    }
}

/// Ruling C3: an id with two records returns the second ("last entry wins"), both through a plain
/// decode (a fresh handle's cold-path build of `by_id`) and through `rebuild_index_from_containers`
/// (type-then-container order) -- the ruling rests on an id living in exactly one container, so its
/// records keep their write order even under that reordering, not merely on the one call site
/// `resolve_cached` reaches day to day.
#[test]
fn last_entry_wins_before_and_after_a_rebuild_from_containers() -> prikk_error::Result<()> {
    let root = unique_temp_dir("c1-last-entry-wins");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let payload = BlobPayload::new(BlobKind::Text, b"last entry wins\n".to_vec());
    let mut first =
        prikk_object::ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    first.add_signature(maintainer_signature())?;
    let id = store.write_object(&first)?;

    // The superseding record: same id (signatures are not part of the id's own preimage), one more
    // signature than the stored record.
    let mut second = first.clone();
    second.add_signature(second_maintainer_signature())?;
    assert_eq!(
        second.object_id(),
        id,
        "a signature merge must not change the object id"
    );
    append_superseding_record_for_test_support(&layout, &second)?;

    // A plain decode: a fresh handle's own first lookup already goes through the cold path that
    // builds `by_id` from scratch over the two-record index.
    let fresh = FileObjectStore::new(layout.clone());
    let read_back = fresh
        .read_object(id)?
        .unwrap_or_else(|| panic!("object {id} must still be readable"));
    assert_eq!(
        read_back.signatures.len(),
        2,
        "the plain decode path must resolve to the second (superseding) record, not the first"
    );

    // The other half of C3's own claim: force the type-then-container rebuild and check `by_id`
    // directly against a reverse scan over the rebuilt entries.
    let rebuilt = rebuild_index_from_containers(&layout)?;
    assert_eq!(
        rebuilt.iter().filter(|entry| entry.object_id == id).count(),
        2,
        "format 7 keeps both records for a signature-merged id; the rebuild must not drop either"
    );
    let by_id = index_position_map(&rebuilt)?;
    let mapped_position = by_id
        .get(&id)
        .copied()
        .unwrap_or_else(|| panic!("rebuilt index must still carry an entry for {id}"));
    let reference_position = rebuilt
        .iter()
        .enumerate()
        .rev()
        .find(|(_, entry)| entry.object_id == id)
        .map(|(position, _)| position)
        .expect("at least one entry for id");
    assert_eq!(
        mapped_position as usize, reference_position,
        "by_id's own position for a repeated id must agree with the reverse scan it replaced, \
         after a rebuild as much as before one"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// A small id universe (one byte varies) so duplicates are common across a random sequence --
/// exactly the shape ruling C3 says is not rare (every format-7 signature merge appends a new
/// record for the same id).
fn object_id_for_byte(id_byte: u8) -> ObjectId {
    let mut bytes = [0u8; 32];
    bytes[0] = id_byte;
    ObjectId::from_bytes(bytes)
}

fn entry_strategy() -> impl Strategy<Value = IndexEntry> {
    (0u8..6, any::<u64>()).prop_map(|(id_byte, length)| IndexEntry {
        object_id: object_id_for_byte(id_byte),
        object_type: ObjectType::Blob,
        slot: ContainerSlot::A,
        offset: 0,
        length,
        container_checksum: [0u8; 32],
    })
}

/// One step of the kind `IndexSnapshot` actually takes: `Extend` batches new entries onto the end --
/// the one true incremental path, `ensure_current_impl`'s own insert-before-extend discipline,
/// reproduced verbatim below -- and `Rebuild` discards the map and recomputes it wholesale from
/// whatever `entries` holds right then (`open`, the shrink branch, and both damage-rebuild branches
/// all call `index_position_map` over the full vector, never incrementally).
#[derive(Debug, Clone)]
enum Step {
    Extend(Vec<IndexEntry>),
    Rebuild,
}

fn step_strategy() -> impl Strategy<Value = Step> {
    prop_oneof![
        proptest::collection::vec(entry_strategy(), 0..4).prop_map(Step::Extend),
        Just(Step::Rebuild),
    ]
}

/// The reverse-scan reference `by_id` replaced -- what every case below checks it against.
fn reverse_scan_position(entries: &[IndexEntry], id: ObjectId) -> Option<usize> {
    entries.iter().rposition(|entry| entry.object_id == id)
}

proptest! {
    /// Ruling C1: whatever sequence of batched extends and rebuilds produced `entries`, `by_id`
    /// must agree with a reverse scan over it for every id in the small universe, at every step --
    /// not only the ids the sequence happened to end on.
    #[test]
    fn by_id_agrees_with_reverse_scan_over_random_extend_and_rebuild_sequences(
        steps in proptest::collection::vec(step_strategy(), 0..12)
    ) {
        let mut entries: Vec<IndexEntry> = Vec::new();
        let mut by_id: HashMap<ObjectId, u32> = HashMap::new();
        for step in steps {
            match step {
                Step::Extend(tail) => {
                    // `ensure_current_impl`'s own discipline: compute each new entry's position
                    // before moving it into `entries`, so a repeated id's later occurrence always
                    // overwrites the map entry an earlier one in this same batch just inserted.
                    let start = entries.len();
                    for (offset, entry) in tail.iter().enumerate() {
                        let position = checked_index_position(start + offset).unwrap();
                        by_id.insert(entry.object_id, position);
                    }
                    entries.extend(tail);
                }
                Step::Rebuild => {
                    by_id = index_position_map(&entries).unwrap();
                }
            }
            for id_byte in 0u8..6 {
                let id = object_id_for_byte(id_byte);
                let expected = reverse_scan_position(&entries, id);
                let actual = by_id.get(&id).map(|&position| position as usize);
                prop_assert_eq!(
                    actual, expected,
                    "by_id disagrees with a reverse scan for id byte {} after a step", id_byte
                );
            }
        }
    }
}
