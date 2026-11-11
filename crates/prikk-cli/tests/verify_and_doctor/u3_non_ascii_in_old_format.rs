//! 0.57.0 U3 (U2 review, item 2): a format-6 or format-7 repository that **already holds** a non-ASCII path (the 0.55.0 hole
//! let one in from an archive) reads clean now that decode admits non-ASCII. But format 7's own rule is that it holds none, and
//! an older prikk opening it meets history it cannot read. So `verify` and `doctor` give **one warning, not a failure**, naming
//! the path and `prikk format upgrade`, with the reason: after the upgrade the format makes an older prikk refuse the
//! repository cleanly.
//!
//! The fixture is the hole's end state, built honestly: a format-8 repository commits and seals `日本語.md`, then its marker
//! is set back to 7 (nothing stored differs between 7 and 8).
//!
//! | repository | holds a non-ASCII path | `verify` | `doctor` |
//! |---|---|---|---|
//! | format 7 | yes | exit 0, **one warning** | exit 0, `PRIKK-DOCTOR-NON-ASCII-PATH-IN-OLD-FORMAT` (warning) |
//! | format 8 | yes | exit 0, no warning | no such warning |
//! | format 7 | no | exit 0, no warning | no such warning |
//! | format 7 after `format upgrade` | yes | exit 0, no warning | no such warning |

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn set_marker(repo: &Path, number: u8) {
    std::fs::write(repo.join(".prikk").join("FORMAT"), [b'0' + number, b'\n']).unwrap();
}

/// A sealed repository whose one generation adds `name`.
fn sealed_with(tag: &str, name: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    std::fs::write(repo.join(name), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "g"), "commit");
    support::ok(&support::seal(&repo, "heads/main"), "seal");
    repo
}

fn run(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo).args(args).output().unwrap()
}

const NON_ASCII: &str = "pages-\u{65e5}\u{672c}.md";
const WARNING_START: &str = "warning: this format-7 repository holds the non-ASCII path ";

#[test]
fn a_format_7_repository_holding_a_non_ascii_path_gets_one_verify_warning_and_exit_0() {
    let repo = sealed_with("u3-old-format-warning", NON_ASCII);
    set_marker(&repo, 7);
    let output = run(&repo, &["verify"]);
    let said = text(&output);
    assert_eq!(
        output.status.code(),
        Some(0),
        "a warning, never a failure: {said}"
    );
    assert_eq!(
        said.matches(WARNING_START).count(),
        1,
        "one warning: {said}"
    );
    assert!(
        said.contains("pages-\\u{65e5}\\u{672c}.md")
            && said.contains("which format 7 does not admit"),
        "the path (escaped) and the format: {said}"
    );
    assert!(said.contains("prikk format upgrade"), "{said}");
    assert!(
        said.contains("an older prikk refuses the repository cleanly"),
        "the reason: {said}"
    );

    let json = run(&repo, &["verify", "--format", "json"]);
    assert_eq!(json.status.code(), Some(0), "{}", text(&json));
    let json = String::from_utf8_lossy(&json.stdout).to_string();
    assert!(
        json.contains("\"non_ascii_paths_in_old_format\": [\n    {\"patch_id\": \""),
        "{json}"
    );
    assert!(json.contains("\"repository_format\": 7}"), "{json}");

    let doctor = run(&repo, &["doctor"]);
    let said = text(&doctor);
    assert_eq!(
        doctor.status.code(),
        Some(0),
        "doctor does not fail over it: {said}"
    );
    assert!(
        said.contains("warning [PRIKK-DOCTOR-NON-ASCII-PATH-IN-OLD-FORMAT]"),
        "{said}"
    );
    assert!(said.contains("prikk format upgrade"), "{said}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn nothing_is_said_where_the_format_admits_the_path_or_there_is_no_such_path() {
    // Format 8 holding the path: nothing to warn about.
    let repo = sealed_with("u3-old-format-8", NON_ASCII);
    for output in [run(&repo, &["verify"]), run(&repo, &["doctor"])] {
        let said = text(&output);
        assert_eq!(output.status.code(), Some(0), "{said}");
        assert!(
            !said.contains("non-ASCII path") && !said.contains("NON-ASCII-PATH-IN-OLD-FORMAT"),
            "{said}"
        );
    }
    // Format 7 holding only ASCII.
    let ascii = sealed_with("u3-old-format-ascii", "plain.md");
    set_marker(&ascii, 7);
    assert!(!text(&run(&ascii, &["verify"])).contains("non-ASCII path"));
    // Format 7 holding the path, after the upgrade the warning asks for.
    set_marker(&repo, 7);
    assert!(text(&run(&repo, &["verify"])).contains(WARNING_START));
    support::ok(&run(&repo, &["format", "upgrade"]), "format upgrade");
    let after = run(&repo, &["verify"]);
    assert_eq!(after.status.code(), Some(0));
    assert!(!text(&after).contains("non-ASCII path"), "{}", text(&after));
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&ascii);
}
