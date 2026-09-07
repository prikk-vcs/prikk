//! DC-86 property tests, retargeted from the retired per-file `received.rs` wire format (RFC 102
//! Stage 5) onto this container's own record framing. `decode_received_index_records` is read
//! whenever a received ref is looked up or listed, and `write_received_pointer` (via `import_bundle`)
//! is the path that appends bytes derived from data a party the operator does not control — the same
//! hardening rationale the original coverage stated, unchanged by the storage-shape migration.
//!
//! Two properties: round-trip for an arbitrary ref name and object id, and totality for arbitrary
//! bytes. Case budget is proptest's own default (256/run), overridable with `PROPTEST_CASES` for a
//! campaign run.

#![allow(clippy::expect_used)]

use proptest::prelude::*;

use prikk_object::ObjectId;

use super::super::{
    ReceivedIndexEntry, decode_received_index_records, encode_received_index_record,
};
use crate::foundation::layout::ref_name_key_bytes;

fn object_id_strategy() -> impl Strategy<Value = ObjectId> {
    proptest::array::uniform32(any::<u8>()).prop_map(ObjectId::from_bytes)
}

fn ref_name_strategy() -> impl Strategy<Value = String> {
    "remotes/[a-zA-Z0-9_/-]{0,32}"
}

proptest! {
    #[test]
    fn received_index_entry_round_trips_an_arbitrary_ref_name_and_id(
        ref_name in ref_name_strategy(),
        ref_state_id in object_id_strategy()
    ) {
        let entry = ReceivedIndexEntry {
            ref_name_key: ref_name_key_bytes(&ref_name),
            ref_name: ref_name.clone(),
            ref_state_id,
        };
        let record = encode_received_index_record(&entry)
            .expect("generation invariants keep the ref name encodable");
        let replay = decode_received_index_records(&record)
            .expect("a single well-formed record must decode");
        prop_assert_eq!(replay.entries, vec![entry]);
        prop_assert_eq!(replay.trailing_partial_bytes, 0);
    }

    #[test]
    fn decode_received_index_records_never_panics_on_arbitrary_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..256)
    ) {
        let _ = decode_received_index_records(&bytes);
    }
}
