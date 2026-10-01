//! RFC 164 Addendum 3 item 3: every reader, in the suite, not a scratch probe.
//!
//! Mirrors the architect's own `arch-seal/rfc164_all_files_reader_probe.sh` exactly in shape: one
//! combined fixture with real content in all eight covered files (the seven Rule-A files and the
//! pointer index), each with at least two records. For the last record of each file, one byte is
//! flipped in each of five fields -- magic, version, length, checksum, body -- and, before and after
//! `doctor --repair-tails`, every one of five readers (`trust maintainer list`, `branch list`, `sync
//! tags`, `status`, `log`) must either refuse or print *exactly* its own pre-fault baseline. The
//! baseline is captured at the same working directory every flip run reuses, so `status`/`log`'s own
//! printed repository path never differs from the comparison (the review's own first-run bug: a
//! differing path made every comparison a false rollback).

#![allow(clippy::expect_used, clippy::unwrap_used, clippy::indexing_slicing)]

mod support;

use std::path::{Path, PathBuf};

/// magic(8) version(2) body_len(8) checksum(32) -- shared by every one of the eight covered files.
const HEADER_LEN: usize = 50;

fn read_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).expect("target file exists")
}

fn flip(path: &Path, offset: usize) {
    let mut bytes = read_bytes(path);
    bytes[offset] ^= 0xFF;
    std::fs::write(path, &bytes).unwrap();
}

/// `(offset, total_len)` of the last complete frame in `bytes`, scanning from the start the same way
/// every decoder's own isolate-and-continue loop does, but trusting only sound frames (stops at the
/// first one that does not parse as whole).
fn last_record_span(bytes: &[u8]) -> (usize, usize) {
    let magic = bytes[0..8].to_vec();
    let mut offset = 0_usize;
    let mut last = None;
    while offset + HEADER_LEN <= bytes.len() && bytes[offset..offset + 8] == magic[..] {
        let body_len =
            u64::from_be_bytes(bytes[offset + 10..offset + 18].try_into().unwrap()) as usize;
        if offset + HEADER_LEN + body_len > bytes.len() {
            break;
        }
        last = Some((offset, HEADER_LEN + body_len));
        offset += HEADER_LEN + body_len;
    }
    last.expect("at least one complete record")
}

/// The five readers the handoff names, each refusing or captured as exact text.
const READER_COMMANDS: [(&str, &[&str]); 5] = [
    ("trust maintainer list", &["trust", "maintainer", "list"]),
    ("branch list", &["branch", "list"]),
    ("sync tags", &["sync", "tags"]),
    ("status", &["status"]),
    ("log", &["log"]),
];

#[derive(Clone, PartialEq, Eq, Debug)]
enum ReaderOutcome {
    Refused,
    Output(String),
}

fn run_readers(repo: &Path) -> Vec<(&'static str, ReaderOutcome)> {
    READER_COMMANDS
        .iter()
        .map(|(name, args)| {
            let output = support::prikk(repo).args(*args).output().unwrap();
            let outcome = if output.status.success() {
                ReaderOutcome::Output(format!(
                    "{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr)
                ))
            } else {
                ReaderOutcome::Refused
            };
            (*name, outcome)
        })
        .collect()
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

/// Build the one combined fixture every file's own flip test shares: real content in all eight
/// covered files, each with at least two records, mirroring the architect's own probe build order
/// exactly (compactions before the trust churn, so the generation logs get real records too).
fn build_fixture(tag: &str) -> PathBuf {
    let src = support::unique_repo(&format!("{tag}-src"));
    support::init(&src);
    std::fs::write(src.join("a.txt"), "a\n").unwrap();
    support::ok(&support::commit(&src, "heads/main", "a"), "commit a");
    support::ok(&support::seal(&src, "heads/main"), "seal a");
    let bundle1 = src.join("b1.bundle");
    support::ok(
        &support::prikk(&src)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle1.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "export b1",
    );
    std::fs::write(src.join("b.txt"), "b\n").unwrap();
    support::ok(&support::commit(&src, "heads/main", "b"), "commit b");
    support::ok(&support::seal(&src, "heads/main"), "seal b");
    let bundle2 = src.join("b2.bundle");
    support::ok(
        &support::prikk(&src)
            .args([
                "bundle",
                "export",
                "--ref",
                "heads/main",
                "--output",
                bundle2.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "export b2",
    );

    let base = support::unique_repo(&format!("{tag}-base"));
    support::init(&base);
    std::fs::write(base.join("f1.txt"), "1\n").unwrap();
    support::ok(&support::commit(&base, "heads/main", "c1"), "commit c1");
    support::ok(&support::seal(&base, "heads/main"), "seal c1");
    support::ok(
        &support::prikk(&base)
            .args(["bundle", "import", "--input", bundle1.to_str().unwrap()])
            .output()
            .unwrap(),
        "import b1",
    );
    let (code, text) = add_maintainer(&base, "a3-k1", [0x61; 32]);
    assert_eq!(code, Some(0), "trust add k1: {text}");
    for (index, name) in ["g1", "g2"].into_iter().enumerate() {
        let (code, text) = run(&base, &["compact", "--all"]);
        assert!(
            code == Some(0) || index > 0,
            "compact --all #{index}: {text}"
        );
        std::fs::write(base.join(format!("{name}.txt")), format!("{name}\n")).unwrap();
        support::ok(&support::commit(&base, "heads/main", name), "commit g");
        support::ok(&support::seal(&base, "heads/main"), "seal g");
    }
    let (code, text) = add_maintainer(&base, "a3-k2", [0x62; 32]);
    assert_eq!(code, Some(0), "trust add k2: {text}");
    let (code, text) = run(
        &base,
        &["trust", "maintainer", "remove", "--key-id", "a3-k1"],
    );
    assert_eq!(code, Some(0), "trust remove k1: {text}");
    std::fs::write(base.join("h1.txt"), "a1\n").unwrap();
    let (code, text) = commit_by(&base, "a3-au1", AU1_SEED_HEX, "au1");
    assert_eq!(code, Some(0), "commit au1: {text}");
    support::ok(&support::seal(&base, "heads/main"), "seal au1");
    std::fs::write(base.join("h2.txt"), "a2\n").unwrap();
    let (code, text) = commit_by(&base, "a3-au2", AU2_SEED_HEX, "au2");
    assert_eq!(code, Some(0), "commit au2: {text}");
    support::ok(&support::seal(&base, "heads/main"), "seal au2");
    support::ok(
        &support::prikk(&base)
            .args(["bundle", "import", "--input", bundle2.to_str().unwrap()])
            .output()
            .unwrap(),
        "import b2",
    );
    let branch_output = support::branch_create(&base, "heads/keep", "heads/main");
    assert!(
        branch_output.status.success(),
        "branch create heads/keep: {}{}",
        String::from_utf8_lossy(&branch_output.stdout),
        String::from_utf8_lossy(&branch_output.stderr)
    );
    let (verify_code, verify_text) = run(&base, &["verify"]);
    assert_eq!(verify_code, Some(0), "fixture verify: {verify_text}");
    base
}

const AU1_SEED_HEX: &str = "b6d546efe86546b00fd5a721431dfdee25715663e44db1a646a3184aeb539b29";
const AU2_SEED_HEX: &str = "9e9238e39f6faa63f12a6ea3c2b06f3a56be275b3e846299d6cf8c037d6a1ddb";

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

fn add_maintainer(repo: &Path, key_id: &str, seed: [u8; 32]) -> (Option<i32>, String) {
    use prikk_store::MaintainerSigner;
    let signer = prikk_store::Ed25519MaintainerSigner::from_seed(key_id, &seed).expect("signer");
    let public_key_hex = support::hex(&signer.public_key_bytes());
    run(
        repo,
        &[
            "trust",
            "maintainer",
            "add",
            "--key-id",
            key_id,
            "--public-key",
            &public_key_hex,
        ],
    )
}

/// The larger of an `a`/`b` slot pair is the live one -- generation logs name it precisely, but size
/// suffices for a fixture that was built with real churn.
fn live_slot(base: &Path, stem: &str) -> PathBuf {
    let a = base.join(format!("{stem}-a.container"));
    let b = base.join(format!("{stem}-b.container"));
    let a_len = std::fs::metadata(&a).map(|m| m.len()).unwrap_or(0);
    let b_len = std::fs::metadata(&b).map(|m| m.len()).unwrap_or(0);
    if a_len >= b_len { a } else { b }
}

fn covered_files(base: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![
        ("trust keys", base.join(".prikk/trust/keys.container")),
        (
            "author keys",
            base.join(".prikk/trust/author-keys.container"),
        ),
        (
            "trust policy",
            live_slot(&base.join(".prikk/trust"), "policy"),
        ),
        (
            "received index",
            live_slot(&base.join(".prikk/refs/containers"), "received-index"),
        ),
        (
            "pointer index",
            live_slot(&base.join(".prikk/refs/containers"), "pointer-index"),
        ),
        (
            "trust policy generation log",
            base.join(".prikk/trust/policy-generation.log"),
        ),
        (
            "received index generation log",
            base.join(".prikk/refs/containers/received-index-generation.log"),
        ),
        (
            "pointer index generation log",
            base.join(".prikk/refs/containers/pointer-index-generation.log"),
        ),
    ]
}

/// One field offset within the last record, relative to its own start.
const FIELD_OFFSETS: [(&str, usize); 5] = [
    ("magic", 0),
    ("version", 8),
    ("length", 10),
    ("checksum", 18),
    ("body", 50), // + half the body, computed per record
];

#[test]
fn every_reader_refuses_or_matches_baseline_for_every_field_of_every_covered_file() {
    let base = build_fixture("rfc164-a3-reader");
    let flip_dir = support::unique_repo("rfc164-a3-reader-flip");
    // The baseline is captured at this exact path, reused for every flip below, so `status`/`log`'s
    // own printed repository path is identical in the baseline and in every comparison.
    copy_dir(&base, &flip_dir);
    let baseline = run_readers(&flip_dir);
    let _ = std::fs::remove_dir_all(&flip_dir);

    let mut failures = Vec::new();
    for (label, path) in covered_files(&base) {
        let bytes = read_bytes(&path);
        let (record_offset, record_len) = last_record_span(&bytes);
        let relative = path.strip_prefix(&base).unwrap().to_path_buf();

        for (field, field_offset) in FIELD_OFFSETS {
            let byte_offset = if field == "body" {
                record_offset + 50 + (record_len.saturating_sub(50) / 2)
            } else {
                record_offset + field_offset
            };
            if byte_offset >= record_offset + record_len {
                continue; // a short body (e.g. the generation log's own 1-byte body) has no distinct checksum/body split
            }
            let cell = format!("{label} / {field}");

            copy_dir(&base, &flip_dir);
            flip(&flip_dir.join(&relative), byte_offset);

            let (verify_code, verify_text) = run(&flip_dir, &["verify"]);
            if verify_code == Some(0) {
                failures.push(format!(
                    "{cell}: verify must fail on a flipped {field} byte, exited 0\n{verify_text}"
                ));
            }

            let before = run_readers(&flip_dir);
            check_readers(&cell, "before repair", &baseline, &before, &mut failures);

            let (repair_code, repair_text) = run(&flip_dir, &["doctor", "--repair-tails"]);
            if repair_code == Some(0) {
                failures.push(format!(
                    "{cell}: --repair-tails must refuse on a flipped {field} byte, exited 0\n{repair_text}"
                ));
            }

            let after = run_readers(&flip_dir);
            check_readers(&cell, "after repair", &baseline, &after, &mut failures);

            let _ = std::fs::remove_dir_all(&flip_dir);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n---\n"));
}

fn check_readers(
    cell: &str,
    phase: &str,
    baseline: &[(&'static str, ReaderOutcome)],
    current: &[(&'static str, ReaderOutcome)],
    failures: &mut Vec<String>,
) {
    for ((name, base_outcome), (_, cur_outcome)) in baseline.iter().zip(current.iter()) {
        let ok = matches!(cur_outcome, ReaderOutcome::Refused) || cur_outcome == base_outcome;
        if !ok {
            failures.push(format!(
                "{cell} ({phase}): reader `{name}` neither refused nor matched its baseline -- it \
                 returned an older or different state"
            ));
        }
    }
}

fn copy_dir(from: &Path, to: &Path) {
    let _ = std::fs::remove_dir_all(to);
    copy_dir_recursive(from, to);
}

fn copy_dir_recursive(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let file_type = entry.file_type().unwrap();
        let dest = to.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&entry.path(), &dest);
        } else if file_type.is_symlink() {
            // Not expected under `.prikk/`; skip rather than mis-copy.
        } else {
            std::fs::copy(entry.path(), &dest).unwrap();
        }
    }
}
