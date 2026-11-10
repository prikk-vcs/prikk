//! 0.57.0 step 3, part S5b: `prikk uncommit <patch-id> [--plan-only]` through the binary. One test per case-table row, plus the
//! three things the owner's philosophy check added: a re-run finishes an interrupted run, the next commit authors the same
//! meaning as the commit taken back, and the state after the only patch leaves is the state `seal` leaves.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::process::Output;

use crate::support;
use crate::support::ok;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn code(output: &Output) -> i32 {
    output.status.code().unwrap_or(-1)
}

fn uncommit(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .arg("uncommit")
        .args(args)
        .output()
        .unwrap()
}

/// Write `body` to `path`, commit it on `heads/main`, and return the new patch's id.
fn commit_file(repo: &Path, path: &str, body: &[u8], message: &str) -> String {
    std::fs::write(repo.join(path), body).unwrap();
    let output = support::commit(repo, "heads/main", message);
    ok(&output, &format!("commit {message}"));
    patch_id_of(&output)
}

fn patch_id_of(commit: &Output) -> String {
    String::from_utf8_lossy(&commit.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints its patch id")
        .to_string()
}

/// The queued patch ids, oldest first, from `status --format json`.
fn queued(repo: &Path) -> Vec<String> {
    let output = support::prikk(repo)
        .args(["status", "--format", "json"])
        .output()
        .unwrap();
    ok(&output, "status --format json");
    let json = support::json::parse(&String::from_utf8_lossy(&output.stdout));
    json.get("queue")
        .get("patches")
        .as_array()
        .iter()
        .map(|patch| patch.get("patch_id").as_str().to_string())
        .collect()
}

/// The five files `uncommit` writes, as bytes (absent reads as empty).
fn files(repo: &Path) -> Vec<Vec<u8>> {
    let prikk = repo.join(".prikk");
    [
        "active/default/queue.wal",
        "active/default/witness",
        "active/default/ref-name",
        "worktree.baseline",
        "active/default/declarations",
    ]
    .iter()
    .map(|path| std::fs::read(prikk.join(path)).unwrap_or_default())
    .collect()
}

fn repo_with(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    repo
}

/// The `  <kind> <path>` lines of a commit's report, sorted.
fn operations_of(commit: &Output) -> Vec<String> {
    let mut lines: Vec<String> = String::from_utf8_lossy(&commit.stdout)
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("  declaration"))
        .map(|line| line.trim().to_string())
        .collect();
    lines.sort();
    lines
}

#[test]
fn the_help_line_says_nothing_is_wrong_the_files_stay_and_sealed_history_is_never_touched() {
    let repo = repo_with("s5b-help");
    let help = support::prikk(&repo)
        .args(["uncommit", "--help"])
        .output()
        .unwrap();
    ok(&help, "uncommit --help");
    let words = text(&help);
    for needed in [
        "nothing is wrong",
        "not changed",
        "sealed history is never touched",
        "rollback-draft",
        "<patch-id>",
        "--plan-only",
    ] {
        assert!(words.contains(needed), "help says {needed:?}: {words}");
    }
    assert!(
        !words.contains("discard") && !words.contains("damaged"),
        "the help shares no word with --discard-damaged-commits: {words}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_usage_error_exits_2_before_the_repository_is_read() {
    let repo = repo_with("s5b-usage");
    let id = commit_file(&repo, "a.txt", b"one", "one");
    for (args, what) in [
        (vec![], "no id"),
        (vec!["not-an-id"], "not an id"),
        (vec![&id[..], "--nonsense"], "an unknown flag"),
        (
            vec![&id[..], "--plan-only", "--plan-only"],
            "a duplicate flag",
        ),
        (vec![&id[..], &id[..]], "two ids"),
    ] {
        let output = uncommit(&repo, &args);
        assert_eq!(code(&output), 2, "{what}: {}", text(&output));
    }
    assert_eq!(queued(&repo), vec![id], "a usage error writes nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_newest_commit_leaves_the_files_in_place_and_the_next_commit_authors_them_again() {
    let repo = repo_with("s5b-newest");
    let first = commit_file(&repo, "a.txt", b"one", "one");
    let second = commit_file(&repo, "b.txt", b"two", "two");
    let before = files(&repo);

    let plan = uncommit(&repo, &[&second, "--plan-only"]);
    assert_eq!(code(&plan), 0, "{}", text(&plan));
    let plan_text = text(&plan);
    assert!(
        plan_text.contains("plan only -- nothing written"),
        "{plan_text}"
    );
    assert!(
        plan_text.contains("kept in the recovery log"),
        "{plan_text}"
    );
    assert!(
        plan_text.contains("your files in the worktree are not changed"),
        "{plan_text}"
    );
    assert!(plan_text.contains("create-file b.txt"), "{plan_text}");
    assert_eq!(files(&repo), before, "--plan-only writes nothing");

    let run = uncommit(&repo, &[&second]);
    assert_eq!(code(&run), 0, "{}", text(&run));
    let run_text = text(&run);
    assert!(run_text.contains("commit taken back"), "{run_text}");
    assert!(
        run_text.contains("prikk doctor --recovery-restore"),
        "{run_text}"
    );
    // The plan is the run: every line the plan printed except its last, the run printed too.
    let plan_lines: Vec<&str> = plan_text
        .lines()
        .filter(|l| !l.starts_with("plan only"))
        .collect();
    for line in plan_lines {
        assert!(
            run_text.contains(line),
            "the run says what the plan said: {line}\n{run_text}"
        );
    }

    assert_eq!(queued(&repo), vec![first.clone()]);
    assert_eq!(
        std::fs::read(repo.join("b.txt")).unwrap(),
        b"two",
        "the file stays"
    );
    let verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert_eq!(code(&verify), 0, "verify reads clean: {}", text(&verify));
    let again = support::commit(&repo, "heads/main", "two again");
    ok(&again, "commit again");
    assert!(
        text(&again).contains("create-file b.txt"),
        "the next commit includes the file: {}",
        text(&again)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn the_undo_restores_every_file_and_refuses_once_anything_has_changed() {
    let repo = repo_with("s5b-undo");
    commit_file(&repo, "a.txt", b"one", "one");
    let second = commit_file(&repo, "b.txt", b"two", "two");
    let before = files(&repo);
    let run = uncommit(&repo, &[&second]);
    ok(&run, "uncommit");
    let run_id = text(&run)
        .split("--recovery-restore ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("the run names its recovery run")
        .trim_matches('`')
        .to_string();

    let restore = support::prikk(&repo)
        .args(["doctor", "--recovery-restore", &run_id])
        .output()
        .unwrap();
    assert_eq!(code(&restore), 0, "{}", text(&restore));
    assert_eq!(
        files(&repo),
        before,
        "the undo returns the five files byte for byte"
    );
    assert_eq!(queued(&repo).len(), 2);

    // Again, then something changes after: the undo refuses and names the file.
    let run = uncommit(&repo, &[&second]);
    ok(&run, "uncommit again");
    let run_id = text(&run)
        .split("--recovery-restore ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap()
        .trim_matches('`')
        .to_string();
    commit_file(&repo, "c.txt", b"three", "three");
    let refused = support::prikk(&repo)
        .args(["doctor", "--recovery-restore", &run_id])
        .output()
        .unwrap();
    assert_ne!(
        code(&refused),
        0,
        "the restore refuses once a commit has landed: {}",
        text(&refused)
    );
    assert!(text(&refused).contains("changed"), "{}", text(&refused));
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_middle_commit_a_sealed_patch_a_block_and_an_unknown_id_are_each_refused_by_what_they_are() {
    let repo = repo_with("s5b-refusals");
    let sealed = {
        std::fs::write(repo.join("s.txt"), b"sealed").unwrap();
        let commit = support::commit(&repo, "heads/main", "to seal");
        ok(&commit, "commit to seal");
        let id = patch_id_of(&commit);
        ok(&support::seal(&repo, "heads/main"), "seal");
        id
    };
    let first = commit_file(&repo, "a.txt", b"one", "one");
    let second = commit_file(&repo, "b.txt", b"two", "two");
    let block_id = {
        let log = support::prikk(&repo)
            .args(["log", "--format", "json"])
            .output()
            .unwrap();
        ok(&log, "log");
        support::json::parse(&String::from_utf8_lossy(&log.stdout))
            .get("blocks")
            .as_array()[0]
            .get("block_id")
            .as_str()
            .to_string()
    };
    let before = files(&repo);

    let middle = uncommit(&repo, &[&first]);
    assert_eq!(code(&middle), 1, "{}", text(&middle));
    assert!(
        text(&middle).contains(&second),
        "names the later patch: {}",
        text(&middle)
    );
    assert!(text(&middle).contains("new patch ids"), "{}", text(&middle));

    let sealed_text = text(&uncommit(&repo, &[&sealed]));
    assert!(sealed_text.contains("sealed into a block"), "{sealed_text}");
    assert!(sealed_text.contains("rollback-draft"), "{sealed_text}");

    let block_text = text(&uncommit(&repo, &[&block_id]));
    assert!(block_text.contains("not a queued commit"), "{block_text}");
    assert!(block_text.to_lowercase().contains("block"), "{block_text}");

    let unknown = "cd".repeat(32);
    let unknown_out = uncommit(&repo, &[&unknown]);
    assert_eq!(code(&unknown_out), 1);
    assert!(
        text(&unknown_out).contains("no object in this repository has that id"),
        "{}",
        text(&unknown_out)
    );

    assert_eq!(files(&repo), before, "every refusal writes nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_damaged_queue_refuses_with_its_own_verb() {
    let repo = repo_with("s5b-damaged");
    let id = commit_file(&repo, "a.txt", b"one", "one");
    let wal = repo.join(".prikk/active/default/queue.wal");
    let mut bytes = std::fs::read(&wal).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 0xFF;
    std::fs::write(&wal, &bytes).unwrap();
    let output = uncommit(&repo, &[&id]);
    assert_eq!(code(&output), 1);
    assert!(
        text(&output).contains("prikk doctor --discard-damaged-commits"),
        "{}",
        text(&output)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The owner's check, item 2: the next commit must author the same meaning as the commit taken back. Every operation kind a
/// `commit` authors from the worktree is in one patch; the commit after `uncommit` lists the same operations.
#[test]
fn the_next_commit_authors_the_same_operations_as_the_commit_taken_back() {
    let repo = repo_with("s5b-same-meaning");
    for (path, body) in [
        ("edit.txt", &b"one\ntwo\n"[..]),
        ("gone.txt", b"gone\n"),
        ("moved.txt", b"moved\n"),
        ("mode.sh", b"#!/bin/sh\n"),
        ("blob.bin", b"bin\0\x01"),
    ] {
        std::fs::write(repo.join(path), body).unwrap();
    }
    ok(&support::commit(&repo, "heads/main", "base"), "commit base");
    ok(&support::seal(&repo, "heads/main"), "seal base");

    std::fs::write(repo.join("edit.txt"), b"one\nTWO\n").unwrap();
    std::fs::remove_file(repo.join("gone.txt")).unwrap();
    std::fs::write(repo.join("new.txt"), b"new\n").unwrap();
    std::fs::write(repo.join("blob.bin"), b"bin2\0\x02").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(repo.join("mode.sh"), std::fs::Permissions::from_mode(0o755))
            .unwrap();
    }
    ok(
        &support::prikk(&repo)
            .args(["mv", "moved.txt", "renamed.txt"])
            .output()
            .unwrap(),
        "mv",
    );
    let first = support::commit(&repo, "heads/main", "everything");
    ok(&first, "commit everything");
    let wanted = operations_of(&first);
    assert!(
        wanted.iter().any(|op| op.starts_with("rename-path")),
        "{wanted:?}"
    );

    let plan = uncommit(&repo, &[&patch_id_of(&first), "--plan-only"]);
    assert_eq!(code(&plan), 0, "{}", text(&plan));
    let plan_text = text(&plan);
    assert!(
        plan_text.contains("rename declaration"),
        "the rename is put back: {plan_text}"
    );
    assert!(
        !plan_text.contains("does not have this change"),
        "every change is still in the worktree: {plan_text}"
    );
    ok(&uncommit(&repo, &[&patch_id_of(&first)]), "uncommit");

    let second = support::commit(&repo, "heads/main", "everything again");
    ok(&second, "commit again");
    assert_eq!(
        operations_of(&second),
        wanted,
        "the next commit authors the same operations, including the rename"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The owner's check, item 4: after the only queued patch leaves, the session is an empty queue, the state `seal` leaves.
#[test]
fn the_only_patch_leaves_the_state_seal_leaves() {
    let repo = repo_with("s5b-only");
    let fresh = files(&repo);
    let id = commit_file(&repo, "a.txt", b"one", "one");
    let run = uncommit(&repo, &[&id]);
    assert_eq!(code(&run), 0, "{}", text(&run));
    assert!(
        text(&run).contains("its owner is cleared"),
        "{}",
        text(&run)
    );
    let after = files(&repo);
    assert_eq!(
        &after[..3],
        &fresh[..3],
        "queue, witness and owner are as a fresh repository has them"
    );
    assert!(queued(&repo).is_empty());
    let status = support::prikk(&repo)
        .arg("worktree-status")
        .output()
        .unwrap();
    assert!(
        !text(&status).contains("behind"),
        "the record is the empty queue's, not behind: {}",
        text(&status)
    );
    let verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert_eq!(code(&verify), 0, "{}", text(&verify));
    let _ = std::fs::remove_dir_all(&repo);
}

/// The owner's check, item 1: the natural thing after an interruption is to type the same command again.
#[test]
fn typing_the_same_command_again_finishes_an_interrupted_run_and_says_so() {
    let repo = repo_with("s5b-rerun");
    let first = commit_file(&repo, "a.txt", b"one", "one");
    let second = commit_file(&repo, "b.txt", b"two", "two");
    // The state a crash right after the WAL cut leaves: the frame is gone, the witness still names it.
    let wal = repo.join(".prikk/active/default/queue.wal");
    let bytes = std::fs::read(&wal).unwrap();
    let plan = uncommit(&repo, &[&second, "--plan-only"]);
    ok(&plan, "plan");
    let removed: usize = text(&plan)
        .split("recovery log (")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .expect("the plan says how many bytes leave the WAL");
    std::fs::write(&wal, &bytes[..bytes.len() - removed]).unwrap();

    let verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert_ne!(
        code(&verify),
        0,
        "the interrupted state is the lost-commit row: {}",
        text(&verify)
    );
    assert!(
        text(&verify).contains("interrupted `prikk uncommit`"),
        "the recommendation says a re-run finishes it: {}",
        text(&verify)
    );

    // `doctor`'s way out for this row carries the clause too (its own sentence, beside the refusal text above).
    let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
    let way_out = text(&doctor)
        .lines()
        .find(|line| line.starts_with("way out ["))
        .unwrap_or_else(|| panic!("doctor names a way out: {}", text(&doctor)))
        .to_string();
    assert!(
        way_out.contains("interrupted `prikk uncommit`"),
        "{way_out}"
    );

    // The log holds no `uncommit` run for it (the state was made by hand): the same state is what a lost acknowledged commit looks
    // like, and the sentence says both.
    let rerun = uncommit(&repo, &[&second]);
    assert_eq!(code(&rerun), 0, "{}", text(&rerun));
    assert!(
        text(&rerun).contains(
            "already out of the queue (an interrupted `prikk uncommit`, or an acknowledged commit that was lost); taking it out of the record"
        ),
        "{}",
        text(&rerun)
    );
    assert_eq!(queued(&repo), vec![first]);
    let verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert_eq!(code(&verify), 0, "{}", text(&verify));

    let after = uncommit(&repo, &[&second]);
    assert_eq!(code(&after), 1);
    assert!(
        text(&after).contains("not queued"),
        "a re-run after completion: {}",
        text(&after)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The owner's check, item 3: a blob an uncommit leaves is not "a publication trust that was not checked".
#[test]
fn a_blob_left_behind_reads_as_an_unreached_blob_not_as_a_trust_that_was_not_checked() {
    let repo = repo_with("s5b-blob");
    let id = commit_file(&repo, "d.bin", b"bin\0\x01\x02", "binary");
    ok(&uncommit(&repo, &[&id]), "uncommit");
    let verify = support::prikk(&repo).arg("verify").output().unwrap();
    assert_eq!(
        code(&verify),
        0,
        "an unreached blob is a warning, not a failure: {}",
        text(&verify)
    );
    let words = text(&verify);
    let line = words
        .lines()
        .find(|line| line.starts_with("warning: blob "))
        .unwrap_or_else(|| panic!("verify names the blob: {words}"));
    assert!(
        line.contains("is reached by nothing in this repository"),
        "{line}"
    );
    assert!(line.contains("no publication trust to check"), "{line}");
    assert!(
        !line.contains("was not checked"),
        "a blob has no trust that was skipped: {line}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// inklkunn's stuck state: a queued patch nobody means to keep blocks `sync seal --claim`; `uncommit` clears it.
#[test]
fn an_orphan_queued_patch_blocking_sync_seal_claim_is_cleared_by_uncommit() {
    let repo = repo_with("s5b-claim");
    let orphan = commit_file(&repo, "orphan.txt", b"orphan\n", "orphan");
    let claim = accept_one_claim(&repo, "o.txt", b"o\n");

    let stuck = seal_claim(&repo, &claim);
    assert_ne!(code(&stuck), 0, "{}", text(&stuck));
    assert!(
        text(&stuck).contains("requires an empty active WAL"),
        "{}",
        text(&stuck)
    );

    ok(&uncommit(&repo, &[&orphan]), "uncommit the orphan");
    std::fs::remove_file(repo.join("orphan.txt")).unwrap();
    let sealed = seal_claim(&repo, &claim);
    assert_eq!(code(&sealed), 0, "{}", text(&sealed));
    assert!(
        repo.join("o.txt").exists(),
        "the claim's content is in the worktree"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

fn seal_claim(repo: &Path, claim: &str) -> Output {
    support::trust_maintainer(repo);
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", claim])
        .output()
        .unwrap()
}

/// A claim accepted into `repo`, naming a real patch sealed on a disposable sender (the ordinary RFC 116 loop).
fn accept_one_claim(repo: &Path, path: &str, content: &[u8]) -> String {
    let sender = support::unique_repo(&format!("s5b-sender-{}", support::unique_suffix()));
    support::init(&sender);
    support::generation(&sender, "heads/main", path, content, "genesis");
    let have = std::env::temp_dir().join(format!("s5b-have-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(repo)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );
    let artifact =
        std::env::temp_dir().join(format!("s5b-artifact-{}.bin", support::unique_suffix()));
    ok(
        &support::prikk(&sender)
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
                have.to_str().unwrap(),
                "--output",
                artifact.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    let accept = support::prikk(repo)
        .args(["sync", "accept", artifact.to_str().unwrap()])
        .output()
        .unwrap();
    ok(&accept, "sync accept");
    let claim = String::from_utf8_lossy(&accept.stdout)
        .lines()
        .find(|line| line.trim_start().starts_with("claim "))
        .and_then(|line| line.trim_start().strip_prefix("claim "))
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .trim()
        .to_string();
    let _ = std::fs::remove_dir_all(&sender);
    let _ = std::fs::remove_file(&have);
    let _ = std::fs::remove_file(&artifact);
    claim
}

/// The race, through the binary: `uncommit` of the newest patch against a `commit` loop, N rounds. Each round ends in one of two
/// states, `verify` exits 0 after every round, and no acknowledged commit is lost. (The owner's rule: state n/N.)
#[test]
fn uncommit_against_a_commit_loop_ends_in_one_of_two_states_every_round_and_verify_stays_clean() {
    // 12 rounds in the suite; `PRIKK_S5B_RACE_ROUNDS=<n>` runs a longer stress by hand.
    let rounds: usize = std::env::var("PRIKK_S5B_RACE_ROUNDS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(12);
    let mut taken_back_first = 0;
    let mut commit_first = 0;
    for round in 0..rounds {
        let repo = repo_with(&format!("s5b-race-{round}"));
        commit_file(&repo, "base.txt", b"base", "base");
        let target = commit_file(&repo, "target.txt", b"target", "target");
        let racing_repo = repo.clone();
        let racer = std::thread::spawn(move || {
            std::fs::write(racing_repo.join("racing.txt"), b"racing").unwrap();
            for _ in 0..200 {
                let output = support::commit(&racing_repo, "heads/main", "racing");
                if output.status.success() {
                    return Some(patch_id_of(&output));
                }
                std::thread::yield_now();
            }
            None
        });
        let mut outcome = None;
        for _ in 0..200 {
            let output = uncommit(&repo, &[&target]);
            if text(&output).contains("lock") && !output.status.success() {
                std::thread::yield_now();
                continue;
            }
            outcome = Some(output);
            break;
        }
        let raced = racer.join().unwrap().expect("the racing commit landed");
        let output = outcome.expect("uncommit finished");
        let ids = queued(&repo);
        if output.status.success() {
            taken_back_first += 1;
            assert!(!ids.contains(&target), "round {round}: taken back");
        } else {
            commit_first += 1;
            assert!(
                text(&output).contains("is not the newest queued commit"),
                "round {round}: the commit landed first, and the refusal says so: {}",
                text(&output)
            );
            assert!(ids.contains(&target), "round {round}: refused, so it stays");
        }
        assert!(
            ids.contains(&raced),
            "round {round}: no acknowledged commit is lost"
        );
        let verify = support::prikk(&repo).arg("verify").output().unwrap();
        assert_eq!(
            code(&verify),
            0,
            "round {round}: verify reads clean: {}",
            text(&verify)
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
    // State n/N: every round ended in one of the two allowed states.
    assert_eq!(
        taken_back_first + commit_first,
        rounds,
        "{taken_back_first} + {commit_first} of {rounds}"
    );
    eprintln!(
        "uncommit race: {taken_back_first} taken back first, {commit_first} commit first, {rounds}/{rounds} rounds in an allowed state"
    );
}

fn run_id_in(list: &str) -> String {
    list.lines()
        .find_map(|line| line.strip_prefix("run "))
        .and_then(|rest| rest.split(':').next())
        .expect("the list names a run")
        .to_string()
}

/// S5c: with an `uncommit` run for this patch in the log, the re-run names it (a real interruption: the files a crash after the WAL
/// cut leaves, rebuilt from the run's own pre-state).
#[test]
fn a_rerun_with_the_earlier_run_in_the_log_names_it() {
    let repo = repo_with("s5c-named");
    commit_file(&repo, "a.txt", b"one", "one");
    let second = commit_file(&repo, "b.txt", b"two", "two");
    let pre = files(&repo);
    ok(&uncommit(&repo, &[&second]), "uncommit");
    // Back to the state after the WAL cut: the witness, the owner and the record as they were before the run.
    let prikk = repo.join(".prikk");
    for (path, bytes) in [
        "active/default/witness",
        "active/default/ref-name",
        "worktree.baseline",
    ]
    .iter()
    .zip([&pre[1], &pre[2], &pre[3]])
    {
        std::fs::write(prikk.join(path), bytes).unwrap();
    }
    let list = support::prikk(&repo)
        .args(["doctor", "--recovery-list"])
        .output()
        .unwrap();
    let run = run_id_in(&text(&list));

    let rerun = uncommit(&repo, &[&second]);
    assert_eq!(code(&rerun), 0, "{}", text(&rerun));
    assert!(
        text(&rerun).contains(&format!(
            "finishing an interrupted `prikk uncommit` (recovery run {run})"
        )),
        "{}",
        text(&rerun)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// S5c: the restore's closing sentence follows the run's own recorded kind. After undoing an `uncommit` it says the commit is queued
/// again and does not talk of damage; after undoing a damage repair it still says the damage is back.
#[test]
fn the_restore_closing_sentence_follows_the_kind_of_the_run() {
    let repo = repo_with("s5c-closing-uncommit");
    commit_file(&repo, "a.txt", b"one", "one");
    let second = commit_file(&repo, "b.txt", b"two", "two");
    ok(&uncommit(&repo, &[&second]), "uncommit");
    let list = support::prikk(&repo)
        .args(["doctor", "--recovery-list"])
        .output()
        .unwrap();
    let run = run_id_in(&text(&list));
    let plan = support::prikk(&repo)
        .args(["doctor", "--recovery-restore", &run, "--plan-only"])
        .output()
        .unwrap();
    assert!(
        text(&plan).contains("the commit is queued again"),
        "{}",
        text(&plan)
    );
    assert!(!text(&plan).contains("damage"), "{}", text(&plan));
    let done = support::prikk(&repo)
        .args(["doctor", "--recovery-restore", &run])
        .output()
        .unwrap();
    assert_eq!(code(&done), 0, "{}", text(&done));
    assert!(
        text(&done).contains("the commit is queued again"),
        "{}",
        text(&done)
    );
    assert!(!text(&done).contains("damage"), "{}", text(&done));
    let _ = std::fs::remove_dir_all(&repo);

    let repo = repo_with("s5c-closing-repair");
    commit_file(&repo, "a.txt", b"one", "one");
    let wal = repo.join(".prikk/active/default/queue.wal");
    let mut bytes = std::fs::read(&wal).unwrap();
    let at = bytes.len() - 5;
    bytes[at] ^= 0xFF;
    std::fs::write(&wal, &bytes).unwrap();
    ok(
        &support::prikk(&repo)
            .args(["doctor", "--discard-damaged-commits"])
            .output()
            .unwrap(),
        "discard the damaged commit",
    );
    let list = support::prikk(&repo)
        .args(["doctor", "--recovery-list"])
        .output()
        .unwrap();
    let run = run_id_in(&text(&list));
    let done = support::prikk(&repo)
        .args(["doctor", "--recovery-restore", &run])
        .output()
        .unwrap();
    assert_eq!(code(&done), 0, "{}", text(&done));
    assert!(text(&done).contains("damage included"), "{}", text(&done));
    assert!(!text(&done).contains("queued again"), "{}", text(&done));
    let _ = std::fs::remove_dir_all(&repo);
}
