//! RFC 156 Stage 4, through the compiled binary: `show` names every AUTHOR signer of a rename.
//!
//! A declared rename is committed and sealed by the harness author, `dc67-test-author`; a superseding
//! record then adds a second AUTHOR signature by `zzz-second-author` — the state a format-7 merge leaves.
//! `author_key_id` keeps its meaning (the first signer in canonical order) and `author_key_ids` names both.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

mod support;

use prikk_object::ObjectId;
use prikk_store::{
    Ed25519AuthorSigner, FileObjectStore, ObjectReader, RepositoryLayout,
    append_superseding_record_for_test_support, author_signature,
};

const SECOND_AUTHOR_KEY_ID: &str = "zzz-second-author";

fn text(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).to_string()
}

#[test]
fn show_names_every_author_signer_of_a_rename() {
    let repo = support::unique_repo("rfc156-stage4-show");
    support::init(&repo);
    std::fs::write(repo.join("a.txt"), b"hello\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "add a"), "commit a");
    support::ok(&support::seal(&repo, "heads/main"), "seal a");
    support::ok(
        &support::prikk(&repo)
            .args(["mv", "a.txt", "b.txt"])
            .output()
            .unwrap(),
        "mv",
    );
    let committed = support::commit(&repo, "heads/main", "rename");
    support::ok(&committed, "commit the rename");
    let patch_id: ObjectId = text(&committed)
        .lines()
        .find_map(|line| line.strip_prefix("patch id: "))
        .expect("commit prints a patch id")
        .trim()
        .parse()
        .unwrap();
    support::ok(&support::seal(&repo, "heads/main"), "seal the rename");

    let shown = |args: &[&str]| {
        let output = support::prikk(&repo).args(args).output().unwrap();
        support::ok(&output, "show");
        text(&output)
    };
    let id = patch_id.to_string();
    let single = shown(&["show", &id]);
    assert!(
        single.contains(&format!(
            "asserted by (AUTHOR key id): {}",
            support::AUTHOR_KEY_ID
        )),
        "one signer reports as it always did: {single}"
    );

    let layout = RepositoryLayout::open(repo.clone()).unwrap();
    let mut envelope = FileObjectStore::new(layout.clone())
        .read_object(patch_id)
        .unwrap()
        .expect("the patch is stored");
    let second = Ed25519AuthorSigner::from_seed(SECOND_AUTHOR_KEY_ID, &[0x5e; 32]).unwrap();
    envelope
        .add_signature(author_signature(&second, patch_id).unwrap())
        .unwrap();
    append_superseding_record_for_test_support(&layout, &envelope).unwrap();

    let prose = shown(&["show", &id]);
    assert!(
        prose.contains(&format!(
            "asserted by (AUTHOR key ids): {}, {SECOND_AUTHOR_KEY_ID}",
            support::AUTHOR_KEY_ID
        )),
        "{prose}"
    );
    let json = shown(&["show", &id, "--format", "json"]);
    assert!(
        json.contains(&format!(
            "\"author_key_id\": \"{}\", \"author_key_ids\": [\"{}\", \"{SECOND_AUTHOR_KEY_ID}\"]",
            support::AUTHOR_KEY_ID,
            support::AUTHOR_KEY_ID
        )),
        "{json}"
    );

    let _ = std::fs::remove_dir_all(&repo);
}
