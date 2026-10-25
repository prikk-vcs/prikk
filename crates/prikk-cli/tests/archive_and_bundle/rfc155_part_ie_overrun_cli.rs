//! RFC 155 Part I-E item 0(e): a permanent CLI-level test of the overrun-torn-frame shape --
//! I-D2's own item 3 was delivered only at the library level; the architect ran this manually at
//! the CLI (review `rfc155-impl-ID2-review-v1`) and confirmed it holds. This test reproduces that
//! exact manual run as a real, permanent check: a genuine crash-shaped torn frame, overrun by a
//! later real write, exported and imported through the real `prikk` binary.
//!
//! Debug build, not `#[ignore]`d -- this is a correctness check, not a performance measurement
//! (the real-kill test, `rfc155_part_ib2_real_kill.rs`, is this test's own precedent for that
//! distinction: a real binary, a real on-disk container file edited directly, run under the
//! ordinary `test` gate).
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use support::ok;

/// Append a real-looking but torn frame to a blob container: a genuine header (right magic,
/// version, and a claimed body length far longer than what follows) with only 10 of its own
/// claimed body bytes actually present -- the same shape a crash mid-append leaves, and the exact
/// construction the library-level `an_overrun_torn_frame_is_an_interrupted_append_...` test and
/// `support::append_torn_ref_log_tail` already use for the ref log's own equivalent.
fn append_torn_blob_frame(repo: &std::path::Path) {
    const MAGIC: &[u8; 8] = b"PCONBLB1";
    let container_path = repo.join(".prikk/containers/blob/a.container");
    let mut torn = Vec::new();
    torn.extend_from_slice(MAGIC);
    torn.extend_from_slice(&1u16.to_be_bytes()); // CONTAINER_VERSION
    // A claimed body length that overruns the 10 real junk bytes into the next, real frame's own
    // header -- but stays within the section's own total remaining bytes, so this is a checksum
    // mismatch the overrun rule resolves to an interrupted append, not a `TrailingPartial` (which
    // would need a claim past the *whole section's* own end, a different shape already covered by
    // `agrees_on_a_torn_frame_claiming_past_the_end_with_sound_frames_after`).
    torn.extend_from_slice(&40u64.to_be_bytes());
    torn.extend_from_slice(&[0xABu8; 32]); // a checksum never reached, since the body never finishes
    torn.extend_from_slice(&[0u8; 10]); // only 10 of the claimed body bytes actually arrive
    use std::io::Write as _;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&container_path)
        .expect("open blob container for append");
    file.write_all(&torn).expect("append the torn frame");
}

#[test]
fn an_overrun_torn_frame_survives_export_verify_import_at_the_cli() {
    let repo = support::unique_repo("rfc155-ie-torn-frame-source");
    support::init(&repo);
    support::generation(
        &repo,
        "heads/main",
        "a.txt",
        b"the first file, which finishes writing\n",
        "first generation",
    );

    append_torn_blob_frame(&repo);

    // Containers are append-only: the store has no idea the file now ends in torn bytes, and
    // appends its own next record right after them -- the new record's own header starts inside
    // the range the torn header's own (never-validated) length claims. This is the review's own
    // exact overrun shape.
    support::generation(
        &repo,
        "heads/main",
        "b.txt",
        b"the second file, written after the crash\n",
        "second generation",
    );

    // The source repository's own `prikk verify` must exit 0: a live repository only warns about
    // an interrupted append, it never refuses on one.
    let source_verify = support::verify(&repo);
    assert!(
        source_verify.status.success(),
        "the source repository's own verify must exit 0: {}",
        String::from_utf8_lossy(&source_verify.stderr)
    );

    let mut archive_path = std::env::temp_dir();
    archive_path.push(format!(
        "prikk-cli-rfc155-ie-torn-frame-archive-{}.prepo",
        support::unique_suffix()
    ));
    ok(
        &support::prikk(&repo)
            .args(["archive", "export"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive export",
    );

    let archive_verify = support::prikk(&repo)
        .args(["archive", "verify"])
        .arg(&archive_path)
        .output()
        .unwrap();
    assert!(
        archive_verify.status.success(),
        "archive verify must exit 0 (internally consistent): stdout={} stderr={}",
        String::from_utf8_lossy(&archive_verify.stdout),
        String::from_utf8_lossy(&archive_verify.stderr)
    );
    let archive_verify_stdout = String::from_utf8_lossy(&archive_verify.stdout);
    assert!(
        archive_verify_stdout.contains("interrupted append"),
        "archive verify must name the overrun as an interrupted append, in its own \
         vocabulary, not as a checksum failure: {archive_verify_stdout}"
    );
    assert!(
        !archive_verify_stdout
            .to_lowercase()
            .contains("failed its own checksum"),
        "an interrupted append must never be printed as a checksum failure: \
         {archive_verify_stdout}"
    );

    let dest = support::unique_repo("rfc155-ie-torn-frame-dest");
    support::init(&dest);
    ok(
        &support::prikk(&dest)
            .args(["archive", "import"])
            .arg(&archive_path)
            .output()
            .unwrap(),
        "archive import",
    );

    // The second file, written after the crash, must have made it into the destination -- the
    // overrun torn frame must never have hidden it.
    let cat_second_file = support::prikk(&dest)
        .args(["cat", "--path", "b.txt", "--ref", "remotes/heads/main"])
        .output()
        .unwrap();
    assert!(
        cat_second_file.status.success(),
        "the destination must read the second (post-crash) file back: stderr={}",
        String::from_utf8_lossy(&cat_second_file.stderr)
    );
    assert_eq!(
        cat_second_file.stdout, b"the second file, written after the crash\n",
        "the second file's own content must survive the archive round trip unchanged"
    );

    // The destination's own `prikk verify` exits non-zero afterward, but only for the expected,
    // unrelated reason: no maintainer key has been adopted at the destination yet. Confirm this
    // is the only reason, not a symptom of the overrun shape leaking into the destination.
    let dest_verify = support::verify(&dest);
    assert!(
        !dest_verify.status.success(),
        "the destination's own verify must fail until a maintainer key is adopted"
    );
    let dest_verify_stdout = String::from_utf8_lossy(&dest_verify.stdout);
    let dest_verify_stderr = String::from_utf8_lossy(&dest_verify.stderr);
    let combined = format!("{dest_verify_stdout}{dest_verify_stderr}");
    assert!(
        combined.to_lowercase().contains("trust")
            || combined.to_lowercase().contains("publication"),
        "the destination's verify failure must name the untrusted-publication reason, not \
         anything related to the overrun shape: stdout={dest_verify_stdout} \
         stderr={dest_verify_stderr}"
    );
    assert!(
        !combined.to_lowercase().contains("checksum")
            && !combined.to_lowercase().contains("damage")
            && !combined.to_lowercase().contains("overrun"),
        "the destination's verify failure must not mention the overrun shape at all: \
         stdout={dest_verify_stdout} stderr={dest_verify_stderr}"
    );

    let _ = std::fs::remove_file(&archive_path);
}
