//! 0.59.0 K3 item 0: the restore finishes an interrupted append by its bytes, and a **real later write** is not one. A torn half of a real WAL
//! record is repaired away (`doctor --repair-wal-tail`: a `Cut`), then a real `commit` appends a whole record where the cut was. The restore must
//! refuse, on the length (the file is longer than the cut's offset plus the removed bytes) and on the witness (the commit changed it), and write
//! nothing: the later commit is kept.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

use crate::support;
use std::path::{Path, PathBuf};

fn wal(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/queue.wal")
}

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = support::prikk(repo).args(args).output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn later_write_after_a_cut(tag: &str, later_content: &str) -> (PathBuf, String, Vec<u8>) {
    use std::io::Write;
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("one.txt"), "first\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    // A torn half of that real record, after it.
    let record = std::fs::read(wal(&repo)).unwrap();
    let half = &record[..record.len() / 2];
    std::fs::OpenOptions::new()
        .append(true)
        .open(wal(&repo))
        .unwrap()
        .write_all(half)
        .unwrap();
    let (code, out) = run(&repo, &["doctor", "--repair-wal-tail"]);
    assert_eq!(code, Some(0), "{out}");
    let id = out
        .split("recovery/log, run ")
        .nth(1)
        .unwrap_or_else(|| panic!("the repair names its run: {out}"))
        .chars()
        .take(16)
        .collect::<String>();
    assert_eq!(
        std::fs::read(wal(&repo)).unwrap(),
        record,
        "the repair cut the torn half"
    );
    // A real later write, appended where the cut was.
    std::fs::write(repo.join("two.txt"), later_content).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "later"),
        "later commit",
    );
    let after = std::fs::read(wal(&repo)).unwrap();
    assert!(
        after.len() > record.len() + half.len(),
        "a whole record is longer than half of one"
    );
    (repo, id, after)
}

#[test]
fn a_real_later_commit_makes_the_restore_refuse_and_the_commit_is_kept() {
    for (tag, content) in [
        ("k3-later-write-other", "other content\n"),
        ("k3-later-write-same", "first\n"),
    ] {
        let (repo, id, after) = later_write_after_a_cut(tag, content);
        let (code, out) = run(&repo, &["doctor", "--recovery-restore", &id, "--plan-only"]);
        assert_eq!(code, Some(1), "{tag}: {out}");
        assert!(
            out.contains("bytes; the repair left"),
            "{tag}: the length refuses: {out}"
        );
        assert!(
            out.contains("witness has changed since the repair"),
            "{tag}: the meaning file refuses: {out}"
        );
        let (code, out) = run(&repo, &["doctor", "--recovery-restore", &id]);
        assert_eq!(code, Some(1), "{tag}: {out}");
        assert_eq!(
            std::fs::read(wal(&repo)).unwrap(),
            after,
            "{tag}: nothing was written"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
