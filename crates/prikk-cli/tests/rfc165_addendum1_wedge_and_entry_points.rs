//! RFC 165 Addendum 1 (review v1 of `rfc165-round-1-report-v1.md`):
//!
//! - §1, the wedge: a ref-log tail with no pointer lead must not block `commit`, and every
//!   publication must still refuse over it -- naming the tail, not "incomplete publication", not
//!   "seal retry".
//! - §3, unheld call sites: `seal`, `branch create`, and `branch close` each refuse behind
//!   *another* ref's own genuinely incomplete publication, exercised through the real compiled
//!   binary (not a direct library call), matching `merge`/`tag create`'s own already-accepted
//!   entry-point tests.
//!
//! Drives the real `prikk` binary throughout, the same discipline `genesis_end_to_end.rs` and
//! `rfc163_write_never_buries_a_crash_state.rs` already use for exactly this reason.
#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;

mod support;

use support::{
    branch_close, branch_create, commit, init, ok, prikk, seal, store_bytes, trust_maintainer,
    unique_repo, verify,
};

fn log_container_path(repo: &Path) -> std::path::PathBuf {
    repo.join(".prikk/refs/containers/log-a.container")
}

/// Crash `branch create <name> --from <from>` between its pointer write and its ref-log write: the
/// real command runs to completion (the pointer index durably names the new branch), then the ref
/// log container is truncated back to its own exact pre-create byte length -- the pointer now leads
/// the log, a genuine `PointerLeading` state, not constructed through any failpoint (none are
/// reachable from outside the binary), matching how the round-1 report's own manual verification
/// built this state.
fn crash_branch_create(repo: &Path, name: &str, from: &str) {
    let container = log_container_path(repo);
    let before_len = std::fs::metadata(&container).map(|m| m.len()).unwrap_or(0);
    ok(&branch_create(repo, name, from), "branch create (to crash)");
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(&container)
        .unwrap();
    file.set_len(before_len).unwrap();

    // Confirm the premise: the repository is now genuinely in the `PointerLeading` shape this test
    // needs, not merely "a file got shorter."
    let verify_output = verify(repo);
    let verify_text = String::from_utf8_lossy(&verify_output.stdout);
    assert!(
        !verify_output.status.success() || verify_text.contains("PRIKK-VERIFY-REF-DIVERGENCE"),
        "fixture bug: {name} must read as an interrupted publication after the truncate\n{verify_text}"
    );
}

fn setup_with_main(repo: &Path) {
    init(repo);
    std::fs::write(repo.join("a.txt"), "one\n").unwrap();
    ok(&commit(repo, "heads/main", "one"), "commit one");
    ok(&seal(repo, "heads/main"), "seal main");
}

// ---------------------------------------------------------------------------------------------
// §3: seal / branch create / branch close refuse behind *another* ref's own incomplete publication
// ---------------------------------------------------------------------------------------------

#[test]
fn seal_entry_point_refuses_behind_another_refs_incomplete_publication() {
    let repo = unique_repo("rfc165-a1-seal-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    // Queue the patch `heads/main` will seal *before* crashing `heads/broken` -- `commit` also
    // refuses behind a genuine incomplete publication (unexcluded, unlike a lead-free tail), so the
    // patch must already be queued, not queued afterward.
    std::fs::write(repo.join("b.txt"), "two\n").unwrap();
    ok(&commit(&repo, "heads/main", "two"), "commit two");
    crash_branch_create(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = seal(&repo, "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "seal of heads/main must refuse behind heads/broken's interrupted publication\n{text}"
    );
    assert!(
        text.contains("incomplete ref publication"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_create_entry_point_refuses_behind_another_refs_incomplete_publication() {
    let repo = unique_repo("rfc165-a1-branch-create-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    crash_branch_create(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = branch_create(&repo, "heads/topic", "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "branch create must refuse behind heads/broken's interrupted publication\n{text}"
    );
    assert!(
        text.contains("incomplete ref publication"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn branch_close_entry_point_refuses_behind_another_refs_incomplete_publication() {
    let repo = unique_repo("rfc165-a1-branch-close-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    crash_branch_create(&repo, "heads/broken", "heads/main");
    let before = store_bytes(&repo);

    let output = branch_close(&repo, "heads/topic");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "branch close must refuse behind heads/broken's interrupted publication\n{text}"
    );
    assert!(
        text.contains("incomplete ref publication"),
        "unexpected refusal text: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

// ---------------------------------------------------------------------------------------------
// §1, the wedge: a lead-free ref-log tail never blocks `commit`; every publication still refuses
// over it, naming the tail -- not "incomplete publication", not "seal retry".
// ---------------------------------------------------------------------------------------------

fn append_zero_tail(repo: &Path, n: usize) {
    use std::io::Write;
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(log_container_path(repo))
        .unwrap();
    file.write_all(&vec![0_u8; n]).unwrap();
}

#[test]
fn a_lead_free_tail_never_blocks_commit_through_the_real_binary() {
    let repo = unique_repo("rfc165-a1-tail-commit-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    append_zero_tail(&repo, 100);

    std::fs::write(repo.join("c.txt"), "three\n").unwrap();
    let output = commit(&repo, "heads/main", "three");
    assert!(
        output.status.success(),
        "commit must succeed over a lead-free ref-log tail\nstderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_lead_free_tail_still_refuses_seal_through_the_real_binary() {
    let repo = unique_repo("rfc165-a1-tail-seal-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    std::fs::write(repo.join("c.txt"), "three\n").unwrap();
    ok(&commit(&repo, "heads/main", "three"), "commit three");
    append_zero_tail(&repo, 100);
    let before = store_bytes(&repo);

    trust_maintainer(&repo);
    let output = seal(&repo, "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "seal must still refuse over the ref log's own lead-free tail\n{text}"
    );
    assert!(
        text.contains("the ref log"),
        "must name the ref log: {text}"
    );
    assert!(
        text.contains("RFC 165 R5"),
        "must say the repair arrives with R5: {text}"
    );
    assert!(
        !text.contains("incomplete ref publication"),
        "must not call a lead-free tail an incomplete publication: {text}"
    );
    assert!(
        !text.contains("seal retry"),
        "must not name a seal retry that cannot apply: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_lead_free_tail_still_refuses_branch_create_through_the_real_binary() {
    let repo = unique_repo("rfc165-a1-tail-branch-create-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    append_zero_tail(&repo, 100);
    let before = store_bytes(&repo);

    let output = branch_create(&repo, "heads/topic", "heads/main");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "branch create must still refuse over the ref log's own lead-free tail\n{text}"
    );
    assert!(
        text.contains("the ref log"),
        "must name the ref log: {text}"
    );
    assert!(
        text.contains("RFC 165 R5"),
        "must say the repair arrives with R5: {text}"
    );
    assert!(
        !text.contains("incomplete ref publication"),
        "must not call a lead-free tail an incomplete publication: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_lead_free_tail_still_refuses_branch_close_through_the_real_binary() {
    let repo = unique_repo("rfc165-a1-tail-branch-close-entry");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    ok(
        &branch_create(&repo, "heads/topic", "heads/main"),
        "branch create heads/topic",
    );
    append_zero_tail(&repo, 100);
    let before = store_bytes(&repo);

    let output = branch_close(&repo, "heads/topic");
    let text = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "branch close must still refuse over the ref log's own lead-free tail\n{text}"
    );
    assert!(
        text.contains("the ref log"),
        "must name the ref log: {text}"
    );
    assert!(
        text.contains("RFC 165 R5"),
        "must say the repair arrives with R5: {text}"
    );
    assert!(
        !text.contains("incomplete ref publication"),
        "must not call a lead-free tail an incomplete publication: {text}"
    );
    assert_eq!(before, store_bytes(&repo), "a refusal must write nothing");
    let _ = std::fs::remove_dir_all(&repo);
}

// `seal`'s own retry of its *own* interrupted publication (DC-38) is already covered end to end,
// through the real binary, by `genesis_end_to_end.rs`'s
// `seal_retry_drains_already_published_wal_without_duplicate_ref_update` -- not duplicated here. An
// earlier version of this file also tried an *attributable torn tail* shape via
// `support::append_torn_ref_log_tail`, which duplicates whichever record currently sits last in the
// container; that does not correspond to the seal under test's own actual pending content, so it
// exercises `incomplete_tail_matches`'s own (pre-existing, unchanged) rejection of a tail that does
// not match the expected next write, not this round's own exclusion -- a different, already-settled
// question, not this file's to re-prove.

// ---------------------------------------------------------------------------------------------
// Manual control (not an automated toggle, matching the round-1 report's own established practice
// for `merge`/`tag create`): with each of the three `ensure_may_publish` call sites commented out
// (`crates/prikk-cli/src/seal.rs:91`, `crates/prikk-cli/src/branch.rs:215,384`), rebuilding, and
// rerunning this file, the three "behind another ref's incomplete publication" tests above and the
// three "lead-free tail" tests above all failed (the commands wrongly succeeded, wrongly writing
// past the crash state) -- confirmed, then the call sites were restored and this file rerun green
// again. Not kept as a `#[test]` here: there is no in-binary toggle for "this precondition call
// does not run" to assert against without reintroducing the exact bug under test.
// ---------------------------------------------------------------------------------------------

#[test]
fn sanity_the_support_harness_still_produces_a_sealed_main() {
    let repo = unique_repo("rfc165-a1-sanity");
    std::fs::create_dir_all(&repo).unwrap();
    setup_with_main(&repo);
    let output = prikk(&repo).arg("log").output().unwrap();
    assert!(output.status.success());
    let _ = std::fs::remove_dir_all(&repo);
}
