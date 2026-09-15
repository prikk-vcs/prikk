//! RFC 136 §10.3b, increment 2a: the provisional-worktree marker and the derivation gate, through the
//! compiled binary.
//!
//! `checkout --snapshot-materialize` writes the worktree from a snapshot, which is the block's signed
//! state, not a replay of its history. Until `prikk verify` has replayed the repository, every command
//! that would turn that worktree into history refuses. Only `verify` clears the marker.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_store::{Ed25519MaintainerSigner, MaintainerSigner};

/// A second MAINTAINER key (DC-78's fixed test seed), adopted so the key that sealed the history can be
/// removed: a repository's last trusted key cannot be.
const SECOND_KEY_ID: &str = "dc78-second-maintainer";
const SECOND_SEED: [u8; 32] = [
    0x51, 0x51, 0x62, 0x62, 0x73, 0x73, 0x84, 0x84, 0x95, 0x95, 0xa6, 0xa6, 0xb7, 0xb7, 0xc8, 0xc8,
    0xd9, 0xd9, 0xea, 0xea, 0xfb, 0xfb, 0x0c, 0x0c, 0x1d, 0x1d, 0x2e, 0x2e, 0x3f, 0x3f, 0x40, 0x40,
];

fn second_public_key_hex() -> String {
    Ed25519MaintainerSigner::from_seed(SECOND_KEY_ID, &SECOND_SEED)
        .expect("fixed test seed derives a valid signer")
        .public_key_bytes()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

const GATE_MESSAGE: &str =
    "precondition not met: the worktree was materialized from the snapshot of Block";

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A repository with one sealed generation on `heads/main` (its first block, so a checkpoint) and an
/// open `heads/side` at the same block. The maintainer is trusted once, here, so later commands write
/// nothing to the trust store.
fn sealed_repo(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::generation(&repo, "heads/main", "readme.txt", b"hello\n", "genesis");
    support::ok(
        &support::branch_create(&repo, "heads/side", "heads/main"),
        "branch create",
    );
    support::trust_maintainer(&repo);
    repo
}

/// Run `args` with both signing keys, so a refusal cannot be a missing key's.
fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args(args)
        .output()
        .unwrap()
}

fn materialize(repo: &Path) {
    support::ok(
        &run(repo, &["checkout", "--snapshot-materialize"]),
        "checkout --snapshot-materialize",
    );
}

/// Every file under `.prikk/`, with its bytes: "nothing written" is this map unchanged.
fn metadata_tree(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut tree = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                tree.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    tree
}

fn genesis_block(repo: &Path) -> String {
    let output = run(repo, &["log", "--ref", "heads/main"]);
    support::ok(&output, "log");
    stdout(&output)
        .lines()
        .find_map(|line| line.strip_prefix("block ").map(str::to_string))
        .and_then(|rest| rest.split_whitespace().next().map(str::to_string))
        .expect("log names a block")
}

// ---- The guard: every gated entry point calls the one check ------------------------------------------

/// The eight entry points RFC 136 §10.3b.3 gates, each with the function whose body must call the gate
/// before any write. A row whose call is removed fails naming that command.
const GATED: &[(&str, &str, &str, &str)] = &[
    (
        "commit",
        "crates/prikk-store/src/commit_boundary/worktree_patch/node_authoring.rs",
        "fn author_inner<",
        "ensure_worktree_replay_verified(layout)",
    ),
    (
        "mv",
        "crates/prikk-store/src/rename_declaration.rs",
        "pub fn record_rename_declaration(",
        "ensure_worktree_replay_verified(layout)",
    ),
    (
        "seal",
        "crates/prikk-cli/src/seal.rs",
        "fn seal_active_no_audit(",
        "ensure_worktree_replay_verified(&layout)",
    ),
    (
        "merge",
        "crates/prikk-store/src/merge/execute.rs",
        "pub fn execute_merge(",
        "ensure_worktree_replay_verified(layout)",
    ),
    (
        "sync accept",
        "crates/prikk-store/src/patch_exchange/accept.rs",
        "pub fn accept_exchange_artifact(",
        "ensure_worktree_replay_verified(layout)",
    ),
    (
        "sync seal",
        "crates/prikk-store/src/seal_from_accepted.rs",
        "pub fn seal_from_accepted_claim(",
        "ensure_worktree_replay_verified(layout)",
    ),
    (
        "rollback-draft --append-inverse",
        "crates/prikk-store/src/rollback/draft.rs",
        "pub fn append_rollback_draft(",
        "ensure_worktree_replay_verified(layout)",
    ),
    (
        "branch switch",
        "crates/prikk-store/src/branch_switch.rs",
        "pub fn switch_branch(",
        "ensure_worktree_replay_verified(layout)",
    ),
];

#[test]
fn every_gated_entry_point_calls_the_derivation_gate() {
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut missing = Vec::new();
    for (command, file, definition, call) in GATED {
        let text = std::fs::read_to_string(workspace.join(file))
            .unwrap_or_else(|err| panic!("{command}: cannot read {file}: {err}"));
        let Some(start) = text.find(definition) else {
            missing.push(format!("{command}: `{definition}` not found in {file}"));
            continue;
        };
        let body = &text[start..];
        let end = body[definition.len()..]
            .find("\nfn ")
            .into_iter()
            .chain(body[definition.len()..].find("\npub fn "))
            .chain(body[definition.len()..].find("\npub(crate) fn "))
            .min()
            .map_or(body.len(), |offset| offset + definition.len());
        if !body[..end].contains(call) {
            missing.push(format!(
                "{command}: `{definition}` in {file} does not call `{call}`"
            ));
        }
    }
    assert!(
        missing.is_empty(),
        "entry points that do not call the derivation gate:\n{}",
        missing.join("\n")
    );
}

// ---- The gate, on the binary ----------------------------------------------------------------------------

#[test]
fn while_the_marker_is_set_every_gated_command_refuses_and_writes_nothing() {
    let repo = sealed_repo("rfc136-gate-refuses");
    let genesis = genesis_block(&repo);
    materialize(&repo);

    // Reported while set.
    let status = run(&repo, &["status"]);
    support::ok(&status, "status");
    assert!(
        stdout(&status).contains("provisional worktree: materialized from the snapshot of ")
            && stdout(&status).contains("on heads/main; not replay-verified — run prikk verify"),
        "{}",
        stdout(&status)
    );
    let status_json = run(&repo, &["status", "--format", "json"]);
    support::ok(&status_json, "status --format json");
    assert!(
        stdout(&status_json).contains("\"provisional_worktree\": {\"ref\": \"heads/main\""),
        "{}",
        stdout(&status_json)
    );
    let doctor = run(&repo, &["doctor"]);
    assert!(
        stdout(&doctor).contains("PRIKK-DOCTOR-PROVISIONAL-WORKTREE"),
        "{}",
        stdout(&doctor)
    );

    std::fs::write(repo.join("readme.txt"), b"changed\n").unwrap();
    let artifact = repo.join("not-an-artifact.bin");
    std::fs::write(&artifact, b"not an artifact").unwrap();
    let artifact = artifact.to_str().unwrap().to_string();
    let zero_claim = "0".repeat(64);
    let rows: Vec<(&str, Vec<&str>)> = vec![
        ("commit", vec!["commit", "-m", "gated"]),
        ("mv", vec!["mv", "readme.txt", "moved.txt"]),
        ("seal", vec!["seal", "--allow-no-audit"]),
        (
            "merge",
            vec![
                "merge",
                "--allow-no-audit",
                "--baseline-block",
                &genesis,
                "--into",
                "heads/main",
                "--from",
                "heads/side",
            ],
        ),
        ("sync accept", vec!["sync", "accept", &artifact]),
        (
            "sync seal",
            vec!["sync", "seal", "heads/main", "--claim", &zero_claim],
        ),
        (
            "rollback-draft --append-inverse",
            vec!["rollback-draft", "--append-inverse", "-m", "gated"],
        ),
        ("branch switch", vec!["branch", "switch", "heads/side"]),
    ];
    for (command, args) in rows {
        let before = metadata_tree(&repo);
        let output = run(&repo, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{command} must refuse\nstdout: {}\nstderr: {}",
            stdout(&output),
            stderr(&output)
        );
        assert!(
            stderr(&output).contains(GATE_MESSAGE)
                && stderr(&output).contains("on heads/main")
                && stderr(&output).contains("run `prikk verify`"),
            "{command}: {}",
            stderr(&output)
        );
        assert!(
            before == metadata_tree(&repo),
            "{command}: a refused command must write nothing under .prikk/"
        );
    }

    // A clean `verify` clears the marker, and `commit` proceeds.
    let verify = run(&repo, &["verify"]);
    support::ok(&verify, "verify");
    assert!(
        stdout(&verify).contains("provisional worktree: replay-verified; marker cleared"),
        "{}",
        stdout(&verify)
    );
    assert!(!stdout(&run(&repo, &["status"])).contains("provisional worktree"));
    assert!(
        stdout(&run(&repo, &["status", "--format", "json"]))
            .contains("\"provisional_worktree\": null")
    );
    support::ok(&run(&repo, &["commit", "-m", "after verify"]), "commit");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn verify_that_finds_a_damaged_object_keeps_the_marker() {
    let repo = support::unique_repo("rfc136-gate-damaged");
    support::init(&repo);
    // Two equal-length blobs, so their container frames can be swapped.
    std::fs::write(repo.join("a.txt"), b"aaaa\n").unwrap();
    std::fs::write(repo.join("b.txt"), b"bbbb\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "two"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    materialize(&repo);
    support::swap_two_equal_length_blob_frames(&repo);

    let verify = run(&repo, &["verify"]);
    assert_ne!(
        verify.status.code(),
        Some(0),
        "fixture sanity: verify must fail"
    );
    assert!(
        stdout(&verify).contains("provisional worktree: kept; verify found"),
        "{}",
        stdout(&verify)
    );
    let commit = run(&repo, &["commit", "-m", "still gated"]);
    assert!(
        stderr(&commit).contains(GATE_MESSAGE),
        "{}",
        stderr(&commit)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn verify_whose_only_finding_is_publication_trust_clears_the_marker() {
    let repo = sealed_repo("rfc136-gate-trust-only");
    materialize(&repo);
    support::ok(
        &run(
            &repo,
            &[
                "trust",
                "maintainer",
                "add",
                "--key-id",
                SECOND_KEY_ID,
                "--public-key",
                &second_public_key_hex(),
            ],
        ),
        "trust maintainer add (second key)",
    );
    support::ok(
        &run(
            &repo,
            &[
                "trust",
                "maintainer",
                "remove",
                "--key-id",
                support::MAINTAINER_KEY_ID,
            ],
        ),
        "trust maintainer remove",
    );

    let verify = run(&repo, &["verify"]);
    assert!(
        stderr(&verify).contains("publication-trust issues"),
        "fixture sanity: the only refusal is publication trust\nstdout: {}\nstderr: {}",
        stdout(&verify),
        stderr(&verify)
    );
    assert!(
        stdout(&verify).contains("provisional worktree: replay-verified; marker cleared"),
        "{}",
        stdout(&verify)
    );
    assert!(!stdout(&run(&repo, &["status"])).contains("provisional worktree"));
    let _ = std::fs::remove_dir_all(&repo);
}
