//! 0.57.0 U4b (review `0.57.0-U4-review-v1`, ruling Q1): an archive carries every stored object (RFC 155 R1), so the backup
//! of a repository that holds a patch this version's path rule cannot read, that nothing reaches (the U4 fix's case: the
//! received ref that carried it was removed), carries that patch too. `archive verify` and `archive import` judge it by the
//! function the source's own `verify` asks (`block_state::outside_version_rule_is_excused`), so the receiver's `verify` says
//! what the source's said. Before this, the backup of a repository that `verify` passed was refused by `archive verify` and
//! `archive import`: the way out of the finding (`received remove`) left a repository that could not be backed up.
//!
//! The fixtures are U4's (`u4_unreached_outside_version_rule`): the Block and Patch are written with `CanonicalWriter`
//! (no honest `commit` can author the path), the received pointer with `write_received_pointer_for_test`, and the compiled
//! binary does every read, `received remove`, `archive export`, `archive verify` and `archive import`.
//!
//! Case table (the patch, who reaches it -> the source's `verify`, `archive verify`, `archive import`, the receiver's `verify`):
//!
//! | the patch's path | reached by | source `verify` | `archive verify` / `import` | receiver `verify` |
//! |---|---|---|---|---|
//! | non-ASCII (outside the rule) | a received ref | fails | **refuses**, the path-rule sentence; nothing written | (nothing imported) |
//! | non-ASCII (outside the rule) | nothing | passes, warning | **passes**, warning; imports | **passes**, the same warning |
//! | empty (damage, never excused) | nothing | fails | **refuses**; nothing written | (nothing imported) |
//!
//! The first column of verdicts must agree row by row (the 0.56.0 lesson: a derived check classifies as its source does).

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use super::u4_unreached_outside_version_rule::{
    fresh, received_non_ascii_chain, remove_received, text, verify, write_block,
    write_create_file_patch,
};
use crate::support;
use prikk_object::ObjectId;

fn run(repo: &Path, args: &[&str]) -> std::process::Output {
    support::prikk(repo).args(args).output().unwrap()
}

fn export(repo: &Path) -> PathBuf {
    let archive = repo.join("backup.prepo");
    support::ok(
        &run(repo, &["archive", "export", archive.to_str().unwrap()]),
        "archive export",
    );
    archive
}

fn archive_verify(repo: &Path, archive: &Path, extra: &[&str]) -> std::process::Output {
    let mut args = vec!["archive", "verify", archive.to_str().unwrap()];
    args.extend_from_slice(extra);
    run(repo, &args)
}

fn archive_import(target: &Path, archive: &Path) -> std::process::Output {
    run(target, &["archive", "import", archive.to_str().unwrap()])
}

/// An initialized repository that already trusts the fixture's maintainer key, so its own `verify` is about the objects
/// and not about publication trust.
fn receiver(tag: &str) -> PathBuf {
    let target = support::unique_repo(tag);
    support::init(&target);
    support::trust_maintainer(&target);
    target
}

/// Every file under `.prikk` except lock files: what "nothing was written" is compared against.
fn store_bytes(repo: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut stack = vec![repo.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.contains("lock"))
            {
                continue;
            }
            if path.is_dir() {
                stack.push(path);
            } else {
                files.insert(path.clone(), std::fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// The one line of `verify`'s output that begins `warning: block `, without the repository's path.
fn block_warning(output: &std::process::Output) -> String {
    text(output)
        .lines()
        .find(|line| line.starts_with("warning: block "))
        .unwrap_or_else(|| panic!("a block warning in:\n{}", text(output)))
        .to_string()
}

const PATH_RULE_SENTENCE: &str = "contains U+200D (invisible format character), which is outside what this version of prikk accepts";

#[test]
fn the_backup_of_a_repository_with_an_unreached_unreadable_patch_verifies_imports_and_verifies_there()
 {
    let (repo, block, patch) = received_non_ascii_chain("u4b-backup");
    remove_received(&repo);
    let source = verify(&repo);
    assert_eq!(
        source.status.code(),
        Some(0),
        "the source passes: {}",
        text(&source)
    );

    let archive = export(&repo);
    let said = archive_verify(&repo, &archive, &[]);
    assert_eq!(
        said.status.code(),
        Some(0),
        "archive verify must agree with the source's verify: {}",
        text(&said)
    );
    let said = text(&said);
    assert!(
        said.contains(&format!(
            "warning: patch {patch}: a path in it is outside what this version of prikk accepts"
        )),
        "the patch is named, as a warning\n{said}"
    );
    assert!(
        said.contains("held as an unreached object and this is not a failure"),
        "{said}"
    );
    assert!(said.contains("internally consistent"), "{said}");

    let json = archive_verify(&repo, &archive, &["--format", "json"]);
    assert_eq!(json.status.code(), Some(0), "{}", text(&json));
    let json = String::from_utf8_lossy(&json.stdout).to_string();
    assert!(
        json.contains(&format!(
            "\"unreached_outside_version_rule\": [\"{patch}\"]"
        )),
        "the JSON lists it\n{json}"
    );

    let target = receiver("u4b-backup-target");
    let imported = archive_import(&target, &archive);
    support::ok(&imported, "archive import of the backup");

    let there = verify(&target);
    let there_said = text(&there);
    assert_eq!(
        there.status.code(),
        Some(0),
        "the receiver's verify passes, as the source's does\n{there_said}"
    );
    assert_eq!(
        block_warning(&there),
        block_warning(&source),
        "the receiver's warning for the block is the source's"
    );
    assert!(
        there_said.contains(&format!("warning: block {block}: state root not verified")),
        "{there_said}"
    );
    assert!(
        there_said.contains("unreached objects: 3"),
        "block, patch and ref-state, as at the source\n{there_said}"
    );

    // A second generation: the receiver's own backup is accepted the same way.
    let again = export(&target);
    let again_said = archive_verify(&target, &again, &[]);
    assert_eq!(again_said.status.code(), Some(0), "{}", text(&again_said));
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn a_patch_a_carried_ref_reaches_still_refuses_archive_verify_and_import_and_writes_nothing() {
    // Not removed: the received ref still reaches the block, so the patch is reached.
    let (repo, _block, _patch) = received_non_ascii_chain("u4b-reached");
    assert_eq!(verify(&repo).status.code(), Some(1), "the source fails");
    let archive = export(&repo);

    let said = archive_verify(&repo, &archive, &[]);
    assert_eq!(said.status.code(), Some(1), "{}", text(&said));
    assert!(
        text(&said).contains(PATH_RULE_SENTENCE),
        "the refusal names the path: {}",
        text(&said)
    );

    let target = receiver("u4b-reached-target");
    let before = store_bytes(&target);
    let imported = archive_import(&target, &archive);
    assert_eq!(imported.status.code(), Some(1), "{}", text(&imported));
    assert!(
        text(&imported).contains(PATH_RULE_SENTENCE),
        "{}",
        text(&imported)
    );
    assert_eq!(
        store_bytes(&target),
        before,
        "a refused import writes nothing"
    );
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn an_unreached_patch_with_a_damaged_path_is_never_excused() {
    // An empty path is permanently invalid: damage, not "a newer prikk may have written it". Nothing reaches it, and it
    // still refuses -- only the path rule is excused.
    let (repo, mut store, genesis) = fresh("u4b-damage");
    let patch = write_create_file_patch(&mut store, "", 0x74);
    let _block = write_block(&mut store, genesis, vec![patch]);
    assert_eq!(verify(&repo).status.code(), Some(1), "the source fails");
    let archive = export(&repo);

    let said = archive_verify(&repo, &archive, &[]);
    assert_eq!(said.status.code(), Some(1), "{}", text(&said));
    assert!(
        text(&said).contains("repository path must not be empty"),
        "{}",
        text(&said)
    );
    let target = receiver("u4b-damage-target");
    let before = store_bytes(&target);
    let imported = archive_import(&target, &archive);
    assert_eq!(imported.status.code(), Some(1), "{}", text(&imported));
    assert_eq!(store_bytes(&target), before, "nothing written");
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&target);
}

#[test]
fn the_source_verify_and_the_archive_agree_row_by_row() {
    // The parity row: for every row of the case table, "the source's verify passes" equals "archive verify passes".
    let mut rows: Vec<(&str, bool, bool)> = Vec::new();

    let (reached, _block, _patch) = received_non_ascii_chain("u4b-parity-reached");
    rows.push((
        "outside the rule, reached",
        verify(&reached).status.success(),
        archive_verify(&reached, &export(&reached), &[])
            .status
            .success(),
    ));

    let (unreached, _block, _patch) = received_non_ascii_chain("u4b-parity-unreached");
    remove_received(&unreached);
    rows.push((
        "outside the rule, unreached",
        verify(&unreached).status.success(),
        archive_verify(&unreached, &export(&unreached), &[])
            .status
            .success(),
    ));

    let (damaged, mut store, genesis) = fresh("u4b-parity-damaged");
    let patch: ObjectId = write_create_file_patch(&mut store, "", 0x75);
    let _block = write_block(&mut store, genesis, vec![patch]);
    rows.push((
        "damaged path, unreached",
        verify(&damaged).status.success(),
        archive_verify(&damaged, &export(&damaged), &[])
            .status
            .success(),
    ));

    for (label, source_passes, archive_passes) in &rows {
        assert_eq!(
            source_passes, archive_passes,
            "{label}: the source's verify ({source_passes}) and archive verify ({archive_passes}) must agree"
        );
    }
    assert_eq!(
        rows.iter().map(|row| row.1).collect::<Vec<_>>(),
        vec![false, true, false],
        "the rows are the three the case table names: {rows:?}"
    );
    for repo in [&reached, &unreached, &damaged] {
        let _ = std::fs::remove_dir_all(repo);
    }
}
