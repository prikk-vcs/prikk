//! 0.57.0 step 1c: an absent or ignored `.prikk/worktree.baseline` gets a way out, then a notice.
//!
//! The record is what lets `commit` refuse a worktree that is behind its branch. When it is absent,
//! empty, unreadable, or names a Block this repository does not hold, it reads as no record, the guard
//! is off, and `verify` and `doctor` said nothing (the architect's matrix: 9 `SILENT` cells).
//!
//! - **The way out:** `branch switch <the current branch>` writes the record when it is absent or
//!   ignored **and the worktree is clean against the branch**; otherwise it writes nothing, as before.
//! - **The notice:** `doctor` warns (`PRIKK-DOCTOR-WORKTREE-RECORD-IGNORED`, exit 0) naming that
//!   command, and only when that command would restore the record. Nothing is said for a worktree that
//!   is not clean (a warning with nothing to type is noise; `commit`'s own refusals cover that state).
//!
//! Which matrix shapes are reported: the four that make the record ignored (a byte flipped, a hex digit
//! replaced, the file emptied, the file removed). The five that leave its one line readable (zero or
//! random bytes appended after it, its final newline removed) are *not* ignored -- the record still
//! guards -- and stay silent, which `valid_shapes_stay_silent` pins.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::Path;

fn record_path(repo: &Path) -> std::path::PathBuf {
    repo.join(".prikk").join("worktree.baseline")
}

/// A repository with one sealed generation, so the record names a real Block.
fn sealed_repo(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"alpha\n", "one");
    repo
}

fn text(output: &std::process::Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn doctor(repo: &Path) -> (Option<i32>, String) {
    let output = support::prikk(repo).arg("doctor").output().unwrap();
    (output.status.code(), text(&output))
}

fn switch_to_current(repo: &Path) -> (Option<i32>, String) {
    let output = support::prikk(repo)
        .args(["branch", "switch", "heads/main"])
        .output()
        .unwrap();
    (output.status.code(), text(&output))
}

const NOTICE: &str = "PRIKK-DOCTOR-WORKTREE-RECORD-IGNORED";
const RECORDED: &str = "recorded the worktree's baseline for heads/main";

/// The four matrix shapes that make the record ignored, as the matrix applies them.
type Mutation = fn(&[u8]) -> Vec<u8>;

fn ignoring_shapes() -> Vec<(&'static str, Option<Mutation>)> {
    fn flipped(d: &[u8]) -> Vec<u8> {
        let mut out = d.to_vec();
        let at = d.len() / 2;
        out[at] ^= 0xFF;
        out
    }
    fn hex_replaced(d: &[u8]) -> Vec<u8> {
        let mut out = d.to_vec();
        let at = d.len() / 2;
        let digits = b"0123456789abcdef";
        let i = digits.iter().position(|b| *b == d[at]).unwrap_or(0);
        out[at] = digits[(i + 1) % 16];
        out
    }
    fn emptied(_: &[u8]) -> Vec<u8> {
        Vec::new()
    }
    vec![
        ("a byte flipped in the middle", Some(flipped as Mutation)),
        ("a hex digit replaced in the middle", Some(hex_replaced)),
        ("file emptied", Some(emptied)),
        ("file removed", None),
    ]
}

/// **The way out and the notice, together, for every shape that makes the record ignored.** A clean
/// worktree: `doctor` warns (exit 0) naming `prikk branch switch heads/main`; the switch writes the
/// record -- byte for byte what it was -- and says so; `doctor` is then silent; a second switch is the
/// plain no-op it always was.
///
/// **Perturb (red control):** stop the switch writing (`worktree_record_restored` never set / the
/// write removed) -- the byte-identical assertion goes red; drop the notice from `doctor_repository`
/// -- the first assertion goes red.
#[test]
fn an_ignored_record_is_reported_and_restored_by_switching_to_the_current_branch() {
    for (shape, mutate) in ignoring_shapes() {
        let repo = sealed_repo("step1c-restore");
        let original = std::fs::read(record_path(&repo)).unwrap();
        match mutate {
            Some(mutate) => std::fs::write(record_path(&repo), mutate(&original)).unwrap(),
            None => std::fs::remove_file(record_path(&repo)).unwrap(),
        }

        let (code, report) = doctor(&repo);
        assert_eq!(
            code,
            Some(0),
            "{shape}: a warning keeps doctor's exit 0\n{report}"
        );
        assert!(report.contains(NOTICE), "{shape}: {report}");
        assert!(
            report.contains("run `prikk branch switch heads/main`"),
            "{shape}: the notice names the command: {report}"
        );

        let (code, out) = switch_to_current(&repo);
        assert_eq!(code, Some(0), "{shape}: {out}");
        assert!(out.contains("already on heads/main"), "{shape}: {out}");
        assert!(out.contains(RECORDED), "{shape}: {out}");
        assert_eq!(
            std::fs::read(record_path(&repo)).unwrap(),
            original,
            "{shape}: the record is written back exactly (the branch's live identity)"
        );

        let (_, report) = doctor(&repo);
        assert!(
            !report.contains(NOTICE),
            "{shape}: silent once restored\n{report}"
        );
        let (_, out) = switch_to_current(&repo);
        assert!(
            !out.contains(RECORDED),
            "{shape}: a second switch writes nothing: {out}"
        );

        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// A record that is still readable guards, so it is not reported: zero or random bytes appended after
/// its line, and its final newline removed, leave the one line intact. (The matrix calls these cells
/// silent too; they are silent because nothing is wrong, not because nothing is looked at.)
#[test]
fn valid_shapes_stay_silent() {
    let cases: [(&str, Mutation); 3] = [
        ("30 zero bytes appended", |d| [d, &[0u8; 30]].concat()),
        ("100 non-newline bytes appended", |d| {
            [d, &[0x5au8; 100]].concat()
        }),
        ("final byte removed", |d| d[..d.len() - 1].to_vec()),
    ];
    for (shape, mutate) in cases {
        let repo = sealed_repo("step1c-valid");
        let original = std::fs::read(record_path(&repo)).unwrap();
        std::fs::write(record_path(&repo), mutate(&original)).unwrap();
        let (code, report) = doctor(&repo);
        assert_eq!(code, Some(0), "{shape}\n{report}");
        assert!(!report.contains(NOTICE), "{shape}: {report}");
        let (_, out) = switch_to_current(&repo);
        assert!(!out.contains(RECORDED), "{shape}: {out}");
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// **A worktree that is not clean gets neither the write nor the notice.** The record would say "last
/// written from this tip", which is not true of a worktree that differs: the switch writes nothing and
/// leaves the files alone (as before), and `doctor` prints nothing about the record, because the command
/// it would name writes nothing. A modified file and a deleted tracked file are both "not clean".
///
/// **Perturb (red control):** drop the clean condition from `plan_worktree_record_restore` -- the
/// switch writes a record over a differing worktree and the "writes nothing" assertion goes red; the
/// notice appears and the "no notice" assertion goes red.
#[test]
fn a_worktree_that_is_not_clean_is_neither_restored_nor_reported() {
    type Damage = fn(&Path);
    let damages: [(&str, Damage); 2] = [
        ("a tracked file modified", |repo| {
            std::fs::write(repo.join("a.txt"), "alpha, edited\n").unwrap();
        }),
        ("a tracked file deleted", |repo| {
            std::fs::remove_file(repo.join("a.txt")).unwrap();
        }),
    ];
    for (what, damage) in damages {
        let repo = sealed_repo("step1c-notclean");
        std::fs::remove_file(record_path(&repo)).unwrap();
        damage(&repo);
        let before = std::fs::read_dir(&repo).unwrap().count();

        let (code, report) = doctor(&repo);
        assert_eq!(code, Some(0), "{what}\n{report}");
        assert!(
            !report.contains(NOTICE),
            "{what}: no notice with nothing to type: {report}"
        );

        let (code, out) = switch_to_current(&repo);
        assert_eq!(code, Some(0), "{what}: {out}");
        assert!(out.contains("already on heads/main"), "{what}: {out}");
        assert!(!out.contains(RECORDED), "{what}: {out}");
        assert!(
            !record_path(&repo).exists(),
            "{what}: the switch wrote no record over a worktree that differs"
        );
        assert_eq!(
            std::fs::read_dir(&repo).unwrap().count(),
            before,
            "{what}: no file moved"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// An unpublished current branch (nothing sealed yet) has a genesis record; the same rules hold.
#[test]
fn an_unborn_branch_record_is_restored_too() {
    let repo = support::unique_repo("step1c-unborn");
    support::init(&repo);
    let original = std::fs::read(record_path(&repo)).unwrap();
    std::fs::remove_file(record_path(&repo)).unwrap();

    let (_, report) = doctor(&repo);
    assert!(report.contains(NOTICE), "{report}");
    let (code, out) = switch_to_current(&repo);
    assert_eq!(code, Some(0), "{out}");
    assert!(out.contains(RECORDED), "{out}");
    assert_eq!(std::fs::read(record_path(&repo)).unwrap(), original);
    let _ = std::fs::remove_dir_all(&repo);
}
