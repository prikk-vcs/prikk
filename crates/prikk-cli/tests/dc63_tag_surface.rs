//! CLI end-to-end regression for `prikk tag` (DC-63 Tag Surface).
//!
//! Drives the compiled `prikk` binary, matching the style of `dc60_branch_management.rs`. The two
//! blocker-evidence tests from the withdrawn v1 handoff are now permanent regression tests
//! (criterion 5): they assert the fixes hold, not that the blockers exist.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

use prikk_object::{
    CanonicalEncode, ObjectEnvelope, ObjectId, ObjectType, RefKind, RefStatePayload,
    RefUpdatePayload, TagPayload,
};
use prikk_store::{
    Ed25519MaintainerSigner, FileObjectStore, MaintainerSigner, ObjectReader, RefPublication,
    RefStore, RepositoryLayout, compute_patch_set_digest_and_count_from_block,
    maintainer_signature,
};

fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    // RFC 148: prikk now reads key material from the user's own config directory, so a test that
    // does not neutralise it measures whoever is running it. See `support::isolate_key_environment`.
    support::isolate_key_environment(&mut cmd);
    cmd
}

fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fail(output: &Output, what: &str) {
    assert!(
        !output.status.success(),
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc63-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn public_key_hex(bytes: &[u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn maintainer_seed() -> &'static str {
    "111122223333444455556666777788889999aaaabbbbccccddddeeeeffff0000"
}

fn maintainer_signer() -> Ed25519MaintainerSigner {
    Ed25519MaintainerSigner::from_seed(
        "e2e-maintainer",
        &[
            0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x66, 0x66, 0x77, 0x77,
            0x88, 0x88, 0x99, 0x99, 0xaa, 0xaa, 0xbb, 0xbb, 0xcc, 0xcc, 0xdd, 0xdd, 0xee, 0xee,
            0xff, 0xff, 0x00, 0x00,
        ],
    )
    .unwrap()
}

fn add_trusted_maintainer(repo: &Path) {
    let signer = maintainer_signer();
    let out = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "e2e-maintainer",
            "--public-key",
            &public_key_hex(&signer.public_key_bytes()),
        ])
        .output()
        .unwrap();
    ok(&out, "trust maintainer add");
}

fn commit(repo: &Path, ref_name: &str, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", "e2e-author")
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file("00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff"),
        )
        .args(["commit", "--ref", ref_name, "-m", message])
        .output()
        .unwrap()
}

fn seal(repo: &Path, ref_name: &str) -> Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(["seal", "--allow-no-audit", "--ref", ref_name])
        .output()
        .unwrap()
}

fn tag_create(repo: &Path, args: &[&str]) -> Output {
    let mut full = vec!["tag", "create"];
    full.extend_from_slice(args);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "e2e-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(maintainer_seed()),
        )
        .args(full)
        .output()
        .unwrap()
}

fn tag_list(repo: &Path) -> Output {
    prikk(repo).args(["tag"]).output().unwrap()
}

/// `init`, genesis-commit `readme.txt` on `heads/main`, and seal it.
fn seeded_repo(tag: &str) -> PathBuf {
    let repo = unique_repo(tag);
    ok(&prikk(&repo).arg("init").output().unwrap(), "init");
    std::fs::write(repo.join("readme.txt"), b"hello prikk\n").unwrap();
    ok(&commit(&repo, "heads/main", "genesis"), "commit heads/main");
    add_trusted_maintainer(&repo);
    ok(&seal(&repo, "heads/main"), "seal heads/main");
    repo
}

fn signed_envelope(
    object_type: ObjectType,
    schema_version: u32,
    canonical_payload: Vec<u8>,
    signer: &Ed25519MaintainerSigner,
) -> ObjectEnvelope {
    let mut envelope = ObjectEnvelope::unsigned(object_type, schema_version, canonical_payload);
    let object_id = envelope.object_id();
    envelope
        .add_signature(maintainer_signature(signer, object_type, object_id).unwrap())
        .unwrap();
    envelope
}

/// Build and publish a genesis (`update_seq = 1`, no previous) `RefState`/`RefUpdate` pair for an
/// arbitrary `(kind, ref_name, target_object_id)`, bypassing the CLI entirely. Used to test
/// `validate_coherent_publication`'s kind/namespace mutual enforcement directly, which requires
/// constructing publications the CLI itself would never build (a deliberately mismatched
/// kind/namespace pair).
fn publish_raw(
    ref_store: &RefStore,
    signer: &Ed25519MaintainerSigner,
    kind: RefKind,
    ref_name: &str,
    target_object_id: ObjectId,
) -> Result<ObjectId, String> {
    let ref_state_payload = RefStatePayload {
        ref_name: ref_name.to_string(),
        kind,
        target_object_id,
        update_seq: 1,
        previous_ref_state_id: None,
        required_attestation_ids: Vec::new(),
        closed: false,
    };
    let ref_state_envelope = signed_envelope(
        ObjectType::RefState,
        1,
        ref_state_payload.to_canonical_bytes().unwrap(),
        signer,
    );
    let ref_state_id = ref_state_envelope.object_id();
    let ref_update_payload = RefUpdatePayload {
        ref_name: ref_name.to_string(),
        old_ref_state_id: None,
        new_ref_state_id: ref_state_id,
        new_target_object_id: target_object_id,
        update_seq: 1,
        created_at: 0,
        author_key_id: signer.key_id().to_string(),
    };
    let ref_update_envelope = signed_envelope(
        ObjectType::RefUpdate,
        1,
        ref_update_payload.to_canonical_bytes().unwrap(),
        signer,
    );
    let publication = RefPublication {
        ref_name: ref_name.to_string(),
        expected_previous_ref_state_id: None,
        ref_state: ref_state_envelope,
        ref_update: ref_update_envelope,
    };
    ref_store
        .publish(&publication)
        .map_err(|err| err.to_string())
}

/// Permanent regression test for Blocker 1 (withdrawn v1 handoff): `tag create` must succeed
/// end to end — publish, appear in `tag list`, and `verify` must pass afterward.
#[test]
fn tag_create_publishes_and_verify_passes() {
    let repo = seeded_repo("create-and-verify");
    let out = tag_create(&repo, &["tags/v1", "--target", "heads/main"]);
    ok(&out, "tag create tags/v1 --target heads/main");

    let out = tag_list(&repo);
    ok(&out, "tag list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.lines().any(|line| line.starts_with("tags/v1 ")),
        "expected tags/v1 in listing; stdout: {stdout}"
    );

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify after tag create");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Permanent regression test for Blocker 2 (withdrawn v1 handoff): the published tag must take the
/// two-hop shape §6.6 requires — `RefState.target_object_id` is the `Tag` object, never the block
/// directly — and `verify` must accept it (it used to reject this exact shape as "targets missing
/// block").
#[test]
fn tag_create_resolves_two_hops_ref_to_tag_object_to_block() {
    let repo = seeded_repo("two-hop-resolution");
    let layout = RepositoryLayout::open(&repo).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());

    let main_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let main_envelope = object_store
        .read_typed(main_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let main_payload = RefStatePayload::decode_canonical(
        &main_envelope.canonical_payload,
        main_envelope.schema_version,
    )
    .unwrap();
    let block_id = main_payload.target_object_id;

    ok(
        &tag_create(&repo, &["tags/v1", "--target", "heads/main"]),
        "tag create tags/v1 --target heads/main",
    );

    let tag_ref_state_id = ref_store
        .read_current_ref_state_id("tags/v1")
        .unwrap()
        .unwrap();
    let tag_ref_state_envelope = object_store
        .read_typed(tag_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let tag_ref_state_payload = RefStatePayload::decode_canonical(
        &tag_ref_state_envelope.canonical_payload,
        tag_ref_state_envelope.schema_version,
    )
    .unwrap();
    assert_eq!(tag_ref_state_payload.kind, RefKind::Tag);
    assert_ne!(
        tag_ref_state_payload.target_object_id, block_id,
        "the ref must target the tag object, not the block directly"
    );
    // First hop: the ref's target must actually decode as a Tag object, not a Block. Reading it
    // back typed as Block must fail (not silently succeed) precisely because it is a Tag.
    assert!(
        object_store
            .read_typed(tag_ref_state_payload.target_object_id, ObjectType::Block)
            .is_err(),
        "the ref's target must not itself be readable as a Block"
    );
    let tag_object_envelope = object_store
        .read_typed(tag_ref_state_payload.target_object_id, ObjectType::Tag)
        .unwrap()
        .unwrap();
    let tag_payload = TagPayload::decode_canonical(&tag_object_envelope.canonical_payload).unwrap();
    // Second hop: the tag object's own target is the block.
    assert_eq!(tag_payload.target_block_id, block_id);
    assert_eq!(tag_payload.name, "tags/v1");

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify after tag create (two-hop shape)");

    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 117 stage 1 review condition (`RFC-117-stage-1-tag-payload-digest-review-v1.md` §2): rows 2
/// and 3 in `patch_set_digest/tests.rs` pinned a **test helper** that re-implements what `prikk tag`
/// does, never the production command itself -- so a wrong digest in `tag.rs`'s own wiring (from the
/// block id, from a constant, from anything else 32 bytes long) would ship undetected. This runs the
/// real `prikk tag create` and checks its own output against an independent recomputation.
#[test]
fn tag_create_computes_the_real_patch_set_digest_not_a_parallel_implementation() {
    let repo = seeded_repo("real-patch-set-digest");
    let layout = RepositoryLayout::open(&repo).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());

    let main_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let main_envelope = object_store
        .read_typed(main_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let main_payload = RefStatePayload::decode_canonical(
        &main_envelope.canonical_payload,
        main_envelope.schema_version,
    )
    .unwrap();
    let block_id = main_payload.target_object_id;

    ok(
        &tag_create(&repo, &["tags/v1", "--target", "heads/main"]),
        "tag create tags/v1 --target heads/main",
    );

    let tag_ref_state_id = ref_store
        .read_current_ref_state_id("tags/v1")
        .unwrap()
        .unwrap();
    let tag_ref_state_envelope = object_store
        .read_typed(tag_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let tag_ref_state_payload = RefStatePayload::decode_canonical(
        &tag_ref_state_envelope.canonical_payload,
        tag_ref_state_envelope.schema_version,
    )
    .unwrap();
    let tag_object_envelope = object_store
        .read_typed(tag_ref_state_payload.target_object_id, ObjectType::Tag)
        .unwrap()
        .unwrap();
    let tag_payload = TagPayload::decode_canonical(&tag_object_envelope.canonical_payload).unwrap();
    assert_eq!(tag_payload.target_block_id, block_id);

    // The independent recomputation: same function `tag.rs` itself calls, but invoked here by the
    // test over the block `prikk tag create` actually named -- not copied logic, the same production
    // function, called a second time from outside the command under test. RFC 117 T7: covers
    // `patch_count` too, the same way and for the same reason -- a wrong count here would mean
    // `prikk tag create`'s own wiring, not just the underlying algorithm, computes it differently.
    let (independently_recomputed_digest, independently_recomputed_count) =
        compute_patch_set_digest_and_count_from_block(&object_store, block_id)
            .expect("computing the digest and count over a real sealed block must succeed");
    assert_eq!(
        tag_payload.patch_set_digest, independently_recomputed_digest,
        "prikk tag create's own persisted patch_set_digest must equal an independent \
         recomputation over the block it named -- a wrong value here means the command users \
         actually run computes the tag's global identity differently from what the digest \
         algorithm itself would produce"
    );
    assert_eq!(
        tag_payload.patch_count, independently_recomputed_count,
        "prikk tag create's own persisted patch_count must equal an independent recomputation \
         over the block it named"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_create_writes_created_at_zero() {
    let repo = seeded_repo("created-at-zero");
    ok(
        &tag_create(&repo, &["tags/v1", "--target", "heads/main"]),
        "tag create tags/v1 --target heads/main",
    );

    let layout = RepositoryLayout::open(&repo).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout);

    let tag_ref_state_id = ref_store
        .read_current_ref_state_id("tags/v1")
        .unwrap()
        .unwrap();
    let tag_ref_state_envelope = object_store
        .read_typed(tag_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let tag_ref_state_payload = RefStatePayload::decode_canonical(
        &tag_ref_state_envelope.canonical_payload,
        tag_ref_state_envelope.schema_version,
    )
    .unwrap();
    let tag_envelope = object_store
        .read_typed(tag_ref_state_payload.target_object_id, ObjectType::Tag)
        .unwrap()
        .unwrap();
    let tag_payload = TagPayload::decode_canonical(&tag_envelope.canonical_payload).unwrap();
    assert_eq!(tag_payload.created_at, 0);

    let log = ref_store.replay_log("tags/v1").unwrap();
    assert_eq!(log.records.len(), 1);
    let ref_update =
        RefUpdatePayload::decode_canonical(&log.records[0].envelope.canonical_payload).unwrap();
    assert_eq!(ref_update.created_at, 0);

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_create_fails_closed_on_invalid_name() {
    let repo = seeded_repo("create-invalid-name");
    let out = tag_create(&repo, &["not-a-tag-ref", "--target", "heads/main"]);
    fail(&out, "tag create with invalid name");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(stderr.contains("tags/"), "unexpected stderr: {stderr}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_create_fails_closed_on_existing_name() {
    let repo = seeded_repo("create-existing-name");
    ok(
        &tag_create(&repo, &["tags/v1", "--target", "heads/main"]),
        "tag create tags/v1",
    );
    let out = tag_create(&repo, &["tags/v1", "--target", "heads/main"]);
    fail(&out, "tag create on an existing name");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("already exists"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_create_fails_closed_on_untrusted_signer() {
    let repo = seeded_repo("create-untrusted-signer");
    let out = prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file("222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111"),
        )
        .args(["tag", "create", "tags/v1", "--target", "heads/main"])
        .output()
        .unwrap();
    fail(&out, "tag create with an untrusted maintainer signer");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_create_fails_closed_on_unresolvable_target() {
    let repo = seeded_repo("create-bad-target");
    let out = tag_create(&repo, &["tags/v1", "--target", "heads/does-not-exist"]);
    fail(&out, "tag create with unresolvable --target");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("does not resolve to a published ref"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_list_reports_no_tags_before_any_created() {
    let repo = seeded_repo("list-empty");
    let out = tag_list(&repo);
    ok(&out, "tag list on a repository with no tags");
    assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "no tags");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_list_is_sorted_with_multiple_tags() {
    let repo = seeded_repo("list-sorted");
    ok(
        &tag_create(&repo, &["tags/zzz", "--target", "heads/main"]),
        "tag create tags/zzz",
    );
    ok(
        &tag_create(&repo, &["tags/aaa", "--target", "heads/main"]),
        "tag create tags/aaa",
    );

    let out = tag_list(&repo);
    ok(&out, "tag list");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines.len(), 2, "expected two tags; stdout: {stdout}");
    assert!(lines[0].starts_with("tags/aaa "), "stdout: {stdout}");
    assert!(lines[1].starts_with("tags/zzz "), "stdout: {stdout}");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Namespace/kind mutual enforcement, direction 1: a `RefKind::Tag` publication under a
/// `heads/`-shaped name must be rejected. Constructed directly against `RefStore::publish`,
/// bypassing the CLI (which never builds a mismatched pair), to exercise
/// `validate_coherent_publication`'s kind branch itself.
#[test]
fn publish_rejects_tag_kind_under_branch_shaped_name() {
    let repo = seeded_repo("mutual-enforcement-tag-kind-branch-name");
    let layout = RepositoryLayout::open(&repo).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());
    let signer = maintainer_signer();

    let main_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let main_envelope = object_store
        .read_typed(main_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let main_payload = RefStatePayload::decode_canonical(
        &main_envelope.canonical_payload,
        main_envelope.schema_version,
    )
    .unwrap();

    let result = publish_raw(
        &ref_store,
        &signer,
        RefKind::Tag,
        "heads/impostor",
        main_payload.target_object_id,
    );
    let err = result.expect_err("a Tag-kind publication under a heads/ name must be rejected");
    // `heads/` is a reserved namespace under the tag validator (mirrored from the branch
    // validator's reserved-namespace list), so this is caught there rather than by the
    // `tags/`-prefix check — still a rejection, just via the reserved-namespace branch.
    assert!(
        err.contains("ref namespace is reserved"),
        "unexpected error: {err}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Namespace/kind mutual enforcement, direction 2: a `RefKind::Branch` publication under a
/// `tags/`-shaped name must be rejected.
#[test]
fn publish_rejects_branch_kind_under_tag_shaped_name() {
    let repo = seeded_repo("mutual-enforcement-branch-kind-tag-name");
    let layout = RepositoryLayout::open(&repo).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_store = RefStore::new(layout.clone());
    let signer = maintainer_signer();

    let main_ref_state_id = ref_store
        .read_current_ref_state_id("heads/main")
        .unwrap()
        .unwrap();
    let main_envelope = object_store
        .read_typed(main_ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    let main_payload = RefStatePayload::decode_canonical(
        &main_envelope.canonical_payload,
        main_envelope.schema_version,
    )
    .unwrap();

    let result = publish_raw(
        &ref_store,
        &signer,
        RefKind::Branch,
        "tags/impostor",
        main_payload.target_object_id,
    );
    let err = result.expect_err("a Branch-kind publication under a tags/ name must be rejected");
    assert!(
        err.contains("ref namespace is reserved"),
        "unexpected error: {err}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_help_is_listed() {
    let out = Command::new(env!("CARGO_BIN_EXE_prikk"))
        .arg("--help")
        .output()
        .unwrap();
    ok(&out, "--help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("prikk tag"),
        "help must mention tag: {stdout}"
    );
}
