//! RFC 123 §6/§8.4, review v1 §4's required end-to-end assertion: the message a real `prikk commit`
//! carries actually reaches `prikk log`, driven through the compiled binary rather than assumed from
//! reading the source. The counterpart, unit-level test for the absence case (a schema-1/2/3 patch
//! shows no line at all) lives in `prikk-store`'s `history::tests`, since the CLI itself always mints
//! the current schema and cannot construct an older one directly.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
#[test]
fn commit_message_appears_in_log_under_its_block() {
    let repo = support::unique_repo("rfc123-message-in-log");
    support::init(&repo);
    std::fs::write(repo.join("f.txt"), "hello").unwrap();
    let commit_output = support::commit(&repo, "heads/main", "a distinctive commit message");
    support::ok(&commit_output, "commit");
    let commit_stdout = String::from_utf8_lossy(&commit_output.stdout);
    let patch_id = commit_stdout
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit output must include a patch id line")
        .to_string();

    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let log = support::prikk(&repo).arg("log").output().unwrap();
    support::ok(&log, "log");
    let log_stdout = String::from_utf8_lossy(&log.stdout);
    assert!(
        log_stdout.contains(&format!("patch {patch_id}: a distinctive commit message")),
        "log must show the commit message under its patch id: {log_stdout}"
    );
}
