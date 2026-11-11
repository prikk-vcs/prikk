//! 0.57.0 U5: a Unicode repository path through the whole product, through the compiled binary. One name is authored, sealed,
//! verified, read, extracted, exported as a bundle and as an archive, refused by a format-7 receiver (nothing written), accepted
//! after `format upgrade`, adopted, and read again at the far end. The pieces have their own tests (U1-U4); this is the one
//! walk that shows they agree, and the refusals a user meets on the way (an NFD name, an invisible character, a collision).
//!
//! The case table's platform rows: the names below are NFC and the file systems CI runs (ext4/tmpfs, APFS, NTFS) all keep an NFC
//! name as written, so every row but the collision one runs everywhere; the collision row and the NFD row skip where the file
//! system folds or normalizes names (they report why on stderr), and are read from the unit tests there.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use super::u4_artifact_format::{
    import_archive, import_bundle, receiver, set_marker, store_bytes, temp_file, text,
};
use crate::support;
use std::path::Path;

const PAGE: &str = "pages/\u{65e5}\u{672c}\u{8a9e}.md";
const CAFE: &str = "caf\u{e9}.md";

fn run(repo: &Path, args: &[&str]) -> std::process::Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn read_back(dir: &Path, name: &str) -> Vec<u8> {
    std::fs::read(dir.join(name)).unwrap_or_else(|err| panic!("{name:?} in {dir:?}: {err}"))
}

#[test]
fn a_unicode_name_goes_from_commit_to_a_far_repository_and_back_out_byte_for_byte() {
    // 1. A new repository is format 8; commit records the NFC names; seal and verify are clean.
    let source = support::unique_repo("u5-source");
    support::init(&source);
    assert_eq!(
        std::fs::read(source.join(".prikk").join("FORMAT")).unwrap(),
        b"8\n",
        "a new repository is created at format 8"
    );
    std::fs::create_dir_all(source.join("pages")).unwrap();
    support::generation(
        &source,
        "heads/main",
        PAGE,
        "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\n".as_bytes(),
        "page",
    );
    support::generation(&source, "heads/main", CAFE, b"espresso\n", "cafe");
    support::ok(&run(&source, &["verify"]), "verify at the source");

    // 2. Read it back through the history: `tree` lists both names, `cat` returns the bytes.
    let tree = run(&source, &["tree"]);
    support::ok(&tree, "tree");
    for name in [PAGE, CAFE] {
        assert!(
            text(&tree).contains(name),
            "tree lists {name:?}:\n{}",
            text(&tree)
        );
    }
    let cat = run(&source, &["cat", "--path", CAFE]);
    support::ok(&cat, "cat");
    assert_eq!(cat.stdout, b"espresso\n");

    // 3. Extract writes the names as given.
    let extracted = std::env::temp_dir().join(format!("u5-extract-{}", support::unique_suffix()));
    support::ok(
        &run(
            &source,
            &[
                "extract",
                "--ref",
                "heads/main",
                extracted.to_str().unwrap(),
            ],
        ),
        "extract",
    );
    assert_eq!(read_back(&extracted, CAFE), b"espresso\n");
    assert_eq!(
        read_back(&extracted, PAGE),
        "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\n".as_bytes()
    );

    // 4. The artifacts declare format 8.
    let bundle = temp_file("u5", "pbndl");
    let exported = run(
        &source,
        &[
            "bundle",
            "export",
            "--ref",
            "heads/main",
            "--output",
            bundle.to_str().unwrap(),
        ],
    );
    support::ok(&exported, "bundle export");
    assert!(
        text(&exported).contains("repository format: 8"),
        "{}",
        text(&exported)
    );
    let archive = temp_file("u5", "parc");
    support::ok(
        &run(&source, &["archive", "export", archive.to_str().unwrap()]),
        "archive export",
    );

    // 5. A format-7 receiver refuses both, naming the path, and writes nothing; it moves to 8 only by the explicit upgrade.
    let old = receiver("u5-old", 7);
    let before = store_bytes(&old);
    for (noun, output) in [
        ("bundle", import_bundle(&old, &bundle)),
        ("archive", import_archive(&old, &archive)),
    ] {
        let said = text(&output);
        assert_eq!(output.status.code(), Some(1), "{noun}: {said}");
        assert!(
            said.contains("format upgrade"),
            "{noun} names the way forward: {said}"
        );
        assert_eq!(store_bytes(&old), before, "a refused {noun} wrote nothing");
    }
    support::ok(&run(&old, &["format", "upgrade"]), "format upgrade");
    // `verify` checks a received ref once the sealing key is adopted (the import itself trusts nothing).
    support::trust_maintainer(&old);
    support::ok(
        &import_bundle(&old, &bundle),
        "bundle import after the upgrade",
    );
    support::ok(&run(&old, &["verify"]), "verify after the bundle import");

    // 6. The archive adopts as a real local branch in a fresh format-8 repository, and the names come out again.
    let far = support::unique_repo("u5-far");
    support::init(&far);
    support::trust_maintainer(&far);
    support::ok(
        &run(
            &far,
            &["archive", "import", archive.to_str().unwrap(), "--adopt"],
        ),
        "archive import --adopt",
    );
    support::ok(&run(&far, &["verify"]), "verify at the far end");
    let out = std::env::temp_dir().join(format!("u5-far-extract-{}", support::unique_suffix()));
    support::ok(
        &run(
            &far,
            &["extract", "--ref", "heads/main", out.to_str().unwrap()],
        ),
        "extract at the far end",
    );
    assert_eq!(read_back(&out, CAFE), b"espresso\n");
    assert_eq!(
        read_back(&out, PAGE),
        "\u{3053}\u{3093}\u{306b}\u{3061}\u{306f}\n".as_bytes()
    );

    for path in [&source, &old, &far, &extracted, &out, &bundle, &archive] {
        let _ = std::fs::remove_dir_all(path);
        let _ = std::fs::remove_file(path);
    }
}

#[test]
fn the_refusals_a_user_meets_name_the_path_and_the_fix() {
    let repo = support::unique_repo("u5-refusals");
    support::init(&repo);

    // An invisible character: refused at commit, the path written with the character escaped (a raw one would print as nothing).
    std::fs::write(repo.join("a\u{200d}b.md"), b"x\n").unwrap();
    let invisible = support::commit(&repo, "heads/main", "zwj");
    let said = text(&invisible);
    assert_eq!(invisible.status.code(), Some(1), "{said}");
    assert!(
        said.contains("a\\u{200d}b.md") && !said.contains('\u{200d}'),
        "{said}"
    );
    std::fs::remove_file(repo.join("a\u{200d}b.md")).unwrap();

    // Two names that are one name under Unicode case folding: `commit` records both, `seal` refuses naming both.
    support::generation(&repo, "heads/main", "base.txt", b"base\n", "base");
    std::fs::write(repo.join("Stra\u{df}e.txt"), b"1\n").unwrap();
    let wrote = std::fs::write(repo.join("STRASSE.txt"), b"2\n").is_ok();
    let distinct = std::fs::read_dir(&repo).unwrap().count() == 4; // .prikk, base.txt and the two names
    if wrote && distinct {
        support::ok(&support::commit(&repo, "heads/main", "both"), "commit");
        let sealed = support::seal(&repo, "heads/main");
        let said = text(&sealed);
        assert_eq!(sealed.status.code(), Some(1), "{said}");
        assert!(
            said.contains("STRASSE.txt") && said.contains("Stra\\u{df}e.txt"),
            "{said}"
        );
        assert!(said.contains("Unicode case folding"), "{said}");
    } else {
        eprintln!("skipped the collision row: this file system treats the two names as one");
    }

    // A repository below format 8 refuses to author a non-ASCII name, naming the upgrade (the only place prikk suggests it).
    set_marker(&repo, 7);
    let _ = std::fs::remove_file(repo.join("Stra\u{df}e.txt"));
    let _ = std::fs::remove_file(repo.join("STRASSE.txt"));
    std::fs::write(repo.join("caf\u{e9}.md"), b"c\n").unwrap();
    let old = support::commit(&repo, "heads/main", "cafe");
    assert_eq!(old.status.code(), Some(1), "{}", text(&old));
    assert!(
        text(&old).contains("prikk format upgrade"),
        "{}",
        text(&old)
    );
    let _ = std::fs::remove_dir_all(&repo);
}
