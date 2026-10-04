//! RFC 166 round 2 §0 (review v1's own required fix): `status`'s warning, and `commit`'s,
//! `seal`'s and `rollback-draft`'s own refusals, all name the *same* acknowledged-damage text --
//! the commit-witness classification's own `write_refusal_reason` -- not the older, generic
//! "trailing partial bytes; run `prikk doctor --repair-wal-tail`" text, which used to speak first
//! and sent the user to a repair that then refuses.
//!
//! **Perturb:** move any one of the four call sites' own classification check back after its own
//! older tail check (the round 1 shape): that one surface's assertion below goes red, still
//! showing the old "trailing partial bytes" text instead of the shared one. Demonstrated by hand
//! against `commit`'s own call site while landing this test; not left as a toggle in product code
//! (the standing rule: no selector, environment variable, or test-only switch there).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// The exact substring every one of the four surfaces must share for an acknowledged-damage
/// finding (RFC 166 §5 row 4) -- present in `write_refusal_reason`'s own text, nowhere else.
const SHARED_TEXT: &str = "run `prikk doctor --discard-damaged-commits`";

/// The older, generic text this round's own fix must no longer let speak first for this
/// condition.
const OLD_TEXT: &str = "trailing partial bytes";

fn witness_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/witness")
}

fn wal_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/active/default/queue.wal")
}

/// A repository with one sealed generation (so `rollback-draft` has sealed history to invert) and
/// one queued, witnessed, then damaged commit in the active session -- acknowledged damage (row 4).
fn damaged_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("genesis.txt"), "genesis\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "genesis"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("queued.txt"), "queued\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "queued, acknowledged"),
        "commit",
    );
    let witness_before = std::fs::read(witness_path(&repo)).unwrap();
    assert!(!witness_before.is_empty(), "fixture: a witness exists");

    // Flip a body byte of the lone queued record -- the shape N6 is about: a byte changed after
    // the commit already succeeded, not a genuine crash. The witness, untouched, still names it.
    let wal = wal_path(&repo);
    let mut bytes = std::fs::read(&wal).unwrap();
    let flip_at = bytes.len() - 5;
    bytes[flip_at] ^= 0xFF;
    std::fs::write(&wal, &bytes).unwrap();
    repo
}

fn clone_of(src: &Path, tag: &str) -> PathBuf {
    let dst = support::unique_repo(tag);
    let _ = std::fs::remove_dir_all(&dst);
    support::copy_dir_recursive(src, &dst);
    dst
}

#[test]
fn status_commit_seal_and_rollback_draft_all_name_the_classifications_own_text() {
    let base = damaged_repo("rfc166-r2-u0-base");

    let status_repo = clone_of(&base, "rfc166-r2-u0-status");
    let status_out = support::prikk(&status_repo).arg("status").output().unwrap();
    let status_text = format!(
        "{}{}",
        String::from_utf8_lossy(&status_out.stdout),
        String::from_utf8_lossy(&status_out.stderr)
    );
    assert!(
        status_text.contains(SHARED_TEXT),
        "status must warn with the classification's own text\n{status_text}"
    );

    let commit_repo = clone_of(&base, "rfc166-r2-u0-commit");
    std::fs::write(commit_repo.join("after.txt"), "after\n").unwrap();
    let commit_out = support::commit(&commit_repo, "heads/main", "after the damage");
    let commit_text = format!(
        "{}{}",
        String::from_utf8_lossy(&commit_out.stdout),
        String::from_utf8_lossy(&commit_out.stderr)
    );
    assert!(!commit_out.status.success(), "{commit_text}");
    assert!(
        commit_text.contains(SHARED_TEXT),
        "commit must refuse with the classification's own text, not the older tail text\n{commit_text}"
    );
    assert!(
        !commit_text.contains(OLD_TEXT),
        "commit must not name the older, generic tail text over acknowledged damage\n{commit_text}"
    );
    assert!(
        !commit_text.contains("--repair-wal-tail"),
        "commit must not point at a repair that refuses over acknowledged damage\n{commit_text}"
    );

    let seal_repo = clone_of(&base, "rfc166-r2-u0-seal");
    let seal_out = support::seal(&seal_repo, "heads/main");
    let seal_text = format!(
        "{}{}",
        String::from_utf8_lossy(&seal_out.stdout),
        String::from_utf8_lossy(&seal_out.stderr)
    );
    assert!(!seal_out.status.success(), "{seal_text}");
    assert!(
        seal_text.contains(SHARED_TEXT),
        "seal must refuse with the classification's own text\n{seal_text}"
    );
    assert!(
        !seal_text.contains(OLD_TEXT),
        "seal must not name the older, generic tail text over acknowledged damage\n{seal_text}"
    );

    let rollback_repo = clone_of(&base, "rfc166-r2-u0-rollback");
    let rollback_out = support::prikk(&rollback_repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo"])
        .output()
        .unwrap();
    let rollback_text = format!(
        "{}{}",
        String::from_utf8_lossy(&rollback_out.stdout),
        String::from_utf8_lossy(&rollback_out.stderr)
    );
    assert!(!rollback_out.status.success(), "{rollback_text}");
    assert!(
        rollback_text.contains(SHARED_TEXT),
        "rollback-draft must refuse with the classification's own text\n{rollback_text}"
    );
    assert!(
        !rollback_text.contains(OLD_TEXT),
        "rollback-draft must not name the older, generic tail text over acknowledged damage\n{rollback_text}"
    );

    let _ = std::fs::remove_dir_all(&base);
    let _ = std::fs::remove_dir_all(&status_repo);
    let _ = std::fs::remove_dir_all(&commit_repo);
    let _ = std::fs::remove_dir_all(&seal_repo);
    let _ = std::fs::remove_dir_all(&rollback_repo);
}
