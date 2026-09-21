#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{destination_exists, write_new_file_durably};
use std::cell::Cell;
use std::path::PathBuf;

thread_local! {
    /// Armed by [`fail_before_rename_once`], read (and disarmed) by `write_new_file_durably`.
    static FAIL_BEFORE_RENAME: Cell<bool> = const { Cell::new(false) };
}

/// Make the next durable write on this thread fail **between the completed temporary file and the
/// rename** -- the state a killed process leaves the filesystem in at its most dangerous moment.
pub(super) fn fail_before_rename_once() {
    FAIL_BEFORE_RENAME.with(|armed| armed.set(true));
}

/// Whether that failure is armed, disarming it. Called from production code under `cfg(test)` only.
pub(super) fn take_failure_before_rename() -> bool {
    FAIL_BEFORE_RENAME.with(Cell::take)
}

/// Every entry in `dir`, so a leftover temporary sibling shows.
fn entries(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// RFC 157 §7.2: a process that dies between the temporary write and the rename leaves **no file at the
/// destination and no temporary sibling** -- the destination is what `--output`'s all-or-nothing promise
/// is about. A real `SIGKILL` at the same instant would leave the temporary sibling behind, since nothing
/// can run a cleanup after it; the destination is absent either way, which is the guarantee.
#[test]
fn a_failure_between_the_temp_write_and_the_rename_leaves_no_file() {
    let dir = unique_dir("failure-before-rename");
    let destination = dir.join("out.bin");

    fail_before_rename_once();
    let result = write_new_file_durably(&destination, b"content that must not appear");

    assert!(result.is_err(), "the armed failure must propagate");
    assert!(
        !destination_exists(&destination),
        "the destination must be absent after a failure before the rename"
    );
    assert_eq!(
        entries(&dir),
        Vec::<String>::new(),
        "no temporary sibling may be left behind"
    );

    // Disarmed: the next write succeeds, so the seam cannot silently poison later writes.
    write_new_file_durably(&destination, b"second").unwrap();
    assert_eq!(std::fs::read(&destination).unwrap(), b"second");

    let _ = std::fs::remove_dir_all(dir);
}

/// The same seam over an **existing** destination: the previous content survives untouched.
#[test]
fn a_failure_before_the_rename_leaves_a_previous_destination_intact() {
    let dir = unique_dir("failure-before-rename-existing");
    let destination = dir.join("out.bin");
    std::fs::write(&destination, b"original").unwrap();

    fail_before_rename_once();
    let result = write_new_file_durably(&destination, b"replacement");

    assert!(result.is_err());
    assert_eq!(std::fs::read(&destination).unwrap(), b"original");
    assert_eq!(entries(&dir), vec!["out.bin".to_string()]);

    let _ = std::fs::remove_dir_all(dir);
}

fn unique_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "prikk-durable-output-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn destination_exists_is_true_for_a_real_file_and_false_for_absence() {
    let dir = unique_dir("exists");
    let present = dir.join("present.bin");
    let absent = dir.join("absent.bin");
    std::fs::write(&present, b"anything").unwrap();

    assert!(destination_exists(&present));
    assert!(!destination_exists(&absent));

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn write_new_file_durably_writes_exact_bytes_to_a_new_destination() {
    let dir = unique_dir("new");
    let destination = dir.join("backup.bundle");

    write_new_file_durably(&destination, b"hello durable world").unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), b"hello durable world");

    let _ = std::fs::remove_dir_all(dir);
}

/// Control 3's "permitted case succeeds" half, at this layer: a second write to the same
/// destination replaces the content -- the collision *policy* lives in `bundle.rs`'s own
/// `--force` check, not here; this function's own job is only ever the write.
#[test]
fn write_new_file_durably_overwrites_existing_content_completely() {
    let dir = unique_dir("overwrite");
    let destination = dir.join("backup.bundle");
    std::fs::write(&destination, b"old content, much longer than the new one").unwrap();

    write_new_file_durably(&destination, b"new").unwrap();

    assert_eq!(std::fs::read(&destination).unwrap(), b"new");

    let _ = std::fs::remove_dir_all(dir);
}

/// Control 1, the decisive one, and control 2 together: an unwritable directory makes
/// `File::create_new` for the temp file fail before a single byte of the attempted new content is
/// written anywhere -- the earliest possible failure point, and the cleanest demonstration that a
/// failure before the rename touches neither the pre-existing destination nor creates any temp
/// file at all. A permission-based failure is `#[cfg(unix)]`-only (Windows ACL semantics differ
/// enough that this project already gates its own permission-bit assertions the same way,
/// `dc67_ordinary_use_conformance.rs`'s DC-71 note) -- not a narrower claim about the production
/// code, only about which failure-injection technique is portable enough to assert on here.
#[cfg(unix)]
#[test]
fn a_failed_write_leaves_the_previous_destination_intact_and_creates_no_temp_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = unique_dir("failed-write-intact");
    let destination = dir.join("backup.bundle");
    std::fs::write(&destination, b"the only real backup, must survive").unwrap();

    let original_mode = std::fs::metadata(&dir).unwrap().permissions().mode();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

    let result = write_new_file_durably(&destination, b"attempted replacement");

    // Restore write permission before any assertion can panic and skip cleanup.
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(original_mode)).unwrap();

    assert!(
        result.is_err(),
        "the write must fail when its own directory refuses new files"
    );
    assert_eq!(
        std::fs::read(&destination).unwrap(),
        b"the only real backup, must survive",
        "the previous destination must be completely untouched by a failed write"
    );
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        entries,
        vec![std::ffi::OsString::from("backup.bundle")],
        "no temp file may exist after a failed write: {entries:?}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

/// Control 2, restated for the no-prior-destination case: a failure whose parent directory does
/// not exist at all leaves nothing at the destination either. The two tests together cover both
/// of control 2's own halves ("neither at the destination nor as an abandoned temp file") in the
/// two distinct starting states a real export can be in: overwriting, and writing fresh.
#[test]
fn a_failed_write_to_a_missing_directory_leaves_nothing_behind() {
    let dir = unique_dir("failed-write-nothing");
    let destination = dir.join("does-not-exist-yet").join("backup.bundle");

    let result = write_new_file_durably(&destination, b"never written");
    assert!(result.is_err());
    assert!(
        !destination_exists(&destination),
        "nothing must appear at the destination"
    );

    let _ = std::fs::remove_dir_all(dir);
}
