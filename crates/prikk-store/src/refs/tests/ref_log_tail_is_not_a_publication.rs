//! RFC 165 Addendum 1 §1 (review v1 §1, "the wedge"): a ref-log tail with no pointer lead is not an
//! incomplete publication. `publish_locked` writes the pointer before the log, so a genuine crash
//! mid-publication always leaves a pointer lead; zeros, random bytes, or a torn prefix the crash left
//! behind for an unrelated reason carry no lead at all. `commit` and every other writer that never
//! appends to the ref log must proceed over such a tail; a publication must still refuse over it
//! (RFC 164 Rule D: a writer refuses over a tail in a file it appends to it is about to append to),
//! but naming the tail itself, not "incomplete publication" and not "seal retry" -- neither applies.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use prikk_object::{
    BlockKind, BlockPayload, CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType,
};

use crate::foundation::layout::ContainerSlot;
use crate::maintainer_signing::{Ed25519MaintainerSigner, MaintainerSigner};
use crate::object_store::ObjectWriteSession;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{
    FileObjectStore, ObjectWriter, RefPublication, RefStore, RepositoryLayout,
    add_trusted_maintainer, maintainer_signature as sign_maintainer,
};

fn maintainer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed("rfc165-addendum1-maintainer", &[0x73; 32]).expect("seed")
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn setup(root: &std::path::Path) -> RepositoryLayout {
    let layout = RepositoryLayout::init(root.to_path_buf()).expect("init");
    let signer = maintainer();
    add_trusted_maintainer(
        &layout,
        signer.key_id(),
        &hex_encode(&signer.public_key_bytes()),
    )
    .expect("trust");
    layout
}

fn root_block(layout: &RepositoryLayout) -> ObjectId {
    let payload = BlockPayload {
        parent_block_ids: Vec::new(),
        kind: BlockKind::Root,
        patch_ids: Vec::new(),
        state_merkle_root: crate::compute_state_root(&[]).unwrap(),
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let mut env =
        ObjectEnvelope::unsigned(ObjectType::Block, 2, payload.to_canonical_bytes().unwrap());
    let id = env.object_id();
    env.add_signature(sign_maintainer(&maintainer(), ObjectType::Block, id).unwrap())
        .unwrap();
    FileObjectStore::new(layout.clone())
        .write_object(&env)
        .unwrap()
}

fn build_branch_create_publication(
    ref_name: &str,
    target_block: ObjectId,
    signer: &impl MaintainerSigner,
) -> RefPublication {
    use prikk_object::{RefKind, RefStatePayload, RefUpdatePayload};
    let state = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind: RefKind::Branch,
        target_object_id: target_block,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let mut state_env =
        ObjectEnvelope::unsigned(ObjectType::RefState, 1, state.to_canonical_bytes().unwrap());
    let state_id = state_env.object_id();
    state_env
        .add_signature(sign_maintainer(signer, ObjectType::RefState, state_id).unwrap())
        .unwrap();
    let update = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: state_id,
        new_target_object_id: target_block,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let mut update_env = ObjectEnvelope::unsigned(
        ObjectType::RefUpdate,
        1,
        update.to_canonical_bytes().unwrap(),
    );
    let update_id = update_env.object_id();
    update_env
        .add_signature(sign_maintainer(signer, ObjectType::RefUpdate, update_id).unwrap())
        .unwrap();
    RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state: state_env,
        ref_update: update_env,
    }
}

/// Hash every file under `.prikk/`, path and content both -- the same "the tree is identical"
/// comparison `every_publication_refuses_first.rs` uses.
fn snapshot_tree(layout: &RepositoryLayout) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    fn walk(
        dir: &std::path::Path,
        root: &std::path::Path,
        out: &mut Vec<(std::path::PathBuf, Vec<u8>)>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, root, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                out.push((
                    path.strip_prefix(root).unwrap_or(&path).to_path_buf(),
                    bytes,
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(layout.prikk_dir(), layout.prikk_dir(), &mut out);
    out.sort();
    out
}

/// A repository with `heads/main` soundly published (one real record in the ref log), ready for a
/// lead-free tail to be appended behind it.
fn repo_with_main(root: &std::path::Path) -> RepositoryLayout {
    let layout = setup(root);
    let signer = maintainer();
    let target = root_block(&layout);
    let publication = build_branch_create_publication("heads/main", target, &signer);
    let ref_store = RefStore::new(layout.clone());
    let mut object_store = ObjectWriteSession::open(&layout).unwrap();
    ref_store
        .publish_with_object_store(&mut object_store, &publication)
        .unwrap();
    drop(object_store);
    layout
}

fn ref_log_path(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.ref_log_container_slot_path(ContainerSlot::A)
}

enum TailShape {
    ZeroBytes(usize),
    RandomBytes(usize),
    /// A torn prefix that structurally claims `heads/main`'s own `ref_name_key` in its header (magic
    /// and ref-name bytes intact, body truncated) -- the strongest case: even a tail that *looks*
    /// attributable is not a lead, because the pointer was never advanced to name it.
    AttributableTornPrefix,
}

fn append_tail(layout: &RepositoryLayout, shape: &TailShape) {
    use std::io::Write;
    let path = ref_log_path(layout);
    match shape {
        TailShape::ZeroBytes(n) => {
            let bytes = vec![0_u8; *n];
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(&bytes).unwrap();
        }
        TailShape::RandomBytes(n) => {
            // Deterministic "random": not all-zero, not a valid magic prefix, same shape as the
            // architect's own fixture (not literally `/dev/urandom`, which would make the test
            // flaky without adding anything the fixed pattern doesn't already exercise).
            let bytes: Vec<u8> = (0..*n).map(|i| ((i * 73 + 41) % 251) as u8).collect();
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(&path)
                .unwrap();
            file.write_all(&bytes).unwrap();
        }
        TailShape::AttributableTornPrefix => {
            // A plausible *next* update for a ref that is never published at all (not `heads/main`,
            // which a publishing test below may itself be excluded on -- attributing the tail to the
            // very ref a command excludes would test the exclusion, not the tail check this test is
            // about). The pointer never advances to name it, so the log's torn claim of this ref is
            // lead-free by construction, even though its header is structurally attributable.
            let target = root_block(layout);
            let envelope = build_branch_create_publication(
                "heads/never-excluded-by-any-call-site",
                target,
                &maintainer(),
            )
            .ref_update;
            crate::refs::append_torn_ref_log_tail_for_test(
                layout,
                crate::foundation::layout::ref_name_key_bytes(
                    "heads/never-excluded-by-any-call-site",
                ),
                &envelope,
            )
            .unwrap();
        }
    }
}

fn shapes() -> Vec<(&'static str, TailShape)> {
    vec![
        ("30 zero bytes", TailShape::ZeroBytes(30)),
        ("100 zero bytes", TailShape::ZeroBytes(100)),
        ("4,096 zero bytes", TailShape::ZeroBytes(4096)),
        ("100 random bytes", TailShape::RandomBytes(100)),
        (
            "an attributable torn prefix",
            TailShape::AttributableTornPrefix,
        ),
    ]
}

#[test]
fn a_lead_free_tail_never_blocks_commits_own_precondition() {
    for (label, shape) in shapes() {
        let root = unique_temp_dir("rfc165-a1-tail-commit");
        let layout = repo_with_main(&root);
        append_tail(&layout, &shape);
        assert!(
            crate::refs::ensure_no_incomplete_publication(&layout).is_ok(),
            "{label}: a lead-free ref-log tail must not block commit's own precondition"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn a_lead_free_tail_still_refuses_every_publication_that_appends_to_the_ref_log() {
    use crate::patch_set_digest::compute_patch_set_digest_and_count_from_block;
    use crate::tag_travel::create_local_tag;

    for (label, shape) in shapes() {
        // merge: a second branch confluent with `heads/main`, then another ref's own tail.
        let root = unique_temp_dir("rfc165-a1-tail-merge");
        let layout = repo_with_main(&root);
        let main_tip = {
            let store = RefStore::new(layout.clone());
            store.read_current_ref_state_id("heads/main").unwrap()
        };
        let target = {
            let ref_store = RefStore::new(layout.clone());
            let envelope = ref_store
                .replay_log("heads/main")
                .unwrap()
                .records
                .last()
                .unwrap()
                .envelope
                .clone();
            let update =
                prikk_object::RefUpdatePayload::decode_canonical(&envelope.canonical_payload)
                    .unwrap();
            update.new_target_object_id
        };
        let _ = main_tip;
        let topic = build_branch_create_publication("heads/topic", target, &maintainer());
        {
            let ref_store = RefStore::new(layout.clone());
            let mut object_store = ObjectWriteSession::open(&layout).unwrap();
            ref_store
                .publish_with_object_store(&mut object_store, &topic)
                .unwrap();
        }
        append_tail(&layout, &shape);
        let before = snapshot_tree(&layout);
        let result = crate::merge::execute::execute_merge(
            &layout,
            target,
            "heads/main",
            "heads/topic",
            &maintainer(),
        );
        assert!(result.is_err(), "{label}: merge must refuse over the tail");
        let message = result.unwrap_err().to_string();
        assert!(
            message.contains("the ref log"),
            "{label}: must name the ref log, got: {message}"
        );
        assert!(
            message.contains("--repair-tails"),
            "{label}: must name the repair --repair-tails, got: {message}"
        );
        assert!(
            !message.contains("incomplete ref publication"),
            "{label}: must not call a lead-free tail an incomplete publication, got: {message}"
        );
        assert!(
            !message.contains("seal retry"),
            "{label}: must not point at a seal retry that cannot apply, got: {message}"
        );
        assert_eq!(
            before,
            snapshot_tree(&layout),
            "{label}: a refusal must write nothing"
        );
        let _ = std::fs::remove_dir_all(&root);

        // tag create: a fresh repo, tail appended behind `heads/main`'s own sound record.
        let root = unique_temp_dir("rfc165-a1-tail-tag");
        let layout = repo_with_main(&root);
        append_tail(&layout, &shape);
        let before = snapshot_tree(&layout);
        let target = {
            let store = FileObjectStore::new(layout.clone());
            let ref_store = RefStore::new(layout.clone());
            let envelope = ref_store
                .replay_log("heads/main")
                .unwrap()
                .records
                .last()
                .unwrap()
                .envelope
                .clone();
            let update =
                prikk_object::RefUpdatePayload::decode_canonical(&envelope.canonical_payload)
                    .unwrap();
            let _ = &store;
            update.new_target_object_id
        };
        let (digest, count) = {
            let store = FileObjectStore::new(layout.clone());
            compute_patch_set_digest_and_count_from_block(&store, target).unwrap()
        };
        let mut session = ObjectWriteSession::open(&layout).unwrap();
        let result = create_local_tag(
            &layout,
            &mut session,
            "tags/v1",
            target,
            None,
            digest,
            count,
            &maintainer(),
        );
        drop(session);
        assert!(
            result.is_err(),
            "{label}: tag create must refuse over the tail"
        );
        assert_eq!(
            before,
            snapshot_tree(&layout),
            "{label}: a refusal must write nothing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[test]
fn control_a_lead_free_tail_produces_no_failed_outcome_at_all() {
    // RFC 165 R5 (§9.2): superseded this test's own original form (Addendum 1's "the old predicate
    // blocks on any `Failed` outcome, the new one does not"). R5 changed the *representation* itself
    // -- a genuine, lead-free tail no longer produces a `Failed` outcome at all (`trailing_partial_
    // bytes` alone represents it, exactly like the too-short-for-a-header case already did), so the
    // old predicate's own reproduction is no longer even reachable from this fixture: there is
    // nothing for "any Failed outcome" to see. What remains worth asserting directly: this fixture
    // produces zero `Failed` outcomes and a nonzero `trailing_partial_bytes`, and the real
    // precondition still does not block `commit` over it -- `container.rs`'s own `§9.2 bypassed`
    // control (`no_refs_times_log_precondition.rs` / `container::tests`) is what now exercises the
    // "old, wrong" predicate meaningfully (a *complete* damaged record wrongly read as a tail).
    let root = unique_temp_dir("rfc165-r5-tail-no-failed-outcome");
    let layout = repo_with_main(&root);
    append_tail(&layout, &TailShape::RandomBytes(100));

    let _whole_read_scope = crate::foundation::fsutil::whole_read_guard::declare("ref-log-replay");
    let relative = layout.repository_relative(&ref_log_path(&layout)).unwrap();
    let bytes = crate::foundation::fsutil::read_file_if_exists(
        layout.repository_mutation_root(),
        &relative,
    )
    .unwrap()
    .unwrap();
    let discovery = crate::refs::decode_ref_container_records(&bytes).unwrap();
    let any_failed = discovery.record_outcomes.iter().any(|outcome| {
        matches!(
            outcome.status,
            crate::refs::container::RefContainerRecordStatus::Failed { .. }
        )
    });
    assert!(
        !any_failed,
        "a genuine, lead-free tail must produce zero Failed outcomes under RFC 165 R5"
    );
    assert_ne!(
        discovery.trailing_partial_bytes, 0,
        "fixture bug: a 100-random-byte tail must still be represented as a tail"
    );
    assert!(
        crate::refs::ensure_no_incomplete_publication(&layout).is_ok(),
        "the real precondition must not block commit over a lead-free tail"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// RFC 165 Addendum 1 §1, a second gap found while fixing the first (self-caught, not from review):
/// a torn tail too short to carry a readable `ref_name_key` (fewer than 42 bytes from the tail's own
/// start) cannot be attributed to any ref by header inspection -- including the ref that is actually
/// retrying its own interrupted publication. An earlier version of `ensure_may_publish` excluded a
/// tail only when its header-claimed name matched the excluded ref, which wrongly refused a ref's own
/// first-ever publication crashed after only a few bytes (caught by the existing
/// `seal_truncates_only_partial_tail_before_completion` CLI test, red before this was fixed). The
/// correct question is whether the excluded ref *itself* currently has a pointer lead, not whether the
/// tail's header happens to name it.
#[test]
fn an_unattributably_short_tail_does_not_block_the_excluded_refs_own_retry() {
    let root = unique_temp_dir("rfc165-a1-short-tail-own-retry");
    let layout = setup(&root);
    let target = root_block(&layout);

    // `heads/topic`'s own pointer is written (as a real publish would, pointer before log), then
    // only a 4-byte fragment -- shorter than even the header's own `ref_name_key` field -- is
    // appended to the log. No real publish ever produces content this short; this is deliberately
    // more extreme than `append_torn_ref_log_tail_for_test`'s own ~90-byte torn record, to isolate
    // "cannot be attributed at all" from "attributed to a different ref."
    let publication = build_branch_create_publication("heads/topic", target, &maintainer());
    let state_id = publication.ref_state.object_id();
    FileObjectStore::new(layout.clone())
        .write_object(&publication.ref_state)
        .unwrap();
    crate::refs::write_ref_pointer_candidate_for_test(&layout, "heads/topic", state_id).unwrap();
    {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(ref_log_path(&layout))
            .unwrap();
        file.write_all(&[0xde, 0xad, 0xbe, 0xef]).unwrap();
    }

    // Premise: `heads/topic` really does lead (an incomplete publication for it specifically), and
    // the precondition still correctly refuses an *unrelated* ref over it.
    assert!(
        crate::refs::ensure_no_incomplete_publication(&layout).is_err(),
        "fixture bug: heads/topic must actually be leading"
    );

    // The excluded ref's own retry must proceed despite the unattributable tail.
    assert!(
        crate::refs::ensure_may_publish(&layout, "heads/topic").is_ok(),
        "heads/topic's own retry must not be blocked by its own unattributable tail"
    );
    let _ = std::fs::remove_dir_all(&root);
}
