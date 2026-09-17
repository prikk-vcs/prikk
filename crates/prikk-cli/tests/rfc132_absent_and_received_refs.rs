//! RFC 132 refusal sweep (`rfcs/handoffs/132-error-taxonomy-structure/absent-and-received-ref-refusals-handoff-v1.md`,
//! Addendum 2), through the compiled binary.
//!
//! Fixture `src`: a set-up repository with two sealed blocks on `heads/main` (the tip is not a checkpoint),
//! which imported its own bundle, so it holds both `heads/main` and `remotes/heads/main`. Fixture `dst`: a
//! fresh `init` that imported `src`'s bundle, so it holds `remotes/heads/main` and no `heads/main`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

const ABSENT: &str = "precondition not met: ref heads/nope does not exist in this repository";
const RECEIVED: &str = "precondition not met: remotes/heads/main is a received ref, and this command does \
     not accept received refs; received refs are read by `prikk log`, `prikk merge-evidence`, `prikk \
     merge-plan` and `prikk bundle preview`, and taken into a local branch by `prikk merge --from`";

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn ok(repo: &Path, args: &[&str]) -> String {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn tip_block(repo: &Path, reference: &str) -> String {
    ok(repo, &["log", "--ref", reference, "--limit", "1"])
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .expect("log names a block")
        .trim()
        .to_string()
}

struct Fixture {
    src: PathBuf,
    dst: PathBuf,
    bundle: PathBuf,
    base: String,
}

fn fixture(tag: &str) -> Fixture {
    let src = support::unique_repo(&format!("{tag}-src"));
    ok(&src, &["setup", "."]);
    std::fs::write(src.join("a.txt"), "a\n").unwrap();
    ok(&src, &["commit", "-m", "a"]);
    ok(&src, &["seal", "--allow-no-audit"]);
    let base = tip_block(&src, "heads/main");
    std::fs::write(src.join("b.txt"), "b\n").unwrap();
    ok(&src, &["commit", "-m", "b"]);
    ok(&src, &["seal", "--allow-no-audit"]);
    let bundle = src.join("main.bundle");
    ok(
        &src,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    ok(
        &src,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    let dst = support::unique_repo(&format!("{tag}-dst"));
    ok(&dst, &["init"]);
    ok(
        &dst,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    Fixture {
        src,
        dst,
        bundle,
        base,
    }
}

fn assert_refusal(repo: &Path, args: &[&str], expected: &str) {
    let output = run(repo, args);
    assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
    assert!(
        text(&output).contains(expected),
        "{args:?} must say {expected:?}, said: {}",
        text(&output)
    );
}

/// Control 1: every consumer × {absent, received}: class prefix, exit code and wording from the one resolver.
#[test]
fn control1_every_consumer_refuses_an_absent_ref_and_where_ruled_a_received_ref() {
    let f = fixture("refusals-c1");
    let out = f.src.join("out.bundle");
    let out = out.to_str().unwrap();
    let base = f.base.as_str();
    let absent: Vec<Vec<&str>> = vec![
        vec!["log", "--ref", "heads/nope"],
        vec!["log", "--ref", "heads/nope", "--format", "json"],
        vec!["worktree-status", "--ref", "heads/nope"],
        vec!["worktree-status", "--ref", "heads/nope", "--format", "json"],
        vec!["checkout", "--plan-only", "--ref", "heads/nope"],
        vec!["checkout", "--snapshot-plan", "--ref", "heads/nope"],
        vec!["checkout", "--snapshot-materialize", "--ref", "heads/nope"],
        vec!["checkout", "--patch-plan", "--ref", "heads/nope"],
        vec!["checkout", "--patch-materialize", "--ref", "heads/nope"],
        vec!["checkout", "--patch-delete-plan", "--ref", "heads/nope"],
        vec!["inverse-plan", "--ref", "heads/nope"],
        vec!["rollback-preview", "--ref", "heads/nope"],
        vec![
            "rollback-draft",
            "--append-inverse",
            "--ref",
            "heads/nope",
            "-m",
            "x",
        ],
        vec!["rollback-draft-verify", "--ref", "heads/nope"],
        vec![
            "merge-evidence",
            "--baseline-block",
            base,
            "--left-ref",
            "heads/nope",
            "--right-ref",
            "heads/main",
        ],
        vec![
            "merge-plan",
            "--baseline-block",
            base,
            "--left-ref",
            "heads/main",
            "--right-ref",
            "heads/nope",
        ],
        vec![
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            base,
            "--into",
            "heads/nope",
            "--from",
            "heads/main",
        ],
        vec![
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            base,
            "--into",
            "heads/main",
            "--from",
            "heads/nope",
        ],
        vec!["bundle", "export", "--ref", "heads/nope", "--output", out],
        vec!["branch", "create", "heads/x", "--from", "heads/nope"],
        vec!["tag", "create", "tags/x", "--target", "heads/nope"],
        vec!["branch", "close", "heads/nope"],
    ];
    for args in &absent {
        assert_refusal(&f.src, args, ABSENT);
    }
    let received: Vec<Vec<&str>> = vec![
        vec!["checkout", "--plan-only", "--ref", "remotes/heads/main"],
        vec!["checkout", "--snapshot-plan", "--ref", "remotes/heads/main"],
        vec![
            "checkout",
            "--snapshot-materialize",
            "--ref",
            "remotes/heads/main",
        ],
        vec!["checkout", "--patch-plan", "--ref", "remotes/heads/main"],
        vec![
            "checkout",
            "--patch-materialize",
            "--ref",
            "remotes/heads/main",
        ],
        vec![
            "checkout",
            "--patch-delete-plan",
            "--ref",
            "remotes/heads/main",
        ],
        vec!["inverse-plan", "--ref", "remotes/heads/main"],
        vec!["rollback-preview", "--ref", "remotes/heads/main"],
        vec![
            "bundle",
            "export",
            "--ref",
            "remotes/heads/main",
            "--output",
            out,
        ],
        vec![
            "branch",
            "create",
            "heads/y",
            "--from",
            "remotes/heads/main",
        ],
        vec!["tag", "create", "tags/y", "--target", "remotes/heads/main"],
    ];
    for args in &received {
        assert_refusal(&f.src, args, RECEIVED);
    }
    // Received names left to name validation keep their true `invalid name` refusal (ruling 3).
    for args in [
        vec!["worktree-status", "--ref", "remotes/heads/main"],
        vec![
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            base,
            "--into",
            "remotes/heads/main",
            "--from",
            "heads/main",
        ],
        vec!["branch", "close", "remotes/heads/main"],
    ] {
        assert_refusal(
            &f.src,
            &args,
            "invalid name: ref namespace is reserved: remotes/heads/main",
        );
    }
}

/// Control 2: the commands a refusal names are run from the refusing state, and succeed. The named set is
/// read from the refusal itself, so a sentence naming a command that does not read a received ref fails here.
#[test]
fn control2_every_command_a_refusal_names_runs() {
    let f = fixture("refusals-c2");
    // An absent local name whose `remotes/` twin exists says so, with the same factual list.
    let refused = run(&f.dst, &["checkout", "--patch-plan", "--ref", "heads/main"]);
    let message = text(&refused);
    assert!(
        message.contains("ref heads/main does not exist in this repository; remotes/heads/main exists: received refs are read by"),
        "{message}"
    );
    let named: Vec<String> = message
        .split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert_eq!(
        named,
        [
            "prikk log",
            "prikk merge-evidence",
            "prikk merge-plan",
            "prikk bundle preview",
            "prikk merge --from"
        ],
        "{message}"
    );
    let tip = tip_block(&f.dst, "remotes/heads/main");
    ok(&f.dst, &["log", "--ref", "remotes/heads/main"]);
    ok(
        &f.dst,
        &[
            "merge-evidence",
            "--baseline-block",
            &tip,
            "--left-block",
            &tip,
            "--right-ref",
            "remotes/heads/main",
        ],
    );
    ok(
        &f.dst,
        &[
            "merge-plan",
            "--baseline-block",
            &tip,
            "--left-block",
            &tip,
            "--right-ref",
            "remotes/heads/main",
        ],
    );
    ok(
        &f.dst,
        &[
            "bundle",
            "preview",
            "--input",
            f.bundle.to_str().unwrap(),
            "--ref",
            "remotes/heads/main",
        ],
    );
    // `merge --from`: into a local branch at the base, taking the received history.
    ok(&f.src, &["tag", "create", "tags/base", "--target", &f.base]);
    ok(
        &f.src,
        &["branch", "create", "heads/topic", "--from", "tags/base"],
    );
    ok(
        &f.src,
        &[
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &f.base,
            "--into",
            "heads/topic",
            "--from",
            "remotes/heads/main",
        ],
    );
    // `branch switch` keeps its own route sentence, and that route runs.
    let switch = run(&f.src, &["branch", "switch", "heads/nope"]);
    assert!(
        text(&switch).contains("run `prikk branch create heads/nope` first"),
        "{}",
        text(&switch)
    );
    ok(
        &f.src,
        &["branch", "create", "heads/nope", "--from", "heads/main"],
    );
    ok(&f.src, &["branch", "switch", "heads/nope"]);
}

/// Control 3: `checkout --snapshot-plan` decides absent and received before "not a checkpoint"; a local
/// non-checkpoint target still gets the checkpoint message.
#[test]
fn control3_snapshot_checkout_decides_existence_before_kind() {
    let f = fixture("refusals-c3");
    assert_refusal(
        &f.src,
        &["checkout", "--snapshot-plan", "--ref", "heads/nope"],
        ABSENT,
    );
    assert_refusal(
        &f.src,
        &["checkout", "--snapshot-plan", "--ref", "remotes/heads/main"],
        RECEIVED,
    );
    assert_refusal(
        &f.src,
        &["checkout", "--snapshot-plan", "--ref", "heads/main"],
        "precondition not met: checkout target for heads/main is not a checkpoint",
    );
}

/// Control 4: an explicit absent ref refuses in `log` and `worktree-status`; a fresh repository with no
/// `--ref` keeps today's answers byte for byte.
#[test]
fn control4_an_explicit_absent_ref_refuses_and_the_implicit_fresh_branch_does_not() {
    let f = fixture("refusals-c4");
    for args in [
        vec!["log", "--ref", "heads/nope"],
        vec!["log", "--ref", "heads/nope", "--format", "json"],
        vec!["worktree-status", "--ref", "heads/nope"],
        vec!["worktree-status", "--ref", "heads/nope", "--format", "json"],
    ] {
        assert_refusal(&f.src, &args, ABSENT);
    }
    let fresh = support::unique_repo("refusals-c4-fresh");
    ok(&fresh, &["init"]);
    let prikk_dir = fresh.join(".prikk").display().to_string();
    assert_eq!(
        ok(&fresh, &["log"]),
        format!(
            "history repository: {prikk_dir}\nref: heads/main\ncurrent branch: heads/main\nhistory: <empty>\n"
        )
    );
    assert_eq!(
        ok(&fresh, &["log", "--format", "json"]),
        format!(
            "{{\n  \"schema_version\": \"log-report-v1\",\n  \"repository\": \"{prikk_dir}\",\n  \"ref\": \"heads/main\",\n  \"current_branch\": \"heads/main\",\n  \"blocks\": []\n}}\n"
        )
    );
    let status = ok(&fresh, &["worktree-status"]);
    assert!(
        status.starts_with(&format!(
            "worktree-status repository: {prikk_dir}\nref: heads/main\n"
        )) && status.contains("worktree: clean against baseline\n"),
        "{status}"
    );
    let json = ok(&fresh, &["worktree-status", "--format", "json"]);
    assert!(
        json.contains("\"clean\": true") && json.contains("\"changes\": []"),
        "{json}"
    );
}

/// Control 5: a path with no repository is `no prikk repository at <path>`, in every command tried; a real
/// I/O failure inside an existing repository stays `Io`.
#[test]
fn control5_a_path_with_no_repository_is_a_precondition() {
    let empty = support::unique_repo("refusals-c5-empty");
    let expected = format!(
        "precondition not met: no prikk repository at {}",
        empty.display()
    );
    for args in [
        vec!["verify"],
        vec!["format", "upgrade"],
        vec!["status"],
        vec!["log"],
        vec!["doctor"],
        vec!["worktree-status"],
    ] {
        assert_refusal(&empty, &args, &expected);
    }
    let parent = support::unique_repo("refusals-c5-parent");
    let missing = parent.join("missing");
    assert_refusal(
        &parent,
        &["verify", missing.to_str().unwrap()],
        &format!(
            "precondition not met: no prikk repository at {}",
            missing.display()
        ),
    );
}

/// Control 5, the other half (Unix: the fixture needs a permission bit): an I/O failure inside a real
/// repository is still `Io`, not "no repository".
#[cfg(unix)]
#[test]
fn control5_an_io_failure_inside_a_repository_stays_io() {
    use std::os::unix::fs::PermissionsExt;

    let repo = support::unique_repo("refusals-c5-io");
    ok(&repo, &["init"]);
    let prikk = repo.join(".prikk");
    std::fs::set_permissions(&prikk, std::fs::Permissions::from_mode(0o000)).unwrap();
    let output = run(&repo, &["verify"]);
    std::fs::set_permissions(&prikk, std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains("error: i/o error:"),
        "{}",
        text(&output)
    );
    assert!(
        !text(&output).contains("no prikk repository"),
        "{}",
        text(&output)
    );
}

/// Control 6: the five received-ref consumers that worked keep working, and the non-consumers keep
/// answering an absent ref as a state.
#[test]
fn control6_received_readers_and_non_consumers_are_unchanged() {
    let f = fixture("refusals-c6");
    ok(&f.src, &["log", "--ref", "remotes/heads/main"]);
    ok(
        &f.src,
        &[
            "merge-evidence",
            "--baseline-block",
            &f.base,
            "--left-ref",
            "heads/main",
            "--right-ref",
            "remotes/heads/main",
        ],
    );
    ok(
        &f.src,
        &[
            "merge-plan",
            "--baseline-block",
            &f.base,
            "--left-ref",
            "remotes/heads/main",
            "--right-ref",
            "heads/main",
        ],
    );
    ok(
        &f.src,
        &[
            "bundle",
            "preview",
            "--input",
            f.bundle.to_str().unwrap(),
            "--ref",
            "remotes/heads/main",
        ],
    );
    ok(&f.src, &["tag", "create", "tags/base", "--target", &f.base]);
    ok(
        &f.src,
        &["branch", "create", "heads/topic", "--from", "tags/base"],
    );
    ok(
        &f.src,
        &[
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &f.base,
            "--into",
            "heads/topic",
            "--from",
            "remotes/heads/main",
        ],
    );

    let have = f.src.join("have.bin");
    let written = ok(
        &f.src,
        &[
            "sync",
            "have",
            "heads/nope",
            "--output",
            have.to_str().unwrap(),
        ],
    );
    assert!(
        written.contains("have-list for heads/nope") && have.exists(),
        "{written}"
    );
    let preview = ok(
        &f.src,
        &[
            "bundle",
            "preview",
            "--input",
            f.bundle.to_str().unwrap(),
            "--ref",
            "heads/nope",
        ],
    );
    assert!(
        preview.contains("connectivity: no-local-history"),
        "{preview}"
    );
}

/// Addendum 3: `checkout --plan-only` on the implicit current branch of a fresh repository keeps its
/// pre-sweep answer byte for byte, exit 0 (the expected text was captured from the `63d0dcee` binary), while
/// an explicit absent `--ref` refuses and every other implicit mode gets the rule-1 wording.
#[test]
fn addendum3_implicit_plan_only_keeps_its_answer_in_a_fresh_repository() {
    let fresh = support::unique_repo("refusals-a3-fresh");
    ok(&fresh, &["init"]);
    let prikk_dir = fresh.join(".prikk").display().to_string();
    assert_eq!(
        ok(&fresh, &["checkout", "--plan-only"]),
        format!(
            "checkout plan repository: {prikk_dir}\nref: heads/main\nref-state: <not published>\ntarget block: <none>\nblock kind: <none>\nparents: 0\npatches: 0\nsnapshot blob: <none>\nmaterialization: unpublished-ref\nnote: publish a ref before checkout can target a block\n"
        )
    );
    assert_refusal(
        &fresh,
        &["checkout", "--plan-only", "--ref", "heads/nope"],
        ABSENT,
    );
    for mode in [
        "--snapshot-plan",
        "--snapshot-materialize",
        "--patch-plan",
        "--patch-materialize",
        "--patch-delete-plan",
        "--patch-materialize-delete",
    ] {
        assert_refusal(
            &fresh,
            &["checkout", mode],
            "precondition not met: ref heads/main does not exist in this repository",
        );
    }
}
