//! 0.59.0 step G2d: the case table of `--remove-ref-candidates` (crash, work, lost, repeat, damage, a lead, the way back) and of the write-path
//! precondition's new rule (a candidate no running publisher could be using no longer refuses).

#![allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used
)]

use super::*;
use crate::foundation::layout::DEFAULT_ACTIVE_NAME;
use crate::test_gates::test_support::unique_temp_dir;

fn repository(name: &str) -> (RepositoryLayout, std::path::PathBuf) {
    let root = unique_temp_dir(name);
    let layout = RepositoryLayout::init(root.clone()).expect("init");
    (layout, root)
}

fn tmp_dir(layout: &RepositoryLayout) -> std::path::PathBuf {
    layout.refs_dir().join("tmp")
}

fn legacy(ref_name: &str) -> Vec<u8> {
    let mut bytes = b"PREFPTR1".to_vec();
    bytes.extend_from_slice(&(ref_name.len() as u64).to_be_bytes());
    bytes.extend_from_slice(ref_name.as_bytes());
    bytes.extend_from_slice(&[7; 32]);
    bytes
}

fn put(layout: &RepositoryLayout, name: &str, bytes: &[u8]) {
    std::fs::create_dir_all(tmp_dir(layout)).unwrap();
    std::fs::write(tmp_dir(layout).join(name), bytes).unwrap();
}

fn names(layout: &RepositoryLayout) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(tmp_dir(layout))
        .map(|dir| {
            dir.flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn recovery_bytes(layout: &RepositoryLayout) -> Vec<u8> {
    std::fs::read(layout.prikk_dir().join("recovery/log")).unwrap_or_default()
}

#[test]
fn a_plan_names_each_file_and_writes_nothing() {
    let (layout, root) = repository("g2d-plan");
    put(&layout, "a.tmp", &legacy("heads/main"));
    put(&layout, "b.tmp", b"not a candidate");
    let before = recovery_bytes(&layout);
    let report = plan_remove_ref_candidates(&layout).unwrap();
    assert_eq!(report.candidates.len(), 2);
    assert!(
        report
            .candidates
            .iter()
            .all(|c| c.action == RefCandidateAction::WouldRemove && c.recovery.is_none())
    );
    assert!(matches!(
        &report.candidates[0].kind,
        RefCandidateKind::LegacyPointer { ref_name, .. } if ref_name == "heads/main"
    ));
    assert_eq!(report.candidates[1].kind, RefCandidateKind::NotACandidate);
    assert_eq!(names(&layout), ["a.tmp", "b.tmp"]);
    assert_eq!(recovery_bytes(&layout), before);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn a_run_saves_each_file_first_and_removes_it_and_a_repeat_writes_nothing() {
    let (layout, root) = repository("g2d-run");
    put(&layout, "a.tmp", &legacy("heads/main"));
    put(&layout, "b.tmp", b"not a candidate");
    let report = remove_ref_candidates(&layout).unwrap();
    assert!(
        report
            .candidates
            .iter()
            .all(|c| c.action == RefCandidateAction::Removed && c.recovery.is_some())
    );
    assert!(names(&layout).is_empty());
    let listing = crate::recovery_log::recovery_list(&layout).unwrap();
    assert_eq!(listing.entries.len(), 2);
    assert!(
        listing
            .entries
            .iter()
            .all(|e| e.kind == crate::recovery_log::RecoveryEntryKind::Remove)
    );
    // The repeat: nothing remains, nothing is written.
    let saved = recovery_bytes(&layout);
    let again = remove_ref_candidates(&layout).unwrap();
    assert!(again.candidates.is_empty());
    assert_eq!(recovery_bytes(&layout), saved);
    let _ = std::fs::remove_dir_all(root);
}

/// **Crash:** killed after the save and before the removal: the file is still there and a copy is saved; the run again saves again and removes.
#[test]
fn a_run_again_after_a_crash_between_the_save_and_the_removal_finishes() {
    let (layout, root) = repository("g2d-crash");
    let bytes = legacy("heads/main");
    put(&layout, "a.tmp", &bytes);
    let _run = crate::recovery_log::begin_run();
    crate::recovery_log::save_remove(&layout, "refs/tmp/a.tmp", &bytes, "remove-ref-candidates")
        .unwrap();
    drop(_run);
    assert_eq!(names(&layout), ["a.tmp"]);
    let report = remove_ref_candidates(&layout).unwrap();
    assert_eq!(report.candidates[0].action, RefCandidateAction::Removed);
    assert!(names(&layout).is_empty());
    assert_eq!(
        crate::recovery_log::recovery_list(&layout)
            .unwrap()
            .entries
            .len(),
        2
    );
    let _ = std::fs::remove_dir_all(root);
}

/// **Lost:** the file vanishes between the save and the removal: that step says so and the run goes on.
#[test]
fn a_file_that_vanishes_between_the_save_and_the_removal_is_already_absent() {
    let (layout, root) = repository("g2d-lost");
    put(&layout, "a.tmp", b"x");
    let path = tmp_dir(&layout).join("a.tmp");
    crate::refs::view_hooks::set("remove-candidate-after-save", move || {
        std::fs::remove_file(path).unwrap();
    });
    let report = remove_ref_candidates(&layout).unwrap();
    assert!(!crate::refs::view_hooks::pending(
        "remove-candidate-after-save"
    ));
    assert_eq!(
        report.candidates[0].action,
        RefCandidateAction::AlreadyAbsent
    );
    let _ = std::fs::remove_dir_all(root);
}

/// **Work:** another running process holds the ref's lock, or the active lock: refused before any write, naming the publisher.
#[test]
fn a_running_publisher_of_a_named_ref_or_the_queue_is_refused_before_any_write() {
    let (layout, root) = repository("g2d-work");
    put(&layout, "a.tmp", &legacy("heads/main"));
    let before = recovery_bytes(&layout);
    let (mut child, body) = crate::lock_holder::body_naming_another_running_process("ref");
    std::fs::create_dir_all(layout.ref_lock_path("heads/main").parent().unwrap()).unwrap();
    std::fs::write(layout.ref_lock_path("heads/main"), body).unwrap();
    for run in [plan_remove_ref_candidates, remove_ref_candidates] {
        let error = run(&layout).unwrap_err();
        assert!(
            matches!(&error, PrikkError::Precondition(message)
                if message.contains("another command is publishing heads/main")
                    && message.contains("run this command again when it finishes")),
            "{error:?}"
        );
    }
    assert_eq!(names(&layout), ["a.tmp"]);
    assert_eq!(recovery_bytes(&layout), before);
    child.kill().unwrap();
    std::fs::remove_file(layout.ref_lock_path("heads/main")).unwrap();

    let (mut child, body) = crate::lock_holder::body_naming_another_running_process("active");
    std::fs::write(layout.active_lock_path(DEFAULT_ACTIVE_NAME), body).unwrap();
    let error = remove_ref_candidates(&layout).unwrap_err();
    assert!(
        matches!(&error, PrikkError::Precondition(message) if message.contains("another command is publishing")),
        "{error:?}"
    );
    assert_eq!(names(&layout), ["a.tmp"]);
    child.kill().unwrap();
    let _ = std::fs::remove_dir_all(root);
}

/// **Damage:** an entry that is neither a file nor an empty directory (a directory that holds something, a symbolic link): refused before anything
/// is touched, naming it and saying no prikk command removes it.
#[test]
fn an_entry_that_is_not_prikks_to_remove_refuses_before_anything_is_touched() {
    let (layout, root) = repository("g2e-stuck");
    put(&layout, "a.tmp", b"x");
    std::fs::create_dir_all(tmp_dir(&layout).join("e")).unwrap();
    std::fs::create_dir_all(tmp_dir(&layout).join("d")).unwrap();
    std::fs::write(tmp_dir(&layout).join("d/inside"), b"y").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/nonexistent", tmp_dir(&layout).join("l")).unwrap();
    let before = recovery_bytes(&layout);
    for run in [plan_remove_ref_candidates, remove_ref_candidates] {
        let error = run(&layout).unwrap_err();
        assert!(
            matches!(&error, PrikkError::Integrity(message)
                if message.contains("unexpected non-file in ref candidate directory: refs/tmp/d")
                    && message.contains("no prikk command removes it: prikk never wrote it")),
            "{error:?}"
        );
    }
    #[cfg(unix)]
    {
        // The directory that holds something goes; the link alone is refused too, by name.
        std::fs::remove_file(tmp_dir(&layout).join("d/inside")).unwrap();
        std::fs::remove_dir(tmp_dir(&layout).join("d")).unwrap();
        let error = remove_ref_candidates(&layout).unwrap_err();
        assert!(
            matches!(&error, PrikkError::Integrity(message) if message.contains("refs/tmp/l")),
            "{error:?}"
        );
        assert_eq!(names(&layout), ["a.tmp", "e", "l"]);
    }
    assert_eq!(recovery_bytes(&layout), before);
    assert!(tmp_dir(&layout).join("a.tmp").exists() && tmp_dir(&layout).join("e").is_dir());
    let _ = std::fs::remove_dir_all(root);
}

/// **An empty directory** is removed with no bytes saved: the plan says so, the run removes it, the repeat has none, and the recovery log is untouched.
#[test]
fn an_empty_directory_is_removed_and_nothing_is_saved_for_it() {
    let (layout, root) = repository("g2e-empty");
    std::fs::create_dir_all(tmp_dir(&layout).join("e")).unwrap();
    put(&layout, "a.tmp", b"x");
    let before = recovery_bytes(&layout);
    let plan = plan_remove_ref_candidates(&layout).unwrap();
    assert_eq!(plan.candidates[1].name, "e");
    assert_eq!(plan.candidates[1].kind, RefCandidateKind::EmptyDirectory);
    assert_eq!(plan.candidates[1].action, RefCandidateAction::WouldRemove);
    assert_eq!(names(&layout), ["a.tmp", "e"]);
    let run = remove_ref_candidates(&layout).unwrap();
    assert_eq!(run.candidates[1].action, RefCandidateAction::Removed);
    assert!(run.candidates[1].recovery.is_none());
    assert!(run.candidates[0].recovery.is_some());
    assert!(names(&layout).is_empty());
    // One entry (the file); the directory left none.
    assert_eq!(
        crate::recovery_log::recovery_list(&layout)
            .unwrap()
            .entries
            .len(),
        1
    );
    assert_ne!(recovery_bytes(&layout), before);
    assert!(
        remove_ref_candidates(&layout)
            .unwrap()
            .candidates
            .is_empty()
    );
    let _ = std::fs::remove_dir_all(root);
}

/// **Lost, for a directory:** it fills between the read and the removal: the removal refuses (`rmdir` removes nothing that holds something), the
/// directory and its file are as they were, and the run says so.
#[test]
fn a_directory_that_fills_before_its_removal_is_kept() {
    let (layout, root) = repository("g2e-fills");
    std::fs::create_dir_all(tmp_dir(&layout).join("e")).unwrap();
    let inside = tmp_dir(&layout).join("e/late");
    crate::refs::view_hooks::set("remove-empty-directory-before", move || {
        std::fs::write(inside, b"late").unwrap();
    });
    assert!(remove_ref_candidates(&layout).is_err());
    assert!(!crate::refs::view_hooks::pending(
        "remove-empty-directory-before"
    ));
    assert_eq!(
        std::fs::read(tmp_dir(&layout).join("e/late")).unwrap(),
        b"late"
    );
    let _ = std::fs::remove_dir_all(root);
}

/// `verify`'s finding is read directly: an empty directory is removable, anything else is not, and a stuck entry is named before an empty one.
#[test]
fn the_finding_names_the_entry_no_command_removes_before_one_that_one_does() {
    let (layout, root) = repository("g2e-finding");
    assert_eq!(crate::refs::ref_tmp_finding(&layout), None);
    std::fs::create_dir_all(tmp_dir(&layout).join("a-empty")).unwrap();
    assert_eq!(
        crate::refs::ref_tmp_finding(&layout),
        Some((
            "a-empty".to_string(),
            crate::refs::RefTmpEntryKind::EmptyDirectory
        ))
    );
    std::fs::create_dir_all(tmp_dir(&layout).join("z-full")).unwrap();
    std::fs::write(tmp_dir(&layout).join("z-full/f"), b"y").unwrap();
    assert_eq!(
        crate::refs::ref_tmp_finding(&layout),
        Some((
            "z-full".to_string(),
            crate::refs::RefTmpEntryKind::NotPrikks
        ))
    );
    let _ = std::fs::remove_dir_all(root);
}

/// **The way back:** `--recovery-restore` recreates a removed file with the bytes it held, only where nothing is.
#[test]
fn a_restore_recreates_the_file_only_where_nothing_is() {
    let (layout, root) = repository("g2d-restore");
    let bytes = legacy("heads/main");
    put(&layout, "a.tmp", &bytes);
    let report = remove_ref_candidates(&layout).unwrap();
    let run = report.candidates[0].recovery.as_ref().unwrap().id.clone();
    // Something is at the path now: refused, and untouched.
    put(&layout, "a.tmp", b"someone else's");
    let plan = crate::recovery_log::recovery_restore(&layout, &run, false).unwrap();
    assert!(!plan.written, "{plan:?}");
    assert_eq!(
        std::fs::read(tmp_dir(&layout).join("a.tmp")).unwrap(),
        b"someone else's"
    );
    std::fs::remove_file(tmp_dir(&layout).join("a.tmp")).unwrap();
    // Absent: recreated, byte for byte.
    let plan = crate::recovery_log::recovery_restore(&layout, &run, false).unwrap();
    assert!(plan.written, "{plan:?}");
    assert_eq!(
        std::fs::read(tmp_dir(&layout).join("a.tmp")).unwrap(),
        bytes
    );
    // Run again: already holds its result.
    let plan = crate::recovery_log::recovery_restore(&layout, &run, false).unwrap();
    assert!(plan.steps.iter().all(|step| step.done), "{plan:?}");
    let _ = std::fs::remove_dir_all(root);
}

/// **The frame:** a `Remove` entry is a version-2 frame (a prikk that does not know it counts it as written by a newer prikk and lists every other
/// entry); a version-1 frame of its kind, and a version-2 frame of another kind, are not entries.
#[test]
fn a_remove_entry_is_framed_as_version_two() {
    let (layout, root) = repository("g2d-frame");
    put(&layout, "a.tmp", b"x");
    remove_ref_candidates(&layout).unwrap();
    let bytes = recovery_bytes(&layout);
    assert_eq!(&bytes[8..10], &2_u16.to_be_bytes());
    let listing = crate::recovery_log::recovery_list(&layout).unwrap();
    assert_eq!(
        (
            listing.entries.len(),
            listing.newer_versions,
            listing.damaged_regions
        ),
        (1, 0, 0)
    );
    let _ = std::fs::remove_dir_all(root);
}

// ---- the write-path precondition -----------------------------------------------------------------------------------------------------

/// A leftover candidate no running publisher could be using no longer refuses; one for a ref whose lock another running process holds does.
#[test]
fn the_precondition_refuses_a_candidate_only_while_a_publisher_runs() {
    let (layout, root) = repository("g2d-precondition");
    put(&layout, "undecodable.tmp", b"not a candidate");
    crate::refs::ensure_may_publish(&layout, "heads/x").unwrap();
    put(&layout, "a.tmp", &legacy("heads/main"));
    crate::refs::ensure_may_publish(&layout, "heads/x").unwrap();
    let (mut child, body) = crate::lock_holder::body_naming_another_running_process("ref");
    std::fs::create_dir_all(layout.ref_lock_path("heads/main").parent().unwrap()).unwrap();
    std::fs::write(layout.ref_lock_path("heads/main"), body).unwrap();
    let error = crate::refs::ensure_may_publish(&layout, "heads/x").unwrap_err();
    assert!(
        matches!(&error, PrikkError::Precondition(message)
            if message.contains("another command is publishing heads/main")),
        "{error:?}"
    );
    child.kill().unwrap();
    std::fs::remove_file(layout.ref_lock_path("heads/main")).unwrap();
    crate::refs::ensure_may_publish(&layout, "heads/x").unwrap();
    // 0.59.0 step G2e: nor does a directory (empty or not) or a link: no writer reads `refs/tmp`.
    std::fs::create_dir_all(tmp_dir(&layout).join("d")).unwrap();
    crate::refs::ensure_may_publish(&layout, "heads/x").unwrap();
    std::fs::write(tmp_dir(&layout).join("d/f"), b"y").unwrap();
    crate::refs::ensure_may_publish(&layout, "heads/x").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("/nonexistent", tmp_dir(&layout).join("l")).unwrap();
        crate::refs::ensure_may_publish(&layout, "heads/x").unwrap();
    }
    let _ = std::fs::remove_dir_all(root);
}
