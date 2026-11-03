//! 0.56.0 step W3c-5, R3 for `archive import --adopt` onto the current branch
//! (`.git-exclude/reviewed/0.56.0-step-3w3c-4b-review-v1.md`'s own "Next" section, and the live
//! note in `rfcs/handoffs/155-repository-complete-artifact/0.56.0-step-3-implementation-handoff-
//! v1.md`): a `--adopt` landing destined for the current branch now brings a clean worktree along
//! -- the adopted content is materialized into the worktree -- and a dirty worktree, or a file in
//! the way of the adopted tip's own files, refuses before any ref-pointer write.
//!
//! **No fail-fast in-the-way check exists for this producer** (see `archive::import`'s own doc
//! comment on `import_archive`'s fresh-start branch): building one would need a full,
//! archive-native, random-access `ObjectReader` over the archive's own Patch/Blob sections, which
//! this round does not build. So, unlike `sync seal`'s and `merge --into`'s own in-the-way tests,
//! the in-the-way case below is caught only at stage 3 -- *after* stage 1 has already admitted the
//! archive's own carried objects (`Block`/`RefState`/`Patch`/`Blob`) into the local store,
//! unconditionally, as it always has, independent of this round. A stage-3 refusal therefore
//! leaves those objects locally present but unreached by any ref -- the exact same shape every
//! other mid-stage-3 refusal in this file already has (e.g.
//! `archive::import::tests::received_landing_rollback_refuses_and_leaves_the_pointer_unchanged`,
//! which asserts only that the pointer is unchanged, never that the whole repository is
//! byte-identical) -- so the in-the-way test below asserts "no ref-pointer write, and the
//! colliding file is untouched," not "byte-identical."

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

/// Test 3 (the ruling's own list, narrowed by this file's own module doc): a file in the way of
/// the adopted tip's own target file must refuse *before* the import publishes `heads/main`'s own
/// ref-pointer -- caught at stage 3 (the authoritative check), since no fail-fast in-the-way check
/// exists for this producer. Since `heads/main` is unborn and has no record at all here, an
/// untracked file at the adopted content's own path is never flagged by the clean check
/// (untracked is never a reason to refuse) -- it is the new pre-write in-the-way check, run
/// against the chain's own final block (already a real local object by stage 3), that must catch
/// it.
#[test]
fn archive_import_adopt_onto_current_branch_a_file_in_the_way_refuses_before_publish_then_resyncs()
{
    let archive_path = build_adoptable_archive("in-the-way", "n.txt", b"from-archive\n");
    let dest = support::unique_repo("w3c5-in-the-way-dest");
    support::init(&dest);
    support::trust_maintainer(&dest);

    // The untracked file already in the way, at the exact path the adopted content adds --
    // different content than the import will bring, so the collision is real.
    std::fs::write(dest.join("n.txt"), b"zzz\n").unwrap();

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
            && message.contains("before adopting heads/main"),
        "the pre-write refusal must name the action, heads/main, and n.txt: {message}"
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

    // After the offending file is moved aside, the same command succeeds and materializes --
    // stage 1 already admitted every carried object on the first attempt, so this retry resumes
    // from a fresh journal-less run (the prior attempt's journal was already emptied: the
    // in-the-way refusal happens inside stage 3, before the commit point, so the journal from the
    // first attempt is still present) and lands cleanly.
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
