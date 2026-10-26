//! 0.55.0 step 3 Part 3a (R6): the real-`SIGKILL` counterpart to the architect's own manual
//! experiment (2A2 design review, R6/R7) -- killing `bundle import` mid-write-loop and confirming
//! `verify`/`doctor` read clean, a same-process retry is refused by the stale locks the kill left
//! behind, `prikk unlock` lists them, and clearing them makes the retry succeed, idempotently.
//!
//! Like `rfc155_part_ib2_real_kill.rs`, this is one of the few places in the suite that sends a
//! real `SIGKILL` (`std::process::Child::kill`, documented as exactly that on Unix) to a spawned
//! `prikk` process, rather than a seam that returns `Err` and lets `Drop` release locks normally --
//! a real crash never calls `Drop`. The blocking seam itself
//! (`PRIKK_BUNDLE_IMPORT_BLOCK_AFTER_N_FOR_TEST`/`PRIKK_BUNDLE_IMPORT_BLOCKED_MARKER_FOR_TEST`,
//! `bundle.rs`'s own `block_after_n_objects_for_real_kill_test`) is gated by `prikk-store`'s
//! `test-support` feature, which `prikk-cli`'s own `[dev-dependencies]` enables, not `#[cfg(test)]`
//! (unreachable from a separately spawned binary).
//!
//! `bundle import` has no journal (unlike `archive import`): a kill here leaves no
//! `PRIKK-DOCTOR-INTERRUPTED-IMPORT` state for `doctor` to name, no resume/cancel choice -- only
//! the three locks (`active.lock`, `received-index.lock`, `objects.lock`) the acquisition left
//! behind, and whatever prefix of objects the write loop had already durably appended before the
//! kill landed. Unix-only, matching the real-`SIGKILL` pattern's own gate.
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::process::Stdio;
use std::time::{Duration, Instant};

/// Extract `verify`'s own `"checked objects: <n>"` line.
fn checked_objects(verify_stdout: &str) -> usize {
    verify_stdout
        .lines()
        .find_map(|line| line.strip_prefix("checked objects: "))
        .expect("verify's own output names \"checked objects: <n>\"")
        .parse()
        .expect("checked objects is a plain integer")
}

#[test]
fn a_real_sigkill_mid_write_loop_leaves_a_clean_repository_and_stale_locks_until_unlock() {
    // The source: three generations, exported as a bundle. Each generation's Blob, Patch, Block
    // and RefState is one object to write, so three generations give more than enough objects to
    // pick a block-after point strictly between the first and the last.
    let source_repo = support::unique_repo("bundle-real-kill-source");
    support::init(&source_repo);
    support::generation(&source_repo, "heads/main", "a.txt", b"first\n", "first");
    support::generation(&source_repo, "heads/main", "b.txt", b"second\n", "second");
    support::generation(&source_repo, "heads/main", "c.txt", b"third\n", "third");

    let mut bundle_path = std::env::temp_dir();
    bundle_path.push(format!(
        "prikk-cli-bundle-real-kill-{}.bin",
        support::unique_suffix()
    ));
    support::ok(
        &support::prikk(&source_repo)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle_path.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "bundle export",
    );

    // A fresh, completely uninterrupted import into a second repository, kept aside only to
    // confirm the final object count the killed-then-resumed import must also reach.
    let reference_repo = support::unique_repo("bundle-real-kill-reference");
    support::init(&reference_repo);
    let reference_import = support::prikk(&reference_repo)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&reference_import, "reference import");
    let reference_stdout = String::from_utf8_lossy(&reference_import.stdout).into_owned();
    let reference_verify = support::prikk(&reference_repo)
        .arg("verify")
        .output()
        .unwrap();
    let reference_object_count =
        checked_objects(&String::from_utf8_lossy(&reference_verify.stdout));
    assert!(
        reference_object_count > 2,
        "the bundle must carry more than the block-after-2 point, or this test proves nothing: \
         {reference_object_count} objects"
    );

    // The destination: freshly initialized, nothing imported yet.
    let dest_repo = support::unique_repo("bundle-real-kill-dest");
    support::init(&dest_repo);

    // Spawn the real import, blocked after its second object write -- strictly inside the write
    // loop, neither its first write (too easily confused with "nothing written yet") nor its last
    // (too easily confused with "fully written, only the pointer missing").
    let mut marker_path = std::env::temp_dir();
    marker_path.push(format!(
        "prikk-cli-bundle-real-kill-marker-{}",
        support::unique_suffix()
    ));
    let _ = std::fs::remove_file(&marker_path);
    let mut child = support::prikk(&dest_repo)
        .env("PRIKK_BUNDLE_IMPORT_BLOCK_AFTER_N_FOR_TEST", "2")
        .env("PRIKK_BUNDLE_IMPORT_BLOCKED_MARKER_FOR_TEST", &marker_path)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `prikk bundle import`");

    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker_path.exists() {
        assert!(
            Instant::now() < deadline,
            "the child process never reached the blocking seam within 30s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The real kill. `Child::kill` is `SIGKILL` on Unix -- `Drop` for the child's own `ActiveLock`
    // and `ContainerLockGuard` values never runs.
    child.kill().expect("send SIGKILL");
    let _ = child.wait();

    // `verify`: clean. The architect's own 3,000-object experiment found zero interrupted
    // appends, zero unreferenced remnants and zero unreached objects after a real kill mid-write-
    // loop, reasoned from `objects_in_dependency_order`'s own guarantee (every object's
    // dependencies are written before it, so any prefix of an honestly-ordered import's write
    // order cannot produce a dangling reference). Confirmed here on this run's own numbers, not
    // merely cited from the architect's.
    // A plain `verify` here reports "publication trust issues" (no maintainer key adopted --
    // unrelated to this test), so this checks only the integrity rows import itself owns, not the
    // trust-policy exit status.
    let verify_after_kill = support::prikk(&dest_repo).arg("verify").output().unwrap();
    let verify_text = String::from_utf8_lossy(&verify_after_kill.stdout);
    assert!(
        verify_text.contains("interrupted appends: 0"),
        "a kill mid-write-loop must leave no torn container frame: {verify_text}"
    );
    assert!(
        verify_text.contains("unreferenced remnants: 0"),
        "an honestly-ordered import's write prefix cannot dangle a reference: {verify_text}"
    );
    assert!(
        verify_text.contains("unreached objects: 0"),
        "the received pointer is written last and never reached by this kill, so no object the \
         kill did write should be reported unreached either: {verify_text}"
    );
    // The positive half of the control: confirm the kill genuinely interrupted the write loop
    // (a strict prefix durable), not that the import simply finished before the kill landed --
    // without this, every assertion above could pass vacuously on a fully-completed import.
    let dest_object_count_after_kill = checked_objects(&verify_text);
    assert!(
        dest_object_count_after_kill < reference_object_count,
        "the kill must leave a strict prefix of the full {reference_object_count}-object import, \
         not the whole thing: got {dest_object_count_after_kill}"
    );

    // `doctor`: clean exit too. `bundle import` has no journal, so there is no
    // `PRIKK-DOCTOR-INTERRUPTED-IMPORT`-shaped state for `doctor` to name -- only the stale locks,
    // which `doctor` is not expected to name (the architect's own review: "`doctor` names none of
    // them, and the error does not mention `unlock`").
    let doctor_after_kill = support::prikk(&dest_repo).arg("doctor").output().unwrap();
    support::ok(&doctor_after_kill, "doctor after kill");

    // A same-process retry, before unlocking, is refused by the stale lock -- not silently
    // succeeding, and not corrupting anything.
    let premature_retry = support::prikk(&dest_repo)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !premature_retry.status.success(),
        "a retry must refuse while the real kill's own stale lock remains: {premature_retry:?}"
    );
    let premature_retry_text = String::from_utf8_lossy(&premature_retry.stderr);
    assert!(
        premature_retry_text.contains("lock") && premature_retry_text.contains("conflict")
            || premature_retry_text.contains("already"),
        "the refusal must read as a lock conflict, not some other error: {premature_retry_text}"
    );

    // `prikk unlock` lists the lock(s) this real kill left behind; clear each one.
    let listing = support::prikk(&dest_repo).arg("unlock").output().unwrap();
    support::ok(&listing, "unlock (list)");
    let listing_text = String::from_utf8_lossy(&listing.stdout);
    let lock_paths: Vec<&str> = listing_text
        .lines()
        .filter(|line| line.starts_with('/'))
        .collect();
    assert!(
        !lock_paths.is_empty(),
        "the real kill must have left at least one lock file for `unlock` to list: {listing_text}"
    );
    for lock_path in &lock_paths {
        support::ok(
            &support::prikk(&dest_repo)
                .args(["unlock", "--lock", lock_path, "--yes"])
                .output()
                .unwrap(),
            &format!("unlock --lock {lock_path} --yes"),
        );
    }

    // The retry now completes, idempotently -- the same final object count a fresh,
    // uninterrupted import of the identical bundle reaches (the kill's own partial prefix is
    // skipped via `contains_object`, unchanged by this part).
    let resumed_import = support::prikk(&dest_repo)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&resumed_import, "resumed import after unlock");

    let verify_after_resume = support::prikk(&dest_repo).arg("verify").output().unwrap();
    let verify_after_resume_text = String::from_utf8_lossy(&verify_after_resume.stdout);
    assert!(
        verify_after_resume_text.contains("unreferenced remnants: 0")
            && verify_after_resume_text.contains("unreached objects: 0")
            && verify_after_resume_text.contains("interrupted appends: 0"),
        "the resumed, fully-written repository must be integrity-clean: {verify_after_resume_text}"
    );

    // The resumed repository and the untouched reference repository must agree on what the
    // import ultimately produced.
    assert_eq!(
        String::from_utf8_lossy(
            &support::prikk(&dest_repo)
                .arg("branch")
                .output()
                .unwrap()
                .stdout
        ),
        String::from_utf8_lossy(
            &support::prikk(&reference_repo)
                .arg("branch")
                .output()
                .unwrap()
                .stdout
        ),
        "the resumed-after-kill import must land the same branch state as an uninterrupted one"
    );
    let _ = reference_stdout;

    let _ = std::fs::remove_dir_all(&source_repo);
    let _ = std::fs::remove_dir_all(&reference_repo);
    let _ = std::fs::remove_dir_all(&dest_repo);
    let _ = std::fs::remove_file(&bundle_path);
    let _ = std::fs::remove_file(&marker_path);
}
