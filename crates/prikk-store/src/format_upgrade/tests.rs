//! RFC 156 §5b: `prikk format upgrade` at the store level.

use std::path::Path;

use prikk_error::PrikkError;

use super::{
    FormatUpgradeOutcome, before_marker_write_for_test, upgrade_repository_format,
    upgrade_repository_format_in,
};
use crate::foundation::layout::{DEFAULT_ACTIVE_NAME, RepositoryFormat, RepositoryLayout};
use crate::lock::ActiveLock;
use crate::test_gates::test_support::{repository_bytes, unique_temp_dir};

/// A fresh repository with its marker set to format 6. A newly initialized repository holds nothing a
/// format-6 repository could not, so this is a genuine format-6 repository, not a forged one.
fn format_6_repository(tag: &str) -> prikk_error::Result<(std::path::PathBuf, RepositoryLayout)> {
    let root = unique_temp_dir(tag);
    RepositoryLayout::init(root.clone())?;
    std::fs::write(root.join(".prikk").join("FORMAT"), b"6\n")?;
    let layout = RepositoryLayout::open(root.clone())?;
    assert_eq!(layout.format(), RepositoryFormat::CurrentV6);
    Ok((root, layout))
}

/// A fresh repository with its marker set to format 7 -- genuine for the same reason as above: a new repository holds
/// nothing a format-7 repository could not.
fn format_7_repository(tag: &str) -> prikk_error::Result<(std::path::PathBuf, RepositoryLayout)> {
    let root = unique_temp_dir(tag);
    RepositoryLayout::init(root.clone())?;
    std::fs::write(root.join(".prikk").join("FORMAT"), b"7\n")?;
    let layout = RepositoryLayout::open(root.clone())?;
    assert_eq!(layout.format(), RepositoryFormat::V7);
    Ok((root, layout))
}

fn marker(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(".prikk").join("FORMAT")).unwrap_or_default()
}

fn passes(_: &crate::verify::RepositoryVerification) -> std::result::Result<(), String> {
    Ok(())
}

#[test]
fn a_verified_format_6_repository_is_upgraded_in_place() -> prikk_error::Result<()> {
    let (root, layout) = format_6_repository("format-upgrade-ok")?;
    assert_eq!(
        upgrade_repository_format(&layout, passes)?,
        FormatUpgradeOutcome::Upgraded { from: 6, to: 8 }
    );
    assert_eq!(marker(&root), b"8\n");
    assert_eq!(
        RepositoryLayout::open(root.clone())?.format(),
        RepositoryFormat::V8
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// 0.57.0 U1: the 7 -> 8 step. The marker's first byte is overwritten, and the repository opens at format 8 (which, unlike 7,
/// admits a non-ASCII path to author). Nothing else is written.
#[test]
fn a_verified_format_7_repository_is_upgraded_in_place_to_format_8() -> prikk_error::Result<()> {
    let (root, layout) = format_7_repository("format-upgrade-7-to-8")?;
    assert!(!layout.format().admits_non_ascii_paths());
    let before = repository_bytes(&layout)?;
    assert_eq!(
        upgrade_repository_format(&layout, passes)?,
        FormatUpgradeOutcome::Upgraded { from: 7, to: 8 }
    );
    assert_eq!(marker(&root), b"8\n");
    let upgraded = RepositoryLayout::open(root.clone())?;
    assert_eq!(upgraded.format(), RepositoryFormat::V8);
    assert!(upgraded.format().admits_non_ascii_paths());
    // Everything but the marker is byte for byte what it was.
    let mut after = repository_bytes(&upgraded)?;
    let mut expected = before;
    let marker_path = root.join(".prikk").join("FORMAT");
    assert_eq!(after.remove(&marker_path), Some(b"8\n".to_vec()));
    assert_eq!(expected.remove(&marker_path), Some(b"7\n".to_vec()));
    assert_eq!(after, expected, "nothing but the marker changed");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Control 4, first half: idempotent — a second upgrade reports the current format and writes nothing.
#[test]
fn upgrading_a_format_8_repository_changes_nothing() -> prikk_error::Result<()> {
    let (root, layout) = format_6_repository("format-upgrade-idempotent")?;
    upgrade_repository_format(&layout, passes)?;
    let before = repository_bytes(&RepositoryLayout::open(root.clone())?)?;
    assert_eq!(
        upgrade_repository_format(&layout, passes)?,
        FormatUpgradeOutcome::AlreadyCurrent { format: 8 }
    );
    assert_eq!(
        repository_bytes(&RepositoryLayout::open(root.clone())?)?,
        before
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Control 4, second half: refused while another writer holds the lock, and nothing is written.
#[test]
fn an_upgrade_is_refused_while_another_writer_holds_the_lock() -> prikk_error::Result<()> {
    let (root, layout) = format_6_repository("format-upgrade-locked")?;
    let before = repository_bytes(&layout)?;
    let held = ActiveLock::acquire(&layout, DEFAULT_ACTIVE_NAME)?;
    let refused = upgrade_repository_format(&layout, passes);
    assert!(
        matches!(refused, Err(PrikkError::LockConflict(_))),
        "{refused:?}"
    );
    drop(held);
    assert_eq!(repository_bytes(&layout)?, before);
    assert_eq!(marker(&root), b"6\n");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Control 3, store half: a verification that does not pass refuses the upgrade, writing nothing.
#[test]
fn an_upgrade_is_refused_when_verification_does_not_pass() -> prikk_error::Result<()> {
    let (root, layout) = format_6_repository("format-upgrade-unverified")?;
    let before = repository_bytes(&layout)?;
    let refused = upgrade_repository_format(&layout, |_| Err("item-failure".to_string()));
    assert!(
        matches!(refused, Err(PrikkError::Precondition(ref message)) if message.contains("item-failure") && message.contains("nothing was changed")),
        "{refused:?}"
    );
    assert_eq!(repository_bytes(&layout)?, before);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// The lock is held from verification through the marker write: a writer arriving in between is
/// refused.
#[test]
fn the_upgrade_holds_its_lock_through_the_marker_write() -> prikk_error::Result<()> {
    let (root, layout) = format_6_repository("format-upgrade-lock-held")?;
    let concurrent = layout.clone();
    let observed = std::rc::Rc::new(std::cell::Cell::new(false));
    let seen = std::rc::Rc::clone(&observed);
    before_marker_write_for_test(move || {
        seen.set(matches!(
            ActiveLock::acquire(&concurrent, DEFAULT_ACTIVE_NAME),
            Err(PrikkError::LockConflict(_))
        ));
    });
    upgrade_repository_format(&layout, passes)?;
    assert!(
        observed.get(),
        "a writer must be refused while the upgrade runs"
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// RFC 168 §3.3, torn at the marker's one-byte overwrite: the write does not happen, so the marker still reads format 6 and
/// the repository opens as format 6.
#[test]
fn a_format_marker_write_torn_before_its_byte_leaves_the_marker_at_six() -> prikk_error::Result<()>
{
    use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
    let (root, layout) = format_6_repository("format-torn-marker")?;
    fail_once_for_test(TestFailPoint::InPlaceWrite);
    let torn = upgrade_repository_format(&layout, passes);
    clear_failpoint_for_test();
    assert!(torn.is_err(), "the injected failure must fire");
    assert_eq!(marker(&root), b"6\n", "the marker still reads format 6");
    assert_eq!(
        RepositoryLayout::open(root.clone())?.format(),
        RepositoryFormat::CurrentV6
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// 0.59.0 step K4: a repository whose marker is lost (absent or empty) and that holds more than a killed `init` leaves. The stray file is what makes it
/// populated; `verify` ignores it, so the repository verifies.
fn populated_with_a_lost_marker(tag: &str, empty: bool) -> prikk_error::Result<std::path::PathBuf> {
    let root = unique_temp_dir(tag);
    RepositoryLayout::init(root.clone())?;
    std::fs::write(root.join(".prikk").join("stray"), b"history")?;
    std::fs::remove_file(root.join(".prikk").join("FORMAT"))?;
    if empty {
        std::fs::write(root.join(".prikk").join("FORMAT"), b"")?;
    }
    Ok(root)
}

fn without_the_marker(
    files: std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
) -> std::collections::BTreeMap<std::path::PathBuf, Vec<u8>> {
    files
        .into_iter()
        .filter(|(path, _)| !path.ends_with("FORMAT"))
        .collect()
}

#[test]
fn a_lost_marker_on_a_populated_repository_is_written_after_it_verifies() -> prikk_error::Result<()>
{
    for empty in [false, true] {
        let root = populated_with_a_lost_marker("k4-restore-marker", empty)?;
        let before = without_the_marker(repository_bytes_of(&root)?);
        assert_eq!(
            upgrade_repository_format_in(root.clone(), passes)?,
            FormatUpgradeOutcome::MarkerRestored { to: 8 },
            "empty marker: {empty}"
        );
        assert_eq!(marker(&root), b"8\n");
        assert_eq!(
            RepositoryLayout::open(root.clone())?.format(),
            RepositoryFormat::V8
        );
        assert_eq!(
            without_the_marker(repository_bytes_of(&root)?),
            before,
            "nothing but the marker changed"
        );
        // Run again: the marker is there, so it is the ordinary idempotent answer.
        assert_eq!(
            upgrade_repository_format_in(root.clone(), passes)?,
            FormatUpgradeOutcome::AlreadyCurrent { format: 8 }
        );
        let _ = std::fs::remove_dir_all(root);
    }
    Ok(())
}

fn repository_bytes_of(
    root: &Path,
) -> prikk_error::Result<std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>> {
    // `RepositoryLayout::open` refuses a lost marker, so walk the files directly.
    let mut files = std::collections::BTreeMap::new();
    let mut stack = vec![root.join(".prikk")];
    while let Some(dir) = stack.pop() {
        for entry in std::fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                stack.push(path);
            } else if !path.to_string_lossy().contains("lock") {
                files.insert(path.clone(), std::fs::read(&path)?);
            }
        }
    }
    Ok(files)
}

#[test]
fn a_lost_marker_whose_verification_fails_writes_nothing_and_names_a_way_out()
-> prikk_error::Result<()> {
    let root = populated_with_a_lost_marker("k4-restore-refused", false)?;
    let before = repository_bytes_of(&root)?;
    let refused = upgrade_repository_format_in(root.clone(), |_| Err("a-finding".to_string()));
    let Err(PrikkError::Precondition(sentence)) = refused else {
        panic!("{refused:?}");
    };
    assert!(sentence.contains("a-finding"), "{sentence}");
    assert!(
        sentence.contains(crate::foundation::layout::SOUND_COPY),
        "{sentence}"
    );
    assert!(
        !sentence.contains("run `prikk verify`"),
        "verify cannot run without the marker: {sentence}"
    );
    assert_eq!(repository_bytes_of(&root)?, before);
    assert!(!root.join(".prikk").join("FORMAT").exists());
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_lost_marker_on_an_unpopulated_directory_names_init_and_writes_nothing()
-> prikk_error::Result<()> {
    let root = unique_temp_dir("k4-restore-unpopulated");
    RepositoryLayout::init(root.clone())?;
    std::fs::remove_file(root.join(".prikk").join("FORMAT"))?;
    let before = repository_bytes_of(&root)?;
    let refused = upgrade_repository_format_in(root.clone(), passes);
    let Err(PrikkError::Precondition(sentence)) = refused else {
        panic!("{refused:?}");
    };
    assert!(sentence.contains("`prikk init` finishes it"), "{sentence}");
    assert_eq!(repository_bytes_of(&root)?, before);
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn a_kill_in_the_restored_markers_write_leaves_it_lost_and_a_run_again_finishes()
-> prikk_error::Result<()> {
    use crate::foundation::fsutil::{TestFailPoint, clear_failpoint_for_test, fail_once_for_test};
    let root = populated_with_a_lost_marker("k4-restore-torn", true)?;
    fail_once_for_test(TestFailPoint::MutableRename);
    let torn = upgrade_repository_format_in(root.clone(), passes);
    clear_failpoint_for_test();
    assert!(torn.is_err(), "the injected failure must fire");
    assert_eq!(
        marker(&root),
        b"",
        "the marker is as it was: empty, not a part of format 8"
    );
    assert_eq!(
        upgrade_repository_format_in(root.clone(), passes)?,
        FormatUpgradeOutcome::MarkerRestored { to: 8 }
    );
    assert_eq!(marker(&root), b"8\n");
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
