//! 0.60.0 S1: `show <block>` derives the state a block's patches fold onto from the nearest authenticated checkpoint (`seal`'s mechanism, RFC 159),
//! and **the output is byte-identical to the one derived from the root.**
//!
//! A history of 150 blocks (so two checkpoints, at blocks 1 and 65, sit behind the later blocks): text edits at every step, new files, deletions, a
//! `Merge` block, and a rollback block. Every block is shown, text and JSON, **with the replay-verified record in place** (the anchored derivation
//! is admitted) and **with it removed** (`AnchorTrust::has_record()` is false, so every block takes today's full walk): the two outputs are
//! compared byte for byte. The control that this test reaches the anchored path is its red control: a derivation that is off by one block makes it fail.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::path::Path;

use crate::support;

fn log_blocks(repo: &Path) -> Vec<String> {
    let out = support::prikk(repo)
        .args(["log", "--limit", "100000", "--format", "json"])
        .output()
        .unwrap();
    support::ok(&out, "log");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let mut ids = Vec::new();
    let mut rest = text.as_str();
    while let Some(at) = rest.find("\"block_id\": \"") {
        let start = at + "\"block_id\": \"".len();
        ids.push(rest[start..start + 64].to_string());
        rest = &rest[start + 64..];
    }
    ids
}

fn show_all(repo: &Path, ids: &[String]) -> Vec<(String, Vec<u8>, Vec<u8>)> {
    ids.iter()
        .map(|id| {
            let text = support::prikk(repo).args(["show", id]).output().unwrap();
            support::ok(&text, &format!("show {id}"));
            let json = support::prikk(repo)
                .args(["show", id, "--format", "json"])
                .output()
                .unwrap();
            support::ok(&json, &format!("show {id} --format json"));
            (id.clone(), text.stdout, json.stdout)
        })
        .collect()
}

fn edit_generation(repo: &Path, k: usize) {
    // A text edit at every step (an `edit-text` operation addressed by node), a new file every 7th, a deletion every 11th.
    let lines: String = (0..30)
        .map(|n| {
            if n == k % 30 {
                format!("line {n} edit {k}\n")
            } else {
                format!("line {n}\n")
            }
        })
        .collect();
    std::fs::write(repo.join("page.txt"), lines).unwrap();
    if k % 7 == 0 {
        std::fs::write(repo.join(format!("new{k}.txt")), format!("new {k}\n")).unwrap();
    }
    if k % 11 == 0 {
        let victim = repo.join(format!("new{}.txt", k - 4));
        if victim.exists() {
            std::fs::remove_file(victim).unwrap();
        }
    }
    support::ok(
        &support::commit(repo, "heads/main", &format!("g{k}")),
        "commit",
    );
    support::ok(&support::seal(repo, "heads/main"), "seal");
}

#[test]
fn show_from_a_checkpoint_is_byte_identical_to_show_from_the_root() {
    let repo = support::unique_repo("s1-show-anchored-equality");
    support::init(&repo);
    for k in 1..=70 {
        edit_generation(&repo, k);
    }
    // A Merge block: `side` edits and deletes a file the baseline holds; `main` then merges it (the DC-75 shape `rfc142_show_merge_block_per_patch`
    // builds), at block 71, so the merge block and the blocks after it sit after the checkpoint at block 65.
    std::fs::write(repo.join("d.txt"), b"d 0\n").unwrap();
    support::ok(&support::commit(&repo, "heads/main", "d"), "commit d");
    support::ok(&support::seal(&repo, "heads/main"), "seal d");
    let baseline = log_blocks(&repo).first().cloned().unwrap();
    support::ok(
        &support::branch_create(&repo, "heads/side", "heads/main"),
        "branch create",
    );
    let switch = |target: &str| {
        support::ok(
            &support::prikk(&repo)
                .args(["branch", "switch", target])
                .output()
                .unwrap(),
            "switch",
        );
    };
    switch("heads/side");
    std::fs::write(repo.join("d.txt"), b"d edited\n").unwrap();
    support::ok(
        &support::commit(&repo, "heads/side", "edit d"),
        "commit edit d",
    );
    support::ok(&support::seal(&repo, "heads/side"), "seal edit d");
    std::fs::remove_file(repo.join("d.txt")).unwrap();
    support::ok(
        &support::commit(&repo, "heads/side", "delete d"),
        "commit delete d",
    );
    support::ok(&support::seal(&repo, "heads/side"), "seal delete d");
    switch("heads/main");
    let merged = support::prikk(&repo)
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline,
            "--into",
            "heads/main",
            "--from",
            "heads/side",
        ])
        .output()
        .unwrap();
    support::ok(&merged, "merge");
    for k in 72..=100 {
        edit_generation(&repo, k);
    }
    // A rollback block: the inverse of the tip's patch, drafted and sealed.
    let draft = support::prikk(&repo)
        .env("PRIKK_AUTHOR_KEY_ID", support::AUTHOR_KEY_ID)
        .env(
            "PRIKK_AUTHOR_SEED_FILE",
            support::seed_file(support::AUTHOR_SEED_HEX),
        )
        .args(["rollback-draft", "--append-inverse", "-m", "undo"])
        .output()
        .unwrap();
    support::ok(&draft, "rollback-draft");
    support::ok(&support::seal(&repo, "heads/main"), "seal the rollback");
    // The inverse reaches the worktree by a switch to the current branch (the draft's own note says so).
    switch("heads/main");
    for k in 101..=150 {
        edit_generation(&repo, k);
    }
    support::ok(&support::verify(&repo), "verify");

    let ids = log_blocks(&repo);
    assert!(ids.len() >= 150, "{} blocks", ids.len());
    let record = repo.join(".prikk/cache/replay-verified-blocks.v1");
    assert!(
        record.exists(),
        "the replay-verified record is there: the anchored derivation can be admitted"
    );

    let anchored = show_all(&repo, &ids);
    let text_all: String = anchored
        .iter()
        .map(|(_, text, _)| String::from_utf8_lossy(text).into_owned())
        .collect();
    for kind in ["edit-text", "create-file", "delete-node"] {
        assert!(text_all.contains(kind), "the history holds a {kind}");
    }
    // The history holds the two shapes the review asked for: a Merge block and a rollback block (log says which).
    let log = support::prikk(&repo)
        .args(["log", "--limit", "100000", "--format", "json"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&log.stdout).into_owned();
    assert!(log.contains("\"kind\": \"Merge\""), "a Merge block");
    assert!(log.contains("\"rollback_block\": true"), "a rollback block");

    // The same blocks with no record: every `show` takes the full walk from the root.
    std::fs::remove_file(&record).unwrap();
    let from_the_root = show_all(&repo, &ids);
    for ((id, text_a, json_a), (_, text_r, json_r)) in anchored.iter().zip(&from_the_root) {
        assert_eq!(text_a, text_r, "show {id}: text differs");
        assert_eq!(json_a, json_r, "show {id}: JSON differs");
    }
    let _ = std::fs::remove_dir_all(&repo);
}
