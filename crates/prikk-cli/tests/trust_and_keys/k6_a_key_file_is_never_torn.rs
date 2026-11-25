//! 0.59.0 step K6: a key file is never torn or replaced, a torn one left by an earlier release is said to be that, and `setup` finishes its own
//! interrupted run. Test keys only, in each test's own isolated key directory; no seed is ever printed here.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::Command;

fn run(command: &mut Command) -> (Option<i32>, String) {
    let output = command.output().unwrap();
    (
        output.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
    )
}

/// A command in `repo` that uses `home_of`'s key directory (a second repository sharing one user's keys).
fn prikk_with_keys_of(repo: &Path, home_of: &Path) -> Command {
    let mut command = support::prikk(repo);
    let key_dir = support::isolated_key_dir(home_of);
    let home = key_dir.parent().unwrap().to_path_buf();
    command
        .env("XDG_CONFIG_HOME", &home)
        .env("HOME", &home)
        .env("APPDATA", &home);
    command
}

/// The two paths a sentence tells the user to remove: ``remove `a` and `b` ``.
fn removals(sentence: &str) -> Vec<PathBuf> {
    let after = sentence
        .split("; remove ")
        .nth(1)
        .unwrap_or_else(|| panic!("the sentence says nothing to remove: {sentence}"));
    let mut paths = Vec::new();
    let mut rest = after;
    while let Some(inside) = rest.strip_prefix('`') {
        let (path, tail) = inside.split_once('`').unwrap();
        paths.push(PathBuf::from(path));
        match tail.strip_prefix(" and ") {
            Some(next) => rest = next,
            None => break,
        }
    }
    paths
}

/// A repository with a good key directory of its own: `setup`, one commit.
fn good_repository(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    let (code, text) = run(support::prikk(&repo).arg("setup"));
    assert_eq!(code, Some(0), "{text}");
    std::fs::write(repo.join("a.txt"), "a\n").unwrap();
    let (code, text) = run(support::prikk(&repo).args(["commit", "--from-worktree", "-m", "a"]));
    assert_eq!(code, Some(0), "{text}");
    repo
}

/// The state a `setup` killed while it wrote the first key left in the user's key directory in every earlier release: the key-id file whole, the
/// seed created and empty.
fn tear_the_author_seed(repo: &Path) -> PathBuf {
    let seed = support::isolated_key_dir(repo).join("author.seed");
    std::fs::write(&seed, b"").unwrap();
    seed
}

#[test]
fn a_torn_seed_in_the_key_directory_is_said_to_be_that_by_every_reader_and_removing_its_pair_works()
{
    let repo = good_repository("k6-torn-seed");
    let seed = tear_the_author_seed(&repo);
    let key_id = support::isolated_key_dir(&repo).join("author.key-id");
    assert!(key_id.exists());
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    let other = support::unique_repo("k6-torn-seed-other");
    let mut sentences = Vec::new();
    for mut command in [
        {
            let mut c = support::prikk(&repo);
            c.args(["commit", "--from-worktree", "-m", "b"]);
            c
        },
        {
            let mut c = support::prikk(&repo);
            c.args(["key", "status"]);
            c
        },
        {
            let mut c = support::prikk(&repo);
            c.args(["key", "public"]);
            c
        },
        {
            let mut c = prikk_with_keys_of(&other, &repo);
            c.arg("setup");
            c
        },
    ] {
        let (code, text) = run(&mut command);
        // `key status` reports rather than refusing; every other reader refuses.
        if !text.contains("holds 0 bytes, not a whole seed") {
            panic!("{command:?} did not say the seed is torn: {text}");
        }
        assert!(
            text.contains("nothing was signed with it")
                && text.contains("prikk never deletes a key file"),
            "{text}"
        );
        if !format!("{command:?}").contains("\"status\"") {
            assert_ne!(code, Some(0), "{command:?}: {text}");
        }
        sentences.push(text);
    }
    let paths = removals(&sentences[0]);
    assert_eq!(
        paths,
        vec![seed.clone(), key_id.clone()],
        "the pair, both named"
    );
    for path in &paths {
        std::fs::remove_file(path).unwrap();
    }
    #[cfg(unix)]
    {
        // After removing the pair, this key directory holds the maintainer seed alone, and `setup` in another repository says what that needs (a command
        // that works: `key generate --out` the missing seed), and then works; `commit` here works.
        let (code, text) = run(prikk_with_keys_of(&other, &repo).arg("setup"));
        assert_ne!(code, Some(0), "{text}");
        assert!(
            text.contains(&format!("`prikk key generate --out {}`", seed.display())),
            "{text}"
        );
        let (code, text) = run(prikk_with_keys_of(&other, &repo)
            .args(["key", "generate", "--out"])
            .arg(&seed));
        assert_eq!(code, Some(0), "{text}");
        let (code, text) = run(prikk_with_keys_of(&other, &repo).arg("setup"));
        assert_eq!(code, Some(0), "{text}");
        let (code, text) =
            run(support::prikk(&repo).args(["commit", "--from-worktree", "-m", "b"]));
        assert_eq!(code, Some(0), "{text}");
    }
    // (Windows refuses `key generate --out` for an arbitrary path: the seed is created by `setup` alone there.)
    #[cfg(windows)]
    let _ = (&other, &seed);
}

#[test]
fn a_torn_or_orphaned_key_id_is_said_to_be_that_too() {
    let repo = good_repository("k6-torn-key-id");
    let dir = support::isolated_key_dir(&repo);
    // A key-id file cut short, and no seed beside it (the key-id is written first): what a kill left.
    std::fs::remove_file(dir.join("author.seed")).unwrap();
    std::fs::write(dir.join("author.key-id"), b"ed25519-3f9a").unwrap();
    std::fs::write(repo.join("b.txt"), "b\n").unwrap();
    let (code, text) = run(support::prikk(&repo).args(["commit", "--from-worktree", "-m", "b"]));
    assert_ne!(code, Some(0));
    assert!(
        text.contains("holds 12 bytes, not a whole key id"),
        "{text}"
    );
    assert_eq!(removals(&text), vec![dir.join("author.key-id")], "{text}");
    // A whole key-id and no seed: an interrupted creation, said so, and named.
    std::fs::write(dir.join("author.key-id"), b"ed25519-3f9a3f9a3f9a3f9a\n").unwrap();
    let (code, text) = run(support::prikk(&repo).args(["commit", "--from-worktree", "-m", "b"]));
    assert_ne!(code, Some(0));
    assert!(
        text.contains("is here and") && text.contains("is not: a key creation was interrupted"),
        "{text}"
    );
    assert_eq!(removals(&text), vec![dir.join("author.key-id")], "{text}");
    // A key id that is not part of one (hand-edited) is not called torn.
    std::fs::write(dir.join("author.key-id"), b"my-own-id\n").unwrap();
    let (code, text) = run(support::prikk(&repo).args(["commit", "--from-worktree", "-m", "b"]));
    assert_ne!(code, Some(0));
    assert!(!text.contains("not a whole key id"), "{text}");
}

#[cfg(unix)]
#[test]
fn key_generate_out_over_a_torn_pair_says_so_and_never_replaces_a_whole_one() {
    let repo = support::unique_repo("k6-generate-out");
    let (code, text) = run(support::prikk(&repo).args(["key", "generate", "--out", "seed.out"]));
    assert_eq!(code, Some(0), "{text}");
    let whole = std::fs::read(repo.join("seed.out")).unwrap();
    assert_eq!(whole.len(), 65);
    assert!(repo.join("seed.out.key-id").exists());
    // A whole pair is an existing key: refused, both files as they were.
    let before = std::fs::read(repo.join("seed.out.key-id")).unwrap();
    let (code, text) = run(support::prikk(&repo).args(["key", "generate", "--out", "seed.out"]));
    assert_ne!(code, Some(0));
    assert!(
        text.contains("refusing to overwrite an existing file"),
        "{text}"
    );
    assert_eq!(std::fs::read(repo.join("seed.out")).unwrap(), whole);
    assert_eq!(std::fs::read(repo.join("seed.out.key-id")).unwrap(), before);
    // A torn seed, left by an earlier release: said to be that.
    std::fs::write(repo.join("seed.out"), &whole[..20]).unwrap();
    let (code, text) = run(support::prikk(&repo).args(["key", "generate", "--out", "seed.out"]));
    assert_ne!(code, Some(0));
    assert!(text.contains("holds 20 bytes, not a whole seed"), "{text}");
    for path in removals(&text) {
        std::fs::remove_file(repo.join(path)).unwrap();
    }
    let (code, text) = run(support::prikk(&repo).args(["key", "generate", "--out", "seed.out"]));
    assert_eq!(code, Some(0), "{text}");
    // No temporary file is left by a run that finished.
    let leftovers: Vec<_> = std::fs::read_dir(&repo)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp."))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
}

#[test]
fn setup_finishes_a_repository_nothing_was_adopted_in_and_spells_out_the_adoption_for_one_that_has_history()
 {
    // `init`, then `setup` in the same directory: nothing was adopted, so `setup` finishes it (before K6 it refused naming a command with no arguments).
    let repo = support::unique_repo("k6-setup-finishes");
    let (code, text) = run(support::prikk(&repo).arg("init"));
    assert_eq!(code, Some(0), "{text}");
    let (code, text) = run(support::prikk(&repo).arg("setup"));
    assert_eq!(code, Some(0), "{text}");
    assert!(
        text.contains("finishing the repository an interrupted"),
        "{text}"
    );
    assert!(text.contains("trusted maintainer key:"), "{text}");
    let (code, text) = run(support::prikk(&repo).arg("verify"));
    assert_eq!(code, Some(0), "{text}");
    // A killed `init` (no marker) is finished the same way.
    let killed = support::unique_repo("k6-setup-finishes-killed-init");
    support::init(&killed);
    std::fs::remove_file(killed.join(".prikk/FORMAT")).unwrap();
    std::fs::remove_file(killed.join(".prikk/worktree.baseline")).unwrap();
    let (code, text) = run(support::prikk(&killed).arg("setup"));
    assert_eq!(code, Some(0), "{text}");
    assert_eq!(std::fs::read(killed.join(".prikk/FORMAT")).unwrap(), b"8\n");
    // Now it holds an adopted key: `setup` refuses, and the sentence is a command that can be run as written.
    let (code, text) = run(support::prikk(&repo).arg("setup"));
    assert_ne!(code, Some(0));
    let command = text
        .split('`')
        .skip(1)
        .step_by(2)
        .find_map(|inside| inside.strip_prefix("prikk trust maintainer add --key-id "))
        .unwrap_or_else(|| panic!("the refusal spells out no command: {text}"));
    let words: Vec<&str> = std::iter::once("trust")
        .chain(["maintainer", "add", "--key-id"])
        .chain(command.split_whitespace())
        .collect();
    assert_eq!(words.len(), 7, "{words:?}");
    let (code, text) = run(support::prikk(&repo).args(&words));
    assert_eq!(
        code,
        Some(0),
        "the named command, as printed, exits 0: {text}"
    );
}

#[test]
fn a_key_file_is_published_whole_and_never_over_one_that_is_there() {
    // `key generate --out` publishes through a temporary and a no-replace rename; there is nothing but the whole seed and its key id afterwards.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let repo = support::unique_repo("k6-publish");
        let (code, text) = run(support::prikk(&repo).args(["key", "generate", "--out", "s"]));
        assert_eq!(code, Some(0), "{text}");
        for name in ["s", "s.key-id"] {
            let mode = std::fs::metadata(repo.join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "{name}");
        }
        let names: Vec<String> = std::fs::read_dir(&repo)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }
}
