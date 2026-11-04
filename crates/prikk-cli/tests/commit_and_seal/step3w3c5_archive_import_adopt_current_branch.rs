//! 0.56.0 step W3c-5, R3 for `archive import --adopt` onto the current branch
//! (`.git-exclude/reviewed/0.56.0-step-3w3c-4b-review-v1.md`'s own "Next" section, and the live
//! note in `rfcs/handoffs/155-repository-complete-artifact/0.56.0-step-3-implementation-handoff-
//! v1.md`): a `--adopt` landing destined for the current branch now brings a clean worktree along
//! -- the adopted content is materialized into the worktree -- and a dirty worktree, or a file in
//! the way of the adopted tip's own files, refuses before any ref-pointer write.
//!
//! **A fail-fast in-the-way check now exists for this producer too** (0.56.0 step W3c-5b, defect
//! 1, shape (i): `.git-exclude/reviewed/0.56.0-step-3w3c-5-review-v1.md`): `archive::import`'s own
//! `ArchiveEarlyCheckReader` is the full, archive-native, seeking `ObjectReader` over the
//! archive's own Patch/Blob sections the original round's own doc comment said it would need --
//! built this round, measured first against the import's own total time and found to add only
//! about 8.5% of it (`.git-exclude/review-request/0.56.0-step-3w3c-5b-measurement.md`).
//! So, like `sync seal`'s and `merge --into`'s own in-the-way tests now, the in-the-way case below
//! refuses before the journal, before stage 1 ever admits a single object -- the whole repository
//! stays byte-identical, the same as this file's own dirty-worktree test. The unchanged stage-3
//! check stays as a backstop for the one window the early check cannot see into (a file created by
//! a process outside prikk, between the two checks, under the same lock); its own message no
//! longer claims "nothing was written" (it runs after stage 1's unconditional writes and the
//! journal, every time, not only in that rare race) -- see `archive::import`'s own doc comments at
//! both call sites.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_object::ObjectId;
use prikk_store::RefStore;
use prikk_store::RepositoryLayout;

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

/// Build a source repository with one sealed generation on `heads/main` (`path`/`content`),
/// export it to a fresh archive file, and return the archive's own path (caller's responsibility
/// to remove). The source repository is removed before returning -- only the exported bytes
/// matter to every caller below.
fn build_adoptable_archive(tag: &str, path: &str, content: &[u8]) -> PathBuf {
    let src = support::unique_repo(&format!("w3c5-{tag}-src"));
    support::init(&src);
    support::generation(&src, "heads/main", path, content, "genesis");

    let archive_path = std::env::temp_dir().join(format!(
        "prikk-w3c5-{tag}-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&src)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );
    let _ = std::fs::remove_dir_all(&src);
    archive_path
}

/// Every file in the repository, worktree **and** `.prikk` both (except `.prikk/cache`), keyed by
/// its path relative to `repo` -- the identical idiom
/// `step3w3c4_sync_seal_current_branch.rs`'s own `whole_repo` already established and grounded,
/// reused rather than re-derived.
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

fn import_adopt(repo: &Path, archive_path: &Path) -> Output {
    support::prikk(repo)
        .args(["archive", "import", "--adopt"])
        .arg(archive_path)
        .output()
        .unwrap()
}

/// Test 1 (the ruling's own list): a clean worktree is brought along -- `archive import --adopt`,
/// while `heads/main` is current and genuinely unborn and the worktree is clean, materializes the
/// adopted tip's own content into the worktree, and an ordinary `worktree-status` afterward reads
/// clean.
#[test]
fn archive_import_adopt_onto_current_branch_materializes_a_clean_worktree() {
    let archive_path = build_adoptable_archive("clean", "n.txt", b"n\n");
    let dest = support::unique_repo("w3c5-clean-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);

    assert!(
        read_current_ref_state_id(&dest, "heads/main").is_none(),
        "fixture sanity: heads/main must still be unborn before the import"
    );

    let import = import_adopt(&dest, &archive_path);
    assert!(
        import.status.success(),
        "archive import --adopt must succeed on a clean (empty) worktree: stdout={} stderr={}",
        stdout(&import),
        stderr(&import)
    );
    assert!(
        stdout(&import).contains("materialized into the worktree"),
        "the CLI output must say the import materialized the worktree: {}",
        stdout(&import)
    );
    assert_eq!(
        std::fs::read(dest.join("n.txt")).unwrap(),
        b"n\n",
        "the adopted tip's own file must be materialized into the worktree"
    );

    let status = worktree_status(&dest);
    ok(&status, "worktree-status after the import");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the import: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_file(&archive_path);
}

/// Test 2 (the ruling's own list): a dirty worktree refuses, before any write at all -- caught by
/// the fail-fast gate (before the journal, before stage 1 ever runs), so the whole repository
/// stays byte-identical.
///
/// **Constructing a genuinely dirty worktree for an unborn ref needs a second, already-published
/// ref to leave a real record behind first** -- the identical reasoning
/// `step3w3c4_sync_seal_current_branch.rs`'s own dirty fixture already grounds: `heads/main` stays
/// current and unborn throughout; `heads/other` is built the DC-13 way (`commit --ref
/// heads/other`, then `seal`), leaving the standing worktree-baseline record pointed at
/// `heads/other`'s own real, sealed content, physically present in *this* worktree.
#[test]
fn archive_import_adopt_onto_current_branch_refuses_a_dirty_worktree_and_writes_nothing() {
    let archive_path = build_adoptable_archive("dirty", "n.txt", b"n\n");
    let dest = support::unique_repo("w3c5-dirty-dest");
    support::init(&dest);

    std::fs::write(dest.join("o.txt"), b"o\n").unwrap();
    ok(
        &support::commit(&dest, "heads/other", "genesis"),
        "commit --ref heads/other (DC-13 genesis, heads/main stays current)",
    );
    ok(&support::seal(&dest, "heads/other"), "seal heads/other");
    assert_eq!(
        std::fs::read(dest.join("o.txt")).unwrap(),
        b"o\n",
        "fixture sanity: the worktree still faithfully matches the record right after the seal"
    );

    // Trust the maintainer *before* the snapshot below -- `.prikk/trust/*` is one-time fixture
    // setup, not something the refusal's own "writes nothing" guarantee is about.
    support::trust_maintainer(&dest);

    // Dirty the physically-present `o.txt` the standing record describes, without committing it
    // anywhere.
    std::fs::write(dest.join("o.txt"), b"o-modified-uncommitted\n").unwrap();

    assert!(
        verify_text(&dest).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&dest);

    let import = import_adopt(&dest, &archive_path);
    assert!(
        !import.status.success(),
        "archive import --adopt must refuse while the current branch's worktree is dirty: \
         stdout={} stderr={}",
        stdout(&import),
        stderr(&import)
    );
    let message = stderr(&import);
    assert!(
        message.contains("not clean")
            && message.contains("heads/main")
            && message.contains("o.txt"),
        "the refusal must say the worktree is not clean against heads/main, naming o.txt: \
         {message}"
    );

    assert!(
        read_current_ref_state_id(&dest, "heads/main").is_none(),
        "a refused import must not publish heads/main"
    );
    assert_eq!(
        whole_repo(&dest),
        before,
        "a refused import must leave every file -- worktree AND .prikk -- byte-identical: the \
         fail-fast gate runs before the journal, before stage 1 ever writes a single object"
    );
    assert!(
        verify_text(&dest).contains("unreached objects: 0"),
        "a refused import must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&dest)
    );

    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_file(&archive_path);
}

/// Test 3 (the ruling's own list): a file in the way of the adopted tip's own target file must
/// refuse before any write at all -- 0.56.0 step W3c-5b (defect 1, shape (i))'s own early check,
/// run from the archive directly, before the journal and before stage 1 ever admits a single
/// object. Since `heads/main` is unborn and has no record at all here, an untracked file at the
/// adopted content's own path is never flagged by the clean check (untracked is never a reason to
/// refuse) -- it is this in-the-way check, run against the archive's own carried tip, that must
/// catch it, and the whole repository must stay byte-identical, the same as this file's own
/// dirty-worktree test above.
#[test]
fn archive_import_adopt_onto_current_branch_a_file_in_the_way_refuses_and_writes_nothing() {
    let archive_path = build_adoptable_archive("in-the-way", "n.txt", b"from-archive\n");
    let dest = support::unique_repo("w3c5-in-the-way-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);

    // The untracked file already in the way, at the exact path the adopted content adds --
    // different content than the import will bring, so the collision is real.
    std::fs::write(dest.join("n.txt"), b"zzz\n").unwrap();

    assert!(
        verify_text(&dest).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );
    let before = whole_repo(&dest);

    let import = import_adopt(&dest, &archive_path);
    assert!(
        !import.status.success(),
        "archive import --adopt must refuse while a file is in the way of the adopted tip's own \
         files: stdout={} stderr={}",
        stdout(&import),
        stderr(&import)
    );
    let message = stderr(&import);
    assert!(
        message.contains("are in the way of heads/main's files")
            && message.contains("n.txt")
            && message.contains("before adopting heads/main")
            && message.contains("nothing was written"),
        "the pre-write refusal must name the action, heads/main, and n.txt, and truthfully say \
         nothing was written: {message}"
    );

    assert!(
        read_current_ref_state_id(&dest, "heads/main").is_none(),
        "a refused import must not publish heads/main's own ref-pointer"
    );
    assert_eq!(
        std::fs::read(dest.join("n.txt")).unwrap(),
        b"zzz\n",
        "the untracked file in the way must never be overwritten by a refused import"
    );
    assert_eq!(
        whole_repo(&dest),
        before,
        "a refused import must leave every file -- worktree AND .prikk -- byte-identical: the \
         early in-the-way check runs before the journal, before stage 1 ever writes a single \
         object"
    );
    assert!(
        verify_text(&dest).contains("unreached objects: 0"),
        "a refused import must not leave an orphaned, unreferenced object behind: {}",
        verify_text(&dest)
    );
    // The ruling's own "no writer blocked" check: no journal was ever created, so `doctor`
    // reports no interrupted import (the review's own measured defect left one behind, which an
    // unrelated writer then refused against -- see the dedicated test below for that half).
    let doctor = support::prikk(&dest).arg("doctor").output().unwrap();
    assert!(
        doctor.status.success(),
        "doctor must exit 0 after a refused import that wrote nothing: stdout={} stderr={}",
        stdout(&doctor),
        stderr(&doctor)
    );

    // After the offending file is moved aside, the same command succeeds and materializes --
    // nothing was written on the first attempt (confirmed above), so this retry is a genuinely
    // fresh run, not a resume: there is no journal to resume from.
    std::fs::rename(dest.join("n.txt"), dest.join("n.txt.aside")).unwrap();
    let retry = import_adopt(&dest, &archive_path);
    assert!(
        retry.status.success(),
        "archive import --adopt must succeed once the file in the way is moved aside: stdout={} \
         stderr={}",
        stdout(&retry),
        stderr(&retry)
    );
    assert!(
        stdout(&retry).contains("materialized into the worktree"),
        "the CLI output must say the retried import materialized the worktree: {}",
        stdout(&retry)
    );
    assert_eq!(
        std::fs::read(dest.join("n.txt")).unwrap(),
        b"from-archive\n"
    );

    std::fs::remove_file(dest.join("n.txt.aside")).unwrap();
    let status = worktree_status(&dest);
    ok(&status, "worktree-status after the retried import");
    assert!(
        stdout(&status).contains("worktree: clean against baseline"),
        "the worktree must read clean and current after the retried import: {}",
        stdout(&status)
    );

    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_file(&archive_path);
}

/// The other half of the ruling's own "no writer blocked" check, isolated in its own repository
/// so the unrelated commit it authors (on a *different* ref, `heads/other`) cannot affect the
/// `heads/main`-focused assertions the test above already makes: after a refused import that
/// wrote nothing (the collision from the test above, reproduced fresh here), an unrelated
/// `commit` on another ref proceeds normally -- not refused by a lingering journal/lock the way
/// the review's own measured defect left behind before this round.
#[test]
fn archive_import_adopt_onto_current_branch_a_file_in_the_way_refusal_does_not_block_an_unrelated_commit()
 {
    let archive_path = build_adoptable_archive("in-the-way-unrelated", "n.txt", b"from-archive\n");
    let dest = support::unique_repo("w3c5-in-the-way-unrelated-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);

    std::fs::write(dest.join("n.txt"), b"zzz\n").unwrap();
    let import = import_adopt(&dest, &archive_path);
    assert!(
        !import.status.success(),
        "fixture sanity: the import must refuse on the collision: stdout={} stderr={}",
        stdout(&import),
        stderr(&import)
    );

    std::fs::remove_file(dest.join("n.txt")).unwrap();
    std::fs::write(dest.join("unrelated.txt"), b"unrelated\n").unwrap();
    let unrelated_commit = support::commit(&dest, "heads/other", "unrelated work");
    assert!(
        unrelated_commit.status.success(),
        "an unrelated commit on a different ref must proceed normally after a refused import \
         that wrote nothing: stdout={} stderr={}",
        stdout(&unrelated_commit),
        stderr(&unrelated_commit)
    );
    ok(&support::seal(&dest, "heads/other"), "seal heads/other");

    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_file(&archive_path);
}

/// Red control (0.56.0 step W3c-5b, defect 1): with the new early check disabled
/// (`PRIKK_ARCHIVE_IMPORT_DISABLE_EARLY_IN_THE_WAY_CHECK_FOR_TEST`, a `test-support`-gated seam --
/// see `archive::import`'s own doc comment at its one call site), the collision above is caught
/// only by the unchanged stage-3 backstop: stage 1 has already admitted every carried object and
/// the journal already exists by the time it fires, so `unreached objects` is no longer `0`, the
/// whole repository is no longer byte-identical, and the refusal's own sentence is the corrected
/// "already admitted ... already written" text, not "nothing was written" -- proving the
/// byte-identical, zero-unreached-objects assertions in the test above genuinely depend on the new
/// early check running, not on some other, vacuous reason.
#[test]
fn archive_import_adopt_onto_current_branch_a_file_in_the_way_with_the_early_check_disabled_writes_stage_1()
 {
    let archive_path = build_adoptable_archive("in-the-way-red", "n.txt", b"from-archive\n");
    let dest = support::unique_repo("w3c5-in-the-way-red-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);

    std::fs::write(dest.join("n.txt"), b"zzz\n").unwrap();

    assert!(
        verify_text(&dest).contains("unreached objects: 0"),
        "fixture sanity: no unreached objects before the refusal"
    );

    let import = support::prikk(&dest)
        .env(
            "PRIKK_ARCHIVE_IMPORT_DISABLE_EARLY_IN_THE_WAY_CHECK_FOR_TEST",
            "1",
        )
        .args(["archive", "import", "--adopt"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        !import.status.success(),
        "the stage-3 backstop must still refuse the collision even with the early check \
         disabled: stdout={} stderr={}",
        stdout(&import),
        stderr(&import)
    );
    let message = stderr(&import);
    assert!(
        message.contains("are in the way of heads/main's files")
            && message.contains("n.txt")
            && message.contains("already admitted")
            && message.contains("already written")
            && !message.contains("nothing was written"),
        "with the early check disabled, the backstop's own corrected sentence must name what was \
         actually written, not claim nothing was: {message}"
    );

    assert!(
        verify_text(&dest).contains("unreached objects: 2"),
        "with the early check disabled, stage 1 admits this archive's own objects (the block and \
         the blob) before the stage-3 backstop ever refuses, leaving them locally present but \
         unreached: {}",
        verify_text(&dest)
    );
    let doctor = support::prikk(&dest).arg("doctor").output().unwrap();
    assert!(
        !doctor.status.success(),
        "with the early check disabled, the journal is left behind, and doctor must report the \
         interrupted import: stdout={} stderr={}",
        stdout(&doctor),
        stderr(&doctor)
    );

    let _ = std::fs::remove_dir_all(&dest);
    let _ = std::fs::remove_file(&archive_path);
}
