//! Required directory creation, validation, and race tests.

use std::fs;
use std::path::Path;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::sync::{Arc, Barrier};

use crate::test_gates::test_support::unique_temp_dir;

use super::super::ensure_directory_required;
use super::mutation_root;
// DC-97: `concurrent_required_directory_creation_is_idempotent`'s deterministic race relies on
// `set_directory_create_barrier_for_test`, which is *itself* inside `anchored/failpoints.rs`
// (`wait_at_directory_create` is called only from the Unix `AnchoredDirectory::ensure_child`,
// `directory.rs:283` -- never from `windows_authority.rs`). G8 does not get an equally rigorous
// Windows control from writing a threaded test alone: without the barrier, N threads racing on the
// OS scheduler alone is weaker evidence than a synchronized race, and reproducing the barrier
// mechanism itself needs the same unbuilt Windows failpoint wiring G3 does. Reported, not worked
// around -- see the review submission.
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::super::{TestFailPoint, fail_once_for_test, set_directory_create_barrier_for_test};

#[test]
fn required_directory_creation_builds_each_component() {
    let path = unique_temp_dir("required-directory");
    let root = mutation_root(&path);
    assert!(ensure_directory_required(&root, Path::new("one/two/three")).is_ok());
    assert!(path.join("one/two/three").is_dir());
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn concurrent_required_directory_creation_is_idempotent() -> prikk_error::Result<()> {
    let path = unique_temp_dir("concurrent-required-directory");
    let root = mutation_root(&path);
    let barrier = Arc::new(Barrier::new(8));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let thread_root = root.clone();
        let thread_barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            set_directory_create_barrier_for_test(thread_barrier);
            ensure_directory_required(&thread_root, Path::new("shared/shard"))
        }));
    }
    for handle in handles {
        handle.join().map_err(|_| prikk_error::PrikkError::Io {
            kind: None,
            context: "directory race thread panicked".to_string(),
        })??;
    }
    assert!(path.join("shared/shard").is_dir());
    let _ = fs::remove_dir_all(path);
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn observed_component_parent_sync_failure_is_retryable() {
    let path = unique_temp_dir("observed-parent-sync-failure");
    let root = mutation_root(&path);
    assert!(fs::create_dir(path.join("existing")).is_ok());
    fail_once_for_test(TestFailPoint::ObservedDirectoryParentSync);
    assert!(ensure_directory_required(&root, Path::new("existing/child")).is_err());
    assert!(!path.join("existing/child").exists());
    assert!(ensure_directory_required(&root, Path::new("existing/child")).is_ok());
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn directory_create_failure_has_no_side_effect_and_is_retryable() {
    let path = unique_temp_dir("directory-create-failure");
    let root = mutation_root(&path);
    fail_once_for_test(TestFailPoint::DirectoryCreate);
    assert!(ensure_directory_required(&root, Path::new("child")).is_err());
    assert!(!path.join("child").exists());
    assert!(ensure_directory_required(&root, Path::new("child")).is_ok());
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_directory_parent_sync_retains_created_component() {
    let path = unique_temp_dir("required-directory-failure");
    let root = mutation_root(&path);
    fail_once_for_test(TestFailPoint::CreatedDirectoryParentSync);
    assert!(ensure_directory_required(&root, Path::new("child")).is_err());
    assert!(path.join("child").is_dir());
    assert!(ensure_directory_required(&root, Path::new("child")).is_ok());
    let _ = fs::remove_dir_all(path);
}

#[cfg(target_family = "unix")]
#[test]
fn required_directory_rejects_symlink_component() {
    use std::os::unix::fs::symlink;

    let path = unique_temp_dir("required-symlink");
    let root = mutation_root(&path);
    assert!(ensure_directory_required(&root, Path::new("target")).is_ok());
    assert!(symlink(path.join("target"), path.join("link")).is_ok());
    assert!(ensure_directory_required(&root, Path::new("link/child")).is_err());
    let _ = fs::remove_dir_all(path);
}
