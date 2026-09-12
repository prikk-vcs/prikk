//! Trust-gate caller-level coverage (`trust-gate-caller-coverage-handoff-v1.md` §2, `prikk seal`).
//!
//! `verify_signer_trusted` is unit-tested for both outcomes (`trust/tests.rs:240,424`), and those unit
//! tests passed the entire time `prikk tag create` was ungated (`053e442`) -- they prove the function
//! works, not that any caller reaches it. `seal` is the model DC-63 told every other publishing surface
//! to follow, and until this test existed it was the least-proven of them: caller-level proof existed
//! only for the three surfaces `053e442` itself added.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

/// The untrusted-but-well-formed seed shared with `dc60_branch_management.rs`,
/// `dc61_branch_closure.rs`, and `dc63_tag_surface.rs` -- a valid 64-hex/32-byte seed, so the refusal
/// below is for being untrusted, not malformed.
const UNTRUSTED_MAINTAINER_SEED: &str =
    "222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111";

#[test]
fn seal_fails_closed_on_untrusted_signer() {
    let repo = support::unique_repo("seal-untrusted-signer");
    support::init(&repo);
    std::fs::write(repo.join("state.txt"), b"seal trust gate\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "commit heads/main",
    );
    // A *different* key is trusted first, so the policy is non-empty and the refusal below is a
    // policy decision (untrusted), not a missing-policy accident.
    support::trust_maintainer(&repo);
    // Deliberately not `support::seal`, which always trusts the shared fixed key first -- this is a
    // genesis seal (no prior publication on this ref), so the untrusted signer is checked at
    // `seal.rs:151`, the ordinary new-publication path.
    let out = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(UNTRUSTED_MAINTAINER_SEED),
        )
        .args(["seal", "--allow-no-audit"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "seal with an untrusted maintainer signer unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
