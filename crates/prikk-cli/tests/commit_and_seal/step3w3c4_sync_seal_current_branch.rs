//! 0.56.0 step W3c-4, R3 for `sync seal` onto the current branch
//! (`.git-exclude/reviewed/0.56.0-step-3w3c-1-review-v1.md`'s own ruling for this producer, and
//! the live note in `rfcs/handoffs/155-repository-complete-artifact/0.56.0-step-3-implementation-
//! handoff-v1.md`): a claim (or batch of claims) sealed onto the current branch now brings a
//! clean worktree along -- the claim's own content is materialized into the worktree -- and a
//! dirty worktree, or a file in the way of the claim's own target files, refuses before any claim
//! seals. A single `--claim <id>` is treated as a batch of exactly one claim throughout
//! (`crates/prikk-cli/src/sync.rs::run_seal`), so every test below uses `--claim`, except the
//! mid-batch case, which needs `--claims <file>` by construction.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn worktree_status(repo: &Path) -> Output {
    support::prikk(repo)
        .args(["worktree-status"])
        .output()
        .unwrap()
}

fn verify_text(repo: &Path) -> String {
    let output = support::prikk(repo).arg("verify").output().unwrap();
    format!("{}{}", stdout(&output), stderr(&output))
}

fn read_current_ref_state_id(repo: &Path, ref_name: &str) -> Option<ObjectId> {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
}

fn tip_block_id(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout);
    let ref_state_id = read_current_ref_state_id(repo, ref_name).unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

/// Every file in the repository, worktree **and** `.prikk` both (except `.prikk/cache`), keyed by
/// its path relative to `repo` -- the identical idiom `step3w3c1_merge_refusal_writes_nothing.rs`'s
/// own `whole_repo` already established and grounded, reused rather than re-derived.
fn whole_repo(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == "cache")
                && path
                    .parent()
                    .is_some_and(|parent| parent == repo.join(".prikk"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(
                    path.strip_prefix(repo).unwrap().to_path_buf(),
                    std::fs::read(&path).unwrap(),
                );
            }
        }
    }
    files
}

/// Build a claim, accepted into `repo`, naming a single real patch (`path`/`content`) sealed on a
/// disposable sender's own `heads/main` -- the ordinary RFC 116 loop
/// (`have`/`build`/`accept --claims-out`), content this repository's own worktree never
/// physically received. Returns the accepted claim id.
fn accept_one_claim(repo: &Path, path: &str, content: &[u8]) -> ObjectId {
    let sender = support::unique_repo(&format!("w3c4-sender-{}", support::unique_suffix()));
    support::init(&sender);
    support::generation(&sender, "heads/main", path, content, "genesis");

    let have_file =
        std::env::temp_dir().join(format!("w3c4-have-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(repo)
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
    let artifact_file =
        std::env::temp_dir().join(format!("w3c4-artifact-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(&sender)
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
            .unwrap(),
        "sync build",
    );
    let accept = support::prikk(repo)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    ok(&accept, "sync accept");
    let accept_stdout = stdout(&accept);
    let claim_id = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .and_then(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .trim()
        .parse::<ObjectId>()
        .unwrap();

    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
    claim_id
}

fn seal_claim(repo: &Path, ref_name: &str, claim_id: ObjectId) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", ref_name, "--claim", &claim_id.to_string()])
        .output()
        .unwrap()
}

/// Test 1 (the ruling's own list): a clean worktree is brought along -- `sync seal heads/main
/// --claim <id>`, while `heads/main` is current and genuinely unborn, materializes the claim's own
/// content into the worktree, and an ordinary `worktree-status` afterward reads clean.
#[test]
fn sync_seal_onto_current_branch_materializes_a_clean_worktree() {
    let repo = support::unique_repo("w3c4-seal-clean");
    support::init(&repo);
    let claim_id = accept_one_claim(&repo, "o.txt", b"o\n");

    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "fixture sanity: heads/main must still be unborn before the seal"
    );

    let seal = seal_claim(&repo, "heads/main", claim_id);
    assert!(
        seal.status.success(),
        "sync seal must succeed on a clean (empty) worktree: stdout={} stderr={}",
        stdout(&seal),
        stderr(&seal)
    );
    assert!(
        stdout(&seal).contains("materialized into the worktree"),
        "the CLI output must say the seal materialized the worktree: {}",
        stdout(&seal)
    );
    assert_eq!(
        std::fs::read(repo.join("o.txt")).unwrap(),
        b"o\n",
        "the claim's own file must be materialized into the worktree"
    );

    let status = worktree_status(&repo);
    ok(&status, "worktree-status after the seal");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the seal: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 2 (the ruling's own list): a dirty worktree refuses, before any claim seals, and leaves
/// nothing behind -- not even an orphaned object.
///
/// **Constructing a genuinely dirty worktree for an unborn ref needs a second, already-published
/// ref to leave a real record behind first** -- the identical reasoning
/// `step3w3c2_branch_create_from_current_branch.rs`'s own dirty fixture already grounds:
/// `heads/main` stays current and unborn throughout; `heads/other` is built the DC-13 way
/// (`commit --ref heads/other`, then `seal`), leaving the standing worktree-baseline record
/// pointed at `heads/other`'s own real, sealed content, physically present in *this* worktree.
#[test]
fn sync_seal_onto_current_branch_refuses_a_dirty_worktree_and_writes_nothing() {
    let repo = support::unique_repo("w3c4-seal-dirty");
    support::init(&repo);

    std::fs::write(repo.join("o.txt"), b"o\n").unwrap();
    ok(
        &support::commit(&repo, "heads/other", "genesis"),
        "commit --ref heads/other (DC-13 genesis, heads/main stays current)",
    );
    ok(&support::seal(&repo, "heads/other"), "seal heads/other");
    assert_eq!(
        std::fs::read(repo.join("o.txt")).unwrap(),
        b"o\n",
        "fixture sanity: the worktree still faithfully matches the record right after the seal"
    );

    let claim_id = accept_one_claim(&repo, "q.txt", b"q\n");

    // Dirty the physically-present `o.txt` the standing record describes, without committing it
    // anywhere.
    std::fs::write(repo.join("o.txt"), b"o-modified-uncommitted\n").unwrap();

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&repo);

    let seal = seal_claim(&repo, "heads/main", claim_id);
    assert!(
        !seal.status.success(),
        "sync seal must refuse while the current branch's worktree is dirty: stdout={} stderr={}",
        stdout(&seal),
        stderr(&seal)
    );
    let message = stderr(&seal);
    assert!(
        message.contains("not clean")
            && message.contains("heads/main")
            && message.contains("o.txt"),
        "the refusal must say the worktree is not clean against heads/main, naming o.txt: {message}"
    );

    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "a refused seal must not publish heads/main"
    );
    assert_eq!(
        whole_repo(&repo),
        before,
        "a refused seal must leave every file -- worktree AND .prikk -- byte-identical; a stray \
         object written to the object store before the refusal would show up here even though no \
         ref ever named it"
    );
    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a refused seal must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&repo)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 3 (the ruling's own list): a file in the way of the claim's own target file must refuse
/// *before* the seal publishes `heads/main` -- the same "before any write" guarantee
/// `branch create --from`/`merge --into` already have. Since `heads/main` is unborn and has no
/// record at all here, an untracked file at the claim's own path is never flagged by the clean
/// check (`dirty_worktree_entries`'s own rule: untracked is never a reason to refuse) -- it is the
/// new pre-write in-the-way check, run against the claim's own simulated target state, that must
/// catch it.
#[test]
fn sync_seal_onto_current_branch_a_file_in_the_way_refuses_before_publish_then_resyncs() {
    let repo = support::unique_repo("w3c4-seal-in-the-way");
    support::init(&repo);
    let claim_id = accept_one_claim(&repo, "p.txt", b"side-p\n");

    // The untracked file already in the way, at the exact path the claim's own content adds --
    // different content than the claim will bring, so the collision is real.
    std::fs::write(repo.join("p.txt"), b"zzz\n").unwrap();

    // Trust the maintainer *before* the snapshot below -- `seal_claim`'s own first step
    // (`support::trust_maintainer`) would otherwise write `.prikk/trust/*` for the first time
    // *after* `before` is captured, which is ordinary one-time fixture setup, not something the
    // refusal's own "writes nothing" guarantee is about (idempotent: calling it again from inside
    // `seal_claim` changes nothing further).
    support::trust_maintainer(&repo);

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&repo);

    let seal = seal_claim(&repo, "heads/main", claim_id);
    assert!(
        !seal.status.success(),
        "sync seal must refuse while a file is in the way of the claim's own target files: \
         stdout={} stderr={}",
        stdout(&seal),
        stderr(&seal)
    );
    let message = stderr(&seal);
    assert!(
        message.contains("are in the way of heads/main's files")
            && message.contains("p.txt")
            && message.contains("move them aside before sealing onto heads/main")
            && message.contains("nothing was written"),
        "the pre-write refusal must name the action, heads/main, p.txt, and say nothing was \
         written: {message}"
    );
    assert!(
        !message.contains("was published"),
        "the old post-publish backstop's own sentence ('... was published, but ...') must not be \
         the one that fires here -- the new pre-write check must refuse first: {message}"
    );

    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "a refused seal must not publish heads/main"
    );
    assert_eq!(
        whole_repo(&repo),
        before,
        "a refused seal must leave every file -- worktree AND .prikk -- byte-identical; this also \
         proves no marker was set"
    );
    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a refused seal must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&repo)
    );
    assert_eq!(
        std::fs::read(repo.join("p.txt")).unwrap(),
        b"zzz\n",
        "the untracked file in the way must never be overwritten by a refused seal"
    );

    // After the offending file is moved aside, the same command succeeds and materializes.
    std::fs::rename(repo.join("p.txt"), repo.join("p.txt.aside")).unwrap();
    let retry = seal_claim(&repo, "heads/main", claim_id);
    assert!(
        retry.status.success(),
        "sync seal must succeed once the file in the way is moved aside: stdout={} stderr={}",
        stdout(&retry),
        stderr(&retry)
    );
    assert!(
        stdout(&retry).contains("materialized into the worktree"),
        "the CLI output must say the retried seal materialized the worktree: {}",
        stdout(&retry)
    );
    assert_eq!(std::fs::read(repo.join("p.txt")).unwrap(), b"side-p\n");

    std::fs::remove_file(repo.join("p.txt.aside")).unwrap();
    let status = worktree_status(&repo);
    ok(&status, "worktree-status after the retried seal");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the retried seal: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 4 (the ruling's own list): a mid-batch failure -- after at least one claim has already
/// published, the batch's own error is still returned, but the published tip is materialized into
/// the worktree anyway, so the worktree matches what is really there.
///
/// **Construction, not contrived** (the handoff's own ask): two senders, `a1` and `a2`, each
/// independently seal their own genesis `collide.txt` (different content) from the *same* empty
/// starting state. `repo`'s own `heads/main` is itself unborn/empty, so both claims target the
/// same new path from the same (absent) parent -- an ordinary divergence between two histories
/// that moved differently, not a broken repository. The pre-write in-the-way check defers to the
/// real per-claim loop whenever its own simulation cannot derive a combined state at all (see
/// `require_nothing_in_the_way_for_seal_batch`'s own doc in `seal_from_accepted.rs`), so whichever
/// claim `order_claims_for_sealing` tries first genuinely seals and publishes, and only the second
/// one fails -- this is the identical fixture shape
/// `rfc116_sync_cli.rs::row6_a_mid_batch_seal_failure_stops_reports_and_leaves_the_earlier_seal_intact`
/// already uses (that test predates this round and keeps holding unchanged); this test adds the
/// one assertion that round's own test never needed: that the published tip is also materialized.
#[test]
fn sync_seal_mid_batch_failure_materializes_the_published_tip() {
    let repo_a1 = support::unique_repo("w3c4-mid-batch-a1");
    support::init(&repo_a1);
    support::generation(&repo_a1, "heads/main", "collide.txt", b"from a1\n", "a1");

    let repo_a2 = support::unique_repo("w3c4-mid-batch-a2");
    support::init(&repo_a2);
    support::generation(&repo_a2, "heads/main", "collide.txt", b"from a2\n", "a2");

    let repo = support::unique_repo("w3c4-mid-batch-b");
    support::init(&repo);

    let claims_file = std::env::temp_dir().join(format!(
        "w3c4-mid-batch-claims-{}.txt",
        support::unique_suffix()
    ));
    let mut combined = String::new();
    for sender in [&repo_a1, &repo_a2] {
        let have_file = std::env::temp_dir().join(format!(
            "w3c4-mid-batch-have-{}.bin",
            support::unique_suffix()
        ));
        ok(
            &support::prikk(&repo)
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
        let artifact_file = std::env::temp_dir().join(format!(
            "w3c4-mid-batch-artifact-{}.bin",
            support::unique_suffix()
        ));
        ok(
            &support::prikk(sender)
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
                .unwrap(),
            "sync build",
        );
        let claims_out = std::env::temp_dir().join(format!(
            "w3c4-mid-batch-claims-out-{}.txt",
            support::unique_suffix()
        ));
        ok(
            &support::prikk(&repo)
                .args([
                    "sync",
                    "accept",
                    artifact_file.to_str().unwrap(),
                    "--claims-out",
                    claims_out.to_str().unwrap(),
                ])
                .output()
                .unwrap(),
            "sync accept",
        );
        combined.push_str(&std::fs::read_to_string(&claims_out).unwrap());
        let _ = std::fs::remove_file(have_file);
        let _ = std::fs::remove_file(artifact_file);
        let _ = std::fs::remove_file(claims_out);
    }
    std::fs::write(&claims_file, &combined).unwrap();

    support::trust_maintainer(&repo);
    let seal = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "seal",
            "heads/main",
            "--claims",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !seal.status.success(),
        "a mid-batch seal failure must exit non-zero"
    );
    let seal_stdout = stdout(&seal);
    assert!(
        seal_stdout.contains("sealed 1 patch(es)"),
        "the first claim must still be reported as sealed: {seal_stdout}"
    );
    assert!(
        seal_stdout.contains("FAILED"),
        "the second claim's failure must be reported, not silently swallowed: {seal_stdout}"
    );
    // The new assertion this round adds: the published (first) tip is materialized anyway.
    assert!(
        seal_stdout.contains("materialized into the worktree"),
        "0.56.0 step W3c-4: a mid-batch failure must still materialize the tip that was \
         published: {seal_stdout}"
    );
    let collide = std::fs::read(repo.join("collide.txt")).unwrap();
    assert!(
        collide == b"from a1\n" || collide == b"from a2\n",
        "collide.txt must be materialized with whichever claim's content actually sealed first: \
         {collide:?}"
    );

    let status = worktree_status(&repo);
    ok(
        &status,
        "worktree-status after the mid-batch failure's own materialize",
    );
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean against the real, published tip even though the batch as a \
         whole failed: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo_a1);
    let _ = std::fs::remove_dir_all(&repo_a2);
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_file(&claims_file);
}

/// `sync seal` of a ref that is **not** the current branch is entirely unaffected by R3: no
/// materialization, no clean-worktree precondition -- the worktree (left on `heads/main`) is never
/// even read. The ordinary, pre-existing behavior this round must not have changed.
#[test]
fn sync_seal_onto_a_ref_that_is_not_current_is_unaffected_by_r3() {
    let repo = support::unique_repo("w3c4-seal-not-current");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create heads/other --from heads/main",
    );

    let claim_id = accept_one_claim(&repo, "o.txt", b"o\n");

    // Dirty the worktree deliberately -- must not matter at all, since `heads/other` (the ref
    // being sealed onto) is not the current branch (`heads/main` is).
    std::fs::write(repo.join("a.txt"), b"dirty, but irrelevant\n").unwrap();

    let seal = seal_claim(&repo, "heads/other", claim_id);
    assert!(
        seal.status.success(),
        "sealing a ref that is not the current branch must ignore worktree cleanliness: \
         stdout={} stderr={}",
        stdout(&seal),
        stderr(&seal)
    );
    assert!(
        !stdout(&seal).contains("materialized into the worktree"),
        "a seal whose own target ref is not current must not claim to materialize anything: {}",
        stdout(&seal)
    );
    assert!(
        !repo.join("o.txt").exists(),
        "heads/other's own target file must not appear in heads/main's worktree"
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"dirty, but irrelevant\n",
        "the deliberately dirty, irrelevant file must be left exactly as written"
    );
    assert_ne!(
        tip_block_id(&repo, "heads/other"),
        tip_block_id(&repo, "heads/main")
    );

    let _ = std::fs::remove_dir_all(&repo);
}
