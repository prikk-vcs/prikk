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
#[test]
fn absent_current_branch_pointer_is_not_a_warning() -> Result<()> {
    let root = unique_temp_dir("a4-current-branch-absent");
    let layout = RepositoryLayout::init(root)?;

    let verification = verify_repository(&layout)?;
    assert!(!verification.has_current_branch_warning());
    assert_eq!(verification.current_branch_issue, None);
    Ok(())
}
