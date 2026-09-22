//! RFC 142, the queued-patch-paths handoff (stikk letter 016): `show` on a queued patch resolves a
//! node-addressed operation's path from the folded baseline, truncated at that patch, instead of
//! reporting `unresolved_node_id` for every such operation the way it did before this round.
//!
//! Control 1 (identical before and after `seal`) is checked here, on the compiled binary. Controls 2
//! (truncation), 3 (a node the same patch later deletes), 4 (a bare sealed patch id is unchanged) and
//! 5 (`show` writes nothing) are checked at the `prikk-store` level
//! (`crates/prikk-store/src/show/tests.rs`): 2 and 3 need a raw, directly-appended patch to put two
//! operations on the same node in one queued patch or across two separate ones, which a real `commit`
//! cannot (it authors at most one operation per changed node per invocation); 5 needs the lifecycle
//! cache genuinely stale at the moment `show` runs, which a real `commit` cannot leave it either -- a
//! real `commit` is itself a cache-refreshing call, so by the time a queued patch it created exists
//! to `show`, the cache is already as fresh as any reader would leave it, and no contrast in the same
//! process state has anything left to write.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::Path;
use std::process::Output;

use support::json;

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn show(repo: &Path, id: &str, format_json: bool) -> Output {
    let mut command = support::prikk(repo);
    command.args(["show", id]);
    if format_json {
        command.args(["--format", "json"]);
    }
    command.output().unwrap()
}

fn queued_patch_id(commit_output: &Output) -> String {
    stdout(commit_output)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints the patch id")
        .trim()
        .to_string()
}

fn sealed_block_id(seal_output: &Output) -> String {
    stdout(seal_output)
        .lines()
        .find_map(|line| line.strip_prefix("block id: "))
        .expect("seal prints the block id")
        .trim()
        .to_string()
}

/// Control 1: a queued `edit-text`'s path resolves from the folded baseline, and the same patch's
/// `show <patch-id> --format json` entry is unchanged once sealed apart from `queued`.
#[test]
fn a_queued_edit_texts_path_resolves_and_survives_seal_unchanged() {
    let repo = support::unique_repo("rfc142qpp-control1");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"before\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "genesis"),
        "commit genesis",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal genesis");

    std::fs::write(repo.join("a.txt"), b"after\n").unwrap();
    let queued = support::commit(&repo, "heads/main", "edit a.txt");
    support::ok(&queued, "commit edit");
    let id = queued_patch_id(&queued);

    let queued_out = show(&repo, &id, true);
    support::ok(&queued_out, "show <queued id> --format json");
    let queued_json = stdout(&queued_out);
    let queued_document = json::parse(&queued_json);
    let queued_patches = queued_document.get("patches").as_array();
    let [queued_patch] = queued_patches else {
        panic!("expected exactly one patch, got {queued_json}");
    };
    assert!(queued_patch.get("queued").as_bool());
    let queued_operations = queued_patch.get("operations").as_array();
    let [queued_operation] = queued_operations else {
        panic!("expected one operation, got {queued_json}");
    };
    assert_eq!(queued_operation.get("kind").as_str(), "edit-text");
    assert_eq!(
        queued_operation.get("paths").as_array()[0]
            .get("path")
            .as_str(),
        "a.txt",
        "{queued_json}"
    );

    // RFC 142 §1: a bare patch id, even sealed, still carries no block context of its own, so
    // re-querying by the same patch id would report unresolved again (unchanged by this round). The
    // patch's own rendering once sealed is read from its block instead, exactly as the handoff's own
    // control 1 says: `show <patch-id-in-its-block>`'s entry from `show <block-id> --format json`.
    let seal_out = support::seal(&repo, "heads/main");
    support::ok(&seal_out, "seal");
    let block_id = sealed_block_id(&seal_out);

    let block_out = show(&repo, &block_id, true);
    support::ok(&block_out, "show <block id> --format json");
    let block_json = stdout(&block_out);
    let block_document = json::parse(&block_json);
    let sealed_patch = block_document
        .get("patches")
        .as_array()
        .iter()
        .find(|patch| patch.get("patch_id").as_str() == id)
        .unwrap_or_else(|| panic!("expected {id} among the block's patches: {block_json}"));
    assert!(!sealed_patch.get("queued").as_bool());
    assert_eq!(
        sealed_patch.get("operations"),
        queued_patch.get("operations"),
        "the same patch's operations are unchanged by sealing"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
