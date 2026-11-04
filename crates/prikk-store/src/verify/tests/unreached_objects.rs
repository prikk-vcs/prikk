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
    BlobKind, BlobPayload, BlockKind, BlockPayload, CanonicalEncode, CreateFile, MerkleRoot,
    NodeId, ObjectEnvelope, ObjectId, ObjectType, Operation, OperationKind, PatchPayload,
    PatchPurpose, RecognitionClaimPayload, RefKind, RefStatePayload, RefUpdatePayload, TagPayload,
};

use super::super::reachability::{
    compute_reachable_object_ids, compute_reachable_object_ids_excluding_pending_roots_for_test,
};
use super::assert_unreached_object;
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
use crate::test_gates::test_support::{
    rollback_author_signature, sample_object_id, signed_block, signed_ref_state_envelope,
    signed_ref_update_envelope, unique_temp_dir,
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

// --- 0.56.0 step 3h (#9): `Patch` and `Blob` join this same `unreached_objects` list, over the
// design review v2 A1 ruling's own five objects (`.git-exclude/reviewed/0.56.0-step-2-design-
// review-v2.md`'s own table). Fixture shapes below are lightly adapted from `patch_exchange::
// tests` and `show::tests` (objects 1-4) and `bundle::tests`'s own real pass-2-truncation fixture
// (object 5, external review 037 §1.3's shape) -- not rebuilt from scratch.

/// A `Blob` object, written directly (the AUTHOR/MAINTAINER signing boundary has no concept of a
/// Blob signature -- `ObjectEnvelope::unsigned` is the real, production shape for every Blob this
/// codebase ever writes).
fn write_blob_direct(store: &mut FileObjectStore, content: &[u8]) -> Result<ObjectId> {
    let payload = BlobPayload {
        blob_kind: BlobKind::Text,
        content: content.to_vec(),
        declared_size: content.len() as u64,
    };
    let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, payload.to_canonical_bytes()?);
    store.write_object(&envelope)
}

/// A held, AUTHOR-signed `Patch` with one `CreateFile` operation naming `blob_id` -- written
/// directly to the object store, never through the WAL (so it reads `pending`/`unclaimed`/
/// `sealed`, never `queued`), matching `show::tests::write_pending_patch`'s own shape.
fn write_patch_naming_blob(
    store: &mut FileObjectStore,
    blob_id: ObjectId,
    node_byte: u8,
) -> Result<ObjectId> {
    let payload = PatchPayload {
        operations: vec![Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateFile(CreateFile {
                path: "a.txt".to_string(),
                node_id: NodeId::from_bytes([node_byte; 32]),
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
    envelope.add_signature(rollback_author_signature())?;
    store.write_object(&envelope)
}

/// An unsigned `RecognitionClaim` naming `patch_ids` sealed into `block_id` -- membership only
/// (`patch_ids_pending`'s own claim scan never checks a claim's signature, matching 3g's own
/// reasoning: "a claim's own signature outcome is irrelevant to this question"), matching
/// `show::tests::write_claim`'s own unsigned shape used for the identical purpose there.
fn write_claim_direct(
    store: &mut FileObjectStore,
    block_id: ObjectId,
    patch_ids: Vec<ObjectId>,
) -> Result<ObjectId> {
    let payload = RecognitionClaimPayload {
        block_id,
        patch_ids,
        parent_block_ids: Vec::new(),
    };
    let envelope = ObjectEnvelope::unsigned(
        ObjectType::RecognitionClaim,
        1,
        payload.to_canonical_bytes()?,
    );
    store.write_object(&envelope)
}

/// A `Block` naming `patch_ids`, sealed over `parent_block_ids` -- the shape
/// [`write_root_block`] above does not cover (it always writes an empty-patch root).
fn write_block_naming_patches(
    store: &mut FileObjectStore,
    parent_block_ids: Vec<ObjectId>,
    patch_ids: Vec<ObjectId>,
    distinguishing_root: u8,
) -> Result<ObjectId> {
    let payload = BlockPayload {
        parent_block_ids,
        kind: BlockKind::Normal,
        patch_ids,
        state_merkle_root: MerkleRoot([distinguishing_root; 32]),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let envelope = ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes()?);
    store.write_object(&envelope)
}

/// Object 1 ("accepted, with a held claim"): #9's own rule ("reached by no tip, by no WAL record,
/// and by no pending patch -- with the blobs they name") means a pending patch, and the blob its
/// own operation names, must both stay out of `unreached_objects` -- `verify` must agree with
/// `sync pending`/`show`'s own `Pending` classification that this object has somewhere real for an
/// operator to act on it, not an orphan.
#[test]
fn an_accepted_patch_with_a_held_claim_is_not_counted_unreached() -> Result<()> {
    let root = unique_temp_dir("unreached-3h-object1-pending");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let blob_id = write_blob_direct(&mut store, b"object 1 pending blob\n")?;
    let patch_id = write_patch_naming_blob(&mut store, blob_id, 0xD1)?;
    let prospective_block_id = sample_object_id("unreached-3h-object1-prospective-block");
    write_claim_direct(&mut store, prospective_block_id, vec![patch_id])?;

    let report = verify_repository(&layout)?;
    assert!(
        !report
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == patch_id),
        "a pending patch (held claim, no sealing block) must never be counted unreached: {:?}",
        report.unreached_objects
    );
    assert!(
        !report
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == blob_id),
        "a pending patch's own referenced blob must be reached through it, not counted unreached: \
         {:?}",
        report.unreached_objects
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Red control (the handoff's own instruction: disable the new addition the round judges most
/// novel): remove exactly the new pending-patch root, nothing else, and confirm object 1's own
/// patch and blob regress to the pre-3h gap -- wrongly absent from the reachable set, which is
/// exactly what would make `verify` count them unreached again, were this addition missing.
#[test]
fn disabling_the_pending_patch_root_reproduces_the_pre_3h_gap_for_object_1() -> Result<()> {
    let root = unique_temp_dir("unreached-3h-red-control");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let blob_id = write_blob_direct(&mut store, b"red control pending blob\n")?;
    let patch_id = write_patch_naming_blob(&mut store, blob_id, 0xD9)?;
    let prospective_block_id = sample_object_id("unreached-3h-red-control-prospective-block");
    write_claim_direct(&mut store, prospective_block_id, vec![patch_id])?;

    // Normal run: both reached, through the new pending-patch root.
    let reachable = compute_reachable_object_ids(&layout, &store, None)?;
    assert!(
        reachable.contains(&patch_id),
        "the pending patch must be reached normally"
    );
    assert!(
        reachable.contains(&blob_id),
        "its own referenced blob must be reached normally"
    );

    // With exactly the pending-patch root disabled: the old, wrong gap -- neither is reached.
    let without_pending_root =
        compute_reachable_object_ids_excluding_pending_roots_for_test(&layout, &store, None)?;
    assert!(
        !without_pending_root.contains(&patch_id),
        "disabling the pending-patch root must reproduce the pre-3h gap: the pending patch \
         wrongly unreached"
    );
    assert!(
        !without_pending_root.contains(&blob_id),
        "disabling the pending-patch root must reproduce the pre-3h gap: its blob wrongly \
         unreached too"
    );

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Object 2 ("accepted, no claim, hand-made only"): with no held claim at all, nothing makes this
/// patch (or its blob) reachable, so both must be counted -- the exact gap review 037 §1.3 asked
/// #9 to close for a claimless accepted patch.
#[test]
fn a_claimless_accepted_patch_is_counted_unreached_with_its_blob() -> Result<()> {
    let root = unique_temp_dir("unreached-3h-object2-unclaimed");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let blob_id = write_blob_direct(&mut store, b"object 2 unclaimed blob\n")?;
    let patch_id = write_patch_naming_blob(&mut store, blob_id, 0xD2)?;

    let report = verify_repository(&layout)?;
    assert_unreached_object(&report, patch_id, ObjectType::Patch);
    assert_unreached_object(&report, blob_id, ObjectType::Blob);
    // The handoff's own explicit wording: "verify's object_type values 'patch' and 'blob'" --
    // confirmed here at the value level (`ObjectType`'s own `Display`), which is all the CLI JSON
    // printer (`output/verification.rs`) ever reads: `escape_json_string(&unreached.object_type.
    // to_string())`, with no type-specific match arm to add or get wrong.
    for unreached in &report.unreached_objects {
        match unreached.object_type {
            ObjectType::Patch => assert_eq!(unreached.object_type.to_string(), "patch"),
            ObjectType::Blob => assert_eq!(unreached.object_type.to_string(), "blob"),
            other => panic!("unexpected unreached object type in this fixture: {other:?}"),
        }
    }

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Object 3 ("rolled back, its Block still held"): a received, non-fast-forward "rollback" import
/// (A2's own class; built here, as 2A v2's own R5 measurement did, over `received/write_received_
/// pointer` directly -- the delivered RefState's own `previous_ref_state_id` reflects the
/// *sender's* history, never a link back to the receiver's superseded tip, which is exactly why
/// the old tip genuinely stops being reached rather than staying reachable through a continuing
/// chain; confirmed first in isolation below, matching `a_sound_import_from_a_different_lineage_
/// replaces_an_untrusted_one_and_verify_passes_with_a_warning`'s own identical write mechanism)
/// landing an ancestor (`root_block_id`) over a forward tip (`child_block_id`, whose own parent
/// *is* `root_block_id` -- a genuine rollback, not an unrelated lineage) does not unwrite the block
/// it moved away from, or the patch (and blob) that block names -- all three must now be counted
/// unreached, where before this round only the Block was.
#[test]
fn a_rolled_back_blocks_patch_and_blob_are_both_counted_unreached_alongside_its_block() -> Result<()>
{
    let root = unique_temp_dir("unreached-3h-object3-rollback");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let blob_id = write_blob_direct(&mut store, b"object 3 rollback blob\n")?;
    let patch_id = write_patch_naming_blob(&mut store, blob_id, 0xD3)?;
    let root_block_id =
        store.write_object(&signed_block(BlockKind::Root, Vec::new(), Vec::new(), None))?;
    let child_block_id =
        write_block_naming_patches(&mut store, vec![root_block_id], vec![patch_id], 0xD4)?;

    let forward_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, child_block_id, 1);
    let forward_ref_state_id = store.write_object(&forward_ref_state)?;
    crate::received::write_received_pointer_for_test(
        &layout,
        "remotes/origin/heads/main",
        forward_ref_state_id,
    )?;

    let before = verify_repository(&layout)?;
    assert!(
        !before
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == patch_id
                || unreached.object_id == blob_id
                || unreached.object_id == child_block_id),
        "before any rollback, the tip's own block, patch and blob must all be reached: {:?}",
        before.unreached_objects
    );

    // The rollback landing: a fresh RefState naming the ancestor `root_block_id`, with no
    // `previous_ref_state_id` link of its own (the sender's own history for this ref name starts
    // at its own root; it never names the receiver's now-superseded `forward_ref_state_id` at
    // all). `write_received_pointer` upserts the received-index entry for this ref name in place --
    // the identical mechanism `a_sound_import_from_a_different_lineage_replaces_an_untrusted_one_
    // and_verify_passes_with_a_warning` above already relies on.
    let rollback_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, root_block_id, 1);
    let rollback_ref_state_id = store.write_object(&rollback_ref_state)?;
    crate::received::write_received_pointer_for_test(
        &layout,
        "remotes/origin/heads/main",
        rollback_ref_state_id,
    )?;

    let after = verify_repository(&layout)?;
    assert_unreached_object(&after, child_block_id, ObjectType::Block);
    assert_unreached_object(&after, patch_id, ObjectType::Patch);
    assert_unreached_object(&after, blob_id, ObjectType::Blob);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Object 4 ("after `received remove`, its Block still held"): `remove_received_pointer` deletes
/// only the pointer -- the block it named, and the patch (and blob) that block names, are all
/// still held. Before removal all three are reached (through the received pointer); after, all
/// three must be counted unreached together.
#[test]
fn a_received_removed_blocks_patch_and_blob_are_both_counted_unreached_alongside_its_block()
-> Result<()> {
    let root = unique_temp_dir("unreached-3h-object4-received-remove");
    let layout = RepositoryLayout::init(root.clone())?;
    let mut store = FileObjectStore::new(layout.clone());

    let blob_id = write_blob_direct(&mut store, b"object 4 received-remove blob\n")?;
    let patch_id = write_patch_naming_blob(&mut store, blob_id, 0xD5)?;
    let root_block_id =
        store.write_object(&signed_block(BlockKind::Root, Vec::new(), Vec::new(), None))?;
    let block_id =
        write_block_naming_patches(&mut store, vec![root_block_id], vec![patch_id], 0xD6)?;

    let received_ref_state =
        signed_ref_state_envelope("remotes/origin/heads/main", None, block_id, 1);
    let received_ref_state_id = store.write_object(&received_ref_state)?;
    crate::received::write_received_pointer_for_test(
        &layout,
        "remotes/origin/heads/main",
        received_ref_state_id,
    )?;

    let before = verify_repository(&layout)?;
    assert!(
        !before
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == patch_id
                || unreached.object_id == blob_id
                || unreached.object_id == block_id),
        "before received remove, the received block, patch and blob must all be reached: {:?}",
        before.unreached_objects
    );

    crate::remove_received_pointer(&layout, "remotes/origin/heads/main")?;

    let after = verify_repository(&layout)?;
    assert_unreached_object(&after, block_id, ObjectType::Block);
    assert_unreached_object(&after, patch_id, ObjectType::Patch);
    assert_unreached_object(&after, blob_id, ObjectType::Blob);

    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Object 5 ("a refused or crashed import's leftover"), external review 037 §1.3's exact shape:
/// `bundle import`'s own pass 2 (the write pass) already admits some real objects before a file
/// truncated between the two passes makes it refuse the whole import. Adapted from `bundle::tests
/// ::a_pass_two_file_truncated_between_passes_refuses_with_rs_message_shape_and_writes_no_pointer`
/// (the exact, already-measured "file changed between the passes" fixture that test builds) --
/// this is the same real truncation, the same real refusal, with one addition: `verify` on the
/// receiver afterward, confirming whatever pass 2 did admit before the cut is now counted.
///
/// **Widened from that fixture's own two-block history to a ten-block chain**, each block sealing
/// its own patch and blob: found empirically that the production fixture's own tiny five-object
/// history leaves nothing at all written by a half-length truncation (the cut lands inside the
/// very first object's own frame), which would make this adapted test exercise nothing. A longer
/// chain guarantees several whole (blob, patch, block) groups land before the cut, with no need to
/// know or control exactly which ones -- every patch/blob id this fixture wrote is checked below,
/// and only the ones pass 2 actually admitted are asserted against.
#[test]
#[allow(clippy::expect_used)]
fn a_pass_two_truncated_bundle_import_leaves_already_written_patches_and_blobs_counted_unreached()
-> Result<()> {
    const CHAIN_LENGTH: u8 = 10;

    let source_root = unique_temp_dir("unreached-3h-object5-source");
    let source = RepositoryLayout::init(source_root.clone())?;
    let mut source_store = FileObjectStore::new(source.clone());

    let mut patch_and_blob_ids: Vec<(ObjectId, ObjectId)> = Vec::new();
    let root_block_id =
        source_store.write_object(&signed_block(BlockKind::Root, Vec::new(), Vec::new(), None))?;
    let mut tip_block_id = root_block_id;
    for index in 0..CHAIN_LENGTH {
        let blob_content = format!("object 5 chain blob {index}\n").into_bytes();
        let blob_envelope = ObjectEnvelope::unsigned(
            ObjectType::Blob,
            1,
            BlobPayload {
                blob_kind: BlobKind::Text,
                declared_size: blob_content.len() as u64,
                content: blob_content,
            }
            .to_canonical_bytes()?,
        );
        let blob_id = source_store.write_object(&blob_envelope)?;
        let mut patch_envelope = ObjectEnvelope::unsigned(
            ObjectType::Patch,
            1,
            PatchPayload {
                operations: vec![Operation {
                    op_seq: 1,
                    op_id: None,
                    preconditions: Vec::new(),
                    kind: OperationKind::CreateFile(CreateFile {
                        path: format!("chain-{index}.txt"),
                        node_id: NodeId::from_bytes([0xE0 + index; 32]),
                        blob_id,
                        mode: 0o100_644,
                    }),
                }],
                intent: None,
                preconditions: Vec::new(),
                purpose: PatchPurpose::Normal,
                message: None,
            }
            .to_canonical_bytes()?,
        );
        patch_envelope.add_signature(rollback_author_signature())?;
        let patch_id = source_store.write_object(&patch_envelope)?;
        tip_block_id = source_store.write_object(&signed_block(
            BlockKind::Normal,
            vec![tip_block_id],
            vec![patch_id],
            None,
        ))?;
        patch_and_blob_ids.push((patch_id, blob_id));
    }

    let ref_store = RefStore::new(source.clone());
    let ref_state = signed_ref_state_envelope("heads/main", None, tip_block_id, 1);
    let ref_state_id = ref_state.object_id();
    let ref_update = signed_ref_update_envelope("heads/main", None, ref_state_id, tip_block_id, 1);
    ref_store.publish(&RefPublication {
        ref_name: "heads/main".to_string(),
        expected_previous_ref_state_id: None,
        ref_state,
        ref_update,
    })?;

    let (_, bytes) = crate::bundle::export_bundle(&source, "heads/main")?;
    let mut bundle_path = source_root.clone();
    bundle_path.push("exported.bundle");
    std::fs::write(&bundle_path, &bytes)?;
    let total_len = std::fs::metadata(&bundle_path)?.len();

    let target_root = unique_temp_dir("unreached-3h-object5-target");
    let target = RepositoryLayout::init(target_root.clone())?;

    let seam_path = bundle_path.clone();
    crate::bundle::corrupt_file_between_passes_for_test(move || {
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&seam_path)
            .expect("reopen the bundle file to truncate it");
        file.set_len(total_len / 2)
            .expect("truncate the bundle file to half its own length");
    });

    let mut source_handle = crate::bundle::BundleSource::File(std::fs::File::open(&bundle_path)?);
    let refused = crate::bundle::import_bundle(
        &target,
        &mut source_handle,
        &crate::bundle::BundleImportOptions::default_limits(),
    );
    assert!(
        refused.is_err(),
        "the truncated pass-2 re-read must refuse the whole import: {refused:?}"
    );

    let target_store = FileObjectStore::new(target.clone());
    let mut any_written = false;
    let report = verify_repository(&target)?;
    for (patch_id, blob_id) in patch_and_blob_ids {
        if target_store.read_object(patch_id)?.is_some() {
            any_written = true;
            assert_unreached_object(&report, patch_id, ObjectType::Patch);
        }
        if target_store.read_object(blob_id)?.is_some() {
            any_written = true;
            assert_unreached_object(&report, blob_id, ObjectType::Blob);
        }
    }
    assert!(
        any_written,
        "fixture bug: pass 2 must have already written at least one patch or blob before the \
         truncation point, or this test exercises nothing"
    );

    let _ = std::fs::remove_dir_all(source_root);
    let _ = std::fs::remove_dir_all(target_root);
    Ok(())
}
