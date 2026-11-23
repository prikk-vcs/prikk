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
    LogLeads,
    /// The pointer is two transitions ahead of the log (G2c: the rebuild refuses it; the last arm names `verify`).
    TwoAhead,
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
                State::TwoAhead => {
                    support::generation(&repo, "heads/main", "c.txt", b"three\n", "three");
                    std::fs::copy(g1.join("refs/containers/log-a.container"), repo.join(LOG))
                        .unwrap();
                }
                State::RefStateMissing => {
                    for part in [
                        "containers/ref-state/a.container",
                        "containers/index.container",
                    ] {
                        std::fs::copy(g1.join(part), repo.join(".prikk").join(part)).unwrap();
                    }
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
        State::LogLeads => &[
            "the ref log is ahead of the pointer index for heads/main; run `prikk doctor --rebuild-pointer-index --plan-only`, then `prikk doctor --rebuild-pointer-index`",
        ],
        State::TwoAhead => &["run `prikk verify`, which names the way out of this state"],
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
        State::LogLeads | State::TwoAhead => true,
    }
}

#[test]
fn every_writer_that_printed_the_generic_names_its_states_way_out() {
    for state in [
        State::Lead,
        State::DamagedPointer,
        State::RefStateMissing,
        State::SealStoppedAfterPublish,
        State::LogLeads,
        State::TwoAhead,
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
            assert!(
                !said.contains(OLD_GENERIC),
                "{state:?}/{label} still prints the generic: {said}"
            );
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
    let cases: [(State, &[&[&str]]); 4] = [
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
        (
            State::LogLeads,
            &[
                &["doctor", "--rebuild-pointer-index", "--plan-only"],
                &["doctor", "--rebuild-pointer-index"],
            ],
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
/// pointer more than one transition ahead reaches it (the rebuild refuses that shape), and `verify` names a way out.
#[test]
fn the_last_arm_is_true_verify_names_a_way_out_of_every_state_that_reaches_it() {
    let repo = build(State::TwoAhead);
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
    // G2c: for this shape the way out is not the rebuild: the rebuild refuses a pointer more than one transition ahead, so the sound copy is named.
    assert!(verify.contains("prikk setup <new-dir>"), "{verify}");
    let rebuild = prikk(&repo, &["doctor", "--rebuild-pointer-index", "--plan-only"]);
    assert_eq!(rebuild.status.code(), Some(1), "{}", text(&rebuild));
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

// ---- G2c -----------------------------------------------------------------------------------------------------------------------------

/// **The fork (G2c item 1).** `heads/main` sealed once, then two different second generations, X and Y, in two copies; X's repository takes Y's objects
/// (the archive of Y imported without `--adopt`), its ref log names X and its pointer index names Y (both at update_seq 2, both signed, both
/// chaining to the same first record). The published 0.58.0's rebuild called Y "stale, behind the log" and dropped it; it now refuses, in the plan
/// and in the run, naming the sound copy, and the pointer index is not touched. The log-leads shapes stay accepted (the other tests of this file).
#[test]
fn the_rebuild_refuses_a_fork_and_leaves_the_pointer_index_as_it_was() {
    let base = support::unique_repo("g2c-fork");
    support::init(&base);
    support::generation(&base, "heads/main", "a.txt", b"one\n", "one");
    let x = PathBuf::from(format!("{}.x", base.display()));
    let y = PathBuf::from(format!("{}.y", base.display()));
    for copy in [&x, &y] {
        let _ = std::fs::remove_dir_all(copy);
        copy_tree(&base, copy);
    }
    support::generation(&x, "heads/main", "b.txt", b"two\n", "two on X");
    support::generation(&y, "heads/main", "c.txt", b"three\n", "three on Y");
    let archive = PathBuf::from(format!("{}.prepo", base.display()));
    let _ = std::fs::remove_file(&archive);
    ok(
        &prikk(&y, &["archive", "export", archive.to_str().unwrap()]),
        "archive export of Y",
    );
    ok(
        &prikk(&x, &["archive", "import", archive.to_str().unwrap()]),
        "archive import into X",
    );
    std::fs::copy(y.join(POINTER), x.join(POINTER)).unwrap();

    assert_eq!(
        prikk(&x, &["verify"]).status.code(),
        Some(1),
        "the fork fails verify"
    );
    let before = std::fs::read(x.join(POINTER)).unwrap();
    for args in [
        &["doctor", "--rebuild-pointer-index", "--plan-only"][..],
        &["doctor", "--rebuild-pointer-index"][..],
    ] {
        let out = prikk(&x, args);
        let said = text(&out);
        assert_eq!(out.status.code(), Some(1), "{args:?}: {said}");
        assert!(
            said.contains("have a pointer that names a RefState the ref log does not hold")
                && said.contains("no command resolves a fork in place")
                && said.contains("prikk setup <new-dir>"),
            "{args:?}: {said}"
        );
    }
    assert_eq!(
        std::fs::read(x.join(POINTER)).unwrap(),
        before,
        "the pointer index is as it was"
    );
    // The way out `verify` prints for the failed ref says the same: no command, then the sound copy.
    let verify = text(&prikk(&x, &["verify"]));
    assert!(verify.contains("prikk setup <new-dir>"), "{verify}");
    for dir in [&base, &x, &y] {
        let _ = std::fs::remove_dir_all(dir);
    }
    let _ = std::fs::remove_file(&archive);
}

/// **A RefState the repository does not hold, at every reader (G2c item 3):** one sentence from one constructor, the same words wherever it is
/// printed, naming that no command repairs it in place and the sound copy; and `verify`'s way out for the state says none exists, not the rebuild
/// (which does not restore an object that is in no container).
#[test]
fn a_missing_ref_state_says_the_same_thing_wherever_it_is_read() {
    let repo = build(State::RefStateMissing);
    let phrase = "which this repository does not hold; no command repairs a missing RefState in place: restore a sound copy of the sealed history";
    for args in [
        &["log", "--ref", "heads/main"][..],
        &["branch", "list"][..],
        &["tag", "create", "tags/p", "--target", "heads/main"][..],
        &["branch", "create", "heads/x", "--from", "heads/main"][..],
        &["commit", "-m", "x"][..],
        &["seal", "--allow-no-audit"][..],
    ] {
        // `commit` and `seal` need something queued or changed to get as far as reading the ref.
        std::fs::write(repo.join("w.txt"), b"w\n").unwrap();
        let said = text(&prikk(&repo, args));
        assert!(said.contains(phrase), "{args:?}: {said}");
        assert!(
            !said.contains(" is missing") || said.contains(phrase),
            "{args:?}: {said}"
        );
    }
    let verify = text(&prikk(&repo, &["verify"]));
    assert!(
        verify.contains("way out [a ref's pointer names a RefState this repository does not hold]: no command repairs a missing RefState in place"),
        "{verify}"
    );
    assert!(
        !verify.contains("way out [a ref file or ref failed]"),
        "the rebuild's way out is not offered for a state it does not clear: {verify}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// A path as the shell would split it: `'…'` (with `'\''` for a quote) and `"…"`, else whitespace.
fn split_command(command: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut started = false;
    let mut chars = command.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                started = true;
                for q in chars.by_ref() {
                    if q == '\'' {
                        break;
                    }
                    current.push(q);
                }
            }
            '"' => {
                started = true;
                for q in chars.by_ref() {
                    if q == '"' {
                        break;
                    }
                    current.push(q);
                }
            }
            '\\' if chars.peek().is_some() => {
                started = true;
                current.push(chars.next().unwrap());
            }
            c if c.is_whitespace() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            c => {
                started = true;
                current.push(c);
            }
        }
    }
    if started {
        words.push(current);
    }
    words
}

/// **A path with a space is quoted in a printed command (G2c item 4),** and the command, split as a shell would split it, runs: the circle's way B
/// with a file called `my notes.txt`.
#[test]
fn a_path_with_a_space_is_quoted_in_the_printed_command_and_the_command_runs() {
    let repo = support::unique_repo("g2c-quoted");
    support::init(&repo);
    support::generation(&repo, "heads/main", "my notes.txt", b"base\n", "base");
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
    std::fs::write(repo.join("my notes.txt"), b"base\nmy edit\n").unwrap();

    let switch = text(&prikk(&repo, &["branch", "switch", "heads/main"]));
    let quote = if cfg!(windows) {
        "\"my notes.txt\""
    } else {
        "'my notes.txt'"
    };
    assert!(
        switch.contains(&format!(
            "restore the file with `prikk cat --path {quote} --ref "
        )),
        "{switch}"
    );
    let start = switch.find("`prikk cat ").unwrap() + 1;
    let end = switch[start..].find('`').unwrap() + start;
    let words = split_command(&switch[start..end]);
    assert_eq!(words[0], "prikk");
    assert_eq!(
        words
            .iter()
            .filter(|w| w.as_str() == "my notes.txt")
            .count(),
        2,
        "{words:?}"
    );
    let args: Vec<&str> = words.iter().skip(1).map(String::as_str).collect();
    ok(
        &prikk(&repo, &args),
        "the printed cat, split as a shell splits it",
    );
    assert_eq!(std::fs::read(repo.join("my notes.txt")).unwrap(), b"base\n");
    ok(
        &prikk(&repo, &["branch", "switch", "heads/main"]),
        "the switch after the restore",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

// ---- 0.59.0 step G2d: candidate debris and the unproved lead -------------------------------------------------------------------------------

fn sealed_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "a.txt", b"one\n", "one");
    repo
}

fn plant(repo: &Path, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(repo.join(".prikk/refs/tmp")).unwrap();
    std::fs::write(repo.join(".prikk/refs/tmp").join(name), bytes).unwrap();
}

fn tmp_names(repo: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo.join(".prikk/refs/tmp"))
        .unwrap()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// **A leftover candidate no longer refuses a writer; `verify` keeps its warning and names the verb; the verb's plan, run and repeat do what
/// they say; the restore puts the files back byte for byte.** Every writer that printed the generic for it (G1 §1) goes through now.
#[test]
fn debris_does_not_refuse_a_writer_and_the_verb_clears_the_warning_and_is_undone_by_the_restore() {
    let repo = sealed_repo("g2d-debris");
    plant(&repo, "0.tmp", b"");
    plant(&repo, "1.tmp", b"PREFPTR1 and then garbage");
    for (label, args) in writers() {
        if label == "commit" || label == "rollback-draft" {
            continue;
        }
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let said = text(&prikk(&repo, &args));
        assert!(
            !said.contains(OLD_GENERIC) && !said.contains("incomplete ref publication"),
            "{label}: {said}"
        );
    }
    ok(
        &prikk(
            &repo,
            &["tag", "create", "tags/after", "--target", "heads/main"],
        ),
        "tag create beside the debris",
    );
    let verify = prikk(&repo, &["verify"]);
    ok(&verify, "verify (a warning only)");
    let said = text(&verify);
    assert!(said.contains("PRIKK-VERIFY-REF-CANDIDATE-DEBRIS"), "{said}");
    let line = said
        .lines()
        .find(|line| line.starts_with("way out [a ref pointer candidate remains in refs/tmp]"))
        .unwrap_or_else(|| panic!("no way-out line: {said}"));
    assert!(
        line.contains("`prikk doctor --remove-ref-candidates --plan-only`"),
        "{line}"
    );

    // The plan: names each file, writes nothing.
    let plan = prikk(&repo, &["doctor", "--remove-ref-candidates", "--plan-only"]);
    ok(&plan, "the plan");
    let said = text(&plan);
    assert!(said.contains("refs/tmp/0.tmp: would be removed"), "{said}");
    assert!(said.contains("refs/tmp/1.tmp: would be removed"), "{said}");
    assert!(said.contains("plan only -- nothing written"), "{said}");
    assert_eq!(tmp_names(&repo), ["0.tmp", "1.tmp"]);
    // The run, then the repeat.
    let run = prikk(&repo, &["doctor", "--remove-ref-candidates"]);
    ok(&run, "the run");
    let said = text(&run);
    assert!(said.contains("saved first: recovery/log, run "), "{said}");
    assert!(tmp_names(&repo).is_empty());
    let said = text(&prikk(&repo, &["verify"]));
    assert!(!said.contains("CANDIDATE-DEBRIS"), "{said}");
    let repeat = prikk(&repo, &["doctor", "--remove-ref-candidates"]);
    ok(&repeat, "the repeat");
    assert!(text(&repeat).contains("no ref pointer candidate remains"));
    // The way back.
    let listing = text(&prikk(&repo, &["doctor", "--recovery-list"]));
    let run_id = listing
        .lines()
        .find_map(|line| {
            line.strip_prefix("run ")
                .and_then(|rest| rest.split(':').next())
        })
        .unwrap_or_else(|| panic!("no run: {listing}"))
        .to_string();
    assert!(
        listing.contains("refs/tmp/1.tmp") && listing.contains("remove-ref-candidates"),
        "{listing}"
    );
    ok(
        &prikk(&repo, &["doctor", "--recovery-restore", &run_id]),
        "the restore",
    );
    assert_eq!(tmp_names(&repo), ["0.tmp", "1.tmp"]);
    assert_eq!(
        std::fs::read(repo.join(".prikk/refs/tmp/1.tmp")).unwrap(),
        b"PREFPTR1 and then garbage"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_verb_runs_alone_and_refuses_a_directory_in_refs_tmp_without_touching_anything() {
    let repo = sealed_repo("g2d-damage");
    plant(&repo, "a.tmp", b"x");
    let usage = prikk(
        &repo,
        &["doctor", "--remove-ref-candidates", "--repair-tails"],
    );
    assert_eq!(usage.status.code(), Some(2), "{}", text(&usage));
    assert!(
        text(&usage).contains("--remove-ref-candidates cannot be combined"),
        "{}",
        text(&usage)
    );
    std::fs::create_dir_all(repo.join(".prikk/refs/tmp/d")).unwrap();
    for args in [
        &["doctor", "--remove-ref-candidates", "--plan-only"][..],
        &["doctor", "--remove-ref-candidates"][..],
    ] {
        let out = prikk(&repo, args);
        assert_eq!(out.status.code(), Some(1), "{}", text(&out));
        assert!(
            text(&out).contains("unexpected non-file in ref candidate directory: refs/tmp/d")
                && text(&out).contains("no command removes it"),
            "{}",
            text(&out)
        );
    }
    assert_eq!(tmp_names(&repo), ["a.tmp", "d"]);
    let _ = std::fs::remove_dir_all(&repo);
}

/// **The unproved lead (item 0):** the sentence's first command is run and clears the state: `ref complete` refuses, the rebuild's plan names the
/// lead it drops, the rebuild runs, and `verify` and the next writer go through.
#[test]
fn an_unproved_lead_names_the_rebuild_and_the_rebuild_clears_it() {
    let (repo, g1) = two_generations("g2d-unproved");
    // A queued commit owned by heads/main that is not the leading Block's patches, then the log one transition behind the pointer.
    std::fs::write(repo.join("c.txt"), b"three\n").unwrap();
    ok(
        &support::commit(&repo, "heads/main", "three"),
        "a queued commit",
    );
    std::fs::copy(g1.join("refs/containers/log-a.container"), repo.join(LOG)).unwrap();
    let verify = prikk(&repo, &["verify"]);
    assert_eq!(verify.status.code(), Some(1), "{}", text(&verify));
    let said = text(&verify);
    assert!(said.contains("PRIKK-VERIFY-REF-DIVERGENCE"), "{said}");
    let line = said
        .lines()
        .find(|line| line.starts_with("way out [a ref's pointer and log disagree]"))
        .unwrap_or_else(|| panic!("no way-out line: {said}"));
    assert!(
        line.contains("`prikk ref complete` refuses it")
            && line.contains("`prikk doctor --rebuild-pointer-index --plan-only`")
            && line.contains("drops that lead"),
        "{line}"
    );
    // The sentence's claims, run: ref complete refuses; the plan names the drop; the rebuild runs; the repository is whole again.
    let complete = prikk(&repo, &["ref", "complete", "heads/main", "--plan-only"]);
    assert_eq!(complete.status.code(), Some(1), "{}", text(&complete));
    let plan = prikk(&repo, &["doctor", "--rebuild-pointer-index", "--plan-only"]);
    ok(&plan, "the rebuild's plan");
    assert!(
        text(&plan).contains("dropped lead: heads/main"),
        "{}",
        text(&plan)
    );
    ok(
        &prikk(&repo, &["doctor", "--rebuild-pointer-index"]),
        "the rebuild",
    );
    ok(&prikk(&repo, &["verify"]), "verify after the rebuild");
    ok(
        &prikk(
            &repo,
            &["tag", "create", "tags/ok", "--target", "heads/main"],
        ),
        "the next writer",
    );
    let _ = std::fs::remove_dir_all(&repo);
}
