//! The checkout-refusal round (stikk's letter 012), through the compiled binary.
//!
//! A refused `checkout --patch-materialize`, `--patch-materialize-delete` or `--snapshot-materialize`
//! writes nothing, sets neither marker, names every conflicting path as a precondition, and leaves
//! `commit` working. A checkout that did stop part-way (a crash, or a file changed during it) is reported by
//! `status` and `doctor`, `commit` refuses naming the route, and each route clears it.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Run `args` with both signing keys, so a refusal cannot be a missing key's.
fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(args)
        .output()
        .unwrap()
}

/// Every worktree file outside `.prikk`, with its bytes.
fn worktree(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|name| name == ".prikk") {
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

fn marker_len(repo: &Path, name: &str) -> u64 {
    std::fs::metadata(repo.join(".prikk").join(name)).map_or(0, |metadata| metadata.len())
}

/// stikk's shape: `heads/main` holds `shared.txt` = `main`; `heads/dev` changes it to `dev` and adds
/// `dev-only.txt`. Left on `heads/main` with a clean worktree.
fn two_branch_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "shared.txt", b"main\n", "main");
    support::ok(
        &support::branch_create(&repo, "heads/dev", "heads/main"),
        "branch create heads/dev",
    );
    support::trust_maintainer(&repo);
    support::ok(
        &run(&repo, &["branch", "switch", "heads/dev"]),
        "switch to heads/dev",
    );
    std::fs::write(repo.join("shared.txt"), b"dev\n").unwrap();
    std::fs::write(repo.join("dev-only.txt"), b"dev only\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/dev", "dev"),
        "commit on heads/dev",
    );
    support::ok(&support::seal(&repo, "heads/dev"), "seal heads/dev");
    support::ok(
        &run(&repo, &["branch", "switch", "heads/main"]),
        "switch back to heads/main",
    );
    assert_eq!(
        std::fs::read(repo.join("shared.txt")).unwrap(),
        b"main\n",
        "fixture sanity: back on heads/main"
    );
    assert!(
        !repo.join("dev-only.txt").exists(),
        "fixture sanity: dev-only.txt removed"
    );
    repo
}

/// stikk's letter 012, on the binary, for both patch writers.
#[test]
fn a_refused_checkout_writes_nothing_and_commit_still_works() {
    let repo = two_branch_repo("crwn-stikk");
    for flag in ["--patch-materialize", "--patch-materialize-delete"] {
        let before = worktree(&repo);
        let refused = run(&repo, &["checkout", flag, "--ref", "heads/dev"]);
        assert_eq!(
            refused.status.code(),
            Some(1),
            "{flag}: {}",
            stderr(&refused)
        );
        assert!(
            stderr(&refused).starts_with(
                "error: precondition not met: refusing to materialize: 1 path(s) in the way: \
                 shared.txt (an existing file with different content)"
            ),
            "{flag}: {}",
            stderr(&refused)
        );
        assert_eq!(
            worktree(&repo),
            before,
            "{flag}: the refusal wrote dev-only.txt"
        );
        assert_eq!(
            marker_len(&repo, "worktree.marker"),
            0,
            "{flag}: dirty marker"
        );
        assert_eq!(
            marker_len(&repo, "worktree.provisional"),
            0,
            "{flag}: provisional marker"
        );
    }
    assert!(!stdout(&run(&repo, &["status"])).contains("interrupted materialization"));
    std::fs::write(repo.join("note.txt"), b"note\n").unwrap();
    support::ok(
        &run(
            &repo,
            &[
                "commit",
                "--ref",
                "heads/main",
                "-m",
                "after a refused checkout",
            ],
        ),
        "commit after a refused checkout",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Two conflicting paths are both named.
#[test]
fn a_refused_checkout_names_every_conflicting_path() {
    let repo = two_branch_repo("crwn-two-conflicts");
    std::fs::write(repo.join("dev-only.txt"), b"mine\n").unwrap();
    let before = worktree(&repo);
    let refused = run(
        &repo,
        &["checkout", "--patch-materialize", "--ref", "heads/dev"],
    );
    let message = stderr(&refused);
    assert!(message.contains("2 path(s) in the way"), "{message}");
    assert!(
        message.contains("dev-only.txt (an existing file with different content)"),
        "{message}"
    );
    assert!(
        message.contains("shared.txt (an existing file with different content)"),
        "{message}"
    );
    assert_eq!(worktree(&repo), before);
    let _ = std::fs::remove_dir_all(&repo);
}

/// `--snapshot-materialize` at a checkpoint tip, over a conflicting file, with the record deleted so the
/// provisional marker would be written: nothing is written and neither marker is set.
#[test]
fn a_refused_snapshot_materialization_sets_neither_marker() {
    let repo = support::unique_repo("crwn-snapshot");
    support::init(&repo);
    support::generation(&repo, "heads/main", "shared.txt", b"main\n", "main");
    support::trust_maintainer(&repo);
    let _ = std::fs::remove_file(repo.join(".prikk/cache/replay-verified-blocks.v1"));
    std::fs::write(repo.join("shared.txt"), b"local\n").unwrap();
    let before = worktree(&repo);
    let refused = run(&repo, &["checkout", "--snapshot-materialize"]);
    assert!(
        stderr(&refused).starts_with(
            "error: precondition not met: refusing to materialize: 1 path(s) in the way: \
             shared.txt (an existing file with different content)"
        ),
        "{}",
        stderr(&refused)
    );
    assert_eq!(worktree(&repo), before);
    assert_eq!(marker_len(&repo, "worktree.marker"), 0, "dirty marker");
    assert_eq!(
        marker_len(&repo, "worktree.provisional"),
        0,
        "provisional marker"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// A checkout of `heads/dev` that stopped part-way: `dev-only.txt` written, the RFC 102 dirty marker set.
/// The marker is written directly: after this round only a crash or a change during the checkout sets
/// it, and the store control `a_file_changed_during_the_checkout_keeps_the_marker_and_names_the_route`
/// reaches the real write-time refusal through a test seam.
fn stopped_checkout_repo(tag: &str) -> PathBuf {
    let repo = two_branch_repo(tag);
    std::fs::write(repo.join("dev-only.txt"), b"dev only\n").unwrap();
    std::fs::write(
        repo.join(".prikk/worktree.marker"),
        b"PRIKK-WORKTREE-DIRTY\n",
    )
    .unwrap();
    repo
}

/// `status` (prose and JSON) and `doctor` report a stopped checkout with the route, `commit` refuses naming
/// it, and each of the two routes clears the marker so `commit` works again.
#[test]
fn a_stopped_checkout_is_reported_and_each_route_clears_it() {
    let clean = two_branch_repo("crwn-reported-clean");
    assert!(
        stdout(&run(&clean, &["status", "--format", "json"]))
            .contains("\"interrupted_materialization\": null"),
        "the field is present and null when nothing stopped"
    );
    let _ = std::fs::remove_dir_all(&clean);

    for (tag, route) in [
        (
            "crwn-route-materialize",
            vec!["checkout", "--patch-materialize", "--ref", "heads/main"],
        ),
        ("crwn-route-switch", vec!["branch", "switch", "heads/main"]),
    ] {
        let repo = stopped_checkout_repo(tag);
        let status = stdout(&run(&repo, &["status"]));
        assert!(
            status.contains(
                "interrupted materialization: a checkout or branch switch stopped part-way; move \
                 aside any file it named, then run prikk checkout --patch-materialize --ref heads/main \
                 or prikk branch switch heads/main"
            ),
            "{status}"
        );
        let json = stdout(&run(&repo, &["status", "--format", "json"]));
        assert!(
            json.contains(
                "\"interrupted_materialization\": {\"routes\": [\"prikk checkout \
                 --patch-materialize --ref heads/main\", \"prikk branch switch heads/main\"]}"
            ),
            "{json}"
        );
        let doctor = run(&repo, &["doctor"]);
        let doctor_text = format!("{}{}", stdout(&doctor), stderr(&doctor));
        assert!(
            doctor_text.contains("PRIKK-DOCTOR-INTERRUPTED-MATERIALIZATION")
                && doctor_text.contains("prikk branch switch heads/main"),
            "{doctor_text}"
        );
        std::fs::write(repo.join("note.txt"), b"note\n").unwrap();
        let refused = run(&repo, &["commit", "--ref", "heads/main", "-m", "blocked"]);
        assert!(
            stderr(&refused).starts_with(
                "error: precondition not met: worktree materialization was interrupted"
            ) && stderr(&refused).contains("`prikk branch switch <the current branch>`"),
            "{}",
            stderr(&refused)
        );

        support::ok(&run(&repo, &route), &format!("the route {route:?}"));
        assert_eq!(
            marker_len(&repo, "worktree.marker"),
            0,
            "{route:?} clears the marker"
        );
        assert!(!stdout(&run(&repo, &["status"])).contains("interrupted materialization"));
        support::ok(
            &run(
                &repo,
                &["commit", "--ref", "heads/main", "-m", "after the route"],
            ),
            &format!("commit after {route:?}"),
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}
