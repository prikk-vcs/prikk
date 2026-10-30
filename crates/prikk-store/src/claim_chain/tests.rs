//! Placeholder -- real fixture-based tests for the chain walk live in `crate::extract::tests`
//! (through `prepare_extract_plan_for_claim`, the one production call site this round wires up) and
//! are added there, since building a two-or-more-link chain fixture needs the same hand-crafted
//! claim-writing helpers that module's own tests already define. This file exists only so
//! `mod tests;` in `claim_chain.rs` has somewhere to point once this crate's own lint gate requires
//! every `#[cfg(test)] mod tests;` to resolve to a real file.
