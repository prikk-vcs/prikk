//! DC-75 two-edits handoff §3 and §6: a side that edits one file more than once after the baseline
//! merges, through the compiled binary. Each row proves confluent, merges, leaves `verify` clean, and
//! materializes the tree both sides' edits give. The real overlaps still refuse, with a designed
//! witness.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use std::path::{Path, PathBuf};
use std::process::Output;

use support::*;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

fn tip(repo: &Path, ref_name: &str) -> ObjectId {
    let layout = RepositoryLayout::open(repo.to_path_buf()).unwrap();
    let object_store = FileObjectStore::new(layout.clone());
    let ref_state_id = RefStore::new(layout)
        .read_current_ref_state_id(ref_name)
        .unwrap()
        .unwrap();
    let envelope = object_store
        .read_typed(ref_state_id, ObjectType::RefState)
        .unwrap()
        .unwrap();
    RefStatePayload::decode_canonical(&envelope.canonical_payload, envelope.schema_version)
        .unwrap()
        .target_object_id
}

fn switch(repo: &Path, target: &str) {
    ok(
        &prikk(repo)
            .args(["branch", "switch", target])
            .output()
            .unwrap(),
        &format!("branch switch {target}"),
    );
}

fn evidence(repo: &Path, baseline: ObjectId) -> Output {
    prikk(repo)
        .args([
            "merge-evidence",
            "--baseline-block",
            &baseline.to_string(),
            "--left-ref",
            "heads/main",
            "--right-ref",
            "heads/side",
        ])
        .output()
        .unwrap()
}

fn merge(repo: &Path, baseline: ObjectId) -> Output {
    prikk(repo)
        .env("PRIKK_MAINTAINER_KEY_ID", MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            seed_file(&hex(&MAINTAINER_SEED)),
        )
        .args([
            "merge",
            "--allow-no-audit",
            "--baseline-block",
            &baseline.to_string(),
            "--into",
            "heads/main",
            "--from",
            "heads/side",
        ])
        .output()
        .unwrap()
}

fn reason(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find_map(|line| line.strip_prefix("reason: "))
        .unwrap_or("<no reason line>")
        .to_string()
}

/// Content of every file a repository's sealed history materializes, sorted by path.
fn materialized(repo: &Path, tag: &str) -> Vec<(String, Vec<u8>)> {
    let root = rebuild_from_sealed_history(repo, tag);
    let mut files: Vec<(String, Vec<u8>)> = std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&path).unwrap(),
            )
        })
        .collect();
    files.sort();
    files
}

/// One side's work after the branch point, as file writes and deletes, each its own sealed block.
enum Step {
    Write(&'static str, String),
    Delete(&'static str),
}

fn edits(path: &'static str, count: usize) -> Vec<Step> {
    (1..=count)
        .map(|k| Step::Write(path, format!("{path} {k}\n")))
        .collect()
}

fn apply(repo: &Path, ref_name: &str, steps: &[Step]) {
    for (number, step) in steps.iter().enumerate() {
        match step {
            Step::Write(path, content) => std::fs::write(repo.join(path), content).unwrap(),
            Step::Delete(path) => std::fs::remove_file(repo.join(path)).unwrap(),
        }
        ok(
            &commit(repo, ref_name, &format!("{ref_name} step {number}")),
            "commit",
        );
        ok(&seal(repo, ref_name), "seal");
    }
}

/// The history the handoff measured: `e.txt` and `f.txt` created and sealed (after `before_branch`
/// further edits of `e.txt`), `heads/side` branched there, then each side's steps.
fn history(tag: &str, before_branch: usize, main: &[Step], side: &[Step]) -> (PathBuf, ObjectId) {
    let repo = unique_repo(tag);
    init(&repo);
    std::fs::write(repo.join("e.txt"), b"e.txt 0\n").unwrap();
    std::fs::write(repo.join("f.txt"), b"f.txt 0\n").unwrap();
    ok(&commit(&repo, "heads/main", "baseline"), "commit baseline");
    ok(&seal(&repo, "heads/main"), "seal baseline");
    for k in 1..=before_branch {
        generation(
            &repo,
            "heads/main",
            "e.txt",
            format!("e.txt before {k}\n").as_bytes(),
            &format!("before branch {k}"),
        );
    }
    let baseline = tip(&repo, "heads/main");
    ok(
        &branch_create(&repo, "heads/side", "heads/main"),
        "branch create",
    );
    apply(&repo, "heads/main", main);
    switch(&repo, "heads/side");
    apply(&repo, "heads/side", side);
    switch(&repo, "heads/main");
    (repo, baseline)
}

/// The tree replaying both sides gives: the baseline files, then each side's steps.
fn expected(before_branch: usize, main: &[Step], side: &[Step]) -> Vec<(String, Vec<u8>)> {
    let mut files = std::collections::BTreeMap::new();
    let e = if before_branch == 0 {
        "e.txt 0\n".to_string()
    } else {
        format!("e.txt before {before_branch}\n")
    };
    files.insert("e.txt".to_string(), e.into_bytes());
    files.insert("f.txt".to_string(), b"f.txt 0\n".to_vec());
    for step in main.iter().chain(side) {
        match step {
            Step::Write(path, content) => {
                files.insert((*path).to_string(), content.clone().into_bytes());
            }
            Step::Delete(path) => {
                files.remove(*path);
            }
        }
    }
    files.into_iter().collect()
}

fn assert_merges(tag: &str, before_branch: usize, main: Vec<Step>, side: Vec<Step>) {
    let (repo, baseline) = history(tag, before_branch, &main, &side);
    let report = evidence(&repo, baseline);
    ok(&report, "merge-evidence");
    assert_eq!(
        reason(&report),
        "proven_confluent",
        "{tag}: {}",
        String::from_utf8_lossy(&report.stdout)
    );
    ok(&merge(&repo, baseline), &format!("{tag}: merge"));
    ok(&verify(&repo), &format!("{tag}: verify after merge"));
    assert_eq!(
        materialized(&repo, tag),
        expected(before_branch, &main, &side),
        "{tag}: the merged tree is both sides' edits"
    );
    let _ = std::fs::remove_dir_all(&repo);
}

fn a_new_file(path: &'static str) -> Vec<Step> {
    vec![Step::Write(path, format!("{path} new\n"))]
}

#[test]
fn row_1_0_main_edits_once() {
    assert_merges(
        "dc75-two-edits-1-0",
        0,
        edits("e.txt", 1),
        a_new_file("s.txt"),
    );
}

#[test]
fn row_1_1_each_side_edits_its_file_once() {
    assert_merges(
        "dc75-two-edits-1-1",
        0,
        edits("e.txt", 1),
        edits("f.txt", 1),
    );
}

#[test]
fn row_2_0_main_edits_one_file_twice() {
    assert_merges(
        "dc75-two-edits-2-0",
        0,
        edits("e.txt", 2),
        a_new_file("s.txt"),
    );
}

#[test]
fn row_0_2_the_adopted_side_edits_one_file_twice() {
    assert_merges(
        "dc75-two-edits-0-2",
        0,
        a_new_file("m.txt"),
        edits("f.txt", 2),
    );
}

#[test]
fn row_2_2_each_side_edits_its_file_twice() {
    assert_merges(
        "dc75-two-edits-2-2",
        0,
        edits("e.txt", 2),
        edits("f.txt", 2),
    );
}

#[test]
fn twenty_edits_of_one_file_on_one_side() {
    assert_merges(
        "dc75-two-edits-20",
        0,
        edits("e.txt", 20),
        a_new_file("s.txt"),
    );
}

#[test]
fn a_delete_of_an_edited_file() {
    let mut main = edits("e.txt", 2);
    main.push(Step::Delete("e.txt"));
    assert_merges("dc75-two-edits-delete", 0, main, edits("f.txt", 2));
}

/// Post-RFC 136 1b: 70 blocks before the branch, so the history crosses the checkpoint at block 65.
#[test]
fn a_history_crossing_a_checkpoint_with_edits_on_both_sides() {
    assert_merges(
        "dc75-two-edits-70",
        69,
        edits("e.txt", 3),
        edits("f.txt", 3),
    );
}

/// Main edits `e.txt` twice, the side once. Returns the evidence report and the refused merge.
fn overlap(tag: &str, main_texts: [&str; 2], side_text: &str) -> (Output, Output) {
    let main = main_texts
        .iter()
        .map(|text| Step::Write("e.txt", (*text).to_string()))
        .collect::<Vec<_>>();
    let side = vec![Step::Write("e.txt", side_text.to_string())];
    let repo = unique_repo(tag);
    init(&repo);
    std::fs::write(repo.join("e.txt"), b"one\ntwo\nthree\n").unwrap();
    ok(&commit(&repo, "heads/main", "baseline"), "commit baseline");
    ok(&seal(&repo, "heads/main"), "seal baseline");
    let baseline = tip(&repo, "heads/main");
    ok(
        &branch_create(&repo, "heads/side", "heads/main"),
        "branch create",
    );
    apply(&repo, "heads/main", &main);
    switch(&repo, "heads/side");
    apply(&repo, "heads/side", &side);
    switch(&repo, "heads/main");
    let before = tip(&repo, "heads/main");
    let report = evidence(&repo, baseline);
    let again = evidence(&repo, baseline);
    assert_eq!(
        report.stdout, again.stdout,
        "{tag}: merge-evidence is byte-identical across runs"
    );
    let refused = merge(&repo, baseline);
    assert_eq!(refused.status.code(), Some(1), "{tag}: merge must refuse");
    assert_eq!(
        tip(&repo, "heads/main"),
        before,
        "{tag}: refusal writes nothing"
    );
    let _ = std::fs::remove_dir_all(&repo);
    (report, refused)
}

#[test]
fn an_overlap_on_the_same_span_refuses_as_pair_conflict() {
    let (report, refused) = overlap(
        "dc75-two-edits-overlap",
        ["one\nTWO\nthree\n", "one\n2\nthree\n"],
        "one\nzwei\nthree\n",
    );
    let stdout = String::from_utf8_lossy(&report.stdout);
    assert_eq!(reason(&report), "pair_conflict", "{stdout}");
    assert!(
        stdout.contains("left[0] op_seq=1 EditText folded-through=left[1] op_seq=1"),
        "the folded run is named by its original operations: {stdout}"
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("precondition not met: merge refused:")
            && stderr.contains("reason: pair_conflict"),
        "{stderr}"
    );
}

#[test]
fn an_overlap_on_distinct_spans_refuses_as_same_node_text_transform_deferred() {
    let (report, refused) = overlap(
        "dc75-two-edits-distinct",
        ["ONE\ntwo\nthree\n", "1\ntwo\nthree\n"],
        "one\ntwo\ndrei\n",
    );
    assert_eq!(
        reason(&report),
        "same_node_text_transform_deferred",
        "{}",
        String::from_utf8_lossy(&report.stdout)
    );
    let stderr = String::from_utf8_lossy(&refused.stderr);
    assert!(
        stderr.contains("reason: same_node_text_transform_deferred"),
        "{stderr}"
    );
}
