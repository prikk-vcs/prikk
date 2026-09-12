//! Repository operations, one layer above the store (RFC 149).
//!
//! **Operations above the store; nothing here is reachable from `prikk-store`.** That is the whole
//! claim of this crate, and it is a claim the compiler enforces rather than a convention anyone has
//! to remember: `prikk-store` does not depend on `prikk-operations`, so an operation calling back
//! down into the store is ordinary, and the store calling up into an operation does not build.
//!
//! What lives here is the set of modules the RFC 149 census found the store's core does not reach —
//! bundles, checkout, merge, rollback, sync negotiation, verification reporting, and the rest. What
//! stays in `prikk-store` is the core (`commit_boundary`, `lifecycle_cache`, `patch_replay`, `refs`,
//! `trust`) and the infrastructure beneath it (`foundation`, `path`, `lock`, `node`, `object_store`
//! and their neighbours).
//!
//! The store's side of the boundary is named in one place: `prikk-store`'s own
//! *operations-layer contract* block in its `lib.rs`. Every item this crate reaches inside the store
//! is listed there deliberately; an item that needs adding is a decision recorded in RFC 149, not a
//! `pub` applied where the compiler asked for one.
//!
//! **Empty today, by design.** RFC 149's increment 2 creates the crate and registers it everywhere a
//! workspace member must be registered; increment 3 moves the modules in, one family per commit, so
//! that each move is a diff a reviewer can read rather than one large rearrangement.
