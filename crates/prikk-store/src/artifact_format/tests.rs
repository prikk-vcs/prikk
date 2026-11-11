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

// ---- 0.57.0 U2: rule 3, the content half -------------------------------------------------------------------------------

use crate::patch_replay::decode::{
    DecodedDeletePreimage, DecodedOperationKind, DecodedPatchOperation,
};
use prikk_object::{NodeId, NodeKind, ObjectId};

fn operation(kind: DecodedOperationKind) -> DecodedPatchOperation {
    DecodedPatchOperation {
        op_seq: 1,
        kind,
        patch_id: None,
    }
}

fn node() -> NodeId {
    NodeId::from_bytes([7; 32])
}

/// One operation of each kind that names a path, with the non-ASCII path in each position it can occupy -- the rename's
/// *old* path included, since a rule that looked at one of the two would let the other through.
fn operations_naming(path: &str, ascii: &str) -> Vec<(&'static str, DecodedPatchOperation)> {
    vec![
        (
            "CreateFile",
            operation(DecodedOperationKind::CreateFile {
                path: path.to_string(),
                node_id: node(),
                blob_id: ObjectId::from_bytes([1; 32]),
                mode: 0o100_644,
            }),
        ),
        (
            "DeleteNode",
            operation(DecodedOperationKind::DeleteNode {
                path: path.to_string(),
                node_id: node(),
                preimage: DecodedDeletePreimage::File {
                    old_node_kind: NodeKind::BinaryFile,
                    old_blob_id: ObjectId::from_bytes([1; 32]),
                    old_mode: 0o100_644,
                },
            }),
        ),
        (
            "RenamePath (new path)",
            operation(DecodedOperationKind::RenamePath {
                node_id: node(),
                old_path: ascii.to_string(),
                new_path: path.to_string(),
            }),
        ),
        (
            "RenamePath (old path)",
            operation(DecodedOperationKind::RenamePath {
                node_id: node(),
                old_path: path.to_string(),
                new_path: ascii.to_string(),
            }),
        ),
        (
            "CreateSymlink",
            operation(DecodedOperationKind::CreateSymlink {
                path: path.to_string(),
                node_id: node(),
                target: "t".to_string(),
            }),
        ),
    ]
}

#[test]
fn every_operation_kind_that_names_a_path_is_asked_in_every_path_position() {
    let non_ascii = "pages/\u{65e5}\u{672c}.md";
    for kind in [
        ArtifactKind::Bundle,
        ArtifactKind::Archive,
        ArtifactKind::Sync,
    ] {
        for receiver in [RepositoryFormat::CurrentV6, RepositoryFormat::V7] {
            for (label, op) in operations_naming(non_ascii, "plain.md") {
                let err =
                    super::require_receiver_can_hold_paths(kind, &[op], receiver).expect_err(label);
                assert!(
                    matches!(&err, PrikkError::RepositoryFormatTooOld { have, .. } if *have == receiver.number()),
                    "{label}: {err:?}"
                );
                let said = err.to_string();
                assert!(
                    said.contains(non_ascii)
                        && said.contains("prikk format upgrade")
                        && said.contains("nothing was written"),
                    "{label}: {said}"
                );
            }
        }
        // A format-8 receiver holds them all, and an ASCII path is no one's concern.
        for (label, op) in operations_naming(non_ascii, "plain.md") {
            super::require_receiver_can_hold_paths(kind, &[op], RepositoryFormat::V8).expect(label);
        }
        for (label, op) in operations_naming("plain.md", "other.md") {
            super::require_receiver_can_hold_paths(kind, &[op], RepositoryFormat::CurrentV6)
                .expect(label);
        }
    }
    // The node-addressed kinds name no path at all.
    let change_perm = operation(DecodedOperationKind::ChangePerm {
        node_id: node(),
        old_mode: 0o100_644,
        new_mode: 0o100_755,
    });
    super::require_receiver_can_hold_paths(
        ArtifactKind::Bundle,
        &[change_perm],
        RepositoryFormat::CurrentV6,
    )
    .expect("ChangePerm names no path");
    // The sentence says which artifact.
    let said = super::require_receiver_can_hold_paths(
        ArtifactKind::Sync,
        &[operations_naming(non_ascii, "plain.md").remove(0).1],
        RepositoryFormat::V7,
    )
    .expect_err("sync")
    .to_string();
    assert!(
        said.starts_with("precondition not met: this sync artifact carries the non-ASCII path "),
        "{said}"
    );
}
