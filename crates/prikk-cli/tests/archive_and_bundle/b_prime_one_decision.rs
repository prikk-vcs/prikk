//! 0.58.0 B′b: `bundle preview` and `bundle import` are one decision. The preview lists every refusal the import would raise (in the
//! import's order, with the import's own sentence), takes the import's decision-changing flags, and writes nothing.
//!
//! **The primary witness that the decision writes nothing is behavioural** (ruling 1): for every refusal kind and the clean path the
//! whole `.prikk` tree, lock files and caches included, is byte-identical after the preview, and after a refused import. **Red control:**
//! a write inserted into `decide` (`write_received_pointer`) turns `a_preview_writes_nothing_for_any_row` red.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use prikk_store::MaintainerSigner as _;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

const RECEIVER_KEY_ID: &str = "bprime-receiver";
const RECEIVER_SEED: [u8; 32] = [0x77; 32];

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn as_receiver(repo: &Path, args: &[&str]) -> Output {
    support::prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", RECEIVER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&RECEIVER_SEED)),
        )
        .args(args)
        .output()
        .unwrap()
}

fn trust_receiver_key(repo: &Path) {
    let signer =
        prikk_store::Ed25519MaintainerSigner::from_seed(RECEIVER_KEY_ID, &RECEIVER_SEED).unwrap();
    support::ok(
        &support::prikk(repo)
            .args([
                "trust",
                "maintainer",
                "add",
                "--key-id",
                RECEIVER_KEY_ID,
                "--public-key",
                &support::hex(&signer.public_key_bytes()),
            ])
            .output()
            .unwrap(),
        "trust the receiver's key",
    );
}

/// A receiver that has adopted the sender's key and its own.
fn receiver(tag: &str) -> PathBuf {
    let repo = support::unique_repo(tag);
    support::init(&repo);
    support::trust_maintainer(&repo);
    trust_receiver_key(&repo);
    repo
}

fn bundle_file(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("bprime-{tag}-{}.pbndl", support::unique_suffix()))
}

fn export(repo: &Path, ref_name: &str, tag: &str) -> PathBuf {
    let bundle = bundle_file(tag);
    support::ok(
        &support::prikk(repo)
            .args(["bundle", "export", "--ref", ref_name, "--output"])
            .arg(&bundle)
            .output()
            .unwrap(),
        "bundle export",
    );
    bundle
}

/// A sender with two generations and the bundles of its two states: `(older, newer)`.
fn sender_with_two_bundles(tag: &str) -> (PathBuf, PathBuf) {
    let sender = support::unique_repo(tag);
    support::init(&sender);
    support::generation(&sender, "heads/main", "a.txt", b"one\n", "one");
    let older = export(&sender, "heads/main", &format!("{tag}-older"));
    support::generation(&sender, "heads/main", "b.txt", b"two\n", "two");
    let newer = export(&sender, "heads/main", &format!("{tag}-newer"));
    (older, newer)
}

fn import(repo: &Path, bundle: &Path, flags: &[&str]) -> Output {
    let mut args = vec!["bundle", "import", "--input", bundle.to_str().unwrap()];
    args.extend_from_slice(flags);
    as_receiver(repo, &args)
}

fn preview(repo: &Path, bundle: &Path, flags: &[&str]) -> Output {
    let mut args = vec!["bundle", "preview", "--input", bundle.to_str().unwrap()];
    args.extend_from_slice(flags);
    as_receiver(repo, &args)
}

/// Every file and directory under `.prikk`, lock files and caches included.
fn tree_bytes(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                // A directory is part of the tree: one created under `.prikk` is a write.
                files.insert(path.clone(), Vec::new());
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// The refusal ids the preview lists, in order, and each one's message.
fn listed(preview_stdout: &str) -> Vec<(String, String)> {
    let lines: Vec<&str> = preview_stdout.lines().collect();
    let mut out = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if let Some(id) = line.strip_prefix("bundle import of this delivery would refuse: ") {
            let message = lines.get(index + 1).map_or("", |next| next.trim());
            out.push((id.to_string(), message.to_string()));
        }
    }
    out
}

/// What `bundle import` printed as its refusal: stderr without the `error: ` prefix.
fn refusal_sentence(import_output: &Output) -> String {
    String::from_utf8_lossy(&import_output.stderr)
        .lines()
        .next()
        .unwrap_or("")
        .trim_start_matches("error: ")
        .to_string()
}

/// A second sender whose AUTHOR key has **the same id as the first's and another key**, on `heads/other`, so the delivery shares no
/// history and no ref name with the first.
fn author_key_conflict_pair() -> (PathBuf, PathBuf) {
    author_key_conflict_pair_on("heads/other")
}

/// [`author_key_conflict_pair`] with the second delivery's ref name chosen: `heads/main` makes it **also** unrelated to the first's history
/// under the same name (a second refusal, `does-not-connect`).
fn author_key_conflict_pair_on(second_ref: &str) -> (PathBuf, PathBuf) {
    let first = support::unique_repo("bprime-ak-first");
    support::init(&first);
    support::generation(&first, "heads/main", "k.txt", b"first\n", "first");
    let first_bundle = export(&first, "heads/main", "ak-first");
    let second = support::unique_repo("bprime-ak-second");
    support::init(&second);
    std::fs::write(second.join("k2.txt"), b"second\n").unwrap();
    support::ok(
        &support::prikk(&second)
            .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
            .env(
                "PRIKK_AUTHOR_SEED_FILE",
                support::seed_file(&"11".repeat(32)),
            )
            .args(["commit", "--ref", second_ref, "-m", "second"])
            .output()
            .unwrap(),
        "commit with another author key under the same id",
    );
    support::ok(&support::seal(&second, second_ref), "seal");
    let second_bundle = export(&second, second_ref, "ak-second");
    (first_bundle, second_bundle)
}

/// **The defect B′ exists for.** An author-key conflict: the preview used to read clean while the import refused. Now it lists it, with
/// the import's sentence, and the import refuses with that sentence.
#[test]
fn an_author_key_conflict_is_listed_by_the_preview_and_refused_by_the_import() {
    let (first, second) = author_key_conflict_pair();
    let repo = receiver("bprime-ak-receiver");
    support::ok(&import(&repo, &first, &[]), "the first delivery");
    let before = tree_bytes(&repo);

    let previewed = preview(&repo, &second, &[]);
    assert_eq!(previewed.status.code(), Some(0), "{}", text(&previewed));
    let stdout = String::from_utf8_lossy(&previewed.stdout).to_string();
    let listed = listed(&stdout);
    assert_eq!(
        listed.first().map(|(id, _)| id.as_str()),
        Some("author-key-conflict"),
        "{stdout}"
    );
    assert_eq!(tree_bytes(&repo), before, "the preview wrote nothing");

    let refused = import(&repo, &second, &[]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert_eq!(
        refusal_sentence(&refused),
        listed[0].1,
        "the preview says what the import says"
    );
    assert_eq!(tree_bytes(&repo), before, "a refused import wrote nothing");
}

/// `--allow-non-fast-forward` is the import's flag, and it changes the decision: without it the preview lists the class refusal, with it
/// the preview lists none and the import lands.
#[test]
fn the_preview_takes_allow_non_fast_forward_and_answers_for_the_import_that_follows() {
    let (older, newer) = sender_with_two_bundles("bprime-nff");
    let repo = receiver("bprime-nff-receiver");
    support::ok(&import(&repo, &newer, &[]), "the newer delivery");
    let before = tree_bytes(&repo);

    let without = preview(&repo, &older, &[]);
    let stdout = String::from_utf8_lossy(&without.stdout).to_string();
    assert_eq!(
        listed(&stdout).first().map(|(id, _)| id.as_str()),
        Some("rollback"),
        "{stdout}"
    );
    let with = preview(&repo, &older, &["--allow-non-fast-forward"]);
    assert_eq!(with.status.code(), Some(0), "{}", text(&with));
    assert!(
        listed(&String::from_utf8_lossy(&with.stdout)).is_empty(),
        "{}",
        text(&with)
    );
    assert_eq!(tree_bytes(&repo), before);
    support::ok(
        &import(&repo, &older, &["--allow-non-fast-forward"]),
        "the import the preview answered for",
    );
}

/// `--adopt` on a preview: every adoption refusal the import has, with the import's sentence, and where a clean delivery would land.
#[test]
fn the_preview_answers_for_adopt_with_the_imports_own_refusals() {
    let (_older, newer) = sender_with_two_bundles("bprime-adopt");

    // A queued commit owns the name.
    let queued = receiver("bprime-adopt-queued");
    std::fs::write(queued.join("q.txt"), b"queued\n").unwrap();
    support::ok(
        &support::commit(&queued, "heads/main", "queued"),
        "an unsealed commit",
    );
    let before = tree_bytes(&queued);
    let previewed = preview(&queued, &newer, &["--adopt"]);
    assert_eq!(previewed.status.code(), Some(0), "{}", text(&previewed));
    let stdout = String::from_utf8_lossy(&previewed.stdout).to_string();
    let found = listed(&stdout);
    assert_eq!(
        found.first().map(|(id, _)| id.as_str()),
        Some("adoption-refused"),
        "{stdout}"
    );
    assert_eq!(tree_bytes(&queued), before, "the preview wrote nothing");
    let refused = import(&queued, &newer, &["--adopt"]);
    assert_eq!(refused.status.code(), Some(1), "{}", text(&refused));
    assert_eq!(refusal_sentence(&refused), found[0].1);
    assert_eq!(
        tree_bytes(&queued),
        before,
        "a refused import wrote nothing"
    );

    // A file is in the way of the tip.
    let in_the_way = receiver("bprime-adopt-way");
    std::fs::write(in_the_way.join("a.txt"), b"not what the branch holds\n").unwrap();
    let before = tree_bytes(&in_the_way);
    let previewed = preview(&in_the_way, &newer, &["--adopt"]);
    let stdout = String::from_utf8_lossy(&previewed.stdout).to_string();
    let found = listed(&stdout);
    assert_eq!(
        found.first().map(|(id, _)| id.as_str()),
        Some("adoption-refused"),
        "{stdout}"
    );
    assert_eq!(tree_bytes(&in_the_way), before);
    assert!(
        !stdout.contains("would land") && !stdout.contains("import_adoption\": {"),
        "a refused adoption lands nowhere: {stdout}"
    );
    let refused = import(&in_the_way, &newer, &["--adopt"]);
    assert_eq!(refusal_sentence(&refused), found[0].1);

    // A clean delivery: it would land as a local branch, and the JSON says so; without the flag the field is null.
    let clean = receiver("bprime-adopt-clean");
    let before = tree_bytes(&clean);
    let landed = preview(&clean, &newer, &["--adopt", "--format", "json"]);
    let json = String::from_utf8_lossy(&landed.stdout).to_string();
    assert!(
        json.contains("\"import_adoption\": {\"adopted\": true, \"landed_as\": \"heads/main\""),
        "{json}"
    );
    let plain = preview(&clean, &newer, &["--format", "json"]);
    assert!(
        String::from_utf8_lossy(&plain.stdout).contains("\"import_adoption\": null"),
        "{}",
        text(&plain)
    );
    assert_eq!(tree_bytes(&clean), before);
    assert!(text(&import(&clean, &newer, &["--adopt"])).contains("adopted as a local branch"));

    // The sender's key is not adopted: it lands under `remotes/`, with the import's own reason.
    let unadopted = support::unique_repo("bprime-adopt-unadopted");
    support::init(&unadopted);
    trust_receiver_key(&unadopted);
    let previewed = preview(&unadopted, &newer, &["--adopt"]);
    let said = text(&previewed);
    assert!(
        said.contains("would land: remotes/heads/main (not adopted)")
            && said.contains("has not adopted that key"),
        "{said}"
    );
    let imported = import(&unadopted, &newer, &["--adopt"]);
    assert!(
        text(&imported).contains("has not adopted that key"),
        "{}",
        text(&imported)
    );
}

/// A read-only command opens no secret: `--adopt` on a preview judges the configured key id from the trust policy and says the import's
/// own sentence when the seed file is absent. It never reads the seed (the receiver here has no seed file at all).
#[test]
fn a_preview_with_adopt_and_no_maintainer_key_says_what_the_import_says() {
    let (_older, newer) = sender_with_two_bundles("bprime-nokey");
    let repo = support::unique_repo("bprime-nokey-receiver");
    support::init(&repo);
    support::trust_maintainer(&repo);
    let previewed = support::prikk(&repo)
        .args([
            "bundle",
            "preview",
            "--input",
            newer.to_str().unwrap(),
            "--adopt",
        ])
        .output()
        .unwrap();
    let imported = support::prikk(&repo)
        .args([
            "bundle",
            "import",
            "--input",
            newer.to_str().unwrap(),
            "--adopt",
        ])
        .output()
        .unwrap();
    assert_eq!(previewed.status.code(), Some(1), "{}", text(&previewed));
    assert_eq!(imported.status.code(), Some(1), "{}", text(&imported));
    assert_eq!(
        refusal_sentence(&previewed),
        refusal_sentence(&imported),
        "the same sentence"
    );
    assert!(refusal_sentence(&previewed).contains("none is usable"));
}

/// The primary witness (ruling 1): the preview, which calls the same `decide`, leaves the whole `.prikk` tree byte-identical for the
/// clean path and for every refusal kind it lists. **Red control:** a write inserted into `decide` makes the first row red.
#[test]
fn a_preview_writes_nothing_for_any_row() {
    let (older, newer) = sender_with_two_bundles("bprime-nothing");
    let (first, second) = author_key_conflict_pair();

    let clean = receiver("bprime-nothing-clean");
    let before = tree_bytes(&clean);
    support::ok(&preview(&clean, &newer, &[]), "a clean preview");
    support::ok(
        &preview(&clean, &newer, &["--adopt"]),
        "a clean preview --adopt",
    );
    assert_eq!(tree_bytes(&clean), before, "the clean path");

    let rolled_back = receiver("bprime-nothing-rollback");
    support::ok(&import(&rolled_back, &newer, &[]), "import");
    let before = tree_bytes(&rolled_back);
    support::ok(&preview(&rolled_back, &older, &[]), "a rollback preview");
    assert_eq!(tree_bytes(&rolled_back), before, "a class refusal");

    let conflicted = receiver("bprime-nothing-conflict");
    support::ok(&import(&conflicted, &first, &[]), "import");
    let before = tree_bytes(&conflicted);
    support::ok(
        &preview(&conflicted, &second, &[]),
        "an author-key conflict preview",
    );
    assert_eq!(tree_bytes(&conflicted), before, "an author-key conflict");
}

/// A delivery with two refusals: the preview lists both, in the import's order, and **the import raises the first** -- the one it has
/// always raised. **Red control:** raise the last and this goes red.
#[test]
fn the_import_raises_the_first_refusal_the_preview_lists() {
    let (first, second) = author_key_conflict_pair_on("heads/main");
    let repo = receiver("bprime-two-refusals");
    support::ok(&import(&repo, &first, &[]), "the first delivery");
    let before = tree_bytes(&repo);

    let previewed = preview(&repo, &second, &[]);
    let stdout = String::from_utf8_lossy(&previewed.stdout).to_string();
    let found = listed(&stdout);
    let ids: Vec<&str> = found.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, ["author-key-conflict", "does-not-connect"], "{stdout}");
    let refused = import(&repo, &second, &[]);
    assert_eq!(
        refusal_sentence(&refused),
        found[0].1,
        "the first, not the last"
    );
    assert_ne!(refusal_sentence(&refused), found[1].1);
    assert_eq!(tree_bytes(&repo), before);
}
