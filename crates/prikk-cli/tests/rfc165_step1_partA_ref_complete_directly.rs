//! 0.51.0 step 1 Part A (020's 0.51.0 grade): over an interrupted publication left by a genuinely
//! *torn* ref-log tail (not `crash_branch_create`'s own clean truncate-to-previous-length, which
//! `rfc165_addendum1_wedge_and_entry_points.rs` already covers), `seal`, `tag create`, `branch
//! create <other>` and `branch close` answered `--repair-tails` first -- the tail guard ran before
//! the publication check, inside `ensure_publication_precondition` (`refs.rs`). Both guards would
//! refuse, but only the publication check's refusal is typed `PrikkError::IncompletePublication`,
//! which each of these four commands already matches (via `.map_err`, "Part B review carry"/"019
//! §5.2" comments) to name `ref complete <ref>` directly. Firing the tail guard first meant
//! `--repair-tails` answered first instead, and `--repair-tails` itself then names `ref complete`
//! (per `pointer_rebuild.rs`/`doctor.rs`) -- two steps where one suffices.
//!
//! Drives the real `prikk` binary throughout, matching `rfc165_addendum1_wedge_and_entry_points.rs`'s
//! own discipline.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;

mod support;

use support::{
    branch_close, branch_create, commit, init, ok, prikk, seal, store_bytes, tag_create,
    trust_maintainer, unique_repo,
};

fn log_container_path(repo: &Path) -> std::path::PathBuf {
    repo.join(".prikk/refs/containers/log-a.container")
}

/// Crash `branch create <name> --from <from>` leaving a genuinely torn ref-log tail: the real
/// command runs to completion (pointer and log both durably written), then the ref log container
/// is truncated to a length strictly *between* its own pre-create and post-create lengths -- a
/// partial record, trailing partial bytes, not the clean revert-to-previous-length
/// `rfc165_addendum1_wedge_and_entry_points.rs::crash_branch_create` uses. The pointer now leads
/// the log (same as that helper), but this time with an actual unclean tail for the tail guard to
/// find -- the shape `ensure_publication_precondition`'s tail check and its agreement loop can
/// disagree over.
fn crash_branch_create_with_torn_tail(repo: &Path, name: &str, from: &str) {
    let container = log_container_path(repo);
    let before_len = std::fs::metadata(&container).map(|m| m.len()).unwrap_or(0);
    ok(&branch_create(repo, name, from), "branch create (to crash)");
    let after_len = std::fs::metadata(&container).unwrap().len();
    assert!(
        after_len > before_len + 1,
        "fixture bug: the record must be long enough to truncate mid-record"
    );
    let torn_len = before_len + (after_len - before_len) / 2;
    assert!(
        torn_len > before_len && torn_len < after_len,
        "fixture bug: the truncate point must land strictly inside the new record"
    );
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&container)
        .unwrap();
    file.set_len(torn_len).unwrap();
}

fn setup_with_main(repo: &Path) {
    init(repo);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    ok(&commit(repo, "heads/main", "one"), "commit one");
    ok(&seal(repo, "heads/main"), "seal main");
}

fn ref_complete(repo: &Path, ref_name: &str) -> std::process::Output {
    trust_maintainer(repo);
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(["ref", "complete", ref_name])
        .output()
        .unwrap()
}

#[test]
fn seal_names_ref_complete_directly_over_a_torn_lead_tail() {
    let repo = unique_repo("rfc165-step1-a-seal");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    std::fs::write(repo.join("b.txt"), "two\n").unwrap();
    ok(&commit(&repo, "heads/main", "two"), "commit two");
    crash_branch_create_with_torn_tail(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = seal(&repo, "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "seal must refuse\n{text}");
    assert!(
        !text.contains("--repair-tails"),
        "seal must not answer --repair-tails over a lead's own torn tail: {text}"
    );
    assert!(
        text.contains("run `prikk ref complete heads/broken`"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");

    ok(
        &ref_complete(&repo, "heads/broken"),
        "the named command, ref complete heads/broken",
    );
    ok(&seal(&repo, "heads/main"), "seal main after the completion");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn tag_create_names_ref_complete_directly_over_a_torn_lead_tail() {
    let repo = unique_repo("rfc165-step1-a-tag-create");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    crash_branch_create_with_torn_tail(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = tag_create(&repo, "tags/v1", "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "tag create must refuse\n{text}");
    assert!(
        !text.contains("--repair-tails"),
        "tag create must not answer --repair-tails over a lead's own torn tail: {text}"
    );
    assert!(
        text.contains("run `prikk ref complete heads/broken`"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");

    ok(
        &ref_complete(&repo, "heads/broken"),
        "the named command, ref complete heads/broken",
    );
    ok(
        &tag_create(&repo, "tags/v1", "heads/main"),
        "tag create after the completion",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_names_ref_complete_directly_over_a_torn_lead_tail() {
    let repo = unique_repo("rfc165-step1-a-branch-create");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    crash_branch_create_with_torn_tail(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = branch_create(&repo, "heads/topic", "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "branch create must refuse\n{text}");
    assert!(
        !text.contains("--repair-tails"),
        "branch create must not answer --repair-tails over a lead's own torn tail: {text}"
    );
    assert!(
        text.contains("run `prikk ref complete heads/broken`"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");

    ok(
        &ref_complete(&repo, "heads/broken"),
        "the named command, ref complete heads/broken",
    );
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create after the completion",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_close_names_ref_complete_directly_over_a_torn_lead_tail() {
    let repo = unique_repo("rfc165-step1-a-branch-close");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    crash_branch_create_with_torn_tail(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = branch_close(&repo, "heads/topic");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "branch close must refuse\n{text}");
    assert!(
        !text.contains("--repair-tails"),
        "branch close must not answer --repair-tails over a lead's own torn tail: {text}"
    );
    assert!(
        text.contains("run `prikk ref complete heads/broken`"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");

    ok(
        &ref_complete(&repo, "heads/broken"),
        "the named command, ref complete heads/broken",
    );
    ok(
        &branch_close(&repo, "heads/topic"),
        "branch close after the completion",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// The lead-free control (Part A's "the lead-free control still names `--repair-tails`"): a tail
/// with no pointer lead at all (appended zeros, matching `rfc165_addendum1_wedge_and_entry_points.
/// rs::append_zero_tail`) must still answer `--repair-tails`, not `ref complete` -- this round's fix
/// must not turn every tail into a publication question, only the ones that are genuinely also a
/// lead.
#[test]
fn a_lead_free_torn_tail_still_names_repair_tails() {
    use std::io::Write;

    let repo = unique_repo("rfc165-step1-a-lead-free-control");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    let container = log_container_path(&repo);
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&container)
        .unwrap();
    file.write_all(&[0_u8; 100]).unwrap();
    drop(file);

    let output = seal(&repo, "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "seal must refuse\n{text}");
    assert!(
        text.contains("--repair-tails"),
        "a genuinely lead-free tail must still name --repair-tails: {text}"
    );
    assert!(
        !text.contains("ref complete"),
        "a lead-free tail is not a publication question: {text}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}
