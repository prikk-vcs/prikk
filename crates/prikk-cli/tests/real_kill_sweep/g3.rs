// SPDX-License-Identifier: Apache-2.0
//! **0.59.0 G3: one next step after a `seal` stopped between its pointer and its ref log.** A `seal` killed at the ordinal where its RefState is in the
//! pointer and its record is not in the ref log leaves two things at once: **a pointer one transition ahead of the log, and an active queue that is
//! exactly the patches of the Block the pointer names.** `prikk seal --allow-no-audit` finishes both; `prikk ref complete` finishes the publication
//! alone and leaves `verify` at `PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING` until the `seal` runs. Before G3 `doctor`/`verify` and the writers named
//! `ref complete` and `status` named the `seal`: two first steps for one state.
//!
//! The state is built with the real-kill driver (the ordinal is **found**, not named: the sweep's own scan for the state), and every surface is asked
//! of the same directory. **Both steps need a maintainer key** (`ref complete` refuses without the maintainer seed, `--plan-only` too): the
//! sentences offer no keyless way, and a test holds that.

use super::harness::{Ctx, copy_tree, lock_files, snapshot, text};
use prikk_test_support::ptrace_sweep::{self, When};
use std::path::{Path, PathBuf};

const SEAL: &str = "prikk seal --allow-no-audit";

/// A repository whose `heads/main` has one sealed block, with a second branch `heads/other` at it, and one commit queued for `heads/main`.
fn queued(ctx: &Ctx, name: &str) -> PathBuf {
    let base = ctx.repo(name);
    std::fs::write(base.join("a.txt"), "a\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "a"]);
    ctx.ok(&base, &["seal", "--allow-no-audit"]);
    ctx.ok(&base, &["branch", "create", "heads/other"]);
    std::fs::write(base.join("b.txt"), "b\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "b"]);
    base
}

/// Every ordinal of `argv` run in a copy of `base` at which, after the kill and `unlock --yes`/`--force` of what it left, `verify` reports
/// `PRIKK-VERIFY-REF-POINTER-LEADS-LOG`: the directory of each such state. (Found by running them: nothing here names an ordinal.)
fn lead_states(ctx: &Ctx, base: &Path, argv: &[&str], tag: &str) -> Vec<(u64, PathBuf)> {
    let work = ctx.root.join(format!("g3-work-{tag}"));
    let sink = ctx.root.join(format!("g3-out-{tag}.txt"));
    let _ = std::fs::remove_dir_all(&work);
    copy_tree(base, &work);
    let dry = ptrace_sweep::trace(&ctx.prikk(&work, argv), &sink, None, When::Entry).unwrap();
    let total = dry.mutating.len() as u64;
    let mut found = Vec::new();
    for n in 1..=total {
        let _ = std::fs::remove_dir_all(&work);
        copy_tree(base, &work);
        let run =
            ptrace_sweep::trace(&ctx.prikk(&work, argv), &sink, Some(n), When::Entry).unwrap();
        assert!(
            run.killed_at.is_some(),
            "ordinal {n} of {total} was not reached"
        );
        for lock in lock_files(&work) {
            let path = lock.to_string_lossy().into_owned();
            if !ctx
                .out(&work, &["unlock", "--lock", &path, "--yes"])
                .status
                .success()
            {
                ctx.ok(&work, &["unlock", "--lock", &path, "--force"]);
            }
        }
        if text(&ctx.out(&work, &["verify"])).contains("PRIKK-VERIFY-REF-POINTER-LEADS-LOG") {
            let keep = ctx.root.join(format!("g3-state-{tag}-{n}"));
            let _ = std::fs::remove_dir_all(&keep);
            copy_tree(&work, &keep);
            found.push((n, keep));
        }
    }
    let _ = std::fs::remove_dir_all(&work);
    found
}

/// The position of `needle` in `text`, or a failure naming the surface.
fn at(surface: &str, haystack: &str, needle: &str) -> usize {
    haystack
        .find(needle)
        .unwrap_or_else(|| panic!("{surface} does not name `{needle}`:\n{haystack}"))
}

/// The surface names the `seal` before `ref complete`, and says the `seal` is still due after it.
fn names_the_seal_first(surface: &str, sentence: &str) {
    assert!(
        at(surface, sentence, SEAL) < at(surface, sentence, "prikk ref complete heads/main"),
        "{surface} names `ref complete` before the `seal`:\n{sentence}"
    );
    assert!(
        sentence.contains("still due") && sentence.contains("maintainer key"),
        "{surface} does not say the `seal` is still due, or what it needs:\n{sentence}"
    );
}

/// The first line of `haystack` that contains `marker`, or a failure naming the marker.
fn line_with(haystack: &str, marker: &str) -> String {
    haystack
        .lines()
        .find(|line| line.contains(marker))
        .unwrap_or_else(|| panic!("no line with `{marker}`:\n{haystack}"))
        .to_string()
}

fn tip(ctx: &Ctx, dir: &Path) -> String {
    let log = text(&ctx.out(dir, &["log", "--format", "json"]));
    let id = log
        .lines()
        .find_map(|l| {
            l.trim()
                .strip_prefix("\"block_id\": \"")
                .map(str::to_string)
        })
        .expect("a block in the log");
    id.trim_end_matches(['"', ',']).to_string()
}

#[test]
fn every_surface_names_the_seal_first_and_it_ends_in_verify_0() {
    let ctx = Ctx::new("g3-surfaces");
    let base = queued(&ctx, "base-g3");
    let states = lead_states(&ctx, &base, &["seal", "--allow-no-audit"], "seal");
    assert!(
        !states.is_empty(),
        "no ordinal of `seal` leaves the pointer ahead of the log"
    );
    for (n, dir) in &states {
        let at_n = format!("ordinal {n}");
        // A copy per action: every surface is asked of the same state.
        let fresh = |name: &str| {
            let copy = ctx.root.join(format!("g3-copy-{name}-{n}"));
            let _ = std::fs::remove_dir_all(&copy);
            copy_tree(dir, &copy);
            copy
        };
        // The readers.
        let verify = text(&ctx.out(&fresh("verify"), &["verify"]));
        assert!(
            verify.contains("PRIKK-VERIFY-REF-POINTER-LEADS-LOG"),
            "{at_n}: {verify}"
        );
        // Each line that carries a first step: the finding's message and the way-out line of `verify`, and the same two and the recommendation of `doctor`.
        for (surface, line) in [
            ("verify's finding", line_with(&verify, "ref-publication [")),
            ("verify's way out", line_with(&verify, "way out [")),
        ] {
            names_the_seal_first(&format!("{at_n}: {surface}"), &line);
        }
        let doctor = text(&ctx.out(&fresh("doctor"), &["doctor"]));
        for (surface, line) in [
            ("doctor's finding", line_with(&doctor, "ref-publication [")),
            ("doctor's way out", line_with(&doctor, "way out [")),
            (
                "doctor's recommendation",
                line_with(&doctor, "recommendation:"),
            ),
        ] {
            names_the_seal_first(&format!("{at_n}: {surface}"), &line);
        }
        // `status --format json`: the route is unchanged, and it is the same first step.
        let status = text(&ctx.out(&fresh("status"), &["status", "--format", "json"]));
        assert!(
            status.contains(&format!("\"route\": \"{SEAL}\"")),
            "{at_n}: status's route moved:\n{status}"
        );
        // The writers: each refuses with the same first step.
        let commit_dir = fresh("commit");
        std::fs::write(commit_dir.join("c.txt"), "c\n").unwrap();
        let commit = ctx.out(&commit_dir, &["commit", "--from-worktree", "-m", "c"]);
        assert!(!commit.status.success(), "{at_n}: commit succeeded");
        names_the_seal_first(&format!("{at_n}: commit"), &text(&commit));
        let branch = ctx.out(&fresh("branch"), &["branch", "create", "heads/x"]);
        assert!(!branch.status.success());
        names_the_seal_first(&format!("{at_n}: branch create"), &text(&branch));
        let tag = ctx.out(
            &fresh("tag"),
            &["tag", "create", "tags/t1", "--target", "heads/main"],
        );
        assert!(!tag.status.success(), "{at_n}: tag create succeeded");
        names_the_seal_first(&format!("{at_n}: tag create"), &text(&tag));
        let merge_dir = fresh("merge");
        let baseline = tip(&ctx, &merge_dir);
        let merge = ctx.out(
            &merge_dir,
            &[
                "merge",
                "--into",
                "heads/main",
                "--from",
                "heads/other",
                "--baseline-block",
                &baseline,
                "--allow-no-audit",
            ],
        );
        assert!(!merge.status.success(), "{at_n}: merge succeeded");
        names_the_seal_first(&format!("{at_n}: merge"), &text(&merge));

        // The rebuild's own refusal (0.59.0 P5a2, G3's last surface): it names the `seal` first for this state, too.
        let rebuild = ctx.out(
            &fresh("rebuild"),
            &["doctor", "--rebuild-pointer-index", "--plan-only"],
        );
        assert!(
            !rebuild.status.success(),
            "{at_n}: the rebuild ran over a completable lead"
        );
        let rebuild = text(&rebuild);
        assert!(
            rebuild.contains("a completable lead")
                && at(&format!("{at_n}: rebuild"), &rebuild, SEAL)
                    < at(
                        &format!("{at_n}: rebuild"),
                        &rebuild,
                        "prikk ref complete heads/main"
                    ),
            "{at_n}: the rebuild's refusal does not name the seal first:\n{rebuild}"
        );

        // The named command, run as printed: verify 0, the queue gone, two blocks.
        let by_seal = fresh("by-seal");
        ctx.ok(&by_seal, &["seal", "--allow-no-audit"]);
        assert_eq!(
            ctx.exit(&by_seal, &["verify"]),
            0,
            "{at_n}: verify after the seal"
        );
        // The other order: `ref complete` first leaves `verify` at 1 (the queue is still sealed into the lead), and the `seal` then ends at the same tip.
        let by_both = fresh("by-both");
        ctx.ok(&by_both, &["ref", "complete", "heads/main"]);
        assert_eq!(
            ctx.exit(&by_both, &["verify"]),
            1,
            "{at_n}: `ref complete` alone leaves the seal due"
        );
        ctx.ok(&by_both, &["seal", "--allow-no-audit"]);
        assert_eq!(
            ctx.exit(&by_both, &["verify"]),
            0,
            "{at_n}: verify after both"
        );
        assert_eq!(
            tip(&ctx, &by_seal),
            tip(&ctx, &by_both),
            "{at_n}: the same tip"
        );
    }
    let _ = std::fs::remove_dir_all(&ctx.root);
}

/// Without a maintainer seed both steps refuse, say so, and leave the state as it was; with the key back, the `seal` finishes it. The sentences
/// say what the `seal` needs, and offer no way round it (there is none: `ref complete` signs the completing record too).
#[test]
fn without_a_maintainer_key_neither_step_runs_and_the_sentence_says_what_the_seal_needs() {
    let ctx = Ctx::new("g3-keyless");
    let base = queued(&ctx, "base-g3-keyless");
    let states = lead_states(&ctx, &base, &["seal", "--allow-no-audit"], "keyless");
    let (n, dir) = states.first().expect("a lead state");
    let seed = ctx.config.join("prikk/maintainer.seed");
    let kept = ctx.root.join("maintainer.seed.away");
    std::fs::rename(&seed, &kept).expect("the maintainer seed is where setup put it");
    let before = snapshot(dir);
    for argv in [
        vec!["seal", "--allow-no-audit"],
        vec!["ref", "complete", "heads/main"],
        vec!["ref", "complete", "heads/main", "--plan-only"],
    ] {
        let out = ctx.out(dir, &argv);
        assert!(
            !out.status.success(),
            "ordinal {n}: {argv:?} ran without a key"
        );
        assert!(
            text(&out).contains("maintainer signing is required"),
            "ordinal {n}: {argv:?}: {}",
            text(&out)
        );
    }
    assert_eq!(snapshot(dir), before, "a refused step wrote nothing");
    let verify = text(&ctx.out(dir, &["verify"]));
    assert!(
        verify.contains("needs a maintainer key adopted here"),
        "verify does not say what the seal needs:\n{verify}"
    );
    std::fs::rename(&kept, &seed).unwrap();
    ctx.ok(dir, &["seal", "--allow-no-audit"]);
    assert_eq!(ctx.exit(dir, &["verify"]), 0);
    let _ = std::fs::remove_dir_all(&ctx.root);
}

/// A lead with **no queue sealed into it** still names `ref complete`, and does not name the `seal`: a `branch create` killed between its pointer and
/// its record. (The classifier asks the queue's owner and its Block, not only "is there a lead".)
#[test]
fn a_lead_with_no_matching_queue_still_names_ref_complete() {
    let ctx = Ctx::new("g3-plain");
    let base = ctx.repo("base-g3-plain");
    std::fs::write(base.join("a.txt"), "a\n").unwrap();
    ctx.ok(&base, &["commit", "--from-worktree", "-m", "a"]);
    ctx.ok(&base, &["seal", "--allow-no-audit"]);
    let states = lead_states(&ctx, &base, &["branch", "create", "heads/x"], "plain");
    assert!(
        !states.is_empty(),
        "no ordinal of `branch create` leaves a lead"
    );
    for (n, dir) in &states {
        let verify = text(&ctx.out(dir, &["verify"]));
        assert!(
            verify.contains("prikk ref complete heads/x") && !verify.contains(SEAL),
            "ordinal {n}: verify:\n{verify}"
        );
        let doctor = text(&ctx.out(dir, &["doctor"]));
        assert!(
            doctor.contains("prikk ref complete heads/x") && !doctor.contains(SEAL),
            "ordinal {n}: doctor:\n{doctor}"
        );
        let commit_dir = ctx.root.join(format!("g3-plain-commit-{n}"));
        copy_tree(dir, &commit_dir);
        std::fs::write(commit_dir.join("c.txt"), "c\n").unwrap();
        let commit = text(&ctx.out(&commit_dir, &["commit", "--from-worktree", "-m", "c"]));
        assert!(
            commit.contains("prikk ref complete heads/x") && !commit.contains(SEAL),
            "ordinal {n}: commit:\n{commit}"
        );
        let rebuild = text(&ctx.out(dir, &["doctor", "--rebuild-pointer-index", "--plan-only"]));
        assert!(
            rebuild.contains("a completable lead")
                && rebuild.contains("run `prikk ref complete <ref>` first")
                && !rebuild.contains(SEAL),
            "ordinal {n}: the rebuild's refusal for a plain lead:\n{rebuild}"
        );
        let run = ctx.out(dir, &["ref", "complete", "heads/x"]);
        assert!(run.status.success(), "{}", text(&run));
        assert_eq!(ctx.exit(dir, &["verify"]), 0);
    }
    let _ = std::fs::remove_dir_all(&ctx.root);
}
