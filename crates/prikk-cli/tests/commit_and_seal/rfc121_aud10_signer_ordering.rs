//! AUD-10: `seal` and `merge` must refuse a bad argument before building the maintainer signer
//! from the environment. `main.rs::run_seal`/`run_merge` used to call
//! `maintainer_signer_from_env()` before parsing, so a mistyped flag with no signing key configured
//! reported "maintainer signing is required" instead of the usage error -- masking the real
//! mistake behind an unrelated one. Neither test sets `PRIKK_MAINTAINER_KEY_ID`/`_SEED`: if parsing
//! ever regresses back to running after signer acquisition, these fail on the wrong exit code and
//! message rather than passing for an unrelated reason.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
#[test]
fn seal_refuses_a_bad_flag_before_the_signer_is_ever_built() {
    let repo = support::unique_repo("rfc121-aud10-seal");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["seal", "--bogus-flag"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a usage error (exit 2), not a signer failure: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown seal argument: --bogus-flag"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("maintainer signing is required"),
        "stderr: {stderr}"
    );
}

#[test]
fn merge_refuses_a_bad_flag_before_the_signer_is_ever_built() {
    let repo = support::unique_repo("rfc121-aud10-merge");
    support::init(&repo);
    let output = support::prikk(&repo)
        .args(["merge", "--bogus-flag"])
        .output()
        .unwrap();
    assert_eq!(
        output.status.code(),
        Some(2),
        "expected a usage error (exit 2), not a signer failure: {output:?}"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown merge argument: --bogus-flag"),
        "stderr: {stderr}"
    );
    assert!(
        !stderr.contains("maintainer signing is required"),
        "stderr: {stderr}"
    );
}
