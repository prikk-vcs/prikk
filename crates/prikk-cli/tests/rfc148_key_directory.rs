//! RFC 148: two places per role, in order, and no third.
//!
//! These are the controls the handoff names, minus control 1 — that one lives in
//! `rfc135_key_and_setup.rs::setup_reaches_a_sealed_commit_with_no_environment_at_all`, beside the
//! `setup` behaviour it exercises, rather than being restated here.
//!
//! Every test drives the real binary with an explicitly constructed environment, because the thing
//! under test *is* the environment: a test that inherited the developer's own `XDG_CONFIG_HOME`
//! would be measuring their machine. `support::prikk` neutralises it; these tests then set exactly
//! what they mean to test.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]
#![cfg(target_family = "unix")]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

const SEED_A: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const SEED_B: &str = "9911223344556677889900112233445566778899001122334455667788990011";

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// A repository with one committed file, signed with `seed_hex` through an explicit seed file.
fn repo_with_a_key(tag: &str, config_home: &Path, seed_hex: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    let key_dir = config_home.join("prikk");
    std::fs::create_dir_all(&key_dir).unwrap();
    write_seed(&key_dir.join("author.seed"), seed_hex, 0o600);
    write_seed(&key_dir.join("maintainer.seed"), seed_hex, 0o600);
    repo
}

fn write_seed(path: &Path, hex: &str, mode: u32) {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(mode)
        .open(path)
        .expect("create seed file");
    file.write_all(hex.as_bytes()).expect("write seed");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).expect("set mode");
}

fn unique_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("rfc148-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Control 2: `XDG_CONFIG_HOME` names the directory when set; `$HOME/.config/prikk` when it is not.
/// Both halves asserted, because asserting only the first would pass with the fallback broken.
#[test]
fn control2_xdg_config_home_then_home_dot_config() {
    for (label, use_xdg) in [("xdg", true), ("home", false)] {
        let config_home = unique_dir(label);
        // With XDG set, the key directory is `<xdg>/prikk`; without, `<home>/.config/prikk`.
        let (key_parent, home) = if use_xdg {
            (config_home.clone(), unique_dir("unused-home"))
        } else {
            (config_home.join(".config"), config_home.clone())
        };
        let repo = support::unique_repo(&format!("rfc148-{label}"));
        support::init(&repo);
        std::fs::create_dir_all(key_parent.join("prikk")).unwrap();
        write_seed(&key_parent.join("prikk/author.seed"), SEED_A, 0o600);
        std::fs::write(repo.join("f.txt"), b"hi").unwrap();

        let mut command = support::prikk(&repo);
        command.env("HOME", &home);
        if use_xdg {
            command.env("XDG_CONFIG_HOME", &config_home);
        } else {
            command.env_remove("XDG_CONFIG_HOME");
        }
        let out = command.args(["commit", "-m", "one"]).output().unwrap();
        support::ok(
            &out,
            &format!("{label}: commit using the {label} key directory"),
        );

        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// Control 3: `PRIKK_AUTHOR_SEED_FILE` wins over the default file — asserted by **which key
/// signed**, not by which command succeeded. Two different keys, one in each place; the recorded
/// key id would be the same either way, so the signature's own key material is the discriminator.
#[test]
fn control3_seed_file_override_wins_over_the_default() {
    let config_home = unique_dir("override");
    let repo = repo_with_a_key("rfc148-override", &config_home, SEED_A);
    let elsewhere = unique_dir("elsewhere");
    let override_path = elsewhere.join("other.seed");
    write_seed(&override_path, SEED_B, 0o600);

    // The public key each seed derives, straight from the binary.
    let public_of = |path: &Path| {
        let out = support::prikk(&repo)
            .env("XDG_CONFIG_HOME", &config_home)
            .args(["key", "public", "--seed-file", path.to_str().unwrap()])
            .output()
            .unwrap();
        support::ok(&out, "key public --seed-file");
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .find_map(|line| line.strip_prefix("public key: "))
            .expect("public key line")
            .trim()
            .to_string()
    };
    let default_public = public_of(&config_home.join("prikk/author.seed"));
    let override_public = public_of(&override_path);
    assert_ne!(
        default_public, override_public,
        "the fixture needs two genuinely different keys, or this proves nothing"
    );

    // With no override, `key public` reads the default file; with one, it reads the override.
    let default_read = support::prikk(&repo)
        .env("XDG_CONFIG_HOME", &config_home)
        .args(["key", "public"])
        .output()
        .unwrap();
    support::ok(&default_read, "key public with no override");
    assert!(
        String::from_utf8_lossy(&default_read.stdout).contains(&default_public),
        "with no override the default file is read"
    );

    let overridden = support::prikk(&repo)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("PRIKK_AUTHOR_SEED_FILE", &override_path)
        .args(["key", "public"])
        .output()
        .unwrap();
    support::ok(&overridden, "key public with the override set");
    assert!(
        String::from_utf8_lossy(&overridden.stdout).contains(&override_public),
        "PRIKK_AUTHOR_SEED_FILE must win over the default file"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6, first half: an override that is **set but missing** must refuse, never fall back to
/// the default file. Falling back would sign with a different key than the operator named — the
/// exact failure the retired environment channel is being removed to prevent.
#[test]
fn control6_a_missing_override_refuses_rather_than_falling_back() {
    let config_home = unique_dir("missing-override");
    let repo = repo_with_a_key("rfc148-missing-override", &config_home, SEED_A);
    std::fs::write(repo.join("f.txt"), b"hi").unwrap();

    let out = support::prikk(&repo)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("PRIKK_AUTHOR_SEED_FILE", config_home.join("nope.seed"))
        .args(["commit", "-m", "one"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "must refuse: {}", stderr(&out));
    assert!(
        stderr(&out).contains("nope.seed"),
        "the refusal names the file the operator asked for, not the default: {}",
        stderr(&out)
    );
    assert!(
        !stderr(&out).contains("author.seed"),
        "and must not mention the default file it did not use: {}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: a retired `PRIKK_AUTHOR_SEED` is **refused**, not ignored — and the old
/// "no signing key configured" wording is gone, so a reader searching for it lands on the migration
/// message instead of a dead end.
#[test]
fn control4_a_retired_seed_variable_is_refused() {
    let config_home = unique_dir("retired");
    let repo = repo_with_a_key("rfc148-retired", &config_home, SEED_A);
    std::fs::write(repo.join("f.txt"), b"hi").unwrap();

    let out = support::prikk(&repo)
        .env("XDG_CONFIG_HOME", &config_home)
        .env("PRIKK_AUTHOR_SEED", SEED_A)
        .args(["commit", "-m", "one"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "must refuse: {}", stderr(&out));
    assert!(
        stderr(&out)
            .starts_with("error: precondition not met: PRIKK_AUTHOR_SEED is no longer read"),
        "a retired variable is a caller-fixable precondition: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("PRIKK_AUTHOR_SEED_FILE"),
        "and the message names the replacement: {}",
        stderr(&out)
    );
    assert!(
        !stderr(&out).contains("no signing key configured"),
        "the old wording must be absent, not merely accompanied: {}",
        stderr(&out)
    );

    // **Refused even though a perfectly good seed file exists.** Silent ignore is the failure mode
    // this refusal exists to prevent, so the presence of a working key must not soften it.
    assert!(config_home.join("prikk/author.seed").exists());

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: a seed file group or others can read is refused, naming the mode.
#[test]
fn control5_a_group_readable_seed_file_is_refused() {
    let config_home = unique_dir("mode");
    let repo = repo_with_a_key("rfc148-mode", &config_home, SEED_A);
    write_seed(&config_home.join("prikk/author.seed"), SEED_A, 0o644);
    std::fs::write(repo.join("f.txt"), b"hi").unwrap();

    let out = support::prikk(&repo)
        .env("XDG_CONFIG_HOME", &config_home)
        .args(["commit", "-m", "one"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "must refuse: {}", stderr(&out));
    assert!(
        stderr(&out).contains("readable by group or other (mode 0644)"),
        "the refusal names the actual mode: {}",
        stderr(&out)
    );
    assert!(
        stderr(&out).contains("chmod 600"),
        "and the fix: {}",
        stderr(&out)
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 7: no `--seed-env` survives anywhere a user can see, and `--help` names `--seed-file`.
#[test]
fn control7_help_names_seed_file_and_no_seed_env_remains() {
    let repo = support::unique_repo("rfc148-help");
    support::init(&repo);
    let help = support::prikk(&repo).arg("--help").output().unwrap();
    support::ok(&help, "--help");
    let text = String::from_utf8_lossy(&help.stdout).into_owned();
    assert!(
        text.contains("prikk key public [--seed-file <path>]"),
        "help must advertise --seed-file: {text}"
    );
    assert!(
        !text.contains("--seed-env"),
        "no --seed-env may remain in help: {text}"
    );

    // The subcommand's own usage line too, which is a separate string.
    let usage = support::prikk(&repo).arg("key").output().unwrap();
    let usage_text = stderr(&usage);
    assert!(
        !usage_text.contains("--seed-env"),
        "no --seed-env in `prikk key`'s usage: {usage_text}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
