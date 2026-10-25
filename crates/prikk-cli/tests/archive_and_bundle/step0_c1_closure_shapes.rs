//! 0.52.0 step 0, Part C1: the archive's own closure now follows the store's real rule, through the
//! compiled binary, for the finding's own two reported shapes (`.git-exclude/reviewed/
//! archive-closure-finding-v1.md`) -- both built with real commands, never hand-crafted bytes.
//! Before this part, `archive export` on either shape wrote a file its own `archive verify`/
//! `archive import` refused, even though the source repository's own `prikk verify` passed cleanly.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_store::ObjectReader;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn archive_path(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "prikk-cli-0520-c1-{tag}-{}.prepo",
        support::unique_suffix()
    ))
}

fn commit_and_seal(repo: &Path, message: &str) {
    support::ok(&support::commit(repo, "heads/main", message), message);
    support::ok(&support::seal(repo, "heads/main"), "seal");
}

/// Export, verify and import `repo`'s own archive, asserting every step succeeds -- the shape of
/// assertion both this file's own tests share.
fn assert_archive_round_trips_cleanly(repo: &Path, tag: &str) {
    let path = archive_path(tag);

    let export = support::prikk(repo)
        .args(["archive", "export"])
        .arg(&path)
        .output()
        .unwrap();
    support::ok(&export, "archive export");

    let verify = support::prikk(repo)
        .args(["archive", "verify"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        verify.status.success(),
        "archive verify must exit 0 for a source repository `prikk verify` already passes: {}\n{}",
        stdout(&verify),
        String::from_utf8_lossy(&verify.stderr)
    );
    assert!(
        stdout(&verify).contains("internally consistent"),
        "verify's own stdout: {}",
        stdout(&verify)
    );

    let dest = support::unique_repo(&format!("0520-c1-{tag}-dest"));
    support::init(&dest);
    let import = support::prikk(&dest)
        .args(["archive", "import"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        import.status.success(),
        "archive import must succeed for an archive whose own verify just passed: {}\n{}",
        stdout(&import),
        String::from_utf8_lossy(&import.stderr)
    );

    // C1-5: the two shapes must not stop at `import` succeeding -- the destination's own `prikk
    // verify` must also pass, over the identical content the source repository's own `verify`
    // already passed. The destination adopts the same maintainer key a real receiver would decide
    // to trust (R2: trust is the receiver's own decision, never carried by import itself) -- with
    // that one row settled the same way on both sides, the destination's own verify must agree with
    // the source's.
    support::trust_maintainer(&dest);
    let source_verify = support::verify(repo);
    assert!(
        source_verify.status.success(),
        "the source repository's own prikk verify must pass: {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );
    let dest_verify = support::verify(&dest);
    assert!(
        dest_verify.status.success(),
        "the destination's own prikk verify must pass after import, not merely `archive import` \
         succeeding: {}\n{}",
        stdout(&dest_verify),
        String::from_utf8_lossy(&dest_verify.stderr)
    );
    assert!(
        !stdout(&dest_verify).contains("damaged:"),
        "the destination must carry no damage of its own: {}",
        stdout(&dest_verify)
    );
}

/// Shape 1 of the finding: a repository that ran `sync accept` carries a `RecognitionClaim`
/// referencing another repository's own blocks -- never existence-checked, by standing design
/// (RFC 155 §9.7-adjacent: a claim is never trust-conferring, never existence-checked), so the
/// archive must not require it either.
///
/// 0.52.0 step 0, Part C2 correction: confirmed against the real `0.51.0` release binary (reused
/// from the release's own prep artifact, `~/.pgtmp/rel-0.51.0-bin/prikk`) that **this exact shape,
/// with `sync seal` called right after `sync accept`, does not reach the original finding's own
/// failure at all** -- `sync seal` recreates the claimed block and patches locally before sealing,
/// so by the time this test's own archive is exported, the claim's own `block_id` and every
/// `patch_ids` entry are already locally resolvable (confirmed empirically: a debug probe on this
/// exact fixture showed both `contained=true` on the un-perturbed, un-fixed code). This test still
/// covers a real, legitimate round-trip shape (an accepted-and-sealed claim, carried alongside its
/// own now-local content) and is kept; [`an_unsealed_sync_accepted_claims_dangling_block_reference_exports_verifies_and_imports_cleanly`]
/// below is the test that reaches the original finding's own exact repro (confirmed on the real
/// `0.51.0` binary: `archive verify` fails there with the identical wording the finding's own table
/// quotes, *"recognition-claim ... references \<block\>, which this archive does not carry"*).
#[test]
fn a_repository_that_ran_sync_accept_exports_verifies_and_imports_cleanly() {
    let repo_a = support::unique_repo("0520-c1-sync-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"sync accept shape\n",
        "first",
    );

    let repo_b = support::unique_repo("0520-c1-sync-b");
    support::init(&repo_b);

    let summary_file = archive_path("sync-summary");
    support::ok(
        &support::prikk(&repo_a)
            .args([
                "sync",
                "summary",
                "--output",
                summary_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync summary",
    );

    let have_file = archive_path("sync-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    let artifact_file = archive_path("sync-artifact");
    let build = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            artifact_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&build, "sync build");

    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = stdout(&accept);
    let claim_id = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .and_then(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap_or_else(|| panic!("sync accept must print a claim line: {accept_stdout}"))
        .trim()
        .to_string();

    support::trust_maintainer(&repo_b);
    let seal = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", &claim_id])
        .output()
        .unwrap();
    support::ok(&seal, "sync seal");

    // The source itself must already pass -- confirming the finding's own premise, not assuming it.
    let source_verify = support::prikk(&repo_b).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "repo B's own prikk verify must already pass (the finding's own premise): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );

    assert_archive_round_trips_cleanly(&repo_b, "sync-accept");
}

/// The original finding's own exact repro: `sync accept`, **never sealed** -- the claim sits
/// standalone, its own `block_id` naming the sending repository's own block, which the receiver
/// never locally held and never recreated (that only happens at `sync seal` time). Confirmed on the
/// real `0.51.0` release binary: the source repository's own `prikk verify` passes, `archive export`
/// succeeds, and that same binary's own `archive verify` then fails with *"recognition-claim ...
/// references \<block\>, which this archive does not carry"* -- the identical wording
/// `archive-closure-finding-v1.md`'s own table quotes. The fixed binary accepts the identical,
/// 0.51.0-written archive (§5's own forward-compatibility claim, checked for real, not assumed).
#[test]
fn an_unsealed_sync_accepted_claims_dangling_block_reference_exports_verifies_and_imports_cleanly()
{
    let repo_a = support::unique_repo("0520-c2-sync-unsealed-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"unsealed sync accept shape\n",
        "first",
    );

    let repo_b = support::unique_repo("0520-c2-sync-unsealed-b");
    support::init(&repo_b);

    let summary_file = archive_path("sync-unsealed-summary");
    support::ok(
        &support::prikk(&repo_a)
            .args([
                "sync",
                "summary",
                "--output",
                summary_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync summary",
    );

    let have_file = archive_path("sync-unsealed-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    let artifact_file = archive_path("sync-unsealed-artifact");
    let build = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            artifact_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&build, "sync build");

    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    // Deliberately never calls `sync seal` -- the claim stays standalone, its own `block_id` never
    // locally recreated, matching the original finding's own exact shape.

    let source_verify = support::prikk(&repo_b).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "repo B's own prikk verify must pass even with an unsealed claim present (the claims rule: \
         never existence-checked): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );

    assert_archive_round_trips_cleanly(&repo_b, "sync-unsealed");
}

/// Shape 2 of the finding: a text file edited (so its current content is an `EditText` result
/// nothing stores), sealed, then deleted -- the sealed `Patch`'s own deletion preimage names a Blob
/// id the repository never wrote (DC-65). Never re-derived once sealed, by standing design, so the
/// archive must not require it either.
#[test]
fn an_edited_then_deleted_file_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c1-edited-deleted");
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("notes.txt"), b"line one\nline two\n").unwrap();
    commit_and_seal(&repo, "create notes.txt");
    std::fs::write(repo.join("notes.txt"), b"line one\nline two, edited\n").unwrap();
    commit_and_seal(&repo, "edit notes.txt");
    std::fs::remove_file(repo.join("notes.txt")).unwrap();
    commit_and_seal(&repo, "delete notes.txt");

    let source_verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "the source repository's own prikk verify must already pass (the finding's own premise): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );

    assert_archive_round_trips_cleanly(&repo, "edited-deleted");
}

/// 0.52.0 step 0, Part C1 round 3, finding C1-9: an archive carrying a genuine `UnreferencedRemnant`
/// (RFC 164 Rule E), end to end, through the real compiled binary -- `archive export`, then `archive
/// verify` (prose names the owner and the missing patch; `--format json` lists it under
/// `unreferenced_remnants`), then `archive import`, then the destination's own `prikk verify` (the
/// identical store-side classification, the same warning). No failpoint is wired into any bundle or
/// archive import path (searched: no `anchored::failpoints` consumer anywhere under `archive/` or
/// `bundle.rs`), so the remnant is built the same way the store's own `verify/tests/reachability.rs`
/// builds one for the *live* repository's own tests: an orphan `Block` written directly through
/// `FileObjectStore`, never reachable from any ref, received pointer, or queued patch.
#[test]
fn an_unreferenced_remnant_round_trips_with_its_warning_intact() {
    let repo = support::unique_repo("0520-c1r3-remnant-source");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    let layout = prikk_store::RepositoryLayout::open(repo.clone()).expect("open layout");
    let mut object_store = prikk_store::FileObjectStore::new(layout.clone());
    let missing_patch_id = prikk_object::ObjectId::from_bytes([0xC1; 32]);
    let state_root = prikk_store::compute_state_root(&[]).expect("state root");
    let payload = prikk_object::BlockPayload {
        parent_block_ids: Vec::new(),
        kind: prikk_object::BlockKind::Root,
        patch_ids: vec![missing_patch_id],
        state_merkle_root: state_root,
        snapshot_blob_ref: None,
        mainline_parent_id: None,
        merge_baseline_block_id: None,
    };
    let bytes = prikk_object::CanonicalEncode::to_canonical_bytes(&payload).expect("encode block");
    let mut orphan_envelope =
        prikk_object::ObjectEnvelope::unsigned(prikk_object::ObjectType::Block, 2, bytes);
    let orphan_block_id = orphan_envelope.object_id();
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(
        support::MAINTAINER_KEY_ID,
        &support::MAINTAINER_SEED,
    )
    .expect("build signer");
    let signature = prikk_store::maintainer_signature(
        &signer,
        prikk_object::ObjectType::Block,
        orphan_block_id,
    )
    .expect("sign");
    orphan_envelope
        .add_signature(signature)
        .expect("attach signature");
    prikk_store::ObjectWriter::write_object(&mut object_store, &orphan_envelope)
        .expect("write orphan block");

    let source_verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "the source repository's own prikk verify must already pass (RFC 164 Rule E: an \
         unreferenced remnant is never damage): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );
    assert!(
        stdout(&source_verify).contains(&orphan_block_id.to_string()),
        "the source's own verify must already show the remnant warning: {}",
        stdout(&source_verify)
    );

    let path = archive_path("remnant");
    let export = support::prikk(&repo)
        .args(["archive", "export"])
        .arg(&path)
        .output()
        .unwrap();
    support::ok(&export, "archive export");

    let verify_text = support::prikk(&repo)
        .args(["archive", "verify"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        verify_text.status.success(),
        "archive verify must exit 0 -- a remnant is never damage: {}\n{}",
        stdout(&verify_text),
        String::from_utf8_lossy(&verify_text.stderr)
    );
    let verify_text_stdout = stdout(&verify_text);
    assert!(
        verify_text_stdout.contains(&orphan_block_id.to_string())
            && verify_text_stdout.contains(&missing_patch_id.to_string()),
        "prose must name both the owning block and the missing patch: {verify_text_stdout}"
    );
    assert!(
        verify_text_stdout.contains("references missing block patch")
            && verify_text_stdout.contains("harmless"),
        "prose must use the store's own remnant_warning wording, not a second one: \
         {verify_text_stdout}"
    );

    let verify_json = support::prikk(&repo)
        .args(["archive", "verify", "--format", "json"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        verify_json.status.success(),
        "archive verify --format json must also exit 0"
    );
    let verify_json_stdout = stdout(&verify_json);
    assert!(
        verify_json_stdout.contains("\"unreferenced_remnants\"")
            && verify_json_stdout.contains(&orphan_block_id.to_string())
            && verify_json_stdout.contains(&missing_patch_id.to_string()),
        "--format json must list the remnant under unreferenced_remnants: {verify_json_stdout}"
    );

    let dest = support::unique_repo("0520-c1r3-remnant-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);
    let import = support::prikk(&dest)
        .args(["archive", "import"])
        .arg(&path)
        .output()
        .unwrap();
    support::ok(&import, "archive import");

    let dest_verify = support::prikk(&dest).arg("verify").output().unwrap();
    assert!(
        dest_verify.status.success(),
        "the destination's own prikk verify must pass -- a remnant is never damage there either: \
         {}\n{}",
        stdout(&dest_verify),
        String::from_utf8_lossy(&dest_verify.stderr)
    );
    assert!(
        stdout(&dest_verify).contains(&orphan_block_id.to_string()),
        "the destination's own verify must show the identical remnant warning, the same store-side \
         classification applied after import: {}",
        stdout(&dest_verify)
    );
}

/// 0.52.0 step 0, Part C2, item 3: the remaining named shapes the handoff lists, beyond the two
/// C1 already covered (sync accept; edited-then-deleted) and the remnant (C1 round 3). Each follows
/// the identical round-trip shape: build the repository state with real commands, confirm the
/// source's own `prikk verify` already passes (the matrix's own premise -- item 3 only requires this
/// round trip for states where `prikk verify` exits 0), then `assert_archive_round_trips_cleanly`.
fn genesis_block_id(repo: &Path, ref_name: &str) -> prikk_object::ObjectId {
    let layout = prikk_store::RepositoryLayout::open(repo.to_path_buf()).expect("open layout");
    let ref_store = prikk_store::RefStore::new(layout.clone());
    let ref_state_id = ref_store
        .read_current_ref_state_id(ref_name)
        .expect("read current ref state id")
        .expect("ref state id present");
    let object_store = prikk_store::FileObjectStore::new(layout);
    let envelope = object_store
        .read_typed(ref_state_id, prikk_object::ObjectType::RefState)
        .expect("read ref state")
        .expect("ref state present");
    let ref_state = prikk_object::RefStatePayload::decode_canonical(
        &envelope.canonical_payload,
        envelope.schema_version,
    )
    .expect("decode ref state");
    ref_state.target_object_id
}

fn assert_source_verify_passes(repo: &Path, shape: &str) {
    let source_verify = support::prikk(repo).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "the {shape} source repository's own prikk verify must already pass (item 3's own premise \
         -- only states `prikk verify` already accepts get this round trip): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );
}

/// Shape: a merge, adopting the other side's sealed patch.
#[test]
fn a_merge_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c2-merge");
    support::init(&repo);
    support::trust_maintainer(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "genesis");
    let baseline = genesis_block_id(&repo, "heads/main");

    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    support::generation(
        &repo,
        "heads/topic",
        "topic.txt",
        b"from topic\n",
        "topic change",
    );

    let merge = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            "heads/topic",
        ])
        .output()
        .unwrap();
    support::ok(&merge, "merge");

    assert_source_verify_passes(&repo, "merge");
    assert_archive_round_trips_cleanly(&repo, "merge");
}

/// Shape: a renamed file (`prikk mv`), sealed.
#[test]
fn a_rename_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c2-rename");
    support::init(&repo);
    support::trust_maintainer(&repo);
    support::generation(
        &repo,
        "heads/main",
        "a.txt",
        b"alpha\nline2\nline3\n",
        "base",
    );

    support::ok(
        &support::prikk(&repo)
            .args(["mv", "a.txt", "b.txt"])
            .output()
            .unwrap(),
        "mv a.txt b.txt",
    );
    commit_and_seal(&repo, "rename a to b");

    assert_source_verify_passes(&repo, "rename");
    assert_archive_round_trips_cleanly(&repo, "rename");
}

/// Shape: a tag.
#[test]
fn a_tag_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c2-tag");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");
    support::ok(
        &support::tag_create(&repo, "tags/v1", "heads/main"),
        "tag create",
    );

    assert_source_verify_passes(&repo, "tag");
    assert_archive_round_trips_cleanly(&repo, "tag");
}

/// Shape: a closed branch (a second RefState naming `kind: Closed`, still carried).
#[test]
fn a_closed_branch_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c2-closed-branch");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");
    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create topic",
    );
    support::ok(
        &support::branch_close(&repo, "heads/topic"),
        "branch close topic",
    );

    assert_source_verify_passes(&repo, "closed branch");
    assert_archive_round_trips_cleanly(&repo, "closed-branch");
}

/// Shape: a received bundle pointer (`remotes/<ref>`), its sealing key adopted so the receiving
/// repository's own `prikk verify` passes cleanly (dc78's own "verify must fail while ... untrusted"
/// -- item 3 only covers states `prikk verify` already accepts, so the key is adopted here, same as
/// the bundle exchange test's own "verify after adopting the sealing key" step).
#[test]
fn a_received_bundle_pointer_exports_verifies_and_imports_cleanly() {
    let origin = support::unique_repo("0520-c2-bundle-origin");
    support::init(&origin);
    support::generation(&origin, "heads/main", "a.txt", b"first\n", "first");

    let bundle_file = archive_path("bundle-received");
    let export = support::prikk(&origin)
        .args([
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&export, "bundle export");

    let repo = support::unique_repo("0520-c2-bundle-receiver");
    support::init(&repo);
    let import = support::prikk(&repo)
        .args(["bundle", "import", "--input", bundle_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&import, "bundle import");
    support::trust_maintainer(&repo);

    assert_source_verify_passes(&repo, "received bundle pointer");
    assert_archive_round_trips_cleanly(&repo, "received-bundle");
}

/// Shape: a queued commit whose ownership was lost to a crash (RFC 166 D5), then restored by
/// `doctor --restore-queue-target`, then sealed normally. The same crash shape
/// `restore_queue_target/tests.rs`'s own `clear_ref_name` simulates store-side, built here at the
/// CLI layer: `default_active_ref_name_path` is a durable-write tear this verb exists to repair, not
/// a slow corruption, so truncating it directly reproduces the same "ownership present right up
/// until it is not" condition on a real, compiled-binary repository.
#[test]
fn a_restored_queue_target_exports_verifies_and_imports_cleanly() {
    let repo = support::unique_repo("0520-c2-restoration");
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"first\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "queued"), "commit");

    let layout = prikk_store::RepositoryLayout::open(repo.clone()).expect("open layout");
    std::fs::write(layout.default_active_ref_name_path(), []).expect("clear ref-name");

    let restore = support::prikk(&repo)
        .args(["doctor", "--restore-queue-target", "--ref", "heads/main"])
        .output()
        .unwrap();
    support::ok(&restore, "doctor --restore-queue-target");

    support::ok(&support::seal(&repo, "heads/main"), "seal");

    assert_source_verify_passes(&repo, "restored queue target");
    assert_archive_round_trips_cleanly(&repo, "restoration");
}

/// Regression (item 3's last bullet): queued, unsealed work still refuses `archive export`,
/// unchanged by anything in C1 or C2's own N1 refactor -- the store-side
/// `archive::tests::export_refuses_queued_unsealed_work` already covers this at the library level;
/// this is the CLI-layer twin, through the compiled binary, since every other shape in this file now
/// goes through the binary too.
#[test]
fn queued_unsealed_work_still_refuses_archive_export() {
    let repo = support::unique_repo("0520-c2-queued-refused");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"queued\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "queued, never sealed"),
        "commit",
    );

    let path = archive_path("queued-refused");
    let export = support::prikk(&repo)
        .args(["archive", "export"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        !export.status.success(),
        "archive export must refuse while a session holds queued, unsealed commits: {}\n{}",
        stdout(&export),
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(
        String::from_utf8_lossy(&export.stderr).contains("queued, unsealed")
            || stdout(&export).contains("queued, unsealed"),
        "the refusal must name the queued, unsealed condition: stdout={}\nstderr={}",
        stdout(&export),
        String::from_utf8_lossy(&export.stderr)
    );
    assert!(
        !path.exists(),
        "a refused export must write nothing: {}",
        path.display()
    );
}

/// 0.52.0 step 0, Part C2, item 4: "perturb the shared predicate" for the claims rule.
///
/// An ordinary `sync accept` + `sync seal` always leaves a claim's own `block_id`/`patch_ids`
/// locally resolvable (`sync seal` recreates the referenced content locally before sealing), so
/// neither `a_repository_that_ran_sync_accept_exports_verifies_and_imports_cleanly` nor a live
/// repository's own `verify` ever exercises "a claim whose own references do not resolve" at all --
/// confirmed empirically (not assumed) before writing this test: a debug probe on that existing
/// test's own fixture showed both `block_id` and every `patch_ids` entry already `contained=true`.
/// To exercise the rule at all, this test crafts a claim directly (maintainer-signed, same shape
/// `seal_from_accepted/tests.rs`'s own `write_claim` builds) naming a patch id nothing in the
/// repository ever wrote -- the one way a claim's own references are genuinely foreign, matching
/// `reachability.rs`'s own doc ("never existence-checked by standing design").
///
/// **Unlike the Block/pointer/log checks C1 made literally shared** (`expand_frontier`,
/// `classify_ref_items`, `ensure_ref_target_valid`), there is no single function a live repository's
/// own `verify` and the archive's own closure pass both call for a `RecognitionClaim`'s references:
/// `crate::verify` never mentions `RecognitionClaim` at all (confirmed: zero matches), so the "never
/// checked" rule is enforced by *absence* on both sides, not by one shared predicate. This test
/// proves both halves of that correctly (the live repository's own `prikk verify` passes over the
/// dangling patch id; so does the archive's own round trip), then perturbs *only*
/// `classify_references` (the one place such a check could even be added, since the store has
/// nothing to perturb) and confirms the archive-side assertion alone goes red -- there being no
/// shared function, the store-side assertion in this same test cannot also go red from the same
/// edit, which is the honest, structurally accurate account of this control, not "both red
/// together" the way C1's own perturbations were.
#[test]
fn a_claims_dangling_references_are_never_checked_by_either_side() {
    let repo = support::unique_repo("0520-c2-claims-rule");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");
    let block_id = genesis_block_id(&repo, "heads/main");

    let layout = prikk_store::RepositoryLayout::open(repo.clone()).expect("open layout");
    let mut object_store = prikk_store::FileObjectStore::new(layout);
    let dangling_patch_id = prikk_object::ObjectId::from_bytes([0xC2; 32]);
    let claim_payload = prikk_object::RecognitionClaimPayload {
        block_id,
        patch_ids: vec![dangling_patch_id],
        parent_block_ids: Vec::new(),
    };
    let bytes =
        prikk_object::CanonicalEncode::to_canonical_bytes(&claim_payload).expect("encode claim");
    let mut claim_envelope = prikk_object::ObjectEnvelope::unsigned(
        prikk_object::ObjectType::RecognitionClaim,
        1,
        bytes,
    );
    let claim_id = claim_envelope.object_id();
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(
        support::MAINTAINER_KEY_ID,
        &support::MAINTAINER_SEED,
    )
    .expect("build signer");
    let signature = prikk_store::maintainer_signature(
        &signer,
        prikk_object::ObjectType::RecognitionClaim,
        claim_id,
    )
    .expect("sign");
    claim_envelope
        .add_signature(signature)
        .expect("attach signature");
    prikk_store::ObjectWriter::write_object(&mut object_store, &claim_envelope)
        .expect("write claim");

    // The live repository's own `prikk verify` -- "the store's own test" -- passes over the
    // dangling patch id: `crate::verify` never walks a claim's own references at all.
    let source_verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert!(
        source_verify.status.success(),
        "the source repository's own prikk verify must pass over a claim's dangling patch \
         reference (the claims rule: never existence-checked): {}\n{}",
        stdout(&source_verify),
        String::from_utf8_lossy(&source_verify.stderr)
    );

    // The archive's own round trip, unperturbed: the identical rule, matching the source.
    assert_archive_round_trips_cleanly(&repo, "claims-rule");
}
