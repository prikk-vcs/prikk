//! RFC 126 §2 property tests for the patch algebra
//! (`algebra-property-tests-handoff-v1.md`).
//!
//! **§1: the RFC's own suggested property cannot fail, so it is not built.** `commutation.rs`
//! only ever returns `CommutationResult::Commutes` after the replay oracle has independently
//! confirmed both orders produce the same [`OracleState`] (`prove_pair_replay`) — "classifier says
//! `Commutes` implies oracle agrees" is therefore an invariant of the code's own control flow, not
//! a property a generator could ever falsify. A proptest asserting that sentence would be green
//! forever and prove nothing.
//!
//! **What is actually untested is the classifier's *conservatism*: `DoesNotCommute` and `Unknown`
//! never consult the oracle at all** — they are returned on [`PairClass`]/[`UnknownReason`] grounds
//! alone, from static analysis. **Property A** (`property_a_classified_sweep_over_refused_pairs`)
//! runs the oracle anyway, for every pair the classifier refuses, and buckets the outcome three
//! ways: the oracle confirms the refusal (states differ — the common, uninteresting case), the
//! oracle cannot even evaluate the pair (at least one order fails to replay at all — the shape
//! every `RenamePath`/`CreateSymlink`/`DeleteSymlink` pair has, since [`replay_oracle`]'s
//! `apply_operation` refuses to replay any of the three kinds unconditionally), or **the oracle
//! says the two orders agree despite the refusal** — the only bucket that can mean anything is
//! wrong, and the only one worth naming per-reason rather than only counting.
//!
//! **Property B** (`property_b_composition_can_disagree_even_when_every_pairwise_check_agrees`)
//! is the one property here whose failure would be a *correctness* finding rather than an
//! *availability* one: it generates operation sequences (not just pairs), and looks for the case
//! `check_confluence`'s own `FinalStateInequality` witness exists to catch — every cross pair
//! between the two sequences commutes, and the two full replay orders still disagree. If the
//! pairwise theory is complete, that set is empty.
//!
//! **Generation is co-derived, not filtered after the fact (§4).** The baseline's *topology* is
//! fixed (two text nodes, one binary node, one symlink node, two fresh-create slots, all at known
//! paths and node ids) and only its *content* (text, blob ids, modes) is generated; every candidate
//! operation is then derived from that same state — an edit's old span comes from the node's own
//! real current text, a delete's preimage from the node's own real blob/mode, a create from the one
//! path/node id no baseline node occupies. This is why the discard rate reported by both properties
//! is at or near zero: nothing here is generated unmoored from a state and then discarded when it
//! fails to apply, the failure mode `operation_kind_strategy()`
//! (`patch_replay/tests/proptest_round_trip.rs`) has and this module deliberately does not reuse
//! wholesale. Fixed topology means this module only needs that file's *leaf-value* strategies
//! (`canonical_mode_strategy`, `object_id_strategy`, `ascii_text_strategy`), not its
//! `node_id_strategy`/`repo_path_strategy` — those two exist there to generate a *varying* topology,
//! which this module deliberately does not have; they were widened to `pub(crate)` alongside the
//! three actually used here, per the handoff's own instruction to reuse rather than re-derive.
//!
//! **A generation pitfall found empirically, not anticipated in the handoff:** `TEXT_ANCHOR_WINDOW`
//! (`text_span.rs`) is 64 bytes each side of an edited span. An early draft generated two short,
//! adjacent words in one small string for the "disjoint same-node edit" case; every such pair
//! turned out anchor-*overlapping*, not disjoint, so the oracle could not evaluate either replay
//! order at all — not the `AgreesEqual` result the RFC's own framing of this bucket implies is
//! reachable. `build_text_a_content` exists because of this: two word slots separated by
//! `FILLER_LEN` (96) bytes of unrelated filler on both sides and between them, so their anchor
//! windows provably never overlap. See `adjacent_same_node_edits_cannot_be_evaluated_even_though_they_look_disjoint`
//! for the short-string case pinned as its own, separate, correct result.

use std::collections::BTreeMap;

use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};

use crate::node::node_lifecycle::{LiveNode, NodeContent};
use crate::patch_replay::decode::{DecodedDeletePreimage, DecodedPatchOperation};
use crate::patch_replay::tests::proptest_round_trip::{
    ascii_text_strategy, canonical_mode_strategy, object_id_strategy,
};
use crate::text_span;

use super::*;

// ---------------------------------------------------------------------------------------------
// Fixed baseline shape: which nodes exist, at which paths, with which node ids. Only their
// *content* (text bytes, blob ids, modes) is generated -- the topology stays fixed so the pool of
// derivable operations (and therefore the classification arms reachable) is known and stable
// across cases, which is what makes the bucket labels below meaningful to assert on.
// ---------------------------------------------------------------------------------------------

const TEXT_A_NODE: u8 = 1;
const TEXT_B_NODE: u8 = 2;
const BINARY_C_NODE: u8 = 3;
const SYMLINK_D_NODE: u8 = 4;
const NEW1_NODE: u8 = 101;
const NEW2_NODE: u8 = 102;

const TEXT_A_PATH: &str = "a.txt";
const TEXT_B_PATH: &str = "b.txt";
const BINARY_C_PATH: &str = "c.bin";
const SYMLINK_D_PATH: &str = "d.link";
const SYMLINK_D_TARGET: &str = "somewhere";
const NEW1_PATH: &str = "new1";
const NEW2_PATH: &str = "new2";
const TEXT_A_RENAMED_PATH: &str = "a-renamed.txt";
const BINARY_C_RENAMED_PATH: &str = "c-renamed.bin";

/// `text_span::TEXT_ANCHOR_WINDOW` is 64 bytes each side of a span -- found empirically while
/// building this property (an earlier draft generated two short, adjacent words in one small
/// string and every "disjoint" edit pair still failed to replay: their anchor windows overlapped,
/// so the second edit's own anchor context was stale after the first ran). `FILLER_LEN` must
/// exceed that window so two edits genuinely do not interfere -- generous margin, not tuned to the
/// exact boundary.
const FILLER_LEN: usize = 96;

fn filler() -> Vec<u8> {
    vec![b'z'; FILLER_LEN]
}

/// `text_a`'s content is built from two independently-editable word slots separated (and bounded)
/// by `filler()` on both sides -- `word1`'s right-anchor window and `word2`'s left-anchor window
/// both land entirely inside the middle filler block, never reaching the other word, so editing
/// one genuinely cannot invalidate the other's anchors. Words are drawn from `[a-m]`, disjoint from
/// filler's `z`, so `locate_text_span`'s occurrence search can never confuse a word for filler or
/// vice versa.
fn build_text_a_content(word1: &[u8], word2: &[u8]) -> Vec<u8> {
    let mut content = filler();
    content.extend_from_slice(word1);
    content.extend_from_slice(&filler());
    content.extend_from_slice(word2);
    content.extend_from_slice(&filler());
    content
}

fn word_strategy() -> impl Strategy<Value = Vec<u8>> {
    "[a-m]{1,6}".prop_map(String::into_bytes)
}

#[derive(Debug, Clone)]
struct BaselineSpec {
    text_a_word1: Vec<u8>,
    text_a_word2: Vec<u8>,
    text_a_mode: u32,
    text_b_content: Vec<u8>,
    text_b_mode: u32,
    binary_c_blob: ObjectId,
    binary_c_mode: u32,
}

impl BaselineSpec {
    fn text_a_content(&self) -> Vec<u8> {
        build_text_a_content(&self.text_a_word1, &self.text_a_word2)
    }
}

/// At least two space-separated "words", used only for `text_b`, which never needs the
/// disjoint-anchor property `text_a` is built for (nothing pairs two edits against it).
fn text_content_strategy() -> impl Strategy<Value = Vec<u8>> {
    proptest::collection::vec("[a-z]{1,6}", 2..=4).prop_map(|words| words.join(" ").into_bytes())
}

fn baseline_spec_strategy() -> impl Strategy<Value = BaselineSpec> {
    (
        word_strategy(),
        word_strategy(),
        canonical_mode_strategy(),
        text_content_strategy(),
        canonical_mode_strategy(),
        object_id_strategy(),
        canonical_mode_strategy(),
    )
        .prop_map(
            |(
                text_a_word1,
                text_a_word2,
                text_a_mode,
                text_b_content,
                text_b_mode,
                binary_c_blob,
                binary_c_mode,
            )| BaselineSpec {
                text_a_word1,
                text_a_word2,
                text_a_mode,
                text_b_content,
                text_b_mode,
                binary_c_blob,
                binary_c_mode,
            },
        )
}

fn flip_mode(mode: u32) -> u32 {
    if mode == MODE_REGULAR {
        MODE_EXECUTABLE
    } else {
        MODE_REGULAR
    }
}

fn build_state(spec: &BaselineSpec) -> NodeLifecycleState {
    let mut state = NodeLifecycleState::new();
    seed_text(
        &mut state,
        node(TEXT_A_NODE),
        TEXT_A_PATH,
        &spec.text_a_content(),
        spec.text_a_mode,
    );
    seed_text(
        &mut state,
        node(TEXT_B_NODE),
        TEXT_B_PATH,
        &spec.text_b_content,
        spec.text_b_mode,
    );
    seed_binary(
        &mut state,
        node(BINARY_C_NODE),
        BINARY_C_PATH,
        spec.binary_c_blob,
        spec.binary_c_mode,
    );
    state
        .seed_live_node(
            node(SYMLINK_D_NODE),
            LiveNode {
                path: path(SYMLINK_D_PATH),
                kind: NodeKind::Symlink,
                content: NodeContent::Symlink {
                    target: SYMLINK_D_TARGET.to_string(),
                },
            },
        )
        .expect("seed symlink");
    state
}

fn build_evidence(spec: &BaselineSpec) -> TestTextResolver {
    TestTextResolver::new([
        (node(TEXT_A_NODE), spec.text_a_content()),
        (node(TEXT_B_NODE), spec.text_b_content.clone()),
    ])
}

// ---------------------------------------------------------------------------------------------
// Candidate operations, derived from the baseline rather than generated unmoored from it.
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum OpChoice {
    EditTextAWord1(Vec<u8>),
    EditTextAWord2(Vec<u8>),
    EditTextB(Vec<u8>),
    ChangePermTextA,
    ChangePermTextB,
    ChangePermBinaryC,
    ReplaceBinaryC(ObjectId),
    DeleteTextA,
    DeleteTextB,
    DeleteBinaryC,
    DeleteSymlinkD,
    RenameTextA,
    RenameBinaryC,
    CreateTextNew1(Vec<u8>),
    CreateBinaryNew1(ObjectId),
    CreateTextNew2(Vec<u8>),
    CreateBinaryNew2(ObjectId),
    CreateSymlinkNew1,
    CreateSymlinkNew2,
    /// Targets `NEW1_NODE`/`NEW2_NODE` -- the same identity a `CreateTextNew1`/`CreateBinaryNew1`
    /// candidate would mint, with `old_mode` matching `CreateFile`'s own fixed `MODE_REGULAR`.
    /// Added after the first sweep showed `PairClass::OrderedDependency` was never reached at all:
    /// every `ChangePerm` candidate targets a pre-existing baseline node, and every `CreateFile`
    /// candidate targets a fresh one, so `classify_create_then_mutate`'s own
    /// "create paired with a later mutation of the *same* newly-minted node" arm -- the class the
    /// hand-written `concrete_ordered_dependency_does_not_commute` test exists to cover -- had no
    /// way to fire from this generator. On its own, alone in a baseline with nothing live at
    /// `NEW1_NODE`/`NEW2_NODE`, this is a plain `LiveStateMismatch` conflict; paired with the
    /// matching create, it becomes the ordered-dependency case.
    ChangePermNew1,
    ChangePermNew2,
}

fn new_text_strategy() -> impl Strategy<Value = Vec<u8>> {
    ascii_text_strategy().prop_map(String::into_bytes)
}

fn op_choice_strategy() -> impl Strategy<Value = OpChoice> {
    prop_oneof![
        word_strategy().prop_map(OpChoice::EditTextAWord1),
        word_strategy().prop_map(OpChoice::EditTextAWord2),
        new_text_strategy().prop_map(OpChoice::EditTextB),
        Just(OpChoice::ChangePermTextA),
        Just(OpChoice::ChangePermTextB),
        Just(OpChoice::ChangePermBinaryC),
        object_id_strategy().prop_map(OpChoice::ReplaceBinaryC),
        Just(OpChoice::DeleteTextA),
        Just(OpChoice::DeleteTextB),
        Just(OpChoice::DeleteBinaryC),
        Just(OpChoice::DeleteSymlinkD),
        Just(OpChoice::RenameTextA),
        Just(OpChoice::RenameBinaryC),
        new_text_strategy().prop_map(OpChoice::CreateTextNew1),
        object_id_strategy().prop_map(OpChoice::CreateBinaryNew1),
        new_text_strategy().prop_map(OpChoice::CreateTextNew2),
        object_id_strategy().prop_map(OpChoice::CreateBinaryNew2),
        Just(OpChoice::CreateSymlinkNew1),
        Just(OpChoice::CreateSymlinkNew2),
        Just(OpChoice::ChangePermNew1),
        Just(OpChoice::ChangePermNew2),
    ]
}

/// `None` means the choice was skipped as degenerate (discarded) -- currently only the two edit
/// choices, when the generated replacement text happens to equal the node's current content
/// (`plan_authored_text_span` returns `None` for an unchanged edit; `edit_text` would otherwise
/// panic on that `None`). Every other choice always builds by construction.
fn build_operation(
    op_seq: u32,
    choice: &OpChoice,
    spec: &BaselineSpec,
) -> Option<DecodedPatchOperation> {
    Some(match choice {
        OpChoice::EditTextAWord1(new_word) => {
            if *new_word == spec.text_a_word1 {
                return None;
            }
            let old_text = spec.text_a_content();
            let new_text = build_text_a_content(new_word, &spec.text_a_word2);
            edit_text(op_seq, node(TEXT_A_NODE), &old_text, &new_text)
        }
        OpChoice::EditTextAWord2(new_word) => {
            if *new_word == spec.text_a_word2 {
                return None;
            }
            let old_text = spec.text_a_content();
            let new_text = build_text_a_content(&spec.text_a_word1, new_word);
            edit_text(op_seq, node(TEXT_A_NODE), &old_text, &new_text)
        }
        OpChoice::EditTextB(new_text) => {
            if *new_text == spec.text_b_content {
                return None;
            }
            edit_text(op_seq, node(TEXT_B_NODE), &spec.text_b_content, new_text)
        }
        OpChoice::ChangePermTextA => change_perm(
            op_seq,
            node(TEXT_A_NODE),
            spec.text_a_mode,
            flip_mode(spec.text_a_mode),
        ),
        OpChoice::ChangePermTextB => change_perm(
            op_seq,
            node(TEXT_B_NODE),
            spec.text_b_mode,
            flip_mode(spec.text_b_mode),
        ),
        OpChoice::ChangePermBinaryC => change_perm(
            op_seq,
            node(BINARY_C_NODE),
            spec.binary_c_mode,
            flip_mode(spec.binary_c_mode),
        ),
        OpChoice::ReplaceBinaryC(new_blob) => {
            replace_binary(op_seq, node(BINARY_C_NODE), spec.binary_c_blob, *new_blob)
        }
        OpChoice::DeleteTextA => delete_file(
            op_seq,
            TEXT_A_PATH,
            node(TEXT_A_NODE),
            NodeKind::TextFile,
            text_span::text_blob_id(&spec.text_a_content()).expect("text blob id"),
            spec.text_a_mode,
        ),
        OpChoice::DeleteTextB => delete_file(
            op_seq,
            TEXT_B_PATH,
            node(TEXT_B_NODE),
            NodeKind::TextFile,
            text_span::text_blob_id(&spec.text_b_content).expect("text blob id"),
            spec.text_b_mode,
        ),
        OpChoice::DeleteBinaryC => delete_file(
            op_seq,
            BINARY_C_PATH,
            node(BINARY_C_NODE),
            NodeKind::BinaryFile,
            spec.binary_c_blob,
            spec.binary_c_mode,
        ),
        OpChoice::DeleteSymlinkD => DecodedPatchOperation {
            op_seq,
            kind: DecodedOperationKind::DeleteNode {
                path: SYMLINK_D_PATH.to_string(),
                node_id: node(SYMLINK_D_NODE),
                preimage: DecodedDeletePreimage::Symlink {
                    old_target: SYMLINK_D_TARGET.to_string(),
                },
            },
        },
        OpChoice::RenameTextA => {
            rename_path(op_seq, node(TEXT_A_NODE), TEXT_A_PATH, TEXT_A_RENAMED_PATH)
        }
        OpChoice::RenameBinaryC => rename_path(
            op_seq,
            node(BINARY_C_NODE),
            BINARY_C_PATH,
            BINARY_C_RENAMED_PATH,
        ),
        OpChoice::CreateTextNew1(text) => create_file(
            op_seq,
            NEW1_PATH,
            node(NEW1_NODE),
            text_span::text_blob_id(text).expect("text blob id"),
            MODE_REGULAR,
        ),
        OpChoice::CreateBinaryNew1(blob_id) => {
            create_file(op_seq, NEW1_PATH, node(NEW1_NODE), *blob_id, MODE_REGULAR)
        }
        OpChoice::CreateTextNew2(text) => create_file(
            op_seq,
            NEW2_PATH,
            node(NEW2_NODE),
            text_span::text_blob_id(text).expect("text blob id"),
            MODE_REGULAR,
        ),
        OpChoice::CreateBinaryNew2(blob_id) => {
            create_file(op_seq, NEW2_PATH, node(NEW2_NODE), *blob_id, MODE_REGULAR)
        }
        OpChoice::CreateSymlinkNew1 => create_symlink(op_seq, NEW1_PATH, node(NEW1_NODE), "target"),
        OpChoice::CreateSymlinkNew2 => create_symlink(op_seq, NEW2_PATH, node(NEW2_NODE), "target"),
        OpChoice::ChangePermNew1 => {
            change_perm(op_seq, node(NEW1_NODE), MODE_REGULAR, MODE_EXECUTABLE)
        }
        OpChoice::ChangePermNew2 => {
            change_perm(op_seq, node(NEW2_NODE), MODE_REGULAR, MODE_EXECUTABLE)
        }
    })
}

/// Extend `evidence` with whatever blob a choice newly introduces, so a candidate `CreateFile`'s
/// or `ReplaceBinary`'s own blob is never `MissingCandidateEvidence` purely as an artifact of this
/// harness rather than a real evidence gap.
fn register_choice_evidence(evidence: TestTextResolver, choice: &OpChoice) -> TestTextResolver {
    match choice {
        OpChoice::ReplaceBinaryC(blob_id)
        | OpChoice::CreateBinaryNew1(blob_id)
        | OpChoice::CreateBinaryNew2(blob_id) => {
            evidence.with_blob(*blob_id, BlobKind::Binary, b"generated-binary".to_vec())
        }
        OpChoice::CreateTextNew1(text) | OpChoice::CreateTextNew2(text) => {
            let blob_id = text_span::text_blob_id(text).expect("text blob id");
            evidence.with_blob(blob_id, BlobKind::Text, text.clone())
        }
        _ => evidence,
    }
}

// ---------------------------------------------------------------------------------------------
// Bucket labeling -- one function, so Property A's own report and its assertions read the same
// names, and so a new `PairClass`/`UnknownReason` arm cannot silently fall into an unnamed bucket.
// ---------------------------------------------------------------------------------------------

fn pair_class_bucket(pair_class: &PairClass) -> String {
    match pair_class {
        PairClass::OrderedDependency { .. } => "ordered-dependency".to_string(),
        PairClass::Conflict { witness } => format!("conflict:{}", witness.kind.label()),
        PairClass::Independent | PairClass::Unknown { .. } => {
            unreachable!("commute_pair never returns DoesNotCommute for these classes")
        }
    }
}

fn unknown_reason_bucket(reason: UnknownReason) -> &'static str {
    match reason {
        UnknownReason::MalformedOperation => "unknown:malformed-operation",
        UnknownReason::SameNodeTextCommutationDeferred => "unknown:same-node-text-deferred",
        UnknownReason::RenameDeferred => "unknown:rename-deferred",
        UnknownReason::SymlinkDeferred => "unknown:symlink-deferred",
        #[cfg(test)]
        UnknownReason::FuturePreconditionDeferred => "unknown:future-precondition-deferred",
        UnknownReason::MissingCandidateEvidence => "unknown:missing-candidate-evidence",
        UnknownReason::SequenceInternalDependencyDeferred => "unknown:sequence-internal-dependency",
        UnknownReason::UnknownRelation => "unknown:unknown-relation",
    }
}

/// Buckets whose members are *deliberate* conservatism, named with the reason (handoff §2): the
/// oracle either cannot evaluate the pair at all (`RenameDeferred`/`SymlinkDeferred` -- the oracle
/// refuses to replay these kinds unconditionally) or genuinely may find the two orders equal
/// (`same-node-text-deferred` -- disjoint-span same-node edits are a real, common case where two
/// orders produce identical text). A hit landing anywhere else is the finding this increment exists
/// to surface, not something to allowlist away.
const EXPECTED_HIT_BUCKETS: &[&str] = &[
    "unknown:rename-deferred",
    "unknown:symlink-deferred",
    "unknown:same-node-text-deferred",
];

/// The three-way outcome of re-running the oracle on a pair the classifier already refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OracleVerdict {
    /// Both orders replayed and produced the same [`OracleState`] -- the classifier's refusal was
    /// unnecessary for this specific pair. The only outcome that can mean "over-refusal."
    AgreesEqual,
    /// Both orders replayed and produced different states -- the refusal is confirmed correct.
    ConfirmsDifferent,
    /// At least one order could not even be replayed (a deferred kind the oracle refuses
    /// unconditionally, or a genuine evidence/structural gap) -- there is nothing to compare.
    CannotEvaluate,
}

fn oracle_verdict(
    baseline: &NodeLifecycleState,
    evidence: &TestTextResolver,
    left: &DecodedPatchOperation,
    right: &DecodedPatchOperation,
) -> OracleVerdict {
    let scope = EvidenceScope::UnsealedCandidateOptional;
    let left_then_right = replay_operations(baseline, evidence, scope, [left, right]);
    let right_then_left = replay_operations(baseline, evidence, scope, [right, left]);
    match (left_then_right, right_then_left) {
        (Ok(a), Ok(b)) if a == b => OracleVerdict::AgreesEqual,
        (Ok(_), Ok(_)) => OracleVerdict::ConfirmsDifferent,
        _ => OracleVerdict::CannotEvaluate,
    }
}

// ---------------------------------------------------------------------------------------------
// Property A
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct SweepStats {
    generated: u32,
    discarded: u32,
    refused: u32,
    confirmed_different: BTreeMap<String, u32>,
    cannot_evaluate: BTreeMap<String, u32>,
    agrees_equal: BTreeMap<String, u32>,
}

impl SweepStats {
    fn record_refusal(&mut self, bucket: String, verdict: OracleVerdict) {
        self.refused += 1;
        let target = match verdict {
            OracleVerdict::ConfirmsDifferent => &mut self.confirmed_different,
            OracleVerdict::CannotEvaluate => &mut self.cannot_evaluate,
            OracleVerdict::AgreesEqual => &mut self.agrees_equal,
        };
        *target.entry(bucket).or_insert(0) += 1;
    }
}

/// Property A (handoff §2): for every pair the classifier refuses (`DoesNotCommute`/`Unknown`),
/// run the oracle anyway and bucket the result. Not a bare `assert!` -- a classified sweep, per
/// the handoff's own instruction, so a deliberate deferral failing the first generated case can
/// never make this test flaky-red for the wrong reason.
#[test]
fn property_a_classified_sweep_over_refused_pairs() {
    let cases = 4000;
    let mut runner = TestRunner::new(Config {
        cases,
        ..Config::default()
    });
    let stats = std::cell::RefCell::new(SweepStats::default());
    let strategy = (
        baseline_spec_strategy(),
        op_choice_strategy(),
        op_choice_strategy(),
    );
    runner
        .run(&strategy, |(spec, left_choice, right_choice)| {
            stats.borrow_mut().generated += 1;
            let Some(left) = build_operation(1, &left_choice, &spec) else {
                stats.borrow_mut().discarded += 1;
                return Ok(());
            };
            let Some(right) = build_operation(2, &right_choice, &spec) else {
                stats.borrow_mut().discarded += 1;
                return Ok(());
            };
            let state = build_state(&spec);
            let mut evidence = build_evidence(&spec);
            evidence = register_choice_evidence(evidence, &left_choice);
            evidence = register_choice_evidence(evidence, &right_choice);

            let result = commute_pair_result(
                &state,
                &evidence,
                EvidenceScope::UnsealedCandidateOptional,
                &left,
                &right,
            )
            .expect("evidence is fully registered for every generated candidate");

            match result {
                CommutationResult::Commutes { .. } => {}
                CommutationResult::DoesNotCommute { pair_class } => {
                    let bucket = pair_class_bucket(&pair_class);
                    let verdict = oracle_verdict(&state, &evidence, &left, &right);
                    stats.borrow_mut().record_refusal(bucket, verdict);
                }
                CommutationResult::Unknown { reason } => {
                    let bucket = unknown_reason_bucket(reason).to_string();
                    let verdict = oracle_verdict(&state, &evidence, &left, &right);
                    stats.borrow_mut().record_refusal(bucket, verdict);
                }
            }
            Ok(())
        })
        .expect("no generated case hard-fails; every finding is collected, not asserted per-case");

    let stats = stats.into_inner();
    let discard_rate = f64::from(stats.discarded) / f64::from(stats.generated) * 100.0;
    println!(
        "Property A: {} cases, {} discarded ({discard_rate:.2}%), {} refused",
        stats.generated, stats.discarded, stats.refused
    );
    println!(
        "  confirmed-different (refusal correct): {:?}",
        stats.confirmed_different
    );
    println!(
        "  cannot-evaluate (oracle could not run):  {:?}",
        stats.cannot_evaluate
    );
    println!(
        "  agrees-equal (oracle says states match): {:?}",
        stats.agrees_equal
    );

    for (bucket, count) in &stats.agrees_equal {
        assert!(
            EXPECTED_HIT_BUCKETS.contains(&bucket.as_str()),
            "unexpected over-refusal: bucket {bucket:?} had {count} pair(s) where the classifier \
             refused but the oracle found the two orders produce identical states -- this is a \
             genuine finding, not a harness gap; do not widen the allowlist to silence it"
        );
    }
    // The sweep is only evidence if it actually found something in at least the deliberately
    // reachable buckets -- an all-zero `agrees_equal` map would mean the generator never actually
    // reached the interesting disjoint-span-edit case, which would make this property decorative.
    assert!(
        stats
            .agrees_equal
            .contains_key("unknown:same-node-text-deferred"),
        "the sweep never found a same-node disjoint-span edit pair the oracle agrees on -- \
         strengthen generation rather than trust an assertion that never had a chance to fail"
    );
}

// ---------------------------------------------------------------------------------------------
// Property B
// ---------------------------------------------------------------------------------------------

/// The coarse target a choice touches -- used to keep one side's own sequence internally
/// conflict-free (handoff §4: `check_confluence` only classifies *cross* pairs between the two
/// sequences; it never checks two operations *within* one side against each other, because a real
/// sequence is a single author's own already-consistent local history, never an arbitrary bag of
/// possibly-colliding operations. Two `CreateFile`s at the same fresh path within one generated
/// sequence is exactly the shape that violates that precondition -- found empirically: it does not
/// fail as a discardable "does not apply" case, it fails deep inside `check_confluence`'s own
/// composed replay with an unrelated-looking `EvidenceError`, after the cross-pair phase has
/// already (correctly) found nothing wrong between the two sides). `EditTextAWord1`/`EditTextAWord2`
/// get their own distinct keys rather than sharing `text_a`'s, since editing two anchor-disjoint
/// words in one sequence is the one same-node combination this harness has confirmed is safe.
/// **All five of `EditTextAWord1`/`EditTextAWord2`/`ChangePermTextA`/`DeleteTextA`/`RenameTextA`
/// touch the same node (`TEXT_A_NODE`) and therefore share one key** -- an earlier draft of this
/// filter gave the two word-edits their own distinct keys so they could coexist in one sequence,
/// which correctly let `{EditTextAWord1, EditTextAWord2}` through but also *wrongly* let
/// `{EditTextAWord1, DeleteTextA}` through: found empirically (`property_b`'s own generator handed
/// `check_confluence` a sequence editing `text_a` then deleting it with a preimage referencing the
/// *original* blob, since every op here is built from the raw baseline, never from a prior op's own
/// output -- exactly the "not a real author's own already-consistent sequence" gap this filter
/// exists to close, per its own doc comment above). The one validated-safe exception is carved out
/// explicitly in `op_sequence_strategy` below, not by giving it a quietly-different key.
fn target_key(choice: &OpChoice) -> &'static str {
    match choice {
        OpChoice::EditTextAWord1(_)
        | OpChoice::EditTextAWord2(_)
        | OpChoice::ChangePermTextA
        | OpChoice::DeleteTextA
        | OpChoice::RenameTextA => "text-a-node",
        OpChoice::EditTextB(_) | OpChoice::ChangePermTextB | OpChoice::DeleteTextB => "text-b-node",
        OpChoice::ChangePermBinaryC
        | OpChoice::ReplaceBinaryC(_)
        | OpChoice::DeleteBinaryC
        | OpChoice::RenameBinaryC => "binary-c-node",
        OpChoice::DeleteSymlinkD => "symlink-d-node",
        OpChoice::CreateTextNew1(_)
        | OpChoice::CreateBinaryNew1(_)
        | OpChoice::CreateSymlinkNew1
        | OpChoice::ChangePermNew1 => "new1-slot",
        OpChoice::CreateTextNew2(_)
        | OpChoice::CreateBinaryNew2(_)
        | OpChoice::CreateSymlinkNew2
        | OpChoice::ChangePermNew2 => "new2-slot",
    }
}

/// The one same-node combination known safe to chain: two edits to `text_a`'s own anchor-disjoint
/// words, in either order -- confirmed by
/// `oracle_verdict_finds_a_genuinely_separated_same_node_edit_pair_to_be_deliberate_not_a_bug`.
fn is_the_validated_word_edit_chain(choices: &[OpChoice]) -> bool {
    matches!(
        choices,
        [OpChoice::EditTextAWord1(_), OpChoice::EditTextAWord2(_)]
            | [OpChoice::EditTextAWord2(_), OpChoice::EditTextAWord1(_)]
    )
}

fn op_sequence_strategy() -> impl Strategy<Value = Vec<OpChoice>> {
    proptest::collection::vec(op_choice_strategy(), 1..=2).prop_filter(
        "one side's own sequence must not touch the same target twice, except the validated \
         two-word-edit chain",
        |choices| {
            if is_the_validated_word_edit_chain(choices) {
                return true;
            }
            let mut keys: Vec<&'static str> = choices.iter().map(target_key).collect();
            let before = keys.len();
            keys.sort_unstable();
            keys.dedup();
            keys.len() == before
        },
    )
}

fn build_sequence(choices: &[OpChoice], spec: &BaselineSpec) -> Option<Vec<DecodedPatchOperation>> {
    choices
        .iter()
        .enumerate()
        .map(|(index, choice)| {
            let op_seq = u32::try_from(index + 1).expect("small sequence");
            build_operation(op_seq, choice, spec)
        })
        .collect()
}

/// `check_confluence`'s composed replay can fail even after every cross pair between the two
/// sequences has already been proven to commute (`property-b-evidence-error-handoff-v1.md`):
/// Property B found a case where `left`'s own two operations were authored against a shared
/// baseline rather than each other's result, violating the RFC 134 §7.4 item 1 sequencing
/// invariant (`text_span.rs`'s own module doc states it; `commutation.rs::replay_sequence_order`'s
/// own doc explains the refusal). **Ruled** (RFC 134, 2026-09-04): this generator still builds
/// v1-shaped operations that can produce exactly this shared-baseline shape; the allowlist exists
/// to keep the finding visible (any *other* reason still hard-fails the sweep, exactly as before)
/// without asserting how many times it occurs, since the failing seed is random per run and a
/// pinned count would make this test flaky for the wrong reason the moment the seed changes. It
/// retires only once the generator moves to v2 and the case passes for the right reason -- not
/// because this wording changed.
const ALLOWLISTED_EVIDENCE_ERROR_REASONS: &[&str] =
    &["sequence operations do not compose against a shared baseline"];

/// Bucket an `EvidenceError` by its own `reason` string where it has one (`Malformed`/
/// `Unreadable`); the other three variants carry no free-text reason at all, so their own variant
/// name is the bucket -- and none of those three is ever allowlisted, since they indicate a gap in
/// this harness's own evidence registration, not the composed-replay finding this exists to track.
fn evidence_error_bucket(error: &EvidenceError) -> String {
    match error {
        EvidenceError::Malformed { reason, .. } | EvidenceError::Unreadable { reason, .. } => {
            reason.clone()
        }
        EvidenceError::Missing { .. } => "missing".to_string(),
        EvidenceError::WrongObjectType { .. } => "wrong-object-type".to_string(),
        EvidenceError::WrongBlobKind { .. } => "wrong-blob-kind".to_string(),
    }
}

/// Property B (handoff §3): generate two operation *sequences* from the same baseline and look for
/// the composition failure `check_confluence`'s own `FinalStateInequality` witness exists to catch
/// -- every cross pair between the two sequences commutes, and the two full replay orders still
/// disagree. Unlike Property A, a hit here would be a correctness finding, not an availability one.
///
/// `source_file: Some(file!())` (`property-b-evidence-error-handoff-v1.md` §2.1): this test builds
/// its own `TestRunner` rather than using the `proptest!` macro, so proptest's default
/// `FileFailurePersistence::SourceParallel` has no source path to resolve a regression file from
/// unless one is supplied explicitly here -- without it, a failing case is never persisted, and
/// every reproduction pays the full case count again. With it, a failing seed lands under
/// `crates/prikk-store/proptest-regressions/patch_algebra/tests/algebra_properties.txt` and replays
/// first, deterministically, on every subsequent run.
#[test]
fn property_b_composition_can_disagree_even_when_every_pairwise_check_agrees() {
    let cases = 4000;
    let mut runner = TestRunner::new(Config {
        cases,
        source_file: Some(file!()),
        ..Config::default()
    });
    let generated = std::cell::Cell::new(0u32);
    let discarded = std::cell::Cell::new(0u32);
    let reached_full_sequence_replay = std::cell::Cell::new(0u32);
    let confluent = std::cell::Cell::new(0u32);
    let final_state_inequality_hits = std::cell::Cell::new(0u32);
    let evidence_errors = std::cell::RefCell::new(BTreeMap::<String, u32>::new());
    let strategy = (
        baseline_spec_strategy(),
        op_sequence_strategy(),
        op_sequence_strategy(),
    );
    runner
        .run(&strategy, |(spec, left_choices, right_choices)| {
            generated.set(generated.get() + 1);
            let (Some(left), Some(right)) = (
                build_sequence(&left_choices, &spec),
                build_sequence(&right_choices, &spec),
            ) else {
                discarded.set(discarded.get() + 1);
                return Ok(());
            };
            let state = build_state(&spec);
            let mut evidence = build_evidence(&spec);
            for choice in left_choices.iter().chain(right_choices.iter()) {
                evidence = register_choice_evidence(evidence, choice);
            }

            // Collected by reason, not asserted per-case (§2.2): a hard `.expect()` here would
            // panic the very first time the composed-replay finding below is generated, which
            // contradicts this test's own stated design one paragraph down -- every finding is
            // collected into a bucket, and only an *unlisted* reason hard-fails the sweep.
            match check_confluence_result(
                &state,
                &evidence,
                EvidenceScope::UnsealedCandidateOptional,
                &left,
                &right,
            ) {
                Ok(result) => {
                    if let ConfluenceResult::NotConfluent { witness } = &result {
                        if witness.kind == ConfluenceWitnessKind::FinalStateInequality {
                            reached_full_sequence_replay
                                .set(reached_full_sequence_replay.get() + 1);
                            final_state_inequality_hits.set(final_state_inequality_hits.get() + 1);
                        }
                    }
                    if matches!(result, ConfluenceResult::Confluent { .. }) {
                        reached_full_sequence_replay.set(reached_full_sequence_replay.get() + 1);
                        confluent.set(confluent.get() + 1);
                    }
                }
                Err(error) => {
                    let bucket = evidence_error_bucket(&error);
                    *evidence_errors.borrow_mut().entry(bucket).or_insert(0) += 1;
                }
            }
            Ok(())
        })
        .expect("no generated case hard-fails; every finding is collected, not asserted per-case");

    let generated = generated.get();
    let discarded = discarded.get();
    let reached_full_sequence_replay = reached_full_sequence_replay.get();
    let confluent = confluent.get();
    let final_state_inequality_hits = final_state_inequality_hits.get();
    let evidence_errors = evidence_errors.into_inner();
    let discard_rate = f64::from(discarded) / f64::from(generated) * 100.0;
    println!(
        "Property B: {generated} cases, {discarded} discarded ({discard_rate:.2}%), \
         {reached_full_sequence_replay} reached full-order replay ({confluent} confluent, \
         {final_state_inequality_hits} FinalStateInequality)"
    );
    println!("  evidence errors by reason: {evidence_errors:?}");
    for (bucket, count) in &evidence_errors {
        assert!(
            ALLOWLISTED_EVIDENCE_ERROR_REASONS.contains(&bucket.as_str()),
            "unexpected evidence error while composing two sequences every cross pair already \
             proved commute: {bucket:?} ({count} case(s)) -- this is a new finding, not the one \
             `property-b-evidence-error-handoff-v1.md` allowlisted; do not widen the allowlist to \
             silence it"
        );
    }
    assert_eq!(
        final_state_inequality_hits, 0,
        "found a sequence pair where every cross pair commutes but the two full replay orders \
         disagree -- this is a correctness finding about the pairwise theory's completeness, not \
         a harness bug; do not suppress it"
    );
    assert!(
        confluent > 0,
        "the sweep never reached a genuinely confluent sequence pair -- strengthen generation \
         rather than trust an assertion that never had a chance to fail"
    );
}

// ---------------------------------------------------------------------------------------------
// Control 4: a committed, deterministic proof that the sweep machinery actually detects a known
// deliberate deferral -- complementing (not replacing) the scratch-tree perturbation control
// described in the implementation report, which shows the *test* failing when the *algebra*
// regresses. This shows the bucketing/verdict logic itself is not a no-op on a case known, by the
// existing hand-written `same_node_text_pair_never_commutes` test, to reach `Unknown`.
// ---------------------------------------------------------------------------------------------

/// **A finding surfaced while building this control, kept because the next reader needs it**:
/// `same_node_text_pair_never_commutes` (`tests/commutation.rs`) proves the classifier defers on
/// `"alpha beta gamma"` edited at `"beta"` and at `"gamma"` -- two words that *look* disjoint. They
/// are not, to the oracle: `TEXT_ANCHOR_WINDOW` is 64 bytes each side, `"alpha beta gamma"` is 17
/// bytes total, so both edits' anchor windows cover the *entire* string, and applying one word's
/// edit invalidates the other's stale anchor context. The oracle genuinely `CannotEvaluate` this
/// pair, in **either** order -- not `AgreesEqual`. The classifier's deferral is real conservatism
/// here (a human could tell these are independent; the anchor mechanism, at this size, cannot),
/// but it is not a case Property A's sweep can present as "the oracle says they agree," because the
/// oracle cannot complete a comparison at all. This is asserted here, not silently dropped, exactly
/// because building the *other* half of this test (the genuinely-separated case below) first
/// produced this as a real, reproducible surprise.
#[test]
fn adjacent_same_node_edits_cannot_be_evaluated_even_though_they_look_disjoint() {
    let mut baseline = NodeLifecycleState::new();
    let old = b"alpha beta gamma";
    seed_text(&mut baseline, node(1), "doc.txt", old, MODE_REGULAR);
    let evidence = TestTextResolver::new([(node(1), old.to_vec())]);
    let left = edit_text(1, node(1), old, b"alpha BETA gamma");
    let right = edit_text(2, node(1), old, b"alpha beta GAMMA");

    let result = commute_pair_result(
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    )
    .expect("commutation evidence");
    let CommutationResult::Unknown { reason } = result else {
        panic!("expected the classifier to defer, got {result:?}");
    };
    assert_eq!(reason, UnknownReason::SameNodeTextCommutationDeferred);

    assert_eq!(
        oracle_verdict(&baseline, &evidence, &left, &right),
        OracleVerdict::CannotEvaluate,
        "adjacent-word edits in a short string share anchor context; the oracle cannot complete \
         either replay order, so this is not an 'agrees-equal' case"
    );
}

/// The genuinely-separated case Property A's own generator relies on: two edits whose anchor
/// windows (64 bytes each side) provably do not overlap, built the same way
/// `build_text_a_content`/`op_choice_strategy` build every generated case. This is the control the
/// handoff's own item 4 asks for in committed form: proof that the sweep's bucketing machinery
/// actually reaches `AgreesEqual` for a known case, not only that it never crashes.
#[test]
fn oracle_verdict_finds_a_genuinely_separated_same_node_edit_pair_to_be_deliberate_not_a_bug() {
    let mut baseline = NodeLifecycleState::new();
    let old = build_text_a_content(b"aaa", b"bbb");
    seed_text(&mut baseline, node(1), "doc.txt", &old, MODE_REGULAR);
    let evidence = TestTextResolver::new([(node(1), old.clone())]);
    let new_word1 = build_text_a_content(b"ccc", b"bbb");
    let new_word2 = build_text_a_content(b"aaa", b"ddd");
    let left = edit_text(1, node(1), &old, &new_word1);
    let right = edit_text(2, node(1), &old, &new_word2);

    let result = commute_pair_result(
        &baseline,
        &evidence,
        EvidenceScope::SealedCandidateRequired,
        &left,
        &right,
    )
    .expect("commutation evidence");
    let CommutationResult::Unknown { reason } = result else {
        panic!("expected the classifier to defer, got {result:?}");
    };
    assert_eq!(reason, UnknownReason::SameNodeTextCommutationDeferred);

    let verdict = oracle_verdict(&baseline, &evidence, &left, &right);
    assert_eq!(
        verdict,
        OracleVerdict::AgreesEqual,
        "edits separated by more than the 64-byte anchor window on each side should replay to the \
         same text either order"
    );
    assert!(EXPECTED_HIT_BUCKETS.contains(&unknown_reason_bucket(reason)));
}
