//! RFC 117 stage 3 handoff §7's stop-and-escalate clause: "an end-to-end adopt cannot be driven
//! through the CLI alone... that last one would mean the surface is incomplete, as it did in
//! RFC 116." This is that evidence -- a full two-repository loop through `prikk sync`/`prikk tag`
//! alone, no direct library calls: A creates a tag, B receives it via the ordinary sync loop, seals
//! what it received, then adopts the tag under its own key.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::PathBuf;

mod support;

fn sync_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "rfc117-stage3-cli-{tag}-{}.bin",
        support::unique_suffix()
    ))
}

#[test]
fn end_to_end_tag_travel_and_adoption_via_the_cli() {
    let repo_a = support::unique_repo("rfc117-stage3-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"rfc117 stage 3 tag travel\n",
        "first",
    );
    support::ok(
        &support::tag_create(&repo_a, "tags/v1", "heads/main"),
        "tag create tags/v1",
    );

    let repo_b = support::unique_repo("rfc117-stage3-b");
    support::init(&repo_b);
    // Trusted up front so `sync accept`'s own tag-signature print reads `Sound` -- both repos in
    // this harness share the one fixed maintainer key, the same convention `rfc116_sync_cli.rs`
    // uses.
    support::trust_maintainer(&repo_b);

    // `sync summary`/`sync compare`/`sync have` -- B is empty, so this is the full loop's
    // ordinary opening, unchanged from RFC 116.
    let summary_file = sync_file("summary");
    support::ok(
        &support::prikk(&repo_a)
            .args([
                "sync",
                "summary",
                "--output",
                summary_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync summary",
    );
    let have_file = sync_file("have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    // `sync build` in A -- must report the tag alongside the patch delta.
    let artifact_file = sync_file("artifact");
    let build = support::prikk(&repo_a)
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
    support::ok(&build, "sync build");
    let build_stdout = String::from_utf8_lossy(&build.stdout);
    assert!(
        build_stdout.contains("tags: 1"),
        "A must report exactly one tag traveling: {build_stdout}"
    );

    // `sync accept` in B -- must report the tag, and print its (Sound, since both repos in this
    // harness share the one fixed maintainer key) signature outcome.
    let accept = support::prikk(&repo_b)
        .args([
            "sync",
            "accept",
            artifact_file.to_str().unwrap(),
            "--claims-out",
            sync_file("claims").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = String::from_utf8_lossy(&accept.stdout);
    assert!(
        accept_stdout.contains("tags: 1"),
        "B must report exactly one accepted tag: {accept_stdout}"
    );
    let tag_line = accept_stdout
        .lines()
        .find(|line| line.trim_start().starts_with("tag "))
        .unwrap_or_else(|| panic!("sync accept must print a tag signature line: {accept_stdout}"));
    assert!(
        tag_line.contains("Sound"),
        "the fixed maintainer key both repos share must read Sound here: {tag_line}"
    );

    // DC-78 verify-local-tag-publication-trust §4 item 1/3's control: B now holds a *received*,
    // not-yet-adopted tag -- `LocalTagTrust` must not reach it at all (it enumerates only the local
    // pointer index, never the received namespace), so `verify` must still pass here. Written before
    // this repository has sealed or adopted anything, the sharpest point to catch a future blanket
    // type-based check (the mistake `verify-local-tag-publication-trust-handoff-v1.md` §1 warns
    // against) the moment it lands.
    support::ok(
        &support::verify(&repo_b),
        "verify with a received, unadopted tag",
    );

    // Row 1's CLI-level counterpart: `prikk tag list` in B must show nothing yet -- accept adopts
    // no tag by itself.
    let tag_list_before = support::prikk(&repo_b)
        .args(["tag", "list"])
        .output()
        .unwrap();
    support::ok(&tag_list_before, "tag list before adoption");
    assert!(
        String::from_utf8_lossy(&tag_list_before.stdout).contains("no tags"),
        "accept must not have created a local tag by itself"
    );

    // `sync tags` in B before sealing: the tag must show `NotHeld` -- B has not sealed the patch
    // yet, so it does not yet hold a block with this exact patch set.
    let tags_before_seal = support::prikk(&repo_b)
        .args(["sync", "tags"])
        .output()
        .unwrap();
    support::ok(&tags_before_seal, "sync tags before seal");
    let tags_before_stdout = String::from_utf8_lossy(&tags_before_seal.stdout);
    assert!(
        tags_before_stdout.contains("tags/v1") && tags_before_stdout.contains("NotHeld"),
        "before sealing, the received tag must resolve NotHeld: {tags_before_stdout}"
    );

    // Adopting now must refuse -- NotHeld, not a pick.
    let adopt_too_early = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "adopt-tag", "tags/v1"])
        .output()
        .unwrap();
    assert!(
        !adopt_too_early.status.success(),
        "adopting before sealing must refuse, not pick a candidate"
    );

    // Seal what B accepted -- ordinary RFC 116 seal-from-accepted-claim path.
    let claims_out =
        std::fs::read_to_string(sync_file_from_stdout(&accept_stdout, "wrote claim ids to "))
            .unwrap_or_default();
    let claim_id = claims_out
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("sync accept must have written at least one claim id")
        .trim();
    support::trust_maintainer(&repo_b);
    let seal = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "seal", "heads/main", "--claim", claim_id])
        .output()
        .unwrap();
    support::ok(&seal, "sync seal");

    // `sync tags` in B after sealing: must now show `Resolved <block>`.
    let tags_after_seal = support::prikk(&repo_b)
        .args(["sync", "tags"])
        .output()
        .unwrap();
    support::ok(&tags_after_seal, "sync tags after seal");
    let tags_after_stdout = String::from_utf8_lossy(&tags_after_seal.stdout);
    assert!(
        tags_after_stdout.contains("Resolved"),
        "after sealing, the received tag must resolve to a local block: {tags_after_stdout}"
    );

    // The act this whole handoff exists for: adopt it.
    let adopt = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "adopt-tag", "tags/v1"])
        .output()
        .unwrap();
    support::ok(&adopt, "sync adopt-tag");
    assert!(
        String::from_utf8_lossy(&adopt.stdout).contains("adopted tag tags/v1"),
        "adopt-tag must confirm what it adopted: {}",
        String::from_utf8_lossy(&adopt.stdout)
    );

    // Row 5's CLI-level counterpart: B now has its own local `tags/v1`, naming B's own local block.
    let tag_list_after = support::prikk(&repo_b)
        .args(["tag", "list"])
        .output()
        .unwrap();
    support::ok(&tag_list_after, "tag list after adoption");
    assert!(
        String::from_utf8_lossy(&tag_list_after.stdout).contains("tags/v1 "),
        "B must now list its own local tags/v1: {}",
        String::from_utf8_lossy(&tag_list_after.stdout)
    );

    // Adopting a second time must refuse -- the local tag already exists.
    let adopt_again = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["sync", "adopt-tag", "tags/v1"])
        .output()
        .unwrap();
    assert!(
        !adopt_again.status.success(),
        "adopting the same name twice must refuse, not silently succeed"
    );

    let _ = std::fs::remove_dir_all(&repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}

/// Trust-gate caller-level coverage (`trust-gate-caller-coverage-handoff-v1.md` §2, `prikk sync
/// adopt-tag`, `tag_travel.rs:421`). The full tag-travel-and-seal scenario above, up through B
/// sealing what it accepted (adoption refuses `NotHeld` before that point, for an unrelated reason)
/// -- then `sync adopt-tag` in B is attempted with an untrusted-but-well-formed signer instead of
/// the trusted fixed key.
#[test]
fn sync_adopt_tag_fails_closed_on_untrusted_signer() {
    let repo_a = support::unique_repo("rfc117-stage3-adopt-untrusted-a");
    support::init(&repo_a);
    support::generation(
        &repo_a,
        "heads/main",
        "a.txt",
        b"rfc117 stage 3 adopt-tag trust gate\n",
        "first",
    );
    support::ok(
        &support::tag_create(&repo_a, "tags/v1", "heads/main"),
        "tag create tags/v1",
    );

    let repo_b = support::unique_repo("rfc117-stage3-adopt-untrusted-b");
    support::init(&repo_b);
    support::trust_maintainer(&repo_b);

    let have_file = sync_file("adopt-untrusted-have");
    support::ok(
        &support::prikk(&repo_b)
            .args([
                "sync",
                "have",
                "heads/main",
                "--output",
                have_file.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync have",
    );

    let artifact_file = sync_file("adopt-untrusted-artifact");
    support::ok(
        &support::prikk(&repo_a)
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
            .unwrap(),
        "sync build",
    );

    let accept = support::prikk(&repo_b)
        .args([
            "sync",
            "accept",
            artifact_file.to_str().unwrap(),
            "--claims-out",
            sync_file("adopt-untrusted-claims").to_str().unwrap(),
        ])
        .output()
        .unwrap();
    support::ok(&accept, "sync accept");
    let accept_stdout = String::from_utf8_lossy(&accept.stdout);
    let claims_out =
        std::fs::read_to_string(sync_file_from_stdout(&accept_stdout, "wrote claim ids to "))
            .unwrap_or_default();
    let claim_id = claims_out
        .lines()
        .find(|line| !line.trim().is_empty())
        .expect("sync accept must have written at least one claim id")
        .trim();
    support::ok(
        &support::prikk(&repo_b)
            .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
            .env(
                "PRIKK_MAINTAINER_SEED_FILE",
                support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
            )
            .args(["sync", "seal", "heads/main", "--claim", claim_id])
            .output()
            .unwrap(),
        "sync seal",
    );

    let out = support::prikk(&repo_b)
        .env("PRIKK_MAINTAINER_KEY_ID", "untrusted-maintainer")
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file("222233334444555566667777888899990000aaaabbbbccccddddeeeeffff1111"),
        )
        .args(["sync", "adopt-tag", "tags/v1"])
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "sync adopt-tag with an untrusted maintainer signer unexpectedly succeeded\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("not trusted by policy"),
        "unexpected stderr: {stderr}"
    );

    let _ = std::fs::remove_dir_all(&repo_a);
    let _ = std::fs::remove_dir_all(&repo_b);
}

fn sync_file_from_stdout(stdout: &str, prefix: &str) -> PathBuf {
    let line = stdout
        .lines()
        .find(|line| line.starts_with(prefix))
        .unwrap_or_else(|| panic!("expected a line starting with {prefix:?} in: {stdout}"));
    PathBuf::from(line.trim_start_matches(prefix).trim())
}
