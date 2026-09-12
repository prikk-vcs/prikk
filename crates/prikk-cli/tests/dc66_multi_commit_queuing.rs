//! CLI end-to-end regression for DC-66: multiple `commit`s with no `seal` in between now queue
//! rather than refuse, driving the compiled `prikk` binary exactly as a real user would. Exercises
//! the full user-facing surface named in the RFC's acceptance criteria: `commit` no longer refuses on
//! a non-empty active WAL, `status` reports the queue, `seal` batches N patches into one block, and
//! `verify` reports zero divergence afterward. See
//! `rfcs/handoffs/DC-66-multi-commit-queuing/prerequisite-questions-v1.md` and
//! `queuing-baseline-design-v1.md` for the root design.

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

fn unique_repo(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("prikk-cli-dc66-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

const AUTHOR_KEY_ID: &str = "dc66-test-author";
const AUTHOR_SEED_HEX: &str = "1100223344556677889900112233445566778899001122334455667788990011";
const MAINTAINER_KEY_ID: &str = "dc66-test-maintainer";
const MAINTAINER_SEED: [u8; 32] = [
    0x51, 0x51, 0x62, 0x62, 0x73, 0x73, 0x84, 0x84, 0x95, 0x95, 0xa6, 0xa6, 0xb7, 0xb7, 0xc8, 0xc8,
    0xd9, 0xd9, 0xea, 0xea, 0xfb, 0xfb, 0x0c, 0x0c, 0x1d, 0x1d, 0x2e, 0x2e, 0x3f, 0x3f, 0x40, 0x40,
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

fn status(repo: &Path) -> String {
    let out = prikk(repo).arg("status").output().unwrap();
    ok(&out, "status");
    String::from_utf8_lossy(&out.stdout).into_owned()
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
    let _ = out; // idempotent-enough for repeated calls within one test; only `seal` must succeed.

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

/// The exact scenario the RFC opens with: `commit; commit` with no `seal` in between, which used to
/// print "active WAL already contains patches ... run prikk seal before committing again" and now
/// queues. Two distinct new files across two queued commits (node-identity surface), `status`
/// reporting the queue at each step, one `seal` batching both, and `verify` reporting zero divergence
/// afterward.
#[test]
fn two_commits_with_no_seal_between_queue_and_seal_together() {
    let repo = unique_repo("two-queued-commits");
    init(&repo);

    std::fs::write(repo.join("base.txt"), "base").unwrap();
    ok(&commit(&repo, "genesis"), "genesis commit");
    seal(&repo);
    assert!(status(&repo).contains("queued patches: 0"));

    std::fs::write(repo.join("a.txt"), "alpha").unwrap();
    let out = commit(&repo, "queue a");
    ok(&out, "queue a commit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains("already contains patches"),
        "the second commit must queue, not refuse: {stdout}"
    );
    let after_first = status(&repo);
    assert!(
        after_first.contains("queued patches: 1 targeting heads/main"),
        "status after the first queued commit: {after_first}"
    );

    std::fs::write(repo.join("b.txt"), "beta").unwrap();
    let out = commit(&repo, "queue b");
    ok(&out, "queue b commit");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("create-file b.txt"),
        "the second queued commit must see a.txt as already existing and only create b.txt: {stdout}"
    );
    let after_second = status(&repo);
    assert!(
        after_second.contains("queued patches: 2 targeting heads/main"),
        "status after both queued commits: {after_second}"
    );

    seal(&repo);
    let after_seal = status(&repo);
    assert!(
        after_seal.contains("queued patches: 0"),
        "status after seal must show an empty queue: {after_seal}"
    );

    // Both queued files are live and correctly attributed — proving the chain-baseline fold, not
    // just that the commits individually succeeded.
    let out = prikk(&repo).arg("verify").output().unwrap();
    ok(&out, "verify");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("lifecycle-cache divergences: 0"));
    assert!(stdout.contains("commit-index divergences: 0"));
    assert!(stdout.contains("active WAL ordering issues: 0"));

    // Checkout/materialization must independently reconstruct both queued files from the sealed
    // two-patch block.
    let materialize_root = unique_repo("two-queued-commits-materialize");
    std::fs::create_dir_all(materialize_root.join(".prikk")).unwrap();
    copy_dir_recursive(&repo.join(".prikk"), &materialize_root.join(".prikk"));
    let out = prikk(&materialize_root)
        .arg("checkout")
        .arg("--patch-materialize")
        .output()
        .unwrap();
    ok(&out, "checkout --patch-materialize");
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("a.txt")).unwrap(),
        "alpha"
    );
    assert_eq!(
        std::fs::read_to_string(materialize_root.join("b.txt")).unwrap(),
        "beta"
    );

    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&materialize_root);
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
