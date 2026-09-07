//! RFC 111 Stage 2 §1 -- the cost gate for `seal`.
//!
//! Asserts that sealing one more commit performs a number of full object-index decodes that does not
//! grow with how much history already exists. **Required to FAIL on current `main`**: `seal` (via
//! `derive_next_state_root`'s ancestor-lineage walk) reads O(N) objects to seal one more block, and
//! today every one of those reads decodes the whole index fresh (RFC 111 §1). Stage 2's fix (migrating
//! `seal` and ref publication to `ObjectWriteSession`) turns this green.
//!
//! **What is gated, and why not the CLI path (RFC 111 Stage 2 handoff §1's own question).**
//! `crates/prikk-cli` is a binary-only crate -- no `src/lib.rs`, confirmed by its absence and by
//! `Cargo.toml` carrying no `[lib]` section -- so a `prikk-cli` integration test can only drive `seal`
//! as a compiled subprocess. A subprocess cannot observe this crate's in-process, `#[cfg(test)]`-only
//! decode counter (`index::replay_index_decode_count_for_test`): the counter lives in a different
//! process's memory. Making it observable across that boundary (a debug log, an env-gated dump) would
//! be new machinery built only for this gate, and a wall-clock proxy through the subprocess is exactly
//! what the handoff rules out (`seal` is dominated by `fsync`, not decode cost, at small N). The gate is
//! therefore built at the store level: `simulate_one_seal` (`crate::rfc111_seal_simulation`) reproduces
//! `seal_active_no_audit`'s own "new content" sequence (`crates/prikk-cli/src/seal.rs`, lines 123-229 as
//! of RFC 111 Stage 2), object read and write for object read and write, so that what this gate measures
//! is what `seal` actually does -- not something adjacent to it. One thing is deliberately left out: the
//! idempotent "tip already matches the WAL" shortcut, never reached here since `simulate_one_seal` is
//! always called immediately after a real commit whose WAL record cannot yet match any existing tip.
//! `verify_signer_trusted`'s own explicit pre-check *is* skipped as redundant, not as out of scope --
//! `finish_interrupted_publication` performs its own trust verification internally
//! (`refs/evidence.rs::validate_signer_backed_recovery`, found empirically: this gate's first draft
//! assumed trust checking was confined to the explicit pre-check and failed with "publication trust
//! policy is missing or unreadable" until a real adopted maintainer key was wired in), so a real,
//! adopted `Ed25519MaintainerSigner` is required regardless of whether the pre-check itself runs.
//! **If `seal.rs`'s own sequence changes, `simulate_one_seal` must be updated to match, or the gate
//! silently stops covering `seal`.**
//!
//! **Replica-fidelity risk, and how it is now bounded (Stage 2 gate review C1).** This gate's own
//! decode counter proves nothing about `simulate_one_seal`'s fidelity to `seal.rs` -- the two could
//! silently diverge and the count would still look right. Discharged by a separate drift guard,
//! `crates/prikk-cli/tests/rfc111_seal_drift_guard.rs`: it runs the real `prikk seal` binary against a
//! fixture and `simulate_one_seal` (exposed cross-crate via `crate::rfc111_seal_simulation`'s
//! `test-support`-gated wrapper) against an identical one, and asserts the two resulting repositories
//! agree on object ids and ref state. `simulate_one_seal` now has exactly one implementation, called by
//! both this gate and that drift guard, so the two cannot drift from *each other* -- only from `seal.rs`
//! itself, which is what the drift guard checks.

use prikk_error::Result;

use crate::foundation::index::{
    replay_index_decode_count_for_test, reset_replay_index_decode_count_for_test,
};
use crate::rfc111_seal_simulation::simulate_one_seal;
use crate::test_gates::test_support::unique_temp_dir;
use crate::worktree_patch::{WorktreePatchCommitOptions, commit_worktree_changes_signed};
use crate::{
    Ed25519AuthorSigner, Ed25519MaintainerSigner, MaintainerSigner, RepositoryLayout,
    add_trusted_maintainer,
};

const REF_NAME: &str = "heads/main";

fn maintainer_signer() -> Result<Ed25519MaintainerSigner> {
    Ed25519MaintainerSigner::from_seed("rfc111-seal-gate-maintainer", &[0x72; 32])
}

fn author_signer() -> Result<Ed25519AuthorSigner> {
    Ed25519AuthorSigner::from_seed("rfc111-seal-gate-author", &[0x71; 32])
}

/// One commit-then-seal generation: writes one new file (so each generation authors real,
/// replayable content, matching RFC 111's own Stage 0 gate discipline), commits it, then seals via
/// [`simulate_one_seal`].
fn commit_and_seal(
    layout: &RepositoryLayout,
    index: usize,
    maintainer: &Ed25519MaintainerSigner,
) -> Result<()> {
    let signer = author_signer()?;
    let path = format!("f{index}.txt");
    std::fs::write(layout.root().join(&path), format!("{path}\n").into_bytes())?;
    commit_worktree_changes_signed(
        layout,
        REF_NAME,
        "rfc111-seal-gate",
        WorktreePatchCommitOptions::default(),
        &signer,
    )?;
    simulate_one_seal(layout, REF_NAME, maintainer)?;
    Ok(())
}

/// Builds a repository with `sealed_block_count` prior sealed blocks (genesis plus that many further
/// commit-then-seal generations), then measures the index-decode cost of exactly **one more**
/// commit-then-seal at that depth.
fn measure_replay_index_decodes_for_one_more_seal(sealed_block_count: usize) -> Result<usize> {
    let root = unique_temp_dir(&format!(
        "rfc111-seal-cost-gate-{sealed_block_count}-blocks"
    ));
    let layout = RepositoryLayout::init(root.clone())?;
    let maintainer = maintainer_signer()?;
    add_trusted_maintainer(
        &layout,
        maintainer.key_id(),
        &prikk_hash::to_hex(&maintainer.public_key_bytes()),
    )?;

    for index in 0..sealed_block_count {
        commit_and_seal(&layout, index, &maintainer)?;
    }

    reset_replay_index_decode_count_for_test();
    commit_and_seal(&layout, sealed_block_count, &maintainer)?;
    let decode_count = replay_index_decode_count_for_test();

    let _ = std::fs::remove_dir_all(root);
    Ok(decode_count)
}

/// **This test is required to FAIL on current `main`** (RFC 111 Stage 2 handoff §1: "build a second
/// gate before the fix... it must FAIL on current main"). It passes once Stage 2's fix (migrating
/// `seal` and ref publication to `ObjectWriteSession`) lands.
#[test]
fn seal_index_decode_count_does_not_grow_with_prior_history() -> Result<()> {
    const SMALL: usize = 4;
    const LARGE: usize = 16;

    let small_decodes = measure_replay_index_decodes_for_one_more_seal(SMALL)?;
    let large_decodes = measure_replay_index_decodes_for_one_more_seal(LARGE)?;

    assert_eq!(
        small_decodes,
        large_decodes,
        "sealing one more commit's full-index-decode count must not grow with how much history \
         already exists (RFC 111 Stage 2 handoff §1), but sealing the {}th block after a \
         {SMALL}-block history triggered {small_decodes} decode(s) while sealing the {}th block \
         after a {LARGE}-block history triggered {large_decodes} -- proportional to prior history, \
         exactly the O(N)-per-seal-read regression RFC 111 exists to close (`derive_next_state_root`'s \
         ancestor walk, each read costing a full index decode)",
        SMALL + 1,
        LARGE + 1
    );
    Ok(())
}
