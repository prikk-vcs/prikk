//! RFC 139 increments 1-2: the measurement-corpus profile format, planner, and executor.
//!
//! A profile ([`profile`]) is a small, human-readable TOML document describing a real history's
//! *shape* -- never its content, never its paths. [`extract`] derives one from already-captured
//! `git log`/`git ls-tree` text. [`plan()`] turns a profile into an **action manifest**: the ordered
//! sequence of filesystem operations a build would perform, computed without executing anything
//! (RFC 139 §5a.2 -- this is what makes the determinism test possible with no binary and no
//! repository). [`execute`] drives a real `prikk` binary to turn a manifest into a real, throwaway
//! repository, exactly the same CLI surface a user would drive (RFC 139 §7).
//!
//! This crate is a library with a thin binary over it precisely so the builder, `tools/benchmarks`'
//! criterion benches, and the `#[ignore]`d integration harnesses under `crates/prikk-cli/tests/` can
//! all depend on the same types rather than each parsing TOML or driving the CLI by hand.

pub mod execute;
pub mod extract;
pub mod plan;
pub mod profile;
pub mod rng;

pub use execute::{BinaryIdentity, ExecuteError};
pub use extract::{ExtractError, ExtractionContext, extract_profile};
pub use plan::{ActionManifest, PlanError, PlannedAction, PlannedCommit, plan};
pub use profile::{BuilderInputs, OperationKindMix, Profile, Provenance, SCHEMA_VERSION, Shape};
