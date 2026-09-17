//! RFC 157 §3 and §7.1: `prikk tree`, through the compiled binary.
//!
//! Fixture: a set-up repository whose first sealed block holds a text file, a binary file, an executable
//! script and nested paths, including `srcx/` beside `src/` and `src.rs` beside `src/`, so a string-prefix
//! match would show.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::json::Value;

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn ok(repo: &Path, args: &[&str]) -> String {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    String::from_utf8_lossy(&output.stdout).to_string()
}

fn tip(repo: &Path) -> String {
    ok(repo, &["log", "--limit", "1"])
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .expect("log names a block")
        .trim()
        .to_string()
}

/// `(path, bytes, executable)` written into the fixture's first commit, in the order written.
const FILES: &[(&str, &[u8], bool)] = &[
    ("src/main.rs", b"fn main() {}\n", false),
    ("src/nested/deep/leaf.txt", b"deep\n", false),
    ("src/big.bin", b"\xff\xfe\x00binary\x00\x01", false),
    ("srcx/a.txt", b"x\n", false),
    ("src.rs", b"// beside src/\n", false),
    ("run.sh", b"#!/bin/sh\necho hi\n", true),
];

fn fixture(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    ok(&repo, &["setup", "."]);
    for (path, bytes, executable) in FILES {
        let full = repo.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(&full, bytes).unwrap();
        #[cfg(unix)]
        if *executable {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&full, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        #[cfg(not(unix))]
        let _ = executable;
    }
    ok(&repo, &["commit", "-m", "fixture"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    repo
}

fn listing(repo: &Path, args: &[&str]) -> Value {
    let mut full = vec!["tree", "--format", "json"];
    full.extend_from_slice(args);
    support::json::parse(&ok(repo, &full))
}

fn entry_paths(listing: &Value) -> Vec<String> {
    listing
        .get("entries")
        .as_array()
        .iter()
        .map(|entry| entry.get("path").as_str().to_string())
        .collect()
}

fn has_key(value: &Value, key: &str) -> bool {
    matches!(value, Value::Object(map) if map.contains_key(key))
}

/// Control 1: every file once, in canonical order, with exact sizes equal to the materialized files, the
/// full mode, checkout's classification, and `content_id` on the binary only -- the blob `checkout`
/// names for it.
#[test]
fn control1_every_file_once_in_canonical_order_with_exact_sizes() {
    let repo = fixture("rfc157-tree-control1");
    let value = listing(&repo, &[]);
    assert_eq!(value.get("schema_version").as_str(), "tree-listing-v1");
    assert_eq!(value.get("point").as_str(), "heads/main");
    assert_eq!(value.get("target_block_id").as_str(), tip(&repo));
    assert!(value.get("prefix").is_null());

    let mut expected: Vec<&str> = FILES.iter().map(|(path, _, _)| *path).collect();
    expected.sort_unstable();
    assert_eq!(
        entry_paths(&value),
        expected,
        "each file once, in byte order"
    );

    // Materialize the point into a copy with no worktree, and compare sizes against what was written.
    let copy = support::unique_repo("rfc157-tree-control1-materialized");
    support::copy_dir_recursive(&repo.join(".prikk"), &copy.join(".prikk"));
    ok(
        &copy,
        &["checkout", "--patch-materialize", "--ref", "heads/main"],
    );

    for entry in value.get("entries").as_array() {
        let path = entry.get("path").as_str();
        let (_, _, executable) = FILES.iter().find(|(p, _, _)| *p == path).unwrap();
        let on_disk = std::fs::metadata(copy.join(path)).unwrap();
        assert_eq!(entry.get("kind").as_str(), "file", "{path}");
        assert_eq!(
            entry.get("size"),
            &Value::Number(on_disk.len().to_string()),
            "{path}: exact size"
        );
        let mode = if *executable { "33261" } else { "33188" };
        assert_eq!(
            entry.get("mode"),
            &Value::Number(mode.to_string()),
            "{path}"
        );

        let content = support::json::parse(&ok(
            &repo,
            &[
                "checkout",
                "--patch-plan",
                "--format",
                "json",
                "--content-path",
                path,
            ],
        ));
        let checkout_entry = &content.get("content").as_array()[0].get("content");
        let checkout_kind = checkout_entry.get("kind").as_str();
        assert_eq!(
            entry.get("encoding").as_str(),
            checkout_kind,
            "{path}: checkout's classification"
        );
        if checkout_kind == "binary" {
            assert_eq!(
                entry.get("content_id").as_str(),
                checkout_entry.get("blob_id").as_str(),
                "{path}: the blob checkout names"
            );
        } else {
            assert!(
                !has_key(entry, "content_id"),
                "{path}: no content_id for text"
            );
        }
    }
    assert_eq!(
        value
            .get("entries")
            .as_array()
            .iter()
            .filter(|entry| entry.get("encoding").as_str() == "binary")
            .count(),
        1
    );
    let _ = std::fs::remove_dir_all(&copy);
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2: `--prefix` matches whole path components. `src/` and `src` list `src/…` and never
/// `srcx/…` or `src.rs`.
#[test]
fn control2_prefix_matches_whole_components() {
    let repo = fixture("rfc157-tree-control2");
    for prefix in ["src/", "src"] {
        let value = listing(&repo, &["--prefix", prefix]);
        assert_eq!(value.get("prefix").as_str(), prefix);
        assert_eq!(
            entry_paths(&value),
            ["src/big.bin", "src/main.rs", "src/nested/deep/leaf.txt"],
            "--prefix {prefix}"
        );
    }
    assert_eq!(
        entry_paths(&listing(&repo, &["--prefix", "src/nested"])),
        ["src/nested/deep/leaf.txt"]
    );
    assert!(entry_paths(&listing(&repo, &["--prefix", "sr"])).is_empty());
    let prose = ok(&repo, &["tree", "--prefix", "src/"]);
    assert!(prose.contains("\nprefix: src/\n"), "{prose}");
    assert!(prose.contains("\nentries: 3\n"), "{prose}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: a block id and the ref naming it list byte-identically, but for `point`.
#[test]
fn control3_a_block_id_and_its_ref_list_identically() {
    let repo = fixture("rfc157-tree-control3");
    let block = tip(&repo);
    let by_ref = ok(&repo, &["tree", "--format", "json", "--ref", "heads/main"]);
    let by_id = ok(&repo, &["tree", "--format", "json", "--ref", &block]);
    let ref_line = "  \"point\": \"heads/main\",";
    let id_line = format!("  \"point\": \"{block}\",");
    assert!(by_ref.contains(ref_line) && by_id.contains(&id_line));
    assert_eq!(by_ref.replace(ref_line, ""), by_id.replace(&id_line, ""));
    let prose = ok(&repo, &["tree", "--ref", &block]);
    assert!(prose.contains(&format!("\nblock: {block}\n")), "{prose}");
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 4: a received ref lists (reading is not adopting), in a repository with no `heads/main`.
#[test]
fn control4_a_received_ref_lists() {
    let src = fixture("rfc157-tree-control4-src");
    let bundle = src.join("main.bundle");
    ok(
        &src,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    let dst = support::unique_repo("rfc157-tree-control4-dst");
    ok(&dst, &["init", "."]);
    ok(
        &dst,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    let received = listing(&dst, &["--ref", "remotes/heads/main"]);
    let local = listing(&src, &["--ref", "heads/main"]);
    assert_eq!(received.get("point").as_str(), "remotes/heads/main");
    assert_eq!(received.get("entries"), local.get("entries"));
    let _ = std::fs::remove_dir_all(&src);
    let _ = std::fs::remove_dir_all(&dst);
}

/// Control 5: an explicit absent ref refuses; the implicit current branch of a fresh repository lists
/// nothing, with exit 0, in prose and JSON.
#[test]
fn control5_absent_ref_refuses_and_a_fresh_branch_lists_nothing() {
    let repo = fixture("rfc157-tree-control5");
    let output = run(&repo, &["tree", "--ref", "heads/nope"]);
    assert_eq!(output.status.code(), Some(1), "{}", text(&output));
    assert!(
        text(&output).contains(
            "error: precondition not met: ref heads/nope does not exist in this repository"
        )
    );
    let output = run(&repo, &["tree", "--ref", "main"]);
    assert_eq!(output.status.code(), Some(2), "{}", text(&output));

    let fresh = support::unique_repo("rfc157-tree-control5-fresh");
    ok(&fresh, &["init", "."]);
    let value = listing(&fresh, &[]);
    assert_eq!(value.get("point").as_str(), "heads/main");
    assert!(value.get("target_block_id").is_null());
    assert!(value.get("entries").as_array().is_empty());
    let prose = ok(&fresh, &["tree"]);
    assert!(
        prose.contains("\ntarget block: <not published>\n"),
        "{prose}"
    );
    assert!(prose.contains("\nentries: 0\n"), "{prose}");
    let output = run(&fresh, &["tree", "--ref", "heads/main"]);
    assert_eq!(
        output.status.code(),
        Some(1),
        "an explicit absent ref refuses: {}",
        text(&output)
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&fresh);
}

/// Control 6: an unsupported operation anywhere in the chain fails the whole call, never a partial
/// listing (RFC 157 §5, RFC 140 §7b). A `CreateSymlink` has no authoring path, so it is written as a raw
/// patch and sealed through the test-support seal, as RFC 143's control 3 does.
#[test]
fn control6_an_unsupported_operation_fails_the_call() {
    use prikk_object::{
        BlobKind, BlobPayload, CanonicalEncode, CreateFile, CreateSymlink, NodeId, ObjectEnvelope,
        ObjectType, Operation, OperationKind, PatchPayload, PatchPurpose,
    };
    use prikk_store::{
        Ed25519AuthorSigner, Ed25519MaintainerSigner, ObjectWriteSession, ObjectWriter,
        RepositoryLayout, Wal, add_trusted_maintainer, author_signature,
        simulate_one_seal_for_test_support, write_active_ref_metadata,
    };

    let repo = support::unique_repo("rfc157-tree-control6");
    let layout = RepositoryLayout::init(repo.clone()).unwrap();
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .unwrap();
    let author = Ed25519AuthorSigner::from_seed("rfc157-control6-author", &[0x62; 32]).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();
    let blob_id = {
        let bytes = BlobPayload::new(BlobKind::Text, b"hello\n".to_vec())
            .to_canonical_bytes()
            .unwrap();
        let envelope = ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes);
        ObjectWriteSession::open(&layout)
            .unwrap()
            .write_object(&envelope)
            .unwrap()
    };
    std::fs::write(repo.join("a.txt"), "hello\n").unwrap();
    write_active_ref_metadata(&layout, "heads/main").unwrap();
    let payload = PatchPayload {
        operations: vec![
            Operation {
                op_seq: 1,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateFile(CreateFile {
                    path: "a.txt".to_string(),
                    node_id: NodeId::from_bytes([0x73; 32]),
                    blob_id,
                    mode: 0o100_644,
                }),
            },
            Operation {
                op_seq: 2,
                op_id: None,
                preconditions: Vec::new(),
                kind: OperationKind::CreateSymlink(CreateSymlink {
                    path: "link".to_string(),
                    node_id: NodeId::from_bytes([0x74; 32]),
                    target: "a.txt".to_string(),
                }),
            },
        ],
        intent: None,
        preconditions: Vec::new(),
        purpose: PatchPurpose::Normal,
        message: None,
    };
    let mut envelope =
        ObjectEnvelope::unsigned(ObjectType::Patch, 1, payload.to_canonical_bytes().unwrap());
    let id = envelope.object_id();
    envelope
        .add_signature(author_signature(&author, id).unwrap())
        .unwrap();
    Wal::for_layout(&layout, prikk_store::DEFAULT_ACTIVE_NAME)
        .append_patch(&envelope)
        .unwrap();
    simulate_one_seal_for_test_support(&layout, "heads/main", &maintainer).unwrap();

    for format in [&["--format", "json"][..], &[][..]] {
        let mut args = vec!["tree"];
        args.extend_from_slice(format);
        let output = run(&repo, &args);
        assert_eq!(output.status.code(), Some(1), "{}", text(&output));
        let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
        assert!(
            stderr.contains("symlink") || stderr.contains("unsupported"),
            "the refusal names the operation: {stderr}"
        );
        assert!(
            output.stdout.is_empty(),
            "no partial listing: {}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 7: snapshot cadence does not show. The first block is a checkpoint and the second is not; the
/// entries the second block did not change are byte-identical in both listings, `content_id` included.
#[test]
fn control7_cadence_does_not_show() {
    let repo = fixture("rfc157-tree-control7");
    let checkpoint = tip(&repo);
    let plan = ok(&repo, &["checkout", "--plan-only", "--ref", &checkpoint]);
    assert!(
        plan.contains("\nsnapshot blob: ") && !plan.contains("\nsnapshot blob: <none>"),
        "the first block is a checkpoint: {plan}"
    );
    std::fs::write(repo.join("src/main.rs"), b"fn main() { println!(); }\n").unwrap();
    ok(&repo, &["commit", "-m", "edit main.rs"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let after = tip(&repo);
    let plan = ok(&repo, &["checkout", "--plan-only", "--ref", &after]);
    assert!(
        plan.contains("\nsnapshot blob: <none>\n"),
        "the second block is not: {plan}"
    );

    let at_checkpoint = listing(&repo, &["--ref", &checkpoint]);
    let one_after = listing(&repo, &["--ref", &after]);
    let entries = |value: &Value| value.get("entries").as_array().to_vec();
    let (before, later) = (entries(&at_checkpoint), entries(&one_after));
    assert_eq!(before.len(), later.len());
    for (a, b) in before.iter().zip(&later) {
        if a.get("path").as_str() == "src/main.rs" {
            assert_ne!(a.get("size"), b.get("size"));
            assert!(!has_key(a, "content_id") && !has_key(b, "content_id"));
        } else {
            assert_eq!(a, b, "{} changed with cadence", a.get("path").as_str());
        }
    }
    let _ = std::fs::remove_dir_all(&repo);
}
