//! Internal patch-algebra classifier foundation (DC-16).
//!
//! This module intentionally exposes no public API. It classifies pairs of already-decoded patch
//! operations against a replayed lifecycle baseline, records deterministic reasons/witnesses, and
//! keeps deferred operation families fail-closed as `Unknown` instead of assuming commutation.

mod classify;
mod commutation;
mod create;
mod delete;
mod evidence;
mod evidence_types;
mod facts;
mod fold;
mod preimage;
mod replay_oracle;
mod report;
mod text_pair;
mod text_preimage;
mod types;
mod witness;

pub(crate) use evidence::StorePatchAlgebraEvidence;
pub(crate) use evidence_types::EvidenceScope;
pub(crate) use report::{
    MergeEvidenceItem, MergeEvidenceOperationKind, MergeEvidenceOutcome, MergeEvidenceProofPhase,
    MergeEvidenceReasonCode, MergeEvidenceReport, MergeEvidenceScope, MergeEvidenceSide,
    analyze_merge_evidence,
};
pub(crate) use types::ConflictWitnessKind;

#[cfg(test)]
mod tests;
