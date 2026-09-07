use std::fs;
use std::path::Path;

mod conformance;
mod directory;

use crate::RepositoryLayout;
use crate::test_gates::test_support::unique_temp_dir;

use super::{MutationRoot, write_file_atomically, write_worktree_file_atomically};
// DC-97: the failpoint injection mechanism has no Windows implementation (fsutil.rs's own
// `mod caller_tests` comment states why) -- every test that uses it, and every test that uses these
// otherwise-portable primitives only from within a failpoint-gated test, stays inline-gated below,
// and their imports are gated to match rather than pulled in unconditionally now that this file's
// outer module gate includes Windows.
#[cfg(any(target_os = "linux", target_os = "macos"))]
use super::{
    TestFailPoint, append_file_required, create_new_file_required, fail_once_for_test,
    read_file_if_exists, remove_file_required, truncate_existing_file_required,
};

fn mutation_root(path: &Path) -> MutationRoot {
    match MutationRoot::open(path) {
        Ok(root) => root,
        Err(error) => panic!("test mutation root failed: {error}"),
    }
}

#[test]
fn root_capability_remains_bound_after_path_replacement() {
    let path = unique_temp_dir("root-replacement");
    let replacement = path.with_extension("replacement");
    let root = mutation_root(&path);
    assert!(fs::rename(&path, &replacement).is_ok());
    assert!(fs::create_dir(&path).is_ok());
    assert!(write_file_atomically(&root, Path::new("state"), b"bound").is_ok());
    assert_eq!(
        fs::read(replacement.join("state")).unwrap_or_default(),
        b"bound"
    );
    assert!(!path.join("state").exists());
    let _ = fs::remove_dir_all(path);
    let _ = fs::remove_dir_all(replacement);
}

#[test]
fn worktree_writer_remains_bound_after_root_replacement() {
    let path = unique_temp_dir("worktree-root-replacement");
    let replacement = path.with_extension("replacement");
    let root = mutation_root(&path);
    assert!(fs::rename(&path, &replacement).is_ok());
    assert!(fs::create_dir(&path).is_ok());
    assert!(write_worktree_file_atomically(&root, Path::new("file"), b"bound").is_ok());
    assert_eq!(
        fs::read(replacement.join("file")).unwrap_or_default(),
        b"bound"
    );
    assert!(!path.join("file").exists());
    let _ = fs::remove_dir_all(path);
    let _ = fs::remove_dir_all(replacement);
}

// DC-97: failpoint-based -- no Windows failpoint mechanism exists (see this file's own top-level
// import comment).
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn worktree_write_sync_failure_retains_file_and_is_retryable() {
    let path = unique_temp_dir("worktree-write-failure");
    let root = mutation_root(&path);
    fail_once_for_test(TestFailPoint::MutableParentSync);
    assert!(write_worktree_file_atomically(&root, Path::new("file"), b"retained").is_err());
    assert_eq!(fs::read(path.join("file")).unwrap_or_default(), b"retained");
    assert!(write_worktree_file_atomically(&root, Path::new("file"), b"retry").is_ok());
    assert_eq!(fs::read(path.join("file")).unwrap_or_default(), b"retry");
    let _ = fs::remove_dir_all(path);
}

#[test]
fn repository_layout_remains_bound_after_prikk_replacement() {
    let path = unique_temp_dir("repository-root-replacement");
    let layout = match RepositoryLayout::init(path.clone()) {
        Ok(layout) => layout,
        Err(error) => panic!("repository layout failed: {error}"),
    };
    let displaced = path.join(".prikk-displaced");
    assert!(fs::rename(layout.prikk_dir(), &displaced).is_ok());
    assert!(fs::create_dir(layout.prikk_dir()).is_ok());
    assert!(
        write_file_atomically(
            layout.repository_mutation_root(),
            Path::new("authority-test"),
            b"bound"
        )
        .is_ok()
    );
    assert_eq!(
        fs::read(displaced.join("authority-test")).unwrap_or_default(),
        b"bound"
    );
    assert!(!layout.prikk_dir().join("authority-test").exists());
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn required_open_failure_has_no_side_effect_and_is_retryable() {
    let path = unique_temp_dir("required-open-failure");
    let root = mutation_root(&path);
    fail_once_for_test(TestFailPoint::RequiredOpen);
    assert!(write_file_atomically(&root, Path::new("state"), b"candidate").is_err());
    assert!(!path.join("state").exists());
    assert!(write_file_atomically(&root, Path::new("state"), b"retry").is_ok());
    let _ = fs::remove_dir_all(path);
}

#[test]
fn mutable_atomic_write_replaces_complete_content() {
    let path = unique_temp_dir("required-atomic-write");
    let root = mutation_root(&path);
    assert!(write_file_atomically(&root, Path::new("state"), b"first").is_ok());
    assert!(write_file_atomically(&root, Path::new("state"), b"second").is_ok());
    assert_eq!(fs::read(path.join("state")).unwrap_or_default(), b"second");
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_mutable_parent_sync_retains_replaced_final_name() {
    let path = unique_temp_dir("required-mutable-sync-failure");
    let root = mutation_root(&path);
    fail_once_for_test(TestFailPoint::MutableParentSync);
    assert!(write_file_atomically(&root, Path::new("state"), b"retained").is_err());
    assert_eq!(
        fs::read(path.join("state")).unwrap_or_default(),
        b"retained"
    );
    assert!(write_file_atomically(&root, Path::new("state"), b"retry").is_ok());
    assert_eq!(fs::read(path.join("state")).unwrap_or_default(), b"retry");
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_mutable_file_sync_keeps_only_non_authoritative_temp() {
    let path = unique_temp_dir("required-mutable-file-failure");
    let root = mutation_root(&path);
    fail_once_for_test(TestFailPoint::MutableFileSync);
    assert!(write_file_atomically(&root, Path::new("state"), b"candidate").is_err());
    assert!(!path.join("state").exists());
    let debris = fs::read_dir(&path)
        .map(|entries| entries.filter_map(std::result::Result::ok).count())
        .unwrap_or_default();
    assert_eq!(debris, 1);
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_mutable_rename_keeps_previous_authoritative_state() {
    let path = unique_temp_dir("required-mutable-rename-failure");
    let root = mutation_root(&path);
    assert!(write_file_atomically(&root, Path::new("state"), b"previous").is_ok());
    fail_once_for_test(TestFailPoint::MutableRename);
    assert!(write_file_atomically(&root, Path::new("state"), b"candidate").is_err());
    assert_eq!(
        fs::read(path.join("state")).unwrap_or_default(),
        b"previous"
    );
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_append_write_is_retryable() {
    let path = unique_temp_dir("append-write-failure");
    let root = mutation_root(&path);
    // RFC 102 Stage 5, design-v1.md §14.3/§14.5: `durable_append` now requires an existing file --
    // every production caller's target name is created at `init`, and this test's setup must match
    // that discipline rather than lean on the retired create-on-append fallback.
    assert!(create_new_file_required(&root, Path::new("log"), b"").is_ok());
    fail_once_for_test(TestFailPoint::AppendWrite);
    assert!(append_file_required(&root, Path::new("log"), b"record").is_err());
    assert_eq!(fs::read(path.join("log")).unwrap_or_default(), b"");
    assert!(append_file_required(&root, Path::new("log"), b"record").is_ok());
    assert_eq!(fs::read(path.join("log")).unwrap_or_default(), b"record");
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_truncate_retains_previous_state_and_is_retryable() {
    let path = unique_temp_dir("truncate-failure");
    let root = mutation_root(&path);
    assert!(fs::write(path.join("wal"), b"complete-partial").is_ok());
    fail_once_for_test(TestFailPoint::Truncate);
    assert!(truncate_existing_file_required(&root, Path::new("wal"), 8).is_err());
    assert_eq!(
        fs::read(path.join("wal")).unwrap_or_default(),
        b"complete-partial"
    );
    assert!(truncate_existing_file_required(&root, Path::new("wal"), 8).is_ok());
    assert_eq!(fs::read(path.join("wal")).unwrap_or_default(), b"complete");
    let _ = fs::remove_dir_all(path);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn failed_unlink_retains_file_and_cleanup_sync_reports_removed_state() {
    let path = unique_temp_dir("unlink-failure");
    let root = mutation_root(&path);
    assert!(fs::write(path.join("entry"), b"state").is_ok());
    fail_once_for_test(TestFailPoint::Unlink);
    assert!(remove_file_required(&root, Path::new("entry")).is_err());
    assert!(path.join("entry").is_file());
    fail_once_for_test(TestFailPoint::CleanupDirectorySync);
    assert!(super::remove_worktree_file_required(&root, Path::new("entry")).is_err());
    assert!(!path.join("entry").exists());
    let _ = fs::remove_dir_all(path);
}

/// DC-81: `mkfifoat` and non-blocking-open-refuses-a-FIFO are POSIX-specified behavior (POSIX.1-2017
/// §2.9.7: opening a FIFO `O_WRONLY | O_NONBLOCK` with no reader present fails `ENXIO`), so this is
/// ported to macOS rather than left Linux-only — but it is a genuine port, not a recompile
/// (DC-81 addendum-1 §3): the *POSIX specification* is common to both platforms; this exact runtime
/// behavior on APFS has not been observed and needs the macOS CI job (once it exists) to confirm.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn append_and_truncate_reject_fifo_without_blocking() {
    use std::sync::mpsc;
    use std::time::Duration;

    use crate::test_gates::test_support::create_fifo_for_test;

    let path = unique_temp_dir("fifo-final-entry");
    let fifo = path.join("wal");
    assert!(create_fifo_for_test(&fifo, 0o600).is_ok());
    let root = mutation_root(&path);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let append = append_file_required(&root, Path::new("wal"), b"record");
        let truncate = truncate_existing_file_required(&root, Path::new("wal"), 0);
        let _ = sender.send((append, truncate));
    });
    let result = receiver.recv_timeout(Duration::from_secs(1));
    assert!(result.is_ok(), "special-file rejection must be bounded");
    if let Ok((append, truncate)) = result {
        assert!(append.is_err());
        assert!(truncate.is_err());
    }
    let _ = fs::remove_file(fifo);
    let _ = fs::remove_dir_all(path);
}

/// G6, isolated from G7: opening a FIFO for *reading* with `O_NONBLOCK` succeeds immediately at the
/// OS level even with no writer present (unlike the write side above, which the kernel itself
/// refuses with `ENXIO` when unblocked, independent of anything this crate checks) — so this is the
/// one path where "is it actually a regular file" is checked by our own code, not handed to us for
/// free by FIFO write semantics.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn read_rejects_fifo_content_despite_the_os_permitting_the_open() {
    use crate::test_gates::test_support::create_fifo_for_test;

    let path = unique_temp_dir("fifo-read-rejection");
    let fifo = path.join("entry");
    assert!(create_fifo_for_test(&fifo, 0o600).is_ok());
    let root = mutation_root(&path);
    assert!(read_file_if_exists(&root, Path::new("entry")).is_err());
    let _ = fs::remove_file(fifo);
    let _ = fs::remove_dir_all(path);
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
#[test]
fn unsupported_mutation_fails_before_filesystem_side_effect() {
    let path = unique_temp_dir("unsupported-mutation");
    let root = mutation_root(&path);
    assert!(write_file_atomically(&root, Path::new("state"), b"candidate").is_err());
    assert!(!path.join("state").exists());
    let _ = fs::remove_dir_all(path);
}

/// DC-81 addendum-1 §3: "Q3's measurement is follow-through, not optional" once the macOS CI job
/// runs mutation. `#[ignore]`d by default — this is a data point for NFR-PERF-01, not a pass/fail
/// gate (there is no accepted threshold to assert against, and timing varies by runner) — the CI job
/// runs it explicitly with `--ignored --nocapture` so the numbers land in the log. Compares raw
/// `fsync` against `fcntl_fullfsync` directly, bypassing `MacosDurability` (which always uses the
/// latter for directory syncs, per `AnchoredDirectory::sync`): the point is measuring the primitive
/// gap `contract.rs`'s G3 doc comment describes, not exercising the contract itself.
#[cfg(target_os = "macos")]
#[test]
#[ignore = "prints timing data for NFR-PERF-01; run explicitly with --ignored --nocapture"]
fn measure_directory_sync_fsync_vs_fcntl_fullfsync() {
    use std::time::Instant;

    use rustix::fs::{Mode, OFlags};

    const ITERATIONS: u32 = 50;
    let path = unique_temp_dir("macos-sync-benchmark");
    let fd = match rustix::fs::open(&path, OFlags::RDONLY | OFlags::DIRECTORY, Mode::empty()) {
        Ok(fd) => fd,
        Err(error) => panic!("open benchmark directory failed: {error}"),
    };

    let started = Instant::now();
    for _ in 0..ITERATIONS {
        if let Err(error) = rustix::fs::fsync(&fd) {
            panic!("fsync failed: {error}");
        }
    }
    let fsync_elapsed = started.elapsed();

    let started = Instant::now();
    for _ in 0..ITERATIONS {
        if let Err(error) = rustix::fs::fcntl_fullfsync(&fd) {
            panic!("fcntl_fullfsync failed: {error}");
        }
    }
    let fullfsync_elapsed = started.elapsed();

    let ratio = fullfsync_elapsed.as_secs_f64() / fsync_elapsed.as_secs_f64().max(f64::EPSILON);
    println!(
        "NFR-PERF-01 data point: {ITERATIONS} directory syncs on this runner \u{2014} \
         fsync {fsync_elapsed:?}, fcntl_fullfsync {fullfsync_elapsed:?} (ratio {ratio:.2}x)"
    );

    let _ = fs::remove_dir_all(path);
}

/// DC-82 §3: "mutation fails at runtime, not a compile error" for the implementor-less case, proven
/// by *running* the code, not by cross-target `clippy`/`build` alone (which prove compilation, never
/// runtime behaviour). `none::NoDurability` is visible here under its `#[cfg(any(test, ...))]` gate
/// on Linux and macOS -- **not on Windows** (DC-97 correction: an earlier version of this comment
/// said "regardless of host platform," which stopped being true once Windows got a real
/// `WindowsDurability` implementor; `anchored.rs`'s own `NoDurability` gate excludes Windows in test
/// builds specifically because Windows no longer needs an implementor-less fallback to test). This
/// test constructs it directly and calls every trait method, which the compiler accepts (proving no
/// compile-time obstacle) and which each return `Err` at runtime with the exact message the
/// pre-DC-82 `#[cfg(not(any(...)))]` fallback arms used to construct inline. The production dispatch
/// (`ACTIVE_DURABILITY`) is untouched by this test — on this host it still resolves to
/// `LinuxDurability`/`MacosDurability`, never `NoDurability`.
#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn no_durability_every_method_fails_at_runtime_not_compile_time() {
    use crate::foundation::fsutil::{DurabilityContract, NoDurability};

    const EXPECTED: &str = "i/o error: repository mutation requires Linux, macOS, or Windows root-scoped filesystem capabilities";

    let path = unique_temp_dir("no-durability-runtime-check");
    let root = mutation_root(&path);
    let relative = Path::new("state");

    let results = [
        NoDurability
            .atomic_replace(&root, relative, b"x")
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .durable_append(&root, relative, b"x")
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .durable_truncate(&root, relative, 0)
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .durable_truncate_to_empty(&root, relative)
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .set_permission_bits(&root, relative, 0o644)
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .remove_if_present(&root, relative)
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .ensure_directory(&root, relative)
            .err()
            .map(|error| error.to_string()),
        NoDurability
            .durable_directory_entry(&root, relative)
            .err()
            .map(|error| error.to_string()),
    ];
    for (index, result) in results.iter().enumerate() {
        assert_eq!(
            result.as_deref(),
            Some(EXPECTED),
            "method at index {index} did not fail with the expected runtime error"
        );
    }

    let create_exclusive_error = match NoDurability.create_exclusive(&root, relative, b"x") {
        Ok(()) => panic!("create_exclusive must fail at runtime, not compile time"),
        Err(error) => error,
    };
    assert_eq!(
        create_exclusive_error.kind(),
        std::io::ErrorKind::Unsupported
    );
    assert_eq!(
        create_exclusive_error.to_string(),
        "repository mutation requires Linux, macOS, or Windows anchored filesystem primitives"
    );

    let _ = fs::remove_dir_all(path);
}
