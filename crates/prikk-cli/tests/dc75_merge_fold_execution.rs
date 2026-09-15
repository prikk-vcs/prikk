//! DC-75 two-edits handoff §7.2: a fold is judged by the evidence, but the merge replays each side's
//! *original* patches onto the other side's tip. So every claim of confluence a fold makes must be one
//! the merge can realize, in both directions.
//!
//! - §7.1's hole: a side that edits a word and restores it, beside another side that edits the same
//!   file, must never be proven confluent.
//! - Execution soundness: for every fold kind, with the other side (a) touching nothing the fold touches
//!   and (b) editing a different file, `merge-evidence` says `proven_confluent` and `merge` succeeds in
//!   both directions, with `verify` clean and the expected tree.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Output;

use support::*;

use prikk_object::{ObjectId, ObjectType, RefStatePayload};
use prikk_store::{FileObjectStore, ObjectReader, RefStore, RepositoryLayout};

const MAIN: &str = "heads/main";
const SIDE: &str = "heads/side";

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

fn evidence(repo: &Path, baseline: ObjectId, left: &str, right: &str) -> Output {
    prikk(repo)
        .args([
            "merge-evidence",
            "--baseline-block",
            &baseline.to_string(),
            "--left-ref",
            left,
            "--right-ref",
            right,
        ])
        .output()
        .unwrap()
}

fn merge(repo: &Path, baseline: ObjectId, into: &str, from: &str) -> Output {
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
            into,
            "--from",
            from,
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

/// One sealed block of work on a side.
enum Step {
    Write(&'static str, Vec<u8>),
    Delete(&'static str),
    /// Set or clear the executable bit.
    #[cfg(unix)]
    Executable(&'static str, bool),
}

fn write(path: &'static str, content: &str) -> Step {
    Step::Write(path, content.as_bytes().to_vec())
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if executable { 0o755 } else { 0o644 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).unwrap().permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    false
}

fn apply(repo: &Path, ref_name: &str, steps: &[Step]) {
    for (number, step) in steps.iter().enumerate() {
        match step {
            Step::Write(path, content) => std::fs::write(repo.join(path), content).unwrap(),
            Step::Delete(path) => std::fs::remove_file(repo.join(path)).unwrap(),
            #[cfg(unix)]
            Step::Executable(path, executable) => set_executable(&repo.join(path), *executable),
        }
        ok(
            &commit(repo, ref_name, &format!("{ref_name} step {number}")),
            "commit",
        );
        ok(&seal(repo, ref_name), "seal");
    }
}

type Tree = BTreeMap<String, (Vec<u8>, bool)>;

/// Build the history, and return it with a byte-for-byte copy so each merge direction runs on its own
/// repository.
fn history(
    tag: &str,
    baseline: &Tree,
    main: &[Step],
    side: &[Step],
) -> (PathBuf, PathBuf, ObjectId) {
    let repo = unique_repo(tag);
    init(&repo);
    for (path, (content, executable)) in baseline {
        std::fs::write(repo.join(path), content).unwrap();
        #[cfg(unix)]
        set_executable(&repo.join(path), *executable);
        #[cfg(not(unix))]
        let _ = executable;
    }
    ok(&commit(&repo, MAIN, "baseline"), "commit baseline");
    ok(&seal(&repo, MAIN), "seal baseline");
    let baseline_block = tip(&repo, MAIN);
    ok(&branch_create(&repo, SIDE, MAIN), "branch create");
    apply(&repo, MAIN, main);
    switch(&repo, SIDE);
    apply(&repo, SIDE, side);
    switch(&repo, MAIN);
    let twin = unique_repo(&format!("{tag}-twin"));
    std::fs::create_dir_all(&twin).unwrap();
    copy_dir_recursive(&repo, &twin);
    (repo, twin, baseline_block)
}

fn expected(baseline: &Tree, main: &[Step], side: &[Step]) -> Tree {
    let mut tree = baseline.clone();
    for step in main.iter().chain(side) {
        match step {
            Step::Write(path, content) => {
                let executable = tree.get(*path).is_some_and(|(_, executable)| *executable);
                tree.insert((*path).to_string(), (content.clone(), executable));
            }
            Step::Delete(path) => {
                tree.remove(*path);
            }
            #[cfg(unix)]
            Step::Executable(path, executable) => {
                if let Some(entry) = tree.get_mut(*path) {
                    entry.1 = *executable;
                }
            }
        }
    }
    tree
}

/// Every file `checkout --patch-materialize` rebuilds from the current branch's sealed history.
fn materialized(repo: &Path, tag: &str) -> Tree {
    let root = rebuild_from_sealed_history(repo, tag);
    std::fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file())
        .map(|path| {
            (
                path.file_name().unwrap().to_string_lossy().into_owned(),
                (std::fs::read(&path).unwrap(), is_executable(&path)),
            )
        })
        .collect()
}

/// The two directions: side into main, then (on the twin) main into side.
fn directions<'a>(repo: &'a Path, twin: &'a Path) -> [(&'a Path, &'static str, &'static str); 2] {
    [(repo, MAIN, SIDE), (twin, SIDE, MAIN)]
}

fn assert_merges_both_ways(tag: &str, baseline: &Tree, main: &[Step], side: &[Step]) {
    let (repo, twin, baseline_block) = history(tag, baseline, main, side);
    let want = expected(baseline, main, side);
    for (repository, into, from) in directions(&repo, &twin) {
        let what = format!("{tag}: {from} into {into}");
        switch(repository, into);
        let report = evidence(repository, baseline_block, into, from);
        assert_eq!(
            reason(&report),
            "proven_confluent",
            "{what}: {}",
            String::from_utf8_lossy(&report.stdout)
        );
        ok(
            &merge(repository, baseline_block, into, from),
            &format!("{what}: merge"),
        );
        ok(&verify(repository), &format!("{what}: verify"));
        assert_eq!(
            materialized(repository, &format!("{tag}-{}", into.replace('/', "-"))),
            want,
            "{what}: the merged tree"
        );
    }
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&twin);
}

/// Returns each direction's `merge-evidence` reason; asserts neither is proven confluent, the merge
/// refuses, and nothing moves.
fn assert_refuses_both_ways(
    tag: &str,
    baseline: &Tree,
    main: &[Step],
    side: &[Step],
) -> [String; 2] {
    let (repo, twin, baseline_block) = history(tag, baseline, main, side);
    let reasons = directions(&repo, &twin).map(|(repository, into, from)| {
        let what = format!("{tag}: {from} into {into}");
        let report = evidence(repository, baseline_block, into, from);
        let reason = reason(&report);
        assert_ne!(
            reason, "proven_confluent",
            "{what}: a false confluence claim"
        );
        let before = tip(repository, into);
        let refused = merge(repository, baseline_block, into, from);
        assert_eq!(refused.status.code(), Some(1), "{what}: merge must refuse");
        let stderr = String::from_utf8_lossy(&refused.stderr);
        assert!(
            stderr.contains("precondition not met: merge refused:"),
            "{what}: refused before any replay, as a precondition: {stderr}"
        );
        assert_eq!(
            tip(repository, into),
            before,
            "{what}: refusal writes nothing"
        );
        reason
    });
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&twin);
    reasons
}

fn tree(files: &[(&str, &[u8], bool)]) -> Tree {
    files
        .iter()
        .map(|(path, content, executable)| ((*path).to_string(), (content.to_vec(), *executable)))
        .collect()
}

// ---- §7.1: the hole -----------------------------------------------------------------------------

/// The architect's short file: the anchors of the restored word cover the other side's word.
#[test]
fn a_restored_word_beside_the_other_sides_edit_of_that_short_file_refuses_both_ways() {
    let baseline = tree(&[("e.txt", b"alpha beta gamma delta\n", false)]);
    let side = [
        write("e.txt", "alpha BETA gamma delta\n"),
        write("e.txt", "alpha beta gamma delta\n"),
    ];
    let main = [write("e.txt", "alpha beta gamma DELTA\n")];
    let reasons = assert_refuses_both_ways("dc75-fold-hole-short", &baseline, &main, &side);
    assert_eq!(
        reasons,
        [
            "sequence_internal_dependency_deferred".to_string(),
            "sequence_internal_dependency_deferred".to_string()
        ],
        "the run is judged as authored, and its later edit has a same-node predecessor (R8)"
    );
}

fn long_text(first: &str, far: &str) -> String {
    let mut text = format!("{first}\n");
    for line in 2..400 {
        text.push_str(&format!("line {line} of the long file\n"));
    }
    text.push_str(&format!("{far}\n"));
    text
}

/// The architect's long file: the restored word and the other side's word are 400 lines apart.
///
/// **Measured, and in tension with §7.2.3.** Handoff §7.2.3 expects this case to keep merging. Under
/// §7.2.1 as ruled (a net no-op drops its node only when the other side has no operation on it), the
/// side's run on `e.txt` is judged as authored, and the second edit does not replay alone against the
/// baseline, so its label is `sequence_internal_dependency_deferred` (R8). It refuses in both directions,
/// like the short file. Raised for a ruling in
/// `merge-two-edits-noop-report-v1.md`; this test pins what the rule does today.
#[test]
fn a_restored_word_far_from_the_other_sides_edit_of_that_long_file_refuses_under_rule_1() {
    let baseline_text = long_text("alpha beta", "gamma delta");
    let baseline = tree(&[("e.txt", baseline_text.as_bytes(), false)]);
    let side = [
        Step::Write("e.txt", long_text("alpha BETA", "gamma delta").into_bytes()),
        Step::Write("e.txt", baseline_text.clone().into_bytes()),
    ];
    let main = [Step::Write(
        "e.txt",
        long_text("alpha beta", "gamma DELTA").into_bytes(),
    )];
    let reasons = assert_refuses_both_ways("dc75-fold-hole-long", &baseline, &main, &side);
    assert_eq!(
        reasons,
        [
            "sequence_internal_dependency_deferred".to_string(),
            "sequence_internal_dependency_deferred".to_string()
        ]
    );
}

// ---- §7.2.3: execution soundness, every fold kind ------------------------------------------------

fn soundness_baseline() -> Tree {
    tree(&[
        ("e.txt", b"alpha beta gamma delta\n", false),
        ("f.txt", b"one two three\n", false),
        ("bin.dat", b"\0binary 0", false),
    ])
}

/// (a) the other side adds a file, (b) it edits a different file.
fn other_sides() -> [(&'static str, Vec<Step>); 2] {
    [
        ("untouched", vec![write("new.txt", "new\n")]),
        ("different-file", vec![write("f.txt", "one TWO three\n")]),
    ]
}

fn assert_fold_kind_executes(kind: &str, fold_steps: fn() -> Vec<Step>) {
    for (other, other_steps) in other_sides() {
        assert_merges_both_ways(
            &format!("dc75-fold-{kind}-{other}"),
            &soundness_baseline(),
            &other_steps,
            &fold_steps(),
        );
    }
}

#[test]
fn an_edit_run_executes_both_ways() {
    assert_fold_kind_executes("edit-run", || {
        vec![
            write("e.txt", "alpha BETA gamma delta\n"),
            write("e.txt", "alpha BETA GAMMA delta\n"),
        ]
    });
}

#[test]
fn edits_then_a_delete_execute_both_ways() {
    assert_fold_kind_executes("edits-delete", || {
        vec![
            write("e.txt", "alpha BETA gamma delta\n"),
            write("e.txt", "alpha BETA GAMMA delta\n"),
            Step::Delete("e.txt"),
        ]
    });
}

#[cfg(unix)]
#[test]
fn a_change_perm_run_executes_both_ways() {
    assert_fold_kind_executes("chmod-run", || {
        vec![
            Step::Executable("e.txt", true),
            Step::Executable("e.txt", false),
            Step::Executable("e.txt", true),
        ]
    });
}

#[test]
fn a_replace_binary_run_executes_both_ways() {
    assert_fold_kind_executes("replace-binary-run", || {
        vec![
            Step::Write("bin.dat", b"\0binary 1".to_vec()),
            Step::Write("bin.dat", b"\0binary 2".to_vec()),
        ]
    });
}

#[test]
fn a_net_no_op_executes_both_ways() {
    assert_fold_kind_executes("net-no-op", || {
        vec![
            write("e.txt", "alpha BETA gamma delta\n"),
            write("e.txt", "alpha beta gamma delta\n"),
        ]
    });
}

// ---- §7.3 R6: mixed kinds on one file, both directions --------------------------------------------

#[cfg(unix)]
#[test]
fn an_edit_a_mode_change_and_an_edit_execute_both_ways() {
    assert_fold_kind_executes("mixed-edit-chmod-edit", || {
        vec![
            write("e.txt", "alpha BETA gamma delta\n"),
            Step::Executable("e.txt", true),
            write("e.txt", "alpha BETA GAMMA delta\n"),
        ]
    });
}

#[cfg(unix)]
#[test]
fn an_edit_a_mode_change_and_a_delete_execute_both_ways() {
    assert_fold_kind_executes("mixed-edit-chmod-delete", || {
        vec![
            write("e.txt", "alpha BETA gamma delta\n"),
            Step::Executable("e.txt", true),
            Step::Delete("e.txt"),
        ]
    });
}

#[cfg(unix)]
#[test]
fn two_mode_changes_and_an_edit_execute_both_ways() {
    assert_fold_kind_executes("mixed-chmod-chmod-edit", || {
        vec![
            Step::Executable("e.txt", true),
            Step::Executable("e.txt", false),
            write("e.txt", "alpha beta GAMMA delta\n"),
        ]
    });
}

// ---- §7.3 R7: create-then-delete never folds --------------------------------------------------------

/// The side creates `h.txt` and deletes it again; main edits `f.txt`. Never proven confluent, in either
/// direction, and labelled by R8.
#[test]
fn a_create_then_delete_refuses_both_ways() {
    let side = [write("h.txt", "h\n"), Step::Delete("h.txt")];
    let main = [write("f.txt", "one TWO three\n")];
    let reasons = assert_refuses_both_ways(
        "dc75-fold-create-delete",
        &soundness_baseline(),
        &main,
        &side,
    );
    assert_eq!(
        reasons,
        [
            "sequence_internal_dependency_deferred".to_string(),
            "sequence_internal_dependency_deferred".to_string()
        ]
    );
}

// ---- §7.3 R4: create then edit, both directions -----------------------------------------------------

#[test]
fn a_create_then_edits_execute_both_ways() {
    assert_fold_kind_executes("create-then-edit", || {
        vec![
            write("h.txt", "h 0\n"),
            write("h.txt", "h 1\n"),
            write("h.txt", "h 2\n"),
        ]
    });
}
