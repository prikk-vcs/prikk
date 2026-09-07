//! Candidate cleanup recognizes only atomic-write names generated for the target ref.

use super::root_publication;
use crate::test_gates::test_support::unique_temp_dir;
use crate::{RefStore, RepositoryLayout, verify_repository};

#[test]
fn malformed_same_prefix_candidate_remains_visible_through_retry() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-malformed-candidate-temp");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let candidate = layout.ref_tmp_path("heads/main");
    let parent = candidate
        .parent()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("candidate has no parent".to_string()))?;
    let candidate_name = candidate.file_name().ok_or_else(|| {
        prikk_error::PrikkError::Integrity("candidate has no file name".to_string())
    })?;
    std::fs::create_dir_all(parent)?;
    let malformed = candidate.with_file_name(format!(
        "{}.tmp.bad-suffix",
        candidate_name.to_string_lossy()
    ));
    std::fs::write(&malformed, b"preserve")?;

    // RFC 102 Stage 4: `finish_interrupted_publication` no longer inspects `refs/tmp/` at all --
    // the candidate-write-then-promote mechanism it used to police there is gone, and the ref
    // itself has no interrupted state (`root_publication` was never even published), so retry now
    // *succeeds* rather than refusing on the malformed debris the way it used to. What survives
    // from this test's original name is only "remains visible" -- nothing has ever cleared
    // `refs/tmp/` debris since Stage 4, matching or malformed alike (the registered FINDINGS.md
    // wedge, design-v1.md §13.9/§13.10).
    RefStore::new(layout.clone()).publish(&publication)?;
    assert_eq!(std::fs::read(&malformed)?, b"preserve");
    assert!(
        verify_repository(&layout)?
            .ref_publication_issues
            .iter()
            .any(|issue| issue.code == "PRIKK-VERIFY-REF-CANDIDATE-DEBRIS")
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}

#[test]
fn unrelated_candidate_name_is_not_deleted_by_target_retry() -> prikk_error::Result<()> {
    let root = unique_temp_dir("dc38-unrelated-candidate-temp");
    let layout = RepositoryLayout::init(root.clone())?;
    let publication = root_publication(&layout, "heads/main")?;
    let candidate = layout.ref_tmp_path("heads/main");
    let parent = candidate
        .parent()
        .ok_or_else(|| prikk_error::PrikkError::Integrity("candidate has no parent".to_string()))?;
    std::fs::create_dir_all(parent)?;
    let unrelated = parent.join("unrelated.candidate");
    std::fs::write(&unrelated, b"preserve")?;

    RefStore::new(layout.clone()).publish(&publication)?;
    assert_eq!(std::fs::read(&unrelated)?, b"preserve");
    assert!(
        verify_repository(&layout)?
            .ref_publication_issues
            .iter()
            .any(|issue| issue.code == "PRIKK-VERIFY-REF-CANDIDATE-DEBRIS")
    );
    let _ = std::fs::remove_dir_all(root);
    Ok(())
}
