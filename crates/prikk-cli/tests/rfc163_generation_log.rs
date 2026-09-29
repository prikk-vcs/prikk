//! RFC 163 §9: **the generation log never buries a crash state either.** The same defect as N1, one
//! layer up -- `resolve_live_slot` already tolerates a torn generation-log tail silently (the last
//! sound record is used), and `compact` used to append a new generation record behind it, blind,
//! converting a tail `verify` never reported into interior damage that breaks every later `commit`
//! and `seal`, repository-wide, with no repair verb at all. Now: `compact` confirms under its lock
//! that the generation log ends at its last sound record before it writes anything -- not only before
//! the generation record, before the new slot's first byte too.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

const HEADER_LEN: usize = 8 + 2 + 8 + 32;

fn run(repo: &Path, args: &[&str]) -> (Option<i32>, String) {
    let output = support::prikk(repo).args(args).output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("target file exists")
}

fn append_torn_prefix(path: &Path) {
    let mut bytes = read_bytes(path);
    let prefix_len = (HEADER_LEN - 5).min(bytes.len());
    let prefix = bytes[..prefix_len].to_vec();
    bytes.extend(prefix);
    std::fs::write(path, &bytes).unwrap();
}

fn append_zeros_100(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend(vec![0_u8; 100]);
    std::fs::write(path, &bytes).unwrap();
}

type FaultFn = fn(&Path);
const FAULTS: [(&str, FaultFn); 2] = [
    ("torn prefix", append_torn_prefix as FaultFn),
    ("100 zero bytes", append_zeros_100 as FaultFn),
];

/// One compacting container: its generation-log path, its `compact` flag, and how to build a
/// repository with something for that container's own first `compact` to reclaim.
struct Target {
    name: &'static str,
    flag: &'static str,
    generation_log: fn(&Path) -> PathBuf,
    /// Build a repository, already `compact`-ed once (so the generation log holds one clean record),
    /// with the container non-trivially populated.
    setup: fn(&str) -> PathBuf,
}

fn pointer_index_generation_log(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/pointer-index-generation.log")
}

fn received_index_generation_log(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/received-index-generation.log")
}

fn trust_policy_generation_log(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/policy-generation.log")
}

fn setup_pointer_index(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), "a\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "first"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    std::fs::write(repo.join("b.txt"), "b\n".repeat(20)).unwrap();
    support::ok(&support::commit(&repo, "heads/main", "second"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    let (code, text) = run(&repo, &["compact", "--pointer-index"]);
    assert_eq!(code, Some(0), "first compact (pointer index): {text}");
    repo
}

fn setup_received_index(tag: &str) -> PathBuf {
    let sender = support::unique_repo(&format!("{tag}-sender"));
    support::init(&sender);
    std::fs::write(sender.join("s.txt"), "s\n".repeat(20)).unwrap();
    support::ok(
        &support::commit(&sender, "heads/main", "sender commit"),
        "commit",
    );
    support::ok(&support::seal(&sender, "heads/main"), "seal");
    let bundle = sender.join("export.bundle");
    support::ok(
        &support::prikk(&sender)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "bundle export",
    );
    let repo = support::unique_repo(tag);
    support::init(&repo);
    // The sender sealed under the fixed `support::MAINTAINER_KEY_ID`; the receiver must trust the
    // same key for `verify` to accept the received history's publication trust.
    support::trust_maintainer(&repo);
    support::ok(
        &support::prikk(&repo)
            .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
            .output()
            .unwrap(),
        "bundle import",
    );
    // Re-import: two entries under the same ref name key give the received index's own compaction
    // something to actually reclaim.
    support::ok(
        &support::prikk(&repo)
            .args(["bundle", "import", "--input", bundle.to_str().unwrap()])
            .output()
            .unwrap(),
        "second bundle import",
    );
    let (code, text) = run(&repo, &["compact", "--received-index"]);
    assert_eq!(code, Some(0), "first compact (received index): {text}");
    repo
}

fn setup_trust_policy(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    let (code, text) = run(&repo, &["compact", "--trust-policy"]);
    assert_eq!(code, Some(0), "first compact (trust policy): {text}");
    repo
}

const TARGETS: [Target; 3] = [
    Target {
        name: "pointer index",
        flag: "--pointer-index",
        generation_log: pointer_index_generation_log,
        setup: setup_pointer_index,
    },
    Target {
        name: "received index",
        flag: "--received-index",
        generation_log: received_index_generation_log,
        setup: setup_received_index,
    },
    Target {
        name: "trust policy",
        flag: "--trust-policy",
        generation_log: trust_policy_generation_log,
        setup: setup_trust_policy,
    },
];

#[test]
fn compact_refuses_on_an_unclean_generation_log_tail_then_a_manual_truncate_lets_it_through() {
    let mut failures = Vec::new();
    for target in &TARGETS {
        for (fault_name, fault) in FAULTS {
            let repo = (target.setup)(&format!(
                "rfc163-genlog-{}-{}",
                target.name.replace(' ', "-"),
                fault_name.replace(' ', "-")
            ));
            let path = (target.generation_log)(&repo);
            let original_len = read_bytes(&path).len();
            fault(&path);
            let corrupted = read_bytes(&path);
            let label = format!("{} generation log / {fault_name}", target.name);

            let before = support::store_bytes(&repo);
            let (code, text) = run(&repo, &["compact", target.flag]);
            if code.is_some_and(|code| code == 0) {
                failures.push(format!(
                    "{label}: compact must refuse on the unclean tail, but exited 0\n{text}"
                ));
            }
            let after = support::store_bytes(&repo);
            if before != after {
                failures.push(format!(
                    "{label}: every file under .prikk/ must be byte-identical after a refused compact"
                ));
            }
            if read_bytes(&path) != corrupted {
                failures.push(format!(
                    "{label}: the refused compact changed the generation log"
                ));
            }

            // The way out: manual truncate to the offset before the fault.
            let file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
            file.set_len(original_len as u64).unwrap();
            drop(file);

            let (retry_code, retry_text) = run(&repo, &["compact", target.flag]);
            if retry_code != Some(0) {
                failures.push(format!(
                    "{label}: compact after the manual truncate must succeed\n{retry_text}"
                ));
            }

            let (verify_code, verify_text) = run(&repo, &["verify"]);
            if verify_code != Some(0) {
                failures.push(format!(
                    "{label}: verify after the truncate and retry\n{verify_text}"
                ));
            }

            std::fs::write(repo.join("after.txt"), b"after\n").unwrap();
            let commit_output =
                support::commit(&repo, "heads/main", "after the generation-log repair");
            if !commit_output.status.success() {
                failures.push(format!(
                    "{label}: a commit after the truncate and retry: {}{}",
                    String::from_utf8_lossy(&commit_output.stdout),
                    String::from_utf8_lossy(&commit_output.stderr)
                ));
            }

            let _ = std::fs::remove_dir_all(&repo);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}
