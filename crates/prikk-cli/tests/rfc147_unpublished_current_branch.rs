//! RFC 147 §2i, and its Addendum 1: an unpublished current branch is not an absent ref, through the
//! compiled binary.
//!
//! Fixture: a fresh `init`, one untracked file, nothing sealed, `heads/main` the current branch --
//! stikk's letter 015 reproduction, reproduced by the architect on the released 0.46.0 binary before
//! this round was written. Every surface the handoff names is checked bare and with the same branch
//! named explicitly; the two answers must be byte-identical (control 1). A mistyped ref still refuses,
//! unchanged (control 2). A damaged ref stays `Integrity`, "is not published", distinct from this --
//! already covered by `rfc153_point_resolver.rs`'s own control (control 3), not repeated here. After the
//! first `seal`, the explicit and implicit forms may legitimately differ again, unaffected by this round
//! (control 4). A received ref is unaffected (control 5) -- covered by
//! `rfc132_absent_and_received_refs.rs`, not repeated here.
//!
//! **Addendum 1** ruled two follow-ups from the first round's own wording: `cat`'s parenthetical no
//! longer repeats the now-false "does not exist" reason (§2 below), and `diff --from <the unpublished
//! current branch>` does **not** fold the queue the way the implicit left side does -- control 1's
//! fixture carries no queue, so it is unaffected and stays as it is; a queue is the new
//! `explicit_from_on_the_unpublished_current_branch_does_not_fold_the_queue` control's own subject.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Output;

use support::json::{self, Value};

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn ok(repo: &Path, args: &[&str]) -> Output {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    output
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

struct Case {
    bare: &'static [&'static str],
    explicit: &'static [&'static str],
    expect_exit: i32,
}

/// The surfaces RFC 147 §2i names (handoff §3, control 1), plus their `--format json` twins where one
/// exists. `checkout --snapshot-plan`/`--patch-plan`/`--patch-delete-plan` are here too: they were never
/// specially shortcut for the implicit case either (there is genuinely no block to plan for an
/// unpublished branch), so both forms already agreed before this round -- asserted so a future edit that
/// makes them diverge is caught.
const CASES: &[Case] = &[
    Case {
        bare: &["worktree-status"],
        explicit: &["worktree-status", "--ref", "heads/main"],
        expect_exit: 1,
    },
    Case {
        bare: &["worktree-status", "--format", "json"],
        explicit: &["worktree-status", "--format", "json", "--ref", "heads/main"],
        expect_exit: 1,
    },
    Case {
        bare: &["log"],
        explicit: &["log", "--ref", "heads/main"],
        expect_exit: 0,
    },
    Case {
        bare: &["log", "--format", "json"],
        explicit: &["log", "--format", "json", "--ref", "heads/main"],
        expect_exit: 0,
    },
    Case {
        bare: &["tree"],
        explicit: &["tree", "--ref", "heads/main"],
        expect_exit: 0,
    },
    Case {
        bare: &["tree", "--format", "json"],
        explicit: &["tree", "--format", "json", "--ref", "heads/main"],
        expect_exit: 0,
    },
    Case {
        bare: &["diff"],
        explicit: &["diff", "--from", "heads/main"],
        expect_exit: 0,
    },
    // `diff --format json` is not in this list: see
    // `diff_json_agrees_up_to_the_queued_patches_field_the_schema_reserves_for_the_implicit_side` below
    // for why its one legitimate, pre-existing difference (RFC 153's own schema decision) keeps it out of
    // a byte-identical check even with an empty queue.
    Case {
        bare: &["checkout", "--plan-only"],
        explicit: &["checkout", "--plan-only", "--ref", "heads/main"],
        expect_exit: 0,
    },
    Case {
        bare: &["checkout", "--snapshot-plan"],
        explicit: &["checkout", "--snapshot-plan", "--ref", "heads/main"],
        expect_exit: 1,
    },
    Case {
        bare: &["checkout", "--patch-plan"],
        explicit: &["checkout", "--patch-plan", "--ref", "heads/main"],
        expect_exit: 1,
    },
    Case {
        bare: &["checkout", "--patch-delete-plan"],
        explicit: &["checkout", "--patch-delete-plan", "--ref", "heads/main"],
        expect_exit: 1,
    },
];

/// Control 1: every surface RFC 147 §2i names answers byte-identically bare and with an explicit
/// `--ref`/`--from` naming the same, unpublished, current branch, on a fresh repository with one
/// untracked file (the handoff's own reproduction).
///
/// *Perturb*: make the resolver refuse the current branch again (revert the `is_unpublished_current_branch`
/// check any of `worktree-status`, `log`, `tree`, `diff`, `checkout --plan-only` makes) and every case
/// above whose `expect_exit` is not already shared between the two forms goes red -- verified by hand
/// this round, not automated here, since the fix under test is the production binary itself.
#[test]
fn control1_implicit_equals_explicit_on_every_surface() {
    let repo = support::unique_repo("rfc147-unpublished-control1");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("untracked.txt"), b"hi\n").unwrap();

    for case in CASES {
        let bare = run(&repo, case.bare);
        let explicit = run(&repo, case.explicit);
        assert_eq!(
            bare.status.code(),
            Some(case.expect_exit),
            "{:?}: {}",
            case.bare,
            text(&bare)
        );
        assert_eq!(
            text(&bare),
            text(&explicit),
            "{:?} vs {:?}",
            case.bare,
            case.explicit
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// `diff --format json` is not byte-identical bare vs explicit, even with an empty queue -- and that is
/// correct, pre-existing behavior (RFC 153's diff-worktree round), not something this round changes. The
/// bare/implicit left side always carries `queued_patches` (0 or more, an explicit, always-present
/// count); the schema gives an explicit point no such field at all, ever, queue or no queue -- prose
/// happens to collapse `0 queued` and `no queue` to the same "no clause" text, but the JSON schema does
/// not, by design (`rfc153_diff_worktree.rs` pins `queued_patches: 0` appearing on a bare diff with an
/// empty queue). This asserts everything **else** in the two JSON reports agrees.
#[test]
fn diff_json_agrees_up_to_the_queued_patches_field_the_schema_reserves_for_the_implicit_side() {
    let repo = support::unique_repo("rfc147-unpublished-diff-json");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("untracked.txt"), b"hi\n").unwrap();

    let bare = json::parse(&stdout(&ok(&repo, &["diff", "--format", "json"])));
    let explicit = json::parse(&stdout(&ok(
        &repo,
        &["diff", "--from", "heads/main", "--format", "json"],
    )));
    assert_eq!(
        bare.get("from").get("point"),
        explicit.get("from").get("point")
    );
    assert_eq!(
        bare.get("from").get("target_block_id"),
        explicit.get("from").get("target_block_id")
    );
    assert_eq!(bare.get("to"), explicit.get("to"));
    assert_eq!(bare.get("entries"), explicit.get("entries"));
    assert_eq!(
        bare.get("unsupported_paths"),
        explicit.get("unsupported_paths")
    );
    assert!(
        matches!(bare.get("from"), Value::Object(map) if map.get("queued_patches") == Some(&Value::Number("0".to_string()))),
        "{bare:?}"
    );
    assert!(
        !matches!(explicit.get("from"), Value::Object(map) if map.contains_key("queued_patches")),
        "{explicit:?}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2: a mistyped ref still refuses, exit 1, with today's exact wording -- unchanged by this
/// round.
#[test]
fn control2_a_mistyped_ref_still_refuses() {
    let repo = support::unique_repo("rfc147-unpublished-control2");
    ok(&repo, &["setup", "."]);
    let expected = "precondition not met: ref heads/nope does not exist in this repository";
    for args in [
        vec!["worktree-status", "--ref", "heads/nope"],
        vec!["log", "--ref", "heads/nope"],
        vec!["tree", "--ref", "heads/nope"],
        vec!["diff", "--from", "heads/nope"],
        vec!["checkout", "--plan-only", "--ref", "heads/nope"],
        vec!["checkout", "--snapshot-plan", "--ref", "heads/nope"],
        vec!["cat", "--path", "x", "--ref", "heads/nope"],
    ] {
        let output = run(&repo, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains(expected),
            "{args:?}: {}",
            text(&output)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: after the first `seal`, the explicit and implicit forms may legitimately differ again --
/// unaffected by this round. A bare `diff` folds a queued, unsealed commit into the left side (RFC 153
/// §7.2); an explicit `--from` naming the now-published branch resolves to the sealed tip alone.
#[test]
fn control4_after_the_first_seal_nothing_changes() {
    let repo = support::unique_repo("rfc147-unpublished-control4");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), b"a\n").unwrap();
    ok(&repo, &["commit", "-m", "a"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    std::fs::write(repo.join("b.txt"), b"b\n").unwrap();
    ok(&repo, &["commit", "-m", "b"]); // queued, unsealed

    let bare = text(&ok(&repo, &["diff"]));
    let explicit = text(&ok(&repo, &["diff", "--from", "heads/main"]));
    assert!(
        bare.contains("plus 1 queued commit not yet sealed"),
        "{bare}"
    );
    assert!(
        !explicit.contains("queued"),
        "an explicit --from on a published branch does not fold the queue: {explicit}"
    );
    assert_ne!(bare, explicit);
    let _ = std::fs::remove_dir_all(&repo);
}

/// §2 (Addendum 1): `cat --path <p> --ref <current-branch>` on a fresh repository refuses with the
/// absence of the *path*, not the ref -- named explicitly or not, and the parenthetical gives the
/// accurate reason (nothing published yet), never the resolver's own "does not exist", which this round
/// rules false for the current branch.
#[test]
fn cat_names_the_absent_path_not_the_ref() {
    let repo = support::unique_repo("rfc147-unpublished-cat");
    ok(&repo, &["setup", "."]);
    let expected = "precondition not met: path x.txt does not exist at heads/main (heads/main has no \
                    published history yet)";
    for args in [
        vec!["cat", "--path", "x.txt"],
        vec!["cat", "--path", "x.txt", "--ref", "heads/main"],
    ] {
        let output = run(&repo, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains(expected),
            "{args:?}: {}",
            text(&output)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Addendum 1: `--from <the unpublished current branch>` compares against the empty state exactly --
/// unlike the implicit left side, it does **not** fold a queued, unsealed commit. `tree`/`cat` agree the
/// point holds nothing; `diff --from heads/main` must agree with them, not with the bare form.
#[test]
fn explicit_from_on_the_unpublished_current_branch_does_not_fold_the_queue() {
    let repo = support::unique_repo("rfc147-unpublished-diff-queue");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), b"a\n").unwrap();
    ok(&repo, &["commit", "-m", "a"]); // queued, unsealed, heads/main still unpublished
    std::fs::write(repo.join("b.txt"), b"b\n").unwrap();

    // tree and cat already say the point holds nothing.
    let listing = json::parse(&stdout(&ok(
        &repo,
        &["tree", "--ref", "heads/main", "--format", "json"],
    )));
    assert!(listing.get("entries").as_array().is_empty());
    let cat = run(&repo, &["cat", "--path", "a.txt", "--ref", "heads/main"]);
    assert_eq!(cat.status.code(), Some(1), "{}", text(&cat));
    assert!(
        text(&cat).contains("path a.txt does not exist at heads/main"),
        "{}",
        text(&cat)
    );

    let bare = text(&ok(&repo, &["diff"]));
    let explicit = text(&ok(&repo, &["diff", "--from", "heads/main"]));
    assert!(
        bare.contains("plus 1 queued commit not yet sealed") && bare.contains("added b.txt"),
        "{bare}"
    );
    assert!(!bare.contains("added a.txt"), "{bare}");
    assert!(
        explicit.contains("(not published: the empty state)")
            && !explicit.contains("queued")
            && explicit.contains("added a.txt")
            && explicit.contains("added b.txt"),
        "{explicit}"
    );

    let explicit_json = json::parse(&stdout(&ok(
        &repo,
        &["diff", "--from", "heads/main", "--format", "json"],
    )));
    assert!(
        !matches!(explicit_json.get("from"), Value::Object(map) if map.contains_key("queued_patches")),
        "an explicit point carries no queued_patches field"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
