//! 0.55.0 step 3 Part 3a (R2): `bundle import --input <fifo>` against a genuinely non-seekable
//! input. `crates/prikk-cli/src/bounded_read.rs`'s own module doc already names `bundle import`
//! as one of its six readers, so the buffered fallback this row covers already exists and already
//! applies here -- the three `control3_*` FIFO tests in `rfc158_incoming_bound.rs` prove the
//! identical `read_bounded_file` path, but only ever through `bundle verify`, never through
//! `bundle import` itself. This closes that gap: a real FIFO, fed a real exported bundle's bytes,
//! imported successfully, landing the same state a regular-file import of the identical bytes
//! would.
//!
//! Linux-only, matching `rfc158_incoming_bound.rs`'s own gate for its FIFO controls.
#![cfg(target_os = "linux")]
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

fn make_fifo(path: &Path) {
    let made = std::process::Command::new("mkfifo")
        .arg(path)
        .status()
        .unwrap();
    assert!(made.success(), "mkfifo");
}

/// Feed `bytes` into the FIFO at `path` from a background thread, the same non-blocking-open,
/// bounded-retry shape `rfc158_incoming_bound.rs`'s own `feed_fifo` uses (never able to block the
/// test forever if `prikk` exits before opening the FIFO) -- but writing the real bundle's own
/// bytes verbatim, not a filler pattern, since the reader here must actually decode a valid
/// bundle.
const O_NONBLOCK: i32 = 0o4000;

fn feed_fifo_bytes(path: &Path, bytes: Vec<u8>) -> std::thread::JoinHandle<()> {
    let path = path.to_path_buf();
    std::thread::spawn(move || {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut file = loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .custom_flags(O_NONBLOCK)
                .open(&path)
            {
                Ok(file) => break file,
                Err(_) if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(_) => return,
            }
        };
        let mut written = 0usize;
        while written < bytes.len() && std::time::Instant::now() < deadline {
            match file.write(&bytes[written..]) {
                Ok(0) => break,
                Ok(n) => written += n,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err(_) => break,
            }
        }
    })
}

#[test]
fn bundle_import_through_a_real_fifo_succeeds_exactly_as_a_regular_file_would() {
    // The source: two generations, exported to a real file first (so we have the bytes to feed
    // through the FIFO) and separately imported into a reference repository from that regular
    // file, to compare against.
    let source_repo = support::unique_repo("bundle-fifo-source");
    support::init(&source_repo);
    support::generation(&source_repo, "heads/main", "a.txt", b"first\n", "first");
    support::generation(&source_repo, "heads/main", "b.txt", b"second\n", "second");

    let mut bundle_path = std::env::temp_dir();
    bundle_path.push(format!(
        "prikk-cli-bundle-fifo-{}.bin",
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
    let bundle_bytes = std::fs::read(&bundle_path).unwrap();

    let reference_repo = support::unique_repo("bundle-fifo-reference");
    support::init(&reference_repo);
    let reference_import = support::prikk(&reference_repo)
        .args(["bundle", "import", "--input", bundle_path.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&reference_import, "reference import from a regular file");

    // The FIFO destination: the identical bytes, fed through a genuinely non-seekable pipe.
    let fifo_repo = support::unique_repo("bundle-fifo-dest");
    support::init(&fifo_repo);
    let fifo_path = fifo_repo.join("bundle.fifo");
    make_fifo(&fifo_path);

    let writer = feed_fifo_bytes(&fifo_path, bundle_bytes);

    let fifo_import = support::prikk(&fifo_repo)
        .args(["bundle", "import", "--input", "bundle.fifo"])
        .output()
        .unwrap();
    let _ = writer.join();
    support::ok(&fifo_import, "import through a real FIFO");
    let fifo_import_stdout = String::from_utf8_lossy(&fifo_import.stdout);
    let reference_import_stdout = String::from_utf8_lossy(&reference_import.stdout);
    assert_eq!(
        fifo_import_stdout, reference_import_stdout,
        "importing the identical bundle through a FIFO must report the same object counts as \
         importing it from a regular file"
    );

    // Both destinations must land the same branch/verify state.
    let fifo_branch = support::prikk(&fifo_repo).arg("branch").output().unwrap();
    support::ok(&fifo_branch, "branch list (fifo destination)");
    let reference_branch = support::prikk(&reference_repo)
        .arg("branch")
        .output()
        .unwrap();
    support::ok(&reference_branch, "branch list (reference destination)");
    assert_eq!(
        String::from_utf8_lossy(&fifo_branch.stdout),
        String::from_utf8_lossy(&reference_branch.stdout),
        "a FIFO-fed import must land the identical branch state as a regular-file import"
    );

    // A plain `verify` here reports "publication trust issues" (no maintainer key adopted in
    // either repository -- unrelated to this test, the same shape `dc78_bundle_exchange.rs`'s own
    // tests document: "verify reports it untrusted until adopted"), so this checks only the
    // integrity rows that import itself is responsible for, not the trust-policy exit status.
    let fifo_verify = support::prikk(&fifo_repo).arg("verify").output().unwrap();
    let fifo_verify_stdout = String::from_utf8_lossy(&fifo_verify.stdout);
    assert!(
        fifo_verify_stdout.contains("unreferenced remnants: 0")
            && fifo_verify_stdout.contains("unreached objects: 0")
            && fifo_verify_stdout.contains("interrupted appends: 0"),
        "a FIFO-fed import must leave an integrity-clean repository: {fifo_verify_stdout}"
    );

    let _ = std::fs::remove_dir_all(&source_repo);
    let _ = std::fs::remove_dir_all(&reference_repo);
    let _ = std::fs::remove_dir_all(&fifo_repo);
    let _ = std::fs::remove_file(&bundle_path);
}
