//! RFC 153 §6.1 and the diff handoff's Stage 1: `prikk diff --from A --to B`, through the compiled binary.
//!
//! Fixture: two sealed blocks. The first holds a file of every kind a diff has to tell apart; the second
//! changes each of them in a different way, so every reachable status appears exactly once. **The expected
//! contents are the fixture's own constants**, never read back from prikk, so the round-trip control compares
//! the renderer against something the renderer did not produce.

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

fn ok(repo: &Path, args: &[&str]) -> Output {
    let output = run(repo, args);
    support::ok(&output, &args.join(" "));
    output
}

fn stdout_of(repo: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&ok(repo, args).stdout).into_owned()
}

fn refuses(repo: &Path, args: &[&str], code: i32, expected: &str) {
    let output = run(repo, args);
    assert_eq!(
        output.status.code(),
        Some(code),
        "{}: {}",
        args.join(" "),
        text(&output)
    );
    assert!(
        text(&output).contains(expected),
        "{}: expected {expected:?} in {}",
        args.join(" "),
        text(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "{}: a refusal prints nothing on stdout: {:?}",
        args.join(" "),
        String::from_utf8_lossy(&output.stdout)
    );
}

fn tip(repo: &Path) -> String {
    stdout_of(repo, &["log", "--limit", "1"])
        .lines()
        .find_map(|line| line.strip_prefix("block "))
        .expect("log names a block")
        .trim()
        .to_string()
}

/// Whether the platform under test can record an executable bit at all. Unix can; Windows cannot, so a `mode`
/// entry exists there only if the fixture could have made one. Derived the way the fixture writes it
/// (`tree`'s control 1 went red on Windows by expecting the bit unconditionally).
const EXECUTABLE_BIT_IS_AVAILABLE: bool = cfg!(unix);

#[cfg(unix)]
fn make_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) {}

/// `(path, left content, right content)` for every text file the fixture changes. `None` means absent.
const TEXTS: &[(&str, Option<&str>, Option<&str>)] = &[
    ("crlf.txt", Some("a\r\nb\r\n"), Some("a\r\nB\r\n")),
    (
        "edit.txt",
        Some("one\ntwo\nthree\n"),
        Some("one\nTWO\nthree\nfour\n"),
    ),
    ("gone.txt", Some("bye\n"), None),
    ("new.txt", None, Some("brand new\n")),
    ("nonl.txt", Some("x\ny"), Some("x\nz")),
];
const BINARY_LEFT: &[u8] = b"\xff\x00AAAA";
const BINARY_RIGHT: &[u8] = b"\xff\x00BBBBBB";

struct Fixture {
    repo: PathBuf,
    base: String,
    head: String,
}

fn fixture(tag: &str) -> Fixture {
    let repo = support::unique_repo(tag);
    ok(&repo, &["setup", "."]);
    let write = |path: &str, bytes: &[u8]| std::fs::write(repo.join(path), bytes).unwrap();
    for (path, left, _) in TEXTS {
        if let Some(left) = left {
            write(path, left.as_bytes());
        }
    }
    write("old.txt", b"stay the same\n");
    write("keep.txt", b"keep\n");
    write("run.sh", b"#!/bin/sh\n");
    write("img.bin", BINARY_LEFT);
    ok(&repo, &["commit", "-m", "base"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let base = tip(&repo);

    for (path, _, right) in TEXTS {
        match right {
            Some(right) => write(path, right.as_bytes()),
            None => std::fs::remove_file(repo.join(path)).unwrap(),
        }
    }
    ok(&repo, &["mv", "old.txt", "moved.txt"]);
    make_executable(&repo.join("run.sh"));
    write("img.bin", BINARY_RIGHT);
    ok(&repo, &["commit", "-m", "change"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let head = tip(&repo);
    Fixture { repo, base, head }
}

fn report(repo: &Path, args: &[&str]) -> Value {
    let mut full = vec!["diff"];
    full.extend_from_slice(args);
    full.extend_from_slice(&["--format", "json"]);
    support::json::parse(&stdout_of(repo, &full))
}

fn entries(report: &Value) -> Vec<Value> {
    report.get("entries").as_array().to_vec()
}

fn status_of(report: &Value, path: &str) -> Option<String> {
    entries(report)
        .iter()
        .find(|entry| entry.get("path").as_str() == path)
        .map(|entry| entry.get("status").as_str().to_string())
}

fn has_key(value: &Value, key: &str) -> bool {
    matches!(value, Value::Object(map) if map.contains_key(key))
}

/// Control 1: one of each reachable status, in canonical path order, each once.
#[test]
fn control1_one_of_each_status_in_canonical_order() {
    let f = fixture("rfc153-diff-control1");
    let r = report(&f.repo, &["--from", &f.base, "--to", &f.head]);
    assert_eq!(r.get("schema_version").as_str(), "diff-report-v1");

    let mut expected: Vec<(&str, &str)> = vec![
        ("crlf.txt", "modified"),
        ("edit.txt", "modified"),
        ("gone.txt", "deleted"),
        ("img.bin", "binary"),
        ("moved.txt", "renamed"),
        ("new.txt", "added"),
        ("nonl.txt", "modified"),
    ];
    if EXECUTABLE_BIT_IS_AVAILABLE {
        expected.push(("run.sh", "mode"));
    }
    let got: Vec<(String, String)> = entries(&r)
        .iter()
        .map(|entry| {
            (
                entry.get("path").as_str().to_string(),
                entry.get("status").as_str().to_string(),
            )
        })
        .collect();
    let want: Vec<(String, String)> = expected
        .iter()
        .map(|(p, s)| (p.to_string(), s.to_string()))
        .collect();
    assert_eq!(
        got, want,
        "every reachable status once, in canonical path order"
    );
    assert_eq!(
        status_of(&r, "keep.txt"),
        None,
        "an unchanged file is not an entry"
    );

    // A rename carries the path it came from; nothing else does.
    let moved = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "moved.txt")
        .unwrap();
    assert_eq!(moved.get("from_path").as_str(), "old.txt");
    assert!(
        entries(&r)
            .iter()
            .filter(|e| has_key(e, "from_path"))
            .count()
            == 1
    );

    // Added has no `from` side, deleted has no `to` side, and the others have both.
    let side = |path: &str, key: &str| {
        entries(&r)
            .into_iter()
            .find(|e| e.get("path").as_str() == path)
            .map(|e| has_key(&e, key))
            .unwrap()
    };
    assert!(!side("new.txt", "from") && side("new.txt", "to"));
    assert!(side("gone.txt", "from") && !side("gone.txt", "to"));
    assert!(side("edit.txt", "from") && side("edit.txt", "to"));

    // The two points echo how they were named and where they resolved.
    assert_eq!(r.get("from").get("point").as_str(), f.base);
    assert_eq!(r.get("from").get("target_block_id").as_str(), f.base);
    assert_eq!(r.get("to").get("target_block_id").as_str(), f.head);
    assert!(r.get("unsupported_paths").as_array().is_empty());
    assert!(
        !has_key(&r, "untracked"),
        "RFC 153 §7.2: there is no untracked bucket"
    );
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Control 2: **the renderer cannot lie.** For every text entry, the rendered hunks, applied to the left
/// content the fixture wrote, reproduce the right content byte for byte -- by an independent applier that runs
/// everywhere (CRLF lines, a missing final newline, a whole new file and a whole deleted one all included).
#[test]
fn control2_the_hunks_reproduce_the_right_side_byte_for_byte() {
    let f = fixture("rfc153-diff-control2");
    let r = report(&f.repo, &["--from", &f.base, "--to", &f.head]);
    let mut checked = 0;
    for entry in entries(&r) {
        let path = entry.get("path").as_str().to_string();
        let hunks: Vec<String> = entry
            .get("hunks")
            .as_array()
            .iter()
            .map(|h| h.as_str().to_string())
            .collect();
        let Some((_, left, right)) = TEXTS.iter().find(|(p, _, _)| *p == path) else {
            assert!(hunks.is_empty(), "{path}: only a text change has hunks");
            continue;
        };
        let applied = support::apply_unified_hunks(left.unwrap_or(""), &hunks)
            .unwrap_or_else(|why| panic!("{path}: {why}\n{hunks:?}"));
        assert_eq!(
            applied,
            right.unwrap_or(""),
            "{path}: applying the hunks must give the right side"
        );
        checked += 1;
    }
    assert_eq!(
        checked,
        TEXTS.len(),
        "every text file the fixture changes was checked"
    );
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Whether the `patch` on `PATH` is **GNU patch**: identified positively, since BSD and BusyBox `patch`
/// differ in the flags this control uses. Unix only, as the handoff says.
#[cfg(unix)]
fn gnu_patch() -> Result<(), String> {
    let output = std::process::Command::new("patch")
        .arg("--version")
        .output()
        .map_err(|err| format!("`patch` could not be run: {err}"))?;
    let answer = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && answer.contains("GNU patch") {
        return Ok(());
    }
    Err(format!(
        "the `patch` on PATH is not GNU patch (`patch --version` exited {:?}, first line {:?})",
        output.status.code(),
        answer.lines().next().unwrap_or("")
    ))
}

/// Control 2, second half: **`patch(1)` itself** applies the printed hunks. The prose form is the one a
/// person would pipe to `patch`, so each entry's `---`/`+++` header and hunks are taken from the prose and
/// applied to a copy of the left content with the real tool.
///
/// **Coverage, stated as what is known:** runs where GNU `patch` is on `PATH` (Linux, including CI) and
/// **skips with a printed reason otherwise**; on macOS (BSD `patch`) and Windows the independent applier in
/// control 2 is what covers the renderer -- that macOS skips is reasoned, not observed.
#[cfg(unix)]
#[test]
fn control2b_gnu_patch_applies_the_printed_hunks() {
    if let Err(reason) = gnu_patch() {
        println!("skipping the patch(1) control: {reason}");
        return;
    }
    let f = fixture("rfc153-diff-control2b");
    let prose = stdout_of(&f.repo, &["diff", "--from", &f.base, "--to", &f.head]);
    let work = support::unique_repo("rfc153-diff-control2b-work");
    let mut applied = 0;
    for (path, left, right) in TEXTS {
        // The entry's own block of prose: from its `--- ` line to the next entry header (or the end).
        let header = format!(
            "--- {}",
            if left.is_some() {
                format!("a/{path}")
            } else {
                "/dev/null".to_string()
            }
        );
        let start = prose
            .lines()
            .position(|line| line == header)
            .and_then(|_| {
                let mut offset = 0;
                for line in prose.split_inclusive('\n') {
                    if line.trim_end_matches('\n') == header
                        && prose[offset + line.len()..].starts_with(&format!(
                            "+++ {}",
                            if right.is_some() {
                                format!("b/{path}")
                            } else {
                                "/dev/null".to_string()
                            }
                        ))
                    {
                        return Some(offset);
                    }
                    offset += line.len();
                }
                None
            })
            .unwrap_or_else(|| panic!("no `{header}` block for {path} in:\n{prose}"));
        let mut patch = String::new();
        for (index, line) in prose[start..].split_inclusive('\n').enumerate() {
            let body = line.trim_end_matches('\n');
            let is_hunk_body = index < 2 || line.starts_with(['@', ' ', '-', '+', '\\']);
            let is_next_entry = index >= 2
                && [
                    "modified ",
                    "deleted ",
                    "added ",
                    "renamed ",
                    "binary ",
                    "mode ",
                ]
                .iter()
                .any(|prefix| body.starts_with(prefix));
            if !is_hunk_body || is_next_entry {
                break;
            }
            patch.push_str(line);
        }
        let target = work.join(path);
        std::fs::write(&target, left.unwrap_or("")).unwrap();
        let patch_file = work.join("change.patch");
        std::fs::write(&patch_file, &patch).unwrap();
        let output = std::process::Command::new("patch")
            .current_dir(&work)
            .args(["--no-backup-if-mismatch", "--force", "--silent", "--posix"])
            .arg(&target)
            .arg(&patch_file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{path}: patch refused:\n{}\n{patch}",
            text(&output)
        );
        let got = std::fs::read(&target).unwrap_or_default();
        assert_eq!(
            got,
            right.unwrap_or("").as_bytes(),
            "{path}: `patch` must produce the right side byte for byte\n{patch}"
        );
        applied += 1;
    }
    assert_eq!(applied, TEXTS.len());
    let _ = std::fs::remove_dir_all(&f.repo);
    let _ = std::fs::remove_dir_all(&work);
}

/// Control 3: two points that are the same state give an empty report, exit 0, and the prose says so.
#[test]
fn control3_the_same_point_twice_is_an_empty_report() {
    let f = fixture("rfc153-diff-control3");
    let r = report(&f.repo, &["--from", &f.head, "--to", &f.head]);
    assert!(entries(&r).is_empty());
    assert_eq!(
        r.get("from").get("target_block_id"),
        r.get("to").get("target_block_id")
    );
    let prose = stdout_of(
        &f.repo,
        &["diff", "--from", "heads/main", "--to", "heads/main"],
    );
    assert!(prose.contains("\nno differences\n"), "{prose}");
    assert!(!prose.contains("entries:"), "{prose}");
    // A ref and the block it points at are the same state too.
    let mixed = report(&f.repo, &["--from", "heads/main", "--to", &f.head]);
    assert!(entries(&mixed).is_empty());
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Control 4: a block id and the ref that points at it give byte-identical output, but for the name.
#[test]
fn control4_a_block_id_and_its_ref_give_identical_output() {
    let f = fixture("rfc153-diff-control4");
    let by_ref = stdout_of(
        &f.repo,
        &[
            "diff",
            "--from",
            &f.base,
            "--to",
            "heads/main",
            "--format",
            "json",
        ],
    );
    let by_id = stdout_of(
        &f.repo,
        &[
            "diff", "--from", &f.base, "--to", &f.head, "--format", "json",
        ],
    );
    let named = "\"to\": {\"point\": \"heads/main\",";
    let bare = format!("\"to\": {{\"point\": \"{}\",", f.head);
    assert!(by_ref.contains(named) && by_id.contains(&bare));
    assert_eq!(by_ref.replace(named, ""), by_id.replace(&bare, ""));

    let prose_ref = stdout_of(&f.repo, &["diff", "--from", &f.base, "--to", "heads/main"]);
    let prose_id = stdout_of(&f.repo, &["diff", "--from", &f.base, "--to", &f.head]);
    let (prose_ref, _) = support::split_line_path(&prose_ref, "diff repository: ");
    let (prose_id, _) = support::split_line_path(&prose_id, "diff repository: ");
    let line_ref = format!("to: heads/main (block {})\n", f.head);
    let line_id = format!("to: block {}\n", f.head);
    assert!(prose_ref.contains(&line_ref) && prose_id.contains(&line_id));
    assert_eq!(
        prose_ref.replace(&line_ref, ""),
        prose_id.replace(&line_id, "")
    );
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Control 5: a binary file shows ids and sizes only -- in prose and in JSON -- and names `prikk cat`.
#[test]
fn control5_a_binary_file_shows_ids_and_sizes_and_never_bytes() {
    let f = fixture("rfc153-diff-control5");
    let output = ok(&f.repo, &["diff", "--from", &f.base, "--to", &f.head]);
    for needle in [&b"AAAA"[..], &b"BBBBBB"[..], &b"\xff\x00"[..]] {
        assert!(
            !output
                .stdout
                .windows(needle.len())
                .any(|window| window == needle),
            "binary content reached stdout: {:?}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    let prose = String::from_utf8_lossy(&output.stdout);
    assert!(
        prose.contains("binary img.bin (binary: 6 bytes, id "),
        "{prose}"
    );
    assert!(prose.contains("-> 8 bytes, id "), "{prose}");
    assert!(
        prose.contains(&format!("prikk cat --path img.bin --ref {}", f.base)),
        "{prose}"
    );
    assert!(
        prose.contains(&format!("prikk cat --path img.bin --ref {}", f.head)),
        "{prose}"
    );

    let r = report(&f.repo, &["--from", &f.base, "--to", &f.head]);
    let entry = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "img.bin")
        .unwrap();
    assert_eq!(entry.get("status").as_str(), "binary");
    assert!(entry.get("hunks").as_array().is_empty());
    for (key, size) in [("from", "6"), ("to", "8")] {
        let side = entry.get(key);
        assert_eq!(side.get("encoding").as_str(), "binary");
        assert_eq!(side.get("size"), &Value::Number(size.to_string()));
        assert!(
            has_key(side, "content_id"),
            "{key}: a binary side names its blob"
        );
    }
    // A text side never carries one -- the same rule `tree` follows, from the same emitter.
    let edit = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "edit.txt")
        .unwrap();
    assert!(!has_key(edit.get("from"), "content_id") && !has_key(edit.get("to"), "content_id"));
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Control 6: an unsupported operation in **either** chain fails the whole call, printing nothing -- and only
/// a call whose chains hold one fails. A `CreateSymlink` has no authoring path, so it is written as a raw patch
/// and sealed through the test-support seal, as RFC 143's control 3 does.
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

    let repo = support::unique_repo("rfc153-diff-control6");
    let layout = RepositoryLayout::init(repo.clone()).unwrap();
    add_trusted_maintainer(
        &layout,
        support::MAINTAINER_KEY_ID,
        &support::maintainer_public_key_hex(),
    )
    .unwrap();
    let author = Ed25519AuthorSigner::from_seed("rfc153-diff-control6", &[0x64; 32]).unwrap();
    let maintainer =
        Ed25519MaintainerSigner::from_seed(support::MAINTAINER_KEY_ID, &support::MAINTAINER_SEED)
            .unwrap();
    let blob = |content: &[u8]| {
        let bytes = BlobPayload::new(BlobKind::Text, content.to_vec())
            .to_canonical_bytes()
            .unwrap();
        ObjectWriteSession::open(&layout)
            .unwrap()
            .write_object(&ObjectEnvelope::unsigned(ObjectType::Blob, 1, bytes))
            .unwrap()
    };
    let seal_patch = |operations: Vec<Operation>| {
        write_active_ref_metadata(&layout, "heads/main").unwrap();
        let payload = PatchPayload {
            operations,
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
    };
    let create_file = |op_seq, path: &str, node: u8, content: &[u8]| Operation {
        op_seq,
        op_id: None,
        preconditions: Vec::new(),
        kind: OperationKind::CreateFile(CreateFile {
            path: path.to_string(),
            node_id: NodeId::from_bytes([node; 32]),
            blob_id: blob(content),
            mode: 0o100_644,
        }),
    };
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    seal_patch(vec![create_file(1, "a.txt", 0x81, b"a\n")]);
    let clean = tip(&repo);
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    seal_patch(vec![
        create_file(1, "b.txt", 0x82, b"b\n"),
        Operation {
            op_seq: 2,
            op_id: None,
            preconditions: Vec::new(),
            kind: OperationKind::CreateSymlink(CreateSymlink {
                path: "link".to_string(),
                node_id: NodeId::from_bytes([0x83; 32]),
                target: "b.txt".to_string(),
            }),
        },
    ]);
    let unsupported = tip(&repo);

    // The right chain holds it; the left chain holds it; either way the call fails, and prints nothing.
    for (from, to) in [
        (&clean, &unsupported),
        (&unsupported, &clean),
        (&unsupported, &unsupported),
    ] {
        for format in [&[][..], &["--format", "json"][..]] {
            let mut args = vec!["diff", "--from", from.as_str(), "--to", to.as_str()];
            args.extend_from_slice(format);
            let output = run(&repo, &args);
            assert_eq!(
                output.status.code(),
                Some(1),
                "{}: {}",
                args.join(" "),
                text(&output)
            );
            let stderr = String::from_utf8_lossy(&output.stderr).to_lowercase();
            assert!(
                stderr.contains("symlink") || stderr.contains("unsupported"),
                "the refusal names the operation: {stderr}"
            );
            assert!(
                output.stdout.is_empty(),
                "no partial answer: {:?}",
                output.stdout
            );
        }
    }
    // A history that merely *contains* the bad block elsewhere is fine: only these two chains matter.
    let fine = report(&repo, &["--from", &clean, "--to", &clean]);
    assert!(entries(&fine).is_empty());
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 7: `--path` filters the prose **and** the JSON `entries`, by exact path; a rename is selected by
/// either of its paths.
#[test]
fn control7_path_filters_the_output() {
    let f = fixture("rfc153-diff-control7");
    let all = entries(&report(&f.repo, &["--from", &f.base, "--to", &f.head])).len();
    assert!(all >= 7);

    let one = report(
        &f.repo,
        &["--from", &f.base, "--to", &f.head, "--path", "edit.txt"],
    );
    assert_eq!(entries(&one).len(), 1);
    assert_eq!(status_of(&one, "edit.txt").as_deref(), Some("modified"));
    let prose = stdout_of(
        &f.repo,
        &[
            "diff", "--from", &f.base, "--to", &f.head, "--path", "edit.txt",
        ],
    );
    assert!(
        prose.contains("modified edit.txt") && prose.contains("entries: 1\n"),
        "{prose}"
    );
    for other in [
        "crlf.txt",
        "gone.txt",
        "img.bin",
        "moved.txt",
        "new.txt",
        "nonl.txt",
    ] {
        assert!(
            !prose.contains(other),
            "--path edit.txt must not show {other}: {prose}"
        );
    }

    let two = report(
        &f.repo,
        &[
            "--from", &f.base, "--to", &f.head, "--path", "edit.txt", "--path", "gone.txt",
        ],
    );
    assert_eq!(entries(&two).len(), 2);
    // A rename is found by the new path or the old one.
    for wanted in ["moved.txt", "old.txt"] {
        let renamed = report(
            &f.repo,
            &["--from", &f.base, "--to", &f.head, "--path", wanted],
        );
        assert_eq!(
            status_of(&renamed, "moved.txt").as_deref(),
            Some("renamed"),
            "--path {wanted}"
        );
        assert_eq!(entries(&renamed).len(), 1);
    }
    // Exact, not a prefix, and an unchanged path selects nothing.
    let none = report(
        &f.repo,
        &["--from", &f.base, "--to", &f.head, "--path", "edit"],
    );
    assert!(entries(&none).is_empty());
    let unchanged = stdout_of(
        &f.repo,
        &[
            "diff", "--from", &f.base, "--to", &f.head, "--path", "keep.txt",
        ],
    );
    assert!(unchanged.contains("\nno differences\n"), "{unchanged}");
    refuses(
        &f.repo,
        &["diff", "--from", &f.base, "--to", &f.head, "--path", "/abs"],
        2,
        "is not a valid repository-relative path",
    );
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Control 8: a received ref diffs (reading is not adopting), in a repository that has no `heads/main`.
#[test]
fn control8_a_received_ref_diffs() {
    let f = fixture("rfc153-diff-control8");
    let bundle = f.repo.join("main.bundle");
    ok(
        &f.repo,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    let dst = support::unique_repo("rfc153-diff-control8-dst");
    ok(&dst, &["init", "."]);
    ok(
        &dst,
        &["bundle", "import", "--input", bundle.to_str().unwrap()],
    );
    // The received ref's tip on one side, and an older block only received history holds on the other.
    let received = report(&dst, &["--from", &f.base, "--to", "remotes/heads/main"]);
    let sent = report(&f.repo, &["--from", &f.base, "--to", "heads/main"]);
    assert_eq!(
        received.get("to").get("point").as_str(),
        "remotes/heads/main"
    );
    assert_eq!(received.get("entries"), sent.get("entries"));
    assert!(!entries(&received).is_empty());
    let _ = std::fs::remove_dir_all(&f.repo);
    let _ = std::fs::remove_dir_all(&dst);
}

/// Control 9: what refuses, with which answer and exit code.
#[test]
fn control9_absent_points_and_bad_usage_refuse() {
    let f = fixture("rfc153-diff-control9");
    let zeros = "0".repeat(64);
    refuses(
        &f.repo,
        &["diff", "--from", "heads/nope", "--to", &f.head],
        1,
        "error: precondition not met: ref heads/nope does not exist in this repository",
    );
    refuses(
        &f.repo,
        &["diff", "--from", &f.base, "--to", "heads/nope"],
        1,
        "error: precondition not met: ref heads/nope does not exist in this repository",
    );
    refuses(
        &f.repo,
        &["diff", "--from", &zeros, "--to", &f.head],
        1,
        &format!("error: precondition not met: block {zeros} is not in this repository"),
    );
    let patch = stdout_of(&f.repo, &["log", "--limit", "1"])
        .lines()
        .find_map(|line| line.trim().strip_prefix("patch "))
        .and_then(|rest| rest.split(':').next())
        .unwrap()
        .to_string();
    refuses(
        &f.repo,
        &["diff", "--from", &f.base, "--to", &patch],
        1,
        &format!("error: precondition not met: object {patch} is a patch, not a block"),
    );
    // Usage: exit 2.
    refuses(
        &f.repo,
        &["diff", "--from", &f.base],
        2,
        "diff needs both --from",
    );
    refuses(
        &f.repo,
        &["diff", "--to", &f.base],
        2,
        "diff needs both --from",
    );
    refuses(&f.repo, &["diff"], 2, "diff needs both --from");
    refuses(
        &f.repo,
        &["diff", "--from", "main", "--to", &f.head],
        2,
        "is neither a ref name",
    );
    refuses(
        &f.repo,
        &["diff", "--from", &f.base, "--to", &f.head, "--nope"],
        2,
        "unknown",
    );
    refuses(
        &f.repo,
        &[
            "diff", "--from", &f.base, "--to", &f.head, "--format", "json", "--format", "json",
        ],
        2,
        "--format",
    );
    let _ = std::fs::remove_dir_all(&f.repo);
}

/// Control 10: a rename is **declared, never inferred**. Deleting a file and creating another with the same
/// bytes is a `deleted` and an `added`; only `prikk mv` makes a `renamed`, and a rename that also changes the
/// content keeps its status and carries its hunks.
#[test]
fn control10_a_rename_is_declared_and_never_inferred() {
    let repo = support::unique_repo("rfc153-diff-control10");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("a.txt"), "the same bytes\nsecond line\n").unwrap();
    std::fs::write(repo.join("m.txt"), "moves\nand changes\n").unwrap();
    ok(&repo, &["commit", "-m", "base"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let base = tip(&repo);

    // Undeclared: the same bytes at a new path, the old path removed.
    std::fs::remove_file(repo.join("a.txt")).unwrap();
    std::fs::write(repo.join("b.txt"), "the same bytes\nsecond line\n").unwrap();
    // Declared: a move, with an edit on top.
    ok(&repo, &["mv", "m.txt", "n.txt"]);
    std::fs::write(repo.join("n.txt"), "moves\nand changed\n").unwrap();
    ok(&repo, &["commit", "-m", "change"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let head = tip(&repo);

    let r = report(&repo, &["--from", &base, "--to", &head]);
    assert_eq!(
        status_of(&r, "a.txt").as_deref(),
        Some("deleted"),
        "identical bytes are not a rename"
    );
    assert_eq!(
        status_of(&r, "b.txt").as_deref(),
        Some("added"),
        "identical bytes are not a rename"
    );
    assert_eq!(
        status_of(&r, "n.txt").as_deref(),
        Some("renamed"),
        "a declared move is"
    );
    assert_eq!(
        status_of(&r, "m.txt"),
        None,
        "the old path of a rename is not an entry of its own"
    );
    let moved = entries(&r)
        .into_iter()
        .find(|e| e.get("path").as_str() == "n.txt")
        .unwrap();
    assert_eq!(moved.get("from_path").as_str(), "m.txt");
    let hunks: Vec<String> = moved
        .get("hunks")
        .as_array()
        .iter()
        .map(|h| h.as_str().to_string())
        .collect();
    assert_eq!(
        support::apply_unified_hunks("moves\nand changes\n", &hunks).unwrap(),
        "moves\nand changed\n",
        "a renamed file's content change is rendered"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 11 (RFC 157 §5a, which `diff` shares by the same mechanism): a blob that is missing or does not
/// recompute fails the **whole** call, and prints nothing -- there is no entry-level `unavailable`.
#[test]
fn control11_blob_damage_fails_the_whole_call() {
    let repo = support::unique_repo("rfc153-diff-control11");
    ok(&repo, &["setup", "."]);
    std::fs::write(repo.join("x.bin"), b"\xff\x00AAAA").unwrap();
    std::fs::write(repo.join("y.bin"), b"\xff\x00BBBB").unwrap();
    std::fs::write(repo.join("t.txt"), b"text\n").unwrap();
    ok(&repo, &["commit", "-m", "one"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let base = tip(&repo);
    std::fs::write(repo.join("t.txt"), b"text\nmore\n").unwrap();
    ok(&repo, &["commit", "-m", "two"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    let head = tip(&repo);
    ok(&repo, &["diff", "--from", &base, "--to", &head]);

    support::swap_two_equal_length_blob_frames(&repo);

    for args in [
        vec!["diff", "--from", &base, "--to", &head],
        vec!["diff", "--from", &base, "--to", &head, "--format", "json"],
        vec!["diff", "--from", &base, "--to", &head, "--path", "t.txt"],
    ] {
        let output = run(&repo, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{}: {}",
            args.join(" "),
            text(&output)
        );
        assert!(
            output.stdout.is_empty(),
            "{}: a partial answer: {:?}",
            args.join(" "),
            output.stdout
        );
        assert!(
            text(&output).contains("integrity error"),
            "{}: {}",
            args.join(" "),
            text(&output)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}
