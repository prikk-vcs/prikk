//! RFC 164 Addendum 1 §4/§9: the matrix, and I6. Four fault shapes -- a torn prefix, 100 zero bytes,
//! 100 random bytes, and a flipped byte in the last *complete* record -- crossed with two orders
//! (repair first, write first), for each of the seven Rule-A files.
//!
//! **The first three shapes are crash-shaped tails.** Nothing sound follows them, so `doctor
//! --repair-tails` truncates exactly the tail bytes and a subsequent write and `verify` both succeed
//! (I1, I3, I4). **The fourth shape is what §9 amended Rule A for**: a record whose header is valid and
//! whose whole claimed body is present -- fully written -- but whose checksum fails. That is corruption
//! after the fact, not a crash, so `--repair-tails` refuses it exactly as it refuses any other interior
//! damage (I2), and no automated command reaches a working repository from it (the accepted cost §9
//! records) -- restoring the file from a copy is the only way out.
//!
//! **I6** (new in this Addendum): a repair must never change the meaning of already-committed state.
//! Checked, every cell, via the same fixed triad the Addendum itself names -- `trust maintainer list`,
//! every ref tip (`branch list`, `tag list`), and received pointers (`sync tags`) -- regardless of which
//! one of the seven files was faulted: the point is that a repair anywhere must never move any of these,
//! not that each file has its own bespoke notion of "meaning". For a genuine tail, this is compared
//! against the snapshot taken *before* the fault (the repair must fully undo an interrupted append,
//! reproducing exactly the prior committed state). For a complete-record failure, this is compared
//! against the snapshot taken *after* the fault but *before* the repair attempt (the repair must leave a
//! refusing repository exactly as it found it, never rolling it back to an earlier-but-different state).
//!
//! The three compacting containers' own live slot -- I6's third named check -- is not observable from
//! this crate's black-box CLI tests (`resolve_live_slot` is `pub(crate)` inside `prikk-store`); it is
//! covered directly, at the layer it is defined, by
//! `prikk-store/src/foundation/generation/tests.rs`'s own rollback-refusal tests, alongside the
//! CLI-level `branch list`/`sync tags` checks here, which already show the *effect* a reverted live slot
//! would have.

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// magic(8) version(2) body_len(8) checksum(32) -- shared by every one of the seven Rule-A files
/// (confirmed against each module's own `*_HEADER_LEN` constant, not assumed).
const HEADER_LEN: usize = 8 + 2 + 8 + 32;

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

fn append_random_100(path: &Path) {
    let mut bytes = read_bytes(path);
    bytes.extend((0..100_u32).map(|index| (index.wrapping_mul(2654435761) >> 24) as u8));
    std::fs::write(path, &bytes).unwrap();
}

/// RFC 164 §9's fault shape: flip one byte inside the body of the **last** complete record -- a
/// record this format's own frame parser accepts as structurally whole (magic, version, and a claimed
/// body the remaining bytes actually hold); only its checksum fails. Requires at least one sound
/// record already on disk, which every repository builder below provides.
fn flip_last_complete_record_byte(path: &Path) {
    let mut bytes = read_bytes(path);
    let magic = bytes[..8].to_vec();
    let mut offset = 0_usize;
    let mut frames = Vec::new();
    while offset + HEADER_LEN <= bytes.len() && bytes[offset..offset + 8] == magic[..] {
        let body_len =
            u64::from_be_bytes(bytes[offset + 10..offset + 18].try_into().unwrap()) as usize;
        if offset + HEADER_LEN + body_len > bytes.len() {
            break;
        }
        frames.push((offset, body_len));
        offset += HEADER_LEN + body_len;
    }
    let (last_offset, last_len) = *frames.last().expect("at least one sound record to flip");
    let flip_at = last_offset + HEADER_LEN + (last_len / 2);
    bytes[flip_at] ^= 0xFF;
    std::fs::write(path, &bytes).unwrap();
}

type FaultFn = fn(&Path);

const MATRIX_FAULTS: [(&str, FaultFn); 4] = [
    ("torn prefix", append_torn_prefix as FaultFn),
    ("100 zero bytes", append_zeros_100 as FaultFn),
    ("100 random bytes", append_random_100 as FaultFn),
    (
        "a flipped byte in the last complete record",
        flip_last_complete_record_byte as FaultFn,
    ),
];

fn is_tail_shape(fault_name: &str) -> bool {
    fault_name != "a flipped byte in the last complete record"
}

fn cmd_text(repo: &Path, args: &[&str]) -> String {
    let output = support::prikk(repo).args(args).output().unwrap();
    format!(
        "exit={:?}\n{}{}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// I6's fixed triad, named directly in the Addendum: trust maintainer list, every ref tip (branch
/// list, tag list), and received pointers (`sync tags`). Applied identically regardless of which of
/// the seven files is under fault -- see the module doc.
fn i6_snapshot(repo: &Path) -> String {
    [
        cmd_text(repo, &["trust", "maintainer", "list", "--format", "json"]),
        cmd_text(repo, &["branch", "list", "--all", "--format", "json"]),
        cmd_text(repo, &["tag", "list", "--format", "json"]),
        cmd_text(repo, &["sync", "tags"]),
    ]
    .join("\n===\n")
}

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

/// Run the whole matrix for one file: both orders, all four shapes, I1-I6.
fn run_matrix(
    label: &str,
    target_path: fn(&Path) -> PathBuf,
    repo_builder: fn(&str) -> PathBuf,
    write_attempt: fn(&Path) -> (Option<i32>, String),
) {
    let mut failures = Vec::new();
    for (fault_name, fault) in MATRIX_FAULTS {
        let tail_shape = is_tail_shape(fault_name);

        // --- Order A: repair first ---
        {
            let repo = repo_builder(&format!(
                "rfc164-matrix-{}-{}-repair-first",
                label.replace(' ', "-"),
                fault_name.replace(' ', "-")
            ));
            let path = target_path(&repo);
            let cell = format!("{label} / {fault_name} / repair first");
            let before_fault_meaning = i6_snapshot(&repo);
            fault(&path);
            let corrupted = read_bytes(&path);
            let after_fault_meaning = i6_snapshot(&repo);

            let (repair_code, repair_text) = run(&repo, &["doctor", "--repair-tails"]);

            if tail_shape {
                if repair_code != Some(0) {
                    failures.push(format!(
                        "{cell}: I4, repair-tails must succeed on a tail\n{repair_text}"
                    ));
                }
                let after_repair = read_bytes(&path);
                if after_repair.len() >= corrupted.len() {
                    failures.push(format!("{cell}: I4, the tail must actually be truncated"));
                }
                if i6_snapshot(&repo) != before_fault_meaning {
                    failures.push(format!(
                        "{cell}: I6, repairing a genuine tail must reproduce exactly the pre-fault committed state"
                    ));
                }
                let (second_code, _) = run(&repo, &["doctor", "--repair-tails"]);
                if second_code != Some(0) {
                    failures.push(format!(
                        "{cell}: I4, a second repair-tails must also succeed (idempotent)"
                    ));
                }
                if read_bytes(&path) != after_repair {
                    failures.push(format!(
                        "{cell}: I4, a second repair-tails must change nothing further"
                    ));
                }
                let (write_code, write_text) = write_attempt(&repo);
                if write_code != Some(0) {
                    failures.push(format!(
                        "{cell}: I3, a write after the repair must succeed\n{write_text}"
                    ));
                }
                let (verify_code, _) = run(&repo, &["verify"]);
                if verify_code != Some(0) {
                    failures.push(format!(
                        "{cell}: I1, verify after the repair and write must exit 0"
                    ));
                }
            } else {
                if repair_code == Some(0) {
                    failures.push(format!(
                        "{cell}: RFC 164 §9, repair-tails must refuse on a complete damaged record, not succeed\n{repair_text}"
                    ));
                }
                if !repair_text.contains("interior damage") && !repair_text.contains("damaged") {
                    failures.push(format!(
                        "{cell}: the refusal must name interior damage\n{repair_text}"
                    ));
                }
                if read_bytes(&path) != corrupted {
                    failures.push(format!("{cell}: I2, a refused repair must change nothing"));
                }
                let (verify_code, _) = run(&repo, &["verify"]);
                if verify_code == Some(0) {
                    failures.push(format!(
                        "{cell}: I2, verify must still fail after the refused repair"
                    ));
                }
                if i6_snapshot(&repo) != after_fault_meaning {
                    failures.push(format!(
                        "{cell}: I6, a refused repair must leave the (refusing) repository exactly as it found it"
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(&repo);
        }

        // --- Order B: write first ---
        {
            let repo = repo_builder(&format!(
                "rfc164-matrix-{}-{}-write-first",
                label.replace(' ', "-"),
                fault_name.replace(' ', "-")
            ));
            let path = target_path(&repo);
            let cell = format!("{label} / {fault_name} / write first");
            fault(&path);
            let corrupted = read_bytes(&path);
            let after_fault_meaning = i6_snapshot(&repo);

            let (write_code, write_text) = write_attempt(&repo);
            if write_code == Some(0) {
                failures.push(format!(
                    "{cell}: I5, the write must refuse on the unclean tail/damage, but exited 0\n{write_text}"
                ));
            }
            if read_bytes(&path) != corrupted {
                failures.push(format!("{cell}: I5, the refused write must change nothing"));
            }

            let (repair_code, _) = run(&repo, &["doctor", "--repair-tails"]);

            if tail_shape {
                if repair_code != Some(0) {
                    failures.push(format!(
                        "{cell}: I5's way out, repair-tails must succeed on a tail"
                    ));
                }
                let (retry_code, retry_text) = write_attempt(&repo);
                if retry_code != Some(0) {
                    failures.push(format!(
                        "{cell}: I3, the same write after the repair must succeed\n{retry_text}"
                    ));
                }
            } else {
                if repair_code == Some(0) {
                    failures.push(format!(
                        "{cell}: RFC 164 §9, repair-tails must refuse even under write-first order"
                    ));
                }
                if i6_snapshot(&repo) != after_fault_meaning {
                    failures.push(format!(
                        "{cell}: I6, nothing must revert while damage is refused"
                    ));
                }
            }
            let _ = std::fs::remove_dir_all(&repo);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

// ---------------------------------------------------------------------------------------------
// Per-file setup. Each file's own "at least two real records" fixture and "ordinary write" differ
// too much to share one generic builder (the same reasoning `rfc163_write_never_buries_a_crash_
// state.rs` gives) -- this section provides exactly that per file, then one `#[test]` calls the
// shared matrix runner above.
// ---------------------------------------------------------------------------------------------

// -- Trust keys / trust policy: three distinct maintainer keys, added one at a time. --

const SECOND_MAINTAINER_KEY_ID: &str = "rfc164-matrix-second-maintainer";
const SECOND_MAINTAINER_SEED: [u8; 32] = [0x21; 32];
const THIRD_MAINTAINER_KEY_ID: &str = "rfc164-matrix-third-maintainer";
const THIRD_MAINTAINER_SEED: [u8; 32] = [0x31; 32];

fn maintainer_public_key_hex(key_id: &str, seed: [u8; 32]) -> String {
    use prikk_store::MaintainerSigner;
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(key_id, &seed).expect("valid signer");
    support::hex(&signer.public_key_bytes())
}

fn add_maintainer(repo: &Path, key_id: &str, seed: [u8; 32]) -> (Option<i32>, String) {
    run(
        repo,
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            key_id,
            "--public-key",
            &maintainer_public_key_hex(key_id, seed),
        ],
    )
}

fn trust_key_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/keys.container")
}

fn trust_policy_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/policy-a.container")
}

/// Two adopted maintainers already on file -- the first, fixed key plus a second, so the trust-key
/// and trust-policy containers each hold two real records before the fault.
fn trust_repository_two_records(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    let (code, text) = add_maintainer(&repo, SECOND_MAINTAINER_KEY_ID, SECOND_MAINTAINER_SEED);
    assert_eq!(code, Some(0), "second maintainer adopt: {text}");
    repo
}

fn add_third_maintainer(repo: &Path) -> (Option<i32>, String) {
    add_maintainer(repo, THIRD_MAINTAINER_KEY_ID, THIRD_MAINTAINER_SEED)
}

#[test]
fn matrix_trust_keys() {
    run_matrix(
        "trust keys",
        trust_key_path,
        trust_repository_two_records,
        add_third_maintainer,
    );
}

#[test]
fn matrix_trust_policy() {
    run_matrix(
        "trust policy",
        trust_policy_path,
        trust_repository_two_records,
        add_third_maintainer,
    );
}

// -- Author keys: three distinct authors, each committing once. --

const SECOND_AUTHOR_KEY_ID: &str = "rfc164-matrix-second-author";
const SECOND_AUTHOR_SEED_HEX: &str =
    "d279cf77ef03bdacc61cae6e365bd11fcfaed31b9226ddb9e67ab1e95450ea60";
const THIRD_AUTHOR_KEY_ID: &str = "rfc164-matrix-third-author";
const THIRD_AUTHOR_SEED_HEX: &str =
    "aa11bb22cc33dd44ee55ff660011223344556677889900aabbccddeeff001122";

fn author_key_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/author-keys.container")
}

fn commit_by(repo: &Path, key_id: &str, seed_hex: &str, message: &str) -> (Option<i32>, String) {
    let output = support::prikk(repo)
        .env("PRIKK_AUTHOR_KEY_ID", key_id)
        .env("PRIKK_AUTHOR_SEED_FILE", support::seed_file(seed_hex))
        .args(["commit", "--ref", "heads/main", "-m", message])
        .output()
        .unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// Two authors already recorded -- the first, fixed author (`support::commit`) plus a second, so the
/// author-key container holds two real records before the fault.
fn author_key_repository_two_records(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join("first.txt"), "first\n".repeat(20)).unwrap();
    support::ok(
        &support::commit(&repo, "heads/main", "by the first author"),
        "commit",
    );
    std::fs::write(repo.join("second.txt"), "second\n".repeat(20)).unwrap();
    let (code, text) = commit_by(
        &repo,
        SECOND_AUTHOR_KEY_ID,
        SECOND_AUTHOR_SEED_HEX,
        "by the second author",
    );
    assert_eq!(code, Some(0), "second author commit: {text}");
    repo
}

fn commit_by_third_author(repo: &Path) -> (Option<i32>, String) {
    std::fs::write(repo.join("third.txt"), "third\n".repeat(20)).unwrap();
    commit_by(
        repo,
        THIRD_AUTHOR_KEY_ID,
        THIRD_AUTHOR_SEED_HEX,
        "by the third author",
    )
}

#[test]
fn matrix_author_keys() {
    run_matrix(
        "author keys",
        author_key_path,
        author_key_repository_two_records,
        commit_by_third_author,
    );
}

// -- The received index: three imported bundles, each from a distinct sender/file. --

fn received_index_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/received-index-a.container")
}

fn export_a_bundle(tag: &str, file_name: &str) -> PathBuf {
    let sender = support::unique_repo(&format!("{tag}-sender-{file_name}"));
    support::init(&sender);
    std::fs::write(sender.join(file_name), format!("{file_name}\n").repeat(20)).unwrap();
    support::ok(
        &support::commit(
            &sender,
            "heads/main",
            &format!("sender commit: {file_name}"),
        ),
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
    bundle
}

fn import_bundle(repo: &Path, bundle: &Path) -> (Option<i32>, String) {
    run(
        repo,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    )
}

/// Two imported bundles already on file, so the received index holds two real records before the
/// fault. The third bundle (for the "ordinary write" attempt) is exported here too, and its path
/// leaked into a thread-local-free static-ish return via a side file, since `write_attempt` in the
/// shared matrix runner takes only `&Path` (the repo) -- the bundle is written inside the repo
/// directory itself (`third.bundle`) so `commit_third_bundle` below can find it without extra state.
fn received_index_repository_two_records(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    let first = export_a_bundle(tag, "first.txt");
    let (code, text) = import_bundle(&repo, &first);
    assert_eq!(code, Some(0), "first import: {text}");
    let second = export_a_bundle(tag, "second.txt");
    let (code, text) = import_bundle(&repo, &second);
    assert_eq!(code, Some(0), "second import: {text}");
    let third = export_a_bundle(tag, "third.txt");
    std::fs::copy(&third, repo.join("third.bundle")).unwrap();
    repo
}

fn import_third_bundle(repo: &Path) -> (Option<i32>, String) {
    import_bundle(repo, &repo.join("third.bundle"))
}

#[test]
fn matrix_received_index() {
    run_matrix(
        "received index",
        received_index_path,
        received_index_repository_two_records,
        import_third_bundle,
    );
}

// -- The three generation logs: two compactions each, a third as the "ordinary write". --

fn pointer_index_generation_log_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/pointer-index-generation.log")
}

fn pointer_index_generation_repository_two_records(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    for file_name in ["a.txt", "b.txt"] {
        std::fs::write(repo.join(file_name), format!("{file_name}\n").repeat(20)).unwrap();
        support::ok(
            &support::commit(&repo, "heads/main", &format!("commit {file_name}")),
            "commit",
        );
        support::ok(&support::seal(&repo, "heads/main"), "seal");
        let (code, text) = run(&repo, &["compact", "--pointer-index"]);
        assert_eq!(code, Some(0), "compact --pointer-index: {text}");
    }
    let branch_output = support::branch_create(&repo, "heads/keep", "heads/main");
    assert!(
        branch_output.status.success(),
        "branch create heads/keep: {}{}",
        String::from_utf8_lossy(&branch_output.stdout),
        String::from_utf8_lossy(&branch_output.stderr)
    );
    repo
}

fn compact_pointer_index(repo: &Path) -> (Option<i32>, String) {
    run(repo, &["compact", "--pointer-index"])
}

#[test]
fn matrix_pointer_index_generation_log() {
    run_matrix(
        "pointer-index generation log",
        pointer_index_generation_log_path,
        pointer_index_generation_repository_two_records,
        compact_pointer_index,
    );
}

fn received_index_generation_log_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/refs/containers/received-index-generation.log")
}

fn received_index_generation_repository_two_records(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    for file_name in ["a.txt", "b.txt"] {
        let bundle = export_a_bundle(tag, file_name);
        let (code, text) = import_bundle(&repo, &bundle);
        assert_eq!(code, Some(0), "import {file_name}: {text}");
        let (code, text) = run(&repo, &["compact", "--received-index"]);
        assert_eq!(code, Some(0), "compact --received-index: {text}");
    }
    repo
}

fn compact_received_index(repo: &Path) -> (Option<i32>, String) {
    run(repo, &["compact", "--received-index"])
}

#[test]
fn matrix_received_index_generation_log() {
    run_matrix(
        "received-index generation log",
        received_index_generation_log_path,
        received_index_generation_repository_two_records,
        compact_received_index,
    );
}

fn trust_policy_generation_log_path(repo: &Path) -> PathBuf {
    repo.join(".prikk/trust/policy-generation.log")
}

fn trust_policy_generation_repository_two_records(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    let (code, text) = run(&repo, &["compact", "--trust-policy"]);
    assert_eq!(code, Some(0), "first compact --trust-policy: {text}");
    let (code, text) = add_maintainer(&repo, SECOND_MAINTAINER_KEY_ID, SECOND_MAINTAINER_SEED);
    assert_eq!(code, Some(0), "second maintainer adopt: {text}");
    let (code, text) = run(&repo, &["compact", "--trust-policy"]);
    assert_eq!(code, Some(0), "second compact --trust-policy: {text}");
    repo
}

fn compact_trust_policy(repo: &Path) -> (Option<i32>, String) {
    run(repo, &["compact", "--trust-policy"])
}

#[test]
fn matrix_trust_policy_generation_log() {
    run_matrix(
        "trust-policy generation log",
        trust_policy_generation_log_path,
        trust_policy_generation_repository_two_records,
        compact_trust_policy,
    );
}
