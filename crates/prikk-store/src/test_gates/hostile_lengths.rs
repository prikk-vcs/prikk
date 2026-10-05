//! **P4 -- the hostile-length damage suite** (RFC 160 §3.4). For every on-disk record format with a length or count field, one test
//! sets that field to its maximum -- **with a matching checksum where the format has one**, so the length is what is tested and not
//! the checksum -- and demands the decode is refused: an error, a failed outcome, or a torn-tail remainder; **never an abort, never
//! an allocation of the claimed size**. Each case runs in a child process ([`hostile_case!`]) with an address-space limit, so a
//! regression fails one test naming the case.
//!
//! The formats (RFC 160 §3.4's list, and what the sweep added): container frame, object index, WAL, trust index (key entries and
//! policy snapshots), author-key index, pointer index (`refs/tests/hostile_lengths.rs`), received index, ref containers
//! (`refs/tests/hostile_lengths.rs`), lifecycle cache, verified-blocks record, commit index, generation file. The two `refs` formats
//! live beside `refs` because their replay types are private to it; [`the_suite_covers_every_format`] holds the whole list and fails
//! if a format's case is missing.

#![allow(
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use prikk_error::Result;
use prikk_object::{ObjectEnvelope, ObjectId, ObjectType};

use super::hostile_length_support::{
    CLAIMS, Seen, assert_refused, body_of, claiming, frame_with_body, hostile_case,
};
use super::test_support::signed_patch_envelope;

/// The envelope file's own hostile lengths (every container, WAL and ref-container body is one): the payload length (`u64`, at bytes
/// 14..22 of the file) and the signature count (`u32`, right after the payload).
pub(crate) fn envelope_bodies(body: &[u8]) -> Vec<HostileBody> {
    let mut out = Vec::new();
    for (label, claimed) in [
        ("payload length u64::MAX", u64::MAX),
        ("payload length 2^62", 1 << 62),
    ] {
        let mut hostile = body.to_vec();
        hostile[14..22].copy_from_slice(&claimed.to_be_bytes());
        out.push((label, hostile));
    }
    let payload_len =
        usize::try_from(u64::from_be_bytes(body[14..22].try_into().unwrap())).unwrap();
    let count_at = 22 + payload_len;
    let mut hostile = body.to_vec();
    hostile[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    out.push(("signature count u32::MAX", hostile));
    out
}

/// A body whose own length or count field is hostile, with the label that names the field.
pub(crate) type HostileBody = (&'static str, Vec<u8>);

/// One frame format: how to make a valid record, how many bytes sit between the version and the length field, how to decode a byte
/// string into a [`Seen`], and which body-internal fields to make hostile.
pub(crate) struct Format {
    pub(crate) name: &'static str,
    pub(crate) pre: usize,
    pub(crate) valid: fn() -> Vec<u8>,
    pub(crate) decode: fn(&[u8]) -> Result<Seen>,
    pub(crate) hostile_bodies: fn(&[u8]) -> Vec<HostileBody>,
}

fn blob_envelope() -> ObjectEnvelope {
    ObjectEnvelope::unsigned(
        ObjectType::Blob,
        1,
        b"a blob body for the hostile-length suite".to_vec(),
    )
}

fn container_valid() -> Vec<u8> {
    crate::foundation::container::encode_container_record(ObjectType::Blob, &blob_envelope())
        .expect("encoding")
}

fn container_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::foundation::container::{ContainerRecordStatus, decode_container_records};
    let replay = decode_container_records(ObjectType::Blob, bytes)?;
    Ok(Seen {
        records: replay.records.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, ContainerRecordStatus::Failed { .. }))
            .count(),
    })
}

fn index_entry() -> crate::foundation::index::IndexEntry {
    crate::foundation::index::IndexEntry {
        object_id: ObjectId::from_bytes([3; 32]),
        object_type: ObjectType::Blob,
        slot: crate::foundation::layout::ContainerSlot::A,
        offset: 0,
        length: 100,
        container_checksum: [4; 32],
    }
}

fn index_valid() -> Vec<u8> {
    crate::foundation::index::encode_index_record(&index_entry()).expect("encoding")
}

fn index_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::foundation::index::{IndexRecordStatus, decode_index_records};
    let replay = decode_index_records(bytes, 0)?;
    Ok(Seen {
        records: replay.entries.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, IndexRecordStatus::Failed { .. }))
            .count(),
    })
}

fn wal_valid() -> Vec<u8> {
    crate::wal::encode_record_for_test(&crate::wal::WalRecord {
        seq: 1,
        envelope: signed_patch_envelope(),
    })
    .expect("encoding")
}

fn wal_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::wal::{WalRecordStatus, decode_records};
    let replay = decode_records(bytes)?;
    Ok(Seen {
        records: replay.records.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, WalRecordStatus::Failed { .. }))
            .count(),
    })
}

fn trust_key_valid() -> Vec<u8> {
    crate::trust_index::encode_trust_key_record(&crate::trust_index::TrustKeyEntry {
        key_id: "maintainer-key".to_string(),
        public_key: [7; 32],
    })
    .expect("encoding")
}

fn trust_key_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::trust_index::{TrustKeyRecordStatus, decode_trust_key_records};
    let replay = decode_trust_key_records(bytes)?;
    Ok(Seen {
        records: replay.entries.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, TrustKeyRecordStatus::Failed { .. }))
            .count(),
    })
}

/// A body whose first two bytes (a `u16` string length) claim the maximum.
fn u16_length_bodies(body: &[u8]) -> Vec<HostileBody> {
    let mut hostile = body.to_vec();
    hostile[0..2].copy_from_slice(&u16::MAX.to_be_bytes());
    vec![("key id length u16::MAX", hostile)]
}

fn trust_policy_valid() -> Vec<u8> {
    crate::trust_index::encode_trust_policy_record(&crate::trust_index::TrustPolicySnapshotEntry {
        key_ids: vec!["key-a".to_string(), "key-b".to_string()],
    })
    .expect("encoding")
}

fn trust_policy_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::trust_index::{TrustPolicyRecordStatus, decode_trust_policy_records};
    let replay = decode_trust_policy_records(bytes)?;
    Ok(Seen {
        records: replay.entries.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, TrustPolicyRecordStatus::Failed { .. }))
            .count(),
    })
}

/// The policy snapshot's key count (`u32`, the first four bytes of the body) at its maximum, and at 2^31.
fn trust_policy_bodies(body: &[u8]) -> Vec<HostileBody> {
    [
        ("key count u32::MAX", u32::MAX),
        ("key count 2^31", 1 << 31),
    ]
    .into_iter()
    .map(|(label, count)| {
        let mut hostile = body.to_vec();
        hostile[0..4].copy_from_slice(&count.to_be_bytes());
        (label, hostile)
    })
    .collect()
}

fn author_key_valid() -> Vec<u8> {
    crate::author::author_key_index::encode_author_key_record(
        &crate::author::author_key_index::AuthorKeyEntry {
            key_id: "author-key".to_string(),
            public_key: [9; 32],
        },
    )
    .expect("encoding")
}

fn author_key_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::author::author_key_index::{AuthorKeyRecordStatus, decode_author_key_records};
    let replay = decode_author_key_records(bytes)?;
    Ok(Seen {
        records: replay.entries.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, AuthorKeyRecordStatus::Failed { .. }))
            .count(),
    })
}

fn received_valid() -> Vec<u8> {
    crate::received::received_index::encode_received_index_record(
        &crate::received::received_index::ReceivedIndexEntry {
            ref_name_key: [5; 32],
            ref_name: "heads/main".to_string(),
            ref_state_id: ObjectId::from_bytes([6; 32]),
        },
    )
    .expect("encoding")
}

fn received_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::received::received_index::{
        ReceivedIndexRecordStatus, decode_received_index_records,
    };
    let replay = decode_received_index_records(bytes)?;
    Ok(Seen {
        records: replay.entries.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, ReceivedIndexRecordStatus::Failed { .. }))
            .count(),
    })
}

/// A body of `ref_name_key(32) ref_name(u64-length-prefixed) ref_state_id(32)` whose name length claims the maximum.
pub(crate) fn ref_name_length_bodies(body: &[u8]) -> Vec<HostileBody> {
    let mut hostile = body.to_vec();
    hostile[32..40].copy_from_slice(&u64::MAX.to_be_bytes());
    vec![("ref name length u64::MAX", hostile)]
}

fn generation_valid() -> Vec<u8> {
    crate::foundation::generation::encode_generation_record(
        &crate::foundation::generation::GenerationRecord {
            live_slot: crate::foundation::layout::ContainerSlot::A,
        },
    )
}

fn generation_decode(bytes: &[u8]) -> Result<Seen> {
    use crate::foundation::generation::{GenerationRecordStatus, decode_generation_records};
    let replay = decode_generation_records(bytes)?;
    Ok(Seen {
        records: replay.records.len(),
        trailing_partial_bytes: replay.trailing_partial_bytes,
        failed: replay
            .record_outcomes
            .iter()
            .filter(|outcome| matches!(outcome.status, GenerationRecordStatus::Failed { .. }))
            .count(),
    })
}

fn no_bodies(_: &[u8]) -> Vec<HostileBody> {
    Vec::new()
}

pub(crate) fn formats() -> Vec<Format> {
    vec![
        Format {
            name: "container frame",
            pre: 0,
            valid: container_valid,
            decode: container_decode,
            hostile_bodies: envelope_bodies,
        },
        Format {
            name: "object index",
            pre: 0,
            valid: index_valid,
            decode: index_decode,
            hostile_bodies: no_bodies,
        },
        Format {
            name: "WAL",
            pre: 8,
            valid: wal_valid,
            decode: wal_decode,
            hostile_bodies: envelope_bodies,
        },
        Format {
            name: "trust key",
            pre: 0,
            valid: trust_key_valid,
            decode: trust_key_decode,
            hostile_bodies: u16_length_bodies,
        },
        Format {
            name: "trust policy",
            pre: 0,
            valid: trust_policy_valid,
            decode: trust_policy_decode,
            hostile_bodies: trust_policy_bodies,
        },
        Format {
            name: "author key",
            pre: 0,
            valid: author_key_valid,
            decode: author_key_decode,
            hostile_bodies: u16_length_bodies,
        },
        Format {
            name: "received index",
            pre: 0,
            valid: received_valid,
            decode: received_decode,
            hostile_bodies: ref_name_length_bodies,
        },
        Format {
            name: "generation",
            pre: 0,
            valid: generation_valid,
            decode: generation_decode,
            hostile_bodies: no_bodies,
        },
    ]
}

/// One format, both kinds of hostile input, and the controls that make the verdicts mean something:
/// - a valid record decodes to one record (the adapter can see acceptance);
/// - the frame helper reproduces the format's own framing byte for byte (so a hostile body carries a checksum the format accepts);
/// - every hostile **frame length** in [`CLAIMS`] is refused;
/// - every hostile **body field**, in a frame with a matching checksum, is refused.
pub(crate) fn check(format: &Format) {
    let valid = (format.valid)();
    let accepted = (format.decode)(&valid).expect("a valid record decodes");
    assert_eq!(
        accepted.records, 1,
        "{}: the valid record is accepted",
        format.name
    );
    assert_eq!(
        frame_with_body(&valid, format.pre, body_of(&valid, format.pre)),
        valid,
        "{}: the frame helper is not the format's own framing",
        format.name
    );
    for claimed in CLAIMS {
        let hostile = claiming(&valid, format.pre, *claimed);
        assert_refused(
            &format!("{} / frame length {claimed}", format.name),
            (format.decode)(&hostile),
        );
    }
    let bodies = (format.hostile_bodies)(body_of(&valid, format.pre));
    for (label, body) in bodies {
        let hostile = frame_with_body(&valid, format.pre, &body);
        assert_refused(
            &format!("{} / {label} with a matching checksum", format.name),
            (format.decode)(&hostile),
        );
    }
}

/// **RFC 160 F3, one rule for every framed reader: a torn tail is a prefix of ONE frame.** For a format, with `V` a valid record:
/// - **damage**: `V` with its length field claiming 2^62, then a second sound record. The partial frame is a **failed item**, the
///   sound record after it **decodes**, and `trailing_partial_bytes` is 0 (before the rule: no failure, no record, the whole
///   remainder a "tail" -- which `doctor --repair-wal-tail` then truncated);
/// - **crash recovery unchanged**: one sound record and then a **prefix** of a second (every length from one byte to all but the
///   last), is one record, no failure, and the prefix is the tail -- the interrupted append every reader must still tolerate;
/// - **ambiguity resolves to damage**: a sound record *inside the payload of a partial frame* (`claiming(V) ‖ V` read as if the
///   partial frame's body held a whole frame) is damage, not a tail.
///
/// **Perturb:** make `sound_frame_after_partial` return `None` (the old classification): every format's damage row goes red.
pub(crate) fn check_partial_frames(format: &Format) {
    let valid = (format.valid)();
    let one = |bytes: &[u8]| (format.decode)(bytes).expect("decodes without an error");

    // Damage: a partial frame with a sound frame behind it.
    let mut damaged = claiming(&valid, format.pre, 1 << 62);
    damaged.extend_from_slice(&valid);
    let seen = one(&damaged);
    assert_eq!(
        (seen.records, seen.failed, seen.trailing_partial_bytes),
        (1, 1, 0),
        "{}: a partial frame before a sound one is a failed item, the sound one decodes, and there is no tail: {seen:?}",
        format.name
    );

    // Crash recovery unchanged: a prefix of the last frame is a tail, tolerated.
    for cut in [1, 9, 10, valid.len() / 2, valid.len() - 1] {
        let mut torn = valid.clone();
        torn.extend_from_slice(&valid[..cut]);
        let seen = one(&torn);
        assert_eq!(
            (seen.records, seen.failed, seen.trailing_partial_bytes),
            (1, 0, cut),
            "{}: a {cut}-byte prefix of the last frame is a torn tail, tolerated as before: {seen:?}",
            format.name
        );
    }

    // Ambiguity resolves to damage: the "body" of a partial frame that holds a sound frame.
    let mut embedded = valid[..valid.len() - 1].to_vec();
    embedded.extend_from_slice(&claiming(&valid, format.pre, 1 << 40));
    embedded.extend_from_slice(&valid);
    let seen = one(&embedded);
    assert!(
        seen.failed >= 1 && seen.trailing_partial_bytes == 0 && seen.records >= 1,
        "{}: a sound frame inside a partial one reads as damage, never as a tail: {seen:?}",
        format.name
    );
}

fn check_named(name: &str) {
    let all = formats();
    let format = all
        .iter()
        .find(|format| format.name == name)
        .expect("a format of that name");
    check(format);
}

fn container_frame() {
    check_named("container frame");
    // The reader that fetches a frame from a file is sized by the header's own claim in the object read (`read_object_envelope_at`):
    // asked for a frame of every hostile length, the ranged reader returns what the file holds and allocates no more (RFC 102
    // Addendum 1 item 1: it clamps to the file's size before it allocates).
    use crate::foundation::fsutil::read_file_range_if_exists;
    let root = super::test_support::unique_temp_dir("hostile-frame-read");
    let layout = crate::foundation::layout::RepositoryLayout::init(root.clone()).expect("init");
    let valid = container_valid();
    let path = layout.container_slot_path(
        ObjectType::Blob,
        crate::foundation::layout::ContainerSlot::A,
    );
    let relative = layout.repository_relative(&path).expect("relative");
    for claimed in CLAIMS {
        std::fs::write(&path, claiming(&valid, 0, *claimed))
            .expect("writing the hostile container");
        let asked = usize::try_from(*claimed).unwrap_or(usize::MAX);
        let read =
            read_file_range_if_exists(layout.repository_mutation_root(), &relative, 0, asked)
                .expect("the ranged read")
                .expect("the container exists");
        assert_eq!(
            read.len(),
            valid.len(),
            "the read returns what the file holds, for a claim of {claimed}"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
fn object_index() {
    check_named("object index");
    // The index's entry fields at their maximum decode (they are data, not sizes), and the read that follows them refuses: an entry
    // naming offset and length u64::MAX is an integrity error at the read, never an allocation of that length.
    use crate::foundation::index::{IndexEntry, read_object_envelope_at};
    let root = super::test_support::unique_temp_dir("hostile-index-entry");
    let layout = crate::foundation::layout::RepositoryLayout::init(root.clone()).expect("init");
    for (offset, length) in [(u64::MAX, u64::MAX), (0, u64::MAX), (u64::MAX, 100)] {
        let entry = IndexEntry {
            offset,
            length,
            ..index_entry()
        };
        assert!(
            read_object_envelope_at(&layout, &entry).is_err(),
            "an index entry at offset {offset} of length {length} is refused at the read"
        );
    }
    let _ = std::fs::remove_dir_all(root);
}
fn wal() {
    check_named("WAL");
}
fn trust_key() {
    check_named("trust key");
}
fn trust_policy() {
    check_named("trust policy");
}
fn author_key() {
    check_named("author key");
}
fn received_index() {
    check_named("received index");
}
fn generation_file() {
    check_named("generation");
}

/// **The verified-blocks record** (a whole-file cache): its block count (`u32`) at the maximum, with a matching checksum, is refused
/// (`None`), and so is a claimed count that overflows when multiplied by 32.
fn verified_blocks_record() {
    use crate::verified_blocks::{RECORD_MAGIC, decode, encode};
    let set: std::collections::BTreeSet<ObjectId> =
        [ObjectId::from_bytes([1; 32]), ObjectId::from_bytes([2; 32])].into();
    let valid = encode(&set);
    assert_eq!(
        decode(&valid),
        Some(set),
        "the valid record decodes (the adapter can see acceptance)"
    );
    let body = &valid[RECORD_MAGIC.len() + 32..];
    let version_len = usize::from(u16::from_be_bytes([body[4], body[5]]));
    let count_at = 6 + version_len;
    for count in [u32::MAX, 1 << 31, (u32::MAX / 32) + 1] {
        let mut hostile = body.to_vec();
        hostile[count_at..count_at + 4].copy_from_slice(&count.to_be_bytes());
        let mut bytes = RECORD_MAGIC.to_vec();
        bytes.extend_from_slice(&prikk_hash::sha256(&hostile));
        bytes.extend_from_slice(&hostile);
        assert_eq!(
            decode(&bytes),
            None,
            "block count {count} with a matching checksum is refused"
        );
    }
}

/// **The lifecycle cache** (a whole-file cache of canonical fields, each `tag u16, wire u8, length u64, value`): a field whose length
/// claims the maximum, with a matching checksum, is refused (`None`) for a list item and for a scalar.
fn lifecycle_cache() {
    use crate::lifecycle_cache::incremental::{CACHE_MAGIC, decode};
    use prikk_object::WireType;
    for (tag, wire) in [
        (10_u16, WireType::RecordListItem as u8),
        (11, WireType::RecordListItem as u8),
        (1, WireType::U32 as u8),
    ] {
        for claimed in [u64::MAX, 1 << 62, 1 << 32] {
            let mut body = Vec::new();
            body.extend_from_slice(&tag.to_be_bytes());
            body.push(wire);
            body.extend_from_slice(&claimed.to_be_bytes());
            body.extend_from_slice(b"short");
            let mut bytes = CACHE_MAGIC.to_vec();
            bytes.extend_from_slice(&prikk_hash::sha256(&body));
            bytes.extend_from_slice(&body);
            assert!(
                decode(&bytes).is_none(),
                "a field (tag {tag}) claiming {claimed} bytes with a matching checksum is refused"
            );
        }
    }
}

/// **The commit index** (a text cache: no length or count field). Its numeric fields at their maxima parse -- they are compared, never
/// used to size anything -- and a line with an absurd number of fields is refused; nothing in it allocates from a claim.
fn commit_index() {
    use crate::commit_index::{parse, serialize};
    let mut text = serialize(&crate::commit_index::CommitIndex::default());
    let hash = "0".repeat(64);
    text.extend_from_slice(
        format!(
            "a.txt\t{}\t{}\t{}\t{}\t1\t{hash}\n",
            u64::MAX,
            i64::MAX,
            u32::MAX,
            u32::MAX
        )
        .as_bytes(),
    );
    let parsed = parse(&text);
    assert!(parsed.is_some(), "numeric maxima are data: the line parses");
    let mut many = serialize(&crate::commit_index::CommitIndex::default());
    many.extend_from_slice("a.txt".as_bytes());
    for _ in 0..100_000 {
        many.extend_from_slice(b"\t1");
    }
    many.push(b'\n');
    assert!(
        parse(&many).is_none(),
        "a line with 100,000 fields is refused"
    );
}

/// RFC 160 F3, the table: every framed reader in this file's list (`refs/tests/hostile_lengths.rs` runs the two `refs` ones).
#[test]
fn every_framed_reader_treats_a_partial_frame_before_a_sound_frame_as_damage() {
    for format in formats() {
        check_partial_frames(&format);
    }
}

hostile_case!(hostile_length_container_frame, container_frame);
hostile_case!(hostile_length_object_index, object_index);
hostile_case!(hostile_length_wal, wal);
hostile_case!(hostile_length_trust_key, trust_key);
hostile_case!(hostile_length_trust_policy, trust_policy);
hostile_case!(hostile_length_author_key, author_key);
hostile_case!(hostile_length_received_index, received_index);
hostile_case!(hostile_length_generation, generation_file);
hostile_case!(hostile_length_verified_blocks, verified_blocks_record);
hostile_case!(hostile_length_lifecycle_cache, lifecycle_cache);
hostile_case!(hostile_length_commit_index, commit_index);

/// **RFC 160 F3, fixed width (and bounded width): a header no record of the format could have is malformed, whatever follows -- even
/// when nothing does.** A torn append leaves a **correct** header, so for the object index (its entry body is exactly the fixed
/// width), the generation file (one byte) and the two key formats (a `u16`-prefixed key id and a 32-byte key: at most 65,569 bytes)
/// a header claiming another length is not a prefix of a record. Alone, with nothing behind it, it is a failed item and **not** a
/// tail: the "ambiguous case" of a lone damaged frame is unambiguous here.
/// **Perturb:** remove the width check from the index reader (or the bound from a key reader): that format's row goes red.
#[test]
fn a_header_no_record_of_the_format_could_have_is_malformed_even_alone() {
    for format in formats() {
        if !["object index", "generation", "trust key", "author key"].contains(&format.name) {
            continue;
        }
        let valid = (format.valid)();
        for claimed in [1_u64 << 62, u64::MAX, 65_570_u64 + valid.len() as u64] {
            let seen = (format.decode)(&claiming(&valid, format.pre, claimed))
                .expect("decodes without an error");
            assert_eq!(
                (seen.records, seen.failed, seen.trailing_partial_bytes),
                (0, 1, 0),
                "{}: a lone header claiming {claimed} bytes is malformed, not a torn tail: {seen:?}",
                format.name
            );
        }
    }
}

/// **`ByteCursor::bounded_capacity`**: `min(count, remaining bytes / smallest element)`, never more than what is left could hold, and
/// it shrinks as the cursor advances.
/// **Perturb:** return `count`: the first assertion goes red.
#[test]
fn a_cursors_bounded_capacity_is_what_the_remaining_bytes_could_hold() {
    use crate::foundation::byte_cursor::ByteCursor;
    let bytes = [0_u8; 10];
    let mut cursor = ByteCursor::new(&bytes);
    assert_eq!(
        cursor.bounded_capacity(1 << 32, 2),
        5,
        "10 bytes hold at most 5 two-byte elements"
    );
    assert_eq!(
        cursor.bounded_capacity(3, 2),
        3,
        "a small honest count is not reduced"
    );
    assert_eq!(
        cursor.bounded_capacity(usize::MAX, 0),
        10,
        "a zero element size is treated as one byte"
    );
    cursor.read_exact(6).expect("six bytes");
    assert_eq!(
        cursor.bounded_capacity(1 << 32, 2),
        2,
        "the bound follows the cursor"
    );
    cursor.read_exact(4).expect("the rest");
    assert_eq!(cursor.bounded_capacity(1 << 32, 2), 0, "nothing is left");
}

/// **The one registry** (0.49.0 step 5, D11/P4, `014-review.md:290`: "P4 discovers its formats
/// instead of listing them") **every format-completeness check in this suite reads, instead of each
/// keeping its own separate hand list** -- before this round, [`the_suite_covers_every_format`] and
/// `runaway_guards.rs::every_format_has_a_runaway_guard_case` each carried their own copy, which could
/// drift from each other the same way they could each drift from [`formats`]/`all_formats`. A format
/// with no row here is simply not checked for completeness at all; a row with no matching
/// `hostile_case!` fails [`the_suite_covers_every_format`] by name. `pointer index` and `ref
/// container` are the two also-framed formats whose replay types are private to `refs` (their own
/// `hostile_case!`s live in `refs/tests/hostile_lengths.rs`); the rest are above. **Add a row here**,
/// and its case, in the round that adds a record format with a length or a count -- not two rows in
/// two files.
pub(crate) const FRAMED_FORMAT_CASES: &[(&str, &str)] = &[
    ("container frame", "hostile_length_container_frame"),
    ("object index", "hostile_length_object_index"),
    ("WAL", "hostile_length_wal"),
    ("trust index: key entries", "hostile_length_trust_key"),
    (
        "trust index: policy snapshots",
        "hostile_length_trust_policy",
    ),
    ("author-key index", "hostile_length_author_key"),
    ("pointer index", "hostile_length_pointer_index"),
    ("received index", "hostile_length_received_index"),
    ("ref container", "hostile_length_ref_container"),
    ("lifecycle cache", "hostile_length_lifecycle_cache"),
    ("verified-blocks record", "hostile_length_verified_blocks"),
    ("commit index", "hostile_length_commit_index"),
    ("generation file", "hostile_length_generation"),
];

/// **Control**: add a row to [`FRAMED_FORMAT_CASES`] with no matching `hostile_case!` (a dummy
/// fourteenth format), and this test fails, naming it -- the registry alone is not self-enforcing
/// (nothing stops an entry from being added without its case), which is exactly why this check
/// still runs rather than trusting the registry's own existence. Verified by hand this round: adding
/// `("dummy fourteenth format", "hostile_length_dummy_fourteenth")` made this test fail, naming it
/// exactly; reverted immediately after.
#[test]
fn the_suite_covers_every_format() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut sources = String::new();
    for file in [
        "test_gates/hostile_lengths.rs",
        "refs/tests/hostile_lengths.rs",
    ] {
        sources
            .push_str(&std::fs::read_to_string(root.join(file)).expect("reading a suite source"));
    }
    for (format, case) in FRAMED_FORMAT_CASES {
        assert!(
            sources.contains(&format!("hostile_case!({case},")),
            "{format}: no hostile-length case `{case}`"
        );
    }
}
