//! DC-44's `bundle export` fix (`bundle-export-durability-handoff-v1.md`) had four siblings in
//! `sync.rs` with the identical defect (`sync-output-durability-handoff-v1.md`). Store-level,
//! filesystem-level coverage of the shared atomic-write primitive itself (induced failures,
//! temp-file cleanup) lives in `crates/prikk-cli/src/durable_output/tests.rs`; this file covers
//! what only the CLI surface can prove: which of the four sites refuse an existing destination
//! and which do not (§3.1), that `sync build`'s check does not fire on an `AlreadyInSync` run that
//! was never going to write anything (§3.3), and that `sync accept`'s check fires before the
//! artifact's objects are written into the repository at all (§3.3).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

fn sync_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "dc44-sync-durability-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

/// §3.1: `sync summary` and `sync have` are the two sites that write a freely-regenerable,
/// derived view of local state -- neither gates on `--force`, and a second write to the same
/// destination after the underlying state changed must simply succeed with the new content, the
/// same way a normal loop iteration expects.
#[test]
fn sync_summary_and_sync_have_overwrite_an_existing_destination_without_force() {
    let repo = support::unique_repo("dc44-sync-no-guard");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    let summary_file = sync_file("summary");
    support::ok(
        &support::prikk(&repo)
            .args([
                "sync",
                "summary",
                "--output",
                summary_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "first sync summary",
    );
    let first_summary_bytes = std::fs::read(&summary_file).unwrap();

    support::generation(&repo, "heads/main", "b.txt", b"second\n", "second");
    let second = support::prikk(&repo)
        .args([
            "sync",
            "summary",
            "--output",
            summary_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(
        &second,
        "second sync summary, no --force, over an existing file",
    );
    assert_ne!(
        std::fs::read(&summary_file).unwrap(),
        first_summary_bytes,
        "the second summary must reflect the new ref state, not be refused or left stale"
    );

    let have_file = sync_file("have");
    support::ok(
        &support::prikk(&repo)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "first sync have",
    );
    let first_have_bytes = std::fs::read(&have_file).unwrap();
    support::generation(&repo, "heads/main", "c.txt", b"third\n", "third");
    let have_again = support::prikk(&repo)
        .args([
            "sync",
            "have",
            "heads/main",
            "--output",
            have_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(
        &have_again,
        "second sync have, no --force, over an existing file",
    );
    assert_ne!(
        std::fs::read(&have_file).unwrap(),
        first_have_bytes,
        "the second have-list must reflect the new tip, not be refused or left stale"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_file(summary_file);
    let _ = std::fs::remove_file(have_file);
}

/// §3.1/§5 control 3, protected case, for `sync build`: refuses an existing destination without
/// `--force`, and the original artifact is untouched -- still a real artifact another repository
/// could accept, not merely byte-present.
#[test]
fn sync_build_without_force_refuses_an_existing_destination_and_leaves_it_verifiable() {
    let repo_a = support::unique_repo("dc44-sync-build-protected-a");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"first\n", "first");
    let repo_b = support::unique_repo("dc44-sync-build-protected-b");
    support::init(&repo_b);

    let have_file = sync_file("build-protected-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    let artifact_file = sync_file("build-protected-artifact");
    let build = |repo: &std::path::Path, force: bool| {
        let mut args = vec![
            "sync".to_string(),
            "build".to_string(),
            "heads/main".to_string(),
            "--have".to_string(),
            have_file.to_str().unwrap().to_string(),
            "--output".to_string(),
            artifact_file.to_str().unwrap().to_string(),
        ];
        if force {
            args.push("--force".to_string());
        }
        support::prikk(repo)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args(args)
            .output()
            .unwrap()
    };

    support::ok(&build(&repo_a, false), "first sync build (no prior file)");
    let original_bytes = std::fs::read(&artifact_file).unwrap();

    let second = build(&repo_a, false);
    assert!(
        !second.status.success(),
        "sync build over an existing artifact without --force must refuse"
    );
    let stderr = String::from_utf8_lossy(&second.stderr);
    assert!(
        stderr.contains("refusing to overwrite existing file"),
        "stderr: {stderr}"
    );
    assert_eq!(
        std::fs::read(&artifact_file).unwrap(),
        original_bytes,
        "the refused build must not alter the existing artifact at all"
    );

    // Still a real, independently acceptable artifact -- not merely present.
    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "the untouched original artifact must still accept");
    assert!(
        String::from_utf8_lossy(&accept.stdout).contains("patches: 1"),
        "must still carry the first build's own one patch"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    let _ = std::fs::remove_file(have_file);
    let _ = std::fs::remove_file(artifact_file);
}

/// §3.3, the decisive ordering case for `sync build`: a destination that already exists must
/// *not* be refused when this particular invocation resolves to `AlreadyInSync`, because no write
/// was ever going to happen -- the pre-existing file must survive completely untouched, and the
/// command must still report success, not a spurious collision error.
#[test]
fn sync_build_already_in_sync_does_not_refuse_on_an_existing_destination() {
    let repo = support::unique_repo("dc44-sync-build-already-in-sync");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"first\n", "first");

    // A have-list built from the repository's own current state claims the repository already
    // has everything it has -- so building against it must resolve to `AlreadyInSync`, with no
    // repository B required.
    let own_have_file = sync_file("already-in-sync-have");
    support::ok(
        &support::prikk(&repo)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                own_have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have (own state)",
    );

    let output_file = sync_file("already-in-sync-output");
    std::fs::write(&output_file, b"pre-existing, must survive untouched").unwrap();

    let build = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            own_have_file.to_str().unwrap(),
            "--output",
            output_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(
        &build,
        "an AlreadyInSync build must succeed even with a pre-existing file at --output",
    );
    assert!(
        String::from_utf8_lossy(&build.stdout).contains("already in sync"),
        "must report already-in-sync plainly"
    );
    assert_eq!(
        std::fs::read(&output_file).unwrap(),
        b"pre-existing, must survive untouched",
        "an AlreadyInSync run must never touch a pre-existing file it was never going to write"
    );

    let _ = std::fs::remove_dir_all(repo);
    let _ = std::fs::remove_file(own_have_file);
    let _ = std::fs::remove_file(output_file);
}

/// §5 control 3, permitted case, for `sync build`: `--force` overwrites, and the new artifact is
/// exactly what the second build actually produced.
#[test]
fn sync_build_with_force_overwrites_and_the_result_is_the_new_artifact() {
    let repo_a = support::unique_repo("dc44-sync-build-forced-a");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"first\n", "first");
    let repo_b = support::unique_repo("dc44-sync-build-forced-b");
    support::init(&repo_b);

    let have_file = sync_file("build-forced-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = sync_file("build-forced-artifact");
    support::ok(
        &support::prikk(&repo_a)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                artifact_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "first sync build",
    );

    support::generation(&repo_a, "heads/main", "b.txt", b"second\n", "second");
    let forced = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            artifact_file.to_str().unwrap(),
            "--force",
        ])
        .output()
        .unwrap();
    support::ok(&forced, "second sync build with --force must succeed");
    assert!(
        String::from_utf8_lossy(&forced.stdout).contains("delta patches: 2"),
        "the forced build must report both of A's patches, not the first build's one"
    );

    let accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&accept, "the forced artifact must accept");
    assert!(
        String::from_utf8_lossy(&accept.stdout).contains("patches: 2"),
        "must carry the forced build's own two patches, not the first build's one"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    let _ = std::fs::remove_file(have_file);
    let _ = std::fs::remove_file(artifact_file);
}

/// §5 control 1, through the real binary, for `sync build`: an artifact that cannot be written to
/// its destination directory leaves the previous artifact completely intact, and leaves no
/// abandoned temp file.
#[cfg(unix)]
#[test]
fn sync_build_failed_write_leaves_the_previous_artifact_verifiable() {
    use std::os::unix::fs::PermissionsExt;

    let repo_a = support::unique_repo("dc44-sync-build-failed-write-a");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"first\n", "first");
    let repo_b = support::unique_repo("dc44-sync-build-failed-write-b");
    support::init(&repo_b);

    let have_file = sync_file("build-failed-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    let output_dir = support::unique_repo("dc44-sync-build-failed-write-dir");
    let output = output_dir.join("artifact.bin");
    support::ok(
        &support::prikk(&repo_a)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "first build, before the directory is locked down",
    );
    let original_bytes = std::fs::read(&output).unwrap();

    support::generation(&repo_a, "heads/main", "b.txt", b"second\n", "second");
    let original_mode = std::fs::metadata(&output_dir).unwrap().permissions().mode();
    std::fs::set_permissions(&output_dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    let failed = support::prikk(&repo_a)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            output.to_str().unwrap(),
            "--force", // the refusal under test is the write itself, not the collision policy
        ])
        .output()
        .unwrap();
    std::fs::set_permissions(&output_dir, std::fs::Permissions::from_mode(original_mode)).unwrap();
    assert!(
        !failed.status.success(),
        "the build must fail when its destination directory refuses new files"
    );
    assert_eq!(
        std::fs::read(&output).unwrap(),
        original_bytes,
        "the previous artifact must be byte-for-byte untouched by the failed build"
    );
    let entries: Vec<_> = std::fs::read_dir(&output_dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        entries,
        vec![std::ffi::OsString::from("artifact.bin")],
        "no abandoned temp file may remain: {entries:?}"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    let _ = std::fs::remove_dir_all(output_dir);
    let _ = std::fs::remove_file(have_file);
}

/// §3.1/§5 control 3, both cases, for `sync accept --claims-out`. Two artifacts from the same
/// sender, built before and after a second commit -- so the second genuinely carries a different
/// (larger) set of claims, and a forced overwrite is verifiably a real content change, not a
/// coincidental no-op from re-accepting an identical artifact.
#[test]
fn sync_accept_claims_out_refuses_without_force_and_overwrites_with_it() {
    let repo_a = support::unique_repo("dc44-sync-accept-claims-a");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"first\n", "first");
    let repo_b = support::unique_repo("dc44-sync-accept-claims-b");
    support::init(&repo_b);

    let have_file = sync_file("accept-claims-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have (before either build)",
    );

    let build = |output: &std::path::Path| {
        support::prikk(&repo_a)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                output.to_str().unwrap(),
            ])
            .output()
            .unwrap()
    };

    let first_artifact = sync_file("accept-claims-artifact-1");
    support::ok(&build(&first_artifact), "first sync build (one commit)");

    let claims_file = sync_file("accept-claims-out");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "accept",
                first_artifact.to_str().unwrap(),
                "--claims-out",
                claims_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "first sync accept",
    );
    let original_claims = std::fs::read_to_string(&claims_file).unwrap();
    assert_eq!(
        original_claims
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count(),
        1,
        "the first accept's claims file must carry exactly one claim id: {original_claims:?}"
    );

    // A second commit in A, still measured against B's *original* have-list (B has not sealed
    // anything yet), so the second artifact's own delta and claim set are a real superset of the
    // first's, not identical.
    support::generation(&repo_a, "heads/main", "b.txt", b"second\n", "second");
    let second_artifact = sync_file("accept-claims-artifact-2");
    support::ok(&build(&second_artifact), "second sync build (two commits)");

    let refused = support::prikk(&repo_b)
        .args([
            "sync",
            "accept",
            second_artifact.to_str().unwrap(),
            "--claims-out",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "sync accept --claims-out over an existing file without --force must refuse"
    );
    assert!(
        String::from_utf8_lossy(&refused.stderr).contains("refusing to overwrite existing file"),
        "stderr: {}",
        String::from_utf8_lossy(&refused.stderr)
    );
    assert_eq!(
        std::fs::read_to_string(&claims_file).unwrap(),
        original_claims,
        "the refused accept must not alter the existing claims file at all"
    );

    let forced = support::prikk(&repo_b)
        .args([
            "sync",
            "accept",
            second_artifact.to_str().unwrap(),
            "--claims-out",
            claims_file.to_str().unwrap(),
            "--force",
        ])
        .output()
        .unwrap();
    support::ok(&forced, "sync accept --claims-out --force must overwrite");
    let forced_claims = std::fs::read_to_string(&claims_file).unwrap();
    assert_ne!(
        forced_claims, original_claims,
        "the forced write must carry the second artifact's own (larger) claim set, not the first's"
    );
    assert_eq!(
        forced_claims
            .lines()
            .filter(|l| !l.trim().is_empty())
            .count(),
        2,
        "the second artifact's own two claims must be written: {forced_claims:?}"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    let _ = std::fs::remove_file(have_file);
    let _ = std::fs::remove_file(first_artifact);
    let _ = std::fs::remove_file(second_artifact);
    let _ = std::fs::remove_file(claims_file);
}

/// §3.3, the decisive ordering case for `sync accept`: the collision check runs before
/// `accept_exchange_artifact` mutates the repository, so a refused `--claims-out` write must
/// leave the artifact entirely un-accepted -- not accepted-with-a-failed-claims-file. `sync
/// pending` reads that back directly: zero, not one.
#[test]
fn sync_accept_refusal_happens_before_the_artifact_is_accepted() {
    let repo_a = support::unique_repo("dc44-sync-accept-before-mutation-a");
    support::init(&repo_a);
    support::generation(&repo_a, "heads/main", "a.txt", b"first\n", "first");
    let repo_b = support::unique_repo("dc44-sync-accept-before-mutation-b");
    support::init(&repo_b);

    let have_file = sync_file("before-mutation-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact_file = sync_file("before-mutation-artifact");
    support::ok(
        &support::prikk(&repo_a)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args([
                "sync",
                "build",
                "heads/main",
                "--have",
                have_file.to_str().unwrap(),
                "--output",
                artifact_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );

    // A pre-existing file at the claims-out path, unrelated to any prior accept in this repo B.
    let claims_file = sync_file("before-mutation-claims");
    std::fs::write(&claims_file, b"unrelated pre-existing content").unwrap();

    let refused = support::prikk(&repo_b)
        .args([
            "sync",
            "accept",
            artifact_file.to_str().unwrap(),
            "--claims-out",
            claims_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        !refused.status.success(),
        "the claims-out collision must refuse before accepting anything"
    );

    let pending = support::prikk(&repo_b)
        .args(["sync", "pending"])
        .output()
        .unwrap();
    support::ok(&pending, "sync pending after the refused accept");
    assert!(
        String::from_utf8_lossy(&pending.stdout)
            .contains("pending (accepted, unsealed) patches: 0"),
        "the artifact must not have been accepted at all -- the refusal happened first"
    );

    // Now accept it for real, without the colliding flag, to confirm the artifact itself was
    // never consumed or otherwise damaged by the refused attempt.
    let real_accept = support::prikk(&repo_b)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(
        &real_accept,
        "the artifact must still accept normally afterward",
    );
    assert!(
        String::from_utf8_lossy(&real_accept.stdout).contains("patches: 1"),
        "the artifact's own one patch must still be intact"
    );

    let _ = std::fs::remove_dir_all(repo_a);
    let _ = std::fs::remove_dir_all(repo_b);
    let _ = std::fs::remove_file(have_file);
    let _ = std::fs::remove_file(artifact_file);
    let _ = std::fs::remove_file(claims_file);
}
