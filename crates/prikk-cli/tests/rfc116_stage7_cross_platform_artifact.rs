//! RFC 116 stage 7 handoff (`stage-7-followups-handoff-v1.md`), Part A: CI runs the full suite on
//! Linux, macOS and Windows, so each platform exchanges **with itself** -- nothing tests an
//! artifact produced on one platform being accepted on another, which is the real cross-host risk
//! (the mechanism is file-based; hosts differ by platform, not by being separate machines). A
//! committed byte fixture is cheaper and stricter than two VMs, and is the pattern RFC 114 already
//! uses for migration coverage (`tests/fixtures/dc55_pre_swap_repo`).
//!
//! **`PEXCH001` is representational (RFC 114 §3), not frozen** -- this fixture is *not* a
//! frozen-format promise. When the artifact format legitimately changes, regenerate it (run
//! `cargo test -p prikk --test rfc116_stage7_cross_platform_artifact -- --ignored` to execute
//! `regenerate_cross_platform_artifact_fixture` below) and have the change reviewed like any other.
//! A regeneration the format change actually requires is not a stop-work finding; a casual one is
//! not fine either -- `git log` on the fixture file is the record of which is which.
//!
//! **How the committed fixture was produced:** `regenerate_cross_platform_artifact_fixture`
//! (`#[ignore]`d so it never runs in ordinary CI) builds a fresh one-patch, one-block repository
//! using this test harness's own fixed keys (`support::AUTHOR_KEY_ID`, `support::MAINTAINER_KEY_ID`
//! -- the same keys every other test in this crate uses), then drives `sync have` (in a scratch
//! empty receiver) and `sync build` through the real CLI exactly the way an operator would, and
//! copies the resulting artifact bytes to
//! `tests/fixtures/rfc116_stage7_cross_platform_artifact.pexch002`. **Treat it like
//! `dc55_pre_swap_repo`: it is evidence, not a convenience** -- do not regenerate it casually.
//! **The extension names the format the fixture actually carries** (RFC 117 stage 3 review v1 §4):
//! when the artifact format legitimately changes and this fixture is regenerated for it, rename the
//! file too -- a fixture whose extension names the wrong format costs a reader an hour precisely
//! when they are already confused about why decoding failed.
//! Because the fixed maintainer key travels this way (not through the artifact itself -- see below),
//! any repository in this test suite can trust it and seal what the fixture carries.
//!
//! **The fixture's own author key material is self-contained**: `export_exchange_artifact`
//! (called inside `sync build`) always carries the AUTHOR key material for every patch it exports,
//! recorded locally on the source repository as an ordinary side effect of `prikk commit`
//! (`node_authoring.rs`). A fresh receiver therefore needs no external state to verify the patch's
//! AUTHOR signature as `Sound` -- `accept_exchange_artifact` never touches the network or any
//! shared file outside the artifact itself. Sealing what `accept` lands is a separate step and does
//! need the fixed MAINTAINER key trusted locally first (`support::trust_maintainer`), the same as
//! every other sync test in this crate -- that is ordinary local configuration, not something the
//! artifact format is expected to carry (RFC 115 design: sealing is a receiver's own explicit act).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::{Path, PathBuf};

mod support;

fn fixture_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("rfc116_stage7_cross_platform_artifact.pexch002")
}

fn scratch_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rfc116-stage7-fixture-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

#[test]
#[ignore = "regenerates the committed fixture -- run deliberately when PEXCH001 legitimately \
            changes, never in ordinary CI"]
fn regenerate_cross_platform_artifact_fixture() {
    let source = support::unique_repo("rfc116-stage7-fixture-source");
    support::init(&source);
    support::generation(
        &source,
        "heads/main",
        "cross-platform.txt",
        b"rfc116 stage 7 cross-platform fixture\n",
        "cross-platform fixture",
    );

    // A scratch empty receiver, only to produce an empty have-list -- the full one-patch history is
    // therefore the whole delta, matching `rfc116_sync_cli.rs`'s own single-block shape.
    let scratch_receiver = support::unique_repo("rfc116-stage7-fixture-scratch-receiver");
    support::init(&scratch_receiver);
    let have_file = scratch_file("regen-have");
    support::ok(
        &support::prikk(&scratch_receiver)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have (scratch receiver)",
    );

    let artifact_file = scratch_file("regen-artifact");
    let build = support::prikk(&source)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "sync",
            "build",
            "heads/main",
            "--have",
            have_file.to_str().unwrap(),
            "--output",
            artifact_file.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&build, "sync build (fixture regeneration)");

    std::fs::create_dir_all(fixture_path().parent().unwrap()).unwrap();
    std::fs::copy(&artifact_file, fixture_path()).unwrap();
    println!("regenerated fixture at {}", fixture_path().display());

    let _ = std::fs::remove_dir_all(source);
    let _ = std::fs::remove_dir_all(scratch_receiver);
    let _ = std::fs::remove_file(have_file);
    let _ = std::fs::remove_file(artifact_file);
}

/// The property this fixture exists for: a `PEXCH001` artifact produced once, committed as bytes,
/// accepts into a fresh repository **on whatever platform this test happens to run on**, and what
/// it carries is genuinely sealable afterward -- not merely "the file was read."
#[test]
fn cross_platform_artifact_fixture_accepts_and_seals_on_this_platform() {
    let bytes = std::fs::read(fixture_path())
        .unwrap_or_else(|err| panic!("committed fixture must be readable: {err}"));

    let receiver = support::unique_repo("rfc116-stage7-fixture-target");
    support::init(&receiver);
    let artifact_file = scratch_file("accept-input");
    std::fs::write(&artifact_file, &bytes).unwrap();

    let claims_out = scratch_file("accept-claims");
    let accept = support::prikk(&receiver)
        .args([
            "sync",
            "accept",
            artifact_file.to_str().unwrap(),
            "--claims-out",
            claims_out.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept (cross-platform fixture)");
    let accept_stdout = String::from_utf8_lossy(&accept.stdout);
    assert!(
        accept_stdout.contains("patches: 1") && accept_stdout.contains("claims: 1"),
        "the committed fixture must carry exactly one patch and one claim: {accept_stdout}"
    );

    let claim_id = std::fs::read_to_string(&claims_out)
        .unwrap()
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or_else(|| panic!("sync accept must write a claim id to {claims_out:?}"))
        .trim()
        .to_string();

    support::trust_maintainer(&receiver);
    let seal = support::prikk(&receiver)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", &claim_id])
        .output()
        .unwrap();
    support::ok(&seal, "sync seal (cross-platform fixture)");
    assert!(
        String::from_utf8_lossy(&seal.stdout).contains("sealed 1 patch(es)"),
        "what the fixture carries must actually be sealable: {}",
        String::from_utf8_lossy(&seal.stdout)
    );

    let _ = std::fs::remove_dir_all(receiver);
    let _ = std::fs::remove_file(artifact_file);
    let _ = std::fs::remove_file(claims_out);
}

/// §A3's control: a single corrupted byte in the artifact must be refused, not silently accepted --
/// proof this test suite exercises real decoding, not merely "the file exists and is non-empty."
#[test]
fn a_corrupted_byte_in_the_cross_platform_fixture_is_refused() {
    let mut bytes = std::fs::read(fixture_path())
        .unwrap_or_else(|err| panic!("committed fixture must be readable: {err}"));
    let mid = bytes.len() / 2;
    bytes[mid] ^= 0xff;

    let receiver = support::unique_repo("rfc116-stage7-fixture-corrupt-target");
    support::init(&receiver);
    let artifact_file = scratch_file("corrupt-input");
    std::fs::write(&artifact_file, &bytes).unwrap();

    let accept = support::prikk(&receiver)
        .args(["sync", "accept", artifact_file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        !accept.status.success(),
        "a corrupted artifact byte must be refused, not silently accepted: stdout={}",
        String::from_utf8_lossy(&accept.stdout)
    );

    let _ = std::fs::remove_dir_all(receiver);
    let _ = std::fs::remove_file(artifact_file);
}
