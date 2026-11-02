//! 0.56.0 step W3c-2, R3 for `branch create --from` of the unborn current branch
//! (`.git-exclude/reviewed/0.56.0-step-3w3c-1-review-v1.md`'s own ruling for this producer): a
//! clean worktree is brought along -- the `--from` target's files are materialized into the
//! worktree -- and a dirty one refuses, before any write (`require_worktree_clean_and_capture`,
//! re-exported narrowly from `prikk-store`, called from `crates/prikk-cli/src/branch.rs::run_create`
//! directly, the same precedent `switch_branch`/`worktree_status` already set).
//!
//! The crash-between-publish-and-materialize case needs an in-process failpoint and is **not**
//! duplicated here: it already lives, generically, in
//! `crates/prikk-store/src/worktree_materialize/tests.rs`
//! (`a_crash_between_publish_and_materialize_recovers_via_branch_switch`), which drives
//! `require_worktree_clean_and_capture`/`mark_current_branch_switch_pending`/
//! `materialize_after_publish` directly against a hand-built state -- it knows nothing about which
//! producer called it, so it already proves this shape for `branch create --from` exactly as it does
//! for `merge --into` (confirmed: nothing in this round's wiring changes what happens between the
//! publish and the materialize call, only what happens *before* the publish).

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

fn switch(repo: &Path, target: &str) -> Output {
    support::prikk(repo)
        .args(["branch", "switch", target])
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
/// its path relative to `repo` -- the identical idiom
/// `step3w3c1_merge_refusal_writes_nothing.rs`'s own `whole_repo` already established and grounded
/// (confirmed there by running once without the exclusion: only cache-directory bytes differ, never
/// `.prikk/containers`/`.prikk/refs`/`.prikk/active`), reused rather than re-derived.
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

/// Build `heads/other` as a real local branch holding `o.txt`, with content this repository's own
/// worktree never physically received -- the identical mechanism
/// `step3w2_worktree_baseline.rs`'s own §2.5 case (`case5_branch_create_from_onto_unborn_current_*`)
/// already established and grounded: a second, disposable `src` repository builds and seals `o.txt`
/// on its own `heads/main`, and `sync have`/`sync build`/`sync accept`/`sync seal --claim` carries
/// the sealed content into `repo` as `heads/other`, never touching `repo`'s own worktree (confirmed
/// there: `branch create --from` refuses a *received* ref outright, so `heads/other` must be a real
/// local branch first, and building it via a plain local `commit --ref heads/other` from this same
/// worktree would leave `o.txt` physically present already, defeating the point of this fixture).
fn build_heads_other_with_content_never_physically_received(repo: &Path) {
    let src = support::unique_repo("w3c2-branch-create-src");
    support::init(&src);
    support::generation(&src, "heads/main", "o.txt", b"o\n", "genesis");

    let have_file =
        std::env::temp_dir().join(format!("w3c2-have-{}.bin", support::unique_suffix()));
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
        std::env::temp_dir().join(format!("w3c2-artifact-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(&src)
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
        .to_string();
    support::trust_maintainer(repo);
    let sync_seal = support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/other", "--claim", &claim_id])
        .output()
        .unwrap();
    ok(
        &sync_seal,
        "sync seal heads/other --claim (builds the local sibling)",
    );
    assert!(
        !repo.join("o.txt").exists(),
        "fixture sanity: sync seal never touches the worktree"
    );

    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_file(&have_file);
    let _ = std::fs::remove_file(&artifact_file);
}

/// Test 1 (the ruling's own list): a clean worktree is brought along -- `branch create heads/main
/// --from heads/other`, while `heads/main` is current and genuinely unborn (the RFC 151 §2.2 default
/// right after `init`, never committed or sealed locally), materializes `heads/other`'s content into
/// the worktree, the record ends current, and an ordinary commit afterward has nothing to do.
#[test]
fn branch_create_from_onto_unborn_current_branch_materializes_a_clean_worktree() {
    let repo = support::unique_repo("w3c2-create-from-clean");
    support::init(&repo);
    build_heads_other_with_content_never_physically_received(&repo);

    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "fixture sanity: heads/main must still be unborn before the create"
    );

    let create = support::branch_create(&repo, "heads/main", "heads/other");
    assert!(
        create.status.success(),
        "branch create --from must succeed on a clean (empty) worktree: stdout={} stderr={}",
        stdout(&create),
        stderr(&create)
    );
    assert!(
        stdout(&create).contains("materialized into the worktree"),
        "the CLI output must say the create materialized the worktree: {}",
        stdout(&create)
    );

    // The adopted file is really on disk, not merely recorded as adopted.
    assert_eq!(
        std::fs::read(repo.join("o.txt")).unwrap(),
        b"o\n",
        "heads/other's file must be materialized into the worktree"
    );

    let status = support::prikk(&repo)
        .args(["worktree-status"])
        .output()
        .unwrap();
    ok(&status, "worktree-status after the create");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the create: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Test 2 (the ruling's own list): a dirty worktree refuses, before any write, and leaves nothing
/// behind -- not even an orphaned object (the identical, stronger, whole-repository byte-identity
/// claim `step3w3c1_merge_refusal_writes_nothing.rs` makes for `merge`).
///
/// **Constructing a genuinely dirty worktree for an unborn ref needs a second, already-published
/// ref to leave a real record behind first** (confirmed by reading `worktree_baseline.rs`'s own
/// comparison rule: a never-touched unborn ref's record is the empty baseline `init` itself writes,
/// which trivially matches that same ref's own empty live state -- there is nothing yet for it to be
/// stale *against*). `current_branch()`'s own resolution (`refs.rs`, confirmed by reading it) treats
/// *any* unpublished pointer other than the literal default (`heads/main`) as "does not exist," so
/// the "unborn current branch" shape is always `heads/main` itself, never an arbitrary name.
///
/// This fixture reproduces the identical "stuck" shape `branch_switch.rs`'s own module doc and
/// `worktree-patch.md`'s own walkthrough already name: `heads/main` stays current and unborn, while
/// a **different**, brand-new ref (`heads/other`) is built the DC-13 way (`commit --ref heads/other`
/// on a name that has never existed, then `seal`) -- both steps run while `heads/main` is current,
/// writing `o.txt` physically into *this* worktree as `heads/other`'s own genesis content. `seal`
/// advances the *standing* (ref-name-agnostic) worktree-baseline record to `heads/other`'s own real,
/// sealed block (confirmed from `worktree_baseline_matches_fold_being_sealed`'s own doc: the record
/// and the fold being sealed still match right up to the seal, so `seal` advances it rather than
/// leaving it behind). Checking that same record against `heads/main` (still unborn, live baseline
/// `None`) now reads `Behind`, regardless of ref name (`compare_worktree_baseline`'s own rule is
/// state, not name, confirmed by that module's own doc). Modifying the physically-present `o.txt`
/// afterward, without committing it anywhere, is what the record-vs-worktree comparison actually
/// reports as dirty -- a ref-name mismatch alone is not a hazard (the same module's own doc),
/// confirmed by this fixture's own sanity check that the worktree still faithfully matches the
/// record immediately after the seal, before that last edit.
#[test]
fn branch_create_from_onto_unborn_current_branch_refuses_a_dirty_worktree_and_writes_nothing() {
    let repo = support::unique_repo("w3c2-create-from-dirty");
    support::init(&repo);

    // `heads/main` stays current and unborn throughout (RFC 147 §2i: the *only* ref name
    // `current_branch()` ever resolves while unpublished is the default -- confirmed from
    // `refs.rs::current_branch`'s own `UNBORN_DEFAULT_BRANCH` special case; a pointer naming any
    // other unpublished name refuses outright, so the "unborn current branch" shape is always
    // `heads/main` specifically, not an arbitrary name). `heads/other` is built as a genuinely
    // *different*, real local branch the DC-13 way (`commit --ref heads/other` on a brand-new
    // name, then `seal`), while `heads/main` is current -- exactly the
    // `worktree-patch.md`/`branch_switch.rs` walkthrough's own "stuck" shape: `seal` advances the
    // *standing* worktree-baseline record to `heads/other`'s own real, sealed content (confirmed
    // from `worktree_baseline_matches_fold_being_sealed`'s own doc: the record and the fold being
    // sealed match right up to the seal, so `seal` advances it), even though the record's own
    // comparison is state-, not name-, keyed (`compare_worktree_baseline`'s own doc), so checking
    // it against `heads/main` (still unborn, live baseline `None`) now reads `Behind` against that
    // real block.
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

    // Now dirty the physically-present `o.txt` the standing record describes, without committing
    // it anywhere -- without this step the worktree would still faithfully match the record (a
    // ref-name mismatch alone is not a hazard, grounded above) and read clean.
    std::fs::write(repo.join("o.txt"), b"o-modified-uncommitted\n").unwrap();

    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&repo);

    let create = support::branch_create(&repo, "heads/main", "heads/other");
    assert!(
        !create.status.success(),
        "branch create --from must refuse while the current branch's worktree is dirty: \
         stdout={} stderr={}",
        stdout(&create),
        stderr(&create)
    );
    let message = stderr(&create);
    assert!(
        message.contains("not clean")
            && message.contains("heads/main")
            && message.contains("o.txt"),
        "the refusal must say the worktree is not clean against heads/main, naming o.txt: {message}"
    );

    assert!(
        read_current_ref_state_id(&repo, "heads/main").is_none(),
        "a refused create must not publish heads/main"
    );
    assert_eq!(
        whole_repo(&repo),
        before,
        "a refused create must leave every file -- worktree AND .prikk -- byte-identical; a stray \
         object written to the object store before the refusal would show up here even though no \
         ref ever named it"
    );
    assert!(
        verify_text(&repo).contains("unreached objects: 0"),
        "a refused create must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&repo)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// `branch create --from` of a ref that is **not** the current branch is entirely unaffected by R3:
/// no materialization, no clean-worktree precondition -- the worktree (left on `heads/main`) is
/// never even read. The ordinary, pre-existing behavior this round must not have changed.
#[test]
fn branch_create_from_a_ref_that_is_not_current_is_unaffected_by_r3() {
    let repo = support::unique_repo("w3c2-create-from-not-current");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"a\n", "genesis");
    ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create heads/other --from heads/main",
    );
    ok(&switch(&repo, "heads/other"), "switch to heads/other");
    support::generation(
        &repo,
        "heads/other",
        "c.txt",
        b"sibling content\n",
        "other adds its own file",
    );
    ok(&switch(&repo, "heads/main"), "switch back to heads/main");

    // Dirty the worktree deliberately -- must not matter at all, since `heads/target` (the ref
    // being created) is not the current branch (`heads/main` is).
    std::fs::write(repo.join("a.txt"), b"dirty, but irrelevant\n").unwrap();

    let create = support::branch_create(&repo, "heads/target", "heads/other");
    assert!(
        create.status.success(),
        "creating a branch that is not the current branch must ignore worktree cleanliness: \
         stdout={} stderr={}",
        stdout(&create),
        stderr(&create)
    );
    assert!(
        !stdout(&create).contains("materialized into the worktree"),
        "a create whose own new ref is not current must not claim to materialize anything: {}",
        stdout(&create)
    );
    assert!(
        !repo.join("c.txt").exists(),
        "heads/target's own target file must not appear in heads/main's worktree"
    );
    assert_eq!(
        tip_block_id(&repo, "heads/target"),
        tip_block_id(&repo, "heads/other"),
        "heads/target must point at exactly heads/other's own tip"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
