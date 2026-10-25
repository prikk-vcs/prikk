//! DC-72 path-safety conformance: case-insensitive collision rejection on branch ref names, tag ref
//! names, and maintainer trust key ids, plus the Windows-reserved-name gap on trust key ids.
//! Repository-path collision rejection is already covered by
//! `crates/prikk-replay/src/path/tests.rs` and `crates/prikk-store/src/state_root/tests.rs` and is
//! not repeated here.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support::*;

#[test]
fn branch_create_rejects_case_insensitive_collision() {
    let repo = unique_repo("dc72-branch-collision");
    init(&repo);
    generation(&repo, "heads/main", "a.txt", b"a", "genesis");

    let colliding = branch_create(&repo, "heads/Main", "heads/main");
    assert!(
        !colliding.status.success(),
        "expected heads/Main to collide with heads/main"
    );
    assert!(
        String::from_utf8_lossy(&colliding.stderr).contains("case-insensitive ref-name collision"),
        "stderr: {}",
        String::from_utf8_lossy(&colliding.stderr)
    );
}

#[test]
fn tag_create_rejects_case_insensitive_collision() {
    let repo = unique_repo("dc72-tag-collision");
    init(&repo);
    generation(&repo, "heads/main", "a.txt", b"a", "genesis");

    ok(
        &tag_create(&repo, "tags/v1", "heads/main"),
        "tag create tags/v1",
    );
    let colliding = tag_create(&repo, "tags/V1", "heads/main");
    assert!(
        !colliding.status.success(),
        "expected tags/V1 to collide with tags/v1"
    );
    assert!(
        String::from_utf8_lossy(&colliding.stderr).contains("case-insensitive ref-name collision"),
        "stderr: {}",
        String::from_utf8_lossy(&colliding.stderr)
    );
}

#[test]
fn branch_and_tag_namespaces_do_not_collide_with_each_other() {
    let repo = unique_repo("dc72-namespace-no-collision");
    init(&repo);
    generation(&repo, "heads/main", "a.txt", b"a", "genesis");

    ok(
        &branch_create(&repo, "heads/release", "heads/main"),
        "branch create heads/release",
    );
    ok(
        &tag_create(&repo, "tags/Release", "heads/main"),
        "tag create tags/Release: same folded stem, different namespace, not a collision",
    );
}

#[test]
fn maintainer_key_id_rejects_case_insensitive_collision() {
    let repo = unique_repo("dc72-trust-collision");
    init(&repo);
    let key_hex = maintainer_public_key_hex();

    ok(
        &prikk(&repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                "Dev-Maintainer",
                "--public-key",
                &key_hex,
            ])
            .output()
            .unwrap(),
        "trust maintainer add Dev-Maintainer",
    );
    let colliding = prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "dev-maintainer",
            "--public-key",
            &key_hex,
        ])
        .output()
        .unwrap();
    assert!(
        !colliding.status.success(),
        "expected dev-maintainer to collide with Dev-Maintainer"
    );
    assert!(
        String::from_utf8_lossy(&colliding.stderr)
            .contains("case-insensitive maintainer key id collision"),
        "stderr: {}",
        String::from_utf8_lossy(&colliding.stderr)
    );
}

#[test]
fn maintainer_key_id_add_or_replace_is_not_a_self_collision() {
    let repo = unique_repo("dc72-trust-replace");
    init(&repo);
    let key_hex = maintainer_public_key_hex();
    for _ in 0..2 {
        ok(
            &prikk(&repo)
                .args([
                    "trust",
                    "maintainer",
                    "add",
                    "--key-id",
                    "dev-maintainer",
                    "--public-key",
                    &key_hex,
                ])
                .output()
                .unwrap(),
            "trust maintainer add dev-maintainer (add-or-replace)",
        );
    }
}

#[test]
fn maintainer_key_id_rejects_windows_reserved_name() {
    let repo = unique_repo("dc72-trust-reserved");
    init(&repo);
    let out = prikk(&repo)
        .args([
            "trust",
            "maintainer",
            "add",
            "--key-id",
            "CON",
            "--public-key",
            &maintainer_public_key_hex(),
        ])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "expected maintainer key id CON to be rejected as a Windows reserved name"
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("Windows reserved device name"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn branch_names_accept_non_ascii_without_folding_it() {
    // Recorded limitation (docs/src/reference/path-safety.md): ASCII-only case folding. An
    // NFC/NFD-equivalent pair is not rejected — this pins that as an intentional, documented gap
    // rather than an accident a future change could silently close or silently widen.
    let repo = unique_repo("dc72-branch-non-ascii");
    init(&repo);
    generation(&repo, "heads/main", "a.txt", b"a", "genesis");

    let nfc = "heads/caf\u{00e9}";
    let nfd = "heads/cafe\u{0301}";
    assert_ne!(nfc, nfd, "the two forms must be distinct byte sequences");

    ok(
        &branch_create(&repo, nfc, "heads/main"),
        "branch create (NFC)",
    );
    ok(
        &branch_create(&repo, nfd, "heads/main"),
        "branch create (NFD) — accepted today; Unicode normalization is a recorded limitation, not implemented",
    );
}
