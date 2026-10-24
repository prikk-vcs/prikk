# prikk-test-support

Shared CLI end-to-end test harness for `crates/prikk-cli`'s own integration tests (0.55.0 G2 Part A,
DC-67). Not part of the shipped product, not published, and built only when `prikk`'s own tests are
built.

## Why this crate exists

Before 0.55.0, this was `crates/prikk-cli/tests/support/` (a `mod support;` textually included into
every one of `prikk`'s own integration-test files). Each of those files is its own compilation
unit, so the support code was recompiled once per test file. Moving it into its own crate compiles
it once; every test binary just links against the already-built `rlib`.

`crates/prikk-cli/tests/support/mod.rs` still exists, as a thin re-export of this crate, so every
existing `mod support;` declaration (and `merge_evidence_support/mod.rs`'s own
`#[path = "../support/mod.rs"]`) keeps resolving unchanged.

## `env!("CARGO_BIN_EXE_prikk")` does not work here

That macro only resolves at compile time, for the specific rustc invocation building an integration
test or benchmark that belongs to the same package as the `[[bin]]` target (or a crate with an
actual Cargo dependency edge to a package that has both a `[lib]` and that `[[bin]]`). `prikk`
(`crates/prikk-cli`) has no `[lib]` target, so nothing — including this crate — can form a
dependency edge to it, and this crate's own compilation is a separate rustc invocation from
`prikk`'s test/bench compilation regardless. `tools/corpus/tests/support/mod.rs` found the identical
constraint first, for the identical reason, and its fix is reused here: ask Cargo itself, via
`cargo build --message-format=json`, where it already built the `prikk` binary, rather than reading
a build-time environment variable this crate's own compilation never receives.
