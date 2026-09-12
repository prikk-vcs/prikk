//! RFC 151 increment 1: the current-branch pointer and the `--ref` default.
//!
//! Every command runs through `support::prikk`, the isolation seam: an empty per-repository config
//! home and no ambient `PRIKK_*` key variables.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

const POINTER: &str = ".prikk/current-branch";

fn pointer(repo: &Path) -> PathBuf {
    repo.join(POINTER)
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repository with one sealed generation on `heads/main`.
fn sealed_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "readme.txt", b"hello\n", "genesis");
    repo
}

/// Run `args` with both signing keys available, so a refusal about the pointer cannot be masked by
/// a missing-key refusal that happens to come first.
fn run_with_keys(repo: &Path, args: &[&str]) -> Output {
    support::trust_maintainer(repo);
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

fn assert_refused(output: &Output, what: &str, needles: &[&str]) {
    assert_eq!(
        output.status.code(),
        Some(1),
        "{what} must refuse\nstdout: {}\nstderr: {}",
        stdout(output),
        stderr(output)
    );
    for needle in needles {
        assert!(
            stderr(output).contains(needle),
            "{what}: stderr must contain {needle:?}\nstderr: {}",
            stderr(output)
        );
    }
}

#[test]
fn a_fresh_repository_has_the_pointer_on_heads_main_and_reinit_keeps_a_moved_one() {
    let repo = support::unique_repo("rfc151-fresh");
    support::init(&repo);
    assert_eq!(std::fs::read(pointer(&repo)).unwrap(), b"heads/main\n");

    std::fs::write(pointer(&repo), b"heads/elsewhere\n").unwrap();
    support::init(&repo);
    assert_eq!(
        std::fs::read(pointer(&repo)).unwrap(),
        b"heads/elsewhere\n",
        "a re-init must never move the current branch"
    );
}

#[test]
fn a_repository_without_the_pointer_defaults_to_heads_main_and_no_read_writes_it() {
    let repo = sealed_repo("rfc151-pre-rfc");
    std::fs::remove_file(pointer(&repo)).unwrap();

    let status = support::prikk(&repo)
        .args(["worktree-status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&status, "worktree-status without a pointer");
    assert!(stdout(&status).contains("\"ref\": \"heads/main\""));
    assert!(stdout(&status).contains("\"current_branch\": \"heads/main\""));

    let log = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&log, "log without a pointer");
    assert!(stdout(&log).contains("\"ref\": \"heads/main\""));

    let list = support::prikk(&repo)
        .args(["branch", "list"])
        .output()
        .unwrap();
    support::ok(&list, "branch list without a pointer");
    assert!(stdout(&list).contains("* heads/main "));

    let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
    assert!(!stdout(&doctor).contains("PRIKK-DOCTOR-CURRENT-BRANCH"));

    assert!(
        !pointer(&repo).exists(),
        "reading the default must never create the pointer"
    );
}

#[test]
fn a_malformed_pointer_is_refused_naming_the_file() {
    let repo = sealed_repo("rfc151-malformed");
    for contents in [
        &b"heads/main"[..],
        b"tags/v1\n",
        b"heads/main\nheads/other\n",
        b"\xff\xfe\n",
        b"\n",
    ] {
        std::fs::write(pointer(&repo), contents).unwrap();
        let what = format!("worktree-status with pointer {contents:?}");
        let output = support::prikk(&repo)
            .arg("worktree-status")
            .output()
            .unwrap();
        assert_refused(
            &output,
            &what,
            &["precondition not met", ".prikk/current-branch is malformed"],
        );

        let explicit = support::prikk(&repo)
            .args(["worktree-status", "--ref", "heads/main"])
            .output()
            .unwrap();
        support::ok(&explicit, &format!("{what}, --ref given explicitly"));
        assert!(stdout(&explicit).contains("current branch: <unresolved; run `prikk doctor`>"));
    }
}

#[test]
fn a_pointer_naming_a_closed_or_missing_branch_is_refused_and_doctor_reports_it() {
    let repo = sealed_repo("rfc151-closed");
    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    support::ok(
        &support::branch_close(&repo, "heads/topic"),
        "branch close heads/topic",
    );

    for (target, reason) in [
        ("heads/topic", "which is closed"),
        ("heads/nowhere", "which does not exist"),
    ] {
        std::fs::write(pointer(&repo), format!("{target}\n")).unwrap();
        let output = support::prikk(&repo).arg("log").output().unwrap();
        assert_refused(
            &output,
            &format!("log with the pointer on {target}"),
            &[
                "precondition not met",
                &format!("{POINTER} names {target}, {reason}"),
                "prikk branch switch",
                "prikk branch create",
            ],
        );

        let doctor = support::prikk(&repo).arg("doctor").output().unwrap();
        let report = stdout(&doctor);
        assert!(
            report.contains("PRIKK-DOCTOR-CURRENT-BRANCH") && report.contains(reason),
            "doctor must report the pointer on {target}\n{report}"
        );
    }
}

/// Every site whose `--ref` (or `branch create --from`) used to default to `heads/main`. With the
/// pointer on a branch that does not exist, each must refuse naming that branch -- which it can
/// only do by having resolved the pointer -- and the same command with the ref given explicitly
/// must never mention the pointer at all.
#[test]
fn every_default_site_resolves_through_the_pointer() {
    let repo = sealed_repo("rfc151-sites");
    let bundle = repo.join("not-a-bundle.bin");
    std::fs::write(&bundle, b"not a bundle").unwrap();
    let bundle = bundle.to_str().unwrap().to_string();
    std::fs::write(pointer(&repo), b"heads/other\n").unwrap();

    let sites: Vec<(Vec<&str>, Vec<&str>)> = vec![
        (vec!["commit", "-m", "site"], vec!["--ref", "heads/main"]),
        (
            vec!["seal", "--allow-no-audit"],
            vec!["--ref", "heads/main"],
        ),
        (vec!["log"], vec!["--ref", "heads/main"]),
        (vec!["checkout", "--plan-only"], vec!["--ref", "heads/main"]),
        (
            vec!["checkout", "--snapshot-plan"],
            vec!["--ref", "heads/main"],
        ),
        (
            vec!["checkout", "--snapshot-materialize"],
            vec!["--ref", "heads/main"],
        ),
        (
            vec!["checkout", "--patch-plan"],
            vec!["--ref", "heads/main"],
        ),
        (
            vec!["checkout", "--patch-materialize"],
            vec!["--ref", "heads/main"],
        ),
        (
            vec!["checkout", "--patch-delete-plan"],
            vec!["--ref", "heads/main"],
        ),
        (
            vec!["checkout", "--patch-materialize-delete"],
            vec!["--ref", "heads/main"],
        ),
        (vec!["worktree-status"], vec!["--ref", "heads/main"]),
        (vec!["inverse-plan"], vec!["--ref", "heads/main"]),
        (vec!["rollback-preview"], vec!["--ref", "heads/main"]),
        (
            vec!["rollback-draft", "--append-inverse", "-m", "site"],
            vec!["--ref", "heads/main"],
        ),
        (vec!["rollback-draft-verify"], vec!["--ref", "heads/main"]),
        (
            vec!["bundle", "preview", "--input", &bundle],
            vec!["--ref", "heads/main"],
        ),
        (
            vec!["branch", "create", "heads/new"],
            vec!["--from", "heads/main"],
        ),
    ];
    for (command, explicit) in &sites {
        let what = command.join(" ");
        let output = run_with_keys(&repo, command);
        assert_refused(
            &output,
            &what,
            &[&format!(
                "{POINTER} names heads/other, which does not exist"
            )],
        );

        let mut with_ref = command.clone();
        with_ref.extend(explicit.iter().copied());
        let output = run_with_keys(&repo, &with_ref);
        assert!(
            !stderr(&output).contains(POINTER),
            "{} must not consult the pointer\nstderr: {}",
            with_ref.join(" "),
            stderr(&output)
        );
    }
    assert_eq!(std::fs::read(pointer(&repo)).unwrap(), b"heads/other\n");
}

#[test]
fn the_current_branch_is_visible_in_worktree_status_log_and_branch_list() {
    let repo = sealed_repo("rfc151-visible");
    support::ok(
        &support::branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    std::fs::write(pointer(&repo), b"heads/topic\n").unwrap();

    let status = support::prikk(&repo)
        .args(["worktree-status", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&status, "worktree-status on heads/topic");
    assert!(stdout(&status).contains("\"ref\": \"heads/topic\""));
    assert!(stdout(&status).contains("\"current_branch\": \"heads/topic\""));

    let explicit = support::prikk(&repo)
        .args(["worktree-status", "--ref", "heads/main", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&explicit, "worktree-status --ref heads/main");
    assert!(stdout(&explicit).contains("\"ref\": \"heads/main\""));
    assert!(stdout(&explicit).contains("\"current_branch\": \"heads/topic\""));

    let log = support::prikk(&repo).arg("log").output().unwrap();
    support::ok(&log, "log on heads/topic");
    assert!(stdout(&log).contains("ref: heads/topic\ncurrent branch: heads/topic\n"));
    let log_json = support::prikk(&repo)
        .args(["log", "--format", "json"])
        .output()
        .unwrap();
    assert!(stdout(&log_json).contains("\"current_branch\": \"heads/topic\""));

    let list = support::prikk(&repo)
        .args(["branch", "list"])
        .output()
        .unwrap();
    support::ok(&list, "branch list");
    let listed = stdout(&list);
    assert!(
        listed
            .lines()
            .any(|line| line.starts_with("* heads/topic "))
    );
    assert!(listed.lines().any(|line| line.starts_with("heads/main ")));
    let list_json = support::prikk(&repo)
        .args(["branch", "list", "--format", "json"])
        .output()
        .unwrap();
    let listed_json = stdout(&list_json);
    assert!(listed_json.contains("\"ref_name\": \"heads/topic\""));
    assert_eq!(listed_json.matches("\"current\": true").count(), 1);
    assert_eq!(listed_json.matches("\"current\": false").count(), 1);
    let topic_entry = listed_json
        .lines()
        .find(|line| line.contains("\"ref_name\": \"heads/topic\""))
        .unwrap();
    assert!(topic_entry.contains("\"current\": true"));
}

/// The pointer is a default, never an authority: in both production trees, the only callers of
/// `current_branch` are the CLI's resolution module and `doctor`. A new reader -- in `verify`,
/// trust, signing, `bundle`, `sync`, or anywhere else -- fails this.
#[test]
fn only_the_cli_resolution_path_and_doctor_read_the_pointer() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let mut callers = Vec::new();
    for tree in ["prikk-store/src", "prikk-cli/src"] {
        let root = crates.join(tree);
        let mut files = Vec::new();
        collect_production_rust_files(&root, &mut files);
        assert!(!files.is_empty(), "{tree} has no production files");
        for file in files {
            let text = std::fs::read_to_string(&file).unwrap();
            let relative = file
                .strip_prefix(crates)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            for line in text.lines() {
                let code = line.trim_start();
                if code.starts_with("//") || code.starts_with("pub fn current_branch(") {
                    continue;
                }
                if calls_current_branch(code) {
                    callers.push(relative.clone());
                }
            }
        }
    }
    callers.sort();
    callers.dedup();
    assert_eq!(
        callers,
        [
            "prikk-cli/src/current_branch.rs",
            "prikk-store/src/doctor.rs"
        ],
        "only the CLI's resolution module and doctor may read the current-branch pointer"
    );
}

/// `current_branch(` as a whole identifier, so `displayed_current_branch(` is not counted.
fn calls_current_branch(code: &str) -> bool {
    code.match_indices("current_branch(").any(|(index, _)| {
        index == 0
            || !code[..index]
                .chars()
                .next_back()
                .is_some_and(|previous| previous.is_ascii_alphanumeric() || previous == '_')
    })
}

fn collect_production_rust_files(directory: &Path, files: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(directory).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        if path.is_dir() {
            if name == "tests" || name == "test_gates" || name.ends_with("_tests") {
                continue;
            }
            collect_production_rust_files(&path, files);
        } else if name.ends_with(".rs") && name != "tests.rs" {
            files.push(path);
        }
    }
}
