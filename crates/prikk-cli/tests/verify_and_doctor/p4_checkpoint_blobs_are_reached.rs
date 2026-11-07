//! 0.56.0 P4: a checkpoint's content Blobs are reached through its snapshot manifest, so an honest
//! repository whose one text file is edited at every step reads **no unreached objects**.
//!
//! **The measured defect (external review 046, §0).** An `EditText` names no Blob, so the content
//! Blobs of a text file edited step after step are named only by the snapshot manifest a checkpoint
//! Block (every 64th) carries. The reach walk followed a Block's `snapshot_blob_ref` to the manifest
//! Blob and then expanded nothing, so `verify` and `doctor` reported one unreached Blob per
//! checkpoint, each with the forgery-leftover warning: 64 blocks read 0, 130 read 2.
//!
//! Without the `Blob` arm of `verify::reachability::expand_frontier` the 65- and 130-block
//! assertions here fail (the store-level twin is `verify/tests/checkpoint_manifest_blobs.rs`).

#![allow(clippy::expect_used, clippy::unwrap_used)]

use crate::support;
use std::path::Path;
use std::process::Output;

fn text(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// Append one line to `a.txt` and commit and seal it: `steps` times, numbering from `first`.
fn edit_steps(repo: &Path, first: usize, steps: usize) {
    for step in first..first + steps {
        let mut content = std::fs::read(repo.join("a.txt")).unwrap_or_default();
        content.extend_from_slice(format!("line {step}\n").as_bytes());
        support::generation(
            repo,
            "heads/main",
            "a.txt",
            &content,
            &format!("edit {step}"),
        );
    }
}

fn assert_no_unreached(repo: &Path, blocks: usize) {
    let verify = support::prikk(repo).arg("verify").output().unwrap();
    let verify_text = text(&verify);
    assert!(verify.status.success(), "{blocks} blocks: {verify_text}");
    assert!(
        verify_text.contains("unreached objects: 0"),
        "{blocks} blocks: expected `unreached objects: 0`, got:\n{verify_text}"
    );
    assert!(
        !verify_text.contains("is reached by nothing"),
        "{blocks} blocks: a checkpoint Blob read as unreached:\n{verify_text}"
    );
    let doctor = support::prikk(repo).arg("doctor").output().unwrap();
    let doctor_text = text(&doctor);
    assert!(doctor.status.success(), "{blocks} blocks: {doctor_text}");
    assert!(
        !doctor_text.contains("PRIKK-DOCTOR-UNREACHED-OBJECT"),
        "{blocks} blocks: doctor named an unreached object:\n{doctor_text}"
    );
}

#[test]
fn a_text_file_edited_at_every_step_reads_no_unreached_objects_past_a_checkpoint() {
    let repo = support::unique_repo("p4-checkpoint-blobs");
    support::init(&repo);

    // 65 blocks: the first checkpoint (the 64th) has been written.
    edit_steps(&repo, 1, 65);
    assert_no_unreached(&repo, 65);

    // The same history exported: nothing unreached in the archive either.
    let archive = repo.join("history.prepo");
    let export = support::prikk(&repo)
        .args(["archive", "export"])
        .arg(&archive)
        .output()
        .unwrap();
    assert!(export.status.success(), "{}", text(&export));
    let archive_verify = support::prikk(&repo)
        .args(["archive", "verify"])
        .arg(&archive)
        .output()
        .unwrap();
    let archive_text = text(&archive_verify);
    assert!(archive_verify.status.success(), "{archive_text}");
    assert!(
        !archive_text.contains("unreached") && !archive_text.contains("unreferenced"),
        "archive verify reports an unreached object:\n{archive_text}"
    );

    // 130 blocks: the second checkpoint (the 128th) too.
    edit_steps(&repo, 66, 65);
    assert_no_unreached(&repo, 130);

    let _ = std::fs::remove_dir_all(&repo);
}
