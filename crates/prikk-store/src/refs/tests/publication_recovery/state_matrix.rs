//! Declarative persisted-state outcomes required by DC-34/DC-38.

mod fixture;

use crate::{
    ActiveLock, ActiveRefMetadata, ActiveSession, DEFAULT_ACTIVE_NAME, DEFAULT_ACTIVE_PATCH_LIMIT,
    DoctorSeverity, RefStore, Wal, doctor_repository, finish_active_publication_cleanup,
    read_active_ref_metadata, verify_repository,
};
use fixture::{Fixture, PersistedState};

#[derive(Clone, Copy)]
enum VerifyExpectation {
    Issue {
        code: &'static str,
        blocking: bool,
    },
    NoRefIssue,
    /// DC-95 Stage 2 Level 2 (refs half): what was a whole-`Refs`-stage `Failed` outcome under
    /// Level 1 is now contained one level further, to a single ref's own pointer file, log file,
    /// or classification -- `verify_repository` returns `Ok` and the `Refs` `StageOutcome` itself
    /// reads `Evaluated`. Checks all three item-outcome buckets, matching `verify/tests.rs`'s own
    /// `assert_ref_failed` (which the fixture in this module could not reuse directly since it
    /// asserts against `verify_repository`'s return value inline, not through that helper). No
    /// remaining `CASES` row exercises the previous, whole-stage form of this expectation --
    /// item containment covers every case that used to need it.
    RefItemFailure {
        message_substring: &'static str,
    },
}

#[derive(Clone, Copy)]
enum RetryExpectation {
    Completes { log_records: usize },
    Refuses,
}

#[derive(Clone, Copy)]
struct StateCase {
    state: PersistedState,
    verify: VerifyExpectation,
    doctor_code: &'static str,
    doctor_severity: DoctorSeverity,
    recommendation: &'static str,
    retry: RetryExpectation,
    mutation_succeeds: bool,
}

const CASES: &[StateCase] = &[
    StateCase {
        state: PersistedState::Candidate,
        verify: VerifyExpectation::Issue {
            code: "PRIKK-VERIFY-REF-CANDIDATE-DEBRIS",
            blocking: false,
        },
        doctor_code: "PRIKK-VERIFY-REF-CANDIDATE-DEBRIS",
        doctor_severity: DoctorSeverity::Warning,
        recommendation: "preserve the candidate",
        retry: RetryExpectation::Completes { log_records: 1 },
        mutation_succeeds: false,
    },
    StateCase {
        state: PersistedState::PointerLeading,
        verify: VerifyExpectation::Issue {
            code: "PRIKK-VERIFY-REF-POINTER-LEADS-LOG",
            blocking: true,
        },
        doctor_code: "PRIKK-VERIFY-REF-POINTER-LEADS-LOG",
        doctor_severity: DoctorSeverity::Error,
        recommendation: "signer-backed",
        retry: RetryExpectation::Completes { log_records: 1 },
        mutation_succeeds: false,
    },
    StateCase {
        state: PersistedState::PartialTail,
        verify: VerifyExpectation::Issue {
            code: "PRIKK-VERIFY-REF-POINTER-LEADS-LOG",
            blocking: true,
        },
        doctor_code: "PRIKK-VERIFY-REF-POINTER-LEADS-LOG",
        doctor_severity: DoctorSeverity::Error,
        recommendation: "signer-backed",
        retry: RetryExpectation::Completes { log_records: 1 },
        mutation_succeeds: false,
    },
    StateCase {
        state: PersistedState::CompleteCleanup,
        verify: VerifyExpectation::Issue {
            code: "PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING",
            blocking: true,
        },
        doctor_code: "PRIKK-VERIFY-REF-ACTIVE-CLEANUP-PENDING",
        doctor_severity: DoctorSeverity::Error,
        recommendation: "signer-backed",
        retry: RetryExpectation::Completes { log_records: 1 },
        mutation_succeeds: false,
    },
    StateCase {
        state: PersistedState::LegacyLogLeading,
        verify: VerifyExpectation::Issue {
            code: "PRIKK-VERIFY-REF-DIVERGENCE",
            blocking: true,
        },
        doctor_code: "PRIKK-VERIFY-REF-DIVERGENCE",
        doctor_severity: DoctorSeverity::Error,
        recommendation: "preserve the repository",
        retry: RetryExpectation::Refuses,
        mutation_succeeds: false,
    },
    StateCase {
        state: PersistedState::Divergence,
        verify: VerifyExpectation::RefItemFailure {
            message_substring: "ref-log chain or sequence diverges",
        },
        doctor_code: "PRIKK-DOCTOR-VERIFY-REF-FILE-INCOMPLETE",
        doctor_severity: DoctorSeverity::Error,
        recommendation: "preserve the repository",
        retry: RetryExpectation::Refuses,
        mutation_succeeds: false,
    },
    StateCase {
        state: PersistedState::EmptyWalMetadata,
        verify: VerifyExpectation::NoRefIssue,
        doctor_code: "PRIKK-DOCTOR-ACTIVE-REF-METADATA-DEBRIS",
        doctor_severity: DoctorSeverity::Warning,
        recommendation: "no repair is required",
        retry: RetryExpectation::Refuses,
        mutation_succeeds: true,
    },
];

#[test]
fn every_state_has_explicit_verify_and_doctor_read_only_outcomes() -> prikk_error::Result<()> {
    for case in CASES {
        let fixture = Fixture::new(case.state)?;
        let before = fixture.state_bytes()?;
        match (verify_repository(&fixture.layout), case.verify) {
            (Ok(report), VerifyExpectation::Issue { code, blocking }) => {
                assert!(
                    report
                        .ref_publication_issues
                        .iter()
                        .any(|issue| { issue.code == code && issue.blocking == blocking })
                );
            }
            (Ok(report), VerifyExpectation::NoRefIssue) => {
                assert!(report.ref_publication_issues.is_empty());
                assert!(report.has_active_wal_metadata_warning());
            }
            (Ok(report), VerifyExpectation::RefItemFailure { message_substring }) => {
                assert!(report.has_item_failure());
                let found = report
                    .pointer_outcomes
                    .iter()
                    .chain(&report.log_outcomes)
                    .any(|outcome| {
                        matches!(&outcome.status, crate::RefFileStatus::Failed { message } if message.contains(message_substring))
                    })
                    || report.ref_item_outcomes.iter().any(|outcome| {
                        matches!(&outcome.status, crate::RefItemStatus::Failed { message } if message.contains(message_substring))
                    });
                assert!(
                    found,
                    "expected a ref outcome Failed containing {message_substring:?} for {:?}, got: \
                     pointer_outcomes={:?} log_outcomes={:?} ref_item_outcomes={:?}",
                    case.state,
                    report.pointer_outcomes,
                    report.log_outcomes,
                    report.ref_item_outcomes
                );
            }
            (result, _) => panic!("unexpected verify result for {:?}: {result:?}", case.state),
        }
        let doctor = doctor_repository(&fixture.layout);
        let issue = doctor
            .issues
            .iter()
            .find(|issue| issue.code == case.doctor_code)
            .ok_or_else(|| {
                prikk_error::PrikkError::Integrity(format!(
                    "missing doctor issue {} for {:?}",
                    case.doctor_code, case.state
                ))
            })?;
        assert_eq!(issue.severity, case.doctor_severity);
        assert!(issue.recommendation.contains(case.recommendation));
        assert_eq!(fixture.state_bytes()?, before);
        fixture.remove();
    }
    Ok(())
}

#[test]
fn every_state_has_explicit_production_retry_and_exact_post_state() -> prikk_error::Result<()> {
    for case in CASES {
        let fixture = Fixture::new(case.state)?;
        let before = fixture.state_bytes()?;
        let active_lock = ActiveLock::acquire(&fixture.layout, DEFAULT_ACTIVE_NAME)?;
        let store = RefStore::new(fixture.layout.clone());
        let first = store.finish_interrupted_publication(&active_lock, &fixture.publication);
        let second = store.finish_interrupted_publication(&active_lock, &fixture.publication);
        match case.retry {
            RetryExpectation::Completes { log_records } => {
                assert_eq!(first?, fixture.publication.ref_state.object_id());
                assert_eq!(second?, fixture.publication.ref_state.object_id());
                assert_eq!(store.replay_log("heads/main")?.records.len(), log_records);
                finish_active_publication_cleanup(&fixture.layout, &active_lock)?;
                assert!(
                    Wal::for_layout(&fixture.layout, DEFAULT_ACTIVE_NAME)
                        .replay()?
                        .records
                        .is_empty()
                );
                assert_eq!(
                    read_active_ref_metadata(&fixture.layout)?,
                    ActiveRefMetadata::Missing
                );
                let report = verify_repository(&fixture.layout)?;
                // RFC 102 Stage 4: `Candidate` debris under `refs/tmp/` is never cleaned by
                // anything anymore -- the candidate-write-then-promote mechanism whose retry path
                // used to sweep it as a side effect of finishing *any* publish is gone entirely.
                // The non-blocking `PRIKK-VERIFY-REF-CANDIDATE-DEBRIS` issue this leaves behind
                // forever is the registered FINDINGS.md wedge (design-v1.md §13.9); this is not a
                // regression to paper over here -- `ensure_no_incomplete_publication`'s own
                // semantics are explicitly out of scope for Stage 4.
                if matches!(case.state, PersistedState::Candidate) {
                    assert_eq!(report.ref_publication_issues.len(), 1);
                    assert_eq!(
                        report
                            .ref_publication_issues
                            .first()
                            .map(|issue| issue.code),
                        Some("PRIKK-VERIFY-REF-CANDIDATE-DEBRIS")
                    );
                } else {
                    assert!(report.ref_publication_issues.is_empty());
                }
                assert!(report.publication_trust_issues.is_empty());
            }
            RetryExpectation::Refuses => {
                assert!(first.is_err());
                assert!(second.is_err());
                assert_eq!(fixture.state_bytes()?, before);
            }
        }
        drop(active_lock);
        fixture.remove();
    }
    Ok(())
}

#[test]
fn every_state_has_explicit_representative_command_mutation_outcome() -> prikk_error::Result<()> {
    for case in CASES {
        let fixture = Fixture::new(case.state)?;
        let before = fixture.state_bytes()?;
        let result = ActiveSession::new(fixture.layout.clone()).append_patch(
            &crate::test_gates::test_support::signed_patch_envelope(),
            DEFAULT_ACTIVE_PATCH_LIMIT,
        );
        assert_eq!(result.is_ok(), case.mutation_succeeds);
        if case.mutation_succeeds {
            assert_eq!(
                Wal::for_layout(&fixture.layout, DEFAULT_ACTIVE_NAME)
                    .replay()?
                    .records
                    .len(),
                1
            );
            assert_eq!(
                read_active_ref_metadata(&fixture.layout)?,
                ActiveRefMetadata::Valid("heads/main".to_string())
            );
        } else {
            assert_eq!(fixture.state_bytes()?, before);
        }
        fixture.remove();
    }
    Ok(())
}
