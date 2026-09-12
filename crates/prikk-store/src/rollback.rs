//! Rollback: the read-or-stage steps of one user-facing verb.
//!
//! `preview` shows what an inverse would do and writes nothing; `draft` stages that inverse as a
//! Patch in an empty active WAL; `verify` checks a staged draft against the plan it claims to be.
//! None of them publishes — `seal` does that, afterwards — so the family is exactly the part of
//! rollback a user can run and then change their mind about.
//!
//! RFC 131 §6f, grouping increment 1. They were grouped because they are one role as well as one
//! name: each is a step of the same verb, and nothing outside the family depends on more than one of
//! them (`verify.rs` and `history.rs` reach `verify` alone).
//!
//! **This grouping was refused once, at RFC 131 §2.2a (`544cc6c2`, 2026-09-08), for introducing a
//! `verify <-> rollback` coupling cycle.** It is safe now because the graph changed, not because the
//! earlier measurement was wrong: node naming became *qualified* afterwards, so a group no longer
//! collapses its members into a single node whose edges are their union — which is what created that
//! cycle. Re-measured before this move and again after: `subtree_cycles` unchanged at 13, hub set
//! unchanged, gate green.

pub(crate) mod draft;
pub(crate) mod preview;
pub(crate) mod verify;
