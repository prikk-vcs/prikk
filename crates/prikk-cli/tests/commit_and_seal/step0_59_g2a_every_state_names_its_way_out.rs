//! 0.59.0 step G2a, through the compiled binary: **every writer's refusal for a state that blocks publication names the command that clears
//! the state (measured: it is run, and the next writer proceeds), or says plainly that none exists.**
//!
//! The generic `repository mutation is blocked by incomplete ref publication; run verify/doctor and use signer-backed seal retry` named no
//! command. Each state of `.git-exclude/review-request/0.59.0-step-0-G-part-1-report.md` §1 is built the way `states.sh` builds it, every
//! writer that goes through a publication precondition runs against it (the five that used to bypass the one wrapper included), the sentence
//! is asserted, the command it names is run, and the next writer goes through.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use crate::support::ok;
use std::path::{Path, PathBuf};
use std::process::Output;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// A command with both signing identities, as the publishers need them (the author for a commit, the maintainer for a seal or a ref).
fn keyed(repo: &Path) -> std::process::Command {
    let mut command = support::prikk(repo);
    command
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        );
    command
}

/// As [`prikk`], without adopting the fixed maintainer key first (a test about trust must not undo its own removal).
fn plain(repo: &Path, args: &[&str]) -> Output {
    keyed(repo).args(args).output().unwrap()
}

fn prikk(repo: &Path, args: &[&str]) -> Output {
    support::trust_maintainer(repo);
    keyed(repo).args(args).output().unwrap()
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let (src, dst) = (entry.path(), to.join(entry.file_name()));
        if src.is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).unwrap();
        }
    }
}

const POINTER: &str = ".prikk/refs/containers/pointer-index-a.container";
const LOG: &str = ".prikk/refs/containers/log-a.container";

/// A repository with two sealed generations on `heads/main`; `.prikk` as it was after the first is returned beside it.
fn two_generations(tag: &str) -> (PathBuf, PathBuf) {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"one\n", "one");
    let g1 = PathBuf::from(format!("{}.g1", repo.display()));
    let _ = std::fs::remove_dir_all(&g1);
    copy_tree(&repo.join(".prikk"), &g1);
    support::generation(&repo, "heads/main", "b.txt", b"two\n", "two");
    (repo, g1)
}

fn flip_near_end(path: &Path) {
    let mut bytes = std::fs::read(path).unwrap();
    let at = bytes.len() - 5;
    bytes[at] = 0xff;
    std::fs::write(path, bytes).unwrap();
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Lead,
    DamagedPointer,
    RefStateMissing,
    SealStoppedAfterPublish,
    Debris,
    LogLeads,
}

fn build(state: State) -> PathBuf {
    let tag = format!("g2a-{state:?}");
    match state {
        State::SealStoppedAfterPublish => {
            let repo = support::unique_repo(&tag);
            support::init(&repo);
            std::fs::write(repo.join("a.txt"), b"one\n").unwrap();
            ok(&support::commit(&repo, "heads/main", "one"), "commit");
            let saved = PathBuf::from(format!("{}.active", repo.display()));
            let _ = std::fs::remove_dir_all(&saved);
            copy_tree(&repo.join(".prikk/active/default"), &saved);
            ok(&support::seal(&repo, "heads/main"), "seal");
            std::fs::remove_dir_all(repo.join(".prikk/active/default")).unwrap();
            copy_tree(&saved, &repo.join(".prikk/active/default"));
            repo
        }
        _ => {
            let (repo, g1) = two_generations(&tag);
            match state {
                State::Lead => {
                    std::fs::copy(g1.join("refs/containers/log-a.container"), repo.join(LOG))
                        .unwrap();
                }
                State::LogLeads => {
                    std::fs::copy(
                        g1.join("refs/containers/pointer-index-a.container"),
                        repo.join(POINTER),
                    )
                    .unwrap();
                }
                State::DamagedPointer => flip_near_end(&repo.join(POINTER)),
                State::RefStateMissing => {
                    for part in [
                        "containers/ref-state/a.container",
                        "containers/index.container",
                    ] {
                        std::fs::copy(g1.join(part), repo.join(".prikk").join(part)).unwrap();
                    }
                }
                State::Debris => {
                    std::fs::create_dir_all(repo.join(".prikk/refs/tmp")).unwrap();
                    std::fs::write(repo.join(".prikk/refs/tmp/candidate.tmp"), b"x").unwrap();
                }
                State::SealStoppedAfterPublish => unreachable!(),
            }
            repo
        }
    }
}

/// The writers that go through a publication precondition, by the label the matrix used. `commit` needs a changed file first.
fn writers() -> Vec<(&'static str, Vec<String>)> {
    let zeros = "0".repeat(64);
    let ones = "1".repeat(64);
    let s = |args: &[&str]| args.iter().map(|a| (*a).to_string()).collect::<Vec<_>>();
    vec![
        (
            "tag create",
            s(&["tag", "create", "tags/p", "--target", "heads/main"]),
        ),
        (
            "branch create",
            s(&["branch", "create", "heads/x", "--from", "heads/main"]),
        ),
        ("branch close", s(&["branch", "close", "heads/x"])),
        (
            "rollback-draft",
            s(&["rollback-draft", "--append-inverse", "-m", "r"]),
        ),
        ("uncommit", vec!["uncommit".to_string(), zeros]),
        (
            "trust maintainer add",
            vec![
                "trust".into(),
                "maintainer".into(),
                "add".into(),
                "--key-id".into(),
                "k9".into(),
                "--public-key".into(),
                ones,
            ],
        ),
        (
            "trust maintainer remove",
            s(&["trust", "maintainer", "remove", "--key-id", "nope"]),
        ),
        (
            "doctor --discard-damaged-commits",
            s(&["doctor", "--discard-damaged-commits", "--plan-only"]),
        ),
        (
            "doctor --repair-wal-tail",
            s(&["doctor", "--repair-wal-tail"]),
        ),
        (
            "doctor --restore-queue-target",
            s(&[
                "doctor",
                "--restore-queue-target",
                "--ref",
                "heads/main",
                "--plan-only",
            ]),
        ),
    ]
}

const OLD_GENERIC: &str = "use signer-backed seal retry";

/// What every writer that used to print the generic now says, per state: a phrase that names the command (or says none exists).
fn names(state: State) -> &'static [&'static str] {
    match state {
        State::Lead => &["run `prikk ref complete heads/main`"],
        State::DamagedPointer => &[
            "`prikk doctor --rebuild-pointer-index --plan-only`, then `prikk doctor --rebuild-pointer-index`",
        ],
        State::RefStateMissing => &[
            "no command repairs a missing RefState in place: restore a sound copy of the sealed history",
        ],
        State::SealStoppedAfterPublish => &["run `prikk seal --allow-no-audit`"],
        State::LogLeads => &["run `prikk verify`, which names the way out of this state"],
        State::Debris => &[OLD_GENERIC],
    }
}

/// Which writers printed the generic text in each state before this part (G1 §1), so the assertion is on exactly those.
fn used_to_print_the_generic(state: State, label: &str) -> bool {
    let always = [
        "trust maintainer add",
        "trust maintainer remove",
        "doctor --discard-damaged-commits",
        "doctor --restore-queue-target",
        "doctor --repair-wal-tail",
        "rollback-draft",
    ];
    match state {
        State::Lead | State::SealStoppedAfterPublish => always.contains(&label),
        State::DamagedPointer | State::RefStateMissing => {
            ["branch create", "branch close", "uncommit"].contains(&label)
                || always.contains(&label) && label != "rollback-draft"
        }
        State::LogLeads | State::Debris => true,
    }
}

#[test]
fn every_writer_that_printed_the_generic_names_its_states_way_out() {
    for state in [
        State::Lead,
        State::DamagedPointer,
        State::RefStateMissing,
        State::SealStoppedAfterPublish,
        State::Debris,
        State::LogLeads,
    ] {
        let repo = build(state);
        for (label, args) in writers() {
            if !used_to_print_the_generic(state, label) {
                continue;
            }
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            let out = prikk(&repo, &args);
            let said = text(&out);
            assert_eq!(out.status.code(), Some(1), "{state:?}/{label}: {said}");
            if state != State::Debris {
                assert!(
                    !said.contains(OLD_GENERIC),
                    "{state:?}/{label} still prints the generic: {said}"
                );
            }
            assert!(
                names(state).iter().any(|phrase| said.contains(phrase)),
                "{state:?}/{label}: expected one of {:?} in: {said}",
                names(state)
            );
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// **The command each sentence names is run, and then the next writer goes through** (the lead and the damaged pointer index and the
/// seal that stopped after it published: the three states a named command clears).
#[test]
fn the_command_a_sentence_names_clears_the_state_and_the_next_writer_goes_through() {
    let cases: [(State, &[&[&str]]); 3] = [
        (State::Lead, &[&["ref", "complete", "heads/main"]]),
        (
            State::DamagedPointer,
            &[
                &["doctor", "--rebuild-pointer-index", "--plan-only"],
                &["doctor", "--rebuild-pointer-index"],
            ],
        ),
        (
            State::SealStoppedAfterPublish,
            &[&["seal", "--allow-no-audit"]],
        ),
    ];
    for (state, commands) in cases {
        let repo = build(state);
        for command in commands {
            ok(&prikk(&repo, command), &format!("{state:?}: {command:?}"));
        }
        ok(
            &prikk(
                &repo,
                &["tag", "create", "tags/ok", "--target", "heads/main"],
            ),
            &format!("{state:?}: the next writer"),
        );
        ok(&prikk(&repo, &["verify"]), &format!("{state:?}: verify"));
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// **The last arm holds only if it is true:** a state it names must be one `verify` reads and prints a way out for. The log leading the
/// pointer reaches it today (the rebuild's acceptance of a fork is the architect's ruling, so that arm is held), and `verify` names a way out.
#[test]
fn the_last_arm_is_true_verify_names_a_way_out_of_every_state_that_reaches_it() {
    let repo = build(State::LogLeads);
    let refused = prikk(
        &repo,
        &["tag", "create", "tags/p", "--target", "heads/main"],
    );
    assert!(
        text(&refused).contains("run `prikk verify`, which names the way out of this state"),
        "{}",
        text(&refused)
    );
    let verify = text(&prikk(&repo, &["verify"]));
    assert!(
        verify.lines().any(|line| line.starts_with("way out [")),
        "verify names a way out: {verify}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The two owner-mismatch refusals name the commands that work from there (measured in `mismatch.sh`/`mismatch2.sh`): `uncommit <newest id>`
/// first (then the commit goes through), `seal --ref <owner>` second; `seal --ref <other>` names the seal of the owner.
#[test]
fn the_owner_mismatch_refusals_name_commands_that_work() {
    let repo = support::unique_repo("g2a-mismatch");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"base\n", "base");
    ok(
        &support::branch_create(&repo, "heads/other", "heads/main"),
        "branch create other",
    );
    std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
    ok(
        &support::commit(&repo, "heads/other", "queued"),
        "commit --ref heads/other",
    );

    // `seal --ref heads/main` while the queue is heads/other's.
    let seal = prikk(&repo, &["seal", "--allow-no-audit", "--ref", "heads/main"]);
    assert!(
        text(&seal).contains("active WAL is owned by heads/other; requested seal ref is heads/main: `prikk seal --allow-no-audit --ref heads/other` seals that queue onto heads/other"),
        "{}",
        text(&seal)
    );

    // `commit` onto heads/main.
    std::fs::write(repo.join("a.txt"), b"base\nmore\n").unwrap();
    let commit = support::commit(&repo, "heads/main", "next");
    let message = text(&commit);
    assert_eq!(commit.status.code(), Some(1), "{message}");
    let after = message
        .split("`prikk uncommit ")
        .nth(1)
        .unwrap_or_else(|| panic!("names uncommit: {message}"));
    let id: String = after.chars().take_while(char::is_ascii_hexdigit).collect();
    assert_eq!(id.len(), 64, "{message}");
    assert!(
        message.contains("active WAL is owned by heads/other; requested ref heads/main: `prikk uncommit ")
            && message.contains("takes the newest commit back and leaves your files (the next commit onto heads/main then authors them), or `prikk seal --allow-no-audit --ref heads/other` publishes them to heads/other"),
        "{message}"
    );
    // The first arm goes straight through.
    ok(&prikk(&repo, &["uncommit", &id]), "the named uncommit");
    ok(
        &support::commit(&repo, "heads/main", "next"),
        "the commit after it",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The key-removal wordings (G1 §4), measured on 0.58.0 and now said: removing the key that sealed the history makes `verify` fail for all of
/// it; the way-out heading is about an *unadopted key*, not a signature that did not verify; the success line says what a revocation does,
/// without a count; re-adding the key with its original public key restores everything.
#[test]
fn removing_the_sealing_key_says_what_it_does_and_re_adding_it_restores_the_history() {
    let repo = support::unique_repo("g2a-key-removal");
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"one\n", "one");
    support::generation(&repo, "heads/main", "b.txt", b"two\n", "two");
    // A second key, so the first can be removed (the last cannot). Made by `prikk setup` in a second repository, whose config home is its own
    // (`isolated_config_home` is keyed by repository): `prikk key generate --out <file>` writes a seed file at inherited permissions, which
    // prikk refuses on Windows by design, and this test must run there.
    let other = support::unique_repo("g2a-key-removal-second");
    ok(
        &support::prikk(&other)
            .args(["setup"])
            .arg(&other)
            .output()
            .unwrap(),
        "setup in a second repository",
    );
    let status = text(
        &support::prikk(&other)
            .args(["key", "status", "--role", "maintainer"])
            .output()
            .unwrap(),
    );
    let second_public: String = status
        .lines()
        .find_map(|line| line.strip_prefix("public key: "))
        .and_then(|rest| rest.split_whitespace().next())
        .expect("key status prints the public key")
        .to_string();
    let _ = std::fs::remove_dir_all(&other);
    ok(
        &plain(
            &repo,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "second",
                "--public-key",
                &second_public,
            ],
        ),
        "adopt a second key",
    );
    ok(&plain(&repo, &["verify"]), "verify before the removal");

    let removed = prikk(
        &repo,
        &[
            "trust",
            "maintainer",
            "remove",
            "--key-id",
            support::MAINTAINER_KEY_ID,
        ],
    );
    ok(&removed, "remove the sealing key");
    let said = text(&removed);
    assert!(
        said.starts_with(&format!("revoked maintainer key: {}\n", support::MAINTAINER_KEY_ID))
            && said.contains("everything it signed stops verifying (`prikk verify` lists it); `prikk trust maintainer add --key-id")
            && said.contains("with its original public key restores it"),
        "{said}"
    );
    assert!(
        !said.chars().any(|c| c.is_ascii_digit()) || said.contains(support::MAINTAINER_KEY_ID),
        "no count is claimed: {said}"
    );

    assert_eq!(plain(&repo, &["verify"]).status.code(), Some(1));
    let doctor = text(&plain(&repo, &["doctor"]));
    assert!(
        doctor.contains("way out [a publication's signing key is not adopted]: adopt the key that signed it with `prikk trust maintainer add --key-id <id> --public-key <hex>`")
            && doctor.contains("adding it back with its original public key restores it")
            && !doctor.contains("a publication signature did not verify"),
        "{doctor}"
    );
    ok(
        &plain(
            &repo,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                support::MAINTAINER_KEY_ID,
                "--public-key",
                &support::maintainer_public_key_hex(),
            ],
        ),
        "re-add the original key",
    );
    ok(&plain(&repo, &["verify"]), "verify after the re-add");
    let help = text(&plain(&repo, &["help", "trust"]));
    assert!(
        help.contains("prikk trust maintainer remove --key-id ID Revoke one MAINTAINER key: everything it signed stops verifying until it is added back with its original public key"),
        "{help}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The `prikk …` command a sentence names between backticks, starting at `prefix`, as an argument list.
fn named(message: &str, prefix: &str) -> Vec<String> {
    let start = message
        .find(&format!("`{prefix}"))
        .unwrap_or_else(|| panic!("no `{prefix}…` in {message}"))
        + 1;
    let end = message[start..].find('`').unwrap() + start;
    message[start..end]
        .split_whitespace()
        .skip(1)
        .map(str::to_string)
        .collect()
}

/// **The stale-record circle (G2b):** after `commit --ref heads/other` and a `seal`, the record names `heads/other`; a commit of the user's own edit
/// onto `heads/main` refuses naming the switch, the switch refuses as dirty and says "commit it", which is what refused. Both sentences now name
/// the two ways that work, and **both are run from the sentence**: restore the recorded file with `prikk cat`, switch, put the change back,
/// commit (exit 0, clean); or commit the change where the worktree was last written, seal that, switch.
#[test]
fn the_stale_record_circle_names_two_ways_out_and_each_works() {
    fn world(tag: &str) -> PathBuf {
        let repo = support::unique_repo(tag);
        support::init(&repo);
        support::generation(&repo, "heads/main", "a.txt", b"base\n", "base");
        ok(
            &support::branch_create(&repo, "heads/other", "heads/main"),
            "branch create other",
        );
        std::fs::write(repo.join("queued.txt"), b"q\n").unwrap();
        ok(
            &support::commit(&repo, "heads/other", "q"),
            "commit --ref heads/other",
        );
        ok(
            &support::seal(&repo, "heads/other"),
            "seal --ref heads/other",
        );
        std::fs::write(repo.join("a.txt"), b"base\nmy edit\n").unwrap();
        repo
    }

    // Way B, from the commit's refusal: keep a copy, restore with `cat`, switch, put the change back, commit.
    let repo = world("g2a-circle-b");
    let commit = text(&prikk(&repo, &["commit", "-m", "mine"]));
    assert!(
        commit.contains(
            "keep a copy of your change, restore the file with `prikk cat --path a.txt --ref "
        ) && commit.contains(
            "`prikk commit --ref heads/other`, then `prikk seal --allow-no-audit --ref heads/other`"
        ),
        "{commit}"
    );
    let switch = text(&prikk(&repo, &["branch", "switch", "heads/main"]));
    assert!(
        switch.contains("commit it, or restore those files, before switching")
            && switch.contains(
                "keep a copy of your change, restore the file with `prikk cat --path a.txt --ref "
            ),
        "{switch}"
    );
    std::fs::copy(repo.join("a.txt"), repo.join("a.mine")).unwrap();
    let restore = named(&switch, "prikk cat ");
    let restore: Vec<&str> = restore.iter().map(String::as_str).collect();
    ok(&prikk(&repo, &restore), "the cat the sentence names");
    assert_eq!(std::fs::read(repo.join("a.txt")).unwrap(), b"base\n");
    ok(
        &prikk(&repo, &["branch", "switch", "heads/main"]),
        "the switch after the restore",
    );
    std::fs::copy(repo.join("a.mine"), repo.join("a.txt")).unwrap();
    std::fs::remove_file(repo.join("a.mine")).unwrap();
    ok(
        &prikk(&repo, &["commit", "-m", "mine"]),
        "the commit after it",
    );
    let _ = std::fs::remove_dir_all(&repo);

    // Way A, from the switch's refusal: commit it where the worktree was last written, seal that, switch.
    let repo = world("g2a-circle-a");
    let switch = text(&prikk(&repo, &["branch", "switch", "heads/main"]));
    let commit_there = named(&switch, "prikk commit --ref heads/other");
    let commit_there: Vec<&str> = commit_there
        .iter()
        .map(String::as_str)
        .chain(["-m", "mine"])
        .collect();
    ok(
        &prikk(&repo, &commit_there),
        "the commit the sentence names",
    );
    let seal_there = named(&switch, "prikk seal ");
    let seal_there: Vec<&str> = seal_there.iter().map(String::as_str).collect();
    ok(&prikk(&repo, &seal_there), "the seal the sentence names");
    ok(
        &prikk(&repo, &["branch", "switch", "heads/main"]),
        "the switch after both",
    );
    assert_eq!(
        std::fs::read(repo.join("a.txt")).unwrap(),
        b"base\n",
        "the change now lives on heads/other"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
