//! RFC 156 §5b: `prikk format upgrade` at the store level.

use std::path::Path;

use prikk_error::PrikkError;

use super::{FormatUpgradeOutcome, before_marker_write_for_test, upgrade_repository_format};
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
        FormatUpgradeOutcome::Upgraded { from: 6, to: 7 }
    );
    assert_eq!(marker(&root), b"7\n");
    assert_eq!(
        RepositoryLayout::open(root.clone())?.format(),
        RepositoryFormat::V7
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

/// Control 4, first half: idempotent — a second upgrade reports the current format and writes nothing.
#[test]
fn upgrading_a_format_7_repository_changes_nothing() -> prikk_error::Result<()> {
    let (root, layout) = format_6_repository("format-upgrade-idempotent")?;
    upgrade_repository_format(&layout, passes)?;
    let before = repository_bytes(&RepositoryLayout::open(root.clone())?)?;
    assert_eq!(
        upgrade_repository_format(&layout, passes)?,
        FormatUpgradeOutcome::AlreadyCurrent { format: 7 }
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
