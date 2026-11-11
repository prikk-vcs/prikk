//! 0.57.0 U2 (Unicode repository paths), through the compiled binary: repository format 8 admits non-ASCII paths, and
//! `commit` is where a name is judged.
//!
//! Case table (a new repository is format 8; "7" is the same repository with its `FORMAT` marker set to 7):
//!
//! | the worktree name | format | `commit` | `worktree-status` entry |
//! |---|---|---|---|
//! | NFC, non-ASCII (`日本語.md`, `café.md`, `한글.txt`) | 8 | **authors it**; seal, `tree`, `verify` read it back | `untracked`, `authoring: authored` |
//! | NFD (`cafe`+U+0301) | 8 | **refused**, naming it and its NFC spelling (both written `\u{..}`, they look identical) | `untracked`, `authoring: refused` |
//! | NFC and NFD of one name both present | 8 | **refused**, naming both | (each entry judged alone) |
//! | contains ZWJ / a bidi control / a noncharacter | 8 and 7 | **refused**, naming the character | `unsupported-path`, `authoring: refused` |
//! | `COM¹.txt` (a Windows device name) | 8 | **refused** as reserved | refused |
//! | NFC, non-ASCII | 7 | refused, naming `prikk format upgrade` (U1) | `untracked`, `authoring: refused`, the upgrade sentence |
//! | contains ZWJ | 7 | refused for the character, **not** told to upgrade (an upgrade would not help) | `unsupported-path`, refused |
//!
//! The last two columns are the final shapes the CHANGELOG `### Upgrading` entry states for `worktree-status`.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::Path;
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

fn fresh(tag: &str) -> std::path::PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    repo
}

/// The names in `repo`'s top directory (not `.prikk`), as the file system reports them.
fn listed_names(repo: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(repo)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name != ".prikk")
        .collect();
    names.sort();
    names
}

/// Write `name`, or say the file system would not take it. **Platform-shaped** (the lesson of the APFS non-UTF-8 name): a file
/// system may refuse a name, or fold two canonically equivalent names into one entry (APFS compares names canonically while
/// keeping the form it was given), and a test of what `commit` does with such names can only run where both survive.
fn write_name(repo: &Path, name: &str) -> bool {
    std::fs::write(repo.join(name), b"x\n").is_ok()
}

fn worktree_status_json(repo: &Path) -> String {
    let output = support::prikk(repo)
        .args(["worktree-status", "--format", "json"])
        .output()
        .unwrap();
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// The one JSON line (each change is one line) that names `path`.
fn change_line<'a>(json: &'a str, path: &str) -> &'a str {
    json.lines()
        .find(|line| line.contains(&format!("\"path\": \"{path}\"")))
        .unwrap_or_else(|| panic!("a change for {path:?} in:\n{json}"))
}

#[test]
fn nfc_names_commit_seal_and_read_back_in_a_new_repository() {
    let repo = fresh("u2-nfc-roundtrip");
    let names = [
        "pages/\u{65e5}\u{672c}\u{8a9e}.md",
        "caf\u{e9}.md",
        "\u{d55c}\u{ae00}.txt",
        "emoji-\u{1f600}.txt",
    ];
    std::fs::create_dir_all(repo.join("pages")).unwrap();
    for name in names {
        std::fs::write(repo.join(name), name.as_bytes()).unwrap();
    }
    let status = worktree_status_json(&repo);
    for name in names {
        let line = change_line(&status, name);
        assert!(line.contains("\"kind\": \"untracked\""), "{line}");
        assert!(line.contains("\"authoring\": \"authored\""), "{line}");
    }
    support::ok(
        &support::commit(&repo, "heads/main", "unicode names"),
        "commit",
    );
    support::ok(&support::seal(&repo, "heads/main"), "seal");

    let tree = support::prikk(&repo).arg("tree").output().unwrap();
    support::ok(&tree, "tree");
    let listed = text(&tree);
    for name in names {
        assert!(listed.contains(name), "tree lists {name:?}:\n{listed}");
    }
    support::ok(
        &support::prikk(&repo).arg("verify").output().unwrap(),
        "verify",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn an_nfd_name_is_refused_naming_it_and_its_nfc_spelling_and_the_fix_works() {
    let repo = fresh("u2-nfd");
    if !write_name(&repo, "cafe\u{301}.md") || listed_names(&repo) != ["cafe\u{301}.md"] {
        eprintln!("skipped: this file system does not keep an NFD name as given");
        return;
    }

    let output = support::commit(&repo, "heads/main", "nfd");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains("not in Unicode normalization form C (NFC)"),
        "{said}"
    );
    assert!(
        said.contains("cafe\\u{301}.md"),
        "the name as given, with the mark written out: {said}"
    );
    assert!(said.contains("caf\\u{e9}.md"), "its NFC spelling: {said}");
    assert!(said.contains("never rewrites a name for you"), "{said}");

    let status = worktree_status_json(&repo);
    let line = change_line(&status, "cafe\u{301}.md");
    assert!(line.contains("\"kind\": \"untracked\""), "{line}");
    assert!(line.contains("\"authoring\": \"refused\""), "{line}");

    // The fix the sentence names: rename to the NFC spelling, then commit.
    std::fs::rename(repo.join("cafe\u{301}.md"), repo.join("caf\u{e9}.md")).unwrap();
    if listed_names(&repo) != ["caf\u{e9}.md"] {
        eprintln!(
            "skipped the rest: this file system kept the NFD spelling through a rename to its NFC twin"
        );
        return;
    }
    support::ok(
        &support::commit(&repo, "heads/main", "nfc"),
        "commit after the rename",
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn an_nfc_and_an_nfd_spelling_of_one_name_are_refused_naming_both() {
    let repo = fresh("u2-twins");
    std::fs::write(repo.join("caf\u{e9}.md"), b"nfc\n").unwrap();
    let wrote_twin = write_name(&repo, "cafe\u{301}.md");
    if !wrote_twin || listed_names(&repo).len() != 2 {
        eprintln!(
            "skipped: this file system folds canonically equivalent names into one entry (APFS)"
        );
        return;
    }
    let output = support::commit(&repo, "heads/main", "twins");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(
        said.contains("two entries with canonically equivalent names"),
        "{said}"
    );
    assert!(
        said.contains("cafe\\u{301}.md") && said.contains("caf\\u{e9}.md"),
        "both are named: {said}"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn a_refused_character_is_refused_in_every_format_and_names_the_character() {
    for (format, name, code, kind) in [
        (
            8u8,
            "a\u{200d}b.md",
            "U+200D (invisible format character)",
            "ZWJ",
        ),
        (
            8,
            "a\u{200c}b.md",
            "U+200C (invisible format character)",
            "ZWNJ",
        ),
        (8, "a\u{202e}b.md", "U+202E (bidirectional control)", "RLO"),
        (8, "a\u{fffe}b.md", "U+FFFE (noncharacter)", "noncharacter"),
        (8, "a\u{85}b.md", "U+0085 (control)", "C1 control"),
        (
            7,
            "a\u{200d}b.md",
            "U+200D (invisible format character)",
            "ZWJ at format 7",
        ),
    ] {
        let repo = fresh("u2-refused-character");
        set_marker(&repo, format);
        if !write_name(&repo, name) {
            eprintln!("skipped {kind}: this file system does not take that name");
            continue;
        }
        let output = support::commit(&repo, "heads/main", "refused");
        let said = text(&output);
        assert_eq!(output.status.code(), Some(1), "{kind}: {said}");
        assert!(said.contains(code), "{kind}: {said}");
        // The path in the sentence shows the refused character escaped: a raw ZWJ prints as nothing, so `a<ZWJ>b.md` would read
        // `ab.md` (the U2 review's item 1).
        let refused = name.chars().nth(1).unwrap();
        let escaped = format!("a\\u{{{:x}}}b.md", u32::from(refused));
        assert!(
            said.contains(&escaped),
            "{kind}: the path is written with the character escaped ({escaped}): {said}"
        );
        assert!(
            !said.contains(refused),
            "{kind}: the raw character is never printed: {said}"
        );
        assert!(
            !said.contains("format upgrade"),
            "{kind}: an upgrade would not help, so it is not suggested: {said}"
        );
        let status = worktree_status_json(&repo);
        let line = change_line(&status, name);
        assert!(
            line.contains("\"kind\": \"unsupported-path\""),
            "{kind}: {line}"
        );
        assert!(
            line.contains("\"authoring\": \"refused\""),
            "{kind}: {line}"
        );
        assert!(
            line.contains(&escaped.replace('\\', "\\\\")),
            "{kind}: the refusal text in the JSON is escaped too: {line}"
        );
        let _ = std::fs::remove_dir_all(&repo);
    }
}

#[test]
fn the_windows_superscript_device_names_are_refused() {
    let repo = fresh("u2-com-superscript");
    if !write_name(&repo, "COM\u{b9}.txt") {
        eprintln!("skipped: this file system (Windows) does not let the name be created at all");
        return;
    }
    let output = support::commit(&repo, "heads/main", "reserved");
    let said = text(&output);
    assert_eq!(output.status.code(), Some(1), "{said}");
    assert!(said.contains("Windows reserved path component"), "{said}");
    let _ = std::fs::remove_dir_all(&repo);
}

#[test]
fn worktree_status_final_shapes_per_format_and_form() {
    // The four cases the CHANGELOG states, side by side.
    let repo = fresh("u2-status-shapes");
    std::fs::write(repo.join("nfc-caf\u{e9}.md"), b"x\n").unwrap();
    std::fs::write(repo.join("plain.md"), b"x\n").unwrap();
    if !write_name(&repo, "nfd-cafe\u{301}.md") || !write_name(&repo, "zw\u{200d}j.md") {
        eprintln!("skipped: this file system does not take an NFD or a ZWJ name");
        return;
    }
    if !listed_names(&repo).contains(&"nfd-cafe\u{301}.md".to_string()) {
        eprintln!("skipped: this file system does not keep an NFD name as given");
        return;
    }

    // Format 8.
    let json = worktree_status_json(&repo);
    let nfc = change_line(&json, "nfc-caf\u{e9}.md");
    assert!(
        nfc.contains("\"kind\": \"untracked\"")
            && nfc.contains("\"authoring\": \"authored\"")
            && nfc.contains("\"refusal\": null"),
        "{nfc}"
    );
    let nfd = change_line(&json, "nfd-cafe\u{301}.md");
    assert!(
        nfd.contains("\"kind\": \"untracked\"") && nfd.contains("\"authoring\": \"refused\""),
        "{nfd}"
    );
    assert!(nfd.contains("not in Unicode normalization form C"), "{nfd}");
    let zwj = change_line(&json, "zw\u{200d}j.md");
    assert!(
        zwj.contains("\"kind\": \"unsupported-path\"")
            && zwj.contains("\"authoring\": \"refused\""),
        "{zwj}"
    );
    assert!(zwj.contains("U+200D"), "{zwj}");
    let plain = change_line(&json, "plain.md");
    assert!(
        plain.contains("\"kind\": \"untracked\"") && plain.contains("\"authoring\": \"authored\""),
        "{plain}"
    );
    assert!(
        json.contains("\"refused_count\": 2"),
        "the NFD name and the ZWJ name: {json}"
    );

    // Format 7: every non-ASCII name needs the upgrade (the ZWJ one is refused for the character, as before).
    set_marker(&repo, 7);
    let json = worktree_status_json(&repo);
    let nfc = change_line(&json, "nfc-caf\u{e9}.md");
    assert!(
        nfc.contains("\"kind\": \"untracked\"") && nfc.contains("\"authoring\": \"refused\""),
        "{nfc}"
    );
    assert!(nfc.contains("prikk format upgrade"), "{nfc}");
    let nfd = change_line(&json, "nfd-cafe\u{301}.md");
    assert!(
        nfd.contains("prikk format upgrade"),
        "the upgrade comes before the NFC rule: {nfd}"
    );
    let zwj = change_line(&json, "zw\u{200d}j.md");
    assert!(
        zwj.contains("\"kind\": \"unsupported-path\"") && !zwj.contains("format upgrade"),
        "{zwj}"
    );
    let plain = change_line(&json, "plain.md");
    assert!(plain.contains("\"authoring\": \"authored\""), "{plain}");
    let _ = std::fs::remove_dir_all(&repo);
}
