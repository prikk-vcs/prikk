//! The two rules of `artifact_format`, as a table: declared format x receiving format.

#![allow(clippy::expect_used)]

use super::{
    ArtifactKind, known_repository_format_numbers, require_known_declared_format,
    require_receiver_can_hold,
};
use crate::foundation::layout::RepositoryFormat;
use prikk_error::PrikkError;

const RECEIVERS: [RepositoryFormat; 3] = [
    RepositoryFormat::CurrentV6,
    RepositoryFormat::V7,
    RepositoryFormat::V8,
];

#[test]
fn the_known_numbers_are_exactly_six_seven_and_eight() {
    assert_eq!(known_repository_format_numbers(), [6, 7, 8]);
}

#[test]
fn an_unknown_number_is_refused_naming_it_for_either_artifact() {
    for kind in [ArtifactKind::Archive, ArtifactKind::Bundle] {
        for declared in [0, 5, 9, 99, u32::MAX] {
            let err = require_known_declared_format(kind, declared).expect_err("unknown");
            let said = err.to_string();
            assert!(
                said.contains(&format!(
                    "declares repository format {declared}, which this build does not know"
                )),
                "{said}"
            );
            assert!(
                matches!(&err, PrikkError::UnknownArtifactFormat { declared: seen, .. } if *seen == declared),
                "{err:?}"
            );
            assert_eq!(
                err.decode_failure_kind(),
                prikk_error::DecodeFailureKind::OutsideVersionRule,
                "a number this build does not know is outside its rules, never damage: {err:?}"
            );
            assert!(
                !said.contains("malformed"),
                "the sentence must not read as damage: {said}"
            );
            assert!(said.contains("not damage"), "{said}");
        }
        for declared in [6, 7, 8] {
            require_known_declared_format(kind, declared).expect("known");
        }
    }
}

/// The archive's sentence changed in U4b: it was `malformed persisted data: ...`, a damage-shaped class, for a file
/// whose checksum was sound and that a different version of prikk may have written.
#[test]
fn the_archive_sentence_no_longer_reads_as_damage() {
    let said = require_known_declared_format(ArtifactKind::Archive, 9)
        .expect_err("unknown")
        .to_string();
    assert!(
        said.starts_with(
            "archive declares repository format 9, which this build does not know how to read;"
        ),
        "{said}"
    );
    assert!(said.ends_with("-- refusing the whole archive"), "{said}");
    assert!(!said.contains("malformed persisted data"), "{said}");
}

/// declared x receiver. Only a format-8 declaration into a receiver that cannot hold non-ASCII is refused.
#[test]
fn only_a_format_eight_artifact_into_an_older_receiver_is_refused() {
    for declared in [6u32, 7, 8] {
        for receiver in RECEIVERS {
            let result = require_receiver_can_hold(ArtifactKind::Bundle, declared, receiver);
            let must_refuse = declared == 8 && receiver != RepositoryFormat::V8;
            assert_eq!(
                result.is_err(),
                must_refuse,
                "declared {declared} into format {}",
                receiver.number()
            );
        }
    }
}

#[test]
fn the_refusal_names_both_formats_the_command_and_the_cost() {
    for (receiver, number) in [(RepositoryFormat::CurrentV6, 6), (RepositoryFormat::V7, 7)] {
        let err = require_receiver_can_hold(ArtifactKind::Archive, 8, receiver).expect_err("older");
        let said = err.to_string();
        assert!(
            matches!(
                &err,
                PrikkError::RepositoryFormatTooOld { artifact: "archive", declared: 8, have, .. } if *have == number
            ),
            "{err:?}"
        );
        assert!(said.starts_with("precondition not met: "), "{said}");
        assert!(
            said.contains("this archive declares repository format 8"),
            "{said}"
        );
        assert!(
            said.contains(&format!("this repository is format {number}")),
            "{said}"
        );
        assert!(
            said.contains("run `prikk format upgrade`")
                || said.contains("Run `prikk format upgrade`"),
            "{said}"
        );
        assert!(
            said.contains("a prikk older than this one cannot open a format-8 repository"),
            "{said}"
        );
        assert!(said.contains("nothing was written"), "{said}");
    }
}
