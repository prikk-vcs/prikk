//! 0.57.0 step 0a: `sync seal` onto a branch the receiver does not hold.
//!
//! **The measured defect (0.56.0).** Carry `heads/main` (three sealed commits) into a fresh
//! repository, then carry `heads/feature` (a branch of `heads/main` plus one commit adding
//! `f.txt`) the same way *without* `branch create heads/feature --from heads/main` first.
//! `sync seal heads/feature --claims …` printed `heads/feature was already fully sealed -- no-op`
//! for the three claims `heads/main` had sealed (false: `heads/feature` did not exist), then
//! sealed `f.txt`'s patch onto the unborn `heads/feature` as a new root. Every exit code was 0 and
//! `verify` was clean, and the branch silently lacked its earlier files.
//!
//! **The rule these tests pin.** "Already sealed" is judged against the target ref's own history;
//! and a claim that builds on a block the target ref's tip does not reach is refused before
//! anything is written, naming a way out the user can type. The rows are the target ref's state
//! against the claims' parents:
//!
//! | row | target ref | claims | expected |
//! |---|---|---|---|
//! | A | unborn | the root claim and the chain after it | seals, as before |
//! | B1 | unborn | only the feature claim (parent not reached) | **refused**, `.prikk` unchanged |
//! | B2 | unborn | the whole chain, in a file | seals the whole chain (rule 1: main's blocks are not "already sealed" here) |
//! | C | existing, its tip reaches the parent | the feature claim | seals, as before |
//! | D | existing, on another line | the feature claim | **refused**, `.prikk` unchanged |
//! | E | existing, behind the parent | the feature claim | **refused**, `.prikk` unchanged |
//! | F | unborn, no local branch holds the parent | the feature claim | **refused**, naming the carry commands |
//! | G | unborn copy of a branch another ref sealed | main's claims | seals (not "already fully sealed"); a second run is a true no-op |
//!
//! **Without the change:** B1, D, E and F fail (they seal onto the wrong history); B2 and G fail
//! (they print the false no-op and lose the files).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use crate::support;
use std::path::{Path, PathBuf};
use std::process::Output;

use prikk_store::{ObjectReadSnapshot, RepositoryLayout, order_claims_for_sealing};

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn scratch(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("step0a-{tag}-{}.bin", support::unique_suffix()))
}

fn with_maintainer(mut command: std::process::Command) -> std::process::Command {
    command
        .env("PRIKK_MAINTAINER_KEY_ID", support::MAINTAINER_KEY_ID)
        .env(
            "PRIKK_MAINTAINER_SEED_FILE",
            support::seed_file(&support::hex(&support::MAINTAINER_SEED)),
        );
    command
}

fn switch(repo: &Path, target: &str) {
    support::ok(
        &support::prikk(repo)
            .args(["branch", "switch", target])
            .output()
            .unwrap(),
        &format!("switch to {target}"),
    );
}

/// The sending repository: `heads/main` with three sealed commits; `heads/feature` is a branch of
/// it plus one commit adding `f.txt`.
fn old_repository(tag: &str) -> PathBuf {
    let old = support::unique_repo(tag);
    support::init(&old);
    for i in 1..=3 {
        support::generation(
            &old,
            "heads/main",
            &format!("m{i}.txt"),
            format!("main {i}\n").as_bytes(),
            &format!("main {i}"),
        );
    }
    support::ok(
        &support::branch_create(&old, "heads/feature", "heads/main"),
        "branch create heads/feature",
    );
    switch(&old, "heads/feature");
    support::generation(&old, "heads/feature", "f.txt", b"feature\n", "feature f1");
    switch(&old, "heads/main");
    old
}

/// A fresh receiving repository, trusting the maintainer key (the same keys as `old`).
fn new_repository(tag: &str) -> PathBuf {
    let new = support::unique_repo(tag);
    support::init(&new);
    support::trust_maintainer(&new);
    new
}

/// Carry `ref_name` from `old` into `new` (`sync have` / `build` / `accept`); returns the claim ids
/// `sync accept` wrote, in the order it wrote them.
fn carry(old: &Path, new: &Path, ref_name: &str, tag: &str) -> Vec<String> {
    let have = scratch(&format!("{tag}-have"));
    let artifact = scratch(&format!("{tag}-artifact"));
    let claims = scratch(&format!("{tag}-claims"));
    support::ok(
        &support::prikk(new)
            .args(["sync", "have", ref_name, "--output", have.to_str().unwrap()])
            .output()
            .unwrap(),
        "sync have",
    );
    support::ok(
        &with_maintainer(support::prikk(old))
            .args([
                "sync",
                "build",
                ref_name,
                "--have",
                have.to_str().unwrap(),
                "--output",
                artifact.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync build",
    );
    support::ok(
        &support::prikk(new)
            .args([
                "sync",
                "accept",
                artifact.to_str().unwrap(),
                "--claims-out",
                claims.to_str().unwrap(),
            ])
            .output()
            .unwrap(),
        "sync accept",
    );
    std::fs::read_to_string(&claims)
        .unwrap()
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_string())
        .collect()
}

/// The claim ids, ordered by lineage (root first) as `sync seal` itself orders them.
fn lineage_order(new: &Path, ids: &[String]) -> Vec<String> {
    let layout = RepositoryLayout::open(new.to_path_buf()).unwrap();
    let store = ObjectReadSnapshot::open(&layout).unwrap();
    let parsed: Vec<prikk_object::ObjectId> = ids.iter().map(|id| id.parse().unwrap()).collect();
    order_claims_for_sealing(&store, &parsed)
        .unwrap()
        .iter()
        .map(ToString::to_string)
        .collect()
}

fn claims_file(tag: &str, ids: &[String]) -> PathBuf {
    let path = scratch(tag);
    std::fs::write(
        &path,
        ids.iter().map(|id| format!("{id}\n")).collect::<String>(),
    )
    .unwrap();
    path
}

fn seal(new: &Path, ref_name: &str, claims: &Path) -> Output {
    with_maintainer(support::prikk(new))
        .args([
            "sync",
            "seal",
            ref_name,
            "--claims",
            claims.to_str().unwrap(),
        ])
        .output()
        .unwrap()
}

fn seal_ok(new: &Path, ref_name: &str, ids: &[String], tag: &str) -> String {
    let output = seal(new, ref_name, &claims_file(tag, ids));
    support::ok(&output, &format!("sync seal {ref_name}"));
    text(&output)
}

/// `.prikk` as bytes, without the derived cache (a refusal may touch only that).
fn store_without_cache(repo: &Path) -> std::collections::BTreeMap<PathBuf, Vec<u8>> {
    support::store_bytes(repo)
        .into_iter()
        .filter(|(path, _)| !path.components().any(|part| part.as_os_str() == "cache"))
        .collect()
}

/// The files `tree --ref` lists, sorted.
fn tree_files(repo: &Path, ref_name: &str) -> Vec<String> {
    let output = support::prikk(repo)
        .args(["tree", "--ref", ref_name])
        .output()
        .unwrap();
    support::ok(&output, "tree");
    let mut files: Vec<String> = text(&output)
        .split_whitespace()
        .filter(|word| word.ends_with(".txt"))
        .map(str::to_string)
        .collect();
    files.sort();
    files
}

/// The block ids of `ref_name`'s history, sorted.
fn block_ids(repo: &Path, ref_name: &str) -> Vec<String> {
    let output = support::prikk(repo)
        .args(["log", "--ref", ref_name, "--limit", "100"])
        .output()
        .unwrap();
    support::ok(&output, "log");
    let mut ids: Vec<String> = text(&output)
        .lines()
        .filter_map(|line| line.strip_prefix("block "))
        .map(|rest| rest.split_whitespace().next().unwrap().to_string())
        .collect();
    ids.sort();
    ids
}

fn assert_verify_clean(repo: &Path) {
    support::ok(
        &support::prikk(repo).arg("verify").output().unwrap(),
        "verify",
    );
}

/// The refusal every B1/D/E/F row shares: exit 1, a precondition, the claim and the unreached
/// parent named, and nothing written.
fn assert_refused_and_unchanged(
    new: &Path,
    ref_name: &str,
    claims: &Path,
    before: &std::collections::BTreeMap<PathBuf, Vec<u8>>,
) -> String {
    let output = seal(new, ref_name, claims);
    let message = text(&output);
    assert_eq!(output.status.code(), Some(1), "{message}");
    assert!(
        message.contains("error: precondition not met: sync seal"),
        "{message}"
    );
    assert!(message.contains("builds on block"), "{message}");
    assert!(
        message.contains("nothing was written"),
        "the refusal says nothing was written: {message}"
    );
    assert!(
        !message.contains("already fully sealed"),
        "never the false no-op: {message}"
    );
    assert_eq!(
        &store_without_cache(new),
        before,
        "a refused `sync seal` leaves .prikk byte-identical"
    );
    assert_verify_clean(new);
    message
}

#[test]
fn row_a_an_unborn_target_takes_the_root_claim_and_the_chain_after_it() {
    let old = old_repository("s0a-a-old");
    let new = new_repository("s0a-a-new");
    let ids = carry(&old, &new, "heads/main", "a");
    let out = seal_ok(&new, "heads/main", &ids, "a-seal");
    assert_eq!(out.matches("sealed 1 patch(es)").count(), 3, "{out}");
    assert!(!out.contains("already fully sealed"), "{out}");
    assert_eq!(
        tree_files(&new, "heads/main"),
        ["m1.txt", "m2.txt", "m3.txt"]
    );
    assert_eq!(
        block_ids(&new, "heads/main"),
        block_ids(&old, "heads/main"),
        "the same block ids as the sender"
    );
    assert_verify_clean(&new);
}

#[test]
fn row_b1_an_unborn_target_refuses_a_claim_whose_parent_it_does_not_reach() {
    let old = old_repository("s0a-b1-old");
    let new = new_repository("s0a-b1-new");
    let main_ids = carry(&old, &new, "heads/main", "b1m");
    seal_ok(&new, "heads/main", &main_ids, "b1m-seal");
    let feature_ids = carry(&old, &new, "heads/feature", "b1f");
    let feature_claim = lineage_order(&new, &feature_ids).pop().unwrap();
    let file = claims_file("b1-one", &[feature_claim]);
    let before = store_without_cache(&new);
    let message = assert_refused_and_unchanged(&new, "heads/feature", &file, &before);
    assert!(
        message.contains("heads/feature has no history yet"),
        "{message}"
    );
    assert!(
        message.contains("run `prikk branch create heads/feature --from heads/main`"),
        "the way out is typeable: {message}"
    );
    // The documented way out works, and gives the sender's tree and block ids.
    support::ok(
        &support::branch_create(&new, "heads/feature", "heads/main"),
        "branch create heads/feature --from heads/main",
    );
    let out = seal_ok(
        &new,
        "heads/feature",
        &lineage_order(&new, &feature_ids),
        "b1-after",
    );
    assert_eq!(out.matches("sealed 1 patch(es)").count(), 1, "{out}");
    assert_eq!(
        tree_files(&new, "heads/feature"),
        tree_files(&old, "heads/feature")
    );
    assert_eq!(
        block_ids(&new, "heads/feature"),
        block_ids(&old, "heads/feature")
    );
}

#[test]
fn row_b2_the_whole_chain_in_one_file_seals_the_whole_branch() {
    let old = old_repository("s0a-b2-old");
    let new = new_repository("s0a-b2-new");
    let main_ids = carry(&old, &new, "heads/main", "b2m");
    seal_ok(&new, "heads/main", &main_ids, "b2m-seal");
    // heads/main's claims are held and sealed *on heads/main*; heads/feature does not exist.
    let feature_ids = carry(&old, &new, "heads/feature", "b2f");
    let out = seal_ok(&new, "heads/feature", &feature_ids, "b2-seal");
    assert!(
        !out.contains("already fully sealed"),
        "main's blocks are not heads/feature's: {out}"
    );
    assert_eq!(out.matches("sealed 1 patch(es)").count(), 4, "{out}");
    assert_eq!(
        tree_files(&new, "heads/feature"),
        ["f.txt", "m1.txt", "m2.txt", "m3.txt"],
        "the branch has its earlier files"
    );
    // `log --ref` lists the ref's updates, and this ref was built claim by claim (four updates)
    // where the sender's was branched from main (two): every block id the sender's ref log names is
    // among them, so the blocks are the sender's.
    let (sent, got) = (
        block_ids(&old, "heads/feature"),
        block_ids(&new, "heads/feature"),
    );
    assert_eq!(got.len(), 4, "{got:?}");
    assert!(
        sent.iter().all(|id| got.contains(id)),
        "{sent:?} not in {got:?}"
    );
    assert_verify_clean(&new);
}

#[test]
fn row_c_an_existing_target_whose_tip_reaches_the_parent_seals_as_before() {
    let old = old_repository("s0a-c-old");
    let new = new_repository("s0a-c-new");
    let main_ids = carry(&old, &new, "heads/main", "cm");
    seal_ok(&new, "heads/main", &main_ids, "cm-seal");
    support::ok(
        &support::branch_create(&new, "heads/feature", "heads/main"),
        "branch create",
    );
    let feature_ids = carry(&old, &new, "heads/feature", "cf");
    let out = seal_ok(
        &new,
        "heads/feature",
        &lineage_order(&new, &feature_ids),
        "c-seal",
    );
    assert_eq!(out.matches("sealed 1 patch(es)").count(), 1, "{out}");
    assert_eq!(
        tree_files(&new, "heads/feature"),
        ["f.txt", "m1.txt", "m2.txt", "m3.txt"]
    );
    assert_verify_clean(&new);
}

/// Carry `heads/main` into `new` in two steps: its root claim first, then the rest, so a branch
/// can be made at the one-block state; returns (root claim, the rest in lineage order).
fn carry_main_root_first(old: &Path, new: &Path, tag: &str) -> (String, Vec<String>) {
    let ids = carry(old, new, "heads/main", tag);
    let mut ordered = lineage_order(new, &ids);
    let root = ordered.remove(0);
    seal_ok(
        new,
        "heads/main",
        std::slice::from_ref(&root),
        &format!("{tag}-root"),
    );
    (root, ordered)
}

#[test]
fn row_d_an_existing_target_on_another_line_is_refused() {
    let old = old_repository("s0a-d-old");
    let new = new_repository("s0a-d-new");
    let (_root, rest) = carry_main_root_first(&old, &new, "dm");
    support::ok(
        &support::branch_create(&new, "heads/other", "heads/main"),
        "branch create heads/other",
    );
    switch(&new, "heads/other");
    support::generation(&new, "heads/other", "y.txt", b"own\n", "own y");
    switch(&new, "heads/main");
    seal_ok(&new, "heads/main", &rest, "dm-rest");
    let feature_ids = carry(&old, &new, "heads/feature", "df");
    let feature_claim = lineage_order(&new, &feature_ids).pop().unwrap();
    let file = claims_file("d-one", &[feature_claim]);
    let before = store_without_cache(&new);
    let message = assert_refused_and_unchanged(&new, "heads/other", &file, &before);
    assert!(
        message.contains("does not contain that history"),
        "{message}"
    );
    assert!(
        message.contains("prikk branch create <new-branch> --from heads/main"),
        "{message}"
    );
    assert_eq!(tree_files(&new, "heads/other"), ["m1.txt", "y.txt"]);
}

#[test]
fn row_e_an_existing_target_behind_the_parent_is_refused() {
    let old = old_repository("s0a-e-old");
    let new = new_repository("s0a-e-new");
    let (_root, rest) = carry_main_root_first(&old, &new, "em");
    support::ok(
        &support::branch_create(&new, "heads/feature", "heads/main"),
        "branch create heads/feature",
    );
    seal_ok(&new, "heads/main", &rest, "em-rest");
    let feature_ids = carry(&old, &new, "heads/feature", "ef");
    let feature_claim = lineage_order(&new, &feature_ids).pop().unwrap();
    let file = claims_file("e-one", &[feature_claim]);
    let before = store_without_cache(&new);
    let message = assert_refused_and_unchanged(&new, "heads/feature", &file, &before);
    assert!(
        message.contains("does not contain that history"),
        "{message}"
    );
    assert_eq!(tree_files(&new, "heads/feature"), ["m1.txt"]);
}

#[test]
fn row_f_no_local_branch_holds_the_parent_so_the_refusal_names_the_carry_commands() {
    let old = old_repository("s0a-f-old");
    let new = new_repository("s0a-f-new");
    // heads/feature's four claims are accepted, but nothing is sealed anywhere.
    let feature_ids = carry(&old, &new, "heads/feature", "ff");
    let feature_claim = lineage_order(&new, &feature_ids).pop().unwrap();
    let file = claims_file("f-one", &[feature_claim]);
    let before = store_without_cache(&new);
    let message = assert_refused_and_unchanged(&new, "heads/feature", &file, &before);
    assert!(
        message.contains("is not held on any local branch here"),
        "{message}"
    );
    for command in [
        "prikk sync have <that-branch> --output have.bin",
        "prikk sync build <that-branch> --have have.bin --output art.bin",
        "prikk sync accept art.bin --claims-out claims.txt",
        "prikk sync seal <that-branch> --claims claims.txt",
        "prikk branch create heads/feature --from <that-branch>",
    ] {
        assert!(message.contains(command), "missing `{command}`: {message}");
    }
}

#[test]
fn row_g_a_patch_another_ref_sealed_is_work_for_this_ref_and_a_second_run_is_a_true_no_op() {
    let old = old_repository("s0a-g-old");
    let new = new_repository("s0a-g-new");
    let main_ids = carry(&old, &new, "heads/main", "gm");
    seal_ok(&new, "heads/main", &main_ids, "gm-seal");
    // heads/copy does not exist; main's three blocks are sealed on heads/main only.
    let first = seal_ok(&new, "heads/copy", &main_ids, "g-first");
    assert!(!first.contains("already fully sealed"), "{first}");
    assert_eq!(first.matches("sealed 1 patch(es)").count(), 3, "{first}");
    assert_eq!(
        tree_files(&new, "heads/copy"),
        ["m1.txt", "m2.txt", "m3.txt"]
    );
    // Now it is true: heads/copy's own history holds them all.
    let before = store_without_cache(&new);
    let second = seal_ok(&new, "heads/copy", &main_ids, "g-second");
    assert_eq!(
        second.matches("was already fully sealed -- no-op").count(),
        3,
        "{second}"
    );
    assert!(!second.contains("sealed 1 patch(es)"), "{second}");
    assert_eq!(store_without_cache(&new), before);
    assert_verify_clean(&new);
}
