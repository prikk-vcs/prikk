//! 0.54.0 B1 (RFC 164 Rule E, a second consumption site): a stored `Block` or `RefState` that
//! nothing in committed state reaches has its publication trust check skipped -- reported as a
//! counted, non-blocking `unreached_objects` warning instead of a `publication_trust_issues`
//! failure. `verify/tests/reachability.rs` already covers Rule E's original consumption site (a
//! dangling *reference*); this file covers the new one (the object itself).
//!
//! **Crafted shapes, from the step-2 B design report's own table (`verify/reachability.rs`'s
//! module doc gives the same roots-and-expansion rule these rely on):**
//! - shape 1 (`a_block_reached_only_through_a_received_pointer_still_fails_trust`): a received
//!   pointer's own `target_object_id` names an untrusted Block directly.
//! - shape 2 (`a_block_reached_only_through_a_tag_still_fails_trust`): a tag's own
//!   `target_block_id` (via the Tag object's own `target_block_id`, reached from the tag ref's
//!   `target_object_id`) names an untrusted Block.
//! - shape 3 / the review's required continuing-lineage addition
//!   (`a_superseding_import_continuing_the_earlier_lineage_still_fails_trust`): a
//!   `previous_ref_state_id` chain keeps an earlier, untrusted lineage reached.
//! - shape 4 (piggybacking an untrusted object onto an already-reached one's own fields) is not
//!   independently testable: objects are content-addressed, so forging a different, trusted
//!   object's own `parent_block_ids`/`patch_ids`/etc. to list an attacker's id without changing
//!   that trusted object's own id would need a hash collision. Not built.
//! - shape 5 (a cycle in `previous_ref_state_id`/`parent_block_ids`) is likewise not constructible
//!   against content-addressed ids without a hash collision -- each id in a would-be two-object
//!   cycle depends on the other's id, which is circular before either object can exist. The
//!   termination guarantee the shape would have exercised is already provided by the frontier
//!   loop's own `if reached.contains(&id) { continue; }` dedup
//!   (`verify/reachability.rs`, confirmed read this round), which this file does not re-test.

use prikk_error::Result;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, MerkleRoot, ObjectEnvelope, ObjectId, ObjectType,
    RefKind, RefStatePayload, RefUpdatePayload, TagPayload,
};

use super::super::reachability::compute_reachable_object_ids;
use super::assert_unreached_object;
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
use crate::test_gates::test_support::{
    rollback_author_signature, sample_object_id, unique_temp_dir,
};
use crate::{
    DEFAULT_ACTIVE_NAME, FileObjectStore, ObjectReader, ObjectWriter, RefPublication, RefStore,
    RepositoryLayout, Wal, add_trusted_maintainer, maintainer_signature, verify_repository,
};

fn signer(discriminant: u8) -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed(
        format!("unreached-objects-fixture-{discriminant}"),
        &[discriminant; 32],
    )
}

fn public_key_hex(signer: &Ed25519MaintainerSigner) -> String {
    signer
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn adopt(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) -> Result<()> {
    add_trusted_maintainer(layout, signer.key_id(), &public_key_hex(signer))?;
    Ok(())
}

fn signed(
    object_type: ObjectType,
    schema_version: u32,
    payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> Result<ObjectEnvelope> {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, payload);
    let id = envelope.object_id();
    envelope.add_signature(maintainer_signature(signer, object_type, id)?)?;
    Ok(envelope)
}

/// A plain root Block (no missing references -- this file is about trust, not damage/remnant
/// classification), signed by `block_signer`.
fn write_root_block(
    store: &mut FileObjectStore,
    block_signer: &Ed25519MaintainerSigner,
    distinguishing_root: u8,
) -> Result<ObjectId> {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: MerkleRoot([distinguishing_root; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let envelope = signed(
        ObjectType::Block,
        2,
        payload.to_canonical_bytes()?,
        block_signer,
    )?;
    store.write_object(&envelope)
}

/// Publishes a local branch at `target_object_id`, signed by `ref_signer`. Returns the branch's
/// own RefState id.
fn publish_branch(
    layout: &RepositoryLayout,
    ref_name: &str,
    target_object_id: ObjectId,
    ref_signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    publish_ref(
        layout,
        ref_name,
        RefKind::Branch,
        None,
        target_object_id,
        ref_signer,
    )
}

/// Writes (never publishes) a RefState+RefUpdate pair for `ref_name` naming `target_object_id`,
/// with an optional `previous_ref_state_id` chain link, signed by `ref_signer`. Returns the
/// RefState's own id. Shared by `publish_branch` (a real local publish) and the received-pointer
/// helpers below (which never call `RefStore::publish` -- a received ref is never local).
fn build_ref_state_and_update(
    ref_name: &str,
    kind: RefKind,
    previous_ref_state_id: Option<ObjectId>,
    target_object_id: ObjectId,
    ref_signer: &Ed25519MaintainerSigner,
) -> Result<(ObjectEnvelope, ObjectEnvelope, ObjectId)> {
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state = signed(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes()?,
        ref_signer,
    )?;
    let ref_state_id = ref_state.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: target_object_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: ref_signer.key_id().to_string(),
    };
    let ref_update = signed(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes()?,
        ref_signer,
    )?;
    Ok((ref_state, ref_update, ref_state_id))
}

fn publish_ref(
    layout: &RepositoryLayout,
    ref_name: &str,
    kind: RefKind,
    previous_ref_state_id: Option<ObjectId>,
    target_object_id: ObjectId,
    ref_signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    let (ref_state, ref_update, ref_state_id) = build_ref_state_and_update(
        ref_name,
        kind,
        previous_ref_state_id,
        target_object_id,
        ref_signer,
    )?;
    RefStore::new(layout.clone()).publish(&RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: previous_ref_state_id,
        ref_state,
        ref_update,
    })?;
    Ok(ref_state_id)
}

/// Writes a received pointer for `ref_name` naming `target_object_id`, with an optional
/// `previous_ref_state_id` chain link, signed by `ref_signer`. The RefState object is written
/// directly (a received pointer is never a local publish); only `write_received_pointer` records
/// the received-index entry.
fn write_received_pointer(
    layout: &RepositoryLayout,
    objects: &mut FileObjectStore,
    ref_name: &str,
    previous_ref_state_id: Option<ObjectId>,
    target_object_id: ObjectId,
    ref_signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    let (ref_state, _ref_update, ref_state_id) = build_ref_state_and_update(
        ref_name,
        RefKind::Branch,
        previous_ref_state_id,
        target_object_id,
        ref_signer,
    )?;
    objects.write_object(&ref_state)?;
    crate::received::write_received_pointer(layout, ref_name, ref_state_id)?;
    Ok(ref_state_id)
}

/// Publishes `tags/<name>` through a real `TagPayload` object (`ObjectType::Tag`), matching the
/// real two-hop shape `prikk tag create` writes (`tag.rs`'s own `target_object_id` ->
/// `ObjectType::Tag` -> `target_block_id`) -- not `verify/tests/local_tag_trust.rs`'s own
/// simplified one-hop fixture, which skips the Tag object entirely for a different test's own
/// narrower purpose. `tag_signer` signs the Tag object and the tag ref; `block_signer` is who
/// signed `target_block_id` itself (the one under test for shape 2).
fn publish_tag(
    layout: &RepositoryLayout,
    objects: &mut FileObjectStore,
    name: &str,
    target_block_id: ObjectId,
    tag_signer: &Ed25519MaintainerSigner,
) -> Result<ObjectId> {
    let (digest, count) = compute_patch_set_digest_and_count_from_block(objects, target_block_id)?;
    let tag_payload = TagPayload {
        name: format!("tags/{name}"),
        target_block_id,
        message: None,
        created_at: 0,
        author_key_id: tag_signer.key_id().to_string(),
        patch_set_digest: digest,
        patch_count: count,
    };
    let tag_envelope = signed(
        ObjectType::Tag,
        1,
        tag_payload.to_canonical_bytes()?,
        tag_signer,
    )?;
    let tag_object_id = objects.write_object(&tag_envelope)?;
    publish_ref(
        layout,
        &format!("tags/{name}"),
        RefKind::Tag,
        None,
        tag_object_id,
        tag_signer,
    )?;
    Ok(tag_object_id)
}

/// The baseline: an untrusted Block nothing references at all is a warning, not a trust failure.
#[test]
fn an_unreached_untrusted_block_is_a_warning_not_a_trust_failure() -> Result<()> {
    let root = unique_temp_dir("unreached-baseline-unreached");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x01)?;
    adopt(&layout, &trusted)?;
    let untrusted = signer(0x02)?;

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &untrusted, 0xA0)?;

    let report = verify_repository(&layout)?;
    assert!(
        !report.has_publication_trust_issues(),
        "an unreached object must never fail publication trust: {:?}",
        report.publication_trust_issues
    );
    assert_unreached_object(&report, block, ObjectType::Block);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The companion control: the identical untrusted Block, but reached by a branch -- must still
/// fail trust exactly as before this round, and must not appear in `unreached_objects`.
#[test]
fn a_reached_untrusted_block_via_a_branch_still_fails_trust() -> Result<()> {
    let root = unique_temp_dir("unreached-baseline-reached");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x03)?;
    adopt(&layout, &trusted)?;
    let untrusted = signer(0x04)?;

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &untrusted, 0xA1)?;
    publish_branch(&layout, "heads/main", block, &trusted)?;

    let report = verify_repository(&layout)?;
    assert!(
        report.has_publication_trust_issues(),
        "a reached, untrusted Block must still fail publication trust"
    );
    assert!(
        !report
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == block),
        "a reached object must never also be reported unreached: {:?}",
        report.unreached_objects
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Shape 1: a received pointer's own `target_object_id` names an untrusted Block directly --
/// still fails, since a received ref's current RefState is itself a root.
#[test]
fn a_block_reached_only_through_a_received_pointer_still_fails_trust() -> Result<()> {
    let root = unique_temp_dir("unreached-shape1-received");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x05)?;
    adopt(&layout, &trusted)?;
    let untrusted = signer(0x06)?;

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &untrusted, 0xA2)?;
    write_received_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        block,
        &trusted,
    )?;

    let report = verify_repository(&layout)?;
    assert!(
        report.has_publication_trust_issues(),
        "a Block reached only through a received pointer must still fail publication trust"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Shape 2: a tag's own target (via the real `ObjectType::Tag` -> `target_block_id` two-hop
/// shape) names an untrusted Block -- still fails, since a tag ref is a root too.
#[test]
fn a_block_reached_only_through_a_tag_still_fails_trust() -> Result<()> {
    let root = unique_temp_dir("unreached-shape2-tag");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x07)?;
    adopt(&layout, &trusted)?;
    let untrusted = signer(0x08)?;

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &untrusted, 0xA3)?;
    publish_tag(&layout, &mut store, "v1", block, &trusted)?;

    let report = verify_repository(&layout)?;
    assert!(
        report.has_publication_trust_issues(),
        "a Block reached only through a tag must still fail publication trust"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Shape 3, and the review's own required continuing-lineage addition: a superseding import whose
/// own `previous_ref_state_id` *continues* the earlier lineage keeps the earlier, untrusted
/// RefState and Block reached through the chain -- `verify` must still fail trust on them, exactly
/// as before this round. Contrast with
/// `a_sound_import_from_a_different_lineage_replaces_an_untrusted_one_and_verify_passes_with_a_warning`
/// below, where the second import does *not* continue the chain.
#[test]
fn a_superseding_import_continuing_the_earlier_lineage_still_fails_trust() -> Result<()> {
    let root = unique_temp_dir("unreached-continuing-lineage");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x09)?;
    adopt(&layout, &trusted)?;
    let untrusted = signer(0x0A)?;

    let mut store = FileObjectStore::new(layout.clone());
    let first_block = write_root_block(&mut store, &untrusted, 0xA4)?;
    let first_ref_state = write_received_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        first_block,
        &trusted,
    )?;

    // Confirm the fixture's first half in isolation: before any superseding import, the
    // unadopted-key Block fails trust (the 0.52.0 shape).
    let before = verify_repository(&layout)?;
    assert!(
        before.has_publication_trust_issues(),
        "fixture bug: the unadopted-key import must fail trust before replacement"
    );

    let second_block = write_root_block(&mut store, &trusted, 0xA5)?;
    write_received_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        Some(first_ref_state), // continues the earlier lineage
        second_block,
        &trusted,
    )?;

    let after = verify_repository(&layout)?;
    assert!(
        after.has_publication_trust_issues(),
        "a continuing lineage must keep the earlier, untrusted RefState/Block reached, and must \
         still fail trust: {:?}",
        after.publication_trust_issues
    );
    assert!(
        !after
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == first_block
                || unreached.object_id == first_ref_state),
        "the continuing lineage's own earlier objects must not be reported unreached: {:?}",
        after.unreached_objects
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The 0.52.0 case, end to end: an import under a key this repository never adopts fails `verify`;
/// a later, sound import from a genuinely *different* lineage (no `previous_ref_state_id` link)
/// replaces the received pointer; `verify` now passes, with the earlier RefState and Block
/// reported as unreached objects -- a warning, not a trust failure. The bytes stay; nothing is
/// deleted.
#[test]
fn a_sound_import_from_a_different_lineage_replaces_an_untrusted_one_and_verify_passes_with_a_warning()
-> Result<()> {
    let root = unique_temp_dir("unreached-052-end-to-end");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x0B)?;
    adopt(&layout, &trusted)?;
    let untrusted = signer(0x0C)?;

    let mut store = FileObjectStore::new(layout.clone());
    let first_block = write_root_block(&mut store, &untrusted, 0xA6)?;
    let first_ref_state = write_received_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        first_block,
        &trusted,
    )?;

    let before = verify_repository(&layout)?;
    assert!(
        before.has_publication_trust_issues(),
        "the unadopted-key import must fail publication trust before any replacement"
    );

    let second_block = write_root_block(&mut store, &trusted, 0xA7)?;
    write_received_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None, // a genuinely different lineage -- no previous_ref_state_id link
        second_block,
        &trusted,
    )?;

    let after = verify_repository(&layout)?;
    assert!(
        !after.has_publication_trust_issues(),
        "once the pointer is replaced by an unrelated, sound lineage, verify must pass: {:?}",
        after.publication_trust_issues
    );
    assert_unreached_object(&after, first_block, ObjectType::Block);
    assert_unreached_object(&after, first_ref_state, ObjectType::RefState);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// An independent reachability walk, written fresh in this test (never calling
/// `compute_reachable_object_ids`'s own expansion logic), agreeing with the real implementation on
/// every object id the fixture constructs: a branch's own RefState and target Block, a tag's own
/// RefState, Tag object and target Block, a received pointer's own RefState and target Block, a
/// queued patch's own referenced Blob, and one wholly unreferenced control Block.
#[test]
fn an_independent_walk_agrees_with_compute_reachable_object_ids_on_every_fixture_object()
-> Result<()> {
    let root = unique_temp_dir("unreached-independent-walk");
    let layout = RepositoryLayout::init(root.clone())?;
    let trusted = signer(0x0D)?;
    adopt(&layout, &trusted)?;

    let mut store = FileObjectStore::new(layout.clone());

    let branch_block = write_root_block(&mut store, &trusted, 0xB0)?;
    let branch_ref_state = publish_branch(&layout, "heads/main", branch_block, &trusted)?;

    let tag_block = write_root_block(&mut store, &trusted, 0xB1)?;
    let tag_object_id = publish_tag(&layout, &mut store, "v1", tag_block, &trusted)?;
    // Re-derive the tag ref's own RefState id by reading it back (publish_tag doesn't return it).
    let tag_ref_state = {
        let pointers = RefStore::new(layout.clone()).list_ref_pointers()?;
        let Some(pointer) = pointers
            .iter()
            .find(|pointer| pointer.ref_name == "tags/v1")
        else {
            panic!("the tag ref must be published");
        };
        pointer.ref_state_id
    };

    let received_block = write_root_block(&mut store, &trusted, 0xB2)?;
    let received_ref_state = write_received_pointer(
        &layout,
        &mut store,
        "remotes/heads/dev",
        None,
        received_block,
        &trusted,
    )?;

    // A queued patch, referencing a blob nothing else names -- the one root kind that reaches a
    // blob directly rather than through a Block.
    crate::write_active_ref_metadata(&layout, "heads/main")?;
    let queued_blob = sample_object_id("unreached-independent-walk-blob");
    let patch_payload = prikk_object::PatchPayload {
        operations: vec![prikk_object::Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: prikk_object::OperationKind::CreateFile(prikk_object::CreateFile {
                path: "a.txt".to_string(),
                node_id: prikk_object::NodeId::from_bytes([0x72; 32]),
                blob_id: queued_blob,
                mode: 0o100_644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: prikk_object::PatchPurpose::Normal,
        message: None,
    };
    let mut patch_envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, patch_payload.to_canonical_bytes()?);
    patch_envelope.add_signature(rollback_author_signature())?;
    Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).append_patch(&patch_envelope)?;

    let control_block = write_root_block(&mut store, &trusted, 0xB3)?;

    // `compute_reachable_object_ids` takes the default active session's own already-decoded
    // replay from its caller (confirmed from source this round: passing `None` skips the default
    // session's own queued-patch roots entirely, via a `None => continue` in its own loop) --
    // this test's own fixture queues against the default session, so it must supply one, exactly
    // as `verify_objects` itself does.
    let default_replay = Wal::for_layout(&layout, DEFAULT_ACTIVE_NAME).replay()?;
    let real = compute_reachable_object_ids(&layout, &store, Some(&default_replay))?;

    // An independent walk, written fresh: roots are every local ref's and every received ref's
    // own RefState id, plus every queued patch's own referenced blob ids (read directly from the
    // active WAL here, not through any shared decode helper this test doesn't already use
    // elsewhere). Expansion matches `reachability.rs`'s own module doc, decoded independently via
    // each payload type's own `decode_canonical`.
    let mut reached: std::collections::BTreeSet<ObjectId> = std::collections::BTreeSet::new();
    let mut frontier: Vec<ObjectId> = Vec::new();
    for pointer in RefStore::new(layout.clone()).list_ref_pointers()? {
        frontier.push(pointer.ref_state_id);
    }
    for pointer in crate::received::list_received_pointers(&layout)? {
        frontier.push(pointer.ref_state_id);
    }
    // The one piece reused from the real implementation's own low-level decoder, rather than
    // reimplemented: extracting a queued patch's own referenced blob ids from its raw canonical
    // bytes is a mechanical codec detail, not part of the roots-and-expansion orchestration this
    // test independently re-derives below.
    for record in &default_replay.records {
        let blob_ids = crate::patch_replay::decode::patch_referenced_blob_ids(
            &record.envelope.canonical_payload,
            record.envelope.schema_version,
        )?;
        reached.extend(blob_ids);
    }
    while let Some(id) = frontier.pop() {
        if reached.contains(&id) {
            continue;
        }
        let Some(envelope) = store.read_object(id)? else {
            continue;
        };
        reached.insert(id);
        match envelope.object_type {
            ObjectType::RefState => {
                let payload = RefStatePayload::decode_canonical(
                    &envelope.canonical_payload,
                    envelope.schema_version,
                )?;
                frontier.push(payload.target_object_id);
                if let Some(previous) = payload.previous_ref_state_id {
                    frontier.push(previous);
                }
            }
            ObjectType::Tag => {
                let payload = TagPayload::decode_canonical(&envelope.canonical_payload)?;
                frontier.push(payload.target_block_id);
            }
            ObjectType::Block => {
                let payload = BlockPayload::decode_canonical(&envelope.canonical_payload)?;
                frontier.extend(payload.parent_block_ids.iter().copied());
                frontier.extend(payload.patch_ids.iter().copied());
                if let Some(snapshot) = payload.snapshot_blob_ref {
                    frontier.push(snapshot);
                }
                if let Some(mainline) = payload.mainline_parent_id {
                    frontier.push(mainline);
                }
                if let Some(baseline) = payload.merge_baseline_block_id {
                    frontier.push(baseline);
                }
            }
            _ => {}
        }
    }

    let tracked = [
        ("branch RefState", branch_ref_state, true),
        ("branch target Block", branch_block, true),
        ("tag ref RefState", tag_ref_state, true),
        ("Tag object", tag_object_id, true),
        ("tag target Block", tag_block, true),
        ("received RefState", received_ref_state, true),
        ("received target Block", received_block, true),
        ("queued patch blob", queued_blob, true),
        ("unreferenced control Block", control_block, false),
    ];
    for (label, id, expected_reachable) in tracked {
        assert_eq!(
            real.contains(&id),
            expected_reachable,
            "real implementation disagrees with the fixture's own expectation for {label} ({id})"
        );
        assert_eq!(
            reached.contains(&id),
            expected_reachable,
            "independent walk disagrees with the fixture's own expectation for {label} ({id})"
        );
        assert_eq!(
            real.contains(&id),
            reached.contains(&id),
            "the real implementation and this independent walk disagree on {label} ({id})"
        );
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
