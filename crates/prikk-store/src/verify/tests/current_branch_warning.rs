//! 0.50.0 step 1, A4 (019 §5.8): `verify` reads `.prikk/current-branch` and **warns**, never fails,
//! when it cannot be resolved -- malformed, missing, or naming a branch that does not exist or is
//! closed. Exit status is unchanged (the external architect's own condition for accepting this at
//! all): the repository is intact, and every command still works with `--ref` given explicitly;
//! only the default is unusable. `doctor`'s own `PRIKK-DOCTOR-CURRENT-BRANCH` issue already reported
//! this; `verify` previously said nothing at all about it.

use prikk_error::Result;

use crate::test_gates::test_support::unique_temp_dir;
use crate::{RepositoryLayout, verify_repository};

fn write_current_branch(layout: &RepositoryLayout, content: &str) -> Result<()> {
    std::fs::write(layout.current_branch_path(), content)?;
    Ok(())
}

#[test]
fn malformed_current_branch_pointer_is_a_warning_not_a_failure() -> Result<()> {
    let root = unique_temp_dir("a4-current-branch-malformed");
    let layout = RepositoryLayout::init(root)?;
    write_current_branch(&layout, "garbage, not a ref name\n")?;

    let verification = verify_repository(&layout)?;
    assert!(verification.has_current_branch_warning());
    assert!(
        verification
            .current_branch_issue
            .as_ref()
            .is_some_and(|message| message.contains("malformed")),
        "{:?}",
        verification.current_branch_issue
    );
    assert!(!verification.has_stage_failure());
    assert!(!verification.has_item_failure());
    Ok(())
}

#[test]
fn current_branch_naming_a_missing_branch_is_a_warning_not_a_failure() -> Result<()> {
    let root = unique_temp_dir("a4-current-branch-missing-target");
    let layout = RepositoryLayout::init(root)?;
    write_current_branch(&layout, "heads/does-not-exist\n")?;

    let verification = verify_repository(&layout)?;
    assert!(verification.has_current_branch_warning());
    assert!(
        verification
            .current_branch_issue
            .as_ref()
            .is_some_and(|message| message.contains("heads/does-not-exist")),
        "{:?}",
        verification.current_branch_issue
    );
    assert!(!verification.has_stage_failure());
    assert!(!verification.has_item_failure());
    Ok(())
}

/// Control: a repository with no `current-branch` file at all (every repository initialized before
/// RFC 151) resolves to the unborn default (`heads/main`) and must not warn.
///
/// `init` writes the pointer itself (RFC 151 §2.1) -- correctness here depends on removing it
/// first, a fix made alongside Part D1's own absence test, after noticing this test's name was
/// never actually true of what it ran: a present, valid pointer and a genuinely absent one both
/// pass the two assertions below, so this test alone could not have told them apart.
#[test]
fn absent_current_branch_pointer_is_not_a_warning() -> Result<()> {
    let root = unique_temp_dir("a4-current-branch-absent");
    let layout = RepositoryLayout::init(root)?;
    std::fs::remove_file(layout.current_branch_path())?;

    let verification = verify_repository(&layout)?;
    assert!(!verification.has_current_branch_warning());
    assert_eq!(verification.current_branch_issue, None);
    Ok(())
}

/// Part D1: an absent pointer is reported, not silent -- the external architect's own condition
/// kept from Part C ("today its removal is silent" must not stay true), as an informational fact
/// distinct from the warning above.
#[test]
fn absent_current_branch_pointer_is_reported_as_absent() -> Result<()> {
    let root = unique_temp_dir("d1-current-branch-absent-reported");
    let layout = RepositoryLayout::init(root)?;
    // `init` writes the pointer itself (RFC 151 §2.1); remove it to simulate a repository
    // initialized before that line existed -- the only way this handoff's own "absent" state
    // actually arises today.
    std::fs::remove_file(layout.current_branch_path())?;

    let verification = verify_repository(&layout)?;
    assert!(verification.current_branch_absent);
    assert!(!verification.has_current_branch_warning());
    Ok(())
}

/// Control: a present, valid pointer is neither absent nor a warning -- the healthy, ordinary case
/// every other test here assumes but none states directly.
#[test]
fn present_valid_current_branch_pointer_is_neither_absent_nor_a_warning() -> Result<()> {
    let root = unique_temp_dir("d1-current-branch-present-valid");
    let layout = RepositoryLayout::init(root)?;
    write_current_branch(&layout, "heads/main\n")?;

    let verification = verify_repository(&layout)?;
    assert!(!verification.current_branch_absent);
    assert!(!verification.has_current_branch_warning());
    Ok(())
}

/// Control: a malformed pointer is present (not absent) -- the two facts are independent, and a
/// malformed file must never also be read as "not set."
#[test]
fn malformed_current_branch_pointer_is_not_reported_as_absent() -> Result<()> {
    let root = unique_temp_dir("d1-current-branch-malformed-not-absent");
    let layout = RepositoryLayout::init(root)?;
    write_current_branch(&layout, "garbage, not a ref name\n")?;

    let verification = verify_repository(&layout)?;
    assert!(!verification.current_branch_absent);
    assert!(verification.has_current_branch_warning());
    Ok(())
}
