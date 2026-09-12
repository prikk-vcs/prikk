//! RFC 150: `prikk key status` — can I sign here, and with which key?
//!
//! **The controls that matter are the agreement ones.** A status command that answers from its own
//! copy of the rules is worse than no status command: it would tell a front-end "usable" where
//! `commit` refuses. So `commit_and_key_status_agree_on_every_state` drives both against one fixture
//! per state, and control 6's perturbation breaks the shared query to prove they are actually
//! sharing it.
//!
//! **Only the mode controls are Unix-gated, not the file.** Gating the whole file was the first
//! shape, and it would have given Windows CI zero coverage of a command whose entire job is to
//! report what the key path resolves to — the same blindness that took `main` red once already when
//! the isolation seam missed `%APPDATA%` and thirteen green host gates said nothing. The permission
//! rule genuinely does not exist on Windows (the key directory relies on `%APPDATA%`'s per-user
//! ACL); presence, override resolution, decoding and binding all do.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use support::json;

/// One key-directory state to drive both commands against: a label, and what to do to the fixture's
/// key directory to produce it.
type KeyState = (&'static str, fn(&Path));

const SEED_A: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
const SEED_B: &str = "9911223344556677889900112233445566778899001122334455667788990011";

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn unique_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!("rfc150-{tag}-{}", support::unique_suffix()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Write a seed file. `mode` is applied on Unix and ignored elsewhere, where the platform has no
/// such rule and `key_material` does not check one.
fn write_seed(path: &Path, hex: &str, mode: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(path, hex.as_bytes()).expect("write seed");
    set_mode(path, mode);
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) {}

/// A repository with both seeds in an isolated key directory.
fn fixture(tag: &str) -> (PathBuf, PathBuf) {
    let config_home = unique_dir(tag);
    let repo = support::unique_repo(&format!("rfc150-{tag}"));
    support::init(&repo);
    write_seed(&config_home.join("prikk/author.seed"), SEED_A, 0o600);
    write_seed(&config_home.join("prikk/maintainer.seed"), SEED_A, 0o600);
    (repo, config_home)
}

/// Point a command at this fixture's key directory — **all three variables `default_key_dir` reads
/// on any platform**, as the shared seam does. Setting only `XDG_CONFIG_HOME` would make every
/// assertion below vacuous on Windows: the command would find no key whatever the fixture wrote.
fn with_key_home(command: &mut Command, config_home: &Path) {
    command
        .env("XDG_CONFIG_HOME", config_home)
        .env("HOME", config_home)
        .env("APPDATA", config_home);
}

fn status(repo: &Path, config_home: &Path, args: &[&str]) -> Output {
    let mut command = support::prikk(repo);
    with_key_home(&mut command, config_home);
    command.args(["key", "status"]);
    command.args(args);
    command.output().unwrap()
}

fn role_json<'a>(report: &'a json::Value, role: &str) -> &'a json::Value {
    report
        .get("roles")
        .as_array()
        .iter()
        .find(|entry| entry.get("role").as_str() == role)
        .unwrap_or_else(|| panic!("no {role} entry in {report:?}"))
}

/// Control 1: the ready state — **exit 0 with an answer**, which is the whole point. A not-ready
/// answer exits 0 too (controls 2, 3 and 6); a non-zero exit would make "no key here" indistinguishable
/// from a broken repository.
///
/// The second half of this control used to assert that a stale `PRIKK_AUTHOR_SEED` was *reported* as
/// `legacy_variable_set`. RFC 148 rule 1's window closed in 0.41.0 and the field went with the
/// detection, before `key-status-v1` was ever published. The replacement assertion — a set retired
/// variable is unread and `key status` names the key that actually signs — lives in
/// `rfc148_key_directory.rs::control4_a_retired_seed_variable_is_unread`, beside the other key
/// discovery controls.
#[test]
fn control1_the_ready_state_answers_at_exit_zero() {
    let (repo, config_home) = fixture("ready");

    let out = status(&repo, &config_home, &["--format", "json"]);
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let report = json::parse(&stdout(&out));
    assert_eq!(
        report.get("schema_version").as_str(),
        "key-status-v1",
        "the schema version opens the document"
    );
    let author = role_json(&report, "author");
    assert!(author.get("usable").as_bool());
    assert_eq!(author.get("source").as_str(), "key-directory");
    assert_eq!(author.get("key_id").as_str(), "author");
    assert_eq!(author.get("key_id_source").as_str(), "default");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2: an override that is set and missing names **the override**, and does not name the
/// default file it did not use.
#[test]
fn control2_a_missing_override_is_reported_as_such() {
    let (repo, config_home) = fixture("override-missing");
    let missing = config_home.join("nope.seed");

    let mut command = support::prikk(&repo);
    with_key_home(&mut command, &config_home);
    let out = command
        .env("PRIKK_AUTHOR_SEED_FILE", &missing)
        .args(["key", "status", "--role", "author", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let report = json::parse(&stdout(&out));
    let author = role_json(&report, "author");
    assert_eq!(author.get("source").as_str(), "seed-file-override");
    assert!(!author.get("usable").as_bool());
    assert_eq!(author.get("reason").as_str(), "override-missing");
    assert_eq!(author.get("path").as_str(), missing.display().to_string());
    assert!(
        !author.get("path").as_str().contains("author.seed"),
        "the default file must not be named: {author:?}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: a group-readable seed is not usable, and the reason names the mode. Unix only — the
/// rule it asserts does not exist on Windows.
#[cfg(unix)]
#[test]
fn control3_a_group_readable_seed_names_its_mode() {
    let (repo, config_home) = fixture("mode");
    write_seed(&config_home.join("prikk/author.seed"), SEED_A, 0o644);

    let out = status(
        &repo,
        &config_home,
        &["--role", "author", "--format", "json"],
    );
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let report = json::parse(&stdout(&out));
    let author = role_json(&report, "author");
    assert!(!author.get("usable").as_bool());
    assert_eq!(
        author.get("reason").as_str(),
        "readable-by-others (mode 0644)"
    );

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: every binding state, **and the signing command's own behaviour in the same state,
/// from one fixture**. A binding a front-end displays is only worth displaying if it predicts what
/// `commit`/`seal` will do.
#[test]
fn control4_binding_states_match_what_commit_and_seal_do() {
    let (repo, config_home) = fixture("binding");

    let author_binding = |extra: &[(&str, &str)]| -> String {
        let mut command = support::prikk(&repo);
        with_key_home(&mut command, &config_home);
        for (name, value) in extra {
            command.env(name, value);
        }
        let out = command
            .args(["key", "status", "--role", "author", "--format", "json"])
            .output()
            .unwrap();
        role_json(&json::parse(&stdout(&out)), "author")
            .get("binding")
            .as_str()
            .to_string()
    };

    // Fresh id, never used here.
    assert_eq!(author_binding(&[]), "unrecorded");

    // After one commit, the id is recorded and matches.
    std::fs::write(repo.join("a.txt"), b"hello").unwrap();
    let mut command = support::prikk(&repo);
    with_key_home(&mut command, &config_home);
    support::ok(
        &command.args(["commit", "-m", "one"]).output().unwrap(),
        "commit",
    );
    assert_eq!(author_binding(&[]), "matches");

    // A different seed under that same id: `mismatch`, and `commit` refuses -- both asserted here,
    // because the status is a prediction and this is the assertion that it is a true one.
    let other = unique_dir("binding-other").join("other.seed");
    write_seed(&other, SEED_B, 0o600);
    let override_env: &[(&str, &str)] = &[("PRIKK_AUTHOR_SEED_FILE", other.to_str().unwrap())];
    assert_eq!(author_binding(override_env), "mismatch");

    std::fs::write(repo.join("b.txt"), b"second").unwrap();
    let mut command = support::prikk(&repo);
    with_key_home(&mut command, &config_home);
    let refused = command
        .env("PRIKK_AUTHOR_SEED_FILE", &other)
        .args(["commit", "-m", "two"])
        .output()
        .unwrap();
    assert_eq!(
        refused.status.code(),
        Some(1),
        "commit must refuse where the binding says mismatch: {}",
        stdout(&refused)
    );
    assert!(
        stderr(&refused).contains("already has a different recorded public key"),
        "{}",
        stderr(&refused)
    );

    // Maintainer: adopted by `init`? No -- this fixture used `init`, not `setup`, so the maintainer
    // key is not adopted, and `seal` refuses for exactly that reason.
    let out = status(
        &repo,
        &config_home,
        &["--role", "maintainer", "--format", "json"],
    );
    let report = json::parse(&stdout(&out));
    let maintainer = role_json(&report, "maintainer");
    assert_eq!(maintainer.get("binding").as_str(), "not-adopted");

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: JSON succeeds wherever prose does, and the two agree on `usable` per role — asserted
/// from one repository state, not from two runs that might have drifted apart.
#[test]
fn control5_prose_and_json_agree_on_usable() {
    let (repo, config_home) = fixture("agree");
    // Undecodable rather than group-readable: the disagreement this control looks for is in the
    // *rendering*, so the state that produces it should be one every platform has.
    write_seed(
        &config_home.join("prikk/maintainer.seed"),
        "not-a-seed",
        0o600,
    );

    let prose = status(&repo, &config_home, &[]);
    let as_json = status(&repo, &config_home, &["--format", "json"]);
    assert_eq!(prose.status.code(), Some(0), "{}", stderr(&prose));
    assert_eq!(as_json.status.code(), Some(0), "{}", stderr(&as_json));

    let report = json::parse(&stdout(&as_json));
    let text = stdout(&prose);
    for role in ["author", "maintainer"] {
        let usable = role_json(&report, role).get("usable").as_bool();
        let block = text
            .split("role: ")
            .find(|block| block.starts_with(role))
            .unwrap_or_else(|| panic!("no prose block for {role}: {text}"));
        assert!(
            block.contains(&format!("usable: {usable}")),
            "{role}: prose and JSON disagree on usable\nprose: {block}\njson: {usable}"
        );
    }
    // The fixture is only meaningful if the two roles differ -- otherwise the agreement is trivial.
    assert!(role_json(&report, "author").get("usable").as_bool());
    assert!(!role_json(&report, "maintainer").get("usable").as_bool());

    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6: **`key status` and the signing path answer from one computation.**
///
/// Every state below is driven through both commands against one fixture: whenever `usable` is
/// false, `commit` must refuse, and whenever it is true, `commit` must proceed. Perturbing the
/// shared query in `key_material::status` fails this test — which is the only way to know the two
/// are sharing it rather than agreeing by coincidence.
#[test]
fn control6_commit_and_key_status_agree_on_every_state() {
    // `mut` only because of the Unix-only push below -- without this the Windows cross-target
    // clippy run fails on `unused_mut`, which is the addendum earning its place again.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut states: Vec<KeyState> = vec![
        ("usable", |_: &Path| {}),
        ("missing", |home: &Path| {
            std::fs::remove_file(home.join("prikk/author.seed")).unwrap();
        }),
        ("undecodable", |home: &Path| {
            write_seed(&home.join("prikk/author.seed"), "not-a-seed", 0o600);
        }),
    ];
    // The one state that exists only where the permission rule does.
    #[cfg(unix)]
    states.push(("group-readable", |home: &Path| {
        write_seed(&home.join("prikk/author.seed"), SEED_A, 0o644);
    }));

    for (label, prepare) in states {
        let (repo, config_home) = fixture(&format!("agree-{label}"));
        prepare(&config_home);

        let out = status(
            &repo,
            &config_home,
            &["--role", "author", "--format", "json"],
        );
        assert_eq!(out.status.code(), Some(0), "{label}: {}", stderr(&out));
        let usable = role_json(&json::parse(&stdout(&out)), "author")
            .get("usable")
            .as_bool();

        std::fs::write(repo.join("f.txt"), b"x").unwrap();
        let mut command = support::prikk(&repo);
        with_key_home(&mut command, &config_home);
        let commit = command.args(["commit", "-m", label]).output().unwrap();

        assert_eq!(
            commit.status.success(),
            usable,
            "{label}: key status says usable={usable} but commit {} -- the two are not sharing one \
             computation\ncommit stderr: {}",
            if commit.status.success() {
                "succeeded"
            } else {
                "refused"
            },
            stderr(&commit)
        );

        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// Control 7 (RFC 148 rule 1, moved here for the platform): a retired `PRIKK_AUTHOR_SEED` is **simply
/// unread**, and `key status` names the key that actually signs.
///
/// 0.40.0 refused while the variable was set, for exactly one release, so that no automation could
/// silently start signing with a different key than it thought. That release shipped; a stale export
/// is now an unused variable like any other name prikk knows nothing about. **What replaced the
/// refusal is this command** — so the control asserts both halves at once: `commit` succeeds with the
/// variable set, and `key status` reports the key-directory key.
///
/// **The variable and the directory hold different seeds on purpose.** If the retired channel were
/// read anywhere, by any path, the reported public key would be the other one and this names it. Two
/// copies of the same seed would pass whichever channel won.
///
/// The expected key is derived from the directory file through `key public --seed-file` rather than
/// written down, so the assertion is "the key in the directory is the one in effect", not "this hex
/// string is".
///
/// It lives in this file rather than beside RFC 148's other key-discovery controls because that file
/// is Unix-gated wholesale and this claim is not platform-specific — a variable being unread on
/// Windows is exactly as much of a claim, and untested there is where this repository has been burned.
#[test]
fn control7_a_retired_seed_variable_is_unread() {
    let (repo, config_home) = fixture("retired");
    std::fs::write(repo.join("f.txt"), b"hi").unwrap();

    let mut command = support::prikk(&repo);
    with_key_home(&mut command, &config_home);
    let out = command
        .env("PRIKK_AUTHOR_SEED", SEED_B)
        .args(["commit", "-m", "one"])
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "a retired variable is no longer a refusal: {}",
        stderr(&out)
    );

    let mut command = support::prikk(&repo);
    with_key_home(&mut command, &config_home);
    let from_file = command
        .args([
            "key",
            "public",
            "--seed-file",
            config_home.join("prikk/author.seed").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert_eq!(from_file.status.code(), Some(0), "{}", stderr(&from_file));
    let expected = stdout(&from_file)
        .trim()
        .strip_prefix("public key: ")
        .expect("`key public` prints `public key: <hex>`")
        .to_string();
    assert_eq!(expected.len(), 64, "a public key is 64 hex characters");

    let mut command = support::prikk(&repo);
    with_key_home(&mut command, &config_home);
    let reported = command
        .env("PRIKK_AUTHOR_SEED", SEED_B)
        .args(["key", "status", "--role", "author", "--format", "json"])
        .output()
        .unwrap();
    assert_eq!(reported.status.code(), Some(0), "{}", stderr(&reported));
    let author = json::parse(&stdout(&reported));
    let author = role_json(&author, "author");
    assert_eq!(
        author.get("public_key").as_str(),
        expected,
        "`key status` must name the key-directory key, not the retired variable's"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
