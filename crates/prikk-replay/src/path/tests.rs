#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{RepoPath, validate_no_path_collisions};

#[test]
fn repo_path_accepts_lexical_repository_relative_path() -> prikk_error::Result<()> {
    let path = RepoPath::parse("src/main.rs")?;

    assert_eq!(path.as_str(), "src/main.rs");
    Ok(())
}

#[test]
fn repo_path_rejects_filesystem_or_platform_ambiguous_paths() {
    for path in [
        "",
        "/absolute",
        "../escape",
        "src/../escape",
        ".prikk/FORMAT",
        ".PRIKK/FORMAT",
        "src\\main.rs",
        "CON.txt",
        // A character the decode table refuses (ZWJ): outside the rule in every format. A non-ASCII name in itself is no
        // longer refused (0.57.0, format 8): see `repo_path_accepts_non_ascii_names_in_either_normalization_form`.
        "zw\u{200d}j.txt",
        "COM\u{b9}.txt",
    ] {
        assert!(RepoPath::parse(path).is_err(), "{path}");
    }
}

#[test]
fn repo_path_accepts_non_ascii_names_in_either_normalization_form() {
    // Decode does not ask for NFC (ruling C3); only `commit` does.
    for path in [
        "日本語.txt",
        "caf\u{e9}.txt",
        "cafe\u{301}.txt",
        "dir/한글.md",
    ] {
        assert!(RepoPath::parse(path).is_ok(), "{path}");
    }
}

#[test]
fn repo_path_detects_exact_and_case_folded_collisions() -> prikk_error::Result<()> {
    let exact = [RepoPath::parse("README.md")?, RepoPath::parse("README.md")?];
    assert!(validate_no_path_collisions(&exact).is_err());

    let folded = [RepoPath::parse("README.md")?, RepoPath::parse("readme.md")?];
    assert!(validate_no_path_collisions(&folded).is_err());
    Ok(())
}

// ---- 0.57.0 U3: collisions are judged by the canonical caseless key ----------------------------------------------------

use prikk_error::PrikkError;

fn collide(left: &str, right: &str) -> Option<PrikkError> {
    let paths = [
        RepoPath::parse(left).unwrap(),
        RepoPath::parse(right).unwrap(),
    ];
    validate_no_path_collisions(&paths).err()
}

#[test]
fn two_paths_with_one_canonical_caseless_key_collide_and_the_sentence_names_both() {
    // Each pair is two individually valid paths that a case-insensitive or normalizing file system makes one.
    let colliding = [
        ("README.md", "readme.md"),
        ("Stra\u{df}e.txt", "STRASSE.txt"),
        ("Stra\u{df}e.txt", "strasse.txt"),
        ("\u{c9}.md", "\u{e9}.md"),
        ("caf\u{e9}.md", "cafe\u{301}.md"),
        ("\u{1e9e}.md", "ss.md"),
        ("\u{212a}.md", "k.md"),
        ("dir/\u{c9}cole.md", "dir/e\u{301}cole.md"),
        ("a\u{345}\u{301}.md", "a\u{301}\u{345}.md"),
    ];
    for (left, right) in colliding {
        let err = collide(left, right).unwrap_or_else(|| panic!("{left:?} and {right:?} collide"));
        let said = err.to_string();
        assert!(
            said.contains("case-insensitive path collision involving: "),
            "{said}"
        );
        assert!(
            !said.contains(|c: char| !c.is_ascii()),
            "the sentence prints no raw non-ASCII: {said}"
        );
        let escaped_left = prikk_error::escape_non_ascii(left);
        let escaped_right = prikk_error::escape_non_ascii(right);
        assert!(
            said.contains(&escaped_left) && said.contains(&escaped_right),
            "both named: {said}"
        );
    }
    // Different names stay different: the directory is part of the path, and a different letter is a different letter.
    let distinct = [
        ("a/\u{e9}.md", "b/\u{e9}.md"),
        ("e.md", "\u{e9}.md"),
        ("ss.md", "s.md"),
        ("\u{65e5}\u{672c}.md", "\u{65e5}\u{672c}\u{8a9e}.md"),
        ("file.md", "files.md"),
    ];
    for (left, right) in distinct {
        assert!(
            collide(left, right).is_none(),
            "{left:?} and {right:?} are different names"
        );
    }
}

#[test]
fn an_exact_duplicate_keeps_its_own_sentence() {
    let one = RepoPath::parse("caf\u{e9}.md").unwrap();
    let err = validate_no_path_collisions(&[one.clone(), one]).expect_err("duplicate");
    assert!(
        err.to_string().contains("duplicate repository path"),
        "{err}"
    );
}
