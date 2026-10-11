//! RFC 155 Part I-B2's own required test (review `rfc155-impl-IB-review-v1`, finding IB2): every
//! kill `prikk-store`'s own unit tests exercise is a seam that returns an `Err`, so `Drop` still
//! releases every lock normally -- a real `SIGKILL` does not. This is the one place in the whole
//! RFC 155 test suite where a real child process is genuinely killed with `SIGKILL`
//! (`std::process::Child::kill`, which the standard library documents as exactly that on Unix),
//! not a seam that returns cleanly, and what is left on disk afterward is inspected for real.
//!
//! The blocking seam itself (`PRIKK_ARCHIVE_IMPORT_BLOCK_AFTER_N_FOR_TEST`/
//! `PRIKK_ARCHIVE_IMPORT_BLOCKED_MARKER_FOR_TEST`) is gated by `prikk-store`'s own `test-support`
//! feature, not `#[cfg(test)]` (which only ever compiles into `prikk-store`'s own unit-test
//! binary and could never be reached from a child process this test spawns) -- `test-support` is
//! unified into the `prikk` binary `env!("CARGO_BIN_EXE_prikk")` names here because this whole
//! test binary is built by the same `cargo test` invocation that needs it (`prikk-cli/Cargo.toml`'s
//! own `[dev-dependencies]` enables it), the same reason the module's own doc comment gives for
//! never reaching the binary a plain `cargo build --release` ships.
//!
//! Unix-only: `SIGKILL`/stale-lock-survival is a Unix-shaped scenario (`Child::kill`'s own doc
//! names it a `SIGKILL` specifically on Unix; Windows terminates the process a different way and
//! this part's own review never asked for a Windows-specific repro).
#![cfg(unix)]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

use support::ok;

/// Extract the import id doctor's own message names: `"...(id <hex>)..."`.
fn extract_import_id(doctor_text: &str) -> String {
    let start = doctor_text
        .find("(id ")
        .expect("doctor's own message names the import id")
        + "(id ".len();
    let rest = &doctor_text[start..];
    let end = rest
        .find(')')
        .expect("doctor's own message closes the id with ')'");
    rest[..end].to_string()
}

#[test]
fn a_real_sigkill_mid_objects_is_recovered_through_doctor_and_unlock() {
    // The source archive: one real generation, exported.
    let source_repo = support::unique_repo("rfc155-ib2-real-kill-source");
    support::init(&source_repo);
    support::generation(
        &source_repo,
        "heads/main",
        "a.txt",
        b"real-kill test content\n",
        "one real generation",
    );
    let mut archive_path = std::env::temp_dir();
    archive_path.push(format!(
        "prikk-cli-ib2-real-kill-archive-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&source_repo)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );

    // The destination: freshly initialized, nothing imported yet. Its own pre-import bytes are
    // the baseline a cancel must restore, byte for byte (locks and the journal excepted -- the
    // journal did not exist before the import either, and is the one new, empty file a cancel's
    // own commit point intentionally leaves, by RFC 168 §6's own conservative choice).
    let dest_repo = support::unique_repo("rfc155-ib2-real-kill-dest");
    support::init(&dest_repo);
    let before_import = store_bytes_excluding_locks_and_journal(&dest_repo);

    // Spawn the real import, blocked after its first object write.
    let mut marker_path = std::env::temp_dir();
    marker_path.push(format!(
        "prikk-cli-ib2-real-kill-marker-{}",
        support::unique_suffix()
    ));
    let _ = std::fs::remove_file(&marker_path);
    let mut child = support::prikk(&dest_repo)
        .env("PRIKK_ARCHIVE_IMPORT_BLOCK_AFTER_N_FOR_TEST", "1")
        .env("PRIKK_ARCHIVE_IMPORT_BLOCKED_MARKER_FOR_TEST", &marker_path)
        .args(["archive", "import"])
        .arg(&archive_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn `prikk archive import`");

    // Wait for the marker -- the process is now genuinely blocked, mid-stage-1, holding every
    // container lock for real, not merely "about to write" and racily killed before it does.
    let deadline = Instant::now() + Duration::from_secs(30);
    while !marker_path.exists() {
        assert!(
            Instant::now() < deadline,
            "the child process never reached the blocking seam within 30s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // The real kill. `Child::kill` is documented as `SIGKILL` on Unix -- `Drop` for every lock
    // this child's own `ActiveLock`/`ContainerLockGuard` values hold never runs.
    child.kill().expect("send SIGKILL");
    let _ = child.wait();

    // What a real user sees first: `doctor` names the interrupted import, and -- because the
    // kill left real lock files behind, not a clean `Err` return -- names `prikk unlock` first,
    // ahead of resume or cancel.
    let doctor_output = support::prikk(&dest_repo).arg("doctor").output().unwrap();
    let doctor_text = String::from_utf8_lossy(&doctor_output.stdout).into_owned();
    assert!(
        doctor_text.contains("PRIKK-DOCTOR-INTERRUPTED-IMPORT"),
        "doctor must name the interrupted import: {doctor_text}"
    );
    assert!(
        doctor_text.contains("prikk unlock"),
        "a real kill leaves real lock files behind -- doctor must say so, naming `prikk unlock` \
         before resume or cancel: {doctor_text}"
    );
    let id = extract_import_id(&doctor_text);

    // Following the message in order: a resume attempted before `unlock` refuses on the stale
    // lock, not silently succeeding or corrupting anything.
    let premature_resume = support::prikk(&dest_repo)
        .args(["archive", "import"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        !premature_resume.status.success(),
        "a resume must refuse while the real kill's own stale lock remains: {premature_resume:?}"
    );

    // `prikk unlock` with no arguments only lists; clearing one lock always needs its own
    // `--lock <path>` (confirmation bypassed with `--yes`, the scripting escape `prikk unlock
    // --help` documents) -- there is no "clear everything" flag, by the tool's own design (never
    // auto-steal). List, then clear each lock this real kill left behind, one call per path.
    let listing = support::prikk(&dest_repo).arg("unlock").output().unwrap();
    ok(&listing, "unlock (list)");
    let listing_text = String::from_utf8_lossy(&listing.stdout);
    let lock_paths: Vec<&str> = listing_text
        .lines()
        .filter(|line| line.starts_with('/'))
        .collect();
    assert!(
        !lock_paths.is_empty(),
        "the real kill must have left at least one lock file: {listing_text}"
    );
    for lock_path in lock_paths {
        ok(
            &support::prikk(&dest_repo)
                .args(["unlock", "--lock", lock_path, "--yes"])
                .output()
                .unwrap(),
            &format!("unlock --lock {lock_path} --yes"),
        );
    }

    // Branch A, on a fresh copy of the post-kill, post-unlock state: resume finishes the import
    // cleanly, landing the one real branch under `remotes/` (no `--adopt`).
    let resume_copy = support::unique_repo("rfc155-ib2-real-kill-resume-copy");
    std::fs::create_dir_all(resume_copy.join(".prikk")).unwrap();
    support::copy_dir_recursive(&dest_repo.join(".prikk"), &resume_copy.join(".prikk"));

    ok(
        &support::prikk(&resume_copy)
            .args(["archive", "import"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "resumed import",
    );
    let branch_output = support::prikk(&resume_copy).arg("branch").output().unwrap();
    ok(&branch_output, "branch list after resume");
    let branch_text = String::from_utf8_lossy(&branch_output.stdout);
    assert!(
        branch_text.contains("remotes/heads/main"),
        "the resumed import must land the archive's own branch under remotes/: {branch_text}"
    );
    let doctor_after_resume = support::prikk(&resume_copy).arg("doctor").output().unwrap();
    let doctor_after_resume_text = String::from_utf8_lossy(&doctor_after_resume.stdout);
    assert!(
        !doctor_after_resume_text.contains("PRIKK-DOCTOR-INTERRUPTED-IMPORT"),
        "the commit point must have emptied the journal: {doctor_after_resume_text}"
    );

    // Branch B, on a separate fresh copy of the same post-kill, post-unlock state: cancel
    // restores every journaled container, byte for byte, to what the destination had before the
    // import ever started.
    let cancel_copy = support::unique_repo("rfc155-ib2-real-kill-cancel-copy");
    std::fs::create_dir_all(cancel_copy.join(".prikk")).unwrap();
    support::copy_dir_recursive(&dest_repo.join(".prikk"), &cancel_copy.join(".prikk"));

    ok(
        &support::prikk(&cancel_copy)
            .args(["doctor", "--cancel-import", &id])
            .output()
            .unwrap(),
        "cancel-import",
    );
    let doctor_after_cancel = support::prikk(&cancel_copy).arg("doctor").output().unwrap();
    let doctor_after_cancel_text = String::from_utf8_lossy(&doctor_after_cancel.stdout);
    assert!(
        !doctor_after_cancel_text.contains("PRIKK-DOCTOR-INTERRUPTED-IMPORT"),
        "the cancel must have emptied the journal too: {doctor_after_cancel_text}"
    );
    let branch_after_cancel = support::prikk(&cancel_copy).arg("branch").output().unwrap();
    ok(&branch_after_cancel, "branch list after cancel");
    let branch_after_cancel_text = String::from_utf8_lossy(&branch_after_cancel.stdout);
    assert!(
        branch_after_cancel_text.contains("no branches"),
        "a cancelled import must leave no real branch behind (a fresh `init`'s own unpublished \
         current-branch pointer line is expected and is not a published ref): \
         {branch_after_cancel_text}"
    );
    assert!(
        !branch_after_cancel_text.contains("remotes/"),
        "a cancelled import must leave no received ref behind either: {branch_after_cancel_text}"
    );
    let verify_after_cancel = support::prikk(&cancel_copy).arg("verify").output().unwrap();
    assert!(
        verify_after_cancel.status.success(),
        "the cancelled repository must still verify clean: {verify_after_cancel:?}"
    );

    let after_cancel = store_bytes_excluding_locks_and_journal(&cancel_copy);
    assert_eq!(
        before_import, after_cancel,
        "every journaled container must be byte-identical to the destination's own state before \
         the import ever started"
    );

    let _ = std::fs::remove_dir_all(&source_repo);
    let _ = std::fs::remove_dir_all(&dest_repo);
    let _ = std::fs::remove_dir_all(&resume_copy);
    let _ = std::fs::remove_dir_all(&cancel_copy);
    let _ = std::fs::remove_file(&archive_path);
    let _ = std::fs::remove_file(&marker_path);
}

/// As `support::store_bytes`, but also excludes two files a successful cancel deliberately
/// changes on purpose, neither of which is a journaled container:
/// - `.prikk/import/journal` -- present-but-empty where it did not exist at all before the import
///   started (RFC 168 §6's own conservative choice to keep the journal's name rather than remove
///   it);
/// - `.prikk/recovery/log` -- the cancel's own "save before truncate" step appends the real bytes
///   it cut, exactly so `--recovery-restore` can undo the cancel later; growing this file is the
///   whole point of that step, not a leftover.
///
/// Comparing either directly would call an expected, documented difference a byte mismatch.
fn store_bytes_excluding_locks_and_journal(
    repo: &Path,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    let files = support::store_bytes(repo);
    files
        .into_iter()
        .filter(|(path, _)| {
            let is_import_journal = path.file_name().and_then(|name| name.to_str())
                == Some("journal")
                && path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("import");
            let is_recovery_log = path.file_name().and_then(|name| name.to_str()) == Some("log")
                && path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|name| name.to_str())
                    == Some("recovery");
            !is_import_journal && !is_recovery_log
        })
        // Keyed by the path relative to `repo` -- two different repositories' own absolute
        // temp-directory paths are never equal, by construction, so comparing the raw maps
        // `store_bytes` returns would always fail regardless of content.
        .map(|(path, bytes)| {
            (
                path.strip_prefix(repo)
                    .expect("store_bytes paths are under repo")
                    .to_path_buf(),
                bytes,
            )
        })
        .collect()
}
