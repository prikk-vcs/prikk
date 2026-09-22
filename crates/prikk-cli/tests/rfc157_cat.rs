//! RFC 157 §4 and §7.2: `prikk cat`, through the compiled binary.
//!
//! The terminal rule is controlled twice: the decision itself is a unit test beside the code
//! (`src/cat/tests.rs`), and here through a **real** pseudo-terminal, driven by util-linux `script` where
//! that tool exists. The killed-process `--output` case is a unit test too
//! (`src/durable_output/tests.rs`), since the store's failpoints cannot reach a write outside a repository
//! — see the report.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use std::path::{Path, PathBuf};
#[cfg(unix)]
use std::process::Command;
use std::process::Output;

use support::json::Value;

const TEXT: &[u8] = b"fn main() {\n    println!(\"hi\");\n}\n";
const BINARY: &[u8] = b"\xff\xfe\x00binary\x01\x02";

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn text_of(output: &Output) -> String {
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

fn stdout_string(repo: &Path, args: &[&str]) -> String {
    String::from_utf8_lossy(&ok(repo, args).stdout).into_owned()
}

fn refuses(repo: &Path, args: &[&str], expected: &str) -> Output {
    let output = run(repo, args);
    assert_eq!(output.status.code(), Some(1), "{}", text_of(&output));
    assert!(
        text_of(&output).contains(expected),
        "{}: expected {expected:?} in {}",
        args.join(" "),
        text_of(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "a refusal writes no bytes to stdout: {:?}",
        output.stdout
    );
    output
}

/// One sealed block: a text file, a binary file, and a nested path.
fn fixture(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    ok(&repo, &["setup", "."]);
    std::fs::create_dir_all(repo.join("src")).unwrap();
    std::fs::write(repo.join("src/main.rs"), TEXT).unwrap();
    std::fs::write(repo.join("src/big.bin"), BINARY).unwrap();
    ok(&repo, &["commit", "-m", "fixture"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    repo
}

/// Control 1: the bytes are the file's own, byte for byte, for text and binary alike — compared against the
/// materialized worktree, not against the fixture's own copy.
#[test]
fn control1_bytes_equal_the_materialized_file() {
    let repo = fixture("rfc157-cat-control1");
    let materialized = support::unique_repo("rfc157-cat-control1-materialized");
    support::copy_dir_recursive(&repo.join(".prikk"), &materialized.join(".prikk"));
    ok(
        &materialized,
        &["checkout", "--patch-materialize", "--ref", "heads/main"],
    );

    for path in ["src/main.rs", "src/big.bin"] {
        let from_cat = ok(&repo, &["cat", "--path", path]).stdout;
        assert_eq!(
            from_cat,
            std::fs::read(materialized.join(path)).unwrap(),
            "{path}: cat's bytes equal the materialized file"
        );
    }
    // And through --output, which takes the same bytes to a file.
    let out = materialized.join("out.bin");
    ok(
        &repo,
        &[
            "cat",
            "--path",
            "src/big.bin",
            "--output",
            out.to_str().unwrap(),
        ],
    );
    assert_eq!(std::fs::read(&out).unwrap(), BINARY);

    let _ = std::fs::remove_dir_all(&materialized);
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 2: `--max-bytes` one below the size refuses with **no file and no stdout bytes**; at exactly the
/// size it succeeds.
#[test]
fn control2_max_bytes_is_all_or_nothing() {
    let repo = fixture("rfc157-cat-control2");
    let size = TEXT.len() as u64;
    let destination = repo.join("out.txt");

    let below = (size - 1).to_string();
    let output = run(
        &repo,
        &[
            "cat",
            "--path",
            "src/main.rs",
            "--max-bytes",
            &below,
            "--output",
            destination.to_str().unwrap(),
        ],
    );
    // **What was written is checked first**, so a bound applied after the write fails here rather than on
    // the wording of the refusal that follows it.
    assert!(
        !destination.exists(),
        "the bound must refuse before anything is written: {}",
        text_of(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "no bytes to stdout either: {:?}",
        output.stdout
    );
    assert_eq!(output.status.code(), Some(1), "{}", text_of(&output));
    assert!(
        text_of(&output).contains(&format!(
            "src/main.rs is {size} bytes at heads/main, above the --max-bytes bound of {}",
            size - 1
        )),
        "{}",
        text_of(&output)
    );
    assert!(text_of(&output).contains("nothing was written"));

    // To stdout, the same bound writes nothing at all.
    refuses(
        &repo,
        &["cat", "--path", "src/main.rs", "--max-bytes", &below],
        "above the --max-bytes bound",
    );

    // Exactly the size succeeds, and is the whole content.
    let at_bound = ok(
        &repo,
        &[
            "cat",
            "--path",
            "src/main.rs",
            "--max-bytes",
            &size.to_string(),
        ],
    );
    assert_eq!(at_bound.stdout, TEXT);
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 3: `--output` over an existing file refuses without `--force`, leaves the original untouched,
/// and replaces it with `--force`. It also refuses a path inside `.prikk/`.
#[test]
fn control3_output_collision_and_the_prikk_directory() {
    let repo = fixture("rfc157-cat-control3");
    let destination = repo.join("out.bin");
    std::fs::write(&destination, b"original").unwrap();

    refuses(
        &repo,
        &[
            "cat",
            "--path",
            "src/big.bin",
            "--output",
            destination.to_str().unwrap(),
        ],
        "refusing to overwrite existing file",
    );
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");

    ok(
        &repo,
        &[
            "cat",
            "--path",
            "src/big.bin",
            "--output",
            destination.to_str().unwrap(),
            "--force",
        ],
    );
    assert_eq!(std::fs::read(&destination).unwrap(), BINARY);

    let inside = repo.join(".prikk/sneak.bin");
    refuses(
        &repo,
        &[
            "cat",
            "--path",
            "src/big.bin",
            "--output",
            inside.to_str().unwrap(),
        ],
        "refusing to write inside the repository's own `.prikk` directory",
    );
    assert!(!inside.exists(), "nothing may be written inside .prikk");
    let _ = std::fs::remove_dir_all(&repo);
}

/// `Ok` when the `script` on `PATH` is **util-linux's** -- the only `script` this control drives, because
/// its `-qec <command> /dev/null` synopsis is util-linux's own -- and otherwise the reason it cannot be
/// used, worded from what was actually found.
///
/// **Identified positively, not assumed.** BSD `script` (macOS) takes `[-aeFkqr] [-t time] [file [command
/// ...]]` and has no `-c`, and BusyBox's differs again, so "a `script` exists" says nothing about whether
/// `-qec` means what this control needs. Unix only: the control itself is, and a Windows build would
/// otherwise carry this as dead code.
#[cfg(unix)]
fn util_linux_script() -> Result<(), String> {
    let output = Command::new("script")
        .arg("--version")
        .output()
        .map_err(|err| format!("`script` could not be run: {err}"))?;
    let answer = String::from_utf8_lossy(&output.stdout);
    if output.status.success() && answer.contains("util-linux") {
        return Ok(());
    }
    Err(format!(
        "the `script` on PATH is not util-linux's (`script --version` exited {:?}, stdout {:?}, stderr {:?})",
        output.status.code(),
        answer.lines().next().unwrap_or(""),
        String::from_utf8_lossy(&output.stderr)
            .lines()
            .next()
            .unwrap_or("")
    ))
}

/// Run `prikk <args>` with **stdout on a real pseudo-terminal**, through `script -qec`.
#[cfg(unix)]
fn on_a_terminal(repo: &Path, args: &[&str]) -> Output {
    let command_line = format!(
        "{} {}",
        env!("CARGO_BIN_EXE_prikk"),
        args.iter()
            .map(|arg| format!("'{arg}'"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let mut command = Command::new("script");
    command
        .current_dir(repo)
        .args(["-qec", &command_line, "/dev/null"]);
    support::isolate_key_environment_for(&mut command, Some(repo));
    command.output().unwrap()
}

/// Control 4: binary content refuses a **real** terminal, naming `--output`, and writes nothing; text goes
/// to the same terminal as-is.
///
/// **Coverage, stated as what is known:** the control drives util-linux `script -qec`, so it runs wherever
/// util-linux's `script` is on `PATH` -- Linux, including CI's -- and **skips with a printed reason
/// everywhere else**. On macOS the `script` is BSD's, whose documented synopsis has no `-c`; the control
/// therefore skips there, and that has been **reasoned from the documented synopsis, not observed**. Windows
/// has no `script` and no pseudo-terminal of this shape, and the control is compiled out. **On macOS and
/// Windows the terminal rule is covered by the unit test in `src/cat/tests.rs`**, which tests the one
/// function `IsTerminal` feeds -- that is the whole of what those two platforms cover.
#[cfg(unix)]
#[test]
fn control4_binary_refuses_a_real_terminal_and_text_is_written() {
    let repo = fixture("rfc157-cat-control4");
    if let Err(reason) = util_linux_script() {
        println!("skipping the pseudo-terminal control: {reason}");
        let _ = std::fs::remove_dir_all(&repo);
        return;
    }
    // The fixture itself must be sound on this host before the terminal question is asked.
    let piped = ok(&repo, &["cat", "--path", "src/big.bin"]);
    assert_eq!(piped.stdout, BINARY, "binary to a pipe is written");

    // util-linux is positively identified above, so a refusal here is a real failure, not a skip.
    let binary = on_a_terminal(&repo, &["cat", "--path", "src/big.bin"]);
    let seen = text_of(&binary);
    assert_eq!(binary.status.code(), Some(1), "{seen}");
    assert!(
        seen.contains("is binary, and binary content is not written to a terminal"),
        "{seen}"
    );
    assert!(seen.contains("prikk cat --output <file>"), "{seen}");
    assert!(
        !binary
            .stdout
            .windows(4)
            .any(|window| window == b"\xff\xfe\x00b"),
        "no binary content may reach the terminal: {:?}",
        binary.stdout
    );

    let text = on_a_terminal(&repo, &["cat", "--path", "src/main.rs"]);
    assert_eq!(text.status.code(), Some(0), "{}", text_of(&text));
    assert!(
        String::from_utf8_lossy(&text.stdout).contains("fn main()"),
        "text goes to a terminal as-is: {}",
        text_of(&text)
    );
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 5: `--format json` writes no bytes, and its fields are the ones `tree` lists for that path.
#[test]
fn control5_json_writes_no_bytes_and_agrees_with_tree() {
    let repo = fixture("rfc157-cat-control5");
    for path in ["src/main.rs", "src/big.bin"] {
        let raw = stdout_string(&repo, &["cat", "--path", path, "--format", "json"]);
        assert!(
            !raw.contains("binary\x01") && !raw.contains("fn main() {"),
            "the JSON form carries no content: {raw}"
        );
        let content = support::json::parse(&raw);
        assert_eq!(content.get("schema_version").as_str(), "path-content-v1");

        let listing = support::json::parse(&stdout_string(&repo, &["tree", "--format", "json"]));
        let entry = listing
            .get("entries")
            .as_array()
            .iter()
            .find(|entry| entry.get("path").as_str() == path)
            .expect("tree lists the path")
            .clone();
        assert_eq!(content.get("point"), listing.get("point"));
        assert_eq!(
            content.get("target_block_id"),
            listing.get("target_block_id")
        );
        for field in ["path", "kind", "encoding", "mode", "size"] {
            assert_eq!(content.get(field), entry.get(field), "{path}: {field}");
        }
        assert_eq!(
            matches!(&content, Value::Object(map) if map.contains_key("content_id")),
            matches!(&entry, Value::Object(map) if map.contains_key("content_id")),
            "{path}: content_id agrees with tree"
        );
        if let Value::Object(map) = &entry {
            if map.contains_key("content_id") {
                assert_eq!(content.get("content_id"), entry.get("content_id"));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&repo);
}

/// Control 6: an absent path, a directory prefix, and a point's own refusals.
#[test]
fn control6_absent_path_and_directory_prefix_refuse() {
    let repo = fixture("rfc157-cat-control6");
    refuses(
        &repo,
        &["cat", "--path", "nope.txt"],
        "precondition not met: path nope.txt does not exist at heads/main",
    );
    refuses(
        &repo,
        &["cat", "--path", "src"],
        "precondition not met: path src does not exist at heads/main",
    );
    refuses(
        &repo,
        &["cat", "--path", "src/main.rs", "--ref", "heads/nope"],
        "precondition not met: ref heads/nope does not exist in this repository",
    );
    let usage = run(&repo, &["cat", "--path", "src/main.rs", "--ref", "main"]);
    assert_eq!(usage.status.code(), Some(2), "{}", text_of(&usage));

    // A fresh repository's unpublished current branch holds no content, and says so as an absent path.
    // **The whole wording is pinned, parenthetical included** (Addendum 3, reworded by RFC 147 §2i
    // Addendum 1 once the branch stopped being "absent"): it names the question asked (the path) *and*
    // the reason (nothing published yet, not ref absence), and a fresh repository is exactly where a new
    // user meets it. Named explicitly with `--ref` or not: the same answer either way (RFC 147 §2i).
    let fresh = support::unique_repo("rfc157-cat-control6-fresh");
    ok(&fresh, &["init", "."]);
    let expected = "error: precondition not met: path src/main.rs does not exist at heads/main (heads/main \
                    has no published history yet)";
    refuses(&fresh, &["cat", "--path", "src/main.rs"], expected);
    refuses(
        &fresh,
        &["cat", "--path", "src/main.rs", "--ref", "heads/main"],
        expected,
    );
    let _ = std::fs::remove_dir_all(&fresh);
    let _ = std::fs::remove_dir_all(&repo);
}

/// Addendum 1 (RFC 157 §5a): post-seal blob damage fails the **whole call** — `tree`, `tree --format json`
/// and `cat` each exit 1 and print nothing on stdout. There is no entry-level `unavailable`.
#[test]
fn control7_blob_damage_fails_the_whole_call() {
    let repo = support::unique_repo("rfc157-cat-control7");
    ok(&repo, &["setup", "."]);
    // Two binary blobs of identical length, so their frames can be swapped, plus a text file whose own
    // blob is a different length and is therefore not the pair that gets swapped.
    std::fs::write(repo.join("x.bin"), b"\xff\x00AAAA").unwrap();
    std::fs::write(repo.join("y.bin"), b"\xff\x00BBBB").unwrap();
    std::fs::write(repo.join("t.txt"), b"text\n").unwrap();
    ok(&repo, &["commit", "-m", "two binaries"]);
    ok(&repo, &["seal", "--allow-no-audit"]);
    // Sound before the damage: every command answers.
    ok(&repo, &["tree", "--format", "json"]);
    ok(&repo, &["cat", "--path", "x.bin"]);

    support::swap_two_equal_length_blob_frames(&repo);

    for args in [
        vec!["tree"],
        vec!["tree", "--format", "json"],
        vec!["cat", "--path", "x.bin"],
        vec!["cat", "--path", "t.txt"],
        vec!["cat", "--path", "x.bin", "--format", "json"],
    ] {
        let output = run(&repo, &args);
        assert_eq!(
            output.status.code(),
            Some(1),
            "{} must fail the whole call: {}",
            args.join(" "),
            text_of(&output)
        );
        assert!(
            output.stdout.is_empty(),
            "{} printed a partial answer: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stdout)
        );
        assert!(
            text_of(&output).contains("integrity error"),
            "{} names the damage: {}",
            args.join(" "),
            text_of(&output)
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
}
