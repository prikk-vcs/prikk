//! CLI end-to-end regression for the DC-56 commit-index cache.
//!
//! Covers the two evidence obligations the RFC names explicitly: deletion/rebuild must leave commit
//! outcomes unchanged (NFR-PERF-04), and index/worktree divergence must be detectable and reported
//! (`verify`), not silently trusted. See
//! `rfcs/handoffs/DC-56-commit-full-tree-scan-compliance/cache-validity-specification-v1.md` for the
//! trust condition these tests exercise.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

mod support;

fn prikk(repo: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_prikk"));
    cmd.current_dir(repo);
    // RFC 148: prikk now reads key material from the user's own config directory, so a test that
    // does not neutralise it measures whoever is running it. See `support::isolate_key_environment`.
    support::isolate_key_environment(&mut cmd);
    cmd
}

fn ok(output: &Output, what: &str) {
    assert!(
        output.status.success(),
        "{what} failed (status {:?})\nstdout: {}\nstderr: {}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn fail(output: &Output, what: &str) {
    assert!(
        !output.status.success(),
        "{what} unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc56-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const AUTHOR_KEY_ID: &str = "dc56-test-author";
const AUTHOR_SEED_HEX: &str = "0011223344556677889900112233445566778899001122334455667788990011";
const MAINTAINER_KEY_ID: &str = "dc56-test-maintainer";
const MAINTAINER_SEED: [u8; 32] = [
    0x21, 0x21, 0x32, 0x32, 0x43, 0x43, 0x54, 0x54, 0x65, 0x65, 0x76, 0x76, 0x87, 0x87, 0x98, 0x98,
    0xa9, 0xa9, 0xba, 0xba, 0xcb, 0xcb, 0xdc, 0xdc, 0xed, 0xed, 0xfe, 0xfe, 0x0f, 0x0f, 0x10, 0x10,
];

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn maintainer_public_key_hex() -> String {
    use prikk_store::MaintainerSigner;
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(MAINTAINER_KEY_ID, &MAINTAINER_SEED)
            .expect("fixed maintainer seed derives a valid signer");
    hex(&signer.public_key_bytes())
}

fn init(repo: &Path) {
    ok(&prikk(repo).arg("init").output().unwrap(), "init");
}

fn commit(repo: &Path, message: &str) -> Output {
    prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(AUTHOR_SEED_HEX),
        )
        .args(["commit", "-m", message])
        .output()
        .unwrap()
}

fn seal(repo: &Path) {
    let out = prikk(repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            MAINTAINER_KEY_ID,
            "--public-key",
            &maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
    ok(&out, "trust maintainer add");

    let out = prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    ok(&out, "seal");
}

fn patch_id_line(stdout: &[u8]) -> String {
    String::from_utf8_lossy(stdout)
        .lines()
        .find(|line| line.starts_with("patch id: "))
        .expect("commit output must include a patch id line")
        .to_string()
}

fn commit_index_path(repo: &Path) -> PathBuf {
    repo.join(".prikk").join("cache").join("commit-index.v1")
}

fn copy_dir_recursive(src: &Path, dst: &Path) {
    std::fs::create_dir_all(dst).unwrap();
    for entry in std::fs::read_dir(src).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path);
        } else {
            std::fs::copy(entry.path(), &dst_path).unwrap();
        }
    }
}

/// The RFC's stated NFR-PERF-04 evidence obligation: deleting the commit-index cache and then
/// committing must produce a result identical to committing with the cache intact.
///
/// `prikk commit`'s fresh-node ids come from real OS randomness in production (only test builds
/// inject a deterministic generator), so two independently-run repositories — even with identical
/// content — cannot be compared directly: their genesis commits would mint different node ids and
/// every later patch built on them would differ for that reason alone, not because of the cache.
/// Isolating the cache as the only variable instead requires one shared lineage: commit and seal a
/// genesis baseline once, then byte-for-byte copy the whole repository before diverging — one copy
/// keeps its cache, the other has it deleted — so both copies start from identical node ids and
/// active-WAL state and differ only in whether the commit-index survives.
#[test]
fn deleting_the_index_does_not_change_commit_outcome() {
    let origin = unique_repo("rebuild-origin");
    init(&origin);
    std::fs::write(origin.join("a.txt"), "alpha content\n").unwrap();
    std::fs::write(origin.join("b.txt"), "bravo content\n").unwrap();
    std::fs::write(origin.join("c.txt"), "charlie content\n").unwrap();
    ok(&commit(&origin, "genesis"), "genesis commit");
    seal(&origin);
    std::fs::write(origin.join("b.txt"), "bravo content, edited\n").unwrap();

    let with_cache = unique_repo("rebuild-with-cache");
    let without_cache = unique_repo("rebuild-without-cache");
    copy_dir_recursive(&origin, &with_cache);
    copy_dir_recursive(&origin, &without_cache);

    assert!(commit_index_path(&with_cache).exists());
    std::fs::remove_file(commit_index_path(&without_cache)).unwrap();
    assert!(!commit_index_path(&without_cache).exists());

    // RFC 123: the message is now inside object identity, so it must be held constant across
    // both branches -- the cache is the only variable this test is isolating (see the doc comment
    // above). Before RFC 123 the two branches used different literal messages here and still
    // produced the same patch id, since the message was validated and discarded; that is no
    // longer true, correctly.
    let with_cache_output = commit(&with_cache, "second commit");
    ok(&with_cache_output, "second commit with cache intact");
    let without_cache_output = commit(&without_cache, "second commit");
    ok(&without_cache_output, "second commit with cache deleted");

    assert_eq!(
        patch_id_line(&with_cache_output.stdout),
        patch_id_line(&without_cache_output.stdout),
        "deleting the commit-index cache must not change the committed patch"
    );

    let _ = std::fs::remove_dir_all(&origin);
    let _ = std::fs::remove_dir_all(&with_cache);
    let _ = std::fs::remove_dir_all(&without_cache);
}

/// The index lives under `cache_dir()` (`rfcs/accepted/DC-56-COMMIT-FULL-TREE-SCAN-COMPLIANCE.md`
/// §7): `.prikk/cache/`, never a new top-level location.
#[test]
fn commit_populates_the_index_under_cache_dir() {
    let repo = unique_repo("cache-dir-location");
    init(&repo);
    std::fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");

    let index_path = commit_index_path(&repo);
    assert!(
        index_path.exists(),
        "commit-index.v1 must exist under .prikk/cache/ after a commit"
    );
    let contents = std::fs::read_to_string(&index_path).unwrap();
    assert!(contents.starts_with("PRIKK-COMMIT-INDEX-V1\n"));
    assert!(contents.contains("a.txt\t"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// The ordinary case: a repository committed once, untouched since, has no commit-index divergence.
#[test]
fn verify_reports_no_divergence_for_an_untouched_repository() {
    let repo = unique_repo("verify-clean");
    init(&repo);
    std::fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("commit-index divergences: 0"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// An ordinary uncommitted edit changes the file's stat, so the commit-index entry no longer matches
/// it — this is expected staleness, not divergence, and must not be reported as one (cache-validity
/// specification §6).
#[test]
fn verify_does_not_flag_an_ordinary_uncommitted_edit_as_divergence() {
    let repo = unique_repo("verify-uncommitted-edit");
    init(&repo);
    std::fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");

    std::fs::write(repo.join("a.txt"), "alpha, edited but not committed\n").unwrap();

    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("commit-index divergences: 0"));

    let _ = std::fs::remove_dir_all(&repo);
}

/// The divergence-detection test the RFC asks for explicitly: a deliberately stale index entry —
/// same recorded stat as the real file, but a content hash that disagrees with the file's actual
/// bytes (the mtime-granularity/clock-skew failure mode the cache-validity specification §5
/// describes) — must be reported by `verify`, not silently trusted.
#[test]
fn verify_reports_a_deliberately_stale_index_entry_as_divergence() {
    let repo = unique_repo("verify-stale-entry");
    init(&repo);
    std::fs::write(repo.join("a.txt"), "alpha\n").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");

    let index_path = commit_index_path(&repo);
    let original = std::fs::read_to_string(&index_path).unwrap();
    let mut lines: Vec<&str> = original.lines().collect();
    assert_eq!(
        lines.len(),
        2,
        "expected one header line and one entry line"
    );
    let entry_fields: Vec<&str> = lines[1].split('\t').collect();
    assert_eq!(
        entry_fields.len(),
        7,
        "expected the documented 7-field entry format"
    );
    // Replace only the trailing content-hash field, leaving size/mtime/mode — the trust
    // condition's inputs — untouched, so the entry still passes `matches_stat`.
    let fabricated_hash = "0".repeat(64);
    let corrupted_entry = format!(
        "{}\t{}\t{}\t{}\t{}\t{}\t{fabricated_hash}",
        entry_fields[0],
        entry_fields[1],
        entry_fields[2],
        entry_fields[3],
        entry_fields[4],
        entry_fields[5],
    );
    lines[1] = &corrupted_entry;
    let rewritten = format!("{}\n{}\n", lines[0], lines[1]);
    std::fs::write(&index_path, rewritten).unwrap();

    let out = prikk(&repo).arg("verify").output().unwrap();
    fail(&out, "verify with a deliberately stale index entry");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("commit-index divergences: 1"));
    assert!(stdout.contains("commit-index [divergence] a.txt"));

    let _ = std::fs::remove_dir_all(&repo);
}
