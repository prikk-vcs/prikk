//! RFC 135 §6: `prikk key` and `prikk setup`, driven through the compiled binary.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;

fn extract_after(stdout: &str, prefix: &str) -> Option<String> {
    stdout
        .lines()
        .find_map(|line| line.trim_start().strip_prefix(prefix))
        .map(str::trim)
        .map(str::to_string)
}

/// Control 6: two independent draws must differ -- the whole claim that the CSPRNG is wired.
#[test]
fn key_generate_twice_produces_different_output() {
    let repo = support::unique_repo("rfc135-key-generate-twice");
    let first = support::prikk(&repo)
        .args(["key", "generate"])
        .output()
        .unwrap();
    support::ok(&first, "key generate (first)");
    let second = support::prikk(&repo)
        .args(["key", "generate"])
        .output()
        .unwrap();
    support::ok(&second, "key generate (second)");
    assert_ne!(
        first.stdout, second.stdout,
        "two independent `key generate` runs must not print the same seed or public key"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3 (path-inside-.prikk half): refused regardless of platform, since this check runs
/// before the platform-specific write.
#[test]
fn key_generate_out_refuses_a_path_inside_dot_prikk() {
    let repo = support::unique_repo("rfc135-key-generate-dot-prikk");
    std::fs::create_dir_all(repo.join(".prikk")).unwrap();
    let out = support::prikk(&repo)
        .args(["key", "generate", "--out", ".prikk/seed"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "key generate --out into .prikk/ must be refused"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains(".prikk"),
        "refusal must name the reason: {stderr}"
    );
    assert!(
        !repo.join(".prikk/seed").exists(),
        "no seed file may be written when the path is refused"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[cfg(unix)]
mod unix_only {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// Control 2: `--out` never prints the seed, asserted on captured stdout/stderr.
    #[test]
    fn key_generate_out_never_prints_the_seed() {
        let repo = support::unique_repo("rfc135-key-generate-out-no-print");
        let seed_path = repo.join("author.seed");
        let out = support::prikk(&repo)
            .args(["key", "generate", "--out", seed_path.to_str().unwrap()])
            .output()
            .unwrap();
        support::ok(&out, "key generate --out");
        let stdout = String::from_utf8_lossy(&out.stdout);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(
            !stdout.contains("seed:") && !stdout.starts_with("seed"),
            "stdout must not print the seed when --out is given: {stdout}"
        );
        let seed_hex = std::fs::read_to_string(&seed_path).unwrap();
        let seed_hex = seed_hex.trim();
        assert_eq!(seed_hex.len(), 64, "the written seed must be 64 hex chars");
        assert!(
            !stdout.contains(seed_hex) && !stderr.contains(seed_hex),
            "the seed's own hex must not appear anywhere in stdout or stderr: stdout={stdout} stderr={stderr}"
        );
        assert!(
            stdout.contains("public key:"),
            "stdout must still show the public key: {stdout}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// Control 3 (mode + overwrite-refusal half).
    #[test]
    fn key_generate_out_sets_mode_0600_and_refuses_overwrite() {
        let repo = support::unique_repo("rfc135-key-generate-out-mode");
        let seed_path = repo.join("author.seed");
        support::ok(
            &support::prikk(&repo)
                .args(["key", "generate", "--out", seed_path.to_str().unwrap()])
                .output()
                .unwrap(),
            "key generate --out",
        );
        let mode = std::fs::metadata(&seed_path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "seed file must be written at mode 0600");

        let second = support::prikk(&repo)
            .args(["key", "generate", "--out", seed_path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            !second.status.success(),
            "key generate --out must refuse to overwrite an existing file"
        );
        let stderr = String::from_utf8_lossy(&second.stderr);
        assert!(
            stderr.contains("overwrite") || stderr.contains("exist"),
            "refusal must explain why: {stderr}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }

    /// Control 4: a generated seed round-trips through `key public --seed-env`.
    #[test]
    fn key_generate_out_then_key_public_round_trips() {
        let repo = support::unique_repo("rfc135-key-round-trip");
        let seed_path = repo.join("maintainer.seed");
        let generate = support::prikk(&repo)
            .args(["key", "generate", "--out", seed_path.to_str().unwrap()])
            .output()
            .unwrap();
        support::ok(&generate, "key generate --out");
        let generate_stdout = String::from_utf8_lossy(&generate.stdout);
        let generated_public_key = extract_after(&generate_stdout, "public key:")
            .expect("key generate must print a public key line");

        let seed_hex = std::fs::read_to_string(&seed_path).unwrap();
        let public = support::prikk(&repo)
            .env("RFC135_ROUND_TRIP_SEED", seed_hex.trim())
            .args(["key", "public", "--seed-env", "RFC135_ROUND_TRIP_SEED"])
            .output()
            .unwrap();
        support::ok(&public, "key public --seed-env");
        let public_stdout = String::from_utf8_lossy(&public.stdout);
        let derived_public_key = extract_after(&public_stdout, "public key:")
            .expect("key public must print a public key line");

        assert_eq!(
            generated_public_key, derived_public_key,
            "the public key derived from the written seed must match the one `key generate` printed"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[cfg(windows)]
mod windows_only {
    use super::*;

    /// §2.1's default ruling: `--out` refuses outright on Windows, naming the reason, rather than
    /// writing a secret at inherited permissions silently.
    #[test]
    fn key_generate_out_refuses_on_windows() {
        let repo = support::unique_repo("rfc135-key-generate-out-windows");
        let seed_path = repo.join("author.seed");
        let out = support::prikk(&repo)
            .args(["key", "generate", "--out", seed_path.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            !out.status.success(),
            "key generate --out must refuse on Windows"
        );
        assert!(
            !seed_path.exists(),
            "no seed file may be written when --out is refused"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// Control 5: `setup` prints the trust decision it makes, not just "repository ready."
#[test]
fn setup_prints_the_trust_decision() {
    let repo = support::unique_repo("rfc135-setup-trust");
    let out = support::prikk(Path::new("."))
        .args(["setup", repo.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&out, "setup");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("trusted maintainer key:"),
        "setup must print the same trust line `trust maintainer add` itself prints: {stdout}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Property 1/end-to-end: one command reaches a working repository, and its printed exports
/// actually authorize a real commit and seal -- not asserted from reading the source.
#[test]
fn setup_reaches_a_sealed_commit() {
    let repo = support::unique_repo("rfc135-setup-e2e");
    // `setup` must create a missing leading directory itself (property 1: no other step first).
    let target = repo.join("nested/does/not/exist/yet");
    let out = support::prikk(Path::new("."))
        .args(["setup", target.to_str().unwrap()])
        .output()
        .unwrap();
    support::ok(&out, "setup");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let author_seed = extract_after(&stdout, "export PRIKK_AUTHOR_SEED=\"")
        .map(|s| s.trim_end_matches('"').to_string())
        .expect("setup must print an AUTHOR seed export line");
    let maintainer_seed = extract_after(&stdout, "export PRIKK_MAINTAINER_SEED=\"")
        .map(|s| s.trim_end_matches('"').to_string())
        .expect("setup must print a MAINTAINER seed export line");
    assert_eq!(author_seed.len(), 64);
    assert_eq!(maintainer_seed.len(), 64);
    assert_ne!(
        author_seed, maintainer_seed,
        "setup must generate two independent seeds, one per role"
    );

    std::fs::write(target.join("f.txt"), b"hello").unwrap();
    let commit = support::prikk(&target)
        .env("PRIKK_AUTHOR_KEY_ID", "author")
        .env("PRIKK_AUTHOR_SEED", &author_seed)
        .args(["commit", "--from-worktree", "-m", "genesis"])
        .output()
        .unwrap();
    support::ok(&commit, "commit");
    let seal = support::prikk(&target)
        .env("PRIKK_MAINTAINER_KEY_ID", "maintainer")
        .env("PRIKK_MAINTAINER_SEED", &maintainer_seed)
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    support::ok(&seal, "seal");
    support::ok(&support::verify(&target), "verify");
    let _ = std::fs::remove_dir_all(&repo);
}

/// RFC 135: `setup` on a directory that already holds a repository refuses **before touching
/// anything**.
///
/// Before this, `RepositoryLayout::init`'s idempotence carried the re-run past the point of no
/// return: "initialized Prikk repository at …" printed, two fresh keys were minted, any
/// `--*-seed-out` file was written, and only then did the trust step collide. The assertions below
/// are therefore about what did **not** happen, which is the whole point — a message alone would
/// pass just as well with the writes still occurring behind it.
#[test]
fn setup_on_an_existing_repository_refuses_before_writing_anything() {
    let root = support::unique_repo("rfc135-setup-rerun");
    let repo = root.join("r");
    support::ok(
        &support::prikk(Path::new("."))
            .args(["setup", repo.to_str().unwrap()])
            .output()
            .unwrap(),
        "first setup",
    );

    let author_seed = root.join("author.seed");
    let maintainer_seed = root.join("maintainer.seed");
    let rerun = support::prikk(Path::new("."))
        .args([
            "setup",
            repo.to_str().unwrap(),
            "--author-seed-out",
            author_seed.to_str().unwrap(),
            "--maintainer-seed-out",
            maintainer_seed.to_str().unwrap(),
        ])
        .output()
        .unwrap();

    assert_eq!(rerun.status.code(), Some(1), "a re-run must refuse");
    let stderr = String::from_utf8_lossy(&rerun.stderr).into_owned();
    assert!(
        stderr.contains("already holds a repository"),
        "and say why: {stderr}"
    );
    // The refusal names both ways out; a dead end would be a worse message, not a shorter one.
    assert!(
        stderr.contains("prikk trust maintainer add") && stderr.contains("different directory"),
        "the refusal must name both routes: {stderr}"
    );
    assert!(
        stderr.starts_with("error: precondition not met: "),
        "a caller-fixable state is a precondition, not damage: {stderr}"
    );

    let stdout = String::from_utf8_lossy(&rerun.stdout).into_owned();
    assert!(
        !stdout.contains("initialized"),
        "nothing may claim to have initialized anything: {stdout}"
    );
    assert!(
        !stdout
            .chars()
            .collect::<Vec<_>>()
            .windows(64)
            .any(|window| { window.iter().all(|c| c.is_ascii_hexdigit()) }),
        "no 64-hex run may reach stdout -- a minted seed must never be printed here: {stdout}"
    );
    assert!(
        !author_seed.exists() && !maintainer_seed.exists(),
        "no seed file may be written: a seed on disk that no repository adopted is worse than \
         none, because the user has every reason to think it is theirs"
    );

    // And the repository that was already there is untouched.
    support::ok(
        &support::verify(&repo),
        "verify the pre-existing repository",
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// The other half: the `create_dir_all` property RFC 135 added must survive the new check — a
/// fresh path, and a fresh path whose parents do not exist yet, both still work.
#[test]
fn setup_still_creates_a_fresh_and_a_nested_fresh_directory() {
    let root = support::unique_repo("rfc135-setup-fresh");
    for relative in ["fresh", "deep/nested/does/not/exist"] {
        let target = root.join(relative);
        let out = support::prikk(Path::new("."))
            .args(["setup", target.to_str().unwrap()])
            .output()
            .unwrap();
        support::ok(&out, &format!("setup {relative}"));
        assert!(
            target.join(".prikk").exists(),
            "{relative}: setup must still create the repository"
        );
    }
    let _ = std::fs::remove_dir_all(&root);
}
