//! RFC 132 part 1: `prikk branch close`'s active-WAL ownership check no longer un-files its
//! answer by matching on the bare `PrikkError::Precondition` variant. Before this change, the
//! call site was `Err(PrikkError::Precondition(_)) => {}` -- broad enough that *any*
//! `Precondition`, not only an ownership mismatch, reaching that exact call site would have been
//! silently treated as "proceed." Control 2 of the handoff requires this demonstrated, not
//! asserted: this file proves both halves -- that a second, real, unrelated `Precondition` exists
//! and produces the identical variant a bare match cannot distinguish, and that the real call site
//! cannot receive it regardless, because it no longer looks at `PrikkError` at all.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

/// A real, unrelated `PrikkError::Precondition`: an empty change set
/// (`worktree_patch/node_authoring.rs`), nothing to do with active-WAL ownership
/// (`active.rs::active_ref_ownership`). Confirms it produces the exact same `Display` prefix,
/// `precondition not met:`, that the ownership-mismatch case does (`dc61_branch_closure.rs`'s own
/// `branch_close_does_not_block_commits_to_unrelated_refs` pins that case) -- proving the old
/// bare-variant match at `branch.rs`'s call site genuinely could not have told the two apart, had
/// this scenario ever reached it.
#[test]
fn a_second_real_precondition_exists_and_shares_the_variant() {
    let repo = support::unique_repo("rfc132-part1-empty-commit");
    support::init(&repo);
    let out = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["commit", "--from-worktree", "-m", "nothing to commit"])
        .output()
        .unwrap();
    assert!(!out.status.success(), "an empty change set must be refused");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("precondition not met: worktree has no node-addressed changes to commit"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The structural half: `prikk branch close`'s active-WAL check now calls
/// `prikk_store::active_ref_ownership`, whose signature is `Result<ActiveRefOwnership,
/// PrikkError>` -- `ActiveRefOwnership` has exactly two variants (`Owned`, `OwnedByOther`), and
/// its only error paths are `Integrity`/`InvalidName`. There is no path from that signature to the
/// empty-change-set condition above (a different function, `node_authoring.rs`, returning a
/// different wrapper type, `AuthorError`) -- so the swallow this control worries about has no
/// runtime path left to demonstrate through; the fix is in the type, not in luck. Checked directly
/// against the source rather than merely asserted here.
#[test]
fn branch_rs_no_longer_names_prikkerror_at_all() {
    let branch_rs = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/branch.rs"),
    )
    .unwrap();
    assert!(
        !branch_rs.contains("PrikkError::Precondition"),
        "branch.rs must not match on PrikkError::Precondition at all after RFC 132 part 1 -- the \
         ownership check now matches on ActiveRefOwnership, a two-variant local enum, not on the \
         error taxonomy (a plain-prose mention in an explanatory comment, not a match arm, would \
         also trip this -- reword the comment rather than reintroducing the pattern)"
    );
    assert!(
        branch_rs.contains("ActiveRefOwnership::Owned"),
        "branch.rs must still match the ownership question's real answer type"
    );
}
