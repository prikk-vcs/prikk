//! RFC 147 §2f, through the compiled binary: a declaration reports what `commit` will do with it, and
//! a refusal names a route that works.
//!
//! From stikk's letter 013. `worktree-status` used to list a declaration with no verdict, so a
//! front-end could show "rename" while `commit` was going to refuse — and in one state the worktree
//! reads `clean` and the commit still refuses. The resolution now comes from the one classifier
//! `commit` itself obeys, and every refusal is run here as written.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::json::{self, Value};

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

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

/// `worktree-status` exits 1 whenever the worktree has changes, so these controls read the document
/// it printed rather than its exit status.
fn status(repo: &Path) -> Value {
    let output = run(repo, &["worktree-status", "--format", "json"]);
    let text = stdout(&output);
    assert!(
        text.starts_with('{'),
        "worktree-status printed no document: {}",
        stderr(&output)
    );
    json::parse(&text)
}

/// The shared parser keeps numbers as their source text; these counts are read as written.
fn count(report: &Value, key: &str) -> String {
    match report.get(key) {
        Value::Number(text) => text.clone(),
        other => panic!("{key} must be a number, got {other:?}"),
    }
}

fn declarations(report: &Value) -> &[Value] {
    report.get("declarations").as_array()
}

fn only_declaration(report: &Value) -> &Value {
    let all = declarations(report);
    assert_eq!(all.len(), 1, "expected one declaration, got {all:?}");
    &all[0]
}

/// `a.txt` = `alpha` and `keep.txt`, sealed on `heads/main`, then `prikk mv a.txt b.txt`.
fn declared_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    std::fs::write(repo.join("keep.txt"), b"keep\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "genesis seal");
    support::ok(&run(&repo, &["mv", "a.txt", "b.txt"]), "mv a.txt b.txt");
    repo
}

/// A repository whose only tracked file is `keep.txt`, plus an untracked `c.txt` declared moved to
/// `d.txt`: the declaration's source was never a node.
fn never_tracked_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("keep.txt"), b"keep\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "genesis seal");
    std::fs::write(repo.join("c.txt"), b"never committed\n").unwrap();
    support::ok(&run(&repo, &["mv", "c.txt", "d.txt"]), "mv c.txt d.txt");
    repo
}

/// `a.txt` tracked, `build/` ignored, then `prikk mv a.txt build/a.txt`: the destination is on disk
/// but excluded, which is a different cause from a deleted destination and has its own resolution.
fn ignored_destination_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    std::fs::write(repo.join(".prikkignore"), b"build/\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "genesis seal");
    std::fs::create_dir(repo.join("build")).unwrap();
    support::ok(
        &run(&repo, &["mv", "a.txt", "build/a.txt"]),
        "mv a.txt build/a.txt",
    );
    repo
}

/// One row of the handoff's table: a name, the repository it starts from, the step that reaches the
/// state, and what `commit` does with the declaration there.
struct State {
    tag: &'static str,
    fixture: fn(&str) -> PathBuf,
    reach: fn(&Path),
    resolution: &'static str,
}

/// Every state, covering all five resolutions. The first six are the handoff's own table, each a step
/// applied to `declared_repo`; the last two (0.43.0 prep, review ruling 2) start from their own
/// fixtures, because `never-tracked` and `deletion-ignored` cannot be reached from a tracked move.
fn states() -> Vec<State> {
    fn state(tag: &'static str, reach: fn(&Path), resolution: &'static str) -> State {
        State {
            tag,
            fixture: declared_repo,
            reach,
            resolution,
        }
    }
    vec![
        state("untouched", |_repo| {}, "rename"),
        state(
            "destination-edited",
            |repo| std::fs::write(repo.join("b.txt"), b"alpha, edited\n").unwrap(),
            "rename",
        ),
        state(
            "destination-deleted",
            |repo| std::fs::remove_file(repo.join("b.txt")).unwrap(),
            "deletion",
        ),
        state(
            "destination-moved-by-shell",
            |repo| std::fs::rename(repo.join("b.txt"), repo.join("c.txt")).unwrap(),
            "deletion",
        ),
        state(
            "source-recreated",
            |repo| std::fs::write(repo.join("a.txt"), b"resurrected\n").unwrap(),
            "refused",
        ),
        state(
            "moved-back-by-shell",
            |repo| std::fs::rename(repo.join("b.txt"), repo.join("a.txt")).unwrap(),
            "refused",
        ),
        State {
            tag: "source-never-tracked",
            fixture: never_tracked_repo,
            reach: |_repo| {},
            resolution: "never-tracked",
        },
        State {
            tag: "destination-ignored",
            fixture: ignored_destination_repo,
            reach: |_repo| {},
            resolution: "deletion-ignored",
        },
    ]
}

/// Control 1 (§3.1): for every state in the table, the reported resolution is what `commit` then
/// does, and a refusal's `refusal` is commit's own message byte for byte.
#[test]
fn every_resolution_matches_what_commit_then_does() {
    for State {
        tag,
        fixture,
        reach,
        resolution: expected,
    } in states()
    {
        let repo = fixture(&format!("rfc147-parity-{tag}"));
        reach(&repo);
        let report = status(&repo);
        let declaration = only_declaration(&report);
        let old_path = declaration.get("old_path").as_str().to_owned();
        let new_path = declaration.get("new_path").as_str().to_owned();
        assert_eq!(
            declaration.get("resolution").as_str(),
            expected,
            "{tag}: reported resolution"
        );

        let committed = run(&repo, &["commit", "--ref", "heads/main", "-m", tag]);
        match expected {
            "refused" => {
                assert!(
                    !committed.status.success(),
                    "{tag}: commit must refuse when the report says refused: {}",
                    stdout(&committed)
                );
                let message = stderr(&committed)
                    .trim()
                    .strip_prefix("error: precondition not met: ")
                    .map(str::to_owned)
                    .unwrap_or_else(|| panic!("{tag}: unexpected refusal: {}", stderr(&committed)));
                assert_eq!(
                    declaration.get("refusal").as_str(),
                    message,
                    "{tag}: the reported refusal must be commit's own message"
                );
            }
            "rename" => {
                support::ok(&committed, "commit");
                assert!(
                    stdout(&committed).contains(&format!("rename-path {old_path} -> {new_path}")),
                    "{tag}: commit must author the rename: {}",
                    stdout(&committed)
                );
                assert!(
                    declaration.get("refusal").is_null(),
                    "{tag}: a rename carries no refusal"
                );
            }
            _ => {
                support::ok(&committed, "commit");
                assert!(
                    stdout(&committed).contains(&format!("declaration {old_path} -> {new_path}:")),
                    "{tag}: commit must disclose what the declaration became: {}",
                    stdout(&committed)
                );
                assert!(
                    !stdout(&committed).contains("rename-path"),
                    "{tag}: and must not author a rename: {}",
                    stdout(&committed)
                );
            }
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// One way out, as the refusal states it: the files it says to delete, then either a backticked
/// `prikk mv` or `commit`.
#[derive(Debug)]
struct Route {
    deletes: Vec<String>,
    command: Option<Vec<String>>,
}

/// Read the routes out of a refusal exactly as written. Clauses are separated by `, or `; within a
/// clause, `delete <path>` is a step and a backticked `prikk mv …` is the command.
fn routes(refusal: &str) -> Vec<Route> {
    refusal
        .split(", or ")
        .filter_map(|clause| {
            let words: Vec<&str> = clause.split_whitespace().collect();
            let deletes: Vec<String> = words
                .windows(2)
                .filter(|pair| pair[0] == "delete")
                .map(|pair| pair[1].to_owned())
                .collect();
            let command = clause
                .split('`')
                .find(|part| part.starts_with("prikk mv "))
                .map(|part| part.split_whitespace().skip(1).map(str::to_owned).collect());
            let commits = clause.contains("commit to author");
            (command.is_some() || commits).then_some(Route { deletes, command })
        })
        .collect()
}

/// Control 2 (§3.2): every route a refusal names is run, in the state that produced it, and must end
/// with a commit that goes through and a file still holding the content.
///
/// This is the control the old advice fails: "move the destination back to the source" produced
/// exactly the state whose refusal said it again, which is the loop stikk reported. It also caught
/// the first version of the both-paths-exist message, which named a `prikk mv` that `mv` itself
/// refuses while both paths are there.
#[test]
fn every_route_a_refusal_names_works_in_that_state() {
    for State {
        tag,
        fixture,
        reach,
        resolution,
    } in states()
    {
        if resolution != "refused" {
            continue;
        }
        let build = |repo_tag: &str| {
            let repo = fixture(repo_tag);
            reach(&repo);
            repo
        };
        let probe = build(&format!("rfc147-routes-{tag}"));
        let refusal = only_declaration(&status(&probe))
            .get("refusal")
            .as_str()
            .to_owned();
        let _ = std::fs::remove_dir_all(&probe);
        run_named_routes(tag, &refusal, &build);
    }
}

/// Control 2b (§3.2, the third refusal): the destination is another tracked node this commit does
/// not also move. Reached by deleting the tracked destination and then declaring the move onto it.
#[test]
fn an_occupied_destination_names_a_route_that_works() {
    let build = |tag: &str| {
        let repo = support::unique_repo(tag);
        support::init(&repo);
        support::trust_maintainer(&repo);
        std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
        std::fs::write(repo.join("b.txt"), b"beta\n").unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", "genesis"),
            "genesis commit",
        );
        support::ok(&support::seal(&repo, "heads/main"), "genesis seal");
        std::fs::remove_file(repo.join("b.txt")).unwrap();
        support::ok(&run(&repo, &["mv", "a.txt", "b.txt"]), "mv a.txt b.txt");
        repo
    };

    let probe = build("rfc147-occupied");
    let report = status(&probe);
    let declaration = only_declaration(&report);
    assert_eq!(declaration.get("resolution").as_str(), "refused");
    let refusal = declaration.get("refusal").as_str().to_owned();
    assert!(
        refusal.contains("the destination is already occupied"),
        "{refusal}"
    );
    let refused = run(&probe, &["commit", "--ref", "heads/main", "-m", "occupied"]);
    assert_eq!(
        stderr(&refused).trim(),
        format!("error: precondition not met: {refusal}"),
        "commit refuses with the reported message"
    );
    let _ = std::fs::remove_dir_all(&probe);

    run_named_routes("occupied-destination", &refusal, &build);
}

/// Run every route a refusal names, in a fresh repository put back into the state that produced it.
fn run_named_routes(tag: &str, refusal: &str, build: &impl Fn(&str) -> PathBuf) {
    let routes = routes(refusal);
    assert!(
        !routes.is_empty(),
        "{tag}: the refusal must name at least one way out: {refusal}"
    );
    for (index, route) in routes.iter().enumerate() {
        let repo = build(&format!("rfc147-route-{tag}-{index}"));
        for path in &route.deletes {
            std::fs::remove_file(repo.join(path))
                .unwrap_or_else(|err| panic!("{tag}: the refusal says to delete {path}: {err}"));
        }
        if let Some(command) = &route.command {
            let args: Vec<&str> = command.iter().map(String::as_str).collect();
            support::ok(
                &run(&repo, &args),
                &format!("{tag}: the named command `prikk {}`", args.join(" ")),
            );
        }

        let report = status(&repo);
        assert_eq!(
            count(&report, "refused_declaration_count"),
            "0",
            "{tag} route {index}: no refusal may survive the route it named: {refusal}"
        );
        // A route that drops the declaration can leave the worktree matching its baseline exactly --
        // `prikk mv b.txt a.txt` in the moved-back state nets to no move -- and "nothing to commit"
        // is that route working, not failing. Anything else is not.
        let committed = run(
            &repo,
            &["commit", "--ref", "heads/main", "-m", "after the route"],
        );
        assert!(
            committed.status.success()
                || stderr(&committed).contains("worktree has no node-addressed changes to commit"),
            "{tag} route {index}: the commit after `{refusal}` failed: {}",
            stderr(&committed)
        );
        let kept = std::fs::read(repo.join("a.txt"))
            .or_else(|_| std::fs::read(repo.join("b.txt")))
            .unwrap_or_else(|err| panic!("{tag} route {index}: the file is gone: {err}"));
        assert!(!kept.is_empty(), "{tag} route {index}: the file is empty");
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// Control 3 (§3.3): a two-node swap resolves `rename` for both declarations, with no false refusal —
/// each source is back in the worktree, but as the other declaration's destination.
#[test]
fn a_two_node_swap_resolves_as_two_renames() {
    let repo = support::unique_repo("rfc147-swap");
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("x.txt"), b"ex\n").unwrap();
    std::fs::write(repo.join("y.txt"), b"why\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "genesis seal");

    support::ok(&run(&repo, &["mv", "x.txt", "tmp.txt"]), "mv x.txt tmp.txt");
    support::ok(&run(&repo, &["mv", "y.txt", "x.txt"]), "mv y.txt x.txt");
    support::ok(&run(&repo, &["mv", "tmp.txt", "y.txt"]), "mv tmp.txt y.txt");

    let report = status(&repo);
    let resolutions: Vec<&str> = declarations(&report)
        .iter()
        .map(|entry| entry.get("resolution").as_str())
        .collect();
    assert_eq!(
        resolutions,
        vec!["rename", "rename"],
        "a swap is two renames, not a refusal"
    );
    assert_eq!(count(&report, "refused_declaration_count"), "0");

    let committed = run(&repo, &["commit", "--ref", "heads/main", "-m", "swap"]);
    support::ok(&committed, "commit the swap");
    assert_eq!(
        stdout(&committed).matches("rename-path").count(),
        2,
        "commit authors both renames: {}",
        stdout(&committed)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4 (§3.4): the state stikk measured — a shell move back leaves the worktree `clean` while
/// `commit` refuses. Both truths are in the one document now.
#[test]
fn a_refused_declaration_can_sit_in_a_clean_worktree() {
    let repo = declared_repo("rfc147-clean-refusal");
    std::fs::rename(repo.join("b.txt"), repo.join("a.txt")).unwrap();
    let report = status(&repo);
    assert!(report.get("clean").as_bool(), "the worktree is clean");
    assert_eq!(count(&report, "refused_count"), "0");
    assert_eq!(count(&report, "refused_declaration_count"), "1");
    assert_eq!(
        only_declaration(&report).get("resolution").as_str(),
        "refused"
    );
    assert_eq!(
        run(&repo, &["commit", "--ref", "heads/main", "-m", "refused"])
            .status
            .code(),
        Some(1),
        "commit refuses in exactly this state"
    );
    // The prose form says the same thing, in the §2e style.
    let prose = stdout(&run(&repo, &["worktree-status"]));
    assert!(prose.contains("refused declarations: 1"), "{prose}");
    assert!(prose.contains("[refused:"), "{prose}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5 (§3.5): a rename says whether content or mode also changed — the question stikk asked,
/// because a rename and a rename-plus-edit read identically before.
#[test]
fn a_rename_says_whether_content_or_mode_also_changed() {
    let plain = declared_repo("rfc147-rename-plain");
    let report = status(&plain);
    let declaration = only_declaration(&report);
    assert!(!declaration.get("content_changed").as_bool());
    assert!(!declaration.get("mode_changed").as_bool());
    let _ = std::fs::remove_dir_all(&plain);

    let edited = declared_repo("rfc147-rename-edited");
    std::fs::write(edited.join("b.txt"), b"alpha, edited\n").unwrap();
    let report = status(&edited);
    assert!(only_declaration(&report).get("content_changed").as_bool());
    let committed = run(
        &edited,
        &["commit", "--ref", "heads/main", "-m", "rename and edit"],
    );
    support::ok(&committed, "commit");
    assert!(
        stdout(&committed).contains("rename-path") && stdout(&committed).contains("edit-text"),
        "commit authors both, which is what content_changed reports: {}",
        stdout(&committed)
    );
    let _ = std::fs::remove_dir_all(&edited);
}

/// Control 5, the mode half: Linux-gated, because it needs an observable POSIX mode.
#[cfg(target_os = "linux")]
#[test]
fn a_rename_reports_a_mode_change() {
    use std::os::unix::fs::PermissionsExt;

    let repo = declared_repo("rfc147-rename-chmod");
    let mut permissions = std::fs::metadata(repo.join("b.txt")).unwrap().permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(repo.join("b.txt"), permissions).unwrap();
    let report = status(&repo);
    let declaration = only_declaration(&report);
    assert!(declaration.get("mode_changed").as_bool());
    assert!(!declaration.get("content_changed").as_bool());
    let committed = run(
        &repo,
        &["commit", "--ref", "heads/main", "-m", "rename and chmod"],
    );
    support::ok(&committed, "commit");
    assert!(
        stdout(&committed).contains("change-perm"),
        "commit authors the mode change: {}",
        stdout(&committed)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6 (§3.6): additive. A consumer written against the 0.42.0 field set reads this document
/// unchanged — every field it knew is still there, with the same shape.
#[test]
fn the_report_is_additive_for_a_0_42_consumer() {
    let repo = declared_repo("rfc147-additive");
    let report = status(&repo);
    assert_eq!(
        report.get("schema_version").as_str(),
        "worktree-status-report-v1",
        "an additive change keeps the schema version"
    );
    for field in [
        "ref",
        "tracked_files",
        "unchanged_files",
        "clean",
        "refused_count",
        "queued_elsewhere",
        "changes",
        "declarations",
    ] {
        let _ = report.get(field);
    }
    let declaration = only_declaration(&report);
    assert_eq!(declaration.get("old_path").as_str(), "a.txt");
    assert_eq!(declaration.get("new_path").as_str(), "b.txt");
    let _ = std::fs::remove_dir_all(&repo);
}

// RFC 147 §2g/§2h: the destination's *kind* — the parity §2f never tested. Every row runs `worktree-status`
// and `commit` under a timeout, because the defect §2h fixed was a hang, and a hang must fail a test
// rather than stall the suite.

/// Run `args` in `repo`, killing the process and failing if it has not exited within `seconds`.
fn run_bounded(repo: &Path, args: &[&str], seconds: u64) -> Output {
    let mut child = support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(seconds);
    while child.try_wait().unwrap().is_none() {
        if std::time::Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "`prikk {}` did not return within {seconds} s",
                args.join(" ")
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.wait_with_output().unwrap()
}

/// `a.txt` and `keep.txt` sealed (with `ignore` as the `.prikkignore`, if any), then `prikk mv a.txt b.txt`.
fn declared_repo_ignoring(tag: &str, ignore: Option<&str>) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    std::fs::write(repo.join("a.txt"), b"alpha\n").unwrap();
    std::fs::write(repo.join("keep.txt"), b"keep\n").unwrap();
    if let Some(rule) = ignore {
        std::fs::write(repo.join(".prikkignore"), format!("{rule}\n")).unwrap();
    }
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "genesis commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "genesis seal");
    support::ok(&run(&repo, &["mv", "a.txt", "b.txt"]), "mv a.txt b.txt");
    repo
}

/// What `commit` then does, for one row.
enum CommitDoes {
    Renames,
    RecordsDeletion(&'static str),
    // Every row that reaches it is a symlink, FIFO or socket, so none exists off Unix.
    #[cfg_attr(not(unix), allow(dead_code))]
    RefusesOverThePath(&'static str),
}

/// One destination kind: how to put it there, what status must report, and what commit then does.
struct KindRow {
    tag: &'static str,
    ignore: Option<&'static str>,
    reach: fn(&Path),
    resolution: &'static str,
    /// Whether `content_changed`/`mode_changed` are booleans (`true`) or `null` (`false`).
    compared: bool,
    commit: CommitDoes,
}

fn replace_destination_with_directory(repo: &Path) {
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    std::fs::create_dir(repo.join("b.txt")).unwrap();
}

fn kind_rows() -> Vec<KindRow> {
    // Extended below on Unix only.
    #[cfg_attr(not(unix), allow(unused_mut))]
    let mut rows = vec![
        KindRow {
            tag: "regular-file",
            ignore: None,
            reach: |_repo| {},
            resolution: "rename",
            compared: true,
            commit: CommitDoes::Renames,
        },
        KindRow {
            tag: "empty-directory",
            ignore: None,
            reach: replace_destination_with_directory,
            resolution: "deletion",
            compared: false,
            commit: CommitDoes::RecordsDeletion("destination is a directory"),
        },
        KindRow {
            tag: "directory-holding-a-file",
            ignore: None,
            reach: |repo| {
                replace_destination_with_directory(repo);
                std::fs::write(repo.join("b.txt/q.txt"), b"q\n").unwrap();
            },
            resolution: "deletion",
            compared: false,
            commit: CommitDoes::RecordsDeletion("destination is a directory"),
        },
        KindRow {
            tag: "ignored-file",
            ignore: Some("b.txt"),
            reach: |_repo| {},
            resolution: "deletion-ignored",
            compared: false,
            commit: CommitDoes::RecordsDeletion("destination is ignored"),
        },
        KindRow {
            tag: "ignored-directory",
            ignore: Some("b.txt/"),
            reach: |repo| {
                replace_destination_with_directory(repo);
                std::fs::write(repo.join("b.txt/q.txt"), b"q\n").unwrap();
            },
            resolution: "deletion-ignored",
            compared: false,
            commit: CommitDoes::RecordsDeletion("destination is ignored"),
        },
    ];
    #[cfg(unix)]
    rows.extend([
        KindRow {
            tag: "symlink-to-a-file",
            ignore: None,
            reach: |repo| {
                std::fs::remove_file(repo.join("b.txt")).unwrap();
                std::os::unix::fs::symlink("keep.txt", repo.join("b.txt")).unwrap();
            },
            resolution: "rename",
            compared: false,
            commit: CommitDoes::RefusesOverThePath("worktree symlink authoring is out of scope"),
        },
        KindRow {
            tag: "dangling-symlink",
            ignore: None,
            reach: |repo| {
                std::fs::remove_file(repo.join("b.txt")).unwrap();
                std::os::unix::fs::symlink("nowhere", repo.join("b.txt")).unwrap();
            },
            resolution: "rename",
            compared: false,
            commit: CommitDoes::RefusesOverThePath("worktree symlink authoring is out of scope"),
        },
    ]);
    #[cfg(target_os = "linux")]
    rows.extend([
        KindRow {
            tag: "fifo",
            ignore: None,
            reach: make_fifo_destination,
            resolution: "rename",
            compared: false,
            commit: CommitDoes::RefusesOverThePath("worktree entry is not a regular file"),
        },
        KindRow {
            tag: "socket",
            ignore: None,
            reach: |repo| {
                std::fs::remove_file(repo.join("b.txt")).unwrap();
                drop(std::os::unix::net::UnixListener::bind(repo.join("b.txt")).unwrap());
            },
            resolution: "rename",
            compared: false,
            commit: CommitDoes::RefusesOverThePath("worktree entry is not a regular file"),
        },
    ]);
    rows
}

#[cfg(target_os = "linux")]
fn make_fifo_destination(repo: &Path) {
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(repo.join("b.txt"))
        .status()
        .unwrap();
    assert!(made.success(), "mkfifo");
}

/// §2g control 1 and §2h: for every kind of entry at the destination, `worktree-status` reports what
/// `commit` then does, and neither command hangs.
#[test]
fn every_destination_kind_resolves_as_commit_then_acts() {
    for row in kind_rows() {
        let repo = declared_repo_ignoring(&format!("rfc147-kind-{}", row.tag), row.ignore);
        (row.reach)(&repo);
        let tag = row.tag;

        let status = run_bounded(&repo, &["worktree-status", "--format", "json"], 20);
        let report = json::parse(&stdout(&status));
        let declaration = only_declaration(&report);
        assert_eq!(
            declaration.get("resolution").as_str(),
            row.resolution,
            "{tag}: reported resolution"
        );
        for field in ["content_changed", "mode_changed"] {
            assert_eq!(
                !declaration.get(field).is_null(),
                row.compared,
                "{tag}: {field} is a boolean only for a regular-file destination"
            );
        }

        let committed = run_bounded(&repo, &["commit", "--ref", "heads/main", "-m", tag], 20);
        match row.commit {
            CommitDoes::Renames => {
                support::ok(&committed, tag);
                assert!(
                    stdout(&committed).contains("rename-path a.txt -> b.txt"),
                    "{tag}: {}",
                    stdout(&committed)
                );
            }
            CommitDoes::RecordsDeletion(cause) => {
                support::ok(&committed, tag);
                let text = stdout(&committed);
                assert!(text.contains("delete-file a.txt"), "{tag}: {text}");
                assert!(
                    text.contains(&format!("declaration a.txt -> b.txt: {cause};")),
                    "{tag}: the disclosure names the cause: {text}"
                );
                assert!(!text.contains("rename-path"), "{tag}: {text}");
            }
            CommitDoes::RefusesOverThePath(reason) => {
                assert!(!committed.status.success(), "{tag}: commit must refuse");
                assert!(
                    stderr(&committed).contains(&format!("b.txt: {reason}")),
                    "{tag}: {}",
                    stderr(&committed)
                );
            }
        }
        let _ = std::fs::remove_dir_all(&repo);
    }
}

/// §2g control 2: a directory at the destination is disclosed as a directory, and the word "ignored"
/// — which is what 0.43.0 printed — appears nowhere in the commit's output.
#[test]
fn a_directory_at_the_destination_is_not_called_ignored() {
    let repo = declared_repo_ignoring("rfc147-directory-disclosure", None);
    replace_destination_with_directory(&repo);
    let committed = run_bounded(&repo, &["commit", "--ref", "heads/main", "-m", "dir"], 20);
    support::ok(&committed, "commit");
    let text = stdout(&committed);
    assert!(
        text.contains(
            "declaration a.txt -> b.txt: destination is a directory; recorded as a deletion, not a rename"
        ),
        "{text}"
    );
    assert!(!text.contains("ignored"), "{text}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// §2h: the 0.43.0 hang. `worktree-status` opened a FIFO at the destination to compare its bytes and
/// blocked forever; it must now return promptly, having never opened it.
#[cfg(target_os = "linux")]
#[test]
fn worktree_status_returns_with_a_fifo_at_the_destination() {
    let repo = declared_repo_ignoring("rfc147-fifo-hang", None);
    make_fifo_destination(&repo);
    let status = run_bounded(&repo, &["worktree-status", "--format", "json"], 10);
    let report = json::parse(&stdout(&status));
    let declaration = only_declaration(&report);
    assert_eq!(declaration.get("resolution").as_str(), "rename");
    assert!(declaration.get("content_changed").is_null());
    let _ = std::fs::remove_dir_all(&repo);
}

/// §2h: a symlink at the destination is never followed to compare content — `content_changed` is `null`,
/// where 0.43.0 compared the link's target.
#[cfg(unix)]
#[test]
fn a_symlink_at_the_destination_reports_no_content_comparison() {
    let repo = declared_repo_ignoring("rfc147-symlink-null", None);
    std::fs::remove_file(repo.join("b.txt")).unwrap();
    // Point it at different content, so a followed comparison would have said `true`.
    std::os::unix::fs::symlink("keep.txt", repo.join("b.txt")).unwrap();
    let report = status(&repo);
    let declaration = only_declaration(&report);
    assert!(
        declaration.get("content_changed").is_null(),
        "{declaration:?}"
    );
    assert!(declaration.get("mode_changed").is_null(), "{declaration:?}");
    let _ = std::fs::remove_dir_all(&repo);
}
