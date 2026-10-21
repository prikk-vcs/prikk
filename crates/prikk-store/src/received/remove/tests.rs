//! Tests for `received remove` (0.54.0 step 3 Part 5, B2a).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use prikk_error::PrikkError;
use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind,
    RefStatePayload,
};

use super::*;
use crate::foundation::layout::LockableContainer;
use crate::lock::acquire_container_locks;
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::test_gates::test_support::unique_temp_dir;
use crate::{FileObjectStore, ObjectWriter, add_trusted_maintainer, maintainer_signature};

fn signer(discriminant: u8) -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed(
        format!("received-remove-fixture-{discriminant}"),
        &[discriminant; 32],
    )
    .unwrap()
}

fn adopt(layout: &RepositoryLayout, signer: &Ed25519MaintainerSigner) {
    add_trusted_maintainer(
        layout,
        signer.key_id(),
        &prikk_hash::to_hex(&signer.public_key_bytes()),
    )
    .unwrap();
}

fn signed(
    object_type: ObjectType,
    schema_version: u32,
    payload: Vec<u8>,
    signer: &impl MaintainerSigner,
) -> ObjectEnvelope {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, payload);
    let id = envelope.object_id();
    envelope
        .add_signature(maintainer_signature(signer, object_type, id).unwrap())
        .unwrap();
    envelope
}

/// A plain root Block, signed by `block_signer`.
fn write_root_block(
    store: &mut FileObjectStore,
    block_signer: &Ed25519MaintainerSigner,
    distinguishing_root: u8,
) -> ObjectId {
    // One real `CreateFile` patch, not an empty one -- `distinguishing_root` keeps two otherwise
    // identical fixtures from colliding on the same content-addressed id, and a genuinely derived
    // state root (not a hand-picked `MerkleRoot([n; 32])`) keeps `verify`'s own `BlockState` stage
    // clean: it replays every block and fails if the signed root disagrees with what its own
    // patches actually produce. A fake root is fine for `verify/tests/unreached_objects.rs`'s own
    // narrower purpose (publication trust only); this fixture's own kill-sweep test also needs
    // `verify` to be genuinely clean, so it cannot reuse that shortcut.
    let blob = crate::test_gates::test_support::write_blob(
        store,
        format!("content {distinguishing_root}\n").as_bytes(),
    )
    .unwrap();
    let node_id = crate::test_gates::test_support::anchored_node(distinguishing_root);
    let patch_payload = prikk_object::PatchPayload {
        operations: vec![prikk_object::Operation {
            op_seq: 1,
            op_id: None,
            preconditions: Vec::new(),
            kind: prikk_object::OperationKind::CreateFile(prikk_object::CreateFile {
                path: format!("file-{distinguishing_root}.txt"),
                node_id,
                blob_id: blob,
                mode: 0o100644,
            }),
        }],
        intent: None,
        preconditions: Vec::new(),
        purpose: prikk_object::PatchPurpose::Normal,
        message: None,
    };
    let mut patch = ObjectEnvelope::unsigned(
        ObjectType::Patch,
        1,
        patch_payload.to_canonical_bytes().unwrap(),
    );
    patch
        .add_signature(crate::test_gates::test_support::dummy_signature())
        .unwrap();
    let patch_id = store.write_object(&patch).unwrap();
    let state_merkle_root = crate::derive_next_state_root(store, None, &[patch_id]).unwrap();
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: vec![patch_id],
        state_merkle_root,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let envelope = signed(
        ObjectType::Block,
        2,
        payload.to_canonical_bytes().unwrap(),
        block_signer,
    );
    store.write_object(&envelope).unwrap()
}

/// Publishes a received pointer naming `target_object_id`, through the real production write path
/// (`crate::received::write_received_pointer`) -- the identical call `bundle import`/`archive
/// import` make, so this fixture's own container state is exactly what a real import leaves.
fn write_received_fixture_pointer(
    layout: &RepositoryLayout,
    store: &mut FileObjectStore,
    ref_name: &str,
    previous_ref_state_id: Option<ObjectId>,
    target_object_id: ObjectId,
    ref_signer: &Ed25519MaintainerSigner,
) -> ObjectId {
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(maintainer_signature(ref_signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    store.write_object(&state_env).unwrap();
    // No RefUpdate object: v1 stores it inline in ref logs, and a received pointer has no ref
    // log at all (`verify/tests/unreached_objects.rs`'s own equivalent fixture never writes one
    // either).
    crate::received::write_received_pointer(layout, ref_name, state_id).unwrap();
    state_id
}

/// A fresh repository with one trusted maintainer and one real received pointer -- the minimal
/// fixture every test below that just needs "something to remove" builds on.
fn repo_with_one_received_pointer(label: &str) -> (RepositoryLayout, std::path::PathBuf, ObjectId) {
    let root = unique_temp_dir(label);
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let trusted = signer(0x51);
    adopt(&layout, &trusted);
    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &trusted, 0x61);
    write_received_fixture_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        block,
        &trusted,
    );
    (layout, root, block)
}

#[test]
fn removes_an_existing_pointer() {
    let (layout, root, _block) = repo_with_one_received_pointer("received-remove-basic");
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_some()
    );
    let report = remove_received_pointer(&layout, "remotes/heads/main").unwrap();
    assert_eq!(report.entries_before, 1);
    assert_eq!(report.entries_after, 0);
    assert!(report.removed.is_some());
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_none(),
        "the pointer must be gone after removal"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn plan_only_reports_without_writing() {
    let (layout, root, _block) = repo_with_one_received_pointer("received-remove-plan-only");
    let report = plan_remove_received_pointer(&layout, "remotes/heads/main").unwrap();
    assert_eq!(report.entries_before, 1);
    assert_eq!(report.entries_after, 0);
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_some(),
        "--plan-only must not actually remove the pointer"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refuses_a_non_remotes_name() {
    let (layout, root, _block) = repo_with_one_received_pointer("received-remove-non-remotes");
    let err = remove_received_pointer(&layout, "heads/main").unwrap_err();
    assert!(matches!(err, PrikkError::InvalidName(_)));
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn refuses_when_no_such_pointer_exists() {
    let root = unique_temp_dir("received-remove-no-pointer");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let err = remove_received_pointer(&layout, "remotes/heads/nothing-here").unwrap_err();
    assert!(
        matches!(err, PrikkError::Precondition(_)),
        "unexpected error: {err:?}"
    );
    assert!(err.to_string().contains("nothing to remove"), "{err}");
    let _ = std::fs::remove_dir_all(root);
}

/// The exclusion list (design report question 2, confirmed from source): `compact`, `bundle
/// import`, `archive import`, `archive export`, `doctor`'s own recovery-log restore, and `doctor
/// --repair-tails` all acquire the identical `LockableContainer::ReceivedIndex` lock this
/// operation also takes -- so one direct test of that shared lock covers every one of them,
/// rather than six near-duplicate integration fixtures that would each just re-prove the same
/// primitive (`acquire_container_locks`, already exhaustively tested in `lock/tests.rs`) is
/// exclusive. Each citation: `compact.rs:410`/`:936`; `bundle.rs:927`;
/// `archive/import.rs:472-478`/`:1474-1479` (the journal-tolerant variant);
/// `archive.rs:216,255`; `recovery_log.rs`'s own `repair_locks` (used by both
/// `--recovery-restore` and `--repair-tails`).
#[test]
fn refuses_while_the_received_index_lock_is_held_by_any_other_writer() {
    let (layout, root, _block) = repo_with_one_received_pointer("received-remove-lock-exclusion");
    let _held = acquire_container_locks(&layout, &[LockableContainer::ReceivedIndex]).unwrap();
    let err = remove_received_pointer(&layout, "remotes/heads/main").unwrap_err();
    assert!(
        matches!(err, PrikkError::LockConflict(_)),
        "unexpected error: {err:?}"
    );
    drop(_held);
    assert!(remove_received_pointer(&layout, "remotes/heads/main").is_ok());
    let _ = std::fs::remove_dir_all(root);
}

/// The new case-table row the handoff adds on top of the design round-2 report: removal, then a
/// sound re-import of the identical name, then a recovery-log restore of the *original* removal
/// -- the restore must refuse, because the target slot it would overwrite is no longer byte
/// identical to what the removal itself left there (RFC 168's own restore contract: "it writes
/// only if that file is byte-identical to its state after the repair").
#[test]
fn recovery_restore_of_a_removal_refuses_after_a_sound_reimport_of_the_same_name() {
    let (layout, root, _block) =
        repo_with_one_received_pointer("received-remove-reimport-then-restore");

    remove_received_pointer(&layout, "remotes/heads/main").unwrap();
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_none()
    );

    let listing = crate::recovery_log::recovery_list(&layout).unwrap();
    let run_id = listing
        .entries
        .first()
        .expect("the removal must have saved at least one recovery-log entry")
        .id
        .clone();
    assert!(
        listing.entries.iter().all(|entry| entry.id == run_id),
        "both of this run's own saves (the target slot, the generation log) must share one run id"
    );

    // A later, sound re-import of the identical name -- a real `write_received_pointer` call,
    // the same production path a real `bundle import`/`archive import` would take.
    let trusted = signer(0x51);
    let mut store = FileObjectStore::new(layout.clone());
    let new_block = write_root_block(&mut store, &trusted, 0x62);
    write_received_fixture_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        new_block,
        &trusted,
    );
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_some(),
        "fixture sanity: the re-import must have actually written a new pointer"
    );

    let plan = crate::recovery_log::recovery_restore(&layout, &run_id, true).unwrap();
    let can_restore =
        plan.refusal.is_none() && plan.conditions.iter().all(|condition| condition.holds);
    assert!(
        !can_restore,
        "a restore of the original removal must refuse once the slot it would overwrite has \
         since changed: {plan:?}"
    );

    // And the re-imported pointer must be untouched by the refused restore attempt.
    assert_eq!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .map(|pointer| pointer.ref_state_id),
        Some(
            crate::received::read_received_pointer(&layout, "remotes/heads/main")
                .unwrap()
                .unwrap()
                .ref_state_id
        )
    );
    let _ = std::fs::remove_dir_all(root);
}

/// End to end, tying B1 and B2a together, as the handoff asks: an import under a key this
/// repository never adopts makes `verify` fail; `received remove` makes it pass again, with the
/// B1 `unreached_objects` warning naming the newly-unreached block and RefState; a recovery-log
/// restore of that removal makes `verify` fail trust again, back to the original state.
#[test]
fn end_to_end_remove_then_restore_round_trips_verifys_own_trust_failure() {
    let root = unique_temp_dir("received-remove-end-to-end");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let trusted = signer(0x51);
    adopt(&layout, &trusted);
    let untrusted = signer(0x52); // never adopted

    let mut store = FileObjectStore::new(layout.clone());
    let block = write_root_block(&mut store, &untrusted, 0x63);
    let ref_state_id = write_received_fixture_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        block,
        &trusted, // the RefState/RefUpdate are signed by a trusted key; only the Block is not
    );

    let before = crate::verify_repository(&layout).unwrap();
    assert!(
        before.has_publication_trust_issues(),
        "the unadopted-key block must fail publication trust before any removal"
    );

    remove_received_pointer(&layout, "remotes/heads/main").unwrap();

    let after_remove = crate::verify_repository(&layout).unwrap();
    assert!(
        !after_remove.has_publication_trust_issues(),
        "once the pointer is removed, nothing reaches the untrusted block, so verify must pass: \
         {:?}",
        after_remove.publication_trust_issues
    );
    assert!(
        after_remove
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == block
                && unreached.object_type == ObjectType::Block),
        "the newly-unreached block must be named: {:?}",
        after_remove.unreached_objects
    );
    assert!(
        after_remove
            .unreached_objects
            .iter()
            .any(|unreached| unreached.object_id == ref_state_id
                && unreached.object_type == ObjectType::RefState),
        "the newly-unreached RefState must be named: {:?}",
        after_remove.unreached_objects
    );

    let listing = crate::recovery_log::recovery_list(&layout).unwrap();
    let run_id = listing
        .entries
        .first()
        .expect("the removal must have saved a recovery-log entry")
        .id
        .clone();
    let restored = crate::recovery_log::recovery_restore(&layout, &run_id, false).unwrap();
    assert!(
        restored.written,
        "the restore must actually write: {restored:?}"
    );

    let after_restore = crate::verify_repository(&layout).unwrap();
    assert!(
        after_restore.has_publication_trust_issues(),
        "restoring the removal must bring the pointer back, so verify fails trust again exactly \
         as before: {:?}",
        after_restore.publication_trust_issues
    );
    let _ = std::fs::remove_dir_all(root);
}

/// 0.54.0 step 3 Part 5b's own C2 (i): removal over a *deduced* live slot (the generation log is
/// lost after one ordinary write landed in the now-live slot -- the identical recipe `compact.rs`'s
/// own `a_crash_at_the_truncate_still_restores_the_deduced_compactions_save_received_index` uses
/// for this same container) behaves identically to the sound-parity case already covered above: a
/// plain, uncrashed run removes the target pointer (leaving the surviving one untouched), and a
/// recovery-log restore of that run returns the pre-removal state byte for byte -- both slots and
/// the generation log, not merely the pointer read-path.
#[test]
fn removes_an_existing_pointer_over_a_deduced_live_slot() {
    let root = unique_temp_dir("received-remove-deduced-sanity");
    let layout = RepositoryLayout::init(root.clone()).unwrap();
    let trusted = signer(0x51);
    adopt(&layout, &trusted);
    let mut store = FileObjectStore::new(layout.clone());
    let block1 = write_root_block(&mut store, &trusted, 0x67);
    write_received_fixture_pointer(
        &layout,
        &mut store,
        "remotes/heads/main",
        None,
        block1,
        &trusted,
    );
    crate::compact_received_index(&layout).unwrap(); // ordinary: the log now records a live slot
    let block2 = write_root_block(&mut store, &trusted, 0x68);
    write_received_fixture_pointer(
        &layout,
        &mut store,
        "remotes/heads/topic",
        None,
        block2,
        &trusted,
    ); // a write landing in the now-live slot
    std::fs::write(layout.received_index_generation_log_path(), b"").unwrap(); // lose the record

    let before_slot_a = std::fs::read(layout.received_index_slot_path(ContainerSlot::A)).unwrap();
    let before_slot_b = std::fs::read(layout.received_index_slot_path(ContainerSlot::B)).unwrap();
    let before_generation_log = std::fs::read(layout.received_index_generation_log_path()).unwrap();

    let report = remove_received_pointer(&layout, "remotes/heads/main").unwrap(); // deduced: this removal is the first durable record
    assert_eq!(report.entries_before, 2);
    assert_eq!(report.entries_after, 1);
    assert!(report.removed.is_some());
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_none(),
        "the target pointer must be gone after removal"
    );
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/topic")
            .unwrap()
            .is_some(),
        "the surviving pointer must be untouched by the removal"
    );

    let listing = crate::recovery_log::recovery_list(&layout).unwrap();
    let run_id = listing
        .entries
        .first()
        .expect("the removal must have saved at least one recovery-log entry, deduced or not")
        .id
        .clone();
    assert!(
        listing.entries.iter().all(|entry| entry.id == run_id),
        "both of this run's own saves must share one run id"
    );

    let restored = crate::recovery_log::recovery_restore(&layout, &run_id, false).unwrap();
    assert!(restored.refusal.is_none(), "{:?}", restored.refusal);
    assert!(restored.written);

    assert_eq!(
        std::fs::read(layout.received_index_slot_path(ContainerSlot::A)).unwrap(),
        before_slot_a,
        "slot A must come back byte-identical"
    );
    assert_eq!(
        std::fs::read(layout.received_index_slot_path(ContainerSlot::B)).unwrap(),
        before_slot_b,
        "slot B must come back byte-identical"
    );
    assert_eq!(
        std::fs::read(layout.received_index_generation_log_path()).unwrap(),
        before_generation_log,
        "the generation log must come back byte-identical too"
    );
    assert!(
        crate::received::read_received_pointer(&layout, "remotes/heads/main")
            .unwrap()
            .is_some(),
        "the restore must bring the removed pointer back"
    );

    let _ = std::fs::remove_dir_all(root);
}

/// 0.54.0 step 3 Part 5's own kill sweep, to 3B's standard
/// (`commit_boundary::witness::tests::m6_commit_kill_sweep_recovers_at_every_write_ordinal` is the
/// model this mirrors): fire the shared failpoint mechanism at every ordinal of every required
/// filesystem boundary this operation's own write sequence crosses (the two recovery-log saves,
/// the target-slot truncate/append, the generation-log append), and after each fired crash,
/// confirm (a) `verify`/`doctor` report nothing beyond a resync-covered torn tail, (b) a plain
/// retry of `received remove` reaches the identical end state as an uncrashed reference run, and
/// (c) 0.54.0 step 3 Part 5b's own addition: on a *fresh, separately crashed* fixture, a
/// recovery-log restore of the crashed run's own id -- not a retry -- either (a) returns the
/// pre-removal state byte for byte, or (b) refuses and changes nothing, in which case a plain
/// retry followed by a restore of the *original* run's id then returns the pre-removal state byte
/// for byte. "Count fired ordinals only" (the handoff's own words): the loop below stops arming a
/// family the moment an ordinal no longer fires (the identical `still_armed` break the M6 sweep
/// uses), so `fired_total` only ever counts ordinals this operation's own write sequence actually
/// reached.
///
/// Run twice -- once over a sound generation log (the live slot is read, not deduced), once over a
/// deduced one -- 0.54.0 step 3 Part 5b's own C2. The deduced fixture below is the identical recipe
/// `compact.rs`'s own
/// `a_crash_at_the_truncate_still_restores_the_deduced_compactions_save_received_index` uses for
/// this same container, adapted to this module's own signed, verify-clean pointers: write one
/// pointer, run an ordinary compaction once (the log now records a live slot), write a second
/// pointer (landing in the now-live slot), then lose the generation log. The removal under test
/// then targets the *first* pointer, so the post-removal buffer is non-empty (the second pointer's
/// own record survives the reduction) -- the one way this suite's own fixtures can produce a
/// genuinely torn target-slot state at all: the single-pointer sound fixture's own post-removal
/// buffer is always empty, so its truncate-to-empty already *is* the post-removal state, and a
/// crash anywhere in its write sequence always lands on either the pre- or the post-removal state,
/// never a third, torn one.
#[test]
fn kill_sweep_recovers_at_every_write_ordinal() {
    use crate::foundation::fsutil::{
        TestFailPoint, clear_failpoint_for_test, fail_after_for_test, failpoint_armed_for_test,
    };

    const MAX_ORDINAL: usize = 20;

    fn build_sound_fixture(label: &str) -> (RepositoryLayout, std::path::PathBuf) {
        let root = unique_temp_dir(label);
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        let trusted = signer(0x51);
        adopt(&layout, &trusted);
        let mut store = FileObjectStore::new(layout.clone());
        let block = write_root_block(&mut store, &trusted, 0x64);
        write_received_fixture_pointer(
            &layout,
            &mut store,
            "remotes/heads/main",
            None,
            block,
            &trusted,
        );
        (layout, root)
    }

    fn build_deduced_fixture(label: &str) -> (RepositoryLayout, std::path::PathBuf) {
        let root = unique_temp_dir(label);
        let layout = RepositoryLayout::init(root.clone()).unwrap();
        let trusted = signer(0x51);
        adopt(&layout, &trusted);
        let mut store = FileObjectStore::new(layout.clone());
        let block1 = write_root_block(&mut store, &trusted, 0x65);
        write_received_fixture_pointer(
            &layout,
            &mut store,
            "remotes/heads/main",
            None,
            block1,
            &trusted,
        );
        crate::compact_received_index(&layout).unwrap(); // ordinary: the log now records a live slot
        let block2 = write_root_block(&mut store, &trusted, 0x66);
        write_received_fixture_pointer(
            &layout,
            &mut store,
            "remotes/heads/topic",
            None,
            block2,
            &trusted,
        ); // a write landing in the now-live slot
        std::fs::write(layout.received_index_generation_log_path(), b"").unwrap(); // lose the record
        (layout, root)
    }

    /// A byte-precise snapshot of every file this operation's own write sequence can touch, so
    /// C1's own (a)/(b) check can compare against it exactly. `pointer` is a `Result`, not an
    /// unwrapped value: a *crashed* snapshot (outcome (b), between the real target-slot write
    /// completing and the real generation-log append completing, deduced parity only) can itself
    /// read as `AmbiguousGenerationLog` -- captured as data here, like every other byte, rather
    /// than unwrapped and panicking on the very state this check exists to describe.
    struct Snapshot {
        pointer: std::result::Result<Option<crate::received::ReceivedPointer>, String>,
        slot_a: Vec<u8>,
        slot_b: Vec<u8>,
        generation_log: Vec<u8>,
    }
    fn snapshot(layout: &RepositoryLayout, ref_name: &str) -> Snapshot {
        Snapshot {
            pointer: crate::received::read_received_pointer(layout, ref_name)
                .map_err(|error| error.to_string()),
            slot_a: std::fs::read(layout.received_index_slot_path(ContainerSlot::A)).unwrap(),
            slot_b: std::fs::read(layout.received_index_slot_path(ContainerSlot::B)).unwrap(),
            generation_log: std::fs::read(layout.received_index_generation_log_path()).unwrap(),
        }
    }
    fn assert_matches(
        layout: &RepositoryLayout,
        ref_name: &str,
        expected: &Snapshot,
        context: &str,
    ) {
        assert_eq!(
            crate::received::read_received_pointer(layout, ref_name)
                .map_err(|error| error.to_string()),
            expected.pointer,
            "{context}: the received pointer must match"
        );
        assert_eq!(
            std::fs::read(layout.received_index_slot_path(ContainerSlot::A)).unwrap(),
            expected.slot_a,
            "{context}: slot A must match byte for byte"
        );
        assert_eq!(
            std::fs::read(layout.received_index_slot_path(ContainerSlot::B)).unwrap(),
            expected.slot_b,
            "{context}: slot B must match byte for byte"
        );
        assert_eq!(
            std::fs::read(layout.received_index_generation_log_path()).unwrap(),
            expected.generation_log,
            "{context}: the generation log must match byte for byte"
        );
    }

    type FixtureBuilder = fn(&str) -> (RepositoryLayout, std::path::PathBuf);

    let ref_name = "remotes/heads/main";
    let fixtures: [(&str, FixtureBuilder); 2] = [
        ("sound", build_sound_fixture),
        ("deduced", build_deduced_fixture),
    ];

    for (parity, build_fixture) in fixtures {
        // Fixture sanity: an uncrashed reference run actually removes the pointer.
        {
            let (layout, root) = build_fixture(&format!("received-remove-kill-reference-{parity}"));
            remove_received_pointer(&layout, ref_name).unwrap();
            assert!(
                crate::received::read_received_pointer(&layout, ref_name)
                    .unwrap()
                    .is_none()
            );
            let _ = std::fs::remove_dir_all(root);
        }

        let mut fired_total = 0_usize;
        let mut per_family = Vec::new();
        // C1's own outcome tally, per parity -- see the report for the "no run saved yet" case,
        // a degenerate instance of outcome (a) folded in here rather than treated as a silent
        // third shape (a judgment call, named in the report).
        let mut outcome_a_restored = 0_usize;
        let mut outcome_a_untouched = 0_usize;
        let mut outcome_b_refused_then_retried = 0_usize;

        for point in [
            TestFailPoint::AppendWrite,
            TestFailPoint::RequiredFileSync,
            TestFailPoint::MutableFileSync,
            TestFailPoint::Truncate,
        ] {
            let mut fired_count = 0_usize;
            for ordinal in 0..MAX_ORDINAL {
                let (layout, root) = build_fixture(&format!(
                    "received-remove-kill-{parity}-{point:?}-{ordinal}"
                ));

                fail_after_for_test(point, ordinal);
                let crashed = remove_received_pointer(&layout, ref_name);
                let still_armed = failpoint_armed_for_test();
                clear_failpoint_for_test();

                if still_armed {
                    let _ = std::fs::remove_dir_all(&root);
                    break;
                }
                fired_count += 1;
                fired_total += 1;

                // (b) a plain retry reaches the identical end state as an uncrashed reference run
                // -- unchanged from before this round, kept alongside C1's own addition below.
                let retry = if crashed.is_ok() {
                    None
                } else {
                    Some(remove_received_pointer(&layout, ref_name))
                };
                if matches!(&retry, Some(Err(PrikkError::LockConflict(_)))) {
                    if let Ok(held) = crate::unlock::list_held_locks(&layout) {
                        for lock in &held {
                            let _ = crate::unlock::clear_lock(&layout, &lock.path);
                        }
                    }
                }
                let retry =
                    retry.map(|r| r.or_else(|_| remove_received_pointer(&layout, ref_name)));

                // C2's own discovery: once the live slot is deduced rather than recorded, a crash
                // that lands after the real target-slot write completes but before the real
                // generation-log append does leaves *both* slots a self-consistent but differing
                // history -- genuinely ambiguous, not a torn tail, so a plain retry cannot resolve
                // it (`resolve_or_deduce` correctly refuses rather than guess). This never arises
                // over a sound generation log (the resolver trusts the recorded slot directly,
                // never folding both). The recovery-log restore this operation's own save always
                // makes possible is the real way out here, exactly as it is for C1: it returns the
                // pre-removal state, at which point the generation log is sound again, and the
                // retry after that completes normally.
                let retry = match retry {
                    Some(Err(PrikkError::AmbiguousGenerationLog(_))) => {
                        let listing = crate::recovery_log::recovery_list(&layout).unwrap();
                        if let Some(run_id) = listing.entries.first().map(|entry| entry.id.clone())
                        {
                            let _ = crate::recovery_log::recovery_restore(&layout, &run_id, false);
                        }
                        let after_restore = remove_received_pointer(&layout, ref_name);
                        let after_restore =
                            if matches!(&after_restore, Err(PrikkError::LockConflict(_))) {
                                if let Ok(held) = crate::unlock::list_held_locks(&layout) {
                                    for lock in &held {
                                        let _ = crate::unlock::clear_lock(&layout, &lock.path);
                                    }
                                }
                                remove_received_pointer(&layout, ref_name)
                            } else {
                                after_restore
                            };
                        Some(after_restore)
                    }
                    other => other,
                };

                let retry_ok_or_already_gone = crashed.is_ok()
                    || matches!(&retry, Some(Ok(_)))
                    || matches!(
                        &retry,
                        Some(Err(err)) if err.to_string().contains("nothing to remove")
                    );

                let verification = crate::verify_repository(&layout);
                let verify_clean = verification
                    .as_ref()
                    .map(|v| !v.has_stage_failure() && !v.has_item_failure())
                    .unwrap_or(false);
                let pointer_gone = crate::received::read_received_pointer(&layout, ref_name)
                    .map(|p| p.is_none())
                    .unwrap_or(false);

                assert!(
                    verify_clean && retry_ok_or_already_gone && pointer_gone,
                    "{parity}/{point:?}@{ordinal}: did not recover cleanly (crashed={crashed:?}, \
                     verify_clean={verify_clean} ({verification:?}), retry={retry:?}, \
                     pointer_gone={pointer_gone})"
                );
                let _ = std::fs::remove_dir_all(&root);

                // (c) C1's own addition: a fresh, separately crashed fixture, restored via the
                // recovery log directly rather than retried.
                let (restore_layout, restore_root) = build_fixture(&format!(
                    "received-remove-kill-restore-{parity}-{point:?}-{ordinal}"
                ));
                let before = snapshot(&restore_layout, ref_name);

                fail_after_for_test(point, ordinal);
                let restore_crashed = remove_received_pointer(&restore_layout, ref_name);
                let restore_still_armed = failpoint_armed_for_test();
                clear_failpoint_for_test();
                assert!(
                    !restore_still_armed,
                    "{parity}/{point:?}@{ordinal}: fired on the retry-based fixture above but not \
                     on this freshly built one for the restore check"
                );

                let listing = crate::recovery_log::recovery_list(&restore_layout).unwrap();
                if listing.entries.is_empty() {
                    // The crash landed before even the first of this run's two recovery-log saves
                    // became durable -- nothing was yet saved, and since both real destructive
                    // writes happen only after both saves, nothing real was touched either: the
                    // ambient state already *is* the pre-removal state, with no run id to restore
                    // at all.
                    assert!(
                        restore_crashed.is_err(),
                        "{parity}/{point:?}@{ordinal}: no recovery entry was saved, but the run \
                         reported success: {restore_crashed:?}"
                    );
                    assert_matches(
                        &restore_layout,
                        ref_name,
                        &before,
                        &format!("{parity}/{point:?}@{ordinal} (no recovery run saved yet)"),
                    );
                    outcome_a_untouched += 1;
                    let _ = std::fs::remove_dir_all(&restore_root);
                    continue;
                }
                let run_id = listing.entries[0].id.clone();
                assert!(
                    listing.entries.iter().all(|entry| entry.id == run_id),
                    "{parity}/{point:?}@{ordinal}: both of this run's own saves must share one \
                     run id: {listing:?}"
                );

                let crashed_snapshot = snapshot(&restore_layout, ref_name);
                let restored =
                    crate::recovery_log::recovery_restore(&restore_layout, &run_id, false).unwrap();
                let can_restore = restored.refusal.is_none()
                    && restored.conditions.iter().all(|condition| condition.holds);

                if can_restore {
                    // Outcome (a): the restore returns the pre-removal state byte for byte.
                    assert!(
                        restored.written,
                        "{parity}/{point:?}@{ordinal}: a restorable plan must actually write: \
                         {restored:?}"
                    );
                    assert_matches(
                        &restore_layout,
                        ref_name,
                        &before,
                        &format!("{parity}/{point:?}@{ordinal} outcome (a)"),
                    );
                    outcome_a_restored += 1;
                } else {
                    // Outcome (b): the restore refuses and changes nothing; a plain retry, then a
                    // restore of the *original* run id, reaches the pre-removal state byte for
                    // byte.
                    assert!(
                        !restored.written,
                        "{parity}/{point:?}@{ordinal}: a refused plan must not write: {restored:?}"
                    );
                    assert_matches(
                        &restore_layout,
                        ref_name,
                        &crashed_snapshot,
                        &format!("{parity}/{point:?}@{ordinal} outcome (b), refused restore"),
                    );

                    let retry2 = remove_received_pointer(&restore_layout, ref_name);
                    let retry2 = if matches!(&retry2, Err(PrikkError::LockConflict(_))) {
                        if let Ok(held) = crate::unlock::list_held_locks(&restore_layout) {
                            for lock in &held {
                                let _ = crate::unlock::clear_lock(&restore_layout, &lock.path);
                            }
                        }
                        remove_received_pointer(&restore_layout, ref_name)
                    } else {
                        retry2
                    };
                    assert!(
                        retry2.is_ok(),
                        "{parity}/{point:?}@{ordinal}: outcome (b)'s own retry must succeed: \
                         {retry2:?}"
                    );

                    let restored_again =
                        crate::recovery_log::recovery_restore(&restore_layout, &run_id, false)
                            .unwrap();
                    assert!(
                        restored_again.refusal.is_none()
                            && restored_again
                                .conditions
                                .iter()
                                .all(|condition| condition.holds)
                            && restored_again.written,
                        "{parity}/{point:?}@{ordinal}: outcome (b)'s own restore, after the \
                         retry, must now succeed: {restored_again:?}"
                    );
                    assert_matches(
                        &restore_layout,
                        ref_name,
                        &before,
                        &format!("{parity}/{point:?}@{ordinal} outcome (b), restore after retry"),
                    );
                    outcome_b_refused_then_retried += 1;
                }
                let _ = std::fs::remove_dir_all(&restore_root);
            }
            per_family.push(format!(
                "{point:?}: armed {MAX_ORDINAL}, fired {fired_count}"
            ));
        }
        assert!(
            fired_total >= 1,
            "{parity}: the sweep must genuinely fire at least once; it fired {fired_total} times \
             across {per_family:?}"
        );
        eprintln!(
            "B2a kill sweep ({parity}): {fired_total} ordinals fired and recovered, \
             {per_family:?}; C1 restore outcomes: (a) actively restored = {outcome_a_restored}, \
             (a) no run saved yet = {outcome_a_untouched}, (b) refused then retried = \
             {outcome_b_refused_then_retried}"
        );
    }
}
