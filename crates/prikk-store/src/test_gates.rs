//! RFC 131 §2.2a ruling (a): gates, evidence harnesses and shared test fixtures, grouped into one
//! directory because `lib.rs` already declared them as one contiguous `#[cfg(test)]` run and this
//! documents that fact rather than inventing one. None of these eight is production code -- see
//! each module's own doc for what it gates or provides. `rfc111_seal_simulation` is production
//! (despite the name family) and is not one of these eight.

#[cfg(test)]
pub(crate) mod dc55_identity_evidence;
#[cfg(test)]
pub(crate) mod format_stability_gate;
#[cfg(test)]
pub(crate) mod release_compatibility_gate;
#[cfg(test)]
pub(crate) mod rfc111_index_decode_cost_gate;
#[cfg(test)]
pub(crate) mod rfc111_seal_decode_cost_gate;
#[cfg(test)]
pub(crate) mod signature_contract_tests;
#[cfg(test)]
pub(crate) mod test_support;
#[cfg(test)]
pub(crate) mod trust_gated_operations_binding_gate;
