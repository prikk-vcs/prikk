//! RFC 147 §2i: an unpublished current branch is not an absent ref, through the compiled binary.
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

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Output;

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn ok(repo: &Path, args: &[&str]) -> Output {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    output
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
    Case {
        bare: &["diff", "--format", "json"],
        explicit: &["diff", "--format", "json", "--from", "heads/main"],
        expect_exit: 0,
    },
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

/// §2: `cat --path <p> --ref <current-branch>` on a fresh repository refuses with the absence of the
/// *path*, not the ref -- named explicitly or not.
#[test]
fn cat_names_the_absent_path_not_the_ref() {
    let repo = support::unique_repo("rfc147-unpublished-cat");
    ok(&repo, &["setup", "."]);
    for args in [
        vec!["cat", "--path", "x.txt"],
        vec!["cat", "--path", "x.txt", "--ref", "heads/main"],
    ] {
        let output = run(&repo, &args);
        assert_eq!(output.status.code(), Some(1), "{args:?}: {}", text(&output));
        assert!(
            text(&output).contains("precondition not met: path x.txt does not exist at heads/main"),
            "{args:?}: {}",
            text(&output)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}
